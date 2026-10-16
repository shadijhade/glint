import { Channel, invoke } from "@tauri-apps/api/core";
import { decodeListing, type FileEntry, type Listing, withDetails } from "./entries";
import { decodeUsage, type UsageMap } from "./usage";

export type { Listing };

/** One folder, read with a single Win32 enumeration pass and sorted folders-first in natural order. */
export const listDir = async (path: string): Promise<Listing> => decodeListing(await invoke<ArrayBuffer>("list_dir", { path }));

export interface Drive {
    /** Root, e.g. "C:\". */
    path: string;
    label: string;
    kind: "fixed" | "removable" | "network" | "cdrom" | "ram" | "unknown";
    fs: string;
    total: number;
    free: number;
}

const DEFAULT_LABEL: Record<Drive["kind"], string> = {
    fixed: "Local Disk",
    removable: "USB Drive",
    network: "Network Drive",
    cdrom: "DVD Drive",
    ram: "RAM Disk",
    unknown: "Drive",
};

/** "Local Disk (C:)", like Explorer. */
export const driveName = (drive: Drive) => `${drive.label || DEFAULT_LABEL[drive.kind]} (${drive.path.replace(/\\$/, "")})`;

export const listDrives = () => invoke<Drive[]>("list_drives");

/** Checks a typed location; a file resolves too, so its folder can be opened with it selected. */
export const resolvePath = (path: string) => invoke<{ path: string; dir: boolean }>("resolve_path", { path });

export const readTextPreview = (path: string, maxBytes: number) =>
    invoke<{ text: string; truncated: boolean; binary: boolean }>("read_text_preview", { path, maxBytes });

/** Creates a folder or empty file, numbered like "New folder (2)" if the name is taken; returns its path. */
export const createEntry = (parent: string, name: string, dir: boolean) => invoke<string>("create_entry", { parent, name, dir });

export interface FindOptions {
    hidden: boolean;
    respectIgnore: boolean;
    exclude: string[];
    maxResults: number | null;
}

export type FindEvent =
    /** Who answers: "index", "fd", or "index + fd" when the index covers only some folders. */
    | { type: "started"; engine: string }
    | { type: "entries"; entries: FileEntry[] }
    | { type: "error"; message: string }
    | { type: "finished"; count: number; elapsed: number; limited: boolean };

/**
 * Searches file names under `roots` (with the index when it runs, else fd), streaming results with
 * metadata. Each tab searches in its own `lane`: a new search stops only that tab's last one.
 */
export async function findFiles(roots: string[], query: string, options: FindOptions, lane: string, onEvent: (event: FindEvent) => void) {
    const channel = new Channel<FindEvent>();
    channel.onmessage = (event) => {
        if (event.type === "entries") for (const entry of event.entries) withDetails(entry);
        onEvent(event);
    };
    await invoke("find_files", { roots, query, options, onEvent: channel, lane });
}

export const cancelFind = (lane: string) => invoke("cancel_find", { lane });

/** Who measured a folder: the index, dust, or (in an archive) the archive's own list of entries. */
export type SizeEngine = "index" | "dust" | "zip" | "7z";

export interface FolderSizes {
    path: string;
    total: number;
    /** Subfolder name → bytes. */
    sizes: [string, number][];
    elapsed: number;
    engine: SizeEngine;
}

/**
 * Sizes of every subfolder: from the index when it has the folder, else calculated by dust (unless
 * `indexOnly`, which rejects instead). Rejects with "cancelled" when superseded in its `lane` (the
 * tab asking) or stopped.
 */
export const folderSizes = (path: string, lane: string, indexOnly = false) => invoke<FolderSizes>("folder_sizes", { path, indexOnly, lane });
export const cancelFolderSizes = (lane: string) => invoke("cancel_folder_sizes", { lane });

/**
 * A disk-usage map of `path`: its biggest files and folders (at most `maxItems`), `depth` levels down,
 * from the index when it has the folder, else measured by dust. Each tab maps in its own `lane`;
 * rejects with "cancelled" when superseded there or stopped.
 */
export const diskUsage = async (path: string, depth: number, maxItems: number, lane: string): Promise<UsageMap> =>
    decodeUsage(await invoke<ArrayBuffer>("disk_usage", { path, depth, maxItems, lane }));
export const cancelDiskUsage = (lane: string) => invoke("cancel_disk_usage", { lane });

export interface ThumbRequest {
    path: string;
    /** Device pixels, the most on either side. */
    size: number;
    /** The file's date and size from its listing: a file that changed gets a new thumbnail. */
    modified: number;
    bytes: number;
    /** Only from Windows' cache (an online-only file, which making a thumbnail would download). */
    cachedOnly: boolean;
    /** The tab asking, and the request's number in it. */
    lane: string;
    seq: number;
}

/**
 * One item's thumbnail from Windows' thumbnail cache, else made from the file, as binary (read it with
 * `decodeThumb`). Rejects with "cancelled" once dropped by `cancelThumbnails`.
 */
export const thumbnail = (request: ThumbRequest) => invoke<ArrayBuffer>("thumbnail", { ...request });

/** Drops a tab's thumbnail requests up to number `upTo`: for `paths`, or all of them (null). */
export const cancelThumbnails = (lane: string, paths: string[] | null, upTo: number) => invoke("cancel_thumbnails", { lane, paths, upTo });

/** Stops every running rg, fd and dust, in every tab (before an update replaces them). */
export const stopTools = () => invoke("stop_tools");

export type FileOp =
    | { kind: "copy" | "move"; sources: string[]; dest: string }
    | { kind: "delete"; paths: string[]; permanent: boolean; confirm: boolean }
    | { kind: "rename"; path: string; name: string };

/** Runs through Windows' own file engine (progress, conflicts, Recycle Bin). False if cancelled. */
export const fileOp = (op: FileOp) => invoke<boolean>("file_op", { op });

export const clipboardSetFiles = (paths: string[], cut: boolean) => invoke("clipboard_set_files", { paths, cut });
export const clipboardGetFiles = () => invoke<{ paths: string[]; cut: boolean } | null>("clipboard_get_files");
export const clipboardClear = () => invoke("clipboard_clear");

/**
 * Drags files out to other apps; call while the mouse button is down. Resolves when dropped or
 * cancelled, or "released" when the button was up before it could start. `copyOnly` offers a copy
 * and nothing else (files extracted from an archive).
 */
export const startDrag = (paths: string[], copyOnly = false) => invoke<"copy" | "move" | "link" | "none" | "released">("start_drag", { paths, copyOnly });

/**
 * A file in a zip or 7z archive extracted to a read-only temp copy, made once and reused: to
 * preview it (within a size budget) or to open it. Resolves to the copy's path.
 */
export const archiveExtract = (path: string, purpose: "preview" | "open") => invoke<string>("archive_extract", { path, purpose });

export interface Staged {
    /** The staging folder, to delete once its contents are copied (`archiveUnstage`). */
    folder: string;
    /** The extracted copy of each item asked for that could be extracted, in order. */
    paths: string[];
    /** Files that couldn't be (password-protected, damaged…), with the reason. */
    skipped: { path: string; reason: string }[];
    bytes: number;
}

/**
 * Extracts files and folders from archives into a staging folder for Windows' file engine to copy
 * from: "copy" for a copy made here (delete it after), "out" for the clipboard or a drag (Glint
 * deletes those once they're off the clipboard). Each tab extracts in its own `lane`; rejects with
 * "cancelled" when stopped.
 */
export async function archiveStage(paths: string[], purpose: "copy" | "out", lane: string, onProgress: (done: number, total: number) => void) {
    const channel = new Channel<{ done: number; total: number }>();
    channel.onmessage = ({ done, total }) => onProgress(done, total);
    return invoke<Staged>("archive_stage", { paths, purpose, lane, onProgress: channel });
}

export const cancelArchiveStage = (lane: string) => invoke("cancel_archive_stage", { lane });
export const archiveUnstage = (folder: string) => invoke("archive_unstage", { folder });

export const showProperties = (path: string) => invoke("show_properties", { path });
export const openWith = (path: string) => invoke("open_with", { path });
export const openTerminal = (path: string) => invoke("open_terminal", { path });

/** Watches exactly these folders (the ones on screen); changes arrive as "dir-changed" events carrying the folder's path. */
export const watchDirs = (paths: string[]) => invoke("watch_dirs", { paths });

/** A folder handed over from outside (File Explorer, a double-click, the command line), and the file to select in it. */
export interface LaunchTarget {
    path: string;
    select: string | null;
}

/** Folders waiting to open, each in a new tab: the command line's at start, then each later launch's ("open-requested" says more came). */
export const takeLaunchTargets = () => invoke<LaunchTarget[]>("take_launch_targets");

/** File Explorer's "Open in Glint", and opening folders here when they're double-clicked. */
export interface ExplorerIntegration {
    /** Only a copy the setup program or Scoop installed can turn them on. */
    available: boolean;
    menu: boolean;
    byDefault: boolean;
}

export const explorerIntegration = () => invoke<ExplorerIntegration>("explorer_integration");
/** Opening folders by default needs the menu entries: the folder's entry becomes its default action. */
export const setExplorerIntegration = (menu: boolean, byDefault: boolean) => invoke<ExplorerIntegration>("set_explorer_integration", { menu, byDefault });

/** One drive in the instant search index. */
export interface IndexVolume {
    /** "C:" */
    volume: string;
    state: "scanning" | "ready" | "error";
    files: number;
    folders: number;
    /** Bytes of memory the drive's index takes. */
    memory: number;
    /** "scan" (read the MFT) or "snapshot" (loaded from disk), and how long it took. */
    source: string;
    buildMs: number;
    progress: number;
    progressTotal: number;
    /** When the last change was applied, ms since the Unix epoch (0 if none yet). */
    updated: number;
    error: string | null;
}

/** Instant search: the MFT index service. */
export interface IndexInfo {
    state: "on" | "off" | "stopped" | "noAccess" | "untrusted" | "unsupported";
    /** The running service's version, and the app's (an update is due when they differ). */
    version: string | null;
    appVersion: string;
    volumes: IndexVolume[];
    error: string | null;
}

export const indexStatus = () => invoke<IndexInfo>("index_status");
/** Installs and starts the service after Windows asks for administrator approval; rejects with "cancelled" if declined. */
export const indexEnable = () => invoke<IndexInfo>("index_enable");
/** Stops and removes the service and its index files (administrator approval again). */
export const indexDisable = () => invoke<IndexInfo>("index_disable");
