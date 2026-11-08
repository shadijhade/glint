import { create } from "zustand";

/**
 * The archives Glint reads (src-tauri/src/archive/read.rs): the label of each format by its code, as a
 * listing's header and a disk-usage map's engine give it, and which file names Enter opens like folders.
 */
export const ARCHIVE_FORMATS = [
    "zip", "7z", "tar", "tar.gz", "tar.xz", "tar.bz2", "tar.zst", "tar.lz4", "gz", "xz", "bz2", "zst", "lz4", "cab", "msi", "rar",
    "ncdu", "ncdu.gz", "ncdu.xz", "ncdu.bz2", "ncdu.zst", "ncdu.lz4",
] as const;
export type ArchiveFormat = (typeof ARCHIVE_FORMATS)[number];

/** Extensions Enter opens like a folder: archives, tars (.tar.gz ends in "gz" like a lone .gz), single compressed files, and ncdu exports. */
export const BROWSED = ["zip", "7z", "rar", "tar", "tgz", "txz", "tbz", "tbz2", "tzst", "gz", "xz", "bz2", "zst", "lz4", "cab", "ncdu"];

/** Formats that hold one compressed file (shown as a folder of one): Extract here writes the file itself next to them. */
export const SINGLE_FILE: readonly ArchiveFormat[] = ["gz", "xz", "bz2", "zst", "lz4"];

/**
 * Files that are archives by another name (a .docx, a .jar, an .msi…): Enter opens them in their app
 * (or runs the installer), and "Open as archive" looks inside.
 */
export const ON_REQUEST = [
    "docx", "docm", "dotx", "dotm", "xlsx", "xlsm", "xltx", "xltm", "pptx", "pptm", "potx", "potm", "odt", "ods", "odp", "odg", "ott", "ots", "otp", "jar", "war", "ear", "apk", "aab", "nupkg", "vsix",
    "whl", "epub", "msix", "msixbundle", "appx", "appxbundle", "xpi", "kmz", "3mf", "ipa", "msi", "msm",
];

/** A path into an archive, by its names (one opened by its bytes says so in its listing instead). */
export const ARCHIVE_IN_PATH = new RegExp(`\\.(${[...BROWSED, ...ON_REQUEST].join("|")})([\\\\/]|$)`, "i");

/** Whether an error from an archive asks for its password: password-protected, or the one given doesn't open it. */
export const needsPassword = (error: string) => /password-protected|password doesn't open/i.test(error);

/** The archive the password dialog asks about (where opening it failed), and what to do once it opens. */
export const usePasswordAsk = create<{ path: string | null; retry?: () => void }>(() => ({ path: null }));

/** Asks for the password of what `path` leads to in an archive, then runs `retry`. */
export const askArchivePassword = (path: string, retry: () => void) => usePasswordAsk.setState({ path, retry });

/** Compress with a password: what to compress, and in which format, until the password is given. */
export const useCompressAsk = create<{ run: ((format: "zip" | "7z", password: string) => void) | null; format: "zip" | "7z"; what: string }>(() => ({ run: null, format: "zip", what: "" }));

/** Counts the passwords that opened something, so what failed for want of one is read again. */
export const useUnlocks = create(() => ({ count: 0 }));
