import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Check, Loader2, Search } from "lucide-react";
import { useTheme } from "@/hooks/useTheme";
import { type FileEntry, isHidden } from "@/lib/entries";
import { kindOf } from "@/lib/fileTypes";
import { formatCount, formatDuration, formatSize } from "@/lib/format";
import { cancelFind, findFiles, listDrives, openItem, quickHide, quickReveal, startDrag } from "@/lib/fs";
import { baseName, parentPath } from "@/lib/paths";
import { plainQuery } from "@/lib/plain";
import { typoDistance } from "@/lib/rank";
import { refreshZoxide, usedFolders, useZoxide } from "@/lib/zoxide";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Kbd } from "./ui/kbd";

const LANE = "quick";
/** Shown; more are asked for, so the best can be picked from them. */
const LIMIT = 100;
const ASKED = 1000;

/** Windows' own folders, and caches: what a search for your files rarely means. */
const SYSTEM = /\\(windows\\(winsxs|servicing|assembly|installer|system32|syswow64|softwaredistribution|systemapps|immersivecontrolpanel)|programdata|\$recycle\.bin|appdata\\local\\(temp|packages|microsoft)|node_modules|\.git)\\/i;

/** The words typed, without filters (ext:pdf…) and wildcards. */
const wordsOf = (query: string) =>
    query
        .toLowerCase()
        .split(/\s+/)
        .filter((w) => w && !w.includes(":"))
        .map((w) => w.replace(/[*?"]/g, ""))
        .filter(Boolean);

/**
 * Best first: a name that is what was typed, then one that starts with it, then one with a word that
 * does, then the index's close matches, fewest typos first; then what's outside Windows' own folders
 * and not hidden; then the shorter path.
 */
function rank(entries: FileEntry[], words: string[]) {
    const first = words[0] ?? "";
    const wordStart = new RegExp(`(^|[\\s._-])${first.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`);
    const score = (e: FileEntry) => {
        const stem = e.lower.replace(/\.[^.]*$/, "");
        const match = !first || stem === first || e.lower === first ? 0 : e.lower.startsWith(first) ? 1 : wordStart.test(e.lower) ? 2 : 3;
        const typos = first && !e.lower.includes(first) ? 1 + (typoDistance(words.join(" "), e.name) ?? 3) : 0;
        return (match + typos) * 4 + (SYSTEM.test(e.path) ? 2 : 0) + (isHidden(e) ? 1 : 0);
    };
    return entries
        .map((e) => ({ e, s: score(e) }))
        .sort((a, b) => a.s - b.s || a.e.path.length - b.e.path.length)
        .map((x) => x.e);
}

/** The name, with the part that matched what was typed marked. */
function Name({ name, words }: { name: string; words: string[] }) {
    const lower = name.toLowerCase();
    const word = words.find((w) => lower.includes(w));
    if (!word) return <>{name}</>;
    const at = lower.indexOf(word);
    return (
        <>
            {name.slice(0, at)}
            <mark className="bg-transparent font-semibold text-primary">{name.slice(at, at + word.length)}</mark>
            {name.slice(at + word.length)}
        </>
    );
}

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
    const [copied, setCopied] = useState(false);
    const input = useRef<HTMLInputElement>(null);
    const list = useRef<HTMLDivElement>(null);
    const roots = useRef<string[]>([]);
    const dragRef = useRef<{ x: number; y: number; path: string; started: boolean } | null>(null);

    // Each time it's called up: settings and folders as the main window left them, the drives as they
    // are now, and the last query selected, to type over.
    useEffect(() => {
        const reload = () => void useStore.persist.rehydrate();
        const ready = () => {
            reload();
            void refreshZoxide();
            searchable().then((r) => (roots.current = r), () => {});
            input.current?.focus();
            input.current?.select();
        };
        ready();
        const shown = listen("quick-shown", ready);
        const away = () => void quickHide();
        window.addEventListener("blur", away);
        // The main window saved a change (the theme, a folder opened): it shows here at once.
        const saved = (e: StorageEvent) => e.key === useStore.persist.getOptions().name && reload();
        window.addEventListener("storage", saved);
        return () => {
            shown.then((stop) => stop());
            window.removeEventListener("blur", away);
            window.removeEventListener("storage", saved);
        };
    }, []);

    useEffect(() => {
        const text = query.trim();
        if (!text) {
            void cancelFind(LANE).catch(() => {});
            setFound([]);
            setStatus("");
            setBusy(false);
            setActive(0);
            return;
        }
        let current = true;
        const timer = setTimeout(async () => {
            const { settings } = useStore.getState();
            const entries: FileEntry[] = [];
            const words = wordsOf(text);
            let engine = "";
            setBusy(true);
            setActive(0);
            try {
                if (!roots.current.length) roots.current = await searchable();
                const options = { hidden: settings.showHidden, respectIgnore: false, exclude: settings.findExclusions, maxResults: ASKED, fuzzy: true, rank: true };
                await findFiles(roots.current, text, options, LANE, (event) => {
                    if (!current) return;
                    if (event.type === "started") engine = event.engine;
                    else if (event.type === "entries") {
                        entries.push(...event.entries);
                        setFound(rank(entries, words).slice(0, LIMIT));
                    } else if (event.type === "error") {
                        setBusy(false);
                        setStatus(event.message);
                    } else if (event.type === "finished") {
                        setBusy(false);
                        setFound(rank(entries, words).slice(0, LIMIT));
                        const close = event.fuzzy ? (event.count ? "nothing as typed · close matches: " : "nothing as typed, nor close · ") : "";
                        setStatus(`${engine} · ${close}${formatCount(event.count)}${event.limited ? "+" : ""} found in ${formatDuration(event.elapsed)}${event.count > LIMIT ? ` · the best ${LIMIT} shown` : ""}`);
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
        list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
    }, [active]);

    // Folders open in Glint; files with their app, or in Glint with Alt.
    const open = (entry: FileEntry | undefined, inGlint: boolean) => {
        if (!entry) return;
        if (inGlint || entry.dir) return void quickReveal(entry.path, entry.dir);
        void quickHide();
        // A shortcut to a folder opens the folder in Glint.
        openItem(entry.path)
            .then((folder) => folder && quickReveal(folder, true))
            .catch(() => quickReveal(entry.path, false));
    };

    // Nothing typed yet: the folders you use most (and, shared, the ones zoxide knows), as the command palette ranks them.
    const visits = useStore((s) => s.visits);
    const zoxide = useZoxide((s) => s.folders);
    const recent = useMemo(
        () => usedFolders(visits, 8).map((path): FileEntry => ({ name: baseName(path) || path, path, dir: true, size: 0, modified: 0, attrs: 0, lower: path.toLowerCase(), ext: "" })),
        // eslint-disable-next-line react-hooks/exhaustive-deps
        [visits, zoxide]
    );
    const shown = query.trim() ? found : recent;

    // Plain words read as filters ("photos from 2024"), offered under the box: Tab takes them.
    const plain = useMemo(() => plainQuery(query), [query]);

    const onKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === "Tab" && !e.shiftKey && plain) setQuery(plain.query);
        else if (e.key === "Escape") void quickHide();
        else if (e.key === "ArrowDown") setActive((i) => Math.min(i + 1, shown.length - 1));
        else if (e.key === "ArrowUp") setActive((i) => Math.max(i - 1, 0));
        else if (e.key === "Enter") open(shown[active], e.altKey);
        else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "c" && shown[active] && input.current?.selectionStart === input.current?.selectionEnd) {
            void navigator.clipboard.writeText(shown[active].path).then(() => {
                setCopied(true);
                setTimeout(() => setCopied(false), 1400);
            });
        } else return;
        e.preventDefault();
    };

    // What the names are marked with.
    const words = wordsOf(query);

    const hint = (keys: string, what: string) => (
        <span className="shrink-0">
            <Kbd>{keys}</Kbd> {what}
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
                    role="combobox"
                    aria-expanded
                    aria-controls="quick-results"
                    aria-activedescendant={shown.length ? `quick-${active}` : undefined}
                    placeholder="Search every drive…"
                    className="h-14 min-w-0 flex-1 bg-transparent text-lg outline-hidden placeholder:text-muted-foreground"
                />
            </div>
            {plain && (
                <button type="button" tabIndex={-1} onClick={() => setQuery(plain.query)} className="flex shrink-0 items-center gap-2 border-b px-4 py-2 text-left text-xs hover:bg-accent">
                    <Kbd>Tab</Kbd>
                    <span className="flex min-w-0 flex-wrap gap-1">
                        {plain.chips.map((chip) => (
                            <span key={chip} className="rounded-[4px] bg-primary/12 px-1.5 py-0.5 font-medium text-primary">
                                {chip}
                            </span>
                        ))}
                    </span>
                </button>
            )}
            <div ref={list} id="quick-results" role="listbox" aria-label="Results" className="min-h-0 flex-1 overflow-y-auto p-1.5">
                {!query.trim() && recent.length > 0 && <p className="px-3 pb-1 pt-1.5 text-xs font-semibold text-muted-foreground">Folders you use most</p>}
                {shown.map((entry, i) => {
                    const kind = kindOf(entry);
                    return (
                        <button
                            key={entry.path}
                            data-index={i}
                            id={`quick-${i}`}
                            role="option"
                            aria-selected={i === active}
                            tabIndex={-1}
                            onMouseDown={(e) => {
                                if (e.button === 0) dragRef.current = { x: e.clientX, y: e.clientY, path: entry.path, started: false };
                            }}
                            onMouseMove={(e) => {
                                setActive(i);
                                const d = dragRef.current;
                                if (d && !d.started && (e.buttons & 1) === 1 && Math.hypot(e.clientX - d.x, e.clientY - d.y) >= 6) {
                                    d.started = true;
                                    void startDrag([d.path]);
                                }
                            }}
                            onMouseUp={() => {
                                dragRef.current = null;
                            }}
                            onClick={(e) => open(entry, e.altKey)}
                            className={cn("flex w-full items-center gap-3 rounded-md px-3 py-1.5 text-left", i === active && "bg-accent")}
                        >
                            <kind.icon className={cn("size-4 shrink-0", entry.dir ? "fill-amber-400/80 text-amber-500" : kind.tone)} />
                            <span className="min-w-0 flex-1">
                                <span className="block truncate text-sm">
                                    <Name name={entry.name} words={words} />
                                </span>
                                <span className="block truncate text-xs text-muted-foreground">{parentPath(entry.path)}</span>
                            </span>
                            {!entry.dir && <span className="shrink-0 text-xs tabular-nums text-muted-foreground">{formatSize(entry.size)}</span>}
                        </button>
                    );
                })}
                {!shown.length && (
                    <p className="px-3 py-8 text-center text-sm text-muted-foreground">
                        {query.trim() ? (busy ? "Searching…" : status || "Nothing found") : "Type part of a name. Filters work too: ext:pdf, size:>10mb, dm:7d"}
                    </p>
                )}
            </div>
            <div className="flex h-8 shrink-0 items-center gap-4 border-t px-4 text-xs text-muted-foreground">
                <span className="min-w-0 flex-1 truncate">
                    {copied ? (
                        <span className="inline-flex items-center gap-1 font-medium text-emerald-500">
                            <Check className="size-3.5" /> Copied path
                        </span>
                    ) : found.length ? (
                        status
                    ) : (
                        ""
                    )}
                </span>
                {hint("Enter", "open")}
                {hint("Alt+Enter", "show in Glint")}
                {hint("Ctrl+C", "copy path")}
                {hint("Esc", "close")}
            </div>
        </div>
    );
}
