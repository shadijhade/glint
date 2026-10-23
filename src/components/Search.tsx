import { useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { CaseSensitive, FolderOpen, Regex, Replace, Search as SearchIcon, Square, TextSearch, WholeWord } from "lucide-react";
import { defaultTextSearchFolder } from "@/lib/explorer";
import { SearchOptions, useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { ToolNeeded } from "./ToolNeeded";
import { Button } from "./ui/button";

const TOGGLES: { key: keyof SearchOptions; hotkey: string; icon: typeof Regex; label: string }[] = [
    { key: "caseSensitive", hotkey: "c", icon: CaseSensitive, label: "Match case (Alt+C)" },
    { key: "wholeWord", hotkey: "w", icon: WholeWord, label: "Whole word (Alt+W)" },
    { key: "regex", hotkey: "r", icon: Regex, label: "Regular expression (Alt+R)" },
];

const fieldClass = "h-9 min-w-0 flex-1 bg-transparent px-2 text-sm outline-hidden placeholder:text-muted-foreground";

function Toggle({ pressed, className, ...props }: React.ComponentProps<"button"> & { pressed: boolean }) {
    return (
        <button
            type="button"
            aria-pressed={pressed}
            className={cn(
                "grid size-7 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground [&_svg]:size-4",
                pressed && "bg-primary/15 text-primary hover:bg-primary/20 hover:text-primary",
                className
            )}
            {...props}
        />
    );
}

interface SearchBarProps {
    searching: boolean;
    canReplace: boolean;
    onSearch: () => void;
    onStop: () => void;
    onReplace: (replacement: string) => void;
}

export function SearchBar({ searching, canReplace, onSearch, onStop, onReplace }: SearchBarProps) {
    const { query, setQuery, path, setPath, options, setOption, history } = useStore();
    const [replaceOpen, setReplaceOpen] = useState(false);
    const [replacement, setReplacement] = useState("");
    const queryRef = useRef<HTMLTextAreaElement>(null);
    const recentPaths = useMemo(() => [...new Set(history.map((h) => h.path))].slice(0, 20), [history]);

    useEffect(() => {
        if (!path.trim()) {
            const fallback = defaultTextSearchFolder();
            if (fallback) setPath(fallback);
        }
        // Ctrl+F / Ctrl+K focus the query, Ctrl+H toggles replace — from anywhere.
        const onKey = (e: KeyboardEvent) => {
            if (!(e.ctrlKey || e.metaKey)) return;
            const key = e.key.toLowerCase();
            if (key === "f" || key === "k") {
                e.preventDefault();
                queryRef.current?.focus();
                queryRef.current?.select();
            } else if (key === "h") {
                e.preventDefault();
                setReplaceOpen((open) => !open);
            }
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, []);

    const browse = async () => {
        const dir = await open({ directory: true, defaultPath: path || undefined });
        if (typeof dir === "string") setPath(dir);
    };

    const onQueryKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
        const toggle = e.altKey && TOGGLES.find((t) => t.hotkey === e.key.toLowerCase());
        if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            onSearch();
        } else if (e.key === "Escape") {
            e.currentTarget.blur();
        } else if (e.key === "ArrowDown" && !e.shiftKey && !e.currentTarget.value.slice(e.currentTarget.selectionEnd).includes("\n")) {
            // From the last line, on to the results, as from Files' search box. Sent from the page
            // itself: the results' handler wants an element as the target.
            e.preventDefault();
            e.currentTarget.blur();
            document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
        } else if (toggle) {
            e.preventDefault();
            setOption(toggle.key, !options[toggle.key]);
        }
    };

    return (
        <div className="flex min-w-0 flex-1 items-start gap-2">
            <div className="fluent-field min-w-0 flex-1 divide-y overflow-hidden">
                <div className="flex items-start pr-1">
                    <SearchIcon className="ml-3 mt-2.5 size-4 shrink-0 text-muted-foreground" />
                    <textarea
                        ref={queryRef}
                        rows={1}
                        autoFocus
                        spellCheck={false}
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        onKeyDown={onQueryKeyDown}
                        placeholder="Search for text…  (Shift+Enter adds a line)"
                        aria-label="Search query"
                        className={cn(fieldClass, "h-auto max-h-40 min-h-9 resize-none py-2 font-mono placeholder:font-sans field-sizing-content")}
                    />
                    <div className="flex items-center gap-0.5 pt-1">
                        {TOGGLES.map(({ key, icon: Icon, label }) => (
                            <Toggle key={key} pressed={options[key]} onClick={() => setOption(key, !options[key])} title={label} aria-label={label}>
                                <Icon />
                            </Toggle>
                        ))}
                        <span className="mx-1 h-4 w-px bg-border" />
                        <Toggle pressed={replaceOpen} onClick={() => setReplaceOpen(!replaceOpen)} title="Replace (Ctrl+H)" aria-label="Toggle replace">
                            <Replace />
                        </Toggle>
                    </div>
                </div>

                {replaceOpen && (
                    <div className="flex items-center pr-1">
                        <Replace className="ml-3 size-4 shrink-0 text-muted-foreground" />
                        <input
                            spellCheck={false}
                            value={replacement}
                            onChange={(e) => setReplacement(e.target.value)}
                            onKeyDown={(e) => e.key === "Enter" && canReplace && onReplace(replacement)}
                            placeholder={options.regex ? "Replace with…  ($1 inserts a capture group)" : "Replace with…"}
                            aria-label="Replacement"
                            className={cn(fieldClass, "font-mono placeholder:font-sans")}
                        />
                        <Button size="sm" variant="ghost" disabled={!canReplace} onClick={() => onReplace(replacement)} title="Replace in every file of the current results">
                            Replace all
                        </Button>
                    </div>
                )}

                <div className="flex items-center pr-1">
                    <FolderOpen className="ml-3 size-4 shrink-0 text-muted-foreground" />
                    <input
                        spellCheck={false}
                        list="recent-paths"
                        value={path}
                        onChange={(e) => setPath(e.target.value)}
                        onKeyDown={(e) => e.key === "Enter" && onSearch()}
                        placeholder="Folder to search…"
                        aria-label="Folder to search"
                        className={fieldClass}
                    />
                    <datalist id="recent-paths">
                        {recentPaths.map((p) => <option key={p} value={p} />)}
                    </datalist>
                    <Button size="sm" variant="ghost" onClick={browse}>Browse…</Button>
                </div>
            </div>

            {searching ? (
                <Button variant="outline" onClick={onStop} title="Stop (Esc)">
                    <Square className="fill-current size-3!" /> Stop
                </Button>
            ) : (
                <Button onClick={onSearch} disabled={!query} title="Search (Enter)">
                    <SearchIcon /> Search
                </Button>
            )}
        </div>
    );
}

/**
 * Shown when the ripgrep Text search runs is missing (a security tool quarantined it, a portable copy):
 * one click installs it with winget, and Glint uses that copy.
 */
export function RipgrepCard() {
    return <ToolNeeded id="ripgrep" icon={TextSearch} why="ripgrep, which searches inside files, is missing from this copy of Glint." className="mx-3 mt-3" />;
}
