import { useEffect, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { Channel, invoke } from "@tauri-apps/api/core";
import { open as pickFolder } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, CheckCircle2, CloudUpload, DatabaseBackup, History, Loader2, Square } from "lucide-react";
import { type FileEntry, isArchived, isClosed, isOnlineOnly, isRecycled } from "@/lib/entries";
import { formatCount, formatDate, formatSize, plural } from "@/lib/format";
import { type RcloneRemote, rcloneRemotes } from "@/lib/rclone";
import { useTool } from "@/lib/tools";
import { ToolNeeded } from "../ToolNeeded";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";
import { Choices } from "./ActionParts";
import { openVersions } from "./VersionsDialog";

type Schedule = "manual" | "daily" | "weekly";
export interface LastRun {
    at: number;
    ok: boolean;
    snapshot: string;
    filesNew: number;
    filesChanged: number;
    bytesAdded: number;
    totalFiles: number;
    totalBytes: number;
    skipped: string[];
    skippedCount: number;
    error: string | null;
    elapsedMs: number;
}
export interface Plan {
    id: string;
    source: string;
    repository: string;
    schedule: Schedule;
    last: LastRun | null;
}
interface Progress {
    stage: string;
    fraction: number;
    filesDone: number;
    totalFiles: number;
    bytesDone: number;
    totalBytes: number;
    current: string;
}

/** A folder Back up takes: one on this PC's drives. */
export const canBackUp = (e: FileEntry) => e.dir && !isArchived(e) && !isClosed(e) && !isRecycled(e) && !isOnlineOnly(e) && /^[A-Za-z]:\\/.test(e.path);

const useBackup = create<{ folder: string | null }>(() => ({ folder: null }));

/** Opens Back up for a folder. */
export function openBackup(entry: FileEntry) {
    if (!canBackUp(entry)) return void toast.message("Back up works on folders on this PC's drives");
    useBackup.setState({ folder: entry.path });
}

const close = () => useBackup.setState({ folder: null });

/** Where a backup goes, in words. */
export const describeRepository = (repository: string) => (repository.startsWith("rclone:") ? `${repository.slice(7).split(":")[0]} (a cloud)` : repository);

export function BackupDialog() {
    const folder = useBackup((s) => s.folder);
    if (!folder) return null;
    return <BackupModal key={folder} folder={folder} />;
}

function BackupModal({ folder }: { folder: string }) {
    const restic = useTool("restic");
    const rclone = useTool("rclone");
    const [plan, setPlan] = useState<Plan | null | undefined>(undefined);
    const [where, setWhere] = useState<"drive" | "cloud">("drive");
    const [drive, setDrive] = useState("");
    const [remotes, setRemotes] = useState<RcloneRemote[] | null>(null);
    const [remote, setRemote] = useState("");
    const [password, setPassword] = useState("");
    const [again, setAgain] = useState("");
    const [schedule, setSchedule] = useState<Schedule>("daily");
    const [busy, setBusy] = useState<Progress | null>(null);
    const [result, setResult] = useState<LastRun | null>(null);
    const [error, setError] = useState<string | null>(null);
    const name = folder.split("\\").pop() || folder;

    useEffect(() => {
        invoke<Plan[]>("backup_plans").then(
            (all) => setPlan(all.find((p) => p.source.toLowerCase() === folder.toLowerCase()) ?? null),
            () => setPlan(null)
        );
    }, [folder]);
    useEffect(() => {
        if (where === "cloud" && rclone?.origin && remotes === null) rcloneRemotes().then((r) => (setRemotes(r), setRemote(r[0]?.name ?? "")), () => setRemotes([]));
    }, [where, rclone?.origin, remotes]);

    const repository = where === "drive" ? (drive ? `${drive.replace(/\\$/, "")}\\Glint backup` : "") : remote ? `rclone:${remote}:Glint backup` : "";
    const badPassword = password.length < 8 || password !== again;
    const ready = !!restic?.origin && (where === "drive" || !!rclone?.origin) && !!repository && !badPassword;

    const backUp = async (id: string) => {
        setError(null);
        setResult(null);
        const channel = new Channel<Progress>();
        channel.onmessage = setBusy;
        setBusy({ stage: "Starting", fraction: 0, filesDone: 0, totalFiles: 0, bytesDone: 0, totalBytes: 0, current: "" });
        try {
            const done = await invoke<LastRun>("backup_run", { id, onProgress: channel });
            setResult(done);
            if (!done.ok) setError(done.error ?? "The backup didn't finish");
            setPlan((p) => (p ? { ...p, last: done } : p));
        } catch (e) {
            if (String(e) !== "cancelled") setError(String(e));
        } finally {
            setBusy(null);
        }
    };

    const setUp = async () => {
        setError(null);
        setBusy({ stage: "Setting up the backup", fraction: 0, filesDone: 0, totalFiles: 0, bytesDone: 0, totalBytes: 0, current: "" });
        try {
            const made = await invoke<Plan>("backup_setup", { source: folder, repository, password, schedule });
            setPlan(made);
            setPassword("");
            setAgain("");
            await backUp(made.id);
        } catch (e) {
            setError(String(e));
            setBusy(null);
        }
    };

    return (
        <Modal label="Back up" onClose={() => !busy && close()} onSubmit={() => (plan ? !busy && void backUp(plan.id) : ready && !busy && void setUp())} className="max-h-[88vh] max-w-xl">
            <ModalHeader icon={DatabaseBackup} title={`Back up “${name}”`} subtitle={folder} onClose={() => !busy && close()} />
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                <ToolNeeded id="restic" why="restic backs folders up, encrypted, each backup adding only what changed." icon={DatabaseBackup} />
                {plan === undefined && <p className="text-muted-foreground">Looking…</p>}
                {busy && <BackupRunning progress={busy} />}
                {!busy && result && result.ok && <BackupDone run={result} />}
                {!busy && error && (
                    <div className="flex gap-2 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
                        <AlertTriangle className="size-4 shrink-0" /> {error}
                    </div>
                )}
                {!busy && plan && (
                    <div className="space-y-3">
                        <p>
                            Backed up to <span className="font-medium">{describeRepository(plan.repository)}</span>
                            {plan.last && (
                                <span className="text-muted-foreground">
                                    {" "}
                                    · last {plan.last.ok ? "backup" : "try"} {formatDate(plan.last.at)}
                                    {plan.last.ok ? `, ${formatCount(plan.last.totalFiles)} files, ${formatSize(plan.last.totalBytes)}` : ""}
                                </span>
                            )}
                        </p>
                        {plan.last && !plan.last.ok && plan.last.error && !error && <p className="text-xs text-destructive">{plan.last.error}</p>}
                        <section className="space-y-1.5">
                            <h3 className="text-xs font-semibold text-muted-foreground">When</h3>
                            <Choices
                                label="When"
                                columns={3}
                                value={plan.schedule}
                                onChange={(s) => void invoke<Plan>("backup_schedule", { id: plan.id, schedule: s }).then(setPlan, (e) => setError(String(e)))}
                                options={[
                                    { value: "manual", label: "When I ask", hint: "Back up now" },
                                    { value: "daily", label: "Every day", hint: "While Glint runs" },
                                    { value: "weekly", label: "Every week", hint: "While Glint runs" },
                                ]}
                            />
                        </section>
                        <p className="text-xs text-muted-foreground">Glint keeps 7 daily, 4 weekly and 12 monthly backups, and checks them once a month. They show in Previous versions, beside Windows' own.</p>
                    </div>
                )}
                {!busy && plan === null && !result && (
                    <>
                        <p className="text-muted-foreground">Each backup adds only what changed since the last, encrypted, so even a cloud sees nothing but scrambled data. Earlier backups of every file stay to go back to.</p>
                        <section className="space-y-1.5">
                            <h3 className="text-xs font-semibold text-muted-foreground">To</h3>
                            <Choices
                                label="To"
                                value={where}
                                onChange={setWhere}
                                options={[
                                    { value: "drive", label: "A drive or folder", hint: "A USB drive, another disk, a network folder" },
                                    { value: "cloud", label: "A cloud", hint: "One added in Remote (rclone)" },
                                ]}
                            />
                            {where === "drive" ? (
                                <div className="flex items-center gap-2">
                                    <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">{drive ? `${drive.replace(/\\$/, "")}\\Glint backup` : "No place chosen yet"}</span>
                                    <Button
                                        size="sm"
                                        variant="outline"
                                        onClick={async () => {
                                            const picked = await pickFolder({ directory: true, multiple: false });
                                            if (typeof picked === "string") setDrive(picked);
                                        }}
                                    >
                                        Choose…
                                    </Button>
                                </div>
                            ) : rclone && !rclone.origin ? (
                                <ToolNeeded id="rclone" why="rclone reaches the clouds you've added in Remote." icon={CloudUpload} />
                            ) : remotes && !remotes.length ? (
                                <p className="text-xs text-muted-foreground">No cloud has been added yet: add one under Remote in the sidebar, then come back.</p>
                            ) : (
                                <select value={remote} onChange={(e) => setRemote(e.target.value)} aria-label="Cloud" className="h-8 w-full rounded-md border bg-background px-2">
                                    {(remotes ?? []).map((r) => (
                                        <option key={r.name} value={r.name}>
                                            {r.name} ({r.kind})
                                        </option>
                                    ))}
                                </select>
                            )}
                        </section>
                        <section className="space-y-2">
                            <h3 className="text-xs font-semibold text-muted-foreground">Password</h3>
                            <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} placeholder="At least 8 characters" autoComplete="new-password" aria-label="Password" className="h-8 w-full rounded-md border bg-background px-2" />
                            <input type="password" value={again} onChange={(e) => setAgain(e.target.value)} placeholder="Again" autoComplete="new-password" aria-label="The password again" className={`h-8 w-full rounded-md border bg-background px-2 ${again && again !== password ? "border-destructive" : ""}`} />
                            <p className="text-xs text-amber-700 dark:text-amber-400">Without this password nobody can read the backup, Glint neither. It's kept in Windows Credential Manager on this PC: write it down somewhere else too. An existing backup at that place opens with its own password.</p>
                        </section>
                        <section className="space-y-1.5">
                            <h3 className="text-xs font-semibold text-muted-foreground">When</h3>
                            <Choices
                                label="When"
                                columns={3}
                                value={schedule}
                                onChange={setSchedule}
                                options={[
                                    { value: "manual", label: "When I ask", hint: "From here" },
                                    { value: "daily", label: "Every day", hint: "While Glint runs" },
                                    { value: "weekly", label: "Every week", hint: "While Glint runs" },
                                ]}
                            />
                        </section>
                        <p className="text-xs text-muted-foreground">Files an app keeps open (Outlook's mailbox, say) are skipped and listed.</p>
                    </>
                )}
            </div>
            <div className="flex items-center justify-end gap-2 border-t bg-muted/20 px-4 py-2.5">
                {busy ? (
                    <Button variant="outline" size="sm" onClick={() => void invoke("backup_cancel")}>
                        <Square className="size-3! fill-current" /> Stop
                    </Button>
                ) : (
                    <>
                        {plan && (
                            <>
                                <Button
                                    variant="ghost"
                                    size="sm"
                                    className="me-auto"
                                    onClick={() =>
                                        void invoke("backup_remove", { id: plan.id }).then(() => {
                                            toast.message(`“${name}” isn't backed up any more`, { description: "The backups made so far stay where they are." });
                                            close();
                                        })
                                    }
                                >
                                    Stop backing up
                                </Button>
                                <Button
                                    variant="outline"
                                    size="sm"
                                    onClick={() => {
                                        close();
                                        openVersions(folder);
                                    }}
                                >
                                    <History /> Backups
                                </Button>
                            </>
                        )}
                        <Button variant="outline" size="sm" onClick={close}>
                            Close
                        </Button>
                        {plan ? (
                            <Button size="sm" onClick={() => void backUp(plan.id)} disabled={!restic?.origin}>
                                <DatabaseBackup /> Back up now
                            </Button>
                        ) : (
                            !result && (
                                <Button size="sm" onClick={() => void setUp()} disabled={!ready}>
                                    <DatabaseBackup /> Back up
                                </Button>
                            )
                        )}
                    </>
                )}
            </div>
        </Modal>
    );
}

function BackupRunning({ progress }: { progress: Progress }) {
    return (
        <div className="space-y-2 py-4">
            <div className="flex items-center gap-2 text-muted-foreground">
                <Loader2 className="size-4 animate-spin text-primary" />
                <span className="truncate">
                    {progress.stage}
                    {progress.totalFiles > 0 && ` · ${formatCount(progress.filesDone)} of ${formatCount(progress.totalFiles)} files, ${formatSize(progress.bytesDone)} of ${formatSize(progress.totalBytes)}`}
                </span>
            </div>
            <div className="h-1.5 overflow-hidden rounded-full bg-muted">
                <div className="h-full rounded-full bg-primary transition-[width]" style={{ width: `${Math.round(progress.fraction * 100)}%` }} />
            </div>
            {progress.current && <p className="truncate text-xs text-muted-foreground">{progress.current}</p>}
        </div>
    );
}

function BackupDone({ run }: { run: LastRun }) {
    return (
        <div className="space-y-2">
            <div className="flex items-start gap-2">
                <CheckCircle2 className="mt-0.5 size-5 shrink-0 text-emerald-500" />
                <div>
                    <p className="font-medium">
                        Backed up {formatCount(run.totalFiles)} files, {formatSize(run.totalBytes)}
                    </p>
                    <p className="text-xs text-muted-foreground">
                        {run.filesNew || run.filesChanged ? `${plural(run.filesNew, "new file")}, ${plural(run.filesChanged, "changed file")}; ${formatSize(run.bytesAdded)} added` : "Nothing had changed since the last backup"} · {Math.round(run.elapsedMs / 100) / 10} s
                    </p>
                </div>
            </div>
            {run.skippedCount > 0 && (
                <div className="rounded-md bg-amber-500/10 px-3 py-2 text-xs text-amber-900 dark:text-amber-300">
                    <p className="font-medium">{plural(run.skippedCount, "file")} couldn't be read (an app holds {run.skippedCount === 1 ? "it" : "them"} open):</p>
                    <ul className="mt-1 space-y-0.5">
                        {run.skipped.slice(0, 5).map((p) => (
                            <li key={p} className="truncate" title={p}>
                                {p}
                            </li>
                        ))}
                    </ul>
                </div>
            )}
            {run.error && <p className="text-xs text-amber-700 dark:text-amber-400">{run.error}</p>}
        </div>
    );
}
