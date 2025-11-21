import { useEffect, useRef, useState, ReactNode } from "react";

interface Size {
    width: number;
    height: number;
}

interface AutoSizerProps {
    children: (size: Size) => ReactNode;
    className?: string;
}

export function AutoSizer({ children, className }: AutoSizerProps) {
    const ref = useRef<HTMLDivElement>(null);
    const [size, setSize] = useState<Size>({ width: 0, height: 0 });

    useEffect(() => {
        const element = ref.current;
        if (!element) return;

        const observer = new ResizeObserver((entries) => {
            for (const entry of entries) {
                const { width, height } = entry.contentRect;
                setSize({ width, height });
            }
        });

        observer.observe(element);
        return () => observer.disconnect();
    }, []);

    return (
        <div ref={ref} className={className} style={{ width: "100%", height: "100%" }}>
            {size.width > 0 && size.height > 0 && children(size)}
        </div>
    );
}
