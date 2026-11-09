import { useEffect, useState } from "react";
import { ChevronRight, File, Folder } from "lucide-react";
import type { FileEntry } from "@/lib/entries";
import { isHidden } from "@/lib/entries";
import { navigate } from "@/lib/explorer";
import { formatCount, formatSize } from "@/lib/format";
import { listDir } from "@/lib/fs";
import { baseName, isFolderPath, isRoot, parentPath, samePath, THIS_PC } from "@/lib/paths";
import { cn } from "@/lib/utils";

/** What a column lists: folders first, by name, as the list sorts them by default. */
function useListing(path: string | null, showHidden: boolean) {
    const [entries, setEntries] = useState<FileEntry[] | null>(null);
    useEffect(() => {
        setEntries(null);
        if (!path || !isFolderPath(path)) return;
        let stale = false;
        listDir(path).then(
            (listing) => !stale && setEntries(listing.entries.filter((e) => showHidden || !isHidden(e))),
            () => !stale && setEntries([])
        );
        return () => {
            stale = true;
        };
    }, [path, showHidden]);
    return entries;
}

function Row({ entry, current, onOpen }: { entry: FileEntry; current?: boolean; onOpen: () => void }) {
    return (
        <button
            type="button"
            onClick={onOpen}
            title={entry.name}
            className={cn("flex h-7 w-full items-center gap-2 rounded px-2 text-start text-sm hover:bg-accent", current && "bg-primary/15 font-medium text-foreground", !entry.dir && "text-muted-foreground")}
        >
            {entry.dir ? <Folder className="size-4 shrink-0 fill-amber-400/80 text-amber-500" /> : <File className="size-4 shrink-0" />}
            <span className="min-w-0 flex-1 truncate">{entry.name}</span>
            {entry.dir && <ChevronRight className="size-3.5 shrink-0 opacity-50" />}
        </button>
    );
}

/** The folder above: the one open marked; a click opens what's clicked. */
export function ParentColumn({ path, showHidden }: { path: string; showHidden: boolean }) {
    const parent = path === THIS_PC || isRoot(path) ? null : parentPath(path);
    const entries = useListing(parent, showHidden);
    if (!parent) return null;
    return (
        <div className="flex h-full w-56 shrink-0 flex-col border-e" aria-label={`The folder above: ${baseName(parent)}`}>
            <div className="truncate border-b px-3 py-1.5 text-xs font-semibold text-muted-foreground" title={parent}>
                {baseName(parent) || parent}
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto p-1">
                {entries?.map((e) => (
                    <Row key={e.name} entry={e} current={samePath(`${parent.replace(/\\$/, "")}\\${e.name}`, path)} onOpen={() => void navigate(e.dir ? `${parent.replace(/\\$/, "")}\\${e.name}` : parent, e.dir ? undefined : { select: `${parent.replace(/\\$/, "")}\\${e.name}` })} />
                ))}
            </div>
        </div>
    );
}

/** What's in the focused folder (→ or a click opens it), or the focused file's few facts. */
export function NextColumn({ entry, showHidden }: { entry: FileEntry | null; showHidden: boolean }) {
    const entries = useListing(entry?.dir ? entry.path : null, showHidden);
    return (
        <div className="flex h-full w-64 shrink-0 flex-col border-s" aria-label="What's in the focused folder">
            {entry?.dir ? (
                <>
                    <div className="truncate border-b px-3 py-1.5 text-xs font-semibold text-muted-foreground" title={entry.path}>
                        {entry.name}
                        {entries && <span className="ms-1.5 font-normal">({formatCount(entries.length)})</span>}
                    </div>
                    <div className="min-h-0 flex-1 overflow-y-auto p-1">
                        {entries?.length === 0 && <p className="px-2 py-3 text-xs text-muted-foreground">It's empty</p>}
                        {entries?.map((e) => (
                            <Row key={e.name} entry={e} onOpen={() => void navigate(e.dir ? `${entry.path}\\${e.name}` : entry.path, e.dir ? undefined : { select: `${entry.path}\\${e.name}` })} />
                        ))}
                    </div>
                </>
            ) : entry ? (
                <div className="space-y-1 p-3 text-xs text-muted-foreground">
                    <p className="truncate text-sm font-medium text-foreground" title={entry.name}>
                        {entry.name}
                    </p>
                    <p>{formatSize(entry.size)}</p>
                    <p>Space shows it full size; Alt+P the preview pane.</p>
                </div>
            ) : (
                <p className="p-3 text-xs text-muted-foreground">← and → move between the columns.</p>
            )}
        </div>
    );
}
