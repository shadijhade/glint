import { useEffect, useRef } from "react";
import { create } from "zustand";
import { Clock, Cloud, Folder, HardDrive, Images, Monitor, Plus, Search, Trash2, X } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { activateTab, closeTab, newTab, placeTab, tabName, useExplorer } from "@/lib/explorer";
import { isGallery, isRecent, isRecycleBin, isRemotePath, isRoot, THIS_PC } from "@/lib/paths";
import { cn } from "@/lib/utils";

/** A tab being dragged, and where it would go: its starting pane/slot, live pointer delta, and target slot. */
interface TabDrag {
    id: string | null;
    fromPane: number;
    fromIndex: number;
    width: number;
    dx: number;
    over: { pane: number; index: number; x: number } | null;
}
const useTabDrag = create<TabDrag>(() => ({ id: null, fromPane: 0, fromIndex: 0, width: 0, dx: 0, over: null }));

/** Where a tab dropped at (x, y) would go: the strip under it, and the gap nearest among its tabs. */
function dropAt(x: number, y: number): TabDrag["over"] {
    const strip = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-tabstrip]");
    if (!strip) return null;
    const tabEls = [...strip.querySelectorAll<HTMLElement>("[data-tab]")];
    // Measure natural (un-translated) layout positions using offsetLeft inside the tablist scroll container.
    const listLeft = strip.getBoundingClientRect().left;
    const tabs = tabEls.map((el) => {
        const rect = el.getBoundingClientRect();
        // Subtract any CSS transform applied during live reflow so hit-testing never oscillates.
        const parentRect = el.parentElement?.getBoundingClientRect();
        const scrollLeft = el.parentElement?.scrollLeft ?? 0;
        const naturalLeft = parentRect ? parentRect.left + el.offsetLeft - scrollLeft : rect.left;
        return { id: el.dataset.tab, left: naturalLeft, right: naturalLeft + rect.width, width: rect.width };
    });
    let index = tabs.findIndex((r) => x < r.left + r.width / 2);
    if (index < 0) index = tabs.length;
    const edge = index < tabs.length ? tabs[index].left : tabs.length ? tabs[tabs.length - 1].right : listLeft;
    return { pane: Number(strip.dataset.tabstrip), index, x: edge - listLeft };
}

/** Pressing a tab and moving it drags it with live neighbor reflow, as in a browser; just a press shows it. */
function dragTab(e: React.PointerEvent, id: string, fromPane: number, fromIndex: number) {
    if (e.button !== 0 || (e.target as Element).closest("button")) return;
    const [x0, y0] = [e.clientX, e.clientY];
    const width = (e.currentTarget as HTMLElement).getBoundingClientRect().width + 2;
    let dragging = false;
    const move = (m: PointerEvent) => {
        if (!dragging && Math.hypot(m.clientX - x0, m.clientY - y0) < 6) return;
        dragging = true;
        useTabDrag.setState({
            id,
            fromPane,
            fromIndex,
            width,
            dx: m.clientX - x0,
            over: dropAt(m.clientX, m.clientY),
        });
    };
    const up = () => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        window.removeEventListener("pointercancel", up);
        const { over } = useTabDrag.getState();
        useTabDrag.setState({ id: null, fromPane: 0, fromIndex: 0, width: 0, dx: 0, over: null });
        if (dragging && over) placeTab(id, over.pane, over.index);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
}

interface TabProps {
    id: string;
    pane: number;
    index: number;
    selected: boolean;
    /** The tab on screen in the pane the keyboard acts on. */
    current: boolean;
    canClose: boolean;
    onContextMenu: (e: React.MouseEvent, id: string) => void;
}

function Tab({ id, pane, index, selected, current, canClose, onContextMenu }: TabProps) {
    const { path, query, busy, spring } = useExplorer(
        useShallow((s) => {
            const t = s.tabs[id];
            return {
                path: t?.path ?? THIS_PC,
                query: t?.find?.query ?? null,
                busy: !!t?.loading || !!t?.find?.running,
                spring: s.springTab === id,
            };
        })
    );
    const drives = useExplorer((s) => s.drives);
    const { dragged, shiftX } = useTabDrag(
        useShallow((s) => {
            if (!s.id) return { dragged: false, shiftX: 0 };
            if (s.id === id) return { dragged: true, shiftX: s.dx };
            if (s.fromPane !== pane || s.over?.pane !== pane) return { dragged: false, shiftX: 0 };
            const targetSlot = s.over.index > s.fromIndex ? s.over.index - 1 : s.over.index;
            if (index > s.fromIndex && index <= targetSlot) return { dragged: false, shiftX: -s.width };
            if (index < s.fromIndex && index >= targetSlot) return { dragged: false, shiftX: s.width };
            return { dragged: false, shiftX: 0 };
        })
    );
    const ref = useRef<HTMLDivElement>(null);
    useEffect(() => {
        if (selected) ref.current?.scrollIntoView({ block: "nearest", inline: "nearest" });
    }, [selected]);

    const name = query ?? tabName(path, drives);
    const where = path === THIS_PC ? "This PC" : isRecycleBin(path) ? "Recycle Bin" : isRecent(path) ? "Recent: the files opened lately" : isGallery(path) ? "Gallery: your photos and videos" : path;
    const folder = query === null && path !== THIS_PC && !isRoot(path);
    const Icon = query !== null ? Search : path === THIS_PC ? Monitor : isRecycleBin(path) ? Trash2 : isRecent(path) ? Clock : isGallery(path) ? Images : isRemotePath(path) && isRoot(path) ? Cloud : folder ? Folder : HardDrive;
    return (
        <div
            ref={ref}
            data-tab={id}
            role="tab"
            aria-selected={selected}
            onPointerDown={(e) => dragTab(e, id, pane, index)}
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
            style={shiftX ? { transform: `translate3d(${shiftX}px, 0, 0)` } : undefined}
            className={cn(
                "group relative -mb-px flex h-8 min-w-24 max-w-52 flex-1 basis-0 cursor-default items-center gap-1.5 rounded-t-md border border-transparent pl-2.5 pr-1 text-[0.8125rem] text-muted-foreground hover:bg-accent/60 hover:text-foreground",
                !dragged && "transition-[background-color,color,transform] duration-150 ease-out",
                selected && "border-border border-b-background bg-background text-foreground hover:bg-background",
                dragged && "z-20 border-border bg-background text-foreground shadow-md",
                spring && "bg-primary/15 text-primary ring-1 ring-inset ring-primary"
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
    // Where a tab dragged from the OTHER pane over this strip would go.
    const over = useTabDrag((s) => (s.over?.pane === pane && s.fromPane !== pane ? s.over.x : null));
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
                {tabs.map((id, index) => (
                    <Tab key={id} id={id} pane={pane} index={index} selected={id === active} current={id === active && paneActive} canClose={canClose} onContextMenu={onContextMenu} />
                ))}
            </div>
            <button
                type="button"
                onClick={() => newTab()}
                title="New tab (Ctrl+T)"
                aria-label="New tab"
                className="mb-1 ml-1 grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground transition-all hover:bg-accent hover:text-foreground active:scale-92"
            >
                <Plus className="size-4" />
            </button>
        </div>
    );
}
