import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openPath } from "@tauri-apps/plugin-opener";
import { File, Folder, Loader2, Search } from "lucide-react";
import { useTheme } from "@/hooks/useTheme";
import type { FileEntry } from "@/lib/entries";
import { formatCount, formatDuration, formatSize } from "@/lib/format";
import { cancelFind, findFiles, listDrives, quickHide, quickReveal } from "@/lib/fs";
import { parentPath } from "@/lib/paths";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";

const LANE = "quick";
const LIMIT = 100;

const searchable = () => listDrives().then((drives) => drives.filter((d) => d.kind === "fixed" || d.kind === "removable").map((d) => d.path));

/**
 * The quick search window (Settings → Quick search): names on every drive as you type, from the index
 * where it runs. Enter opens, Alt+Enter shows the item in Glint, Esc or a click elsewhere puts it away.
 */
export function QuickSearch() {
    useTheme();
    const [query, setQuery] = useState("");
    const [found, setFound] = useState<FileEntry[]>([]);
    const [status, setStatus] = useState("");
    const [busy, setBusy] = useState(false);
    const [active, setActive] = useState(0);
    const input = useRef<HTMLInputElement>(null);
    const list = useRef<HTMLDivElement>(null);
    const roots = useRef<string[]>([]);

    // Each time it's called up: the drives as they are now, and the last query selected, to type over.
    useEffect(() => {
        const ready = () => {
            searchable().then((r) => (roots.current = r), () => {});
            input.current?.focus();
            input.current?.select();
        };
        ready();
        const shown = listen("quick-shown", ready);
        const away = () => void quickHide();
        window.addEventListener("blur", away);
        return () => {
            shown.then((stop) => stop());
            window.removeEventListener("blur", away);
        };
    }, []);

    useEffect(() => {
        const text = query.trim();
        if (!text) {
            void cancelFind(LANE).catch(() => {});
            setFound([]);
            setStatus("");
            setBusy(false);
            return;
        }
        let current = true;
        const timer = setTimeout(async () => {
            const { settings } = useStore.getState();
            const entries: FileEntry[] = [];
            let engine = "";
            setBusy(true);
            setActive(0);
            try {
                if (!roots.current.length) roots.current = await searchable();
                const options = { hidden: settings.showHidden, respectIgnore: false, exclude: settings.findExclusions, maxResults: LIMIT };
                await findFiles(roots.current, text, options, LANE, (event) => {
                    if (!current) return;
                    if (event.type === "started") engine = event.engine;
                    else if (event.type === "entries") {
                        entries.push(...event.entries);
                        setFound(entries.slice());
                    } else if (event.type === "error") {
                        setBusy(false);
                        setStatus(event.message);
                    } else if (event.type === "finished") {
                        setBusy(false);
                        setFound(entries.slice());
                        setStatus(`${engine} · ${formatCount(event.count)}${event.limited ? "+" : ""} found in ${formatDuration(event.elapsed)}`);
                    }
                });
            } catch (e) {
                if (current) {
                    setBusy(false);
                    setStatus(String(e));
                }
            }
        }, 120);
        return () => {
            current = false;
            clearTimeout(timer);
        };
    }, [query]);

    useEffect(() => {
        list.current?.children[active]?.scrollIntoView({ block: "nearest" });
    }, [active]);

    // Folders open in Glint; files with their app, or in Glint with Alt.
    const open = (entry: FileEntry | undefined, inGlint: boolean) => {
        if (!entry) return;
        if (inGlint || entry.dir) return void quickReveal(entry.path, entry.dir);
        void quickHide();
        openPath(entry.path).catch(() => quickReveal(entry.path, false));
    };

    const onKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === "Escape") void quickHide();
        else if (e.key === "ArrowDown") setActive((i) => Math.min(i + 1, found.length - 1));
        else if (e.key === "ArrowUp") setActive((i) => Math.max(i - 1, 0));
        else if (e.key === "Enter") open(found[active], e.altKey);
        else return;
        e.preventDefault();
    };

    const hint = (keys: string, what: string) => (
        <span className="shrink-0">
            <kbd className="rounded border bg-muted px-1 font-sans text-[11px]">{keys}</kbd> {what}
        </span>
    );

    return (
        <div className="flex h-screen flex-col overflow-hidden border bg-background text-foreground">
            <div data-tauri-drag-region className="flex shrink-0 items-center gap-3 border-b px-4">
                {busy ? <Loader2 className="size-5 shrink-0 animate-spin text-primary" /> : <Search className="size-5 shrink-0 text-muted-foreground" />}
                <input
                    ref={input}
                    value={query}
                    onChange={(e) => setQuery(e.target.value)}
                    onKeyDown={onKeyDown}
                    spellCheck={false}
                    aria-label="Search file names on every drive"
                    placeholder="Search every drive…"
                    className="h-14 min-w-0 flex-1 bg-transparent text-lg outline-hidden placeholder:text-muted-foreground"
                />
            </div>
            <div ref={list} role="listbox" aria-label="Results" className="min-h-0 flex-1 overflow-y-auto p-1.5">
                {found.map((entry, i) => (
                    <button
                        key={entry.path}
                        role="option"
                        aria-selected={i === active}
                        tabIndex={-1}
                        onMouseMove={() => setActive(i)}
                        onClick={(e) => open(entry, e.altKey)}
                        className={cn("flex w-full items-center gap-3 rounded-md px-3 py-1.5 text-left", i === active && "bg-accent")}
                    >
                        {entry.dir ? <Folder className="size-4 shrink-0 fill-amber-400/80 text-amber-500" /> : <File className="size-4 shrink-0 text-muted-foreground" />}
                        <span className="min-w-0 flex-1">
                            <span className="block truncate text-sm">{entry.name}</span>
                            <span className="block truncate text-xs text-muted-foreground">{parentPath(entry.path)}</span>
                        </span>
                        {!entry.dir && <span className="shrink-0 text-xs tabular-nums text-muted-foreground">{formatSize(entry.size)}</span>}
                    </button>
                ))}
                {!found.length && (
                    <p className="px-3 py-8 text-center text-sm text-muted-foreground">
                        {query.trim() ? (busy ? "Searching…" : status || "Nothing found") : "Type part of a name. Filters work too: ext:pdf, size:>10mb, dm:7d"}
                    </p>
                )}
            </div>
            <div className="flex h-8 shrink-0 items-center gap-4 border-t px-4 text-xs text-muted-foreground">
                <span className="min-w-0 flex-1 truncate">{found.length ? status : ""}</span>
                {hint("Enter", "open")}
                {hint("Alt+Enter", "show in Glint")}
                {hint("Esc", "close")}
            </div>
        </div>
    );
}
