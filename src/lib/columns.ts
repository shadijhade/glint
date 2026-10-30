import type { FileEntry, Sort, SortKey } from "./entries.ts";
import { TYPE_GROUPS, typeGroup } from "./fileTypes.ts";
import type { Column } from "./store.ts";
import type { ViewMode } from "./store.ts";

/** The list's columns besides the name (and, in a list of items from many folders, where each is). */
export type ColumnId = "modified" | "taken" | "type" | "size" | "dimensions" | "duration" | "artist" | "album";

export interface ColumnDef {
    label: string;
    /** Its width's name: `--col-<width>`, dragged in the header (settings.columns). */
    width: Column;
    sort: SortKey;
    /** Left out first in a narrow list ("md": under 42rem), or only in a narrow pane ("sm": under 26rem). */
    drop: "md" | "sm" | null;
    /** Read from what photos, videos and songs say about themselves (media.ts). */
    facts?: boolean;
    /** Right-aligned: numbers that compare by their ends. */
    end?: boolean;
}

export const COLUMNS: Record<ColumnId, ColumnDef> = {
    modified: { label: "Date modified", width: "date", sort: "modified", drop: "sm" },
    taken: { label: "Date taken", width: "taken", sort: "taken", drop: "md", facts: true },
    type: { label: "Type", width: "type", sort: "type", drop: "md" },
    dimensions: { label: "Dimensions", width: "dimensions", sort: "dimensions", drop: "md", facts: true },
    duration: { label: "Duration", width: "duration", sort: "duration", drop: "md", facts: true, end: true },
    artist: { label: "Artist", width: "artist", sort: "artist", drop: "md", facts: true },
    album: { label: "Album", width: "album", sort: "album", drop: "md", facts: true },
    size: { label: "Size", width: "size", sort: "size", drop: null, end: true },
};

/** Sorts by what media files say about themselves (read before they sort). */
export const FACT_SORTS: SortKey[] = ["taken", "dimensions", "duration", "artist", "album"];

/** Sorts whose first click puts the biggest, longest or newest first. */
export const DESCENDING: SortKey[] = ["modified", "taken", "size", "dimensions", "duration"];

/** The order columns show in, whichever are chosen. */
export const COLUMN_ORDER: ColumnId[] = ["modified", "taken", "type", "dimensions", "duration", "artist", "album", "size"];

/** Chosen columns in their order (unknown ones left out). */
export const inOrder = (columns: readonly string[]) => COLUMN_ORDER.filter((id) => columns.includes(id));

/** Kinds of folder, each with the columns (and sort) it opens with until it's given its own. */
export type FolderKind = "general" | "pictures" | "videos" | "music" | "downloads" | "recent" | "gallery";

export const KINDS: Record<FolderKind, { label: string; columns: ColumnId[]; sort?: Sort; view?: ViewMode }> = {
    general: { label: "Any folder", columns: ["modified", "type", "size"] },
    pictures: { label: "Pictures", columns: ["taken", "dimensions", "type", "size"] },
    videos: { label: "Videos", columns: ["modified", "duration", "dimensions", "size"] },
    music: { label: "Music", columns: ["artist", "album", "duration", "size"] },
    downloads: { label: "Downloads", columns: ["modified", "type", "size"], sort: { key: "modified", desc: true } },
    recent: { label: "Recent", columns: ["modified", "size"], sort: { key: "modified", desc: true } },
    gallery: { label: "Gallery", columns: ["modified", "dimensions", "size"], sort: { key: "modified", desc: true }, view: "grid" },
};

/**
 * What kind of folder a listing is: Recent and Downloads by where they are, else by what's in it: mostly
 * pictures, videos or songs (at least five files, three in five of them of one kind), or any.
 */
export function folderKind(entries: readonly FileEntry[], place: "recent" | "gallery" | "downloads" | null): FolderKind {
    if (place) return place;
    let files = 0;
    const counts = { pictures: 0, videos: 0, music: 0 };
    for (const e of entries) {
        if (e.dir) continue;
        files++;
        const group = typeGroup(e);
        if (group === TYPE_GROUPS.images) counts.pictures++;
        else if (group === TYPE_GROUPS.video) counts.videos++;
        else if (group === TYPE_GROUPS.audio) counts.music++;
    }
    if (files < 5) return "general";
    const [kind, count] = (Object.entries(counts) as [FolderKind, number][]).reduce((best, next) => (next[1] > best[1] ? next : best));
    // Photos and videos together (a phone's camera folder): the kind there's more of.
    const media = counts.pictures + counts.videos;
    if ((kind === "pictures" || kind === "videos") && media >= files * 0.6) return counts.pictures >= counts.videos ? "pictures" : "videos";
    return count >= files * 0.6 ? kind : "general";
}

/** Where a column is left out as the list narrows: the date goes sooner among items from many folders. */
const dropOf = (id: ColumnId, located: boolean) => (located && id === "modified" ? "md" : COLUMNS[id].drop);

/** The class that leaves a column's cells out as the list narrows (index.css). */
export function dropClass(id: ColumnId, located: boolean) {
    const drop = dropOf(id, located);
    return drop === "md" ? "col-optional" : drop === "sm" ? "col-narrow" : undefined;
}

/** The row's grid: the name, where each item is (in a list of items from many folders), then the
 * columns; as wide as it is, narrow (under 42rem) and in a narrow pane (under 26rem). */
export function gridTemplates(columns: readonly ColumnId[], located: boolean) {
    const width = (id: ColumnId) => `var(--col-${COLUMNS[id].width})`;
    const start = ["minmax(0, 1fr)", ...(located ? ["minmax(0, 0.9fr)"] : [])];
    return {
        full: [...start, ...columns.map(width)].join(" "),
        md: [...start, ...columns.filter((id) => dropOf(id, located) !== "md").map(width)].join(" "),
        sm: [...start, ...columns.filter((id) => !dropOf(id, located)).map(width)].join(" "),
    };
}

/** A song's or a video's length: "3:05", "1:02:03". */
export function clockTime(seconds: number) {
    const s = Math.round(seconds);
    const [h, m, rest] = [Math.floor(s / 3600), Math.floor((s % 3600) / 60), s % 60];
    const two = (n: number) => String(n).padStart(2, "0");
    return h ? `${h}:${two(m)}:${two(rest)}` : `${m}:${two(rest)}`;
}
