import { useRef, useState } from "react";
import { flushSync } from "react-dom";
import { ArrowLeft, Check, ChevronRight, type LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

export interface MenuAction {
    label: string;
    icon: LucideIcon;
    shortcut?: string;
    disabled?: boolean;
    danger?: boolean;
    active?: boolean;
    onSelect: () => void;
}

export type MenuItem =
    /** A compact top icon strip for frequent actions (Cut, Copy, Paste, Rename, Copy path, Pin, Delete). */
    | { actions: (MenuAction | false | null | undefined)[] }
    /** `checked` makes it one of a set to choose from (a check mark in place of an icon), or with `toggle` something on or off; `image` is a picture (an app's icon) in its icon's place. */
    | { label: string; icon?: LucideIcon; image?: string; shortcut?: string; disabled?: boolean; danger?: boolean; checked?: boolean; toggle?: boolean; onSelect: () => void }
    /** A menu of its own: choosing it (or Right) shows `items` in the menu's place, with Back. */
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
const isActionRow = (item: Shown | undefined): item is { actions: (MenuAction | false | null | undefined)[] } => typeof item === "object" && "actions" in item;
const shownActions = (item: { actions: (MenuAction | false | null | undefined)[] }): MenuAction[] => item.actions.filter((a): a is MenuAction => !!a);

/** What a menu shows: no heading with nothing under it, no empty action row or submenu, and no leading, trailing or doubled separators left by hidden items. */
export function visibleItems(items: MenuItem[]): Shown[] {
    const kept = items.filter((item): item is Shown => {
        if (!item) return false;
        if (isActionRow(item)) return shownActions(item).length > 0;
        if (isSubmenu(item)) return visibleItems(item.items).length > 0;
        return true;
    });
    const headed = kept.filter((item, i) => !isHeading(item) || (kept[i + 1] !== undefined && kept[i + 1] !== "separator" && !isHeading(kept[i + 1])));
    return headed.filter((item, i, list) => item !== "separator" || (i > 0 && i < list.length - 1 && list[i - 1] !== "separator" && !isActionRow(list[i - 1]) && !isHeading(list[i + 1])));
}

const itemClass =
    "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm transition-colors duration-75 hover:bg-accent focus-visible:bg-accent focus-visible:outline-hidden disabled:pointer-events-none disabled:opacity-40 [&_svg]:size-4 [&_svg]:shrink-0";

/** The item the keyboard starts on: the first one past the quick-action icons and Back. */
const focusFirst = (menu: HTMLElement) =>
    (menu.querySelector<HTMLButtonElement>("button[role^='menuitem']:not(:disabled):not([data-action])") ?? menu.querySelector<HTMLButtonElement>("button:not(:disabled)"))?.focus();

/** A right-click menu in a native popover at the pointer: Esc and outside clicks close it, arrows move. */
export function useMenu() {
    const ref = useRef<HTMLDivElement>(null);
    const [items, setItems] = useState<MenuItem[]>([]);
    const [stack, setStack] = useState<{ label: string; items: MenuItem[] }[]>([]);

    const open = (at: { clientX: number; clientY: number }, next: MenuItem[]) => {
        flushSync(() => {
            setStack([]);
            setItems(next);
        });
        const menu = ref.current!;
        if (!menu.matches(":popover-open")) menu.showPopover();
        const { width, height } = menu.getBoundingClientRect();
        const left = Math.max(4, Math.min(at.clientX, innerWidth - width - 4));
        const top = Math.max(4, Math.min(at.clientY, innerHeight - height - 4));
        menu.style.left = `${left}px`;
        menu.style.top = `${top}px`;
        menu.style.transformOrigin = `${top < at.clientY - 8 ? "bottom" : "top"} ${left < at.clientX - 8 ? "right" : "left"}`;
        focusFirst(menu);
    };

    /** A menu of the menu's: its items in the menu's place, at the row it was chosen from (`from`), with Back. */
    const enter = (label: string, next: MenuItem[], from: Element | null) => {
        const menu = ref.current!;
        const top = (from ?? menu).getBoundingClientRect().top;
        flushSync(() => {
            setStack((s) => [...s, { label, items }]);
            setItems(next);
        });
        const { height } = menu.getBoundingClientRect();
        menu.style.top = `${Math.max(4, Math.min(top - 4, innerHeight - height - 4))}px`;
        focusFirst(menu);
    };

    /** Returns to the parent menu when inside a submenu. */
    const leave = () => {
        const prev = stack[stack.length - 1];
        if (!prev) return;
        const menu = ref.current!;
        const top = menu.getBoundingClientRect().top;
        flushSync(() => {
            setStack((s) => s.slice(0, -1));
            setItems(prev.items);
        });
        const { height } = menu.getBoundingClientRect();
        menu.style.top = `${Math.max(4, Math.min(top, innerHeight - height - 4))}px`;
        focusFirst(menu);
    };

    const onKeyDown = (e: React.KeyboardEvent) => {
        if ((e.key === "ArrowLeft" || e.key === "Backspace") && stack.length > 0) {
            e.preventDefault();
            leave();
            return;
        }
        if (e.key === "ArrowRight") {
            const menuButtons = [...ref.current!.querySelectorAll<HTMLButtonElement>("button[data-submenu]")];
            if (menuButtons.includes(document.activeElement as HTMLButtonElement)) {
                const idx = Number((document.activeElement as HTMLButtonElement).dataset.submenu);
                const item = visible[idx];
                if (isSubmenu(item) && !item.disabled) {
                    e.preventDefault();
                    enter(item.label, item.items, document.activeElement);
                }
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
    const parent = stack[stack.length - 1];

    const element = (
        <div
            ref={ref}
            popover="auto"
            role="menu"
            onKeyDown={onKeyDown}
            className="fixed inset-auto m-0 min-w-[14.5rem] rounded-lg border border-border/80 bg-popover/95 p-1 text-popover-foreground shadow-xl backdrop-blur-xl"
        >
            {parent && (
                <button
                    type="button"
                    onClick={leave}
                    className="mb-1 flex w-full items-center gap-2 rounded-sm border-b border-border/60 px-2 py-1.5 text-left text-xs font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:bg-accent focus-visible:outline-hidden [&_svg]:size-3.5"
                >
                    <ArrowLeft />
                    <span className="truncate">{parent.label}</span>
                </button>
            )}
            {visible.map((item, i) =>
                item === "separator" ? (
                    <div key={i} className="-mx-1 my-1 h-px bg-border/70" />
                ) : isActionRow(item) ? (
                    <div
                        key={i}
                        role="group"
                        aria-label="Quick actions"
                        className={cn("flex items-center gap-0.5 px-0.5 py-0.5", i < visible.length - 1 && "-mx-1 mb-1 border-b border-border/70 px-1.5 pb-1.5")}
                    >
                        {shownActions(item).map((action, j) => (
                            <button
                                key={j}
                                type="button"
                                role="menuitem"
                                data-action
                                aria-label={action.label}
                                title={action.shortcut ? `${action.label} (${action.shortcut})` : action.label}
                                disabled={action.disabled}
                                onClick={() => {
                                    ref.current?.hidePopover();
                                    action.onSelect();
                                }}
                                className={cn(
                                    "flex flex-1 items-center justify-center rounded-md p-1.5 transition-all duration-75 hover:bg-accent focus-visible:bg-accent focus-visible:outline-hidden active:scale-92 disabled:pointer-events-none disabled:opacity-35 [&_svg]:size-4",
                                    action.danger
                                        ? "text-destructive hover:bg-destructive/15 focus-visible:bg-destructive/15"
                                        : action.active
                                          ? "text-primary"
                                          : "text-popover-foreground/85 hover:text-popover-foreground"
                                )}
                            >
                                <action.icon />
                            </button>
                        ))}
                    </div>
                ) : isHeading(item) ? (
                    <div key={i} role="presentation" className={cn("px-2 pb-1 pt-1.5 text-xs font-medium text-muted-foreground", i > 0 && "-mx-1 mt-1 border-t border-border/70 px-3 pt-2")}>
                        {item.heading}
                    </div>
                ) : isSubmenu(item) ? (
                    <button
                        key={i}
                        type="button"
                        role="menuitem"
                        aria-haspopup="menu"
                        data-submenu={i}
                        disabled={item.disabled}
                        className={cn(itemClass, "[&_svg]:text-muted-foreground")}
                        onClick={(e) => enter(item.label, item.items, e.currentTarget)}
                    >
                        {item.icon && <item.icon />}
                        <span className="flex-1 truncate">{item.label}</span>
                        <ChevronRight className="ml-6 opacity-70" />
                    </button>
                ) : (
                    item && (
                        <button
                            key={i}
                            type="button"
                            role={item.checked === undefined ? "menuitem" : item.toggle ? "menuitemcheckbox" : "menuitemradio"}
                            aria-checked={item.checked}
                            disabled={item.disabled}
                            className={cn(itemClass, item.danger ? "text-destructive [&_svg]:text-destructive" : "[&_svg]:text-muted-foreground")}
                            onClick={() => {
                                ref.current?.hidePopover();
                                item.onSelect();
                            }}
                        >
                            {item.checked !== undefined ? (
                                <Check className={item.checked ? "text-primary!" : "invisible"} />
                            ) : item.image ? (
                                <img src={item.image} alt="" draggable={false} className="size-4 shrink-0" />
                            ) : (
                                item.icon && <item.icon />
                            )}
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
