import { useEffect, useRef } from "react";
import { cn } from "@/lib/utils";

const FOCUSABLE = "button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])";

/**
 * A dialog over the panes, as the tool dialogs share it: focus moves into it (to what asks for it,
 * else the dialog itself), Tab stays inside, and focus goes back where it was when it closes. A click
 * on the backdrop closes it; each dialog handles Esc itself. With `onSubmit` it's a form: Enter in a
 * field submits it.
 */
export function Modal({ label, onClose, onSubmit, className, children }: { label: string; onClose: () => void; onSubmit?: () => void; className?: string; children: React.ReactNode }) {
    const ref = useRef<HTMLElement>(null);

    useEffect(() => {
        const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        const box = ref.current!;
        // A field with autoFocus already has it.
        if (!box.contains(document.activeElement)) box.focus();
        return () => before?.focus();
    }, []);

    const onKeyDown = (e: React.KeyboardEvent) => {
        if (e.key !== "Tab") return;
        const items = [...ref.current!.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.offsetParent !== null);
        if (!items.length) return e.preventDefault();
        const [first, last] = [items[0], items[items.length - 1]];
        if (e.shiftKey ? document.activeElement === first || document.activeElement === ref.current : document.activeElement === last) {
            e.preventDefault();
            (e.shiftKey ? last : first).focus();
        }
    };

    const panel = {
        role: "dialog",
        "aria-modal": true,
        "aria-label": label,
        tabIndex: -1,
        onKeyDown,
        className: cn("flex w-full flex-col overflow-hidden rounded-xl border bg-popover text-popover-foreground shadow-2xl outline-hidden", className),
    } as const;
    return (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
            {onSubmit ? (
                <form
                    ref={ref as React.Ref<HTMLFormElement>}
                    {...panel}
                    onSubmit={(e) => {
                        e.preventDefault();
                        onSubmit();
                    }}
                >
                    {children}
                </form>
            ) : (
                <div ref={ref as React.Ref<HTMLDivElement>} {...panel}>
                    {children}
                </div>
            )}
        </div>
    );
}

/** A row that acts like a button (it holds buttons of its own, so it can't be one): Tab reaches it, Enter and Space press it. */
export const pressable = (run: () => void) => ({
    tabIndex: 0,
    onClick: run,
    onKeyDown: (e: React.KeyboardEvent) => {
        if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
            e.preventDefault();
            run();
        }
    },
});
