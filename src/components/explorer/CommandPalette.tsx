import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { CornerDownLeft, type LucideIcon, Search } from "lucide-react";
import { fuzzyScore } from "@/lib/rank";
import { cn } from "@/lib/utils";

export interface Command {
    id: string;
    label: string;
    /** Shown dimmed after the label: a folder's path. */
    detail?: string;
    shortcut?: string;
    icon: LucideIcon;
    iconClass?: string;
    kind: "folder" | "action";
    /** Ranking boost, e.g. how often a folder is opened. */
    weight?: number;
    run: () => void;
}

/** Typed text that's a location rather than a search: C:\…, \\server, ~, %APPDATA%, .\… */
const looksLikePath = (text: string) => /^([a-z]:[\\/]?|\\\\|~|%|\.{1,2}[\\/])/i.test(text.trim());

/** Ctrl+K: find any action or a folder you've been to by typing a few of its letters. */
export function CommandPalette({ commands, onGoTo, onClose }: { commands: Command[]; onGoTo: (text: string) => void; onClose: () => void }) {
    const [query, setQuery] = useState("");
    const [active, setActive] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    useLayoutEffect(() => inputRef.current?.focus(), []);

    const results = useMemo(() => {
        const q = query.trim();
        if (!q) {
            // Nothing typed yet: the folders you use most, then every action.
            const folders = commands.filter((c) => c.kind === "folder").sort((a, b) => (b.weight ?? 0) - (a.weight ?? 0)).slice(0, 6);
            return [...folders, ...commands.filter((c) => c.kind === "action")];
        }
        const scored = commands.flatMap((c) => {
            // A match in the label counts more than one only in a folder's path.
            const byLabel = fuzzyScore(q, c.label);
            const byDetail = c.detail ? fuzzyScore(q, c.detail) : null;
            const score = byLabel !== null ? byLabel + 2 : byDetail !== null ? byDetail - 1 : null;
            return score === null ? [] : [{ c, score: score + (c.weight ?? 0) }];
        });
        scored.sort((a, b) => b.score - a.score);
        const found = scored.slice(0, 40).map((s) => s.c);
        if (!looksLikePath(q)) return found;
        const go: Command = { id: "go", label: `Go to ${q}`, icon: CornerDownLeft, kind: "action", run: () => onGoTo(q) };
        return [go, ...found];
    }, [commands, query, onGoTo]);

    useEffect(() => setActive(0), [query]);
    useEffect(() => {
        listRef.current?.children[active]?.scrollIntoView({ block: "nearest" });
    }, [active]);

    const run = (command: Command | undefined) => {
        if (!command) return;
        onClose();
        command.run();
    };

    const onKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            const step = e.key === "ArrowDown" ? 1 : -1;
            setActive((a) => (a + step + results.length) % Math.max(results.length, 1));
        } else if (e.key === "Enter") {
            e.preventDefault();
            run(results[active]);
        } else if (e.key === "Escape") {
            e.preventDefault();
            onClose();
        }
    };

    return (
        <div className="fixed inset-0 z-40 flex justify-center bg-black/30 px-4 pt-[12vh]" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
            <div role="dialog" aria-label="Command palette" className="h-fit w-full max-w-[40rem] overflow-hidden rounded-xl border bg-popover text-popover-foreground shadow-2xl">
                <div className="flex items-center gap-2.5 border-b px-3.5">
                    <Search className="size-4 shrink-0 text-muted-foreground" />
                    <input
                        ref={inputRef}
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        onKeyDown={onKeyDown}
                        spellCheck={false}
                        placeholder="Type a folder, a path or a command…"
                        aria-label="Command or folder"
                        className="h-12 min-w-0 flex-1 bg-transparent text-sm outline-hidden placeholder:text-muted-foreground"
                    />
                    <kbd className="rounded-sm border bg-card px-1.5 py-0.5 font-sans text-[0.6875rem] text-muted-foreground">Esc</kbd>
                </div>
                <div ref={listRef} role="listbox" className="max-h-[min(60vh,26rem)] overflow-y-auto p-1.5">
                    {results.map((c, i) => (
                        <button
                            key={c.id}
                            role="option"
                            aria-selected={i === active}
                            onMouseMove={() => i !== active && setActive(i)}
                            onClick={() => run(c)}
                            className={cn("flex w-full items-center gap-3 rounded-md px-2.5 py-2 text-left text-sm", i === active && "bg-accent")}
                        >
                            <c.icon className={cn("size-4 shrink-0 text-muted-foreground", c.iconClass)} />
                            <span className="shrink-0 truncate">{c.label}</span>
                            {c.detail && <span className="min-w-0 truncate text-xs text-muted-foreground">{c.detail}</span>}
                            {c.shortcut && <span className="ml-auto shrink-0 pl-4 text-xs text-muted-foreground">{c.shortcut}</span>}
                        </button>
                    ))}
                    {!results.length && <p className="px-3 py-6 text-center text-sm text-muted-foreground">Nothing matches “{query}”</p>}
                </div>
            </div>
        </div>
    );
}
