import { create } from "zustand";
import { toast } from "sonner";
import { openPath } from "@tauri-apps/plugin-opener";
import { audioDir, desktopDir, documentDir, downloadDir, homeDir, pictureDir, videoDir } from "@tauri-apps/api/path";
import { type FileEntry, withDetails } from "./entries";
import {
    cancelFind,
    cancelFolderSizes,
    clipboardClear,
    clipboardGetFiles,
    clipboardSetFiles,
    createEntry,
    type Drive,
    fileOp,
    findFiles,
    folderSizes,
    indexDisable,
    indexEnable,
    type IndexInfo,
    indexStatus,
    listDir,
    listDrives,
    type Listing,
    watchDir,
} from "./fs";
import { baseName, crumbs, joinPath, parentPath, samePath, THIS_PC } from "./paths";
import { useStore } from "./store";

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
    engine: "index" | "dust";
}

interface ExplorerState {
    /** False until the first folder has opened (startup doesn't go into Back history). */
    ready: boolean;
    path: string;
    listing: Listing | null;
    loading: boolean;
    /** From the click to the new list on screen, ms. */
    shownIn: number;
    back: string[];
    forward: string[];
    drives: Drive[];
    known: KnownFolder[];
    /** Text in the search box: filters what's shown; Enter searches subfolders by name. */
    filter: string;
    find: FindRun | null;
    selection: ReadonlySet<string>;
    anchor: string | null;
    focus: string | null;
    renaming: string | null;
    editingAddress: boolean;
    sizes: Record<string, SizeRun>;
    /** Paths cut to the clipboard here, shown dimmed until pasted. */
    cut: ReadonlySet<string>;
    /** Folder that files dragged in from outside would land in. */
    dropTarget: string | null;
    /** Instant search's service, once asked. */
    index: IndexInfo | null;
}

export const useExplorer = create<ExplorerState>(() => ({
    ready: false,
    path: THIS_PC,
    listing: null,
    loading: false,
    shownIn: 0,
    back: [],
    forward: [],
    drives: [],
    known: [],
    filter: "",
    find: null,
    selection: new Set(),
    anchor: null,
    focus: null,
    renaming: null,
    editingAddress: false,
    sizes: {},
    cut: new Set(),
    dropTarget: null,
    index: null,
}));

const get = useExplorer.getState;
const set = useExplorer.setState;
const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

/** Shows `label` while slow work runs (quick operations finish before it appears). */
async function busy<T>(label: string, work: Promise<T>): Promise<T> {
    let id: string | number | undefined;
    const timer = setTimeout(() => (id = toast.loading(label)), 500);
    try {
        return await work;
    } finally {
        clearTimeout(timer);
        if (id !== undefined) toast.dismiss(id);
    }
}

let navSeq = 0;
let navStarted = 0;

interface NavOptions {
    /** Paths to select once the folder is open. */
    select?: string | string[];
    /** Record in Back/Forward history (default true). */
    history?: boolean;
    /** Keep the current list on screen while loading, as a refresh does. */
    quiet?: boolean;
    /** Don't report failures; the caller handles them. */
    silent?: boolean;
}

/** Opens a folder (or This PC). Resolves false if it couldn't, or a newer navigation won. */
export async function navigate(target: string, { select, history = true, quiet = false, silent = false }: NavOptions = {}) {
    const seq = ++navSeq;
    const started = performance.now();
    if (!quiet) set({ loading: true });
    let listing: Listing | null = null;
    if (target !== THIS_PC) {
        try {
            listing = await listDir(target);
        } catch (e) {
            if (seq === navSeq) {
                set({ loading: false });
                if (!silent) toast.error(`Can't open “${baseName(target)}”`, { description: String(e) });
            }
            return false;
        }
    }
    if (seq !== navSeq) return false;

    const state = get();
    const path = listing?.path ?? THIS_PC;
    const same = state.ready && (path === THIS_PC ? state.path === THIS_PC : state.path !== THIS_PC && samePath(path, state.path));
    // Select what was asked for; on a refresh keep the selection; going up selects where we came from.
    let wanted: string[] = [];
    if (select !== undefined) wanted = [select].flat();
    else if (same) wanted = [...state.selection];
    else if (state.path !== THIS_PC && samePath(parentPath(state.path), path)) wanted = [state.path];
    let selection: string[] = [];
    if (wanted.length && listing) {
        const byLower = new Map(listing.entries.map((e) => [e.path.toLowerCase(), e.path]));
        selection = wanted.flatMap((p) => byLower.get(p.replace(/[\\/]+$/, "").toLowerCase()) ?? []);
    }
    const focus = same && state.focus && selection.includes(state.focus) ? state.focus : (selection[selection.length - 1] ?? null);

    if (!same && state.find?.running) cancelFind();
    navStarted = started;
    set({
        ready: true,
        path,
        listing,
        loading: false,
        selection: new Set(selection),
        focus,
        anchor: focus,
        ...(same ? {} : { filter: "", find: null, renaming: null, editingAddress: false }),
        ...(history && state.ready && !same ? { back: [...state.back, state.path].slice(-100), forward: [] } : {}),
    });
    if (!same) {
        watchDir(listing ? path : null).catch(() => {});
        useStore.getState().setLastFolder(path);
        if (path === THIS_PC) refreshDrives();
    }
    // With the index, every folder shows its subfolders' sizes as it opens (and after each refresh).
    if (listing && indexed(path)) autoSizes();
    return true;
}

/** Called once the new list is on screen, to time the whole navigation. */
export function markShown() {
    if (navStarted) set({ shownIn: performance.now() - navStarted });
    navStarted = 0;
}

export async function goBack() {
    const { back, forward, path } = get();
    if (!back.length) return;
    if (await navigate(back[back.length - 1], { history: false })) set({ back: back.slice(0, -1), forward: [path, ...forward] });
}

export async function goForward() {
    const { back, forward, path } = get();
    if (!forward.length) return;
    if (await navigate(forward[0], { history: false })) set({ back: [...back, path], forward: forward.slice(1) });
}

export const goUp = () => {
    const { path } = get();
    return path === THIS_PC ? Promise.resolve(false) : navigate(parentPath(path));
};

/** Re-reads the open folder in place; if it's gone, opens its parent. */
export async function reload() {
    const { path } = get();
    if (path === THIS_PC) return refreshDrives();
    const seq = navSeq + 1;
    // A false result from a navigation that was overtaken by a newer one isn't a missing folder.
    if (!(await navigate(path, { history: false, quiet: true, silent: true })) && navSeq === seq) {
        toast.message(`“${baseName(path)}” is no longer available`);
        await navigate(parentPath(path), { history: false });
    }
}

/** F5: runs the search again when one is shown, otherwise re-reads the folder. */
export const refresh = () => {
    const { find } = get();
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

let initialized = false;

export async function initExplorer() {
    if (initialized) return;
    initialized = true;
    // Drives fill in when they answer; a sleeping network drive mustn't hold up the first folder.
    refreshDrives();
    // Asked first (briefly), so the first folder can already show its sizes from the index.
    await Promise.race([refreshIndex(), new Promise((resolve) => setTimeout(resolve, 300))]);
    await loadKnownFolders();
    const { settings, lastFolder } = useStore.getState();
    const home = get().known.find((k) => k.id === "home")?.path ?? THIS_PC;
    const start = settings.startFolder === "thispc" ? THIS_PC : settings.startFolder === "last" && lastFolder !== null ? lastFolder : home;
    if (!(await navigate(start, { silent: start !== home })) && !(await navigate(home))) await navigate(THIS_PC);
}

let findSeq = 0;

/** Searches the open folder and its subfolders (or every drive from This PC) by name: index, else fd. */
export async function runFind(query: string) {
    if (!query.trim()) return clearFind();
    const { path, drives } = get();
    const settings = useStore.getState().settings;
    const id = ++findSeq;
    const roots = path === THIS_PC ? drives.filter((d) => d.kind === "fixed" || d.kind === "removable").map((d) => d.path) : [path];
    const started = performance.now();
    set({
        find: { id, query, engine: "", scope: path, entries: [], running: true, elapsed: 0, limited: false, error: "" },
        selection: new Set(),
        focus: null,
        anchor: null,
        renaming: null,
    });
    const update = (patch: Partial<FindRun>) => {
        const find = get().find;
        if (find?.id === id) set({ find: { ...find, ...patch } });
    };
    const options = {
        hidden: settings.showHidden,
        respectIgnore: settings.findRespectIgnore,
        exclude: settings.findExclusions,
        maxResults: settings.findMaxResults,
    };
    try {
        await findFiles(roots, query, options, (event) => {
            const find = get().find;
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
    if (get().find?.running) cancelFind();
};

/** Leaves search results (and the filter) for the folder itself. */
export function clearFind() {
    const { find } = get();
    if (find?.running) cancelFind();
    set({ filter: "", ...(find ? { find: null, selection: new Set(), focus: null, anchor: null } : {}) });
    if (find) reload();
}

/** Sizes of every subfolder of the open folder: from the index when it has the folder, else with dust. */
export const calcSizes = () => sizeFolder(false);

/** As a folder opens: from the index only, and quietly nothing when it can't answer. */
const autoSizes = () => sizeFolder(true);

async function sizeFolder(auto: boolean) {
    const { path } = get();
    if (path === THIS_PC) return;
    const previous = get().sizes[path];
    const save = (run: SizeRun | null) => {
        const { [path]: _, ...rest } = get().sizes;
        set({ sizes: run ? { ...rest, [path]: run } : rest });
    };
    // The index answers in a millisecond or two: no "calculating…" flash for it.
    if (!auto) save({ sizes: previous?.sizes ?? new Map(), total: previous?.total ?? 0, elapsed: previous?.elapsed ?? 0, running: true, engine: previous?.engine ?? "dust" });
    try {
        const result = await folderSizes(path, auto);
        save({ sizes: new Map(result.sizes), total: result.total, elapsed: result.elapsed, running: false, engine: result.engine });
    } catch (e) {
        if (auto) return;
        save(previous ? { ...previous, running: false } : null);
        if (String(e) !== "cancelled") toast.error("Couldn't calculate folder sizes", { description: String(e) });
    }
}

// Instant search

let indexPoll: ReturnType<typeof setTimeout> | undefined;

/** Asks the service how it's doing; while a drive is being read, asks again every second. */
export async function refreshIndex() {
    clearTimeout(indexPoll);
    try {
        const { path } = get();
        const before = indexed(path);
        const index = await indexStatus();
        set({ index });
        // The open folder's drive just became ready: show its sizes without waiting for a navigation.
        if (!before && path === get().path && indexed(path)) autoSizes();
        if (index.state === "on" && index.volumes.some((v) => v.state === "scanning")) indexPoll = setTimeout(refreshIndex, 1000);
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

/** Turns instant search on (or updates it, or allows this user); Windows asks for administrator approval. */
export async function enableIndex() {
    try {
        set({ index: await busy("Turning on instant search…", indexEnable()) });
        refreshIndex();
        toast.success("Instant search is on", { description: "Each drive is searched with the index as soon as it has been read." });
    } catch (e) {
        if (String(e) !== "cancelled") toast.error("Couldn't turn on instant search", { description: String(e) });
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

export const stopSizes = () => cancelFolderSizes();

// Selection

export function selectOnly(path: string | null) {
    set({ selection: new Set(path ? [path] : []), focus: path, anchor: path });
}

export function toggleSelected(path: string) {
    const selection = new Set(get().selection);
    if (!selection.delete(path)) selection.add(path);
    set({ selection, focus: path, anchor: path });
}

/** Shift-selection from the anchor to `to`; with `add` it extends the selection instead of replacing it. */
export function selectRange(rows: FileEntry[], to: number, add = false) {
    const { anchor, selection } = get();
    const at = anchor === null ? -1 : rows.findIndex((r) => r.path === anchor);
    const from = at < 0 ? to : at;
    const next = new Set(add ? selection : []);
    for (let i = Math.min(from, to); i <= Math.max(from, to); i++) next.add(rows[i].path);
    set({ selection: next, focus: rows[to].path, anchor: rows[from].path });
}

export function selectAll(rows: FileEntry[]) {
    set({ selection: new Set(rows.map((r) => r.path)) });
}

// Actions on files

export function openEntry(entry: FileEntry) {
    if (entry.dir) return navigate(entry.path);
    return openPath(entry.path).catch((e) => toast.error(`Couldn't open “${entry.name}”`, { description: String(e) }));
}

/** Shows a file (e.g. a text search result) in its folder here. */
export const revealInApp = (path: string) => navigate(parentPath(path), { select: path });

export async function copyToClipboard(paths: string[], cut: boolean) {
    if (!paths.length) return;
    try {
        await clipboardSetFiles(paths, cut);
        set({ cut: new Set(cut ? paths : []) });
        toast.success(`${cut ? "Cut" : "Copied"} ${plural(paths.length, "item")}`, { duration: 1500 });
    } catch (e) {
        toast.error("Couldn't use the clipboard", { description: String(e) });
    }
}

/** Pastes files copied or cut here or in File Explorer into `dest` (default: the open folder). */
export async function paste(dest = get().path) {
    if (dest === THIS_PC) return;
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
        if (samePath(dest, get().path) && !get().find) {
            await navigate(dest, { history: false, quiet: true, select: files.paths.map((p) => joinPath(dest, baseName(p))) });
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
        if (samePath(dest, get().path) && !get().find) {
            await navigate(dest, { history: false, quiet: true, select: sources.map((p) => joinPath(dest, baseName(p))) });
        } else toast.success(`${move ? "Moved" : "Copied"} ${what} to “${baseName(dest)}”`);
    } catch (e) {
        toast.error(`Couldn't ${move ? "move" : "copy"} ${what}`, { description: String(e) });
    }
}

/** Moves to the Recycle Bin (or deletes for good), then selects `next` like Explorer does. */
export async function deletePaths(paths: string[], permanent: boolean, next?: string) {
    if (!paths.length) return;
    try {
        const confirm = useStore.getState().settings.confirmDelete;
        const done = await busy(`Deleting ${plural(paths.length, "item")}…`, fileOp({ kind: "delete", paths, permanent, confirm }));
        const { find } = get();
        if (find) {
            if (done) {
                const gone = new Set(paths);
                set({ find: { ...find, entries: find.entries.filter((e) => !gone.has(e.path)) }, selection: new Set(next ? [next] : []), focus: next ?? null, anchor: next ?? null });
            }
        } else await navigate(get().path, { history: false, quiet: true, ...(done ? { select: next ?? [] } : {}) });
    } catch (e) {
        toast.error("Couldn't delete", { description: String(e) });
    }
}

const INVALID_NAME = /[\\/:*?"<>|]/;
export const INVALID_NAME_HINT = 'A name can\'t contain any of these characters: \\ / : * ? " < > |';

export async function renameEntry(entry: FileEntry, name: string) {
    set({ renaming: null });
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
        const { find } = get();
        if (find) {
            const entries = find.entries.map((e) => (e === entry ? withDetails({ ...e, name: trimmed, path: renamed }) : e));
            set({ find: { ...find, entries }, selection: new Set([renamed]), focus: renamed, anchor: renamed });
        } else await navigate(get().path, { history: false, quiet: true, select: renamed });
    } catch (e) {
        toast.error(`Couldn't rename “${entry.name}”`, { description: String(e) });
    }
}

/** Creates "New folder" (or a new text file) in the open folder and starts renaming it. */
export async function newItem(dir: boolean) {
    const { path } = get();
    if (path === THIS_PC) return;
    try {
        const created = await createEntry(path, dir ? "New folder" : "New Text Document.txt", dir);
        stopFind();
        set({ find: null, filter: "" });
        await navigate(path, { history: false, quiet: true, select: created });
        set({ renaming: get().focus });
    } catch (e) {
        toast.error(`Couldn't create a ${dir ? "folder" : "file"}`, { description: String(e) });
    }
}
