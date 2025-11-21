import { useRef, useState, ReactNode, forwardRef, useImperativeHandle } from "react";

interface VirtualListProps {
    height: number;
    width: number;
    itemCount: number;
    itemSize: number;
    children: (props: { index: number; style: React.CSSProperties }) => ReactNode;
}

export interface VirtualListHandle {
    scrollToItem: (index: number) => void;
}

export const VirtualList = forwardRef<VirtualListHandle, VirtualListProps>(({ height, width, itemCount, itemSize, children }, ref) => {
    const [scrollTop, setScrollTop] = useState(0);
    const containerRef = useRef<HTMLDivElement>(null);

    useImperativeHandle(ref, () => ({
        scrollToItem: (index: number) => {
            if (containerRef.current) {
                const top = index * itemSize;
                // Check if item is visible
                if (top < containerRef.current.scrollTop || top > containerRef.current.scrollTop + height - itemSize) {
                    containerRef.current.scrollTo({ top, behavior: 'smooth' });
                }
            }
        }
    }));

    const totalHeight = itemCount * itemSize;
    const startIndex = Math.floor(scrollTop / itemSize);
    const endIndex = Math.min(itemCount - 1, Math.ceil((scrollTop + height) / itemSize));

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
                        height: itemSize,
                        transform: `translateY(${i * itemSize}px)`,
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
