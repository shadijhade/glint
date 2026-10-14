import { ReactNode, useEffect, useRef, useState } from "react";

const OVERSCAN = 8;

/** Fixed-row-height virtual list. Scrolls `scrollToIndex` into view whenever it changes. */
export function VirtualList({ count, rowHeight, scrollToIndex = -1, children }: {
    count: number;
    rowHeight: number;
    scrollToIndex?: number;
    children: (index: number) => ReactNode;
}) {
    const ref = useRef<HTMLDivElement>(null);
    const [view, setView] = useState({ top: 0, height: 0 });
    const measure = () => {
        const el = ref.current;
        if (el) setView({ top: el.scrollTop, height: el.clientHeight });
    };

    useEffect(() => {
        const observer = new ResizeObserver(measure);
        observer.observe(ref.current!);
        return () => observer.disconnect();
    }, []);

    useEffect(() => {
        const el = ref.current;
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
        <div ref={ref} onScroll={measure} className="h-full overflow-y-auto overflow-x-hidden">
            <div className="relative" style={{ height: count * rowHeight }}>{rows}</div>
        </div>
    );
}
