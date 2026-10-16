import { useEffect, useRef } from "react";
import { Folder, HardDrive, Monitor, Plus, Search, X } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { activateTab, closeTab, newTab, tabName, useExplorer } from "@/lib/explorer";
import { isRoot, THIS_PC } from "@/lib/paths";
import { cn } from "@/lib/utils";

interface TabProps {
    id: string;
    selected: boolean;
    /** The tab on screen in the pane the keyboard acts on. */
    current: boolean;
    canClose: boolean;
    onContextMenu: (e: React.MouseEvent, id: string) => void;
}

function Tab({ id, selected, current, canClose, onContextMenu }: TabProps) {
    const { path, query, busy } = useExplorer(
        useShallow((s) => {
            const t = s.tabs[id];
            return { path: t?.path ?? THIS_PC, query: t?.find?.query ?? null, busy: !!t?.loading || !!t?.find?.running };
        })
    );
    const drives = useExplorer((s) => s.drives);
    const ref = useRef<HTMLDivElement>(null);
    useEffect(() => {
        if (selected) ref.current?.scrollIntoView({ block: "nearest", inline: "nearest" });
    }, [selected]);

    const name = query ?? tabName(path, drives);
    const where = path === THIS_PC ? "This PC" : path;
    const folder = query === null && path !== THIS_PC && !isRoot(path);
    const Icon = query !== null ? Search : path === THIS_PC ? Monitor : folder ? Folder : HardDrive;
    return (
        <div
            ref={ref}
            role="tab"
            aria-selected={selected}
            title={query !== null ? `Search results for “${query}” in ${where}` : where}
            onMouseDown={(e) => {
                if (e.button === 0) activateTab(id);
                // A middle-click closes the tab when released; this keeps autoscroll from starting.
                else if (e.button === 1) e.preventDefault();
            }}
            onAuxClick={(e) => e.button === 1 && canClose && closeTab(id)}
            onContextMenu={(e) => {
                e.preventDefault();
                onContextMenu(e, id);
            }}
            className={cn(
                "group relative -mb-px flex h-8 min-w-24 max-w-52 flex-1 basis-0 cursor-default items-center gap-1.5 rounded-t-md border border-transparent pl-2.5 pr-1 text-[0.8125rem] text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground",
                selected && "border-border border-b-background bg-background text-foreground hover:bg-background"
            )}
        >
            {current && <span className="absolute inset-x-2 top-0 h-0.5 rounded-full bg-primary" />}
            <Icon className={cn("size-3.5 shrink-0", folder ? "fill-amber-400/80 text-amber-500" : "text-muted-foreground", busy && "animate-pulse")} />
            <span className="min-w-0 flex-1 truncate">{name}</span>
            {canClose && (
                <button
                    type="button"
                    onMouseDown={(e) => e.stopPropagation()}
                    onClick={() => closeTab(id)}
                    title="Close tab (Ctrl+W)"
                    aria-label={`Close ${name}`}
                    className={cn(
                        "grid size-5 shrink-0 place-items-center rounded-sm opacity-0 transition-opacity hover:bg-accent hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100",
                        selected && "opacity-100"
                    )}
                >
                    <X className="size-3.5" />
                </button>
            )}
        </div>
    );
}

/** A pane's tabs, browser-style: click to show, middle-click or × to close, double-click the empty strip for a new one. */
export function TabStrip({ pane, onContextMenu }: { pane: number; onContextMenu: (e: React.MouseEvent, id: string) => void }) {
    const { tabs, active, paneActive, dual } = useExplorer(
        useShallow((s) => {
            // Selectors also run for a pane that's about to go (its pane already closed).
            const p = s.panes[pane] ?? s.panes[0];
            return { tabs: p.tabs, active: p.active, paneActive: s.activePane === pane, dual: s.dual };
        })
    );
    const canClose = tabs.length > 1 || dual;
    return (
        <div className="flex h-9 shrink-0 items-end border-b bg-card/40 pl-1.5 pr-1">
            <div
                role="tablist"
                aria-label={dual ? `${pane === 0 ? "Left" : "Right"} pane tabs` : "Tabs"}
                onDoubleClick={(e) => e.target === e.currentTarget && newTab()}
                onWheel={(e) => {
                    if (e.deltaY) e.currentTarget.scrollLeft += e.deltaY;
                }}
                className="flex h-full min-w-0 flex-1 items-end gap-0.5 overflow-x-auto overflow-y-hidden [scrollbar-width:none]"
            >
                {tabs.map((id) => (
                    <Tab key={id} id={id} selected={id === active} current={id === active && paneActive} canClose={canClose} onContextMenu={onContextMenu} />
                ))}
            </div>
            <button
                type="button"
                onClick={() => newTab()}
                title="New tab (Ctrl+T)"
                aria-label="New tab"
                className="mb-1 ml-1 grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            >
                <Plus className="size-4" />
            </button>
        </div>
    );
}
