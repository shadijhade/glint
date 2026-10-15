import { Loader2, Zap } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import type { FileEntry } from "@/lib/entries";
import { useExplorer } from "@/lib/explorer";
import { formatCount, formatDuration, formatSize } from "@/lib/format";
import { THIS_PC } from "@/lib/paths";

interface StatusBarProps {
    shown: number;
    hidden: number;
    selected: FileEntry[];
}

const items = (n: number) => `${formatCount(n)} ${n === 1 ? "item" : "items"}`;

/** Counts on the left; on the right, which tool did the work and how long it took. */
export function StatusBar({ shown, hidden, selected }: StatusBarProps) {
    const { path, listing, find, shownIn, sizes, drives } = useExplorer(
        useShallow((s) => ({ path: s.path, listing: s.listing, find: s.find, shownIn: s.shownIn, sizes: s.sizes[s.path], drives: s.drives.length }))
    );
    if (path === THIS_PC && !find) {
        return <div className="flex h-7 shrink-0 items-center border-t bg-card/40 px-3 text-xs text-muted-foreground">{`${drives} ${drives === 1 ? "drive" : "drives"}`}</div>;
    }
    const selectedFiles = selected.filter((e) => !e.dir);
    const selectedBytes = selectedFiles.reduce((sum, e) => sum + e.size, 0);

    let work: React.ReactNode = null;
    if (find) {
        const engine = find.engine ? `${find.engine} · ` : "";
        work = find.running ? (
            <>
                <Loader2 className="size-3 animate-spin text-primary" /> {engine}searching… {formatCount(find.entries.length)} found
            </>
        ) : (
            `${engine}${formatCount(find.entries.length)} found in ${formatDuration(find.elapsed)}${find.limited ? " · stopped at the limit" : ""}`
        );
    } else if (sizes?.running) {
        work = (
            <>
                <Loader2 className="size-3 animate-spin text-primary" /> calculating folder sizes…
            </>
        );
    } else if (path !== THIS_PC && listing) {
        work = (
            <span title={`Rust read this folder in ${formatDuration(listing.elapsed)}; it was on screen ${formatDuration(shownIn)} after the click.`} className="flex items-center gap-1">
                <Zap className="size-3 text-primary" /> read in {formatDuration(listing.elapsed)} · shown in {formatDuration(shownIn)}
                {sizes && ` · ${sizes.engine} sized ${formatSize(sizes.total)} in ${formatDuration(sizes.elapsed)}`}
            </span>
        );
    }

    return (
        <div className="flex h-7 shrink-0 items-center gap-3 border-t bg-card/40 px-3 text-xs text-muted-foreground">
            <span className="truncate">
                {items(shown)}
                {hidden > 0 && ` · ${formatCount(hidden)} hidden`}
                {selected.length > 0 && ` · ${formatCount(selected.length)} selected`}
                {selectedFiles.length > 0 && ` · ${formatSize(selectedBytes)}`}
            </span>
            <span className="ml-auto flex shrink-0 items-center gap-1 tabular-nums">{work}</span>
        </div>
    );
}
