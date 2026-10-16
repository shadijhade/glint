import { useLayoutEffect, useRef } from "react";
import { toast } from "sonner";
import { ChevronDown, ChevronUp, Cloud } from "lucide-react";
import { type FileEntry, isHidden, isOnlineOnly, type Sort, type SortKey } from "@/lib/entries";
import { INVALID_NAME_HINT, type SizeRun } from "@/lib/explorer";
import { kindOf, typeLabel } from "@/lib/fileTypes";
import { formatDate, formatFullDate, formatSize } from "@/lib/format";
import { parentPath } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { VirtualList } from "../VirtualList";

interface FileListProps {
    rows: FileEntry[];
    rowHeight: number;
    /** Search results: a Folder column shows where each one is, relative to `scope`. */
    search: boolean;
    scope: string;
    sizes?: SizeRun;
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
    /** Where to start scrolled to (where this list was left), and where it's scrolled since. */
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

function RenameInput({ entry, onCommit, onCancel }: { entry: FileEntry; onCommit: (name: string) => void; onCancel: () => void }) {
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
            className="h-6 min-w-0 flex-1 rounded-sm border border-ring bg-background px-1 text-sm outline-hidden"
        />
    );
}

function Header({ sortKey, sort, onSort, className, children }: { sortKey: SortKey; sort: Sort; onSort: (key: SortKey) => void; className?: string; children: React.ReactNode }) {
    const active = sort.key === sortKey;
    const Arrow = sort.desc ? ChevronDown : ChevronUp;
    return (
        <button
            onClick={() => onSort(sortKey)}
            className={cn("flex h-full min-w-0 items-center gap-1 hover:text-foreground", active && "text-foreground", className)}
            title={`Sort by ${String(children).toLowerCase()}`}
        >
            <span className="truncate">{children}</span>
            {active && <Arrow className="size-3.5 shrink-0" />}
        </button>
    );
}

export function FileList(props: FileListProps) {
    const { rows, rowHeight, search, scope, sizes, sort, onSort, selection, focusIndex, renaming, cut, dropTarget, inactive } = props;
    const focusPath = rows[focusIndex]?.path;
    // Where a search result is, below the folder that was searched ("" when right in it).
    const base = scope.replace(/[\\/]+$/, "").toLowerCase();
    const relative = (path: string) => {
        const folder = parentPath(path);
        if (!scope) return folder;
        const lower = folder.replace(/[\\/]+$/, "").toLowerCase();
        if (lower === base) return "";
        return lower.startsWith(base + "\\") || lower.startsWith(base + "/") ? folder.slice(base.length + 1) : folder;
    };

    const renderRow = (i: number) => {
        const entry = rows[i];
        const kind = kindOf(entry);
        // Folders found by the index come with their total; fd's come without one (0).
        const size = entry.dir ? (search ? entry.size || undefined : sizes?.sizes.get(entry.name)) : entry.size;
        const selected = selection.has(entry.path);
        return (
            <div
                data-row
                data-search={search || undefined}
                data-drop={entry.dir ? entry.path : undefined}
                onMouseDown={(e) => props.onRowMouseDown(e, i)}
                onDoubleClick={() => props.onOpen(entry)}
                onContextMenu={(e) => props.onRowContextMenu(e, i)}
                className={cn(
                    "file-row h-full cursor-default px-3 text-sm",
                    selected ? (inactive ? "bg-foreground/[0.07]" : "bg-primary/15 dark:bg-primary/20") : "hover:bg-accent/60",
                    entry.path === focusPath && !inactive && "outline -outline-offset-1 outline-primary/40",
                    entry.path === dropTarget && "bg-primary/20 outline-2 -outline-offset-2 outline-primary",
                    (isHidden(entry) || cut.has(entry.path)) && "opacity-55"
                )}
            >
                <div className="flex min-w-0 items-center gap-2" title={entry.name}>
                    <kind.icon className={cn("size-4 shrink-0", kind.tone)} />
                    {renaming === entry.path ? (
                        <RenameInput entry={entry} onCommit={(name) => props.onRename(entry, name)} onCancel={props.onCancelRename} />
                    ) : (
                        <span className="truncate">{entry.name}</span>
                    )}
                    {isOnlineOnly(entry) && <Cloud className="size-3.5 shrink-0 text-sky-500" aria-label="Online-only" />}
                </div>
                {search && (
                    <span className="truncate text-xs text-muted-foreground" title={parentPath(entry.path)}>
                        {relative(entry.path)}
                    </span>
                )}
                <span className={cn("truncate text-xs tabular-nums text-muted-foreground", search ? "col-optional" : "col-narrow")} title={formatFullDate(entry.modified)}>
                    {formatDate(entry.modified)}
                </span>
                {!search && <span className="col-optional truncate text-xs text-muted-foreground">{typeLabel(entry)}</span>}
                <span className="truncate text-right text-xs tabular-nums text-muted-foreground">
                    {size !== undefined ? formatSize(size) : entry.dir && sizes?.running ? "…" : ""}
                </span>
            </div>
        );
    };

    return (
        <div className={cn("files flex h-full min-h-0 flex-col", dropTarget && dropTarget === scope && !search && "bg-primary/5 outline-2 -outline-offset-2 outline-dashed outline-primary/60")}>
            <div data-search={search || undefined} className="file-row h-8 shrink-0 overflow-y-hidden border-b px-3 text-xs font-medium text-muted-foreground [scrollbar-gutter:stable]">
                <Header sortKey="name" sort={sort} onSort={onSort}>Name</Header>
                {search && <span className="truncate">Folder</span>}
                <Header sortKey="modified" sort={sort} onSort={onSort} className={search ? "col-optional" : "col-narrow"}>
                    Date modified
                </Header>
                {!search && (
                    <Header sortKey="type" sort={sort} onSort={onSort} className="col-optional">
                        Type
                    </Header>
                )}
                <Header sortKey="size" sort={sort} onSort={onSort} className="justify-end">
                    Size
                </Header>
            </div>
            <div className="relative min-h-0 flex-1">
                <VirtualList
                    data-file-list
                    count={rows.length}
                    rowHeight={rowHeight}
                    scrollToIndex={focusIndex}
                    initialScrollTop={props.scrollTop}
                    onScrollTop={props.onScrollTop}
                    className="[scrollbar-gutter:stable]"
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
