import { useEffect, useMemo, useState } from "react";
import { create } from "zustand";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
    Activity,
    AlertTriangle,
    Clock,
    FileClock,
    FileEdit,
    FilePlus2,
    FileX2,
    Flame,
    FolderInput,
    FolderOpen,
    Loader2,
    RotateCw,
    Search,
    X,
} from "lucide-react";
import { navigate, revealInApp } from "@/lib/explorer";
import { formatCount, formatDate, formatFullDate, formatSize } from "@/lib/format";
import { cancelTimeline, scanTimeline, type TimelineReport } from "@/lib/fs";
import { baseName } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, pressable } from "../Modal";

interface TimelineState {
    open: boolean;
    root: string;
    windowMs: number;
    loading: boolean;
    report: TimelineReport | null;
    error: string | null;
}

const DEFAULT_WINDOW_MS = 60 * 60 * 1000; // 1 hour

const useTimelineStore = create<TimelineState>(() => ({
    open: false,
    root: "",
    windowMs: DEFAULT_WINDOW_MS,
    loading: false,
    report: null,
    error: null,
}));

const TIME_WINDOWS: { label: string; title: string; ms: number }[] = [
    { label: "15m", title: "Last 15 minutes", ms: 15 * 60 * 1000 },
    { label: "1h", title: "Last hour", ms: 60 * 60 * 1000 },
    { label: "6h", title: "Last 6 hours", ms: 6 * 60 * 60 * 1000 },
    { label: "24h", title: "Last 24 hours", ms: 24 * 60 * 60 * 1000 },
    { label: "7d", title: "Last 7 days", ms: 7 * 24 * 60 * 60 * 1000 },
    { label: "30d", title: "Last 30 days", ms: 30 * 24 * 60 * 60 * 1000 },
];

let activeScanId = 0;

export async function inspectTimeline(root: string, windowMs = useTimelineStore.getState().windowMs) {
    if (!root) return;
    const scanId = ++activeScanId;
    useTimelineStore.setState({ open: true, root, windowMs, loading: true, error: null });
    try {
        const report = await scanTimeline(root, windowMs);
        if (scanId === activeScanId) {
            useTimelineStore.setState({ report, loading: false });
        }
    } catch (e) {
        if (scanId === activeScanId) {
            useTimelineStore.setState({ error: String(e), loading: false });
        }
    }
}

function closeTimeline() {
    activeScanId++;
    cancelTimeline().catch(() => {});
    useTimelineStore.setState({ open: false });
}

export function TimelineDialog() {
    const { open, root, windowMs, loading, report, error } = useTimelineStore();
    const [tab, setTab] = useState<"history" | "live">("history");
    const [query, setQuery] = useState("");
    const [folderFilter, setFolderFilter] = useState<string | null>(null);

    useEffect(() => {
        if (!open) return;
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") {
                e.preventDefault();
                e.stopPropagation();
                closeTimeline();
            }
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [open]);

    useEffect(() => {
        setFolderFilter(null);
    }, [root, windowMs]);

    const filteredEntries = useMemo(() => {
        if (!report) return [];
        const q = query.trim().toLowerCase();
        return report.entries.filter((e) => {
            if (folderFilter && e.folder.toLowerCase() !== folderFilter.toLowerCase()) return false;
            if (!q) return true;
            return e.name.toLowerCase().includes(q) || e.path.toLowerCase().includes(q);
        });
    }, [report, query, folderFilter]);

    const filteredLive = useMemo(() => {
        if (!report) return [];
        const q = query.trim().toLowerCase();
        if (!q) return report.liveEvents;
        return report.liveEvents.filter((e) => e.name.toLowerCase().includes(q) || e.path.toLowerCase().includes(q));
    }, [report, query]);

    if (!open) return null;

    const maxHotBytes = report?.hotFolders[0]?.bytesChanged || 1;

    return (
        <Modal label="Recent changes" onClose={closeTimeline} className="max-h-[86vh] max-w-4xl">
            {/* Header */}
            <div className="flex items-center gap-2.5 border-b px-4 py-3">
                <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
                    <FileClock className="size-4" />
                </div>
                <div className="min-w-0 flex-1">
                    <h2 className="truncate text-sm font-semibold">Recent changes in “{baseName(root) || root}”</h2>
                    <p className="truncate text-xs text-muted-foreground" title={root}>
                        {root}
                    </p>
                </div>

                {/* Time-horizon scrubbers */}
                <div className="flex items-center gap-0.5 rounded-md border bg-muted/40 p-0.5">
                    {TIME_WINDOWS.map((w) => (
                        <button
                            key={w.label}
                            type="button"
                            title={w.title}
                            onClick={() => inspectTimeline(root, w.ms)}
                            className={cn(
                                "rounded px-2 py-0.5 text-xs font-medium transition-colors",
                                windowMs === w.ms
                                    ? "bg-background text-foreground shadow-2xs"
                                    : "text-muted-foreground hover:text-foreground"
                            )}
                        >
                            {w.label}
                        </button>
                    ))}
                </div>

                <Button variant="ghost" size="icon" className="size-7" onClick={() => inspectTimeline(root, windowMs)} disabled={loading} title="Rescan">
                    <RotateCw className={cn("size-3.5", loading && "animate-spin")} />
                </Button>
                <Button variant="ghost" size="icon" className="size-7" onClick={closeTimeline} title="Close (Esc)">
                    <X className="size-4" />
                </Button>
            </div>

            {/* Mode tabs & filter bar */}
            <div className="flex flex-wrap items-center gap-2 border-b bg-muted/20 px-4 py-2 text-xs">
                <button
                    type="button"
                    onClick={() => setTab("history")}
                    className={cn(
                        "inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 font-medium transition-colors",
                        tab === "history"
                            ? "bg-primary/15 text-primary"
                            : "text-muted-foreground hover:bg-muted/60 hover:text-foreground"
                    )}
                >
                    <Clock className="size-3.5" />
                    Modified in window
                    {report && (
                        <span className="rounded-full bg-primary/20 px-1.5 py-0.2 text-[10px] font-semibold tabular-nums">
                            {formatCount(report.totalFilesChanged)}
                        </span>
                    )}
                </button>
                <button
                    type="button"
                    onClick={() => setTab("live")}
                    className={cn(
                        "inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 font-medium transition-colors",
                        tab === "live"
                            ? "bg-emerald-500/15 text-emerald-700 dark:text-emerald-400"
                            : "text-muted-foreground hover:bg-muted/60 hover:text-foreground"
                    )}
                >
                    <Activity className="size-3.5" />
                    Live
                    {report && (
                        <span className="rounded-full bg-emerald-500/20 px-1.5 py-0.2 text-[10px] font-semibold tabular-nums">
                            {formatCount(report.liveEvents.length)}
                        </span>
                    )}
                </button>

                {folderFilter && tab === "history" && (
                    <button
                        type="button"
                        onClick={() => setFolderFilter(null)}
                        className="inline-flex items-center gap-1 rounded-md bg-amber-500/15 px-2 py-0.5 font-medium text-amber-700 dark:text-amber-400"
                        title="Clear subfolder filter"
                    >
                        In: {baseName(folderFilter) || folderFilter}
                        <X className="size-3" />
                    </button>
                )}

                <div className="relative ml-auto w-56">
                    <Search className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                    <input
                        type="text"
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        placeholder="Filter files by name or path…"
                        className="h-7 w-full rounded-md border bg-background pl-7 pr-2 text-xs outline-none focus:border-primary"
                    />
                </div>
            </div>

            {/* Body */}
            <div className="min-h-[20rem] flex-1 overflow-y-auto p-4">
                {loading && (
                    <div className="flex h-64 flex-col items-center justify-center gap-2.5 text-sm text-muted-foreground">
                        <Loader2 className="size-6 animate-spin text-primary" />
                        <p>Scanning modification timestamps across {baseName(root) || root}…</p>
                    </div>
                )}

                {!loading && error && (
                    <div className="flex h-64 flex-col items-center justify-center gap-2 text-center text-sm">
                        <AlertTriangle className="size-6 text-destructive" />
                        <p className="font-medium">Couldn't scan folder timeline</p>
                        <p className="max-w-md text-xs text-muted-foreground">{error}</p>
                    </div>
                )}

                {!loading && !error && report && tab === "history" && (
                    <div className="space-y-4">
                        {/* Summary banner */}
                        <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border bg-muted/20 px-3.5 py-2.5 text-xs">
                            <div>
                                <span className="font-semibold text-foreground">{formatCount(report.totalFilesChanged)}</span>{" "}
                                {report.totalFilesChanged === 1 ? "file" : "files"} touched (
                                <span className="font-semibold text-primary">{formatSize(report.totalBytesChanged)}</span>)
                            </div>
                            <div className="text-muted-foreground">
                                Looked at {formatCount(report.filesScanned)} files in{" "}
                                <span className="font-mono text-foreground">{report.elapsedMs.toFixed(1)} ms</span>
                                {report.stoppedEarly && <span className="text-amber-800 dark:text-amber-400"> · stopped there: the folder has more</span>}
                            </div>
                        </div>

                        {/* Hot Subfolders breakdown */}
                        {report.hotFolders.length > 0 && (
                            <div className="space-y-2 rounded-lg border bg-card p-3">
                                <div className="flex items-center gap-1.5 text-xs font-semibold text-foreground">
                                    <Flame className="size-3.5 text-orange-500" />
                                    Busiest folders (by bytes changed)
                                </div>
                                <div className="grid gap-1.5 sm:grid-cols-2">
                                    {report.hotFolders.slice(0, 6).map((hf) => {
                                        const pct = Math.max(4, Math.round((hf.bytesChanged / maxHotBytes) * 100));
                                        const active = folderFilter?.toLowerCase() === hf.path.toLowerCase();
                                        return (
                                            <div
                                                key={hf.path}
                                                role="button"
                                                aria-pressed={active}
                                                aria-label={`Only changes in ${hf.name}`}
                                                {...pressable(() => setFolderFilter(active ? null : hf.path))}
                                                className={cn(
                                                    "group relative cursor-pointer overflow-hidden rounded-md border px-2.5 py-1.5 text-xs transition-colors",
                                                    active ? "border-primary bg-primary/5" : "hover:bg-muted/40"
                                                )}
                                            >
                                                <div
                                                    className="pointer-events-none absolute inset-y-0 left-0 bg-orange-500/10 transition-all"
                                                    style={{ width: `${pct}%` }}
                                                />
                                                <div className="relative flex items-center gap-2">
                                                    <span className="min-w-0 flex-1 truncate font-medium" title={hf.path}>
                                                        {hf.name}
                                                    </span>
                                                    <span className="shrink-0 font-mono text-[11px] font-semibold text-orange-600 dark:text-orange-400 tabular-nums">
                                                        {formatSize(hf.bytesChanged)}
                                                    </span>
                                                    <span className="shrink-0 text-[10px] text-muted-foreground tabular-nums">
                                                        ({formatCount(hf.filesChanged)})
                                                    </span>
                                                    <button
                                                        type="button"
                                                        onClick={(e) => {
                                                            e.stopPropagation();
                                                            closeTimeline();
                                                            navigate(hf.path);
                                                        }}
                                                        title="Open folder in Glint"
                                                        className="rounded p-0.5 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:bg-muted hover:text-foreground"
                                                    >
                                                        <FolderOpen className="size-3.5" />
                                                    </button>
                                                </div>
                                            </div>
                                        );
                                    })}
                                </div>
                            </div>
                        )}

                        {/* Chronological files list */}
                        {filteredEntries.length === 0 ? (
                            <div className="flex h-44 flex-col items-center justify-center gap-2 text-center text-sm text-muted-foreground">
                                <Clock className="size-7 stroke-[1.25]" />
                                <p className="font-medium text-foreground">No files modified in this time window</p>
                                <p className="text-xs">Try selecting a wider time horizon above (e.g. 24h, 7d, or 30d).</p>
                            </div>
                        ) : (
                            <div className="divide-y rounded-lg border bg-card">
                                {report.entries.length < report.totalFilesChanged && (
                                    <p className="px-3 py-2 text-xs text-muted-foreground">
                                        The newest {formatCount(report.entries.length)} of {formatCount(report.totalFilesChanged)} files touched
                                    </p>
                                )}
                                {filteredEntries.map((item) => {
                                    const relFolder = item.folder.startsWith(root)
                                        ? item.folder.slice(root.length).replace(/^\\+/, "") || "."
                                        : item.folder;
                                    return (
                                        <div key={item.path} className="flex items-center gap-3 px-3 py-2 text-xs hover:bg-muted/30">
                                            <div className="min-w-0 flex-1">
                                                <div className="flex items-center gap-2">
                                                    <span className="truncate font-medium text-foreground" title={item.path}>
                                                        {item.name}
                                                    </span>
                                                    <span className="shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums">
                                                        {formatSize(item.size)}
                                                    </span>
                                                </div>
                                                <p className="truncate font-mono text-[11px] text-muted-foreground" title={item.folder}>
                                                    {relFolder}
                                                </p>
                                            </div>
                                            <span
                                                className="shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums"
                                                title={formatFullDate(item.modified)}
                                            >
                                                {formatDate(item.modified)}
                                            </span>
                                            <div className="flex shrink-0 items-center gap-0.5">
                                                <Button
                                                    variant="ghost"
                                                    size="icon"
                                                    className="size-6"
                                                    onClick={() => {
                                                        closeTimeline();
                                                        revealInApp(item.path);
                                                    }}
                                                    title="Reveal in Glint"
                                                >
                                                    <FolderInput className="size-3.5" />
                                                </Button>
                                                <Button
                                                    variant="ghost"
                                                    size="icon"
                                                    className="size-6"
                                                    onClick={() => revealItemInDir(item.path)}
                                                    title="Show in File Explorer"
                                                >
                                                    <FolderOpen className="size-3.5" />
                                                </Button>
                                            </div>
                                        </div>
                                    );
                                })}
                            </div>
                        )}
                    </div>
                )}

                {!loading && !error && report && tab === "live" && (
                    <div className="space-y-3">
                        <p className="text-xs text-muted-foreground">
                            Changes Glint saw in folders open in its tabs since it started.
                        </p>
                        {filteredLive.length === 0 ? (
                            <div className="flex h-48 flex-col items-center justify-center gap-2 text-center text-sm text-muted-foreground">
                                <Activity className="size-7 stroke-[1.25] text-emerald-500" />
                                <p className="font-medium text-foreground">No changes seen in this folder yet</p>
                                <p className="max-w-sm text-xs">
                                    Changes to files in folders open in Glint show here. Make some, then click Rescan.
                                </p>
                            </div>
                        ) : (
                            <div className="divide-y rounded-lg border bg-card">
                                {filteredLive.map((ev, idx) => {
                                    const Icon = ev.kind === "created" ? FilePlus2 : ev.kind === "deleted" ? FileX2 : FileEdit;
                                    const badgeTone =
                                        ev.kind === "created"
                                            ? "bg-emerald-500/15 text-emerald-700 dark:text-emerald-400"
                                            : ev.kind === "deleted"
                                              ? "bg-rose-500/15 text-rose-600 dark:text-rose-400"
                                              : "bg-sky-500/15 text-sky-600 dark:text-sky-400";
                                    return (
                                        <div key={`${ev.path}:${ev.timestampMs}:${idx}`} className="flex items-center gap-3 px-3 py-2 text-xs">
                                            <span className={cn("inline-flex items-center gap-1 rounded px-1.5 py-0.5 font-mono text-[10px] font-semibold uppercase", badgeTone)}>
                                                <Icon className="size-3" />
                                                {ev.kind}
                                            </span>
                                            <div className="min-w-0 flex-1">
                                                <p className="truncate font-medium text-foreground" title={ev.path}>
                                                    {ev.name}
                                                </p>
                                                <p className="truncate font-mono text-[11px] text-muted-foreground" title={ev.path}>
                                                    {ev.path}
                                                </p>
                                            </div>
                                            {ev.size !== null && (
                                                <span className="shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums">
                                                    {formatSize(ev.size)}
                                                </span>
                                            )}
                                            <span className="shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums">
                                                {formatFullDate(ev.timestampMs)}
                                            </span>
                                            {ev.kind !== "deleted" && (
                                                <Button
                                                    variant="ghost"
                                                    size="icon"
                                                    className="size-6"
                                                    onClick={() => {
                                                        closeTimeline();
                                                        revealInApp(ev.path);
                                                    }}
                                                    title="Reveal in Glint"
                                                >
                                                    <FolderInput className="size-3.5" />
                                                </Button>
                                            )}
                                        </div>
                                    );
                                })}
                            </div>
                        )}
                    </div>
                )}
            </div>

            {/* Footer */}
            <div className="flex items-center justify-between border-t bg-muted/20 px-4 py-2.5 text-xs text-muted-foreground">
                <span>
                    {report
                        ? tab === "history"
                            ? `${formatCount(filteredEntries.length)} ${filteredEntries.length === 1 ? "file" : "files"}`
                            : `${formatCount(filteredLive.length)} ${filteredLive.length === 1 ? "change" : "changes"}`
                        : ""}
                </span>
                <Button variant="outline" size="sm" onClick={closeTimeline}>
                    Close
                </Button>
            </div>
        </Modal>
    );
}
