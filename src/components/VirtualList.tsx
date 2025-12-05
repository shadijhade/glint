import { useRef, useState, ReactNode, forwardRef, useImperativeHandle, useMemo } from "react";

interface VirtualListProps {
    height: number;
    width: number;
    itemCount: number;
    itemSize: (index: number) => number;
    children: (props: { index: number; style: React.CSSProperties }) => ReactNode;
}

export interface VirtualListHandle {
    scrollToItem: (index: number) => void;
}

export const VirtualList = forwardRef<VirtualListHandle, VirtualListProps>(({ height, width, itemCount, itemSize, children }, ref) => {
    const [scrollTop, setScrollTop] = useState(0);
    const containerRef = useRef<HTMLDivElement>(null);

    // Precompute offsets
    const itemOffsets = useMemo(() => {
        const offsets = new Array(itemCount);
        let current = 0;
        for (let i = 0; i < itemCount; i++) {
            offsets[i] = current;
            current += itemSize(i);
        }
        return offsets;
    }, [itemCount, itemSize]);

    const totalHeight = itemOffsets.length > 0 ? itemOffsets[itemCount - 1] + itemSize(itemCount - 1) : 0;

    useImperativeHandle(ref, () => ({
        scrollToItem: (index: number) => {
            if (containerRef.current && index >= 0 && index < itemCount) {
                const top = itemOffsets[index];
                const size = itemSize(index);
                // Check if item is visible
                if (top < containerRef.current.scrollTop || top > containerRef.current.scrollTop + height - size) {
                    containerRef.current.scrollTo({ top, behavior: 'smooth' });
                }
            }
        }
    }));

    // Identify visible range with binary search or simple scan
    // Scan is faster for small lists, but binary search is safer for huge ones.
    // Given we might have thousands, binary search is better.
    const findStartIndex = (offset: number) => {
        let low = 0, high = itemCount - 1;
        while (low <= high) {
            const mid = Math.floor((low + high) / 2);
            const midOffset = itemOffsets[mid];
            const midNextOffset = mid < itemCount - 1 ? itemOffsets[mid + 1] : Infinity;

            if (midOffset <= offset && offset < midNextOffset) {
                return mid;
            } else if (midOffset < offset) {
                low = mid + 1;
            } else {
                high = mid - 1;
            }
        }
        return Math.min(low, itemCount - 1);
    };

    const startIndex = Math.max(0, findStartIndex(scrollTop) - 2); // Buffer
    const endIndex = Math.min(itemCount - 1, findStartIndex(scrollTop + height) + 4); // Buffer

    const visibleItems = [];
    for (let i = startIndex; i <= endIndex; i++) {
        visibleItems.push(
            <div key={i}>
                {children({
                    index: i,
                    style: {
                        position: "absolute",
                        top: 0,
                        left: 0,
                        width: "100%",
                        height: itemSize(i),
                        transform: `translateY(${itemOffsets[i]}px)`,
                    },
                })}
            </div>
        );
    }

    return (
        <div
            ref={containerRef}
            style={{ height, width, overflow: "auto", position: "relative" }}
            onScroll={(e) => setScrollTop((e.target as HTMLDivElement).scrollTop)}
        >
            <div style={{ height: totalHeight, position: "relative" }}>
                {visibleItems}
            </div>
        </div>
    );
});
