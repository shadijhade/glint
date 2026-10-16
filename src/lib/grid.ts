// The grid view's arithmetic, kept free of the app so explorer.check.ts can run it: where keys move,
// how many columns fit, and what the thumbnail service answers.

/**
 * The thumbnail size to ask for, in device pixels, for a square of `css` pixels at a device pixel ratio
 * of `dpr`: what the screen shows one to one, so it's sharp (rounded up to a multiple of 4, so a few
 * sizes cover every text size and scale).
 */
export function thumbStep(css: number, dpr: number) {
    const px = Math.ceil(css * dpr - 0.01);
    return Math.min(1024, Math.max(16, Math.ceil(px / 4) * 4));
}

/** How many tiles at least `tile` pixels wide fit across `width`: one at the least. */
export const gridColumns = (width: number, tile: number) => Math.max(1, Math.floor(width / tile));

/**
 * Where a key moves the cursor in items laid out `cols` to a row (1 for the list), from `at` (-1 when
 * nothing is focused), with `page` rows on screen; null for a key that doesn't move it. As in
 * Explorer: Left and Right step through the items (only in a grid), Up and Down go a row, and Down
 * from above a short last row goes to its last item.
 */
export function gridMove(key: string, at: number, count: number, cols: number, page: number): number | null {
    if (!count) return null;
    const last = count - 1;
    const row = (i: number) => Math.floor(i / cols);
    switch (key) {
        case "ArrowRight":
            return cols > 1 ? Math.min(last, at + 1) : null;
        case "ArrowLeft":
            return cols > 1 ? Math.max(0, at - 1) : null;
        case "ArrowDown":
            if (at < 0) return 0;
            if (at + cols <= last) return at + cols;
            return row(at) < row(last) ? last : at;
        case "ArrowUp":
            if (at < 0) return 0;
            return at - cols >= 0 ? at - cols : at;
        case "PageDown":
            return Math.min(last, Math.max(0, at + page * cols));
        case "PageUp":
            return Math.max(0, at - page * cols);
        case "Home":
            return 0;
        case "End":
            return last;
    }
    return null;
}

/** Where a thumbnail came from: this app's memory, Windows' thumbnail cache, or made from the file. */
export type ThumbSource = "memory" | "cache" | "made";
const SOURCES: ThumbSource[] = ["memory", "cache", "made"];

export interface ThumbAnswer {
    /** The thumbnail as a BMP file, or null when the file has none. */
    bmp: Uint8Array<ArrayBuffer> | null;
    /** For none: for good. Otherwise there may be one later (an online-only file, a handler that timed out). */
    lasting: boolean;
    source: ThumbSource;
    /** Milliseconds the shell spent on it. */
    ms: number;
}

/** Reads an answer from `thumbnail` (layout documented on `Thumb::encode` in thumbs.rs). */
export function decodeThumb(buffer: ArrayBuffer): ThumbAnswer {
    const view = new DataView(buffer);
    const found = view.getUint8(0) === 0 && buffer.byteLength > 8;
    return { bmp: found ? new Uint8Array(buffer, 8) : null, lasting: view.getUint8(2) === 1, source: SOURCES[view.getUint8(1)] ?? "cache", ms: view.getFloat32(4, true) };
}
