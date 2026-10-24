import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";
import { toast } from "sonner";
import { revealInApp } from "./explorer";
import { lanReach, type LanReach, lanShareStart, type LanOptions, receiveStart, sendStart, shareStop } from "./fs";
import { baseName, samePath } from "./paths";

/** A file or folder offered to another PC with sendme (share.rs), from Glint's start to its Stop. */
export interface Sending {
    key: number;
    path: string;
    /** Its run's id, once sendme has started. */
    id: number | null;
    /** What receives it, once sendme has read it through; and as a QR code (an SVG). */
    ticket?: string;
    size?: string;
    qr?: string;
    /** Why it ended by itself. */
    ended?: string;
}

/** What's being received, from a ticket into a folder. */
export interface Receiving {
    key: number;
    id: number | null;
    folder: string;
    files?: number;
    size?: string;
    /** Its size in bytes, and how much is here. */
    total?: number;
    bytes: number;
    /** Where it went. */
    done?: string;
    failed?: string;
}

/** A folder or a file shared on this network with dufs (lan.rs): its link once dufs is listening. */
export interface Serving {
    key: number;
    path: string;
    id: number | null;
    uploads: boolean;
    url?: string;
    qr?: string;
    /** The address it's served at, once it's listening. */
    address?: string;
    /** What may keep other devices from it, as Windows has it: read once it's listening, and again when Glint's back in front. */
    reach?: LanReach;
    /** When it ends by itself (ms since 1970). */
    ends?: number | null;
    /** What was asked for: how many requests, from which addresses, and the last. */
    visits: number;
    from: string[];
    last?: { from: string; request: string; status: number; at: number };
    ended?: string;
}

export type ShareView = "send" | "receive" | "network";

interface ShareState {
    /** What the dialog shows, while it's open. */
    open: ShareView | null;
    sends: Sending[];
    receive: Receiving | null;
    /** Where Receive puts what comes. */
    into: string;
    serving: Serving[];
    /** What Share on this network was opened on, to be shared. */
    offer: string;
}

export const useShare = create<ShareState>(() => ({ open: null, sends: [], receive: null, into: "", serving: [], offer: "" }));
const set = useShare.setState;
const get = useShare.getState;
let next = 0;

export const closeShare = () => set({ open: null });
export const showShare = (open: ShareView) => set({ open });

/** How many sends and shares are on, and which the title bar opens: it says so while there are. */
export const useSharing = () =>
    useShare(
        useShallow((s) => {
            const sends = s.sends.filter((x) => !x.ended).length;
            const serving = s.serving.filter((x) => !x.ended).length;
            return { count: sends + serving, view: (sends ? "send" : "network") as ShareView };
        })
    );

const patchSend = (key: number, patch: Partial<Sending>) => set((s) => ({ sends: s.sends.map((x) => (x.key === key ? { ...x, ...patch } : x)) }));

/** Offers `path` to another PC: the dialog opens on it, its ticket showing once sendme has read it. */
export async function sendToPC(path: string) {
    set({ open: "send" });
    if (get().sends.some((s) => samePath(s.path, path) && !s.ended)) return;
    const key = ++next;
    set((s) => ({ sends: [...s.sends, { key, path, id: null }] }));
    try {
        const id = await sendStart(path, (e) => patchSend(key, e.kind === "ready" ? { ticket: e.ticket, size: e.size, qr: e.qr } : { ended: e.message }));
        // Stopped while it started.
        if (!get().sends.some((s) => s.key === key)) return void shareStop(id);
        patchSend(key, { id });
    } catch (e) {
        patchSend(key, { ended: String(e) });
    }
}

/** Stops a send (its ticket stops working), or takes away one that ended. */
export function stopSend(key: number) {
    const send = get().sends.find((s) => s.key === key);
    if (send?.id != null && !send.ended) void shareStop(send.id);
    set((s) => ({ sends: s.sends.filter((x) => x.key !== key) }));
}

/** Opens Receive, into `folder` (unless something's coming already). */
export function openReceive(folder: string) {
    const running = get().receive;
    set({ open: "receive", ...(running && !running.done && !running.failed ? {} : { into: folder, receive: null }) });
}

/** Receives what `ticket` offers into the folder Receive was opened on. */
export async function receive(ticket: string) {
    const key = ++next;
    const folder = get().into;
    const patch = (change: Partial<Receiving>) => set((s) => (s.receive?.key === key ? { receive: { ...s.receive, ...change } } : {}));
    set({ receive: { key, id: null, folder, bytes: 0 } });
    try {
        const id = await receiveStart(folder, ticket, (e) => {
            if (e.kind === "getting") patch({ files: e.files, size: e.size, total: e.bytes });
            else if (e.kind === "progress") patch({ bytes: e.bytes });
            else if (e.kind === "failed") patch({ failed: e.message });
            else {
                patch({ done: e.path });
                if (get().open !== "receive") toast.success(`Received “${baseName(e.path)}”`, { action: { label: "Show", onClick: () => void revealInApp(e.path) } });
            }
        });
        if (get().receive?.key !== key) return void shareStop(id);
        patch({ id });
    } catch (e) {
        patch({ failed: String(e) });
    }
}

/** Stops what's coming (what came so far goes), or clears what's done for the next. */
export function stopReceive() {
    const r = get().receive;
    if (r?.id != null && !r.done && !r.failed) void shareStop(r.id);
    set({ receive: null });
}

/** Opens Share on this network on `path`, to share it from there. */
export const openNetworkShare = (path: string) => set({ open: "network", offer: path });

const patchServing = (key: number, patch: (s: Serving) => Partial<Serving>) => set((s) => ({ serving: s.serving.map((x) => (x.key === key ? { ...x, ...patch(x) } : x)) }));

/** Shares `path` on this network as `options` say, its link showing once dufs is listening. */
export async function serve(path: string, options: LanOptions) {
    set({ offer: "" });
    if (get().serving.some((s) => samePath(s.path, path) && !s.ended)) return;
    const key = ++next;
    set((s) => ({ serving: [...s.serving, { key, path, id: null, uploads: options.uploads, visits: 0, from: [] }] }));
    try {
        const id = await lanShareStart(path, options, (e) => {
            patchServing(key, (now) =>
                e.kind === "ready"
                    ? { url: e.url, qr: e.qr, ends: e.ends, address: new URL(e.url).hostname }
                    : e.kind === "visit"
                      ? { visits: now.visits + 1, from: now.from.includes(e.from) ? now.from : [...now.from, e.from], last: { from: e.from, request: e.request, status: e.status, at: Date.now() } }
                      : { ended: e.message }
            );
            if (e.kind === "ready") void checkReach(key);
        });
        if (!get().serving.some((s) => s.key === key)) return void shareStop(id);
        patchServing(key, () => ({ id }));
    } catch (e) {
        patchServing(key, () => ({ ended: String(e) }));
    }
}

/** Reads what may keep other devices from a share, as Windows has it now (its prompt answered, Settings changed). */
async function checkReach(key: number) {
    const address = get().serving.find((s) => s.key === key && !s.ended)?.address;
    if (!address) return;
    const reach = await lanReach(address).catch(() => undefined);
    if (reach) patchServing(key, () => ({ reach }));
}

/** When Glint's back in front: what may keep other devices from each share, read again. */
export const recheckShares = () => get().serving.forEach((s) => void checkReach(s.key));

/** Stops sharing it on the network (its link stops working), or takes away one that ended. */
export function stopServing(key: number) {
    const one = get().serving.find((s) => s.key === key);
    if (one?.id != null && !one.ended) void shareStop(one.id);
    set((s) => ({ serving: s.serving.filter((x) => x.key !== key) }));
}
