import { useDeferredValue, useEffect, useLayoutEffect, useMemo, useRef } from "react";
import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirm } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import {
    AlertTriangle,
    AppWindow,
    ArrowLeft,
    ArrowRight,
    ArrowRightLeft,
    ArrowUp,
    ChevronLeft,
    ChevronRight,
    ClipboardCopy,
    ClipboardPaste,
    Code,
    Columns2,
    Copy,
    CopyPlus,
    ExternalLink,
    Eye,
    EyeOff,
    FilePlus,
    Folder,
    FolderInput,
    FolderOpen,
    FolderOutput,
    FolderPlus,
    FolderSearch,
    HardDrive,
    Info,
    LayoutDashboard,
    LayoutGrid,
    List,
    ListChecks,
    Loader2,
    Monitor,
    Moon,
    PanelRight,
    Pin,
    PenLine,
    PinOff,
    Plus,
    RotateCcw,
    RotateCw,
    Scissors,
    Search,
    SearchX,
    Settings as SettingsIcon,
    Square,
    SquarePlus,
    SquareTerminal,
    Sun,
    TextSearch,
    Trash2,
    X,
    ZoomIn,
    ZoomOut,
} from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { chooseView, showsUsage, toggleUsage, useMapRows } from "@/lib/diskUsage";
import { type FileEntry, isHidden, matcher, sortEntries, type SortKey } from "@/lib/entries";
import {
    calcSizes,
    canReopenTab,
    clearFind,
    closeOtherTabs,
    closeTab,
    copyToClipboard,
    copyToOtherPane,
    currentPath,
    cycleTab,
    deletePaths,
    dropFiles,
    duplicateTab,
    focusPane,
    goBack,
    goForward,
    goTo,
    goUp,
    initExplorer,
    markShown,
    moveTabToOtherPane,
    navigate,
    newItem,
    newTab,
    openEntry,
    openInNewTab,
    otherPaneFolder,
    PaneProvider,
    paste,
    refresh,
    refreshDrives,
    reload,
    reloadFolder,
    renameEntry,
    reopenClosedTab,
    revealInApp,
    runFind,
    savedScroll,
    saveScroll,
    selectAll,
    selectOnly,
    selectRange,
    selectTab,
    stopFind,
    switchPane,
    tabState,
    toggleDual,
    toggleSelected,
    updateTab,
    useExplorer,
    useTab,
    viewKey,
    zoomGrid,
} from "@/lib/explorer";
import { formatCount } from "@/lib/format";
import { driveName, openTerminal, openWith, showProperties, startDrag } from "@/lib/fs";
import { gridMove, TILE_SIZES, zoomTiles } from "@/lib/grid";
import { baseName, THIS_PC } from "@/lib/paths";
import { frecency } from "@/lib/rank";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { gridTile, remPx, rowHeight } from "@/hooks/useTheme";
import { type MenuItem, useMenu } from "../Menu";
import { Button } from "../ui/button";
import { FIND_BOX_ID } from "./AddressBar";
import { CommandBar } from "./CommandBar";
import { type Command, CommandPalette } from "./CommandPalette";
import { DiskUsage } from "./DiskUsage";
import { FileGrid } from "./FileGrid";
import { FileList } from "./FileList";
import { FilePreview } from "./FilePreview";
import { QuickLook } from "./QuickLook";
import { DRIVE_ICONS, KNOWN_ICONS, Sidebar } from "./Sidebar";
import { StatusBar } from "./StatusBar";
import { TabStrip } from "./TabStrip";
import { ThisPC } from "./ThisPC";

const NO_ENTRIES: FileEntry[] = [];

const copyText = (text: string, what: string) =>
    navigator.clipboard.writeText(text).then(
        () => toast.success(`Copied ${what}`, { duration: 1500 }),
        () => toast.error(`Couldn't copy ${what}`)
    );

const report = (what: string) => (e: unknown) => toast.error(what, { description: String(e) });
const properties = (path: string) => showProperties(path).catch(report("Couldn't show the properties"));
const terminal = (path: string) => openTerminal(path).catch(report("Couldn't open a terminal"));
const focusFind = () => {
    const box = document.getElementById(FIND_BOX_ID) as HTMLInputElement | null;
    box?.focus();
    box?.select();
};

function Centered({ children }: { children: React.ReactNode }) {
    return (
        <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-sm text-muted-foreground [&_button]:pointer-events-auto">
            {children}
        </div>
    );
}

/** What the active pane shows, for the toolbar and the preview pane, which sit outside the panes. */
interface ActiveView {
    selected: FileEntry[];
    items: number;
    folder: string;
    total?: number;
}
const useActiveView = create<ActiveView>(() => ({ selected: NO_ENTRIES, items: 0, folder: THIS_PC }));

/** What only a pane can do (it needs the pane's sorted, filtered rows), for the toolbar. */
interface PaneActions {
    startRename: () => void;
    deleteSelected: (permanent: boolean) => void;
}
const paneActions: (PaneActions | undefined)[] = [];
const activePaneActions = () => paneActions[useExplorer.getState().activePane];

interface MenuContext {
    pinned: string[];
    togglePin: (path: string) => void;
    onSearchText: (folder: string) => void;
}

/** For a folder: the one open in a pane (right-click on empty space, `current`), or a place in the sidebar. */
function folderMenu(folder: string, current: boolean, ctx: MenuContext, view?: { find: boolean; selectAll: () => void }): MenuItem[] {
    if (folder === THIS_PC) return [{ label: "Refresh", icon: RotateCw, shortcut: "F5", onSelect: refreshDrives }];
    const isPinned = ctx.pinned.includes(folder);
    const find = !!view?.find;
    return [
        !current && { label: "Open", icon: FolderOpen, onSelect: () => navigate(folder) },
        !current && { label: "Open in new tab", icon: SquarePlus, onSelect: () => openInNewTab(folder) },
        current && { label: "New folder", icon: FolderPlus, shortcut: "Ctrl+Shift+N", disabled: find, onSelect: () => newItem(true) },
        current && { label: "New text document", icon: FilePlus, disabled: find, onSelect: () => newItem(false) },
        "separator",
        { label: current ? "Paste" : "Paste into folder", icon: ClipboardPaste, shortcut: current ? "Ctrl+V" : undefined, disabled: current && find, onSelect: () => paste(folder) },
        current && !!view && { label: "Select all", icon: ListChecks, shortcut: "Ctrl+A", onSelect: view.selectAll },
        current && { label: "Refresh", icon: RotateCw, shortcut: "F5", onSelect: refresh },
        "separator",
        current && { label: "Folder sizes", icon: HardDrive, disabled: find, onSelect: calcSizes },
        { label: "Search text here", icon: TextSearch, onSelect: () => ctx.onSearchText(folder) },
        { label: "Copy path", icon: ClipboardCopy, onSelect: () => copyText(folder, "path") },
        "separator",
        { label: "Open in File Explorer", icon: FolderSearch, onSelect: () => openPath(folder).catch(report("Couldn't open File Explorer")) },
        { label: "Open in Terminal", icon: SquareTerminal, onSelect: () => terminal(folder) },
        { label: isPinned ? "Unpin from sidebar" : "Pin to sidebar", icon: isPinned ? PinOff : Pin, onSelect: () => ctx.togglePin(folder) },
        { label: "Properties", icon: Info, shortcut: current ? "Alt+Enter" : undefined, onSelect: () => properties(folder) },
    ];
}

function tabMenu(id: string): MenuItem[] {
    const s = useExplorer.getState();
    const tabs = s.panes.find((p) => p.tabs.includes(id))?.tabs.length ?? 0;
    return [
        { label: "Duplicate tab", icon: Copy, onSelect: () => duplicateTab(id) },
        s.dual && { label: "Move to other pane", icon: ArrowRightLeft, disabled: tabs < 2, onSelect: () => moveTabToOtherPane(id) },
        "separator",
        { label: "Close tab", icon: X, shortcut: "Ctrl+W", disabled: tabs < 2 && !s.dual, onSelect: () => closeTab(id) },
        { label: "Close other tabs", icon: X, disabled: tabs < 2, onSelect: () => closeOtherTabs(id) },
        { label: "Reopen closed tab", icon: RotateCcw, shortcut: "Ctrl+Shift+T", disabled: !canReopenTab(), onSelect: reopenClosedTab },
    ];
}

interface ExplorerProps {
    active: boolean;
    /** Switch to text search (ripgrep) in `folder`. */
    onSearchText: (folder: string) => void;
    onOpenSettings: () => void;
}

export function Explorer({ active, onSearchText, onOpenSettings }: ExplorerProps) {
    const { path, find } = useTab(useShallow((t) => ({ path: t.path, find: !!t.find })));
    const { dual, palette } = useExplorer(useShallow((s) => ({ dual: s.dual, palette: s.palette })));
    const showPreviewPane = useStore((s) => s.settings.showPreviewPane);
    const selected = useActiveView((v) => v.selected);
    const pinned = useStore((s) => s.pinned);
    const { setSettings, togglePin } = useStore.getState();
    const menu = useMenu();
    const menuContext: MenuContext = { pinned, togglePin, onSearchText };

    useEffect(() => {
        initExplorer();
        // Refresh the folders on screen when they change on disk; drives may come and go while we're away.
        const unlisten = listen<string>("dir-changed", (event) => reloadFolder(event.payload));
        const onFocus = () => refreshDrives();
        window.addEventListener("focus", onFocus);
        return () => {
            unlisten.then((stop) => stop());
            window.removeEventListener("focus", onFocus);
        };
    }, []);

    const activeRef = useRef(active);
    activeRef.current = active;

    // Files dragged in from File Explorer, the desktop or the other pane land on the folder row or
    // sidebar place under the pointer, or else in the folder open in the pane under it.
    useEffect(() => {
        const targetAt = ({ x, y }: { x: number; y: number }) => {
            const under = document.elementFromPoint(x / devicePixelRatio, y / devicePixelRatio);
            const place = under?.closest<HTMLElement>("[data-drop]")?.dataset.drop;
            if (place) return place;
            const pane = under?.closest<HTMLElement>("[data-pane]")?.dataset.pane;
            if (pane === undefined || !under?.closest(".files")) return null;
            const s = useExplorer.getState();
            const tab = s.panes[Number(pane)] && s.tabs[s.panes[Number(pane)].active];
            return tab && tab.path !== THIS_PC && !tab.find ? tab.path : null;
        };
        const unlisten = getCurrentWebview().onDragDropEvent(({ payload }) => {
            const target = activeRef.current && payload.type !== "leave" ? targetAt(payload.position) : null;
            if (payload.type === "drop" && target) dropFiles(payload.paths, target);
            const dropTarget = payload.type === "drop" ? null : target;
            if (useExplorer.getState().dropTarget !== dropTarget) useExplorer.setState({ dropTarget });
        });
        return () => {
            unlisten.then((stop) => stop());
        };
    }, []);

    const searchTextHere = () => {
        const folder = currentPath();
        if (folder !== THIS_PC) onSearchText(folder);
    };

    const onPlaceContextMenu = (e: React.MouseEvent, place: string) => {
        e.preventDefault();
        menu.open(e, folderMenu(place, false, menuContext));
    };

    /** Everything the command palette offers: folders you use (most-used first) and every action. */
    const paletteCommands = (): Command[] => {
        const s = useExplorer.getState();
        const t = tabState()!;
        const { visits, settings: now } = useStore.getState();
        const inFolder = t.path !== THIS_PC;
        const seen = new Set<string>();
        const folders: Command[] = [];
        const addFolder = (folder: string, label: string, icon: Command["icon"], weight: number, iconClass?: string) => {
            const key = folder.toLowerCase();
            if (seen.has(key)) return;
            seen.add(key);
            const visit = visits[folder];
            folders.push({ id: `folder:${key}`, label, detail: folder, icon, iconClass, kind: "folder", weight: weight + (visit ? Math.log2(1 + frecency(visit)) : 0), run: () => navigate(folder) });
        };
        for (const p of pinned) addFolder(p, baseName(p), Pin, 1.5);
        for (const k of s.known) addFolder(k.path, k.name, KNOWN_ICONS[k.id], 0.5);
        for (const [folder] of Object.entries(visits).sort((a, b) => frecency(b[1]) - frecency(a[1])).slice(0, 60)) {
            addFolder(folder, baseName(folder), Folder, 0, "fill-amber-400/80 text-amber-500");
        }
        for (const d of s.drives) addFolder(d.path, driveName(d), DRIVE_ICONS[d.kind], 0);

        const action = (id: string, label: string, icon: Command["icon"], run: () => void, shortcut?: string): Command => ({ id, label, icon, run, shortcut, kind: "action" });
        const isPinned = pinned.includes(t.path);
        const tabs = s.panes[s.activePane].tabs.length;
        const other = otherPaneFolder();
        const chosen = t.selection.size > 0;
        const canGrid = inFolder || !!t.find;
        const grid = canGrid && t.view === "grid" && !showsUsage(t.id);
        // Each size, once something's typed ("large", "tiles"): the grid at that size.
        const tileSizes = TILE_SIZES.map(
            ({ size, label }) =>
                canGrid &&
                !(grid && size === now.tileSize) && {
                    ...action(
                        `tiles-${size}`,
                        `${label} tiles`,
                        LayoutGrid,
                        () => {
                            setSettings({ tileSize: size });
                            chooseView("grid", t.id);
                        },
                        grid && size === "medium" ? "Ctrl+0" : undefined
                    ),
                    searchOnly: true,
                }
        );
        const actions: (Command | false)[] = [
            inFolder && !t.find && action("new-folder", "New folder", FolderPlus, () => newItem(true), "Ctrl+Shift+N"),
            inFolder && !t.find && action("new-file", "New text document", FilePlus, () => newItem(false)),
            action("find", "Find files in this folder", Search, focusFind, "Ctrl+F"),
            inFolder && action("search-text", "Search text in this folder", TextSearch, () => onSearchText(t.path), "Ctrl+Shift+F"),
            inFolder && !t.find && action("sizes", "Calculate folder sizes", HardDrive, calcSizes),
            inFolder && !t.find && action("disk-usage", showsUsage(t.id) ? "Hide disk usage" : "Show disk usage", showsUsage(t.id) ? List : LayoutDashboard, () => toggleUsage(t.id)),
            canGrid &&
                (grid
                    ? action("view-list", "Show as a list", List, () => chooseView("list", t.id), "Ctrl+Shift+6")
                    : action("view-grid", "Show as a grid, with thumbnails", LayoutGrid, () => chooseView("grid", t.id), "Ctrl+Shift+2")),
            grid && zoomTiles(now.tileSize, 1) !== now.tileSize && action("zoom-in", "Zoom in: larger tiles", ZoomIn, () => zoomGrid(1), "Ctrl+="),
            grid && zoomTiles(now.tileSize, -1) !== now.tileSize && action("zoom-out", "Zoom out: smaller tiles", ZoomOut, () => zoomGrid(-1), "Ctrl+-"),
            ...tileSizes,
            !!t.focus && action("quick-look", "Quick Look", Eye, () => useExplorer.setState({ quickLook: true }), "Space"),
            action("hidden", now.showHidden ? "Hide hidden items" : "Show hidden items", now.showHidden ? EyeOff : Eye, () => setSettings({ showHidden: !now.showHidden }), "Ctrl+H"),
            action("preview", now.showPreviewPane ? "Hide preview pane" : "Show preview pane", PanelRight, () => setSettings({ showPreviewPane: !now.showPreviewPane }), "Alt+P"),
            action("refresh", "Refresh", RotateCw, refresh, "F5"),
            t.back.length > 0 && action("back", "Back", ArrowLeft, goBack, "Alt+Left"),
            t.forward.length > 0 && action("forward", "Forward", ArrowRight, goForward, "Alt+Right"),
            inFolder && action("up", "Up one folder", ArrowUp, goUp, "Alt+Up"),
            action("this-pc", "This PC", Monitor, () => navigate(THIS_PC)),
            action("new-tab", "New tab", Plus, () => newTab(), "Ctrl+T"),
            (tabs > 1 || s.dual) && action("close-tab", "Close tab", X, () => closeTab(), "Ctrl+W"),
            canReopenTab() && action("reopen-tab", "Reopen closed tab", RotateCcw, reopenClosedTab, "Ctrl+Shift+T"),
            tabs > 1 && action("next-tab", "Next tab", ChevronRight, () => cycleTab(1), "Ctrl+Tab"),
            tabs > 1 && action("previous-tab", "Previous tab", ChevronLeft, () => cycleTab(-1), "Ctrl+Shift+Tab"),
            action("dual", s.dual ? "Close the second pane" : "Dual pane: two folders side by side", Columns2, toggleDual, "Ctrl+Shift+D"),
            s.dual && action("switch-pane", "Switch to the other pane", ArrowRightLeft, switchPane, "F6"),
            !!other && chosen && action("copy-other", "Copy the selection to the other pane", CopyPlus, () => copyToOtherPane(false)),
            !!other && chosen && action("move-other", "Move the selection to the other pane", FolderOutput, () => copyToOtherPane(true)),
            inFolder && action("copy-path", "Copy this folder's path", ClipboardCopy, () => copyText(t.path, "path")),
            inFolder && action("terminal", "Open in Terminal", SquareTerminal, () => terminal(t.path)),
            inFolder && action("explorer", "Open in File Explorer", FolderSearch, () => openPath(t.path).catch(report("Couldn't open File Explorer"))),
            inFolder && action("pin", isPinned ? "Unpin this folder" : "Pin this folder to the sidebar", isPinned ? PinOff : Pin, () => togglePin(t.path)),
            inFolder && action("properties", "Properties", Info, () => properties(t.path), "Alt+Enter"),
            action("theme", now.theme === "light" ? "Switch to dark theme" : "Switch to light theme", now.theme === "light" ? Moon : Sun, () => setSettings({ theme: now.theme === "light" ? "dark" : "light" })),
            action("settings", "Settings", SettingsIcon, onOpenSettings, "Ctrl+,"),
        ];
        return [...folders, ...actions.filter((a): a is Command => !!a)];
    };

    return (
        <div className={active ? "flex min-h-0 flex-1" : "hidden"}>
            <Sidebar onContextMenu={onPlaceContextMenu} />
            <div className="flex min-w-0 flex-1 flex-col">
                <CommandBar
                    selected={selected}
                    onRename={() => activePaneActions()?.startRename()}
                    onDelete={() => activePaneActions()?.deleteSelected(false)}
                    onSearchText={searchTextHere}
                />
                <div className="flex min-h-0 flex-1">
                    <Pane index={0} active={active} menuContext={menuContext} />
                    {dual && <Pane index={1} active={active} menuContext={menuContext} />}
                </div>
            </div>
            {showPreviewPane && (path !== THIS_PC || find) && <PreviewPane />}
            {menu.element}
            {palette && <CommandPalette commands={paletteCommands()} onGoTo={(text) => goTo(text)} onClose={() => useExplorer.setState({ palette: false })} />}
        </div>
    );
}

/** The selection (or the folder) of the pane the keyboard acts on. */
function PreviewPane() {
    const { selected, items, folder, total } = useActiveView();
    return (
        <aside className="hidden w-[min(28%,28rem)] min-w-[16rem] shrink-0 border-l lg:flex">
            <FilePreview selected={selected} folder={{ path: folder, items, total }} />
        </aside>
    );
}

interface PaneProps {
    index: number;
    /** Files mode is on screen (keys are only handled then). */
    active: boolean;
    menuContext: MenuContext;
}

/** One side of the window: a tab strip over the list of the tab it shows, and that tab's status bar. */
function Pane(props: PaneProps) {
    return (
        <PaneProvider value={props.index}>
            <PaneView {...props} />
        </PaneProvider>
    );
}

function PaneView({ index, active, menuContext }: PaneProps) {
    const { tabId, path, listing, loading, find, filter, selection, focus, renaming, view, cut, sizes, dropTarget, quickLook, current, dual } = useExplorer(
        useShallow((s) => {
            const t = s.tabs[(s.panes[index] ?? s.panes[0]).active];
            return {
                tabId: t.id,
                path: t.path,
                listing: t.listing,
                loading: t.loading,
                find: t.find,
                filter: t.filter,
                selection: t.selection,
                focus: t.focus,
                renaming: t.renaming,
                view: t.view,
                cut: s.cut,
                sizes: s.sizes[t.path],
                dropTarget: s.dropTarget,
                quickLook: s.quickLook,
                current: s.activePane === index,
                dual: s.dual,
            };
        })
    );
    const settings = useStore((s) => s.settings);
    const { sort, foldersFirst, showHidden, editorPath } = settings;
    const { setSettings } = useStore.getState();
    const menu = useMenu();
    const height = rowHeight(settings.fontSize);
    const tile = useMemo(() => gridTile(settings.fontSize, settings.tileSize), [settings.fontSize, settings.tileSize]);
    const paneRef = useRef<HTMLDivElement>(null);
    // Handlers that outlive a render (drags, the rename box) act on the tab they started in.
    const tabRef = useRef(tabId);
    tabRef.current = tabId;

    // Search results stream in; deferring them keeps typing and scrolling smooth while they do.
    const found = useDeferredValue(find?.entries ?? NO_ENTRIES);
    const source = find ? found : (listing?.entries ?? NO_ENTRIES);
    const sizeRun = find ? undefined : sizes;
    const sorted = useMemo(() => {
        const sizeOf = sizeRun ? (e: FileEntry) => (e.dir ? (sizeRun.sizes.get(e.name) ?? -1) : e.size) : undefined;
        // Listings arrive sorted by name from Rust, so the default view needs no work here.
        return sortEntries(source, sort, foldersFirst, source === listing?.entries, sizeOf);
    }, [source, listing, sort, foldersFirst, sizeRun]);

    const deferredFilter = useDeferredValue(filter);
    const { rows: listRows, hidden } = useMemo(() => {
        const match = matcher(deferredFilter);
        if (!match && showHidden) return { rows: sorted, hidden: 0 };
        let hidden = 0;
        const rows = sorted.filter((e) => {
            if (!showHidden && isHidden(e)) {
                hidden++;
                return false;
            }
            return !match || match(e);
        });
        return { rows, hidden };
    }, [sorted, deferredFilter, showHidden]);
    // A tab showing its disk-usage map has the map's items as rows.
    const mapRows = useMapRows(tabId);
    const rows = mapRows ?? listRows;

    const focusIndex = useMemo(() => (focus ? rows.findIndex((r) => r.path === focus) : -1), [rows, focus]);
    const selected = useMemo(() => (selection.size ? rows.filter((r) => selection.has(r.path)) : NO_ENTRIES), [rows, selection]);
    // Quick Look follows the cursor, so arrowing through the list flips through previews.
    const looked = quickLook && current ? (rows[focusIndex] ?? selected[0]) : undefined;
    useEffect(() => {
        if (quickLook && current && !looked) useExplorer.setState({ quickLook: false });
    }, [quickLook, current, looked]);

    useLayoutEffect(() => markShown(tabId), [tabId, listing]);

    // The toolbar and the preview pane show what the active pane shows.
    const folder = find?.scope ?? path;
    const total = sizeRun?.total;
    useLayoutEffect(() => {
        if (current) useActiveView.setState({ selected, items: rows.length, folder, total });
    }, [current, selected, rows.length, folder, total]);

    const rowsRef = useRef(rows);
    rowsRef.current = rows;
    const typeahead = useRef({ text: "", at: 0 });

    const selectedNow = () => {
        const t = tabState(tabRef.current);
        return t ? rowsRef.current.filter((r) => t.selection.has(r.path)) : NO_ENTRIES;
    };

    const startRename = (target?: string) => {
        const t = tabState(tabRef.current);
        const path = target ?? t?.focus ?? (t ? [...t.selection][0] : undefined);
        if (path) updateTab({ renaming: path, selection: new Set([path]), focus: path, anchor: path }, tabRef.current);
    };

    // Explorer selects the next item after a delete.
    const deleteSelected = (permanent: boolean) => {
        const rows = rowsRef.current;
        const chosen = selectedNow();
        if (!chosen.length) return;
        const picked = new Set(chosen.map((r) => r.path));
        const first = rows.indexOf(chosen[0]);
        const last = rows.indexOf(chosen[chosen.length - 1]);
        const next = rows.slice(last + 1).find((r) => !picked.has(r.path)) ?? rows.slice(0, first).reverse().find((r) => !picked.has(r.path));
        deletePaths([...picked], permanent, next?.path);
    };

    const openSelected = async () => {
        const chosen = selectedNow();
        if (chosen.length === 1) return openEntry(chosen[0]);
        const files = chosen.filter((e) => !e.dir);
        if (!files.length) return chosen[0] && openEntry(chosen[0]);
        if (files.length > 10 && !(await confirm(`Open ${files.length} files?`, { title: "Open files", kind: "warning" }))) return;
        files.forEach(openEntry);
    };

    /** Ctrl+Enter: each selected folder in a new tab. */
    const openSelectedInTabs = () => {
        for (const entry of selectedNow()) if (entry.dir) openInNewTab(entry.path);
    };

    useEffect(() => {
        paneActions[index] = { startRename: () => startRename(), deleteSelected };
        return () => {
            paneActions[index] = undefined;
        };
    });

    useEffect(() => {
        // Only the active pane listens; the other one waits for a click (or F6).
        if (!active || !current) return;
        const onKey = (e: KeyboardEvent) => {
            if (e.defaultPrevented) return;
            const ctrl = e.ctrlKey || e.metaKey;
            const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
            const target = e.target instanceof Element ? e.target : null;
            // A focused button handles its own Enter and Space.
            if ((key === "Enter" || key === " ") && target?.closest("button, a")) return;
            // Fields keep their keys, except that navigation (and tabs) also work from the search box, as in Explorer.
            if (target?.closest("input, textarea, select, [contenteditable=true], dialog, [popover]")) {
                const navigation =
                    (e.altKey && !ctrl && ["ArrowLeft", "ArrowRight", "ArrowUp", "d"].includes(key)) ||
                    (ctrl && !e.altKey && ["l", "f", "e", "t", "w", "d", "Tab"].includes(key)) ||
                    ["F3", "F4", "F5", "F6"].includes(key);
                if (target.id !== FIND_BOX_ID || !navigation) return;
            }
            const rows = rowsRef.current;
            const tab = tabRef.current;
            const s = tabState(tab);
            if (!s) return;
            const at = s.focus ? rows.findIndex((r) => r.path === s.focus) : -1;
            // The grid moves by tiles and rows of tiles (Left and Right too), the list by rows.
            const grid = paneRef.current?.querySelector<HTMLElement>("[data-file-grid]");
            const cols = Number(grid?.dataset.cols) || 1;
            const lines = paneRef.current?.querySelector<HTMLElement>("[data-file-list]");
            const page = grid ? Math.max(1, Math.floor(grid.clientHeight / (Number(grid.dataset.rowHeight) || 1)) - 1) : Math.max(1, Math.floor((lines?.clientHeight ?? 400) / height) - 1);
            const to = gridMove(key, at, rows.length, cols, page);
            // Arrows select; with Shift they extend from the anchor; with Ctrl they only move the cursor.
            const go = (index: number) => {
                if (e.shiftKey) selectRange(rows, index, ctrl);
                else if (ctrl) updateTab({ focus: rows[index].path }, tab);
                else selectOnly(rows[index].path);
            };
            const { quickLook, dual } = useExplorer.getState();

            if (quickLook && (key === "Escape" || key === " ")) useExplorer.setState({ quickLook: false });
            else if (key === "F6") {
                if (!dual) return;
                switchPane();
            } else if (e.altKey && !ctrl) {
                if (key === "ArrowLeft") goBack();
                else if (key === "ArrowRight") goForward();
                else if (key === "ArrowUp") goUp();
                else if (key === "Enter") properties(s.focus ?? s.path);
                else if (key === "d") useExplorer.setState({ editingAddress: true });
                else if (key === "p") setSettings({ showPreviewPane: !useStore.getState().settings.showPreviewPane });
                else return;
            } else if (to !== null && !e.altKey) {
                go(to);
            } else if (ctrl) {
                const paths = selectedNow().map((r) => r.path);
                // As in Explorer: Ctrl+Shift+2 for large icons, Ctrl+Shift+6 for details.
                if (e.shiftKey && (e.code === "Digit2" || e.code === "Digit6") && (s.path !== THIS_PC || s.find)) chooseView(e.code === "Digit2" ? "grid" : "list", tab);
                // In the grid, as a browser zooms: Ctrl+= (or +) bigger tiles, Ctrl+- smaller, Ctrl+0 medium again.
                else if (grid && !e.altKey && ["=", "+", "-", "_", "0"].includes(key)) zoomGrid(key === "0" ? 0 : key === "-" || key === "_" ? -1 : 1);
                else if (key === "Tab") cycleTab(e.shiftKey ? -1 : 1);
                else if (key === "t" && e.shiftKey) reopenClosedTab();
                else if (key === "t") newTab();
                else if (key === "w") closeTab();
                else if (key === "d" && e.shiftKey) toggleDual();
                else if (/^[1-9]$/.test(key) && !e.shiftKey) selectTab(Number(key));
                else if (key === "Enter") openSelectedInTabs();
                else if (key === "a") selectAll(rows);
                else if (key === "c" && e.shiftKey) paths.length && copyText(paths.join("\r\n"), paths.length === 1 ? "path" : "paths");
                else if (key === "c") copyToClipboard(paths, false);
                else if (key === "x") copyToClipboard(paths, true);
                else if (key === "v") paste();
                else if (key === "n" && e.shiftKey) newItem(true);
                else if (key === "l") useExplorer.setState({ editingAddress: true });
                else if ((key === "f" || key === "e") && !e.shiftKey) focusFind();
                else if (key === "h") setSettings({ showHidden: !useStore.getState().settings.showHidden });
                else if (key === "r") refresh();
                else if (key === "k" || key === "p") useExplorer.setState({ palette: true });
                else if (key === " " && s.focus) toggleSelected(s.focus);
                else return;
            } else if (key === "Enter") {
                useExplorer.setState({ quickLook: false });
                openSelected();
            } else if (key === " " && !e.shiftKey && (s.focus || s.selection.size)) useExplorer.setState({ quickLook: true });
            else if (key === "Backspace") goBack();
            else if (key === "Delete") deleteSelected(e.shiftKey);
            else if (key === "F2") startRename();
            else if (key === "F3") focusFind();
            else if (key === "F4") useExplorer.setState({ editingAddress: true });
            else if (key === "F5") refresh();
            else if (key === "Escape") {
                if (s.find || s.filter) clearFind();
                else if (s.selection.size) selectOnly(null);
                else return;
            } else if (key.length === 1 && key !== " " && !e.altKey) {
                // Type-ahead: jump to the next name starting with what was typed; one letter repeated cycles.
                const t = typeahead.current;
                const now = Date.now();
                t.text = now - t.at > 900 ? key : t.text + key;
                t.at = now;
                const needle = [...t.text].every((c) => c === key) ? key : t.text;
                const start = needle.length === 1 ? at + 1 : Math.max(at, 0);
                for (let k = 0; k < rows.length; k++) {
                    const i = (start + k) % rows.length;
                    if (rows[i].lower.startsWith(needle)) {
                        selectOnly(rows[i].path);
                        break;
                    }
                }
            } else return;
            e.preventDefault();
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, [active, current, height]);

    // A press on a row can turn into dragging the selection out of the window (or into the other
    // pane). Like Explorer, pressing an item of a multi-selection keeps the selection until release,
    // so all of it can be dragged.
    const press = useRef<{ x: number; y: number; narrowTo: string | null } | null>(null);

    const onRowMouseDown = (e: React.MouseEvent, index: number) => {
        const entry = rows[index];
        if (e.button === 1) {
            // Middle-click: the folder in a new tab (and no autoscroll).
            e.preventDefault();
            if (entry.dir) openInNewTab(entry.path);
            return;
        }
        const selection = tabState(tabRef.current)?.selection ?? new Set();
        const additive = e.ctrlKey || e.metaKey;
        if (e.button === 2) {
            if (!selection.has(entry.path)) selectOnly(entry.path);
        } else if (e.button === 0) {
            const keep = !e.shiftKey && !additive && selection.has(entry.path) && selection.size > 1;
            if (e.shiftKey) selectRange(rows, index, additive);
            else if (additive) toggleSelected(entry.path);
            else if (keep) updateTab({ focus: entry.path, anchor: entry.path }, tabRef.current);
            else selectOnly(entry.path);
            press.current = { x: e.clientX, y: e.clientY, narrowTo: keep ? entry.path : null };
        }
    };

    useEffect(() => {
        const onMove = (e: MouseEvent) => {
            const start = press.current;
            if (!start) return;
            if (!(e.buttons & 1)) press.current = null;
            else if (Math.hypot(e.clientX - start.x, e.clientY - start.y) >= 6) {
                press.current = null;
                const paths = selectedNow().map((r) => r.path);
                // Moved away (e.g. into another drive's folder in Explorer): show what's left.
                if (!paths.length) return;
                const from = tabRef.current;
                startDrag(paths)
                    .then((effect) => {
                        if (effect === "move") reload(from);
                    })
                    .catch(report("Couldn't drag these items"));
            }
        };
        const onUp = () => {
            const start = press.current;
            press.current = null;
            if (start?.narrowTo) selectOnly(start.narrowTo);
        };
        window.addEventListener("mousemove", onMove);
        window.addEventListener("mouseup", onUp);
        return () => {
            window.removeEventListener("mousemove", onMove);
            window.removeEventListener("mouseup", onUp);
        };
    }, []);

    const { pinned, togglePin, onSearchText } = menuContext;
    const editorName = baseName(editorPath.trim()).replace(/\.exe$/i, "");
    const itemMenu = (targets: FileEntry[]): MenuItem[] => {
        const one = targets.length === 1 ? targets[0] : null;
        const paths = targets.map((t) => t.path);
        const folders = targets.filter((t) => t.dir);
        const other = dual && !!otherPaneFolder();
        return [
            { label: one?.dir ? "Open" : targets.length > 1 ? `Open ${targets.length} items` : "Open", icon: one?.dir ? FolderOpen : ExternalLink, shortcut: "Enter", onSelect: openSelected },
            folders.length > 0 && {
                label: folders.length > 1 ? `Open ${folders.length} folders in new tabs` : "Open in new tab",
                icon: SquarePlus,
                shortcut: "Ctrl+Enter",
                onSelect: () => folders.forEach((f) => openInNewTab(f.path)),
            },
            one && {
                label: "Quick Look",
                icon: Eye,
                shortcut: "Space",
                onSelect: () => {
                    updateTab({ focus: one.path }, tabRef.current);
                    useExplorer.setState({ quickLook: true });
                },
            },
            one && !one.dir && !!editorName && { label: `Open with ${editorName}`, icon: Code, onSelect: () => openPath(one.path, editorPath.trim()).catch(report("Couldn't open the file")) },
            one && !one.dir && { label: "Open with…", icon: AppWindow, onSelect: () => openWith(one.path).catch(report("Couldn't open the app picker")) },
            one?.dir && { label: "Open in Terminal", icon: SquareTerminal, onSelect: () => terminal(one.path) },
            find && one && { label: "Open file location", icon: FolderInput, onSelect: () => revealInApp(one.path) },
            { label: "Show in File Explorer", icon: FolderSearch, onSelect: () => revealItemInDir(paths).catch(report("Couldn't open File Explorer")) },
            "separator",
            { label: "Cut", icon: Scissors, shortcut: "Ctrl+X", onSelect: () => copyToClipboard(paths, true) },
            { label: "Copy", icon: Copy, shortcut: "Ctrl+C", onSelect: () => copyToClipboard(paths, false) },
            dual && { label: "Copy to other pane", icon: CopyPlus, disabled: !other, onSelect: () => copyToOtherPane(false, paths) },
            dual && { label: "Move to other pane", icon: FolderOutput, disabled: !other, onSelect: () => copyToOtherPane(true, paths) },
            one?.dir && { label: "Paste into folder", icon: ClipboardPaste, onSelect: () => paste(one.path) },
            { label: paths.length === 1 ? "Copy path" : "Copy paths", icon: ClipboardCopy, shortcut: "Ctrl+Shift+C", onSelect: () => copyText(paths.join("\r\n"), paths.length === 1 ? "path" : "paths") },
            "separator",
            one && { label: "Rename", icon: PenLine, shortcut: "F2", onSelect: () => startRename(one.path) },
            { label: "Delete", icon: Trash2, shortcut: "Del", danger: true, onSelect: () => deleteSelected(false) },
            "separator",
            one?.dir && { label: pinned.includes(one.path) ? "Unpin from sidebar" : "Pin to sidebar", icon: pinned.includes(one.path) ? PinOff : Pin, onSelect: () => togglePin(one.path) },
            one?.dir && { label: "Search text in folder", icon: TextSearch, onSelect: () => onSearchText(one.path) },
            one && { label: "Properties", icon: Info, shortcut: "Alt+Enter", onSelect: () => properties(one.path) },
        ];
    };

    const onRowContextMenu = (e: React.MouseEvent, index: number) => {
        e.preventDefault();
        const entry = rows[index];
        const chosen = selectedNow();
        menu.open(e, itemMenu(chosen.some((c) => c.path === entry.path) ? chosen : [entry]));
    };

    const onPlaceContextMenu = (e: React.MouseEvent, place: string) => {
        e.preventDefault();
        menu.open(e, folderMenu(place, false, menuContext));
    };

    /** Right-click where the open folder shows but none of its items does. */
    const onFolderContextMenu = (e: React.MouseEvent) => {
        e.preventDefault();
        selectOnly(null);
        menu.open(e, folderMenu(path, true, menuContext, { find: !!find, selectAll: () => selectAll(rowsRef.current) }));
    };

    const onSort = (key: SortKey) => setSettings({ sort: { key, desc: sort.key === key ? !sort.desc : key === "modified" || key === "size" } });

    let empty: React.ReactNode = null;
    if (find?.error) {
        empty = (
            <Centered>
                <AlertTriangle className="size-6 text-destructive" />
                <p className="font-medium text-foreground">Search failed</p>
                <pre className="max-w-full select-text whitespace-pre-wrap font-mono text-xs">{find.error}</pre>
            </Centered>
        );
    } else if (find?.running && !rows.length) {
        empty = (
            <Centered>
                <Loader2 className="size-5 animate-spin text-primary" /> Searching…
            </Centered>
        );
    } else if (find && !rows.length) {
        empty = (
            <Centered>
                <SearchX className="size-6" />
                <p className="font-medium text-foreground">Nothing named like “{find.query}”</p>
                <p className="max-w-sm">
                    {settings.findExclusions.length ? `Skipped: ${settings.findExclusions.join(", ")}. ` : ""}
                    {showHidden ? "" : "Hidden files aren't searched. "}You can change this in Settings.
                </p>
            </Centered>
        );
    } else if (listing && !rows.length) {
        empty = filter ? (
            <Centered>
                <SearchX className="size-6" />
                <p>Nothing in this folder matches “{filter}”.</p>
                <Button variant="outline" size="sm" onClick={() => runFind(filter)}>
                    <Search /> Search subfolders too
                </Button>
            </Centered>
        ) : listing.entries.length ? (
            <Centered>
                <p>This folder only has hidden items.</p>
                <Button variant="outline" size="sm" onClick={() => setSettings({ showHidden: true })}>
                    Show hidden items
                </Button>
            </Centered>
        ) : (
            <Centered>This folder is empty.</Centered>
        );
    }

    const scopeName = find && (find.scope === THIS_PC ? "all drives" : baseName(find.scope));
    const listKey = viewKey({ path, find });
    // Each view keeps its own place in the folder: a list's scroll position means nothing to the grid.
    const placeKey = `${listKey}:${view}`;
    const viewProps = {
        rows,
        search: !!find,
        scope: find?.scope ?? path,
        sizes: sizeRun,
        sort,
        onSort,
        selection,
        focusIndex,
        renaming,
        cut,
        dropTarget,
        inactive: dual && !current,
        lane: tabId,
        thumbScope: placeKey,
        scrollTop: savedScroll(tabId, placeKey),
        onScrollTop: (top: number) => saveScroll(tabId, placeKey, top),
        onRowMouseDown,
        onRowContextMenu,
        onOpen: openEntry,
        onBackgroundMouseDown: (e: React.MouseEvent) => e.button === 0 && !e.ctrlKey && !e.shiftKey && selectOnly(null),
        onBackgroundContextMenu: onFolderContextMenu,
        onRename: (entry: FileEntry, name: string) => renameEntry(entry, name, tabRef.current),
        onCancelRename: () => updateTab({ renaming: null }, tabRef.current),
        empty,
    };

    return (
        <div
            ref={paneRef}
            data-pane={index}
            onMouseDownCapture={() => focusPane(index)}
            className={cn("relative flex min-w-0 flex-1 basis-0 flex-col", index > 0 && "border-l")}
        >
            <TabStrip pane={index} onContextMenu={(e, id) => menu.open(e, tabMenu(id))} />
            {loading && <div className="absolute inset-x-0 top-9 z-10 h-0.5 animate-pulse bg-primary" />}
            {find && (
                <div className="flex h-9 shrink-0 items-center gap-2 border-b bg-primary/5 px-3 text-sm">
                    {find.running ? <Loader2 className="size-4 shrink-0 animate-spin text-primary" /> : <Search className="size-4 shrink-0 text-primary" />}
                    <span className="truncate">
                        Results for <b className="font-semibold">“{find.query}”</b> in {scopeName}
                        {find.scope !== THIS_PC && " and its subfolders"}
                    </span>
                    {find.limited && (
                        <span className="shrink-0 rounded-md bg-amber-500/10 px-2 py-0.5 text-xs font-medium text-amber-600 dark:text-amber-400">
                            Stopped at {formatCount(settings.findMaxResults ?? 0)}
                        </span>
                    )}
                    <div className="ml-auto flex shrink-0 gap-1">
                        {find.running && (
                            <Button variant="ghost" size="sm" onClick={stopFind}>
                                <Square className="size-3! fill-current" /> Stop
                            </Button>
                        )}
                        <Button variant="ghost" size="sm" onClick={clearFind} title="Back to the folder (Esc)">
                            <X /> Close
                        </Button>
                    </div>
                </div>
            )}
            <div className="min-h-0 flex-1">
                {path === THIS_PC && !find ? (
                    <ThisPC onContextMenu={onPlaceContextMenu} />
                ) : mapRows ? (
                    <DiskUsage tab={tabId} onItemContextMenu={onRowContextMenu} onFolderContextMenu={onFolderContextMenu} />
                ) : view === "grid" ? (
                    <FileGrid key={`${tabId}:${placeKey}`} {...viewProps} pane={index} tile={tile} onZoom={zoomGrid} />
                ) : (
                    <FileList key={`${tabId}:${placeKey}`} {...viewProps} rowHeight={height} thumbnails={settings.listThumbnails} iconBox={remPx(settings.fontSize)} />
                )}
            </div>
            <StatusBar shown={rows.length} hidden={hidden} selected={selected} />
            {menu.element}
            {looked && <QuickLook entry={looked} onClose={() => useExplorer.setState({ quickLook: false })} />}
        </div>
    );
}
