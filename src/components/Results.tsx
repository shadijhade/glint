import { useDeferredValue, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import {
    AlertTriangle,
    ChevronRight,
    ChevronsDownUp,
    ChevronsUpDown,
    Copy,
    Download,
    ExternalLink,
    FileText,
    Fingerprint,
    FolderInput,
    FolderSearch,
    Loader2,
    Lock,
    RotateCw,
    Search as SearchIcon,
    SearchX,
    X,
} from "lucide-react";
import { keyboardElsewhere } from "@/lib/explorer";
import { formatCount } from "@/lib/format";
import { RgMatch } from "@/lib/ripgrep";
import { useStore } from "@/lib/store";
import { segments } from "@/lib/text";
import { rowHeight } from "@/hooks/useTheme";
import { cn } from "@/lib/utils";
import type { SearchRun } from "@/App";
import { inspectLocks } from "./explorer/LocksDialog";
import { inspectXRay } from "./explorer/XRayDialog";
import { useMenu } from "./Menu";
import { VirtualList } from "./VirtualList";
import { Preview } from "./Preview";
import { Button } from "./ui/button";

type Row =
    | { type: "file"; path: string; count: number; first: number }
    | { type: "match"; index: number };

const fmt = formatCount;
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
    /** Keyboard navigation only while text search is the visible mode. */
    active: boolean;
    run: SearchRun;
    matches: RgMatch[];
    searching: boolean;
    error: string;
    duration: number;
    filesSearched: number | null;
    /** A Documents search's documents it couldn't read. */
    unreadable: number;
    selected: number;
    onSelect: (index: number) => void;
    onOpen: (path: string) => void;
    onReveal: (path: string) => void;
    onShowAll: () => void;
    /** The same search again, or (`everything`) skipping nothing. */
    onSearchAgain: (everything: boolean) => void;
    /** Back to the start screen. */
    onClose: () => void;
}

export function Results({ active, run, matches, searching, error, duration, filesSearched, unreadable, selected, onSelect, onOpen, onReveal, onShowAll, onSearchAgain, onClose }: ResultsProps) {
    const fontSize = useStore((s) => s.settings.fontSize);
    const exclusions = useStore((s) => s.settings.exclusions);
    const [filter, setFilter] = useState("");
    const [activeExts, setActiveExts] = useState<string[]>([]);
    const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
    const deferredFilter = useDeferredValue(filter.toLowerCase());

    useEffect(() => {
        setCollapsed(new Set());
    }, [run]);

    // Group contiguous matches by file (rg emits each file's matches together) and apply filters.
    const { rows, fileCount, exts, filePaths, maxCount } = useMemo(() => {
        const rows: Row[] = [];
        const extCounts = new Map<string, number>();
        const filePaths: string[] = [];
        let fileCount = 0;
        let maxCount = 0;
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
                filePaths.push(path);
            }
            header.count++;
            if (header.count > maxCount) maxCount = header.count;
            if (!collapsed.has(path)) {
                rows.push({ type: "match", index });
            }
        });
        const exts = [...extCounts].sort((a, b) => b[1] - a[1]).slice(0, 4);
        return { rows, fileCount, exts, filePaths, maxCount };
    }, [matches, activeExts, deferredFilter, collapsed]);

    const toggleFile = (path: string) => {
        setCollapsed((prev) => {
            const next = new Set(prev);
            if (next.has(path)) next.delete(path);
            else next.add(path);
            return next;
        });
    };

    const allCollapsed = filePaths.length > 0 && collapsed.size >= filePaths.length;
    const toggleCollapseAll = () => {
        setCollapsed(allCollapsed ? new Set() : new Set(filePaths));
    };

    const selectedRow = useMemo(() => rows.findIndex((r) => r.type === "match" && r.index === selected), [rows, selected]);
    const selectedMatch = matches[selected];

    // ↑/↓ walk the visible matches, Enter opens — unless the user is typing somewhere.
    useEffect(() => {
        if (!active) return;
        const onKey = (e: KeyboardEvent) => {
            const target = e.target as Element;
            if (target.closest("input, textarea, dialog, [popover]")) return;
            if (keyboardElsewhere(target, document.querySelector("[data-results]"))) return;
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                // Handed over from the search box (the page sent it): onto the match on show, if there's one.
                if (!e.isTrusted && selectedRow >= 0) return;
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
    }, [active, rows, selectedRow, selectedMatch, onSelect, onOpen]);

    const exportResults = async () => {
        const visible = rows.flatMap((r) => (r.type === "match" ? [matches[r.index]] : []));
        const file = await save({ defaultPath: "search-results.csv", filters: [{ name: "CSV", extensions: ["csv"] }, { name: "JSON", extensions: ["json"] }] });
        if (!file) return;
        // A document's matches say where they are (p. 3) rather than a line.
        const content = file.toLowerCase().endsWith(".json")
            ? JSON.stringify(visible, null, 2)
            : `File,${run.options.documents ? "Where" : "Line"},Content\n` + visible.map((m) => [m.data.path?.text ?? "", m.data.label ?? m.data.line_number ?? "", (m.data.lines?.text ?? "").trim()].map(csv).join(",")).join("\n");
        try {
            await invoke("write_file_content", { path: file, content });
            toast.success(`Exported ${fmt(visible.length)} results`, { description: file });
        } catch (e) {
            toast.error("Couldn't export the results", { description: String(e) });
        }
    };

    const limited = !searching && run.limit !== null && matches.length >= run.limit;
    const relative = (path: string) => (path.startsWith(run.path) ? path.slice(run.path.length).replace(/^[\\/]/, "") : path);

    // The right-click menu Files has too.
    const menu = useMenu();
    const openMenu = (e: React.MouseEvent, path: string, match?: RgMatch) => {
        e.preventDefault();
        const line = match?.data.lines?.text.trim();
        menu.open(e, [
            { label: "Open file", icon: ExternalLink, onSelect: () => onOpen(path) },
            { label: "Show in Files", icon: FolderInput, onSelect: () => onReveal(path) },
            { label: "Show in File Explorer", icon: FolderSearch, onSelect: () => void revealItemInDir(path).catch((e) => toast.error("Couldn't open File Explorer", { description: String(e) })) },
            "separator",
            { label: "Copy path", icon: Copy, onSelect: () => copy(path, "path") },
            { label: "Copy line", icon: Copy, disabled: line === undefined, onSelect: () => copy(line ?? "", "line") },
            "separator",
            { label: "File X-Ray", icon: Fingerprint, onSelect: () => inspectXRay(path) },
            { label: "What's locking this?", icon: Lock, onSelect: () => inspectLocks([path]) },
        ]);
    };
    const copy = (text: string, what: string) =>
        navigator.clipboard.writeText(text).then(() => toast.success(`Copied ${what}`), () => toast.error(`Couldn't copy ${what}`));

    const renderRow = (i: number) => {
        const row = rows[i];
        if (row.type === "file") {
            const isCollapsed = collapsed.has(row.path);
            const pct = maxCount > 0 ? Math.max(8, Math.round((row.count / maxCount) * 100)) : 0;
            return (
                <div
                    onClick={() => {
                        toggleFile(row.path);
                        onSelect(row.first);
                    }}
                    onDoubleClick={() => onOpen(row.path)}
                    onContextMenu={(e) => openMenu(e, row.path)}
                    className="flex h-full cursor-default items-center gap-1.5 bg-muted/50 pl-2 pr-3 hover:bg-accent"
                    title={`${row.path}\nClick to ${isCollapsed ? "expand" : "collapse"} matches · Double-click to open`}
                >
                    <ChevronRight className={cn("size-3.5 shrink-0 text-muted-foreground transition-transform duration-150", !isCollapsed && "rotate-90")} />
                    <FileText className="size-4 shrink-0 text-muted-foreground" />
                    <span className="truncate text-sm">
                        <span className="font-medium">{basename(row.path)}</span>
                        <span className="ml-2 text-xs text-muted-foreground">{relative(row.path).slice(0, -basename(row.path).length)}</span>
                    </span>
                    <span className="relative ml-auto flex h-5 min-w-9 shrink-0 items-center justify-end overflow-hidden rounded-full bg-background px-2 text-xs tabular-nums text-muted-foreground">
                        {pct > 0 && (
                            <span
                                aria-hidden
                                className="pointer-events-none absolute inset-0 origin-right bg-primary/15 transition-transform duration-150 dark:bg-primary/20"
                                style={{ transform: `scaleX(${pct / 100})` }}
                            />
                        )}
                        <span className="relative">{fmt(row.count)}</span>
                    </span>
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
                {match.data.label !== undefined ? (
                    <span className="w-24 shrink-0 truncate text-right text-xs tabular-nums text-muted-foreground/70" title={match.data.label}>{match.data.label}</span>
                ) : (
                    <span className="w-12 shrink-0 text-right font-mono text-xs tabular-nums text-muted-foreground/70">{match.data.line_number}</span>
                )}
                {/* A document's text is prose: in the sans font. */}
                <span className={cn("min-w-0 truncate text-[0.8125rem]", match.data.label === undefined && "font-mono")}>
                    {segments(match).map((p, k) =>
                        p.hit ? <mark key={k} className="rounded-sm bg-primary/20 px-px font-semibold text-pink-700 dark:text-pink-300">{p.text}</mark> : p.text
                    )}
                </span>
                {match.data.replacement !== undefined && (
                    <span className="max-w-[45%] shrink-0 truncate font-mono text-[0.8125rem] text-emerald-700 dark:text-emerald-400" title={`Becomes: ${match.data.replacement}`}>
                        → {match.data.replacement.replace(/\s*\n\s*/g, " ")}
                    </span>
                )}
            </div>
        );
    };


    let body: React.ReactNode;
    if (error) {
        body = (
            <Centered>
                <AlertTriangle className="size-6 text-destructive" />
                <p className="font-medium text-foreground">The search failed</p>
                <pre className="max-w-full select-text whitespace-pre-wrap text-left font-mono text-xs">{error}</pre>
                <Button variant="outline" size="sm" onClick={() => onSearchAgain(!!run.everything)}>
                    <RotateCw /> Try again
                </Button>
            </Centered>
        );
    } else if (!matches.length) {
        body = searching ? (
            <Centered><Loader2 className="size-5 animate-spin text-primary" /> Searching…</Centered>
        ) : (
            <Centered>
                <SearchX className="size-6" />
                <p className="font-medium text-foreground">No results</p>
                {run.options.documents ? (
                    <p className="max-w-sm">
                        {filesSearched === 0
                            ? `No ${run.options.meaning ? "documents or notes" : "PDF, Word, Excel, PowerPoint, OpenDocument or EPUB files"} in “${run.path}”. `
                            : run.options.meaning
                              ? "Nothing there is close enough in meaning: try saying it another way. "
                              : ""}
                        {unreadable > 0 && `${fmt(unreadable)} couldn't be read. `}
                        {run.everything ? "Only files kept in the cloud were skipped." : "Skipped: hidden files and folders, files kept in the cloud, and the folders excluded in Settings."}
                    </p>
                ) : (
                    <p className="max-w-sm">
                        {filesSearched === 0 ? `Nothing was searched: check that “${run.path}” exists and isn't skipped. ` : ""}
                        {run.everything ? "Nothing was skipped." : `Skipped: ${[...exclusions, "hidden files", "what .gitignore files list"].join(", ")}.`}
                    </p>
                )}
                {!run.everything && (
                    <Button variant="outline" size="sm" onClick={() => onSearchAgain(true)}>
                        <FolderSearch /> Search again, skipping nothing
                    </Button>
                )}
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
        body = <VirtualList data-results count={rows.length} rowHeight={rowHeight(fontSize)} scrollToIndex={selectedRow}>{renderRow}</VirtualList>;
    }

    return (
        <div className="flex min-h-0 flex-1 flex-col">
            <div className="flex h-10 shrink-0 items-center gap-3 border-b bg-chrome px-3 text-sm">
                {searching && <Loader2 className="size-4 shrink-0 animate-spin text-primary" />}
                <span className="truncate text-muted-foreground">
                    <b className="font-semibold tabular-nums text-foreground">{fmt(matches.length)}</b> {matches.length === 1 ? "result" : "results"} in{" "}
                    <b className="font-semibold tabular-nums text-foreground">{fmt(fileCount)}</b> {fileCount === 1 ? "file" : "files"}
                    {!searching && duration > 0 && ` · ${duration < 1000 ? `${Math.round(duration)} ms` : `${(duration / 1000).toFixed(2)} s`}`}
                    {run.options.documents && filesSearched !== null && ` · ${fmt(filesSearched)} ${filesSearched === 1 ? "document" : "documents"} read`}
                </span>
                {unreadable > 0 && (
                    <span className="shrink-0 rounded-md bg-amber-500/10 px-2 py-0.5 text-xs font-medium text-amber-800 dark:text-amber-400" title="Damaged, locked with a password, over 256 MB, or still being read after 30 seconds">
                        {fmt(unreadable)} couldn't be read
                    </span>
                )}
                {limited && (
                    <button onClick={onShowAll} className="shrink-0 rounded-md bg-amber-500/10 px-2 py-0.5 text-xs font-medium text-amber-800 hover:bg-amber-500/20 dark:text-amber-400" title="Run this search again without the result limit">
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
                            className="fluent-field h-7 w-44 pl-7 pr-6 text-xs outline-hidden placeholder:text-muted-foreground"
                        />
                        {filter && (
                            <button onClick={() => setFilter("")} className="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground" aria-label="Clear filter">
                                <X className="size-3.5" />
                            </button>
                        )}
                    </div>
                    {filePaths.length > 1 && (
                        <Button
                            variant="ghost"
                            size="icon"
                            className="size-7"
                            onClick={toggleCollapseAll}
                            title={allCollapsed ? "Expand all files" : "Collapse all files"}
                            aria-label={allCollapsed ? "Expand all files" : "Collapse all files"}
                        >
                            {allCollapsed ? <ChevronsUpDown /> : <ChevronsDownUp />}
                        </Button>
                    )}
                    <Button variant="ghost" size="icon" className="size-7" onClick={exportResults} disabled={!rows.length} title="Export results (CSV or JSON)">
                        <Download />
                    </Button>
                    <Button variant="ghost" size="sm" onClick={onClose} title="Close the results (back to recent searches)">
                        <X /> Close
                    </Button>
                </div>
            </div>

            <div className="flex min-h-0 flex-1">
                <div className="min-w-0 flex-1">{body}</div>
                {menu.element}
                {matches.length > 0 && (
                    <div className="hidden min-w-0 flex-1 border-l md:flex">
                        <Preview match={selectedMatch} onOpen={onOpen} onReveal={onReveal} />
                    </div>
                )}
            </div>
        </div>
    );
}
