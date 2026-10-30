import { useState } from "react";
import { AlertTriangle, ChevronDown, ChevronUp, CircleCheck, Copy, FolderInput, Pause, Play, ShieldCheck, X } from "lucide-react";
import { formatCount, formatSize, plural } from "@/lib/format";
import { finished, jobFrom, jobTitle, queueAnswer, queueCancel, queueClear, type QueueJob, queuePause, queueResume, useQueue } from "@/lib/queue";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";

/** "2 min left", "8 s left". */
function left(job: QueueJob) {
    if (!job.speed || job.bytesDone >= job.bytes) return "";
    const seconds = (job.bytes - job.bytesDone) / job.speed;
    if (seconds < 60) return `${Math.max(1, Math.round(seconds))} s left`;
    if (seconds < 3600) return `${Math.round(seconds / 60)} min left`;
    return `${Math.floor(seconds / 3600)} h ${Math.round((seconds % 3600) / 60)} min left`;
}

/** How long a finished job took: "12 s", "3 min". */
function took(job: QueueJob) {
    if (!job.started || !job.finished) return "";
    const s = (job.finished - job.started) / 1000;
    return s < 60 ? `${Math.max(1, Math.round(s))} s` : `${Math.round(s / 60)} min`;
}

const STATES: Record<QueueJob["state"], string> = {
    waiting: "Waiting its turn",
    planning: "Looking at what's to copy…",
    copying: "Copying",
    checking: "Checking the copy",
    paused: "Paused",
    asking: "Asks you",
    done: "Done",
    cancelled: "Cancelled",
    failed: "Failed",
};

function JobCard({ job }: { job: QueueJob }) {
    const [all, setAll] = useState(false);
    const [errorsOpen, setErrorsOpen] = useState(false);
    const over = finished(job);
    const share = job.bytes ? Math.min(100, (job.bytesDone / job.bytes) * 100) : over ? 100 : 0;
    return (
        <li className="space-y-1.5 border-b px-3 py-2.5 last:border-b-0">
            <div className="flex items-start gap-2">
                {job.kind === "move" ? <FolderInput className="mt-0.5 size-4 shrink-0 text-primary" /> : <Copy className="mt-0.5 size-4 shrink-0 text-primary" />}
                <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium" title={`From ${jobFrom(job)} to ${job.dest}`}>
                        {jobTitle(job)}
                    </p>
                    <p className="truncate text-xs text-muted-foreground">
                        {over
                            ? `${STATES[job.state]}: ${plural(job.filesDone, "file")} (${formatSize(job.bytesDone)})${took(job) ? ` in ${took(job)}` : ""}${job.verify ? ` · ${job.verified === job.filesDone - job.errors.length || job.verified === job.filesDone ? "each one checked" : `${formatCount(job.verified)} checked`}` : ""}`
                            : `${STATES[job.state]}${job.files ? ` · ${formatCount(job.filesDone)} of ${plural(job.files, "file")}` : ""}${job.bytes ? ` · ${formatSize(job.bytesDone)} of ${formatSize(job.bytes)}` : ""}${job.speed && job.state === "copying" ? ` · ${formatSize(job.speed)}/s · ${left(job)}` : ""}`}
                    </p>
                </div>
                {!over && (
                    <div className="flex shrink-0 gap-0.5">
                        {job.state === "paused" ? (
                            <Button variant="ghost" size="icon" className="size-7" onClick={() => void queueResume(job.id)} title="Resume" aria-label="Resume">
                                <Play />
                            </Button>
                        ) : (
                            <Button variant="ghost" size="icon" className="size-7" onClick={() => void queuePause(job.id)} title="Pause: a file half-copied waits where it is" aria-label="Pause" disabled={job.state === "asking"}>
                                <Pause />
                            </Button>
                        )}
                        <Button variant="ghost" size="icon" className="size-7" onClick={() => void queueCancel(job.id)} title="Cancel: what's copied stays, a file half-copied goes" aria-label="Cancel">
                            <X />
                        </Button>
                    </div>
                )}
                {over && job.state === "done" && !job.errors.length && <CircleCheck className="mt-0.5 size-4 shrink-0 text-emerald-500" aria-label="Done" />}
            </div>
            {!over && (
                <div className="h-1.5 overflow-hidden rounded-full bg-muted" role="progressbar" aria-valuenow={Math.round(share)} aria-valuemin={0} aria-valuemax={100}>
                    <div className={cn("h-full rounded-full transition-[width] duration-200", job.state === "paused" ? "bg-amber-500" : "bg-primary")} style={{ width: `${share}%` }} />
                </div>
            )}
            {job.current && !over && (
                <p className="truncate text-[0.6875rem] text-muted-foreground" title={job.current}>
                    {job.current}
                </p>
            )}
            {job.state === "asking" && job.conflict && (
                <div className="space-y-1.5 rounded-md border border-amber-500/40 bg-amber-500/10 p-2 text-xs">
                    <p>
                        <span className="font-medium">“{job.conflict.split(/[\\/]/).pop()}”</span> is there already.
                    </p>
                    <div className="flex flex-wrap items-center gap-1.5">
                        <Button size="sm" className="h-7" onClick={() => void queueAnswer(job.id, "replace", all)}>
                            Replace it
                        </Button>
                        <Button size="sm" variant="outline" className="h-7" onClick={() => void queueAnswer(job.id, "skip", all)}>
                            Skip it
                        </Button>
                        <Button size="sm" variant="outline" className="h-7" onClick={() => void queueAnswer(job.id, "both", all)}>
                            Keep both
                        </Button>
                        <label className="ml-1 flex items-center gap-1.5">
                            <input type="checkbox" checked={all} onChange={(e) => setAll(e.target.checked)} className="accent-primary" /> For the rest too
                        </label>
                    </div>
                </div>
            )}
            {job.errors.length > 0 && (
                <div className="text-xs">
                    <button type="button" onClick={() => setErrorsOpen(!errorsOpen)} className="flex items-center gap-1 text-destructive hover:underline">
                        <AlertTriangle className="size-3.5" /> {job.errors.length === 1 ? "1 didn't go" : `${formatCount(job.errors.length)} didn't go`}
                        {errorsOpen ? <ChevronUp className="size-3.5" /> : <ChevronDown className="size-3.5" />}
                    </button>
                    {errorsOpen && (
                        <ul className="mt-1 max-h-32 space-y-0.5 overflow-y-auto">
                            {job.errors.map((e, i) => (
                                <li key={i} className="truncate text-muted-foreground" title={`${e.path}\n${e.error}`}>
                                    <span className="text-foreground">{e.path.split(/[\\/]/).pop()}</span>: {e.error}
                                </li>
                            ))}
                        </ul>
                    )}
                </div>
            )}
        </li>
    );
}

/**
 * The copy queue, over the bottom right of the window: each job with how far it is, its speed and time
 * left, Pause, Resume and Cancel, a name that's taken to decide, and what didn't go. Closed, while jobs
 * run, it's a pill with how far they are.
 */
export function CopyQueue() {
    const { jobs, open } = useQueue();
    if (!jobs.length) return null;
    const running = jobs.filter((j) => !finished(j));
    const bytes = running.reduce((n, j) => n + j.bytes, 0);
    const done = running.reduce((n, j) => n + j.bytesDone, 0);
    const asking = running.some((j) => j.state === "asking");
    if (!open)
        return running.length ? (
            <button
                type="button"
                onClick={() => useQueue.setState({ open: true })}
                className={cn("fixed right-4 bottom-10 z-40 flex items-center gap-2 rounded-full border bg-popover px-3 py-1.5 text-xs shadow-lg hover:bg-accent", asking && "border-amber-500")}
            >
                <Copy className="size-3.5 text-primary" />
                {asking ? "The copy queue asks you" : `Copy queue · ${bytes ? Math.round((done / bytes) * 100) : 0}%`}
            </button>
        ) : null;
    return (
        <section aria-label="Copy queue" className="fixed right-4 bottom-10 z-40 flex max-h-[60vh] w-[27rem] max-w-[calc(100vw-2rem)] flex-col overflow-hidden rounded-xl border bg-popover shadow-2xl">
            <header className="flex shrink-0 items-center gap-2 border-b px-3 py-2">
                <ShieldCheck className="size-4 text-primary" />
                <h2 className="flex-1 text-sm font-semibold">Copy queue</h2>
                {jobs.some(finished) && (
                    <Button variant="ghost" size="sm" className="h-7" onClick={() => void queueClear()}>
                        Clear finished
                    </Button>
                )}
                <Button variant="ghost" size="icon" className="size-7" onClick={() => useQueue.setState({ open: false })} title="Hide (it carries on)" aria-label="Hide the copy queue">
                    <X />
                </Button>
            </header>
            <ul className="min-h-0 flex-1 overflow-y-auto">
                {jobs.map((job) => (
                    <JobCard key={job.id} job={job} />
                ))}
            </ul>
        </section>
    );
}
