import { useRef } from "react";
import { ArrowDown, ArrowUp, FolderGit2, GitBranch, ListFilter, Loader2, RotateCw, ShieldAlert } from "lucide-react";
import { navigate, refresh, updateTab } from "@/lib/explorer";
import { formatCount, formatDuration } from "@/lib/format";
import { trustRepository, useGit } from "@/lib/git";
import { baseName, samePath } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";

/**
 * The branch a tab's folder is on, in the address bar ("⎇ main ↑2 ↓1"), when it's in a Git repository:
 * a click shows the repository, its upstream, what changed here, and Show changes (a git:changed filter).
 */
export function GitChip({ tab, path }: { tab: string; path: string }) {
    const run = useGit((s) => s[tab]);
    const popRef = useRef<HTMLDivElement>(null);

    /** Below the chip, kept on screen: placed as it's clicked, before it opens. */
    const place = (e: React.MouseEvent<HTMLButtonElement>) => {
        const chip = e.currentTarget.getBoundingClientRect();
        const pop = popRef.current;
        if (!pop) return;
        // Closed, it has no width yet: it's w-80.
        pop.style.left = `${Math.max(4, Math.min(chip.left, window.innerWidth - (pop.offsetWidth || 320) - 4))}px`;
        pop.style.top = `${chip.bottom + 4}px`;
    };

    const view = run?.view;
    if (!view || !samePath(run.folder, path)) return null;
    const { counts } = view;
    const changes = [
        counts.conflicted > 0 && `${formatCount(counts.conflicted)} with conflicts`,
        counts.modified > 0 && `${formatCount(counts.modified)} changed`,
        counts.staged > 0 && `${formatCount(counts.staged)} staged`,
        counts.untracked > 0 && `${formatCount(counts.untracked)} new`,
    ].filter(Boolean);
    const name = view.branch ?? (view.head ? `at ${view.head}` : "no commits yet");
    const close = () => popRef.current?.hidePopover();
    const atRoot = samePath(view.root, path);

    return (
        <>
            <button
                type="button"
                popoverTarget={`git-${tab}`}
                onClick={place}
                title={`Git: ${view.branch ? `on ${view.branch}` : view.head ? `HEAD detached at ${view.head}` : "a repository with no commits yet"}${view.upstream ? `, tracking ${view.upstream}` : ""}`}
                className="ml-1 flex h-6 shrink-0 items-center gap-1 rounded-[4px] px-1.5 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground @max-[26rem]:hidden"
            >
                <GitBranch className="size-3.5" />
                <span className="max-w-32 truncate font-medium">{name}</span>
                {view.ahead > 0 && (
                    <span className="flex items-center tabular-nums" title={`${formatCount(view.ahead)} commits not on ${view.upstream} yet`}>
                        <ArrowUp className="size-3" />
                        {formatCount(view.ahead)}
                    </span>
                )}
                {view.behind > 0 && (
                    <span className="flex items-center tabular-nums" title={`${formatCount(view.behind)} commits on ${view.upstream} not here yet`}>
                        <ArrowDown className="size-3" />
                        {formatCount(view.behind)}
                    </span>
                )}
                {(view.limited || view.onRequest) && <ShieldAlert className="size-3 text-amber-600 dark:text-amber-500" />}
                {run.running && <Loader2 className="size-3 animate-spin" />}
            </button>
            <div id={`git-${tab}`} ref={popRef} popover="auto" className="fixed inset-auto m-0 w-80 rounded-md border bg-popover p-3 text-xs text-popover-foreground shadow-lg">
                <div className="flex items-center gap-2">
                    <GitBranch className="size-4 text-primary" />
                    <span className="min-w-0 truncate text-sm font-semibold">{name}</span>
                    {view.head && view.branch && <code className="ml-auto shrink-0 font-mono text-[0.6875rem] text-muted-foreground">{view.head}</code>}
                </div>
                <p className="mt-1 truncate text-muted-foreground" title={view.root}>
                    In {baseName(view.root) || view.root}
                </p>
                <p className="mt-2">
                    {view.upstream ? (
                        <>
                            Tracks <span className="font-medium">{view.upstream}</span>
                            {view.ahead || view.behind
                                ? `: ${[view.ahead && `${formatCount(view.ahead)} ahead`, view.behind && `${formatCount(view.behind)} behind`].filter(Boolean).join(", ")}`
                                : ", up to date with it (as last fetched)"}
                        </>
                    ) : (
                        <span className="text-muted-foreground">Tracks no upstream branch</span>
                    )}
                </p>
                {view.limited ? (
                    <div className="mt-2.5 rounded-md bg-amber-500/10 p-2.5">
                        <p>This repository's own settings name programs for Git status to run (filter drivers, or core.fsmonitor), so Glint doesn't look at what changed in it. Trust it only if you trust where it came from.</p>
                        <Button
                            size="sm"
                            variant="outline"
                            className="mt-2 h-7 text-xs"
                            onClick={() => {
                                close();
                                trustRepository(view.root);
                            }}
                        >
                            <ShieldAlert /> Trust this repository
                        </Button>
                    </div>
                ) : view.onRequest ? (
                    <div className="mt-2.5 rounded-md bg-muted p-2.5">
                        <p>This repository has too many files (or is on another computer) to look through on every visit.</p>
                        <Button
                            size="sm"
                            variant="outline"
                            className="mt-2 h-7 text-xs"
                            onClick={() => {
                                close();
                                void refresh();
                            }}
                        >
                            <RotateCw /> Show its status (F5)
                        </Button>
                    </div>
                ) : (
                    <p className="mt-2">
                        {changes.length ? `${changes.join(", ")} in this folder and below` : "Nothing changed in this folder"}
                        <span className="text-muted-foreground">
                            {" "}
                            · {formatDuration(view.elapsed)}
                            {view.partial ? ", stopped at 4 s: what's shown is what it found by then" : ""}
                        </span>
                    </p>
                )}
                <div className="mt-2.5 flex flex-wrap gap-1.5">
                    {!view.limited && !view.onRequest && changes.length > 0 && (
                        <Button
                            size="sm"
                            variant="outline"
                            className="h-7 text-xs"
                            title="Filter this folder to what changed (git:changed)"
                            onClick={() => {
                                close();
                                updateTab({ filter: "git:changed" });
                            }}
                        >
                            <ListFilter /> Show changes
                        </Button>
                    )}
                    {!atRoot && (
                        <Button
                            size="sm"
                            variant="ghost"
                            className={cn("h-7 text-xs")}
                            title={view.root}
                            onClick={() => {
                                close();
                                void navigate(view.root);
                            }}
                        >
                            <FolderGit2 /> Open the repository's folder
                        </Button>
                    )}
                </div>
            </div>
        </>
    );
}
