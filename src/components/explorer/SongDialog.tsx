import { useEffect, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { invoke } from "@tauri-apps/api/core";
import { open as pickFile } from "@tauri-apps/plugin-dialog";
import { Music, Wand2 } from "lucide-react";
import { type FileEntry, isArchived, isClosed, isOnlineOnly, isRecycled } from "@/lib/entries";
import { runAction } from "@/lib/actions";
import { formatCount, plural } from "@/lib/format";
import { baseName, parentPath } from "@/lib/paths";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";
import { RunFooter, RunProgress, RunResult, useActionRun } from "./ActionParts";

/** What lofty reads and writes details in (songtags.rs). */
const SONGS = new Set(["mp3", "flac", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wav", "aif", "aiff", "ape", "wv", "mpc", "spx"]);
const extOf = (name: string) => (name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "");
/** A song whose details Glint edits. */
export const isSong = (e: FileEntry) => !e.dir && !isArchived(e) && !isClosed(e) && !isRecycled(e) && !isOnlineOnly(e) && SONGS.has(extOf(e.name));

type Tags = { path: string; title: string; artist: string; album: string; albumArtist: string; track: number | null; trackTotal: number | null; year: number | null; genre: string; cover: boolean; error: string | null };
type Field = "title" | "artist" | "album" | "albumArtist" | "track" | "trackTotal" | "year" | "genre";
const FIELDS: { key: Field; label: string; each?: boolean }[] = [
    { key: "title", label: "Title", each: true },
    { key: "artist", label: "Artist" },
    { key: "album", label: "Album" },
    { key: "albumArtist", label: "Album artist" },
    { key: "track", label: "Track", each: true },
    { key: "trackTotal", label: "Tracks in all" },
    { key: "year", label: "Year" },
    { key: "genre", label: "Genre" },
];

const useSongs = create<{ files: FileEntry[] | null }>(() => ({ files: null }));

/** Opens Edit song details on the songs among `entries`. */
export function openSongs(entries: FileEntry[]) {
    const files = entries.filter(isSong);
    if (!files.length) return void toast.message("Edit song details works on MP3, FLAC, M4A, Ogg, Opus, WAV and AIFF files");
    useSongs.setState({ files });
}

const close = () => useSongs.setState({ files: null });

export function SongDialog() {
    const files = useSongs((s) => s.files);
    if (!files) return null;
    return <SongModal key={files.map((f) => f.path).join("\n")} files={files} />;
}

const show = (v: string | number | null) => (v === null || v === undefined ? "" : String(v));

/** "03 - Artist - Title" → its track number and title, from a file's name. */
export function fromName(name: string) {
    const stem = name.replace(/\.[^.]+$/, "");
    const m = /^\s*(\d{1,3})\s*[-._)\s]\s*(.*)$/.exec(stem);
    const rest = (m ? m[2] : stem).trim();
    const parts = rest.split(/\s+-\s+/);
    return { track: m ? String(Number(m[1])) : "", title: parts[parts.length - 1] || rest };
}

function SongModal({ files }: { files: FileEntry[] }) {
    const [tags, setTags] = useState<Tags[] | null>(null);
    // For one song, every field; for many, what's typed applies to all (the titles and tracks stay each song's own, unless filled in from names).
    const [values, setValues] = useState<Record<Field, string>>({ title: "", artist: "", album: "", albumArtist: "", track: "", trackTotal: "", year: "", genre: "" });
    const [touched, setTouched] = useState<Set<Field>>(new Set());
    const [fromNames, setFromNames] = useState(false);
    const [numbered, setNumbered] = useState(false);
    const [cover, setCover] = useState<string | null | undefined>(undefined);
    const run = useActionRun();
    const one = files.length === 1;

    useEffect(() => {
        invoke<Tags[]>("song_tags", { paths: files.map((f) => f.path) })
            .then((read) => {
                setTags(read);
                // Shown: one song's own details, or what all the songs share.
                const shared = (key: Field) => {
                    const all = new Set(read.map((t) => show(t[key])));
                    return all.size === 1 ? [...all][0] : "";
                };
                setValues(Object.fromEntries(FIELDS.map((f) => [f.key, one || !f.each ? shared(f.key) : ""])) as Record<Field, string>);
            })
            .catch((e) => toast.error("Couldn't read the songs' details", { description: String(e) }));
    }, [files, one]);

    const set = (key: Field, value: string) => {
        setValues((v) => ({ ...v, [key]: value }));
        setTouched((t) => new Set(t).add(key));
    };
    const badNumber = (["track", "trackTotal", "year"] as Field[]).some((k) => touched.has(k) && values[k].trim() && !/^\d+$/.test(values[k].trim()));
    const nothing = !touched.size && !fromNames && !numbered && cover === undefined;

    const start = () => {
        const ordered = [...files].sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
        const edits = ordered.map((f, i) => {
            const edit: Record<string, string | undefined> = { path: f.path };
            for (const key of touched) edit[key] = values[key];
            if (fromNames) {
                const parsed = fromName(f.name);
                edit.title = parsed.title;
                if (parsed.track && !numbered) edit.track = parsed.track;
            }
            if (numbered) {
                edit.track = String(i + 1);
                edit.trackTotal = String(ordered.length);
            }
            if (cover !== undefined) edit.cover = cover ?? "";
            return edit;
        });
        void run.start(files.length, (p) => runAction("song_write", { edits }, p), (r) => `details of ${plural(r.made.length, "song")}`);
    };

    const dirs = new Set(files.map((f) => parentPath(f.path).toLowerCase()));
    return (
        <Modal label="Edit song details" onClose={close} onSubmit={() => !nothing && !badNumber && !run.started && start()} className="max-h-[88vh] max-w-lg">
            <ModalHeader icon={Music} title={one ? files[0].name : `Details of ${formatCount(files.length)} songs`} subtitle={dirs.size === 1 ? parentPath(files[0].path) : `In ${formatCount(dirs.size)} folders`} onClose={close} />
            <div className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4 text-sm">
                {!run.started && !tags && <p className="text-muted-foreground">Reading their details…</p>}
                {!run.started && tags && (
                    <>
                        {tags.some((t) => t.error) && <p className="text-xs text-amber-700 dark:text-amber-400">{plural(tags.filter((t) => t.error).length, "song")} couldn't be read, and won't change.</p>}
                        <div className="grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-1.5">
                            {FIELDS.filter((f) => one || !f.each || (f.key === "track" ? !numbered && !fromNames : f.key === "title" ? !fromNames : true)).map((f) => (
                                <label key={f.key} className="contents">
                                    <span className="text-xs font-semibold text-muted-foreground">{f.label}</span>
                                    <input
                                        value={values[f.key]}
                                        onChange={(e) => set(f.key, e.target.value)}
                                        placeholder={!one && !touched.has(f.key) && !values[f.key] ? "(as each is)" : ""}
                                        inputMode={f.key === "track" || f.key === "trackTotal" || f.key === "year" ? "numeric" : undefined}
                                        dir="auto"
                                        className="h-8 min-w-0 rounded-md border bg-background px-2"
                                    />
                                </label>
                            ))}
                        </div>
                        {!one && (
                            <div className="space-y-1">
                                <label className="flex items-center gap-2">
                                    <input type="checkbox" checked={fromNames} onChange={(e) => setFromNames(e.target.checked)} className="accent-primary" />
                                    <Wand2 className="size-3.5 text-muted-foreground" /> Titles (and track numbers) from the file names, as in “03 - Artist - Title”
                                </label>
                                <label className="flex items-center gap-2">
                                    <input type="checkbox" checked={numbered} onChange={(e) => setNumbered(e.target.checked)} className="accent-primary" /> Number the tracks 1 to {files.length}, in the files' name order
                                </label>
                                {fromNames && <p className="ps-6 text-xs text-muted-foreground">“{files[0].name}” becomes “{fromName(files[0].name).title}”{fromName(files[0].name).track ? `, track ${fromName(files[0].name).track}` : ""}.</p>}
                            </div>
                        )}
                        <div className="flex items-center gap-2">
                            <span className="text-xs font-semibold text-muted-foreground">Cover</span>
                            <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                                {cover === undefined ? (tags.every((t) => t.cover) ? "Each has one" : tags.some((t) => t.cover) ? "Some have one" : "None") : cover === null ? "Taken out" : baseName(cover)}
                            </span>
                            <Button
                                size="sm"
                                variant="outline"
                                onClick={async () => {
                                    const picked = await pickFile({ multiple: false, filters: [{ name: "Pictures", extensions: ["jpg", "jpeg", "png", "gif", "bmp"] }] });
                                    if (typeof picked === "string") setCover(picked);
                                }}
                            >
                                Choose…
                            </Button>
                            <Button size="sm" variant="ghost" onClick={() => setCover(null)}>
                                Take out
                            </Button>
                        </div>
                        <p className="text-xs text-muted-foreground">An empty field takes that detail out. Each song is saved as a copy with its new details that takes its place, so Ctrl+Z puts them all back.</p>
                    </>
                )}
                {run.running && run.progress && <RunProgress progress={run.progress} verb="Saving" />}
                <RunResult report={run.report} error={run.error} noun="song" failedWhat="Couldn't save their details" sizes={false} />
            </div>
            <RunFooter run={run} onClose={close} onStart={start} onStop={() => void invoke("song_cancel")} startLabel="Save" startIcon={Music} disabled={!tags || nothing || badNumber} />
        </Modal>
    );
}
