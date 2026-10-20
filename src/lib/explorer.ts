import { createContext, useContext } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { openPath } from "@tauri-apps/plugin-opener";
import { audioDir, desktopDir, documentDir, downloadDir, homeDir, pictureDir, videoDir } from "@tauri-apps/api/path";
import { type FileEntry, type InArchive, isArchived, isArchiveFile, withDetails } from "./entries";
import {
    archiveCompress,
    archiveExtract,
    archiveExtractHere,
    archiveStage,
    archiveUnstage,
    cancelArchiveStage,
    cancelFind,
    cancelFolderSizes,
    clipboardClear,
    clipboardGetFiles,
    clipboardSetFiles,
    createEntry,
    type Drive,
    driveName,
    type FileOp,
    fileOp as performFileOp,
    findFiles,
    folderSizes,
    indexDisable,
    indexEnable,
    type IndexInfo,
    indexStatus,
    type LaunchTarget,
    type Progress,
    listDir,
    listDrives,
    type Listing,
    resolvePath,
    type SizeEngine,
    type Staged,
    startDrag,
    takeLaunchTargets,
    watchDirs,
} from "./fs";
import { zoomTiles } from "./grid";
import { baseName, crumbs, isRoot, joinPath, parentPath, samePath, THIS_PC } from "./paths";
import { DEFAULT_SETTINGS, type Session, useStore, type ViewMode } from "./store";
import { dropThumbnails } from "./thumbs";
import { firstNotice } from "./utils";

export type { ViewMode };

export interface KnownFolder {
    id: "home" | "desktop" | "documents" | "downloads" | "pictures" | "music" | "videos";
    name: string;
    path: string;
}

/** A name search, by the index or fd. */
export interface FindRun {
    id: number;
    query: string;
    /** Who answers ("index", "fd", "index + fd"); empty until known. */
    engine: string;
    /** Folder searched (with its subfolders); This PC searches every drive. */
    scope: string;
    entries: FileEntry[];
    running: boolean;
    elapsed: number;
    limited: boolean;
    error: string;
}

/** Subfolder sizes for one folder, from the index or dust. */
export interface SizeRun {
    sizes: Map<string, number>;
    total: number;
    elapsed: number;
    running: boolean;
    /** The index or dust, or the archive's format for a folder in one (its sizes come with its listing). */
    engine: SizeEngine;
    /** The tab measuring it, so Stop stops the right run. */
    lane: string;
}

/** One tab: a folder (or This PC, or search results) with its own history, selection and search. */
export interface TabState {
    id: string;
    /** False until its first folder has opened (that one doesn't go into Back history). */
    ready: boolean;
    path: string;
    listing: Listing | null;
    loading: boolean;
    /** From the click to the new list on screen, ms. */
    shownIn: number;
    back: string[];
    forward: string[];
    /** Text in the search box: filters what's shown; Enter searches subfolders by name. */
    filter: string;
    find: FindRun | null;
    selection: ReadonlySet<string>;
    anchor: string | null;
    focus: string | null;
    renaming: string | null;
    /** A list with columns, or a grid of thumbnails. */
    view: ViewMode;
}

/** One side of the window: its tabs in order, and the one on screen. */
export interface PaneState {
    tabs: string[];
    active: string;
}

interface ExplorerState {
    tabs: Record<string, TabState>;
    /** The left pane, and the right one once dual-pane view has been on (it keeps its tabs while hidden). */
    panes: PaneState[];
    /** The pane the keyboard, toolbar and address bar act on. */
    activePane: number;
    dual: boolean;
    /** Highlights unique, newer, older, and size-different items across the two dual panes. */
    comparePanes: boolean;
    /** When comparing panes, hides identical items so only differences remain on screen. */
    diffOnly: boolean;
    drives: Drive[];
    known: KnownFolder[];
    editingAddress: boolean;
    /** By folder path, shared by every tab that shows the folder. */
    sizes: Record<string, SizeRun>;
    /** Paths cut to the clipboard here, shown dimmed until pasted. */
    cut: ReadonlySet<string>;
    /** Folder that files dragged in from outside would land in. */
    dropTarget: string | null;
    /** Instant search's service, once asked. */
    index: IndexInfo | null;
    /** Space-bar preview of the active pane's focused item, over everything. */
    quickLook: boolean;
    palette: boolean;
}

let tabSeq = 0;
const newTabId = () => `tab${++tabSeq}`;

/** A tab that opens `path` when it's first shown, as `view`, with `select` selected. */
const blankTab = (id: string, path: string, view: ViewMode = "list", select?: string): TabState => ({
    id,
    ready: false,
    path,
    listing: null,
    loading: false,
    shownIn: 0,
    back: [],
    forward: [],
    filter: "",
    find: null,
    selection: new Set(select ? [select] : []),
    anchor: null,
    focus: null,
    renaming: null,
    view,
});

const firstTab = newTabId();

export const useExplorer = create<ExplorerState>(() => ({
    tabs: { [firstTab]: blankTab(firstTab, THIS_PC) },
    panes: [{ tabs: [firstTab], active: firstTab }],
    activePane: 0,
    dual: false,
    comparePanes: false,
    diffOnly: false,
    drives: [],
    known: [],
    editingAddress: false,
    sizes: {},
    cut: new Set(),
    dropTarget: null,
    index: null,
    quickLook: false,
    palette: false,
}));

const get = useExplorer.getState;
const set = useExplorer.setState;
const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

// Tabs

/** The panes on screen: the left one, and the right one in dual-pane view. */
const shownPanes = (s = get()) => (s.dual ? s.panes : s.panes.slice(0, 1));
/** The tabs on screen, one per visible pane. */
export const visibleTabs = (s = get()) => shownPanes(s).map((p) => p.active);
const paneOf = (id: string, s = get()) => s.panes.findIndex((p) => p.tabs.includes(id));
const shownTab = (s: ExplorerState, pane: number | null) => s.tabs[(s.panes[pane ?? s.activePane] ?? s.panes[0]).active];

/** The tab the keyboard, toolbar and palette act on: the active pane's. */
export const activeTabId = () => shownTab(get(), null).id;
/** A tab's state (the active tab's by default); undefined once it's closed. */
export const tabState = (id = activeTabId()): TabState | undefined => get().tabs[id];
/** The active tab's folder, or This PC. */
export const currentPath = () => tabState()?.path ?? THIS_PC;

/** The archive a tab's folder is in (the active tab's by default), from its listing; undefined for a folder on disk. */
export const archiveOf = (tab = activeTabId()): InArchive | undefined => tabState(tab)?.listing?.archive;

/** Archives open read-only: says so when something would change one (once at a time). */
export function readOnlyNotice() {
    toast.message("Archives are read-only", {
        id: "read-only",
        description: "Glint opens zip and 7z archives to look inside and copy out. To change what's in one, copy it out first.",
    });
}

/** Changes a tab's state: the active tab's unless `tab` says otherwise. */
export function updateTab(patch: Partial<TabState>, tab = activeTabId()) {
    set((s) => (s.tabs[tab] ? { tabs: { ...s.tabs, [tab]: { ...s.tabs[tab], ...patch } } } : {}));
}

const PaneContext = createContext<number | null>(null);
/** Wraps a pane, so `useTab` in it reads that pane's tab. */
export const PaneProvider = PaneContext.Provider;

/** Reads the tab a component shows: its pane's, or the active pane's outside the panes (address bar, toolbar, preview). */
export function useTab<T>(selector: (tab: TabState) => T): T {
    const pane = useContext(PaneContext);
    return useExplorer((s) => selector(shownTab(s, pane)));
}

/** What a tab shows: a folder, or a search's results. Scroll positions and thumbnail timings are kept by it, for each view. */
export const viewKey = (tab: Pick<TabState, "path" | "find">) => (tab.find ? `find:${tab.find.id}` : `dir:${tab.path}`);

/** Shows a tab's items (the active tab's by default) as a list, or as a grid of thumbnails. */
export function setView(view: ViewMode, tab = activeTabId()) {
    if (tabState(tab)?.view !== view) updateTab({ view }, tab);
}

/** Ctrl+wheel, Ctrl+= and Ctrl+- in a grid: bigger tiles (`steps` up) or smaller ones, in every grid; 0 for medium again. */
export function zoomGrid(steps: number) {
    const { settings, setSettings } = useStore.getState();
    const tileSize = steps ? zoomTiles(settings.tileSize, steps) : DEFAULT_SETTINGS.tileSize;
    if (tileSize !== settings.tileSize) setSettings({ tileSize });
}

/** "This PC", a drive's name, or the folder's. */
export function tabName(path: string, drives: Drive[]) {
    if (path === THIS_PC) return "This PC";
    if (!isRoot(path)) return baseName(path);
    const drive = drives.find((d) => samePath(d.path, path));
    return drive ? driveName(drive) : path;
}

/** Per-tab bookkeeping that never needs a render: which navigation is newest, and when it started. */
const runtime = new Map<string, { navSeq: number; navStarted: number }>();
function rt(tab: string) {
    let r = runtime.get(tab);
    if (!r) runtime.set(tab, (r = { navSeq: 0, navStarted: 0 }));
    return r;
}

/** Where each tab's list was scrolled (for the folder or search shown then), to come back to it. */
const scrolls = new Map<string, { key: string; top: number }>();
export const savedScroll = (tab: string, key: string) => {
    const saved = scrolls.get(tab);
    return saved?.key === key ? saved.top : 0;
};
export const saveScroll = (tab: string, key: string, top: number) => scrolls.set(tab, { key, top });

/** How many busy() operations run, and who waits for none. */
let working = 0;
const idle: (() => void)[] = [];

/** Shows `label` while slow work runs (quick operations finish before it appears). */
async function busy<T>(label: string, work: Promise<T>): Promise<T> {
    let id: string | number | undefined;
    const timer = setTimeout(() => (id = toast.loading(label)), 500);
    working++;
    try {
        return await work;
    } finally {
        clearTimeout(timer);
        if (id !== undefined) toast.dismiss(id);
        if (--working === 0) idle.splice(0).forEach((resolve) => resolve());
    }
}

/** Resolves once no file operation (copy, move, delete…) is running: an update waits for them before the app closes. */
export const operationsDone = () => (working === 0 ? Promise.resolve() : new Promise<void>((resolve) => idle.push(resolve)));
export const operationsRunning = () => working > 0;

interface NavOptions {
    /** Paths to select once the folder is open. */
    select?: string | string[];
    /** Record in Back/Forward history (default true). */
    history?: boolean;
    /** Keep the current list on screen while loading, as a refresh does. */
    quiet?: boolean;
    /** Don't report failures; the caller handles them. */
    silent?: boolean;
    /** The tab to open it in (default: the active one). */
    tab?: string;
}

/** Opens a folder (or This PC) in a tab. Resolves false if it couldn't, or a newer navigation won. */
export async function navigate(target: string, { select, history = true, quiet = false, silent = false, tab = activeTabId() }: NavOptions = {}) {
    const r = rt(tab);
    const seq = ++r.navSeq;
    const started = performance.now();
    if (!quiet) updateTab({ loading: true }, tab);
    let listing: Listing | null = null;
    if (target !== THIS_PC) {
        try {
            listing = await listDir(target);
        } catch (e) {
            if (seq === r.navSeq) {
                updateTab({ loading: false }, tab);
                if (!silent) toast.error(`Can't open “${baseName(target)}”`, { description: String(e) });
            }
            return false;
        }
    }
    const state = get().tabs[tab];
    // A newer navigation in this tab won, or the tab was closed meanwhile.
    if (seq !== r.navSeq || !state) return false;

    const path = listing?.path ?? THIS_PC;
    const same = state.ready && (path === THIS_PC ? state.path === THIS_PC : state.path !== THIS_PC && samePath(path, state.path));
    // Select what was asked for; on a refresh keep the selection; a new tab selects what it was
    // opened with; going up selects where we came from.
    let wanted: string[] = [];
    if (select !== undefined) wanted = [select].flat();
    else if (same || (!state.ready && state.selection.size)) wanted = [...state.selection];
    else if (state.path !== THIS_PC && samePath(parentPath(state.path), path)) wanted = [state.path];
    let selection: string[] = [];
    if (wanted.length && listing) {
        const byLower = new Map(listing.entries.map((e) => [e.path.toLowerCase(), e.path]));
        selection = wanted.flatMap((p) => byLower.get(p.replace(/[\\/]+$/, "").toLowerCase()) ?? []);
    }
    const focus = same && state.focus && selection.includes(state.focus) ? state.focus : (selection[selection.length - 1] ?? null);

    if (!same && state.find?.running) cancelFind(tab);
    // Timed from the click to the list on screen; a tab out of sight is timed when it's shown.
    r.navStarted = visibleTabs().includes(tab) ? started : 0;
    updateTab(
        {
            ready: true,
            path,
            listing,
            loading: false,
            selection: new Set(selection),
            focus,
            anchor: focus,
            ...(same ? {} : { filter: "", find: null, renaming: null }),
            ...(history && state.ready && !same ? { back: [...state.back, state.path].slice(-100), forward: [] } : {}),
        },
        tab
    );
    if (!same) {
        if (tab === activeTabId()) {
            set({ editingAddress: false });
            useStore.getState().setLastFolder(path);
        }
        syncWatches();
        if (path !== THIS_PC) useStore.getState().recordVisit(path);
        if (path === THIS_PC) refreshDrives();
    }
    // In an archive, every folder's size comes with the listing; with the index, every folder shows
    // its subfolders' sizes as it opens (and after each refresh).
    if (listing?.archive) archiveSizes(listing, tab);
    else if (listing && indexed(path)) autoSizes(tab);
    return true;
}

/** A folder in an archive: its subfolders' totals are in its listing, as the archive has them. */
function archiveSizes(listing: Listing, tab: string) {
    const sizes = new Map<string, number>();
    let total = 0;
    for (const e of listing.entries) {
        if (e.dir) sizes.set(e.name, e.size);
        total += e.size;
    }
    const { [listing.path]: _, ...rest } = get().sizes;
    set({ sizes: { ...rest, [listing.path]: { sizes, total, elapsed: 0, running: false, engine: listing.archive!.format, lane: tab } } });
}

/** Called once a tab's new list is on screen, to time the whole navigation. */
export function markShown(tab: string) {
    const r = runtime.get(tab);
    if (!r?.navStarted) return;
    updateTab({ shownIn: performance.now() - r.navStarted }, tab);
    r.navStarted = 0;
}

export async function goBack() {
    const tab = activeTabId();
    const t = tabState(tab);
    if (!t?.back.length) return;
    const { back, forward, path } = t;
    if (await navigate(back[back.length - 1], { history: false, tab })) updateTab({ back: back.slice(0, -1), forward: [path, ...forward] }, tab);
}

export async function goForward() {
    const tab = activeTabId();
    const t = tabState(tab);
    if (!t?.forward.length) return;
    const { back, forward, path } = t;
    if (await navigate(forward[0], { history: false, tab })) updateTab({ back: [...back, path], forward: forward.slice(1) }, tab);
}

export const goUp = () => {
    const path = currentPath();
    return path === THIS_PC ? Promise.resolve(false) : navigate(parentPath(path));
};

/** Re-reads a tab's folder in place; if it's gone, opens its parent. */
export async function reload(tab = activeTabId()) {
    const t = tabState(tab);
    if (!t) return;
    if (t.path === THIS_PC) return refreshDrives();
    const r = rt(tab);
    const seq = r.navSeq + 1;
    // A false result from a navigation that was overtaken by a newer one isn't a missing folder.
    if (!(await navigate(t.path, { history: false, quiet: true, silent: true, tab })) && r.navSeq === seq && tabState(tab)) {
        toast.message(`“${baseName(t.path)}” is no longer available`);
        if (!t.listing?.archive) {
            await navigate(parentPath(t.path), { history: false, tab });
            return;
        }
        // A changed (or deleted) archive: the nearest folder above that's still there, in it or out of it.
        for (let at = parentPath(t.path); at !== THIS_PC; at = parentPath(at)) {
            if (await navigate(at, { history: false, silent: true, tab })) return;
            if (!tabState(tab)) return;
        }
        await navigate(THIS_PC, { history: false, tab });
    }
}

/** The folder on disk to watch for a tab: its own, or the one its archive is in. */
const watchedFolder = (t: TabState) => (t.listing?.archive ? parentPath(t.listing.archive.outer) : t.path);

/** Re-reads `folder` in every tab on screen that shows it, or an archive in it (it changed on disk). */
export function reloadFolder(folder: string) {
    const s = get();
    for (const id of visibleTabs(s)) {
        const t = s.tabs[id];
        if (t && !t.find && t.path !== THIS_PC && samePath(folder, watchedFolder(t))) reload(id);
    }
}

/** F5: runs the search again when one is shown, otherwise re-reads the folder. */
export const refresh = () => {
    const find = tabState()?.find;
    return find ? runFind(find.query) : reload();
};

export async function refreshDrives() {
    try {
        set({ drives: await listDrives() });
    } catch {
        // keep the drives we know
    }
}

async function loadKnownFolders() {
    const sources: [KnownFolder["id"], string, () => Promise<string>][] = [
        ["home", "Home", homeDir],
        ["desktop", "Desktop", desktopDir],
        ["documents", "Documents", documentDir],
        ["downloads", "Downloads", downloadDir],
        ["pictures", "Pictures", pictureDir],
        ["music", "Music", audioDir],
        ["videos", "Videos", videoDir],
    ];
    const found = await Promise.all(
        sources.map(async ([id, name, dir]) => {
            try {
                return { id, name, path: (await dir()).replace(/[\\/]+$/, "") };
            } catch {
                return null;
            }
        })
    );
    set({ known: found.filter((k): k is KnownFolder => !!k?.path) });
}

const homeFolder = () => get().known.find((k) => k.id === "home")?.path ?? THIS_PC;

/** A folder on disk to search in when Text mode has none yet: Default search folder, the open folder, Home, or the first drive. */
export function defaultTextSearchFolder(): string {
    const { settings } = useStore.getState();
    if (settings.defaultSearchPath.trim()) return settings.defaultSearchPath.trim();
    const cur = currentPath();
    if (cur && cur !== THIS_PC && !archiveOf()) return cur;
    const home = homeFolder();
    if (home && home !== THIS_PC) return home;
    return get().drives[0]?.path ?? "";
}

let initialized: Promise<void> | undefined;
/** The session is saved only once startup has restored it, so starting can't overwrite it. */
let restored = false;

/** Opens the first folders: last time's tabs or the start folder, and any the app was started with. */
export const initExplorer = () => (initialized ??= init());

async function init() {
    // Drives fill in when they answer; a sleeping network drive mustn't hold up the first folder.
    refreshDrives();
    // Asked first (briefly), so the first folder can already show its sizes from the index.
    await Promise.race([refreshIndex().then(noticeServiceUpdate), new Promise((resolve) => setTimeout(resolve, 300))]);
    await loadKnownFolders();
    // Folders from File Explorer, a double-click or the command line, each in a tab of its own.
    const launch = await takeLaunchTargets().catch((): LaunchTarget[] => []);
    const { settings, lastFolder, session } = useStore.getState();
    const home = homeFolder();
    if (settings.restoreTabs && session && restoreSession(session)) {
        showOnStart(placeOutsideTabs(launch, true));
        await Promise.all(visibleTabs().map((id) => openTab(id)));
    } else if (launch.length) {
        // The first one instead of the start folder.
        const tab = activeTabId();
        set((s) => ({ tabs: { ...s.tabs, [tab]: blankTab(tab, launch[0].path, s.tabs[tab]?.view, launch[0].select ?? undefined) } }));
        showOnStart(placeOutsideTabs(launch.slice(1), false));
        await Promise.all(visibleTabs().map((id) => openTab(id)));
    } else {
        const start = settings.startFolder === "thispc" ? THIS_PC : settings.startFolder === "last" && lastFolder !== null ? lastFolder : home;
        const tab = activeTabId();
        if (!(await navigate(start, { tab, silent: start !== home })) && !(await navigate(home, { tab }))) await navigate(THIS_PC, { tab });
    }
    restored = true;
    saveSession();
}

/** Puts back last time's tabs and panes; each folder opens when its tab is first shown. False if there's nothing to restore. */
function restoreSession(session: Session) {
    const tabs: Record<string, TabState> = {};
    const panes = session.panes
        .filter((p) => p.paths.length)
        .slice(0, 2)
        .map((p) => {
            const ids = p.paths.map((path, i) => {
                const id = newTabId();
                tabs[id] = blankTab(id, path, p.views?.[i] === "grid" ? "grid" : "list");
                return id;
            });
            return { tabs: ids, active: ids[Math.min(Math.max(0, p.active), ids.length - 1)] };
        });
    if (!panes.length) return false;
    runtime.clear();
    const dual = session.dual && panes.length === 2;
    set({ tabs, panes, dual, activePane: dual ? Math.min(Math.max(0, session.activePane), 1) : 0 });
    return true;
}

/** Opens a tab's folder for the first time; if it's gone, the nearest parent that's there, else Home. */
async function openTab(id: string) {
    const path = tabState(id)?.path ?? THIS_PC;
    if (path !== THIS_PC) {
        for (let at = path; at !== THIS_PC; at = parentPath(at)) {
            if (await navigate(at, { tab: id, silent: true })) return;
            if (!tabState(id)) return;
        }
        if (await navigate(homeFolder(), { tab: id, silent: true })) return;
    }
    await navigate(THIS_PC, { tab: id });
}

let savedSession = "";

/** Remembers each pane's tabs for the next start. */
function saveSession(s = get()) {
    if (!restored) return;
    const session: Session = {
        dual: s.dual,
        activePane: s.activePane,
        panes: s.panes.map((p) => ({
            paths: p.tabs.map((id) => s.tabs[id]?.path ?? THIS_PC),
            active: Math.max(0, p.tabs.indexOf(p.active)),
            views: p.tabs.map((id) => s.tabs[id]?.view ?? "list"),
        })),
    };
    const json = JSON.stringify(session);
    if (json === savedSession) return;
    savedSession = json;
    useStore.getState().setSession(session);
}

useExplorer.subscribe((s) => saveSession(s));

let watching = "";

/** Watches the folders on screen, and only those (a tab out of sight is re-read when it's shown). */
function syncWatches() {
    const s = get();
    const folders = new Map<string, string>();
    for (const id of visibleTabs(s)) {
        const t = s.tabs[id];
        if (t?.listing && t.path !== THIS_PC) folders.set(watchedFolder(t).toLowerCase(), watchedFolder(t));
    }
    const key = [...folders.keys()].join("|");
    if (key === watching) return;
    watching = key;
    watchDirs([...folders.values()]).catch(() => {});
}

// Tabs and panes

/** Closed tabs, newest last, for Ctrl+Shift+T. */
const closedTabs: { path: string; view: ViewMode; pane: number; index: number }[] = [];
/** Background tabs opened from one tab go after each other, as in a browser. */
let openedFrom: { from: string; last: string } | null = null;

/**
 * Adds a tab for `path` to `pane`, after `after` (default: the pane's active tab), shown as `view`
 * (default: the active tab's), with `select` selected once it opens.
 */
function addTab(path: string, pane: number, after?: string, view = tabState()?.view, select?: string) {
    const id = newTabId();
    set((s) => {
        const p = s.panes[pane];
        const at = p.tabs.indexOf(after ?? p.active);
        const tabs = [...p.tabs];
        tabs.splice(at < 0 ? tabs.length : at + 1, 0, id);
        const panes = [...s.panes];
        panes[pane] = { ...p, tabs };
        return { tabs: { ...s.tabs, [id]: blankTab(id, path, view, select) }, panes };
    });
    return id;
}

/**
 * Adds a tab at the end of the active pane for each folder handed over from outside, and returns
 * the last one's id. With `reuse` (at start), a restored tab on screen that already shows a folder
 * stands in for it: an update restarts the app with the folders it was first started with.
 */
function placeOutsideTabs(targets: LaunchTarget[], reuse: boolean) {
    let last: string | null = null;
    for (const { path, select } of targets) {
        const s = get();
        const shown = reuse && !select ? shownPanes(s).find((p) => p.tabs.some((id) => samePath(s.tabs[id]?.path ?? THIS_PC, path))) : undefined;
        if (shown) {
            last = shown.tabs.find((id) => samePath(s.tabs[id]?.path ?? THIS_PC, path))!;
            continue;
        }
        const pane = s.panes[s.activePane];
        last = addTab(path, s.activePane, pane.tabs[pane.tabs.length - 1], undefined, select ?? undefined);
    }
    return last;
}

/** At start, before any folder opens: makes `id` the tab on screen in its pane, and that pane the active one. */
function showOnStart(id: string | null) {
    const pane = id ? paneOf(id) : -1;
    if (pane < 0) return;
    set((s) => {
        const panes = [...s.panes];
        panes[pane] = { ...panes[pane], active: id! };
        return { panes, activePane: pane };
    });
}

/** Opens the folders File Explorer or a later launch handed over ("open-requested"), in new tabs in front. Resolves whether there were any. */
export async function openRequested() {
    await initExplorer();
    const targets = await takeLaunchTargets().catch((): LaunchTarget[] => []);
    const last = placeOutsideTabs(targets, false);
    if (!last) return false;
    useStore.getState().setMode("files");
    set({ palette: false });
    activateTab(last);
    return true;
}

/** A tab coming on screen: opens its folder the first time, else re-reads it (folders out of sight aren't watched). */
function showTab(id: string) {
    const t = tabState(id);
    if (!t) return;
    if (!t.ready) {
        openTab(id);
        return;
    }
    rt(id).navStarted = performance.now();
    if (!t.find) reload(id);
}

/** Shows tab `id` in its pane and makes that pane the active one. */
export function activateTab(id: string) {
    const s = get();
    const pane = paneOf(id, s);
    if (pane < 0) return;
    const wasShown = visibleTabs(s).includes(id);
    const changed = s.activePane !== pane || s.panes[pane].active !== id;
    const panes = [...s.panes];
    panes[pane] = { ...panes[pane], active: id };
    set({ panes, activePane: pane, dual: s.dual || pane > 0, ...(changed ? { quickLook: false, editingAddress: false } : {}) });
    if (!wasShown) showTab(id);
    syncWatches();
    if (changed) useStore.getState().setLastFolder(currentPath());
}

/** Ctrl+T: a new tab on the open folder (or `path`), shown at once. */
export function newTab(path = currentPath()) {
    activateTab(addTab(path, get().activePane));
}

/** Middle-click on a folder: a new tab for it next to the current one, staying where you are. */
export function openInNewTab(path: string) {
    const s = get();
    const pane = s.activePane;
    const from = s.panes[pane].active;
    const after = openedFrom?.from === from && s.panes[pane].tabs.includes(openedFrom.last) ? openedFrom.last : from;
    openedFrom = { from, last: addTab(path, pane, after) };
}

/** For links to folders: a middle-click opens the folder in a new tab, like a browser. */
export const middleClickOpens = (path: string) => (e: { button: number; preventDefault(): void }) => {
    if (e.button !== 1) return;
    // Also keeps the browser's autoscroll from starting.
    e.preventDefault();
    openInNewTab(path);
};

export function duplicateTab(id: string) {
    const t = tabState(id);
    const pane = paneOf(id);
    if (t && pane >= 0) activateTab(addTab(t.path, pane, id, t.view));
}

/** Ctrl+Tab and Ctrl+Shift+Tab: the next or previous tab in the active pane. */
export function cycleTab(step: number) {
    const s = get();
    const { tabs, active } = s.panes[s.activePane];
    activateTab(tabs[(tabs.indexOf(active) + step + tabs.length) % tabs.length]);
}

/** Ctrl+1…8: that tab of the active pane; Ctrl+9: its last one. */
export function selectTab(n: number) {
    const { tabs } = get().panes[get().activePane];
    const id = n >= 9 ? tabs[tabs.length - 1] : tabs[n - 1];
    if (id) activateTab(id);
}

/** Stops what a closed tab had running and drops its bookkeeping. */
function forget(id: string, tab: TabState | undefined) {
    if (tab?.find?.running) cancelFind(id);
    if (Object.values(get().sizes).some((run) => run.running && run.lane === id)) cancelFolderSizes(id);
    runtime.delete(id);
    scrolls.delete(id);
    dropThumbnails(id);
}

function remember(tab: TabState | undefined, pane: number, index: number) {
    if (!tab) return;
    closedTabs.push({ path: tab.path, view: tab.view, pane, index });
    if (closedTabs.length > 25) closedTabs.shift();
}

/** Whether Ctrl+Shift+T has a tab to bring back. */
export const canReopenTab = () => closedTabs.length > 0;

/** Closes a tab. A pane's only tab closes that pane in dual-pane view; the last tab of all stays. */
export function closeTab(id = activeTabId()) {
    const s = get();
    const pane = paneOf(id, s);
    if (pane < 0) return;
    const p = s.panes[pane];
    if (p.tabs.length === 1) {
        if (s.dual) closePane(pane);
        return;
    }
    const at = p.tabs.indexOf(id);
    const tabs = p.tabs.filter((t) => t !== id);
    // Like a browser: the tab that takes its place, or the one before it at the end.
    const active = p.active === id ? tabs[Math.min(at, tabs.length - 1)] : p.active;
    const shown = p.active === id && shownPanes(s).includes(p);
    const panes = [...s.panes];
    panes[pane] = { tabs, active };
    const { [id]: closed, ...rest } = s.tabs;
    remember(closed, pane, at);
    set({ tabs: rest, panes, ...(shown ? { quickLook: false } : {}) });
    forget(id, closed);
    if (shown) showTab(active);
    syncWatches();
}

/** Closes a pane in dual-pane view, with its tabs; the other one is left on its own. */
function closePane(pane: number) {
    const s = get();
    if (!s.dual || s.panes.length < 2) return;
    const gone = s.panes[pane];
    const tabs = { ...s.tabs };
    gone.tabs.forEach((id, i) => {
        remember(s.tabs[id], pane, i);
        delete tabs[id];
    });
    set({ tabs, panes: [s.panes[1 - pane]], dual: false, activePane: 0, quickLook: false, editingAddress: false });
    for (const id of gone.tabs) forget(id, s.tabs[id]);
    syncWatches();
    useStore.getState().setLastFolder(currentPath());
}

export function closeOtherTabs(id: string) {
    const pane = paneOf(id);
    if (pane < 0) return;
    // Shown first, so closing the others never brings up one that's about to go.
    activateTab(id);
    for (const other of get().panes[pane].tabs) if (other !== id) closeTab(other);
}

/** Ctrl+Shift+T: brings back the last closed tab where it was (after the current tab if its pane is gone). */
export function reopenClosedTab() {
    const last = closedTabs.pop();
    if (!last) return;
    const s = get();
    const there = !!shownPanes(s)[last.pane];
    const pane = there ? last.pane : s.activePane;
    const id = newTabId();
    set((st) => {
        const p = st.panes[pane];
        const tabs = [...p.tabs];
        tabs.splice(there ? Math.min(last.index, tabs.length) : tabs.indexOf(p.active) + 1, 0, id);
        const panes = [...st.panes];
        panes[pane] = { ...p, tabs };
        return { tabs: { ...st.tabs, [id]: blankTab(id, last.path, last.view) }, panes };
    });
    activateTab(id);
}

/** Dual-pane view: moves a tab to the other pane (not a pane's only tab). */
export function moveTabToOtherPane(id: string) {
    const s = get();
    const from = paneOf(id, s);
    if (!s.dual || from < 0 || s.panes[from].tabs.length < 2) return;
    const to = 1 - from;
    const source = s.panes[from];
    const at = source.tabs.indexOf(id);
    const left = source.tabs.filter((t) => t !== id);
    const sourceActive = source.active === id ? left[Math.min(at, left.length - 1)] : source.active;
    const target = s.panes[to];
    const tabs = [...target.tabs];
    tabs.splice(tabs.indexOf(target.active) + 1, 0, id);
    const panes = [...s.panes];
    panes[from] = { tabs: left, active: sourceActive };
    panes[to] = { tabs, active: target.active };
    set({ panes });
    if (source.active === id) showTab(sourceActive);
    activateTab(id);
}

/** Makes `pane` the one the keyboard and toolbar act on (a click in it does this). */
export function focusPane(pane: number) {
    const s = get();
    if (pane === s.activePane || !shownPanes(s)[pane]) return;
    set({ activePane: pane, quickLook: false, editingAddress: false });
    useStore.getState().setLastFolder(currentPath());
}

/** F6 in dual-pane view: the other pane. */
export const switchPane = () => focusPane(1 - get().activePane);

/** Ctrl+Shift+D: two panes side by side, or back to one. The right pane keeps its tabs while hidden. */
export function toggleDual() {
    const s = get();
    if (s.dual) {
        set({ dual: false, comparePanes: false, diffOnly: false, activePane: 0, quickLook: false, editingAddress: false });
    } else if (s.panes.length < 2) {
        // A new right pane starts on the folder you're in.
        const id = newTabId();
        set({ tabs: { ...s.tabs, [id]: blankTab(id, currentPath(), tabState()?.view) }, panes: [s.panes[0], { tabs: [id], active: id }], dual: true, activePane: 1, quickLook: false, editingAddress: false });
        openTab(id);
    } else {
        set({ dual: true, activePane: 1, quickLook: false, editingAddress: false });
        showTab(s.panes[1].active);
    }
    syncWatches();
    useStore.getState().setLastFolder(currentPath());
}

/** Ctrl+Shift+M: compares the two panes side by side, turning on dual-pane view first if needed. */
export function toggleComparePanes() {
    const s = get();
    if (!s.dual) {
        toggleDual();
        set({ comparePanes: true, diffOnly: false });
    } else {
        set({ comparePanes: !s.comparePanes, diffOnly: false });
    }
}

export function toggleDiffOnly() {
    set((s) => ({ diffOnly: !s.diffOnly }));
}

/** The folder open in the other pane, when there's one to copy or move into (an archive isn't one). */
export function otherPaneFolder() {
    const s = get();
    if (!s.dual) return null;
    const other = s.panes[1 - s.activePane];
    const t = other && s.tabs[other.active];
    return t?.ready && t.path !== THIS_PC && !t.find && !t.listing?.archive ? { tab: t.id, path: t.path } : null;
}

/** Dual-pane view: copies (or moves) items to the folder open in the other pane, through Windows' file engine. */
export async function copyToOtherPane(move: boolean, paths = [...(tabState()?.selection ?? [])]) {
    const target = otherPaneFolder();
    if (!target) {
        toast.message(archiveOf(get().panes[1 - get().activePane]?.active) ? "Archives are read-only: open a folder in the other pane to copy into" : "Open a folder in the other pane first");
        return;
    }
    const tab = activeTabId();
    if (archiveOf(tab)) {
        if (move) readOnlyNotice();
        else copyOutOfArchive(paths, target);
        return;
    }
    const sources = paths.filter((p) => !samePath(parentPath(p), target.path) && !samePath(p, target.path));
    if (!sources.length) return;
    const what = plural(sources.length, "item");
    try {
        const done = await busy(`${move ? "Moving" : "Copying"} ${what}…`, fileOp({ kind: move ? "move" : "copy", sources, dest: target.path }));
        if (!done) return;
        await navigate(target.path, { tab: target.tab, history: false, quiet: true, select: sources.map((p) => joinPath(target.path, baseName(p))) });
        if (move && !tabState(tab)?.find) reload(tab);
    } catch (e) {
        toast.error(`Couldn't ${move ? "move" : "copy"} ${what}`, { description: String(e) });
    }
}

// Archives: what's copied out of one is extracted first, then copied by Windows' file engine.

/**
 * Extracts `paths` (in an archive) for Windows' file engine to copy, with progress once it takes a
 * moment and a Stop button, then runs `then` on the copies. Says which files couldn't be extracted.
 * Resolves undefined when it was stopped or failed (and said so).
 */
function extracting<T>(paths: string[], purpose: "copy" | "out", then: (staged: Staged) => Promise<T>): Promise<T | undefined> {
    const what = plural(paths.length, "item");
    return staging({ doing: `Extracting ${what} from the archive`, failed: `Couldn't extract ${what}`, skipped: "extracted" }, (lane, p) => archiveStage(paths, purpose, lane, p), then);
}

/**
 * Archive work with a progress toast (and Stop) once it takes a moment: extracting, compressing.
 * Reports the files it had to leave out, then hands over what it made.
 */
async function staging<T>(say: { doing: string; failed: string; skipped: string }, run: (lane: string, onProgress: Progress) => Promise<Staged>, then: (staged: Staged) => Promise<T>): Promise<T | undefined> {
    const lane = activeTabId();
    let shown: string | number | undefined;
    let percent = -1;
    const label = (p: number) => `${say.doing}…${p >= 0 ? ` ${p}%` : ""}`;
    const show = () => (shown = toast.loading(label(percent), { action: { label: "Stop", onClick: () => cancelArchiveStage(lane) }, duration: Infinity }));
    const timer = setTimeout(show, 400);
    working++;
    try {
        const staged = await run(lane, (done, total) => {
            const next = total ? Math.floor((100 * done) / total) : 0;
            if (next === percent) return;
            percent = next;
            if (shown !== undefined) toast.loading(label(percent), { id: shown, action: { label: "Stop", onClick: () => cancelArchiveStage(lane) }, duration: Infinity });
        });
        clearTimeout(timer);
        if (shown !== undefined) toast.dismiss(shown);
        if (staged.skipped.length) {
            const names = staged.skipped.slice(0, 3).map((s) => `“${baseName(s.path)}”`).join(", ");
            const more = staged.skipped.length > 3 ? ` and ${staged.skipped.length - 3} more` : "";
            toast.warning(`${plural(staged.skipped.length, "file")} couldn't be ${say.skipped}`, { description: `${names}${more}: ${staged.skipped[0].reason}`, duration: 10_000 });
        }
        return await then(staged);
    } catch (e) {
        clearTimeout(timer);
        if (shown !== undefined) toast.dismiss(shown);
        if (String(e) !== "cancelled") toast.error(say.failed, { description: String(e) });
        return undefined;
    } finally {
        if (--working === 0) idle.splice(0).forEach((resolve) => resolve());
    }
}

/**
 * Compress to zip: `entries` (on disk) into a zip next to them, named after the first one as
 * Explorer does, then renamed in place like a new folder. Ctrl+Z takes it back.
 */
export function compressEntries(entries: FileEntry[]) {
    const tab = activeTabId();
    if (!entries.length) return;
    if (entries.some(isArchived)) return readOnlyNotice();
    const first = entries[0];
    const folder = parentPath(first.path);
    const name = `${first.dir || !first.name.includes(".") ? first.name : first.name.slice(0, first.name.lastIndexOf("."))}.zip`;
    const what = entries.length === 1 ? `“${first.name}”` : plural(entries.length, "item");
    return staging({ doing: `Compressing ${what}`, failed: `Couldn't compress ${what}`, skipped: "added" }, (lane, p) => archiveCompress(entries.map((e) => e.path), folder, name, lane, p), async (staged) => {
        const zip = staged.paths[0];
        rememberUndo(`compression into “${baseName(zip)}”`, () => performFileOp({ kind: "delete", paths: [zip], permanent: false, confirm: false }));
        const t = tabState(tab);
        if (t && !t.find && samePath(t.path, folder)) {
            await navigate(folder, { history: false, quiet: true, select: zip, tab });
            updateTab({ renaming: tabState(tab)?.focus ?? null }, tab);
        } else toast.success(`Compressed ${what} into “${baseName(zip)}”`);
    });
}

/** Extract here: a zip or 7z on disk into a new folder next to it, named after it (Explorer's Extract All). Ctrl+Z takes it back. */
export function extractHere(entry: FileEntry) {
    const tab = activeTabId();
    if (isArchived(entry)) return readOnlyNotice();
    const what = `“${entry.name}”`;
    return staging({ doing: `Extracting ${what}`, failed: `Couldn't extract ${what}`, skipped: "extracted" }, (lane, p) => archiveExtractHere(entry.path, lane, p), async (staged) => {
        rememberUndo(`extraction of ${what}`, () => performFileOp({ kind: "delete", paths: [staged.folder], permanent: false, confirm: false }));
        const t = tabState(tab);
        if (t && !t.find && samePath(t.path, parentPath(entry.path))) await navigate(t.path, { history: false, quiet: true, select: staged.folder, tab });
        else toast.success(`Extracted ${what} into “${baseName(staged.folder)}”`);
    });
}

/** Copy to the other pane, out of an archive: extracted, copied by Windows' file engine (progress, *Replace or Skip*), then the extracted copies go. */
function copyOutOfArchive(paths: string[], target: { tab: string; path: string }) {
    if (!paths.length) return;
    return extracting(paths, "copy", async (staged) => {
        const what = plural(staged.paths.length, "item");
        try {
            const done = await busy(`Copying ${what}…`, fileOp({ kind: "copy", sources: staged.paths, dest: target.path }));
            if (done) await navigate(target.path, { tab: target.tab, history: false, quiet: true, select: staged.paths.map((p) => joinPath(target.path, baseName(p))) });
        } catch (e) {
            toast.error(`Couldn't copy ${what}`, { description: String(e) });
        } finally {
            archiveUnstage(staged.folder).catch(() => {});
        }
    });
}

/** Copies extracted for a drag that didn't start (the button came up while they were being made), to drag at once next time. */
let readyToDrag: { key: string; staged: Staged } | null = null;

/**
 * Drags items out of an archive: they're extracted first (Windows' drag carries files), then
 * dragged as copies. A big selection can take longer to extract than the button is held: then
 * the next drag of the same items starts at once.
 */
export async function dragFromArchive(paths: string[]) {
    const key = paths.join("\n").toLowerCase();
    const ready = readyToDrag?.key === key ? readyToDrag.staged : null;
    readyToDrag = null;
    const drag = async (staged: Staged) => {
        const effect = await startDrag(staged.paths, true);
        if (effect === "released") {
            readyToDrag = { key, staged };
            toast.message("Ready to drag", { id: "drag-ready", description: "They took a moment to extract: drag them again." });
        }
    };
    try {
        if (ready) return await drag(ready);
    } catch {
        // Gone meanwhile (dropped into a folder here, which moves them): extract them again.
    }
    return extracting(paths, "out", (staged) => drag(staged).catch((e) => toast.error("Couldn't drag these items", { description: String(e) })));
}

/** Opens a file in an archive with its app: a read-only temp copy of it, so changes can't be lost silently. */
async function openFromArchive(entry: FileEntry, open: (path: string) => Promise<unknown> = openPath) {
    try {
        const copy = await busy(`Extracting “${entry.name}”…`, archiveExtract(entry.path, "open"));
        await open(copy);
    } catch (e) {
        toast.error(`Couldn't open “${entry.name}”`, { description: String(e) });
    }
}

/** "Open with" an app of your choice (or the editor in Settings), for a file in an archive too. */
export function openArchivedWith(entry: FileEntry, open: (path: string) => Promise<unknown>) {
    return openFromArchive(entry, open);
}

// Search

let findSeq = 0;

/** Searches the open folder and its subfolders (or every drive from This PC) by name: index, else fd. */
export async function runFind(query: string) {
    if (!query.trim()) return clearFind();
    const tab = activeTabId();
    const t = tabState(tab);
    if (!t) return;
    const settings = useStore.getState().settings;
    const id = ++findSeq;
    const roots = t.path === THIS_PC ? get().drives.filter((d) => d.kind === "fixed" || d.kind === "removable").map((d) => d.path) : [t.path];
    const started = performance.now();
    updateTab(
        {
            find: { id, query, engine: "", scope: t.path, entries: [], running: true, elapsed: 0, limited: false, error: "" },
            selection: new Set(),
            focus: null,
            anchor: null,
            renaming: null,
        },
        tab
    );
    const update = (patch: Partial<FindRun>) => {
        const find = tabState(tab)?.find;
        if (find?.id === id) updateTab({ find: { ...find, ...patch } }, tab);
    };
    const options = {
        hidden: settings.showHidden,
        respectIgnore: settings.findRespectIgnore,
        exclude: settings.findExclusions,
        maxResults: settings.findMaxResults,
    };
    try {
        await findFiles(roots, query, options, tab, (event) => {
            const find = tabState(tab)?.find;
            if (find?.id !== id) return;
            if (event.type === "entries") update({ entries: find.entries.concat(event.entries) });
            else if (event.type === "started") update({ engine: event.engine });
            else if (event.type === "error") update({ error: event.message });
            else update({ running: false, elapsed: performance.now() - started, limited: event.limited });
        });
    } catch (e) {
        update({ running: false, error: String(e) });
    }
}

export const stopFind = () => {
    const tab = activeTabId();
    if (tabState(tab)?.find?.running) cancelFind(tab);
};

/** Leaves search results (and the filter) for the folder itself. */
export function clearFind() {
    const tab = activeTabId();
    const find = tabState(tab)?.find;
    if (find?.running) cancelFind(tab);
    updateTab({ filter: "", ...(find ? { find: null, selection: new Set(), focus: null, anchor: null } : {}) }, tab);
    if (find) reload(tab);
}

/** Sizes of every subfolder of the open folder: from the index when it has the folder, else with dust. */
export const calcSizes = () => sizeFolder(activeTabId(), false);

/** As a folder opens: from the index only, and quietly nothing when it can't answer. */
const autoSizes = (tab: string) => sizeFolder(tab, true);

async function sizeFolder(tab: string, auto: boolean) {
    const path = tabState(tab)?.path;
    if (!path || path === THIS_PC) return;
    const previous = get().sizes[path];
    const save = (run: SizeRun | null) => {
        const { [path]: _, ...rest } = get().sizes;
        set({ sizes: run ? { ...rest, [path]: run } : rest });
    };
    // The index answers in a millisecond or two: no "calculating…" flash for it.
    if (!auto) save({ sizes: previous?.sizes ?? new Map(), total: previous?.total ?? 0, elapsed: previous?.elapsed ?? 0, running: true, engine: previous?.engine ?? "dust", lane: tab });
    try {
        const result = await folderSizes(path, tab, auto);
        save({ sizes: new Map(result.sizes), total: result.total, elapsed: result.elapsed, running: false, engine: result.engine, lane: tab });
    } catch (e) {
        if (auto) return;
        save(previous ? { ...previous, running: false } : null);
        if (String(e) !== "cancelled") toast.error("Couldn't calculate folder sizes", { description: String(e) });
    }
}

/** Stops measuring the open folder, whichever tab started it. */
export const stopSizes = () => {
    const run = get().sizes[currentPath()];
    if (run?.running) cancelFolderSizes(run.lane);
};

// Instant search

let indexPoll: ReturnType<typeof setTimeout> | undefined;

/** Asks the service how it's doing; while a drive is being read, asks again every second. */
export async function refreshIndex() {
    clearTimeout(indexPoll);
    try {
        const shown = visibleTabs().map((id) => ({ id, path: tabState(id)?.path ?? THIS_PC }));
        const before = shown.map((t) => indexed(t.path));
        const index = await indexStatus();
        set({ index });
        // A folder on screen just had its drive become ready: show its sizes without waiting for a navigation.
        shown.forEach((t, i) => {
            if (!before[i] && tabState(t.id)?.path === t.path && indexed(t.path)) autoSizes(t.id);
        });
        if (index.state === "on") {
            if (!index.volumes.length || index.volumes.some((v) => v.state === "scanning")) {
                indexPoll = setTimeout(refreshIndex, 1000);
            } else if (index.volumes.some((v) => v.state === "error")) {
                indexPoll = setTimeout(refreshIndex, 5000);
            }
        }
    } catch {
        // keep what we knew
    }
}

/** Whether the index has the drive `path` is on, ready to answer. */
export function indexed(path: string) {
    const { index } = get();
    const drive = path.slice(0, 2).toUpperCase();
    return index?.state === "on" && index.volumes.some((v) => v.volume === drive && v.state === "ready");
}

/** Whether the service runs an older (or newer) build than the app's: true after the app updated itself. */
export const serviceOutdated = (index: IndexInfo | null) =>
    index?.state === "on" && !!index.version && index.version !== index.appVersion;

/** After the app updates, the service keeps running the version it had until it's updated too (one more approval). */
function noticeServiceUpdate() {
    const { index } = get();
    if (!index || !serviceOutdated(index) || !firstNotice("service-update-notice", index.appVersion)) return;
    toast.info("Instant search has an update", {
        description: `Its service is still version ${index.version}. Updating it asks for administrator approval.`,
        action: { label: "Update", onClick: () => void enableIndex() },
        duration: 15_000,
    });
}

let enabling: Promise<void> | undefined;

/**
 * Turns instant search on (or updates it, or allows this user); Windows asks for administrator
 * approval. Clicking again meanwhile (the card, the status bar, Settings) waits for the same run.
 */
export function enableIndex() {
    return (enabling ??= turnOnIndex().finally(() => (enabling = undefined)));
}

async function turnOnIndex() {
    const updating = serviceOutdated(get().index);
    try {
        set({ index: await busy(updating ? "Updating instant search…" : "Turning on instant search…", indexEnable()) });
        refreshIndex();
        if (updating) toast.success("Instant search is updated", { description: `Its service now runs version ${get().index?.appVersion}.` });
        else toast.success("Instant search is on", { description: "Each drive is searched with the index as soon as it has been read." });
    } catch (e) {
        if (String(e) !== "cancelled") toast.error(updating ? "Couldn't update instant search" : "Couldn't turn on instant search", { description: String(e) });
    }
}

/** Stops and removes the service and its index files (administrator approval again). */
export async function disableIndex() {
    try {
        set({ index: await busy("Turning off instant search…", indexDisable()) });
        toast.success("Instant search is off", { description: "The service and its index files were removed." });
    } catch (e) {
        if (String(e) !== "cancelled") toast.error("Couldn't turn off instant search", { description: String(e) });
    }
}

// Selection (in the active tab)

export function selectOnly(path: string | null) {
    updateTab({ selection: new Set(path ? [path] : []), focus: path, anchor: path });
}

export function toggleSelected(path: string) {
    const tab = activeTabId();
    const selection = new Set(tabState(tab)?.selection);
    if (!selection.delete(path)) selection.add(path);
    updateTab({ selection, focus: path, anchor: path }, tab);
}

/** Shift-selection from the anchor to `to`; with `add` it extends the selection instead of replacing it. */
export function selectRange(rows: FileEntry[], to: number, add = false) {
    const tab = activeTabId();
    const t = tabState(tab);
    if (!t) return;
    const at = t.anchor === null ? -1 : rows.findIndex((r) => r.path === t.anchor);
    const from = at < 0 ? to : at;
    const next = new Set(add ? t.selection : []);
    for (let i = Math.min(from, to); i <= Math.max(from, to); i++) next.add(rows[i].path);
    updateTab({ selection: next, focus: rows[to].path, anchor: rows[from].path }, tab);
}

export function selectAll(rows: FileEntry[]) {
    updateTab({ selection: new Set(rows.map((r) => r.path)) });
}

// Actions on files

export function openEntry(entry: FileEntry) {
    // A zip or 7z archive opens like a folder.
    if (entry.dir || isArchiveFile(entry)) return navigate(entry.path);
    if (isArchived(entry)) return openFromArchive(entry);
    return openPath(entry.path).catch((e) => toast.error(`Couldn't open “${entry.name}”`, { description: String(e) }));
}

// Undo (Ctrl+Z): the last file operations, newest last, each with what takes it back.

export interface UndoStep {
    /** "move of 3 items", as in "Undid the move of 3 items". */
    label: string;
    run: () => Promise<unknown>;
}
const undoSteps: UndoStep[] = [];

/** Keeps how to take back what was just done, for Ctrl+Z (the newest 50 steps). */
export function rememberUndo(label: string, run: () => Promise<unknown>): UndoStep {
    const step = { label, run };
    undoSteps.push(step);
    if (undoSteps.length > 50) undoSteps.shift();
    return step;
}

/**
 * A copy, move, rename or delete through Windows' file engine that Ctrl+Z can take back. False when
 * the person cancelled in one of its dialogs; what was done by then can still be undone.
 */
export async function fileOp(op: FileOp): Promise<boolean> {
    const { completed, items } = await performFileOp(op);
    const done = items.flatMap((i) => (i.to ? [{ from: i.from, to: i.to }] : []));
    const what = plural(done.length, "item");
    if (done.length) {
        // Taking back a copy recycles it rather than deleting it for good.
        if (op.kind === "copy") rememberUndo(`copy of ${what}`, () => performFileOp({ kind: "delete", paths: done.map((i) => i.to), permanent: false, confirm: false }));
        else if (op.kind === "move") rememberUndo(`move of ${what}`, () => performFileOp({ kind: "moveBack", items: done.map((i) => ({ path: i.to, folder: parentPath(i.from), name: baseName(i.from) })) }));
        else if (op.kind === "rename") rememberUndo(`rename to “${baseName(done[0].to)}”`, () => performFileOp({ kind: "rename", path: done[0].to, name: baseName(done[0].from) }));
        else if (op.kind === "delete") rememberUndo(`deletion of ${what}`, () => performFileOp({ kind: "restore", items: done.map((i) => ({ path: i.to, folder: parentPath(i.from), name: baseName(i.from) })) }));
    }
    return completed;
}

/** Takes back the last copy, move, rename, deletion or new item (Ctrl+Z), or `step` if it's still to undo. */
export async function undo(step = undoSteps[undoSteps.length - 1]) {
    const at = step ? undoSteps.lastIndexOf(step) : -1;
    if (!step || at < 0) return void toast.message("Nothing to undo", { id: "undo" });
    undoSteps.splice(at, 1);
    try {
        await busy(`Undoing the ${step.label}…`, step.run());
        toast.success(`Undid the ${step.label}`, { id: "undo" });
    } catch (e) {
        toast.error(`Couldn't undo the ${step.label}`, { description: String(e), id: "undo" });
    }
    reload();
}

/** Shows a file (e.g. a text search result) in its folder here, in Files. */
export function revealInApp(path: string) {
    useStore.getState().setMode("files");
    return navigate(parentPath(path), { select: path });
}

/**
 * A dialog over the panes (batch rename, duplicates, X-Ray…) is open, so their keys mustn't reach the
 * list underneath: Delete there would recycle the selection. Quick Look is the exception, as its
 * arrows move through the list.
 */
export const modalOpen = () => !!document.querySelector('[aria-modal="true"]:not([data-quicklook])');

/** Opens what someone typed: "This PC", a folder, or a file (its folder, with it selected). */
export async function goTo(text: string) {
    const value = text.trim();
    if (!value || /^this pc$/i.test(value)) return navigate(THIS_PC);
    const tab = activeTabId();
    try {
        const target = await resolvePath(value);
        return target.dir ? navigate(target.path, { tab }) : navigate(parentPath(target.path), { select: target.path, tab });
    } catch (e) {
        toast.error(String(e));
        return false;
    }
}

export async function copyToClipboard(paths: string[], cut: boolean) {
    if (!paths.length) return;
    if (archiveOf()) {
        if (cut) return readOnlyNotice();
        // Extracted first: the clipboard carries files. Pasting (here or in File Explorer) copies them.
        return extracting(paths, "out", async (staged) => {
            try {
                await clipboardSetFiles(staged.paths, false);
                set({ cut: new Set() });
                toast.success(`Copied ${plural(staged.paths.length, "item")}`, { duration: 1500 });
            } catch (e) {
                toast.error("Couldn't use the clipboard", { description: String(e) });
            }
        });
    }
    try {
        await clipboardSetFiles(paths, cut);
        set({ cut: new Set(cut ? paths : []) });
        toast.success(`${cut ? "Cut" : "Copied"} ${plural(paths.length, "item")}`, { duration: 1500 });
    } catch (e) {
        toast.error("Couldn't use the clipboard", { description: String(e) });
    }
}

/** Pastes files copied or cut here or in File Explorer into `dest` (default: the open folder). */
export async function paste(dest = currentPath()) {
    if (dest === THIS_PC) return;
    const tab = activeTabId();
    if (samePath(dest, tabState(tab)?.path ?? THIS_PC) && archiveOf(tab)) return readOnlyNotice();
    let files: Awaited<ReturnType<typeof clipboardGetFiles>>;
    try {
        files = await clipboardGetFiles();
    } catch (e) {
        toast.error("Couldn't read the clipboard", { description: String(e) });
        return;
    }
    if (!files?.paths.length) {
        toast.message("Nothing to paste", { description: "Copy or cut files first, here or in File Explorer." });
        return;
    }
    try {
        const label = `${files.cut ? "Moving" : "Copying"} ${plural(files.paths.length, "item")}…`;
        const done = await busy(label, fileOp({ kind: files.cut ? "move" : "copy", sources: files.paths, dest }));
        if (done && files.cut) {
            set({ cut: new Set() });
            clipboardClear().catch(() => {});
        }
        const t = tabState(tab);
        if (t && samePath(dest, t.path) && !t.find) {
            await navigate(dest, { history: false, quiet: true, select: files.paths.map((p) => joinPath(dest, baseName(p))), tab });
        }
    } catch (e) {
        toast.error("Couldn't paste", { description: String(e) });
    }
}

/** Files dropped in from outside: moved within a drive, copied across drives, as Explorer does. */
export async function dropFiles(paths: string[], dest: string) {
    const sources = paths.filter((p) => !samePath(parentPath(p), dest) && !samePath(p, dest));
    if (!sources.length) return;
    const root = (p: string) => crumbs(p)[0]?.path.toLowerCase();
    const move = sources.every((p) => root(p) === root(dest));
    const what = plural(sources.length, "item");
    try {
        const done = await busy(`${move ? "Moving" : "Copying"} ${what}…`, fileOp({ kind: move ? "move" : "copy", sources, dest }));
        if (!done) return;
        const s = get();
        const showing = visibleTabs(s).find((id) => {
            const t = s.tabs[id];
            return t && !t.find && t.path !== THIS_PC && samePath(dest, t.path);
        });
        if (showing) await navigate(dest, { history: false, quiet: true, select: sources.map((p) => joinPath(dest, baseName(p))), tab: showing });
        else toast.success(`${move ? "Moved" : "Copied"} ${what} to “${baseName(dest)}”`);
    } catch (e) {
        toast.error(`Couldn't ${move ? "move" : "copy"} ${what}`, { description: String(e) });
    }
}

/** Moves to the Recycle Bin (or deletes for good), then selects `next` like Explorer does. */
export async function deletePaths(paths: string[], permanent: boolean, next?: string) {
    if (!paths.length) return;
    const tab = activeTabId();
    if (archiveOf(tab)) return readOnlyNotice();
    try {
        const confirm = useStore.getState().settings.confirmDelete;
        const done = await busy(`Deleting ${plural(paths.length, "item")}…`, fileOp({ kind: "delete", paths, permanent, confirm }));
        const t = tabState(tab);
        if (!t) return;
        if (t.find) {
            if (done) {
                const gone = new Set(paths);
                updateTab({ find: { ...t.find, entries: t.find.entries.filter((e) => !gone.has(e.path)) }, selection: new Set(next ? [next] : []), focus: next ?? null, anchor: next ?? null }, tab);
            }
        } else await navigate(t.path, { history: false, quiet: true, tab, ...(done ? { select: next ?? [] } : {}) });
    } catch (e) {
        toast.error("Couldn't delete", { description: String(e) });
    }
}

const INVALID_NAME = /[\\/:*?"<>|]/;
export const INVALID_NAME_HINT = 'A name can\'t contain any of these characters: \\ / : * ? " < > |';

/** Renames from the box in `tab`'s list (it can lose focus to another pane, so the tab is passed in). */
export async function renameEntry(entry: FileEntry, name: string, tab = activeTabId()) {
    updateTab({ renaming: null }, tab);
    if (isArchived(entry)) return readOnlyNotice();
    // Windows drops trailing dots and spaces itself.
    const trimmed = name.trim().replace(/[. ]+$/, "");
    if (!trimmed || trimmed === entry.name) return;
    if (INVALID_NAME.test(trimmed)) {
        toast.error(INVALID_NAME_HINT);
        return;
    }
    try {
        if (!(await fileOp({ kind: "rename", path: entry.path, name: trimmed }))) return;
        const renamed = joinPath(parentPath(entry.path), trimmed);
        const t = tabState(tab);
        if (!t) return;
        if (t.find) {
            const entries = t.find.entries.map((e) => (e === entry ? withDetails({ ...e, name: trimmed, path: renamed }) : e));
            updateTab({ find: { ...t.find, entries }, selection: new Set([renamed]), focus: renamed, anchor: renamed }, tab);
        } else await navigate(t.path, { history: false, quiet: true, select: renamed, tab });
    } catch (e) {
        toast.error(`Couldn't rename “${entry.name}”`, { description: String(e) });
    }
}

/** Creates "New folder" (or a new text file) in the open folder and starts renaming it. */
export async function newItem(dir: boolean) {
    const tab = activeTabId();
    const path = tabState(tab)?.path;
    if (!path || path === THIS_PC) return;
    if (archiveOf(tab)) return readOnlyNotice();
    try {
        const created = await createEntry(path, dir ? "New folder" : "New Text Document.txt", dir);
        rememberUndo(`new ${dir ? "folder" : "file"} “${baseName(created)}”`, () => performFileOp({ kind: "delete", paths: [created], permanent: false, confirm: false }));
        if (tabState(tab)?.find?.running) cancelFind(tab);
        updateTab({ find: null, filter: "" }, tab);
        await navigate(path, { history: false, quiet: true, select: created, tab });
        updateTab({ renaming: tabState(tab)?.focus ?? null }, tab);
    } catch (e) {
        toast.error(`Couldn't create a ${dir ? "folder" : "file"}`, { description: String(e) });
    }
}
