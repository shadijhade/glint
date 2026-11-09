import { useEffect, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { Channel, invoke } from "@tauri-apps/api/core";
import { AudioLines, Captions, Clapperboard, Download, Film, Loader2 } from "lucide-react";
import type { FileEntry } from "@/lib/entries";
import type { ActionOutput } from "@/lib/actions";
import { badOutput } from "@/lib/actions";
import { formatCount, formatSize, plural } from "@/lib/format";
import { type MediaStatus, mediaStatus } from "@/lib/fs";
import { parentPath } from "@/lib/paths";
import { type AudioFormat, type Encoders, encoderName, isAudioFile, isMediaFile, isVideoFile, mediaCancel, mediaEncoders, mediaProbe, mediaRun, type Probe, type Quality, type VideoFormat } from "@/lib/studio";
import { refreshTools, useTool } from "@/lib/tools";
import { ToolNeeded } from "../ToolNeeded";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";
import { Choices, RunFooter, RunProgress, RunResult, useActionRun, WhereChooser } from "./ActionParts";

export type MediaMode = "convert" | "audio" | "gif" | "subtitles";

const useMedia = create<{ files: FileEntry[] | null; mode: MediaMode }>(() => ({ files: null, mode: "convert" }));

/** Opens Media studio on the videos and recordings among `entries`, at `mode`. */
export function openMedia(entries: FileEntry[], mode: MediaMode) {
    const files = entries.filter(mode === "convert" || mode === "gif" ? isVideoFile : isMediaFile);
    if (!files.length) return void toast.message(mode === "convert" || mode === "gif" ? "No videos chosen" : "No videos or recordings chosen");
    useMedia.setState({ files, mode });
}

const close = () => useMedia.setState({ files: null });

const MODES: { mode: MediaMode; label: string; icon: typeof Film }[] = [
    { mode: "convert", label: "Convert & shrink", icon: Film },
    { mode: "audio", label: "Sound only", icon: AudioLines },
    { mode: "gif", label: "GIF", icon: Clapperboard },
    { mode: "subtitles", label: "Subtitles", icon: Captions },
];

/** The choices made last time, for the next (while Glint runs). */
let last = {
    format: "h264" as VideoFormat,
    quality: "good" as "good" | "high" | "size",
    mb: 25,
    height: 0,
    audio: "same" as AudioFormat,
    gifWidth: 480,
    fps: 12,
    webp: false,
    translate: false,
};

export function MediaDialog() {
    const files = useMedia((s) => s.files);
    const mode = useMedia((s) => s.mode);
    if (!files) return null;
    return <MediaModal key={files.map((f) => f.path).join("\n")} files={files} initial={mode} />;
}

/** "1:05" or "12.5" as seconds; null when it isn't one. */
function seconds(text: string): number | null {
    const t = text.trim();
    if (!t) return null;
    let total = 0;
    for (const part of t.split(":")) {
        const n = Number(part);
        if (!Number.isFinite(n) || n < 0) return null;
        total = total * 60 + n;
    }
    return total;
}

function clock(s: number) {
    const m = Math.floor(s / 60);
    return `${m}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
}

function MediaModal({ files, initial }: { files: FileEntry[]; initial: MediaMode }) {
    const tool = useTool("ffmpeg");
    const [mode, setMode] = useState<MediaMode>(initial);
    const [choice, setChoice] = useState(last);
    const [encoders, setEncoders] = useState<Encoders | null>(null);
    const [probe, setProbe] = useState<Probe | null>(null);
    const [from, setFrom] = useState("");
    const [to, setTo] = useState("");
    const [speech, setSpeech] = useState<MediaStatus | null>(null);
    const [fetching, setFetching] = useState<string | null>(null);
    const [output, setOutput] = useState<ActionOutput>({ kind: "beside", suffix: initial === "convert" ? " (converted)" : "" });
    const run = useActionRun();
    const choose = (patch: Partial<typeof last>) => setChoice((c) => (last = { ...c, ...patch }));
    const ready = !!tool?.origin;

    useEffect(() => {
        void refreshTools();
    }, []);
    useEffect(() => {
        if (!ready) return;
        mediaEncoders().then(setEncoders, () => {});
        mediaProbe(files[0].path).then(setProbe, () => {});
    }, [ready, files]);
    useEffect(() => {
        if (mode === "subtitles") mediaStatus().then(setSpeech, () => {});
    }, [mode]);

    const videos = files.filter(isVideoFile);
    const usable = mode === "convert" || mode === "gif" ? videos : files.filter((f) => isVideoFile(f) || isAudioFile(f));
    const folders = new Set(files.map((f) => parentPath(f.path).toLowerCase()));
    const title = files.length === 1 ? files[0].name : `${formatCount(files.length)} ${videos.length === files.length ? "videos" : "files"}`;
    const subtitle = folders.size === 1 ? parentPath(files[0].path) : `In ${formatCount(folders.size)} folders`;

    const quality = (): Quality => (choice.quality === "size" ? { kind: "size", mb: Math.max(1, choice.mb) } : { kind: choice.quality });
    const range = { start: seconds(from), end: seconds(to) };
    const rangeBad = (from.trim() !== "" && range.start === null) || (to.trim() !== "" && range.end === null) || (range.start !== null && range.end !== null && range.end <= range.start);
    const encoderOf = (f: VideoFormat) => (encoders ? encoders[f] : null);
    const speechMissing = mode === "subtitles" && speech !== null && !speech.speechInstalled;

    const start = () => {
        const task =
            mode === "convert"
                ? ({ kind: "convert", format: choice.format, quality: quality(), height: choice.height || null } as const)
                : mode === "audio"
                  ? ({ kind: "audio", format: choice.audio } as const)
                  : mode === "gif"
                    ? ({ kind: "gif", width: choice.gifWidth, fps: choice.fps, start: range.start, end: range.end, webp: choice.webp } as const)
                    : ({ kind: "subtitles", translate: choice.translate } as const);
        const verb = { convert: "conversion", audio: "sound taken out", gif: "GIF", subtitles: "subtitles" }[mode];
        void run.start(
            usable.length,
            (p) =>
                mediaRun(
                    usable.map((f) => f.path),
                    task,
                    output,
                    p
                ),
            (r) => `${verb} of ${plural(r.made.length, "file")}`
        );
    };

    const fetchSpeech = async () => {
        const channel = new Channel<{ stage: string; done: number; total: number }>();
        channel.onmessage = (p) => setFetching(p.total ? `${p.stage} ${Math.round((100 * p.done) / p.total)}%` : p.stage);
        setFetching("Starting");
        try {
            setSpeech(await invoke<MediaStatus>("media_ai_install", { kind: "speech", onProgress: channel }));
        } catch (e) {
            if (String(e) !== "cancelled") toast.error("Couldn't download the speech model", { description: String(e) });
        } finally {
            setFetching(null);
        }
    };

    const Icon = MODES.find((m) => m.mode === mode)!.icon;
    return (
        <Modal label={`Media: ${title}`} onClose={close} className="max-h-[88vh] max-w-xl">
            <ModalHeader icon={Icon} title={title} subtitle={subtitle} onClose={close} />
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                {tool && !tool.origin && <ToolNeeded id="ffmpeg" why="FFmpeg converts, shrinks and cuts videos and sound, with your graphics card when it can." icon={Film} />}

                {!run.started && (
                    <>
                        <div role="tablist" aria-label="What to make" className="flex gap-1 rounded-lg bg-muted p-1">
                            {MODES.map((m) => (
                                <button
                                    key={m.mode}
                                    type="button"
                                    role="tab"
                                    aria-selected={mode === m.mode}
                                    disabled={(m.mode === "convert" || m.mode === "gif") && !videos.length}
                                    onClick={() => {
                                        setMode(m.mode);
                                        if (output.kind === "beside") setOutput({ kind: "beside", suffix: m.mode === "convert" ? " (converted)" : "" });
                                    }}
                                    className={`flex flex-1 items-center justify-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium transition-colors disabled:opacity-40 ${mode === m.mode ? "bg-background shadow-sm" : "text-muted-foreground hover:text-foreground"}`}
                                >
                                    <m.icon className="size-3.5" /> {m.label}
                                </button>
                            ))}
                        </div>

                        {probe && files.length === 1 && (
                            <p className="text-xs text-muted-foreground">
                                {[probe.video && `${probe.video.toUpperCase()} ${probe.width}×${probe.height}${probe.fps ? ` at ${Math.round(probe.fps)} fps` : ""}`, probe.audio && `${probe.audio.toUpperCase()} sound`, probe.duration && clock(probe.duration), formatSize(files[0].size)]
                                    .filter(Boolean)
                                    .join(" · ")}
                            </p>
                        )}

                        {mode === "convert" && (
                            <>
                                <section className="space-y-1.5">
                                    <h3 className="text-xs font-semibold text-muted-foreground">Into</h3>
                                    <Choices
                                        label="Format"
                                        columns={3}
                                        value={choice.format}
                                        onChange={(format) => choose({ format })}
                                        options={[
                                            { value: "h264", label: "MP4 (H.264)", hint: "Plays everywhere", disabled: !!encoders && !encoders.h264 },
                                            { value: "hevc", label: "MP4 (HEVC)", hint: "About half the size", disabled: !!encoders && !encoders.hevc },
                                            { value: "av1", label: "MP4 (AV1)", hint: encoders && !encoders.av1 ? "No AV1 encoder here" : "Smallest, newer devices", disabled: !!encoders && !encoders.av1 },
                                        ]}
                                    />
                                    {encoderOf(choice.format) && <p className="text-xs text-muted-foreground">Encoded with {encoderName(encoderOf(choice.format))}.</p>}
                                </section>
                                <section className="space-y-1.5">
                                    <h3 className="text-xs font-semibold text-muted-foreground">Quality</h3>
                                    <Choices
                                        label="Quality"
                                        columns={3}
                                        value={choice.quality}
                                        onChange={(q) => choose({ quality: q })}
                                        options={[
                                            { value: "good", label: "Good", hint: "Small, fine to share" },
                                            { value: "high", label: "High", hint: "Close to the original" },
                                            { value: "size", label: "Fit under a size", hint: "For mail and chats" },
                                        ]}
                                    />
                                    {choice.quality === "size" && (
                                        <div className="flex flex-wrap items-center gap-1.5 pt-1">
                                            {[8, 10, 25, 50, 100].map((mb) => (
                                                <Button key={mb} variant={choice.mb === mb ? "default" : "outline"} size="sm" className="h-7" onClick={() => choose({ mb })}>
                                                    {mb} MB
                                                </Button>
                                            ))}
                                            <input type="number" min={1} value={choice.mb} onChange={(e) => choose({ mb: Number(e.target.value) || 1 })} aria-label="Megabytes" className="h-7 w-20 rounded-md border bg-background px-2 text-sm" />
                                            <span className="text-xs text-muted-foreground">MB each</span>
                                        </div>
                                    )}
                                </section>
                                <label className="flex items-center gap-3">
                                    <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Height</span>
                                    <select value={choice.height} onChange={(e) => choose({ height: Number(e.target.value) })} aria-label="Height" className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2 text-sm">
                                        <option value={0}>Keep it</option>
                                        <option value={2160}>2160p (4K) at most</option>
                                        <option value={1080}>1080p at most</option>
                                        <option value={720}>720p at most</option>
                                        <option value={480}>480p at most</option>
                                    </select>
                                </label>
                            </>
                        )}

                        {mode === "audio" && (
                            <section className="space-y-1.5">
                                <h3 className="text-xs font-semibold text-muted-foreground">The sound, as</h3>
                                <Choices
                                    label="Format"
                                    columns={3}
                                    value={choice.audio}
                                    onChange={(audio) => choose({ audio })}
                                    options={[
                                        { value: "same", label: "As it is", hint: "No quality lost, at once" },
                                        { value: "mp3", label: "MP3", hint: "Plays everywhere" },
                                        { value: "m4a", label: "M4A (AAC)", hint: "Phones and iTunes" },
                                        { value: "opus", label: "Opus", hint: "Smallest for speech" },
                                        { value: "flac", label: "FLAC", hint: "Lossless" },
                                        { value: "wav", label: "WAV", hint: "Uncompressed, for editing" },
                                    ]}
                                />
                            </section>
                        )}

                        {mode === "gif" && (
                            <>
                                <Choices
                                    label="Kind"
                                    value={choice.webp ? "webp" : "gif"}
                                    onChange={(k) => choose({ webp: k === "webp" })}
                                    options={[
                                        { value: "gif", label: "GIF", hint: "Works in every chat and page" },
                                        { value: "webp", label: "Animated WebP", hint: encoders && !encoders.webp ? "This FFmpeg can't make WebP" : "Far smaller, modern browsers", disabled: !!encoders && !encoders.webp },
                                    ]}
                                />
                                <label className="flex items-center gap-3">
                                    <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Width</span>
                                    <select value={choice.gifWidth} onChange={(e) => choose({ gifWidth: Number(e.target.value) })} aria-label="Width" className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2 text-sm">
                                        {[320, 480, 640, 800, 1080].map((w) => (
                                            <option key={w} value={w}>
                                                {w} pixels
                                            </option>
                                        ))}
                                    </select>
                                </label>
                                <label className="flex items-center gap-3">
                                    <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Frames</span>
                                    <select value={choice.fps} onChange={(e) => choose({ fps: Number(e.target.value) })} aria-label="Frames a second" className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2 text-sm">
                                        {[8, 10, 12, 15, 20, 24].map((f) => (
                                            <option key={f} value={f}>
                                                {f} a second
                                            </option>
                                        ))}
                                    </select>
                                </label>
                                <div className="flex items-center gap-2">
                                    <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Part</span>
                                    <input value={from} onChange={(e) => setFrom(e.target.value)} placeholder="0:00" aria-label="From" className="h-7 w-20 rounded-md border bg-background px-2 text-sm" />
                                    <span className="text-muted-foreground">to</span>
                                    <input value={to} onChange={(e) => setTo(e.target.value)} placeholder={probe ? clock(probe.duration) : "the end"} aria-label="To" className="h-7 w-20 rounded-md border bg-background px-2 text-sm" />
                                    {rangeBad && <span className="text-xs text-destructive">Not a part of it</span>}
                                </div>
                                <p className="text-xs text-muted-foreground">Keep it short: a GIF of a few seconds is already a few megabytes.</p>
                            </>
                        )}

                        {mode === "subtitles" && (
                            <>
                                <Choices
                                    label="Language"
                                    value={choice.translate ? "en" : "same"}
                                    onChange={(v) => choose({ translate: v === "en" })}
                                    options={[
                                        { value: "same", label: "As spoken", hint: "In the language heard" },
                                        { value: "en", label: "In English", hint: "Translated as it's heard" },
                                    ]}
                                />
                                <p className="text-xs text-muted-foreground">Heard on this PC by whisper.cpp (speech search's model); nothing is sent anywhere. Players find an .srt named like its video by themselves.</p>
                                {speechMissing && (
                                    <div className="flex items-center gap-3 rounded-lg border bg-primary/5 px-4 py-3">
                                        <Captions className="size-4 shrink-0 text-primary" />
                                        <span className="min-w-0 flex-1">Subtitles need whisper.cpp and its speech model (60 MB).</span>
                                        <Button size="sm" disabled={!!fetching} onClick={() => void fetchSpeech()}>
                                            {fetching ? <Loader2 className="animate-spin" /> : <Download />} {fetching ?? "Download"}
                                        </Button>
                                    </div>
                                )}
                            </>
                        )}

                        {usable.length < files.length && <p className="text-xs text-amber-700 dark:text-amber-400">{plural(files.length - usable.length, "file")} without a video will be left out.</p>}

                        <WhereChooser value={output} onChange={setOutput} replace={mode === "convert"} noun={mode === "convert" ? "video" : "file"} />
                    </>
                )}

                {run.running && run.progress && <RunProgress progress={run.progress} verb={mode === "subtitles" ? "Listening to" : "Working on"} />}
                <RunResult report={run.report} error={run.error} failedWhat="Couldn't make it" sizes={mode === "convert"} />
            </div>
            <RunFooter
                run={run}
                onClose={close}
                onStart={start}
                onStop={() => void mediaCancel()}
                startLabel={{ convert: "Convert", audio: "Take the sound out", gif: "Make it", subtitles: "Make subtitles" }[mode]}
                startIcon={Icon}
                disabled={!ready || !usable.length || badOutput(output) || (mode === "gif" && rangeBad) || speechMissing}
            />
        </Modal>
    );
}
