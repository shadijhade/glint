import { useDeferredValue, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirm } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import {
    AlertTriangle,
    AppWindow,
    ArchiveRestore,
    ArrowLeft,
    ArrowRight,
    ArrowRightLeft,
    ArrowUp,
    Bookmark,
    BookmarkCheck,
    BrushCleaning,
    ChevronLeft,
    ChevronRight,
    ClipboardCopy,
    ClipboardPaste,
    Cloud,
    Code,
    Columns2,
    Copy,
    CopyPlus,
    Download,
    ExternalLink,
    Eye,
    EyeOff,
    FileArchive,
    FileClock,
    FileJson,
    FilePlus,
    Files,
    Fingerprint,
    Folder,
    FolderCog,
    FolderInput,
    FolderLock,
    FolderOpen,
    FolderOutput,
    FolderPlus,
    FolderSearch,
    GitCompareArrows,
    HardDrive,
    History,
    ImageDown,
    Info,
    Keyboard,
    KeyRound,
    LayoutDashboard,
    LayoutGrid,
    List,
    ListChecks,
    Loader2,
    Lock,
    Monitor,
    Moon,
    PanelRight,
    PenLine,
    PencilOff,
    Pin,
    PinOff,
    Play,
    PlugZap,
    Plus,
    RotateCcw,
    RotateCw,
    ScanSearch,
    Scissors,
    ScrollText,
    Search,
    SearchX,
    Send,
    Server,
    Settings as SettingsIcon,
    ShieldCheck,
    SlidersHorizontal,
    Square,
    SquarePlus,
    SquareTerminal,
    Sun,
    TextSearch,
    Trash2,
    Undo2,
    Wand2,
    Wifi,
    X,
    ZoomIn,
    ZoomOut,
} from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { SINGLE_FILE } from "@/lib/archives";
import { scanWithDefender } from "@/lib/defender";
import { chooseView, showsUsage, toggleUsage, useMapRows } from "@/lib/diskUsage";
import { compareEntries, type DiffStatus, type FileEntry, hasPropFilter, type InArchive, isArchived, isArchiveFile, isBrowsable, isClosed, isHidden, isRecycled, isRemote, matcher, sortEntries, type SortKey } from "@/lib/entries";
import {
    archiveOf,
    calcSizes,
    canReopenTab,
    clearFind,
    closedNotice,
    closeOtherTabs,
    compressEntries,
    closeTab,
    copyToClipboard,
    copyToOtherPane,
    deepKey,
    exportAsNcdu,
    extractHere,
    currentPath,
    cycleTab,
    deletePaths,
    dragFromArchive,
    dropFiles,
    duplicateTab,
    emptyRecycleBin,
    focusPane,
    getAccess,
    goBack,
    goForward,
    goTo,
    goUp,
    initExplorer,
    keyboardElsewhere,
    markShown,
    moveTabToOtherPane,
    modalOpen,
    navigate,
    newItem,
    newTab,
    nextUndo,
    type NoAccess,
    openEntry,
    openArchivedWith,
    openFile,
    openInNewTab,
    otherPaneFolder,
    PaneProvider,
    paste,
    purgeRecycled,
    readOnlyNotice,
    recycleBinOf,
    remoteOf,
    recycledNotice,
    refresh,
    refreshDrives,
    reload,
    reloadFolder,
    reloadRecycleBins,
    renameEntry,
    reopenClosedTab,
    restoreRecycled,
    revealInApp,
    runDeepCompare,
    runFind,
    runSavedSearch,
    savedScroll,
    saveScroll,
    selectAll,
    selectOnly,
    selectRange,
    selectTab,
    stopDeepCompare,
    stopFind,
    switchPane,
    syncInto,
    tabState,
    toggleComparePanes,
    toggleDiffOnly,
    toggleDual,
    toggleSelected,
    undo,
    updateTab,
    useDeepCompare,
    useExplorer,
    useTab,
    viewKey,
    zoomGrid,
} from "@/lib/explorer";
import { formatCount, plural } from "@/lib/format";
import { driveName, openIn, openItem, openTerminal, openWith, showProperties, startDrag } from "@/lib/fs";
import { useGit } from "@/lib/git";
import { readFacts, readTaken, takenKey, useFacts } from "@/lib/media";
import { gridMove, TILE_SIZES, zoomTiles } from "@/lib/grid";
import { baseName, inArchivePath, isFolderPath, isRecycleBin, isRemotePath, isRoot, joinPath, parentPath, RECYCLE_BIN, samePath, THIS_PC } from "@/lib/paths";
import { remoteOfPath, remotePath } from "@/lib/remotes";
import { useStore } from "@/lib/store";
import { refreshZoxide, usedFolders, usedScore, useZoxide } from "@/lib/zoxide";
import { openInItems, refreshOpeners, useOpeners } from "@/lib/openers";
import { knownTasks, runTask } from "@/lib/projects";
import { openScriptsFolder, refreshScripts, runScript, scriptItems, useScripts } from "@/lib/scripts";
import { openReceive, recheckShares, showShare, useShare } from "@/lib/share";
import { toggleTerminal, useTerminals } from "@/lib/terminal";
import { cn } from "@/lib/utils";
import { gridTile, remPx, rowHeight } from "@/hooks/useTheme";
import { type MenuItem, useMenu } from "../Menu";
import { openShortcuts } from "../ShortcutsDialog";
import { ResizeHandle } from "../ResizeHandle";
import { Button } from "../ui/button";
import { FIND_BOX_ID } from "./AddressBar";
import { openBatchRename } from "./BatchRenameDialog";
import { CommandBar } from "./CommandBar";
import { type Command, CommandPalette } from "./CommandPalette";
import { checkForSecrets, checkWithYara, secretsCheckFirst } from "./CheckDialog";
import { offerOnNetwork, offerToPC } from "./ShareDialog";
import { addRemote, editRemote, removeRemote } from "./RemoteDialog";
import { openRules } from "./RulesDialog";
import { openCleanup } from "./CleanupDialog";
import { compareFiles } from "./CompareFilesDialog";
import { canConvert, convertPictures } from "./ConvertDialog";
import { DiskUsage } from "./DiskUsage";
import { inspectDuplicates } from "./DuplicatesDialog";
import { FileGrid } from "./FileGrid";
import { FileList } from "./FileList";
import { FilePreview } from "./FilePreview";
import { openHistory } from "./HistoryDialog";
import { inspectLocks } from "./LocksDialog";
import { QuickLook } from "./QuickLook";
import { DRIVE_ICONS, KNOWN_ICONS, Sidebar } from "./Sidebar";
import { StatusBar } from "./StatusBar";
import { TabStrip } from "./TabStrip";
import { ReadmeCard, readmeOf } from "./ReadmeCard";
import { TerminalPanel, terminalFolder } from "./TerminalPanel";
import { ThisPC } from "./ThisPC";
import { inspectTimeline } from "./TimelineDialog";
import { inspectXRay } from "./XRayDialog";

const NO_ENTRIES: FileEntry[] = [];

const copyText = (text: string, what: string) =>
    navigator.clipboard.writeText(text).then(
        () => toast.success(`Copied ${what}`, { duration: 1500 }),
        () => toast.error(`Couldn't copy ${what}`)
    );

const report = (what: string) => (e: unknown) => toast.error(what, { description: String(e) });
const properties = (path: string) => showProperties(path).catch(report("Couldn't show the properties"));
/** File Explorer, with the archive file selected: what's inside it isn't a place File Explorer can show here. */
const revealArchive = (archive: InArchive) => revealItemInDir(archive.outer).catch(report("Couldn't open File Explorer"));
/** A menu's last line in an archive: why the commands that change things are greyed out. */
const READ_ONLY_NOTE = { label: "Read-only: inside an archive", icon: PencilOff, disabled: true, onSelect: () => {} };
/** On a remote connection: why nothing changes. */
const REMOTE_NOTE = { label: "Read-only: on a remote connection", icon: PencilOff, disabled: true, onSelect: () => {} };
/** In a folder listed from the index: why nothing opens or changes. */
const CLOSED_NOTE = { label: "Read-only: Windows keeps this folder closed", icon: FolderLock, disabled: true, onSelect: () => {} };
/** Undo, saying what it takes back (the background menu, the palette). */
const undoLabel = () => {
    const step = nextUndo();
    return step ? `Undo the ${step.label}` : "Undo";
};
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

/** A folder Windows keeps this account out of: why, and Get access (File Explorer's Continue button). */
function NoAccessView({ tab, path, info }: { tab: string; path: string; info: NoAccess }) {
    const [working, setWorking] = useState(false);
    const name = baseName(path) || path;
    const explorer = (
        <Button variant="outline" size="sm" onClick={() => openPath(path).catch(report("Couldn't open File Explorer"))}>
            <FolderSearch /> Open in File Explorer
        </Button>
    );
    const security = (
        <Button variant="outline" size="sm" onClick={() => properties(path)}>
            <Info /> Properties
        </Button>
    );
    let title = `You don't have permission to open “${name}”`;
    let text: string;
    let actions: React.ReactNode;
    if (info.remote) {
        text = "It's on another computer, which decides who may open it. File Explorer can ask for a different user name and password.";
        actions = explorer;
    } else if (info.refused) {
        title = `Windows keeps “${name}” to itself`;
        text = "Even administrators may not open it without taking it over, which can stop Windows, or the apps it holds, from working.";
        actions = security;
    } else if (info.stillDenied) {
        title = `“${name}” still won't open`;
        text = "Your account may open it now, but a rule in its permissions keeps you out. Its Properties show who may open it, on the Security tab.";
        actions = security;
    } else {
        text = "Glint can give your account permission to open it and everything in it, as File Explorer's Continue button does. Windows asks for administrator approval.";
        actions = (
            <>
                <Button
                    size="sm"
                    disabled={working}
                    onClick={async () => {
                        setWorking(true);
                        await getAccess(tab);
                        setWorking(false);
                    }}
                >
                    {working ? <Loader2 className="animate-spin" /> : <ShieldCheck />} Get access
                </Button>
                {explorer}
            </>
        );
    }
    return (
        <Centered>
            <FolderLock className="size-8 stroke-[1.25] text-muted-foreground/60" />
            <p className="max-w-md text-balance font-medium text-foreground">{title}</p>
            <p className="max-w-sm text-balance">{text}</p>
            <div className="mt-1 flex flex-wrap justify-center gap-2">{actions}</div>
        </Centered>
    );
}

/** Over a folder listed from instant search's index: Windows keeps it closed, so it's read-only here, with Get access where that can help. */
function ClosedBar({ tab, info }: { tab: string; info: NoAccess | null }) {
    const [working, setWorking] = useState(false);
    const canGet = !!info && !info.remote && !info.refused;
    return (
        <div className="flex min-h-9 shrink-0 items-center gap-2 border-b bg-amber-500/10 px-3 py-1 text-sm">
            <FolderLock className="size-4 shrink-0 text-amber-700 dark:text-amber-400" />
            <span
                className="min-w-0 flex-1 truncate"
                title="Instant search reads each drive's file table as the system, so it knows what's here; your account still may not open or change any of it."
            >
                Windows keeps this folder {info?.refused ? "to itself" : "closed to you"}: shown read-only from instant search.
            </span>
            {canGet && (
                <Button
                    variant="ghost"
                    size="sm"
                    disabled={working}
                    title="Windows asks for administrator approval"
                    onClick={async () => {
                        setWorking(true);
                        await getAccess(tab);
                        setWorking(false);
                    }}
                >
                    {working ? <Loader2 className="animate-spin" /> : <ShieldCheck />} Get access
                </Button>
            )}
        </div>
    );
}

/** What the active pane shows, for the toolbar and the preview pane, which sit outside the panes. */
interface ActiveView {
    selected: FileEntry[];
    items: number;
    folder: string;
    total?: number;
    /** Windows keeps this account out of the folder. */
    noAccess?: boolean;
    /** The folder is a Git repository's own (it has its .git). */
    repo?: boolean;
}
const useActiveView = create<ActiveView>(() => ({ selected: NO_ENTRIES, items: 0, folder: THIS_PC }));

/** What only a pane can do (it needs the pane's sorted, filtered rows), for the toolbar. */
interface PaneActions {
    startRename: () => void;
    startBatchRename: () => void;
    deleteSelected: (permanent: boolean) => void;
    /** The keyboard's right-click (the menu key, Shift+F10). */
    keyboardMenu: () => void;
}
const paneActions: (PaneActions | undefined)[] = [];
const activePaneActions = () => paneActions[useExplorer.getState().activePane];

interface MenuContext {
    pinned: string[];
    togglePin: (path: string) => void;
    onSearchText: (folder: string) => void;
}

/**
 * For a folder: the one open in a pane (right-click on empty space, `current`), or a place in the
 * sidebar. In an archive (`view.archive`), what would change it is greyed out, as it is in a folder
 * Windows keeps this account out of (`view.noAccess`), where Get access comes first and what needs to
 * read the folder goes, unless the index listed it (`view.fromIndex`).
 */
function folderMenu(
    folder: string,
    current: boolean,
    ctx: MenuContext,
    view?: { find: boolean; selectAll: () => void; archive?: InArchive; noAccess?: NoAccess | null; fromIndex?: boolean; tab?: string; restoreShown?: { label: string; run: () => void } }
): MenuItem[] {
    if (folder === THIS_PC) return [{ label: "Refresh", icon: RotateCw, shortcut: "F5", onSelect: refreshDrives }];
    // A remote's folder: read-only; its connection is changed from its top.
    if (isRemotePath(folder)) {
        const remote = isRoot(folder) ? remoteOfPath(folder) : undefined;
        const pinned = ctx.pinned.includes(folder);
        return [
            !current && { label: "Open", icon: FolderOpen, onSelect: () => navigate(folder) },
            !current && { label: "Open in new tab", icon: SquarePlus, onSelect: () => openInNewTab(folder) },
            current && !!view && { label: "Select all", icon: ListChecks, shortcut: "Ctrl+A", onSelect: view.selectAll },
            current && { label: "Refresh", icon: RotateCw, shortcut: "F5", onSelect: refresh },
            "separator",
            { label: "Copy path", icon: ClipboardCopy, onSelect: () => copyText(folder, "path") },
            !remote && { label: pinned ? "Unpin from sidebar" : "Pin to sidebar", icon: pinned ? PinOff : Pin, onSelect: () => ctx.togglePin(folder) },
            !!remote && "separator",
            !!remote && { label: "Edit the connection…", icon: PlugZap, onSelect: () => editRemote(remote) },
            !!remote && { label: "Remove the connection…", icon: Trash2, onSelect: () => void removeRemote(remote) },
            "separator",
            REMOTE_NOTE,
        ];
    }
    // The Recycle Bin: its items go back, or for good.
    if (isRecycleBin(folder))
        return [
            !current && { label: "Open", icon: FolderOpen, onSelect: () => navigate(folder) },
            !current && { label: "Open in new tab", icon: SquarePlus, onSelect: () => openInNewTab(folder) },
            !!view?.restoreShown && { label: view.restoreShown.label, icon: RotateCcw, onSelect: view.restoreShown.run },
            current && !!view && { label: "Select all", icon: ListChecks, shortcut: "Ctrl+A", onSelect: view.selectAll },
            current && { label: undoLabel(), icon: Undo2, shortcut: "Ctrl+Z", disabled: !nextUndo(), onSelect: () => void undo() },
            current && { label: "Refresh", icon: RotateCw, shortcut: "F5", onSelect: refresh },
            "separator",
            { label: "Empty the Recycle Bin", icon: Trash2, danger: true, onSelect: () => void emptyRecycleBin() },
        ];
    const isPinned = ctx.pinned.includes(folder);
    const find = !!view?.find;
    const archive = view?.archive;
    const noAccess = view?.noAccess;
    const shut = !!noAccess;
    const unlisted = shut && !view?.fromIndex;
    const canGet = !!noAccess && !noAccess.remote && !noAccess.refused;
    // A pinned folder can be inside an archive too.
    const inArchive = !!archive || inArchivePath(folder);
    return [
        canGet && { label: "Get access", icon: ShieldCheck, onSelect: () => void getAccess(view?.tab) },
        canGet && "separator",
        !current && { label: "Open", icon: FolderOpen, onSelect: () => navigate(folder) },
        !current && { label: "Open in new tab", icon: SquarePlus, onSelect: () => openInNewTab(folder) },
        current && { label: "New folder", icon: FolderPlus, shortcut: "Ctrl+Shift+N", disabled: find || !!archive || shut, onSelect: () => newItem(true) },
        current && { label: "New text document", icon: FilePlus, disabled: find || !!archive || shut, onSelect: () => newItem(false) },
        "separator",
        { label: current ? "Paste" : "Paste into folder", icon: ClipboardPaste, shortcut: current ? "Ctrl+V" : undefined, disabled: (current && find) || inArchive || shut, onSelect: () => paste(folder) },
        current && { label: undoLabel(), icon: Undo2, shortcut: "Ctrl+Z", disabled: !nextUndo(), onSelect: () => void undo() },
        current && !!view && { label: "Select all", icon: ListChecks, shortcut: "Ctrl+A", disabled: unlisted, onSelect: view.selectAll },
        current && { label: "Refresh", icon: RotateCw, shortcut: "F5", onSelect: refresh },
        "separator",
        current && { label: "Folder sizes", icon: HardDrive, disabled: find || unlisted, onSelect: calcSizes },
        !inArchive && !shut && { label: "Search text", icon: TextSearch, onSelect: () => ctx.onSearchText(folder) },
        !inArchive && !shut && { label: "Recent changes", icon: FileClock, onSelect: () => inspectTimeline(folder) },
        !inArchive && !shut && { label: "Moved and deleted here", icon: History, onSelect: () => openHistory(folder) },
        !inArchive && !shut && { label: "Scan with Microsoft Defender", icon: ShieldCheck, onSelect: () => void scanWithDefender([folder]) },
        !inArchive && !shut && { label: "Look for passwords and keys", icon: KeyRound, onSelect: () => checkForSecrets(folder) },
        !inArchive && !shut && { label: "Check with YARA rules", icon: ScanSearch, onSelect: () => checkWithYara(folder) },
        !inArchive && !shut && { label: "Find duplicate files", icon: Files, onSelect: () => inspectDuplicates(folder) },
        !inArchive && !shut && { label: "Clean up…", icon: BrushCleaning, onSelect: () => openCleanup(folder) },
        !inArchive && !shut && isFolderPath(folder) && { label: "A rule for this folder…", icon: Wand2, onSelect: () => openRules(folder) },
        !inArchive && !shut && { label: "Export as ncdu JSON…", icon: FileJson, onSelect: () => void exportAsNcdu(folder) },
        !inArchive && !shut && isFolderPath(folder) && { label: current ? "Send this folder to another PC…" : "Send to another PC…", icon: Send, onSelect: () => offerToPC(folder) },
        !inArchive && !shut && isFolderPath(folder) && { label: current ? "Share this folder on this network…" : "Share on this network…", icon: Wifi, onSelect: () => offerOnNetwork(folder) },
        current && !inArchive && !shut && !find && isFolderPath(folder) && { label: "Receive from another PC here…", icon: Download, onSelect: () => openReceive(folder) },
        { label: "Copy path", icon: ClipboardCopy, onSelect: () => copyText(folder, "path") },
        "separator",
        archive
            ? { label: "Show the archive in File Explorer", icon: FolderSearch, onSelect: () => revealArchive(archive) }
            : { label: "Open in File Explorer", icon: FolderSearch, onSelect: () => openPath(folder).catch(report("Couldn't open File Explorer")) },
        !inArchive && !shut && { label: "Open in Terminal", icon: SquareTerminal, onSelect: () => terminal(folder) },
        !inArchive && !shut && useOpeners.getState().list.length > 0 && { label: "Open in", icon: AppWindow, items: openInItems(folder) },
        !inArchive && !shut && isFolderPath(folder) && { label: "Scripts", icon: ScrollText, items: scriptItems(folder, [folder]) },
        { label: isPinned ? "Unpin from sidebar" : "Pin to sidebar", icon: isPinned ? PinOff : Pin, onSelect: () => ctx.togglePin(folder) },
        !inArchive && { label: "What's locking this?", icon: Lock, onSelect: () => inspectLocks([folder]) },
        archive
            ? { label: "Archive properties", icon: Info, onSelect: () => properties(archive.outer) }
            : { label: "Properties", icon: Info, shortcut: current ? "Alt+Enter" : undefined, onSelect: () => properties(folder) },
        (archive || view?.fromIndex) && "separator",
        archive && READ_ONLY_NOTE,
        !!view?.fromIndex && CLOSED_NOTE,
    ];
}

function tabMenu(id: string): MenuItem[] {
    const s = useExplorer.getState();
    const tabs = s.panes.find((p) => p.tabs.includes(id))?.tabs.length ?? 0;
    return [
        { label: "Duplicate tab", icon: Copy, onSelect: () => duplicateTab(id) },
        s.dual && { label: "Move to the other pane", icon: ArrowRightLeft, disabled: tabs < 2, onSelect: () => moveTabToOtherPane(id) },
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
    // zoxide's folders, read again each time the palette opens (it re-renders with them).
    useZoxide((s) => s.folders);
    useScripts((s) => s.list);
    useEffect(() => {
        if (!palette) return;
        void refreshZoxide();
        refreshScripts();
    }, [palette]);
    const showPreviewPane = useStore((s) => s.settings.showPreviewPane);
    const selected = useActiveView((v) => v.selected);
    const pinned = useStore((s) => s.pinned);
    const { setSettings, togglePin } = useStore.getState();
    const menu = useMenu();
    const menuContext: MenuContext = { pinned, togglePin, onSearchText };

    useEffect(() => {
        initExplorer();
        refreshOpeners();
        refreshScripts();
        // Refresh the folders on screen when they change on disk; drives may come and go while we're away,
        // what's deleted elsewhere goes into the Recycle Bin, an editor may have been installed, a
        // script written, and Windows told to let a share's visitors in.
        const unlisten = listen<string>("dir-changed", (event) => {
            reloadFolder(event.payload);
            const scripts = useScripts.getState().folder;
            if (scripts && samePath(event.payload, scripts)) refreshScripts();
        });
        const onFocus = () => {
            refreshDrives();
            reloadRecycleBins();
            refreshOpeners();
            refreshScripts();
            recheckShares();
        };
        window.addEventListener("focus", onFocus);
        return () => {
            unlisten.then((stop) => stop());
            window.removeEventListener("focus", onFocus);
        };
    }, []);

    const activeRef = useRef(active);
    activeRef.current = active;

    // Ctrl+` shows or hides the terminal pane, from the files or from the terminal itself.
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (!activeRef.current || !e.ctrlKey || e.altKey || e.code !== "Backquote" || modalOpen()) return;
            e.preventDefault();
            e.stopPropagation();
            const hiding = useTerminals.getState().open;
            toggleTerminal(terminalFolder());
            // Back to the files, rather than a hidden terminal.
            if (hiding && document.activeElement instanceof HTMLElement && document.activeElement.closest("[data-terminal]")) document.activeElement.blur();
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, []);

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
            // Nothing can be dropped into an archive (it's read-only), nor into a folder Windows keeps closed.
            return tab && tab.path !== THIS_PC && !tab.find && !tab.listing?.archive && !tab.noAccess ? tab.path : null;
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
        if (isFolderPath(folder)) onSearchText(folder);
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
        const dark = document.documentElement.classList.contains("dark");
        // The Recycle Bin is a place, not a folder: what's done to folders isn't done to it.
        const bin = isRecycleBin(t.path) && !t.find;
        const inFolder = isFolderPath(t.path);
        // A remote's folder: read-only, its files downloaded to be read.
        const remote = isRemotePath(t.path);
        const seen = new Set<string>();
        const folders: Command[] = [];
        const addFolder = (folder: string, label: string, icon: Command["icon"], weight: number, iconClass?: string) => {
            const key = folder.toLowerCase();
            if (seen.has(key)) return;
            seen.add(key);
            folders.push({ id: `folder:${key}`, label, detail: folder, icon, iconClass, kind: "folder", weight: weight + Math.log2(1 + usedScore(folder, visits)), run: () => navigate(folder) });
        };
        for (const p of pinned) addFolder(p, baseName(p), Pin, 1.5);
        for (const k of s.known) addFolder(k.path, k.name, KNOWN_ICONS[k.id], 0.5);
        // Used most, here and (with history shared with zoxide) in terminals.
        for (const folder of usedFolders(visits, 60)) addFolder(folder, baseName(folder), Folder, 0, "fill-amber-400/80 text-amber-500");
        for (const d of s.drives) addFolder(d.path, driveName(d), DRIVE_ICONS[d.kind], 0);
        for (const r of useStore.getState().remotes) addFolder(remotePath(r), r.name, r.kind === "s3" ? Cloud : r.kind === "sftp" ? SquareTerminal : Server, 0.5);
        for (const saved of useStore.getState().savedSearches) {
            const where = saved.folder || "every drive";
            folders.push({ id: `search:${where}\n${saved.query}`, label: saved.query, detail: `Saved search in ${where}`, icon: Search, kind: "folder", weight: 1.5, run: () => void runSavedSearch(saved) });
        }

        const action = (id: string, label: string, icon: Command["icon"], run: () => void, shortcut?: string): Command => ({ id, label, icon, run, shortcut, kind: "action" });
        const isPinned = pinned.includes(t.path);
        const tabs = s.panes[s.activePane].tabs.length;
        const other = otherPaneFolder();
        const chosen = t.selection.size > 0;
        const canGrid = inFolder || bin || remote || !!t.find;
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
        const archive = t.listing?.archive;
        const writable = inFolder && !archive;
        const chosenEntries = useActiveView.getState().selected;
        const oneArchive = chosenEntries.length === 1 && isArchiveFile(chosenEntries[0]) && !archive ? chosenEntries[0] : undefined;
        const step = nextUndo();
        // In a folder Windows keeps this account out of, nothing changes and nothing reads it; unless
        // the index listed it, there's nothing to measure either.
        const shut = t.noAccess;
        const unlisted = !!shut && !t.listing;
        const closedItems = chosenEntries.some(isClosed);
        const actions: (Command | false)[] = [
            !!shut && !shut.remote && !shut.refused && action("get-access", "Get access to this folder", ShieldCheck, () => getAccess(t.id)),
            writable && !t.find && !shut && action("new-folder", "New folder", FolderPlus, () => newItem(true), "Ctrl+Shift+N"),
            writable && !t.find && !shut && action("new-file", "New text document", FilePlus, () => newItem(false)),
            action("find", bin ? "Filter what's in the Recycle Bin" : "Find files in this folder", Search, focusFind, "Ctrl+F"),
            bin && chosen && action("restore", "Restore the selection where it was deleted from", RotateCcw, () => void restoreRecycled(chosenEntries)),
            bin && chosen && action("purge", "Delete the selection for good", Trash2, () => void purgeRecycled(chosenEntries)),
            writable && !shut && action("search-text", "Search text in this folder", TextSearch, () => onSearchText(t.path), "Ctrl+Shift+F"),
            inFolder && !t.find && !unlisted && action("sizes", "Calculate folder sizes", HardDrive, calcSizes),
            inFolder && !t.find && !unlisted && action("disk-usage", showsUsage(t.id) ? "Hide disk usage" : "Show disk usage", showsUsage(t.id) ? List : LayoutDashboard, () => toggleUsage(t.id)),
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
            !bin && action("recycle-bin", "Recycle Bin", Trash2, () => navigate(RECYCLE_BIN)),
            action("empty-bin", "Empty the Recycle Bin", Trash2, () => void emptyRecycleBin()),
            action("new-tab", "New tab", Plus, () => newTab(), "Ctrl+T"),
            (tabs > 1 || s.dual) && action("close-tab", "Close tab", X, () => closeTab(), "Ctrl+W"),
            tabs > 1 && action("close-other-tabs", "Close the other tabs", X, () => closeOtherTabs(t.id)),
            action("duplicate-tab", "Duplicate tab", Copy, () => duplicateTab(t.id)),
            s.dual && tabs > 1 && action("move-tab", "Move the tab to the other pane", ArrowRightLeft, () => moveTabToOtherPane(t.id)),
            canReopenTab() && action("reopen-tab", "Reopen closed tab", RotateCcw, reopenClosedTab, "Ctrl+Shift+T"),
            tabs > 1 && action("next-tab", "Next tab", ChevronRight, () => cycleTab(1), "Ctrl+Tab"),
            tabs > 1 && action("previous-tab", "Previous tab", ChevronLeft, () => cycleTab(-1), "Ctrl+Shift+Tab"),
            action("dual", s.dual ? "Close the second pane" : "Dual pane: two folders side by side", Columns2, toggleDual, "Ctrl+Shift+D"),
            action("compare-panes", s.dual && s.comparePanes ? "Stop comparing panes" : "Compare panes: highlight differences between both panes", GitCompareArrows, toggleComparePanes, "Ctrl+Shift+M"),
            s.dual && action("switch-pane", "Switch to the other pane", ArrowRightLeft, switchPane, "F6"),
            !archive && !remote &&
                !closedItems &&
                !bin &&
                chosenEntries.length > 0 &&
                action("compress", "Compress the selection to a zip file", FileArchive, () =>
                    secretsCheckFirst(
                        chosenEntries.map((e) => e.path),
                        "Zip anyway",
                        () => compressEntries(chosenEntries)
                    )
                ),
            !!oneArchive && !closedItems && action("extract", `Extract “${oneArchive.name}” here`, ArchiveRestore, () => extractHere(oneArchive.path)),
            chosenEntries.some(canConvert) && action("convert", "Convert the selected pictures: JPEG, PNG, WebP or AVIF, smaller", ImageDown, () => convertPictures(chosenEntries)),
            !!archive &&
                samePath(t.path, archive.path) &&
                samePath(archive.path, archive.outer) &&
                action("extract-all", SINGLE_FILE.includes(archive.format) ? "Extract the file next to the archive" : "Extract all: everything into a folder next to the archive", ArchiveRestore, () => extractHere(archive.outer)),
            !!other && chosen && !bin && action("copy-other", "Copy the selection to the other pane", CopyPlus, () => copyToOtherPane(false)),
            !!other && chosen && !archive && !remote && !shut && !bin && action("move-other", "Move the selection to the other pane", FolderOutput, () => copyToOtherPane(true)),
            inFolder && action("copy-path", "Copy this folder's path", ClipboardCopy, () => copyText(t.path, "path")),
            action("terminal-pane", useTerminals.getState().open ? "Hide the terminal" : "Terminal here, under the files", SquareTerminal, () => toggleTerminal(terminalFolder()), "Ctrl+`"),
            writable && !shut && action("terminal", "Open in Terminal", SquareTerminal, () => terminal(t.path)),
            // What this folder's projects can run, as the Run menu has them.
            ...(writable && !shut && !t.find
                ? knownTasks(t.path).flatMap((g) => (g.missing ? [] : g.tasks.map((task) => action(`run:${g.folder}:${task.command}`, `Run ${task.command}`, Play, () => runTask(g, task.command)))))
                : []),
            ...(!archive && !remote && !shut && !closedItems && !bin && (inFolder || chosenEntries.length > 0)
                ? useScripts.getState().list.map((script) => {
                      const on = chosenEntries.length > 0 ? chosenEntries.map((e) => e.path) : [t.path];
                      const cwd = inFolder ? t.path : parentPath(on[0]);
                      const detail = script.missing
                          ? "installs what it needs first"
                          : chosenEntries.length > 1
                            ? `on the ${plural(chosenEntries.length, "selected item")}`
                            : `on ${chosenEntries.length ? chosenEntries[0].name : "this folder"}`;
                      return { ...action(`script:${script.path}`, `Script: ${script.name}`, ScrollText, () => runScript(script, cwd, on)), detail };
                  })
                : []),
            action("scripts-folder", "Open the Scripts folder", FolderCog, () => void openScriptsFolder()),
            ...(writable && !shut
                ? useOpeners.getState().list.map((o) =>
                      action(`open-in-${o.id}`, `Open this folder in ${o.name}`, o.terminal ? SquareTerminal : Code, () => void openIn(o.id, t.path).catch(report(`Couldn't open it in ${o.name}`)))
                  )
                : []),
            !!archive && action("explorer", "Show the archive in File Explorer", FileArchive, () => revealArchive(archive)),
            writable && action("explorer", "Open in File Explorer", FolderSearch, () => openPath(t.path).catch(report("Couldn't open File Explorer"))),
            inFolder && action("pin", isPinned ? "Unpin this folder" : "Pin this folder to the sidebar", isPinned ? PinOff : Pin, () => togglePin(t.path)),
            writable && !t.find && !shut && action("batch-rename", chosen ? "Rename the selected items" : "Rename items in this folder", PenLine, () => activePaneActions()?.startBatchRename(), "Ctrl+Shift+R"),
            !archive && !remote && !closedItems && (!!t.focus || chosen) && action("file-xray", "File X-Ray: hashes and hidden streams", Fingerprint, () => inspectXRay(t.focus ?? [...t.selection][0])),
            !archive && !remote &&
                !closedItems &&
                chosenEntries.length === 2 &&
                chosenEntries.every((e) => !e.dir) &&
                action("compare-files", "Compare the two selected files", GitCompareArrows, () => compareFiles(chosenEntries[0].path, chosenEntries[1].path)),
            writable && !shut && action("timeline", "Recent changes in this folder", FileClock, () => inspectTimeline(t.path)),
            action("history", "Moved and deleted: where did a file go?", History, () => openHistory()),
            writable && !shut && action("duplicates", "Find duplicate files in this folder", Files, () => inspectDuplicates(t.path)),
            writable && !shut && action("cleanup", "Clean up this folder: build output and package caches", BrushCleaning, () => openCleanup(t.path)),
            writable && !shut && action("export-ncdu", "Export this folder as ncdu JSON…", FileJson, () => void exportAsNcdu(t.path)),
            !archive && !remote &&
                !closedItems &&
                !bin &&
                !shut &&
                (chosenEntries.length === 1 || (inFolder && !chosen)) &&
                action("send-pc", chosenEntries.length === 1 ? `Send “${chosenEntries[0].name}” to another PC` : "Send this folder to another PC", Send, () =>
                    offerToPC(chosenEntries.length === 1 ? chosenEntries[0].path : t.path)
                ),
            !archive && !remote &&
                !closedItems &&
                !bin &&
                !shut &&
                (chosenEntries.length === 1 || (inFolder && !chosen)) &&
                action("share-network", chosenEntries.length === 1 ? `Share “${chosenEntries[0].name}” on this network: a link for a phone` : "Share this folder on this network: a link for a phone", Wifi, () =>
                    offerOnNetwork(chosenEntries.length === 1 ? chosenEntries[0].path : t.path)
                ),
            writable && !shut && !t.find && action("receive-pc", "Receive from another PC here", Download, () => openReceive(t.path)),
            useShare.getState().sends.length > 0 && action("sending", "Sending to another PC: the tickets, and Stop", Send, () => showShare("send")),
            useShare.getState().serving.length > 0 && action("serving", "Shared on this network: the links, and Stop", Wifi, () => showShare("network")),
            !archive && !remote && (inFolder || chosen) && action("who-locks", chosen ? "What's locking the selection?" : "What's locking this folder?", Lock, () => inspectLocks(chosen ? [...t.selection] : [t.path])),
            !archive && !remote &&
                !closedItems &&
                (inFolder || chosen) &&
                action("defender", chosen ? "Scan the selection with Microsoft Defender" : "Scan this folder with Microsoft Defender", ShieldCheck, () =>
                    scanWithDefender(chosen ? [...t.selection] : [t.path])
                ),
            !archive && !remote && !closedItems && (inFolder || t.selection.size === 1) && action("secrets", "Look for passwords and keys left in files", KeyRound, () => checkForSecrets(t.selection.size === 1 ? [...t.selection][0] : t.path)),
            !archive && !remote && !closedItems && (inFolder || t.selection.size === 1) && action("yara", "Check with YARA rules", ScanSearch, () => checkWithYara(t.selection.size === 1 ? [...t.selection][0] : t.path)),
            !!archive && action("properties", "Archive properties", Info, () => properties(archive.outer)),
            writable && action("properties", "Properties", Info, () => properties(t.path), "Alt+Enter"),
            // From the theme showing, which System leaves to Windows.
            dark
                ? action("theme", "Switch to light theme", Sun, () => setSettings({ theme: "light" }))
                : action("theme", "Switch to dark theme", Moon, () => setSettings({ theme: "dark" })),
            !!step && action("undo", `Undo the ${step.label}`, Undo2, () => undo(), "Ctrl+Z"),
            action("settings", "Settings", SettingsIcon, onOpenSettings, "Ctrl+,"),
            action("shortcuts", "Keyboard shortcuts", Keyboard, openShortcuts, "F1"),
            action("add-remote", "Add a remote connection: an S3 bucket, a WebDAV or an SFTP server", PlugZap, addRemote),
            action("rules", "Folder rules: sort out what comes into a folder, and what they did", Wand2, () => openRules()),
            writable && !shut && !t.find && action("rule-here", "A rule for this folder: move, rename or clear out what comes in", Wand2, () => openRules(t.path)),
        ];
        return [...folders, ...actions.filter((a): a is Command => !!a)];
    };

    return (
        <div className={active ? "flex min-h-0 flex-1" : "hidden"}>
            <Sidebar onContextMenu={onPlaceContextMenu} />
            {/* The files on a layer of their own: the backdrop shows only around them. */}
            <div className="flex min-w-0 flex-1 flex-col bg-background">
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
                <TerminalPanel />
            </div>
            {showPreviewPane && (path !== THIS_PC || find) && <PreviewPane />}
            {menu.element}
            {palette && <CommandPalette commands={paletteCommands()} onGoTo={(text) => goTo(text)} onClose={() => useExplorer.setState({ palette: false })} />}
        </div>
    );
}

/** The selection (or the folder) of the pane the keyboard acts on. Its left edge drags its width. */
function PreviewPane() {
    const { selected, items, folder, total, noAccess, repo } = useActiveView();
    const width = useStore((s) => s.settings.previewWidth);
    const drag = useRef({ from: 0, now: 0, pane: null as HTMLElement | null });
    return (
        // A narrow window narrows it first: the list keeps 16rem beside the sidebar (3rem as a rail, 14rem from md).
        <aside
            className="relative flex bg-background w-[min(28%,28rem)] min-w-[min(16rem,100%_-_19rem)] max-w-[min(60%,100%_-_19rem)] shrink-0 border-l md:min-w-[min(16rem,100%_-_30rem)] md:max-w-[min(60%,100%_-_30rem)]"
            style={width ? { width } : undefined}
        >
            <ResizeHandle
                label="Preview pane width"
                className="-left-1"
                onStart={(bar) => {
                    const pane = bar.parentElement!;
                    drag.current = { from: pane.getBoundingClientRect().width, now: 0, pane };
                }}
                onDrag={(left) => {
                    const d = drag.current;
                    d.now = Math.round(Math.max(256, d.from + left));
                    if (d.pane) d.pane.style.width = `${d.now}px`;
                }}
                onEnd={() => {
                    // What the pane came to, within its limits.
                    const shown = drag.current.pane?.getBoundingClientRect().width;
                    if (drag.current.now && shown) useStore.getState().setSettings({ previewWidth: Math.round(shown) });
                }}
                onReset={() => useStore.getState().setSettings({ previewWidth: null })}
            />
            <FilePreview selected={selected} folder={{ path: folder, items, total, noAccess, repo }} />
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
    const {
        tabId,
        path,
        listing,
        noAccess,
        loading,
        find,
        filter,
        selection,
        focus,
        renaming,
        view,
        cut,
        sizes,
        dropTarget,
        quickLook,
        current,
        dual,
        comparePanes,
        diffOnly,
        otherListing,
        otherPath,
        otherSizes,
        otherWritable,
    } = useExplorer(
        useShallow((s) => {
            const t = s.tabs[(s.panes[index] ?? s.panes[0]).active];
            const otherPane = s.dual && s.comparePanes ? s.panes[1 - index] : undefined;
            const otherTab = otherPane ? s.tabs[otherPane.active] : undefined;
            const otherReady = !!otherTab?.ready && isFolderPath(otherTab.path) && !otherTab.find;
            return {
                tabId: t.id,
                path: t.path,
                listing: t.listing,
                noAccess: t.noAccess,
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
                comparePanes: s.comparePanes,
                diffOnly: s.diffOnly,
                otherListing: otherReady ? otherTab.listing : null,
                otherPath: otherReady ? otherTab.path : null,
                otherSizes: otherReady ? s.sizes[otherTab.path] : undefined,
                otherWritable: otherReady && !otherTab.listing?.archive && !otherTab.noAccess,
            };
        })
    );
    const settings = useStore((s) => s.settings);
    const { sort, foldersFirst, showHidden, editorPath } = settings;
    const { setSettings } = useStore.getState();
    // What Git says of what's in the folder, when it's in a repository.
    const gitMarks = useGit((s) => (find ? undefined : s[tabId]?.view?.marks));
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
    // Sorted by date taken: the photos' and videos' dates are read in the background, and sort as they come.
    // Filtered by width:, taken: and the like (the folder, not search results, which the search checked):
    // what its photos, videos and songs say is read the same way, and they show as it comes.
    const deferredFilter = useDeferredValue(filter);
    const filterReads = !find && hasPropFilter(deferredFilter);
    const factsMap = useFacts((s) => (sort.key === "taken" || filterReads ? s.facts : null));
    const takenOf = useMemo(() => {
        if (!factsMap || sort.key !== "taken") return undefined;
        return (e: FileEntry) => {
            const facts = factsMap.get(takenKey(e));
            return facts === undefined ? undefined : (facts?.taken ?? null);
        };
    }, [factsMap, sort.key]);
    const factsOf = useMemo(() => (factsMap && filterReads ? (e: FileEntry) => factsMap.get(takenKey(e)) : undefined), [factsMap, filterReads]);
    useEffect(() => {
        if (sort.key === "taken") void readTaken(source);
        if (filterReads) void readFacts(source);
    }, [sort.key, source, filterReads]);
    const sorted = useMemo(() => {
        const sizeOf = sizeRun ? (e: FileEntry) => (e.dir ? (sizeRun.sizes.get(e.name) ?? -1) : e.size) : undefined;
        // Listings arrive sorted by name from Rust, so the default view needs no work here; close matches
        // arrive closest first from the index, and keep that order unless another one is chosen.
        const presorted = source === listing?.entries || (!!find?.fuzzy && source === found);
        return sortEntries(source, sort, foldersFirst, presorted, sizeOf, takenOf);
    }, [source, listing, find?.fuzzy, found, sort, foldersFirst, sizeRun, takenOf]);

    // Compared deeper (the compare bar's Subfolders and Contents): Rust's answer for these two folders, once it's in.
    const deep = useDeepCompare();
    const pair = otherPath ? (index === 0 ? [path, otherPath] : [otherPath, path]) : null;
    const deepResult = pair && deep.result && deep.key === deepKey(pair[0], pair[1]) ? deep.result : null;
    const deepStatuses = deepResult ? (index === 0 ? deepResult.left : deepResult.right) : undefined;
    const deepSync = deepResult ? (index === 0 ? deepResult.toRight : deepResult.toLeft) : null;
    // The left pane asks for it, when both show folders on disk.
    const deepOn = settings.compareSubfolders || settings.compareContents;
    const asks = index === 0 && dual && comparePanes && deepOn && !find && isFolderPath(path) && !!listing && !listing.archive && otherWritable && !!otherPath;
    useEffect(() => {
        if (asks) void runDeepCompare(path, otherPath!);
        else if (index === 0 && useDeepCompare.getState().key) stopDeepCompare();
    }, [asks, index, path, otherPath, listing, otherListing, settings.compareSubfolders, settings.compareContents, showHidden, settings.findExclusions]);

    const comparison = useMemo(() => {
        if (!dual || !comparePanes || find || !isFolderPath(path) || !listing || !otherListing) return null;
        const otherMap = new Map<string, FileEntry>();
        for (const e of otherListing.entries) {
            if (showHidden || !isHidden(e)) otherMap.set(e.lower, e);
        }
        const diffs = new Map<string, DiffStatus>();
        let unique = 0;
        let newer = 0;
        let differ = 0;
        let older = 0;
        let same = 0;
        const diffPaths: string[] = [];
        const syncPaths: string[] = [];
        for (const e of sorted) {
            if (!showHidden && isHidden(e)) continue;
            const other = otherMap.get(e.lower);
            const ownDirSize = e.dir ? sizeRun?.sizes.get(e.name) : undefined;
            const otherDirSize = other?.dir ? otherSizes?.sizes.get(other.name) : undefined;
            const status = deepStatuses?.[e.lower] ?? compareEntries(e, other, ownDirSize, otherDirSize);
            diffs.set(e.lower, status);
            if (status === "unique") {
                unique++;
                diffPaths.push(e.path);
                syncPaths.push(e.path);
            } else if (status === "newer") {
                newer++;
                diffPaths.push(e.path);
                syncPaths.push(e.path);
            } else if (status === "differ") {
                differ++;
                diffPaths.push(e.path);
            } else if (status === "older") {
                older++;
                diffPaths.push(e.path);
            } else {
                same++;
            }
        }
        return { diffs, unique, newer, differ, older, same, diffPaths, syncPaths };
    }, [dual, comparePanes, find, path, listing, otherListing, sorted, showHidden, sizeRun, otherSizes, deepStatuses]);

    const findQuery = find?.query;
    const { rows: listRows, hidden } = useMemo(() => {
        // A search's results are what the box asked for: its text narrows them only once it's changed
        // (close matches, a letter or two off what was typed, would all be filtered out by it).
        const narrowing = findQuery !== undefined && deferredFilter.trim() === findQuery.trim() ? "" : deferredFilter;
        const match = matcher(narrowing, gitMarks, factsOf);
        const filterDiffs = !!comparison && diffOnly;
        if (!match && showHidden && !filterDiffs) return { rows: sorted, hidden: 0 };
        let hidden = 0;
        const rows = sorted.filter((e) => {
            if (!showHidden && isHidden(e)) {
                hidden++;
                return false;
            }
            if (filterDiffs && comparison.diffs.get(e.lower) === "same") return false;
            return !match || match(e);
        });
        return { rows, hidden };
    }, [sorted, deferredFilter, findQuery, showHidden, comparison, diffOnly, gitMarks, factsOf]);
    // A tab showing its disk-usage map has the map's items as rows.
    const mapRows = useMapRows(tabId);
    // The folder's README, folded above its list (not over search results, the map, or a closed folder).
    const readme = useMemo(
        () => (settings.showReadme && !find && !mapRows && isFolderPath(path) && listing && !listing.fromIndex ? readmeOf(listing.entries) : undefined),
        [settings.showReadme, find, mapRows, path, listing]
    );
    const rows = mapRows ?? listRows;

    const focusIndex = useMemo(() => (focus ? rows.findIndex((r) => r.path === focus) : -1), [rows, focus]);
    const selected = useMemo(() => (selection.size ? rows.filter((r) => selection.has(r.path)) : NO_ENTRIES), [rows, selection]);
    // Quick Look follows the cursor, so arrowing through the list flips through previews.
    const looked = quickLook && current ? (rows[focusIndex] ?? selected[0]) : undefined;
    useEffect(() => {
        if (quickLook && current && !looked) useExplorer.setState({ quickLook: false });
    }, [quickLook, current, looked]);

    useLayoutEffect(() => markShown(tabId), [tabId, listing]);

    // The Recycle Bin: each item says where it was deleted from; each folder comes with its size.
    const bin = isRecycleBin(path) && !find;
    const binTotal = useMemo(() => (bin && listing ? listing.entries.reduce((sum, e) => sum + e.size, 0) : undefined), [bin, listing]);
    // The toolbar and the preview pane show what the active pane shows.
    const folder = find?.scope ?? path;
    const total = binTotal ?? sizeRun?.total;
    const shut = !!noAccess && !find;
    const repo = !find && !!listing && !listing.archive && listing.entries.some((e) => e.lower === ".git");
    useLayoutEffect(() => {
        if (current) useActiveView.setState({ selected, items: rows.length, folder, total, noAccess: shut, repo });
    }, [current, selected, rows.length, folder, total, shut, repo]);

    const rowsRef = useRef(rows);
    rowsRef.current = rows;
    const typeahead = useRef({ text: "", at: 0 });

    const selectedNow = () => {
        const t = tabState(tabRef.current);
        return t ? rowsRef.current.filter((r) => t.selection.has(r.path)) : NO_ENTRIES;
    };

    const startBatchRename = () => {
        if (tabState(tabRef.current)?.listing?.archive || remoteOf(tabRef.current)) return readOnlyNotice();
        if (tabState(tabRef.current)?.noAccess) return closedNotice(tabRef.current);
        if (recycleBinOf(tabRef.current)) return recycledNotice();
        const chosen = selectedNow();
        openBatchRename(chosen.length ? chosen : rowsRef.current, rowsRef.current);
    };

    const startRename = (target?: string) => {
        const t = tabState(tabRef.current);
        if (t?.listing?.archive || remoteOf(tabRef.current)) return readOnlyNotice();
        if (t?.noAccess) return closedNotice(tabRef.current);
        if (recycleBinOf(tabRef.current)) return recycledNotice();
        if (!target) {
            const chosen = selectedNow();
            if (chosen.length > 1) {
                openBatchRename(chosen, rowsRef.current);
                return;
            }
        }
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
        // In the Recycle Bin, Delete is for good (asked first), as in File Explorer.
        if (recycleBinOf(tabRef.current)) void purgeRecycled(chosen, next?.path);
        else deletePaths([...picked], permanent, next?.path);
    };

    const openSelected = async () => {
        const chosen = selectedNow();
        if (chosen.length === 1) return openEntry(chosen[0]);
        const files = chosen.filter((e) => !e.dir);
        if (!files.length) return chosen[0] && openEntry(chosen[0]);
        if (files.length > 10 && !(await confirm(`Open ${files.length} files?`, { title: "Open files", kind: "warning", okLabel: `Open ${files.length}`, cancelLabel: "Cancel" }))) return;
        files.forEach(openEntry);
    };

    /** Ctrl+Enter: each selected folder (and archive) in a new tab. */
    const openSelectedInTabs = () => {
        for (const entry of selectedNow()) if ((entry.dir || isArchiveFile(entry)) && !isRecycled(entry)) openInNewTab(entry.path);
    };

    useEffect(() => {
        paneActions[index] = { startRename: () => startRename(), startBatchRename, deleteSelected, keyboardMenu };
        return () => {
            paneActions[index] = undefined;
        };
    });

    useEffect(() => {
        // Only the active pane listens; the other one waits for a click (or F6).
        if (!active || !current) return;
        const onKey = (e: KeyboardEvent) => {
            if (e.defaultPrevented || modalOpen()) return;
            const ctrl = e.ctrlKey || e.metaKey;
            const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
            const target = e.target instanceof Element ? e.target : null;
            // A focused button handles its own Enter and Space.
            if ((key === "Enter" || key === " ") && target?.closest("button, a")) return;
            // Fields keep their keys, except that navigation (and tabs) also work from the search box, as in Explorer.
            if (target?.closest("input, textarea, select, [contenteditable=true], dialog, [popover], [data-terminal]")) {
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
            const listKey =
                (to !== null && !e.altKey) ||
                ["Enter", " ", "Delete", "F2", "ContextMenu"].includes(key) ||
                (key === "F10" && e.shiftKey) ||
                (ctrl ? ["a", "c", "x", "v", "z", " ", "Enter"].includes(key) : key.length === 1 && !e.altKey);
            if (listKey && !(quickLook && key === " ") && keyboardElsewhere(target, paneRef.current?.querySelector("[data-file-list], [data-file-grid]"))) return;

            if (quickLook && (key === "Escape" || key === " ")) useExplorer.setState({ quickLook: false });
            else if (key === "F6") {
                if (!dual) return;
                switchPane();
            } else if (key === "ContextMenu" || (key === "F10" && e.shiftKey && !ctrl && !e.altKey)) {
                paneActions[index]?.keyboardMenu();
            } else if (e.altKey && !ctrl) {
                if (key === "ArrowLeft") goBack();
                else if (key === "ArrowRight") goForward();
                else if (key === "ArrowUp") goUp();
                else if (key === "Enter") {
                    // Properties are File Explorer's, for files on disk; the preview pane has an archived item's details.
                    if (s.listing?.archive) toast.message("Properties are for files on disk", { id: "archive-properties", description: "For something inside an archive, the preview pane (Alt+P) shows its details." });
                    else if (isRecycleBin(s.path)) recycledNotice();
                    else properties(s.focus ?? s.path);
                }
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
                else if (key === "m" && e.shiftKey) toggleComparePanes();
                else if (/^[1-9]$/.test(key) && !e.shiftKey) selectTab(Number(key));
                else if (key === "Enter") openSelectedInTabs();
                else if (key === "a") selectAll(rows);
                else if (key === "c" && e.shiftKey) paths.length && copyText(paths.join("\r\n"), paths.length === 1 ? "path" : "paths");
                else if (key === "c") copyToClipboard(paths, false);
                else if (key === "x") copyToClipboard(paths, true);
                else if (key === "v") paste();
                else if (key === "z" && !e.shiftKey) undo();
                else if (key === "n" && e.shiftKey) newItem(true);
                else if (key === "l") useExplorer.setState({ editingAddress: true });
                else if ((key === "f" || key === "e") && !e.shiftKey) focusFind();
                else if (key === "h") setSettings({ showHidden: !useStore.getState().settings.showHidden });
                else if (key === "r" && e.shiftKey) startBatchRename();
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
            // Middle-click: the folder (or archive) in a new tab, and no autoscroll.
            e.preventDefault();
            if ((entry.dir || isArchiveFile(entry)) && !isRecycled(entry)) openInNewTab(entry.path);
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
                const chosen = selectedNow();
                const paths = chosen.map((r) => r.path);
                // Out of the Recycle Bin, an item would land named as Windows keeps it there: Restore it instead.
                if (!paths.length || chosen.some(isRecycled)) return;
                // Out of an archive: extracted first, and dragged as copies.
                if (chosen.some(isArchived)) {
                    dragFromArchive(paths);
                    return;
                }
                // Moved away (e.g. into another drive's folder in Explorer): show what's left.
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
    /** In the Recycle Bin: back where it was deleted from, a look at it, or gone for good. */
    const recycledMenu = (targets: FileEntry[]): MenuItem[] => {
        const one = targets.length === 1 ? targets[0] : null;
        const originals = targets.map((t) => joinPath(t.origin ?? "", t.name));
        return [
            { label: one ? "Restore" : `Restore ${targets.length} items`, icon: RotateCcw, onSelect: () => void restoreRecycled(targets) },
            one && {
                label: "Quick Look",
                icon: Eye,
                shortcut: "Space",
                onSelect: () => {
                    updateTab({ focus: one.path }, tabRef.current);
                    useExplorer.setState({ quickLook: true });
                },
            },
            !!one?.origin && { label: "Open the folder it was deleted from", icon: FolderOpen, onSelect: () => navigate(one.origin!) },
            "separator",
            { label: one ? "Copy original path" : "Copy original paths", icon: ClipboardCopy, onSelect: () => copyText(originals.join("\r\n"), one ? "path" : "paths") },
            "separator",
            { label: "Delete for good", icon: Trash2, shortcut: "Del", danger: true, onSelect: () => void purgeRecycled(targets) },
        ];
    };

    const itemMenu = (targets: FileEntry[]): MenuItem[] => {
        if (targets.some(isRecycled)) return recycledMenu(targets);
        const one = targets.length === 1 ? targets[0] : null;
        const paths = targets.map((t) => t.path);
        // Archives open like folders: in a new tab too.
        const folders = targets.filter((t) => t.dir || isArchiveFile(t));
        const other = dual && !!otherPaneFolder();
        // Inside an archive, nothing changes: what would is greyed out, and opening a file extracts it.
        const archived = targets.some(isArchived) ? archiveOf(tabRef.current) : undefined;
        // Listed from the index (Windows keeps their folder closed): nothing changes, and only folders open.
        const closed = targets.some(isClosed);
        // On a remote: read-only, and downloaded to be opened or copied, as an archive's are extracted.
        const remote = targets.some(isRemote);
        const fixed = !!archived || closed || remote;
        const browses = !!one && (one.dir || isArchiveFile(one));
        const pictures = targets.filter(canConvert);
        // Two files side by side: the two chosen, or (comparing panes) one and its namesake in the other pane.
        const pair = targets.length === 2 && !fixed && targets.every((t) => !t.dir) ? targets : null;
        const status = one && !one.dir && !fixed && otherPath ? comparison?.diffs.get(one.lower) : undefined;
        const namesake = status && status !== "unique" && otherPath && one ? joinPath(otherPath, one.name) : null;
        // Open, then the clipboard and the other pane, archives, renaming and deleting, tools, properties.
        return [
            { label: browses ? "Open" : targets.length > 1 ? `Open ${targets.length} items` : "Open", icon: browses ? (one!.dir ? FolderOpen : FileArchive) : ExternalLink, shortcut: "Enter", onSelect: openSelected },
            // A .docx or .jar is a zip too: Enter opens it in its app, this looks inside.
            one && isBrowsable(one) && !closed && { label: "Open as archive", icon: FileArchive, onSelect: () => navigate(one.path) },
            one && (isArchiveFile(one) || isBrowsable(one)) && !fixed && { label: "Extract here", icon: ArchiveRestore, onSelect: () => extractHere(one.path) },
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
            one && isArchiveFile(one) && !closed && {
                label: "Open with its app",
                icon: AppWindow,
                onSelect: () => (archived ? openArchivedWith(one, openItem) : openFile(one.path)),
            },
            one && !one.dir && !closed && !!editorName && {
                label: `Open with ${editorName}`,
                icon: Code,
                onSelect: () => (archived || remote ? openArchivedWith(one, (p) => openPath(p, editorPath.trim())) : openPath(one.path, editorPath.trim()).catch(report("Couldn't open the file"))),
            },
            one && !one.dir && !closed && {
                label: "Open with…",
                icon: AppWindow,
                onSelect: () => (archived || remote ? openArchivedWith(one, openWith) : openWith(one.path).catch(report("Couldn't open the app picker"))),
            },
            one?.dir && !fixed && { label: "Open in Terminal", icon: SquareTerminal, onSelect: () => terminal(one.path) },
            one && !fixed && useOpeners.getState().list.length > 0 && { label: "Open in", icon: AppWindow, items: openInItems(one.path) },
            !fixed && { label: "Scripts", icon: ScrollText, items: scriptItems(isFolderPath(path) ? path : parentPath(paths[0]), paths) },
            find && one && { label: "Open file location", icon: FolderInput, onSelect: () => revealInApp(one.path) },
            !remote &&
                (archived
                    ? { label: "Show the archive in File Explorer", icon: FolderSearch, onSelect: () => revealArchive(archived) }
                    : { label: "Show in File Explorer", icon: FolderSearch, onSelect: () => revealItemInDir(paths).catch(report("Couldn't open File Explorer")) }),
            "separator",
            { label: "Cut", icon: Scissors, shortcut: "Ctrl+X", disabled: fixed, onSelect: () => copyToClipboard(paths, true) },
            { label: "Copy", icon: Copy, shortcut: "Ctrl+C", onSelect: () => copyToClipboard(paths, false) },
            one?.dir && { label: "Paste into folder", icon: ClipboardPaste, disabled: fixed, onSelect: () => paste(one.path) },
            { label: paths.length === 1 ? "Copy path" : "Copy paths", icon: ClipboardCopy, shortcut: "Ctrl+Shift+C", onSelect: () => copyText(paths.join("\r\n"), paths.length === 1 ? "path" : "paths") },
            dual && { label: "Copy to the other pane", icon: CopyPlus, disabled: !other, onSelect: () => copyToOtherPane(false, paths) },
            dual && { label: "Move to the other pane", icon: FolderOutput, disabled: !other || fixed, onSelect: () => copyToOtherPane(true, paths) },
            "separator",
            !fixed && { label: "Compress to zip file", icon: FileArchive, onSelect: () => void secretsCheckFirst(paths, "Zip anyway", () => compressEntries(targets)) },
            one && !fixed && { label: "Send to another PC…", icon: Send, onSelect: () => offerToPC(one.path) },
            one && !fixed && { label: "Share on this network…", icon: Wifi, onSelect: () => offerOnNetwork(one.path) },
            pictures.length > 0 && { label: pictures.length > 1 ? `Convert ${pictures.length} pictures…` : "Convert picture…", icon: ImageDown, onSelect: () => convertPictures(pictures) },
            "separator",
            one
                ? { label: "Rename", icon: PenLine, shortcut: "F2", disabled: fixed, onSelect: () => startRename(one.path) }
                : { label: `Rename ${targets.length} items…`, icon: PenLine, shortcut: "F2", disabled: fixed, onSelect: () => openBatchRename(targets, rowsRef.current) },
            one && !find && !fixed && { label: "Rename items in this folder…", icon: PenLine, shortcut: "Ctrl+Shift+R", onSelect: () => openBatchRename(rowsRef.current, rowsRef.current) },
            { label: "Delete", icon: Trash2, shortcut: "Del", danger: !fixed, disabled: fixed, onSelect: () => deleteSelected(false) },
            "separator",
            one?.dir && { label: pinned.includes(one.path) ? "Unpin from sidebar" : "Pin to sidebar", icon: pinned.includes(one.path) ? PinOff : Pin, onSelect: () => togglePin(one.path) },
            one?.dir && !fixed && { label: "Search text", icon: TextSearch, onSelect: () => onSearchText(one.path) },
            one?.dir && !fixed && { label: "Recent changes", icon: FileClock, onSelect: () => inspectTimeline(one.path) },
            one?.dir && !fixed && { label: "Find duplicate files", icon: Files, onSelect: () => inspectDuplicates(one.path) },
            one?.dir && !fixed && { label: "Clean up…", icon: BrushCleaning, onSelect: () => openCleanup(one.path) },
            one?.dir && !fixed && { label: "Export as ncdu JSON…", icon: FileJson, onSelect: () => void exportAsNcdu(one.path) },
            pair && { label: "Compare the two files", icon: GitCompareArrows, onSelect: () => compareFiles(pair[0].path, pair[1].path) },
            !!namesake && one && { label: "Compare with the other pane's", icon: GitCompareArrows, onSelect: () => (index === 0 ? compareFiles(one.path, namesake) : compareFiles(namesake, one.path)) },
            one && !one.dir && !fixed && { label: "File X-Ray", icon: Fingerprint, onSelect: () => inspectXRay(one.path) },
            !archived && !remote && { label: "What's locking this?", icon: Lock, onSelect: () => inspectLocks(paths) },
            !fixed && { label: "Scan with Microsoft Defender", icon: ShieldCheck, onSelect: () => void scanWithDefender(paths) },
            one && !fixed && { label: "Look for passwords and keys", icon: KeyRound, onSelect: () => checkForSecrets(one.path) },
            one && !fixed && { label: "Check with YARA rules", icon: ScanSearch, onSelect: () => checkWithYara(one.path) },
            "separator",
            one && !archived && !remote && { label: "Properties", icon: Info, shortcut: "Alt+Enter", onSelect: () => properties(one.path) },
            !!archived && READ_ONLY_NOTE,
            remote && REMOTE_NOTE,
            closed && CLOSED_NOTE,
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

    // In the Recycle Bin, everything it shows can go back at once (what's typed in the box narrows it).
    const restoreShown =
        bin && rows.length > 0 ? { label: rows.length === listing?.entries.length ? "Restore all items" : `Restore the ${formatCount(rows.length)} items shown`, run: () => void restoreRecycled(rowsRef.current) } : undefined;
    const backgroundMenu = () =>
        folderMenu(path, true, menuContext, { find: !!find, selectAll: () => selectAll(rowsRef.current), archive: listing?.archive, noAccess, fromIndex: !!listing?.fromIndex, tab: tabId, restoreShown });

    /**
     * The keyboard's right-click (the menu key, Shift+F10): what's selected gets its menu, at the item
     * with the focus, else the folder gets its own, as in Explorer.
     */
    const keyboardMenu = () => {
        const list = paneRef.current?.querySelector<HTMLElement>("[data-file-list], [data-file-grid]");
        const focused = document.getElementById(list?.getAttribute("aria-activedescendant") ?? "")?.getBoundingClientRect();
        const box = focused ?? list?.getBoundingClientRect();
        const at = box ? { clientX: box.left + 24, clientY: focused ? box.bottom : box.top + 8 } : { clientX: innerWidth / 2, clientY: innerHeight / 3 };
        const chosen = selectedNow();
        menu.open(at, chosen.length ? itemMenu(chosen) : backgroundMenu());
    };

    /** Right-click where the open folder shows but none of its items does (or the keyboard's, on the list). */
    const onFolderContextMenu = (e: React.MouseEvent) => {
        e.preventDefault();
        if (e.button !== 2) return keyboardMenu();
        selectOnly(null);
        menu.open(e, backgroundMenu());
    };

    const onSort = (key: SortKey) => setSettings({ sort: { key, desc: sort.key === key ? !sort.desc : key === "modified" || key === "taken" || key === "size" } });
    // The header's own menu: what to sort by, Date taken among them (read from photos and videos).
    const onHeaderMenu = (e: React.MouseEvent) => {
        e.preventDefault();
        const labels: [SortKey, string][] = [
            ["name", "Name"],
            ["modified", "Date modified"],
            ["taken", "Date taken (photos and videos)"],
            ["type", "Type"],
            ["size", "Size"],
        ];
        menu.open(
            e,
            labels.map(([key, label]) => ({ label, toggle: true, checked: sort.key === key, onSelect: () => sort.key !== key && onSort(key) }))
        );
    };

    let empty: React.ReactNode = null;
    if (find?.error) {
        empty = (
            <Centered>
                <AlertTriangle className="size-6 text-destructive" />
                <p className="font-medium text-foreground">The search failed</p>
                <pre className="max-w-full select-text whitespace-pre-wrap font-mono text-xs">{find.error}</pre>
                <Button variant="outline" size="sm" onClick={() => runFind(find.query, { scope: find.scope })}>
                    <RotateCw /> Try again
                </Button>
            </Centered>
        );
    } else if (find?.running && !rows.length) {
        empty = (
            <Centered>
                <Loader2 className="size-5 animate-spin text-primary" /> Searching…
            </Centered>
        );
    } else if (find && !rows.length) {
        // Each thing the search left out, with a button to look there too.
        empty = (
            <Centered>
                <SearchX className="size-6" />
                <p className="font-medium text-foreground">Nothing named like “{find.query}”</p>
                <p className="max-w-sm">
                    {find.skipped.length ? `Skipped: ${find.skipped.join(", ")}. ` : ""}
                    {showHidden ? "" : "Hidden files weren't searched. "}
                    Filters work too: ext:pdf, size:&gt;10mb, dm:7d.
                </p>
                <div className="flex flex-wrap justify-center gap-2">
                    {!showHidden && (
                        <Button
                            variant="outline"
                            size="sm"
                            onClick={() => {
                                setSettings({ showHidden: true });
                                void runFind(find.query, { scope: find.scope });
                            }}
                        >
                            <Eye /> Include hidden files
                        </Button>
                    )}
                    {find.skipped.length > 0 && (
                        <Button variant="outline" size="sm" onClick={() => runFind(find.query, { scope: find.scope, skipNothing: true })}>
                            <FolderSearch /> Search skipped folders too
                        </Button>
                    )}
                    {find.scope !== THIS_PC && (
                        <Button variant="outline" size="sm" onClick={() => runFind(find.query, { scope: THIS_PC })}>
                            <HardDrive /> Search every drive
                        </Button>
                    )}
                </div>
            </Centered>
        );
    } else if (noAccess && !listing) {
        empty = <NoAccessView tab={tabId} path={path} info={noAccess} />;
    } else if (listing && !rows.length) {
        empty = filter ? (
            <Centered>
                <SearchX className="size-6" />
                <p>Nothing in {bin ? "the Recycle Bin" : "this folder"} matches “{filter}”.</p>
                {!bin && (
                    <Button variant="outline" size="sm" onClick={() => runFind(filter)}>
                        <Search /> Search subfolders too
                    </Button>
                )}
            </Centered>
        ) : bin && !listing.entries.length ? (
            <Centered>
                <Trash2 className="size-8 stroke-[1.25] text-muted-foreground/60" />
                <p className="font-medium text-foreground">The Recycle Bin is empty</p>
                <p className="text-xs">What you delete waits here, to be restored or deleted for good.</p>
            </Centered>
        ) : comparison && diffOnly && comparison.same > 0 ? (
            <Centered>
                <GitCompareArrows className="size-6 text-emerald-500" />
                <p className="font-medium text-foreground">No differences in this pane</p>
                <p>All {formatCount(comparison.same)} items match the other pane.</p>
                <Button variant="outline" size="sm" onClick={toggleDiffOnly}>
                    Show identical items
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
            <Centered>
                <FolderOpen className="size-8 stroke-[1.25] text-muted-foreground/60" />
                <p className="font-medium text-foreground">{listing.archive && samePath(listing.archive.path, path) ? "This archive is empty" : "This folder is empty"}</p>
                {!listing.archive && <p className="text-xs">Drop files here, or paste them with Ctrl+V.</p>}
            </Centered>
        );
    }

    const scopeName = find && (find.scope === THIS_PC ? "all drives" : baseName(find.scope));
    const toggleSavedSearch = useStore((s) => s.toggleSavedSearch);
    const searchSaved = useStore((s) => !!find && s.savedSearches.some((x) => x.query === find.query && x.folder.toLowerCase() === find.scope.toLowerCase()));
    const listKey = viewKey({ path, find });
    // Each view keeps its own place in the folder: a list's scroll position means nothing to the grid.
    const placeKey = `${listKey}:${view}`;
    const viewProps = {
        rows,
        search: !!find,
        recycled: bin,
        scope: find?.scope ?? path,
        sizes: sizeRun,
        diffs: comparison?.diffs,
        git: gitMarks,
        sort,
        onSort,
        onHeaderMenu,
        takenOf,
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

    const selectDiffs = () => {
        if (!comparison?.diffPaths.length) return;
        const first = comparison.diffPaths[0];
        updateTab({ selection: new Set(comparison.diffPaths), focus: first, anchor: first }, tabRef.current);
    };

    // Deeper, what's missing or newer inside subfolders too, each into its folder.
    const syncCount = deepSync ? deepSync.length : (comparison?.syncPaths.length ?? 0);
    const syncToOther = () => {
        focusPane(index);
        if (deepSync) void syncInto(deepSync);
        else if (comparison?.syncPaths.length) void copyToOtherPane(false, comparison.syncPaths);
    };
    /** How the panes compare, in a menu so the bar stays one line in a narrow pane. */
    const compareOptions = (e: React.MouseEvent<HTMLButtonElement>) => {
        const below = e.currentTarget.getBoundingClientRect();
        const skipping = settings.findExclusions.length ? ` (skipping ${settings.findExclusions.join(", ")})` : "";
        menu.open({ clientX: below.left, clientY: below.bottom + 4 }, [
            { label: `Subfolders too${skipping}`, toggle: true, checked: settings.compareSubfolders, onSelect: () => setSettings({ compareSubfolders: !settings.compareSubfolders }) },
            { label: "File contents, not only sizes and dates", toggle: true, checked: settings.compareContents, onSelect: () => setSettings({ compareContents: !settings.compareContents }) },
            "separator",
            { label: "Only the differences", toggle: true, checked: diffOnly, onSelect: toggleDiffOnly },
        ]);
    };
    const comparingMore = settings.compareSubfolders || settings.compareContents || diffOnly;

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
                    <span className="truncate" title={find.fuzzy ? "Nothing is named as typed: these names are a letter or two off" : undefined}>
                        {find.fuzzy ? "Close matches" : "Results"} for <b className="font-semibold">“{find.query}”</b> in {scopeName}
                        {find.scope !== THIS_PC && " and its subfolders"}
                    </span>
                    {find.limited && (
                        <span className="flex shrink-0 items-center gap-1.5 rounded-md bg-amber-500/10 py-0.5 pl-2 pr-0.5 text-xs font-medium text-amber-800 dark:text-amber-400">
                            Stopped at {formatCount(settings.findMaxResults ?? 0)}
                            <button
                                type="button"
                                onClick={() => runFind(find.query, { scope: find.scope, unlimited: true })}
                                title="Search again without the limit (Settings → Find files)"
                                className="rounded-[4px] px-1.5 hover:bg-amber-500/15"
                            >
                                Show all
                            </button>
                        </span>
                    )}
                    <div className="ml-auto flex shrink-0 gap-1">
                        <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => toggleSavedSearch({ query: find.query, folder: find.scope })}
                            title={searchSaved ? "Take this search out of the sidebar" : "Keep this search in the sidebar, to run it again with one click"}
                        >
                            {searchSaved ? <BookmarkCheck className="text-primary" /> : <Bookmark />} {searchSaved ? "Saved" : "Save"}
                        </Button>
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
            {listing?.fromIndex && !find && <ClosedBar tab={tabId} info={noAccess} />}
            {dual && comparePanes && !find && isFolderPath(path) && (
                <div className="flex min-h-8 shrink-0 flex-wrap items-center gap-1.5 border-b bg-primary/5 px-2.5 py-1 text-xs">
                    <GitCompareArrows className="size-3.5 shrink-0 text-primary" />
                    {comparison ? (
                        <>
                            {comparison.unique > 0 && (
                                <span className="shrink-0 rounded-[4px] bg-emerald-500/15 px-1.5 py-0.5 font-medium text-emerald-700 dark:text-emerald-300">
                                    {formatCount(comparison.unique)} only here
                                </span>
                            )}
                            {comparison.newer > 0 && (
                                <span className="shrink-0 rounded-[4px] bg-sky-500/15 px-1.5 py-0.5 font-medium text-sky-700 dark:text-sky-300">
                                    {formatCount(comparison.newer)} newer
                                </span>
                            )}
                            {comparison.differ > 0 && (
                                <span className="shrink-0 rounded-[4px] bg-amber-500/15 px-1.5 py-0.5 font-medium text-amber-700 dark:text-amber-300">
                                    {formatCount(comparison.differ)} different
                                </span>
                            )}
                            {comparison.older > 0 && (
                                <span className="shrink-0 rounded-[4px] bg-muted px-1.5 py-0.5 font-medium text-muted-foreground">
                                    {formatCount(comparison.older)} older
                                </span>
                            )}
                            {comparison.diffPaths.length === 0 && (
                                <span className="shrink-0 font-medium text-emerald-700 dark:text-emerald-400">All match</span>
                            )}
                            <span className="shrink-0 text-muted-foreground">· {formatCount(comparison.same)} same</span>
                            {deep.running && <Loader2 className="size-3 shrink-0 animate-spin text-primary" aria-label="Comparing deeper" />}
                            {deep.error && (
                                <span className="min-w-0 max-w-full truncate text-destructive" title={deep.error}>
                                    Couldn't compare deeper: {deep.error}
                                </span>
                            )}
                            <div className="ml-auto flex shrink-0 items-center gap-1">
                                <button
                                    type="button"
                                    aria-haspopup="menu"
                                    onClick={compareOptions}
                                    title="Compare subfolders too, file contents, or show only the differences"
                                    aria-label="Compare options"
                                    className={cn(
                                        "relative grid size-5 place-items-center rounded-[4px] transition-colors hover:bg-accent hover:text-foreground",
                                        comparingMore ? "text-primary" : "text-muted-foreground"
                                    )}
                                >
                                    <SlidersHorizontal className="size-3.5" />
                                    {comparingMore && <span className="absolute -right-0.5 -top-0.5 size-1.5 rounded-full bg-primary" />}
                                </button>
                                <button
                                    type="button"
                                    disabled={!comparison.diffPaths.length}
                                    onClick={selectDiffs}
                                    title="Select all items that differ in this pane"
                                    className="rounded-[4px] px-1.5 py-0.5 font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-40"
                                >
                                    Select differences ({formatCount(comparison.diffPaths.length)})
                                </button>
                                <button
                                    type="button"
                                    disabled={!otherWritable || !syncCount}
                                    onClick={syncToOther}
                                    title={deepSync ? "Copy what's missing or newer here, in subfolders too, to the other pane" : "Copy unique and newer items from this pane to the other pane"}
                                    className="rounded-[4px] bg-primary/15 px-1.5 py-0.5 font-medium text-primary transition-colors hover:bg-primary/25 disabled:pointer-events-none disabled:opacity-40"
                                >
                                    {index === 0 ? `Sync ${formatCount(syncCount)} →` : `← Sync ${formatCount(syncCount)}`}
                                </button>
                                <button
                                    type="button"
                                    onClick={toggleComparePanes}
                                    title="Stop comparing panes (Ctrl+Shift+M)"
                                    aria-label="Stop comparing panes"
                                    className="grid size-5 place-items-center rounded-[4px] text-muted-foreground hover:bg-accent hover:text-foreground"
                                >
                                    <X className="size-3.5" />
                                </button>
                            </div>
                        </>
                    ) : (
                        <>
                            <span className="truncate text-muted-foreground">Open a folder in the other pane to compare</span>
                            <button
                                type="button"
                                onClick={toggleComparePanes}
                                title="Stop comparing panes (Ctrl+Shift+M)"
                                aria-label="Stop comparing panes"
                                className="ml-auto grid size-5 shrink-0 place-items-center rounded-[4px] text-muted-foreground hover:bg-accent hover:text-foreground"
                            >
                                <X className="size-3.5" />
                            </button>
                        </>
                    )}
                </div>
            )}
            {readme && <ReadmeCard key={readme.path} entry={readme} />}
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
