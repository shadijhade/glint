import { useMemo } from "react";
import type { FileEntry } from "@/lib/entries";
import { formatCount } from "@/lib/format";
import { updateTab } from "@/lib/explorer";
import { cn } from "@/lib/utils";

const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

interface Month {
    year: number;
    month: number;
    count: number;
    /** Its first photo or video, in the order shown. */
    first: FileEntry;
}

/** The months photos and videos were taken in, in the order shown (newest first), each with how many. */
function monthsOf(rows: readonly FileEntry[]): Month[] {
    const months: Month[] = [];
    for (const entry of rows) {
        if (entry.dir || !entry.modified) continue;
        const at = new Date(entry.modified);
        const last = months[months.length - 1];
        if (last && last.year === at.getFullYear() && last.month === at.getMonth()) last.count++;
        else months.push({ year: at.getFullYear(), month: at.getMonth(), count: 1, first: entry });
    }
    return months;
}

/**
 * Beside the gallery: its years and months, newest first, each with how many photos and videos it has;
 * a click goes to the month's first (selected, and scrolled to).
 */
export function GalleryMonths({ rows, tab, current }: { rows: readonly FileEntry[]; tab: string; current: string | null }) {
    const months = useMemo(() => monthsOf(rows), [rows]);
    // The month of what's selected.
    const here = useMemo(() => {
        const entry = current ? rows.find((r) => r.path === current) : undefined;
        if (!entry?.modified) return null;
        const at = new Date(entry.modified);
        return `${at.getFullYear()}-${at.getMonth()}`;
    }, [rows, current]);
    if (months.length < 2) return null;
    const go = (entry: FileEntry) => updateTab({ selection: new Set([entry.path]), focus: entry.path, anchor: entry.path }, tab);
    return (
        <nav aria-label="Months" className="hidden w-40 shrink-0 overflow-y-auto border-l py-2 text-xs md:block">
            {months.map((m, i) => (
                <div key={`${m.year}-${m.month}`}>
                    {(i === 0 || months[i - 1].year !== m.year) && <h3 className="px-3 pt-2 pb-1 font-semibold text-foreground">{m.year}</h3>}
                    <button
                        type="button"
                        onClick={() => go(m.first)}
                        className={cn(
                            "flex w-full items-center justify-between gap-2 px-3 py-1 text-left text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
                            here === `${m.year}-${m.month}` && "bg-accent/60 font-medium text-foreground"
                        )}
                    >
                        <span className="truncate">{MONTHS[m.month]}</span>
                        <span className="tabular-nums">{formatCount(m.count)}</span>
                    </button>
                </div>
            ))}
        </nav>
    );
}
