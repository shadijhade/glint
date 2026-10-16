import { ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { keepPlace } from "@/lib/grid";
import { cn } from "@/lib/utils";

/** Fixed-row-height virtual list. Scrolls `scrollToIndex` into view whenever it changes. */
export function VirtualList({ count, rowHeight, scrollToIndex = -1, scrollKey, initialScrollTop = 0, onScrollTop, overscan = 8, onWidth, perRow, anchor = -1, children, className, ...props }: {
    count: number;
    rowHeight: number;
    scrollToIndex?: number;
    /** Scrolls `scrollToIndex` into view again when this changes too (a grid's cursor moving along a row). */
    scrollKey?: unknown;
    /** Where to start, e.g. where this list was left last time (then `scrollToIndex` waits for its next change). */
    initialScrollTop?: number;
    onScrollTop?: (top: number) => void;
    /** Rows drawn beyond each edge, so scrolling never shows a gap. */
    overscan?: number;
    /** The width rows get, as it changes (a grid fits its columns to it); first called before the first paint. */
    onWidth?: (width: number) => void;
    /**
     * For a grid: the items to a row, once known. When they or `rowHeight` change (its tiles zoom, its
     * pane narrows), it keeps its place: item `anchor` (the focused one) stays where it was if it's on
     * screen, otherwise the items at the top stay there (see `keepPlace`).
     */
    perRow?: number;
    anchor?: number;
    children: (index: number) => ReactNode;
} & Omit<React.ComponentProps<"div">, "children" | "onScroll" | "ref">) {
    const ref = useRef<HTMLDivElement>(null);
    const [view, setView] = useState({ top: initialScrollTop, height: 0 });
    const restoring = useRef(initialScrollTop > 0);
    const widthRef = useRef(onWidth);
    widthRef.current = onWidth;
    // Where it's scrolled, as of its last scroll: by the time a new layout is in the page, the browser
    // may have cut scrollTop down to fit it.
    const scrolled = useRef(initialScrollTop);
    const laidOut = useRef({ cols: perRow ?? 0, rowHeight });
    const relaidOut = useRef(false);
    const measure = () => {
        const el = ref.current;
        if (!el) return;
        setView((v) => (v.top === el.scrollTop && v.height === el.clientHeight ? v : { top: el.scrollTop, height: el.clientHeight }));
        widthRef.current?.(el.clientWidth);
    };

    // Only where it starts; later scrolling is the user's. Measured now too, so the first frame shows
    // every row that fits rather than the first few.
    useLayoutEffect(() => {
        if (restoring.current) ref.current!.scrollTop = initialScrollTop;
        measure();
    }, []);

    useEffect(() => {
        const observer = new ResizeObserver(measure);
        observer.observe(ref.current!);
        return () => observer.disconnect();
    }, []);

    // A grid laid out anew is scrolled to keep its place before it's painted, with the rows for there.
    useLayoutEffect(() => {
        const was = laidOut.current;
        laidOut.current = { cols: perRow ?? 0, rowHeight };
        const el = ref.current;
        if (!el || !perRow || !was.cols || (was.cols === perRow && was.rowHeight === rowHeight)) return;
        el.scrollTop = keepPlace(was, laidOut.current, scrolled.current, el.clientHeight, count, anchor);
        scrolled.current = el.scrollTop;
        relaidOut.current = true;
        setView({ top: el.scrollTop, height: el.clientHeight });
    }, [perRow, rowHeight]);

    useEffect(() => {
        const el = ref.current;
        if (restoring.current) {
            restoring.current = false;
            return;
        }
        // Laid out anew: the focused item was kept in view if it was in view; if not, the place is kept.
        if (relaidOut.current) {
            relaidOut.current = false;
            return;
        }
        if (!el || scrollToIndex < 0) return;
        const top = scrollToIndex * rowHeight;
        if (top < el.scrollTop) el.scrollTop = top;
        else if (top + rowHeight > el.scrollTop + el.clientHeight) el.scrollTop = top + rowHeight - el.clientHeight;
        scrolled.current = el.scrollTop;
    }, [scrollToIndex, rowHeight, scrollKey, perRow]);

    const first = Math.max(0, Math.floor(view.top / rowHeight) - overscan);
    const last = Math.min(count, Math.ceil((view.top + view.height) / rowHeight) + overscan);
    const rows: ReactNode[] = [];
    for (let i = first; i < last; i++) {
        rows.push(
            <div key={i} className="absolute inset-x-0" style={{ top: i * rowHeight, height: rowHeight }}>
                {children(i)}
            </div>
        );
    }

    return (
        <div
            ref={ref}
            onScroll={() => {
                scrolled.current = ref.current!.scrollTop;
                measure();
                onScrollTop?.(ref.current!.scrollTop);
            }}
            className={cn("h-full overflow-y-auto overflow-x-hidden", className)}
            {...props}
        >
            <div className="relative" style={{ height: count * rowHeight }}>{rows}</div>
        </div>
    );
}
