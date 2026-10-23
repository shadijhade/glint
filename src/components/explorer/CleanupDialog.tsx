import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { AlertTriangle, BrushCleaning, CheckCircle2, FolderInput, Loader2, RotateCw, Square, Trash2 } from "lucide-react";
import { fileOp, reload, revealInApp } from "@/lib/explorer";
import { formatCount, formatDate, formatDuration, formatSize, plural } from "@/lib/format";
import { cancelCleanup, cleanupJunk, type Junk, type JunkReport, resolvePath } from "@/lib/fs";
import { baseName } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, ModalHeader, pressable } from "../Modal";

interface CleanupState {
    root: string | null;
}

const useCleanupStore = create<CleanupState>(() => ({ root: null }));

/** The lane Clean up scans in: one at a time, whichever tab opened it. */
const LANE = "cleanup";

/** Opens Clean up for the folder `root`: what can go to free space below it. */
export function openCleanup(root: string) {
    if (!root) return;
    useCleanupStore.setState({ root });
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

/** Untouched for this long, a project's junk is easy to let go. */
const IDLE_DAYS = 30;

const ENGINES: Record<string, string> = { index: "the index", "fd + dust": "fd and dust", "index + fd + dust": "the index, fd and dust" };

function CleanupModal({ root, onClose }: { root: string; onClose: () => void }) {
    const [report, setReport] = useState<JunkReport | null>(null);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const [stopped, setStopped] = useState(false);
    const [busy, setBusy] = useState(false);
    const [chosen, setChosen] = useState<Set<string>>(new Set());

    // The scan asked for last: one stopped for it (Refresh) mustn't say it stopped.
    const latest = useRef(0);
    const scan = useCallback(() => {
        const run = ++latest.current;
        setLoading(true);
        setError(null);
        setStopped(false);
        cleanupJunk(root, LANE).then(
            (found) => {
                if (run !== latest.current) return;
                setReport(found);
                // Nothing is chosen until it's looked at: a project's own build may be wanted.
                setChosen(new Set());
                setLoading(false);
            },
            (e) => {
                if (run !== latest.current) return;
                if (String(e) === "cancelled") setStopped(true);
                else setError(String(e));
                setLoading(false);
            }
        );
    }, [root]);

    useEffect(() => {
        scan();
        return () => {
            cancelCleanup(LANE).catch(() => {});
        };
    }, [scan]);

    // Grouped by what it is, biggest group first (items are biggest first already).
    const groups = useMemo(() => {
        const by = new Map<string, Junk[]>();
        for (const item of report?.items ?? []) by.set(item.kind, [...(by.get(item.kind) ?? []), item]);
        return [...by.entries()].map(([kind, items]) => ({ kind, items, size: items.reduce((s, i) => s + i.size, 0) })).sort((a, b) => b.size - a.size);
    }, [report]);

    const chosenItems = (report?.items ?? []).filter((i) => chosen.has(i.path));
    const chosenBytes = chosenItems.reduce((s, i) => s + i.size, 0);
    const toggle = (paths: string[], on: boolean) =>
        setChosen((prev) => {
            const next = new Set(prev);
            for (const p of paths) on ? next.add(p) : next.delete(p);
            return next;
        });
    const idleSince = Date.now() - IDLE_DAYS * 86_400_000;

    const recycle = async () => {
        const going = chosenItems;
        if (!going.length) return;
        setBusy(true);
        try {
            await fileOp({ kind: "delete", paths: going.map((i) => i.path), permanent: false, confirm: true });
        } catch (e) {
            toast.error("Couldn't recycle the folders", { description: String(e) });
        }
        // Cancelled in Windows' dialog partway, some went and some didn't: what's gone leaves the
        // list, and the rest stays as it was found.
        const there = await Promise.all(going.map((i) => resolvePath(i.path).then(() => true, () => false)));
        const gone = new Set(going.filter((_, n) => !there[n]).map((i) => i.path));
        if (gone.size) {
            setReport((r) => {
                if (!r) return r;
                const items = r.items.filter((i) => !gone.has(i.path));
                return { ...r, items, total: items.reduce((s, i) => s + i.size, 0) };
            });
            setChosen((prev) => new Set([...prev].filter((p) => !gone.has(p))));
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

    return (
        <Modal label={`Clean up ${folderTitle}`} onClose={onClose} className="max-h-[min(84vh,44rem)] max-w-[50rem]">
            <ModalHeader icon={BrushCleaning} title={`Clean up “${folderTitle}”`} subtitle={root} onClose={onClose} onRefresh={scan} refreshing={loading || busy}>
                {report && report.total > 0 && (
                    <span className="shrink-0 rounded-md bg-amber-500/15 px-2 py-0.5 text-xs font-semibold text-amber-700 dark:text-amber-400">{formatSize(report.total)} of developer junk</span>
                )}
            </ModalHeader>

            <div className="flex shrink-0 items-center gap-2 border-b bg-chrome/60 px-3.5 py-1.5 text-[0.6875rem] text-muted-foreground">
                <span className="font-medium text-foreground">Developer junk</span>
                <span className="min-w-0 truncate">build output and package caches that a project's own tools make again, each beside its project's file</span>
            </div>

            <div className="min-h-0 flex-1 overflow-y-auto p-3">
                {loading ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-14 text-sm text-muted-foreground">
                        <Loader2 className="size-5 animate-spin text-primary" />
                        <span>Looking for build output and caches…</span>
                        <Button variant="outline" size="sm" onClick={() => void cancelCleanup(LANE)}>
                            <Square className="size-3! fill-current" /> Stop
                        </Button>
                    </div>
                ) : stopped ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-14 text-center text-sm text-muted-foreground">
                        <p className="font-medium text-foreground">Stopped before it finished</p>
                        <Button variant="outline" size="sm" onClick={scan}>
                            <RotateCw /> Look again
                        </Button>
                    </div>
                ) : error ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-12 text-center text-sm">
                        <AlertTriangle className="size-6 text-destructive" />
                        <p className="font-medium text-foreground">Couldn't look for developer junk</p>
                        <p className="max-w-md text-xs text-muted-foreground">{error}</p>
                    </div>
                ) : report && report.items.length === 0 ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-14 text-center text-sm text-muted-foreground">
                        <CheckCircle2 className="size-8 stroke-[1.5] text-emerald-500" />
                        <p className="font-medium text-foreground">No developer junk here</p>
                        <p className="max-w-sm text-xs">No build output or package caches beside a project's file, in {formatDuration(report.elapsed)}.</p>
                    </div>
                ) : (
                    <div className="flex flex-col gap-2.5">
                        {groups.map((group) => {
                            const all = group.items.every((i) => chosen.has(i.path));
                            const some = !all && group.items.some((i) => chosen.has(i.path));
                            return (
                                <div key={group.kind} className="overflow-hidden rounded-lg border bg-card/50">
                                    <label className="flex cursor-pointer items-center gap-2 border-b bg-muted/40 px-3 py-1.5 text-xs">
                                        <input
                                            type="checkbox"
                                            checked={all}
                                            ref={(el) => {
                                                if (el) el.indeterminate = some;
                                            }}
                                            onChange={(e) => toggle(group.items.map((i) => i.path), e.target.checked)}
                                            className="size-3.5 accent-primary"
                                        />
                                        <span className="font-semibold text-foreground">{group.kind}</span>
                                        <span className="text-muted-foreground">· {plural(group.items.length, "folder")}</span>
                                        <span className="ml-auto font-medium text-amber-700 tabular-nums dark:text-amber-400">{formatSize(group.size)}</span>
                                    </label>
                                    <div className="divide-y">
                                        {group.items.map((item) => {
                                            const on = chosen.has(item.path);
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
                                                    <span className="min-w-0 flex-1 truncate font-mono text-foreground" title={item.path}>
                                                        {inRoot(item.path)}
                                                    </span>
                                                    {item.projectChanged > 0 && (
                                                        <span
                                                            className={cn("shrink-0 text-[0.6875rem] tabular-nums", item.projectChanged < idleSince ? "text-muted-foreground" : "text-foreground")}
                                                            title="When its project last changed, leaving out its build output and caches"
                                                        >
                                                            {formatDate(item.projectChanged)}
                                                        </span>
                                                    )}
                                                    <span className="w-16 shrink-0 text-right tabular-nums text-foreground">{formatSize(item.size)}</span>
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
                <span className="min-w-0 truncate">
                    {report && !loading && report.items.length > 0
                        ? `${plural(report.items.length, "folder")} · ${formatSize(report.total)} · found by ${ENGINES[report.engine] ?? report.engine} in ${formatDuration(report.elapsed)}`
                        : ""}
                </span>
                {report && !loading && report.items.length > 0 && (
                    <div className="ml-auto flex shrink-0 items-center gap-1.5">
                        <Button variant="ghost" size="sm" className="h-7 px-2 text-xs" onClick={() => setChosen(new Set(report.items.map((i) => i.path)))}>
                            All
                        </Button>
                        <Button
                            variant="ghost"
                            size="sm"
                            className="h-7 px-2 text-xs"
                            title={`Only junk of projects nothing has changed in for ${IDLE_DAYS} days (their build output and caches aside)`}
                            onClick={() => setChosen(new Set(report.items.filter((i) => i.projectChanged > 0 && i.projectChanged < idleSince).map((i) => i.path)))}
                        >
                            Projects untouched for {IDLE_DAYS} days
                        </Button>
                        <Button variant="ghost" size="sm" className="h-7 px-2 text-xs" disabled={!chosen.size} onClick={() => setChosen(new Set())}>
                            None
                        </Button>
                        <Button size="sm" variant="destructive" className="h-7 gap-1 px-2.5 text-xs" disabled={busy || !chosenItems.length} onClick={() => void recycle()} title="Move them to the Recycle Bin (Ctrl+Z takes it back)">
                            <Trash2 className="size-3.5" />
                            {chosenItems.length ? `Recycle ${formatCount(chosenItems.length)} · ${formatSize(chosenBytes)}` : "Recycle"}
                        </Button>
                    </div>
                )}
            </div>
        </Modal>
    );
}
