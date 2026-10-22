import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Cloud } from "lucide-react";
import { isHidden, isLocked, isOnlineOnly } from "@/lib/entries";
import { typeLabel } from "@/lib/fileTypes";
import { formatFullDate, formatSize } from "@/lib/format";
import { gridColumns, type GridTile, placeOf, topOf } from "@/lib/grid";
import { timeThumbnails } from "@/lib/thumbs";
import { cn } from "@/lib/utils";
import { VirtualList } from "../VirtualList";
import { DIFF_BADGE, dropPath, type FileViewProps, Header, LockMark, relativeTo, RenameInput, sizeOf } from "./FileList";
import { Thumbnail } from "./Thumbnail";

interface FileGridProps extends FileViewProps {
    /** The pane it's in: each pane's grid starts as wide as its last one was. */
    pane: number;
    tile: GridTile;
    /** Ctrl+wheel over it: bigger tiles (steps up) or smaller ones (down). */
    onZoom: (steps: number) => void;
}

/** How wide each pane's grid was last, so the next folder's grid has its columns in its first frame. */
const lastWidth = new Map<number, number>();

/** Wheel movement for one tile size: a mouse wheel's notch. Touchpads and fine-grained wheels add up to it. */
const WHEEL_STEP = 100;

/**
 * The items as tiles: Windows' thumbnail of each file (its icon until it's in, or when it has none),
 * the name below it, and its size (a folder's too, once sized). Virtualized by rows like the list, and
 * driven by the same handlers, so selection, keys, menus, rename and drag and drop work alike.
 */
export function FileGrid(props: FileGridProps) {
    const { rows, search, scope, sizes, diffs, sort, onSort, selection, focusIndex, renaming, cut, dropTarget, inactive, lane, thumbScope, pane, tile } = props;
    const [width, setWidth] = useState(() => lastWidth.get(pane) ?? 0);
    const cols = gridColumns(width - 2 * tile.pad, tile.width);
    // Where the tab left this grid is kept as the tiles at its top, so it comes back there even if the
    // tiles were zoomed meanwhile (in another tab, or the other pane).
    const layout = { cols, rowHeight: tile.height };
    const focusPath = rows[focusIndex]?.path;
    const relative = useMemo(() => relativeTo(scope), [scope]);
    useLayoutEffect(() => timeThumbnails(lane, thumbScope), [lane, thumbScope]);

    // Ctrl+wheel zooms the tiles. It's listened for here rather than with onWheel, which React makes
    // passive: only an active listener can stop WebView2 zooming the whole window (and the grid scrolling).
    const ref = useRef<HTMLDivElement>(null);
    const zoom = useRef({ onZoom: props.onZoom, renaming });
    zoom.current = { onZoom: props.onZoom, renaming };
    useEffect(() => {
        const grid = ref.current!;
        let turned = 0;
        let last = 0;
        const onWheel = (e: WheelEvent) => {
            if (!e.ctrlKey) return;
            e.preventDefault();
            // Tiles move to other rows as they zoom, which would make the rename box anew.
            if (zoom.current.renaming) return;
            const delta = e.deltaY * (e.deltaMode === WheelEvent.DOM_DELTA_LINE ? WHEEL_STEP / 3 : e.deltaMode === WheelEvent.DOM_DELTA_PAGE ? WHEEL_STEP : 1);
            // A pause, or a turn the other way, starts over.
            if (e.timeStamp - last > 400 || Math.sign(delta) !== Math.sign(turned)) turned = 0;
            last = e.timeStamp;
            turned += delta;
            const steps = Math.trunc(turned / WHEEL_STEP);
            if (!steps) return;
            turned -= steps * WHEEL_STEP;
            // Away from you (up) for bigger tiles, as in Explorer.
            zoom.current.onZoom(-steps);
        };
        grid.addEventListener("wheel", onWheel, { passive: false });
        return () => grid.removeEventListener("wheel", onWheel);
    }, []);

    const renderTile = (i: number) => {
        const entry = rows[i];
        const size = sizeOf(entry, search, sizes);
        const where = search ? relative(entry.path) : "";
        const selected = selection.has(entry.path);
        const sizeText = size !== undefined ? formatSize(size) : entry.dir && sizes?.running ? "…" : "";
        const diff = diffs?.get(entry.lower);
        const badge = diff && diff !== "same" ? DIFF_BADGE[diff] : undefined;
        return (
            <div
                key={entry.path}
                data-row
                data-drop={dropPath(entry)}
                onMouseDown={(e) => props.onRowMouseDown(e, i)}
                onDoubleClick={() => props.onOpen(entry)}
                onContextMenu={(e) => props.onRowContextMenu(e, i)}
                title={[entry.name, where && `in ${where}`, entry.dir ? "" : `${typeLabel(entry)} · ${formatSize(entry.size)}`, formatFullDate(entry.modified)].filter(Boolean).join("\n")}
                className={cn(
                    "relative m-0.5 flex min-w-0 cursor-default flex-col items-center rounded-md px-1.5 pt-2 text-center",
                    selected ? (inactive ? "bg-foreground/[0.07]" : "bg-primary/15 dark:bg-primary/20") : "hover:bg-accent/60",
                    entry.path === focusPath && !inactive && "outline -outline-offset-1 outline-primary",
                    entry.path === dropTarget && "bg-primary/20 outline-2 -outline-offset-2 outline-primary",
                    (isHidden(entry) || cut.has(entry.path)) && "opacity-55",
                    diff === "same" && !selected && "opacity-60"
                )}
            >
                <div className="relative grid shrink-0 place-items-center" style={{ width: tile.thumb, height: tile.thumb }}>
                    <Thumbnail entry={entry} box={tile.thumb} lane={lane} enabled={!entry.dir && !isLocked(entry)} bottom iconSize={tile.thumb * (entry.dir ? 2 / 3 : 7 / 12)} iconClassName="stroke-[1.1]" />
                    {badge && (
                        <span
                            title={badge.title}
                            className={cn(" absolute top-0.5 right-0.5 rounded-[4px] bg-background/90 px-1 py-px text-[9px] font-medium leading-tight shadow-xs backdrop-blur-xs", badge.className)}
                        >
                            {badge.label}
                        </span>
                    )}
                    {isOnlineOnly(entry) && <Cloud className="absolute bottom-0.5 right-0.5 size-4 fill-background text-sky-500" aria-label="Online-only" />}
                    {isLocked(entry) && <LockMark entry={entry} className="absolute bottom-0.5 right-0.5 size-4 fill-background text-amber-600 dark:text-amber-500" />}
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
        <div ref={ref} className={cn("files flex h-full min-h-0 flex-col", dropTarget && dropTarget === scope && !search && "bg-primary/5 outline-2 -outline-offset-2 outline-dashed outline-primary/60")}>
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
                    perRow={width > 0 ? cols : undefined}
                    anchor={focusIndex}
                    initialScrollTop={props.scrollTop && width ? topOf(props.scrollTop, layout) : 0}
                    onScrollTop={(top) => props.onScrollTop?.(placeOf(top, layout))}
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
