/**
 * The archives Glint reads (src-tauri/src/archive/read.rs): the label of each format by its code, as a
 * listing's header and a disk-usage map's engine give it, and which file names Enter opens like folders.
 */
export const ARCHIVE_FORMATS = ["zip", "7z"] as const;
export type ArchiveFormat = (typeof ARCHIVE_FORMATS)[number];

/** Extensions Enter opens like a folder. */
export const BROWSED = ["zip", "7z"];

/**
 * Files that are archives by another name (a .docx, a .jar, an .msi…): Enter opens them in their app,
 * and "Open as archive" looks inside.
 */
export const ON_REQUEST = [
    "docx", "docm", "dotx", "dotm", "xlsx", "xlsm", "xltx", "xltm", "pptx", "pptm", "potx", "potm", "odt", "ods", "odp", "odg", "ott", "ots", "otp", "jar", "war", "ear", "apk", "aab", "nupkg", "vsix",
    "whl", "epub", "msix", "msixbundle", "appx", "appxbundle", "xpi", "kmz", "3mf", "ipa",
];

/** A path into an archive, by its names (one opened by its bytes says so in its listing instead). */
export const ARCHIVE_IN_PATH = new RegExp(`\\.(${[...BROWSED, ...ON_REQUEST].join("|")})([\\\\/]|$)`, "i");
