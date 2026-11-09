import { useEffect, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { Channel, invoke } from "@tauri-apps/api/core";
import { AlertTriangle, CheckCircle2, Expand, Loader2, Shrink, Square } from "lucide-react";
import { type FileEntry, isArchived, isClosed, isOnlineOnly, isRecycled, isRemote } from "@/lib/entries";
import { offerUndo, rememberUndo } from "@/lib/explorer";
import { formatCount, formatDuration, formatSize, plural } from "@/lib/format";
import { baseName, parentPath } from "@/lib/paths";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";
import { Choices } from "./ActionParts";

/** WOF's algorithms (wof.rs), lightest first. */
type Algorithm = "xpress4k" | "xpress8k" | "xpress16k" | "lzx";

interface Measure {
    ntfs: boolean;
    files: number;
    bytes: number;
    already: number;
    alreadyBytes: number;
    alreadyOnDisk: number;
    wof: number;
    skipped: number;
    estimates: { algorithm: Algorithm; after: number }[];
    stopped: boolean;
}

interface Progress {
    done: number;
    total: number;
    current: string;
    saved: number;
}

interface Report {
    done: number;
    unchanged: number;
    failed: [string, string][];
    failedCount: number;
    before: number;
    after: number;
    undo: number | null;
    stopped: boolean;
    elapsedMs: number;
}

const onDisk = (e: FileEntry) => !isArchived(e) && !isClosed(e) && !isRecycled(e) && !isOnlineOnly(e) && !isRemote(e);

/** Files and folders Compress in place takes: on the PC itself (NTFS is checked when measuring). */
export const canCompressInPlace = (e: FileEntry) => onDisk(e) && !e.path.startsWith("\\\\");

const useCompressInPlace = create<{ entries: FileEntry[] | null }>(() => ({ entries: null }));

/** Opens Compress in place on `entries`. */
export function openCompressInPlace(entries: FileEntry[]) {
    const chosen = entries.filter(canCompressInPlace);
    if (!chosen.length) return void toast.message("Compress in place works on files and folders on this PC's drives");
    useCompressInPlace.setState({ entries: chosen });
}

const close = () => useCompressInPlace.setState({ entries: null });

/** Runs a wof.rs command with a progress channel. */
function runWof(command: string, args: Record<string, unknown>, onProgress: (p: Progress) => void) {
    const channel = new Channel<Progress>();
    channel.onmessage = onProgress;
    return invoke<Report>(command, { ...args, onProgress: channel });
}

/** Offers Undo for a compression: its files decompressed. */
function offerWofUndo(report: Report, what: string) {
    const id = report.undo;
    if (id == null) return;
    const step = rememberUndo(what, async () => {
        const back = await runWof("wof_undo", { id }, () => {});
        if (back.failedCount) toast.error(`Couldn't decompress ${plural(back.failedCount, "file")}`, { description: back.failed[0]?.[1] });
    });
    offerUndo(what.charAt(0).toUpperCase() + what.slice(1), step);
}

export function CompressInPlaceDialog() {
    const entries = useCompressInPlace((s) => s.entries);
    if (!entries) return null;
    return <CompressInPlaceModal key={entries.map((e) => e.path).join("\n")} entries={entries} />;
}

const HINTS: Record<Algorithm, { label: string; hint: string }> = {
    xpress4k: { label: "Light", hint: "Fastest to open" },
    xpress8k: { label: "Balanced", hint: "Windows' own default" },
    xpress16k: { label: "Strong", hint: "A little more saved" },
    lzx: { label: "Strongest", hint: "Most saved; for things used rarely" },
};

function CompressInPlaceModal({ entries }: { entries: FileEntry[] }) {
    const [measure, setMeasure] = useState<Measure | null>(null);
    const [measureError, setMeasureError] = useState<string | null>(null);
    const [algorithm, setAlgorithm] = useState<Algorithm>("xpress8k");
    const [progress, setProgress] = useState<Progress | null>(null);
    const [report, setReport] = useState<Report | null>(null);
    const [decompressed, setDecompressed] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const paths = entries.map((e) => e.path);
    const running = !!progress && !report && !error;

    useEffect(() => {
        invoke<Measure>("wof_measure", { paths }).then(setMeasure, (e) => setMeasureError(String(e)));
        return () => void invoke("wof_cancel");
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    const run = async (decompress: boolean) => {
        setError(null);
        setReport(null);
        setDecompressed(decompress);
        setProgress({ done: 0, total: decompress ? (measure?.wof ?? 0) : (measure?.files ?? 0), current: "", saved: 0 });
        try {
            const done = decompress ? await runWof("wof_decompress", { paths }, setProgress) : await runWof("wof_compress", { paths, algorithm }, setProgress);
            setReport(done);
            if (!decompress) offerWofUndo(done, `compressing ${plural(done.done, "file")} in place`);
        } catch (e) {
            setError(String(e));
        }
    };

    const title = entries.length === 1 ? entries[0].name : `${formatCount(entries.length)} items`;
    const dirs = new Set(entries.map((e) => parentPath(e.path).toLowerCase()));
    const estimate = (a: Algorithm) => measure?.estimates.find((e) => e.algorithm === a);
    const saving = (a: Algorithm) => {
        const e = estimate(a);
        return e && measure ? Math.max(0, measure.bytes - e.after) : null;
    };
    const chosenSaving = saving(algorithm);

    return (
        <Modal label="Compress in place" onClose={close} onSubmit={() => measure?.files && !progress && void run(false)} className="max-h-[88vh] max-w-xl">
            <ModalHeader icon={Shrink} title={`Compress in place: ${title}`} subtitle={dirs.size === 1 ? parentPath(entries[0].path) : `In ${formatCount(dirs.size)} folders`} onClose={close} />
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                {!progress && (
                    <>
                        <p className="text-muted-foreground">
                            Windows stores the files compressed and decompresses them as they're read, so they open as before, just taking less space. Best for apps, games, tools and old projects; a file an app changes is stored whole again.
                        </p>
                        {measureError && <p className="text-destructive">{measureError}</p>}
                        {!measure && !measureError && (
                            <p className="flex items-center gap-2 text-muted-foreground">
                                <Loader2 className="size-4 animate-spin text-primary" /> Measuring what's there…
                            </p>
                        )}
                        {measure && !measure.ntfs && (
                            <p className="flex items-start gap-2 rounded-md bg-amber-500/10 px-3 py-2 text-xs text-amber-900 dark:text-amber-300">
                                <AlertTriangle className="mt-0.5 size-4 shrink-0" /> Compress in place works on NTFS drives only: this drive is formatted another way.
                            </p>
                        )}
                        {measure?.ntfs && (
                            <>
                                <ul className="space-y-1 text-xs text-muted-foreground">
                                    <li>
                                        <b className="font-semibold text-foreground">{plural(measure.files, "file")}</b> to compress, {formatSize(measure.bytes)} in all
                                    </li>
                                    {measure.already > 0 && (
                                        <li>
                                            {plural(measure.already, "file")} compressed already: {formatSize(measure.alreadyBytes)} taking {formatSize(measure.alreadyOnDisk)}
                                        </li>
                                    )}
                                    {measure.skipped > 0 && <li>{plural(measure.skipped, "file")} left alone: photos, videos, archives and other kinds that are compressed inside, tiny files, links and online-only files</li>}
                                </ul>
                                {measure.files > 0 && (
                                    <section className="space-y-1.5">
                                        <h3 className="text-xs font-semibold text-muted-foreground">How hard</h3>
                                        <Choices
                                            label="How hard"
                                            columns={4}
                                            value={algorithm}
                                            onChange={setAlgorithm}
                                            options={(["xpress4k", "xpress8k", "xpress16k", "lzx"] as const).map((a) => {
                                                const s = saving(a);
                                                return { value: a, label: HINTS[a].label, hint: s != null ? `About ${formatSize(s)} saved` : HINTS[a].hint };
                                            })}
                                        />
                                        <p className="text-xs text-muted-foreground">
                                            {chosenSaving != null ? (
                                                <>
                                                    About <b className="font-semibold text-foreground">{formatSize(chosenSaving)}</b> saved ({Math.round((100 * chosenSaving) / Math.max(1, measure.bytes))}%), measured on a sample. {HINTS[algorithm].hint}.
                                                </>
                                            ) : (
                                                HINTS[algorithm].hint
                                            )}{" "}
                                            Ctrl+Z decompresses them.
                                        </p>
                                    </section>
                                )}
                                {measure.wof > 0 && (
                                    <div className="flex items-center gap-3 rounded-md bg-muted/40 px-3 py-2">
                                        <span className="min-w-0 flex-1 text-xs text-muted-foreground">{plural(measure.wof, "file")} here are compressed this way. Decompressing needs as much free space as they'll take.</span>
                                        <Button size="sm" variant="outline" onClick={() => void run(true)}>
                                            <Expand /> Decompress
                                        </Button>
                                    </div>
                                )}
                            </>
                        )}
                    </>
                )}
                {running && progress && (
                    <div className="space-y-2 py-6">
                        <div className="flex items-center gap-2 text-muted-foreground">
                            <Loader2 className="size-4 animate-spin text-primary" />
                            <span className="truncate">
                                {decompressed ? "Decompressing" : "Compressing"} {progress.total > 0 && `${formatCount(progress.done)} of ${formatCount(progress.total)}`}
                                {progress.current && ` · ${progress.current}`}
                                {progress.saved > 0 && ` · ${formatSize(progress.saved)} ${decompressed ? "back" : "saved"}`}
                            </span>
                        </div>
                        <div className="h-1.5 overflow-hidden rounded-full bg-muted">
                            <div className="h-full rounded-full bg-primary transition-[width]" style={{ width: `${progress.total ? Math.round((100 * progress.done) / progress.total) : 0}%` }} />
                        </div>
                    </div>
                )}
                {error && (
                    <div className="flex flex-col items-center gap-2 py-6 text-center">
                        <AlertTriangle className="size-6 text-destructive" />
                        <p className="font-medium">{decompressed ? "Couldn't decompress them" : "Couldn't compress them"}</p>
                        <p className="max-w-md text-xs text-muted-foreground">{error}</p>
                    </div>
                )}
                {report && <WofResult report={report} decompressed={decompressed} />}
            </div>
            <div className="flex items-center justify-end gap-2 border-t bg-muted/20 px-4 py-2.5">
                {running && (
                    <Button variant="outline" size="sm" onClick={() => void invoke("wof_cancel")}>
                        <Square className="size-3! fill-current" /> Stop
                    </Button>
                )}
                {report || error ? (
                    <Button size="sm" onClick={close}>
                        Close
                    </Button>
                ) : (
                    <>
                        <Button variant="outline" size="sm" onClick={close} disabled={running}>
                            Cancel
                        </Button>
                        <Button size="sm" onClick={() => void run(false)} disabled={running || !measure?.ntfs || !measure.files}>
                            <Shrink /> Compress
                        </Button>
                    </>
                )}
            </div>
        </Modal>
    );
}

function WofResult({ report, decompressed }: { report: Report; decompressed: boolean }) {
    const { before, after } = report;
    const changed = Math.abs(before - after);
    return (
        <div className="space-y-3">
            <div className="flex items-start gap-2">
                <CheckCircle2 className={report.done ? "mt-0.5 size-5 shrink-0 text-emerald-500" : "mt-0.5 size-5 shrink-0 text-muted-foreground"} />
                <div>
                    <p className="font-medium">
                        {report.done ? `${decompressed ? "Decompressed" : "Compressed"} ${plural(report.done, "file")}` : decompressed ? "Nothing was decompressed" : "Nothing was compressed"}
                        {report.stopped && " before it was stopped"}
                    </p>
                    {report.done > 0 && (
                        <p className="text-xs text-muted-foreground">
                            {formatSize(before)} → {formatSize(after)} on the drive ({decompressed ? "+" : "−"}
                            {formatSize(changed)}) · {formatDuration(report.elapsedMs)}
                        </p>
                    )}
                    {report.unchanged > 0 && <p className="text-xs text-muted-foreground">{plural(report.unchanged, "file")} wouldn't get smaller, so Windows left them as they were.</p>}
                </div>
            </div>
            {report.failedCount > 0 && (
                <div className="rounded-md bg-amber-500/10 px-3 py-2 text-xs text-amber-900 dark:text-amber-300">
                    <p className="font-medium">{plural(report.failedCount, "file")} not done:</p>
                    <ul className="mt-1 space-y-0.5">
                        {report.failed.slice(0, 6).map(([path, why]) => (
                            <li key={path} className="truncate" title={`${path}\n${why}`}>
                                {baseName(path)}: {why}
                            </li>
                        ))}
                        {report.failedCount > 6 && <li>and {formatCount(report.failedCount - 6)} more</li>}
                    </ul>
                </div>
            )}
        </div>
    );
}
