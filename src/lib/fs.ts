import { Channel, invoke } from "@tauri-apps/api/core";
import { decodeListing, type FileEntry, type Listing, withDetails } from "./entries";

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

/** Searches file names under `roots` (with the index when it runs, else fd), streaming results with metadata. */
export async function findFiles(roots: string[], query: string, options: FindOptions, onEvent: (event: FindEvent) => void) {
    const channel = new Channel<FindEvent>();
    channel.onmessage = (event) => {
        if (event.type === "entries") for (const entry of event.entries) withDetails(entry);
        onEvent(event);
    };
    await invoke("find_files", { roots, query, options, onEvent: channel });
}

export const cancelFind = () => invoke("cancel_find");

export interface FolderSizes {
    path: string;
    total: number;
    /** Subfolder name → bytes. */
    sizes: [string, number][];
    elapsed: number;
    engine: "index" | "dust";
}

/**
 * Sizes of every subfolder: from the index when it has the folder, else calculated by dust (unless
 * `indexOnly`, which rejects instead). Rejects with "cancelled" when superseded or stopped.
 */
export const folderSizes = (path: string, indexOnly = false) => invoke<FolderSizes>("folder_sizes", { path, indexOnly });
export const cancelFolderSizes = () => invoke("cancel_folder_sizes");

export type FileOp =
    | { kind: "copy" | "move"; sources: string[]; dest: string }
    | { kind: "delete"; paths: string[]; permanent: boolean; confirm: boolean }
    | { kind: "rename"; path: string; name: string };

/** Runs through Windows' own file engine (progress, conflicts, Recycle Bin). False if cancelled. */
export const fileOp = (op: FileOp) => invoke<boolean>("file_op", { op });

export const clipboardSetFiles = (paths: string[], cut: boolean) => invoke("clipboard_set_files", { paths, cut });
export const clipboardGetFiles = () => invoke<{ paths: string[]; cut: boolean } | null>("clipboard_get_files");
export const clipboardClear = () => invoke("clipboard_clear");

/** Drags files out to other apps; call while the mouse button is down. Resolves when dropped or cancelled. */
export const startDrag = (paths: string[]) => invoke<"copy" | "move" | "link" | "none">("start_drag", { paths });

export const showProperties = (path: string) => invoke("show_properties", { path });
export const openWith = (path: string) => invoke("open_with", { path });
export const openTerminal = (path: string) => invoke("open_terminal", { path });

/** Watches one folder (or none); changes arrive as "dir-changed" events carrying its path. */
export const watchDir = (path: string | null) => invoke("watch_dir", { path });

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
