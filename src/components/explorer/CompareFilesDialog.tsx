import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";
import { toast } from "sonner";
import { convertFileSrc } from "@tauri-apps/api/core";
import { AlertTriangle, ArrowLeftRight, Binary, CheckCircle2, ChevronDown, ChevronUp, Download, FileText, GitCompareArrows, Image as ImageIcon, Loader2 } from "lucide-react";
import { withDetails } from "@/lib/entries";
import { kindOf } from "@/lib/fileTypes";
import { formatCount, formatDuration, formatSize, plural } from "@/lib/format";
import { type BytesWindow, compareBytes, compareTextFiles, nextByteDifference, type TextCompare } from "@/lib/fs";
import { baseName, parentPath } from "@/lib/paths";
import { installTool, useTool, useTools } from "@/lib/tools";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";
import { VirtualList } from "../VirtualList";

/** One of the two files: its path, and what to call it when its name doesn't say ("In the last commit"). */
export interface CompareSide {
    path: string;
    label?: string;
}

interface CompareState {
    left: CompareSide | null;
    right: CompareSide | null;
    /** Run once the comparison closes (a temp copy is removed). */
    onClose?: () => void;
}

const useCompareStore = create<CompareState>(() => ({ left: null, right: null }));

/** Opens two files side by side: text (by Difftastic when it's installed), pictures, or bytes. */
export function compareFiles(left: string | CompareSide, right: string | CompareSide, onClose?: () => void) {
    const side = (s: string | CompareSide) => (typeof s === "string" ? { path: s } : s);
    useCompareStore.getState().onClose?.();
    useCompareStore.setState({ left: side(left), right: side(right), onClose });
}

function closeCompare() {
    useCompareStore.getState().onClose?.();
    useCompareStore.setState({ left: null, right: null, onClose: undefined });
}

export function CompareFilesDialog() {
    const { left, right } = useCompareStore(useShallow((s) => ({ left: s.left, right: s.right })));
    if (!left || !right) return null;
    return <CompareModal key={`${left.path}|${right.path}`} left={left} right={right} onClose={closeCompare} />;
}

type View = "text" | "pictures" | "bytes";

const isPicture = (path: string) => kindOf(withDetails({ name: baseName(path), path, dir: false, size: 0, modified: 0, attrs: 0 })).preview === "image";
const nameOf = (side: CompareSide) => side.label ?? baseName(side.path);

function CompareModal({ left, right, onClose }: { left: CompareSide; right: CompareSide; onClose: () => void }) {
    const pictures = isPicture(left.path) && isPicture(right.path);
    const [view, setView] = useState<View>(pictures ? "pictures" : "text");
    const [swapped, setSwapped] = useState(false);
    const [a, b] = swapped ? [right, left] : [left, right];
    const views: { key: View; label: string; icon: typeof FileText }[] = [
        { key: "text", label: "Text", icon: FileText },
        ...(pictures ? [{ key: "pictures" as View, label: "Pictures", icon: ImageIcon }] : []),
        { key: "bytes", label: "Bytes", icon: Binary },
    ];
    // Where they are: each folder once, leaving out a side that has a label (a temp copy's folder says nothing).
    const folders = [...new Set([a, b].filter((side) => !side.label).map((side) => parentPath(side.path)))];
    const subtitle = (folders.length ? folders : [parentPath(a.path)]).join("  ·  ");

    return (
        <Modal label={`Compare ${nameOf(left)} and ${nameOf(right)}`} onClose={onClose} className="h-[min(92vh,64rem)] max-h-none max-w-[min(96vw,120rem)]">
            <ModalHeader icon={GitCompareArrows} title={`Compare “${nameOf(a)}” and “${nameOf(b)}”`} subtitle={subtitle} onClose={onClose}>
                <div role="group" aria-label="Compare as" className="flex shrink-0 rounded-md bg-foreground/[0.05] p-0.5">
                    {views.map((v) => (
                        <button
                            key={v.key}
                            type="button"
                            aria-pressed={view === v.key}
                            onClick={() => setView(v.key)}
                            className={cn(
                                "flex h-6 items-center gap-1 rounded-[4px] px-2 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground",
                                view === v.key && "bg-background text-primary shadow-xs dark:bg-foreground/[0.12]"
                            )}
                        >
                            <v.icon className="size-3.5" /> {v.label}
                        </button>
                    ))}
                </div>
                <Button variant="ghost" size="sm" className="h-7 shrink-0 px-2 text-xs" onClick={() => setSwapped((s) => !s)} title="Swap the sides">
                    <ArrowLeftRight className="size-3.5" /> Swap
                </Button>
            </ModalHeader>
            <div className="flex shrink-0 border-b bg-chrome/60 text-xs">
                {[a, b].map((side, n) => (
                    <div key={n} className={cn("flex min-w-0 flex-1 items-baseline gap-2 px-3.5 py-1.5", n === 1 && "border-l")}>
                        <span className={cn("size-2 shrink-0 self-center rounded-full", n === 0 ? "bg-red-500/70" : "bg-emerald-500/70")} />
                        <span className="truncate font-medium text-foreground" title={side.path}>
                            {nameOf(side)}
                        </span>
                        {side.label && <span className="truncate text-muted-foreground">{baseName(side.path)}</span>}
                    </div>
                ))}
            </div>
            <div className="flex min-h-0 flex-1 flex-col">
                {view === "text" ? <TextView left={a} right={b} onBytes={() => setView("bytes")} /> : view === "pictures" ? <PicturesView left={a} right={b} /> : <BytesView left={a} right={b} />}
            </div>
        </Modal>
    );
}

/** `text` with the ranges in `spans` marked: what changed in the line. */
function Marked({ text, spans, mark }: { text: string; spans: [number, number][]; mark: string }) {
    const sorted = [...spans].sort((x, y) => x[0] - y[0]);
    const parts: React.ReactNode[] = [];
    let at = 0;
    for (const [start, end] of sorted) {
        if (end <= at) continue;
        const from = Math.max(start, at);
        if (from > at) parts.push(text.slice(at, from));
        parts.push(
            <mark key={from} className={cn("rounded-[2px] text-inherit", mark)}>
                {text.slice(from, end)}
            </mark>
        );
        at = end;
    }
    if (at < text.length) parts.push(text.slice(at));
    return <>{parts}</>;
}

function Half({ line, text, spans, side }: { line: number | null; text?: string; spans?: [number, number][]; side: 0 | 1 }) {
    if (line === null) return <div className="min-w-0 flex-1 bg-[repeating-linear-gradient(135deg,transparent_0_6px,var(--color-border)_6px_7px)] opacity-60" />;
    const changed = spans !== undefined;
    const tint = changed ? (side === 0 ? "bg-red-500/10" : "bg-emerald-500/10") : "";
    const mark = side === 0 ? "bg-red-500/35" : "bg-emerald-500/35";
    return (
        <div className={cn("flex min-w-0 flex-1", tint)} title={text}>
            <span className="w-12 shrink-0 select-none pr-2 text-right tabular-nums text-muted-foreground/70">{line + 1}</span>
            <span className="min-w-0 flex-1 overflow-hidden whitespace-pre text-ellipsis pr-2 [tab-size:4]">{spans?.length ? <Marked text={text ?? ""} spans={spans} mark={mark} /> : text || " "}</span>
        </div>
    );
}

function TextView({ left, right, onBytes }: { left: CompareSide; right: CompareSide; onBytes: () => void }) {
    const [compare, setCompare] = useState<TextCompare | null>(null);
    const [error, setError] = useState<string | null>(null);
    const difftastic = useTool("difftastic");
    const installing = useTools((s) => !!s.busy.difftastic);
    const run = useCallback(() => {
        let stale = false;
        setCompare(null);
        setError(null);
        compareTextFiles(left.path, right.path).then(
            (found) => !stale && setCompare(found),
            (e) => !stale && setError(String(e))
        );
        return () => {
            stale = true;
        };
    }, [left.path, right.path]);
    useEffect(run, [run]);
    // Once Difftastic is installed, they're compared again with it.
    const withDifftastic = !!difftastic?.origin;
    const engine = compare?.engine;
    useEffect(() => {
        if (withDifftastic && engine === "lines") run();
    }, [withDifftastic, engine, run]);

    // The changes, as blocks of rows, to go from one to the next.
    const blocks = useMemo(() => {
        if (!compare) return [];
        const starts: number[] = [];
        let inside = false;
        compare.rows.forEach(([l, r], i) => {
            const changed = (l !== null && compare.leftChanges[l] !== undefined) || (r !== null && compare.rightChanges[r] !== undefined);
            if (changed && !inside) starts.push(i);
            inside = changed;
        });
        return starts;
    }, [compare]);
    const [at, setAt] = useState(-1);
    const go = useCallback((step: number) => setAt((n) => (blocks.length ? Math.max(0, Math.min(blocks.length - 1, n + step)) : -1)), [blocks]);
    // Opens on the first change.
    useEffect(() => setAt(blocks.length ? 0 : -1), [blocks]);
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (!e.altKey || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
            e.preventDefault();
            go(e.key === "ArrowDown" ? 1 : -1);
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [go]);

    if (error)
        return (
            <div className="flex flex-1 flex-col items-center justify-center gap-2 text-sm">
                <AlertTriangle className="size-6 text-destructive" />
                <p>{error}</p>
            </div>
        );
    if (!compare)
        return (
            <div className="flex flex-1 items-center justify-center">
                <Loader2 className="size-6 animate-spin text-primary" />
            </div>
        );
    if (compare.binary || compare.tooBig)
        return (
            <div className="flex flex-1 flex-col items-center justify-center gap-2 text-sm text-muted-foreground">
                <Binary className="size-10 stroke-[1.25]" />
                <p>{compare.binary ? "They aren't text: their bytes can be compared." : "Too big to compare as text here (over 8 MB)."}</p>
                <Button variant="outline" size="sm" onClick={onBytes}>
                    <Binary /> Compare their bytes
                </Button>
            </div>
        );

    const removed = Object.keys(compare.leftChanges).length;
    const added = Object.keys(compare.rightChanges).length;
    return (
        <>
            <div className="flex h-9 shrink-0 items-center gap-2 border-b px-3.5 text-xs text-muted-foreground">
                {blocks.length ? (
                    <span className="font-medium text-foreground">
                        {plural(blocks.length, "change")} <span className="text-red-700 dark:text-red-400">−{formatCount(removed)}</span>{" "}
                        <span className="text-emerald-700 dark:text-emerald-400">+{formatCount(added)}</span> lines
                    </span>
                ) : (
                    <span className="flex items-center gap-1.5 font-medium text-foreground">
                        <CheckCircle2 className="size-3.5 text-emerald-500" />
                        {compare.engine === "difftastic" ? "The same code" : "The same text"}
                    </span>
                )}
                <span
                    title={compare.engine === "difftastic" ? "Difftastic compared them by the code's structure: a change of layout alone isn't one" : "Compared line by line"}
                >
                    · {compare.engine === "difftastic" ? `Difftastic${compare.language ? `, as ${compare.language}` : ""}` : "line by line"} in {formatDuration(compare.elapsed)}
                </span>
                {compare.engine === "lines" && difftastic && !difftastic.origin && (
                    <Button variant="ghost" size="sm" className="h-6 px-1.5 text-xs" disabled={installing} onClick={() => void installTool("difftastic")} title="Difftastic compares code by its structure: what changed in it, not its layout. It installs with winget.">
                        {installing ? <Loader2 className="size-3 animate-spin" /> : <Download className="size-3" />} Difftastic
                    </Button>
                )}
                <div className="ml-auto flex items-center gap-1">
                    {blocks.length > 0 && <span className="tabular-nums">{at >= 0 ? `${at + 1} of ${blocks.length}` : ""}</span>}
                    <Button variant="ghost" size="icon" className="size-7" disabled={!blocks.length || at <= 0} onClick={() => go(-1)} title="Previous change (Alt+Up)">
                        <ChevronUp />
                    </Button>
                    <Button variant="ghost" size="icon" className="size-7" disabled={!blocks.length || at >= blocks.length - 1} onClick={() => go(1)} title="Next change (Alt+Down)">
                        <ChevronDown />
                    </Button>
                </div>
            </div>
            <VirtualList count={compare.rows.length} rowHeight={20} scrollToIndex={at >= 0 ? blocks[at] : -1} className="min-h-0 flex-1 overflow-auto font-mono text-xs leading-5" role="table" aria-label="Side by side">
                {(i) => {
                    const [l, r] = compare.rows[i];
                    return (
                        <div role="row" className={cn("flex h-5", at >= 0 && blocks[at] === i && "outline outline-1 -outline-offset-1 outline-primary/50")}>
                            <Half line={l} text={l !== null ? compare.left[l] : undefined} spans={l !== null ? compare.leftChanges[l] : undefined} side={0} />
                            <div className="w-px shrink-0 bg-border" />
                            <Half line={r} text={r !== null ? compare.right[r] : undefined} spans={r !== null ? compare.rightChanges[r] : undefined} side={1} />
                        </div>
                    );
                }}
            </VirtualList>
        </>
    );
}

type PictureMode = "side" | "swipe" | "onion";

function PicturesView({ left, right }: { left: CompareSide; right: CompareSide }) {
    const [mode, setMode] = useState<PictureMode>("side");
    const [amount, setAmount] = useState(50);
    const [dims, setDims] = useState<[string, string]>(["", ""]);
    const src = [convertFileSrc(left.path), convertFileSrc(right.path)];
    const loaded = (n: 0 | 1) => (e: React.SyntheticEvent<HTMLImageElement>) => {
        const { naturalWidth: w, naturalHeight: h } = e.currentTarget;
        setDims((d) => (n === 0 ? [`${w} × ${h}`, d[1]] : [d[0], `${w} × ${h}`]));
    };
    const img = "absolute inset-0 m-auto max-h-full max-w-full object-contain";
    return (
        <>
            <div className="flex h-9 shrink-0 items-center gap-3 border-b px-3.5 text-xs text-muted-foreground">
                <div role="group" aria-label="Show them" className="flex rounded-md bg-foreground/[0.05] p-0.5">
                    {(
                        [
                            ["side", "Side by side"],
                            ["swipe", "Swipe"],
                            ["onion", "Onion skin"],
                        ] as [PictureMode, string][]
                    ).map(([key, label]) => (
                        <button
                            key={key}
                            type="button"
                            aria-pressed={mode === key}
                            onClick={() => setMode(key)}
                            className={cn("h-6 rounded-[4px] px-2 font-medium transition-colors hover:text-foreground", mode === key && "bg-background text-primary shadow-xs dark:bg-foreground/[0.12]")}
                        >
                            {label}
                        </button>
                    ))}
                </div>
                {mode !== "side" && (
                    <input type="range" min={0} max={100} value={amount} onChange={(e) => setAmount(Number(e.target.value))} aria-label={mode === "swipe" ? "Where the swipe is" : "How much of the second shows"} className="w-48 accent-primary" />
                )}
                <span className="ml-auto tabular-nums">
                    {dims[0] || "…"} · {dims[1] || "…"}
                </span>
            </div>
            {mode === "side" ? (
                <div className="flex min-h-0 flex-1">
                    {[0, 1].map((n) => (
                        <div key={n} className={cn("relative min-w-0 flex-1 bg-[conic-gradient(var(--color-muted)_25%,transparent_0_50%,var(--color-muted)_0_75%,transparent_0)] bg-[length:16px_16px]", n === 1 && "border-l")}>
                            <img src={src[n]} alt={baseName(n === 0 ? left.path : right.path)} onLoad={loaded(n as 0 | 1)} className={img} draggable={false} />
                        </div>
                    ))}
                </div>
            ) : (
                <div className="relative min-h-0 flex-1 bg-[conic-gradient(var(--color-muted)_25%,transparent_0_50%,var(--color-muted)_0_75%,transparent_0)] bg-[length:16px_16px]">
                    <img src={src[0]} alt={baseName(left.path)} onLoad={loaded(0)} className={img} draggable={false} />
                    <img
                        src={src[1]}
                        alt={baseName(right.path)}
                        onLoad={loaded(1)}
                        className={img}
                        draggable={false}
                        style={mode === "swipe" ? { clipPath: `inset(0 0 0 ${amount}%)` } : { opacity: amount / 100 }}
                    />
                    {mode === "swipe" && <div className="pointer-events-none absolute inset-y-0 w-0.5 bg-primary shadow" style={{ left: `${amount}%` }} />}
                </div>
            )}
        </>
    );
}

const ROW = 16;
const WINDOW = 64 * 1024;
const hex = (n: number, width: number) => n.toString(16).toUpperCase().padStart(width, "0");
const ascii = (byte: number) => (byte >= 0x20 && byte < 0x7f ? String.fromCharCode(byte) : "·");

function BytesView({ left, right }: { left: CompareSide; right: CompareSide }) {
    const windows = useRef(new Map<number, BytesWindow | "loading">());
    const [, loadedOne] = useReducer((n: number) => n + 1, 0);
    const [sizes, setSizes] = useState<[number, number] | null>(null);
    const [error, setError] = useState<string | null>(null);
    /** Where the last difference shown is, and where the first one is (null: none). */
    const [shown, setShown] = useState(-1);
    const [first, setFirst] = useState<number | null | undefined>(undefined);
    const [busy, setBusy] = useState(false);

    const load = useCallback(
        (n: number) => {
            if (windows.current.has(n)) return;
            windows.current.set(n, "loading");
            compareBytes(left.path, right.path, n * WINDOW, WINDOW).then(
                (window) => {
                    windows.current.set(n, window);
                    setSizes([window.leftSize, window.rightSize]);
                    loadedOne();
                },
                (e) => setError(String(e))
            );
        },
        [left.path, right.path]
    );
    useEffect(() => {
        load(0);
        nextByteDifference(left.path, right.path, 0).then(
            (at) => {
                setFirst(at);
                if (at !== null) setShown(at);
            },
            (e) => setError(String(e))
        );
    }, [left.path, right.path, load]);

    const next = async () => {
        setBusy(true);
        try {
            const at = await nextByteDifference(left.path, right.path, (Math.floor(shown / ROW) + 1) * ROW);
            if (at === null) toast.message("No more differences after this one", { id: "bytes" });
            else setShown(at);
        } catch (e) {
            setError(String(e));
        } finally {
            setBusy(false);
        }
    };

    if (error)
        return (
            <div className="flex flex-1 flex-col items-center justify-center gap-2 text-sm">
                <AlertTriangle className="size-6 text-destructive" />
                <p>{error}</p>
            </div>
        );
    if (!sizes)
        return (
            <div className="flex flex-1 items-center justify-center">
                <Loader2 className="size-6 animate-spin text-primary" />
            </div>
        );
    const rows = Math.ceil(Math.max(sizes[0], sizes[1]) / ROW);
    const verdict =
        first === undefined ? (
            <span className="flex items-center gap-1.5">
                <Loader2 className="size-3 animate-spin" /> Looking for where they differ…
            </span>
        ) : first === null ? (
            <span className="flex items-center gap-1.5 font-medium text-foreground">
                <CheckCircle2 className="size-3.5 text-emerald-500" /> The same, byte for byte
            </span>
        ) : (
            <span className="font-medium text-foreground">First difference at 0x{hex(first, 8)}</span>
        );

    const side = (bytes: Uint8Array | undefined, other: Uint8Array | undefined, n: 0 | 1, offset: number) => {
        const cells: React.ReactNode[] = [];
        const chars: React.ReactNode[] = [];
        for (let k = 0; k < ROW; k++) {
            const byte = bytes?.[k];
            const differs = byte !== undefined && (other?.[k] === undefined || other[k] !== byte);
            const mark = differs ? (n === 0 ? "bg-red-500/30" : "bg-emerald-500/30") : "";
            const here = offset + k === shown ? "outline outline-1 outline-primary" : "";
            cells.push(
                <span key={k} className={cn("w-[2.5ch] text-center", k === 8 && "ml-[1ch]", mark, here)}>
                    {byte === undefined ? "  " : hex(byte, 2)}
                </span>
            );
            chars.push(
                <span key={k} className={mark}>
                    {byte === undefined ? " " : ascii(byte)}
                </span>
            );
        }
        return (
            <div className={cn("flex min-w-0 flex-1 gap-3", n === 1 && "border-l pl-3")}>
                <span className="flex">{cells}</span>
                <span className="flex whitespace-pre text-muted-foreground">{chars}</span>
            </div>
        );
    };

    return (
        <>
            <div className="flex h-9 shrink-0 items-center gap-2 border-b px-3.5 text-xs text-muted-foreground">
                {verdict}
                <span>
                    · {formatSize(sizes[0])} and {formatSize(sizes[1])}
                    {sizes[0] !== sizes[1] ? ` (${formatCount(Math.abs(sizes[0] - sizes[1]))} bytes apart)` : ""}
                </span>
                <Button variant="ghost" size="sm" className="ml-auto h-7 px-2 text-xs" disabled={!first || busy} onClick={() => void next()} title="The next place they differ, after this row">
                    {busy ? <Loader2 className="size-3.5 animate-spin" /> : <ChevronDown className="size-3.5" />} Next difference
                </Button>
            </div>
            <VirtualList count={rows} rowHeight={20} scrollToIndex={shown >= 0 ? Math.floor(shown / ROW) : -1} className="min-h-0 flex-1 overflow-auto px-3.5 font-mono text-xs leading-5" role="table" aria-label="Bytes side by side">
                {(i) => {
                    const offset = i * ROW;
                    const n = Math.floor(offset / WINDOW);
                    const window = windows.current.get(n);
                    if (!window || window === "loading") {
                        load(n);
                        return <div className="h-5 text-muted-foreground/50">{hex(offset, 8)}</div>;
                    }
                    const local = offset - n * WINDOW;
                    const [l, r] = [window.left.subarray(local, local + ROW), window.right.subarray(local, local + ROW)];
                    return (
                        <div role="row" className="flex h-5 gap-3 tabular-nums">
                            <span className="w-[8ch] shrink-0 select-none text-muted-foreground/70">{hex(offset, 8)}</span>
                            {side(l.length ? l : undefined, r.length ? r : undefined, 0, offset)}
                            {side(r.length ? r : undefined, l.length ? l : undefined, 1, offset)}
                        </div>
                    );
                }}
            </VirtualList>
        </>
    );
}
