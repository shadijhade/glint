import { useEffect, useRef } from "react";
import { create } from "zustand";
import { Folder, HardDrive, Monitor, Plus, Search, X } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { activateTab, closeTab, newTab, placeTab, tabName, useExplorer } from "@/lib/explorer";
import { isRoot, THIS_PC } from "@/lib/paths";
import { cn } from "@/lib/utils";

/** A tab being dragged, and where it would go: a pane, its place among that pane's tabs, and where to mark it (x in the strip). */
interface TabDrag {
    id: string | null;
    over: { pane: number; index: number; x: number } | null;
}
const useTabDrag = create<TabDrag>(() => ({ id: null, over: null }));

/** Where a tab dropped at (x, y) would go: the strip under it, and the gap nearest among its tabs. */
function dropAt(x: number, y: number): TabDrag["over"] {
    const strip = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-tabstrip]");
    if (!strip) return null;
    const tabs = [...strip.querySelectorAll<HTMLElement>("[data-tab]")].map((t) => t.getBoundingClientRect());
    let index = tabs.findIndex((r) => x < r.left + r.width / 2);
    if (index < 0) index = tabs.length;
    const left = strip.getBoundingClientRect().left;
    const edge = index < tabs.length ? tabs[index].left : tabs.length ? tabs[tabs.length - 1].right : left;
    return { pane: Number(strip.dataset.tabstrip), index, x: edge - left };
}

/** Pressing a tab and moving it drags it, as in a browser; just a press shows it (on mouse down). */
function dragTab(e: React.PointerEvent, id: string) {
    if (e.button !== 0 || (e.target as Element).closest("button")) return;
    const [x0, y0] = [e.clientX, e.clientY];
    let dragging = false;
    const move = (m: PointerEvent) => {
        if (!dragging && Math.hypot(m.clientX - x0, m.clientY - y0) < 6) return;
        dragging = true;
        useTabDrag.setState({ id, over: dropAt(m.clientX, m.clientY) });
    };
    const up = () => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        window.removeEventListener("pointercancel", up);
        const { over } = useTabDrag.getState();
        useTabDrag.setState({ id: null, over: null });
        if (dragging && over) placeTab(id, over.pane, over.index);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
}

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
    const dragged = useTabDrag((s) => s.id === id);
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
            data-tab={id}
            role="tab"
            aria-selected={selected}
            onPointerDown={(e) => dragTab(e, id)}
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
                selected && "border-border border-b-background bg-background text-foreground hover:bg-background",
                dragged && "opacity-50"
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
    // Where a tab dragged over this strip would go.
    const over = useTabDrag((s) => (s.over?.pane === pane ? s.over.x : null));
    return (
        <div data-tabstrip={pane} className="relative flex h-9 shrink-0 items-end border-b bg-chrome pl-1.5 pr-1">
            {over !== null && <span aria-hidden className="pointer-events-none absolute bottom-1 top-1.5 z-10 w-0.5 -translate-x-1/2 rounded-full bg-primary" style={{ left: over }} />}
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
