import { ARCHIVE_IN_PATH } from "./archives.ts";

/** The "This PC" view (drives and known folders) has no real path. */
export const THIS_PC = "";

/** The Recycle Bin, by File Explorer's name for it: a place, not a folder, whose items each have a path of their own (recycle.rs). */
export const RECYCLE_BIN = "shell:RecycleBinFolder";
export const isRecycleBin = (path: string) => path.toLowerCase() === "shell:recyclebinfolder";

/** Recent, by File Explorer's name for Windows' Recent Items: the files opened lately, each at its own path (recent.rs). */
export const RECENT = "shell:Recent";
export const isRecent = (path: string) => path.toLowerCase() === "shell:recent";

/** Gallery: the photos and videos in the gallery's folders, newest first by when each was taken (gallery.rs). */
export const GALLERY = "shell:Gallery";
export const isGallery = (path: string) => path.toLowerCase() === "shell:gallery";

/** A remote connection's folders (remote/): `remote:<id>` and below, read-only, their files downloaded to be read. */
export const REMOTE = "remote:";
export const isRemotePath = (path: string) => path.slice(0, REMOTE.length).toLowerCase() === REMOTE;
/** The id of the connection a remote path is on ("" for any other path). */
export const remoteIdOf = (path: string) => (isRemotePath(path) ? (path.slice(REMOTE.length).split(/[\\/]/)[0] ?? "") : "");

/** A shadow copy (Windows' earlier state of a drive: previous versions, versions.rs) and what's in it, read-only. */
const SHADOW = /^\\\\\?\\GLOBALROOT\\Device\\HarddiskVolumeShadowCopy(\d+)(?=\\|$)/i;
const SHADOW_ROOT = /^\\\\\?\\GLOBALROOT\\Device\\HarddiskVolumeShadowCopy\d+\\?$/i;
export const isShadowPath = (path: string) => SHADOW.test(path);
/** A shadow copy's top, with its closing backslash ("" for any other path). */
export const shadowRootOf = (path: string) => {
    const match = SHADOW.exec(path);
    return match ? `${match[0]}\\` : "";
};
/** What shadow copies' tops are called ("C: on 2 Nov 2026, 14:20"), as Glint learned them. */
export const shadowNames = new Map<string, string>();
export const shadowName = (path: string) => shadowNames.get(shadowRootOf(path).toLowerCase()) ?? `Shadow copy ${SHADOW.exec(path)?.[1] ?? ""}`.trim();

/** A folder on this PC (on disk, or in an archive) rather than This PC, the Recycle Bin or a remote: what's in it has its path, for other tools too. */
export const isFolderPath = (path: string) => path !== THIS_PC && !isRecycleBin(path) && !isRecent(path) && !isGallery(path) && !isRemotePath(path);

const SEPARATORS = /[\\/]+$/;

/** In an archive (or the archive itself), by its names: read-only, and not on disk for other tools. */
export const inArchivePath = (path: string) => ARCHIVE_IN_PATH.test(path);

/** "C:\", "C:", "\\server\share", "\\server" (its shares) and "/" are roots, and so are the Recycle Bin, Recent and a remote's top (This PC is above them). */
export function isRoot(path: string) {
    return /^[A-Za-z]:[\\/]?$/.test(path) || SHADOW_ROOT.test(path) || /^\\\\[^\\]+(\\[^\\]+)?\\?$/.test(path) || path === "/" || isRecycleBin(path) || isRecent(path) || isGallery(path) || /^remote:[^\\/]+[\\/]?$/i.test(path);
}

export function joinPath(dir: string, name: string) {
    if (/[\\/]$/.test(dir)) return dir + name;
    return dir + (dir.includes("/") && !dir.includes("\\") ? "/" : "\\") + name;
}

export function baseName(path: string) {
    const trimmed = path.replace(SEPARATORS, "");
    return trimmed.slice(Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/")) + 1) || path;
}

/** "C:\a\b" → "C:\a", "C:\a" → "C:\", a share → its server, and a root → This PC. */
export function parentPath(path: string) {
    const share = /^(\\\\[^\\]+)\\[^\\]+\\?$/.exec(path);
    if (share) return share[1];
    if (path === THIS_PC || isRoot(path)) return THIS_PC;
    const trimmed = path.replace(SEPARATORS, "");
    const cut = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
    if (cut < 0) return THIS_PC;
    const parent = trimmed.slice(0, cut);
    if (/^[A-Za-z]:$/.test(parent) || SHADOW_ROOT.test(parent)) return parent + "\\";
    return parent || "/";
}

/** Same location, ignoring case and a trailing separator (Windows paths are case-insensitive). */
export function samePath(a: string, b: string) {
    return a.replace(SEPARATORS, "").toLowerCase() === b.replace(SEPARATORS, "").toLowerCase();
}

/** The drive ("C:") or share ("\\server\share") a path is on. */
export const volumeOf = (path: string) => /^(?:[a-z]:|\\\\\?\\GLOBALROOT\\Device\\HarddiskVolumeShadowCopy\d+|\\\\[^\\]+\\[^\\]+)/i.exec(path)?.[0] ?? "";

/**
 * The file a link in a document names (a Markdown link or picture): its URL's path, relative to the
 * document's `folder`, or to `root` (its repository's, or its drive's) when it starts with / as on
 * GitHub; or a path like C:\…. Null for a web address or another scheme.
 */
export function linkedPath(url: string, folder: string, root: string): string | null {
    let path = url.replace(/[?#].*$/, "");
    if (!path) return null;
    try {
        path = decodeURIComponent(path);
    } catch {
        // A stray % is the name itself.
    }
    if (/^[a-z][a-z\d+.-]*:/i.test(path) && !/^[a-z]:[\\/]/i.test(path)) return null;
    // A leading / or \ is the root's, never a share's \\server.
    const full = /^[a-z]:/i.test(path) ? path : `${/^[\\/]/.test(path) ? root : folder}\\${path}`;
    const volume = volumeOf(full);
    if (!volume) return null;
    const parts: string[] = [];
    for (const part of full.slice(volume.length).split(/[\\/]+/)) {
        if (part === "..") parts.pop();
        else if (part && part !== ".") parts.push(part);
    }
    return `${volume}\\${parts.join("\\")}`;
}

/** The breadcrumb trail: root first, then each folder down to `path`. */
export function crumbs(path: string): { name: string; path: string }[] {
    if (path === THIS_PC) return [];
    if (isRecycleBin(path)) return [{ name: "Recycle Bin", path: RECYCLE_BIN }];
    if (isRecent(path)) return [{ name: "Recent", path: RECENT }];
    if (isGallery(path)) return [{ name: "Gallery", path: GALLERY }];
    // A remote's top is named for its connection's id (the address bar shows the connection's name).
    if (isRemotePath(path)) {
        const [id, ...rest] = path.slice(REMOTE.length).split(/[\\/]/).filter(Boolean);
        const trail = [{ name: id ?? "", path: REMOTE + (id ?? "") }];
        for (const part of rest) trail.push({ name: part, path: joinPath(trail[trail.length - 1].path, part) });
        return trail;
    }
    // A shadow copy's top is named for its drive and when it was taken.
    const shadow = shadowRootOf(path);
    if (shadow) {
        const trail = [{ name: shadowName(path), path: shadow }];
        for (const part of path.slice(shadow.length).split(/[\\/]/).filter(Boolean)) trail.push({ name: part, path: joinPath(trail[trail.length - 1].path, part) });
        return trail;
    }
    // A share comes after its server, which lists its shares.
    const unc = path.match(/^(\\\\[^\\]+)(.*)$/);
    let root: string;
    let rest: string;
    if (unc) [root, rest] = [unc[1], unc[2]];
    else if (/^[A-Za-z]:/.test(path)) [root, rest] = [path.slice(0, 2) + "\\", path.slice(2)];
    else if (path.startsWith("/")) [root, rest] = ["/", path.slice(1)];
    else return [{ name: path, path }];
    const trail = [{ name: root, path: root }];
    let current = root;
    for (const part of rest.split(/[\\/]/).filter(Boolean)) {
        current = joinPath(current, part);
        trail.push({ name: part, path: current });
    }
    return trail;
}
