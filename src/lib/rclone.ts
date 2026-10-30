import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** A question rclone asks while setting a remote up, or one of a service's settings (rclone.rs). */
export interface RcloneQuestion {
    name: string;
    help: string;
    /** "bool", "string", "int", "SizeSuffix", "Duration"… */
    kind: string;
    default: string;
    examples: { value: string; help: string; provider: string }[];
    /** Only one of the examples will do. */
    exclusive: boolean;
    required: boolean;
    password: boolean;
    /** The services (S3's) it's for, as rclone writes them: "AWS,Minio", "!AWS", or "" for all. */
    provider: string;
}

/** A kind of storage rclone reaches, with the settings it asks for first. */
export interface RcloneProvider {
    name: string;
    description: string;
    /** Signed in to in a browser. */
    oauth: boolean;
    options: RcloneQuestion[];
}

/** A step of setting up a remote: rclone's next question, or (no question, no state) done. */
export interface RcloneStep {
    state: string;
    question: RcloneQuestion | null;
    error: string;
}

/** A remote in rclone's configuration. */
export interface RcloneRemote {
    name: string;
    /** Its type: "drive", "onedrive"… */
    kind: string;
    /** S3's service: "AWS", "Cloudflare"… */
    provider: string;
}

export interface RcloneStatus {
    installed: boolean;
    version: string | null;
    /** Where rclone keeps its configuration. */
    config: string | null;
    /** It's encrypted, and Glint hasn't its password yet. */
    locked: boolean;
}

export interface CloudSpace {
    total: number | null;
    used: number | null;
    free: number | null;
}

/** What an error from rclone starts with when its configuration needs its password (rclone.rs's LOCKED). */
export const LOCKED = "rclone's configuration is encrypted";

export const rcloneStatus = () => invoke<RcloneStatus>("rclone_status");
export const rcloneProviders = () => invoke<RcloneProvider[]>("rclone_providers");
export const rcloneRemotes = () => invoke<RcloneRemote[]>("rclone_remotes");
export const rcloneDelete = (name: string) => invoke<void>("rclone_delete", { name });
export const rcloneAbout = (remote: string, root: string) => invoke<CloudSpace | null>("rclone_about", { remote, root });
export const rcloneUnlock = (password: string) => invoke<void>("rclone_unlock", { password });
export const rcloneConfigStart = (name: string, kind: string, parameters: Record<string, string>) => invoke<RcloneStep>("rclone_config_start", { name, kind, parameters });
export const rcloneConfigAnswer = (name: string, state: string, answer: string) => invoke<RcloneStep>("rclone_config_answer", { name, state, answer });
/** Runs a browser sign-in as rclone's question (its `help`) says: resolves with what to answer it. */
export const rcloneAuthorize = (help: string) => invoke<string>("rclone_authorize", { help });
export const rcloneAuthorizeCancel = () => invoke<void>("rclone_authorize_cancel");
/** The sign-in page's address, once `rclone authorize` has it ready. */
export const onSignInPage = (handler: (url: string) => void) => listen<string>("rclone-sign-in", (e) => handler(e.payload));

/** The services most people look for first, in the picker's order. */
export const POPULAR = ["drive", "onedrive", "dropbox", "box", "pcloud", "mega", "protondrive", "iclouddrive", "s3", "b2", "azureblob", "google cloud storage", "sftp", "ftp", "smb", "webdav"];

/** rclone's types that aren't a place of their own (they wrap or join others, or are for tests): not offered. */
const WRAPPERS = new Set(["alias", "archive", "cache", "chunker", "combine", "compress", "hasher", "local", "memory", "tardigrade", "union"]);
export const offered = (provider: RcloneProvider) => !WRAPPERS.has(provider.name);

/** Plainer names for some services than rclone's descriptions. */
const NAMES: Record<string, string> = {
    drive: "Google Drive",
    onedrive: "OneDrive",
    dropbox: "Dropbox",
    box: "Box",
    pcloud: "pCloud",
    mega: "MEGA",
    protondrive: "Proton Drive",
    iclouddrive: "iCloud Drive",
    s3: "Amazon S3 and compatible",
    b2: "Backblaze B2",
    azureblob: "Azure Blob Storage",
    azurefiles: "Azure Files",
    "google cloud storage": "Google Cloud Storage",
    "google photos": "Google Photos",
    sftp: "SFTP",
    ftp: "FTP",
    smb: "Windows share (SMB)",
    webdav: "WebDAV",
    http: "A website's files (HTTP)",
    local: "A folder on this PC",
    yandex: "Yandex Disk",
    crypt: "Encrypted folder (rclone crypt)",
};

/** A service's name: "Google Drive" for "drive", else rclone's description. */
export function serviceName(kind: string, providers?: RcloneProvider[]) {
    return NAMES[kind] ?? providers?.find((p) => p.name === kind)?.description ?? kind;
}

/** A name for a new remote in rclone's configuration, from the one it's given: letters, digits, "-" and "_", unlike the others'. */
export function remoteNameFor(name: string, taken: string[]) {
    const base = name.toLowerCase().normalize("NFKD").replace(/[^a-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 32) || "cloud";
    const used = new Set(taken.map((t) => t.toLowerCase()));
    let candidate = base;
    for (let n = 2; used.has(candidate); n++) candidate = `${base}-${n}`;
    return candidate;
}

/** Whether a setting applies to an S3 service (`provider`, as rclone writes them: "AWS,Minio", "!AWS,Other", "" for all). */
export function forProvider(filter: string, provider: string) {
    if (!filter) return true;
    const negated = filter.startsWith("!");
    const names = (negated ? filter.slice(1) : filter).split(",").map((n) => n.trim());
    return negated ? !names.includes(provider) : names.includes(provider);
}

/** A question's text: its first paragraph as the label, the rest as more. */
export function helpOf(question: RcloneQuestion): { label: string; more: string } {
    const text = question.help.trim();
    const cut = text.indexOf("\n\n");
    const first = (cut < 0 ? text : text.slice(0, cut)).replace(/\s*\n\s*/g, " ");
    return { label: first || question.name, more: cut < 0 ? "" : text.slice(cut + 2).trim() };
}
