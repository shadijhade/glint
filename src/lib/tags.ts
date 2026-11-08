import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { toast } from "sonner";

/**
 * Tags kept in the files themselves (src-tauri/src/tags.rs): read for the rows on screen, a batch at a
 * time, and kept here until a file's tags change. The color tags come first and show as dots; any other
 * tag shows with a color of its own, from its name.
 */
export const COLOR_TAGS = [
    { name: "Red", color: "#ef4444" },
    { name: "Orange", color: "#f97316" },
    { name: "Yellow", color: "#eab308" },
    { name: "Green", color: "#22c55e" },
    { name: "Blue", color: "#3b82f6" },
    { name: "Purple", color: "#a855f7" },
    { name: "Gray", color: "#9ca3af" },
] as const;

const OTHER_COLORS = ["#0ea5e9", "#14b8a6", "#ec4899", "#84cc16", "#f59e0b", "#6366f1", "#10b981", "#e11d48"];

/** A tag's color: a color tag's own, any other's from its name. */
export function tagColor(tag: string) {
    const named = COLOR_TAGS.find((t) => t.name.toLowerCase() === tag.toLowerCase());
    if (named) return named.color;
    let hash = 0;
    for (const ch of tag.toLowerCase()) hash = (hash * 31 + ch.charCodeAt(0)) | 0;
    return OTHER_COLORS[Math.abs(hash) % OTHER_COLORS.length];
}

interface TagsState {
    /** By lowercase path: the tags read (an empty list when none). */
    byPath: Record<string, string[]>;
    /** Tags used lately here, most recent first (the sidebar offers them when instant search can't list them all). */
    recent: string[];
}

const RECENT_KEY = "glint-recent-tags";
const savedRecent = (): string[] => {
    try {
        return JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
    } catch {
        return [];
    }
};

export const useTags = create<TagsState>(() => ({ byPath: {}, recent: savedRecent() }));

let pending = new Set<string>();
let queued = false;

/** Asks for `path`'s tags with the others asked for in the same moment (one call for the rows on screen). */
export function wantTags(path: string) {
    const key = path.toLowerCase();
    if (key in useTags.getState().byPath || pending.has(path)) return;
    pending.add(path);
    if (queued) return;
    queued = true;
    setTimeout(() => {
        const paths = [...pending];
        pending = new Set();
        queued = false;
        invoke<string[][]>("tags_read", { paths }).then(
            (tags) => useTags.setState((s) => ({ byPath: { ...s.byPath, ...Object.fromEntries(paths.map((p, i) => [p.toLowerCase(), tags[i] ?? []])) } })),
            () => {}
        );
    }, 0);
}

/** Forgets what was read, so the rows ask again (after a refresh). */
export function forgetTags(paths?: string[]) {
    if (!paths) return useTags.setState({ byPath: {} });
    useTags.setState((s) => {
        const byPath = { ...s.byPath };
        for (const p of paths) delete byPath[p.toLowerCase()];
        return { byPath };
    });
}

/** Adds and removes tags on `paths`; what couldn't be tagged is said. */
export async function setTags(paths: string[], add: string[], remove: string[] = []) {
    try {
        const failed = await invoke<[string, string][]>("tags_set", { paths, add, remove });
        forgetTags(paths);
        paths.forEach(wantTags);
        if (add.length) {
            const recent = [...add, ...useTags.getState().recent.filter((t) => !add.some((a) => a.toLowerCase() === t.toLowerCase()))].slice(0, 20);
            useTags.setState({ recent });
            try {
                localStorage.setItem(RECENT_KEY, JSON.stringify(recent));
            } catch {
                // Kept for this session.
            }
        }
        if (failed.length) toast.error(failed.length === 1 ? `Couldn't tag “${failed[0][0].split("\\").pop()}”` : `Couldn't tag ${failed.length} items`, { description: failed[0][1] });
    } catch (e) {
        toast.error("Couldn't change the tags", { description: String(e) });
    }
}

/** Every tag on the drives instant search covers, with how many items have it (none when it's off). */
export const allTags = () => invoke<[string, number][]>("tags_all").catch((): [string, number][] => []);

/** Edit tags…: which items, until the dialog is closed. */
export const useTagsEdit = create<{ paths: string[] | null }>(() => ({ paths: null }));
