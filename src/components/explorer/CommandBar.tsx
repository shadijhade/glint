import { useLayoutEffect, useRef, useState } from "react";
import {
    ArchiveRestore,
    BrushCleaning,
    ChevronDown,
    ClipboardPaste,
    Columns2,
    Command,
    Copy,
    CopyPlus,
    Download,
    Ellipsis,
    Eye,
    EyeOff,
    FileClock,
    Files,
    Fingerprint,
    FolderOutput,
    FolderLock,
    FolderPlus,
    GitCompareArrows,
    HardDrive,
    History,
    ImageDown,
    KeyRound,
    LayoutDashboard,
    LayoutGrid,
    List,
    Loader2,
    Lock,
    type LucideIcon,
    PanelRight,
    PenLine,
    PencilOff,
    Play,
    RotateCcw,
    ScanSearch,
    Scissors,
    Send,
    ShieldCheck,
    SlidersHorizontal,
    SquareTerminal,
    TextSearch,
    Trash2,
    Wand2,
    Wifi,
    Wrench,
    ZoomIn,
    ZoomOut,
} from "lucide-react";
import { toast } from "sonner";
import { useShallow } from "zustand/react/shallow";
import { scanWithDefender } from "@/lib/defender";
import { chooseView, toggleUsage, useUsage } from "@/lib/diskUsage";
import type { FileEntry } from "@/lib/entries";
import {
    calcSizes,
    copyToClipboard,
    copyToOtherPane,
    emptyRecycleBin,
    extractHere,
    newItem,
    paste,
    restoreRecycled,
    stopSizes,
    toggleComparePanes,
    toggleDual,
    useExplorer,
    useTab,
    type ViewMode,
} from "@/lib/explorer";
import { openTerminal } from "@/lib/fs";
import { TILE_SIZES, type TileSize, zoomTiles } from "@/lib/grid";
import { isFolderPath, isRecycleBin, isRemotePath, samePath } from "@/lib/paths";
import { taskMenu, useProjectTasks } from "@/lib/projects";
import { useStore } from "@/lib/store";
import { toggleTerminal } from "@/lib/terminal";
import { cn } from "@/lib/utils";
import { useMenu } from "../Menu";
import { Button } from "../ui/button";
import { checkForSecrets, checkWithYara } from "./CheckDialog";
import { newMenuItems } from "./newMenu";
import { canConvert, convertPictures } from "./ConvertDialog";
import { openCleanup } from "./CleanupDialog";
import { offerOnNetwork, offerToPC } from "./ShareDialog";
import { openReceive } from "@/lib/share";
import { openRules } from "./RulesDialog";
import { inspectDuplicates } from "./DuplicatesDialog";
import { openHistory } from "./HistoryDialog";
import { inspectLocks } from "./LocksDialog";
import { terminalFolder } from "./TerminalPanel";
import { inspectTimeline } from "./TimelineDialog";
import { inspectXRay } from "./XRayDialog";

function Tool({ icon: Icon, label, pressed, className, ...props }: { icon: LucideIcon; label: string; pressed?: boolean } & React.ComponentProps<"button">) {
    return (
        <button
            type="button"
            title={label}
            aria-label={label}
            aria-pressed={pressed}
            className={cn(
                "grid size-8 shrink-0 place-items-center rounded-md text-muted-foreground transition-all duration-100 hover:bg-accent hover:text-foreground active:scale-93 disabled:pointer-events-none disabled:opacity-40 [&_svg]:size-4",
                pressed && "bg-primary/15 text-primary hover:bg-primary/20 hover:text-primary",
                className
            )}
            {...props}
        >
            <Icon />
        </button>
    );
}

const Divider = () => <span className="mx-1 h-5 w-px shrink-0 bg-border" />;

const VIEWS: { view: ViewMode; icon: LucideIcon; label: string }[] = [
    { view: "list", icon: List, label: "List (Ctrl+Shift+6)" },
    { view: "grid", icon: LayoutGrid, label: "Grid, with thumbnails (Ctrl+Shift+2)" },
];

/**
 * List or grid, for the tab on screen; while it shows its disk-usage map, neither is pressed and both go
 * back to the files. The arrow after them opens the grid's tile sizes.
 */
function ViewSwitch({ view, mapped, disabled, tileSize, onChange, onSizes }: { view: ViewMode; mapped: boolean; disabled: boolean; tileSize: TileSize; onChange: (view: ViewMode) => void; onSizes: (e: React.MouseEvent<HTMLButtonElement>) => void }) {
    const sizes = `Tile size: ${TILE_SIZES.find((t) => t.size === tileSize)?.label ?? "Medium"} (Ctrl+wheel, Ctrl+= and Ctrl+- in the grid)`;
    return (
        <div role="group" aria-label="View" className="relative flex shrink-0 rounded-md bg-foreground/[0.05] p-0.5">
            {!mapped && (
                <span
                    aria-hidden
                    className={cn(
                        "pointer-events-none absolute bottom-0.5 left-0.5 top-0.5 w-8 rounded-[5px] bg-background shadow-xs transition-transform duration-150 ease-out dark:bg-foreground/[0.12]",
                        view === "grid" && "translate-x-full",
                        disabled && "opacity-40"
                    )}
                />
            )}
            {VIEWS.map(({ view: v, icon: Icon, label }) => (
                <button
                    key={v}
                    type="button"
                    title={label}
                    aria-label={label}
                    aria-pressed={!mapped && view === v}
                    disabled={disabled}
                    onClick={() => onChange(v)}
                    className={cn(
                        "relative z-10 grid h-7 w-8 place-items-center rounded-[5px] text-muted-foreground transition-all duration-100 hover:text-foreground active:scale-93 disabled:pointer-events-none disabled:opacity-40 [&_svg]:size-4",
                        !mapped && view === v && "text-primary hover:text-primary"
                    )}
                >
                    <Icon />
                </button>
            ))}
            <button
                type="button"
                title={sizes}
                aria-label={sizes}
                aria-haspopup="menu"
                disabled={disabled}
                onClick={onSizes}
                className="relative z-10 grid h-7 w-5 place-items-center rounded-[5px] text-muted-foreground transition-all duration-100 hover:text-foreground active:scale-93 disabled:pointer-events-none disabled:opacity-40 [&_svg]:size-3.5"
            >
                <ChevronDown />
            </button>
        </div>
    );
}

const NEW_TITLE = "New: a folder, a text document, or a kind of file your apps make (a Word document, a zip…)";

/** The most the bar folds: see `useFold`. */
const MOST = 6;

/**
 * How far the bar folds to fit: 0 shows every label; 1 hides the `data-fold="1"` ones (icons and
 * tooltips stay); 2 hides them all; 3 also puts the view toggles in a menu; 4 puts Disk usage,
 * Folder sizes and Search text in the Tools menu too; 5 puts list or grid in the view menu and the
 * palette in Tools; 6, a window half a small screen wide, puts cut, copy, paste, rename and delete
 * in a "…" menu. Measured, so it holds at any text size; `buttons` changes when the set of buttons
 * does, so the widths are measured again.
 */
function useFold(buttons: unknown) {
    const ref = useRef<HTMLDivElement>(null);
    const [fold, setFold] = useState(0);
    // What the bar needs at each fold: 0 and 1 measured at 0, the others at that fold when even it
    // overflows (0 until then: as far as anyone knows, it fits).
    const needs = useRef<number[]>(Array(MOST).fill(0));
    const measuredFor = useRef(buttons);
    useLayoutEffect(() => {
        const bar = ref.current!;
        if (measuredFor.current !== buttons) {
            measuredFor.current = buttons;
            needs.current = Array(MOST).fill(0);
        }
        const check = () => {
            if (fold === 0) {
                // Every label on screen: what the bar needs with them, and without the first to go.
                const first = [...bar.querySelectorAll<HTMLElement>("[data-fold='1']")].reduce(
                    (sum, label) => sum + label.offsetWidth + (parseFloat(getComputedStyle(label.parentElement!).columnGap) || 0),
                    0
                );
                needs.current = [bar.scrollWidth, bar.scrollWidth - first, ...Array(MOST - 2).fill(0)];
            } else if (fold >= 2 && fold < MOST && bar.scrollWidth > bar.clientWidth) needs.current[fold] = bar.scrollWidth;
            const fits = needs.current.findIndex((need) => bar.clientWidth >= need);
            setFold(fits < 0 ? MOST : fits);
        };
        check();
        const observer = new ResizeObserver(check);
        observer.observe(bar);
        return () => observer.disconnect();
    }, [fold, buttons]);
    return [ref, fold] as const;
}

interface CommandBarProps {
    selected: FileEntry[];
    onRename: () => void;
    onDelete: () => void;
    onSearchText: () => void;
}

/** Acts on the active pane (its tab and selection). */
export function CommandBar({ selected, onRename, onDelete, onSearchText }: CommandBarProps) {
    const { path, find, tab, view, readOnly, closed, bin, wholeArchive } = useTab(
        useShallow((t) => {
            const archive = t.listing?.archive;
            const bin = isRecycleBin(t.path) && !t.find;
            return {
                path: t.path,
                find: !!t.find,
                tab: t.id,
                view: t.view,
                readOnly: !!archive || !!t.noAccess || bin || isRemotePath(t.path),
                // The Recycle Bin: its items go back where they were deleted from, or for good.
                bin,
                // Windows keeps the folder closed to this account: nothing in it opens or changes.
                closed: !!t.noAccess,
                // At the top of an archive on disk (not one inside another): it can be extracted whole.
                wholeArchive: archive && samePath(t.path, archive.path) && samePath(archive.path, archive.outer) ? archive.outer : undefined,
            };
        })
    );
    const mapped = useUsage((s) => !!s[tab]);
    const { sizing, dual, comparePanes, other } = useExplorer(
        useShallow((s) => {
            const pane = s.dual ? s.panes[1 - s.activePane] : undefined;
            const t = pane && s.tabs[pane.active];
            // An archive in the other pane is read-only, and Windows may keep its folder closed: nothing goes into either.
            return {
                sizing: !!s.sizes[path]?.running,
                dual: s.dual,
                comparePanes: s.comparePanes,
                other: !!t?.ready && isFolderPath(t.path) && !t.find && !t.listing?.archive && !t.noAccess,
            };
        })
    );
    const { showHidden, showPreviewPane, tileSize } = useStore(
        useShallow((s) => ({ showHidden: s.settings.showHidden, showPreviewPane: s.settings.showPreviewPane, tileSize: s.settings.tileSize }))
    );
    const setSettings = useStore((s) => s.setSettings);
    const inFolder = isFolderPath(path);
    const paths = selected.map((e) => e.path);
    const terminal = () => openTerminal(path).catch((e) => toast.error("Couldn't open a terminal", { description: String(e) }));
    // What this folder's projects can run (its own, and those above it to the repository's root).
    const tasks = useProjectTasks(inFolder && !readOnly && !find ? path : null);
    const [ref, fold] = useFold(`${dual} ${readOnly} ${bin} ${tasks.length > 0}`);
    const menu = useMenu();

    /** New, under the arrow by New folder: a folder, a text document, and what Windows' New menu makes. */
    const showNew = (e: React.MouseEvent<HTMLButtonElement>) => {
        const below = e.currentTarget.getBoundingClientRect();
        menu.open({ clientX: below.left, clientY: below.bottom + 4 }, newMenuItems(true));
    };

    const showTasks = (e: React.MouseEvent<HTMLButtonElement>) => {
        const below = e.currentTarget.getBoundingClientRect();
        menu.open({ clientX: below.left, clientY: below.bottom + 4 }, taskMenu(tasks));
    };

    /** The grid's tile sizes, under the arrow: choosing one shows the grid at it. */
    const showSizes = (e: React.MouseEvent<HTMLButtonElement>) => {
        const below = e.currentTarget.getBoundingClientRect();
        const pick = (size: TileSize) => {
            setSettings({ tileSize: size });
            chooseView("grid", tab);
        };
        const bigger = zoomTiles(tileSize, 1);
        const smaller = zoomTiles(tileSize, -1);
        menu.open({ clientX: below.left, clientY: below.bottom + 4 }, [
            { label: "Larger tiles", icon: ZoomIn, shortcut: "Ctrl+=", disabled: bigger === tileSize, onSelect: () => pick(bigger) },
            { label: "Smaller tiles", icon: ZoomOut, shortcut: "Ctrl+-", disabled: smaller === tileSize, onSelect: () => pick(smaller) },
            "separator",
            ...TILE_SIZES.map(({ size, label }) => ({ label, checked: size === tileSize, shortcut: size === "medium" ? "Ctrl+0" : undefined, onSelect: () => pick(size) })),
        ]);
    };

    /** The tools for this folder or the selection, in one menu so the bar fits beside the preview pane. */
    const showTools = (e: React.MouseEvent<HTMLButtonElement>) => {
        const below = e.currentTarget.getBoundingClientRect();
        const file = selected.length === 1 && !selected[0].dir ? selected[0] : undefined;
        menu.open({ clientX: below.left, clientY: below.bottom + 4 }, [
            ...(fold >= 5 ? [{ label: "Command palette", icon: Command, shortcut: "Ctrl+K", onSelect: () => useExplorer.setState({ palette: true }) }, "separator" as const] : []),
            // Folded in from the bar, when it's that narrow.
            ...(fold >= 4
                ? [
                      { label: mapped ? "Back to the file list" : "Disk usage", icon: LayoutDashboard, disabled: !inFolder || find, onSelect: () => toggleUsage(tab) },
                      { label: sizing ? "Stop calculating folder sizes" : "Folder sizes", icon: HardDrive, disabled: !inFolder || find, onSelect: sizing ? stopSizes : calcSizes },
                      { label: "Search text in this folder", icon: TextSearch, shortcut: "Ctrl+Shift+F", disabled: !inFolder || readOnly, onSelect: onSearchText },
                      "separator" as const,
                  ]
                : []),
            { label: "Terminal here, under the files", icon: SquareTerminal, shortcut: "Ctrl+`", onSelect: () => toggleTerminal(terminalFolder()) },
            { label: "Open in Terminal", icon: SquareTerminal, disabled: !inFolder, onSelect: terminal },
            { heading: "Inspect" },
            { label: "File X-Ray: hashes and hidden streams", icon: Fingerprint, disabled: !file, onSelect: () => file && inspectXRay(file.path) },
            { label: paths.length ? "What's locking the selection?" : "What's locking this folder?", icon: Lock, disabled: !inFolder && !paths.length, onSelect: () => inspectLocks(paths.length ? paths : [path]) },
            { label: "Recent changes in this folder", icon: FileClock, disabled: !inFolder, onSelect: () => inspectTimeline(path) },
            { label: "Moved and deleted: where did a file go?", icon: History, onSelect: () => openHistory() },
            { heading: "Clean up" },
            { label: "Find duplicate files in this folder", icon: Files, disabled: !inFolder, onSelect: () => inspectDuplicates(path) },
            { label: "Clean up this folder…", icon: BrushCleaning, disabled: !inFolder || readOnly, onSelect: () => openCleanup(path) },
            { label: "Convert the selected pictures…", icon: ImageDown, disabled: !selected.some(canConvert), onSelect: () => convertPictures(selected) },
            { label: "Folder rules: sort out what comes in…", icon: Wand2, onSelect: () => openRules(inFolder && !readOnly ? path : undefined) },
            { heading: "Share" },
            {
                label: paths.length === 1 ? "Send the selection to another PC…" : "Send this folder to another PC…",
                icon: Send,
                disabled: readOnly || bin || paths.length > 1 || (!inFolder && paths.length !== 1),
                onSelect: () => offerToPC(paths.length === 1 ? paths[0] : path),
            },
            {
                label: paths.length === 1 ? "Share the selection on this network…" : "Share this folder on this network…",
                icon: Wifi,
                disabled: readOnly || bin || paths.length > 1 || (!inFolder && paths.length !== 1),
                onSelect: () => offerOnNetwork(paths.length === 1 ? paths[0] : path),
            },
            { label: "Receive from another PC here…", icon: Download, disabled: !inFolder || readOnly || find, onSelect: () => openReceive(path) },
            { heading: "Check" },
            {
                label: paths.length ? "Scan the selection with Microsoft Defender" : "Scan this folder with Microsoft Defender",
                icon: ShieldCheck,
                disabled: readOnly || (!inFolder && !paths.length),
                onSelect: () => void scanWithDefender(paths.length ? paths : [path]),
            },
            {
                label: "Look for passwords and keys left in files",
                icon: KeyRound,
                disabled: readOnly || (!inFolder && paths.length !== 1),
                onSelect: () => checkForSecrets(paths.length === 1 ? paths[0] : path),
            },
            { label: "Check with YARA rules", icon: ScanSearch, disabled: readOnly || (!inFolder && paths.length !== 1), onSelect: () => checkWithYara(paths.length === 1 ? paths[0] : path) },
        ]);
    };

    /** When even the icons don't fit, the view toggles in a menu (and narrower still, list or grid). */
    const showToggles = (e: React.MouseEvent<HTMLButtonElement>) => {
        const below = e.currentTarget.getBoundingClientRect();
        const canView = inFolder || find || bin || isRemotePath(path);
        menu.open({ clientX: below.left, clientY: below.bottom + 4 }, [
            ...(fold >= 5
                ? [
                      { label: "List", checked: !mapped && view === "list", disabled: !canView, shortcut: "Ctrl+Shift+6", onSelect: () => chooseView("list", tab) },
                      { label: "Grid, with thumbnails", checked: !mapped && view === "grid", disabled: !canView, shortcut: "Ctrl+Shift+2", onSelect: () => chooseView("grid", tab) },
                      "separator" as const,
                  ]
                : []),
            { label: "Show hidden items", toggle: true, checked: showHidden, shortcut: "Ctrl+H", onSelect: () => setSettings({ showHidden: !showHidden }) },
            { label: "Preview pane", toggle: true, checked: showPreviewPane, shortcut: "Alt+P", onSelect: () => setSettings({ showPreviewPane: !showPreviewPane }) },
            { label: "Dual pane", toggle: true, checked: dual, shortcut: "Ctrl+Shift+D", onSelect: toggleDual },
            { label: "Compare panes", toggle: true, checked: dual && comparePanes, shortcut: "Ctrl+Shift+M", onSelect: toggleComparePanes },
        ]);
    };

    /** Narrowest: what's done to the selection, in a menu by New folder. */
    const showEdits = (e: React.MouseEvent<HTMLButtonElement>) => {
        const below = e.currentTarget.getBoundingClientRect();
        menu.open({ clientX: below.left, clientY: below.bottom + 4 }, [
            { label: "Cut", icon: Scissors, shortcut: "Ctrl+X", disabled: !paths.length || readOnly, onSelect: () => copyToClipboard(paths, true) },
            { label: "Copy", icon: Copy, shortcut: "Ctrl+C", disabled: !paths.length, onSelect: () => copyToClipboard(paths, false) },
            { label: "Paste", icon: ClipboardPaste, shortcut: "Ctrl+V", disabled: !inFolder || find || readOnly, onSelect: () => paste() },
            dual && { label: "Copy to the other pane", icon: CopyPlus, disabled: !paths.length || !other, onSelect: () => copyToOtherPane(false, paths) },
            dual && { label: "Move to the other pane", icon: FolderOutput, disabled: !paths.length || !other || readOnly, onSelect: () => copyToOtherPane(true, paths) },
            "separator",
            { label: selected.length > 1 ? `Rename ${selected.length} items` : "Rename", icon: PenLine, shortcut: "F2", disabled: !selected.length || readOnly, onSelect: onRename },
            { label: "Delete", icon: Trash2, shortcut: "Del", danger: true, disabled: !paths.length || readOnly, onSelect: onDelete },
        ]);
    };

    return (
        <div ref={ref} className="flex h-10 shrink-0 items-center gap-0.5 overflow-hidden border-b bg-chrome px-2">
            {bin ? (
                <>
                    <Button variant="ghost" size="sm" onClick={() => void restoreRecycled(selected)} disabled={!selected.length} title="Restore: the selection back into the folder each was deleted from">
                        <RotateCcw /> <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>Restore</span>
                    </Button>
                    <Tool icon={Trash2} label="Delete for good (Del)" disabled={!paths.length} onClick={onDelete} />
                    <Divider />
                    <Button variant="ghost" size="sm" onClick={() => void emptyRecycleBin()} title="Empty the Recycle Bin: everything in it, on every drive, deleted for good (Windows asks first)">
                        <Trash2 /> <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>Empty the Recycle Bin</span>
                    </Button>
                </>
            ) : (
                <>
                    <Button variant="ghost" size="sm" onClick={() => newItem(true)} disabled={!inFolder || find || readOnly} title="New folder (Ctrl+Shift+N)">
                        <FolderPlus /> <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>New folder</span>
                    </Button>
                    <button
                        type="button"
                        title={NEW_TITLE}
                        aria-label={NEW_TITLE}
                        aria-haspopup="menu"
                        disabled={!inFolder || find || readOnly}
                        onClick={showNew}
                        className="-ml-1 grid h-7 w-5 shrink-0 place-items-center rounded-[5px] text-muted-foreground transition-all duration-100 hover:bg-accent hover:text-foreground active:scale-93 disabled:pointer-events-none disabled:opacity-40 [&_svg]:size-3.5"
                    >
                        <ChevronDown />
                    </button>
                    {fold < MOST ? (
                        <>
                            <Divider />
                            <Tool icon={Scissors} label="Cut (Ctrl+X)" disabled={!paths.length || readOnly} onClick={() => copyToClipboard(paths, true)} />
                            <Tool icon={Copy} label={readOnly && !closed ? "Copy (Ctrl+C): extracted, to paste anywhere" : "Copy (Ctrl+C)"} disabled={!paths.length} onClick={() => copyToClipboard(paths, false)} />
                            <Tool icon={ClipboardPaste} label="Paste (Ctrl+V)" disabled={!inFolder || find || readOnly} onClick={() => paste()} />
                            {dual && (
                                <>
                                    <Tool icon={CopyPlus} label="Copy to the other pane" disabled={!paths.length || !other} onClick={() => copyToOtherPane(false, paths)} />
                                    <Tool icon={FolderOutput} label="Move to the other pane" disabled={!paths.length || !other || readOnly} onClick={() => copyToOtherPane(true, paths)} />
                                </>
                            )}
                            <Tool
                                icon={PenLine}
                                label={selected.length > 1 ? `Rename ${selected.length} items (F2)` : "Rename (F2) · Ctrl+Shift+R renames many"}
                                disabled={!selected.length || readOnly}
                                onClick={onRename}
                            />
                            <Tool icon={Trash2} label="Delete (Del) · Shift+Del deletes permanently" disabled={!paths.length || readOnly} onClick={onDelete} />
                        </>
                    ) : (
                        <Tool icon={Ellipsis} label="Cut, copy, paste, rename, delete" aria-haspopup="menu" onClick={showEdits} />
                    )}
                    <Divider />
                    {/* In an archive, or a folder Windows keeps closed, where the tools would be (they're for folders on disk you may read): why the rest is greyed out. */}
                    {readOnly ? (
                        <>
                            <span
                                title={
                                    closed
                                        ? "Windows keeps this folder closed to your account: nothing in it can be opened or changed here. Get access from the folder itself, or its right-click menu."
                                        : "Glint opens archives read-only. Copy (Ctrl+C), copy to the other pane or drag items to get them out; to change what's in the archive, copy it out first."
                                }
                                className="flex h-7 shrink-0 items-center gap-1 rounded-md bg-amber-500/10 px-2 text-xs font-medium text-amber-800 dark:text-amber-400"
                            >
                                {closed ? <FolderLock className="size-3.5" /> : <PencilOff className="size-3.5" />}
                                <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>
                                    {closed ? "No access" : "Read-only archive"}
                                </span>
                            </span>
                            {wholeArchive && (
                                <Button variant="ghost" size="sm" onClick={() => extractHere(wholeArchive)} title="Extract all: everything into a folder next to the archive">
                                    <ArchiveRestore /> <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>Extract all</span>
                                </Button>
                            )}
                        </>
                    ) : (
                        <Button variant="ghost" size="sm" aria-haspopup="menu" onClick={showTools} title="Tools: Terminal, File X-Ray, what's locking, recent changes, duplicate files">
                            <Wrench /> <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>Tools</span>
                            <ChevronDown className="-ml-1 size-3.5! opacity-70" />
                        </Button>
                    )}
                    {tasks.length > 0 && (
                        <Button variant="ghost" size="sm" aria-haspopup="menu" onClick={showTasks} title="Run: this project's scripts, recipes and commands, in the terminal under the files">
                            <Play /> <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>Run</span>
                            <ChevronDown className="-ml-1 size-3.5! opacity-70" />
                        </Button>
                    )}
                </>
            )}

            <div className="ml-auto flex shrink-0 items-center gap-0.5">
                {fold < 5 && <ViewSwitch view={view} mapped={mapped && !find} disabled={!inFolder && !find && !bin} tileSize={tileSize} onChange={(v) => chooseView(v, tab)} onSizes={showSizes} />}
                {fold < 4 && (
                <>
                <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => toggleUsage(tab)}
                    disabled={!inFolder || find}
                    aria-pressed={mapped}
                    title={mapped ? "Back to the file list" : "Disk usage: a map of what fills this folder, from the instant search index when it has it, otherwise measured with pdu"}
                    className={cn(mapped && "bg-primary/15 text-primary hover:bg-primary/20 hover:text-primary")}
                >
                    <LayoutDashboard /> <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>Disk usage</span>
                </Button>
                <Button
                    variant="ghost"
                    size="sm"
                    onClick={sizing ? stopSizes : calcSizes}
                    disabled={!inFolder || find}
                    title={sizing ? "Stop calculating" : "Size every folder here: from the instant search index when it has them, otherwise measured with pdu"}
                >
                    {sizing ? <Loader2 className="animate-spin" /> : <HardDrive />} <span data-fold="2" className={cn(fold >= 2 && "sr-only")}>{sizing ? "Stop" : "Folder sizes"}</span>
                </Button>
                <Button variant="ghost" size="sm" onClick={onSearchText} disabled={!inFolder || readOnly} title="Search inside the files in this folder (ripgrep) · Ctrl+Shift+F">
                    <TextSearch /> <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>Search text</span>
                </Button>
                </>
                )}
                {fold < 5 && (
                    <>
                        <Tool icon={Command} label="Command palette (Ctrl+K)" onClick={() => useExplorer.setState({ palette: true })} />
                        <Divider />
                    </>
                )}
                {fold < 3 ? (
                    <>
                        <Tool icon={showHidden ? Eye : EyeOff} label="Show hidden items (Ctrl+H)" pressed={showHidden} onClick={() => setSettings({ showHidden: !showHidden })} />
                        <Tool icon={PanelRight} label="Preview pane (Alt+P)" pressed={showPreviewPane} onClick={() => setSettings({ showPreviewPane: !showPreviewPane })} />
                        <Tool icon={Columns2} label="Dual pane: two folders side by side (Ctrl+Shift+D)" pressed={dual} onClick={toggleDual} />
                        <Tool
                            icon={GitCompareArrows}
                            label="Compare panes: highlight differences between both panes (Ctrl+Shift+M)"
                            pressed={dual && comparePanes}
                            onClick={toggleComparePanes}
                        />
                    </>
                ) : (
                    <Button variant="ghost" size="sm" aria-haspopup="menu" onClick={showToggles} title={fold >= 5 ? "View: list or grid, hidden items, preview pane, dual pane, compare panes" : "View: hidden items, preview pane, dual pane, compare panes"}>
                        <SlidersHorizontal />
                        <ChevronDown className="-ml-1 size-3.5! opacity-70" />
                    </Button>
                )}
            </div>
            {menu.element}
        </div>
    );
}
