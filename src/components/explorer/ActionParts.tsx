import { useState } from "react";
import { AlertTriangle, CheckCircle2, FolderInput, Loader2, Square } from "lucide-react";
import { type ActionOutput, type ActionProgress, type ActionReport, badOutput, offerActionUndo } from "@/lib/actions";
import { revealInApp } from "@/lib/explorer";
import { formatCount, formatDuration, formatSize, plural } from "@/lib/format";
import { baseName } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";

/** A file action's run: its progress, then its report or error; `start` runs one and offers Undo after. */
export function useActionRun() {
    const [progress, setProgress] = useState<ActionProgress | null>(null);
    const [report, setReport] = useState<ActionReport | null>(null);
    const [error, setError] = useState<string | null>(null);
    const running = !!progress && !report && !error;
    const start = async (total: number, run: (onProgress: (p: ActionProgress) => void) => Promise<ActionReport>, undoLabel: (r: ActionReport) => string) => {
        setError(null);
        setReport(null);
        setProgress({ done: 0, total, current: "", part: 0 });
        try {
            const done = await run(setProgress);
            setReport(done);
            if (done.made.length) offerActionUndo(done, undoLabel(done));
            return done;
        } catch (e) {
            setError(String(e));
            return null;
        }
    };
    const reset = () => {
        setProgress(null);
        setReport(null);
        setError(null);
    };
    return { progress, report, error, running, start, reset, started: !!progress };
}

/** Where what's made goes: beside each file (a suffix after the name), into a folder there, or in their place. */
export function WhereChooser({
    value,
    onChange,
    noun = "file",
    replace = true,
    note,
}: {
    value: ActionOutput;
    onChange: (o: ActionOutput) => void;
    noun?: string;
    replace?: boolean;
    note?: string;
}) {
    const [suffix, setSuffix] = useState(value.kind === "beside" ? value.suffix : "");
    const [folder, setFolder] = useState(value.kind === "folder" ? value.name : "Converted");
    const bad = badOutput(value);
    return (
        <section className="space-y-1.5" role="radiogroup" aria-label="Where">
            <h3 className="text-xs font-semibold text-muted-foreground">Where</h3>
            <label className="flex items-center gap-2">
                <input type="radio" name="where" checked={value.kind === "beside"} onChange={() => onChange({ kind: "beside", suffix })} className="accent-primary" />
                <span className="shrink-0">Next to each {noun}, its name and</span>
                <input
                    value={suffix}
                    onChange={(e) => {
                        setSuffix(e.target.value);
                        onChange({ kind: "beside", suffix: e.target.value });
                    }}
                    placeholder="nothing"
                    aria-label="Added to the name"
                    spellCheck={false}
                    className={cn("h-7 w-28 rounded-md border bg-background px-2 text-sm outline-hidden focus:border-primary", value.kind === "beside" && bad && "border-destructive")}
                />
            </label>
            <label className="flex items-center gap-2">
                <input type="radio" name="where" checked={value.kind === "folder"} onChange={() => onChange({ kind: "folder", name: folder })} className="accent-primary" />
                <span className="shrink-0">Into a folder next to them:</span>
                <input
                    value={folder}
                    onChange={(e) => {
                        setFolder(e.target.value);
                        onChange({ kind: "folder", name: e.target.value });
                    }}
                    aria-label="Folder name"
                    spellCheck={false}
                    className={cn("h-7 w-36 rounded-md border bg-background px-2 text-sm outline-hidden focus:border-primary", value.kind === "folder" && bad && "border-destructive")}
                />
            </label>
            {replace && (
                <label className="flex items-center gap-2">
                    <input type="radio" name="where" checked={value.kind === "replace"} onChange={() => onChange({ kind: "replace" })} className="accent-primary" />
                    <span>In their place, the {noun}s moved to the Recycle Bin</span>
                </label>
            )}
            <p className="ps-6 text-xs text-muted-foreground">{note ?? "A name that's taken gets “(2)”, and Ctrl+Z takes it all back."}</p>
        </section>
    );
}

/** A run under way: which of how many, and how far the one being made is. */
export function RunProgress({ progress, verb }: { progress: ActionProgress; verb: string }) {
    const whole = progress.total ? (progress.done + Math.min(progress.part, 0.999)) / progress.total : 0;
    return (
        <div className="space-y-2 py-6">
            <div className="flex items-center gap-2 text-muted-foreground">
                <Loader2 className="size-4 animate-spin text-primary" />
                <span className="truncate">
                    {verb} {progress.total > 1 ? `${formatCount(Math.min(progress.done + 1, progress.total))} of ${formatCount(progress.total)}` : ""}
                    {progress.current && ` · ${progress.current}`}
                    {progress.part > 0 && ` · ${Math.round(progress.part * 100)}%`}
                </span>
            </div>
            <div className="h-1.5 overflow-hidden rounded-full bg-muted">
                <div className="h-full rounded-full bg-primary transition-[width]" style={{ width: `${Math.round(whole * 100)}%` }} />
            </div>
        </div>
    );
}

/** What a run did: made how many, the sizes before and after, and what failed. */
export function RunResult({ report, error, noun = "file", failedWhat = "Couldn't do it", sizes = true }: { report: ActionReport | null; error: string | null; noun?: string; failedWhat?: string; sizes?: boolean }) {
    if (error)
        return (
            <div className="flex flex-col items-center gap-2 py-6 text-center">
                <AlertTriangle className="size-6 text-destructive" />
                <p className="font-medium">{failedWhat}</p>
                <p className="max-w-md text-xs text-muted-foreground">{error}</p>
            </div>
        );
    if (!report) return null;
    const { before, after } = report;
    return (
        <div className="space-y-3">
            <div className="flex items-start gap-2">
                <CheckCircle2 className={cn("mt-0.5 size-5 shrink-0", report.made.length ? "text-emerald-500" : "text-muted-foreground")} />
                <div>
                    <p className="font-medium">
                        {report.made.length ? `Made ${plural(report.made.length, noun)}` : "Nothing was made"}
                        {report.stopped && " before it was stopped"}
                    </p>
                    {report.made.length > 0 && (
                        <p className="text-xs text-muted-foreground">
                            {sizes && before > 0 && (
                                <>
                                    {formatSize(before)} → {formatSize(after)} ({after <= before ? "−" : "+"}
                                    {Math.abs(Math.round((100 * (before - after)) / before))}%) ·{" "}
                                </>
                            )}
                            {formatDuration(report.elapsedMs)}
                        </p>
                    )}
                </div>
            </div>
            {report.failed.length > 0 && (
                <div className="rounded-md bg-amber-500/10 px-3 py-2 text-xs text-amber-900 dark:text-amber-300">
                    <p className="font-medium">{plural(report.failed.length, "file")} not done:</p>
                    <ul className="mt-1 space-y-0.5">
                        {report.failed.slice(0, 6).map(([path, why]) => (
                            <li key={path} className="truncate" title={`${path}\n${why}`}>
                                {baseName(path)}: {why}
                            </li>
                        ))}
                        {report.failed.length > 6 && <li>and {formatCount(report.failed.length - 6)} more</li>}
                    </ul>
                </div>
            )}
        </div>
    );
}

/** The dialog's buttons: Show and Close after a run, Cancel and the action before; Stop while it runs. */
export function RunFooter({
    run,
    onClose,
    onStart,
    startLabel,
    startIcon: StartIcon,
    disabled,
    onStop,
}: {
    run: ReturnType<typeof useActionRun>;
    onClose: () => void;
    onStart: () => void;
    startLabel: string;
    startIcon: React.ComponentType<{ className?: string }>;
    disabled?: boolean;
    onStop?: () => void;
}) {
    const first = run.report?.made[0];
    return (
        <div className="flex items-center justify-end gap-2 border-t bg-muted/20 px-4 py-2.5">
            {run.running && onStop && (
                <Button variant="outline" size="sm" onClick={onStop}>
                    <Square className="size-3! fill-current" /> Stop
                </Button>
            )}
            {run.report && first && (
                <Button
                    variant="outline"
                    size="sm"
                    onClick={() => {
                        onClose();
                        void revealInApp(first);
                    }}
                >
                    <FolderInput /> Show
                </Button>
            )}
            {run.report || run.error ? (
                <Button size="sm" onClick={onClose}>
                    Close
                </Button>
            ) : (
                <>
                    <Button variant="outline" size="sm" onClick={onClose} disabled={run.running}>
                        Cancel
                    </Button>
                    <Button size="sm" onClick={onStart} disabled={run.running || disabled}>
                        <StartIcon /> {startLabel}
                    </Button>
                </>
            )}
        </div>
    );
}

/** A row of choices, one picked, as Convert's formats are. */
export function Choices<T extends string | number>({ value, options, onChange, label, columns = 2 }: { value: T; options: { value: T; label: string; hint?: string; disabled?: boolean }[]; onChange: (v: T) => void; label: string; columns?: number }) {
    return (
        <div role="radiogroup" aria-label={label} className="grid gap-1.5" style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}>
            {options.map((o) => (
                <button
                    key={String(o.value)}
                    type="button"
                    role="radio"
                    aria-checked={value === o.value}
                    disabled={o.disabled}
                    title={o.hint}
                    onClick={() => onChange(o.value)}
                    className={cn(
                        "rounded-md border px-2 py-1.5 text-start transition-colors hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50",
                        value === o.value && "border-primary bg-primary/10 hover:bg-primary/15"
                    )}
                >
                    <span className="block font-medium">{o.label}</span>
                    {o.hint && <span className="block truncate text-[0.6875rem] text-muted-foreground">{o.hint}</span>}
                </button>
            ))}
        </div>
    );
}
