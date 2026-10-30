import { useLayoutEffect, useMemo, useRef } from "react";
import { toast } from "sonner";
import { ChevronDown, ChevronUp, Cloud, FileX, KeyRound } from "lucide-react";
import { ATTR, type DiffStatus, type FileEntry, isArchived, isClosed, isHidden, isListOnly, isLocked, isOnlineOnly, isRecycled, type Sort, type SortKey } from "@/lib/entries";
import { INVALID_NAME_HINT, type SizeRun } from "@/lib/explorer";
import { isPictureOrVideo, typeLabel } from "@/lib/fileTypes";
import { formatDate, formatFullDate, formatSize } from "@/lib/format";
import type { GitMark } from "@/lib/fs";
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
    differ: { label: "Different", title: "Not the same as in the other pane: another size, or compared deeper, other contents or something different inside", className: "bg-amber-500/15 text-amber-700 dark:text-amber-300" },
    older: { label: "Older", title: "Modified earlier than in the other pane", className: "bg-muted text-muted-foreground" },
};

/** What Git says of an item; a folder's tooltip says it of what's in it. Ignored items show faded instead. */
export const GIT_BADGE: Record<Exclude<GitMark, "ignored">, { label: string; title: string; folder: string; className: string }> = {
    conflicted: { label: "Conflict", title: "Git: a merge left conflicts in it", folder: "Git: a merge left conflicts in here", className: "bg-red-500/15 text-red-700 dark:text-red-300" },
    modified: { label: "Changed", title: "Git: changed since it was committed", folder: "Git: something in here changed since it was committed", className: "bg-amber-500/15 text-amber-700 dark:text-amber-300" },
    staged: { label: "Staged", title: "Git: changed and staged for the next commit", folder: "Git: changes in here are staged for the next commit", className: "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300" },
    untracked: { label: "New", title: "Git: new, not added yet", folder: "Git: new files in here, not added yet", className: "bg-sky-500/15 text-sky-700 dark:text-sky-300" },
};

/** An item's Git badge, and whether .gitignore leaves it out (it shows faded). */
export function gitOf(entry: FileEntry, git?: Readonly<Record<string, GitMark>>) {
    const mark = git?.[entry.lower];
    const badge = mark && mark !== "ignored" ? GIT_BADGE[mark] : undefined;
    return { badge: badge && { ...badge, title: entry.dir ? badge.folder : badge.title }, ignored: mark === "ignored" };
}

/** What the list and the grid both show, and how they report what's done to them. */
export interface FileViewProps {
    rows: FileEntry[];
    /** Search results: each one shows where it is, relative to `scope`. */
    search: boolean;
    /** The Recycle Bin: each item shows the folder it was deleted from, and when (its date). */
    recycled?: boolean;
    scope: string;
    sizes?: SizeRun;
    /** Lowercased name → comparison status against the other pane, when Compare Panes is active. */
    diffs?: ReadonlyMap<string, DiffStatus>;
    /** Lowercased name → what Git says of it, when the folder is in a repository. */
    git?: Readonly<Record<string, GitMark>>;
    sort: Sort;
    onSort: (key: SortKey) => void;
    /** The header's menu: what to sort by. */
    onHeaderMenu?: (e: React.MouseEvent) => void;
    /** When a photo or video was taken, as far as it's known (sorting by date taken). */
    takenOf?: (entry: FileEntry) => number | null | undefined;
    selection: ReadonlySet<string>;
    focusIndex: number;
    renaming: string | null;
    cut: ReadonlySet<string>;
    dragging: ReadonlySet<string>;
    flashed: ReadonlySet<string>;
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

/** A folder that files dragged in can be dropped on: not one in an archive, which is read-only, nor one Windows keeps closed, nor one in the Recycle Bin. */
export const dropPath = (entry: FileEntry) => (entry.dir && !isArchived(entry) && !isClosed(entry) && !isRecycled(entry) ? entry.path : undefined);

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

/** A folder's size: the index's total for a search result (or the Recycle Bin's, with `located`), else its size measured here (folders sized as they open). */
export const sizeOf = (entry: FileEntry, located: boolean, sizes?: SizeRun) => (entry.dir ? (located ? entry.size || undefined : sizes?.sizes.get(entry.name)) : entry.size);

/** Where an item is, in a list of items from many folders: a search result's folder (below `scope`), or the folder a recycled one was deleted from. */
export function whereOf(relative: (path: string) => string) {
    return (entry: FileEntry) => (isRecycled(entry) ? (entry.origin ?? "") : relative(entry.path));
}

export function FileList(props: FileListProps) {
    const { rows, rowHeight, search, recycled, scope, sizes, diffs, git, sort, onSort, onHeaderMenu, takenOf, selection, focusIndex, renaming, cut, dragging, flashed, dropTarget, inactive, lane, thumbScope, thumbnails, iconBox } = props;
    // Items from many folders (search results, the Recycle Bin's) say where each is, in a column of its own.
    const located = search || !!recycled;
    // Sorted by date taken, the date column shows it (a file that says none, its date modified).
    const byTaken = sort.key === "taken";
    const focusPath = rows[focusIndex]?.path;
    const columns = useStore((s) => s.settings.columns);
    const where = useMemo(() => whereOf(relativeTo(scope)), [scope]);
    const maxSize = useMemo(() => {
        let max = 0;
        const limit = Math.min(rows.length, 4000);
        for (let i = 0; i < limit; i++) {
            const s = sizeOf(rows[i], located, sizes);
            if (s !== undefined && s > max) max = s;
        }
        return max;
    }, [rows, located, sizes]);
    const nowMs = Date.now();

    useLayoutEffect(() => {
        if (thumbnails) timeThumbnails(lane, thumbScope);
    }, [thumbnails, lane, thumbScope]);

    const renderRow = (i: number) => {
        const entry = rows[i];
        // Folders found by the index come with their total; fd's come without one (0).
        const size = sizeOf(entry, located, sizes);
        const selected = selection.has(entry.path);
        // A bar only where it says something: 8% of the biggest here or more (smaller ones would be slivers).
        const share = size && maxSize > 0 ? (size / maxSize) * 100 : 0;
        const sizePct = share >= 8 ? Math.round(share) : 0;
        const isHot = !recycled && entry.modified > 0 && nowMs - entry.modified >= 0 && nowMs - entry.modified < 3_600_000;
        const diff = diffs?.get(entry.lower);
        const badge = diff && diff !== "same" ? DIFF_BADGE[diff] : undefined;
        const tracked = gitOf(entry, git);
        return (
            <div
                data-row
                id={`${lane}-row-${i}`}
                role="option"
                aria-selected={selected}
                data-search={located || undefined}
                data-drop={dropPath(entry)}
                onMouseDown={(e) => props.onRowMouseDown(e, i)}
                onDoubleClick={() => props.onOpen(entry)}
                onContextMenu={(e) => props.onRowContextMenu(e, i)}
                className={cn(
                    "file-row h-full cursor-default px-3 text-sm transition-colors duration-75",
                    selected ? (inactive ? "bg-foreground/[0.07]" : "bg-primary/15 dark:bg-primary/20") : "hover:bg-accent/60",
                    entry.path === focusPath && !inactive && "outline -outline-offset-1 outline-primary",
                    entry.path === dropTarget && "bg-primary/20 outline-2 -outline-offset-2 outline-primary",
                    (isHidden(entry) || cut.has(entry.path) || tracked.ignored) && "opacity-55",
                    dragging.has(entry.path) && "opacity-45",
                    flashed.has(entry.path) && "glint-flash",
                    diff === "same" && !selected && "opacity-60"
                )}
            >
                <div className="flex min-w-0 items-center gap-2" title={tracked.ignored ? `${entry.name}\nGit: left out by .gitignore` : entry.name}>
                    <span className="relative grid size-4 shrink-0 place-items-center">
                        <Thumbnail entry={entry} box={iconBox} lane={lane} enabled={thumbnails && isPictureOrVideo(entry) && !isLocked(entry) && !isListOnly(entry)} iconClassName="size-4" />
                    </span>
                    {renaming === entry.path ? (
                        <RenameInput entry={entry} onCommit={(name) => props.onRename(entry, name)} onCancel={props.onCancelRename} />
                    ) : (
                        <span className="truncate">{entry.name}</span>
                    )}
                    {badge && (
                        <span title={badge.title} className={cn("shrink-0 rounded-[4px] px-1.5 py-px text-[0.625rem] font-medium leading-tight", badge.className)}>
                            {badge.label}
                        </span>
                    )}
                    {tracked.badge && (
                        <span data-git title={tracked.badge.title} className={cn("shrink-0 rounded-[4px] px-1.5 py-px text-[0.625rem] font-medium leading-tight", tracked.badge.className)}>
                            {tracked.badge.label}
                        </span>
                    )}
                    {isOnlineOnly(entry) && <Cloud className="size-3.5 shrink-0 text-sky-500" aria-label="Online-only" />}
                    {isLocked(entry) && <LockMark entry={entry} className="size-3.5 shrink-0 text-amber-600 dark:text-amber-500" />}
                </div>
                {located && (
                    <span className="truncate text-xs text-muted-foreground" title={recycled ? entry.origin : parentPath(entry.path)}>
                        {where(entry)}
                    </span>
                )}
                {byTaken && typeof takenOf?.(entry) === "number" ? (
                    <span className={cn("truncate text-xs tabular-nums text-foreground/80", located ? "col-optional" : "col-narrow")} title={`Taken ${formatFullDate(takenOf(entry)!)}`}>
                        {formatDate(takenOf(entry)!)}
                    </span>
                ) : (
                    <span
                        className={cn("flex items-center gap-1.5 truncate text-xs tabular-nums text-muted-foreground", located ? "col-optional" : "col-narrow")}
                        title={`${recycled ? "Deleted " : ""}${formatFullDate(entry.modified)}`}
                    >
                        {isHot && <span className="size-1.5 shrink-0 rounded-full bg-emerald-500" title="Modified within the last hour" />}
                        <span className="truncate">{formatDate(entry.modified)}</span>
                    </span>
                )}
                {!located && <span className="col-optional truncate text-xs text-muted-foreground">{typeLabel(entry)}</span>}
                <span className="relative flex h-5 items-center justify-end overflow-hidden rounded-[3px] px-1 text-right text-xs tabular-nums text-muted-foreground">
                    {sizePct > 0 && (
                        <span
                            aria-hidden
                            className="pointer-events-none absolute inset-x-0 inset-y-0.5 origin-right rounded-[3px] bg-primary/12 transition-transform duration-200 ease-out dark:bg-primary/18"
                            style={{ transform: `scaleX(${sizePct / 100})` }}
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
            <div
                data-search={located || undefined}
                onContextMenu={onHeaderMenu}
                className="file-row h-8 shrink-0 overflow-y-hidden border-b px-3 text-xs font-medium text-muted-foreground [scrollbar-gutter:stable]"
            >
                <Header sortKey="name" sort={sort} onSort={onSort}>Name</Header>
                {located && <span className="truncate">{recycled ? "Deleted from" : "Folder"}</span>}
                <Header sortKey={byTaken ? "taken" : "modified"} sort={sort} onSort={onSort} column="date" className={located ? "col-optional" : "col-narrow"}>
                    {byTaken ? "Date taken" : recycled ? "Date deleted" : "Date modified"}
                </Header>
                {!located && (
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
                    aria-label={search ? "Search results" : recycled ? "Recycle Bin" : "Files"}
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
