import { useEffect, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { AlertTriangle, CheckCircle2, FolderInput, ImageDown, Loader2, Square } from "lucide-react";
import { type FileEntry, isArchived, isClosed, isOnlineOnly, isRecycled } from "@/lib/entries";
import { offerUndo, rememberUndo, revealInApp } from "@/lib/explorer";
import { formatCount, formatDuration, formatSize, plural } from "@/lib/format";
import { convertCancel, type ConvertFormat, convertImages, type ConvertOptions, type ConvertReport, fileOp } from "@/lib/fs";
import { baseName, parentPath, samePath } from "@/lib/paths";
import { refreshTools, useTool } from "@/lib/tools";
import { cn } from "@/lib/utils";
import { ToolNeeded } from "../ToolNeeded";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";

/** What rimage reads (convert.rs's READS). */
const CONVERTS = new Set(["avif", "bmp", "ff", "hdr", "jpg", "jpeg", "jpe", "jfif", "jxl", "png", "ppm", "pgm", "pbm", "psd", "qoi", "svg", "tif", "tiff", "webp"]);

/** A picture Convert can read: a file on disk, there to be read. */
export const canConvert = (e: FileEntry) => !e.dir && CONVERTS.has(e.ext) && !isArchived(e) && !isClosed(e) && !isRecycled(e) && !isOnlineOnly(e);

const useConvert = create<{ pictures: FileEntry[] | null }>(() => ({ pictures: null }));

/** Opens Convert for the pictures among `entries`. */
export function convertPictures(entries: FileEntry[]) {
    const pictures = entries.filter(canConvert);
    if (!pictures.length) return void toast.message("No pictures to convert", { description: "Convert reads JPEG, PNG, WebP, AVIF, TIFF, BMP, PSD, SVG and JPEG XL files." });
    useConvert.setState({ pictures });
}

const FORMATS: { format: ConvertFormat; label: string; hint: string }[] = [
    { format: "jpeg", label: "JPEG", hint: "Photos, opened by everything (MozJPEG)" },
    { format: "png", label: "PNG", hint: "Lossless: screenshots and drawings (OxiPNG)" },
    { format: "webp", label: "WebP", hint: "Smaller than JPEG, for the web" },
    { format: "avif", label: "AVIF", hint: "The smallest, for newer apps and browsers" },
];

const SIZES: { label: string; resize: ConvertOptions["resize"] }[] = [
    { label: "Keep their size", resize: null },
    { label: "75%", resize: { kind: "percent", value: 75 } },
    { label: "50%", resize: { kind: "percent", value: 50 } },
    { label: "25%", resize: { kind: "percent", value: 25 } },
    { label: "Longest side 3840 px", resize: { kind: "longest", value: 3840 } },
    { label: "Longest side 2560 px", resize: { kind: "longest", value: 2560 } },
    { label: "Longest side 1920 px", resize: { kind: "longest", value: 1920 } },
    { label: "Longest side 1280 px", resize: { kind: "longest", value: 1280 } },
];

/** The choices made last time, for the next (while Glint runs). */
let last: { format: ConvertFormat; quality: number; size: number; strip: boolean; where: "beside" | "folder" | "replace"; suffix: string; folder: string } = {
    format: "webp",
    quality: 80,
    size: 0,
    strip: false,
    where: "beside",
    suffix: "",
    folder: "Converted",
};

const NOT_IN_NAMES = /[\\/:*?"<>|]|[. ]$/;

function close() {
    useConvert.setState({ pictures: null });
}

export function ConvertDialog() {
    const pictures = useConvert((s) => s.pictures);
    if (!pictures) return null;
    return <ConvertModal key={pictures.map((p) => p.path).join("\n")} pictures={pictures} />;
}

function ConvertModal({ pictures }: { pictures: FileEntry[] }) {
    const tool = useTool("rimage");
    const [choice, setChoice] = useState(last);
    const [progress, setProgress] = useState<{ done: number; total: number; current: string } | null>(null);
    const [report, setReport] = useState<ConvertReport | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        void refreshTools();
    }, []);
    const running = !!progress && !report && !error;
    const choose = (patch: Partial<typeof last>) => setChoice((c) => (last = { ...c, ...patch }));

    const folders = new Set(pictures.map((p) => parentPath(p.path).toLowerCase()));
    const title = pictures.length === 1 ? `Convert “${pictures[0].name}”` : `Convert ${formatCount(pictures.length)} pictures`;
    const subtitle = folders.size === 1 ? parentPath(pictures[0].path) : `In ${formatCount(folders.size)} folders`;
    const suffixBad = choice.where === "beside" && NOT_IN_NAMES.test(choice.suffix);
    const folderBad = choice.where === "folder" && (!choice.folder.trim() || NOT_IN_NAMES.test(choice.folder));

    const start = async () => {
        const options: ConvertOptions = {
            format: choice.format,
            quality: choice.quality,
            resize: SIZES[choice.size]?.resize ?? null,
            strip: choice.strip,
            output: choice.where === "beside" ? { kind: "beside", suffix: choice.suffix } : choice.where === "folder" ? { kind: "folder", name: choice.folder.trim() } : { kind: "replace" },
        };
        setError(null);
        setReport(null);
        setProgress({ done: 0, total: pictures.length, current: "" });
        try {
            const done = await convertImages(
                pictures.map((p) => p.path),
                options,
                (p) => setProgress(p)
            );
            setReport(done);
            if (done.converted.length) {
                const made = done.converted;
                const what = plural(made.length, "picture");
                // Taken back as one step: what was made recycled, then the pictures it replaced restored.
                const step = rememberUndo(`conversion of ${what}`, async () => {
                    await fileOp({ kind: "delete", paths: made.map((c) => c.to), permanent: false, confirm: false });
                    const back = made.filter((c) => c.recycled);
                    if (back.length) await fileOp({ kind: "restore", items: back.map((c) => ({ path: c.recycled!, folder: parentPath(c.from), name: baseName(c.from) })) });
                });
                offerUndo(`Converted ${what}`, step);
            }
        } catch (e) {
            setError(String(e));
        }
    };

    const before = report?.converted.reduce((sum, c) => sum + c.before, 0) ?? 0;
    const after = report?.converted.reduce((sum, c) => sum + c.after, 0) ?? 0;
    const firstMade = report?.converted[0]?.to;
    const percent = progress && progress.total ? Math.round((100 * progress.done) / progress.total) : 0;

    return (
        <Modal label={title} onClose={close} className="max-h-[88vh] max-w-xl">
            <ModalHeader icon={ImageDown} title={title} subtitle={subtitle} onClose={close}>
                {running && (
                    <Button variant="outline" size="sm" className="h-7" onClick={() => void convertCancel()}>
                        <Square className="size-3! fill-current" /> Stop
                    </Button>
                )}
            </ModalHeader>

            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                {tool && !tool.origin && <ToolNeeded id="rimage" why="rimage converts pictures with the best encoders there are: MozJPEG, OxiPNG, WebP and AVIF." icon={ImageDown} />}

                {!progress && (
                    <>
                        <section className="space-y-1.5">
                            <h3 className="text-xs font-semibold text-muted-foreground">Into</h3>
                            <div role="radiogroup" aria-label="Format" className="grid grid-cols-2 gap-1.5">
                                {FORMATS.map(({ format, label, hint }) => (
                                    <button
                                        key={format}
                                        type="button"
                                        role="radio"
                                        aria-checked={choice.format === format}
                                        title={hint}
                                        onClick={() => choose({ format })}
                                        className={cn(
                                            "rounded-md border px-2 py-1.5 text-left transition-colors hover:bg-accent",
                                            choice.format === format && "border-primary bg-primary/10 hover:bg-primary/15"
                                        )}
                                    >
                                        <span className="block font-medium">{label}</span>
                                        <span className="block truncate text-[0.6875rem] text-muted-foreground">{hint}</span>
                                    </button>
                                ))}
                            </div>
                        </section>

                        {choice.format !== "png" && (
                            <label className="flex items-center gap-3">
                                <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Quality</span>
                                <input type="range" min={1} max={100} value={choice.quality} onChange={(e) => choose({ quality: Number(e.target.value) })} className="min-w-0 flex-1 accent-primary" />
                                <span className="w-8 text-right tabular-nums">{choice.quality}</span>
                            </label>
                        )}

                        <label className="flex items-center gap-3">
                            <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Size</span>
                            <select
                                value={choice.size}
                                onChange={(e) => choose({ size: Number(e.target.value) })}
                                aria-label="Size"
                                className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2 text-sm outline-hidden focus:border-primary"
                            >
                                {SIZES.map((s, i) => (
                                    <option key={s.label} value={i}>
                                        {s.label}
                                    </option>
                                ))}
                            </select>
                        </label>
                        <label className="flex items-center gap-2 pl-[5.75rem]">
                            <input type="checkbox" checked={choice.strip} onChange={(e) => choose({ strip: e.target.checked })} className="accent-primary" />
                            <span>Take out the camera, the place and when it was taken</span>
                        </label>

                        <section className="space-y-1.5" role="radiogroup" aria-label="Where">
                            <h3 className="text-xs font-semibold text-muted-foreground">Where</h3>
                            <label className="flex items-center gap-2">
                                <input type="radio" name="where" checked={choice.where === "beside"} onChange={() => choose({ where: "beside" })} className="accent-primary" />
                                <span className="shrink-0">Next to each picture, its name and</span>
                                <input
                                    value={choice.suffix}
                                    onChange={(e) => choose({ suffix: e.target.value, where: "beside" })}
                                    placeholder="nothing"
                                    aria-label="Added to the name"
                                    spellCheck={false}
                                    className={cn("h-7 w-28 rounded-md border bg-background px-2 text-sm outline-hidden focus:border-primary", suffixBad && "border-destructive")}
                                />
                            </label>
                            <label className="flex items-center gap-2">
                                <input type="radio" name="where" checked={choice.where === "folder"} onChange={() => choose({ where: "folder" })} className="accent-primary" />
                                <span className="shrink-0">Into a folder next to them:</span>
                                <input
                                    value={choice.folder}
                                    onChange={(e) => choose({ folder: e.target.value, where: "folder" })}
                                    aria-label="Folder name"
                                    spellCheck={false}
                                    className={cn("h-7 w-36 rounded-md border bg-background px-2 text-sm outline-hidden focus:border-primary", folderBad && "border-destructive")}
                                />
                            </label>
                            <label className="flex items-center gap-2">
                                <input type="radio" name="where" checked={choice.where === "replace"} onChange={() => choose({ where: "replace" })} className="accent-primary" />
                                <span>In their place, the pictures moved to the Recycle Bin</span>
                            </label>
                            <p className="pl-6 text-xs text-muted-foreground">A name that's taken gets “(2)”. Each keeps its picture's date modified, and Ctrl+Z takes it all back.</p>
                        </section>
                    </>
                )}

                {running && (
                    <div className="space-y-2 py-6">
                        <div className="flex items-center gap-2 text-muted-foreground">
                            <Loader2 className="size-4 animate-spin text-primary" />
                            <span className="truncate">
                                Converting {formatCount(Math.min(progress.done + 1, progress.total))} of {formatCount(progress.total)}
                                {progress.current && ` · ${progress.current} done`}
                            </span>
                        </div>
                        <div className="h-1.5 overflow-hidden rounded-full bg-muted">
                            <div className="h-full rounded-full bg-primary transition-[width]" style={{ width: `${percent}%` }} />
                        </div>
                    </div>
                )}

                {error && (
                    <div className="flex flex-col items-center gap-2 py-6 text-center">
                        <AlertTriangle className="size-6 text-destructive" />
                        <p className="font-medium">Couldn't convert</p>
                        <p className="max-w-md text-xs text-muted-foreground">{error}</p>
                    </div>
                )}

                {report && (
                    <div className="space-y-3">
                        <div className="flex items-start gap-2">
                            <CheckCircle2 className={cn("mt-0.5 size-5 shrink-0", report.converted.length ? "text-emerald-500" : "text-muted-foreground")} />
                            <div>
                                <p className="font-medium">
                                    {report.converted.length ? `Converted ${plural(report.converted.length, "picture")}` : "Nothing was converted"}
                                    {report.stopped && " before it was stopped"}
                                </p>
                                {report.converted.length > 0 && (
                                    <p className="text-xs text-muted-foreground">
                                        {formatSize(before)} → {formatSize(after)}
                                        {before > 0 && ` (${after <= before ? "−" : "+"}${Math.abs(Math.round((100 * (before - after)) / before))}%)`} · {formatDuration(report.elapsedMs)}
                                    </p>
                                )}
                            </div>
                        </div>
                        {report.failed.length > 0 && (
                            <div className="rounded-md bg-amber-500/10 px-3 py-2 text-xs text-amber-900 dark:text-amber-300">
                                <p className="font-medium">{plural(report.failed.length, "picture")} not converted:</p>
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
                )}
            </div>

            <div className="flex items-center justify-end gap-2 border-t bg-muted/20 px-4 py-2.5">
                {report && firstMade && (
                    <Button
                        variant="outline"
                        size="sm"
                        onClick={() => {
                            close();
                            // In a folder of their own, the folder; otherwise the first made, selected.
                            void revealInApp(samePath(parentPath(firstMade), parentPath(report.converted[0].from)) ? firstMade : parentPath(firstMade));
                        }}
                    >
                        <FolderInput /> Show
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
                        <Button size="sm" onClick={() => void start()} disabled={running || !tool?.origin || suffixBad || folderBad}>
                            <ImageDown /> Convert
                        </Button>
                    </>
                )}
            </div>
        </Modal>
    );
}
