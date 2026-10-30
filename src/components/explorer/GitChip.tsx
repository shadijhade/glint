import { useRef, useState } from "react";
import { ArrowDown, ArrowUp, Check, Copy, FolderGit2, GitBranch, ListFilter, Loader2, RotateCw, ShieldAlert } from "lucide-react";
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
    const [copiedWhat, setCopiedWhat] = useState<"branch" | "sha" | null>(null);

    const copyText = (text: string, what: "branch" | "sha") => {
        void navigator.clipboard.writeText(text).then(() => {
            setCopiedWhat(what);
            setTimeout(() => setCopiedWhat((cur) => (cur === what ? null : cur)), 1300);
        });
    };

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
    const statusParts = [
        { key: "conflicted", label: "Conflicts", count: counts.conflicted, query: "git:conflicted", bar: "bg-red-500", pill: "border-red-500/30 bg-red-500/10 text-red-600 dark:text-red-400 hover:bg-red-500/20" },
        { key: "modified", label: "Modified", count: counts.modified, query: "git:modified", bar: "bg-amber-500", pill: "border-amber-500/30 bg-amber-500/10 text-amber-600 dark:text-amber-400 hover:bg-amber-500/20" },
        { key: "staged", label: "Staged", count: counts.staged, query: "git:staged", bar: "bg-emerald-500", pill: "border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 hover:bg-emerald-500/20" },
        { key: "untracked", label: "Untracked", count: counts.untracked, query: "git:untracked", bar: "bg-sky-500", pill: "border-sky-500/30 bg-sky-500/10 text-sky-600 dark:text-sky-400 hover:bg-sky-500/20" },
    ].filter((s) => s.count > 0);
    const totalChanged = statusParts.reduce((s, p) => s + p.count, 0);

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
                    <GitBranch className="size-4 shrink-0 text-primary" />
                    <button
                        type="button"
                        onClick={() => view.branch && copyText(view.branch, "branch")}
                        className="group flex min-w-0 items-center gap-1 truncate text-sm font-semibold hover:text-primary"
                        title={view.branch ? `Click to copy branch "${view.branch}"` : undefined}
                    >
                        <span className="truncate">{name}</span>
                        {view.branch && (copiedWhat === "branch" ? <Check className="size-3 shrink-0 text-emerald-500" /> : <Copy className="size-3 shrink-0 opacity-0 transition-opacity group-hover:opacity-70" />)}
                    </button>
                    {view.head && view.branch && (
                        <button
                            type="button"
                            onClick={() => copyText(view.head!, "sha")}
                            className="ml-auto flex shrink-0 items-center gap-1 rounded-sm bg-muted/70 px-1.5 py-0.5 font-mono text-[0.6875rem] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                            title={`Click to copy commit SHA (${view.head})`}
                        >
                            {copiedWhat === "sha" ? <Check className="size-3 text-emerald-500" /> : null}
                            {view.head}
                        </button>
                    )}
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
                    <>
                        <p className="mt-2">
                            {changes.length ? `${changes.join(", ")} in this folder and below` : "Nothing changed in this folder"}
                            <span className="text-muted-foreground">
                                {" "}
                                · {formatDuration(view.elapsed)}
                                {view.partial ? ", stopped at 4 s: what's shown is what it found by then" : ""}
                            </span>
                        </p>
                        {statusParts.length > 0 && totalChanged > 0 && (
                            <div className="mt-2 space-y-2">
                                <div className="flex h-1.5 w-full overflow-hidden rounded-full bg-muted/60">
                                    {statusParts.map((part) => (
                                        <button
                                            key={part.key}
                                            type="button"
                                            onClick={() => {
                                                close();
                                                updateTab({ filter: part.query });
                                            }}
                                            style={{ width: `${Math.max((part.count / totalChanged) * 100, 5)}%` }}
                                            className={cn("h-full transition-opacity hover:opacity-80", part.bar)}
                                            title={`${part.label}: ${formatCount(part.count)} — click to filter (${part.query})`}
                                        />
                                    ))}
                                </div>
                                <div className="flex flex-wrap gap-1">
                                    {statusParts.map((part) => (
                                        <button
                                            key={part.key}
                                            type="button"
                                            onClick={() => {
                                                close();
                                                updateTab({ filter: part.query });
                                            }}
                                            className={cn("inline-flex items-center gap-1 rounded-md border px-1.5 py-0.5 text-[11px] font-medium transition-colors", part.pill)}
                                            title={`Filter this folder by ${part.query}`}
                                        >
                                            <span className="tabular-nums">{formatCount(part.count)}</span> {part.label}
                                        </button>
                                    ))}
                                </div>
                            </div>
                        )}
                    </>
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
