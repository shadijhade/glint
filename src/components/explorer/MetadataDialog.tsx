import { useEffect, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { invoke } from "@tauri-apps/api/core";
import { CalendarClock, EyeOff, MapPin, ShieldCheck } from "lucide-react";
import { type FileEntry, isArchived, isClosed, isOnlineOnly, isRecycled } from "@/lib/entries";
import { type ActionOutput, badOutput, runAction } from "@/lib/actions";
import { formatCount, plural } from "@/lib/format";
import { parentPath } from "@/lib/paths";
import { refreshTools, useTool } from "@/lib/tools";
import { ToolNeeded } from "../ToolNeeded";
import { Modal, ModalHeader } from "../Modal";
import { Choices, RunFooter, RunProgress, RunResult, useActionRun, WhereChooser } from "./ActionParts";

/** What ExifTool writes, and Office's files (cleaned in Glint itself): metadata.rs. */
const EXIFTOOL = new Set(["jpg", "jpeg", "jpe", "png", "tif", "tiff", "webp", "heic", "heif", "avif", "jxl", "gif", "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf", "psd", "mp4", "m4v", "mov", "3gp", "m4a", "pdf"]);
const OFFICE = new Set(["docx", "docm", "xlsx", "xlsm", "pptx", "pptm"]);
const DATED = new Set(["jpg", "jpeg", "jpe", "png", "tif", "tiff", "webp", "heic", "heif", "avif", "jxl", "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf", "mp4", "m4v", "mov", "3gp"]);

const onDisk = (e: FileEntry) => !e.dir && !isArchived(e) && !isClosed(e) && !isRecycled(e) && !isOnlineOnly(e);
/** A file whose personal info Glint can take out. */
export const canClean = (e: FileEntry) => onDisk(e) && (EXIFTOOL.has(e.ext) || OFFICE.has(e.ext));
/** A photo or video whose date taken can be set. */
export const canRedate = (e: FileEntry) => onDisk(e) && DATED.has(e.ext);

type Mode = "scrub" | "date";
const useMeta = create<{ files: FileEntry[] | null; mode: Mode }>(() => ({ files: null, mode: "scrub" }));

/** Opens Remove personal info (or the date taken) on the files among `entries` it can change. */
export function openMetadata(entries: FileEntry[], mode: Mode) {
    const files = entries.filter(mode === "scrub" ? canClean : canRedate);
    if (!files.length) return void toast.message(mode === "scrub" ? "Nothing chosen that Glint can clean" : "No photos or videos chosen", { description: mode === "scrub" ? "Photos, videos, PDFs and Office files." : undefined });
    useMeta.setState({ files, mode });
}

const close = () => useMeta.setState({ files: null });

interface Tag {
    group: string;
    name: string;
    value: string;
    kind: "place" | "device" | "people" | "history" | "dates" | "other";
}

const KINDS: Record<Tag["kind"], string> = {
    place: "Where it was",
    people: "Who made it",
    device: "The camera, phone or app",
    history: "Its editing history",
    dates: "When",
    other: "Everything else",
};

export function MetadataDialog() {
    const files = useMeta((s) => s.files);
    const mode = useMeta((s) => s.mode);
    if (!files) return null;
    return <MetadataModal key={files.map((f) => f.path).join("\n")} files={files} mode={mode} />;
}

/** "2024-06-01T12:00" (an input's) as ExifTool's "2024:06:01 12:00:00". */
const exifDate = (local: string) => (local ? `${local.slice(0, 10).replace(/-/g, ":")} ${local.slice(11, 16)}:00` : "");

function MetadataModal({ files, mode }: { files: FileEntry[]; mode: Mode }) {
    const tool = useTool("exiftool");
    const needsTool = files.some((f) => !OFFICE.has(f.ext));
    const [tags, setTags] = useState<Tag[] | null>(null);
    const [dateMode, setDateMode] = useState<"set" | "shift">("shift");
    const [when, setWhen] = useState("");
    const [hours, setHours] = useState(0);
    const [minutes, setMinutes] = useState(0);
    const [output, setOutput] = useState<ActionOutput>(mode === "scrub" ? { kind: "beside", suffix: " (clean)" } : { kind: "replace" });
    const run = useActionRun();
    const ready = !needsTool || !!tool?.origin;

    useEffect(() => {
        void refreshTools();
    }, []);
    useEffect(() => {
        if (mode === "scrub" && files.length === 1 && ready) invoke<Tag[]>("meta_read", { path: files[0].path }).then(setTags, () => setTags([]));
    }, [mode, files, ready]);

    const folders = new Set(files.map((f) => parentPath(f.path).toLowerCase()));
    const title = files.length === 1 ? files[0].name : `${formatCount(files.length)} files`;
    const shift = hours * 60 + minutes;
    const dateBad = mode === "date" && (dateMode === "set" ? !when : shift === 0);

    const start = () => {
        const task = mode === "scrub" ? { kind: "scrub" } : dateMode === "set" ? { kind: "setDate", when: exifDate(when) } : { kind: "shiftDate", minutes: shift };
        void run.start(
            files.length,
            (p) => runAction("meta_run", { files: files.map((f) => f.path), task, output }, p),
            (r) => (mode === "scrub" ? `cleaning of ${plural(r.made.length, "file")}` : `date change of ${plural(r.made.length, "file")}`)
        );
    };

    const grouped = tags ? (Object.keys(KINDS) as Tag["kind"][]).map((k) => [k, tags.filter((t) => t.kind === k)] as const).filter(([, list]) => list.length) : [];
    const personal = tags?.filter((t) => t.kind !== "other" && t.kind !== "dates").length ?? 0;

    return (
        <Modal label={mode === "scrub" ? "Remove personal info" : "Date taken"} onClose={close} className="max-h-[88vh] max-w-xl">
            <ModalHeader icon={mode === "scrub" ? EyeOff : CalendarClock} title={mode === "scrub" ? `Remove personal info: ${title}` : `When it was taken: ${title}`} subtitle={folders.size === 1 ? parentPath(files[0].path) : `In ${formatCount(folders.size)} folders`} onClose={close} />
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                {needsTool && tool && !tool.origin && <ToolNeeded id="exiftool" why="ExifTool reads and takes out what photos, videos and PDFs say about where, who and with what. Nothing is sent anywhere." icon={ShieldCheck} />}

                {!run.started && mode === "scrub" && (
                    <>
                        <p className="text-muted-foreground">
                            Takes out where a photo was taken, the camera and its serial number, names, the apps used and editing history. Pictures keep their orientation and colors; nothing in what they show changes.
                        </p>
                        {tags && files.length === 1 && (
                            <div className="space-y-2 rounded-md border p-2">
                                <p className="flex items-center gap-1.5 text-xs font-medium">
                                    <MapPin className="size-3.5 text-primary" />
                                    {tags.length ? `${plural(tags.length, "detail")} in it, ${personal} about where, who or with what` : "It carries no details to take out"}
                                </p>
                                <div className="max-h-56 space-y-2 overflow-y-auto">
                                    {grouped.map(([kind, list]) => (
                                        <details key={kind} open={kind === "place" || kind === "people" || kind === "device"}>
                                            <summary className="cursor-pointer text-xs font-semibold text-muted-foreground">
                                                {KINDS[kind]} ({list.length})
                                            </summary>
                                            <table className="mt-1 w-full text-xs">
                                                <tbody>
                                                    {list.slice(0, 40).map((t) => (
                                                        <tr key={`${t.group}:${t.name}`} className="[&>td]:py-0.5">
                                                            <td className="w-40 truncate pe-2 text-muted-foreground" title={`${t.group}:${t.name}`}>
                                                                {t.name}
                                                            </td>
                                                            <td className="max-w-0 truncate" title={t.value}>
                                                                {t.value}
                                                            </td>
                                                        </tr>
                                                    ))}
                                                </tbody>
                                            </table>
                                        </details>
                                    ))}
                                </div>
                            </div>
                        )}
                        <WhereChooser value={output} onChange={setOutput} note="A copy to share keeps the original as it is; in their place, Ctrl+Z puts the originals back." />
                    </>
                )}

                {!run.started && mode === "date" && (
                    <>
                        <Choices
                            label="How"
                            value={dateMode}
                            onChange={setDateMode}
                            options={[
                                { value: "shift", label: "Move it", hint: "A camera clock that was off: every one moved the same" },
                                { value: "set", label: "Set it", hint: "The same date and time for each" },
                            ]}
                        />
                        {dateMode === "set" ? (
                            <label className="flex items-center gap-3">
                                <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Taken</span>
                                <input type="datetime-local" value={when} onChange={(e) => setWhen(e.target.value)} aria-label="Date taken" className="h-8 rounded-md border bg-background px-2" />
                            </label>
                        ) : (
                            <div className="flex items-center gap-2">
                                <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">By</span>
                                <input type="number" value={hours} onChange={(e) => setHours(Math.trunc(Number(e.target.value) || 0))} aria-label="Hours" className="h-8 w-20 rounded-md border bg-background px-2" />
                                <span className="text-muted-foreground">hours</span>
                                <input type="number" value={minutes} onChange={(e) => setMinutes(Math.trunc(Number(e.target.value) || 0))} aria-label="Minutes" className="h-8 w-20 rounded-md border bg-background px-2" />
                                <span className="text-muted-foreground">minutes (less than 0 for earlier)</span>
                            </div>
                        )}
                        <WhereChooser value={output} onChange={setOutput} />
                    </>
                )}

                {run.running && run.progress && <RunProgress progress={run.progress} verb={mode === "scrub" ? "Cleaning" : "Changing"} />}
                <RunResult report={run.report} error={run.error} failedWhat="Couldn't change it" sizes={false} />
            </div>
            <RunFooter
                run={run}
                onClose={close}
                onStart={start}
                onStop={() => void invoke("meta_cancel")}
                startLabel={mode === "scrub" ? "Remove personal info" : "Change the date"}
                startIcon={mode === "scrub" ? EyeOff : CalendarClock}
                disabled={!ready || badOutput(output) || dateBad}
            />
        </Modal>
    );
}
