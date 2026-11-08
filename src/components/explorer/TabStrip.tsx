import { useEffect, useRef } from "react";
import { create } from "zustand";
import { Clock, Cloud, Columns2, Folder, HardDrive, Images, Monitor, Plus, Search, Trash2, X } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { activateTab, closeTab, moveTabToPane, newTab, openInNewWindow, placeTab, splitWithTab, tabName, tabState, useExplorer } from "@/lib/explorer";
import { t } from "@/lib/i18n";
import { inArchivePath, isGallery, isRecent, isRecycleBin, isRemotePath, isRoot, THIS_PC } from "@/lib/paths";
import { cn } from "@/lib/utils";

/**
 * A tab being dragged, and where it would go: its starting pane/slot, live pointer delta, and target slot
 * in a strip, or, over the files, a pane (`split`: a second one, beside the only pane).
 */
interface TabDrag {
    id: string | null;
    fromPane: number;
    fromIndex: number;
    width: number;
    dx: number;
    over: { pane: number; index: number; x: number } | null;
    zone: { pane: number; split: boolean } | null;
}
const NO_DRAG: TabDrag = { id: null, fromPane: 0, fromIndex: 0, width: 0, dx: 0, over: null, zone: null };
const useTabDrag = create<TabDrag>(() => NO_DRAG);

/**
 * Over the files, where a dragged tab would go: into the other pane (not its pane's only tab), or with
 * one pane, from its far half, into a second pane beside it.
 */
function zoneAt(x: number, y: number, fromPane: number): TabDrag["zone"] {
    const under = document.elementFromPoint(x, y);
    const pane = under?.closest<HTMLElement>("[data-pane]");
    if (!pane || under?.closest("[data-tabstrip]")) return null;
    const index = Number(pane.dataset.pane);
    const s = useExplorer.getState();
    if (s.dual) return index !== fromPane && (s.panes[fromPane]?.tabs.length ?? 0) > 1 ? { pane: index, split: false } : null;
    const box = pane.getBoundingClientRect();
    const far = document.documentElement.dir === "rtl" ? x < box.left + box.width / 2 : x > box.left + box.width / 2;
    return far ? { pane: 0, split: true } : null;
}

/** Where a tab dragged over the files would go, shown on the pane: the other pane, or a second one beside the only pane. */
export function PaneDropZone({ pane }: { pane: number }) {
    const zone = useTabDrag((s) => s.zone);
    if (!zone || zone.pane !== pane) return null;
    return (
        <div
            aria-hidden
            className={cn(
                "pointer-events-none absolute top-9 bottom-0 z-30 grid place-items-center rounded-md border-2 border-dashed border-primary/70 bg-primary/10 zone-in",
                zone.split ? "end-0 w-1/2" : "inset-x-0"
            )}
        >
            <span className="flex items-center gap-1.5 rounded-md bg-background/95 px-2.5 py-1 text-sm font-medium text-primary shadow-sm">
                <Columns2 className="size-4" />
                {t(zone.split ? "Open side by side" : "Move the tab here")}
            </span>
        </div>
    );
}

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
/** A tab dropped outside the window: its folder in a window of its own, where it was dropped (the tab closes, unless it's the last). */
function tearOffTab(id: string, at: { x: number; y: number }) {
    const path = tabState(id)?.path;
    if (!path) return;
    openInNewWindow(path, at);
    const all = useExplorer.getState().panes.reduce((n, p) => n + p.tabs.length, 0);
    if (all > 1) closeTab(id);
}

function dragTab(e: React.PointerEvent, id: string, fromPane: number, fromIndex: number) {
    if (e.button !== 0 || (e.target as Element).closest("button")) return;
    const [x0, y0] = [e.clientX, e.clientY];
    const width = (e.currentTarget as HTMLElement).getBoundingClientRect().width + 2;
    let dragging = false;
    const move = (m: PointerEvent) => {
        if (!dragging && Math.hypot(m.clientX - x0, m.clientY - y0) < 6) return;
        dragging = true;
        const over = dropAt(m.clientX, m.clientY);
        useTabDrag.setState({
            id,
            fromPane,
            fromIndex,
            width,
            dx: m.clientX - x0,
            over,
            zone: over ? null : zoneAt(m.clientX, m.clientY, fromPane),
        });
    };
    // Held, the pointer keeps reporting past the window's edge, where a tab can be dropped to have a
    // window of its own.
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
    const up = (u: PointerEvent) => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        window.removeEventListener("pointercancel", up);
        const { over, zone } = useTabDrag.getState();
        useTabDrag.setState(NO_DRAG);
        if (dragging && over) placeTab(id, over.pane, over.index);
        else if (dragging && zone && u.type === "pointerup") {
            if (zone.split) splitWithTab(id);
            else moveTabToPane(id, zone.pane);
            return;
        }
        // Dropped outside the window, or pulled well away from the strip: a window of its own.
        const outside = u.clientX < 0 || u.clientY < 0 || u.clientX > window.innerWidth || u.clientY > window.innerHeight;
        if (dragging && !over && u.type === "pointerup" && (outside || Math.abs(u.clientY - y0) > 90)) tearOffTab(id, { x: u.screenX, y: u.screenY });
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
    const { path, query, busy, spring, dropInto, dropTarget } = useExplorer(
        useShallow((s) => {
            const t = s.tabs[id];
            return {
                path: t?.path ?? THIS_PC,
                query: t?.find?.query ?? null,
                busy: !!t?.loading || !!t?.find?.running,
                // Files dragged over it: lit (and soon shown).
                spring: s.springTab === id,
                // What a drop there would go into, while it's the tab under the pointer.
                dropTarget: s.springTab === id ? s.dropTarget : null,
                // Files dropped on it go into its folder: not a search, an archive, a remote (read-only), This PC, Recent or Gallery.
                dropInto:
                    t && !t.find && t.path !== THIS_PC && !t.listing?.archive && !t.noAccess && !inArchivePath(t.path) && !isRemotePath(t.path) && !isRecent(t.path) && !isGallery(t.path)
                        ? t.path
                        : undefined,
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
            data-drop={dropInto}
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
                "group relative -mb-px flex h-8 min-w-24 max-w-52 flex-1 basis-0 cursor-default items-center gap-1.5 rounded-t-md border border-transparent ps-2.5 pe-1 text-[0.8125rem] text-muted-foreground hover:bg-accent/60 hover:text-foreground",
                !dragged && "transition-[background-color,color,transform] duration-150 ease-out",
                selected && "border-border border-b-background bg-background text-foreground hover:bg-background",
                dragged && "z-20 border-border bg-background text-foreground shadow-md",
                spring && (!selected || (dropInto !== undefined && dropTarget === dropInto)) && "bg-primary/15 text-primary ring-1 ring-inset ring-primary"
            )}
        >
            {current && <span className="absolute inset-x-2 top-0 h-0.5 rounded-full bg-primary" />}
            {/* Files held over a tab that isn't showing: it fills as it's about to open. */}
            {spring && !selected && <span aria-hidden className="spring-load absolute inset-x-1.5 bottom-0.5 h-0.5 rounded-full bg-primary" />}
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
export function TabStrip({ pane, onContextMenu, onPaneMenu }: { pane: number; onContextMenu: (e: React.MouseEvent, id: string) => void; onPaneMenu: (at: { clientX: number; clientY: number }) => void }) {
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
        <div data-tabstrip={pane} className="relative flex h-9 shrink-0 items-end border-b bg-chrome ps-1.5 pe-1">
            {over !== null && <span aria-hidden className="pointer-events-none absolute bottom-1 top-1.5 z-10 w-0.5 -translate-x-1/2 rounded-full bg-primary" style={{ left: over }} />}
            <div
                role="tablist"
                aria-label={dual ? `${pane === 0 ? "Left" : "Right"} pane tabs` : "Tabs"}
                onDoubleClick={(e) => e.target === e.currentTarget && newTab()}
                onWheel={(e) => {
                    if (e.deltaY) e.currentTarget.scrollLeft += e.deltaY;
                }}
                // Its ends fade while tabs are scrolled past them; a tab brought into view stops clear of the fade.
                className="tabs-fade flex h-full min-w-0 flex-1 scroll-px-6 items-end gap-0.5 overflow-x-auto overflow-y-hidden [scrollbar-width:none]"
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
                className="mb-1 ms-1 grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground transition-all hover:bg-accent hover:text-foreground active:scale-92"
            >
                <Plus className="size-4" />
            </button>
            {dual && (
                <button
                    type="button"
                    onClick={(e) => {
                        const below = e.currentTarget.getBoundingClientRect();
                        onPaneMenu({ clientX: below.left, clientY: below.bottom + 4 });
                    }}
                    title={t("This pane: swap, show a folder in the other pane, even out, close")}
                    aria-label={t("Pane menu")}
                    className="mb-1 grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground transition-all hover:bg-accent hover:text-foreground active:scale-92"
                >
                    <Columns2 className="size-4" />
                </button>
            )}
        </div>
    );
}
