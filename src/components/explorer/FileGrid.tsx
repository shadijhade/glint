import { useLayoutEffect, useState } from "react";
import { Cloud } from "lucide-react";
import { isHidden, isOnlineOnly } from "@/lib/entries";
import { typeLabel } from "@/lib/fileTypes";
import { formatFullDate, formatSize } from "@/lib/format";
import { gridColumns } from "@/lib/grid";
import { timeThumbnails } from "@/lib/thumbs";
import { cn } from "@/lib/utils";
import type { GridTile } from "@/hooks/useTheme";
import { VirtualList } from "../VirtualList";
import { type FileViewProps, Header, relativeTo, RenameInput, sizeOf } from "./FileList";
import { Thumbnail } from "./Thumbnail";

interface FileGridProps extends FileViewProps {
    /** The pane it's in: each pane's grid starts as wide as its last one was. */
    pane: number;
    tile: GridTile;
}

/** How wide each pane's grid was last, so the next folder's grid has its columns in its first frame. */
const lastWidth = new Map<number, number>();

/**
 * The items as tiles: Windows' thumbnail of each file (its icon until it's in, or when it has none),
 * the name below it, and its size (a folder's too, once sized). Virtualized by rows like the list, and
 * driven by the same handlers, so selection, keys, menus, rename and drag and drop work alike.
 */
export function FileGrid(props: FileGridProps) {
    const { rows, search, scope, sizes, sort, onSort, selection, focusIndex, renaming, cut, dropTarget, inactive, lane, thumbScope, pane, tile } = props;
    const [width, setWidth] = useState(() => lastWidth.get(pane) ?? 0);
    const cols = gridColumns(width - 2 * tile.pad, tile.width);
    const focusPath = rows[focusIndex]?.path;
    const relative = relativeTo(scope);
    useLayoutEffect(() => timeThumbnails(lane, thumbScope), [lane, thumbScope]);

    const renderTile = (i: number) => {
        const entry = rows[i];
        const size = sizeOf(entry, search, sizes);
        const where = search ? relative(entry.path) : "";
        const selected = selection.has(entry.path);
        const sizeText = size !== undefined ? formatSize(size) : entry.dir && sizes?.running ? "…" : "";
        return (
            <div
                key={entry.path}
                data-row
                data-drop={entry.dir ? entry.path : undefined}
                onMouseDown={(e) => props.onRowMouseDown(e, i)}
                onDoubleClick={() => props.onOpen(entry)}
                onContextMenu={(e) => props.onRowContextMenu(e, i)}
                title={[entry.name, where && `in ${where}`, entry.dir ? "" : `${typeLabel(entry)} · ${formatSize(entry.size)}`, formatFullDate(entry.modified)].filter(Boolean).join("\n")}
                className={cn(
                    "relative m-0.5 flex min-w-0 cursor-default flex-col items-center rounded-md px-1.5 pt-2 text-center",
                    selected ? (inactive ? "bg-foreground/[0.07]" : "bg-primary/15 dark:bg-primary/20") : "hover:bg-accent/60",
                    entry.path === focusPath && !inactive && "outline -outline-offset-1 outline-primary/40",
                    entry.path === dropTarget && "bg-primary/20 outline-2 -outline-offset-2 outline-primary",
                    (isHidden(entry) || cut.has(entry.path)) && "opacity-55"
                )}
            >
                <div className="relative grid shrink-0 place-items-center" style={{ width: tile.thumb, height: tile.thumb }}>
                    <Thumbnail entry={entry} box={tile.thumb} lane={lane} enabled={!entry.dir} bottom iconClassName={entry.dir ? "size-16 stroke-[1.1]" : "size-14 stroke-[1.1]"} />
                    {isOnlineOnly(entry) && <Cloud className="absolute bottom-0.5 right-0.5 size-4 fill-background text-sky-500" aria-label="Online-only" />}
                </div>
                {renaming === entry.path ? (
                    <RenameInput
                        entry={entry}
                        onCommit={(name) => props.onRename(entry, name)}
                        onCancel={props.onCancelRename}
                        className="mt-1.5 w-full flex-none text-center text-xs"
                    />
                ) : (
                    <span className="mt-1.5 line-clamp-2 w-full text-xs leading-4 wrap-anywhere">{entry.name}</span>
                )}
                <span className="h-4 w-full shrink-0 truncate text-[0.6875rem] leading-4 tabular-nums text-muted-foreground">{where || sizeText}</span>
            </div>
        );
    };

    const renderRow = (r: number) => {
        const tiles = [];
        for (let i = r * cols; i < Math.min(rows.length, (r + 1) * cols); i++) tiles.push(renderTile(i));
        return (
            <div className="grid h-full" style={{ gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`, paddingInline: tile.pad }}>
                {tiles}
            </div>
        );
    };

    return (
        <div className={cn("files flex h-full min-h-0 flex-col", dropTarget && dropTarget === scope && !search && "bg-primary/5 outline-2 -outline-offset-2 outline-dashed outline-primary/60")}>
            <div className="flex h-8 shrink-0 items-center gap-4 overflow-hidden border-b px-3 text-xs font-medium text-muted-foreground">
                <span className="shrink-0">Sort by</span>
                <Header sortKey="name" sort={sort} onSort={onSort}>Name</Header>
                <Header sortKey="modified" sort={sort} onSort={onSort}>Date modified</Header>
                {!search && (
                    <Header sortKey="type" sort={sort} onSort={onSort}>
                        Type
                    </Header>
                )}
                <Header sortKey="size" sort={sort} onSort={onSort}>Size</Header>
            </div>
            <div className="relative min-h-0 flex-1">
                <VirtualList
                    data-file-grid
                    data-cols={cols}
                    data-row-height={tile.height}
                    count={Math.ceil(rows.length / cols)}
                    rowHeight={tile.height}
                    overscan={2}
                    scrollToIndex={focusIndex < 0 ? -1 : Math.floor(focusIndex / cols)}
                    scrollKey={focusIndex}
                    initialScrollTop={props.scrollTop}
                    onScrollTop={props.onScrollTop}
                    onWidth={(w) => {
                        lastWidth.set(pane, w);
                        setWidth(w);
                    }}
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
