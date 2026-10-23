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

/** A color on the disk-usage map, shared by related kinds of files. */
export interface TypeGroup {
    name: string;
    hue: number;
    /** Saturation, in percent. */
    sat: number;
}

export const TYPE_GROUPS = {
    images: { name: "Images", hue: 199, sat: 78 },
    video: { name: "Videos", hue: 262, sat: 66 },
    audio: { name: "Audio", hue: 322, sat: 66 },
    archives: { name: "Archives", hue: 36, sat: 86 },
    documents: { name: "Documents", hue: 4, sat: 70 },
    code: { name: "Code and text", hue: 150, sat: 50 },
    programs: { name: "Programs", hue: 188, sat: 60 },
    system: { name: "System files", hue: 216, sat: 46 },
    disks: { name: "Disk images", hue: 240, sat: 56 },
    data: { name: "Databases", hue: 172, sat: 56 },
    fonts: { name: "Fonts", hue: 345, sat: 62 },
    other: { name: "Other files", hue: 30, sat: 8 },
} satisfies Record<string, TypeGroup>;

interface Kind {
    icon: LucideIcon;
    /** Tailwind classes for the icon. */
    tone: string;
    /** Type column text; `%` is the uppercased extension. */
    label: string;
    preview?: "image" | "video" | "audio" | "text" | "pdf";
    group: TypeGroup;
}

const G = TYPE_GROUPS;
const KINDS: [string, Kind][] = [
    ["png jpg jpeg gif webp bmp ico svg avif", { icon: FileImage, tone: "text-sky-500", label: "% image", preview: "image", group: G.images }],
    ["tif tiff heic heif psd raw cr2 nef arw dng", { icon: FileImage, tone: "text-sky-500", label: "% image", group: G.images }],
    ["mp4 webm m4v mov ogv", { icon: FileVideo, tone: "text-violet-500", label: "% video", preview: "video", group: G.video }],
    ["mkv avi wmv flv mpg mpeg 3gp m2ts", { icon: FileVideo, tone: "text-violet-500", label: "% video", group: G.video }],
    ["mp3 wav ogg oga opus m4a aac flac", { icon: FileAudio, tone: "text-pink-500", label: "% audio", preview: "audio", group: G.audio }],
    ["wma aiff aif mid midi", { icon: FileAudio, tone: "text-pink-500", label: "% audio", group: G.audio }],
    ["zip rar 7z tar gz tgz txz tbz tbz2 tzst bz2 xz zst lz4 cab", { icon: FileArchive, tone: "text-amber-600 dark:text-amber-500", label: "% archive", group: G.archives }],
    ["ncdu", { icon: FileArchive, tone: "text-amber-600 dark:text-amber-500", label: "ncdu export", group: G.archives }],
    ["pdf", { icon: FileText, tone: "text-red-500", label: "PDF document", preview: "pdf", group: G.documents }],
    ["doc docx odt rtf pages", { icon: FileText, tone: "text-blue-600 dark:text-blue-400", label: "Word document", group: G.documents }],
    ["xls xlsx xlsm xlsb xltx ods numbers", { icon: FileSpreadsheet, tone: "text-green-600 dark:text-green-500", label: "Spreadsheet", group: G.documents }],
    ["csv tsv", { icon: FileSpreadsheet, tone: "text-green-600 dark:text-green-500", label: "% file", preview: "text", group: G.documents }],
    ["ppt pptx odp key", { icon: Presentation, tone: "text-orange-500", label: "Presentation", group: G.documents }],
    ["json jsonc json5 geojson webmanifest har", { icon: FileJson, tone: "text-yellow-600 dark:text-yellow-500", label: "JSON file", preview: "text", group: G.code }],
    [
        "js mjs cjs ts mts cts tsx jsx rs go py rb java kt kts c h cpp hpp cc cs php swift scala sh bash zsh ps1 psm1 psd1 bat cmd lua sql vue svelte html htm css scss sass less xml xaml csproj sln props targets yml yaml toml ini cfg conf gradle dart r pl ex exs erl hs fs fsx clj zig nim dockerfile makefile cmake",
        { icon: FileCode, tone: "text-emerald-500", label: "% source file", preview: "text", group: G.code },
    ],
    ["txt md markdown mdown mkd mkdn log nfo text env gitignore gitattributes editorconfig properties lock", { icon: FileText, tone: "text-muted-foreground", label: "% file", preview: "text", group: G.code }],
    ["exe msi com scr appx msix appinstaller", { icon: AppWindow, tone: "text-slate-500 dark:text-slate-400", label: "Application", group: G.programs }],
    ["dll sys so dylib ocx drv cpl mui", { icon: FileCog, tone: "text-slate-500 dark:text-slate-400", label: "% system file", group: G.system }],
    ["ttf otf woff woff2 eot fon", { icon: FileType, tone: "text-rose-500", label: "% font", group: G.fonts }],
    ["iso img vhd vhdx vmdk dmg", { icon: Disc, tone: "text-indigo-500", label: "Disk image", group: G.disks }],
    ["lnk url", { icon: Link, tone: "text-cyan-600 dark:text-cyan-400", label: "Shortcut", group: G.other }],
    ["db sqlite sqlite3 mdb accdb", { icon: Database, tone: "text-teal-600 dark:text-teal-400", label: "Database", group: G.data }],
    ["parquet", { icon: Database, tone: "text-teal-600 dark:text-teal-400", label: "Parquet file", group: G.data }],
    ["epub mobi azw3 cbz cbr", { icon: BookOpen, tone: "text-amber-700 dark:text-amber-400", label: "E-book", group: G.documents }],
    ["pem key crt cer pfx p12 gpg asc", { icon: FileLock, tone: "text-yellow-700 dark:text-yellow-500", label: "Certificate or key", group: G.other }],
];

const BY_EXT = new Map<string, Kind>();
for (const [exts, kind] of KINDS) for (const ext of exts.split(" ")) BY_EXT.set(ext, kind);

const FOLDER: Kind = { icon: Folder, tone: "fill-amber-400/80 text-amber-500 dark:fill-amber-400/70", label: "File folder", group: G.other };
const GENERIC: Kind = { icon: File, tone: "text-muted-foreground", label: "% file", group: G.other };

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

const MARKDOWN = new Set(["md", "markdown", "mdown", "mkd", "mkdn"]);

/** Markdown, which the preview shows as it reads (or as written). */
export const isMarkdown = (entry: FileEntry) => !entry.dir && MARKDOWN.has(entry.ext);

const DATA = new Map<string, "json" | "yaml" | "toml">([
    ["json", "json"],
    ["geojson", "json"],
    ["webmanifest", "json"],
    ["har", "json"],
    ["yaml", "yaml"],
    ["yml", "yaml"],
    ["toml", "toml"],
]);

/** JSON, YAML and TOML files, which the preview shows as a tree to run jq filters over. */
export const dataFormat = (entry: FileEntry) => (entry.dir ? undefined : DATA.get(entry.ext));

/** False for extensions we know nothing about (the preview tries those as text). */
export const isKnownType = (entry: FileEntry) => kindOf(entry) !== GENERIC;

/** A file's color group on the disk-usage map. */
export const typeGroup = (entry: FileEntry) => kindOf(entry).group;

/** Pictures and videos: what a thumbnail in the list's icon column tells apart better than an icon. */
export const isPictureOrVideo = (entry: FileEntry) => !entry.dir && (kindOf(entry).group === G.images || kindOf(entry).group === G.video);
