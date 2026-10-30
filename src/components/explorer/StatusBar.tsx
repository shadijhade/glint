import { useMemo, useRef } from "react";
import { ClipboardCopy, FileArchive, Fingerprint, FolderLock, ListFilter, Loader2, PenLine, Zap } from "lucide-react";
import { toast } from "sonner";
import { useShallow } from "zustand/react/shallow";
import { useUsage } from "@/lib/diskUsage";
import type { FileEntry } from "@/lib/entries";
import { compressEntries, enableIndex, updateTab, useExplorer, useTab, viewKey } from "@/lib/explorer";
import { groupFilter, TYPE_GROUPS, type TypeGroup, typeGroup } from "@/lib/fileTypes";
import { formatCount, formatDuration, formatSize } from "@/lib/format";
import { type GitRun, useGit } from "@/lib/git";
import { baseName, isRecycleBin, THIS_PC } from "@/lib/paths";
import { type ThumbRun, useThumbRuns } from "@/lib/thumbs";
import { cn } from "@/lib/utils";
import { openBatchRename } from "./BatchRenameDialog";
import { secretsCheckFirst } from "./CheckDialog";
import { UsageWork } from "./DiskUsage";
import { inspectXRay } from "./XRayDialog";

interface StatusBarProps {
    shown: number;
    hidden: number;
    selected: FileEntry[];
}

const items = (n: number) => `${formatCount(n)} ${n === 1 ? "item" : "items"}`;
const FOLDERS_GROUP: TypeGroup = { name: "Folders", hue: 38, sat: 82 };

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

/** How Git status of the folder went: how long it took, or why it waits (trust, or F5). */
function GitWork({ run }: { run: GitRun }) {
    const { view } = run;
    if (!view) return null;
    const { counts } = view;
    const changes = [
        counts.conflicted && `${formatCount(counts.conflicted)} with conflicts`,
        counts.modified && `${formatCount(counts.modified)} changed`,
        counts.staged && `${formatCount(counts.staged)} staged`,
        counts.untracked && `${formatCount(counts.untracked)} new`,
    ].filter(Boolean);
    const [said, title] = view.limited
        ? ["git · waits for trust", "This repository's own settings name programs for Git status to run (filter drivers or core.fsmonitor), so Glint doesn't look at its status until you trust it, from the branch in the address bar."]
        : view.onRequest
          ? ["git · F5 for status", "This repository has too many files (or is on another computer) to look through on every visit: F5 shows its status."]
          : [
                `git · ${formatDuration(view.elapsed)}${view.partial ? " · partial" : ""}`,
                `Git status of this folder in ${formatDuration(view.elapsed)}${view.partial ? ", stopped at 4 s: what's shown is what it found by then" : ""}: ${changes.length ? `${changes.join(", ")} in here and below` : "nothing changed in here"}${view.branch ? `, on ${view.branch}` : ""}.`,
            ];
    return (
        <span title={title} className="hidden @xl:inline">
            · {said}
        </span>
    );
}

/** Counts on the left; on the right, which tool did the work and how long it took. Clicking opens the telemetry & file-type spectrum popover. */
export function StatusBar({ shown, hidden, selected }: StatusBarProps) {
    const { tab, path, listing, noAccess, find, filter, shownIn, view } = useTab(
        useShallow((t) => ({ tab: t.id, path: t.path, listing: t.listing, noAccess: !!t.noAccess, find: t.find, filter: t.filter, shownIn: t.shownIn, view: t.view }))
    );
    const popRef = useRef<HTMLDivElement>(null);
    // A tab showing its disk-usage map tells how that was mapped; its rows are the map's items.
    const mapped = useUsage((s) => !!s[tab]) && !find;
    const git = useGit((s) => s[tab]);
    const { sizes, drives, index } = useExplorer(useShallow((s) => ({ sizes: s.sizes[path], drives: s.drives.length, index: s.index })));
    const scanning = index?.state === "on" ? index.volumes.find((v) => v.state === "scanning") : undefined;

    // Fast O(N) category breakdown of the current folder (sampled up to 4,000 entries for 0 ms latency).
    const spectrum = useMemo(() => {
        const entries = listing?.entries;
        if (!entries?.length) return [];
        const map = new Map<TypeGroup, { count: number; bytes: number; query: string }>();
        const limit = Math.min(entries.length, 4000);
        for (let i = 0; i < limit; i++) {
            const e = entries[i];
            const group = e.dir ? FOLDERS_GROUP : typeGroup(e);
            const cur = map.get(group);
            const bytes = e.dir ? (sizes?.sizes.get(e.name) ?? 0) : e.size;
            if (cur) {
                cur.count++;
                cur.bytes += bytes;
            } else {
                const query = e.dir ? "folder:" : group !== TYPE_GROUPS.other ? groupFilter(group) : "";
                map.set(group, { count: 1, bytes, query });
            }
        }
        return [...map.entries()].sort((a, b) => b[1].count - a[1].count);
    }, [listing?.entries, sizes]);

    // Offered on This PC and after a search without it; a drive being read shows in every folder.
    const turnOn =
        index && (index.state === "off" || index.state === "stopped") ? (
            <button
                type="button"
                onClick={() => void enableIndex()}
                title="Find files on every drive in milliseconds (Windows asks for administrator approval)"
                className="inline-flex shrink-0 items-center gap-1 rounded bg-primary/10 px-1.5 py-0.5 text-[0.6875rem] font-medium text-primary transition-colors hover:bg-primary/20"
            >
                <Zap className="size-3" /> Turn on instant search
            </button>
        ) : null;
    const indexing = scanning ? (
        <span className="inline-flex shrink-0 items-center gap-1">
            <Loader2 className="size-3 animate-spin text-primary" /> Indexing {scanning.volume}
            {scanning.progressTotal ? ` ${Math.floor((100 * scanning.progress) / scanning.progressTotal)}%` : ""}…
        </span>
    ) : null;
    // The thumbnails of what this tab shows now, in the view it's in, once any came.
    const thumbs = useThumbRuns((s) => s[tab]);
    const thumbsHere = !mapped && thumbs?.scope === `${viewKey({ path, find })}:${view}` && thumbs.found > 0 ? thumbs : null;
    if (path === THIS_PC && !find) {
        return (
            <div className="flex h-7 shrink-0 items-center gap-2 border-t px-3 text-xs text-muted-foreground">
                <span>{`${drives} ${drives === 1 ? "drive" : "drives"}`}</span>
                <span className="ml-auto">{turnOn ?? indexing}</span>
            </div>
        );
    }
    const selectedFiles = selected.filter((e) => !e.dir);
    const selectedBytes = selectedFiles.reduce((sum, e) => sum + e.size, 0);
    const writable = !listing?.archive && !listing?.remote && !noAccess && !isRecycleBin(path);

    const placePopover = (e: React.MouseEvent<HTMLButtonElement>) => {
        const pop = popRef.current;
        if (!pop) return;
        const rect = e.currentTarget.getBoundingClientRect();
        const width = 320;
        const left = Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8));
        pop.style.left = `${left}px`;
        pop.style.top = `${Math.max(8, rect.top - 8)}px`;
        pop.style.transform = "translateY(-100%)";
        pop.style.transformOrigin = "bottom right";
    };

    const toggleCategoryFilter = (query: string) => {
        if (!query) return;
        const next = filter.trim() === query ? "" : query;
        updateTab({ filter: next }, tab);
        popRef.current?.hidePopover();
    };

    let work: React.ReactNode = null;
    if (mapped) work = <UsageWork tab={tab} />;
    else if (find) {
        const engine = find.engine ? `${find.engine} · ` : "";
        work = find.running ? (
            <>
                <Loader2 className="size-3 animate-spin text-primary" /> {engine}searching… {formatCount(find.entries.length)} found
            </>
        ) : (
            `${engine}${find.fuzzy ? "fuzzy · " : ""}${formatCount(find.entries.length)} found in ${formatDuration(find.elapsed)}${find.limited ? " · stopped at the limit" : ""}`
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
            : `Glint read the list of what's in ${name} (${formatCount(archive.entries)} files and folders) in ${formatDuration(archive.readMs)}, without extracting anything; this folder was on screen ${formatDuration(shownIn)} after the click. The archive is read-only.`;
        work = (
            <span title={title} className="flex items-center gap-1">
                <FileArchive className="size-3 text-amber-600 dark:text-amber-500" />
                <span className="hidden @lg:inline">
                    {archive.format} · {archive.cached ? "" : `${formatCount(archive.entries)} items `}read in {formatDuration(listing.elapsed)} ·
                </span>
                <span>shown in {formatDuration(shownIn)}</span>
            </span>
        );
    } else if (path !== THIS_PC && listing?.fromIndex) {
        // A folder Windows keeps closed: listed from what the index has in it.
        work = (
            <span
                title={`Windows keeps this folder closed to your account, so Glint listed what instant search's index has in it, in ${formatDuration(listing.elapsed)}; it was on screen ${formatDuration(shownIn)} after the click. Nothing in it can be opened or changed here.`}
                className="flex items-center gap-1"
            >
                <FolderLock className="size-3 text-amber-600 dark:text-amber-500" />
                <span className="hidden @lg:inline">index · listed in {formatDuration(listing.elapsed)} ·</span>
                <span>shown in {formatDuration(shownIn)}</span>
            </span>
        );
    } else if (path !== THIS_PC && listing) {
        const sized = sizes && `${sizes.engine} sized ${formatSize(sizes.total)} in ${formatDuration(sizes.elapsed)}`;
        // A narrow pane (two side by side) keeps the time to screen, and says the rest on hover.
        work = (
            <span title={`Glint read this folder in ${formatDuration(listing.elapsed)}; it was on screen ${formatDuration(shownIn)} after the click.${sized ? ` The ${sized}.` : ""} Click for what's in it.`} className="flex items-center gap-1">
                <Zap className="size-3 text-primary" />
                <span className="hidden @lg:inline">read in {formatDuration(listing.elapsed)} ·</span>
                <span>shown in {formatDuration(shownIn)}</span>
                {sized && <span className="hidden @xl:inline">· {sized}</span>}
                {git && <GitWork run={git} />}
            </span>
        );
    }

    const totalSpectrumCount = spectrum.reduce((sum, [, v]) => sum + v.count, 0) || 1;
    const telemetryStages = [
        listing && { label: "Folder read", ms: listing.elapsed, detail: listing.archive ? listing.archive.format : listing.fromIndex ? "from the index" : "from the drive" },
        shownIn > 0 && { label: "On screen", ms: shownIn, detail: items(shown) },
        sizes && !sizes.running && { label: `Folder sizes (${sizes.engine})`, ms: sizes.elapsed, detail: formatSize(sizes.total) },
        git?.view && !git.view.limited && !git.view.onRequest && { label: "Git status", ms: git.view.elapsed, detail: git.view.branch ?? "repo" },
        thumbsHere && !thumbsHere.pending && { label: `Thumbnails (${formatCount(thumbsHere.found)})`, ms: thumbsHere.elapsed, detail: `${thumbsHere.cache} cached` },
    ].filter((x): x is { label: string; ms: number; detail: string } => !!x);
    const maxStageMs = Math.max(1, ...telemetryStages.map((s) => s.ms));

    // The counts stay whole; what did the work and how long it took gives way first (all of it in a narrow pane).
    return (
        <div className="@container flex h-7 shrink-0 items-center gap-1.5 overflow-hidden border-t px-3 text-xs text-muted-foreground">
            <span className="shrink-0">
                {noAccess && !listing && !find ? "No access" : items(shown)}
                {hidden > 0 && !mapped && ` · ${formatCount(hidden)} hidden`}
                {selected.length > 0 && ` · ${formatCount(selected.length)} selected`}
                {selectedFiles.length > 0 && ` · ${formatSize(selectedBytes)}`}
            </span>
            {/* An archive's items, a remote's, a closed folder's and the Recycle Bin's aren't files to look into or rename here. */}
            {writable && selected.length === 1 && !selected[0].dir && (
                <button
                    type="button"
                    onClick={() => inspectXRay(selected[0].path)}
                    title="File X-Ray: hashes and hidden streams"
                    className="hidden shrink-0 items-center gap-1 whitespace-nowrap rounded bg-primary/10 px-1.5 py-0.5 text-[0.6875rem] font-medium text-primary transition-all hover:bg-primary/20 active:scale-95 @md:inline-flex"
                >
                    <Fingerprint className="size-3" /> X-Ray
                </button>
            )}
            {writable && selected.length > 1 && (
                <>
                    <button
                        type="button"
                        onClick={() => openBatchRename(selected, listing?.entries ?? selected)}
                        title="Rename the selected items (F2)"
                        className="hidden shrink-0 items-center gap-1 whitespace-nowrap rounded bg-primary/10 px-1.5 py-0.5 text-[0.6875rem] font-medium text-primary transition-all hover:bg-primary/20 active:scale-95 @md:inline-flex"
                    >
                        <PenLine className="size-3" /> Rename
                    </button>
                    <button
                        type="button"
                        onClick={() => {
                            const paths = selected.map((s) => s.path);
                            void secretsCheckFirst(paths, "Zip anyway", () => compressEntries(selected));
                        }}
                        title="Compress the selected items to a .zip archive"
                        className="hidden shrink-0 items-center gap-1 whitespace-nowrap rounded bg-foreground/[0.06] px-1.5 py-0.5 text-[0.6875rem] font-medium text-foreground/85 transition-all hover:bg-accent hover:text-foreground active:scale-95 @lg:inline-flex"
                    >
                        <FileArchive className="size-3" /> Zip
                    </button>
                </>
            )}
            {selected.length > 1 && (
                <button
                    type="button"
                    onClick={() => {
                        const text = selected.map((s) => s.path).join("\r\n");
                        void navigator.clipboard.writeText(text).then(() => toast.success(`Copied ${selected.length} paths`));
                    }}
                    title="Copy selected paths (Ctrl+Shift+C)"
                    className="hidden shrink-0 items-center gap-1 whitespace-nowrap rounded bg-foreground/[0.06] px-1.5 py-0.5 text-[0.6875rem] font-medium text-foreground/85 transition-all hover:bg-accent hover:text-foreground active:scale-95 @xl:inline-flex"
                >
                    <ClipboardCopy className="size-3" /> Copy paths
                </button>
            )}
            {listing && !mapped && !find ? (
                <button
                    type="button"
                    popoverTarget={`status-${tab}`}
                    onClick={placePopover}
                    className="ml-auto hidden min-w-0 items-center gap-1 overflow-hidden whitespace-nowrap rounded-[4px] px-1.5 py-0.5 tabular-nums transition-colors hover:bg-accent hover:text-foreground @2xs:flex"
                >
                    {indexing && <span className="hidden @md:inline-flex">{indexing}</span>}
                    {work}
                    {thumbsHere && <ThumbWork run={thumbsHere} />}
                </button>
            ) : (
                <span className="ml-auto hidden min-w-0 items-center gap-1 overflow-hidden whitespace-nowrap tabular-nums @2xs:flex">
                    {indexing && <span className="hidden @md:inline-flex">{indexing}</span>}
                    {work}
                    {thumbsHere && <ThumbWork run={thumbsHere} />}
                    {find && !find.running && find.engine === "fd" && turnOn && <span className="ml-1 hidden @lg:inline-flex">{turnOn}</span>}
                </span>
            )}
            {listing && !mapped && !find && (
                <div
                    id={`status-${tab}`}
                    ref={popRef}
                    popover="auto"
                    className="fixed inset-auto m-0 w-80 rounded-lg border border-border/80 bg-popover/95 p-3 text-xs text-popover-foreground shadow-xl backdrop-blur-xl"
                >
                    <div className="flex items-center justify-between gap-2">
                        <span className="font-semibold text-foreground">What's in this folder</span>
                        <span className="text-[0.6875rem] text-muted-foreground tabular-nums">{items(listing.entries.length)}</span>
                    </div>
                    {spectrum.length > 0 && (
                        <>
                            <div className="mt-2 flex h-2 w-full gap-0.5 overflow-hidden rounded-full bg-muted p-px">
                                {spectrum.map(([group, info]) => (
                                    <button
                                        key={group.name}
                                        type="button"
                                        disabled={!info.query}
                                        onClick={() => toggleCategoryFilter(info.query)}
                                        title={`${group.name}: ${formatCount(info.count)} (${info.bytes > 0 ? formatSize(info.bytes) : "click to filter"})`}
                                        style={{
                                            width: `${Math.max(4, Math.round((info.count / totalSpectrumCount) * 100))}%`,
                                            background: `hsl(${group.hue} ${group.sat}% 52%)`,
                                        }}
                                        className="h-full rounded-xs transition-opacity hover:opacity-80 disabled:cursor-default"
                                    />
                                ))}
                            </div>
                            <div className="mt-2 flex flex-wrap gap-1">
                                {spectrum.slice(0, 6).map(([group, info]) => {
                                    const activeFilter = !!info.query && filter.trim() === info.query;
                                    return (
                                        <button
                                            key={group.name}
                                            type="button"
                                            disabled={!info.query}
                                            onClick={() => toggleCategoryFilter(info.query)}
                                            title={info.query ? `Click to filter to ${group.name.toLowerCase()}` : group.name}
                                            className={cn(
                                                "flex items-center gap-1.5 rounded-md border px-1.5 py-0.5 text-[0.6875rem] transition-all hover:bg-accent active:scale-95 disabled:pointer-events-none",
                                                activeFilter ? "border-primary bg-primary/15 font-medium text-primary" : "border-border/60 text-muted-foreground hover:text-foreground"
                                            )}
                                        >
                                            <span className="size-2 rounded-full" style={{ background: `hsl(${group.hue} ${group.sat}% 52%)` }} />
                                            <span>{group.name}</span>
                                            <span className="font-mono text-[0.625rem] opacity-75">{formatCount(info.count)}</span>
                                        </button>
                                    );
                                })}
                            </div>
                        </>
                    )}
                    {filter && (
                        <button
                            type="button"
                            onClick={() => {
                                updateTab({ filter: "" }, tab);
                                popRef.current?.hidePopover();
                            }}
                            className="mt-2 flex w-full items-center justify-center gap-1.5 rounded-md bg-primary/12 py-1 text-[0.6875rem] font-medium text-primary hover:bg-primary/20"
                        >
                            <ListFilter className="size-3" /> Show everything again
                        </button>
                    )}
                    {telemetryStages.length > 0 && (
                        <div className="mt-3 border-t border-border/70 pt-2.5">
                            <div className="mb-1.5 flex items-center gap-1.5 font-semibold text-foreground">
                                <Zap className="size-3.5 text-primary" /> How long it took
                            </div>
                            <div className="space-y-1.5">
                                {telemetryStages.map((stage) => (
                                    <div key={stage.label} className="space-y-0.5">
                                        <div className="flex items-center justify-between text-[0.6875rem]">
                                            <span className="text-muted-foreground">
                                                {stage.label} <span className="opacity-65">· {stage.detail}</span>
                                            </span>
                                            <span className="font-mono font-medium text-foreground tabular-nums">{formatDuration(stage.ms)}</span>
                                        </div>
                                        <div className="h-1 w-full overflow-hidden rounded-full bg-muted">
                                            <div
                                                className="h-full origin-left rounded-full bg-primary/75"
                                                style={{ transform: `scaleX(${Math.max(0.06, Math.min(1, stage.ms / maxStageMs))})` }}
                                            />
                                        </div>
                                    </div>
                                ))}
                            </div>
                        </div>
                    )}
                </div>
            )}
        </div>
    );
}
