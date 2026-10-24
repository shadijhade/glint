import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import {
    AlertTriangle,
    BrushCleaning,
    CheckCircle2,
    FileBox,
    FileWarning,
    FileX,
    FolderInput,
    FolderX,
    Hourglass,
    Images,
    Loader2,
    type LucideIcon,
    Music,
    Package,
    PenLine,
    RotateCw,
    Square,
    Trash2,
    Video,
} from "lucide-react";
import { type FileEntry, withDetails } from "@/lib/entries";
import { fileOp, rememberUndo, reload, revealInApp, undo } from "@/lib/explorer";
import { isPictureOrVideo, kindOf } from "@/lib/fileTypes";
import { formatCount, formatDate, formatDuration, formatSize, plural } from "@/lib/format";
import { batchRename, cancelCleanup, type CleanupItem, type CleanupMode, type CleanupReport, cleanupScan, resolvePath } from "@/lib/fs";
import { baseName } from "@/lib/paths";
import { refreshTools, useTools } from "@/lib/tools";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, ModalHeader, pressable } from "../Modal";
import { ToolNeeded } from "../ToolNeeded";
import { Thumbnail } from "./Thumbnail";

type ModeKey = CleanupMode["mode"];

interface CleanupState {
    root: string | null;
    /** The kind of clean-up shown: the last one picked, while the app runs. */
    mode: ModeKey;
}

const useCleanupStore = create<CleanupState>(() => ({ root: null, mode: "junk" }));

/** The lane Clean up scans in (and asks for thumbnails in): one at a time, whichever tab opened it. */
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

/** A quick way to choose: everything, what's been left alone a while, or all but the best of each look-alike group. */
interface Helper {
    label: string;
    title: string;
    pick: (items: CleanupItem[], now: number) => CleanupItem[];
}

const everything: Helper = { label: "All", title: "Choose everything listed", pick: (items) => items };
const unchangedFor = (days: number, label: string, title: string): Helper => ({ label, title, pick: (items, now) => items.filter((i) => i.date > 0 && i.date < now - days * DAY) });
/** Everything but the one `better` puts first in each group of look-alikes. */
const keepOne = (label: string, title: string, better: (a: CleanupItem, b: CleanupItem) => number): Helper => ({
    label,
    title,
    pick: (items) => {
        const groups = new Map<number, CleanupItem[]>();
        for (const item of items) groups.set(item.group, [...(groups.get(item.group) ?? []), item]);
        return [...groups.values()].flatMap((group) => {
            const kept = [...group].sort(better)[0];
            return group.filter((i) => i !== kept);
        });
    },
});
const keepBiggest = keepOne("Keep the biggest", "Choose all but the biggest of each group of look-alikes (the most pixels, for pictures)", (a, b) => b.pixels - a.pixels || b.size - a.size);
const keepNewest = keepOne("Keep the newest", "Choose all but the newest of each group of look-alikes", (a, b) => b.date - a.date);

/** Look-alikes found: how many groups, and what all but the biggest of each take up. */
function spareOf(report: CleanupReport) {
    const biggest = new Map<number, number>();
    for (const item of report.items) biggest.set(item.group, Math.max(biggest.get(item.group) ?? 0, item.size));
    return { groups: biggest.size, spare: report.total - [...biggest.values()].reduce((s, n) => s + n, 0) };
}

const lookAlikes = (report: CleanupReport) => {
    const { groups, spare } = spareOf(report);
    return `${plural(groups, "group")} · ${formatSize(spare)} in extra copies`;
};

interface ModeInfo {
    key: ModeKey;
    label: string;
    icon: LucideIcon;
    /** Above the results: what it looks for. */
    about: string;
    /** One of what it finds. */
    noun: string;
    /** In the header, for what was found. */
    badge: (report: CleanupReport) => string;
    /** The date's tooltip. */
    dated: string;
    helpers: Helper[];
    /** It finds folders (with no size to show, for empty ones). */
    folders?: boolean;
    /** The optional tools it runs (the tools shelf installs them). */
    needs?: { id: string; why: string }[];
    /** Look-alikes, shown in their groups. */
    grouped?: boolean;
    /** What's chosen gets its real extension rather than going to the Recycle Bin. */
    renames?: boolean;
}

const CZKAWKA = { id: "czkawka", why: "Czkawka finds look-alike pictures, videos and music, broken files and wrong extensions. It installs for your account with winget." };

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
    },
    {
        key: "images",
        label: "Look-alike pictures",
        icon: Images,
        about: "Pictures that look the same though their files differ: resized, converted or re-saved copies. The biggest of each group comes first.",
        noun: "picture",
        badge: lookAlikes,
        dated: "Last changed",
        helpers: [keepBiggest, keepNewest],
        needs: [CZKAWKA],
        grouped: true,
    },
    {
        key: "videos",
        label: "Look-alike videos",
        icon: Video,
        about: "Videos with the same scenes, whatever their size or format. FFmpeg reads them, so a big folder takes a while.",
        noun: "video",
        badge: lookAlikes,
        dated: "Last changed",
        helpers: [keepBiggest, keepNewest],
        needs: [CZKAWKA, { id: "ffmpeg", why: "FFmpeg reads the videos for Czkawka. It installs for your account with winget." }],
        grouped: true,
    },
    {
        key: "music",
        label: "Same music",
        icon: Music,
        about: "Songs that are the same: by their tags (title and artist), or by how they sound.",
        noun: "song",
        badge: lookAlikes,
        dated: "Last changed",
        helpers: [keepBiggest, keepNewest],
        needs: [CZKAWKA],
        grouped: true,
    },
    {
        key: "broken",
        label: "Broken files",
        icon: FileX,
        about: "Pictures, sound, PDFs and archives that don't open, with why. A file with the wrong extension can show here too: see Wrong extensions.",
        noun: "file",
        badge: (r) => plural(r.items.length, "broken file"),
        dated: "Last changed",
        helpers: [everything],
        needs: [CZKAWKA],
    },
    {
        key: "misnamed",
        label: "Wrong extensions",
        icon: FileWarning,
        about: "Files whose extension says another type than what's in them, such as a PNG named .jpg. Chosen ones get their real extension.",
        noun: "file",
        badge: (r) => plural(r.items.length, "misnamed file"),
        dated: "Last changed",
        helpers: [everything],
        needs: [CZKAWKA],
        renames: true,
    },
];

const SECTIONS: { title: string; keys: ModeKey[] }[] = [
    { title: "Free up space", keys: ["junk", "empty", "temporary", "big"] },
    { title: "With Czkawka", keys: ["images", "videos", "music", "broken", "misnamed"] },
];

/** Big files' sizes to pick from: 100 MB to 10 GB. */
const SIZES = [100, 500, 1024, 4096, 10240].map((mb) => mb * 1024 * 1024);

/** How different look-alikes may be, for pictures (perceptual hash distance) and videos (frame tolerance). */
const ALIKE: { label: string; images: number; videos: number }[] = [
    { label: "Nearly identical", images: 2, videos: 4 },
    { label: "Very similar", images: 6, videos: 10 },
    { label: "Similar", images: 12, videos: 16 },
];

interface Options {
    atLeast: number;
    /** An index into ALIKE. */
    alike: number;
    bySound: boolean;
}

function requestFor(key: ModeKey, options: Options): CleanupMode {
    switch (key) {
        case "big":
            return { mode: "big", atLeast: options.atLeast };
        case "images":
            return { mode: "images", alike: ALIKE[options.alike].images };
        case "videos":
            return { mode: "videos", alike: ALIKE[options.alike].videos };
        case "music":
            return { mode: "music", bySound: options.bySound };
        default:
            return { mode: key };
    }
}

const ENGINE_NAMES: Record<string, string> = { index: "the index", fd: "fd", pdu: "pdu", czkawka: "Czkawka" };

/** "index + fd + pdu" → "the index, fd and pdu". */
function engineText(engine: string) {
    const parts = engine.split(" + ").map((p) => ENGINE_NAMES[p] ?? p);
    return parts.length > 1 ? `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}` : parts[0];
}

/** "photo.JPG" with the extension `fix` instead: "photo.png". */
const withExtension = (path: string, fix: string) => `${path.replace(/\.[^.\\/]*$/, "")}.${fix}`;

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
    const [options, setOptions] = useState<Options>({ atLeast: SIZES[2], alike: 1, bySound: false });
    const [outcomes, setOutcomes] = useState<Partial<Record<ModeKey, Outcome>>>({});
    const [chosenBy, setChosenBy] = useState<Partial<Record<ModeKey, Set<string>>>>({});
    const [busy, setBusy] = useState(false);
    const outcome = outcomes[mode];
    const report = outcome?.report;
    const chosen = chosenBy[mode] ?? NOTHING;

    // The optional tools a kind runs: it looks once they're all there.
    const tools = useTools((s) => s.tools);
    const toolsKnown = Object.keys(tools).length > 0;
    useEffect(() => {
        if (!toolsKnown) void refreshTools();
    }, [toolsKnown]);
    const missing = (info.needs ?? []).filter((need) => !tools[need.id]?.origin);
    const ready = !info.needs || (toolsKnown && missing.length === 0);

    // The scan asked for last: one stopped for it (another kind, Refresh) mustn't say it stopped.
    const latest = useRef(0);
    const scan = useCallback(
        (key: ModeKey, with_: Options) => {
            const run = ++latest.current;
            setOutcomes((o) => {
                // One still looking is stopped by this one: it looks again when it's picked again.
                const next: Partial<Record<ModeKey, Outcome>> = {};
                for (const [k, v] of Object.entries(o) as [ModeKey, Outcome][]) if (!v.loading) next[k] = v;
                return { ...next, [key]: { loading: true } };
            });
            // Nothing is chosen until it's looked at: a project's own build may be wanted.
            setChosenBy((c) => ({ ...c, [key]: new Set() }));
            cleanupScan(root, requestFor(key, with_), LANE).then(
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

    // Each kind is looked for the first time it's picked, once what it runs is installed.
    useEffect(() => {
        if (!outcomes[mode] && ready) scan(mode, options);
    }, [mode, outcomes, ready, options, scan]);

    useEffect(
        () => () => {
            cancelCleanup(LANE).catch(() => {});
        },
        []
    );

    const changeOptions = (change: Partial<Options>) => {
        const next = { ...options, ...change };
        setOptions(next);
        if (ready) scan(mode, next);
    };

    // Grouped by what it is (big files by type, look-alikes by group), biggest group first; empty folders in one, by path.
    const groups = useMemo(() => {
        const by = new Map<string, { item: CleanupItem; entry: FileEntry }[]>();
        for (const item of report?.items ?? []) {
            const entry = withDetails({ name: baseName(item.path), path: item.path, dir: !!info.folders, size: item.size, modified: item.date, attrs: 0 });
            const kind = info.grouped ? `#${item.group}` : item.kind || (info.folders ? "Empty folders" : kindOf(entry).group.name);
            by.set(kind, [...(by.get(kind) ?? []), { item, entry }]);
        }
        return [...by.entries()]
            .map(([kind, rows]) => ({
                kind,
                // A group of look-alikes is called by its first (biggest) one.
                title: info.grouped ? `“${rows[0].entry.name}” and ${plural(rows.length - 1, `look-alike`)}` : kind,
                rows,
                size: rows.reduce((s, r) => s + r.item.size, 0),
            }))
            .sort((a, b) => b.size - a.size);
    }, [report, info]);

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

    /** What's gone (or renamed) leaves the list; the rest stays as it was found. */
    const drop = (at: ModeKey, gone: Set<string>) => {
        if (!gone.size) return;
        setOutcomes((o) => {
            const was = o[at]?.report;
            if (!was) return o;
            const left = was.items.filter((i) => !gone.has(i.path));
            // A look-alike left on its own is no longer one.
            const counts = new Map<number, number>();
            for (const i of left) counts.set(i.group, (counts.get(i.group) ?? 0) + 1);
            const kept = left.filter((i) => !i.group || (counts.get(i.group) ?? 0) > 1);
            return { ...o, [at]: { report: { ...was, items: kept, total: kept.reduce((s, i) => s + i.size, 0) } } };
        });
        setChosenBy((c) => ({ ...c, [at]: new Set([...(c[at] ?? [])].filter((p) => !gone.has(p))) }));
        reload();
    };

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
        // Cancelled in Windows' dialog partway, some went and some didn't.
        const there = await Promise.all(going.map((i) => resolvePath(i.path).then(() => true, () => false)));
        drop(at, new Set(going.filter((_, n) => !there[n]).map((i) => i.path)));
        setBusy(false);
    };

    /** Gives each chosen misnamed file its real extension, all or none, with Undo. */
    const giveExtensions = async () => {
        const going = chosenItems.filter((i) => i.fix);
        const at = mode;
        if (!going.length) return;
        setBusy(true);
        try {
            const done = await batchRename(going.map((i) => ({ from: i.path, to: withExtension(i.path, i.fix) })));
            const what = plural(done.renamed, "file");
            const step = done.undoOps.length ? rememberUndo(`extensions of ${what}`, () => batchRename(done.undoOps)) : null;
            toast.success(`Gave ${what} ${done.renamed === 1 ? "its" : "their"} real extension`, { action: step ? { label: "Undo", onClick: () => void undo(step) } : undefined });
            drop(at, new Set(going.map((i) => i.path)));
        } catch (e) {
            toast.error("Couldn't rename them", { description: String(e) });
        }
        setBusy(false);
    };

    const folderTitle = baseName(root) || root;
    /** A path from the folder scanned (web\node_modules), so the project's name isn't what gets cut off. */
    const inRoot = (path: string) => {
        const base = root.replace(/[\\/]+$/, "");
        return path.toLowerCase().startsWith(`${base.toLowerCase()}\\`) ? path.slice(base.length + 1) : path;
    };

    const nothingFound = {
        junk: "No developer junk here",
        empty: "No empty folders here",
        temporary: "Nothing left behind here",
        big: `No files of ${formatSize(options.atLeast)} or more here`,
        images: "No look-alike pictures here",
        videos: "No look-alike videos here",
        music: "No songs found twice here",
        broken: "Nothing broken here",
        misnamed: "Every file here has the right extension",
    }[mode];
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
    const select = "h-6 rounded border bg-background px-1.5 text-xs outline-hidden focus:border-primary";

    return (
        <Modal label={`Clean up ${folderTitle}`} onClose={onClose} className="max-h-[min(88vh,48rem)] max-w-[60rem]">
            <ModalHeader icon={BrushCleaning} title={`Clean up “${folderTitle}”`} subtitle={root} onClose={onClose} onRefresh={ready ? () => scan(mode, options) : undefined} refreshing={!!outcome?.loading || busy}>
                {shown && (mode === "empty" || report.total > 0 || !!info.needs) && (
                    <span className="shrink-0 rounded-md bg-amber-500/15 px-2 py-0.5 text-xs font-semibold text-amber-700 dark:text-amber-400">{info.badge(report)}</span>
                )}
            </ModalHeader>

            <div className="flex min-h-0 flex-1">
                <nav aria-label="What to look for" className="flex w-48 shrink-0 flex-col gap-3 overflow-y-auto border-r bg-chrome/60 p-2">
                    {SECTIONS.map((section) => (
                        <div key={section.title} role="tablist" aria-label={section.title} aria-orientation="vertical" className="flex flex-col gap-0.5">
                            <span className="px-2 pb-0.5 text-[0.625rem] font-semibold tracking-wide text-muted-foreground uppercase">{section.title}</span>
                            {section.keys.map((key) => {
                                const m = MODES.find((x) => x.key === key)!;
                                const found = outcomes[key]?.report;
                                return (
                                    <button
                                        key={key}
                                        type="button"
                                        role="tab"
                                        aria-selected={key === mode}
                                        onClick={() => useCleanupStore.setState({ mode: key })}
                                        className={cn(
                                            "flex h-7 items-center gap-2 rounded-md px-2 text-left text-xs font-medium text-muted-foreground transition-colors hover:bg-foreground/[0.05] hover:text-foreground",
                                            key === mode && "bg-background text-primary shadow-xs hover:bg-background dark:bg-foreground/[0.12]"
                                        )}
                                    >
                                        <m.icon className="size-3.5 shrink-0" />
                                        <span className="min-w-0 flex-1 truncate">{m.label}</span>
                                        {found && found.items.length > 0 && <span className="shrink-0 text-[0.625rem] tabular-nums text-muted-foreground">{formatCount(found.items.length)}</span>}
                                    </button>
                                );
                            })}
                        </div>
                    ))}
                </nav>

                <div className="flex min-w-0 flex-1 flex-col">
                    <div className="flex shrink-0 items-center gap-3 border-b px-3.5 py-2">
                        <p className="min-w-0 flex-1 text-[0.6875rem] text-muted-foreground">{info.about}</p>
                        {mode === "big" && (
                            <label className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
                                At least
                                <select value={options.atLeast} onChange={(e) => changeOptions({ atLeast: Number(e.target.value) })} className={select}>
                                    {SIZES.map((size) => (
                                        <option key={size} value={size}>
                                            {formatSize(size)}
                                        </option>
                                    ))}
                                </select>
                            </label>
                        )}
                        {(mode === "images" || mode === "videos") && (
                            <label className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
                                Match
                                <select value={options.alike} onChange={(e) => changeOptions({ alike: Number(e.target.value) })} className={select}>
                                    {ALIKE.map((level, n) => (
                                        <option key={level.label} value={n}>
                                            {level.label}
                                        </option>
                                    ))}
                                </select>
                            </label>
                        )}
                        {mode === "music" && (
                            <label className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
                                Compare
                                <select value={options.bySound ? "sound" : "tags"} onChange={(e) => changeOptions({ bySound: e.target.value === "sound" })} className={select}>
                                    <option value="tags">Tags: title and artist</option>
                                    <option value="sound">How they sound (slower)</option>
                                </select>
                            </label>
                        )}
                    </div>

                    <div className="min-h-0 flex-1 overflow-y-auto p-3">
                        {!ready ? (
                            <div className="flex flex-col gap-2">
                                {toolsKnown ? (
                                    missing.map((need) => <ToolNeeded key={need.id} id={need.id} why={need.why} icon={info.icon} />)
                                ) : (
                                    <Loader2 className="mx-auto mt-10 size-5 animate-spin text-muted-foreground" />
                                )}
                            </div>
                        ) : !outcome || outcome.loading ? (
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
                                <Button variant="outline" size="sm" onClick={() => scan(mode, options)}>
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
                                                <span className="min-w-0 truncate font-semibold text-foreground">{group.title}</span>
                                                {!info.grouped && <span className="shrink-0 text-muted-foreground">· {plural(group.rows.length, info.folders ? "folder" : "file")}</span>}
                                                {group.size > 0 && <span className="ml-auto shrink-0 font-medium text-amber-700 tabular-nums dark:text-amber-400">{formatSize(group.size)}</span>}
                                            </label>
                                            <div className="divide-y">
                                                {group.rows.map(({ item, entry }) => {
                                                    const on = chosen.has(item.path);
                                                    const kind = kindOf(entry);
                                                    const picture = !entry.dir && isPictureOrVideo(entry);
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
                                                            {picture ? (
                                                                <span className="relative flex size-10 shrink-0 items-center justify-center overflow-hidden rounded-sm">
                                                                    <Thumbnail entry={entry} box={40} lane={LANE} enabled iconSize={18} />
                                                                </span>
                                                            ) : (
                                                                <kind.icon className={cn("size-3.5 shrink-0", kind.tone)} />
                                                            )}
                                                            <span className="flex min-w-0 flex-1 flex-col">
                                                                <span className="truncate font-mono text-foreground" title={item.path}>
                                                                    {inRoot(item.path)}
                                                                </span>
                                                                {item.note && (
                                                                    <span className="truncate text-[0.6875rem] text-muted-foreground" title={item.note}>
                                                                        {item.note}
                                                                    </span>
                                                                )}
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
                </div>
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
                        {info.renames ? (
                            <Button size="sm" className="h-7 gap-1 px-2.5 text-xs" disabled={busy || !chosenItems.length} onClick={() => void giveExtensions()} title="Rename them to the extension that says what's in them: Ctrl+Z takes it back">
                                <PenLine className="size-3.5" />
                                {chosenItems.length ? `Give ${plural(chosenItems.length, "file")} ${chosenItems.length === 1 ? "its" : "their"} real extension` : "Give the real extension"}
                            </Button>
                        ) : (
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
                        )}
                    </div>
                )}
            </div>
        </Modal>
    );
}
