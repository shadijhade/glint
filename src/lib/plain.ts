// Plain words into the search box's filters: "pdfs from last week over 5 mb" is ext:pdf dm:7d size:>5mb.
// A small lexicon, read as you type and in microseconds, nothing leaving the PC. The box offers what it
// read under it, and Tab puts the filters in the box, so the syntax is learned by using it.

/** What the box would search for, and what each part means, in words. */
export interface Plain {
    query: string;
    chips: string[];
}

type Media = "photo" | "video" | "music" | null;

const PHOTOS = "jpg;jpeg;png;gif;webp;heic;heif;bmp;tif;tiff;avif;jxl;raw;cr2;nef;arw;dng";
const VIDEOS = "mp4;mkv;mov;avi;wmv;webm;m4v;mpg;mpeg;3gp;m2ts";
const MUSIC = "mp3;flac;wav;m4a;aac;ogg;opus;wma;aiff";

/** Kinds of files by the words for them, as Glint's own groups have them (fileTypes.ts). */
const KINDS: [string[], string, string, Media][] = [
    [["photos", "photo", "pictures", "picture", "pics", "images", "image"], PHOTOS, "photos", "photo"],
    [["videos", "video", "movies", "movie", "clips", "films"], VIDEOS, "videos", "video"],
    [["music", "songs", "song", "audio", "tracks"], MUSIC, "music", "music"],
    [["documents", "document", "docs"], "pdf;doc;docx;odt;rtf;txt;md", "documents", null],
    [["spreadsheets", "spreadsheet", "excel", "workbooks"], "xls;xlsx;xlsm;ods;csv", "spreadsheets", null],
    [["presentations", "presentation", "slides", "powerpoints", "decks"], "ppt;pptx;odp", "presentations", null],
    [["archives", "archive", "zips", "compressed"], "zip;7z;rar;tar;gz;tgz;xz;bz2;zst", "archives", null],
    [["installers", "installer", "programs", "apps", "executables"], "exe;msi", "programs", null],
    [["ebooks", "e-books", "books"], "epub;mobi;azw3", "e-books", null],
    [["fonts", "font"], "ttf;otf;woff;woff2", "fonts", null],
];

/** Kinds in two words, read before the words alone. */
const PHRASES: [string, string, string, string][] = [
    ["word", "documents", "doc;docx", "Word documents"],
    ["disk", "images", "iso;img;vhd;vhdx", "disk images"],
];

/** Extensions a word like "pngs" or "png files" can name. */
const EXTS = new Set([PHOTOS, VIDEOS, MUSIC, "pdf;doc;docx;odt;rtf;txt;md;xls;xlsx;csv;ppt;pptx;zip;7z;rar;tar;gz;exe;msi;iso;vhd;vhdx;epub;json;log;svg;psd;dll;lnk;ttf;otf"].flatMap((list) => list.split(";")));

/** Words that say nothing a filter needs. */
const FILLER = new Set(["all", "any", "every", "my", "the", "a", "an", "of", "that", "which", "are", "is", "were", "was", "files", "file", "items", "stuff", "things", "show", "find", "get", "list", "search", "me", "please", "and", "or", "with", "from", "in", "on", "at", "since", "changed", "modified", "edited", "updated", "saved", "made", "created"]);

const SIZE_UNITS: Record<string, [string, string]> = {
    b: ["b", "bytes"], bytes: ["b", "bytes"], kb: ["kb", "KB"], k: ["kb", "KB"], kilobytes: ["kb", "KB"], mb: ["mb", "MB"], m: ["mb", "MB"], megabytes: ["mb", "MB"],
    gb: ["gb", "GB"], g: ["gb", "GB"], gigabytes: ["gb", "GB"], tb: ["tb", "TB"], terabytes: ["tb", "TB"],
};
/** The next unit down, for a size given with decimals (the filter takes whole numbers). */
const SMALLER: Record<string, string> = { tb: "gb", gb: "mb", mb: "kb", kb: "b" };
/** dm:'s units: a month counts as 30 days. */
const AGE_UNITS: Record<string, [string, number, string]> = {
    hour: ["h", 1, "hour"], hours: ["h", 1, "hours"], day: ["d", 1, "day"], days: ["d", 1, "days"], week: ["w", 1, "week"], weeks: ["w", 1, "weeks"],
    month: ["d", 30, "month"], months: ["d", 30, "months"], year: ["y", 1, "year"], years: ["y", 1, "years"],
};
const DURATION_UNITS: Record<string, [string, string]> = {
    seconds: ["s", "s"], second: ["s", "s"], secs: ["s", "s"], sec: ["s", "s"], s: ["s", "s"], minutes: ["m", "min"], minute: ["m", "min"], mins: ["m", "min"], min: ["m", "min"],
    hours: ["h", "h"], hour: ["h", "h"], hrs: ["h", "h"],
};
/** Comparisons, with how each reads. */
const MORE: Record<string, string> = { over: "over", above: "over", bigger: "over", larger: "over", more: "over", greater: "over", longer: "longer than", wider: "wider than", taller: "taller than", least: "at least" };
const LESS: Record<string, string> = { under: "under", below: "under", smaller: "under", less: "under", shorter: "shorter than", narrower: "narrower than", most: "at most" };

const YEAR = /^(19|20)\d\d$/;
/** "5", "1.5" or "5mb": the number and what's stuck to it. */
const numberAt = (word: string | undefined) => word?.match(/^(\d+(?:\.\d+)?)([a-z]*)$/) ?? null;
/** Whether files of this kind have a date they were taken. */
const taken = (media: Media) => media === "photo" || media === "video";

/**
 * What plain words in the box mean as filters, or null when they read as names: nothing recognized,
 * or the box holds filters, quotes or wildcards already.
 */
export function plainQuery(text: string): Plain | null {
    if (!text.trim() || /[:"*?]/.test(text)) return null;
    const words = text.trim().split(/\s+/);
    const lower = words.map((w) => w.toLowerCase().replace(/[,;.!]$/, ""));
    const names: string[] = [];
    const filters: string[] = [];
    const chips: string[] = [];
    let media: Media = null;
    // One filter of each kind; kinds of files add up ("photos and videos").
    const add = (filter: string, chip: string) => {
        const key = filter.slice(0, filter.indexOf(":"));
        const at = filters.findIndex((f) => f.startsWith(`${key}:`));
        if (at < 0) {
            filters.push(filter);
            chips.push(chip);
        } else if (key === "ext") {
            filters[at] += `;${filter.slice(4)}`;
            chips[at] += ` or ${chip}`;
        }
    };
    for (let i = 0; i < lower.length; i++) {
        const w = lower[i], next = lower[i + 1], after = lower[i + 2];
        const phrase = PHRASES.find(([first, second]) => first === w && (next === second || `${next}s` === second));
        if (phrase) {
            add(`ext:${phrase[2]}`, phrase[3]);
            i++;
            continue;
        }
        const kind = KINDS.find(([forms]) => forms.includes(w));
        if (kind) {
            add(`ext:${kind[1]}`, kind[2]);
            media = kind[3] ?? media;
            continue;
        }
        // "pngs", "mp3s", "png files".
        const ext = w.endsWith("s") && EXTS.has(w.slice(0, -1)) ? w.slice(0, -1) : EXTS.has(w) && (next === "files" || next === "file") ? w : null;
        if (ext) {
            add(`ext:${ext}`, `${ext.toUpperCase()} files`);
            continue;
        }
        if (w === "folders" || w === "folder") {
            add("folder:", "folders");
            continue;
        }
        // When it changed: today, yesterday, recently, this week, the last 3 days; older than 2 years.
        if (w === "today" || w === "yesterday" || w === "recent" || w === "recently" || w === "lately") {
            if (w === "today") add("dm:1d", "changed in the last day");
            else if (w === "yesterday") add("dm:2d", "changed since yesterday");
            else add("dm:7d", "changed in the last 7 days");
            continue;
        }
        if ((w === "this" || w === "last" || w === "past") && (next === "week" || next === "month" || next === "year")) {
            const [age, span] = next === "week" ? ["7d", "7 days"] : next === "month" ? ["30d", "30 days"] : ["1y", "year"];
            if (taken(media) && (lower[i - 1] === "taken" || lower[i - 1] === "shot")) add(`taken:${age}`, `taken in the last ${span}`);
            else add(`dm:${age}`, `changed in the last ${span}`);
            i++;
            continue;
        }
        const count = numberAt(next);
        if ((w === "last" || w === "past") && count && !count[2] && after && AGE_UNITS[after]) {
            const [unit, times, span] = AGE_UNITS[after];
            add(`dm:${Math.round(Number(count[1]) * times)}${unit}`, `changed in the last ${count[1]} ${span}`);
            i += 2;
            continue;
        }
        if (w === "older" && next === "than") {
            const n = numberAt(after);
            const unitWord = n?.[2] || lower[i + 3];
            if (n && unitWord && AGE_UNITS[unitWord]) {
                const [unit, times, span] = AGE_UNITS[unitWord];
                add(`dm:>${Math.round(Number(n[1]) * times)}${unit}`, `older than ${n[1]} ${span}`);
                i += n[2] ? 2 : 3;
                continue;
            }
        }
        if (w === "old") {
            add("dm:>1y", "older than a year");
            continue;
        }
        // When a photo or video was taken: taken in 2024, from 2024.
        if (w === "taken" || w === "shot" || ((w === "from" || w === "in") && taken(media))) {
            const year = YEAR.test(next ?? "") ? next : next === "in" && YEAR.test(after ?? "") ? after : null;
            if (year) {
                add(`taken:${year}`, `taken in ${year}`);
                i += year === next ? 1 : 2;
                continue;
            }
        }
        // Sizes, lengths and pixels: bigger than 5 mb, over 1.5gb, at least 10kb, longer than 5 minutes, wider than 1920.
        const said = MORE[w] ?? LESS[w];
        if (said) {
            const at = next === "than" ? i + 2 : i + 1;
            const n = numberAt(lower[at]);
            if (n) {
                const op = w === "least" ? ">=" : w === "most" ? "<=" : MORE[w] ? ">" : "<";
                const unitWord = n[2] || lower[at + 1];
                const used = n[2] ? at - i : at + 1 - i;
                if ((w === "wider" || w === "taller" || w === "narrower") && !n[2]) {
                    add(`${w === "taller" ? "height" : "width"}:${op}${n[1]}`, `${said} ${n[1]} pixels`);
                    i = at;
                    continue;
                }
                if (unitWord && DURATION_UNITS[unitWord] && (w === "longer" || w === "shorter" || media === "video" || media === "music")) {
                    const [unit, name] = DURATION_UNITS[unitWord];
                    add(`duration:${op}${n[1]}${unit}`, `${said} ${n[1]} ${name}`);
                    i += used;
                    continue;
                }
                if (unitWord && SIZE_UNITS[unitWord]) {
                    let [unit] = SIZE_UNITS[unitWord];
                    let value = Number(n[1]);
                    while (!Number.isInteger(value) && SMALLER[unit]) {
                        value *= 1024;
                        unit = SMALLER[unit];
                    }
                    add(`size:${op}${Math.round(value)}${unit}`, `${said} ${n[1]} ${SIZE_UNITS[unitWord][1]}`);
                    i += used;
                    continue;
                }
            }
        }
        if (w === "big" || w === "large" || w === "huge" || w === "small" || w === "tiny") {
            if (w === "huge") add("size:>1gb", "over 1 GB");
            else if (w === "small" || w === "tiny") add("size:<100kb", "under 100 KB");
            else add("size:>100mb", "over 100 MB");
            continue;
        }
        // Who made it: songs by an artist, photos taken with (or shot on) a camera.
        if (w === "by" && media === "music" && next) {
            const who = next === "the" && after ? i + 2 : i + 1;
            add(`artist:${lower[who]}`, `by ${words[who]}`);
            i = who;
            continue;
        }
        if ((w === "with" || w === "on") && next && !FILLER.has(next) && (lower[i - 1] === "taken" || lower[i - 1] === "shot" || media === "photo")) {
            add(`camera:${next}`, `taken with ${words[i + 1]}`);
            i++;
            continue;
        }
        // "named budget", "called budget": what follows is the name.
        if ((w === "named" || w === "called") && next) {
            names.push(words[i + 1]);
            i++;
            continue;
        }
        if (FILLER.has(w) || w === "taken" || w === "shot" || w === "than") continue;
        names.push(words[i]);
    }
    if (!filters.length) return null;
    if (names.length) chips.push(`named “${names.join(" ")}”`);
    return { query: [...names, ...filters].join(" "), chips };
}
