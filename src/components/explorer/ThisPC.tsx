import { Files, History, LayoutDashboard } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { toggleUsage, useUsage } from "@/lib/diskUsage";
import { middleClickOpens, navigate, useExplorer, useTab } from "@/lib/explorer";
import { formatSize } from "@/lib/format";
import { driveName } from "@/lib/fs";
import { cn } from "@/lib/utils";
import { inspectDuplicates } from "./DuplicatesDialog";
import { CapacityBar, DRIVE_ICONS, freeOf, KNOWN_ICONS } from "./Sidebar";
import { inspectTimeline } from "./TimelineDialog";

const tile =
    "group relative flex items-center gap-3 rounded-lg border bg-card p-3 text-left transition-all duration-150 hover:border-primary/40 hover:bg-accent/60 hover:shadow-xs focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring/50";

/** The root view: known folders, and every drive with its free space and quick actions. */
export function ThisPC({ onContextMenu }: { onContextMenu: (e: React.MouseEvent, path: string) => void }) {
    const { known, drives, dropTarget } = useExplorer(useShallow((s) => ({ known: s.known, drives: s.drives, dropTarget: s.dropTarget })));
    const tabId = useTab((t) => t.id);
    const dropping = (path: string) => path === dropTarget && "border-primary bg-primary/15";

    const readyDrives = drives.filter((d) => d.total > 0);
    const totalCapacity = readyDrives.reduce((sum, d) => sum + d.total, 0);
    const totalFree = readyDrives.reduce((sum, d) => sum + d.free, 0);
    const totalUsed = Math.max(0, totalCapacity - totalFree);
    const totalPct = totalCapacity > 0 ? Math.round((totalUsed / totalCapacity) * 100) : 0;

    const openDriveTreemap = (e: React.MouseEvent, drivePath: string) => {
        e.stopPropagation();
        navigate(drivePath);
        if (!useUsage.getState()[tabId]) {
            setTimeout(() => toggleUsage(tabId), 0);
        }
    };

    return (
        <div className="h-full overflow-y-auto px-6 py-5">
            <h2 className="mb-2.5 text-sm font-semibold">Folders</h2>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(10.5rem,1fr))] gap-2">
                {known.map((k) => {
                    const Icon = KNOWN_ICONS[k.id];
                    return (
                        <button
                            key={k.id}
                            data-drop={k.path}
                            className={cn(tile, dropping(k.path))}
                            onClick={() => navigate(k.path)}
                            onMouseDown={middleClickOpens(k.path)}
                            onContextMenu={(e) => onContextMenu(e, k.path)}
                            title={k.path}
                        >
                            <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary transition-colors group-hover:bg-primary/15">
                                <Icon className="size-4" />
                            </div>
                            <span className="truncate text-sm font-medium">{k.name}</span>
                        </button>
                    );
                })}
            </div>

            <div className="mb-3 mt-8 flex flex-wrap items-center justify-between gap-2">
                <h2 className="text-sm font-semibold">Devices and drives</h2>
                {totalCapacity > 0 && (
                    <div className="flex items-center gap-2.5 rounded-full border bg-muted/30 px-3 py-1 text-xs text-muted-foreground">
                        <span>
                            Total: <b className="font-semibold text-foreground">{formatSize(totalUsed)}</b> used of{" "}
                            <b className="font-semibold text-foreground">{formatSize(totalCapacity)}</b>
                        </span>
                        <span className="text-border">·</span>
                        <span className="font-medium text-emerald-600 dark:text-emerald-400">{formatSize(totalFree)} free</span>
                        <span className="rounded bg-primary/15 px-1.5 py-0.2 font-mono text-[11px] font-semibold text-primary tabular-nums">
                            {totalPct}%
                        </span>
                    </div>
                )}
            </div>

            <div className="grid grid-cols-[repeat(auto-fill,minmax(17.5rem,1fr))] gap-3">
                {drives.map((d) => {
                    const Icon = DRIVE_ICONS[d.kind];
                    const usedPct = d.total > 0 ? Math.round(((d.total - d.free) / d.total) * 100) : 0;
                    const nearlyFull = usedPct >= 90;
                    return (
                        <div
                            key={d.path}
                            role="button"
                            tabIndex={0}
                            data-drop={d.path}
                            className={cn(tile, "items-start cursor-pointer", dropping(d.path))}
                            onClick={() => navigate(d.path)}
                            onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && navigate(d.path)}
                            onMouseDown={middleClickOpens(d.path)}
                            onContextMenu={(e) => onContextMenu(e, d.path)}
                        >
                            <div
                                className={cn(
                                    "mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-lg border bg-muted/40 text-muted-foreground transition-colors group-hover:text-foreground",
                                    nearlyFull && "border-destructive/40 bg-destructive/10 text-destructive"
                                )}
                            >
                                <Icon className="size-5" />
                            </div>
                            <div className="min-w-0 flex-1">
                                <div className="flex items-center gap-1.5">
                                    <span className="truncate text-sm font-medium">{driveName(d)}</span>
                                    {d.fs && (
                                        <span className="shrink-0 rounded bg-muted px-1.5 py-0.2 font-mono text-[10px] font-medium text-muted-foreground">
                                            {d.fs}
                                        </span>
                                    )}
                                    {d.total > 0 && (
                                        <span
                                            className={cn(
                                                "ml-auto shrink-0 font-mono text-[11px] font-semibold tabular-nums",
                                                nearlyFull ? "text-destructive" : "text-muted-foreground"
                                            )}
                                        >
                                            {usedPct}%
                                        </span>
                                    )}
                                </div>
                                <CapacityBar drive={d} className="my-2 h-1.5" />
                                <div className="flex items-center justify-between gap-2">
                                    <span className="truncate text-xs text-muted-foreground">
                                        {freeOf(d) || (d.fs ? d.fs : "Not ready")}
                                    </span>
                                    {d.total > 0 && (
                                        <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
                                            <button
                                                type="button"
                                                title="Map disk usage"
                                                onClick={(e) => openDriveTreemap(e, d.path)}
                                                className="rounded p-1 text-muted-foreground hover:bg-background hover:text-primary"
                                            >
                                                <LayoutDashboard className="size-3.5" />
                                            </button>
                                            <button
                                                type="button"
                                                title="Recent changes"
                                                onClick={(e) => {
                                                    e.stopPropagation();
                                                    inspectTimeline(d.path);
                                                }}
                                                className="rounded p-1 text-muted-foreground hover:bg-background hover:text-primary"
                                            >
                                                <History className="size-3.5" />
                                            </button>
                                            <button
                                                type="button"
                                                title="Find duplicates"
                                                onClick={(e) => {
                                                    e.stopPropagation();
                                                    inspectDuplicates(d.path);
                                                }}
                                                className="rounded p-1 text-muted-foreground hover:bg-background hover:text-primary"
                                            >
                                                <Files className="size-3.5" />
                                            </button>
                                        </div>
                                    )}
                                </div>
                            </div>
                        </div>
                    );
                })}
            </div>
        </div>
    );
}
