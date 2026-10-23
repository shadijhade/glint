import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { frecency, type Visit } from "./rank";
import { useStore } from "./store";

/**
 * Folder history shared with zoxide (Settings → History, zoxide.rs): its folders, by lowercase path, as
 * it has them and with its score (a frecency, as Glint's visits have). Empty while sharing is off.
 */
export const useZoxide = create<{ folders: ReadonlyMap<string, { path: string; score: number }> }>(() => ({ folders: new Map() }));

/** Reads zoxide's folders again, when history is shared with it. */
export async function refreshZoxide() {
    if (!useStore.getState().settings.zoxide) return useZoxide.setState({ folders: new Map() });
    const list = await invoke<[string, number][] | null>("zoxide_folders").catch(() => null);
    useZoxide.setState({ folders: new Map((list ?? []).map(([path, score]) => [path.toLowerCase(), { path, score }])) });
}

/** A folder opened in Glint goes to zoxide, when history is shared with it, so `z` finds it in a terminal. */
export function shareVisit(path: string) {
    if (useStore.getState().settings.zoxide) void invoke("zoxide_add", { path }).catch(() => {});
}

/** How much a folder is used: its visits here (frecency) and its zoxide score, together. */
export function usedScore(path: string, visits: Readonly<Record<string, Visit>>, now = Date.now()) {
    const visit = visits[path];
    return (visit ? frecency(visit, now) : 0) + (useZoxide.getState().folders.get(path.toLowerCase())?.score ?? 0);
}

/** The folders used most, here and (shared) in terminals, best first. */
export function usedFolders(visits: Readonly<Record<string, Visit>>, limit: number, now = Date.now()): string[] {
    const all = new Map<string, string>();
    for (const path of Object.keys(visits)) all.set(path.toLowerCase(), path);
    for (const [key, { path }] of useZoxide.getState().folders) if (!all.has(key)) all.set(key, path);
    return [...all.values()]
        .map((path) => ({ path, score: usedScore(path, visits, now) }))
        .sort((a, b) => b.score - a.score)
        .slice(0, limit)
        .map((u) => u.path);
}
