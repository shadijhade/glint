import { useEffect, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { invoke } from "@tauri-apps/api/core";
import { FileArchive, FileImage, FileText, Film, Minimize2, Shrink } from "lucide-react";
import { type FileEntry, isArchived, isClosed, isOnlineOnly, isRecycled } from "@/lib/entries";
import { type ActionOutput, badOutput, offerActionUndo, runAction } from "@/lib/actions";
import { compressEntries } from "@/lib/explorer";
import { formatCount, plural } from "@/lib/format";
import { parentPath } from "@/lib/paths";
import { useTool } from "@/lib/tools";
import { ToolNeeded } from "../ToolNeeded";
import { openCompressInPlace } from "./CompressInPlaceDialog";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";
import { Choices, RunFooter, RunProgress, RunResult, useActionRun, WhereChooser } from "./ActionParts";

/** What Make smaller does with each kind of file (shrink.rs's kind_of). */
type Kind = "video" | "jpeg" | "bmp" | "png" | "pdf" | "office";
const KINDS: Record<string, Kind> = {};
for (const [kind, exts] of [
    ["video", "mp4 m4v mov mkv webm avi wmv flv mts m2ts ts 3gp mpg mpeg"],
    ["jpeg", "jpg jpeg jpe jfif"],
    ["bmp", "bmp"],
    ["png", "png"],
    ["pdf", "pdf"],
    ["office", "docx docm pptx pptm xlsx xlsm"],
] as const)
    for (const ext of exts.split(" ")) KINDS[ext] = kind;

const extOf = (name: string) => (name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "");
const onDisk = (e: FileEntry) => !isArchived(e) && !isClosed(e) && !isRecycled(e) && !isOnlineOnly(e);
const kindOf = (e: FileEntry): Kind | undefined => (e.dir ? undefined : KINDS[extOf(e.name)]);
/** A file Make smaller works on. */
export const canShrink = (e: FileEntry) => onDisk(e) && !!kindOf(e);
/** A JPEG XL file, for Back to JPEG. */
export const isJxl = (e: FileEntry) => onDisk(e) && !e.dir && extOf(e.name) === "jxl";

const useShrink = create<{ files: FileEntry[] | null; folders: FileEntry[] }>(() => ({ files: null, folders: [] }));

/** Opens Make smaller on what among `entries` it works on (folders are offered Compress instead). */
export function openShrink(entries: FileEntry[]) {
    const files = entries.filter(canShrink);
    const folders = entries.filter((e) => e.dir && onDisk(e));
    if (!files.length && !folders.length) return void toast.message("Make smaller works on videos, photos, PNGs, BMPs, PDFs, and Word, PowerPoint and Excel files");
    useShrink.setState({ files, folders });
}

/** Back to JPEG: JPEG XL files as JPEGs beside them (the very JPEG, for ones Make smaller repacked). */
export async function backToJpeg(entries: FileEntry[]) {
    const files = entries.filter(isJxl).map((e) => e.path);
    if (!files.length) return;
    try {
        const report = await runAction("jxl_to_jpeg", { files, output: { kind: "beside", suffix: "" } }, () => {});
        if (report.made.length) offerActionUndo(report, `JPEG${report.made.length > 1 ? "s" : ""} from JPEG XL`);
        if (report.failed.length) toast.error(`Couldn't make ${plural(report.failed.length, "JPEG")}`, { description: report.failed[0][1] });
    } catch (e) {
        toast.error("Couldn't make JPEGs", { description: String(e) });
    }
}

const close = () => useShrink.setState({ files: null, folders: [] });

export function ShrinkDialog() {
    const files = useShrink((s) => s.files);
    const folders = useShrink((s) => s.folders);
    if (!files) return null;
    return <ShrinkModal key={files.map((f) => f.path).join("\n")} files={files} folders={folders} />;
}

type Engines = { h264: boolean; hevc: boolean; av1: boolean; vmaf: boolean };
type VideoCodec = "h264" | "hevc" | "av1";
type PhotoWay = "jpeg" | "jxl";
type PdfWay = "screen" | "print" | "lossless";

function ShrinkModal({ files, folders }: { files: FileEntry[]; folders: FileEntry[] }) {
    const counts: Record<Kind, number> = { video: 0, jpeg: 0, bmp: 0, png: 0, pdf: 0, office: 0 };
    for (const f of files) counts[kindOf(f)!]++;
    const [video, setVideo] = useState<VideoCodec>("h264");
    const [photo, setPhoto] = useState<PhotoWay>("jpeg");
    const [pdf, setPdf] = useState<PdfWay>("screen");
    const [quality, setQuality] = useState(80);
    const [output, setOutput] = useState<ActionOutput>({ kind: "beside", suffix: " (smaller)" });
    const [engines, setEngines] = useState<Engines | null>(null);
    const run = useActionRun();
    const tools = { ffmpeg: useTool("ffmpeg"), abav1: useTool("ab-av1"), rimage: useTool("rimage"), libjxl: useTool("libjxl"), ghostscript: useTool("ghostscript"), pdfcpu: useTool("pdfcpu") };

    const ffmpegReady = !!tools.ffmpeg?.origin;
    useEffect(() => {
        if (counts.video && ffmpegReady) {
            invoke<Engines>("shrink_engines")
                .then((e) => {
                    setEngines(e);
                    if (!e.h264) setVideo(e.hevc ? "hevc" : "av1");
                })
                .catch(() => setEngines({ h264: false, hevc: false, av1: false, vmaf: false }));
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [ffmpegReady]);

    // The tools this selection needs, as chosen.
    const needs: { id: string; ready: boolean }[] = [];
    if (counts.video) needs.push({ id: "ffmpeg", ready: ffmpegReady }, { id: "ab-av1", ready: !!tools.abav1?.origin });
    if ((counts.jpeg && photo === "jpeg") || counts.png || counts.bmp || counts.office) needs.push({ id: "rimage", ready: !!tools.rimage?.origin });
    if (counts.jpeg && photo === "jxl") needs.push({ id: "libjxl", ready: !!tools.libjxl?.origin });
    if (counts.pdf) needs.push(pdf === "lossless" ? { id: "pdfcpu", ready: !!tools.pdfcpu?.origin } : { id: "ghostscript", ready: !!tools.ghostscript?.origin });
    const ready = needs.every((n) => n.ready) && (!counts.video || !!engines);
    const videoOk = !counts.video || !engines || engines[video];

    const parts = [
        counts.video && plural(counts.video, "video"),
        counts.jpeg && plural(counts.jpeg, "photo"),
        counts.png + counts.bmp && plural(counts.png + counts.bmp, "picture"),
        counts.pdf && plural(counts.pdf, "PDF"),
        counts.office && plural(counts.office, "Office file"),
        folders.length && plural(folders.length, "folder"),
    ].filter(Boolean);
    const dirs = new Set([...files, ...folders].map((f) => parentPath(f.path).toLowerCase()));
    const title = files.length + folders.length === 1 ? (files[0] ?? folders[0]).name : parts.join(", ");

    const start = () =>
        void run.start(
            files.length,
            (p) => runAction("shrink_run", { files: files.map((f) => f.path), choices: { video, photo, pdf, quality }, output }, p),
            (r) => `making ${plural(r.made.length, "file")} smaller`
        );

    return (
        <Modal label="Make smaller" onClose={close} onSubmit={() => ready && files.length > 0 && !run.started && start()} className="max-h-[88vh] max-w-xl">
            <ModalHeader icon={Minimize2} title={`Make smaller: ${title}`} subtitle={dirs.size === 1 ? parentPath((files[0] ?? folders[0]).path) : `In ${formatCount(dirs.size)} folders`} onClose={close} />
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                {!run.started && (
                    <>
                        <p className="text-muted-foreground">Each file is made smaller by the best tool for its kind. Nothing that would come out bigger is kept, and every copy keeps its original's date.</p>
                        {needs.map((n) => (
                            <ToolNeeded key={n.id} id={n.id} why={WHY[n.id]} icon={n.id === "ffmpeg" || n.id === "ab-av1" ? Film : n.id === "ghostscript" || n.id === "pdfcpu" ? FileText : FileImage} />
                        ))}
                        {counts.video > 0 && (
                            <section className="space-y-1.5">
                                <h3 className="text-xs font-semibold text-muted-foreground">{plural(counts.video, "video")}: looks the same, smaller</h3>
                                <Choices
                                    label="Video format"
                                    columns={3}
                                    value={video}
                                    onChange={setVideo}
                                    options={[
                                        { value: "h264", label: "H.264", hint: "Plays everywhere", disabled: !!engines && !engines.h264 },
                                        { value: "hevc", label: "HEVC", hint: "Smaller; phones, newer PCs", disabled: !!engines && !engines.hevc },
                                        { value: "av1", label: "AV1", hint: engines && !engines.av1 ? "Needs FFmpeg's full build" : "Smallest; newest devices", disabled: !!engines && !engines.av1 },
                                    ]}
                                />
                                <p className="text-xs text-muted-foreground">
                                    Short samples are encoded and scored by {engines && !engines.vmaf ? "XPSNR" : "VMAF"} first, to find the smallest size that still looks the same, then the whole video is encoded: it takes a while. An HDR video keeps its
                                    colours as HEVC or AV1.
                                </p>
                            </section>
                        )}
                        {counts.jpeg > 0 && (
                            <section className="space-y-1.5">
                                <h3 className="text-xs font-semibold text-muted-foreground">{plural(counts.jpeg, "photo")}</h3>
                                <Choices
                                    label="Photos"
                                    value={photo}
                                    onChange={setPhoto}
                                    options={[
                                        { value: "jpeg", label: "A smaller JPEG", hint: "Opens everywhere" },
                                        { value: "jxl", label: "JPEG XL, losslessly", hint: "About a fifth smaller, every bit kept" },
                                    ]}
                                />
                                {photo === "jxl" && <p className="text-xs text-muted-foreground">Windows 11 and Chrome open JPEG XL. Back to JPEG, on a .jxl file's menu, gives back the very same JPEG.</p>}
                            </section>
                        )}
                        {((counts.jpeg > 0 && photo === "jpeg") || counts.office > 0) && (
                            <section className="space-y-1.5">
                                <h3 className="text-xs font-semibold text-muted-foreground">JPEG quality{counts.office ? " (pictures in Office files too)" : ""}</h3>
                                <Choices
                                    label="JPEG quality"
                                    columns={3}
                                    value={quality}
                                    onChange={setQuality}
                                    options={[
                                        { value: 70, label: "Smallest", hint: "Quality 70" },
                                        { value: 80, label: "Balanced", hint: "Quality 80" },
                                        { value: 90, label: "Best", hint: "Quality 90" },
                                    ]}
                                />
                            </section>
                        )}
                        {counts.pdf > 0 && (
                            <section className="space-y-1.5">
                                <h3 className="text-xs font-semibold text-muted-foreground">{plural(counts.pdf, "PDF")}</h3>
                                <Choices
                                    label="PDFs"
                                    columns={3}
                                    value={pdf}
                                    onChange={setPdf}
                                    options={[
                                        { value: "screen", label: "For screens and mail", hint: "Pictures at 150 dpi" },
                                        { value: "print", label: "For print", hint: "Pictures at 300 dpi" },
                                        { value: "lossless", label: "Lossless", hint: "Nothing changes on the page" },
                                    ]}
                                />
                                {pdf !== "lossless" && <p className="text-xs text-muted-foreground">Ghostscript rewrites the PDF: its text and links stay, a digital signature wouldn't.</p>}
                            </section>
                        )}
                        {(counts.png > 0 || counts.bmp > 0) && <p className="text-xs text-muted-foreground">PNGs are made smaller losslessly; BMPs become PNGs, the same picture at a fraction of the size.</p>}
                        {counts.office > 0 && <p className="text-xs text-muted-foreground">Word, PowerPoint and Excel files get their pictures made smaller (at most 3,000 pixels across); the rest of each file stays as it was.</p>}
                        {folders.length > 0 && (
                            <div className="flex items-center gap-3 rounded-md bg-muted/40 px-3 py-2">
                                <span className="min-w-0 flex-1 text-xs text-muted-foreground">{folders.length === 1 ? `“${folders[0].name}” is a folder: compress it in place (it still opens normally), or into one smaller file.` : `${formatCount(folders.length)} folders: compress them in place (they still open normally), or into smaller files.`}</span>
                                <Button
                                    size="sm"
                                    variant="outline"
                                    title="Windows stores them compressed; they open as before"
                                    onClick={() => {
                                        openCompressInPlace(folders);
                                        if (!files.length) close();
                                    }}
                                >
                                    <Shrink /> In place
                                </Button>
                                <Button
                                    size="sm"
                                    variant="outline"
                                    onClick={() => {
                                        void compressEntries(folders, "7z");
                                        if (!files.length) close();
                                    }}
                                >
                                    <FileArchive /> Compress to 7z
                                </Button>
                            </div>
                        )}
                        {files.length > 0 && <WhereChooser value={output} onChange={setOutput} noun="file" note="In their place, the originals go to the Recycle Bin, and Ctrl+Z takes it all back." />}
                    </>
                )}
                {run.running && run.progress && <RunProgress progress={run.progress} verb="Making smaller" />}
                <RunResult report={run.report} error={run.error} failedWhat="Couldn't make them smaller" />
            </div>
            <RunFooter run={run} onClose={close} onStart={start} onStop={() => void invoke("shrink_cancel")} startLabel="Make smaller" startIcon={Minimize2} disabled={!files.length || !ready || !videoOk || badOutput(output)} />
        </Modal>
    );
}

const WHY: Record<string, string> = {
    ffmpeg: "FFmpeg encodes the videos, on this PC.",
    "ab-av1": "ab-av1 finds the smallest size at which a video still looks the same, from short samples.",
    rimage: "rimage makes photos and pictures smaller, on this PC.",
    libjxl: "libjxl repacks JPEGs as JPEG XL without losing a bit.",
    ghostscript: "Ghostscript rewrites a PDF's pictures for screens or print, on this PC.",
    pdfcpu: "pdfcpu makes PDFs smaller without changing what's on their pages.",
};
