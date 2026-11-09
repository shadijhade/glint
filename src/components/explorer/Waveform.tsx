import { useEffect, useRef, useState } from "react";

/** Recordings up to this size are decoded for their waveform (a few minutes of sound is a few MB of samples). */
const MAX_BYTES = 80 * 1024 * 1024;
const BARS = 480;

/** Each bar's loudest sample, 0 to 1, from the sound decoded in the page. */
async function peaks(src: string): Promise<Float32Array> {
    const response = await fetch(src);
    const bytes = await response.arrayBuffer();
    const context = new AudioContext();
    try {
        const sound = await context.decodeAudioData(bytes);
        const out = new Float32Array(BARS);
        const step = Math.max(1, Math.floor(sound.length / BARS));
        for (let c = 0; c < Math.min(2, sound.numberOfChannels); c++) {
            const data = sound.getChannelData(c);
            for (let bar = 0; bar < BARS; bar++) {
                let loudest = 0;
                const end = Math.min(data.length, (bar + 1) * step);
                // Every fourth sample is plenty to find a bar's peak.
                for (let i = bar * step; i < end; i += 4) {
                    const v = Math.abs(data[i]);
                    if (v > loudest) loudest = v;
                }
                out[bar] = Math.max(out[bar], loudest);
            }
        }
        const top = out.reduce((m, v) => Math.max(m, v), 0) || 1;
        return out.map((v) => v / top);
    } finally {
        void context.close();
    }
}

/** A recording's waveform under its player: where it's playing shown, and a click plays from there. */
export function Waveform({ src, size, player }: { src: string; size: number; player: React.RefObject<HTMLMediaElement | null> }) {
    const canvas = useRef<HTMLCanvasElement>(null);
    const [bars, setBars] = useState<Float32Array | null>(null);
    const [at, setAt] = useState(0);

    useEffect(() => {
        if (size > MAX_BYTES) return;
        let live = true;
        setBars(null);
        peaks(src).then((p) => live && setBars(p), () => {});
        return () => {
            live = false;
        };
    }, [src, size]);

    useEffect(() => {
        const media = player.current;
        if (!media) return;
        const tick = () => setAt(media.duration ? media.currentTime / media.duration : 0);
        media.addEventListener("timeupdate", tick);
        media.addEventListener("seeked", tick);
        return () => {
            media.removeEventListener("timeupdate", tick);
            media.removeEventListener("seeked", tick);
        };
    }, [player, bars]);

    useEffect(() => {
        const c = canvas.current;
        if (!c || !bars) return;
        const ratio = window.devicePixelRatio || 1;
        const width = c.clientWidth * ratio;
        const height = c.clientHeight * ratio;
        c.width = width;
        c.height = height;
        const g = c.getContext("2d");
        if (!g) return;
        const style = getComputedStyle(c);
        const played = style.getPropertyValue("--wave-played").trim() || "#e5488f";
        const rest = style.getPropertyValue("--wave-rest").trim() || "#8884";
        const w = width / bars.length;
        for (let i = 0; i < bars.length; i++) {
            const h = Math.max(ratio, bars[i] * height * 0.95);
            g.fillStyle = i / bars.length <= at ? played : rest;
            g.fillRect(i * w, (height - h) / 2, Math.max(ratio, w - ratio * 0.5), h);
        }
    }, [bars, at]);

    if (size > MAX_BYTES || !bars) return null;
    return (
        <canvas
            ref={canvas}
            aria-label="Waveform: click to play from there"
            onClick={(e) => {
                const media = player.current;
                if (!media?.duration) return;
                const box = e.currentTarget.getBoundingClientRect();
                media.currentTime = ((e.clientX - box.left) / box.width) * media.duration;
            }}
            className="h-14 w-full max-w-sm cursor-pointer [--wave-played:var(--color-primary)] [--wave-rest:color-mix(in_oklab,var(--color-muted-foreground)_35%,transparent)]"
        />
    );
}
