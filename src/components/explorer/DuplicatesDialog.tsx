import { useCallback, useEffect, useRef, useState } from "react";
import { create } from "zustand";
import { confirm } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import {
    AlertTriangle,
    CheckCircle2,
    Files,
    FolderInput,
    Link2,
    Loader2,
    RotateCw,
    Sparkles,
    Square,
    Trash2,
    Zap,
} from "lucide-react";
import { fileOp, reload, revealInApp } from "@/lib/explorer";
import { formatCount, formatDate, formatDuration, formatSize } from "@/lib/format";
import { cancelDedup, type DedupReport, type DuplicateGroup, findDuplicates, hardlinkDuplicates } from "@/lib/fs";
import { baseName } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, ModalHeader, pressable } from "../Modal";

interface DedupState {
    root: string | null;
}

const useDedupStore = create<DedupState>(() => ({ root: null }));

/** Opens the 3-stage duplicate finder & hardlink deduplicator for `root`. */
export function inspectDuplicates(root: string) {
    if (!root) return;
    useDedupStore.setState({ root });
}

export function closeDuplicates() {
    cancelDedup().catch(() => {});
    useDedupStore.setState({ root: null });
}

export function DuplicatesDialog() {
    const root = useDedupStore((s) => s.root);
    if (!root) return null;
    return <DuplicatesModal root={root} onClose={closeDuplicates} />;
}

function DuplicatesModal({ root, onClose }: { root: string; onClose: () => void }) {
    const [report, setReport] = useState<DedupReport | null>(null);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [stopped, setStopped] = useState(false);
    /** Per group hash, which file path is chosen as the master to keep (defaults to the first/oldest). */
    const [keptByGroup, setKeptByGroup] = useState<Record<string, string>>({});

    // The scan asked for last: one stopped for it (another folder, or Refresh) mustn't say it stopped.
    const latest = useRef(0);
    const scan = useCallback(() => {
        const run = ++latest.current;
        setLoading(true);
        setError(null);
        setStopped(false);
        findDuplicates(root, 1).then(
            (res) => {
                if (run !== latest.current) return;
                setReport(res);
                setLoading(false);
            },
            (e) => {
                if (run !== latest.current) return;
                const msg = String(e);
                if (msg === "cancelled") setStopped(true);
                else setError(msg);
                setLoading(false);
            }
        );
    }, [root]);

    useEffect(() => {
        scan();
        return () => {
            cancelDedup().catch(() => {});
        };
    }, [scan]);


    const keptFor = (group: DuplicateGroup) => {
        const chosen = keptByGroup[group.hash];
        return chosen && group.files.some((f) => f.path === chosen) ? chosen : group.files[0].path;
    };

    const dupsFor = (group: DuplicateGroup) => {
        const keep = keptFor(group);
        return group.files.filter((f) => f.path !== keep).map((f) => f.path);
    };

    const linkOneGroup = async (group: DuplicateGroup) => {
        const keep = keptFor(group);
        const dups = dupsFor(group);
        if (!dups.length) return;
        const ok = await confirm(
            `Replace ${dups.length === 1 ? "the other copy" : `the ${dups.length} other copies`} of “${baseName(keep)}” with hard links to it?\n\nEvery path stays where it is and ${formatSize(group.wastedBytes)} is freed, but they all become one file: changing any of them changes them all, and they share one set of dates and attributes. Each copy is compared byte for byte first.`,
            { title: "Hard-link duplicates", kind: "warning", okLabel: "Hard-link", cancelLabel: "Cancel" }
        );
        if (!ok) return;
        setBusy(true);
        try {
            const res = await hardlinkDuplicates(keep, dups);
            toast.success(`Linked ${res.linked} ${res.linked === 1 ? "file" : "files"} · reclaimed ${formatSize(res.reclaimedBytes)} in ${formatDuration(res.elapsedMs)}`);
            reload();
            scan();
        } catch (e) {
            toast.error("Couldn't hard-link the duplicates", { description: String(e) });
        } finally {
            setBusy(false);
        }
    };

    const recycleOneGroup = async (group: DuplicateGroup) => {
        const dups = dupsFor(group);
        if (!dups.length) return;
        setBusy(true);
        try {
            const done = await fileOp({ kind: "delete", paths: dups, permanent: false, confirm: true });
            if (done) {
                toast.success(`Moved ${dups.length} duplicate ${dups.length === 1 ? "file" : "files"} to the Recycle Bin`);
                reload();
                scan();
            }
        } catch (e) {
            toast.error("Couldn't recycle duplicates", { description: String(e) });
        } finally {
            setBusy(false);
        }
    };

    const linkAllGroups = async () => {
        if (!report || !report.groups.length) return;
        const ok = await confirm(
            `Replace all ${formatCount(report.totalDuplicateFiles)} duplicate files in ${formatCount(report.groups.length)} groups with hard links to the copy you keep?\n\nEvery path stays where it is and ${formatSize(report.totalWastedBytes)} is freed, but each group becomes one file: changing any copy changes them all. Each copy is compared byte for byte first.`,
            { title: "Hard-link all duplicates", kind: "warning", okLabel: "Hard-link all", cancelLabel: "Cancel" }
        );
        if (!ok) return;
        setBusy(true);
        let totalLinked = 0;
        let totalReclaimed = 0;
        for (const group of report.groups) {
            try {
                const res = await hardlinkDuplicates(keptFor(group), dupsFor(group));
                totalLinked += res.linked;
                totalReclaimed += res.reclaimedBytes;
            } catch (e) {
                toast.error(`Couldn't hard-link “${group.files[0]?.name}”`, { description: String(e) });
                break;
            }
        }
        setBusy(false);
        if (totalLinked > 0) {
            toast.success(`Linked ${formatCount(totalLinked)} files · reclaimed ${formatSize(totalReclaimed)}`);
            reload();
            scan();
        }
    };

    const recycleAllGroups = async () => {
        if (!report || !report.groups.length) return;
        const allDups = report.groups.flatMap((g) => dupsFor(g));
        if (!allDups.length) return;
        setBusy(true);
        try {
            const done = await fileOp({ kind: "delete", paths: allDups, permanent: false, confirm: true });
            if (done) {
                toast.success(`Moved ${formatCount(allDups.length)} duplicate files to the Recycle Bin`);
                reload();
                scan();
            }
        } catch (e) {
            toast.error("Couldn't recycle duplicates", { description: String(e) });
        } finally {
            setBusy(false);
        }
    };

    const folderTitle = baseName(root) || root;
    /** A path from the folder scanned on (sub\beta.bin), so the name isn't what gets cut off. */
    const inRoot = (path: string) => {
        const base = root.replace(/[\\/]+$/, "");
        return path.toLowerCase().startsWith(`${base.toLowerCase()}\\`) ? path.slice(base.length + 1) : path;
    };

    return (
        <Modal label={`Duplicate files in ${folderTitle}`} onClose={onClose} className="max-h-[min(84vh,42rem)] max-w-[48rem]">
            <ModalHeader icon={Files} title={`Duplicate files in “${folderTitle}”`} subtitle={root} onClose={onClose} onRefresh={scan} refreshing={loading || busy}>
                {report && report.totalWastedBytes > 0 && (
                    <span className="shrink-0 rounded-md bg-amber-500/15 px-2 py-0.5 text-xs font-semibold text-amber-700 dark:text-amber-400">
                        {formatSize(report.totalWastedBytes)} reclaimable
                    </span>
                )}
            </ModalHeader>

            {report && (
                <div className="flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1 border-b bg-chrome/60 px-3.5 py-1.5 text-[0.6875rem] text-muted-foreground">
                    <Sparkles className="size-3 shrink-0 text-primary" />
                    <span>
                        <b className="font-medium text-foreground">{formatCount(report.filesScanned)}</b> files scanned →{" "}
                        <b className="font-medium text-foreground">{formatCount(report.sizeCandidates)}</b> same-size →{" "}
                        <b className="font-medium text-foreground">{formatCount(report.edgeCandidates)}</b> same start and end →{" "}
                        <b className="font-medium text-foreground">{formatCount(report.groups.length)}</b> duplicate{" "}
                        {report.groups.length === 1 ? "group" : "groups"}
                        {report.hardlinksSkipped > 0
                            ? ` (${formatCount(report.hardlinksSkipped)} already hard-linked, skipped)`
                            : ""}
                    </span>
                </div>
            )}

            <div className="min-h-0 flex-1 overflow-y-auto p-3">
                {loading ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-14 text-sm text-muted-foreground">
                        <Loader2 className="size-5 animate-spin text-primary" />
                        <span>Comparing sizes, then contents…</span>
                        <Button variant="outline" size="sm" onClick={() => void cancelDedup()}>
                            <Square className="size-3! fill-current" /> Stop
                        </Button>
                    </div>
                ) : stopped ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-14 text-center text-sm text-muted-foreground">
                        <p className="font-medium text-foreground">Stopped before it finished</p>
                        <Button variant="outline" size="sm" onClick={scan}>
                            <RotateCw /> Scan again
                        </Button>
                    </div>
                ) : error ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-12 text-center text-sm">
                        <AlertTriangle className="size-6 text-destructive" />
                        <p className="font-medium text-foreground">Couldn't scan for duplicates</p>
                        <p className="max-w-md text-xs text-muted-foreground">{error}</p>
                    </div>
                ) : report && report.groups.length === 0 ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-14 text-center text-sm text-muted-foreground">
                        <CheckCircle2 className="size-8 stroke-[1.5] text-emerald-500" />
                        <p className="font-medium text-foreground">No duplicate files found</p>
                        <p className="max-w-sm text-xs">
                            Checked {formatCount(report.filesScanned)} {report.filesScanned === 1 ? "file" : "files"} in {formatDuration(report.elapsedMs)}.
                            {report.hardlinksSkipped > 0 && ` (${formatCount(report.hardlinksSkipped)} already-hardlinked files were skipped.)`}
                        </p>
                    </div>
                ) : (
                    <div className="flex flex-col gap-2.5">
                        {report?.groups.map((group) => {
                            const kept = keptFor(group);
                            return (
                                <div key={group.hash} className="overflow-hidden rounded-lg border bg-card/50">
                                    <div className="flex items-center justify-between gap-2 border-b bg-muted/40 px-3 py-1.5 text-xs">
                                        <div className="flex min-w-0 items-center gap-2">
                                            <span className="font-semibold text-foreground">{formatSize(group.size)} each</span>
                                            <span className="text-muted-foreground">·</span>
                                            <span className="font-medium text-amber-700 dark:text-amber-400">
                                                {formatSize(group.wastedBytes)} duplicate
                                            </span>
                                            <span className="rounded-xs border bg-background px-1.5 py-0.2 font-mono text-[0.625rem] text-muted-foreground">
                                                #{group.hash.slice(0, 12)}
                                            </span>
                                        </div>
                                        <div className="flex shrink-0 items-center gap-1">
                                            <Button
                                                variant="outline"
                                                size="sm"
                                                disabled={busy}
                                                onClick={() => linkOneGroup(group)}
                                                className="h-6 gap-1 px-2 text-[0.6875rem]"
                                                title="Replace the other copies with hard links to the one you keep: the paths stay, the space is freed, and they become one file"
                                            >
                                                <Link2 className="size-3 text-primary" /> Hard-link
                                            </Button>
                                            <Button
                                                variant="outline"
                                                size="sm"
                                                disabled={busy}
                                                onClick={() => recycleOneGroup(group)}
                                                className="h-6 gap-1 border-destructive/30 px-2 text-[0.6875rem] text-destructive hover:bg-destructive/10 hover:text-destructive"
                                                title="Move non-kept copies to the Recycle Bin"
                                            >
                                                <Trash2 className="size-3" /> Recycle
                                            </Button>
                                        </div>
                                    </div>
                                    <div role="radiogroup" aria-label="The copy to keep" className="divide-y">
                                        {group.files.map((file) => {
                                            const isKept = file.path === kept;
                                            return (
                                                <div
                                                    key={file.path}
                                                    role="radio"
                                                    aria-checked={isKept}
                                                    aria-label={`Keep ${inRoot(file.path)}`}
                                                    {...pressable(() => setKeptByGroup((prev) => ({ ...prev, [group.hash]: file.path })))}
                                                    className={cn(
                                                        "flex cursor-pointer items-center justify-between gap-2 px-3 py-1.5 text-xs transition-colors hover:bg-accent/40",
                                                        isKept && "bg-primary/[0.04]"
                                                    )}
                                                >
                                                    <div className="flex min-w-0 items-center gap-2">
                                                        <span
                                                            className={cn(
                                                                "shrink-0 rounded-xs px-1.5 py-0.5 text-[0.625rem] font-semibold uppercase",
                                                                isKept
                                                                    ? "bg-emerald-500/15 text-emerald-700 dark:text-emerald-400"
                                                                    : "bg-muted text-muted-foreground"
                                                            )}
                                                            title={isKept ? "This copy will be kept" : "Click to keep this copy instead"}
                                                        >
                                                            {isKept ? "Keep" : "Duplicate"}
                                                        </span>
                                                        <span className="truncate font-mono text-foreground" title={file.path}>
                                                            {inRoot(file.path)}
                                                        </span>
                                                    </div>
                                                    <div className="flex shrink-0 items-center gap-2">
                                                        {file.modified > 0 && (
                                                            <span className="text-[0.6875rem] text-muted-foreground tabular-nums">
                                                                {formatDate(file.modified)}
                                                            </span>
                                                        )}
                                                        <Button
                                                            variant="ghost"
                                                            size="icon"
                                                            className="size-6"
                                                            title="Open file location"
                                                            onClick={(e) => {
                                                                e.stopPropagation();
                                                                onClose();
                                                                revealInApp(file.path);
                                                            }}
                                                        >
                                                            <FolderInput className="size-3.5" />
                                                        </Button>
                                                    </div>
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

            <div className="flex h-9 shrink-0 items-center justify-between border-t bg-chrome px-3.5 text-xs text-muted-foreground">
                <span className="flex items-center gap-1 truncate">
                    <Zap className="size-3 text-primary" />
                    {report
                        ? `${formatCount(report.groups.length)} ${report.groups.length === 1 ? "group" : "groups"} (${formatCount(report.totalDuplicateFiles)} ${report.totalDuplicateFiles === 1 ? "duplicate" : "duplicates"}) in ${formatDuration(report.elapsedMs)}`
                        : ""}
                </span>
                {report && report.groups.length > 1 && (
                    <div className="flex shrink-0 items-center gap-1.5">
                        <Button
                            variant="ghost"
                            size="sm"
                            disabled={busy}
                            onClick={linkAllGroups}
                            className="h-6 gap-1 px-2 text-xs text-primary hover:bg-primary/10 hover:text-primary"
                        >
                            <Link2 className="size-3" /> Hard-link all ({formatCount(report.totalDuplicateFiles)})
                        </Button>
                        <Button
                            variant="ghost"
                            size="sm"
                            disabled={busy}
                            onClick={recycleAllGroups}
                            className="h-6 gap-1 px-2 text-xs text-destructive hover:bg-destructive/10 hover:text-destructive"
                        >
                            <Trash2 className="size-3" /> Recycle all
                        </Button>
                    </div>
                )}
            </div>
        </Modal>
    );
}
