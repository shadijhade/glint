import {
    AppWindow,
    BookOpen,
    Database,
    Disc,
    File,
    FileArchive,
    FileAudio,
    FileCode,
    FileCog,
    FileImage,
    FileJson,
    FileLock,
    FileSpreadsheet,
    FileText,
    FileType,
    FileVideo,
    Folder,
    Link,
    type LucideIcon,
    Presentation,
} from "lucide-react";
import type { FileEntry } from "./entries";

interface Kind {
    icon: LucideIcon;
    /** Tailwind classes for the icon. */
    tone: string;
    /** Type column text; `%` is the uppercased extension. */
    label: string;
    preview?: "image" | "video" | "audio" | "text" | "pdf";
}

const KINDS: [string, Kind][] = [
    ["png jpg jpeg gif webp bmp ico svg avif", { icon: FileImage, tone: "text-sky-500", label: "% image", preview: "image" }],
    ["tif tiff heic heif psd raw cr2 nef arw dng", { icon: FileImage, tone: "text-sky-500", label: "% image" }],
    ["mp4 webm m4v mov ogv", { icon: FileVideo, tone: "text-violet-500", label: "% video", preview: "video" }],
    ["mkv avi wmv flv mpg mpeg 3gp m2ts", { icon: FileVideo, tone: "text-violet-500", label: "% video" }],
    ["mp3 wav ogg oga opus m4a aac flac", { icon: FileAudio, tone: "text-pink-500", label: "% audio", preview: "audio" }],
    ["wma aiff aif mid midi", { icon: FileAudio, tone: "text-pink-500", label: "% audio" }],
    ["zip rar 7z tar gz tgz bz2 xz zst cab", { icon: FileArchive, tone: "text-amber-600 dark:text-amber-500", label: "% archive" }],
    ["pdf", { icon: FileText, tone: "text-red-500", label: "PDF document", preview: "pdf" }],
    ["doc docx odt rtf pages", { icon: FileText, tone: "text-blue-600 dark:text-blue-400", label: "Word document" }],
    ["xls xlsx xlsm ods numbers", { icon: FileSpreadsheet, tone: "text-green-600 dark:text-green-500", label: "Spreadsheet" }],
    ["csv tsv", { icon: FileSpreadsheet, tone: "text-green-600 dark:text-green-500", label: "% file", preview: "text" }],
    ["ppt pptx odp key", { icon: Presentation, tone: "text-orange-500", label: "Presentation" }],
    ["json jsonc json5", { icon: FileJson, tone: "text-yellow-600 dark:text-yellow-500", label: "JSON file", preview: "text" }],
    [
        "js mjs cjs ts mts cts tsx jsx rs go py rb java kt kts c h cpp hpp cc cs php swift scala sh bash zsh ps1 psm1 psd1 bat cmd lua sql vue svelte html htm css scss sass less xml xaml csproj sln props targets yml yaml toml ini cfg conf gradle dart r pl ex exs erl hs fs fsx clj zig nim dockerfile makefile cmake",
        { icon: FileCode, tone: "text-emerald-500", label: "% source file", preview: "text" },
    ],
    ["txt md markdown log nfo text env gitignore gitattributes editorconfig properties lock", { icon: FileText, tone: "text-muted-foreground", label: "% file", preview: "text" }],
    ["exe msi com scr appx msix appinstaller", { icon: AppWindow, tone: "text-slate-500 dark:text-slate-400", label: "Application" }],
    ["dll sys so dylib ocx drv cpl mui", { icon: FileCog, tone: "text-slate-500 dark:text-slate-400", label: "% system file" }],
    ["ttf otf woff woff2 eot fon", { icon: FileType, tone: "text-rose-500", label: "% font" }],
    ["iso img vhd vhdx vmdk dmg", { icon: Disc, tone: "text-indigo-500", label: "Disk image" }],
    ["lnk url", { icon: Link, tone: "text-cyan-600 dark:text-cyan-400", label: "Shortcut" }],
    ["db sqlite sqlite3 mdb accdb", { icon: Database, tone: "text-teal-600 dark:text-teal-400", label: "Database" }],
    ["epub mobi azw3 cbz cbr", { icon: BookOpen, tone: "text-amber-700 dark:text-amber-400", label: "E-book" }],
    ["pem key crt cer pfx p12 gpg asc", { icon: FileLock, tone: "text-yellow-700 dark:text-yellow-500", label: "Certificate or key" }],
];

const BY_EXT = new Map<string, Kind>();
for (const [exts, kind] of KINDS) for (const ext of exts.split(" ")) BY_EXT.set(ext, kind);

const FOLDER: Kind = { icon: Folder, tone: "fill-amber-400/80 text-amber-500 dark:fill-amber-400/70", label: "File folder" };
const GENERIC: Kind = { icon: File, tone: "text-muted-foreground", label: "% file" };

/** Names without an extension that are still text, e.g. Dockerfile or LICENSE. */
const TEXT_NAMES = new Set(["dockerfile", "makefile", "license", "readme", "changelog", "authors", "codeowners", ".gitignore", ".env"]);

export function kindOf(entry: FileEntry): Kind {
    if (entry.dir) return FOLDER;
    return BY_EXT.get(entry.ext) ?? (TEXT_NAMES.has(entry.lower) ? BY_EXT.get("txt")! : GENERIC);
}

export function typeLabel(entry: FileEntry) {
    const { label } = kindOf(entry);
    if (!label.includes("%")) return label;
    return entry.ext ? label.replace("%", entry.ext.toUpperCase()) : "File";
}

/** What the preview pane can show for a file. */
export const previewKind = (entry: FileEntry) => kindOf(entry).preview;

/** False for extensions we know nothing about (the preview tries those as text). */
export const isKnownType = (entry: FileEntry) => kindOf(entry) !== GENERIC;
