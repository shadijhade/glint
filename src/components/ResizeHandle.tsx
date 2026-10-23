import { cn } from "@/lib/utils";

/**
 * A thin bar on an edge, to drag for a width: `onStart` gets the bar (to measure what it sits on),
 * `onDrag` how far the pointer is left of where it started (positive: to the left), `onEnd` comes
 * when it's let go, and a double-click asks for the default back (`onReset`). Across (`across`), on
 * a top edge, it's for a height: `onDrag` gets how far up.
 */
export function ResizeHandle({
    label,
    onStart,
    onDrag,
    onEnd,
    onReset,
    className,
    across,
}: {
    label: string;
    onStart: (bar: HTMLElement) => void;
    onDrag: (left: number) => void;
    onEnd: () => void;
    onReset: () => void;
    className?: string;
    across?: boolean;
}) {
    return (
        <span
            role="separator"
            aria-orientation={across ? "horizontal" : "vertical"}
            aria-label={label}
            title={`${label} (double-click: as it was)`}
            onPointerDown={(e) => {
                if (e.button !== 0) return;
                e.preventDefault();
                e.stopPropagation();
                const bar = e.currentTarget;
                bar.setPointerCapture(e.pointerId);
                const from = across ? e.clientY : e.clientX;
                onStart(bar);
                const move = (m: PointerEvent) => onDrag(from - (across ? m.clientY : m.clientX));
                const up = () => {
                    bar.removeEventListener("pointermove", move);
                    bar.removeEventListener("pointerup", up);
                    bar.removeEventListener("pointercancel", up);
                    onEnd();
                };
                bar.addEventListener("pointermove", move);
                bar.addEventListener("pointerup", up);
                bar.addEventListener("pointercancel", up);
            }}
            // Not a click on what it sits in (a column's header sorts).
            onClick={(e) => e.stopPropagation()}
            onDoubleClick={(e) => {
                e.stopPropagation();
                onReset();
            }}
            className={cn(
                across
                    ? "absolute left-0 z-10 h-2 w-full cursor-row-resize touch-none after:absolute after:inset-x-1 after:top-1/2 after:h-px after:-translate-y-1/2 hover:after:bg-primary/60"
                    : "absolute top-0 z-10 h-full w-2 cursor-col-resize touch-none after:absolute after:inset-y-1 after:left-1/2 after:w-px after:-translate-x-1/2 hover:after:bg-primary/60",
                className
            )}
        />
    );
}
