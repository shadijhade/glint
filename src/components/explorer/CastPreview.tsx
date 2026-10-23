import { useEffect, useRef, useState } from "react";
import { Pause, Play, RotateCcw } from "lucide-react";
import type { FileEntry } from "@/lib/entries";
import { readTextPreview } from "@/lib/fs";
import { type Cast, clock, outputUntil, parseCast } from "@/lib/cast";
import { loadRenderer, type TermLike } from "@/lib/terminal";
import { Button } from "../ui/button";
import { terminalTheme } from "./TerminalPanel";
import { BigIcon, Waiting } from "./PreviewState";

/** The most of a recording read: past it, what it printed after isn't played. */
const MAX_BYTES = 4 << 20;
const SPEEDS = [1, 2, 4];

/**
 * An asciinema recording, played in the terminal pane's own renderer (Ghostty's, or xterm.js): what it
 * printed, when it printed it, long pauses cut short. It opens on its last frame; Play starts it over.
 */
export function CastPreview({ entry }: { entry: FileEntry }) {
    const [state, setState] = useState<{ cast: Cast; truncated: boolean } | { error: string } | null>(null);
    useEffect(() => {
        let stale = false;
        readTextPreview(entry.path, MAX_BYTES).then(
            (read) => {
                if (stale) return;
                const cast = read.binary ? null : parseCast(read.text);
                setState(cast ? { cast, truncated: read.truncated } : { error: "This isn't an asciinema recording Glint can play (version 2 or 3)." });
            },
            (e) => !stale && setState({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified]);
    if (!state) return <Waiting />;
    if ("error" in state) return <BigIcon entry={entry} note={state.error} />;
    return <Player key={entry.path} cast={state.cast} truncated={state.truncated} />;
}

function Player({ cast, truncated }: { cast: Cast; truncated: boolean }) {
    const host = useRef<HTMLDivElement>(null);
    const term = useRef<TermLike | null>(null);
    // Where playing is: seconds in, the next event to print, and when (in page time) it was at `base`.
    const play = useRef({ base: 0, next: 0, wall: 0, frame: 0 });
    const [position, setPosition] = useState(cast.duration);
    const [playing, setPlaying] = useState(false);
    const [speed, setSpeed] = useState(1);
    const [error, setError] = useState<string | null>(null);

    /** Shows the moment `seconds` in: the screen cleared, then everything printed up to it. */
    const seek = (seconds: number) => {
        const t = term.current;
        if (!t) return;
        const { data, count } = outputUntil(cast, seconds);
        t.write("\x1bc" + data);
        play.current.next = count;
        play.current.base = seconds;
        play.current.wall = performance.now();
        setPosition(seconds);
    };

    useEffect(() => {
        let gone = false;
        let made: TermLike | null = null;
        loadRenderer().then(
            (renderer) => {
                const box = host.current;
                if (gone || !box) return;
                const make = (fontSize: number) => {
                    const t = renderer.create({ cols: cast.cols, rows: cast.rows, fontSize, fontFamily: "'Cascadia Mono', 'Cascadia Code', Consolas, monospace", cursorBlink: false, scrollback: 0, theme: terminalTheme() });
                    t.open(box);
                    // A recording takes no keys: they stay the app's.
                    renderer.keys(t, () => true);
                    return t;
                };
                // Opened at 14 px and measured, then made again as big as fits the width: 10 to 18 px (a
                // narrow pane scrolls sideways rather than go smaller).
                made = make(14);
                const drawn = (box.firstElementChild as HTMLElement | null)?.getBoundingClientRect().width ?? 0;
                const fits = drawn > 0 ? Math.max(10, Math.min(18, Math.floor((14 * box.clientWidth) / drawn))) : 14;
                if (fits !== 14) {
                    made.dispose();
                    box.replaceChildren();
                    made = make(fits);
                }
                term.current = made;
                seek(cast.duration);
            },
            (e) => !gone && setError(String(e))
        );
        return () => {
            gone = true;
            cancelAnimationFrame(play.current.frame);
            made?.dispose();
            term.current = null;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [cast]);

    useEffect(() => {
        if (!playing) return;
        const p = play.current;
        p.wall = performance.now();
        const tick = (now: number) => {
            const at = Math.min(cast.duration, p.base + ((now - p.wall) / 1000) * speed);
            let out = "";
            while (p.next < cast.events.length && cast.events[p.next].t <= at) out += cast.events[p.next++].data;
            if (out) term.current?.write(out);
            setPosition(at);
            if (at >= cast.duration) {
                p.base = at;
                setPlaying(false);
                return;
            }
            p.frame = requestAnimationFrame(tick);
        };
        p.frame = requestAnimationFrame(tick);
        return () => {
            cancelAnimationFrame(p.frame);
            // Paused (or the speed changed): it goes on from here.
            p.base = Math.min(cast.duration, p.base + ((performance.now() - p.wall) / 1000) * speed);
        };
    }, [playing, speed, cast]);

    const toggle = () => {
        if (playing) return setPlaying(false);
        // At the end: from the start again.
        if (play.current.base >= cast.duration - 0.01) seek(0);
        setPlaying(true);
    };

    if (error) return <p className="p-4 text-sm text-destructive">{error}</p>;
    return (
        <div className="flex h-full flex-col">
            <div className="min-h-0 flex-1 overflow-auto p-2">
                <div ref={host} className="w-full" />
            </div>
            <div className="flex h-10 shrink-0 items-center gap-2 border-t px-2 text-xs text-muted-foreground">
                <Button variant="ghost" size="icon" className="size-7" onClick={toggle} title={playing ? "Pause" : "Play"} aria-label={playing ? "Pause" : "Play"}>
                    {playing ? <Pause /> : position >= cast.duration - 0.01 ? <RotateCcw /> : <Play />}
                </Button>
                <input
                    type="range"
                    min={0}
                    max={cast.duration || 1}
                    step={0.05}
                    value={position}
                    aria-label="Position"
                    onChange={(e) => {
                        setPlaying(false);
                        seek(Number(e.target.value));
                    }}
                    className="min-w-0 flex-1 accent-[hsl(var(--primary))]"
                />
                <span className="shrink-0 font-mono tabular-nums">
                    {clock(position)} / {clock(cast.duration)}
                </span>
                <select
                    value={speed}
                    onChange={(e) => setSpeed(Number(e.target.value))}
                    aria-label="Speed"
                    className="h-6 shrink-0 rounded border bg-background px-1 text-xs outline-hidden"
                >
                    {SPEEDS.map((s) => (
                        <option key={s} value={s}>
                            {s}×
                        </option>
                    ))}
                </select>
            </div>
            {(cast.title || truncated) && (
                <p className="shrink-0 truncate border-t px-3 py-1.5 text-xs text-muted-foreground">
                    {cast.title}
                    {cast.title && truncated && " · "}
                    {truncated && "Plays its first 4 MB."}
                </p>
            )}
        </div>
    );
}
