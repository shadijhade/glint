import { useLayoutEffect, useReducer } from "react";
import { create } from "zustand";
import { type FileEntry, isOnlineOnly } from "./entries";
import { cancelThumbnails, thumbnail } from "./fs";
import { decodeThumb, type ThumbAnswer } from "./grid";

// Thumbnails for the tiles and rows on screen. Each one asks for its item's thumbnail as it mounts and
// lets go as it unmounts; once per task the asks and let-gos are settled together: new ones go to
// Rust (thumbs.rs), and requests nobody waits for any more (scrolled away, another folder, another
// tab) are dropped there in one call. What comes back stays on hand as object URLs, within a budget.

/** A thumbnail as the UI keeps it: an object URL for its BMP, or null for a file without one. */
interface Held {
    url: string | null;
    bytes: number;
}

/** Thumbnails on hand, least recently shown first. */
const held = new Map<string, Held>();
let heldBytes = 0;
/** About a thousand tiles' worth; thumbnails on screen are always kept. */
const HELD_BUDGET = 48 << 20;
/** How many tiles and rows show each thumbnail right now. */
const showing = new Map<string, number>();

interface Request {
    key: string;
    entry: FileEntry;
    size: number;
    /** The tab it's asked in (a tile in another pane can wait for it too), and its number once sent. */
    lane: string;
    seq: number;
    /** Who waits for it: each tile's re-render, and the tab it's in. */
    waiting: Map<() => void, string>;
    /** The burst it was timed in. */
    run?: ThumbRun;
}

/** Thumbnails waited for, by key: asked for, or about to be. */
const wanted = new Map<string, Request>();
let seq = 0;

/** A file as it is now, at a size: a file that changes (or is downloaded, from online-only) gets a new thumbnail. */
const thumbKey = (entry: FileEntry, size: number) => `${size}:${entry.modified}:${entry.size}:${isOnlineOnly(entry) ? "cloud:" : ""}${entry.path.toLowerCase()}`;

function hold(key: string, url: string | null, bytes: number) {
    // A file without a thumbnail costs a little too, so a folder of them can't grow this forever.
    const cost = bytes || 64;
    held.set(key, { url, bytes: cost });
    heldBytes += cost;
    for (const [old, thumb] of held) {
        if (heldBytes <= HELD_BUDGET) break;
        if (showing.has(old)) continue;
        held.delete(old);
        heldBytes -= thumb.bytes;
        if (thumb.url) URL.revokeObjectURL(thumb.url);
    }
}

/** A tile showing `key` from now on; `update` re-renders it once the thumbnail is in. Returns the let-go. */
function watch(key: string, entry: FileEntry, size: number, lane: string, update: () => void) {
    showing.set(key, (showing.get(key) ?? 0) + 1);
    const have = held.get(key);
    let request: Request | undefined;
    if (have) {
        held.delete(key);
        held.set(key, have);
    } else {
        request = wanted.get(key);
        if (!request) wanted.set(key, (request = { key, entry, size, lane, seq: 0, waiting: new Map() }));
        request.waiting.set(update, lane);
        schedule();
    }
    return () => {
        const tiles = (showing.get(key) ?? 1) - 1;
        if (tiles) showing.set(key, tiles);
        else showing.delete(key);
        if (request?.waiting.delete(update) && !request.waiting.size) schedule();
    };
}

let scheduled = false;
function schedule() {
    if (scheduled) return;
    scheduled = true;
    queueMicrotask(settleWants);
}

/** Drops what nobody waits for any more, then asks for what's new. */
function settleWants() {
    scheduled = false;
    // Only requests made up to now are dropped, even if Rust gets this after the ones that follow.
    const upTo = seq;
    const dropped = new Map<string, string[]>();
    for (const request of [...wanted.values()]) {
        if (request.waiting.size) continue;
        wanted.delete(request.key);
        if (request.seq) dropped.set(request.lane, [...(dropped.get(request.lane) ?? []), request.entry.path]);
    }
    for (const [lane, paths] of dropped) cancelThumbnails(lane, paths, upTo).catch(() => {});
    for (const request of wanted.values()) if (!request.seq) send(request);
}

function send(request: Request) {
    const { entry } = request;
    request.seq = ++seq;
    request.lane = request.waiting.values().next().value ?? request.lane;
    request.run = runs.get(request.lane);
    if (request.run) {
        if (!request.run.pending) startBurst(request.run);
        request.run.pending++;
        publish();
    }
    thumbnail({ path: entry.path, size: request.size, modified: entry.modified, bytes: entry.size, cachedOnly: isOnlineOnly(entry), lane: request.lane, seq: request.seq }).then(
        (buffer) => answered(request, decodeThumb(buffer)),
        (e) => failed(request, String(e))
    );
}

function answered(request: Request, answer: ThumbAnswer) {
    count(request, answer);
    const { key } = request;
    // A file with no thumbnail yet (online-only, or its handler timed out) is asked again next time it's shown.
    if (!held.has(key) && (answer.bmp || answer.lasting)) hold(key, answer.bmp ? URL.createObjectURL(new Blob([answer.bmp], { type: "image/bmp" })) : null, answer.bmp?.byteLength ?? 0);
    // Tiles that asked again meanwhile (a request of their own may still be out) have it now too.
    const current = wanted.get(key);
    wanted.delete(key);
    for (const r of new Set([request, current])) r?.waiting.forEach((_, update) => update());
}

function failed(request: Request, error: string) {
    count(request, null);
    if (wanted.get(request.key) !== request) return;
    if (error === "cancelled" && request.waiting.size) {
        // Dropped with the tab that asked (it closed or moved on) while a tile in another pane waits.
        request.seq = 0;
        schedule();
        return;
    }
    wanted.delete(request.key);
    if (error !== "cancelled") {
        hold(request.key, null, 0);
        request.waiting.forEach((_, update) => update());
    }
}

/**
 * The thumbnail of `entry` at `size` device pixels for a tile in tab `lane`, once it's in: an object
 * URL, or null (none yet, none at all, or not `enabled`: the tile shows the file's icon).
 */
export function useThumbnail(entry: FileEntry, size: number, lane: string, enabled = true): string | null {
    const key = enabled && !entry.dir ? thumbKey(entry, size) : "";
    const [, update] = useReducer((n: number) => n + 1, 0);
    const url = key ? (held.get(key)?.url ?? null) : null;
    // Asked for as the tile mounts, before the browser paints, so the request goes out at once.
    useLayoutEffect(() => {
        if (!key) return;
        const stop = watch(key, entry, size, lane, update);
        // It came in between the render and now.
        if ((held.get(key)?.url ?? null) !== url) update();
        return stop;
    }, [key, lane]);
    return url;
}

/** Drops everything a tab still waits for (it closed). */
export function dropThumbnails(lane: string) {
    runs.delete(lane);
    cancelThumbnails(lane, null, seq).catch(() => {});
    publish();
}

// Timing, for the status bar.

/** A tab's latest burst of thumbnails, from its first request to its last answer. */
export interface ThumbRun {
    /** The folder or search shown (see `viewKey`) and the view: another one's thumbnails don't count. */
    scope: string;
    /** performance.now() at the burst's first request. */
    started: number;
    pending: number;
    /** Thumbnails that came, and from where: this app's memory, Windows' cache, or made from the file. */
    found: number;
    memory: number;
    cache: number;
    made: number;
    /** Milliseconds the shell spent making them, all told. */
    madeMs: number;
    /** From the first request to the last answer, ms. */
    elapsed: number;
}

const runs = new Map<string, ThumbRun>();

/** Each tab's latest burst, by tab id. */
export const useThumbRuns = create<Record<string, ThumbRun>>(() => ({}));

let publishing = false;
function publish() {
    if (publishing) return;
    publishing = true;
    requestAnimationFrame(() => {
        publishing = false;
        useThumbRuns.setState(Object.fromEntries([...runs].map(([lane, run]) => [lane, { ...run }])), true);
    });
}

function startBurst(run: ThumbRun) {
    Object.assign(run, { started: performance.now(), pending: 0, found: 0, memory: 0, cache: 0, made: 0, madeMs: 0, elapsed: 0 });
}

/** A view (grid, or list with thumbnails) opens `scope` in tab `lane`: its thumbnails are timed from here. */
export function timeThumbnails(lane: string, scope: string) {
    if (runs.get(lane)?.scope === scope) return;
    const run = { scope } as ThumbRun;
    startBurst(run);
    runs.set(lane, run);
    publish();
}

function count(request: Request, answer: ThumbAnswer | null) {
    const run = request.run;
    if (!run || runs.get(request.lane) !== run) return;
    run.pending = Math.max(0, run.pending - 1);
    // Requests dropped (scrolled away) end the burst without counting.
    if (answer) {
        run.elapsed = performance.now() - run.started;
        if (answer.bmp) {
            run.found++;
            run[answer.source]++;
            if (answer.source === "made") run.madeMs += answer.ms;
        }
    }
    publish();
}
