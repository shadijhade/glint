import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { toast } from "sonner";
import { type FileEntry, isArchived } from "@/lib/entries";
import { formatCount, formatSize } from "@/lib/format";
import { archiveExtract, fileBytes } from "@/lib/fs";
import { byteChar, byteClass, hex, parseOffset } from "@/lib/hex";
import { cn } from "@/lib/utils";
import { VirtualList } from "../VirtualList";
import { Button } from "../ui/button";
import { BigIcon, Centered, Waiting } from "./PreviewState";

const LINE = 20;
const WINDOW = 64 * 1024;
/** Rows a page holds: a browser can't make a list taller than about 33 million pixels, so a big file is read a page at a time. */
const PAGE_ROWS = 1 << 20;

/**
 * A file's bytes as hexyl shows them: offsets, hexadecimal and text, colored by kind (zero, ASCII text,
 * whitespace, other ASCII, the rest), read 64 KB at a time as it scrolls. Click a byte, or go to an
 * offset, to see it as numbers; arrows move.
 */
export function HexView({ entry }: { entry: FileEntry }) {
    const archived = isArchived(entry);
    // In an archive, the bytes are read from a copy extracted for the preview.
    const [source, setSource] = useState<string | { error: string } | null>(archived ? null : entry.path);
    useEffect(() => {
        if (!archived) return setSource(entry.path);
        let stale = false;
        setSource(null);
        archiveExtract(entry.path, "preview").then(
            (copy) => !stale && setSource(copy),
            (e) => !stale && setSource({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified, archived]);
    const path = typeof source === "string" ? source : null;

    const windows = useRef(new Map<number, Uint8Array | "loading">());
    const [, loadedOne] = useReducer((n: number) => n + 1, 0);
    const [size, setSize] = useState<number | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [at, setAt] = useState(-1);
    const [cols, setCols] = useState(16);
    const [typed, setTyped] = useState("");
    const root = useRef<HTMLDivElement>(null);

    const load = useCallback(
        (n: number) => {
            if (!path || windows.current.has(n)) return;
            windows.current.set(n, "loading");
            fileBytes(path, n * WINDOW, WINDOW).then(
                (read) => {
                    windows.current.set(n, read.bytes);
                    setSize(read.size);
                    loadedOne();
                },
                (e) => setError(String(e))
            );
        },
        [path]
    );
    useEffect(() => {
        windows.current = new Map();
        setSize(null);
        setError(null);
        setAt(-1);
        load(0);
    }, [load, entry.modified]);

    if (source && typeof source !== "string") return <BigIcon entry={entry} note={source.error} />;
    if (error) return <BigIcon entry={entry} note={error} />;
    if (size === null) return <Waiting />;
    if (size === 0) return <Centered>This file is empty.</Centered>;

    const byteAt = (offset: number) => {
        const w = windows.current.get(Math.floor(offset / WINDOW));
        return w instanceof Uint8Array ? w[offset % WINDOW] : undefined;
    };
    const pageBytes = PAGE_ROWS * cols;
    const base = Math.floor(Math.max(at, 0) / pageBytes) * pageBytes;
    const rows = Math.ceil(Math.min(size - base, pageBytes) / cols);
    const move = (to: number) => setAt(Math.max(0, Math.min(size - 1, to)));
    const go = () => {
        const offset = parseOffset(typed);
        if (offset === null || offset >= size) return void toast.error(offset === null ? "Type an offset: 0x1F00, or 8192" : `The file ends at 0x${hex(size - 1, 8)}`, { id: "hex-go" });
        move(offset);
        // Arrows then move from there.
        requestAnimationFrame(() => root.current?.querySelector<HTMLElement>("[role=table]")?.focus());
    };
    const onKey = (e: React.KeyboardEvent) => {
        const page = Math.max(1, Math.floor((e.currentTarget.clientHeight || 400) / LINE) - 1) * cols;
        const from = Math.max(at, 0);
        const step: Record<string, number> = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: cols, ArrowUp: -cols, PageDown: page, PageUp: -page };
        if (e.key in step) move(from + step[e.key]);
        else if (e.key === "Home") move(e.ctrlKey ? 0 : from - (from % cols));
        else if (e.key === "End") move(e.ctrlKey ? size - 1 : from - (from % cols) + cols - 1);
        else return;
        e.preventDefault();
    };

    const value = at >= 0 ? byteAt(at) : undefined;
    const word = (n: number) => {
        let total = 0;
        for (let k = n - 1; k >= 0; k--) {
            const b = byteAt(at + k);
            if (b === undefined) return null;
            total = total * 256 + b;
        }
        return total;
    };
    const [u16, u32] = [word(2), word(4)];

    return (
        <div ref={root} className="flex h-full flex-col">
            <div className="flex h-9 shrink-0 items-center gap-2 border-b px-2 text-xs text-muted-foreground">
                <input
                    value={typed}
                    onChange={(e) => setTyped(e.target.value)}
                    onKeyDown={(e) => {
                        if (e.key !== "Enter") return;
                        e.preventDefault();
                        go();
                    }}
                    placeholder="Go to 0x1F00 or 8192"
                    spellCheck={false}
                    aria-label="Go to offset"
                    className="fluent-field h-7 w-40 min-w-0 px-2 font-mono text-xs outline-hidden"
                />
                <span className="truncate">
                    {formatSize(size)}
                    {size >= 1024 && ` (${formatCount(size)} bytes)`}
                </span>
                {size > pageBytes && (
                    <span className="ml-auto flex shrink-0 items-center gap-0.5">
                        <Button variant="ghost" size="icon" className="size-6" disabled={base === 0} onClick={() => move(base - pageBytes)} title="The part before">
                            <ChevronLeft />
                        </Button>
                        <span className="font-mono tabular-nums">
                            {hex(base, 8)}–{hex(Math.min(size, base + pageBytes) - 1, 8)}
                        </span>
                        <Button variant="ghost" size="icon" className="size-6" disabled={base + pageBytes >= size} onClick={() => move(base + pageBytes)} title="The part after">
                            <ChevronRight />
                        </Button>
                    </span>
                )}
            </div>
            <VirtualList
                key={base}
                count={rows}
                rowHeight={LINE}
                scrollToIndex={at >= base ? Math.floor((at - base) / cols) : -1}
                onWidth={(width) => setCols(width >= 540 ? 16 : 8)}
                tabIndex={0}
                onKeyDown={onKey}
                onClick={(e) => {
                    const picked = (e.target as HTMLElement).closest<HTMLElement>("[data-at]")?.dataset.at;
                    if (picked) setAt(Number(picked));
                }}
                className="min-h-0 flex-1 overflow-auto px-3 py-1 font-mono text-xs leading-5 outline-hidden select-text"
                role="table"
                aria-label="Bytes"
            >
                {(i) => {
                    const offset = base + i * cols;
                    const n = Math.floor(offset / WINDOW);
                    const w = windows.current.get(n);
                    if (!(w instanceof Uint8Array)) {
                        load(n);
                        return <div className="h-5 text-muted-foreground/50">{hex(offset, 8)}</div>;
                    }
                    const row = w.subarray(offset - n * WINDOW, offset - n * WINDOW + cols);
                    return (
                        <div role="row" className="flex h-5 gap-3 whitespace-pre tabular-nums">
                            <span className="w-[8ch] shrink-0 select-none text-muted-foreground/70">{hex(offset, 8)}</span>
                            <span className="flex shrink-0">
                                {Array.from({ length: cols }, (_, k) => (
                                    <span
                                        key={k}
                                        data-at={k < row.length ? offset + k : undefined}
                                        className={cn("w-[2.5ch] text-center", k === 8 && "ml-[1ch]", k < row.length && `hx-${byteClass(row[k])}`, offset + k === at && "hx-at")}
                                    >
                                        {k < row.length ? hex(row[k], 2) : "  "}
                                    </span>
                                ))}
                            </span>
                            <span className="flex shrink-0">
                                {Array.from(row, (b, k) => (
                                    <span key={k} data-at={offset + k} className={cn(`hx-${byteClass(b)}`, offset + k === at && "hx-at")}>
                                        {byteChar(b)}
                                    </span>
                                ))}
                            </span>
                        </div>
                    );
                }}
            </VirtualList>
            <div className="flex h-8 shrink-0 items-center gap-3 truncate border-t px-3 font-mono text-xs text-muted-foreground">
                {at >= 0 && value !== undefined ? (
                    <>
                        <span className="text-foreground">
                            0x{hex(at, 8)} <span className="text-muted-foreground">({formatCount(at)})</span>
                        </span>
                        <span>
                            {hex(value, 2)} = {value}
                            {value > 0x20 && value < 0x7f && ` = '${String.fromCharCode(value)}'`}
                        </span>
                        {u16 !== null && <span title="The two bytes from here, little-endian">u16 {formatCount(u16)}</span>}
                        {u32 !== null && <span title="The four bytes from here, little-endian">u32 {formatCount(u32)}</span>}
                    </>
                ) : (
                    <span className="font-sans">Click a byte, or go to an offset</span>
                )}
            </div>
        </div>
    );
}
