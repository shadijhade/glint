import { useEffect, useState } from "react";
import { toast } from "sonner";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Film, Loader2, Play, Scissors, X } from "lucide-react";
import type { FileEntry } from "@/lib/entries";
import { offerActionUndo } from "@/lib/actions";
import { revealInApp } from "@/lib/explorer";
import { mediaCancel, mediaPlayable, mediaRun } from "@/lib/studio";
import { useTool } from "@/lib/tools";
import { ToolNeeded } from "../ToolNeeded";
import { Button } from "../ui/button";

function clock(s: number) {
    const total = Math.max(0, s);
    const m = Math.floor(total / 60);
    const sec = total % 60;
    return `${m}:${sec.toFixed(1).padStart(4, "0")}`;
}

/**
 * Trim under a video or a recording's player: two handles over its length, the player following the one
 * moved; **Save trim** keeps the part between them in a new file beside it (copied as stored, so at once,
 * or re-encoded to start on the very frame), and Ctrl+Z takes it back.
 */
export function TrimBar({ entry, player, video }: { entry: FileEntry; player: React.RefObject<HTMLMediaElement | null>; video: boolean }) {
    const ffmpeg = useTool("ffmpeg");
    const [open, setOpen] = useState(false);
    const [duration, setDuration] = useState(0);
    const [range, setRange] = useState<[number, number]>([0, 0]);
    const [exact, setExact] = useState(false);
    const [saving, setSaving] = useState<number | null>(null);

    useEffect(() => {
        const el = player.current;
        if (!el) return;
        const known = () => {
            if (Number.isFinite(el.duration) && el.duration > 0) {
                setDuration(el.duration);
                setRange((r) => (r[1] === 0 ? [0, el.duration] : r));
            }
        };
        known();
        el.addEventListener("loadedmetadata", known);
        el.addEventListener("durationchange", known);
        return () => {
            el.removeEventListener("loadedmetadata", known);
            el.removeEventListener("durationchange", known);
        };
    }, [player, open]);

    // Playing the part stops at its end.
    useEffect(() => {
        const el = player.current;
        if (!el || !open) return;
        const stopAtEnd = () => {
            if (el.currentTime >= range[1] && !el.paused) el.pause();
        };
        el.addEventListener("timeupdate", stopAtEnd);
        return () => el.removeEventListener("timeupdate", stopAtEnd);
    }, [player, open, range]);

    if (!open)
        return (
            <Button variant="ghost" size="sm" className="h-6 px-2 text-xs" onClick={() => setOpen(true)} title="Cut it to a part, saved as a new file beside it">
                <Scissors className="size-3.5" /> Trim
            </Button>
        );

    const move = (which: 0 | 1, value: number) => {
        setRange((r) => {
            const next: [number, number] = which === 0 ? [Math.min(value, r[1] - 0.1), r[1]] : [r[0], Math.max(value, r[0] + 0.1)];
            if (player.current) player.current.currentTime = next[which];
            return next;
        });
    };

    const save = async () => {
        setSaving(0);
        try {
            const report = await mediaRun([entry.path], { kind: "trim", start: range[0], end: range[1], exact: exact && video }, { kind: "beside", suffix: " (trimmed)" }, (p) => setSaving(p.part));
            if (report.made.length) {
                offerActionUndo(report, `trim of “${entry.name}”`);
                toast.success(`Saved the part, ${clock(range[1] - range[0])} long`, { action: { label: "Show", onClick: () => void revealInApp(report.made[0]) } });
                setOpen(false);
            } else if (report.failed.length) toast.error("Couldn't trim it", { description: report.failed[0][1] });
        } catch (e) {
            if (String(e) !== "cancelled") toast.error("Couldn't trim it", { description: String(e) });
        } finally {
            setSaving(null);
        }
    };

    const left = duration ? (range[0] / duration) * 100 : 0;
    const right = duration ? (range[1] / duration) * 100 : 100;
    return (
        <div className="w-full max-w-xl space-y-2 rounded-lg border bg-background/80 p-3 text-xs">
            {ffmpeg && !ffmpeg.origin && <ToolNeeded id="ffmpeg" why="FFmpeg cuts videos and recordings without re-encoding them." icon={Film} />}
            <div className="relative h-6">
                <div className="absolute inset-x-0 top-1/2 h-1.5 -translate-y-1/2 rounded-full bg-muted" />
                <div className="absolute top-1/2 h-1.5 -translate-y-1/2 rounded-full bg-primary" style={{ insetInlineStart: `${left}%`, width: `${right - left}%` }} />
                {([0, 1] as const).map((which) => (
                    <input
                        key={which}
                        type="range"
                        min={0}
                        max={duration || 1}
                        step={0.05}
                        value={range[which]}
                        onChange={(e) => move(which, Number(e.target.value))}
                        aria-label={which === 0 ? "Start" : "End"}
                        className="trim-handle pointer-events-none absolute inset-0 w-full appearance-none bg-transparent accent-primary"
                    />
                ))}
            </div>
            <div className="flex flex-wrap items-center gap-2">
                <span className="tabular-nums text-muted-foreground">
                    {clock(range[0])} – {clock(range[1])} · keeps {clock(range[1] - range[0])}
                </span>
                <span className="flex-1" />
                {video && (
                    <label className="flex items-center gap-1.5" title="Re-encodes the part, so it starts on the very frame (slower); otherwise it's copied as stored and starts at the keyframe at or before the start">
                        <input type="checkbox" checked={exact} onChange={(e) => setExact(e.target.checked)} className="accent-primary" /> Exact
                    </label>
                )}
                <Button
                    variant="ghost"
                    size="sm"
                    className="h-6 px-2 text-xs"
                    onClick={() => {
                        if (!player.current) return;
                        player.current.currentTime = range[0];
                        void player.current.play();
                    }}
                >
                    <Play className="size-3.5" /> Play the part
                </Button>
                {saving !== null ? (
                    <Button variant="outline" size="sm" className="h-6 px-2 text-xs" onClick={() => void mediaCancel()}>
                        <Loader2 className="size-3.5 animate-spin" /> {Math.round(saving * 100)}% · Stop
                    </Button>
                ) : (
                    <Button size="sm" className="h-6 px-2 text-xs" disabled={!ffmpeg?.origin || !duration} onClick={() => void save()}>
                        <Scissors className="size-3.5" /> Save trim
                    </Button>
                )}
                <Button variant="ghost" size="icon" className="size-6" onClick={() => setOpen(false)} aria-label="Close trim">
                    <X className="size-3.5" />
                </Button>
            </div>
        </div>
    );
}

/**
 * For a video WebView2 can't play (an AVI, a WMV, an MKV of codecs it lacks): FFmpeg makes a copy it can,
 * remuxed as stored when only the container is the problem (seconds), converted otherwise; the copy is
 * kept for next time. Hands the copy's address to `onReady`.
 */
export function CantPlay({ entry, onReady }: { entry: FileEntry; onReady: (src: string) => void }) {
    const ffmpeg = useTool("ffmpeg");
    const [part, setPart] = useState<number | null>(null);
    const [error, setError] = useState<string | null>(null);
    const ready = !!ffmpeg?.origin;

    const make = async () => {
        setError(null);
        setPart(0);
        try {
            const copy = await mediaPlayable(entry.path, setPart);
            onReady(convertFileSrc(copy));
        } catch (e) {
            if (String(e) !== "cancelled") setError(String(e));
        } finally {
            setPart(null);
        }
    };

    // FFmpeg there: start at once for a video of a reasonable size (remuxing usually takes a moment); a
    // big one waits for a click, so selecting it doesn't set the PC converting.
    const automatic = entry.size < 512 * 1024 * 1024;
    useEffect(() => {
        if (ready && automatic) void make();
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [ready, entry.path]);

    return (
        <div className="flex max-w-md flex-col items-center gap-3 text-center text-sm">
            <Film className="size-8 text-muted-foreground" />
            <p className="font-medium">This video doesn't play here as it is</p>
            {ffmpeg && !ffmpeg.origin && <ToolNeeded id="ffmpeg" why="FFmpeg makes a copy that plays here, usually in seconds." icon={Film} />}
            {part !== null && (
                <div className="w-64 space-y-1">
                    <div className="flex items-center justify-center gap-2 text-xs text-muted-foreground">
                        <Loader2 className="size-3.5 animate-spin" /> Making a copy that plays · {Math.round(part * 100)}%
                    </div>
                    <div className="h-1.5 overflow-hidden rounded-full bg-muted">
                        <div className="h-full rounded-full bg-primary transition-[width]" style={{ width: `${Math.round(part * 100)}%` }} />
                    </div>
                    <Button variant="ghost" size="sm" className="h-6 text-xs" onClick={() => void mediaCancel()}>
                        Stop
                    </Button>
                </div>
            )}
            {ready && !automatic && part === null && !error && (
                <Button size="sm" onClick={() => void make()}>
                    <Film /> Make a copy that plays
                </Button>
            )}
            {error && <p className="text-xs text-destructive">{error}</p>}
            {error && ready && (
                <Button size="sm" variant="outline" onClick={() => void make()}>
                    Try again
                </Button>
            )}
        </div>
    );
}
