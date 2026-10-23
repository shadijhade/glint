import { useEffect, useRef, useState } from "react";
import { History as HistoryIcon, X } from "lucide-react";
import { clearWithUndo, HistoryItem, useStore } from "@/lib/store";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Kbd } from "./ui/kbd";

export const HISTORY_POPOVER_ID = "history-popover";

/** "C:\a\b\c\d" → "…\c\d": the end of a path is the part worth seeing. */
const shortPath = (path: string) => {
    const sep = path.includes("\\") ? "\\" : "/";
    const parts = path.split(sep).filter(Boolean);
    return parts.length > 2 ? `…${sep}${parts.slice(-2).join(sep)}` : path;
};

function HistoryList({ items, onPick }: { items: HistoryItem[]; onPick: (item: HistoryItem) => void }) {
    const remove = useStore((s) => s.removeFromHistory);
    return (
        <ul className="space-y-px">
            {items.map((item) => (
                <li key={`${item.query}\0${item.path}`} className="group flex items-center rounded-md hover:bg-accent">
                    <button className="flex min-w-0 flex-1 items-center gap-3 px-2.5 py-1.5 text-left" onClick={() => onPick(item)} title={`${item.query}\n${item.path}`}>
                        <HistoryIcon className="size-4 shrink-0 text-muted-foreground" />
                        <span className="truncate font-mono text-sm">{item.query}</span>
                        <span className="ml-auto max-w-[50%] shrink-0 truncate text-xs text-muted-foreground">{shortPath(item.path)}</span>
                    </button>
                    <button
                        onClick={() => remove(item.query, item.path)}
                        className="mr-1 grid size-6 shrink-0 place-items-center rounded-sm text-muted-foreground opacity-0 hover:bg-background hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
                        title="Remove from history"
                        aria-label="Remove from history"
                    >
                        <X className="size-3.5" />
                    </button>
                </li>
            ))}
        </ul>
    );
}

/** Full, filterable history in a native popover anchored under the header button. */
export function HistoryPopover({ onPick }: { onPick: (item: HistoryItem) => void }) {
    const history = useStore((s) => s.history);
    const [filter, setFilter] = useState("");
    const ref = useRef<HTMLDivElement>(null);
    const inputRef = useRef<HTMLInputElement>(null);

    useEffect(() => {
        const popover = ref.current!;
        const onToggle = (e: Event) => (e as ToggleEvent).newState === "open" && inputRef.current?.focus();
        popover.addEventListener("toggle", onToggle);
        return () => popover.removeEventListener("toggle", onToggle);
    }, []);

    const q = filter.toLowerCase();
    // ponytail: renders at most 100 rows; the filter reaches older entries.
    const items = history.filter((h) => `${h.query}\n${h.path}`.toLowerCase().includes(q)).slice(0, 100);

    return (
        <div
            ref={ref}
            id={HISTORY_POPOVER_ID}
            popover="auto"
            className="fixed inset-auto right-3 top-12 m-0 w-[min(30rem,calc(100vw-1.5rem))] rounded-lg border bg-popover p-2 text-popover-foreground shadow-xl"
        >
            <Input ref={inputRef} value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Filter recent searches…" />
            <div className="mt-2 max-h-[60vh] overflow-y-auto">
                {items.length ? (
                    <HistoryList
                        items={items}
                        onPick={(item) => {
                            ref.current?.hidePopover();
                            onPick(item);
                        }}
                    />
                ) : (
                    <p className="py-6 text-center text-sm text-muted-foreground">{history.length ? "No matching searches" : "No searches yet"}</p>
                )}
            </div>
            {history.length > 0 && (
                <div className="mt-2 flex items-center justify-between border-t pt-2 text-xs text-muted-foreground">
                    <span className="pl-1">{history.length.toLocaleString()} {history.length === 1 ? "search" : "searches"}</span>
                    <Button variant="ghost" size="sm" onClick={() => clearWithUndo("history")}>Clear history</Button>
                </div>
            )}
        </div>
    );
}

const SHORTCUTS = [
    ["Enter", "Search, or open the result picked"],
    ["↓ ↑", "From the search box on through the results"],
    ["Shift+Enter", "A new line: search for several lines at once"],
    ["Alt+C / W / R", "Match case, whole words, regular expression"],
    ["Alt+A / D / O / M", "Code patterns, documents, pictures too, meaning"],
    ["Ctrl+H", "Replace"],
    ["Esc", "Leave the search box; again, stop the search"],
];

/** Start screen: recent searches and a keyboard cheat sheet. */
export function Home({ onPick }: { onPick: (item: HistoryItem) => void }) {
    const history = useStore((s) => s.history);
    return (
        <div className="flex flex-1 justify-center overflow-y-auto px-6 py-12">
            <div className="w-full max-w-2xl space-y-10">
                <div className="space-y-1.5 text-center">
                    <h1 className="text-xl font-semibold tracking-tight">Search across your files</h1>
                    <p className="text-sm text-muted-foreground">Choose a folder, type what you're looking for and press Enter.</p>
                </div>

                {history.length > 0 && (
                    <section>
                        <div className="mb-1.5 flex items-center justify-between px-2.5">
                            <h2 className="text-sm font-semibold">Recent searches</h2>
                            {history.length > 8 && (
                                <button className="text-xs text-muted-foreground hover:text-foreground" onClick={() => document.getElementById(HISTORY_POPOVER_ID)?.showPopover()}>
                                    View all {history.length.toLocaleString()}
                                </button>
                            )}
                        </div>
                        <HistoryList items={history.slice(0, 8)} onPick={onPick} />
                    </section>
                )}

                <dl className="mx-auto grid w-fit grid-cols-[auto_auto] items-center gap-x-3 gap-y-2 text-xs text-muted-foreground">
                    {SHORTCUTS.map(([keys, label]) => (
                        <div key={label} className="contents">
                            <dt className="text-right">
                                <Kbd className="text-foreground">{keys}</Kbd>
                            </dt>
                            <dd>{label}</dd>
                        </div>
                    ))}
                </dl>
            </div>
        </div>
    );
}
