import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { Clock, Cloud, ExternalLink, FileArchive, Fingerprint, FolderLock, FolderOpen, FolderSearch, GitCompareArrows, HardDrive, Loader2, PenLine, Pipette, RotateCcw, RotateCwSquare, ScanText, Trash2, Wallpaper, X } from "lucide-react";
import { toast } from "sonner";
import { ATTR, type FileEntry, isArchived, isArchiveFile, isClosed, isListOnly, isLocked, isOnlineOnly, isRecycled, isRemote } from "@/lib/entries";
import { archiveOf, calcSizes, compressEntries, currentPath, navigate, openEntry, openFile, reload, restoreRecycled, useExplorer } from "@/lib/explorer";
import { dataFormat, isKnownType, isMarkdown, previewKind, typeLabel } from "@/lib/fileTypes";
import { formatCount, formatDate, formatFullDate, formatSize, plural } from "@/lib/format";
import { archiveExtract, type ArchivePeek, archivePeek, listDir, ocrText, readTextPreview, rotateImage, setDesktopWallpaper } from "@/lib/fs";
import { useGitMark } from "@/lib/git";
import { mapLink, useMediaDetails } from "@/lib/media";
import { usePreviewView, type View } from "@/lib/preview";
import { languageFor, SyntaxHighlighter } from "@/lib/highlight";
import { baseName, isRecent, isRecycleBin, parentPath } from "@/lib/paths";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { openBatchRename } from "./BatchRenameDialog";
import { BinaryPreview, isPeCandidate } from "./BinaryPreview";
import { CastPreview } from "./CastPreview";
import { ChangesView } from "./ChangesView";
import { compareFiles } from "./CompareFilesDialog";
import { DataTree } from "./DataTree";
import { HexView } from "./HexView";
import { QueryView } from "./QueryView";
import { isSheetCandidate, isSqliteCandidate, isTabularCandidate, SheetPreview, SqlitePreview, TabularPreview } from "./DataPreview";
import { LockMark } from "./FileList";
import { MarkdownView } from "./MarkdownView";
import { RepoCard } from "./RepoCard";
import { ShellPreview } from "./ShellPreview";
import { BigIcon, Centered, Waiting } from "./PreviewState";
import { Thumbnail } from "./Thumbnail";
import { inspectXRay } from "./XRayDialog";

const TEXT_BYTES = 256 * 1024;
const TEXT_LINES = 2000;
/** The start of an ncdu export: `[1,` its minor version, then its metadata naming the program (archive/ncdu.rs `looks_like`). */
const NCDU_EXPORT = /^﻿?\s*\[\s*1\s*,\s*\d+\s*,\s*\{\s*"progname"/;

type ImageBackdrop = "checker" | "dark" | "light";
/** Pictures Windows' imaging component rotates in place, and those it can set as the desktop background. */
const ROTATE_EXTS = ["jpg", "jpeg", "png", "bmp", "tif", "tiff", "jfif"];
const WALLPAPER_EXTS = ["jpg", "jpeg", "png", "bmp", "gif", "tif", "tiff", "jfif", "webp", "jxr"];

function ImagePreview({ entry, src }: { entry: FileEntry; src: string }) {
    const [dimensions, setDimensions] = useState("");
    const [failed, setFailed] = useState(false);
    const [reading, setReading] = useState(false);
    const [zoom, setZoom] = useState(1);
    const [pan, setPan] = useState({ x: 0, y: 0 });
    const [backdrop, setBackdrop] = useState<ImageBackdrop>("checker");
    const [loupeOn, setLoupeOn] = useState(false);
    const [loupe, setLoupe] = useState<{ x: number; y: number; hex: string } | null>(null);
    const imgRef = useRef<HTMLImageElement | null>(null);
    const loupeCanvasRef = useRef<HTMLCanvasElement | null>(null);
    /** The picture's pixels for the color picker (scaled down past 16 megapixels), once loaded. */
    const sampleCanvasRef = useRef<{ canvas: HTMLCanvasElement; scale: number } | null>(null);
    const dragRef = useRef<{ startX: number; startY: number; panX: number; panY: number } | null>(null);
    const frameRef = useRef<HTMLDivElement | null>(null);
    // Windows' own picture tools, for a picture on disk (not in an archive, a remote or the Recycle Bin).
    const [rotating, setRotating] = useState(false);
    const [version, setVersion] = useState(0);
    const local = !isArchived(entry) && !isRemote(entry) && !isRecycled(entry) && !isClosed(entry);
    const rotatable = local && ROTATE_EXTS.includes(entry.ext);
    const wallpaper = local && WALLPAPER_EXTS.includes(entry.ext);

    const rotate = async () => {
        setRotating(true);
        try {
            await rotateImage(entry.path, true);
            sampleCanvasRef.current = null;
            setVersion((v) => v + 1);
            void reload();
        } catch (e) {
            toast.error(`Couldn't rotate “${entry.name}”`, { description: String(e) });
        } finally {
            setRotating(false);
        }
    };

    const setAsWallpaper = () =>
        setDesktopWallpaper(entry.path).then(
            () => toast.success(`“${entry.name}” is now your desktop background`),
            (e) => toast.error("Couldn't set the desktop background", { description: String(e) })
        );

    useEffect(() => {
        setZoom(1);
        setPan({ x: 0, y: 0 });
        setLoupe(null);
    }, [src]);

    const shown = version ? `${src}${src.includes("?") ? "&" : "?"}v=${version}` : src;
    // The picture on screen comes from another origin, so its pixels can't be read: the color picker
    // loads a copy of its own that may be (a URL of its own too, so a cached plain load isn't reused).
    // The wheel zooms: with a listener of its own, as React's are passive and can't keep the pane from
    // scrolling as well.
    useEffect(() => {
        const frame = frameRef.current;
        if (!frame) return;
        const onWheel = (e: WheelEvent) => {
            e.preventDefault();
            const factor = e.deltaY < 0 ? 1.2 : 1 / 1.2;
            setZoom((z) => {
                const next = Math.min(8, Math.max(1, +(z * factor).toFixed(2)));
                if (next === 1) setPan({ x: 0, y: 0 });
                return next;
            });
        };
        frame.addEventListener("wheel", onWheel, { passive: false });
        return () => frame.removeEventListener("wheel", onWheel);
    }, [failed]);

    useEffect(() => {
        sampleCanvasRef.current = null;
        if (!loupeOn) return;
        let live = true;
        const copy = new Image();
        copy.crossOrigin = "anonymous";
        copy.onload = () => {
            if (!live || !copy.naturalWidth) return;
            const scale = Math.min(1, Math.sqrt(16e6 / (copy.naturalWidth * copy.naturalHeight)));
            const canvas = document.createElement("canvas");
            canvas.width = Math.max(1, Math.round(copy.naturalWidth * scale));
            canvas.height = Math.max(1, Math.round(copy.naturalHeight * scale));
            const ctx = canvas.getContext("2d", { willReadFrequently: true });
            if (!ctx) return;
            ctx.drawImage(copy, 0, 0, canvas.width, canvas.height);
            sampleCanvasRef.current = { canvas, scale };
        };
        copy.src = `${shown}${shown.includes("?") ? "&" : "?"}pixels`;
        return () => {
            live = false;
        };
    }, [loupeOn, shown]);

    const updateLoupe = (clientX: number, clientY: number) => {
        const img = imgRef.current;
        if (!img || !img.naturalWidth) return;
        const rect = img.getBoundingClientRect();
        if (clientX < rect.left || clientX > rect.right || clientY < rect.top || clientY > rect.bottom) {
            setLoupe(null);
            return;
        }
        const relX = (clientX - rect.left) / rect.width;
        const relY = (clientY - rect.top) / rect.height;
        const pixels = sampleCanvasRef.current;
        // Not loaded yet: the magnifier waits, rather than saying black.
        if (!pixels) return setLoupe(null);
        const sample = pixels.canvas;
        const px = Math.min(sample.width - 1, Math.max(0, Math.floor(relX * sample.width)));
        const py = Math.min(sample.height - 1, Math.max(0, Math.floor(relY * sample.height)));
        let hex = "#000000";
        try {
            const ctx = sample.getContext("2d", { willReadFrequently: true });
            if (ctx) {
                const data = ctx.getImageData(px, py, 1, 1).data;
                hex = `#${[data[0], data[1], data[2]].map((v) => v.toString(16).padStart(2, "0")).join("").toUpperCase()}`;
            }
            const lc = loupeCanvasRef.current;
            const lctx = lc?.getContext("2d");
            if (lc && lctx && sample) {
                lctx.clearRect(0, 0, lc.width, lc.height);
                lctx.imageSmoothingEnabled = false;
                const radius = 11;
                lctx.drawImage(sample, px - radius, py - radius, radius * 2 + 1, radius * 2 + 1, 0, 0, lc.width, lc.height);
                // Center pixel reticle
                const cell = lc.width / (radius * 2 + 1);
                lctx.strokeStyle = "rgba(255,255,255,0.9)";
                lctx.lineWidth = 1.5;
                lctx.strokeRect(radius * cell, radius * cell, cell, cell);
            }
        } catch {
            // A picture whose pixels can't be read after all (an SVG's, say): no color to show.
            return setLoupe(null);
        }
        setLoupe({ x: clientX, y: clientY, hex });
    };

    // The text in it, read by Windows' OCR, to the clipboard (a picture on disk: not one in an archive).
    const copyText = async () => {
        setReading(true);
        try {
            const lines = await ocrText(entry.path);
            if (!lines.length) toast.message("No text found in this picture");
            else {
                await navigator.clipboard.writeText(lines.join("\n"));
                toast.success(`Copied ${lines.length === 1 ? "a line" : `${lines.length} lines`} of text`, { description: lines[0] });
            }
        } catch (e) {
            toast.error("Couldn't read the text", { description: String(e) });
        } finally {
            setReading(false);
        }
    };
    if (failed) return <BigIcon entry={entry} note="This image can't be shown here" />;
    const bgClass =
        backdrop === "checker"
            ? "[background:repeating-conic-gradient(hsl(var(--muted))_0%_25%,transparent_0%_50%)_50%/16px_16px]"
            : backdrop === "dark"
              ? "bg-zinc-950"
              : "bg-white";
    return (
        <Centered className="gap-2 p-4">
            <div
                ref={frameRef}
                className="relative flex max-h-[calc(100%-2.75rem)] max-w-full items-center justify-center overflow-hidden rounded-sm"
                onDoubleClick={() => {
                    setZoom((z) => (z > 1 ? 1 : 2));
                    setPan({ x: 0, y: 0 });
                }}
                onPointerDown={(e) => {
                    if (loupeOn && loupe) {
                        void navigator.clipboard.writeText(loupe.hex).then(() => toast.success(`Copied ${loupe.hex}`, { duration: 1300 }));
                        return;
                    }
                    if (zoom <= 1 || e.button !== 0) return;
                    e.currentTarget.setPointerCapture(e.pointerId);
                    dragRef.current = { startX: e.clientX, startY: e.clientY, panX: pan.x, panY: pan.y };
                }}
                onPointerMove={(e) => {
                    if (dragRef.current) {
                        setPan({
                            x: dragRef.current.panX + (e.clientX - dragRef.current.startX),
                            y: dragRef.current.panY + (e.clientY - dragRef.current.startY),
                        });
                    } else if (loupeOn) {
                        updateLoupe(e.clientX, e.clientY);
                    }
                }}
                onPointerUp={() => {
                    dragRef.current = null;
                }}
                onPointerLeave={() => {
                    dragRef.current = null;
                    setLoupe(null);
                }}
            >
                <img
                    ref={imgRef}
                    src={shown}
                    alt={entry.name}
                    draggable={false}
                    onLoad={(e) => setDimensions(`${e.currentTarget.naturalWidth} × ${e.currentTarget.naturalHeight}`)}
                    onError={() => setFailed(true)}
                    style={{
                        transform: zoom > 1 || pan.x || pan.y ? `translate3d(${pan.x}px, ${pan.y}px, 0) scale(${zoom})` : undefined,
                    }}
                    className={cn(
                        "max-h-full max-w-full rounded-sm object-contain shadow-sm transition-transform duration-75",
                        bgClass,
                        loupeOn ? "cursor-crosshair" : zoom > 1 ? "cursor-grab active:cursor-grabbing" : "cursor-zoom-in",
                    )}
                />
                {loupeOn && loupe && (
                    // Fixed, so the frame doesn't cut it off; below the pointer near the top of the window.
                    <div
                        className="pointer-events-none fixed z-50 flex flex-col items-center gap-1"
                        style={{
                            left: loupe.x,
                            top: loupe.y,
                            transform: loupe.y < 140 ? "translate(-50%, 18px)" : "translate(-50%, calc(-100% - 14px))",
                        }}
                    >
                        <canvas
                            ref={loupeCanvasRef}
                            width={88}
                            height={88}
                            className="size-22 rounded-full border-2 border-background bg-zinc-900 shadow-lg ring-1 ring-border"
                        />
                        <span className="flex items-center gap-1.5 rounded-full border border-border bg-popover/95 px-2 py-0.5 font-mono text-[11px] font-medium text-popover-foreground shadow-md">
                            <span className="size-2.5 rounded-full border border-white/30" style={{ background: loupe.hex }} />
                            {loupe.hex}
                        </span>
                    </div>
                )}
            </div>
            <div className="flex flex-wrap items-center justify-center gap-1.5">
                {dimensions && <span className="mr-1 text-xs tabular-nums">{dimensions} px</span>}
                {zoom > 1 && (
                    <button
                        type="button"
                        onClick={() => {
                            setZoom(1);
                            setPan({ x: 0, y: 0 });
                        }}
                        className="rounded-md bg-primary/15 px-1.5 py-0.5 font-mono text-[11px] font-medium text-primary hover:bg-primary/25"
                        title="Click to reset zoom to 1×"
                    >
                        {zoom.toFixed(1)}×
                    </button>
                )}
                <button
                    type="button"
                    onClick={() => setLoupeOn((v) => !v)}
                    className={cn(
                        "inline-flex h-6 items-center gap-1 rounded-md px-2 text-xs transition-colors",
                        loupeOn ? "bg-primary/15 font-medium text-primary ring-1 ring-primary/30" : "text-muted-foreground hover:bg-accent hover:text-foreground",
                    )}
                    title="A magnifier that shows each pixel's color: click the picture to copy it (#RRGGBB)"
                >
                    <Pipette className="size-3.5" /> Colors
                </button>
                <button
                    type="button"
                    onClick={() => setBackdrop((b) => (b === "checker" ? "dark" : b === "dark" ? "light" : "checker"))}
                    className="inline-flex h-6 items-center gap-1.5 rounded-md px-2 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                    title="What shows through a transparent picture: checks, dark or light"
                >
                    <span
                        className={cn(
                            "size-3 rounded-full border border-border",
                            backdrop === "checker"
                                ? "[background:repeating-conic-gradient(hsl(var(--muted-foreground))_0%_25%,transparent_0%_50%)_50%/6px_6px]"
                                : backdrop === "dark"
                                  ? "bg-zinc-950"
                                  : "bg-white",
                        )}
                    />
                    {backdrop === "checker" ? "Checks" : backdrop === "dark" ? "Dark" : "Light"}
                </button>
                {!isArchived(entry) && (
                    <Button variant="ghost" size="sm" className="h-6 px-2 text-xs" disabled={reading} onClick={() => void copyText()} title="Copy the text in this picture, read by Windows' OCR">
                        {reading ? <Loader2 className="size-3.5 animate-spin" /> : <ScanText className="size-3.5" />} Copy text
                    </Button>
                )}
                {rotatable && (
                    <Button variant="ghost" size="sm" className="h-6 px-2 text-xs" disabled={rotating} onClick={() => void rotate()} title="Turn it a quarter clockwise, in place (a photo keeps its quality: only its orientation changes)">
                        {rotating ? <Loader2 className="size-3.5 animate-spin" /> : <RotateCwSquare className="size-3.5" />} Rotate
                    </Button>
                )}
                {wallpaper && (
                    <Button variant="ghost" size="sm" className="h-6 px-2 text-xs" onClick={() => void setAsWallpaper()} title="Set as desktop background">
                        <Wallpaper className="size-3.5" /> Wallpaper
                    </Button>
                )}
            </div>
        </Centered>
    );
}

type MediaKind = "image" | "video" | "audio" | "pdf";

/** A picture, video, sound or PDF, from a file on disk. */
function Media({ entry, kind, src }: { entry: FileEntry; kind: MediaKind; src: string }) {
    if (kind === "image") return <ImagePreview entry={entry} src={src} />;
    if (kind === "video")
        return (
            <Centered className="p-3">
                <video src={src} controls className="max-h-full max-w-full rounded-sm" />
            </Centered>
        );
    if (kind === "audio") return <AudioPreview entry={entry} src={src} />;
    return <PdfPreview entry={entry} src={src} />;
}

/** A song with its cover, when it has one, over its player. */
function AudioPreview({ entry, src }: { entry: FileEntry; src: string }) {
    const cover = useMediaDetails(entry, !isArchived(entry))?.cover;
    return (
        <Centered>
            {cover ? <img src={cover} alt="Cover" draggable={false} className="size-44 rounded-md object-cover shadow-md" /> : <BigIcon entry={entry} />}
            <audio src={src} controls className="w-full max-w-sm" />
        </Centered>
    );
}

/**
 * A PDF in WebView2's viewer, from a copy in memory that is one: the asset protocol serves a file as
 * what its bytes look like, so a ".pdf" that's really a web page would run its scripts, which could
 * read any file.
 */
function PdfPreview({ entry, src }: { entry: FileEntry; src: string }) {
    const [pdf, setPdf] = useState<{ url: string } | { error: string } | null>(null);
    // ponytail: the copy is whole in memory, so a huge scan isn't previewed; streaming it would need
    // a check that can't change between reads (a network share can answer differently each time).
    const tooBig = entry.size > 256 * 1024 * 1024;
    useEffect(() => {
        if (tooBig) return;
        let stale = false;
        let url = "";
        fetch(src)
            .then(async (res) => {
                if (!res.headers.get("content-type")?.startsWith("application/pdf")) throw new Error("This file isn't a PDF inside, so it isn't shown.");
                const blob = await res.blob();
                if (!stale) setPdf({ url: (url = URL.createObjectURL(blob)) });
            })
            .catch((e) => !stale && setPdf({ error: e instanceof Error ? e.message : String(e) }));
        return () => {
            stale = true;
            if (url) URL.revokeObjectURL(url);
        };
    }, [src, tooBig]);
    if (tooBig) return <BigIcon entry={entry} note="Too big to preview here: open it to read it." />;
    if (!pdf) return <Waiting />;
    if ("error" in pdf) return <BigIcon entry={entry} note={pdf.error} />;
    return <iframe src={pdf.url} title={entry.name} className="h-full w-full bg-white" />;
}

/** Media in an archive: shown from a temp copy extracted for the preview (once, and within a size budget). */
function ArchivedMedia({ entry, kind }: { entry: FileEntry; kind: MediaKind }) {
    const [copy, setCopy] = useState<{ path: string } | { error: string } | null>(null);
    useEffect(() => {
        let stale = false;
        archiveExtract(entry.path, "preview").then(
            (path) => !stale && setCopy({ path }),
            (e) => !stale && setCopy({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified]);
    if (!copy) return <Waiting what={isRemote(entry) ? "Downloading…" : "Extracting from the archive…"} />;
    if ("error" in copy) return <BigIcon entry={entry} note={copy.error} />;
    return <Media entry={entry} kind={kind} src={convertFileSrc(copy.path)} />;
}

/** An archive: what's in it, from its list of entries (nothing is extracted, and a big compressed tar isn't read until it's opened). */
function ArchivePreview({ entry }: { entry: FileEntry }) {
    const [summary, setSummary] = useState<ArchivePeek | { error: string } | null>(null);
    useEffect(() => {
        let stale = false;
        archivePeek(entry.path).then(
            (peek) => !stale && setSummary(peek),
            (e) => !stale && setSummary({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified]);
    return (
        <Centered>
            <FileArchive className="size-16 stroke-[1.25] text-amber-600 dark:text-amber-500" />
            {summary &&
                ("error" in summary ? (
                    <p className="max-w-sm">{summary.error}</p>
                ) : summary.entries === null || summary.unpacked === null ? (
                    // A big compressed tar keeps no list of what's in it: it's read when opened, not on selection.
                    <p className="max-w-sm">
                        {formatSize(summary.packed)} as a {summary.format} file
                        <span className="block text-xs text-muted-foreground">Opening it reads it from start to end once, to list what's in it.</span>
                    </p>
                ) : (
                    <>
                        <p>
                            {formatCount(summary.entries)} {summary.entries === 1 ? "file or folder" : "files and folders"}
                        </p>
                        <p className="text-foreground">
                            <b className="font-semibold">{formatSize(summary.unpacked)}</b> when extracted
                            <span className="block text-xs text-muted-foreground">
                                {formatSize(summary.packed)} as a {summary.format} file
                            </span>
                        </p>
                    </>
                ))}
            <Button variant="outline" size="sm" onClick={() => navigate(entry.path)}>
                <FolderOpen /> Open like a folder
            </Button>
        </Centered>
    );
}

/** The start of a file as text, for its preview. */
function useText(entry: FileEntry) {
    const [state, setState] = useState<{ text: string; truncated: boolean; binary: boolean } | { error: string } | null>(null);
    useEffect(() => {
        let stale = false;
        readTextPreview(entry.path, TEXT_BYTES).then(
            (result) => !stale && setState(result),
            (e) => !stale && setState({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified]);
    return state;
}

const SHOWS_START = "Preview shows the beginning of the file. Open it to see the rest.";

function TextPreview({ entry, over }: { entry: FileEntry; over: boolean }) {
    const showLineNumbers = useStore((s) => s.settings.showLineNumbers);
    const state = useText(entry);

    if (!state) return <Waiting />;
    if ("error" in state) return <BigIcon entry={entry} note={state.error} />;
    // A file of a kind Glint doesn't know, that isn't text: Windows' previewer for it, if it has one, or its bytes.
    if (state.binary)
        return isArchived(entry) ? (
            <HexView entry={entry} />
        ) : (
            <ShellPreview entry={entry} over={over} quiet>
                <HexView entry={entry} />
            </ShellPreview>
        );
    if (!state.text) return <Centered>This file is empty.</Centered>;
    const lines = state.text.split("\n");
    const cut = lines.length > TEXT_LINES;
    // An ncdu export (`ncdu -o`) opens as the folders it describes.
    const ncdu = !isArchived(entry) && NCDU_EXPORT.test(state.text);
    return (
        <div className="min-h-full select-text">
            {ncdu && (
                <div className="flex items-center gap-3 border-b px-4 py-2 text-xs text-muted-foreground">
                    <span className="min-w-0 flex-1">An ncdu export: its folders and their sizes can be browsed.</span>
                    <Button variant="outline" size="sm" onClick={() => navigate(entry.path)}>
                        <FolderOpen /> Open like a folder
                    </Button>
                </div>
            )}
            <SyntaxHighlighter
                language={languageFor(entry.ext)}
                useInlineStyles={false}
                showLineNumbers={showLineNumbers}
                wrapLines
                lineProps={{ className: "block pl-3 pr-4" }}
                className="min-w-max py-2 font-mono text-[0.8125rem] leading-5"
            >
                {cut ? lines.slice(0, TEXT_LINES).join("\n") : state.text}
            </SyntaxHighlighter>
            {(cut || state.truncated) && <p className="px-4 pb-3 text-xs text-muted-foreground">{SHOWS_START}</p>}
        </div>
    );
}

/** Markdown as it reads, its pictures and links relative to its folder. */
function MarkdownPreview({ entry }: { entry: FileEntry }) {
    const state = useText(entry);
    if (!state) return <Waiting />;
    if ("error" in state) return <BigIcon entry={entry} note={state.error} />;
    if (state.binary) return <HexView entry={entry} />;
    if (!state.text.trim()) return <Centered>This file is empty.</Centered>;
    return (
        <div className="min-h-full select-text px-5 py-4">
            <MarkdownView text={state.text} file={entry.path} archived={isArchived(entry)} />
            {state.truncated && <p className="mt-2 text-xs text-muted-foreground">{SHOWS_START}</p>}
        </div>
    );
}

/**
 * A file in an archive that can't be read: password-protected, or why not as the archive says (a method, a
 * volume or cabinet that isn't there, a link UnRAR doesn't pass on…), asked of the backend, which knows.
 */
function LockedPreview({ entry }: { entry: FileEntry }) {
    const encrypted = (entry.attrs & ATTR.encrypted) !== 0;
    const [reason, setReason] = useState<string | null>(null);
    useEffect(() => {
        if (encrypted) return;
        let stale = false;
        readTextPreview(entry.path, 1).then(
            () => {},
            (e) => !stale && setReason(String(e))
        );
        return () => {
            stale = true;
        };
    }, [entry.path, encrypted]);
    return (
        <Centered>
            <LockMark entry={entry} className="size-14 stroke-[1.25] text-amber-600 dark:text-amber-500" />
            <p className="max-w-sm">{encrypted ? "This file is password-protected, and Glint can't open encrypted files in archives yet." : (reason ?? "Glint can't read this file from the archive.")}</p>
        </Centered>
    );
}

function FolderPreview({ entry }: { entry: FileEntry }) {
    // A folder in the Recycle Bin comes with its size, as Windows noted it.
    const size = useExplorer((s) => s.sizes[parentPath(entry.path)]?.sizes.get(entry.name)) ?? (isRecycled(entry) ? entry.size : undefined);
    const [counts, setCounts] = useState<{ folders: number; files: number; bytes: number; repo: boolean } | { error: string } | null>(null);
    useEffect(() => {
        let stale = false;
        listDir(entry.path).then(
            ({ entries }) => {
                if (stale) return;
                const files = entries.filter((e) => !e.dir);
                // A repository's own folder has its .git (a file in a worktree): its card shows.
                setCounts({ folders: entries.length - files.length, files: files.length, bytes: files.reduce((sum, e) => sum + e.size, 0), repo: entries.some((e) => e.lower === ".git") });
            },
            (e) => !stale && setCounts({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified]);

    return (
        <Centered>
            <FolderOpen className="size-16 fill-amber-400/80 stroke-[1.25] text-amber-500" />
            {counts &&
                ("error" in counts ? (
                    <p>{counts.error}</p>
                ) : (
                    <p>
                        {counts.folders + counts.files === 0
                            ? "Empty"
                            : [counts.folders > 0 && plural(counts.folders, "folder"), counts.files > 0 && plural(counts.files, "file")].filter(Boolean).join(", ")}
                        {counts.files > 0 && <span className="block text-xs">{formatSize(counts.bytes)} in files directly inside</span>}
                    </p>
                ))}
            {size !== undefined ? (
                <p className="text-foreground">
                    <b className="font-semibold">{formatSize(size)}</b> in total
                </p>
            ) : (
                parentPath(entry.path) === currentPath() && (
                    <Button variant="outline" size="sm" onClick={calcSizes}>
                        <HardDrive /> Calculate folder sizes
                    </Button>
                )
            )}
            {counts && "repo" in counts && counts.repo && !isRecycled(entry) && <RepoCard path={entry.path} />}
        </Centered>
    );
}

function Details({ entry }: { entry: FileEntry }) {
    // A photo's camera, exposure, date taken and place, a song's artist and album…, read from the file.
    const media = useMediaDetails(entry, !isArchived(entry));
    const place = media?.place;
    // In the Recycle Bin, its date is when it was deleted, and where it was is where it goes back to.
    const recycled = isRecycled(entry);
    const rows: [string, string][] = [
        ...(media?.rows ?? []),
        ["Type", typeLabel(entry)],
        ...(!entry.dir || recycled ? [["Size", `${formatSize(entry.size)} (${formatCount(entry.size)} bytes)`] as [string, string]] : []),
        [recycled ? "Deleted" : "Modified", formatFullDate(entry.modified)],
        [recycled ? "Deleted from" : "Location", recycled ? (entry.origin ?? "") : parentPath(entry.path)],
    ];
    return (
        <dl className="grid shrink-0 grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 border-t px-4 py-3 text-xs">
            {rows.map(([label, value]) => (
                <div key={label} className="contents">
                    <dt className="text-muted-foreground">{label}</dt>
                    <dd className="select-text truncate" title={value}>
                        {label === "Place" && place ? (
                            <button type="button" onClick={() => void openUrl(mapLink(place))} title="Show it on OpenStreetMap" className="text-primary hover:underline">
                                {value}
                            </button>
                        ) : (
                            value
                        )}
                    </dd>
                </div>
            ))}
        </dl>
    );
}

/** One item's preview with its name, actions and details; Quick Look shows it large, with a close button. */
export function EntryPreview({ entry, onClose }: { entry: FileEntry; onClose?: () => void }) {
    const kind = previewKind(entry);
    const archived = isArchived(entry);
    const archive = archived ? archiveOf() : undefined;
    const readable = !entry.dir && !isOnlineOnly(entry) && !isClosed(entry) && !isLocked(entry) && !isListOnly(entry);
    const markdown = readable && isMarkdown(entry);
    // JSON, YAML and TOML as a tree (up to the 64 MB the jq worker reads).
    const data = readable && entry.size <= 64 * 1024 * 1024 ? dataFormat(entry) : undefined;
    // Workbooks and Parquet files as tables, from disk; a workbook can show in Windows' previewer too (Excel's).
    const sheet = readable && !archived && isSheetCandidate(entry);
    const workbook = sheet && entry.ext !== "parquet";
    // asciinema recordings play, or show as the JSON lines they are.
    const cast = readable && entry.ext === "cast";
    // Tables and databases take SQL, from disk.
    const queryable = readable && !archived && (sheet || isTabularCandidate(entry) || isSqliteCandidate(entry));
    // A file Git has a change for can show what changed since its last commit instead.
    const mark = useGitMark(entry.path);
    const changed = !entry.dir && !archived && !!mark && mark !== "ignored";
    const views: { view: View; label: string; title: string }[] = [
        markdown
            ? { view: "file", label: "Rendered", title: "The Markdown as it reads" }
            : data
              ? { view: "file", label: "Tree", title: "Its values as a tree, with a jq filter" }
              : sheet
                ? { view: "file", label: "Table", title: "Its rows, read by Glint" }
                : cast
                  ? { view: "file", label: "Play", title: "The recording, played" }
                  : { view: "file", label: "File", title: "The file as it is" },
        ...(markdown ? [{ view: "source" as const, label: "Source", title: "The Markdown as written" }] : []),
        ...(data || cast ? [{ view: "source" as const, label: "Text", title: "The file as written" }] : []),
        ...(workbook ? [{ view: "source" as const, label: "Previewer", title: "As Windows' previewer for it shows it (Excel's, with Office installed)" }] : []),
        ...(changed ? [{ view: "changes" as const, label: "Changes", title: "What changed since the last commit (Git)" }] : []),
        ...(queryable ? [{ view: "query" as const, label: "Query", title: "SQL over it, reading only" }] : []),
        ...(readable ? [{ view: "hex" as const, label: "Hex", title: "Its bytes, in hexadecimal" }] : []),
    ];
    const chosen = usePreviewView((s) => s.view);
    const view = views.some((v) => v.view === chosen) ? chosen : "file";
    let body: React.ReactNode;
    if (view === "changes") body = <ChangesView entry={entry} />;
    else if (view === "hex") body = <HexView entry={entry} />;
    else if (view === "query") body = <QueryView entry={entry} />;
    else if (entry.dir) body = <FolderPreview entry={entry} />;
    else if (isOnlineOnly(entry))
        body = (
            <Centered>
                <Cloud className="size-14 stroke-[1.25] text-sky-500" />
                <p>This file is online-only. Opening it downloads it.</p>
            </Centered>
        );
    else if (isClosed(entry))
        body = (
            <Centered>
                <FolderLock className="size-14 stroke-[1.25] text-amber-600 dark:text-amber-500" />
                <p className="max-w-sm">Windows keeps the folder this file is in closed to your account, so it can't be previewed or opened. Get access to the folder first.</p>
            </Centered>
        );
    else if (isLocked(entry)) body = <LockedPreview entry={entry} />;
    else if (isListOnly(entry)) body = <BigIcon entry={entry} note="This file is listed in an ncdu export, with its size and date. What's in it isn't there." />;
    else if (isArchiveFile(entry)) body = <ArchivePreview entry={entry} />;
    // In an archive, pictures, video, sound and PDFs are shown from a temp copy; text is read straight out of it.
    else if (kind === "image" || kind === "video" || kind === "audio" || kind === "pdf")
        body = archived ? <ArchivedMedia entry={entry} kind={kind} /> : <Media entry={entry} kind={kind} src={convertFileSrc(entry.path)} />;
    // Databases and tables are read from disk, so not from inside an archive (there they show as text).
    else if (!archived && isSqliteCandidate(entry)) body = <SqlitePreview entry={entry} />;
    else if (!archived && isTabularCandidate(entry)) body = <TabularPreview entry={entry} />;
    else if (sheet)
        body =
            view === "source" ? (
                <ShellPreview entry={entry} over={!!onClose}>
                    <BigIcon entry={entry} />
                </ShellPreview>
            ) : (
                <SheetPreview entry={entry} />
            );
    else if (markdown) body = view === "source" ? <TextPreview entry={entry} over={!!onClose} /> : <MarkdownPreview entry={entry} />;
    else if (data) body = view === "source" ? <TextPreview entry={entry} over={!!onClose} /> : <DataTree entry={entry} format={data} />;
    else if (cast) body = view === "source" ? <TextPreview entry={entry} over={!!onClose} /> : <CastPreview entry={entry} />;
    // Unknown extensions get a text preview attempt; the backend spots binary files.
    else if (kind === "text" || !isKnownType(entry)) body = <TextPreview entry={entry} over={!!onClose} />;
    else if (!archived && isPeCandidate(entry)) body = <BinaryPreview entry={entry} />;
    // Word, Excel, PowerPoint, fonts…: Windows' own previewer, where there's one.
    else if (!archived)
        body = (
            <ShellPreview entry={entry} over={!!onClose}>
                <BigIcon entry={entry} />
            </ShellPreview>
        );
    else body = <BigIcon entry={entry} />;

    const closed = isClosed(entry);
    // In the Recycle Bin it's restored rather than opened, and File Explorer would show the bin.
    const recycled = isRecycled(entry);
    const open = () => (archived || entry.dir || closed ? openEntry(entry) : openFile(entry.path));
    // What's in an archive isn't a place File Explorer shows here: it shows the archive.
    const reveal = () => (archive ? revealItemInDir(archive.outer) : revealItemInDir(entry.path));

    return (
        <div className="flex min-w-0 flex-1 flex-col">
            <div className="flex h-10 shrink-0 items-center gap-2 border-b pl-3 pr-1.5">
                <span className="truncate text-sm font-medium" title={entry.path}>
                    {entry.name}
                </span>
                {views.length > 1 && (
                    <div role="group" aria-label="Show" className="ml-auto flex shrink-0 rounded-md bg-foreground/[0.05] p-0.5">
                        {views.map((choice) => (
                            <button
                                key={choice.view}
                                type="button"
                                aria-pressed={view === choice.view}
                                title={choice.title}
                                onClick={() => usePreviewView.setState({ view: choice.view })}
                                className={cn(
                                    "h-5 rounded-[4px] px-1.5 text-[0.6875rem] font-medium text-muted-foreground transition-colors hover:text-foreground",
                                    view === choice.view && "bg-background text-primary shadow-xs dark:bg-foreground/[0.12]"
                                )}
                            >
                                {choice.label}
                            </button>
                        ))}
                    </div>
                )}
                <div className={cn("flex shrink-0", views.length < 2 && "ml-auto")}>
                    {!entry.dir && !archived && !closed && !recycled && (
                        <Button variant="ghost" size="icon" className="size-7" onClick={() => inspectXRay(entry.path)} title="File X-Ray: hashes and hidden streams">
                            <Fingerprint />
                        </Button>
                    )}
                    {recycled ? (
                        <Button variant="ghost" size="icon" className="size-7" onClick={() => void restoreRecycled([entry])} title={`Restore: back into ${entry.origin ?? "the folder it was deleted from"}`}>
                            <RotateCcw />
                        </Button>
                    ) : (
                        <>
                            {/* A remote's file isn't anywhere File Explorer shows. */}
                            {!isRemote(entry) && (
                                <Button variant="ghost" size="icon" className="size-7" onClick={reveal} title={archive ? "Show the archive in File Explorer" : "Show in File Explorer"}>
                                    <FolderSearch />
                                </Button>
                            )}
                            <Button variant="ghost" size="icon" className="size-7" onClick={open} title="Open (Enter)">
                                <ExternalLink />
                            </Button>
                        </>
                    )}
                    {onClose && (
                        <Button variant="ghost" size="icon" className="size-7" onClick={onClose} title="Close (Space or Esc)" aria-label="Close">
                            <X />
                        </Button>
                    )}
                </div>
            </div>
            <div className="min-h-0 flex-1 overflow-auto bg-card">{body}</div>
            <Details entry={entry} />
        </div>
    );
}

/** Waits briefly so arrowing quickly through a list doesn't load every file on the way. */
function useSettled<T>(value: T, ms = 70) {
    const [settled, setSettled] = useState(value);
    useEffect(() => {
        const timer = setTimeout(() => setSettled(value), ms);
        return () => clearTimeout(timer);
    }, [value, ms]);
    return settled;
}

interface FolderSummary {
    path: string;
    items: number;
    /** Everything inside, once sized (by the index or pdu). */
    total?: number;
    /** Windows keeps this account out of it: there's nothing listed to count. */
    noAccess?: boolean;
    /** A Git repository's own folder: its card shows. */
    repo?: boolean;
}

export function FilePreview({ selected, folder }: { selected: FileEntry[]; folder: FolderSummary }) {
    const entry = useSettled(selected.length === 1 ? selected[0] : null);
    if (selected.length > 1) {
        const files = selected.filter((e) => !e.dir);
        const deck = selected.slice(0, 3);
        const mtimes = selected.map((e) => e.modified).filter((m) => m > 0);
        const oldest = mtimes.length ? Math.min(...mtimes) : 0;
        const newest = mtimes.length ? Math.max(...mtimes) : 0;
        const dateSpan =
            oldest > 0 && newest > 0
                ? formatDate(oldest) === formatDate(newest)
                    ? formatDate(newest)
                    : `${formatDate(oldest)} – ${formatDate(newest)}`
                : null;
        const writable = !selected.some((e) => isArchived(e) || isRecycled(e) || isRemote(e) || isClosed(e));
        const canCompare = selected.length === 2 && files.length === 2 && writable;
        const fanTransforms =
            deck.length === 2
                ? ["-translate-x-3.5 translate-y-0.5 -rotate-6 z-0", "translate-x-3.5 translate-y-0.5 rotate-6 z-10"]
                : ["-translate-x-5 translate-y-1.5 -rotate-8 z-0", "z-20 -translate-y-0.5", "translate-x-5 translate-y-1.5 rotate-8 z-10"];
        return (
            <Centered className="gap-3 p-4">
                <div className="relative flex h-24 w-36 items-center justify-center">
                    {deck.map((item, idx) => (
                        <div
                            key={item.path}
                            className={cn(
                                "absolute flex size-18 items-center justify-center overflow-hidden rounded-lg border border-border/80 bg-card p-1.5 shadow-md transition-transform duration-200",
                                fanTransforms[idx],
                            )}
                        >
                            <div className="relative size-14">
                                <Thumbnail entry={item} box={56} lane="preview-deck" enabled iconSize={36} />
                            </div>
                        </div>
                    ))}
                </div>
                <div className="space-y-0.5 text-center">
                    <p className="font-medium text-foreground">{formatCount(selected.length)} items selected</p>
                    <p className="text-xs text-muted-foreground">
                        {[selected.length > files.length && plural(selected.length - files.length, "folder"), files.length > 0 && plural(files.length, "file")].filter(Boolean).join(", ")}
                        {files.length > 0 && ` · ${formatSize(files.reduce((sum, e) => sum + e.size, 0))}`}
                    </p>
                    {dateSpan && <p className="text-[11px] text-muted-foreground/80 tabular-nums">{dateSpan}</p>}
                </div>
                {writable && (
                    <div className="mt-1 flex flex-wrap items-center justify-center gap-1.5">
                        {canCompare && (
                            <Button variant="outline" size="sm" className="h-7 gap-1.5 px-2.5 text-xs" onClick={() => compareFiles(selected[0].path, selected[1].path)}>
                                <GitCompareArrows className="size-3.5" /> Compare
                            </Button>
                        )}
                        <Button variant="outline" size="sm" className="h-7 gap-1.5 px-2.5 text-xs" onClick={() => openBatchRename(selected)}>
                            <PenLine className="size-3.5" /> Rename
                        </Button>
                        <Button variant="outline" size="sm" className="h-7 gap-1.5 px-2.5 text-xs" onClick={() => void compressEntries(selected)}>
                            <FileArchive className="size-3.5" /> Zip
                        </Button>
                    </div>
                )}
            </Centered>
        );
    }
    if (folder.noAccess && !selected.length) {
        return (
            <Centered>
                <FolderLock className="size-14 stroke-[1.25] text-muted-foreground/60" />
                <p className="font-medium text-foreground">{baseName(folder.path) || folder.path}</p>
                <p>
                    {folder.items > 0 && `${formatCount(folder.items)} ${folder.items === 1 ? "item" : "items"} · `}No access
                </p>
            </Centered>
        );
    }
    if (!entry || !selected.length) {
        if (isRecycleBin(folder.path)) {
            return (
                <Centered>
                    <Trash2 className="size-14 stroke-[1.25] text-muted-foreground/70" />
                    <p className="font-medium text-foreground">Recycle Bin</p>
                    <p>
                        {formatCount(folder.items)} {folder.items === 1 ? "item" : "items"}
                        {!!folder.total && ` · ${formatSize(folder.total)} in total`}
                    </p>
                    <p className="text-xs">Select an item to look at it before you restore it</p>
                </Centered>
            );
        }
        if (isRecent(folder.path)) {
            return (
                <Centered>
                    <Clock className="size-14 stroke-[1.25] text-muted-foreground/70" />
                    <p className="font-medium text-foreground">Recent</p>
                    <p>
                        {formatCount(folder.items)} {folder.items === 1 ? "item" : "items"} opened lately
                    </p>
                    <p className="text-xs">Select a file to preview it</p>
                </Centered>
            );
        }
        return (
            <Centered>
                <FolderOpen className="size-14 fill-amber-400/80 stroke-[1.25] text-amber-500" />
                <p className="font-medium text-foreground">{baseName(folder.path) || "This PC"}</p>
                <p>
                    {formatCount(folder.items)} {folder.items === 1 ? "item" : "items"}
                    {folder.total !== undefined && ` · ${formatSize(folder.total)} in total`}
                </p>
                <p className="text-xs">Select a file to preview it</p>
                {folder.repo && <RepoCard path={folder.path} />}
            </Centered>
        );
    }
    return <EntryPreview key={entry.path} entry={entry} />;
}
