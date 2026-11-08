import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** Frames read for a video at a size, kept for the last few videos scrubbed. */
interface Frames {
    count: number;
    width: number;
    height: number;
    seconds: number;
    images: ImageData[];
}
const cache = new Map<string, Promise<Frames | null>>();
const COUNT = 10;

function framesOf(path: string, width: number): Promise<Frames | null> {
    const key = `${path.toLowerCase()}\n${width}`;
    let found = cache.get(key);
    if (!found) {
        found = invoke<ArrayBuffer>("video_frames", { path, count: COUNT, width }).then(
            (buffer) => {
                const view = new DataView(buffer);
                const [count, w, h] = [view.getUint32(0, true), view.getUint32(4, true), view.getUint32(8, true)];
                const seconds = view.getFloat64(12, true);
                const images = Array.from({ length: count }, (_, i) => new ImageData(new Uint8ClampedArray(buffer, 20 + i * w * h * 4, w * h * 4).slice(), w, h));
                return { count, width: w, height: h, seconds, images };
            },
            () => null
        );
        cache.set(key, found);
        while (cache.size > 24) cache.delete(cache.keys().next().value!);
    }
    return found;
}

const clock = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;

/**
 * Over a video's tile in the grid: with the pointer on it, the video at the point the pointer is at
 * across the tile (ten frames, read as the pointer arrives), with where that is in the video below.
 */
export function VideoScrub({ path, box }: { path: string; box: number }) {
    const [frames, setFrames] = useState<Frames | null>(null);
    const [at, setAt] = useState<number | null>(null);
    const canvas = useRef<HTMLCanvasElement>(null);
    const hovering = useRef(false);
    const width = Math.round(box * Math.min(window.devicePixelRatio || 1, 2));
    useEffect(() => {
        if (at === null || !frames || !canvas.current) return;
        const image = frames.images[Math.min(frames.count - 1, Math.floor(at * frames.count))];
        canvas.current.width = image.width;
        canvas.current.height = image.height;
        canvas.current.getContext("2d")?.putImageData(image, 0, 0);
    }, [at, frames]);
    return (
        <div
            className="absolute inset-0 z-[1]"
            onMouseEnter={() => {
                hovering.current = true;
                void framesOf(path, width).then((f) => hovering.current && setFrames(f));
            }}
            onMouseMove={(e) => {
                const rect = e.currentTarget.getBoundingClientRect();
                setAt(Math.max(0, Math.min(0.999, (e.clientX - rect.left) / rect.width)));
            }}
            onMouseLeave={() => {
                hovering.current = false;
                setAt(null);
            }}
        >
            {at !== null && frames && (
                <>
                    <canvas ref={canvas} className="absolute inset-0 m-auto max-h-full max-w-full rounded-sm object-contain" />
                    <div className="absolute inset-x-1 bottom-1 h-0.5 rounded-full bg-black/30">
                        <div className="h-full rounded-full bg-primary" style={{ width: `${at * 100}%` }} />
                    </div>
                    {frames.seconds > 0 && <span className="absolute end-1 top-1 rounded bg-black/55 px-1 text-[0.625rem] tabular-nums text-white">{clock(at * frames.seconds)}</span>}
                </>
            )}
        </div>
    );
}
