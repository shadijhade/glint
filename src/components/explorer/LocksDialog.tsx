import { useCallback, useEffect, useState } from "react";
import { create } from "zustand";
import { confirm } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import {
    AlertTriangle,
    AppWindow,
    ClipboardCopy,
    Cog,
    Folder,
    FolderInput,
    Loader2,
    Lock,
    LockOpen,
    OctagonX,
    RotateCw,
    ShieldAlert,
    SquareTerminal,
    X,
} from "lucide-react";
import { revealInApp } from "@/lib/explorer";
import { formatCount, formatDuration } from "@/lib/format";
import { type LockReport, type LockingProcess, stopLockingProcess, whoLocks } from "@/lib/fs";
import { baseName } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal } from "../Modal";

interface LocksState {
    paths: string[] | null;
}

const useLocksStore = create<LocksState>(() => ({ paths: null }));

/** Opens the kernel lock inspector modal for the given file or folder paths. */
export function inspectLocks(paths: string[]) {
    if (!paths.length) return;
    useLocksStore.setState({ paths });
}

export function closeLocks() {
    useLocksStore.setState({ paths: null });
}

function ProcessIcon({ kind }: { kind: LockingProcess["kind"] }) {
    switch (kind) {
        case "console":
            return <SquareTerminal className="size-4 shrink-0 text-amber-500" />;
        case "service":
            return <Cog className="size-4 shrink-0 text-sky-500" />;
        case "explorer":
            return <Folder className="size-4 shrink-0 text-amber-500" />;
        case "critical":
            return <ShieldAlert className="size-4 shrink-0 text-destructive" />;
        default:
            return <AppWindow className="size-4 shrink-0 text-primary" />;
    }
}

function kindBadge(proc: LockingProcess) {
    if (proc.service) return `Service: ${proc.service}`;
    switch (proc.kind) {
        case "console":
            return "Console";
        case "service":
            return "Service";
        case "explorer":
            return "Explorer";
        case "critical":
            return "System critical";
        default:
            return "Application";
    }
}

export function LocksDialog() {
    const paths = useLocksStore((s) => s.paths);
    if (!paths || !paths.length) return null;
    return <LocksModal paths={paths} onClose={closeLocks} />;
}

/** What End all ends: not File Explorer (the desktop goes with it) or services, which need one choice each. */
const endable = (p: LockingProcess) => p.stoppable && p.kind !== "explorer" && p.kind !== "service";

function LocksModal({ paths, onClose }: { paths: string[]; onClose: () => void }) {
    const [report, setReport] = useState<LockReport | null>(null);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const [stoppingPid, setStoppingPid] = useState<number | null>(null);

    const scan = useCallback(() => {
        setLoading(true);
        setError(null);
        whoLocks(paths).then(
            (res) => {
                setReport(res);
                setLoading(false);
            },
            (e) => {
                setError(String(e));
                setLoading(false);
            }
        );
    }, [paths]);

    useEffect(() => {
        scan();
    }, [scan]);

    useEffect(() => {
        const onKeyDown = (e: KeyboardEvent) => {
            if (e.key === "Escape") {
                e.preventDefault();
                e.stopPropagation();
                onClose();
            } else if (e.key === "F5") {
                e.preventDefault();
                e.stopPropagation();
                scan();
            }
        };
        window.addEventListener("keydown", onKeyDown, true);
        return () => window.removeEventListener("keydown", onKeyDown, true);
    }, [onClose, scan]);

    const stopOne = async (proc: LockingProcess) => {
        const ok = await confirm(
            `End “${proc.name}” (PID ${proc.pid}) to release its file handles?\n\nUnsaved work in that process may be lost.`,
            { title: "End locking process", kind: "warning", okLabel: "End process", cancelLabel: "Cancel" }
        );
        if (!ok) return;
        setStoppingPid(proc.pid);
        try {
            await stopLockingProcess(proc.pid, proc.startTimeLow, proc.startTimeHigh);
            toast.success(`Ended ${proc.name} (PID ${proc.pid})`, { duration: 2000 });
            scan();
        } catch (e) {
            toast.error(`Couldn't end ${proc.name}`, { description: String(e) });
        } finally {
            setStoppingPid(null);
        }
    };

    const stopAll = async () => {
        const stoppable = report?.processes.filter(endable) ?? [];
        if (!stoppable.length) return;
        const ok = await confirm(
            `End ${stoppable.length} ${stoppable.length === 1 ? "program" : "programs"} (${stoppable.map((p) => p.name).join(", ")})?\n\nUnsaved work in them may be lost. File Explorer and services are left running; end those one at a time.`,
            { title: "End all locking processes", kind: "warning", okLabel: "End all", cancelLabel: "Cancel" }
        );
        if (!ok) return;
        setStoppingPid(-1);
        let stopped = 0;
        for (const proc of stoppable) {
            try {
                await stopLockingProcess(proc.pid, proc.startTimeLow, proc.startTimeHigh);
                stopped++;
            } catch (e) {
                toast.error(`Couldn't end ${proc.name}`, { description: String(e) });
            }
        }
        if (stopped > 0) {
            toast.success(`Ended ${stopped} ${stopped === 1 ? "process" : "processes"}`, { duration: 2000 });
        }
        setStoppingPid(null);
        scan();
    };

    const copyInfo = (text: string, what: string) => {
        navigator.clipboard.writeText(text).then(
            () => toast.success(`Copied ${what}`, { duration: 1500 }),
            () => toast.error(`Couldn't copy ${what}`)
        );
    };

    const title = paths.length === 1 ? `What's locking “${baseName(paths[0]) || paths[0]}”` : `What's locking ${formatCount(paths.length)} items`;
    const stoppableCount = report?.processes.filter(endable).length ?? 0;

    return (
        <Modal label={title} onClose={onClose} className="max-h-[min(80vh,34rem)] max-w-[40rem]">
            <div className="flex h-11 shrink-0 items-center gap-2.5 border-b px-3.5">
                <Lock className="size-4 shrink-0 text-primary" />
                <span className="min-w-0 flex-1 truncate text-sm font-medium" title={paths.join("\n")}>
                    {title}
                </span>
                <Button variant="ghost" size="icon" className="size-7" onClick={scan} disabled={loading} title="Refresh (F5)" aria-label="Refresh">
                    <RotateCw className={cn("size-3.5", loading && "animate-spin")} />
                </Button>
                <Button variant="ghost" size="icon" className="size-7" onClick={onClose} title="Close (Esc)" aria-label="Close">
                    <X className="size-4" />
                </Button>
            </div>

            <div className="min-h-0 flex-1 overflow-y-auto p-3">
                {loading && !report ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-12 text-sm text-muted-foreground">
                        <Loader2 className="size-5 animate-spin text-primary" />
                        <span>Checking Windows kernel handles…</span>
                    </div>
                ) : error ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-10 text-center text-sm">
                        <AlertTriangle className="size-6 text-destructive" />
                        <p className="font-medium text-foreground">Couldn't find out what's using it</p>
                        <p className="max-w-md text-xs text-muted-foreground">{error}</p>
                    </div>
                ) : report && report.processes.length === 0 ? (
                    <div className="flex flex-col items-center justify-center gap-2 py-12 text-center text-sm text-muted-foreground">
                        <LockOpen className="size-8 stroke-[1.5] text-emerald-500" />
                        <p className="font-medium text-foreground">Nothing is using {report.filesChecked === 1 ? "it" : "them"}</p>
                        <p className="max-w-sm text-xs">
                            {report.filesChecked === 0
                                ? "There are no files in it to check."
                                : report.filesChecked === 1
                                  ? "No program has the file open right now."
                                  : `No program has any of the ${formatCount(report.filesChecked)} files open right now.`}
                        </p>
                    </div>
                ) : (
                    <div className="flex flex-col gap-1.5">
                        {report?.processes.map((proc) => (
                            <div
                                key={proc.pid}
                                className="flex items-center gap-3 rounded-lg border bg-card/60 px-3 py-2.5 transition-colors hover:bg-accent/40"
                            >
                                <ProcessIcon kind={proc.kind} />
                                <div className="min-w-0 flex-1">
                                    <div className="flex items-center gap-2">
                                        <span className="truncate text-sm font-medium text-foreground">{proc.name}</span>
                                        <span className="shrink-0 rounded-sm border bg-muted px-1.5 py-0.5 font-mono text-[0.6875rem] text-muted-foreground">
                                            PID {proc.pid}
                                        </span>
                                        <span className="shrink-0 rounded-sm bg-primary/10 px-1.5 py-0.5 text-[0.6875rem] font-medium text-primary">
                                            {kindBadge(proc)}
                                        </span>
                                    </div>
                                    {proc.exe && (
                                        <p className="mt-0.5 truncate font-mono text-xs text-muted-foreground" title={proc.exe}>
                                            {proc.exe}
                                        </p>
                                    )}
                                </div>
                                <div className="flex shrink-0 items-center gap-1">
                                    {proc.exe && (
                                        <>
                                            <Button
                                                variant="ghost"
                                                size="icon"
                                                className="size-7"
                                                title="Reveal executable in Glint"
                                                onClick={() => {
                                                    onClose();
                                                    revealInApp(proc.exe!);
                                                }}
                                            >
                                                <FolderInput className="size-3.5" />
                                            </Button>
                                            <Button
                                                variant="ghost"
                                                size="icon"
                                                className="size-7"
                                                title="Copy executable path"
                                                onClick={() => copyInfo(proc.exe!, "executable path")}
                                            >
                                                <ClipboardCopy className="size-3.5" />
                                            </Button>
                                        </>
                                    )}
                                    {proc.stoppable && (
                                        <Button
                                            variant="outline"
                                            size="sm"
                                            disabled={stoppingPid !== null}
                                            onClick={() => stopOne(proc)}
                                            className="h-7 gap-1 border-destructive/30 px-2 text-xs text-destructive hover:bg-destructive/10 hover:text-destructive"
                                            title={`End ${proc.name} (PID ${proc.pid})`}
                                        >
                                            {stoppingPid === proc.pid ? <Loader2 className="size-3 animate-spin" /> : <OctagonX className="size-3" />}
                                            End
                                        </Button>
                                    )}
                                </div>
                            </div>
                        ))}
                    </div>
                )}
            </div>

            <div className="flex h-9 shrink-0 items-center justify-between border-t bg-chrome px-3.5 text-xs text-muted-foreground">
                <span className="truncate">
                    {report
                        ? `${formatCount(report.processes.length)} ${report.processes.length === 1 ? "program" : "programs"} · ${formatCount(report.filesChecked)} ${report.filesChecked === 1 ? "file" : "files"} checked in ${formatDuration(report.elapsedMs)}`
                        : ""}
                </span>
                {stoppableCount > 1 && (
                    <Button
                        variant="ghost"
                        size="sm"
                        disabled={stoppingPid !== null}
                        onClick={stopAll}
                        className="h-6 gap-1 px-2 text-xs text-destructive hover:bg-destructive/10 hover:text-destructive"
                    >
                        <OctagonX className="size-3" /> End all ({stoppableCount})
                    </Button>
                )}
            </div>
        </Modal>
    );
}
