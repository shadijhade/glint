import { useShallow } from "zustand/react/shallow";
import { middleClickOpens, navigate, useExplorer } from "@/lib/explorer";
import { driveName } from "@/lib/fs";
import { cn } from "@/lib/utils";
import { CapacityBar, DRIVE_ICONS, freeOf, KNOWN_ICONS } from "./Sidebar";

const tile = "flex items-center gap-3 rounded-lg border bg-card p-3 text-left transition-colors hover:border-primary/40 hover:bg-accent/60 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring/50";

/** The root view: known folders and every drive with its free space. */
export function ThisPC({ onContextMenu }: { onContextMenu: (e: React.MouseEvent, path: string) => void }) {
    const { known, drives, dropTarget } = useExplorer(useShallow((s) => ({ known: s.known, drives: s.drives, dropTarget: s.dropTarget })));
    const dropping = (path: string) => path === dropTarget && "border-primary bg-primary/15";
    return (
        <div className="h-full overflow-y-auto px-6 py-5">
            <h2 className="mb-2.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">Folders</h2>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(10.5rem,1fr))] gap-2">
                {known.map((k) => {
                    const Icon = KNOWN_ICONS[k.id];
                    return (
                        <button key={k.id} data-drop={k.path} className={cn(tile, dropping(k.path))} onClick={() => navigate(k.path)} onMouseDown={middleClickOpens(k.path)} onContextMenu={(e) => onContextMenu(e, k.path)} title={k.path}>
                            <Icon className="size-5 shrink-0 text-primary" />
                            <span className="truncate text-sm">{k.name}</span>
                        </button>
                    );
                })}
            </div>

            <h2 className="mb-2.5 mt-8 text-xs font-semibold uppercase tracking-wide text-muted-foreground">Devices and drives</h2>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(16rem,1fr))] gap-3">
                {drives.map((d) => {
                    const Icon = DRIVE_ICONS[d.kind];
                    return (
                        <button key={d.path} data-drop={d.path} className={cn(tile, "items-start", dropping(d.path))} onClick={() => navigate(d.path)} onMouseDown={middleClickOpens(d.path)} onContextMenu={(e) => onContextMenu(e, d.path)}>
                            <Icon className="mt-0.5 size-7 shrink-0 text-muted-foreground" />
                            <div className="min-w-0 flex-1">
                                <div className="truncate text-sm font-medium">{driveName(d)}</div>
                                <CapacityBar drive={d} className="my-2 h-1.5" />
                                <div className="truncate text-xs text-muted-foreground">{freeOf(d) || (d.fs ? d.fs : "Not ready")}</div>
                            </div>
                        </button>
                    );
                })}
            </div>
        </div>
    );
}
