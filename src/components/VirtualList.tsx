import { ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";

/** Fixed-row-height virtual list. Scrolls `scrollToIndex` into view whenever it changes. */
export function VirtualList({ count, rowHeight, scrollToIndex = -1, scrollKey, initialScrollTop = 0, onScrollTop, overscan = 8, onWidth, children, className, ...props }: {
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
    children: (index: number) => ReactNode;
} & Omit<React.ComponentProps<"div">, "children" | "onScroll" | "ref">) {
    const ref = useRef<HTMLDivElement>(null);
    const [view, setView] = useState({ top: initialScrollTop, height: 0 });
    const restoring = useRef(initialScrollTop > 0);
    const widthRef = useRef(onWidth);
    widthRef.current = onWidth;
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

    useEffect(() => {
        const el = ref.current;
        if (restoring.current) {
            restoring.current = false;
            return;
        }
        if (!el || scrollToIndex < 0) return;
        const top = scrollToIndex * rowHeight;
        if (top < el.scrollTop) el.scrollTop = top;
        else if (top + rowHeight > el.scrollTop + el.clientHeight) el.scrollTop = top + rowHeight - el.clientHeight;
    }, [scrollToIndex, rowHeight, scrollKey]);

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
