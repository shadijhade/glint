import { useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { openPath } from "@tauri-apps/plugin-opener";
import { ArchiveRestore, Columns2, Copy, ExternalLink, FolderOpen, History, Loader2 } from "lucide-react";
import { compareWithVersion, navigate, nameShadowCopy, openInNewTab, openVersionFile } from "@/lib/explorer";
import { formatFullDate, formatSize } from "@/lib/format";
import { type PreviousVersion, previousVersions, restoreVersion } from "@/lib/fs";
import { baseName, parentPath } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";
import { compareFiles } from "./CompareFilesDialog";

interface VersionsState {
    /** The file or folder whose versions are shown ("" when closed). */
    path: string;
    loading: boolean;
    versions: PreviousVersion[];
    error: string | null;
}

const useVersions = create<VersionsState>(() => ({ path: "", loading: false, versions: [], error: null }));
let asked = 0;

async function load() {
    const { path } = useVersions.getState();
    const id = ++asked;
    useVersions.setState({ loading: true, error: null });
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
    useVersions.setState({ path, versions: [], error: null });
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

export function VersionsDialog() {
    const { path, loading, versions, error } = useVersions();
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
                {!loading && !error && !versions.length && (
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
            </div>
            {near && <div className="border-t px-4 py-2 text-xs text-muted-foreground">Times marked ≈ are when the copy last changed, close to when it was taken. With instant search on, Windows says exactly.</div>}
        </Modal>
    );
}
