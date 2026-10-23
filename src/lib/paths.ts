import { ARCHIVE_IN_PATH } from "./archives.ts";

/** The "This PC" view (drives and known folders) has no real path. */
export const THIS_PC = "";

const SEPARATORS = /[\\/]+$/;

/** In an archive (or the archive itself), by its names: read-only, and not on disk for other tools. */
export const inArchivePath = (path: string) => ARCHIVE_IN_PATH.test(path);

/** "C:\", "C:", "\\server\share" and "/" are roots. */
export function isRoot(path: string) {
    return /^[A-Za-z]:[\\/]?$/.test(path) || /^\\\\[^\\]+\\[^\\]+\\?$/.test(path) || path === "/";
}

export function joinPath(dir: string, name: string) {
    if (/[\\/]$/.test(dir)) return dir + name;
    return dir + (dir.includes("/") && !dir.includes("\\") ? "/" : "\\") + name;
}

export function baseName(path: string) {
    const trimmed = path.replace(SEPARATORS, "");
    return trimmed.slice(Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/")) + 1) || path;
}

/** "C:\a\b" → "C:\a", "C:\a" → "C:\", and a root → This PC. */
export function parentPath(path: string) {
    if (path === THIS_PC || isRoot(path)) return THIS_PC;
    const trimmed = path.replace(SEPARATORS, "");
    const cut = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
    if (cut < 0) return THIS_PC;
    const parent = trimmed.slice(0, cut);
    if (/^[A-Za-z]:$/.test(parent)) return parent + "\\";
    return parent || "/";
}

/** Same location, ignoring case and a trailing separator (Windows paths are case-insensitive). */
export function samePath(a: string, b: string) {
    return a.replace(SEPARATORS, "").toLowerCase() === b.replace(SEPARATORS, "").toLowerCase();
}

/** The drive ("C:") or share ("\\server\share") a path is on. */
export const volumeOf = (path: string) => /^(?:[a-z]:|\\\\[^\\]+\\[^\\]+)/i.exec(path)?.[0] ?? "";

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
    const unc = path.match(/^(\\\\[^\\]+\\[^\\]+)(.*)$/);
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
