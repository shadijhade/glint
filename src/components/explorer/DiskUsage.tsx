import { Component, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { AlertTriangle, BrushCleaning, Flame, Layers, Loader2, Sparkles, Square, Zap } from "lucide-react";
import { entriesOf, mapUsage, markUsageShown, refreshUsage, setUsage, stopUsage, type UsageRun, useUsage } from "@/lib/diskUsage";
import type { FileEntry } from "@/lib/entries";
import { enableIndex, goUp, modalOpen, navigate, openEntry, selectOnly, tabName, updateTab, useExplorer } from "@/lib/explorer";
import { TYPE_GROUPS, type TypeGroup, typeGroup, typeLabel } from "@/lib/fileTypes";
import { formatCount, formatDate, formatDuration, formatSize } from "@/lib/format";
import { parentPath, samePath } from "@/lib/paths";
import { BLOCK, FRAME, hitTest, isFolder, layoutUsage, type UsageLayout, type UsageMap } from "@/lib/usage";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { openCleanup } from "./CleanupDialog";

type ColorMode = "type" | "age";

const DAY_MS = 86_400_000;

/** Perceptual thermal spectrum from Hot (modified in the last 24h) to Frozen (untouched for >1 year). */
const AGE_BANDS: (TypeGroup & { maxAgeMs: number })[] = [
    { name: "Hot (<24h)", hue: 8, sat: 84, maxAgeMs: DAY_MS },
    { name: "Recent (≤7d)", hue: 26, sat: 82, maxAgeMs: 7 * DAY_MS },
    { name: "Warm (≤30d)", hue: 44, sat: 76, maxAgeMs: 30 * DAY_MS },
    { name: "Cool (≤6m)", hue: 172, sat: 54, maxAgeMs: 180 * DAY_MS },
    { name: "Cold (≤1y)", hue: 204, sat: 58, maxAgeMs: 365 * DAY_MS },
    { name: "Frozen (>1y)", hue: 226, sat: 42, maxAgeMs: Infinity },
];

function ageBand(modified: number, now: number): TypeGroup {
    if (!modified || modified <= 0) return AGE_BANDS[AGE_BANDS.length - 1];
    const age = Math.max(0, now - modified);
    for (const band of AGE_BANDS) {
        if (age <= band.maxAgeMs) return band;
    }
    return AGE_BANDS[AGE_BANDS.length - 1];
}

interface Theme {
    background: string;
    /** Frame fill by depth (1 = right inside the folder mapped). */
    frames: string[];
    text: string;
    muted: string;
    /** Folders drawn whole (too deep or too small to show inside). */
    folder: string;
    /** Lightness of file tiles, %. */
    lightness: number;
    /** Text on tiles. */
    label: string;
    labelMuted: string;
    hover: string;
    hoverFill: string;
    selected: string;
    /** Smaller items: stripes over this. */
    rest: string;
    stripe: string;
    /** The light and the shadow of each tile's cushion. */
    shine: string;
    shade: string;
}

const THEMES: Record<"dark" | "light", Theme> = {
    dark: {
        background: "hsl(240 6% 9%)",
        frames: ["hsl(240 5% 15%)", "hsl(240 5% 19%)", "hsl(240 5% 23%)", "hsl(240 5% 27%)"],
        text: "hsl(0 0% 92%)",
        muted: "hsl(240 5% 64%)",
        folder: "hsl(38 16% 33%)",
        lightness: 43,
        label: "rgba(255 255 255 / 0.94)",
        labelMuted: "rgba(255 255 255 / 0.7)",
        hover: "rgba(255 255 255 / 0.95)",
        hoverFill: "rgba(255 255 255 / 0.14)",
        selected: "hsl(330 81% 62%)",
        rest: "hsl(240 4% 24%)",
        stripe: "hsl(240 4% 30%)",
        shine: "rgba(255 255 255 / 0.2)",
        shade: "rgba(0 0 0 / 0.28)",
    },
    light: {
        background: "hsl(0 0% 100%)",
        frames: ["hsl(240 6% 90%)", "hsl(240 6% 85%)", "hsl(240 6% 80%)", "hsl(240 6% 75%)"],
        text: "hsl(240 10% 8%)",
        muted: "hsl(240 4% 38%)",
        folder: "hsl(38 24% 70%)",
        lightness: 63,
        label: "rgba(0 0 0 / 0.86)",
        labelMuted: "rgba(0 0 0 / 0.6)",
        hover: "rgba(0 0 0 / 0.85)",
        hoverFill: "rgba(255 255 255 / 0.3)",
        selected: "hsl(330 81% 52%)",
        rest: "hsl(240 5% 86%)",
        stripe: "hsl(240 5% 78%)",
        shine: "rgba(255 255 255 / 0.35)",
        shade: "rgba(0 0 0 / 0.16)",
    },
};

/** Files of a type nobody listed still get a color of their own, muted, from their extension. */
function extensionHue(ext: string) {
    let hash = 0;
    for (let i = 0; i < ext.length; i++) hash = (hash * 31 + ext.charCodeAt(i)) >>> 0;
    return hash % 360;
}

const tileColor = (entry: FileEntry, group: TypeGroup, theme: Theme) =>
    group === TYPE_GROUPS.other && entry.ext ? `hsl(${extensionHue(entry.ext)} 22% ${theme.lightness}%)` : `hsl(${group.hue} ${group.sat}% ${theme.lightness}%)`;

/** Follows the theme class on <html> (set by useTheme after this component's effects run). */
function useDark() {
    const [dark, setDark] = useState(() => document.documentElement.classList.contains("dark"));
    useEffect(() => {
        const observer = new MutationObserver(() => setDark(document.documentElement.classList.contains("dark")));
        observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
        return () => observer.disconnect();
    }, []);
    return dark;
}

/** Cuts `text` to `max` pixels with an ellipsis ("" if not even that fits). */
function fit(ctx: CanvasRenderingContext2D, text: string, max: number) {
    if (max <= 0) return "";
    const width = ctx.measureText(text).width;
    if (width <= max) return text;
    let cut = Math.floor((text.length * max) / width);
    while (cut > 0 && ctx.measureText(text.slice(0, cut) + "…").width > max) cut--;
    return cut > 0 ? text.slice(0, cut) + "…" : "";
}

let font = "";
/** The app's font, read once its styles are in. */
const fontFamily = () => (font ||= getComputedStyle(document.documentElement).fontFamily || "system-ui, sans-serif");

/** Paints the map: frames, then tiles a color at a time, then their labels. */
function paint(canvas: HTMLCanvasElement, map: UsageMap, layout: UsageLayout, fills: string[], theme: Theme, dpr: number) {
    canvas.width = Math.round(layout.width * dpr);
    canvas.height = Math.round(layout.height * dpr);
    const ctx = canvas.getContext("2d", { alpha: false })!;
    ctx.fillStyle = theme.background;
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    const px = (v: number) => Math.round(v * dpr);
    // Device-pixel boxes, with a one-pixel gap on the right and bottom so neighbours stay apart.
    const box = (x: number, y: number, w: number, h: number) => {
        const [x0, y0] = [px(x), px(y)];
        const [bw, bh] = [px(x + w) - x0, px(y + h) - y0];
        const gap = bw > 3 && bh > 3 ? 1 : 0;
        return [x0, y0, bw - gap, bh - gap] as const;
    };

    const stripes = document.createElement("canvas");
    stripes.width = stripes.height = Math.max(4, Math.round(6 * dpr));
    const s = stripes.getContext("2d")!;
    s.fillStyle = theme.rest;
    s.fillRect(0, 0, stripes.width, stripes.height);
    s.strokeStyle = theme.stripe;
    s.lineWidth = Math.max(1, dpr);
    s.beginPath();
    s.moveTo(0, stripes.height);
    s.lineTo(stripes.width, 0);
    s.stroke();
    const pattern = ctx.createPattern(stripes, "repeat")!;

    const tiles = new Map<string, number[]>();
    const labels: number[] = [];
    for (const i of layout.order) {
        if (layout.kind[i] === FRAME) {
            if (i !== 0) {
                ctx.fillStyle = theme.frames[Math.min(layout.depth[i], theme.frames.length) - 1];
                ctx.fillRect(...box(layout.x[i], layout.y[i], layout.w[i], layout.h[i]));
            }
            if (layout.restW[i] > 0) {
                ctx.fillStyle = pattern;
                ctx.fillRect(...box(layout.restX[i], layout.restY[i], layout.restW[i], layout.restH[i]));
            }
            if (layout.header[i] > 0) labels.push(i);
        } else {
            const fill = fills[i];
            const list = tiles.get(fill);
            if (list) list.push(i);
            else tiles.set(fill, [i]);
            if (layout.w[i] >= 44 && layout.h[i] >= 16) labels.push(i);
        }
    }
    for (const [fill, list] of tiles) {
        ctx.fillStyle = fill;
        for (const i of list) ctx.fillRect(...box(layout.x[i], layout.y[i], layout.w[i], layout.h[i]));
    }
    // A soft cushion on every tile big enough to see it, lit from the top left: one gradient in a unit
    // square, stretched onto each tile.
    const cushion = ctx.createLinearGradient(0, 0, 1, 1);
    cushion.addColorStop(0, theme.shine);
    cushion.addColorStop(0.45, "rgba(0 0 0 / 0)");
    cushion.addColorStop(1, theme.shade);
    ctx.fillStyle = cushion;
    for (const list of tiles.values()) {
        for (const i of list) {
            const [x, y, w, h] = box(layout.x[i], layout.y[i], layout.w[i], layout.h[i]);
            if (w < 6 || h < 6) continue;
            ctx.setTransform(w, 0, 0, h, x, y);
            ctx.fillRect(0, 0, 1, 1);
        }
    }
    ctx.setTransform(1, 0, 0, 1, 0, 0);

    ctx.textBaseline = "middle";
    const bold = `600 ${11 * dpr}px ${fontFamily()}`;
    const small = `${10.5 * dpr}px ${fontFamily()}`;
    for (const i of labels) {
        const [x, y, w, h] = [px(layout.x[i]), px(layout.y[i]), px(layout.w[i]), px(layout.h[i])];
        const size = formatSize(map.size[i]);
        const pad = Math.round(5 * dpr);
        if (layout.kind[i] === FRAME) {
            const mid = y + Math.round((layout.header[i] * dpr) / 2);
            ctx.font = small;
            const sizeWidth = ctx.measureText(size).width;
            ctx.font = bold;
            // The size goes after the name when both fit; otherwise the name alone.
            const withSize = w - 3 * pad - sizeWidth;
            const name = fit(ctx, map.names[i], withSize >= 30 * dpr ? withSize : w - 2 * pad);
            ctx.fillStyle = theme.text;
            ctx.fillText(name, x + pad, mid);
            if (withSize >= 30 * dpr) {
                const after = x + pad + ctx.measureText(name).width + pad;
                ctx.font = small;
                ctx.fillStyle = theme.muted;
                ctx.fillText(size, after, mid);
            }
        } else {
            const top = y + Math.round(9 * dpr);
            ctx.font = bold;
            ctx.fillStyle = theme.label;
            ctx.fillText(fit(ctx, map.names[i], w - 2 * pad), x + pad, top);
            if (h >= 30 * dpr) {
                ctx.font = small;
                ctx.fillStyle = theme.labelMuted;
                ctx.fillText(fit(ctx, size, w - 2 * pad), x + pad, top + Math.round(13 * dpr));
            }
        }
    }
}

/** Outlines the selected items and the one under the pointer. */
function paintOverlay(canvas: HTMLCanvasElement, layout: UsageLayout, selected: number[], hover: Hover | null, theme: Theme, dpr: number) {
    const [width, height] = [Math.round(layout.width * dpr), Math.round(layout.height * dpr)];
    const ctx = canvas.getContext("2d")!;
    if (canvas.width !== width || canvas.height !== height) [canvas.width, canvas.height] = [width, height];
    else ctx.clearRect(0, 0, width, height);
    const outline = (x: number, y: number, w: number, h: number, width: number) => {
        const inset = width / 2;
        ctx.lineWidth = width;
        ctx.strokeRect(Math.round(x * dpr) + inset, Math.round(y * dpr) + inset, Math.max(0, Math.round(w * dpr) - 2 * inset - 1), Math.max(0, Math.round(h * dpr) - 2 * inset - 1));
    };
    ctx.strokeStyle = theme.selected;
    for (const i of selected) if (layout.w[i] > 0) outline(layout.x[i], layout.y[i], layout.w[i], layout.h[i], Math.round(2 * dpr));
    if (!hover || (hover.node === 0 && !hover.rest)) return;
    const i = hover.node;
    const [x, y, w, h] = hover.rest ? [layout.restX[i], layout.restY[i], layout.restW[i], layout.restH[i]] : [layout.x[i], layout.y[i], layout.w[i], layout.h[i]];
    ctx.fillStyle = theme.hoverFill;
    ctx.fillRect(Math.round(x * dpr), Math.round(y * dpr), Math.round(w * dpr), Math.round(h * dpr));
    ctx.strokeStyle = theme.hover;
    outline(x, y, w, h, Math.max(1, Math.round(1.5 * dpr)));
}

interface Hover {
    node: number;
    rest: boolean;
    /** Pointer position in the map, CSS pixels. */
    x: number;
    y: number;
}

function Centered({ children }: { children: React.ReactNode }) {
    return <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-sm text-muted-foreground">{children}</div>;
}

/** Keeps a failure drawing a map from blanking the whole window; the next folder gets a fresh try. */
class MapBoundary extends Component<{ tab: string; folder: string; children: React.ReactNode }, { error: string }> {
    state = { error: "" };
    static getDerivedStateFromError(e: unknown) {
        return { error: String(e) };
    }
    componentDidUpdate(previous: { folder: string }) {
        if (this.state.error && previous.folder !== this.props.folder) this.setState({ error: "" });
    }
    render() {
        if (!this.state.error) return this.props.children;
        return (
            <Centered>
                <AlertTriangle className="size-6 text-destructive" />
                <p className="font-medium text-foreground">Couldn't draw this map</p>
                <p className="max-w-md select-text">{this.state.error}</p>
                <Button variant="outline" size="sm" onClick={() => setUsage(this.props.tab, false)}>
                    Show the file list
                </Button>
            </Centered>
        );
    }
}

/** Seconds since `since`, ticking, for a dust run that can take a while. */
function Elapsed({ since }: { since: number }) {
    const [now, setNow] = useState(performance.now());
    useEffect(() => {
        const timer = setInterval(() => setNow(performance.now()), 500);
        return () => clearInterval(timer);
    }, []);
    return <span className="tabular-nums">{Math.floor((now - since) / 1000)} s</span>;
}

interface DiskUsageProps {
    tab: string;
    /** Right-click on an item: the pane's item menu for row `row` of the map's rows (node `row + 1`). */
    onItemContextMenu: (e: React.MouseEvent, row: number) => void;
    /** Right-click on the folder mapped itself, where none of its items is. */
    onFolderContextMenu: (e: React.MouseEvent) => void;
}

/**
 * A squarified treemap of what fills a tab's folder, like WizTree or WinDirStat: folders nest a few
 * levels deep, files are colored by type. Hover for details, click to go in, right-click for the menu.
 * Its rows (see `useMapRows`) are the pane's, so the pane's keys, menus and Quick Look work on it.
 */
export function DiskUsage(props: DiskUsageProps) {
    const run = useUsage((s) => s[props.tab]);
    if (!run) return null;
    return (
        <MapBoundary tab={props.tab} folder={run.path}>
            <UsageMapView {...props} run={run} />
        </MapBoundary>
    );
}

const NO_ENTRIES: FileEntry[] = [];
const NO_SELECTION: ReadonlySet<string> = new Set();
/** Keys with no meaning on a map (no order to move through, no names to type to). */
const MOVES = new Set(["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "PageUp", "PageDown", "Home", "End"]);

function UsageMapView({ tab, run, onItemContextMenu, onFolderContextMenu }: DiskUsageProps & { run: UsageRun }) {
    const boxRef = useRef<HTMLDivElement>(null);
    const mapRef = useRef<HTMLCanvasElement>(null);
    const overlayRef = useRef<HTMLCanvasElement>(null);
    const tipRef = useRef<HTMLDivElement>(null);
    const [size, setSize] = useState({ w: 0, h: 0, dpr: devicePixelRatio });
    /** Where the pointer is over the map, in CSS pixels. */
    const [pointer, setPointer] = useState<{ x: number; y: number } | null>(null);
    const [colorMode, setColorMode] = useState<ColorMode>("type");
    const picked = useRef<string | null>(null);
    const dark = useDark();
    const theme = THEMES[dark ? "dark" : "light"];
    const drives = useExplorer((s) => s.drives);
    const selection = useExplorer((s) => s.tabs[tab]?.selection ?? NO_SELECTION);
    const renaming = useExplorer((s) => s.tabs[tab]?.renaming ?? null);
    // The pane the keyboard acts on shows this map.
    const keys = useExplorer((s) => s.panes[s.activePane]?.active === tab);

    const map = run.map;
    const entries = map ? entriesOf(map) : NO_ENTRIES;
    // The map on screen is this folder's, or (dimmed, while the next one comes) the last one.
    const current = !!map && samePath(map.path, run.path);

    // Keys only a map has (the pane handles the rest, on the map's rows): Backspace goes back out, as
    // Alt+Up does; F5 maps again; T switches between file-type and thermal-age coloring.
    useEffect(() => {
        if (!keys) return;
        const onKey = (e: KeyboardEvent) => {
            const target = e.target instanceof Element ? e.target : null;
            if (e.defaultPrevented || modalOpen() || !boxRef.current?.offsetParent || target?.closest("input, textarea, select, [contenteditable=true], dialog, [popover], [data-terminal]")) return;
            const ctrl = e.ctrlKey || e.metaKey;
            const plain = !ctrl && !e.altKey;
            if (e.key === "Backspace" && plain) goUp();
            else if (e.key === "F5" && plain) refreshUsage(tab);
            else if (plain && e.key.toLowerCase() === "t") setColorMode((m) => (m === "type" ? "age" : "type"));
            else if (MOVES.has(e.key) && !e.altKey) {
                // nothing to move to
            } else if (ctrl && !e.altKey && (e.key.toLowerCase() === "a" || e.key === " ")) {
                // nothing to select all of, or one by one
            } else if (!(plain && e.key.length === 1 && e.key !== " ")) return;
            e.preventDefault();
        };
        // Before the pane's own handler, which skips a key once it's handled.
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [keys, tab]);

    // Names are edited in the list: Rename (F2 or the menu) hands over to it, in the item's own folder.
    useEffect(() => {
        if (!renaming) return;
        setUsage(tab, false);
        const folder = parentPath(renaming);
        if (samePath(folder, run.path)) return;
        navigate(folder, { select: renaming, tab }).then((done) => done && updateTab({ renaming, selection: new Set([renaming]), focus: renaming, anchor: renaming }, tab));
    }, [renaming, tab, run.path]);

    useLayoutEffect(() => {
        const el = boxRef.current!;
        const measure = () => {
            const { width, height } = el.getBoundingClientRect();
            setSize((s) => (s.w === width && s.h === height && s.dpr === devicePixelRatio ? s : { w: width, h: height, dpr: devicePixelRatio }));
        };
        measure();
        const observer = new ResizeObserver(measure);
        observer.observe(el);
        // Moving to a screen with another scale changes the pixel ratio but not the size.
        window.addEventListener("resize", measure);
        return () => {
            observer.disconnect();
            window.removeEventListener("resize", measure);
        };
    }, []);

    const layout = useMemo(() => (map && size.w > 0 && size.h > 0 ? layoutUsage(map, size.w, size.h) : null), [map, size.w, size.h]);
    const now = useMemo(() => Date.now(), [map]);
    const groups = useMemo(
        () => entries.map((e) => (e.dir ? null : colorMode === "age" ? ageBand(e.modified, now) : typeGroup(e))),
        [entries, colorMode, now]
    );
    const fills = useMemo(
        () =>
            entries.map((e, i) =>
                e.dir
                    ? theme.folder
                    : colorMode === "age"
                      ? `hsl(${groups[i]!.hue} ${groups[i]!.sat}% ${theme.lightness}%)`
                      : tileColor(e, groups[i]!, theme)
            ),
        [entries, groups, theme, colorMode]
    );
    const byPath = useMemo(() => new Map(map ? map.paths.map((p, i) => [p, i]) : []), [map]);
    // What's under the pointer, worked out again when the map changes beneath a pointer that stays put.
    const hover: Hover | null = useMemo(() => {
        if (!pointer || !map || !layout || !current || entries.length !== map.count) return null;
        const hit = hitTest(map, layout, pointer.x, pointer.y);
        return hit && { ...hit, ...pointer };
    }, [pointer, map, layout, current, entries.length]);

    useLayoutEffect(() => {
        if (!map || !layout || entries.length !== map.count || !mapRef.current) return;
        paint(mapRef.current, map, layout, fills, theme, size.dpr);
        // Timed when this folder's own map is drawn (not a part of an earlier one shown meanwhile).
        if (current && !run.running) markUsageShown(tab, run.id);
    }, [map, layout, fills, theme, size.dpr, entries.length, current, run.running, run.id, tab]);

    const selected = useMemo(() => [...selection].flatMap((p) => byPath.get(p) ?? []), [selection, byPath]);
    useLayoutEffect(() => {
        if (layout && overlayRef.current) paintOverlay(overlayRef.current, layout, selected, hover, theme, size.dpr);
    }, [layout, selected, hover, theme, size.dpr]);

    // The tooltip stays inside the map, flipping to the other side of the pointer near an edge.
    useLayoutEffect(() => {
        const tip = tipRef.current;
        if (!tip || !hover) return;
        const { width, height } = tip.getBoundingClientRect();
        const left = hover.x + 16 + width > size.w ? Math.max(4, hover.x - 12 - width) : hover.x + 16;
        const top = hover.y + 18 + height > size.h ? Math.max(4, hover.y - 10 - height) : hover.y + 18;
        tip.style.transform = `translate(${left}px, ${top}px)`;
    }, [hover, size.w, size.h]);

    const at = (e: React.MouseEvent) => {
        const rect = boxRef.current!.getBoundingClientRect();
        const [x, y] = [e.clientX - rect.left, e.clientY - rect.top];
        const hit = map && layout && current ? hitTest(map, layout, x, y) : null;
        return hit && { ...hit, x, y };
    };

    const onMove = (e: React.MouseEvent) => {
        const rect = boxRef.current!.getBoundingClientRect();
        setPointer({ x: e.clientX - rect.left, y: e.clientY - rect.top });
    };

    // Click: a folder (or its smaller items) is mapped; a file is selected, in its own folder if it's deeper.
    const onClick = (e: React.MouseEvent) => {
        const hit = at(e);
        if (!hit || !map || e.button !== 0) return;
        picked.current = null;
        const { node, rest } = hit;
        if (node === 0) return selectOnly(null);
        if (rest || isFolder(map, node)) return navigate(map.paths[node], { tab });
        const folder = map.parent[node];
        if (folder === 0) {
            picked.current = map.paths[node];
            return selectOnly(map.paths[node]);
        }
        navigate(map.paths[folder], { select: map.paths[node], tab });
    };

    // Double-click opens a file the first click selected (not one that a click took you to).
    const onDoubleClick = (e: React.MouseEvent) => {
        const hit = at(e);
        if (hit && map && !hit.rest && picked.current === map.paths[hit.node]) openEntry(entries[hit.node]);
    };

    const title = tabName(run.path, drives);

    // Bytes per kind (or thermal age band) among the files shown: the legend.
    const legend = useMemo(() => {
        if (!map) return [];
        const totals = new Map<TypeGroup, number>();
        groups.forEach((g, i) => g && totals.set(g, (totals.get(g) ?? 0) + map.size[i]));
        if (colorMode === "age") {
            return AGE_BANDS.filter((b) => totals.has(b)).map((b) => [b, totals.get(b)!] as [TypeGroup, number]);
        }
        return [...totals].sort((a, b) => b[1] - a[1]).slice(0, 6);
    }, [map, groups, colorMode]);
    const wholeFolders = useMemo(() => !!map && !!layout && layout.order.some((i) => layout.kind[i] === BLOCK && isFolder(map, i)), [map, layout]);

    let body: React.ReactNode = null;
    if (!current && run.running && (run.engine === "dust" || !map)) {
        body = (
            <Centered>
                <Loader2 className="size-5 animate-spin text-primary" />
                {run.engine !== "dust" ? (
                    <p>Mapping “{title}”…</p>
                ) : (
                    <>
                        <p className="text-foreground">
                            Measuring “{title}” with dust… <Elapsed since={run.started} />
                        </p>
                        <p className="max-w-sm text-xs">Big folders take a while. With instant search on (in Settings), folders on indexed drives are mapped in milliseconds.</p>
                        <Button variant="outline" size="sm" className="mt-1" onClick={() => stopUsage(tab)}>
                            <Square className="size-3! fill-current" /> Stop
                        </Button>
                    </>
                )}
            </Centered>
        );
    } else if (!current && run.error) {
        body = (
            <Centered>
                <AlertTriangle className="size-6 text-destructive" />
                <p className="font-medium text-foreground">Couldn't map “{title}”</p>
                <p className="max-w-md select-text">{run.error}</p>
                <Button variant="outline" size="sm" onClick={() => mapUsage(tab, run.path)}>
                    Try again
                </Button>
            </Centered>
        );
    } else if (!current && !run.running) {
        body = (
            <Centered>
                <p>Measuring stopped.</p>
                <Button variant="outline" size="sm" onClick={() => mapUsage(tab, run.path)}>
                    Measure again
                </Button>
            </Centered>
        );
    } else if (map && current && map.count <= 1) {
        body = <Centered>{map.size[0] ? "Nothing in this folder is big enough to show." : "This folder is empty."}</Centered>;
    }

    const tip = hover && map && current ? <Tooltip map={map} entry={entries[hover.node]} node={hover.node} rest={hover.rest} /> : null;

    return (
        <div className="flex h-full min-h-0 flex-col">
            <div className="flex h-8 shrink-0 items-center gap-3 overflow-hidden border-b px-3 text-xs">
                <span className="flex shrink-0 items-center gap-1.5 font-medium text-foreground">
                    {title}
                    {map && current && <span className="font-normal text-muted-foreground tabular-nums">{formatSize(map.size[0])}</span>}
                    {run.running && current && <Loader2 className="size-3 animate-spin text-primary" />}
                </span>
                <div role="group" aria-label="Color map by" className="flex shrink-0 rounded-md bg-foreground/[0.05] p-0.5">
                    <button
                        type="button"
                        aria-pressed={colorMode === "type"}
                        onClick={() => setColorMode("type")}
                        title="Color tiles by file type (T)"
                        className={cn(
                            "flex h-5 items-center gap-1 rounded-[4px] px-1.5 text-[0.6875rem] font-medium text-muted-foreground transition-colors hover:text-foreground",
                            colorMode === "type" && "bg-background text-primary shadow-xs dark:bg-foreground/[0.12]"
                        )}
                    >
                        <Layers className="size-3" /> Type
                    </button>
                    <button
                        type="button"
                        aria-pressed={colorMode === "age"}
                        onClick={() => setColorMode("age")}
                        title="Color tiles by when they last changed, from today (hot) to over a year ago (frozen) (T)"
                        className={cn(
                            "flex h-5 items-center gap-1 rounded-[4px] px-1.5 text-[0.6875rem] font-medium text-muted-foreground transition-colors hover:text-foreground",
                            colorMode === "age" && "bg-background text-primary shadow-xs dark:bg-foreground/[0.12]"
                        )}
                    >
                        <Flame className="size-3" /> Age
                    </button>
                </div>
                {map && current && (map.engine === "index" || map.engine === "dust") && (
                    <button
                        type="button"
                        onClick={() => openCleanup(run.path)}
                        className="flex shrink-0 items-center gap-1 rounded-md px-1.5 py-0.5 text-[0.6875rem] font-medium text-muted-foreground transition-colors hover:bg-foreground/[0.05] hover:text-foreground"
                        title="Clean up: build output and package caches below this folder that their projects' tools make again"
                    >
                        <BrushCleaning className="size-3" /> Free up space
                    </button>
                )}
                {map && current && map.serviceTooOld && (
                    <button
                        onClick={enableIndex}
                        className="flex min-w-0 items-center gap-1 rounded-md bg-primary/10 px-2 py-0.5 font-medium text-primary hover:bg-primary/15"
                        title="dust measured this map: the instant search service running is a version from before maps. Updated (Windows asks for administrator approval), it maps folders on indexed drives in milliseconds."
                    >
                        <Sparkles className="size-3 shrink-0" /> <span className="truncate">Update instant search</span>
                    </button>
                )}
                {/* What doesn't fit wraps onto a second line, out of sight, rather than being cut. */}
                <div className="ml-auto flex h-full min-w-0 flex-wrap items-center justify-end gap-x-3 overflow-hidden text-muted-foreground">
                    {legend.map(([group, bytes]) => (
                        <span key={group.name} className="flex h-full shrink-0 items-center gap-1.5" title={`${group.name}: ${formatSize(bytes)} among the files shown`}>
                            <span className="size-2.5 rounded-xs" style={{ background: `hsl(${group.hue} ${group.sat}% ${theme.lightness}%)` }} />
                            {group.name}
                        </span>
                    ))}
                    {wholeFolders && (
                        <span className="flex h-full shrink-0 items-center gap-1.5" title="Folders too deep or too small to show what's in them">
                            <span className="size-2.5 rounded-xs" style={{ background: theme.folder }} />
                            Folders
                        </span>
                    )}
                </div>
            </div>
            <div
                ref={boxRef}
                data-drop={run.path}
                tabIndex={-1}
                role="img"
                aria-label={map && current ? `Disk usage of ${title}: ${map.names.slice(1, 6).map((n, k) => `${n} ${formatSize(map.size[k + 1])}`).join(", ")}` : `Disk usage of ${title}`}
                className="files relative min-h-0 flex-1 cursor-default overflow-hidden outline-hidden"
                onMouseDown={(e) => e.currentTarget.focus()}
                onMouseMove={onMove}
                onMouseLeave={() => setPointer(null)}
                onClick={onClick}
                onDoubleClick={onDoubleClick}
                onContextMenu={(e) => {
                    e.preventDefault();
                    const hit = at(e);
                    if (!hit || !map) return;
                    // The folder mapped has the folder's menu; a frame's smaller items count as its folder.
                    if (hit.node === 0) return onFolderContextMenu(e);
                    if (!selection.has(map.paths[hit.node])) selectOnly(map.paths[hit.node]);
                    onItemContextMenu(e, hit.node - 1);
                }}
            >
                {/* While the next map comes, the last one stays, dimmed only if it takes a moment. */}
                <canvas ref={mapRef} className={cn("absolute inset-0 size-full", (!current || body) && "opacity-0", !current && !body && map && "opacity-40 transition-opacity delay-150")} />
                <canvas ref={overlayRef} className={cn("pointer-events-none absolute inset-0 size-full", (!current || body) && "hidden")} />
                {body && <div className="absolute inset-0">{body}</div>}
                {tip && (
                    <div ref={tipRef} className="pointer-events-none absolute left-0 top-0 z-10 max-w-[min(28rem,80%)] rounded-md border bg-popover px-2.5 py-1.5 text-xs text-popover-foreground shadow-lg">
                        {tip}
                    </div>
                )}
            </div>
        </div>
    );
}

function Tooltip({ map, entry, node, rest }: { map: UsageMap; entry: FileEntry; node: number; rest: boolean }) {
    const total = map.size[0];
    if (rest) {
        const listed = Array.from({ length: map.end[node] - map.first[node] }, (_, k) => map.size[map.first[node] + k]).reduce((a, b) => a + b, 0);
        const bytes = Math.max(0, map.size[node] - listed);
        return (
            <>
                <p className="font-medium">Smaller items</p>
                <p className="truncate text-muted-foreground">in {map.paths[node]}</p>
                <p className="tabular-nums">
                    {formatSize(bytes)} · {share(bytes, total)}
                </p>
                {node !== 0 && <p className="text-muted-foreground">Click to map this folder</p>}
            </>
        );
    }
    const folder = isFolder(map, node);
    const deeper = !folder && map.parent[node] !== 0;
    return (
        <>
            <p className="truncate font-medium">{entry.name || map.path}</p>
            <p className="truncate text-muted-foreground">{map.paths[node]}</p>
            <p className="tabular-nums">
                {formatSize(map.size[node])} · {share(map.size[node], total)} · {folder ? "Folder" : typeLabel(entry)}
                {map.size[node] >= 1024 && <span className="text-muted-foreground"> · {formatCount(map.size[node])} bytes</span>}
            </p>
            {entry.modified > 0 && <p className="text-muted-foreground">Modified {formatDate(entry.modified)}</p>}
            {node !== 0 && <p className="text-muted-foreground">{folder ? "Click to map this folder" : deeper ? "Click to go to its folder" : "Double-click to open"}</p>}
        </>
    );
}

function share(bytes: number, total: number) {
    if (!total) return "0%";
    const pct = (bytes / total) * 100;
    return pct >= 10 ? `${Math.round(pct)}%` : pct >= 0.1 ? `${pct.toFixed(1)}%` : "<0.1%";
}


/** The status bar's right side for a tab that shows its map: who mapped it, and how long it took. */
export function UsageWork({ tab }: { tab: string }) {
    const run = useUsage((s) => s[tab]);
    if (!run) return null;
    const current = run.map && samePath(run.map.path, run.path) ? run.map : null;
    if (run.running && (run.preview || !current)) {
        return (
            <>
                <Loader2 className="size-3 animate-spin text-primary" /> {run.engine === "dust" ? "dust · measuring…" : `${run.engine} · mapping…`}
            </>
        );
    }
    if (!current) return null;
    return (
        <span
            title={`${current.engine === "index" ? "The instant search index" : current.engine === "dust" ? "dust" : "The archive's own list of entries"} mapped this folder in ${formatDuration(current.elapsed)}; it was on screen ${formatDuration(run.shownIn)} after the click.`}
            className="flex items-center gap-1"
        >
            <Zap className="size-3 text-primary" /> {current.engine} · mapped {formatSize(current.size[0])} in {formatDuration(current.elapsed)}
            {run.shownIn > 0 && ` · shown in ${formatDuration(run.shownIn)}`}
        </span>
    );
}
