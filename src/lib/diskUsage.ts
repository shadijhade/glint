import { create } from "zustand";
import type { FileEntry } from "./entries";
import { activeTabId, indexed, reload, setView, tabState, updateTab, useExplorer, type ViewMode } from "./explorer";
import { cancelDiskUsage, diskUsage, resolvePath } from "./fs";
import { isFolderPath, samePath, THIS_PC } from "./paths";
import { useStore } from "./store";
import { pruneUsage, subUsage, type UsageEngine, type UsageMap, usageEntries } from "./usage";

/** How many levels down a map goes, and the most items it shows (the biggest; the rest are grouped). */
const DEPTH = 6;
const ITEMS = 20_000;

/** A tab's disk-usage map, from the index or pdu. */
export interface UsageRun {
    id: number;
    /** The folder being mapped. */
    path: string;
    /** The latest map: this folder's once it arrives, until then the one shown before, if any. */
    map: UsageMap | null;
    running: boolean;
    /** Who answers (while running, who's expected to). */
    engine: UsageEngine;
    error: string;
    /** performance.now() at the click or key that asked for it. */
    started: number;
    /** From that click to the map on screen, ms; 0 until it's drawn. */
    shownIn: number;
    /** The map shown is this folder's part of an earlier map, while pdu measures it. */
    preview: boolean;
}

/** By tab id, the tabs that show their folder as a disk-usage map instead of a list. */
export const useUsage = create<Record<string, UsageRun>>(() => ({}));

const runs = useUsage.getState;

function save(tab: string, run: UsageRun | null) {
    const { [tab]: _, ...rest } = runs();
    useUsage.setState(run ? { ...rest, [tab]: run } : rest, true);
}

/** Whether a tab (the active one by default) shows its disk-usage map. */
export const showsUsage = (tab = activeTabId()) => !!runs()[tab];

/** When the last click or key happened: a map is timed from the input that led to it. */
let lastInput = 0;
for (const type of ["pointerdown", "keydown"]) window.addEventListener(type, () => (lastInput = performance.now()), true);
const inputTime = () => (performance.now() - lastInput < 10_000 ? lastInput : performance.now());

/** Shows a tab's folder as a disk-usage map (`on`), or as a list again. */
export function setUsage(tab: string, on: boolean) {
    if (!on || runs()[tab]) {
        if (!on && runs()[tab]) {
            stopUsage(tab);
            save(tab, null);
        }
        return;
    }
    const t = tabState(tab);
    const path = t?.path ?? THIS_PC;
    // A name being edited is the list's.
    if (t?.renaming) updateTab({ renaming: null }, tab);
    save(tab, { id: 0, path, map: null, running: false, engine: "pdu", error: "", started: 0, shownIn: 0, preview: false });
    if (isFolderPath(path)) mapUsage(tab, path);
}

/** The command bar's and the palette's Disk usage: map or list, for the active tab by default. */
export const toggleUsage = (tab = activeTabId()) => setUsage(tab, !runs()[tab]);

/** List or grid, chosen in the command bar, the palette or with a key: a tab showing its map shows its items again. */
export function chooseView(view: ViewMode, tab = activeTabId()) {
    setUsage(tab, false);
    setView(view, tab);
    // The folder keeps it, while each folder keeps its own view (Settings → Files).
    const t = tabState(tab);
    const { settings, setFolderView } = useStore.getState();
    if (settings.perFolderViews && t && !t.find && t.path !== THIS_PC) setFolderView(t.path, { view });
}

/**
 * Maps pdu measured lately, by folder: going back to one shows it again instead of measuring it
 * again (F5 does). Index maps are always asked for afresh; they take milliseconds.
 */
const measuredMaps = new Map<string, UsageMap>();

function remember(map: UsageMap) {
    const key = map.path.toLowerCase();
    measuredMaps.delete(key);
    measuredMaps.set(key, map);
    if (measuredMaps.size > 8) measuredMaps.delete(measuredMaps.keys().next().value!);
}

/** The folder's part of a map on hand, to show while pdu measures it. */
function partOfKnownMap(path: string, current: UsageMap | null): UsageMap | null {
    const wanted = path.toLowerCase();
    for (const map of [current, ...[...measuredMaps.values()].reverse()]) {
        const i = map?.paths.findIndex((p) => p.length === wanted.length && p.toLowerCase() === wanted) ?? -1;
        if (map && i > 0 && map.end[i] > map.first[i]) return subUsage(map, i);
    }
    return null;
}

let seq = 0;
/** The index service answered as a version from before maps: expect pdu until one comes from the index. */
let serviceTooOld = false;
const remapTimers = new Map<string, ReturnType<typeof setTimeout>>();

/**
 * Maps `path` in a tab that shows its map: from the index when it has the folder, else with pdu.
 * `started` is when the click or key that asked for it happened; `again` measures a folder pdu
 * measured before instead of showing that map.
 */
export async function mapUsage(tab: string, path: string, started = inputTime(), again = false) {
    clearTimeout(remapTimers.get(tab));
    const before = runs()[tab];
    if (!before || !isFolderPath(path)) return;
    const id = ++seq;
    // A folder in an archive is mapped from the archive's own list of entries, at once.
    const engine: UsageEngine = tabState(tab)?.listing?.archive?.format ?? (indexed(path) && !serviceTooOld ? "index" : "pdu");
    const run: UsageRun = { id, path, map: before.map, running: true, engine, error: "", started, shownIn: 0, preview: false };
    if (engine === "pdu") {
        const known = !again && measuredMaps.get(path.toLowerCase());
        if (known) return save(tab, { ...run, map: known, running: false });
        const part = partOfKnownMap(path, before.map);
        if (part) Object.assign(run, { map: part, preview: true });
    }
    save(tab, run);
    const update = (patch: Partial<UsageRun>) => {
        const now = runs()[tab];
        if (now?.id === id) save(tab, { ...now, ...patch });
    };
    try {
        // The index tells what grew too, for the Growth colors (it costs it a millisecond or two).
        const map = await diskUsage(path, DEPTH, ITEMS, tab, Date.now() - useStore.getState().settings.growthDays * 86_400_000);
        serviceTooOld = map.serviceTooOld;
        if (map.engine === "pdu") remember(map);
        update({ map, running: false, engine: map.engine, preview: false });
    } catch (e) {
        update({ running: false, error: String(e) === "cancelled" ? "" : String(e) });
    }
}

/** F5 on a map: reads the folder again and maps it again (measures it again when pdu drew it). */
export async function refreshUsage(tab = activeTabId()) {
    const started = inputTime();
    await reload(tab);
    const path = tabState(tab)?.path;
    if (path && isFolderPath(path)) mapUsage(tab, path, started, true);
}

/** Called once a map is drawn, to time it from the click; not for a part of an earlier map shown meanwhile. */
export function markUsageShown(tab: string, id: number) {
    const run = runs()[tab];
    if (run?.id === id && !run.running && !run.shownIn) save(tab, { ...run, shownIn: performance.now() - run.started });
}

/** Stops a tab's map that pdu is still measuring. */
export function stopUsage(tab = activeTabId()) {
    clearTimeout(remapTimers.get(tab));
    if (runs()[tab]?.running) cancelDiskUsage(tab);
}

/**
 * After the tab's folder was read again (it changed, or files were pasted or deleted), maps it again
 * from the index once the index has the change: it reads the change journal every 250 ms. A pdu map
 * stays as measured until F5.
 */
function remapSoon(tab: string) {
    clearTimeout(remapTimers.get(tab));
    remapTimers.set(
        tab,
        setTimeout(() => {
            const run = runs()[tab];
            const path = tabState(tab)?.path;
            if (run && !run.running && run.map && run.map.engine !== "pdu" && path && samePath(run.path, path)) mapUsage(tab, path, performance.now());
        }, 1000)
    );
}

/** Takes the items that were selected and are gone now (deleted, moved away) off the tab's maps at once. */
async function dropGone(tab: string, selected: ReadonlySet<string> | undefined) {
    const map = runs()[tab]?.map;
    if (!map || !selected?.size) return;
    const inMap = [...selected].filter((p) => map.paths.includes(p));
    const missing = await Promise.all(inMap.map((p) => resolvePath(p).then(() => null, () => p)));
    const gone = new Set(missing.filter((p): p is string => p !== null));
    if (!gone.size) return;
    for (const [key, known] of measuredMaps) measuredMaps.set(key, pruneUsage(known, gone));
    const run = runs()[tab];
    if (run?.map) save(tab, { ...run, map: pruneUsage(run.map, gone) });
}

// A map follows its tab: a folder opened is mapped (timed from the click), one read again is mapped
// again shortly, and a tab closed takes its map along.
useExplorer.subscribe((s, prev) => {
    for (const [tab, run] of Object.entries(runs())) {
        const now = s.tabs[tab];
        const before = prev.tabs[tab];
        if (!now) {
            stopUsage(tab);
            save(tab, null);
        } else if (now.listing !== before?.listing) {
            if (!now.listing) stopUsage(tab);
            else if (!samePath(now.path, run.path) || (!run.map && !run.running)) mapUsage(tab, now.path);
            else {
                dropGone(tab, before?.selection);
                remapSoon(tab);
            }
        }
    }
});

const entries = new WeakMap<UsageMap, FileEntry[]>();
const rows = new WeakMap<UsageMap, FileEntry[]>();
const NO_ROWS: FileEntry[] = [];

/** A map's items as file-list entries, by node (the folder mapped first), made once per map. */
export function entriesOf(map: UsageMap) {
    let list = entries.get(map);
    if (!list) entries.set(map, (list = usageEntries(map)));
    return list;
}

/**
 * The rows of a tab that shows its map: the map's items, so selection, keys, the item menu and Quick
 * Look work on them as on a list. Null when the tab shows its list (or This PC, or search results).
 */
export function useMapRows(tab: string): FileEntry[] | null {
    const run = useUsage((s) => s[tab]);
    const folder = useExplorer((s) => (s.tabs[tab] && !s.tabs[tab].find ? s.tabs[tab].path : THIS_PC));
    if (!run || !isFolderPath(folder)) return null;
    if (!run.map) return NO_ROWS;
    let list = rows.get(run.map);
    if (!list) rows.set(run.map, (list = entriesOf(run.map).slice(1)));
    return list;
}
