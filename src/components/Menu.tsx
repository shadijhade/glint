import { useRef, useState } from "react";
import { flushSync } from "react-dom";
import { Check, ChevronRight, type LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

export type MenuItem =
    /** `checked` makes it one of a set to choose from (a check mark in place of an icon), or with `toggle` something on or off. */
    | { label: string; icon?: LucideIcon; shortcut?: string; disabled?: boolean; danger?: boolean; checked?: boolean; toggle?: boolean; onSelect: () => void }
    /** A menu of its own: choosing it (or Right) shows `items` in the menu's place. */
    | { label: string; icon?: LucideIcon; disabled?: boolean; items: MenuItem[] }
    /** Starts a section, with a line above it unless it's first; it goes when nothing is under it. */
    | { heading: string }
    | "separator"
    | false
    | null
    | undefined;

type Shown = Exclude<MenuItem, false | null | undefined>;
const isHeading = (item: Shown | undefined): item is { heading: string } => typeof item === "object" && "heading" in item;
const isSubmenu = (item: Shown | undefined): item is { label: string; icon?: LucideIcon; disabled?: boolean; items: MenuItem[] } => typeof item === "object" && "items" in item;

/** What a menu shows: no heading with nothing under it, and no leading, trailing or doubled separators left by hidden items. */
export function visibleItems(items: MenuItem[]): Shown[] {
    const kept = items.filter((item): item is Shown => !!item);
    const headed = kept.filter((item, i) => !isHeading(item) || (kept[i + 1] !== undefined && kept[i + 1] !== "separator" && !isHeading(kept[i + 1])));
    return headed.filter((item, i, list) => item !== "separator" || (i > 0 && i < list.length - 1 && list[i - 1] !== "separator" && !isHeading(list[i + 1])));
}

const itemClass =
    "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent focus-visible:bg-accent focus-visible:outline-hidden disabled:pointer-events-none disabled:opacity-40 [&_svg]:size-4 [&_svg]:shrink-0";

/** A right-click menu in a native popover at the pointer: Esc and outside clicks close it, arrows move. */
export function useMenu() {
    const ref = useRef<HTMLDivElement>(null);
    const [items, setItems] = useState<MenuItem[]>([]);

    const open = (at: { clientX: number; clientY: number }, next: MenuItem[]) => {
        flushSync(() => setItems(next));
        const menu = ref.current!;
        if (!menu.matches(":popover-open")) menu.showPopover();
        const { width, height } = menu.getBoundingClientRect();
        menu.style.left = `${Math.max(4, Math.min(at.clientX, innerWidth - width - 4))}px`;
        menu.style.top = `${Math.max(4, Math.min(at.clientY, innerHeight - height - 4))}px`;
        menu.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    };

    /** A menu of the menu's: its items in the menu's place, at the row it was chosen from (`from`). */
    const enter = (next: MenuItem[], from: Element | null) => {
        const menu = ref.current!;
        const top = (from ?? menu).getBoundingClientRect().top;
        flushSync(() => setItems(next));
        const { height } = menu.getBoundingClientRect();
        menu.style.top = `${Math.max(4, Math.min(top - 4, innerHeight - height - 4))}px`;
        menu.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    };

    const onKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === "ArrowRight") {
            const at = [...ref.current!.querySelectorAll<HTMLButtonElement>("button")].indexOf(document.activeElement as HTMLButtonElement);
            const item = visible.filter((v) => v !== "separator" && !isHeading(v))[at];
            if (isSubmenu(item) && !item.disabled) {
                e.preventDefault();
                enter(item.items, document.activeElement);
            }
            return;
        }
        if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
        e.preventDefault();
        const buttons = [...ref.current!.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
        const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
        buttons[(at + (e.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length]?.focus();
    };

    const visible = visibleItems(items);

    const element = (
        <div ref={ref} popover="auto" role="menu" onKeyDown={onKeyDown} className="fixed inset-auto m-0 min-w-[14rem] rounded-md border bg-popover p-1 text-popover-foreground shadow-lg">
            {visible.map((item, i) =>
                item === "separator" ? (
                    <div key={i} className="-mx-1 my-1 h-px bg-border" />
                ) : isHeading(item) ? (
                    <div key={i} role="presentation" className={cn("px-2 pb-1 pt-1.5 text-xs font-medium text-muted-foreground", i > 0 && "-mx-1 mt-1 border-t px-3 pt-2")}>
                        {item.heading}
                    </div>
                ) : isSubmenu(item) ? (
                    <button key={i} role="menuitem" aria-haspopup="menu" disabled={item.disabled} className={cn(itemClass, "[&_svg]:text-muted-foreground")} onClick={(e) => enter(item.items, e.currentTarget)}>
                        {item.icon && <item.icon />}
                        <span className="flex-1 truncate">{item.label}</span>
                        <ChevronRight className="ml-6" />
                    </button>
                ) : (
                    item && (
                        <button
                            key={i}
                            role={item.checked === undefined ? "menuitem" : item.toggle ? "menuitemcheckbox" : "menuitemradio"}
                            aria-checked={item.checked}
                            disabled={item.disabled}
                            className={cn(itemClass, item.danger ? "text-destructive [&_svg]:text-destructive" : "[&_svg]:text-muted-foreground")}
                            onClick={() => {
                                ref.current?.hidePopover();
                                item.onSelect();
                            }}
                        >
                            {item.checked !== undefined ? <Check className={item.checked ? "text-primary!" : "invisible"} /> : item.icon && <item.icon />}
                            <span className="flex-1 truncate">{item.label}</span>
                            {item.shortcut && <span className="ml-6 text-xs text-muted-foreground">{item.shortcut}</span>}
                        </button>
                    )
                )
            )}
        </div>
    );

    return { open, element };
}
