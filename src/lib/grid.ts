// The grid view's arithmetic, kept free of the app so explorer.check.ts can run it: its tile sizes,
// where keys move, how many columns fit, where it scrolls as they change, and what the thumbnail
// service answers.

/** The grid's tile sizes, smallest first: the square each thumbnail fits in, in rem (it scales with the text size). */
export const TILE_SIZES = [
    { size: "small", label: "Small", rem: 4 },
    { size: "medium", label: "Medium", rem: 6 },
    { size: "large", label: "Large", rem: 8 },
    { size: "xlarge", label: "Extra large", rem: 12 },
    { size: "huge", label: "Huge", rem: 16 },
] as const;

export type TileSize = (typeof TILE_SIZES)[number]["size"];

/** Where a size is in TILE_SIZES; medium for one this version doesn't know (a setting saved by a newer one). */
function tileIndex(size: string) {
    const at = TILE_SIZES.findIndex((t) => t.size === size);
    return at < 0 ? 1 : at;
}

/** The size `steps` up from `size` (down when negative), as far as the smallest and the biggest. */
export const zoomTiles = (size: string, steps: number): TileSize => TILE_SIZES[Math.max(0, Math.min(TILE_SIZES.length - 1, tileIndex(size) + steps))].size;

/**
 * A grid tile at `size`, in CSS pixels for a rem of `rem`: the narrowest it gets (tiles widen to fill
 * the row), its height, the square its thumbnail or icon fits in, and the grid's side padding. Its
 * name, size and margins take the same room at every size.
 */
export function tileLayout(rem: number, size: string) {
    const thumb = rem * TILE_SIZES[tileIndex(size)].rem;
    return { width: Math.round(thumb + rem * 1.5), height: Math.round(thumb + rem * 4.25), thumb, pad: rem * 0.5 };
}

export type GridTile = ReturnType<typeof tileLayout>;

/** How a grid is laid out: tiles to a row, and each row's height. */
export interface GridLayout {
    cols: number;
    rowHeight: number;
}

/**
 * Where a grid is scrolled to, as a place that outlasts its layout: the first tile of the row at the
 * top, plus how far into that row it's scrolled. A tab keeps this for when it's shown again.
 */
export function placeOf(top: number, at: GridLayout) {
    const row = Math.floor(top / at.rowHeight);
    return row * at.cols + top / at.rowHeight - row;
}

/** The scroll position of `place` (see `placeOf`) in a grid laid out `at`. */
export function topOf(place: number, at: GridLayout) {
    const first = Math.floor(place);
    return (Math.floor(first / at.cols) + place - first) * at.rowHeight;
}

/**
 * Where a grid scrolls to when its layout changes under it (its tiles zoom, its pane narrows), so it
 * keeps its place: tile `keep` (the focused one) stays about where it was if any of it was on screen,
 * whole if it fits; otherwise the tiles at the top stay at the top. `top` and `height` are where the
 * view was scrolled to and how tall it is, `rows` how many rows there are now.
 */
export function keepPlace(was: GridLayout, now: GridLayout, top: number, height: number, rows: number, keep: number): number {
    const row = keep >= 0 ? Math.floor(keep / was.cols) : -1;
    const y = row * was.rowHeight - top;
    let next: number;
    if (row >= 0 && y < height && y + was.rowHeight > 0) {
        // Its middle stays put, as far as the whole tile then fits.
        const at = Math.min(Math.max(0, y + (was.rowHeight - now.rowHeight) / 2), Math.max(0, height - now.rowHeight));
        next = Math.floor(keep / now.cols) * now.rowHeight - at;
    } else next = topOf(placeOf(top, was), now);
    return Math.max(0, Math.min(rows * now.rowHeight - height, next));
}

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
