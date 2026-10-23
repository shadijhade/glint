import { useEffect, useState } from "react";
import { create } from "zustand";
import type { FileEntry } from "./entries";
import { mediaDetails, type MediaDetails, mediaDetailsMany } from "./fs";

const PICTURES = "jpg jpeg jpe jfif heic heif avif png tif tiff webp dng cr2 cr3 nef nrw arw srf sr2 raf orf rw2 pef iiq srw gif bmp ico jxl psd qoi".split(" ");
const VIDEOS = "mp4 m4v mov 3gp 3g2 mkv webm".split(" ");
const SONGS = "mp3 flac m4a m4b ogg oga opus wav aif aiff aifc ape wv mpc spx aac".split(" ");
const DATED = new Set([...PICTURES, ...VIDEOS]);
const MEDIA = new Set([...PICTURES, ...VIDEOS, ...SONGS]);

/** Photos, videos and songs: the files with details to read (media.rs). */
export const hasDetails = (entry: FileEntry) => !entry.dir && MEDIA.has(entry.ext);

/** Photos and videos: the files that can say when they were taken. */
export const canBeTaken = (entry: FileEntry) => !entry.dir && DATED.has(entry.ext);

const read = new Map<string, Promise<MediaDetails | null>>();

/** A photo's, video's or song's details (camera, exposure, place; artist, album, cover…), read once per version. */
export function useMediaDetails(entry: FileEntry, enabled = true) {
    const [details, setDetails] = useState<MediaDetails | null>(null);
    useEffect(() => {
        setDetails(null);
        if (!enabled || !hasDetails(entry)) return;
        const key = `${entry.path.toLowerCase()}|${entry.modified}`;
        let pending = read.get(key);
        if (!pending) {
            pending = mediaDetails(entry.path).catch(() => null);
            read.set(key, pending);
            // Covers are kept with them: the last 100 files read.
            if (read.size > 100) read.delete(read.keys().next().value!);
        }
        let stale = false;
        void pending.then((found) => !stale && setDetails(found));
        return () => {
            stale = true;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [entry.path, entry.modified, enabled]);
    return details;
}

/** Which version of a file what it says (`MediaFacts`) is for. */
export const takenKey = (entry: FileEntry) => `${entry.path.toLowerCase()}|${entry.modified}`;

/** What the Date taken sort and property filters (width:, taken:, artist:…) read of a photo, video or song. */
export interface MediaFacts {
    /** Milliseconds. */
    taken: number | null;
    width: number | null;
    height: number | null;
    /** Seconds. */
    duration: number | null;
    /** Lowercase. */
    camera: string | null;
    artist: string | null;
    album: string | null;
}

/** What's been read, by `takenKey`: null for a file that says nothing. */
export const useFacts = create<{ facts: ReadonlyMap<string, MediaFacts | null> }>(() => ({ facts: new Map() }));

const asked = new Set<string>();
const lower = (text: string | null | undefined) => text?.toLowerCase() ?? null;
const factsOf = (d: MediaDetails | null): MediaFacts | null =>
    d && { taken: d.taken ?? null, width: d.width ?? null, height: d.height ?? null, duration: d.duration ?? null, camera: lower(d.camera), artist: lower(d.artist), album: lower(d.album) };

/** Reads what these files say (photos, videos and songs; `which` of them), the ones not asked yet, 300 at a time. */
export async function readFacts(entries: readonly FileEntry[], which: (entry: FileEntry) => boolean = hasDetails) {
    const wanted = entries.filter((e) => which(e) && !asked.has(takenKey(e)));
    for (let i = 0; i < wanted.length; i += 300) {
        const part = wanted.slice(i, i + 300);
        for (const e of part) asked.add(takenKey(e));
        const found = await mediaDetailsMany(part.map((e) => e.path)).catch(() => part.map(() => null));
        useFacts.setState((s) => {
            const facts = new Map(s.facts);
            part.forEach((e, k) => facts.set(takenKey(e), factsOf(found[k])));
            return { facts };
        });
    }
}

/** Reads when these photos and videos were taken, for the Date taken sort. */
export const readTaken = (entries: readonly FileEntry[]) => readFacts(entries, canBeTaken);

/** A place on OpenStreetMap. */
export const mapLink = ([lat, lon]: [number, number]) => `https://www.openstreetmap.org/?mlat=${lat}&mlon=${lon}#map=16/${lat}/${lon}`;
