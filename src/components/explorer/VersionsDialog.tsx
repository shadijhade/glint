import { useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { openPath } from "@tauri-apps/plugin-opener";
import { invoke } from "@tauri-apps/api/core";
import { ArchiveRestore, Columns2, Copy, DatabaseBackup, ExternalLink, FolderOpen, History, Loader2 } from "lucide-react";
import { compareWithVersion, navigate, nameShadowCopy, openInNewTab, openVersionFile } from "@/lib/explorer";
import { formatFullDate, formatSize } from "@/lib/format";
import { type PreviousVersion, previousVersions, restoreVersion } from "@/lib/fs";
import { baseName, parentPath } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";
import { compareFiles } from "./CompareFilesDialog";

/** A backup Glint made (backup.rs) that holds the file or folder. */
interface BackupVersion {
    plan: string;
    repository: string;
    snapshot: string;
    time: number;
    dir: boolean;
}

interface VersionsState {
    /** The file or folder whose versions are shown ("" when closed). */
    path: string;
    loading: boolean;
    versions: PreviousVersion[];
    error: string | null;
    backups: BackupVersion[];
    backupError: string | null;
}

const useVersions = create<VersionsState>(() => ({ path: "", loading: false, versions: [], error: null, backups: [], backupError: null }));
let asked = 0;

async function load() {
    const { path } = useVersions.getState();
    const id = ++asked;
    useVersions.setState({ loading: true, error: null });
    // Glint's own backups of the folders it's in, read alongside.
    invoke<BackupVersion[]>("backup_versions", { path }).then(
        (backups) => id === asked && useVersions.setState({ backups, backupError: null }),
        (e) => id === asked && useVersions.setState({ backups: [], backupError: String(e) })
    );
    try {
        const versions = await previousVersions(path);
        for (const v of versions) nameShadowCopy(v.root, path.slice(0, 2).toUpperCase(), v.created, v.exact);
        if (id === asked) useVersions.setState({ versions, loading: false });
    } catch (e) {
        if (id === asked) useVersions.setState({ versions: [], error: String(e), loading: false });
    }
}

/** Opens Previous versions for a file or folder on one of this PC's drives. */
export function openVersions(path: string) {
    useVersions.setState({ path, versions: [], error: null, backups: [], backupError: null });
    void load();
}

function close() {
    asked++;
    useVersions.setState({ path: "" });
}

/** "About" for a time found from the copy itself rather than told by VSS. */
const when = (v: PreviousVersion) => (v.created ? `${v.exact ? "" : "≈ "}${formatFullDate(v.created)}` : "When isn't known");

async function restore(v: PreviousVersion, now: string, replace: boolean) {
    try {
        const at = await restoreVersion(v.path, now, replace);
        toast.success(replace ? `Restored “${baseName(now)}”` : `Restored a copy: “${baseName(at)}”`, {
            description: replace ? "What was there is in the Recycle Bin." : undefined,
            action: { label: "Show", onClick: () => void navigate(parentPath(at), { select: at }) },
        });
        void load();
    } catch (e) {
        toast.error("Couldn't restore it", { description: String(e) });
    }
}

/** Where a backup is, in words. */
const placeOf = (repository: string) => (repository.startsWith("rclone:") ? `${repository.slice(7).split(":")[0]} (a cloud)` : repository);

/** "backup 2026-11-09", for a restored copy's name. */
const backupLabel = (time: number) => {
    const d = new Date(time);
    return `backup ${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};

async function openBackup(b: BackupVersion, path: string, compare: boolean) {
    const id = toast.loading(compare ? "Getting it out of the backup to compare…" : "Getting it out of the backup…");
    try {
        const made = await invoke<string>("backup_open", { plan: b.plan, snapshot: b.snapshot, path, dir: b.dir });
        toast.dismiss(id);
        if (compare) {
            close();
            if (b.dir) void compareWithVersion(path, made);
            else compareFiles(made, path);
        } else if (b.dir) openInNewTab(made);
        else await invoke("open_item", { path: made });
    } catch (e) {
        toast.error("Couldn't get it out of the backup", { id, description: String(e) });
    }
}

async function restoreBackup(b: BackupVersion, path: string, replace: boolean) {
    const id = toast.loading("Restoring from the backup…");
    try {
        const at = await invoke<string>("backup_restore", { plan: b.plan, snapshot: b.snapshot, path, dir: b.dir, replace, label: backupLabel(b.time) });
        toast.success(replace ? `Restored “${baseName(path)}”` : `Restored a copy: “${baseName(at)}”`, {
            id,
            description: replace ? "What was there is in the Recycle Bin." : undefined,
            action: { label: "Show", onClick: () => void navigate(parentPath(at), { select: at }) },
        });
    } catch (e) {
        toast.error("Couldn't restore it", { id, description: String(e) });
    }
}

export function VersionsDialog() {
    const { path, loading, versions, error, backups, backupError } = useVersions();
    const [replacing, setReplacing] = useState<string | null>(null);
    if (!path) return null;
    const name = baseName(path);
    const near = versions.some((v) => !v.exact);
    return (
        <Modal label="Previous versions" onClose={close} className="max-h-[86vh] max-w-3xl">
            <ModalHeader icon={History} title={`Previous versions of “${name}”`} subtitle="As Windows' shadow copies kept it: restore points and backups. They're read-only; restoring copies out of them." onClose={close} onRefresh={() => void load()} refreshing={loading} />
            <div className="min-h-[16rem] flex-1 overflow-y-auto p-3">
                {loading && !versions.length && (
                    <div className="flex h-56 items-center justify-center">
                        <Loader2 className="size-6 animate-spin text-primary" />
                    </div>
                )}
                {error && <div className="flex h-56 items-center justify-center px-6 text-center text-sm text-destructive">{error}</div>}
                {!loading && !error && !versions.length && !backups.length && (
                    <div className="flex h-56 flex-col items-center justify-center gap-3 px-8 text-center text-sm text-muted-foreground">
                        <p>No earlier version of “{name}” is kept on this PC.</p>
                        <p className="text-xs">Windows keeps them when System Protection is on for the drive (its restore points) or a backup makes shadow copies; then a file's versions show here from the next one on.</p>
                        <Button variant="outline" size="sm" onClick={() => void openPath("C:\\Windows\\System32\\SystemPropertiesProtection.exe").catch((e) => toast.error("Couldn't open System Protection", { description: String(e) }))}>
                            System Protection…
                        </Button>
                    </div>
                )}
                <div className="space-y-1">
                    {versions.map((v) => (
                        <div key={v.path} className="group flex items-center gap-3 rounded-md border bg-card/40 px-3 py-2 text-sm hover:bg-muted/40">
                            <div className="min-w-0 flex-1">
                                <div className="font-medium">{when(v)}</div>
                                <div className="truncate text-xs text-muted-foreground" title={v.path}>
                                    {v.dir ? "The folder as it was" : `${formatSize(v.size)} · modified ${formatFullDate(v.modified)}`}
                                    {v.gone && <span className="ms-1.5 rounded bg-amber-500/15 px-1.5 py-0.5 text-amber-800 dark:text-amber-400">gone now</span>}
                                </div>
                            </div>
                            {replacing === v.path ? (
                                <div className="flex items-center gap-1.5 text-xs">
                                    <span className="text-muted-foreground">Replace what's there now? It goes to the Recycle Bin.</span>
                                    <Button size="sm" variant="destructive" onClick={() => (setReplacing(null), void restore(v, path, true))}>
                                        Replace
                                    </Button>
                                    <Button size="sm" variant="ghost" onClick={() => setReplacing(null)}>
                                        Cancel
                                    </Button>
                                </div>
                            ) : (
                                <div className={cn("flex items-center gap-1")}>
                                    <Button size="sm" variant="ghost" title={v.dir ? "Open the folder as it was, in a new tab" : "Open a read-only copy of it"} onClick={() => (v.dir ? openInNewTab(v.path) : void openVersionFile(v.path))}>
                                        {v.dir ? <FolderOpen className="size-4" /> : <ExternalLink className="size-4" />}
                                        Open
                                    </Button>
                                    {!v.gone && (
                                        <Button
                                            size="sm"
                                            variant="ghost"
                                            title={v.dir ? "This folder now beside how it was, compared" : "What changed in it since"}
                                            onClick={() => {
                                                close();
                                                if (v.dir) void compareWithVersion(path, v.path);
                                                else compareFiles(v.path, path);
                                            }}
                                        >
                                            <Columns2 className="size-4" />
                                            Compare
                                        </Button>
                                    )}
                                    <Button size="sm" variant="ghost" title="Copy it out beside what's there now, under a new name" onClick={() => void restore(v, path, false)}>
                                        <Copy className="size-4" />
                                        Restore a copy
                                    </Button>
                                    {!v.dir && (
                                        <Button size="sm" variant="ghost" title={v.gone ? "Put it back where it was" : "Put it back in place of what's there now"} onClick={() => (v.gone ? void restore(v, path, true) : setReplacing(v.path))}>
                                            <ArchiveRestore className="size-4" />
                                            Restore
                                        </Button>
                                    )}
                                </div>
                            )}
                        </div>
                    ))}
                </div>
                {(backups.length > 0 || backupError) && (
                    <div className="mt-3 space-y-1">
                        <h3 className="flex items-center gap-1.5 px-1 text-xs font-semibold text-muted-foreground">
                            <DatabaseBackup className="size-3.5" /> Glint's backups
                        </h3>
                        {backupError && <p className="px-1 text-xs text-destructive">{backupError}</p>}
                        {backups.map((b) => (
                            <div key={`${b.plan}-${b.snapshot}`} className="flex items-center gap-3 rounded-md border bg-card/40 px-3 py-2 text-sm hover:bg-muted/40">
                                <div className="min-w-0 flex-1">
                                    <div className="font-medium">{formatFullDate(b.time)}</div>
                                    <div className="truncate text-xs text-muted-foreground" title={b.repository}>
                                        Backed up to {placeOf(b.repository)}
                                    </div>
                                </div>
                                {replacing === b.snapshot ? (
                                    <div className="flex items-center gap-1.5 text-xs">
                                        <span className="text-muted-foreground">Replace what's there now? It goes to the Recycle Bin.</span>
                                        <Button size="sm" variant="destructive" onClick={() => (setReplacing(null), void restoreBackup(b, path, true))}>
                                            Replace
                                        </Button>
                                        <Button size="sm" variant="ghost" onClick={() => setReplacing(null)}>
                                            Cancel
                                        </Button>
                                    </div>
                                ) : (
                                    <div className="flex items-center gap-1">
                                        <Button size="sm" variant="ghost" title={b.dir ? "Open the folder as it was, in a new tab" : "Open a copy of it as it was"} onClick={() => void openBackup(b, path, false)}>
                                            {b.dir ? <FolderOpen className="size-4" /> : <ExternalLink className="size-4" />}
                                            Open
                                        </Button>
                                        <Button size="sm" variant="ghost" title={b.dir ? "This folder now beside how it was, compared" : "What changed in it since"} onClick={() => void openBackup(b, path, true)}>
                                            <Columns2 className="size-4" />
                                            Compare
                                        </Button>
                                        <Button size="sm" variant="ghost" title="Copy it out beside what's there now, under a new name" onClick={() => void restoreBackup(b, path, false)}>
                                            <Copy className="size-4" />
                                            Restore a copy
                                        </Button>
                                        {!b.dir && (
                                            <Button size="sm" variant="ghost" title="Put it back in place of what's there now" onClick={() => setReplacing(b.snapshot)}>
                                                <ArchiveRestore className="size-4" />
                                                Restore
                                            </Button>
                                        )}
                                    </div>
                                )}
                            </div>
                        ))}
                    </div>
                )}
            </div>
            {near && <div className="border-t px-4 py-2 text-xs text-muted-foreground">Times marked ≈ are when the copy last changed, close to when it was taken. With instant search on, Windows says exactly.</div>}
        </Modal>
    );
}
