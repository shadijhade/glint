import type { FileEntry } from "./entries.ts";
import { kindOf, TYPE_GROUPS, typeLabel } from "./fileTypes.ts";
import type { GitMark } from "./fs.ts";

/** What a folder's (or a search's) items are grouped by, as File Explorer's Group by: headers over the rows. */
export type GroupBy = "none" | "kind" | "date" | "size" | "type" | "git";

export const GROUP_BYS: { by: GroupBy; label: string }[] = [
    { by: "none", label: "Nothing" },
    { by: "kind", label: "Kind" },
    { by: "date", label: "Date modified" },
    { by: "size", label: "Size" },
    { by: "type", label: "Type" },
    { by: "git", label: "Git status" },
];

/** A group's place among the others and its header. */
interface Group {
    rank: number;
    label: string;
}

const DAY = 86_400_000;
const KIND_ORDER = Object.values(TYPE_GROUPS).map((g) => g.name);

/** The group `entry` falls in. `now` is when the grouping is made (for dates), `sizeOf` a folder's size. */
export function groupOf(entry: FileEntry, by: GroupBy, now: number, sizeOf?: (e: FileEntry) => number, git?: Readonly<Record<string, GitMark>>): Group {
    switch (by) {
        case "kind": {
            if (entry.dir) return { rank: -1, label: "Folders" };
            const name = kindOf(entry).group.name;
            return { rank: KIND_ORDER.indexOf(name), label: name };
        }
        case "type":
            return entry.dir ? { rank: -1, label: "Folders" } : { rank: 0, label: typeLabel(entry) };
        case "date": {
            if (!entry.modified) return { rank: 99, label: "Unknown" };
            const today = new Date(now);
            today.setHours(0, 0, 0, 0);
            const start = today.getTime();
            const weekStart = start - ((today.getDay() + 6) % 7) * DAY;
            const monthStart = new Date(today.getFullYear(), today.getMonth(), 1).getTime();
            const lastMonth = new Date(today.getFullYear(), today.getMonth() - 1, 1).getTime();
            const yearStart = new Date(today.getFullYear(), 0, 1).getTime();
            const t = entry.modified;
            if (t >= start + DAY) return { rank: 0, label: "Later" };
            if (t >= start) return { rank: 1, label: "Today" };
            if (t >= start - DAY) return { rank: 2, label: "Yesterday" };
            if (t >= weekStart) return { rank: 3, label: "Earlier this week" };
            if (t >= weekStart - 7 * DAY) return { rank: 4, label: "Last week" };
            if (t >= monthStart) return { rank: 5, label: "Earlier this month" };
            if (t >= lastMonth) return { rank: 6, label: "Last month" };
            if (t >= yearStart) return { rank: 7, label: "Earlier this year" };
            const year = new Date(t).getFullYear();
            return { rank: 8 + (today.getFullYear() - year), label: String(year) };
        }
        case "size": {
            const size = entry.dir ? (sizeOf?.(entry) ?? -1) : entry.size;
            if (entry.dir && size < 0) return { rank: -1, label: "Folders" };
            if (size === 0) return { rank: 0, label: "Empty" };
            if (size < 16 << 10) return { rank: 1, label: "Tiny (under 16 KB)" };
            if (size < 1 << 20) return { rank: 2, label: "Small (16 KB to 1 MB)" };
            if (size < 128 << 20) return { rank: 3, label: "Medium (1 to 128 MB)" };
            if (size < 1 << 30) return { rank: 4, label: "Large (128 MB to 1 GB)" };
            return { rank: 5, label: "Huge (over 1 GB)" };
        }
        case "git": {
            const mark = git?.[entry.lower];
            const order: Record<string, Group> = {
                conflicted: { rank: 0, label: "Conflicts" },
                modified: { rank: 1, label: "Changed" },
                staged: { rank: 2, label: "Staged" },
                untracked: { rank: 3, label: "New" },
                ignored: { rank: 5, label: "Ignored" },
            };
            return (mark && order[mark]) || { rank: 4, label: "Unchanged" };
        }
        default:
            return { rank: 0, label: "" };
    }
}

/** A header: its label, how many items it has and their size, and where its first row is. */
export interface GroupHeader {
    label: string;
    count: number;
    size: number;
    first: number;
}

/** Rows grouped: the rows, ordered group by group (each group keeping its order), and the headers. */
export interface Grouped {
    rows: FileEntry[];
    headers: GroupHeader[];
    /** Each list line: a row's index, or (as -1 - i) header i. */
    lines: Int32Array;
    /** Each row's line. */
    lineOf: Int32Array;
}

/** `rows` (sorted) grouped `by`: group order first, the sort within each. */
export function groupRows(rows: FileEntry[], by: GroupBy, now: number, sizeOf?: (e: FileEntry) => number, git?: Readonly<Record<string, GitMark>>): Grouped {
    const keyed = rows.map((entry, i) => ({ entry, i, group: groupOf(entry, by, now, sizeOf, git) }));
    keyed.sort((a, b) => a.group.rank - b.group.rank || (a.group.label === b.group.label ? 0 : a.group.label < b.group.label ? -1 : 1) || a.i - b.i);
    const ordered: FileEntry[] = [];
    const headers: GroupHeader[] = [];
    const lines: number[] = [];
    const lineOf = new Int32Array(rows.length);
    let current: string | null = null;
    for (const { entry, group } of keyed) {
        if (group.label !== current) {
            current = group.label;
            headers.push({ label: group.label, count: 0, size: 0, first: ordered.length });
            lines.push(-headers.length);
        }
        const header = headers[headers.length - 1];
        header.count++;
        header.size += entry.dir ? Math.max(0, sizeOf?.(entry) ?? 0) : entry.size;
        lineOf[ordered.length] = lines.length;
        lines.push(ordered.length);
        ordered.push(entry);
    }
    return { rows: ordered, headers, lines: Int32Array.from(lines), lineOf };
}
