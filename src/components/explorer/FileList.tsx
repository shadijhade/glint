import { useLayoutEffect, useMemo, useRef } from "react";
import { toast } from "sonner";
import { ChevronDown, ChevronUp, Cloud, FileX, KeyRound } from "lucide-react";
import { ATTR, type DiffStatus, type FileEntry, isArchived, isHidden, isLocked, isOnlineOnly, type Sort, type SortKey } from "@/lib/entries";
import { INVALID_NAME_HINT, type SizeRun } from "@/lib/explorer";
import { isPictureOrVideo, typeLabel } from "@/lib/fileTypes";
import { formatDate, formatFullDate, formatSize } from "@/lib/format";
import { parentPath } from "@/lib/paths";
import { type Column, useStore } from "@/lib/store";
import { timeThumbnails } from "@/lib/thumbs";
import { cn } from "@/lib/utils";
import { ResizeHandle } from "../ResizeHandle";
import { VirtualList } from "../VirtualList";
import { Thumbnail } from "./Thumbnail";

export const DIFF_BADGE: Record<Exclude<DiffStatus, "same">, { label: string; title: string; className: string }> = {
    unique: { label: "Only here", title: "Does not exist in the other pane", className: "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300" },
    newer: { label: "Newer", title: "Modified more recently than in the other pane", className: "bg-sky-500/15 text-sky-700 dark:text-sky-300" },
    differ: { label: "Differs", title: "Different size from the item in the other pane", className: "bg-amber-500/15 text-amber-700 dark:text-amber-300" },
    older: { label: "Older", title: "Modified earlier than in the other pane", className: "bg-muted text-muted-foreground" },
};

/** What the list and the grid both show, and how they report what's done to them. */
export interface FileViewProps {
    rows: FileEntry[];
    /** Search results: each one shows where it is, relative to `scope`. */
    search: boolean;
    scope: string;
    sizes?: SizeRun;
    /** Lowercased name → comparison status against the other pane, when Compare Panes is active. */
    diffs?: ReadonlyMap<string, DiffStatus>;
    sort: Sort;
    onSort: (key: SortKey) => void;
    selection: ReadonlySet<string>;
    focusIndex: number;
    renaming: string | null;
    cut: ReadonlySet<string>;
    /** Folder that files dragged in from outside would land in. */
    dropTarget: string | null;
    /** In the pane the keyboard isn't acting on: the selection shows muted, as in an unfocused window. */
    inactive?: boolean;
    /** The tab shown, which thumbnails are asked for in, and what its thumbnails are timed for: the
     * folder or search, in this view (`viewKey` and the view, as the status bar reads it). */
    lane: string;
    thumbScope: string;
    /** Where to start scrolled to (where this view was left), and where it's scrolled since: pixels in the
     * list, a place (`placeOf`) in the grid, whose layout may change meanwhile. */
    scrollTop?: number;
    onScrollTop?: (top: number) => void;
    onRowMouseDown: (e: React.MouseEvent, index: number) => void;
    onRowContextMenu: (e: React.MouseEvent, index: number) => void;
    onOpen: (entry: FileEntry) => void;
    onBackgroundMouseDown: (e: React.MouseEvent) => void;
    onBackgroundContextMenu: (e: React.MouseEvent) => void;
    onRename: (entry: FileEntry, name: string) => void;
    onCancelRename: () => void;
    empty: React.ReactNode;
}

interface FileListProps extends FileViewProps {
    rowHeight: number;
    /** Pictures and videos show a small thumbnail in place of their icon, `iconBox` CSS pixels square. */
    thumbnails: boolean;
    iconBox: number;
}

export function RenameInput({ entry, onCommit, onCancel, className }: { entry: FileEntry; onCommit: (name: string) => void; onCancel: () => void; className?: string }) {
    const ref = useRef<HTMLInputElement>(null);
    const done = useRef(false);
    useLayoutEffect(() => {
        const input = ref.current!;
        input.focus();
        // Like Explorer: select the name but not the extension.
        const dot = entry.dir ? -1 : entry.name.lastIndexOf(".");
        input.setSelectionRange(0, dot > 0 ? dot : entry.name.length);
    }, [entry]);
    const finish = (commit: boolean) => {
        if (done.current) return;
        done.current = true;
        if (commit) onCommit(ref.current!.value);
        else onCancel();
    };
    return (
        <input
            ref={ref}
            defaultValue={entry.name}
            spellCheck={false}
            aria-label="New name"
            onMouseDown={(e) => e.stopPropagation()}
            onDoubleClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Enter") finish(true);
                else if (e.key === "Escape") finish(false);
                else if (/^[\\/:*?"<>|]$/.test(e.key)) {
                    e.preventDefault();
                    toast.error(INVALID_NAME_HINT, { id: "invalid-name" });
                }
            }}
            onBlur={() => finish(true)}
            className={cn("h-6 min-w-0 flex-1 rounded-sm border border-ring bg-background px-1 text-sm outline-hidden", className)}
        />
    );
}

export function Header({ sortKey, sort, onSort, column, className, children }: { sortKey: SortKey; sort: Sort; onSort: (key: SortKey) => void; column?: Column; className?: string; children: React.ReactNode }) {
    const active = sort.key === sortKey;
    const Arrow = sort.desc ? ChevronDown : ChevronUp;
    return (
        <button
            onClick={() => onSort(sortKey)}
            className={cn("relative flex h-full min-w-0 items-center gap-1 hover:text-foreground", active && "text-foreground", className)}
            title={`Sort by ${String(children).toLowerCase()}`}
        >
            <span className="truncate">{children}</span>
            {active && <Arrow className="size-3.5 shrink-0" />}
            {column && <ColumnEdge column={column} name={String(children)} />}
        </button>
    );
}

/** Between a column and the one before it: dragged left, the column widens, in every list. */
function ColumnEdge({ column, name }: { column: Column; name: string }) {
    const drag = useRef({ width: 0, files: null as HTMLElement | null, now: 0 });
    return (
        <ResizeHandle
            label={`${name} column width`}
            className="-left-2.5"
            onStart={(bar) => {
                drag.current = { width: bar.parentElement!.getBoundingClientRect().width, files: bar.closest<HTMLElement>(".files"), now: 0 };
            }}
            onDrag={(left) => {
                const d = drag.current;
                d.now = Math.round(Math.min(480, Math.max(40, d.width + left)));
                d.files?.style.setProperty(`--col-${column}`, `${d.now}px`);
            }}
            onEnd={() => {
                const { settings, setSettings } = useStore.getState();
                if (drag.current.now) setSettings({ columns: { ...settings.columns, [column]: drag.current.now } });
            }}
            onReset={() => {
                const { settings, setSettings } = useStore.getState();
                const columns = { ...settings.columns };
                delete columns[column];
                setSettings({ columns });
            }}
        />
    );
}

/** Why an item in an archive can't be opened, for its lock icon. */
export const lockReason = (entry: FileEntry) => (entry.attrs & ATTR.encrypted ? "Password-protected" : "Compressed in a way Glint can't read");
/** A key for a password, or a crossed-out file for what Glint can't read. */
export const lockIcon = (entry: FileEntry) => (entry.attrs & ATTR.encrypted ? KeyRound : FileX);

export function LockMark({ entry, className }: { entry: FileEntry; className: string }) {
    const Icon = lockIcon(entry);
    return <Icon className={className} aria-label={lockReason(entry)} />;
}

/** A folder that files dragged in can be dropped on: not one in an archive, which is read-only. */
export const dropPath = (entry: FileEntry) => (entry.dir && !isArchived(entry) ? entry.path : undefined);

/** Where a search result is, below the folder that was searched ("" when right in it). */
export function relativeTo(scope: string) {
    const base = scope.replace(/[\\/]+$/, "").toLowerCase();
    return (path: string) => {
        const folder = parentPath(path);
        if (!scope) return folder;
        const lower = folder.replace(/[\\/]+$/, "").toLowerCase();
        if (lower === base) return "";
        return lower.startsWith(base + "\\") || lower.startsWith(base + "/") ? folder.slice(base.length + 1) : folder;
    };
}

/** A folder's size: the index's total for a search result, else its size measured here (folders sized as they open). */
export const sizeOf = (entry: FileEntry, search: boolean, sizes?: SizeRun) => (entry.dir ? (search ? entry.size || undefined : sizes?.sizes.get(entry.name)) : entry.size);

export function FileList(props: FileListProps) {
    const { rows, rowHeight, search, scope, sizes, diffs, sort, onSort, selection, focusIndex, renaming, cut, dropTarget, inactive, lane, thumbScope, thumbnails, iconBox } = props;
    const focusPath = rows[focusIndex]?.path;
    const columns = useStore((s) => s.settings.columns);
    const relative = useMemo(() => relativeTo(scope), [scope]);
    const maxSize = useMemo(() => {
        let max = 0;
        const limit = Math.min(rows.length, 4000);
        for (let i = 0; i < limit; i++) {
            const s = sizeOf(rows[i], search, sizes);
            if (s !== undefined && s > max) max = s;
        }
        return max;
    }, [rows, search, sizes]);
    const nowMs = Date.now();

    useLayoutEffect(() => {
        if (thumbnails) timeThumbnails(lane, thumbScope);
    }, [thumbnails, lane, thumbScope]);

    const renderRow = (i: number) => {
        const entry = rows[i];
        // Folders found by the index come with their total; fd's come without one (0).
        const size = sizeOf(entry, search, sizes);
        const selected = selection.has(entry.path);
        const sizePct = size && maxSize > 0 ? Math.max(3, Math.round((size / maxSize) * 100)) : 0;
        const isHot = entry.modified > 0 && nowMs - entry.modified >= 0 && nowMs - entry.modified < 3_600_000;
        const diff = diffs?.get(entry.lower);
        const badge = diff && diff !== "same" ? DIFF_BADGE[diff] : undefined;
        return (
            <div
                data-row
                id={`${lane}-row-${i}`}
                role="option"
                aria-selected={selected}
                data-search={search || undefined}
                data-drop={dropPath(entry)}
                onMouseDown={(e) => props.onRowMouseDown(e, i)}
                onDoubleClick={() => props.onOpen(entry)}
                onContextMenu={(e) => props.onRowContextMenu(e, i)}
                className={cn(
                    "file-row h-full cursor-default px-3 text-sm transition-colors duration-75",
                    selected ? (inactive ? "bg-foreground/[0.07]" : "bg-primary/15 dark:bg-primary/20") : "hover:bg-accent/60",
                    entry.path === focusPath && !inactive && "outline -outline-offset-1 outline-primary",
                    entry.path === dropTarget && "bg-primary/20 outline-2 -outline-offset-2 outline-primary",
                    (isHidden(entry) || cut.has(entry.path)) && "opacity-55",
                    diff === "same" && !selected && "opacity-60"
                )}
            >
                <div className="flex min-w-0 items-center gap-2" title={entry.name}>
                    <span className="relative grid size-4 shrink-0 place-items-center">
                        <Thumbnail entry={entry} box={iconBox} lane={lane} enabled={thumbnails && isPictureOrVideo(entry) && !isLocked(entry)} iconClassName="size-4" />
                    </span>
                    {renaming === entry.path ? (
                        <RenameInput entry={entry} onCommit={(name) => props.onRename(entry, name)} onCancel={props.onCancelRename} />
                    ) : (
                        <span className="truncate">{entry.name}</span>
                    )}
                    {badge && (
                        <span title={badge.title} className={cn("shrink-0 rounded-[4px] px-1.5 py-px text-[10px] font-medium leading-tight", badge.className)}>
                            {badge.label}
                        </span>
                    )}
                    {isOnlineOnly(entry) && <Cloud className="size-3.5 shrink-0 text-sky-500" aria-label="Online-only" />}
                    {isLocked(entry) && <LockMark entry={entry} className="size-3.5 shrink-0 text-amber-600 dark:text-amber-500" />}
                </div>
                {search && (
                    <span className="truncate text-xs text-muted-foreground" title={parentPath(entry.path)}>
                        {relative(entry.path)}
                    </span>
                )}
                <span className={cn("flex items-center gap-1.5 truncate text-xs tabular-nums text-muted-foreground", search ? "col-optional" : "col-narrow")} title={formatFullDate(entry.modified)}>
                    {isHot && <span className="size-1.5 shrink-0 rounded-full bg-emerald-500" title="Modified within the last hour" />}
                    <span className="truncate">{formatDate(entry.modified)}</span>
                </span>
                {!search && <span className="col-optional truncate text-xs text-muted-foreground">{typeLabel(entry)}</span>}
                <span className="relative flex h-5 items-center justify-end overflow-hidden rounded-[3px] px-1 text-right text-xs tabular-nums text-muted-foreground">
                    {sizePct > 0 && (
                        <span
                            aria-hidden
                            className="pointer-events-none absolute inset-y-0.5 right-0 rounded-[3px] bg-primary/12 dark:bg-primary/18"
                            style={{ width: `${sizePct}%` }}
                        />
                    )}
                    <span className="relative truncate">{size !== undefined ? formatSize(size) : entry.dir && sizes?.running ? "…" : ""}</span>
                </span>
            </div>
        );
    };

    return (
        <div
            className={cn("files flex h-full min-h-0 flex-col", dropTarget && dropTarget === scope && !search && "bg-primary/5 outline-2 -outline-offset-2 outline-dashed outline-primary/60")}
            style={Object.fromEntries(Object.entries(columns).map(([column, width]) => [`--col-${column}`, `${width}px`]))}
        >
            <div data-search={search || undefined} className="file-row h-8 shrink-0 overflow-y-hidden border-b px-3 text-xs font-medium text-muted-foreground [scrollbar-gutter:stable]">
                <Header sortKey="name" sort={sort} onSort={onSort}>Name</Header>
                {search && <span className="truncate">Folder</span>}
                <Header sortKey="modified" sort={sort} onSort={onSort} column="date" className={search ? "col-optional" : "col-narrow"}>
                    Date modified
                </Header>
                {!search && (
                    <Header sortKey="type" sort={sort} onSort={onSort} column="type" className="col-optional">
                        Type
                    </Header>
                )}
                <Header sortKey="size" sort={sort} onSort={onSort} column="size" className="justify-end">
                    Size
                </Header>
            </div>
            <div className="relative min-h-0 flex-1">
                <VirtualList
                    data-file-list
                    role="listbox"
                    aria-multiselectable
                    aria-label={search ? "Search results" : "Files"}
                    aria-activedescendant={focusIndex >= 0 ? `${lane}-row-${focusIndex}` : undefined}
                    tabIndex={0}
                    count={rows.length}
                    rowHeight={rowHeight}
                    scrollToIndex={focusIndex}
                    initialScrollTop={props.scrollTop}
                    onScrollTop={props.onScrollTop}
                    className="outline-hidden [scrollbar-gutter:stable]"
                    onMouseDown={(e) => !(e.target as Element).closest("[data-row]") && props.onBackgroundMouseDown(e)}
                    onContextMenu={(e) => !(e.target as Element).closest("[data-row]") && props.onBackgroundContextMenu(e)}
                >
                    {renderRow}
                </VirtualList>
                {!rows.length && <div className="pointer-events-none absolute inset-0">{props.empty}</div>}
            </div>
        </div>
    );
}
