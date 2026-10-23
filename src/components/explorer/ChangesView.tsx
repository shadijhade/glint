import { Fragment, useEffect, useState } from "react";
import { toast } from "sonner";
import { AlertTriangle, Columns2, FileDiff as FileDiffIcon, FilePlus2, Loader2 } from "lucide-react";
import type { FileEntry } from "@/lib/entries";
import { formatCount, formatDuration, formatSize } from "@/lib/format";
import { type FileDiff, gitDiff, gitHeadCopy, gitHeadDiscard } from "@/lib/fs";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { compareFiles } from "./CompareFilesDialog";
import { Centered } from "./PreviewState";

/** The file as it was in its last commit against it now, side by side (by Difftastic when it's installed). */
async function sideBySide(entry: FileEntry) {
    try {
        const copy = await gitHeadCopy(entry.path);
        if (copy) compareFiles({ path: copy, label: "In the last commit" }, { path: entry.path, label: "Now" }, () => void gitHeadDiscard(copy));
    } catch (e) {
        toast.error("Couldn't read it from the last commit", { description: String(e) });
    }
}

function SideBySide({ entry }: { entry: FileEntry }) {
    return (
        <Button variant="ghost" size="sm" className="h-6 shrink-0 px-1.5 text-xs" onClick={() => void sideBySide(entry)} title="The last commit's version and this one side by side (by the code's structure, with Difftastic)">
            <Columns2 className="size-3.5" /> Side by side
        </Button>
    );
}

const SIGN = { same: " ", removed: "−", added: "+" } as const;
const ROW = {
    same: "",
    removed: "bg-red-500/10 text-red-900 dark:text-red-200",
    added: "bg-emerald-500/10 text-emerald-900 dark:text-emerald-200",
} as const;

/** What changed in a file since its last commit: its version in HEAD against the one on disk, line by line. */
export function ChangesView({ entry }: { entry: FileEntry }) {
    const [diff, setDiff] = useState<FileDiff | null | undefined>(undefined);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        let stale = false;
        setDiff(undefined);
        setError(null);
        gitDiff(entry.path).then(
            (found) => !stale && setDiff(found),
            (e) => !stale && setError(String(e))
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified, entry.size]);

    if (error)
        return (
            <Centered>
                <AlertTriangle className="size-10 stroke-[1.25] text-destructive" />
                <p>Couldn't compare it with the last commit: {error}</p>
            </Centered>
        );
    if (diff === undefined)
        return (
            <Centered>
                <Loader2 className="size-6 animate-spin text-primary" />
            </Centered>
        );
    if (diff === null) return <Centered>It isn't in a Git repository.</Centered>;
    const sizes = `${formatSize(diff.oldSize)} in the last commit, ${formatSize(diff.newSize)} now`;
    if (diff.binary || diff.tooBig)
        return (
            <Centered>
                <FileDiffIcon className="size-12 stroke-[1.25] text-muted-foreground" />
                <p>{diff.binary ? "A binary file: what changed in it isn't shown as lines." : "Too big to compare here (over 1 MB)."}</p>
                <p className="text-xs">{diff.newFile ? `New: ${formatSize(diff.newSize)}, not in the last commit` : sizes}</p>
                {!diff.newFile && <SideBySide entry={entry} />}
            </Centered>
        );

    let lastOld = 0;
    let lastNew = 0;
    return (
        <div className="flex min-h-full flex-col">
            <div className="sticky top-0 z-10 flex items-center gap-2 border-b bg-card/95 px-3 py-1.5 text-xs backdrop-blur-xs">
                {diff.newFile ? (
                    <span className="flex items-center gap-1.5 font-medium text-emerald-700 dark:text-emerald-400">
                        <FilePlus2 className="size-3.5" /> New: not in the last commit
                    </span>
                ) : (
                    <span className="font-medium">
                        <span className="text-emerald-700 dark:text-emerald-400">+{formatCount(diff.added)}</span>{" "}
                        <span className="text-red-700 dark:text-red-400">−{formatCount(diff.removed)}</span> since the last commit
                    </span>
                )}
                <span className="ml-auto truncate font-mono text-[0.6875rem] text-muted-foreground" title={`${diff.path}, compared in ${formatDuration(diff.elapsed)}`}>
                    {diff.path}
                </span>
                {!diff.newFile && <SideBySide entry={entry} />}
            </div>
            {diff.lines.length === 0 ? (
                <Centered>
                    <p>Nothing changed in its lines since the last commit{diff.oldSize !== diff.newSize ? " (its line endings did)" : ""}.</p>
                </Centered>
            ) : (
                <div className="font-mono text-xs leading-5" role="table" aria-label={`Changes in ${entry.name}`}>
                    {diff.lines.map((line, i) => {
                        // Same lines left out between two changes: a gap in the numbers.
                        const gap = i > 0 && ((line.old > 0 && lastOld > 0 && line.old !== lastOld + 1) || (line.new > 0 && lastNew > 0 && line.new !== lastNew + 1));
                        if (line.old) lastOld = line.old;
                        if (line.new) lastNew = line.new;
                        return (
                            <Fragment key={i}>
                                {gap && <div className="border-y border-dashed bg-muted/40 px-3 text-center text-[0.6875rem] text-muted-foreground">⋯</div>}
                                <div role="row" className={cn("flex", ROW[line.kind])}>
                                    <span className="w-10 shrink-0 select-none pr-1.5 text-right text-muted-foreground/70 tabular-nums">{line.old || ""}</span>
                                    <span className="w-10 shrink-0 select-none pr-1.5 text-right text-muted-foreground/70 tabular-nums">{line.new || ""}</span>
                                    <span className="w-4 shrink-0 select-none text-center">{SIGN[line.kind]}</span>
                                    <span className="min-w-0 flex-1 whitespace-pre-wrap break-all pr-3">{line.text || " "}</span>
                                </div>
                            </Fragment>
                        );
                    })}
                    {diff.cut && <p className="border-t px-3 py-2 text-muted-foreground">More changed than is shown here: the first {formatCount(diff.lines.length)} lines.</p>}
                </div>
            )}
        </div>
    );
}
