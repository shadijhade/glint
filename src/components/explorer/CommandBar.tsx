import { useLayoutEffect, useRef, useState } from "react";
import {
    ChevronDown,
    ClipboardPaste,
    Columns2,
    Command,
    Copy,
    CopyPlus,
    Eye,
    EyeOff,
    Files,
    Fingerprint,
    FolderOutput,
    FolderPlus,
    GitCompareArrows,
    HardDrive,
    History,
    LayoutDashboard,
    LayoutGrid,
    List,
    Loader2,
    Lock,
    type LucideIcon,
    PanelRight,
    PenLine,
    Scissors,
    SquareTerminal,
    TextSearch,
    Trash2,
    ZoomIn,
    ZoomOut,
} from "lucide-react";
import { toast } from "sonner";
import { useShallow } from "zustand/react/shallow";
import { chooseView, toggleUsage, useUsage } from "@/lib/diskUsage";
import type { FileEntry } from "@/lib/entries";
import { calcSizes, copyToClipboard, copyToOtherPane, newItem, paste, stopSizes, toggleComparePanes, toggleDual, useExplorer, useTab, type ViewMode } from "@/lib/explorer";
import { openTerminal } from "@/lib/fs";
import { TILE_SIZES, type TileSize, zoomTiles } from "@/lib/grid";
import { THIS_PC } from "@/lib/paths";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { useMenu } from "../Menu";
import { Button } from "../ui/button";
import { inspectDuplicates } from "./DuplicatesDialog";
import { inspectLocks } from "./LocksDialog";
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
                "grid size-8 shrink-0 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-35 [&_svg]:size-4",
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
        <div role="group" aria-label="View" className="flex shrink-0 rounded-md bg-foreground/[0.05] p-0.5">
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
                        "grid h-7 w-8 place-items-center rounded-[5px] text-muted-foreground transition-colors hover:text-foreground disabled:pointer-events-none disabled:opacity-35 [&_svg]:size-4",
                        !mapped && view === v && "bg-background text-primary shadow-xs hover:text-primary dark:bg-foreground/[0.12]"
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
                className="grid h-7 w-5 place-items-center rounded-[5px] text-muted-foreground transition-colors hover:text-foreground disabled:pointer-events-none disabled:opacity-35 [&_svg]:size-3.5"
            >
                <ChevronDown />
            </button>
        </div>
    );
}

/**
 * How many of the text buttons' labels must go (icons and tooltips stay) for the bar to fit: none,
 * the `data-fold="1"` ones, or all of them. Measured, so it holds at any text size; `buttons`
 * changes when the set of buttons does, so the widths are measured again.
 */
function useFold(buttons: unknown) {
    const ref = useRef<HTMLDivElement>(null);
    const [fold, setFold] = useState(0);
    const needs = useRef([0, 0]);
    const measuredFor = useRef(buttons);
    useLayoutEffect(() => {
        const bar = ref.current!;
        if (measuredFor.current !== buttons) {
            measuredFor.current = buttons;
            needs.current = [0, 0];
        }
        const check = () => {
            if (fold === 0) {
                // Every label on screen: what the bar needs with them, and without the first to go.
                const first = [...bar.querySelectorAll<HTMLElement>("[data-fold='1']")].reduce(
                    (sum, label) => sum + label.offsetWidth + (parseFloat(getComputedStyle(label.parentElement!).columnGap) || 0),
                    0
                );
                needs.current = [bar.scrollWidth, bar.scrollWidth - first];
            }
            const [all, some] = needs.current;
            setFold(bar.clientWidth >= all ? 0 : bar.clientWidth >= some ? 1 : 2);
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
    const { path, find, tab, view, readOnly } = useTab(useShallow((t) => ({ path: t.path, find: !!t.find, tab: t.id, view: t.view, readOnly: !!t.listing?.archive })));
    const mapped = useUsage((s) => !!s[tab]);
    const { sizing, dual, comparePanes, other } = useExplorer(
        useShallow((s) => {
            const pane = s.dual ? s.panes[1 - s.activePane] : undefined;
            const t = pane && s.tabs[pane.active];
            // An archive in the other pane is read-only: nothing goes into it.
            return {
                sizing: !!s.sizes[path]?.running,
                dual: s.dual,
                comparePanes: s.comparePanes,
                other: !!t?.ready && t.path !== THIS_PC && !t.find && !t.listing?.archive,
            };
        })
    );
    const { showHidden, showPreviewPane, tileSize } = useStore(
        useShallow((s) => ({ showHidden: s.settings.showHidden, showPreviewPane: s.settings.showPreviewPane, tileSize: s.settings.tileSize }))
    );
    const setSettings = useStore((s) => s.setSettings);
    const inFolder = path !== THIS_PC;
    const paths = selected.map((e) => e.path);
    const terminal = () => openTerminal(path).catch((e) => toast.error("Couldn't open a terminal", { description: String(e) }));
    const [ref, fold] = useFold(`${dual} ${readOnly}`);
    const menu = useMenu();

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

    return (
        <div ref={ref} className="flex h-10 shrink-0 items-center gap-0.5 overflow-hidden border-b bg-chrome px-2">
            <Button variant="ghost" size="sm" onClick={() => newItem(true)} disabled={!inFolder || find || readOnly} title="New folder (Ctrl+Shift+N)">
                <FolderPlus /> <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>New folder</span>
            </Button>
            <Divider />
            <Tool icon={Scissors} label="Cut (Ctrl+X)" disabled={!paths.length || readOnly} onClick={() => copyToClipboard(paths, true)} />
            <Tool icon={Copy} label={readOnly ? "Copy (Ctrl+C): extracted, to paste anywhere" : "Copy (Ctrl+C)"} disabled={!paths.length} onClick={() => copyToClipboard(paths, false)} />
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
            <Divider />
            {/* In an archive, where Open in Terminal would be (there's no folder on disk to open it in): why the rest is greyed out. */}
            {readOnly ? (
                <span
                    title="Glint opens zip and 7z archives read-only. Copy (Ctrl+C), copy to the other pane or drag items to get them out; to change what's in the archive, copy it out first."
                    className="flex h-7 shrink-0 items-center gap-1 rounded-md bg-amber-500/10 px-2 text-xs font-medium text-amber-700 dark:text-amber-400"
                >
                    <Lock className="size-3.5" />
                    <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>
                        Read-only archive
                    </span>
                </span>
            ) : (
                <>
                    <Tool icon={SquareTerminal} label="Open in Terminal" disabled={!inFolder} onClick={terminal} />
                    <Tool
                        icon={Fingerprint}
                        label="File X-Ray: hashes and hidden streams"
                        disabled={selected.length !== 1 || selected[0].dir}
                        onClick={() => selected[0] && inspectXRay(selected[0].path)}
                    />
                    <Tool
                        icon={Lock}
                        label={paths.length ? "What's locking the selection?" : "What's locking this folder?"}
                        disabled={!inFolder && !paths.length}
                        onClick={() => inspectLocks(paths.length ? paths : [path])}
                    />
                    <Tool icon={History} label="Recent changes in this folder" disabled={!inFolder} onClick={() => inspectTimeline(path)} />
                    <Tool icon={Files} label="Find duplicate files" disabled={!inFolder} onClick={() => inspectDuplicates(path)} />
                </>
            )}

            <div className="ml-auto flex shrink-0 items-center gap-0.5">
                <ViewSwitch view={view} mapped={mapped && !find} disabled={!inFolder && !find} tileSize={tileSize} onChange={(v) => chooseView(v, tab)} onSizes={showSizes} />
                <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => toggleUsage(tab)}
                    disabled={!inFolder || find}
                    aria-pressed={mapped}
                    title={mapped ? "Back to the file list" : "Disk usage: a map of what fills this folder, from the instant search index when it has it, otherwise measured with dust"}
                    className={cn("hidden md:inline-flex", mapped && "bg-primary/15 text-primary hover:bg-primary/20 hover:text-primary")}
                >
                    <LayoutDashboard /> <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>Disk usage</span>
                </Button>
                <Button
                    variant="ghost"
                    size="sm"
                    onClick={sizing ? stopSizes : calcSizes}
                    disabled={!inFolder || find}
                    title={sizing ? "Stop calculating" : "Size every folder here: from the instant search index when it has them, otherwise measured with dust"}
                    className="hidden sm:inline-flex"
                >
                    {sizing ? <Loader2 className="animate-spin" /> : <HardDrive />} <span data-fold="2" className={cn(fold >= 2 && "sr-only")}>{sizing ? "Stop" : "Folder sizes"}</span>
                </Button>
                <Button variant="ghost" size="sm" onClick={onSearchText} disabled={!inFolder || readOnly} title="Search inside the files in this folder (ripgrep) · Ctrl+Shift+F" className="hidden lg:inline-flex">
                    <TextSearch /> <span data-fold="1" className={cn(fold >= 1 && "sr-only")}>Search text</span>
                </Button>
                <Divider />
                <Tool icon={showHidden ? Eye : EyeOff} label="Show hidden items (Ctrl+H)" pressed={showHidden} onClick={() => setSettings({ showHidden: !showHidden })} />
                <Tool icon={PanelRight} label="Preview pane (Alt+P)" pressed={showPreviewPane} onClick={() => setSettings({ showPreviewPane: !showPreviewPane })} />
                <Tool icon={Columns2} label="Dual pane: two folders side by side (Ctrl+Shift+D)" pressed={dual} onClick={toggleDual} />
                <Tool
                    icon={GitCompareArrows}
                    label="Compare panes: highlight differences between both panes (Ctrl+Shift+M)"
                    pressed={dual && comparePanes}
                    onClick={toggleComparePanes}
                />
                <Tool icon={Command} label="Command palette (Ctrl+K)" onClick={() => useExplorer.setState({ palette: true })} />
            </div>
            {menu.element}
        </div>
    );
}
