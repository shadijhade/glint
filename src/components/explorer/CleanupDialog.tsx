import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { AlertTriangle, BrushCleaning, CheckCircle2, FileBox, FolderInput, FolderX, Hourglass, Loader2, type LucideIcon, Package, RotateCw, Square, Trash2 } from "lucide-react";
import { type FileEntry, withDetails } from "@/lib/entries";
import { fileOp, reload, revealInApp } from "@/lib/explorer";
import { kindOf } from "@/lib/fileTypes";
import { formatCount, formatDate, formatDuration, formatSize, plural } from "@/lib/format";
import { cancelCleanup, type CleanupItem, type CleanupMode, type CleanupReport, cleanupScan, resolvePath } from "@/lib/fs";
import { baseName } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, ModalHeader, pressable } from "../Modal";

type ModeKey = CleanupMode["mode"];

interface CleanupState {
    root: string | null;
    /** The kind of clean-up shown: the last one picked, while the app runs. */
    mode: ModeKey;
}

const useCleanupStore = create<CleanupState>(() => ({ root: null, mode: "junk" }));

/** The lane Clean up scans in: one at a time, whichever tab opened it. */
const LANE = "cleanup";

/** Opens Clean up for the folder `root` (on `mode`, or the one picked last): what can go to free space below it. */
export function openCleanup(root: string, mode?: ModeKey) {
    if (!root) return;
    useCleanupStore.setState((s) => ({ root, mode: mode ?? s.mode }));
}

function closeCleanup() {
    cancelCleanup(LANE).catch(() => {});
    useCleanupStore.setState({ root: null });
}

export function CleanupDialog() {
    const root = useCleanupStore((s) => s.root);
    if (!root) return null;
    return <CleanupModal key={root} root={root} onClose={closeCleanup} />;
}

const DAY = 86_400_000;

/** A quick way to choose: everything, or what's been left alone a while. */
interface Helper {
    label: string;
    title: string;
    pick: (items: CleanupItem[], now: number) => CleanupItem[];
}

const everything: Helper = { label: "All", title: "Choose everything listed", pick: (items) => items };
const unchangedFor = (days: number, label: string, title: string): Helper => ({ label, title, pick: (items, now) => items.filter((i) => i.date > 0 && i.date < now - days * DAY) });

interface ModeInfo {
    key: ModeKey;
    label: string;
    icon: LucideIcon;
    /** Under the tabs: what it looks for. */
    about: string;
    /** One of what it finds. */
    noun: string;
    /** In the header, for what was found. */
    badge: (report: CleanupReport) => string;
    /** The date's tooltip. */
    dated: string;
    helpers: Helper[];
    /** It finds folders (with no size to show, for empty ones). */
    folders: boolean;
}

const MODES: ModeInfo[] = [
    {
        key: "junk",
        label: "Developer junk",
        icon: Package,
        about: "Build output and package caches that a project's own tools make again, each beside its project's file.",
        noun: "folder",
        badge: (r) => `${formatSize(r.total)} of developer junk`,
        dated: "When its project last changed, leaving out its build output and caches",
        helpers: [everything, unchangedFor(30, "Projects untouched for 30 days", "Only junk of projects nothing has changed in for 30 days (their build output and caches aside)")],
        folders: true,
    },
    {
        key: "empty",
        label: "Empty folders",
        icon: FolderX,
        about: "Folders with nothing in them, down through their subfolders. Hidden folders aren't looked in.",
        noun: "empty folder",
        badge: (r) => plural(r.items.length, "empty folder"),
        dated: "Last changed",
        helpers: [everything],
        folders: true,
    },
    {
        key: "temporary",
        label: "Temporary files",
        icon: Hourglass,
        about: "What programs leave behind: .tmp files, Office's ~$ owner files, Thumbs.db, .DS_Store. What a program still has open is left out.",
        noun: "file",
        badge: (r) => `${formatSize(r.total)} left behind`,
        dated: "Last changed",
        helpers: [everything, unchangedFor(7, "Older than a week", "Only what hasn't changed in a week")],
        folders: false,
    },
    {
        key: "big",
        label: "Big files",
        icon: FileBox,
        about: "The biggest files, outside hidden folders and AppData (where programs keep theirs). Recycle only what you know.",
        noun: "file",
        badge: (r) => `${formatSize(r.total)} in big files`,
        dated: "Last changed",
        helpers: [unchangedFor(365, "Unchanged for a year", "Only files that haven't changed in a year")],
        folders: false,
    },
];

/** Big files' sizes to pick from: 100 MB to 10 GB. */
const SIZES = [100, 500, 1024, 4096, 10240].map((mb) => mb * 1024 * 1024);

const ENGINE_NAMES: Record<string, string> = { index: "the index", fd: "fd", dust: "dust" };

/** "index + fd + dust" → "the index, fd and dust". */
function engineText(engine: string) {
    const parts = engine.split(" + ").map((p) => ENGINE_NAMES[p] ?? p);
    return parts.length > 1 ? `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}` : parts[0];
}

interface Outcome {
    report?: CleanupReport;
    error?: string;
    stopped?: boolean;
    loading?: boolean;
}

const NOTHING = new Set<string>();

function CleanupModal({ root, onClose }: { root: string; onClose: () => void }) {
    const mode = useCleanupStore((s) => s.mode);
    const info = MODES.find((m) => m.key === mode) ?? MODES[0];
    const [atLeast, setAtLeast] = useState(SIZES[2]);
    const [outcomes, setOutcomes] = useState<Partial<Record<ModeKey, Outcome>>>({});
    const [chosenBy, setChosenBy] = useState<Partial<Record<ModeKey, Set<string>>>>({});
    const [busy, setBusy] = useState(false);
    const outcome = outcomes[mode];
    const report = outcome?.report;
    const chosen = chosenBy[mode] ?? NOTHING;

    // The scan asked for last: one stopped for it (another tab, Refresh) mustn't say it stopped.
    const latest = useRef(0);
    const scan = useCallback(
        (key: ModeKey, threshold: number) => {
            const run = ++latest.current;
            setOutcomes((o) => {
                // One still looking is stopped by this one: it looks again when it's picked again.
                const next: Partial<Record<ModeKey, Outcome>> = {};
                for (const [k, v] of Object.entries(o) as [ModeKey, Outcome][]) if (!v.loading) next[k] = v;
                return { ...next, [key]: { loading: true } };
            });
            // Nothing is chosen until it's looked at: a project's own build may be wanted.
            setChosenBy((c) => ({ ...c, [key]: new Set() }));
            const request: CleanupMode = key === "big" ? { mode: "big", atLeast: threshold } : { mode: key };
            cleanupScan(root, request, LANE).then(
                (found) => {
                    if (run === latest.current) setOutcomes((o) => ({ ...o, [key]: { report: found } }));
                },
                (e) => {
                    if (run === latest.current) setOutcomes((o) => ({ ...o, [key]: String(e) === "cancelled" ? { stopped: true } : { error: String(e) } }));
                }
            );
        },
        [root]
    );

    // Each kind is looked for the first time it's picked.
    useEffect(() => {
        if (!outcomes[mode]) scan(mode, atLeast);
    }, [mode, outcomes, atLeast, scan]);

    useEffect(
        () => () => {
            cancelCleanup(LANE).catch(() => {});
        },
        []
    );

    // Grouped by what it is (big files by type), biggest group first; empty folders in one, by path.
    const groups = useMemo(() => {
        const by = new Map<string, { item: CleanupItem; entry: FileEntry }[]>();
        for (const item of report?.items ?? []) {
            const entry = withDetails({ name: baseName(item.path), path: item.path, dir: info.folders, size: item.size, modified: item.date, attrs: 0 });
            const kind = item.kind || (info.folders ? "Empty folders" : kindOf(entry).group.name);
            by.set(kind, [...(by.get(kind) ?? []), { item, entry }]);
        }
        return [...by.entries()].map(([kind, rows]) => ({ kind, rows, size: rows.reduce((s, r) => s + r.item.size, 0) })).sort((a, b) => b.size - a.size);
    }, [report, info.folders]);

    const items = report?.items ?? [];
    const chosenItems = items.filter((i) => chosen.has(i.path));
    const chosenBytes = chosenItems.reduce((s, i) => s + i.size, 0);
    const setChosen = (update: (prev: Set<string>) => Set<string>) => setChosenBy((c) => ({ ...c, [mode]: update(c[mode] ?? new Set()) }));
    const toggle = (paths: string[], on: boolean) =>
        setChosen((prev) => {
            const next = new Set(prev);
            for (const p of paths) on ? next.add(p) : next.delete(p);
            return next;
        });
    const now = Date.now();

    const recycle = async () => {
        const going = chosenItems;
        const at = mode;
        if (!going.length) return;
        setBusy(true);
        try {
            await fileOp({ kind: "delete", paths: going.map((i) => i.path), permanent: false, confirm: true });
        } catch (e) {
            toast.error(`Couldn't recycle the ${info.folders ? "folders" : "files"}`, { description: String(e) });
        }
        // Cancelled in Windows' dialog partway, some went and some didn't: what's gone leaves the
        // list, and the rest stays as it was found.
        const there = await Promise.all(going.map((i) => resolvePath(i.path).then(() => true, () => false)));
        const gone = new Set(going.filter((_, n) => !there[n]).map((i) => i.path));
        if (gone.size) {
            setOutcomes((o) => {
                const was = o[at]?.report;
                if (!was) return o;
                const left = was.items.filter((i) => !gone.has(i.path));
                return { ...o, [at]: { report: { ...was, items: left, total: left.reduce((s, i) => s + i.size, 0) } } };
            });
            setChosenBy((c) => ({ ...c, [at]: new Set([...(c[at] ?? [])].filter((p) => !gone.has(p))) }));
            reload();
        }
        setBusy(false);
    };

    const folderTitle = baseName(root) || root;
    /** A path from the folder scanned (web\node_modules), so the project's name isn't what gets cut off. */
    const inRoot = (path: string) => {
        const base = root.replace(/[\\/]+$/, "");
        return path.toLowerCase().startsWith(`${base.toLowerCase()}\\`) ? path.slice(base.length + 1) : path;
    };

    const nothingFound = mode === "big" ? `No files of ${formatSize(atLeast)} or more here` : { junk: "No developer junk here", empty: "No empty folders here", temporary: "Nothing left behind here" }[mode];
    const shown = report && !outcome?.loading && report.items.length > 0;
    const summary = report
        ? [
              plural(report.items.length + report.more, info.noun),
              !info.folders || mode === "junk" ? formatSize(report.total) : "",
              `found by ${engineText(report.engine)} in ${formatDuration(report.elapsed)}`,
              report.more ? (mode === "empty" ? `the first ${formatCount(report.items.length)} listed` : `the ${formatCount(report.items.length)} biggest listed`) : "",
              report.held ? `${plural(report.held, "file")} in use left out` : "",
          ]
              .filter(Boolean)
              .join(" · ")
        : "";

    return (
        <Modal label={`Clean up ${folderTitle}`} onClose={onClose} className="max-h-[min(86vh,46rem)] max-w-[52rem]">
            <ModalHeader icon={BrushCleaning} title={`Clean up “${folderTitle}”`} subtitle={root} onClose={onClose} onRefresh={() => scan(mode, atLeast)} refreshing={!!outcome?.loading || busy}>
                {shown && (mode === "empty" || report.total > 0) && (
                    <span className="shrink-0 rounded-md bg-amber-500/15 px-2 py-0.5 text-xs font-semibold text-amber-700 dark:text-amber-400">{info.badge(report)}</span>
                )}
            </ModalHeader>

            <div className="flex shrink-0 flex-col gap-1.5 border-b bg-chrome/60 px-3.5 py-2">
                <div className="flex items-center gap-2">
                    <div role="tablist" aria-label="What to look for" className="flex min-w-0 flex-wrap rounded-md bg-foreground/[0.05] p-0.5">
                        {MODES.map((m) => (
                            <button
                                key={m.key}
                                type="button"
                                role="tab"
                                aria-selected={m.key === mode}
                                onClick={() => useCleanupStore.setState({ mode: m.key })}
                                className={cn(
                                    "flex h-6 items-center gap-1.5 rounded-[4px] px-2 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground",
                                    m.key === mode && "bg-background text-primary shadow-xs dark:bg-foreground/[0.12]"
                                )}
                            >
                                <m.icon className="size-3.5" /> {m.label}
                            </button>
                        ))}
                    </div>
                    {mode === "big" && (
                        <label className="ml-auto flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
                            At least
                            <select
                                value={atLeast}
                                onChange={(e) => {
                                    const size = Number(e.target.value);
                                    setAtLeast(size);
                                    scan("big", size);
                                }}
                                className="h-6 rounded border bg-background px-1.5 text-xs outline-hidden focus:border-primary"
                            >
                                {SIZES.map((size) => (
                                    <option key={size} value={size}>
                                        {formatSize(size)}
                                    </option>
                                ))}
                            </select>
                        </label>
                    )}
                </div>
                <p className="text-[0.6875rem] text-muted-foreground">{info.about}</p>
            </div>

            <div className="min-h-0 flex-1 overflow-y-auto p-3">
                {!outcome || outcome.loading ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-14 text-sm text-muted-foreground">
                        <Loader2 className="size-5 animate-spin text-primary" />
                        <span>Looking below “{folderTitle}”…</span>
                        <Button variant="outline" size="sm" onClick={() => void cancelCleanup(LANE)}>
                            <Square className="size-3! fill-current" /> Stop
                        </Button>
                    </div>
                ) : outcome.stopped ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-14 text-center text-sm text-muted-foreground">
                        <p className="font-medium text-foreground">Stopped before it finished</p>
                        <Button variant="outline" size="sm" onClick={() => scan(mode, atLeast)}>
                            <RotateCw /> Look again
                        </Button>
                    </div>
                ) : outcome.error ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-12 text-center text-sm">
                        <AlertTriangle className="size-6 text-destructive" />
                        <p className="font-medium text-foreground">Couldn't look below “{folderTitle}”</p>
                        <p className="max-w-md text-xs text-muted-foreground">{outcome.error}</p>
                    </div>
                ) : report && report.items.length === 0 ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-14 text-center text-sm text-muted-foreground">
                        <CheckCircle2 className="size-8 stroke-[1.5] text-emerald-500" />
                        <p className="font-medium text-foreground">{nothingFound}</p>
                        <p className="max-w-sm text-xs">
                            Looked through in {formatDuration(report.elapsed)}
                            {report.held ? `; ${plural(report.held, "file")} a program has open ${report.held === 1 ? "was" : "were"} left out` : ""}.
                        </p>
                    </div>
                ) : (
                    <div className="flex flex-col gap-2.5">
                        {groups.map((group) => {
                            const all = group.rows.every((r) => chosen.has(r.item.path));
                            const some = !all && group.rows.some((r) => chosen.has(r.item.path));
                            return (
                                <div key={group.kind} className="overflow-hidden rounded-lg border bg-card/50">
                                    <label className="flex cursor-pointer items-center gap-2 border-b bg-muted/40 px-3 py-1.5 text-xs">
                                        <input
                                            type="checkbox"
                                            checked={all}
                                            ref={(el) => {
                                                if (el) el.indeterminate = some;
                                            }}
                                            onChange={(e) => toggle(group.rows.map((r) => r.item.path), e.target.checked)}
                                            className="size-3.5 accent-primary"
                                        />
                                        <span className="font-semibold text-foreground">{group.kind}</span>
                                        <span className="text-muted-foreground">· {plural(group.rows.length, info.folders ? "folder" : "file")}</span>
                                        {group.size > 0 && <span className="ml-auto font-medium text-amber-700 tabular-nums dark:text-amber-400">{formatSize(group.size)}</span>}
                                    </label>
                                    <div className="divide-y">
                                        {group.rows.map(({ item, entry }) => {
                                            const on = chosen.has(item.path);
                                            const kind = kindOf(entry);
                                            return (
                                                <div
                                                    key={item.path}
                                                    role="checkbox"
                                                    aria-checked={on}
                                                    aria-label={inRoot(item.path)}
                                                    {...pressable(() => toggle([item.path], !on))}
                                                    className={cn("flex cursor-pointer items-center gap-2 px-3 py-1.5 text-xs transition-colors hover:bg-accent/40", on && "bg-primary/[0.04]")}
                                                >
                                                    <input type="checkbox" tabIndex={-1} readOnly checked={on} className="pointer-events-none size-3.5 shrink-0 accent-primary" />
                                                    <kind.icon className={cn("size-3.5 shrink-0", kind.tone)} />
                                                    <span className="min-w-0 flex-1 truncate font-mono text-foreground" title={item.path}>
                                                        {inRoot(item.path)}
                                                    </span>
                                                    {item.date > 0 && (
                                                        <span className={cn("shrink-0 text-[0.6875rem] tabular-nums", mode === "junk" && item.date < now - 30 * DAY ? "text-muted-foreground" : "text-foreground")} title={info.dated}>
                                                            {formatDate(item.date)}
                                                        </span>
                                                    )}
                                                    {(item.size > 0 || !info.folders) && <span className="w-16 shrink-0 text-right tabular-nums text-foreground">{formatSize(item.size)}</span>}
                                                    <Button
                                                        variant="ghost"
                                                        size="icon"
                                                        className="size-6 shrink-0"
                                                        title="Show it in its folder"
                                                        onClick={(e) => {
                                                            e.stopPropagation();
                                                            onClose();
                                                            revealInApp(item.path);
                                                        }}
                                                    >
                                                        <FolderInput className="size-3.5" />
                                                    </Button>
                                                </div>
                                            );
                                        })}
                                    </div>
                                </div>
                            );
                        })}
                    </div>
                )}
            </div>

            <div className="flex h-10 shrink-0 items-center gap-2 border-t bg-chrome px-3.5 text-xs text-muted-foreground">
                <span className="min-w-0 truncate" title={summary}>
                    {shown ? summary : ""}
                </span>
                {shown && (
                    <div className="ml-auto flex shrink-0 items-center gap-1.5">
                        {info.helpers.map((helper) => (
                            <Button key={helper.label} variant="ghost" size="sm" className="h-7 px-2 text-xs" title={helper.title} onClick={() => setChosen(() => new Set(helper.pick(items, now).map((i) => i.path)))}>
                                {helper.label}
                            </Button>
                        ))}
                        <Button variant="ghost" size="sm" className="h-7 px-2 text-xs" disabled={!chosen.size} onClick={() => setChosen(() => new Set())}>
                            None
                        </Button>
                        <Button
                            size="sm"
                            variant="destructive"
                            className="h-7 gap-1 px-2.5 text-xs"
                            disabled={busy || !chosenItems.length}
                            onClick={() => void recycle()}
                            title="Move them to the Recycle Bin: Ctrl+Z takes it back. Windows asks first about one too big for the Recycle Bin."
                        >
                            <Trash2 className="size-3.5" />
                            {!chosenItems.length ? "Recycle" : chosenBytes > 0 ? `Recycle ${formatCount(chosenItems.length)} · ${formatSize(chosenBytes)}` : `Recycle ${plural(chosenItems.length, info.noun)}`}
                        </Button>
                    </div>
                )}
            </div>
        </Modal>
    );
}
