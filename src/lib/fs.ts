import { Channel, invoke } from "@tauri-apps/api/core";
import type { ArchiveFormat } from "./archives";
import { decodeItems, decodeListing, type DiffStatus, type FileEntry, type Listing } from "./entries";
import { type JsonValue, parseJson } from "./jsonTree";
import { decodeUsage, type UsageMap } from "./usage";

const utf8 = new TextDecoder();

export type { Listing };

/** What `listDir` rejects with: why the folder couldn't be listed. */
export interface ListFailure {
    /** "denied": Windows doesn't let this account open it; anything else is said in `message`. */
    kind: "denied" | "other";
    message: string;
    /** The folder as asked for, made absolute. */
    path: string;
    /** On another computer, which decides who may open it. */
    remote: boolean;
}

/** One folder, read with a single Win32 enumeration pass and sorted folders-first in natural order. Rejects with a ListFailure. */
export async function listDir(path: string): Promise<Listing> {
    try {
        return decodeListing(await invoke<ArrayBuffer>("list_dir", { path }));
    } catch (e) {
        // An Error too, nameless, so String(e) says the message where errors are shown that way.
        throw typeof e === "object" && e ? Object.assign(new Error((e as ListFailure).message), e, { name: "" }) : e;
    }
}

/**
 * Get access, as File Explorer's Continue button does: gives this account read access to a folder
 * and what's in it, for good (Windows asks for administrator approval unless it's theirs to change).
 * "refused": even administrators may not; "stillDenied": a rule in its permissions still denies this
 * account. Rejects with "cancelled" when the approval was declined.
 */
export const grantAccess = (path: string) => invoke<"granted" | "refused" | "stillDenied">("grant_access", { path });

/**
 * Opens a file as a double-click in File Explorer does (its app, or Windows' app picker when none
 * opens it yet). A shortcut to a folder resolves to that folder instead, for the tab to open.
 */
export const openItem = (path: string) => invoke<string | null>("open_item", { path });

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
/** A program Glint drives (tools.rs): one that comes with it, or an optional one installed on demand. */
export interface ToolStatus {
    id: string;
    name: string;
    usedFor: string;
    /** What Install uses: "bundled" (comes with Glint; winget can put it back), "winget" or "release" (its GitHub release). */
    source: "bundled" | "winget" | "release";
    /** Where the copy that runs came from; null when it's missing. */
    origin: "bundled" | "downloaded" | "installed" | null;
    path: string | null;
    version: string | null;
    /** The version a release download gets. */
    pinned: string | null;
    /** Glint downloaded this copy, so it can remove it (one installed with winget is the person's to uninstall). */
    removable: boolean;
}

/** How an install is going: what it's doing, and bytes when it's downloading (total 0: no bar). */
export interface ToolProgress {
    stage: string;
    done: number;
    total: number;
}

export const toolsStatus = () => invoke<ToolStatus[]>("tools_status");
/** Installs a tool with winget or from its release (checked against its pinned SHA-256); rejects with "cancelled" when stopped. */
export const toolInstall = (id: string, onProgress: (progress: ToolProgress) => void) => {
    const channel = new Channel<ToolProgress>();
    channel.onmessage = onProgress;
    return invoke<ToolStatus>("tool_install", { id, onProgress: channel });
};
export const toolCancelInstall = (id: string) => invoke("tool_cancel_install", { id });
/** Removes a copy of a tool Glint downloaded. */
export const toolRemove = (id: string) => invoke<ToolStatus>("tool_remove", { id });
/** Versions, tools and instant search's state, as text for a bug report. */
export const diagnostics = () => invoke<string>("diagnostics");

export const readTextPreview = (path: string, maxBytes: number) =>
    invoke<{ text: string; truncated: boolean; binary: boolean }>("read_text_preview", { path, maxBytes });

/** Markdown as HTML (GitHub's flavor, by comrak), with its own HTML in it for the preview to keep what's allowed. */
export const markdownHtml = (text: string) => invoke<string>("markdown_html", { text });

export interface JqAnswer {
    values: JsonValue[];
    /** Cut at 100,000 values or 64 MB of JSON. */
    truncated: boolean;
    /** Why there are no values, or why they stop: the file isn't its format, the filter doesn't compile, or it failed. */
    error: string | null;
    elapsed: number;
}

/**
 * A JSON, YAML or TOML file's documents through a jq filter ("." for the file as it is), by jaq in a
 * worker held to 1 GB and 10 seconds. A newer query, or jqStop, stops this one ("stopped").
 */
export async function jqQuery(path: string, format: "json" | "yaml" | "toml", filter: string): Promise<JqAnswer> {
    const bytes = new Uint8Array(await invoke<ArrayBuffer>("jq_query", { path, format, filter }));
    const line = bytes.indexOf(10);
    const head = JSON.parse(utf8.decode(bytes.subarray(0, line))) as Omit<JqAnswer, "values">;
    const values = parseJson(utf8.decode(bytes.subarray(line + 1)));
    return { ...head, values: Array.isArray(values) ? values : [] };
}

export const jqStop = () => invoke<void>("jq_stop");

/** Creates a folder or empty file, numbered like "New folder (2)" if the name is taken; returns its path. */
export const createEntry = (parent: string, name: string, dir: boolean) => invoke<string>("create_entry", { parent, name, dir });

export interface FindOptions {
    hidden: boolean;
    respectIgnore: boolean;
    exclude: string[];
    maxResults: number | null;
    /** When nothing matches as typed, the index looks again with typos allowed (fd doesn't). */
    fuzzy?: boolean;
}

export type FindEvent =
    /** Who answers: "index", "fd", or "index + fd" when the index covers only some folders. */
    | { type: "started"; engine: string }
    | { type: "entries"; entries: FileEntry[] }
    | { type: "error"; message: string }
    /** `fuzzy`: nothing matched as typed, and these are the index's close matches. */
    | { type: "finished"; count: number; elapsed: number; limited: boolean; fuzzy: boolean };

/**
 * Searches file names under `roots` (with the index when it runs, else fd), streaming results with
 * metadata. Each tab searches in its own `lane`: a new search stops only that tab's last one. Results
 * arrive as bytes (`encode_items` in fsx.rs), everything else as JSON.
 */
export async function findFiles(roots: string[], query: string, options: FindOptions, lane: string, onEvent: (event: FindEvent) => void) {
    const channel = new Channel<Exclude<FindEvent, { type: "entries" }> | ArrayBuffer>();
    channel.onmessage = (message) => onEvent(message instanceof ArrayBuffer ? { type: "entries", entries: decodeItems(message).entries } : message);
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

/** Who measured a folder: the index, dust, or (in an archive) the archive's own list of entries, by its format. */
export type SizeEngine = "index" | "dust" | ArchiveFormat;

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
/** A map of `path`; `since` (ms) also asks what each item held at the index's daily mark closest to then. */
export const diskUsage = async (path: string, depth: number, maxItems: number, lane: string, since?: number): Promise<UsageMap> =>
    decodeUsage(await invoke<ArrayBuffer>("disk_usage", { path, depth, maxItems, lane, since: since ?? null }));
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
    | { kind: "restore"; items: { path: string; folder: string; name: string }[] }
    /** Sync in compare panes: each item copied into `folder` as `name`. */
    | { kind: "copyInto"; items: { path: string; folder: string; name: string }[] };

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

/**
 * What Clean up looks for: build output and package caches beside their projects, empty folders, what programs leave behind, files of
 * `atLeast` bytes and up; and with Czkawka, look-alike pictures and videos (`alike`: how different they may be), music with the same
 * tags or sound, broken files, and files with the wrong extension.
 */
export type CleanupMode =
    | { mode: "junk" }
    | { mode: "empty" }
    | { mode: "temporary" }
    | { mode: "big"; atLeast: number }
    | { mode: "images"; alike: number }
    | { mode: "videos"; alike: number }
    | { mode: "music"; bySound: boolean }
    | { mode: "broken" }
    | { mode: "misnamed" };

/** Something Clean up found, to recycle (or give its real extension) if chosen. */
export interface CleanupItem {
    path: string;
    /** In bytes; a folder's is everything in it. */
    size: number;
    /** When it last changed (developer junk: when its project did, leaving out its build output, caches and .git); 0 when that couldn't be told. */
    date: number;
    /** What it is: "Rust builds", "Office owner files"…; "" for empty folders, big files and look-alikes. */
    kind: string;
    /** Look-alikes: the group it's in, from 1 (the biggest first in each); 0 otherwise. */
    group: number;
    /** A picture's size ("1920 × 1080"), a video's length, a song's tags, why a file doesn't open, what a misnamed file really is. */
    note: string;
    /** A misnamed file's right extension ("png"). */
    fix: string;
    /** A picture's width × height. */
    pixels: number;
}

export interface CleanupReport {
    /** Biggest first (empty folders: by path). */
    items: CleanupItem[];
    total: number;
    /** Who found and sized them: "index", "fd + dust", "index + fd"… */
    engine: string;
    elapsed: number;
    /** How many more were found than listed (the biggest are). */
    more: number;
    /** Temporary files left out because a program has them open. */
    held: number;
}

/** What Clean up's `mode` finds below `root`, in the tab's `lane`; rejects with "cancelled" when stopped. Nothing is removed. */
export const cleanupScan = (root: string, mode: CleanupMode, lane: string) => invoke<CleanupReport>("cleanup_scan", { root, mode, lane });
export const cancelCleanup = (lane: string) => invoke("cancel_cleanup", { lane });

/** What a name in a folder has in Git, weakest first: a folder shows the strongest of what's below it. */
export type GitMark = "ignored" | "untracked" | "staged" | "modified" | "conflicted";

/** Git in a folder: its repository's branch and upstream, and what changed in it (git.rs). */
export interface GitView {
    /** The repository's working folder. */
    root: string;
    /** The branch checked out; null when HEAD is detached. */
    branch: string | null;
    /** HEAD's commit, abbreviated; null before the first commit. */
    head: string | null;
    /** The branch it tracks ("origin/main"). */
    upstream: string | null;
    ahead: number;
    behind: number;
    /** What's right in the folder has, by lowercase name. */
    marks: Record<string, GitMark>;
    /** Changes in the folder and below it. */
    counts: { modified: number; staged: number; untracked: number; conflicted: number };
    /** Status ran out of time: what's shown is what it found by then. */
    partial: boolean;
    /** The repository's own settings would run programs: status waits until it's trusted. */
    limited: boolean;
    /** Too many files to look through on every visit (or on another computer): F5 shows status. */
    onRequest: boolean;
    elapsed: number;
}

/** Git in `folder` for the tab `lane` (null: not in a repository); `trusted` repositories may run programs their settings name; `force` (F5) runs status everywhere. */
export const gitView = (folder: string, lane: string, trusted: string[], force: boolean) => invoke<GitView | null>("git_view", { folder, lane, trusted, force });
export const cancelGit = (lane: string) => invoke("cancel_git", { lane });
/** Watches exactly the repositories at `roots` (each emits "git-changed" with its root). */
export const gitWatch = (roots: string[]) => invoke("git_watch", { roots });

/** A line of a diff: its number before and after (0 on the side it isn't on). */
export interface DiffLine {
    kind: "same" | "removed" | "added";
    old: number;
    new: number;
    text: string;
}

/** What changed in a file since the last commit: its version in HEAD against the one on disk. */
export interface FileDiff {
    /** Where it is in its repository ("src/lib/fs.ts"). */
    path: string;
    /** Not in the last commit: all of it is new. */
    newFile: boolean;
    binary: boolean;
    /** Over 1 MB on either side: sizes only. */
    tooBig: boolean;
    oldSize: number;
    newSize: number;
    added: number;
    removed: number;
    /** Changes with up to 3 lines around them; a jump in line numbers is a gap. */
    lines: DiffLine[];
    /** More changed than the 5,000 lines sent. */
    cut: boolean;
    elapsed: number;
}

/** What changed in the file at `path` since the last commit; null outside a repository. */
export const gitDiff = (path: string) => invoke<FileDiff | null>("git_diff", { path });

/** The file at `path` as it is in its last commit, in a temp file (null: it isn't in it); `gitHeadDiscard` removes it. */
export const gitHeadCopy = (path: string) => invoke<string | null>("git_head_copy", { path });
export const gitHeadDiscard = (path: string) => invoke("git_head_discard", { path });

/** Two text files side by side (filediff.rs). */
export interface TextCompare {
    left: string[];
    right: string[];
    /** The rows side by side, by line on each side (from 0); null where only the other side has one. */
    rows: [number | null, number | null][];
    /** The changed lines on each side (by line), with what changed in them: [start, end) in the line; none listed: all of it. */
    leftChanges: Record<string, [number, number][]>;
    rightChanges: Record<string, [number, number][]>;
    /** "difftastic" (by the code's structure) or "lines". */
    engine: string;
    /** The language Difftastic read them as. */
    language: string | null;
    /** Not text: compare the bytes instead. */
    binary: boolean;
    /** Over 8 MB: compare the bytes instead. */
    tooBig: boolean;
    elapsed: number;
}

export const compareTextFiles = (left: string, right: string) => invoke<TextCompare>("compare_text_files", { left, right });

/** A window of two files' bytes, and their sizes. */
export interface BytesWindow {
    leftSize: number;
    rightSize: number;
    left: Uint8Array;
    right: Uint8Array;
}

/** `len` bytes of each file from `offset` (at most 1 MB). */
export async function compareBytes(left: string, right: string, offset: number, len: number): Promise<BytesWindow> {
    const buffer = await invoke<ArrayBuffer>("compare_bytes", { left, right, offset, len });
    const view = new DataView(buffer);
    const [leftLen, rightLen] = [view.getUint32(16, true), view.getUint32(20, true)];
    return {
        leftSize: Number(view.getBigUint64(0, true)),
        rightSize: Number(view.getBigUint64(8, true)),
        left: new Uint8Array(buffer, 24, leftLen),
        right: new Uint8Array(buffer, 24 + leftLen, rightLen),
    };
}

export interface MediaDetails {
    /** Label and value, in the order the preview shows them. */
    rows: [string, string][];
    /** When it was taken (or recorded): milliseconds, the local time it says. */
    taken: number | null;
    camera: string | null;
    artist: string | null;
    album: string | null;
    title: string | null;
    track: number | null;
    /** Latitude and longitude. */
    place: [number, number] | null;
    /** A song's cover, as a data: URL. */
    cover: string | null;
}

/** A photo's, video's or song's details (media.rs); null for other files. */
export const mediaDetails = (path: string) => invoke<MediaDetails | null>("media_details", { path });

/** Many files' details at once, without covers. */
export const mediaDetailsMany = (paths: string[]) => invoke<(MediaDetails | null)[]>("media_details_many", { paths });

/** `len` bytes of a file from `offset` (at most 1 MB), and the file's size. */
export async function fileBytes(path: string, offset: number, len: number) {
    const buffer = await invoke<ArrayBuffer>("file_bytes", { path, offset, len });
    return { size: Number(new DataView(buffer).getBigUint64(0, true)), bytes: new Uint8Array(buffer, 8) };
}

/** The first offset at or after `from` where the files differ; null when they don't from there on. */
export const nextByteDifference = (left: string, right: string, from: number) => invoke<number | null>("next_byte_difference", { left, right, from });

/** Export as ncdu JSON: the folder `path` walked into `dest` (written as `dest.part` until done); progress counts entries, with no total. */
export const exportNcdu = (path: string, dest: string, lane: string, onProgress: Progress) =>
    invoke<Staged>("export_ncdu", { path, dest, lane, onProgress: progressChannel(onProgress) });

export const cancelArchiveStage = (lane: string) => invoke("cancel_archive_stage", { lane });
export const archiveUnstage = (folder: string) => invoke("archive_unstage", { folder });

/**
 * How far the first read of the archive `path` goes into has got: a compressed tar (or a .gz whose size
 * isn't stated) is read whole to list it. Null when nothing is being read.
 */
export const archiveProgress = (path: string) => invoke<{ path: string; done: number; total: number } | null>("archive_progress", { path });
/** Stops that read; the listing waiting for it rejects with "cancelled". */
export const cancelArchiveRead = (path: string) => invoke("cancel_archive_read", { path });

/** What the preview pane says about an archive. A big compressed stream isn't read just to say it: then entries and unpacked are null. */
export interface ArchivePeek {
    format: ArchiveFormat;
    /** Bytes on disk (or in the archive it's in). */
    packed: number;
    entries: number | null;
    unpacked: number | null;
}

export const archivePeek = (path: string) => invoke<ArchivePeek>("archive_peek", { path });

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

/** A file's Authenticode signature, as Windows checks it offline (sign.rs). */
export interface Signature {
    state: "valid" | "unsigned" | "invalid" | "untrusted" | "expired" | "revoked" | "unknown";
    /** Signed in a Windows catalog rather than in the file. */
    catalog: boolean;
    signer: string | null;
    issuer: string | null;
    /** When it was signed, by its timestamp (ms). */
    signedAt: number | null;
    /** What Windows said, when it doesn't hold. */
    detail: string | null;
}

export const verifySignature = (path: string) => invoke<Signature>("verify_signature", { path });

/** A file's type as Magika tells it from its content (xray.rs). */
export interface RealType {
    label: string;
    /** e.g. "PE Windows executable". */
    description: string;
    /** "executable", "code", "image", "document"… */
    group: string;
    mime: string;
    /** The extensions files of this type have. */
    extensions: string[];
    text: boolean;
    /** How sure it is, 0 to 1. */
    score: number;
}

/** What Magika says the file is; null when Magika isn't installed. */
export const fileRealType = (path: string) => invoke<RealType | null>("file_real_type", { path });

/** What a check found (check.rs): a YARA rule a file matched, or a secret in a file. */
export interface Finding {
    path: string;
    /** Where in the file (secrets); 0 for a whole file (YARA rules). */
    line: number;
    /** The rule that matched. */
    rule: string;
    /** A secret's kind in words, or a YARA rule's namespace. */
    title: string;
    /** The secret, masked. */
    value: string;
    /** "high", "medium" or "low" (secrets). */
    confidence: string;
}

export interface CheckReport {
    findings: Finding[];
    elapsedMs: number;
}

/** Scans `target` with the YARA rules in `rules` (YARA-X); rejects with "cancelled" when stopped. */
export const yaraScan = (rules: string, target: string) => invoke<CheckReport>("yara_scan", { rules, target });
/** Looks for passwords, keys and tokens in `target` (Kingfisher, offline); rejects with "cancelled" when stopped. */
export const secretsScan = (target: string) => invoke<CheckReport>("secrets_scan", { target });
export const cancelCheck = () => invoke("cancel_check");

/**
 * Kinds of file Authenticode signs: programs and drivers, installers and cabinets, catalogs, scripts
 * and app packages. Others are only signed in a catalog, which isn't worth hashing them for.
 */
const SIGNABLE = new Set(["exe", "dll", "sys", "ocx", "scr", "cpl", "efi", "drv", "mui", "node", "msi", "msp", "msm", "msix", "msixbundle", "appx", "appxbundle", "cab", "cat", "ps1", "psm1", "psd1", "ps1xml", "vbs", "js", "wsf"]);
export const signable = (path: string) => SIGNABLE.has(path.slice(path.lastIndexOf(".") + 1).toLowerCase()) && path.lastIndexOf(".") > path.lastIndexOf("\\");

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

/** Compare panes, deeper: each top-level item's status (by lowercase name) on either side, and what Sync copies each way. */
export interface FolderDiff {
    left: Record<string, DiffStatus>;
    right: Record<string, DiffStatus>;
    toRight: { path: string; folder: string }[];
    toLeft: { path: string; folder: string }[];
    files: number;
    elapsedMs: number;
}

export const compareFolders = (left: string, right: string, subfolders: boolean, contents: boolean, hidden: boolean, exclude: string[]) =>
    invoke<FolderDiff>("compare_folders", { left, right, subfolders, contents, hidden, exclude });
export const cancelCompare = () => invoke<void>("cancel_compare");

/** Windows' own previewer for a file type, drawn over the preview pane (native_preview.rs). */
export type NativePreviewEvent = { type: "ready" } | { type: "unavailable"; reason: string };
/** A rectangle of the window, in physical pixels. */
export interface PixelRect {
    x: number;
    y: number;
    width: number;
    height: number;
}
export const nativePreviewShow = (id: number, path: string, rect: PixelRect | null, dark: boolean, onEvent: (event: NativePreviewEvent) => void) => {
    const channel = new Channel<NativePreviewEvent>();
    channel.onmessage = onEvent;
    return invoke("native_preview_show", { id, path, rect, dark, onEvent: channel });
};
export const nativePreviewPlace = (id: number, rect: PixelRect | null) => invoke("native_preview_place", { id, rect });
export const nativePreviewClose = (id: number) => invoke("native_preview_close", { id });

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

export interface SheetPreview {
    table: TablePreview;
    /** A workbook's sheets that aren't hidden (none for Parquet), and the one shown. */
    sheets: string[];
    sheet: number;
    /** A Parquet file's columns' types ("INT64", "BYTE_ARRAY · String"…). */
    types: string[];
    totalRows: number;
    totalCols: number;
}

export interface SqlTables {
    tables: string[];
    /** Loading the file took this long (0 when it was loaded already, or is a database). */
    loadMs: number;
}

export interface SqlAnswer {
    columns: TableColumn[];
    rows: string[][];
    /** There were more rows than came back (1,000). */
    truncated: boolean;
    elapsedMs: number;
    loadMs: number;
}

/** The tables to query in a file: a database's own, or a table file's loaded into memory as `data` (a workbook's by its sheets' names). */
export const sqlTables = (path: string) => invoke<SqlTables>("sql_tables", { path });

/** One query that reads, over a file loaded as `sqlTables` says; its first 1,000 rows. A newer one stops it. */
export const sqlRun = (path: string, sql: string) => invoke<SqlAnswer>("sql_run", { path, sql });

/** A workbook's sheet (the first one not hidden, unless `sheet` says), or a Parquet file, as a table: its first 1,000 rows. */
export const inspectSheet = (path: string, sheet?: number) => invoke<SheetPreview>("inspect_sheet", { path, sheet: sheet ?? null });

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
    blake3: string;
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

export type HistoryKind = "renamed" | "moved" | "deleted" | "recycled" | "restored" | "emptied";

/** Something renamed, moved or deleted, as the index service's history has it (history.rs). */
export interface HistoryEvent {
    /** ms since the Unix epoch. */
    time: number;
    kind: HistoryKind;
    dir: boolean;
    /** Where it was; for something in the Recycle Bin, its place there. */
    path: string;
    /** Where it went (renamed, moved, recycled, restored). */
    to: string | null;
    /** For something in the Recycle Bin: where it was before. */
    was: string | null;
    /** Where it is now, if it's still there. */
    now: string | null;
    /** A folder deleted with what was in it: how many items went with it; a burst: how many more were deleted in `path`. */
    items: number;
    burst: boolean;
}

export interface HistoryAnswer {
    events: HistoryEvent[];
    /** More matched than came. */
    limited: boolean;
    /** When the oldest event kept happened (0: none). */
    oldest: number;
    elapsed: number;
}

/** What was renamed, moved and deleted, newest first; rejects with "tooOld" (a service from before it) or "off". */
export const indexHistory = (query: string, since: number, max: number) => invoke<HistoryAnswer>("index_history", { query, since, max });
