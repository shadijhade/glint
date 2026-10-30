import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import type { Visit } from "./rank";
import { DEFAULT_SETTINGS, type FolderRule, type FolderView, type HistoryItem, type RemoteConnection, type SavedSearch, type Settings, useStore } from "./store";

/**
 * A settings file (Settings → About: Export and Import), to bring Glint's set-up to another PC. Never a
 * secret: connections' passwords and keys stay in Windows Credential Manager, clouds' sign-ins in
 * rclone's configuration.
 */
export interface SettingsFile {
    glint: "settings";
    version: 1;
    /** When, as an ISO date, and from which version of Glint. */
    exported: string;
    app?: string;
    settings?: Partial<Settings>;
    pinned?: string[];
    savedSearches?: SavedSearch[];
    remotes?: RemoteConnection[];
    rules?: FolderRule[];
    history?: HistoryItem[];
    visits?: Record<string, Visit>;
    folderViews?: Record<string, FolderView>;
}

export type Part = "settings" | "pinned" | "savedSearches" | "remotes" | "rules" | "history" | "visits" | "folderViews";

/** The parts, in order: what each is called, and what's to know about it. */
export const PARTS: { part: Part; label: string; note?: string }[] = [
    { part: "settings", label: "Settings", note: "every option here" },
    { part: "pinned", label: "Pinned folders" },
    { part: "savedSearches", label: "Saved searches" },
    { part: "remotes", label: "Remote connections", note: "without their passwords and keys; a cloud only by its name in rclone" },
    { part: "rules", label: "Folder rules", note: "they come turned off" },
    { part: "history", label: "Search history" },
    { part: "visits", label: "Folders Ctrl+K ranks" },
    { part: "folderViews", label: "Folders' own views", note: "their columns, sort and list or grid" },
];

/** How many of a part a file holds (settings: how many options Glint knows). */
export function countOf(file: SettingsFile, part: Part): number {
    if (part === "settings") return Object.keys(knownSettings(file.settings)).length;
    const value = file[part];
    if (!value) return 0;
    return Array.isArray(value) ? value.length : Object.keys(value).length;
}

/** Everything that's set up now, as a settings file. */
export async function settingsFile(): Promise<SettingsFile> {
    const s = useStore.getState();
    return {
        glint: "settings",
        version: 1,
        exported: new Date().toISOString(),
        app: await getVersion().catch(() => undefined),
        settings: s.settings,
        pinned: s.pinned,
        savedSearches: s.savedSearches,
        remotes: s.remotes,
        rules: s.rules,
        history: s.history,
        visits: s.visits,
        folderViews: s.folderViews,
    };
}

export const writeSettingsFile = async (path: string) => invoke("write_file_content", { path, content: JSON.stringify(await settingsFile(), null, 2) });

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** Reads a settings file: what's in it, or why it isn't one Glint can take. */
export async function readSettingsFile(path: string): Promise<SettingsFile> {
    const read = await invoke<{ text: string; truncated: boolean; binary: boolean }>("read_text_preview", { path, maxBytes: 4 << 20 });
    if (read.binary || read.truncated) throw new Error("That isn't a Glint settings file");
    let file: unknown;
    try {
        file = JSON.parse(read.text);
    } catch {
        throw new Error("That isn't a Glint settings file (it isn't JSON)");
    }
    if (!isObject(file) || file.glint !== "settings") throw new Error("That isn't a Glint settings file");
    if (typeof file.version !== "number" || file.version > 1) throw new Error("It's from a newer Glint: update this one first");
    return file as unknown as SettingsFile;
}

/** The options a file sets that Glint knows, each of the type its default has. */
function knownSettings(given: unknown): Partial<Settings> {
    if (!isObject(given)) return {};
    const known: Record<string, unknown> = {};
    for (const [key, fallback] of Object.entries(DEFAULT_SETTINGS)) {
        const value = given[key];
        if (value === undefined) continue;
        const fits = fallback === null || value === null || (Array.isArray(fallback) ? Array.isArray(value) : typeof value === typeof fallback);
        if (fits) known[key] = value;
    }
    return known as Partial<Settings>;
}

const list = <T>(value: unknown, keep: (item: Record<string, unknown>) => boolean): T[] => (Array.isArray(value) ? (value.filter((v) => isObject(v) && keep(v)) as T[]) : []);

/**
 * Takes `parts` of a settings file: each added to what's here (what's here kept where both have it), or
 * replacing it. Folder rules always come turned off: their folders may not be on this PC. What it did,
 * in a few words each.
 */
export function importSettings(file: SettingsFile, parts: Set<Part>, replace: boolean): string[] {
    const s = useStore.getState();
    const did: string[] = [];
    const patch: Partial<ReturnType<typeof useStore.getState>> = {};
    if (parts.has("settings")) {
        const known = knownSettings(file.settings);
        patch.settings = replace ? { ...DEFAULT_SETTINGS, ...known } : { ...s.settings, ...known };
        did.push("settings");
    }
    if (parts.has("pinned")) {
        const pins = Array.isArray(file.pinned) ? file.pinned.filter((p): p is string => typeof p === "string") : [];
        const lower = new Set((replace ? [] : s.pinned).map((p) => p.toLowerCase()));
        patch.pinned = [...(replace ? [] : s.pinned), ...pins.filter((p) => !lower.has(p.toLowerCase()))];
        did.push(`${pins.length} pinned`);
    }
    if (parts.has("savedSearches")) {
        const searches = list<SavedSearch>(file.savedSearches, (v) => typeof v.query === "string" && typeof v.folder === "string");
        const same = (a: SavedSearch, b: SavedSearch) => a.query === b.query && a.folder.toLowerCase() === b.folder.toLowerCase();
        const kept = replace ? [] : s.savedSearches;
        patch.savedSearches = [...kept, ...searches.filter((n) => !kept.some((o) => same(o, n)))];
        did.push(`${searches.length} saved searches`);
    }
    if (parts.has("remotes")) {
        const remotes = list<RemoteConnection>(file.remotes, (v) => typeof v.id === "string" && typeof v.name === "string" && typeof v.kind === "string");
        const kept = replace ? [] : s.remotes;
        patch.remotes = [...kept, ...remotes.filter((r) => !kept.some((k) => k.id === r.id))];
        did.push(`${remotes.length} connections`);
    }
    if (parts.has("rules")) {
        const rules = list<FolderRule>(file.rules, (v) => typeof v.id === "string" && typeof v.folder === "string" && typeof v.action === "string").map((r) => ({ ...r, on: false }));
        const kept = replace ? [] : s.rules;
        patch.rules = [...kept, ...rules.filter((r) => !kept.some((k) => k.id === r.id))];
        did.push(`${rules.length} rules, off`);
    }
    if (parts.has("history")) {
        const history = list<HistoryItem>(file.history, (v) => typeof v.query === "string" && typeof v.path === "string" && typeof v.timestamp === "number");
        const all = [...(replace ? [] : s.history), ...history].sort((a, b) => b.timestamp - a.timestamp);
        patch.history = all.filter((h, i) => all.findIndex((o) => o.query === h.query && o.path === h.path) === i).slice(0, Math.max(10, s.settings.maxHistoryItems || 0));
        did.push(`${history.length} searches in the history`);
    }
    if (parts.has("visits") && isObject(file.visits)) {
        const visits: Record<string, Visit> = replace ? {} : { ...s.visits };
        for (const [path, v] of Object.entries(file.visits)) {
            if (!isObject(v) || typeof v.count !== "number" || typeof v.last !== "number") continue;
            const here = visits[path];
            visits[path] = here ? { count: Math.max(here.count, v.count), last: Math.max(here.last, v.last) } : { count: v.count, last: v.last };
        }
        patch.visits = visits;
        did.push(`${Object.keys(file.visits).length} ranked folders`);
    }
    if (parts.has("folderViews") && isObject(file.folderViews)) {
        const views: Record<string, FolderView> = replace ? {} : { ...s.folderViews };
        for (const [path, v] of Object.entries(file.folderViews)) {
            if (isObject(v) && typeof v.at === "number" && !(path.toLowerCase() in (replace ? {} : s.folderViews))) views[path.toLowerCase()] = v as unknown as FolderView;
        }
        patch.folderViews = views;
        did.push(`${Object.keys(file.folderViews).length} folders' views`);
    }
    useStore.setState(patch);
    return did;
}
