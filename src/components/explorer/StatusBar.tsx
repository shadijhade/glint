import { FileArchive, Fingerprint, Loader2, PenLine, Zap } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { useUsage } from "@/lib/diskUsage";
import type { FileEntry } from "@/lib/entries";
import { useExplorer, useTab, viewKey } from "@/lib/explorer";
import { formatCount, formatDuration, formatSize } from "@/lib/format";
import { baseName, THIS_PC } from "@/lib/paths";
import { type ThumbRun, useThumbRuns } from "@/lib/thumbs";
import { openBatchRename } from "./BatchRenameDialog";
import { UsageWork } from "./DiskUsage";
import { inspectXRay } from "./XRayDialog";

interface StatusBarProps {
    shown: number;
    hidden: number;
    selected: FileEntry[];
}

const items = (n: number) => `${formatCount(n)} ${n === 1 ? "item" : "items"}`;

/** How the thumbnails on screen came in: how many, how fast, and from where. */
function ThumbWork({ run }: { run: ThumbRun }) {
    const noun = run.found === 1 ? "thumbnail" : "thumbnails";
    if (run.pending) {
        return (
            <span className="hidden items-center gap-1 @lg:flex">
                · <Loader2 className="size-3 animate-spin text-primary" /> {formatCount(run.found)} {noun}…
            </span>
        );
    }
    const from = [
        run.cache && `${formatCount(run.cache)} from Windows' thumbnail cache`,
        run.made && `${formatCount(run.made)} made from their files (${formatDuration(run.madeMs / run.made)} each)`,
        run.memory && `${formatCount(run.memory)} from memory`,
    ].filter(Boolean);
    const rate = run.elapsed > 0 ? Math.round(run.found / (run.elapsed / 1000)) : 0;
    return (
        <span
            title={`Thumbnails of the items on screen, ${formatDuration(run.elapsed)} from the first request to the last${rate ? ` (${formatCount(rate)} a second)` : ""}: ${from.join(", ")}.`}
            className="hidden @lg:inline"
        >
            · {formatCount(run.found)} {noun} in {formatDuration(run.elapsed)}
        </span>
    );
}

/** Counts on the left; on the right, which tool did the work and how long it took. */
export function StatusBar({ shown, hidden, selected }: StatusBarProps) {
    const { tab, path, listing, find, shownIn, view } = useTab(useShallow((t) => ({ tab: t.id, path: t.path, listing: t.listing, find: t.find, shownIn: t.shownIn, view: t.view })));
    // A tab showing its disk-usage map tells how that was mapped; its rows are the map's items.
    const mapped = useUsage((s) => !!s[tab]) && !find;
    const { sizes, drives } = useExplorer(useShallow((s) => ({ sizes: s.sizes[path], drives: s.drives.length })));
    // The thumbnails of what this tab shows now, in the view it's in, once any came.
    const thumbs = useThumbRuns((s) => s[tab]);
    const thumbsHere = !mapped && thumbs?.scope === `${viewKey({ path, find })}:${view}` && thumbs.found > 0 ? thumbs : null;
    if (path === THIS_PC && !find) {
        return <div className="flex h-7 shrink-0 items-center border-t px-3 text-xs text-muted-foreground">{`${drives} ${drives === 1 ? "drive" : "drives"}`}</div>;
    }
    const selectedFiles = selected.filter((e) => !e.dir);
    const selectedBytes = selectedFiles.reduce((sum, e) => sum + e.size, 0);

    let work: React.ReactNode = null;
    if (mapped) work = <UsageWork tab={tab} />;
    else if (find) {
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
    } else if (path !== THIS_PC && listing?.archive) {
        // A folder in an archive: listed from the archive's own list of entries, read once.
        const { archive } = listing;
        const name = baseName(archive.path);
        const title = archive.cached
            ? `Glint already had the list of what's in ${name} (${formatCount(archive.entries)} files and folders) in memory, and listed this folder from it in ${formatDuration(listing.elapsed)}; it was on screen ${formatDuration(shownIn)} after the click. The archive is read-only.`
            : `Rust read the list of what's in ${name} (${formatCount(archive.entries)} files and folders) in ${formatDuration(archive.readMs)}, without extracting anything; this folder was on screen ${formatDuration(shownIn)} after the click. The archive is read-only.`;
        work = (
            <span title={title} className="flex items-center gap-1">
                <FileArchive className="size-3 text-amber-600 dark:text-amber-500" />
                <span className="hidden @lg:inline">
                    {archive.format} · {archive.cached ? "" : `${formatCount(archive.entries)} items `}read in {formatDuration(listing.elapsed)} ·
                </span>
                <span>shown in {formatDuration(shownIn)}</span>
            </span>
        );
    } else if (path !== THIS_PC && listing) {
        const sized = sizes && `${sizes.engine} sized ${formatSize(sizes.total)} in ${formatDuration(sizes.elapsed)}`;
        // A narrow pane (two side by side) keeps the time to screen, and says the rest on hover.
        work = (
            <span title={`Rust read this folder in ${formatDuration(listing.elapsed)}; it was on screen ${formatDuration(shownIn)} after the click.${sized ? ` The ${sized}.` : ""}`} className="flex items-center gap-1">
                <Zap className="size-3 text-primary" />
                <span className="hidden @lg:inline">read in {formatDuration(listing.elapsed)} ·</span>
                <span>shown in {formatDuration(shownIn)}</span>
                {sized && <span className="hidden @xl:inline">· {sized}</span>}
            </span>
        );
    }

    return (
        <div className="@container flex h-7 shrink-0 items-center gap-2 overflow-hidden border-t px-3 text-xs text-muted-foreground">
            <span className="truncate">
                {items(shown)}
                {hidden > 0 && !mapped && ` · ${formatCount(hidden)} hidden`}
                {selected.length > 0 && ` · ${formatCount(selected.length)} selected`}
                {selectedFiles.length > 0 && ` · ${formatSize(selectedBytes)}`}
            </span>
            {!listing?.archive && selected.length === 1 && !selected[0].dir && (
                <button
                    type="button"
                    onClick={() => inspectXRay(selected[0].path)}
                    title="File X-Ray: hashes and hidden streams"
                    className="hidden items-center gap-1 rounded bg-primary/10 px-1.5 py-0.5 text-[11px] font-medium text-primary transition-colors hover:bg-primary/20 @md:inline-flex"
                >
                    <Fingerprint className="size-3" /> X-Ray
                </button>
            )}
            {!listing?.archive && selected.length > 1 && (
                <button
                    type="button"
                    onClick={() => openBatchRename(selected, listing?.entries ?? selected)}
                    title="Rename the selected items (F2)"
                    className="hidden items-center gap-1 rounded bg-primary/10 px-1.5 py-0.5 text-[11px] font-medium text-primary transition-colors hover:bg-primary/20 @md:inline-flex"
                >
                    <PenLine className="size-3" /> Rename
                </button>
            )}
            <span className="ml-auto flex shrink-0 items-center gap-1 whitespace-nowrap tabular-nums">
                {work}
                {thumbsHere && <ThumbWork run={thumbsHere} />}
            </span>
        </div>
    );
}
