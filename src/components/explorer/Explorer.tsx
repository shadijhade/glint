import { useDeferredValue, useEffect, useLayoutEffect, useMemo, useRef } from "react";
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
    ArrowUp,
    ClipboardCopy,
    ClipboardPaste,
    Code,
    Copy,
    ExternalLink,
    Eye,
    EyeOff,
    FilePlus,
    Folder,
    FolderInput,
    FolderOpen,
    FolderPlus,
    FolderSearch,
    HardDrive,
    Info,
    ListChecks,
    Loader2,
    Monitor,
    Moon,
    PanelRight,
    Pin,
    PenLine,
    PinOff,
    RotateCw,
    Scissors,
    Search,
    SearchX,
    Settings as SettingsIcon,
    Square,
    SquareTerminal,
    Sun,
    TextSearch,
    Trash2,
    X,
} from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { type FileEntry, isHidden, matcher, sortEntries, type SortKey } from "@/lib/entries";
import {
    calcSizes,
    clearFind,
    copyToClipboard,
    deletePaths,
    dropFiles,
    goBack,
    goForward,
    goTo,
    goUp,
    initExplorer,
    markShown,
    navigate,
    newItem,
    openEntry,
    paste,
    refresh,
    refreshDrives,
    reload,
    renameEntry,
    revealInApp,
    runFind,
    selectAll,
    selectOnly,
    selectRange,
    stopFind,
    toggleSelected,
    useExplorer,
} from "@/lib/explorer";
import { formatCount } from "@/lib/format";
import { driveName, openTerminal, openWith, showProperties, startDrag } from "@/lib/fs";
import { baseName, samePath, THIS_PC } from "@/lib/paths";
import { frecency } from "@/lib/rank";
import { useStore } from "@/lib/store";
import { rowHeight } from "@/hooks/useTheme";
import { type MenuItem, useMenu } from "../Menu";
import { Button } from "../ui/button";
import { FIND_BOX_ID } from "./AddressBar";
import { CommandBar } from "./CommandBar";
import { type Command, CommandPalette } from "./CommandPalette";
import { FileList } from "./FileList";
import { FilePreview } from "./FilePreview";
import { QuickLook } from "./QuickLook";
import { DRIVE_ICONS, KNOWN_ICONS, Sidebar } from "./Sidebar";
import { StatusBar } from "./StatusBar";
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

interface ExplorerProps {
    active: boolean;
    /** Switch to text search (ripgrep) in `folder`. */
    onSearchText: (folder: string) => void;
    onOpenSettings: () => void;
}

export function Explorer({ active, onSearchText, onOpenSettings }: ExplorerProps) {
    const { path, listing, loading, find, filter, selection, focus, renaming, cut, sizes, dropTarget, quickLook, palette } = useExplorer(
        useShallow((s) => ({
            path: s.path,
            listing: s.listing,
            loading: s.loading,
            find: s.find,
            filter: s.filter,
            selection: s.selection,
            focus: s.focus,
            renaming: s.renaming,
            cut: s.cut,
            sizes: s.sizes[s.path],
            dropTarget: s.dropTarget,
            quickLook: s.quickLook,
            palette: s.palette,
        }))
    );
    const settings = useStore((s) => s.settings);
    const { sort, foldersFirst, showHidden, showPreviewPane, editorPath } = settings;
    const pinned = useStore((s) => s.pinned);
    const { setSettings, togglePin } = useStore.getState();
    const menu = useMenu();
    const height = rowHeight(settings.fontSize);

    useEffect(() => {
        initExplorer();
        // Refresh when the open folder changes on disk; drives may come and go while we're away.
        const unlisten = listen<string>("dir-changed", (event) => {
            const s = useExplorer.getState();
            if (!s.find && s.path !== THIS_PC && samePath(event.payload, s.path)) reload();
        });
        const onFocus = () => refreshDrives();
        window.addEventListener("focus", onFocus);
        return () => {
            unlisten.then((stop) => stop());
            window.removeEventListener("focus", onFocus);
        };
    }, []);

    const activeRef = useRef(active);
    activeRef.current = active;

    // Files dragged in from File Explorer or the desktop land on the folder row or sidebar place
    // under the pointer, or else in the open folder.
    useEffect(() => {
        const targetAt = ({ x, y }: { x: number; y: number }) => {
            const s = useExplorer.getState();
            const under = document.elementFromPoint(x / devicePixelRatio, y / devicePixelRatio);
            const place = under?.closest<HTMLElement>("[data-drop]")?.dataset.drop;
            if (place) return place;
            return s.path !== THIS_PC && !s.find && under?.closest(".files") ? s.path : null;
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
    const { rows, hidden } = useMemo(() => {
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

    const focusIndex = useMemo(() => (focus ? rows.findIndex((r) => r.path === focus) : -1), [rows, focus]);
    const selected = useMemo(() => (selection.size ? rows.filter((r) => selection.has(r.path)) : NO_ENTRIES), [rows, selection]);
    // Quick Look follows the cursor, so arrowing through the list flips through previews.
    const looked = quickLook ? (rows[focusIndex] ?? selected[0]) : undefined;
    useEffect(() => {
        if (quickLook && !looked) useExplorer.setState({ quickLook: false });
    }, [quickLook, looked]);

    useLayoutEffect(markShown, [listing]);

    const rowsRef = useRef(rows);
    rowsRef.current = rows;
    const typeahead = useRef({ text: "", at: 0 });

    const selectedNow = () => {
        const s = useExplorer.getState();
        return rowsRef.current.filter((r) => s.selection.has(r.path));
    };

    const startRename = (target?: string) => {
        const s = useExplorer.getState();
        const path = target ?? s.focus ?? [...s.selection][0];
        if (path) useExplorer.setState({ renaming: path, selection: new Set([path]), focus: path, anchor: path });
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

    const searchTextHere = () => {
        const s = useExplorer.getState();
        if (s.path !== THIS_PC) onSearchText(s.path);
    };

    useEffect(() => {
        if (!active) return;
        const onKey = (e: KeyboardEvent) => {
            if (e.defaultPrevented) return;
            const ctrl = e.ctrlKey || e.metaKey;
            const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
            const target = e.target instanceof Element ? e.target : null;
            // A focused button handles its own Enter and Space.
            if ((key === "Enter" || key === " ") && target?.closest("button, a")) return;
            // Fields keep their keys, except that navigation also works from the search box, as in Explorer.
            if (target?.closest("input, textarea, select, [contenteditable=true], dialog, [popover]")) {
                const navigation =
                    (e.altKey && !ctrl && ["ArrowLeft", "ArrowRight", "ArrowUp", "d"].includes(key)) ||
                    (ctrl && !e.altKey && ["l", "f", "e"].includes(key)) ||
                    ["F3", "F4", "F5"].includes(key);
                if (target.id !== FIND_BOX_ID || !navigation) return;
            }
            const rows = rowsRef.current;
            const s = useExplorer.getState();
            const at = s.focus ? rows.findIndex((r) => r.path === s.focus) : -1;
            const page = Math.max(1, Math.floor((document.querySelector("[data-file-list]")?.clientHeight ?? 400) / height) - 1);
            // Arrows select; with Shift they extend from the anchor; with Ctrl they only move the cursor.
            const go = (to: number) => {
                if (!rows.length) return;
                const index = Math.max(0, Math.min(rows.length - 1, to));
                if (e.shiftKey) selectRange(rows, index, ctrl);
                else if (ctrl) useExplorer.setState({ focus: rows[index].path });
                else selectOnly(rows[index].path);
            };
            const moves: Record<string, () => number> = {
                ArrowDown: () => at + 1,
                ArrowUp: () => (at < 0 ? 0 : at - 1),
                PageDown: () => at + page,
                PageUp: () => at - page,
                Home: () => 0,
                End: () => rows.length - 1,
            };

            if (s.quickLook && (key === "Escape" || key === " ")) useExplorer.setState({ quickLook: false });
            else if (e.altKey && !ctrl) {
                if (key === "ArrowLeft") goBack();
                else if (key === "ArrowRight") goForward();
                else if (key === "ArrowUp") goUp();
                else if (key === "Enter") properties(s.focus ?? s.path);
                else if (key === "d") useExplorer.setState({ editingAddress: true });
                else if (key === "p") setSettings({ showPreviewPane: !useStore.getState().settings.showPreviewPane });
                else return;
            } else if (moves[key] && !(e.altKey && ctrl)) {
                go(moves[key]());
            } else if (ctrl) {
                const paths = selectedNow().map((r) => r.path);
                if (key === "a") selectAll(rows);
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
    }, [active, height]);

    // A press on a row can turn into dragging the selection out of the window. Like Explorer, pressing
    // an item of a multi-selection keeps the selection until release, so all of it can be dragged.
    const press = useRef<{ x: number; y: number; narrowTo: string | null } | null>(null);

    const onRowMouseDown = (e: React.MouseEvent, index: number) => {
        const entry = rows[index];
        const { selection } = useExplorer.getState();
        const additive = e.ctrlKey || e.metaKey;
        if (e.button === 2) {
            if (!selection.has(entry.path)) selectOnly(entry.path);
        } else if (e.button === 0) {
            const keep = !e.shiftKey && !additive && selection.has(entry.path) && selection.size > 1;
            if (e.shiftKey) selectRange(rows, index, additive);
            else if (additive) toggleSelected(entry.path);
            else if (keep) useExplorer.setState({ focus: entry.path, anchor: entry.path });
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
                startDrag(paths)
                    .then((effect) => {
                        if (effect === "move") reload();
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

    const editorName = baseName(editorPath.trim()).replace(/\.exe$/i, "");
    const itemMenu = (targets: FileEntry[]): MenuItem[] => {
        const one = targets.length === 1 ? targets[0] : null;
        const paths = targets.map((t) => t.path);
        return [
            { label: one?.dir ? "Open" : targets.length > 1 ? `Open ${targets.length} items` : "Open", icon: one?.dir ? FolderOpen : ExternalLink, shortcut: "Enter", onSelect: openSelected },
            one && { label: "Quick Look", icon: Eye, shortcut: "Space", onSelect: () => useExplorer.setState({ focus: one.path, quickLook: true }) },
            one && !one.dir && !!editorName && { label: `Open with ${editorName}`, icon: Code, onSelect: () => openPath(one.path, editorPath.trim()).catch(report("Couldn't open the file")) },
            one && !one.dir && { label: "Open with…", icon: AppWindow, onSelect: () => openWith(one.path).catch(report("Couldn't open the app picker")) },
            one?.dir && { label: "Open in Terminal", icon: SquareTerminal, onSelect: () => terminal(one.path) },
            find && one && { label: "Open file location", icon: FolderInput, onSelect: () => revealInApp(one.path) },
            { label: "Show in File Explorer", icon: FolderSearch, onSelect: () => revealItemInDir(paths).catch(report("Couldn't open File Explorer")) },
            "separator",
            { label: "Cut", icon: Scissors, shortcut: "Ctrl+X", onSelect: () => copyToClipboard(paths, true) },
            { label: "Copy", icon: Copy, shortcut: "Ctrl+C", onSelect: () => copyToClipboard(paths, false) },
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

    /** For the open folder (right-click on empty space) or a place in the sidebar. */
    const folderMenu = (folder: string, current: boolean): MenuItem[] => {
        if (folder === THIS_PC) return [{ label: "Refresh", icon: RotateCw, shortcut: "F5", onSelect: refreshDrives }];
        const isPinned = pinned.includes(folder);
        return [
            !current && { label: "Open", icon: FolderOpen, onSelect: () => navigate(folder) },
            current && { label: "New folder", icon: FolderPlus, shortcut: "Ctrl+Shift+N", disabled: !!find, onSelect: () => newItem(true) },
            current && { label: "New text document", icon: FilePlus, disabled: !!find, onSelect: () => newItem(false) },
            "separator",
            { label: current ? "Paste" : "Paste into folder", icon: ClipboardPaste, shortcut: current ? "Ctrl+V" : undefined, disabled: current && !!find, onSelect: () => paste(folder) },
            current && { label: "Select all", icon: ListChecks, shortcut: "Ctrl+A", onSelect: () => selectAll(rowsRef.current) },
            current && { label: "Refresh", icon: RotateCw, shortcut: "F5", onSelect: refresh },
            "separator",
            current && { label: "Folder sizes", icon: HardDrive, disabled: !!find, onSelect: calcSizes },
            { label: "Search text here", icon: TextSearch, onSelect: () => onSearchText(folder) },
            { label: "Copy path", icon: ClipboardCopy, onSelect: () => copyText(folder, "path") },
            "separator",
            { label: "Open in File Explorer", icon: FolderSearch, onSelect: () => openPath(folder).catch(report("Couldn't open File Explorer")) },
            { label: "Open in Terminal", icon: SquareTerminal, onSelect: () => terminal(folder) },
            { label: isPinned ? "Unpin from sidebar" : "Pin to sidebar", icon: isPinned ? PinOff : Pin, onSelect: () => togglePin(folder) },
            { label: "Properties", icon: Info, shortcut: current ? "Alt+Enter" : undefined, onSelect: () => properties(folder) },
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
        menu.open(e, folderMenu(place, false));
    };

    /** Everything the command palette offers: folders you use (most-used first) and every action. */
    const paletteCommands = (): Command[] => {
        const s = useExplorer.getState();
        const { visits, settings: now } = useStore.getState();
        const inFolder = s.path !== THIS_PC;
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
        const isPinned = pinned.includes(s.path);
        const actions: (Command | false)[] = [
            inFolder && !s.find && action("new-folder", "New folder", FolderPlus, () => newItem(true), "Ctrl+Shift+N"),
            inFolder && !s.find && action("new-file", "New text document", FilePlus, () => newItem(false)),
            action("find", "Find files in this folder", Search, focusFind, "Ctrl+F"),
            inFolder && action("search-text", "Search text in this folder", TextSearch, () => onSearchText(s.path), "Ctrl+Shift+F"),
            inFolder && !s.find && action("sizes", "Calculate folder sizes", HardDrive, calcSizes),
            !!s.focus && action("quick-look", "Quick Look", Eye, () => useExplorer.setState({ quickLook: true }), "Space"),
            action("hidden", now.showHidden ? "Hide hidden items" : "Show hidden items", now.showHidden ? EyeOff : Eye, () => setSettings({ showHidden: !now.showHidden }), "Ctrl+H"),
            action("preview", now.showPreviewPane ? "Hide preview pane" : "Show preview pane", PanelRight, () => setSettings({ showPreviewPane: !now.showPreviewPane }), "Alt+P"),
            action("refresh", "Refresh", RotateCw, refresh, "F5"),
            s.back.length > 0 && action("back", "Back", ArrowLeft, goBack, "Alt+Left"),
            s.forward.length > 0 && action("forward", "Forward", ArrowRight, goForward, "Alt+Right"),
            inFolder && action("up", "Up one folder", ArrowUp, goUp, "Alt+Up"),
            action("this-pc", "This PC", Monitor, () => navigate(THIS_PC)),
            inFolder && action("copy-path", "Copy this folder's path", ClipboardCopy, () => copyText(s.path, "path")),
            inFolder && action("terminal", "Open in Terminal", SquareTerminal, () => terminal(s.path)),
            inFolder && action("explorer", "Open in File Explorer", FolderSearch, () => openPath(s.path).catch(report("Couldn't open File Explorer"))),
            inFolder && action("pin", isPinned ? "Unpin this folder" : "Pin this folder to the sidebar", isPinned ? PinOff : Pin, () => togglePin(s.path)),
            inFolder && action("properties", "Properties", Info, () => properties(s.path), "Alt+Enter"),
            action("theme", now.theme === "light" ? "Switch to dark theme" : "Switch to light theme", now.theme === "light" ? Moon : Sun, () => setSettings({ theme: now.theme === "light" ? "dark" : "light" })),
            action("settings", "Settings", SettingsIcon, onOpenSettings, "Ctrl+,"),
        ];
        return [...folders, ...actions.filter((a): a is Command => !!a)];
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

    return (
        <div className={active ? "flex min-h-0 flex-1" : "hidden"}>
            <Sidebar onContextMenu={onPlaceContextMenu} />
            <div className="relative flex min-w-0 flex-1 flex-col">
                <CommandBar selected={selected} onRename={() => startRename()} onDelete={() => deleteSelected(false)} onSearchText={searchTextHere} />
                {loading && <div className="absolute inset-x-0 top-10 z-10 h-0.5 animate-pulse bg-primary" />}
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
                    ) : (
                        <FileList
                            key={find ? `find:${find.id}` : `dir:${path}`}
                            rows={rows}
                            rowHeight={height}
                            search={!!find}
                            scope={find?.scope ?? path}
                            sizes={sizeRun}
                            sort={sort}
                            onSort={onSort}
                            selection={selection}
                            focusIndex={focusIndex}
                            renaming={renaming}
                            cut={cut}
                            dropTarget={dropTarget}
                            onRowMouseDown={onRowMouseDown}
                            onRowContextMenu={onRowContextMenu}
                            onOpen={openEntry}
                            onBackgroundMouseDown={(e) => e.button === 0 && !e.ctrlKey && !e.shiftKey && selectOnly(null)}
                            onBackgroundContextMenu={(e) => {
                                e.preventDefault();
                                selectOnly(null);
                                menu.open(e, folderMenu(path, true));
                            }}
                            onRename={renameEntry}
                            onCancelRename={() => useExplorer.setState({ renaming: null })}
                            empty={empty}
                        />
                    )}
                </div>
                <StatusBar shown={rows.length} hidden={hidden} selected={selected} />
            </div>
            {showPreviewPane && (path !== THIS_PC || find) && (
                <aside className="hidden w-[min(28%,28rem)] min-w-[16rem] shrink-0 border-l lg:flex">
                    <FilePreview selected={selected} folder={{ path: find?.scope ?? path, items: rows.length, total: sizeRun?.total }} />
                </aside>
            )}
            {menu.element}
            {looked && <QuickLook entry={looked} onClose={() => useExplorer.setState({ quickLook: false })} />}
            {palette && (
                <CommandPalette commands={paletteCommands()} onGoTo={(text) => goTo(text)} onClose={() => useExplorer.setState({ palette: false })} />
            )}
        </div>
    );
}
