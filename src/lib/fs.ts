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
/** Where Text search's ripgrep comes from: next to the app, installed on the PC, or nowhere. */
export const ripgrepStatus = () => invoke<"bundled" | "installed" | "missing">("ripgrep_status");
export const installRipgrep = () => invoke<void>("install_ripgrep");
/** Versions, tools and instant search's state, as text for a bug report. */
export const diagnostics = () => invoke<string>("diagnostics");

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

/** Settings → Quick search: the shortcut (null: off) and whether closing the window keeps Glint in the tray. */
export const quickConfigure = (shortcut: string | null, trayIcon: boolean) => invoke("quick_configure", { shortcut, trayIcon });
/** From the quick search window: shows the item in Glint (a folder opened, a file selected) in a new tab. */
export const quickReveal = (path: string, dir: boolean) => invoke("quick_reveal", { path, dir });
export const quickHide = () => invoke("quick_hide");
/** Starts Glint in the tray when this user signs in, or stops doing so. */
export const setStartWithWindows = (enabled: boolean) => invoke("set_start_with_windows", { enabled });

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
    | { kind: "rename"; path: string; name: string }
    /** Undoing a move: each item back into `folder` as `name`. */
    | { kind: "moveBack"; items: { path: string; folder: string; name: string }[] }
    /** Undoing a delete: Recycle Bin items (as a deletion's `to` names them) back where they were. */
    | { kind: "restore"; items: { path: string; folder: string; name: string }[] };

/**
 * What became of each item asked for: where it is now (the copy, the item moved or renamed, its
 * Recycle Bin entry), or null when it's gone for good or was skipped. `completed` is false when the
 * person cancelled in one of Windows' dialogs.
 */
export interface Performed {
    completed: boolean;
    items: { from: string; to: string | null }[];
}

/** Runs through Windows' own file engine (progress, conflicts, Recycle Bin). */
export const fileOp = (op: FileOp) => invoke<Performed>("file_op", { op });

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
export type Progress = (done: number, total: number) => void;

function progressChannel(onProgress: Progress) {
    const channel = new Channel<{ done: number; total: number }>();
    channel.onmessage = ({ done, total }) => onProgress(done, total);
    return channel;
}

export const archiveStage = (paths: string[], purpose: "copy" | "out", lane: string, onProgress: Progress) =>
    invoke<Staged>("archive_stage", { paths, purpose, lane, onProgress: progressChannel(onProgress) });

/** Compress to zip: the zip, numbered if `name` is taken in `folder`, is `paths[0]` of what comes back. */
export const archiveCompress = (paths: string[], folder: string, name: string, lane: string, onProgress: Progress) =>
    invoke<Staged>("archive_compress", { paths, folder, name, lane, onProgress: progressChannel(onProgress) });

/** Extract here: into a new folder next to the archive, named after it; it's `folder` of what comes back. */
export const archiveExtractHere = (path: string, lane: string, onProgress: Progress) =>
    invoke<Staged>("archive_extract_here", { path, lane, onProgress: progressChannel(onProgress) });

export const cancelArchiveStage = (lane: string) => invoke("cancel_archive_stage", { lane });
export const archiveUnstage = (folder: string) => invoke("archive_unstage", { folder });

export const showProperties = (path: string) => invoke("show_properties", { path });
export const openWith = (path: string) => invoke("open_with", { path });
export const openTerminal = (path: string) => invoke("open_terminal", { path });

export interface LockingProcess {
    pid: number;
    name: string;
    service: string | null;
    kind: "app" | "service" | "explorer" | "console" | "critical";
    exe: string | null;
    stoppable: boolean;
    startTimeLow: number;
    startTimeHigh: number;
}

export interface LockReport {
    paths: string[];
    processes: LockingProcess[];
    filesChecked: number;
    elapsedMs: number;
}

/** Queries Windows Restart Manager for processes holding open handles on `paths` (or files inside them). */
export const whoLocks = (paths: string[]) => invoke<LockReport>("who_locks", { paths });
/** Terminates a locking process after verifying its creation timestamp matches `startTimeLow`/`startTimeHigh`. */
export const stopLockingProcess = (pid: number, startTimeLow: number, startTimeHigh: number) =>
    invoke<void>("stop_locking_process", { pid, startTimeLow, startTimeHigh });

export interface PeSection {
    name: string;
    virtualSize: number;
    virtualAddress: number;
    rawSize: number;
    rawOffset: number;
    flags: string[];
}

export interface PeImportModule {
    module: string;
    symbols: string[];
    totalCount: number;
}

export interface PeExport {
    ordinal: number;
    name: string;
    rva: number;
}

export interface PeInfo {
    format: string;
    machine: string;
    subsystem: string;
    isDll: boolean;
    isDotnet: boolean;
    isSigned: boolean;
    timestampMs: number | null;
    linkerVersion: string;
    osVersion: string;
    entryPoint: number;
    imageBase: number;
    checksum: number;
    features: string[];
    sections: PeSection[];
    imports: PeImportModule[];
    exportCount: number;
    exports: PeExport[];
    elapsedMs: number;
}

/** Zero-copy PE32/PE32+ binary inspection for .exe, .dll, .sys, .ocx, .cpl, .scr, .efi and .node files. */
export const inspectPe = (path: string) => invoke<PeInfo>("inspect_pe", { path });

export interface DuplicateFile {
    path: string;
    name: string;
    modified: number;
}

export interface DuplicateGroup {
    hash: string;
    size: number;
    wastedBytes: number;
    files: DuplicateFile[];
}

export interface DedupReport {
    root: string;
    groups: DuplicateGroup[];
    totalWastedBytes: number;
    totalDuplicateFiles: number;
    filesScanned: number;
    sizeCandidates: number;
    edgeCandidates: number;
    hardlinksSkipped: number;
    elapsedMs: number;
}

export interface HardlinkResult {
    linked: number;
    reclaimedBytes: number;
    elapsedMs: number;
}

export const findDuplicates = (root: string, minBytes?: number) => invoke<DedupReport>("find_duplicates", { root, minBytes });
export const cancelDedup = () => invoke<void>("cancel_dedup");
export const hardlinkDuplicates = (keep: string, duplicates: string[]) => invoke<HardlinkResult>("hardlink_duplicates", { keep, duplicates });

export interface LiveFsEvent {
    path: string;
    name: string;
    kind: "created" | "modified" | "deleted";
    timestampMs: number;
    size: number | null;
    dir: boolean;
}

export interface TimelineEntry {
    path: string;
    name: string;
    folder: string;
    size: number;
    modified: number;
}

export interface HotSubfolder {
    path: string;
    name: string;
    bytesChanged: number;
    filesChanged: number;
}

export interface TimelineReport {
    root: string;
    windowMs: number;
    entries: TimelineEntry[];
    hotFolders: HotSubfolder[];
    liveEvents: LiveFsEvent[];
    totalBytesChanged: number;
    totalFilesChanged: number;
    filesScanned: number;
    /** It stopped looking after 150,000 files. */
    stoppedEarly: boolean;
    elapsedMs: number;
}

export const scanTimeline = (root: string, windowMs: number) => invoke<TimelineReport>("scan_timeline", { root, windowMs });
export const cancelTimeline = () => invoke<void>("cancel_timeline");

export interface SqliteObject {
    kind: "table" | "index" | "view" | "trigger";
    name: string;
    tableName: string;
    rootPage: number;
    sql: string;
}

export interface SqliteInfo {
    pageSize: number;
    pageCount: number;
    freelistPages: number;
    journalMode: string;
    encoding: string;
    sqliteVersion: string;
    userVersion: number;
    applicationId: number;
    objects: SqliteObject[];
    elapsedMs: number;
}

export interface TableColumn {
    name: string;
    kind: "number" | "boolean" | "text";
}

export interface TablePreview {
    format: string;
    delimiter: string;
    columns: TableColumn[];
    rows: string[][];
    totalRowsSampled: number;
    truncated: boolean;
    elapsedMs: number;
}

export const inspectSqlite = (path: string) => invoke<SqliteInfo>("inspect_sqlite", { path });
export const inspectTabular = (path: string, maxBytes = 256 * 1024) => invoke<TablePreview>("inspect_tabular", { path, maxBytes });

export interface RenamePair {
    from: string;
    to: string;
}

export interface BatchRenameResult {
    renamed: number;
    undoOps: RenamePair[];
    elapsedMs: number;
}

export const batchRename = (ops: RenamePair[]) => invoke<BatchRenameResult>("batch_rename", { ops });

export interface NtfsStream {
    name: string;
    cleanName: string;
    size: number;
    preview: string | null;
}

export interface ZoneIdentifier {
    zoneId: number;
    zoneLabel: string;
    hostUrl: string | null;
    referrerUrl: string | null;
}

export interface FileXRayReport {
    path: string;
    logicalSize: number;
    allocatedSize: number;
    hardlinkCount: number;
    fileIndex: string;
    volumeSerial: string;
    attributes: string[];
    streams: NtfsStream[];
    zoneInfo: ZoneIdentifier | null;
    entropy: number;
    entropyLabel: string;
    byteSpectrum: number[];
    nullBytePct: number;
    asciiPct: number;
    highBytePct: number;
    sha256: string;
    crc32: string;
    hashesComplete: boolean;
    elapsedMs: number;
}

/** Hashes the first 256 MB, or the whole file when `full`. */
export const inspectFileXRay = (path: string, full = false) => invoke<FileXRayReport>("inspect_file_xray", { path, full });
export const cancelXRay = () => invoke<void>("cancel_xray");
export const removeNtfsStream = (path: string, streamName: string) => invoke<void>("remove_ntfs_stream", { path, streamName });

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
