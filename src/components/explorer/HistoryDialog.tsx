import { useEffect, useMemo, useState } from "react";
import { create } from "zustand";
import { AlertTriangle, FileX2, FolderInput, FolderOpen, History, Loader2, PenLine, Search, Sparkles, Trash2, Undo2, type LucideIcon } from "lucide-react";
import { enableIndex, navigate, revealInApp } from "@/lib/explorer";
import { formatCount, formatDuration, formatFullDate } from "@/lib/format";
import { type HistoryAnswer, type HistoryEvent, type HistoryKind, indexHistory } from "@/lib/fs";
import { baseName, parentPath } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";

const DAY = 86_400_000;
const SPANS = [
    { label: "24h", title: "The last 24 hours", ms: DAY },
    { label: "7d", title: "The last 7 days", ms: 7 * DAY },
    { label: "30d", title: "The last 30 days: all that's kept", ms: 30 * DAY },
];
const FILTERS: { label: string; kinds: HistoryKind[] }[] = [
    { label: "Everything", kinds: [] },
    { label: "Deleted", kinds: ["deleted", "emptied"] },
    { label: "Moved and renamed", kinds: ["moved", "renamed"] },
    { label: "Recycle Bin", kinds: ["recycled", "restored", "emptied"] },
];
/** At most this many come from the service; a search narrows them down. */
const MAX = 5000;

interface HistoryState {
    open: boolean;
    query: string;
    span: number;
    loading: boolean;
    answer: HistoryAnswer | null;
    error: string | null;
}

const useHistory = create<HistoryState>(() => ({ open: false, query: "", span: 7 * DAY, loading: false, answer: null, error: null }));

let asked = 0;

async function load() {
    const { query, span } = useHistory.getState();
    const id = ++asked;
    useHistory.setState({ loading: true, error: null });
    try {
        const answer = await indexHistory(query, Date.now() - span, MAX);
        if (id === asked) useHistory.setState({ answer, loading: false });
    } catch (e) {
        if (id === asked) useHistory.setState({ answer: null, error: String(e), loading: false });
    }
}

/** Opens "Moved and deleted", searching for `query` (part of a name, or a folder's path) when given. */
export function openHistory(query = "") {
    useHistory.setState({ open: true, query });
    void load();
}

function close() {
    asked++;
    useHistory.setState({ open: false });
}

/** How each kind of event is told: its badge, icon and color. */
const KIND: Record<HistoryKind, { label: string; icon: LucideIcon; tone: string }> = {
    renamed: { label: "Renamed", icon: PenLine, tone: "bg-sky-500/15 text-sky-700 dark:text-sky-400" },
    moved: { label: "Moved", icon: FolderInput, tone: "bg-sky-500/15 text-sky-700 dark:text-sky-400" },
    deleted: { label: "Deleted", icon: FileX2, tone: "bg-rose-500/15 text-rose-700 dark:text-rose-400" },
    recycled: { label: "Recycled", icon: Trash2, tone: "bg-amber-500/15 text-amber-800 dark:text-amber-400" },
    restored: { label: "Restored", icon: Undo2, tone: "bg-emerald-500/15 text-emerald-700 dark:text-emerald-400" },
    emptied: { label: "Emptied", icon: FileX2, tone: "bg-rose-500/15 text-rose-700 dark:text-rose-400" },
};

/** What the row says: its name, and where (from where to where). */
function describe(e: HistoryEvent): { name: string; where: string } {
    const inIt = e.dir && e.items > 0 ? ` and ${formatCount(e.items)} ${e.items === 1 ? "item" : "items"} in it` : "";
    const more = `${formatCount(e.items)} more ${e.items === 1 ? "item" : "items"}`;
    switch (e.kind) {
        case "renamed":
            return { name: `${baseName(e.path)} → ${baseName(e.to ?? "")}`, where: parentPath(e.path) };
        case "moved":
            return { name: baseName(e.path), where: `${parentPath(e.path)} → ${parentPath(e.to ?? "")}` };
        case "recycled":
            return { name: baseName(e.path) + inIt, where: `To the Recycle Bin from ${parentPath(e.path)}` };
        case "restored":
            return { name: baseName(e.to ?? e.path), where: `Back in ${parentPath(e.to ?? e.path)}` };
        case "emptied":
            if (e.burst) return { name: more, where: "Deleted from the Recycle Bin" };
            return { name: baseName(e.was ?? e.path) + inIt, where: e.was ? `Deleted from the Recycle Bin; it was in ${parentPath(e.was)}` : "Deleted from the Recycle Bin" };
        default:
            if (e.burst) return { name: more, where: `Deleted in ${e.path}` };
            return { name: baseName(e.path) + inIt, where: parentPath(e.path) };
    }
}

/** The folder to open for something no longer there: the one it was in (or, for a burst, the one it was in). */
function oldFolder(e: HistoryEvent) {
    if (e.burst) return e.path;
    if (e.kind === "emptied") return e.was ? parentPath(e.was) : null;
    return parentPath(e.path);
}

/** A day's heading: Today, Yesterday, or the date. */
function dayOf(ms: number) {
    const days = Math.round((new Date().setHours(0, 0, 0, 0) - new Date(ms).setHours(0, 0, 0, 0)) / DAY);
    if (days === 0) return "Today";
    if (days === 1) return "Yesterday";
    return new Date(ms).toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" });
}

const inRecycleBin = (path: string) => /^[a-z]:\\\$recycle\.bin\\/i.test(path);

function Row({ e }: { e: HistoryEvent }) {
    const kind = KIND[e.kind];
    const { name, where } = describe(e);
    const folder = oldFolder(e);
    // Still in the Recycle Bin is no place to show (it's restored from there).
    const now = e.now && !inRecycleBin(e.now) ? e.now : null;
    return (
        <div className="flex items-center gap-3 px-3 py-2 text-xs hover:bg-muted/30">
            <span className={cn("inline-flex w-[5.5rem] shrink-0 items-center gap-1 rounded px-1.5 py-0.5 font-medium", kind.tone)}>
                <kind.icon className="size-3 shrink-0" />
                {kind.label}
            </span>
            <div className="min-w-0 flex-1">
                <p className="truncate font-medium text-foreground" title={e.to ? `${e.path} → ${e.to}` : (e.was ?? e.path)}>
                    {name}
                </p>
                <p className="truncate text-muted-foreground" title={where}>
                    {where}
                    {now && now !== e.to && <span> · now in {parentPath(now)}</span>}
                </p>
            </div>
            <span className="shrink-0 tabular-nums text-muted-foreground" title={formatFullDate(e.time)}>
                {new Date(e.time).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}
            </span>
            <div className="flex w-14 shrink-0 justify-end gap-0.5">
                {now ? (
                    <Button
                        variant="ghost"
                        size="icon"
                        className="size-6"
                        title={`Show it where it is now: ${now}`}
                        onClick={() => {
                            close();
                            revealInApp(now);
                        }}
                    >
                        <FolderInput className="size-3.5" />
                    </Button>
                ) : (
                    folder && (
                        <Button
                            variant="ghost"
                            size="icon"
                            className="size-6"
                            title={`Open the folder it was in: ${folder}`}
                            onClick={() => {
                                close();
                                navigate(folder);
                            }}
                        >
                            <FolderOpen className="size-3.5" />
                        </Button>
                    )
                )}
            </div>
        </div>
    );
}

/**
 * "Moved and deleted": what was renamed, moved, deleted for good, put in or taken out of the Recycle
 * Bin, on the drives instant search covers, for the last 30 days; from the index service's history.
 */
export function HistoryDialog() {
    const { open, query, span, loading, answer, error } = useHistory();
    const [typed, setTyped] = useState(query);
    const [filter, setFilter] = useState(0);

    useEffect(() => setTyped(query), [query, open]);
    // The search goes to the service a moment after typing stops.
    useEffect(() => {
        if (!open || typed === useHistory.getState().query) return;
        const timer = setTimeout(() => {
            useHistory.setState({ query: typed });
            void load();
        }, 250);
        return () => clearTimeout(timer);
    }, [typed, open]);

    const days = useMemo(() => {
        const kinds = FILTERS[filter].kinds;
        const shown = (answer?.events ?? []).filter((e) => !kinds.length || kinds.includes(e.kind));
        const groups: [string, HistoryEvent[]][] = [];
        for (const e of shown) {
            const day = dayOf(e.time);
            const last = groups[groups.length - 1];
            if (last?.[0] === day) last[1].push(e);
            else groups.push([day, [e]]);
        }
        return groups;
    }, [answer, filter]);
    const count = days.reduce((n, [, events]) => n + events.length, 0);

    if (!open) return null;
    return (
        <Modal label="Moved and deleted" onClose={close} className="max-h-[86vh] max-w-4xl">
            <ModalHeader icon={History} title="Moved and deleted" subtitle="What was renamed, moved and deleted on the drives instant search covers" onClose={close} onRefresh={() => void load()} refreshing={loading}>
                <div className="flex items-center gap-0.5 rounded-md border bg-muted/40 p-0.5">
                    {SPANS.map((s) => (
                        <button
                            key={s.label}
                            type="button"
                            title={s.title}
                            onClick={() => {
                                useHistory.setState({ span: s.ms });
                                void load();
                            }}
                            className={cn("rounded px-2 py-0.5 text-xs font-medium transition-colors", span === s.ms ? "bg-background text-foreground shadow-2xs" : "text-muted-foreground hover:text-foreground")}
                        >
                            {s.label}
                        </button>
                    ))}
                </div>
            </ModalHeader>

            <div className="flex flex-wrap items-center gap-1 border-b bg-muted/20 px-4 py-2 text-xs">
                {FILTERS.map((f, i) => (
                    <button
                        key={f.label}
                        type="button"
                        aria-pressed={filter === i}
                        onClick={() => setFilter(i)}
                        className={cn("rounded-md px-2.5 py-1 font-medium transition-colors", filter === i ? "bg-primary/15 text-primary" : "text-muted-foreground hover:bg-muted/60 hover:text-foreground")}
                    >
                        {f.label}
                    </button>
                ))}
                <div className="relative ml-auto w-64">
                    <Search className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                    <input
                        type="text"
                        value={typed}
                        autoFocus
                        onChange={(e) => setTyped(e.target.value)}
                        placeholder="Part of a name or a path…"
                        className="h-7 w-full rounded-md border bg-background pl-7 pr-2 text-xs outline-hidden focus:border-primary"
                    />
                </div>
            </div>

            <div className="min-h-[20rem] flex-1 overflow-y-auto p-4">
                {loading && !answer && (
                    <div className="flex h-64 flex-col items-center justify-center gap-2.5 text-sm text-muted-foreground">
                        <Loader2 className="size-6 animate-spin text-primary" />
                    </div>
                )}
                {error && (
                    <div className="flex h-64 flex-col items-center justify-center gap-2 text-center text-sm">
                        {error === "tooOld" || error === "off" ? (
                            <>
                                <History className="size-7 stroke-[1.25] text-muted-foreground" />
                                <p className="font-medium">{error === "off" ? "This comes from instant search, which isn't on" : "The instant search service running is from before this history"}</p>
                                <p className="max-w-md text-xs text-muted-foreground">
                                    Its service reads each drive's change journal, which Windows keeps anyway: once it's {error === "off" ? "on" : "updated"}, what was renamed, moved and
                                    deleted shows here, from as far back as the journal goes.
                                </p>
                                <Button size="sm" className="mt-1" onClick={() => void enableIndex().then(load)} title="Windows asks for administrator approval">
                                    <Sparkles /> {error === "off" ? "Turn on instant search" : "Update instant search"}
                                </Button>
                            </>
                        ) : (
                            <>
                                <AlertTriangle className="size-6 text-destructive" />
                                <p className="font-medium">Couldn't read the history</p>
                                <p className="max-w-md text-xs text-muted-foreground">{error}</p>
                            </>
                        )}
                    </div>
                )}
                {answer && !error && count === 0 && (
                    <div className="flex h-56 flex-col items-center justify-center gap-2 text-center text-sm text-muted-foreground">
                        <History className="size-7 stroke-[1.25]" />
                        <p className="font-medium text-foreground">{query.trim() ? `Nothing with “${query.trim()}” in it` : "Nothing renamed, moved or deleted"} in this time</p>
                        <p className="max-w-md text-xs">What programs do with their own files (AppData, caches, temporary and Windows' folders) isn't kept here.</p>
                    </div>
                )}
                {answer && !error && count > 0 && (
                    <div className={cn("space-y-4", loading && "opacity-60")}>
                        {days.map(([day, events]) => (
                            <section key={day}>
                                <h3 className="mb-1.5 px-1 text-xs font-semibold text-muted-foreground">{day}</h3>
                                <div className="divide-y rounded-lg border bg-card">
                                    {events.map((e, i) => (
                                        <Row key={`${e.time}:${e.path}:${i}`} e={e} />
                                    ))}
                                </div>
                            </section>
                        ))}
                    </div>
                )}
            </div>

            <div className="flex items-center justify-between gap-3 border-t bg-muted/20 px-4 py-2.5 text-xs text-muted-foreground">
                <span className="truncate">
                    {answer &&
                        `${formatCount(count)} ${count === 1 ? "change" : "changes"}${answer.limited ? ` (the newest ${formatCount(MAX)}; a search finds older ones)` : ""}${answer.oldest ? ` · kept since ${formatFullDate(answer.oldest)}` : ""} · ${formatDuration(answer.elapsed)}`}
                </span>
                <Button variant="outline" size="sm" onClick={close}>
                    Close
                </Button>
            </div>
        </Modal>
    );
}
