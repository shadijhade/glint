import { ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";

const OVERSCAN = 8;

/** Fixed-row-height virtual list. Scrolls `scrollToIndex` into view whenever it changes. */
export function VirtualList({ count, rowHeight, scrollToIndex = -1, initialScrollTop = 0, onScrollTop, children, className, ...props }: {
    count: number;
    rowHeight: number;
    scrollToIndex?: number;
    /** Where to start, e.g. where this list was left last time (then `scrollToIndex` waits for its next change). */
    initialScrollTop?: number;
    onScrollTop?: (top: number) => void;
    children: (index: number) => ReactNode;
} & Omit<React.ComponentProps<"div">, "children" | "onScroll" | "ref">) {
    const ref = useRef<HTMLDivElement>(null);
    const [view, setView] = useState({ top: initialScrollTop, height: 0 });
    const restoring = useRef(initialScrollTop > 0);
    const measure = () => {
        const el = ref.current;
        if (el) setView({ top: el.scrollTop, height: el.clientHeight });
    };

    // Only where it starts; later scrolling is the user's.
    useLayoutEffect(() => {
        if (restoring.current) ref.current!.scrollTop = initialScrollTop;
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
    }, [scrollToIndex, rowHeight]);

    const first = Math.max(0, Math.floor(view.top / rowHeight) - OVERSCAN);
    const last = Math.min(count, Math.ceil((view.top + view.height) / rowHeight) + OVERSCAN);
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
