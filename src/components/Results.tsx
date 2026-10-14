import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { AlertTriangle, Copy, Download, ExternalLink, FileText, FolderOpen, Loader2, Search as SearchIcon, SearchX, X } from "lucide-react";
import { RgMatch } from "@/lib/ripgrep";
import { useStore } from "@/lib/store";
import { segments } from "@/lib/text";
import { rowHeight } from "@/hooks/useTheme";
import { cn } from "@/lib/utils";
import type { SearchRun } from "@/App";
import { VirtualList } from "./VirtualList";
import { Preview } from "./Preview";
import { Button } from "./ui/button";

type Row =
    | { type: "file"; path: string; count: number; first: number }
    | { type: "match"; index: number };

const fmt = (n: number) => n.toLocaleString();
const basename = (path: string) => path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1);
const extOf = (path: string) => {
    const name = basename(path);
    const dot = name.lastIndexOf(".");
    return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
};
const csv = (value: unknown) => `"${String(value).replace(/"/g, '""')}"`;

function Centered({ children }: { children: React.ReactNode }) {
    return <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-sm text-muted-foreground">{children}</div>;
}

interface ResultsProps {
    run: SearchRun;
    matches: RgMatch[];
    searching: boolean;
    error: string;
    duration: number;
    filesSearched: number | null;
    selected: number;
    onSelect: (index: number) => void;
    onOpen: (path: string) => void;
    onReveal: (path: string) => void;
    onShowAll: () => void;
}

export function Results({ run, matches, searching, error, duration, filesSearched, selected, onSelect, onOpen, onReveal, onShowAll }: ResultsProps) {
    const fontSize = useStore((s) => s.settings.fontSize);
    const [filter, setFilter] = useState("");
    const [activeExts, setActiveExts] = useState<string[]>([]);
    const deferredFilter = useDeferredValue(filter.toLowerCase());

    // Group contiguous matches by file (rg emits each file's matches together) and apply filters.
    const { rows, fileCount, exts } = useMemo(() => {
        const rows: Row[] = [];
        const extCounts = new Map<string, number>();
        let fileCount = 0;
        let lastPath = "";
        let ext = "";
        let header: Extract<Row, { type: "file" }> | null = null;
        matches.forEach((m, index) => {
            const path = m.data.path?.text ?? "";
            if (path !== lastPath) {
                lastPath = path;
                ext = extOf(path);
                fileCount++;
            }
            extCounts.set(ext, (extCounts.get(ext) ?? 0) + 1);
            if (activeExts.length && !activeExts.includes(ext)) return;
            if (deferredFilter && !(m.data.lines?.text ?? "").toLowerCase().includes(deferredFilter) && !path.toLowerCase().includes(deferredFilter)) return;
            if (header?.path !== path) {
                header = { type: "file", path, count: 0, first: index };
                rows.push(header);
            }
            header.count++;
            rows.push({ type: "match", index });
        });
        const exts = [...extCounts].sort((a, b) => b[1] - a[1]).slice(0, 4);
        return { rows, fileCount, exts };
    }, [matches, activeExts, deferredFilter]);

    const selectedRow = useMemo(() => rows.findIndex((r) => r.type === "match" && r.index === selected), [rows, selected]);
    const selectedMatch = matches[selected];

    // ↑/↓ walk the visible matches, Enter opens — unless the user is typing somewhere.
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            const target = e.target as Element;
            if (target.closest("input, textarea, dialog, [popover]")) return;
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                const step = e.key === "ArrowDown" ? 1 : -1;
                for (let i = selectedRow + step; i >= 0 && i < rows.length; i += step) {
                    const row = rows[i];
                    if (row.type === "match") return onSelect(row.index);
                }
            } else if (e.key === "Enter" && selectedMatch?.data.path && !target.closest("button, a")) {
                e.preventDefault();
                onOpen(selectedMatch.data.path.text);
            }
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, [rows, selectedRow, selectedMatch, onSelect, onOpen]);

    const exportResults = async () => {
        const visible = rows.flatMap((r) => (r.type === "match" ? [matches[r.index]] : []));
        const file = await save({ defaultPath: "search-results.csv", filters: [{ name: "CSV", extensions: ["csv"] }, { name: "JSON", extensions: ["json"] }] });
        if (!file) return;
        const content = file.toLowerCase().endsWith(".json")
            ? JSON.stringify(visible, null, 2)
            : "File,Line,Content\n" + visible.map((m) => [m.data.path?.text ?? "", m.data.line_number ?? "", (m.data.lines?.text ?? "").trim()].map(csv).join(",")).join("\n");
        try {
            await invoke("write_file_content", { path: file, content });
            toast.success(`Exported ${fmt(visible.length)} results`, { description: file });
        } catch (e) {
            toast.error("Export failed", { description: String(e) });
        }
    };

    const limited = !searching && run.limit !== null && matches.length >= run.limit;
    const relative = (path: string) => (path.startsWith(run.path) ? path.slice(run.path.length).replace(/^[\\/]/, "") : path);

    // One shared right-click menu: a native popover positioned at the cursor (light-dismiss and Esc for free).
    const menuRef = useRef<HTMLDivElement>(null);
    const [menuTarget, setMenuTarget] = useState<{ path: string; line?: string }>({ path: "" });
    const openMenu = (e: React.MouseEvent, path: string, match?: RgMatch) => {
        e.preventDefault();
        setMenuTarget({ path, line: match?.data.lines?.text.trim() });
        const menu = menuRef.current!;
        if (!menu.matches(":popover-open")) menu.showPopover();
        const { width, height } = menu.getBoundingClientRect();
        menu.style.left = `${Math.min(e.clientX, window.innerWidth - width - 4)}px`;
        menu.style.top = `${Math.min(e.clientY, window.innerHeight - height - 4)}px`;
    };
    const menuAction = (action: () => void) => () => {
        menuRef.current?.hidePopover();
        action();
    };
    const copy = (text: string, what: string) =>
        navigator.clipboard.writeText(text).then(() => toast.success(`Copied ${what}`), () => toast.error(`Couldn't copy ${what}`));

    const renderRow = (i: number) => {
        const row = rows[i];
        if (row.type === "file") {
            return (
                <div
                    onClick={() => onSelect(row.first)}
                    onDoubleClick={() => onOpen(row.path)}
                    onContextMenu={(e) => openMenu(e, row.path)}
                    className="flex h-full cursor-default items-center gap-2 bg-muted/50 pl-3 pr-3 hover:bg-accent"
                    title={row.path}
                >
                    <FileText className="size-4 shrink-0 text-muted-foreground" />
                    <span className="truncate text-sm">
                        <span className="font-medium">{basename(row.path)}</span>
                        <span className="ml-2 text-xs text-muted-foreground">{relative(row.path).slice(0, -basename(row.path).length)}</span>
                    </span>
                    <span className="ml-auto shrink-0 rounded-full bg-background px-1.5 text-xs tabular-nums text-muted-foreground">{row.count}</span>
                </div>
            );
        }
        const match = matches[row.index];
        const path = match.data.path?.text ?? "";
        return (
            <div
                onClick={() => onSelect(row.index)}
                onDoubleClick={() => onOpen(path)}
                onContextMenu={(e) => {
                    onSelect(row.index);
                    openMenu(e, path, match);
                }}
                className={cn(
                    "relative flex h-full cursor-default items-center gap-2 pl-4 pr-3 hover:bg-accent/60",
                    i === selectedRow && "bg-primary/10 before:absolute before:inset-y-0 before:left-0 before:w-0.5 before:bg-primary hover:bg-primary/15"
                )}
            >
                <span className="w-12 shrink-0 text-right font-mono text-xs tabular-nums text-muted-foreground/70">{match.data.line_number}</span>
                <span className="truncate font-mono text-[0.8125rem]">
                    {segments(match).map((p, k) =>
                        p.hit ? <mark key={k} className="rounded-sm bg-primary/20 px-px font-semibold text-pink-700 dark:text-pink-300">{p.text}</mark> : p.text
                    )}
                </span>
            </div>
        );
    };

    const menuItem = "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent focus-visible:bg-accent focus-visible:outline-hidden disabled:pointer-events-none disabled:opacity-40 [&_svg]:size-4 [&_svg]:text-muted-foreground";
    const menu = (
        <div ref={menuRef} popover="auto" role="menu" className="fixed inset-auto m-0 min-w-[12rem] rounded-md border bg-popover p-1 text-popover-foreground shadow-lg">
            <button role="menuitem" className={menuItem} onClick={menuAction(() => onOpen(menuTarget.path))}><ExternalLink /> Open file</button>
            <button role="menuitem" className={menuItem} onClick={menuAction(() => onReveal(menuTarget.path))}><FolderOpen /> Reveal in folder</button>
            <div className="-mx-1 my-1 h-px bg-border" />
            <button role="menuitem" className={menuItem} onClick={menuAction(() => copy(menuTarget.path, "path"))}><Copy /> Copy path</button>
            <button role="menuitem" className={menuItem} disabled={menuTarget.line === undefined} onClick={menuAction(() => copy(menuTarget.line ?? "", "line"))}><Copy /> Copy line</button>
        </div>
    );

    let body: React.ReactNode;
    if (error) {
        body = (
            <Centered>
                <AlertTriangle className="size-6 text-destructive" />
                <p className="font-medium text-foreground">Search failed</p>
                <pre className="max-w-full select-text whitespace-pre-wrap text-left font-mono text-xs">{error}</pre>
            </Centered>
        );
    } else if (!matches.length) {
        body = searching ? (
            <Centered><Loader2 className="size-5 animate-spin text-primary" /> Searching…</Centered>
        ) : (
            <Centered>
                <SearchX className="size-6" />
                <p className="font-medium text-foreground">No results</p>
                <p className="max-w-sm">
                    {filesSearched === 0
                        ? `Nothing was searched — check that “${run.path}” exists and isn't excluded.`
                        : "Try different search options. Excluded patterns, .gitignore'd and hidden files are skipped."}
                </p>
            </Centered>
        );
    } else if (!rows.length) {
        body = (
            <Centered>
                No results match the filter.
                <Button variant="outline" size="sm" onClick={() => { setFilter(""); setActiveExts([]); }}>Clear filters</Button>
            </Centered>
        );
    } else {
        body = <VirtualList count={rows.length} rowHeight={rowHeight(fontSize)} scrollToIndex={selectedRow}>{renderRow}</VirtualList>;
    }

    return (
        <div className="flex min-h-0 flex-1 flex-col">
            <div className="flex h-10 shrink-0 items-center gap-3 border-b bg-card px-3 text-sm">
                {searching && <Loader2 className="size-4 shrink-0 animate-spin text-primary" />}
                <span className="truncate text-muted-foreground">
                    <b className="font-semibold tabular-nums text-foreground">{fmt(matches.length)}</b> {matches.length === 1 ? "result" : "results"} in{" "}
                    <b className="font-semibold tabular-nums text-foreground">{fmt(fileCount)}</b> {fileCount === 1 ? "file" : "files"}
                    {!searching && duration > 0 && ` · ${duration < 1000 ? `${Math.round(duration)} ms` : `${(duration / 1000).toFixed(2)} s`}`}
                </span>
                {limited && (
                    <button onClick={onShowAll} className="shrink-0 rounded-md bg-amber-500/10 px-2 py-0.5 text-xs font-medium text-amber-600 hover:bg-amber-500/20 dark:text-amber-400" title="Run this search again without the result limit">
                        Stopped at {fmt(run.limit!)} · Show all
                    </button>
                )}
                <div className="ml-auto flex items-center gap-1.5">
                    {exts.length > 1 && exts.map(([ext, count]) => {
                        const active = activeExts.includes(ext);
                        return (
                            <button
                                key={ext}
                                aria-pressed={active}
                                onClick={() => setActiveExts(active ? activeExts.filter((e) => e !== ext) : [...activeExts, ext])}
                                className={cn("hidden h-6 items-center gap-1 rounded-full border px-2 font-mono text-xs transition-colors lg:inline-flex", active ? "border-primary/40 bg-primary/15 text-primary" : "text-muted-foreground hover:bg-accent hover:text-foreground")}
                            >
                                {ext ? `.${ext}` : "no ext"} <span className="tabular-nums opacity-60">{fmt(count)}</span>
                            </button>
                        );
                    })}
                    <div className="relative">
                        <SearchIcon className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                        <input
                            value={filter}
                            onChange={(e) => setFilter(e.target.value)}
                            onKeyDown={(e) => e.key === "Escape" && setFilter("")}
                            placeholder="Filter results"
                            aria-label="Filter results"
                            spellCheck={false}
                            className="h-7 w-44 rounded-md border bg-background pl-7 pr-6 text-xs outline-hidden placeholder:text-muted-foreground focus:border-ring/60"
                        />
                        {filter && (
                            <button onClick={() => setFilter("")} className="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground" aria-label="Clear filter">
                                <X className="size-3.5" />
                            </button>
                        )}
                    </div>
                    <Button variant="ghost" size="icon" className="size-7" onClick={exportResults} disabled={!rows.length} title="Export results (CSV or JSON)">
                        <Download />
                    </Button>
                </div>
            </div>

            <div className="flex min-h-0 flex-1">
                <div className="min-w-0 flex-1">{body}</div>
                {menu}
                {matches.length > 0 && (
                    <div className="hidden min-w-0 flex-1 border-l md:flex">
                        <Preview match={selectedMatch} onOpen={onOpen} onReveal={onReveal} />
                    </div>
                )}
            </div>
        </div>
    );
}
