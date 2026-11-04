import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";
import { toast } from "sonner";
import { convertFileSrc } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import {
    AlertTriangle,
    ArrowLeft,
    ArrowLeftRight,
    ArrowRight,
    Binary,
    CaseSensitive,
    CheckCircle2,
    ChevronDown,
    ChevronUp,
    Download,
    FileText,
    GitCompareArrows,
    Image as ImageIcon,
    Loader2,
    RotateCcw,
    Save,
    Search,
    Sparkles,
    UnfoldVertical,
    X,
    ZoomIn,
    ZoomOut,
} from "lucide-react";
import { withDetails } from "@/lib/entries";
import { kindOf } from "@/lib/fileTypes";
import { formatCount, formatDuration, formatSize, plural } from "@/lib/format";
import {
    type BytesWindow,
    compareBytes,
    compareTextFiles,
    type CompareTextOptions,
    nextByteDifference,
    prevByteDifference,
    type TextCompare,
    writeFileContent,
} from "@/lib/fs";
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
    readOnly?: boolean;
}

interface CompareState {
    left: CompareSide | null;
    right: CompareSide | null;
    /** Run once the comparison closes (a temp copy is removed). */
    onClose?: () => void;
}

const useCompareStore = create<CompareState>(() => ({ left: null, right: null }));

/** Whether the text view has edits not saved yet, which closing it (or Swap, or another view) would lose. */
let unsaved = false;

/** Asks before edits not saved yet are lost: true to go ahead (there are none, or Discard was chosen). */
export async function discardCompareEdits() {
    if (!unsaved) return true;
    const discard = await confirm("The edits made in Compare aren't saved. Discard them?", { title: "Unsaved edits", kind: "warning", okLabel: "Discard", cancelLabel: "Keep editing" });
    if (discard) unsaved = false;
    return discard;
}

/** Opens two files side by side: text (by Difftastic when it's installed), pictures, or bytes. */
export function compareFiles(left: string | CompareSide, right: string | CompareSide, onClose?: () => void) {
    const side = (s: string | CompareSide) => (typeof s === "string" ? { path: s } : s);
    void discardCompareEdits().then((go) => {
        if (!go) return onClose?.();
        useCompareStore.getState().onClose?.();
        useCompareStore.setState({ left: side(left), right: side(right), onClose });
    });
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
    // Where they are: each folder once, leaving out a read-only temp copy.
    const folders = [...new Set([a, b].filter((side) => !side.readOnly).map((side) => parentPath(side.path)))];
    const subtitle = (folders.length ? folders : [parentPath(a.path)]).join("  ·  ");
    // Closing, Swap and another view start the text view over: edits not saved yet are asked about first.
    const close = useCallback(() => void discardCompareEdits().then((go) => go && onClose()), [onClose]);
    const unlessEdited = (then: () => void) => void discardCompareEdits().then((go) => go && then());

    return (
        <Modal label={`Compare ${nameOf(left)} and ${nameOf(right)}`} onClose={close} className="h-[min(92vh,64rem)] max-h-none max-w-[min(96vw,120rem)]">
            <ModalHeader icon={GitCompareArrows} title={`Compare “${nameOf(a)}” and “${nameOf(b)}”`} subtitle={subtitle} onClose={close}>
                <div role="group" aria-label="Compare as" className="flex shrink-0 rounded-md bg-foreground/[0.05] p-0.5">
                    {views.map((v) => (
                        <button
                            key={v.key}
                            type="button"
                            aria-pressed={view === v.key}
                            onClick={() => v.key !== view && unlessEdited(() => setView(v.key))}
                            className={cn(
                                "flex h-6 items-center gap-1 rounded-[4px] px-2 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground",
                                view === v.key && "bg-background text-primary shadow-xs dark:bg-foreground/[0.12]"
                            )}
                        >
                            <v.icon className="size-3.5" /> {v.label}
                        </button>
                    ))}
                </div>
                <Button variant="ghost" size="sm" className="h-7 shrink-0 px-2 text-xs" onClick={() => unlessEdited(() => setSwapped((s) => !s))} title="Swap the sides">
                    <ArrowLeftRight className="size-3.5" /> Swap
                </Button>
            </ModalHeader>
            {view !== "text" && (
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
            )}
            <div className="flex min-h-0 flex-1 flex-col">
                {view === "text" ? (
                    <TextView key={`${a.path}|${b.path}`} left={a} right={b} onBytes={() => setView("bytes")} />
                ) : view === "pictures" ? (
                    <PicturesView left={a} right={b} />
                ) : (
                    <BytesView left={a} right={b} />
                )}
            </div>
        </Modal>
    );
}

/** `text` with the ranges in `spans` marked: what changed in the line, plus any search query hits. */
function Marked({ text, spans, mark, searchSpans }: { text: string; spans: [number, number][]; mark: string; searchSpans?: [number, number][] }) {
    const items: { start: number; end: number; cls: string }[] = [
        ...spans.map(([start, end]) => ({ start, end, cls: mark })),
        ...(searchSpans ?? []).map(([start, end]) => ({ start, end, cls: "bg-amber-400/55 ring-1 ring-amber-500/50 dark:bg-amber-400/40" })),
    ].sort((x, y) => x.start - y.start || y.end - x.end);
    const parts: React.ReactNode[] = [];
    let at = 0;
    for (const { start, end, cls } of items) {
        if (end <= at) continue;
        const from = Math.max(start, at);
        if (from > at) parts.push(text.slice(at, from));
        parts.push(
            <mark key={`${from}-${end}`} className={cn("rounded-[2px] text-inherit", cls)}>
                {text.slice(from, end)}
            </mark>
        );
        at = end;
    }
    if (at < text.length) parts.push(text.slice(at));
    return <>{parts}</>;
}

function findSpansInLine(text: string, query: string): [number, number][] {
    if (!query) return [];
    const hay = text.toLowerCase();
    const needle = query.toLowerCase();
    const out: [number, number][] = [];
    let idx = 0;
    while (idx <= hay.length - needle.length) {
        const found = hay.indexOf(needle, idx);
        if (found < 0) break;
        out.push([found, found + needle.length]);
        idx = found + Math.max(1, needle.length);
    }
    return out;
}

function Half({
    line,
    text,
    spans,
    searchQuery,
    side,
    editable,
    editing,
    onStartEdit,
    onCommitEdit,
    onCancelEdit,
}: {
    line: number | null;
    text?: string;
    spans?: [number, number][];
    searchQuery?: string;
    side: 0 | 1;
    editable: boolean;
    editing: boolean;
    onStartEdit: () => void;
    onCommitEdit: (value: string) => void;
    onCancelEdit: () => void;
}) {
    const [draft, setDraft] = useState(text ?? "");
    useEffect(() => {
        if (editing) setDraft(text ?? "");
    }, [editing, text]);

    if (line === null) return <div className="min-w-0 flex-1 bg-[repeating-linear-gradient(135deg,transparent_0_6px,var(--color-border)_6px_7px)] opacity-60" />;
    const changed = spans !== undefined;
    const tint = changed ? (side === 0 ? "bg-red-500/10" : "bg-emerald-500/10") : "";
    const mark = side === 0 ? "bg-red-500/35" : "bg-emerald-500/35";
    const searchSpans = searchQuery ? findSpansInLine(text ?? "", searchQuery) : undefined;
    return (
        <div
            className={cn("group/half flex min-w-0 flex-1 items-center", tint)}
            title={editable && !editing ? `${text ?? ""}\n(Double-click to edit line ${line + 1})` : text}
            onDoubleClick={editable && !editing ? onStartEdit : undefined}
        >
            <span className="w-12 shrink-0 select-none pr-2 text-right tabular-nums text-muted-foreground/70">{line + 1}</span>
            {editing ? (
                <input
                    type="text"
                    value={draft}
                    autoFocus
                    data-own-escape
                    onChange={(e) => setDraft(e.target.value)}
                    onBlur={() => onCommitEdit(draft)}
                    onKeyDown={(e) => {
                        if (e.key === "Enter") {
                            e.preventDefault();
                            onCommitEdit(draft);
                        } else if (e.key === "Escape") {
                            e.preventDefault();
                            onCancelEdit();
                        }
                        e.stopPropagation();
                    }}
                    className="h-5 min-w-0 flex-1 bg-background px-1 font-mono text-xs text-foreground outline outline-1 outline-primary"
                />
            ) : (
                <span className="min-w-0 flex-1 overflow-hidden whitespace-pre text-ellipsis pr-2 [tab-size:4]">
                    {spans?.length || searchSpans?.length ? <Marked text={text ?? ""} spans={spans ?? []} mark={mark} searchSpans={searchSpans} /> : text || " "}
                </span>
            )}
        </div>
    );
}

interface ChangeBlock {
    startRow: number;
    endRow: number;
    /** Line range `[start, end)` in `compare.left` covered by or insertion point for this block. */
    leftRange: [number, number];
    /** Line range `[start, end)` in `compare.right` covered by or insertion point for this block. */
    rightRange: [number, number];
}

type VisibleItem =
    | { kind: "row"; rowIndex: number; blockIndex: number; isBlockStart: boolean }
    | { kind: "fold"; startRow: number; endRow: number; count: number };

type RowFilter = "all" | "context" | "diffs";

function serializeLines(lines: string[], crlf: boolean, trailingNewline: boolean): string {
    const eol = crlf ? "\r\n" : "\n";
    const body = lines.join(eol);
    return trailingNewline && lines.length > 0 ? body + eol : body;
}

function TextView({ left, right, onBytes }: { left: CompareSide; right: CompareSide; onBytes: () => void }) {
    const [compare, setCompare] = useState<TextCompare | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [rowFilter, setRowFilter] = useState<RowFilter>("all");
    const [ignoreWhitespace, setIgnoreWhitespace] = useState<"none" | "trailing" | "all">("none");
    const [ignoreCase, setIgnoreCase] = useState(false);
    const [lineMode, setLineMode] = useState(false);
    const [expandedFolds, setExpandedFolds] = useState<Set<number>>(() => new Set());

    // Two-way merge & inline editing state (null means unmodified from disk).
    const [leftLines, setLeftLines] = useState<string[] | null>(null);
    const [rightLines, setRightLines] = useState<string[] | null>(null);
    const [history, setHistory] = useState<{ left: string[] | null; right: string[] | null }[]>([]);
    const [savingSide, setSavingSide] = useState<0 | 1 | null>(null);
    const [editingCell, setEditingCell] = useState<{ side: 0 | 1; line: number } | null>(null);
    const diskMeta = useRef<{ leftCrlf: boolean; rightCrlf: boolean; leftNewline: boolean; rightNewline: boolean } | null>(null);
    // Each file as last read from disk (or saved): a save over a file changed elsewhere since is refused.
    const diskHash = useRef<[string | null, string | null]>([null, null]);

    // Inline search state
    const [searchOpen, setSearchOpen] = useState(false);
    const [searchQuery, setSearchQuery] = useState("");
    const [searchIdx, setSearchIdx] = useState(0);
    const searchInputRef = useRef<HTMLInputElement>(null);

    const difftastic = useTool("difftastic");
    const installing = useTools((s) => !!s.busy.difftastic);

    const leftOverride = useMemo(
        () => (leftLines ? serializeLines(leftLines, diskMeta.current?.leftCrlf ?? false, diskMeta.current?.leftNewline ?? true) : undefined),
        [leftLines]
    );
    const rightOverride = useMemo(
        () => (rightLines ? serializeLines(rightLines, diskMeta.current?.rightCrlf ?? false, diskMeta.current?.rightNewline ?? true) : undefined),
        [rightLines]
    );

    const options = useMemo<CompareTextOptions>(
        () => ({ ignoreWhitespace, ignoreCase, lineMode }),
        [ignoreWhitespace, ignoreCase, lineMode]
    );

    const run = useCallback(() => {
        let stale = false;
        setError(null);
        compareTextFiles(left.path, right.path, options, leftOverride, rightOverride).then(
            (found) => {
                if (stale) return;
                if (!diskMeta.current) {
                    diskMeta.current = {
                        leftCrlf: found.leftCrlf,
                        rightCrlf: found.rightCrlf,
                        leftNewline: found.leftNewline,
                        rightNewline: found.rightNewline,
                    };
                }
                if (found.leftHash) diskHash.current[0] = found.leftHash;
                if (found.rightHash) diskHash.current[1] = found.rightHash;
                setCompare(found);
            },
            (e) => !stale && setError(String(e))
        );
        return () => {
            stale = true;
        };
    }, [left.path, right.path, options, leftOverride, rightOverride]);
    useEffect(run, [run]);

    // Once Difftastic is installed, they're compared again with it unless line mode or edits are active.
    const withDifftastic = !!difftastic?.origin;
    const engine = compare?.engine;
    useEffect(() => {
        if (withDifftastic && engine === "lines" && !lineMode && ignoreWhitespace === "none" && !ignoreCase && !leftLines && !rightLines) {
            run();
        }
    }, [withDifftastic, engine, lineMode, ignoreWhitespace, ignoreCase, leftLines, rightLines, run]);

    // Contiguous changed blocks with their corresponding line ranges on each side for 2-way merge.
    const blocks = useMemo<ChangeBlock[]>(() => {
        if (!compare) return [];
        const list: ChangeBlock[] = [];
        const rows = compare.rows;
        let i = 0;
        while (i < rows.length) {
            const [l, r] = rows[i];
            const changed = (l !== null && compare.leftChanges[l] !== undefined) || (r !== null && compare.rightChanges[r] !== undefined);
            if (!changed) {
                i++;
                continue;
            }
            const startRow = i;
            let lMin = Infinity;
            let lMax = -1;
            let rMin = Infinity;
            let rMax = -1;
            while (i < rows.length) {
                const [li, ri] = rows[i];
                const isCh = (li !== null && compare.leftChanges[li] !== undefined) || (ri !== null && compare.rightChanges[ri] !== undefined);
                if (!isCh) break;
                if (li !== null) {
                    lMin = Math.min(lMin, li);
                    lMax = Math.max(lMax, li + 1);
                }
                if (ri !== null) {
                    rMin = Math.min(rMin, ri);
                    rMax = Math.max(rMax, ri + 1);
                }
                i++;
            }
            const endRow = i;
            if (lMax < 0) {
                let ins = 0;
                for (let k = startRow - 1; k >= 0; k--) {
                    if (rows[k][0] !== null) {
                        ins = rows[k][0]! + 1;
                        break;
                    }
                }
                lMin = ins;
                lMax = ins;
            }
            if (rMax < 0) {
                let ins = 0;
                for (let k = startRow - 1; k >= 0; k--) {
                    if (rows[k][1] !== null) {
                        ins = rows[k][1]! + 1;
                        break;
                    }
                }
                rMin = ins;
                rMax = ins;
            }
            list.push({ startRow, endRow, leftRange: [lMin, lMax], rightRange: [rMin, rMax] });
        }
        return list;
    }, [compare]);

    // Map each row index to its block index (-1 if unchanged).
    const rowToBlock = useMemo(() => {
        if (!compare) return new Int32Array(0);
        const map = new Int32Array(compare.rows.length).fill(-1);
        blocks.forEach((b, idx) => {
            for (let r = b.startRow; r < b.endRow; r++) map[r] = idx;
        });
        return map;
    }, [compare, blocks]);

    // Visible items according to rowFilter ("all" | "context" | "diffs") and expandedFolds.
    const visibleItems = useMemo<VisibleItem[]>(() => {
        if (!compare) return [];
        const total = compare.rows.length;
        if (rowFilter === "all") {
            return compare.rows.map((_, r) => {
                const b = rowToBlock[r];
                return { kind: "row", rowIndex: r, blockIndex: b, isBlockStart: b >= 0 && blocks[b].startRow === r };
            });
        }
        if (rowFilter === "diffs") {
            const out: VisibleItem[] = [];
            for (let r = 0; r < total; r++) {
                const b = rowToBlock[r];
                if (b >= 0) out.push({ kind: "row", rowIndex: r, blockIndex: b, isBlockStart: blocks[b].startRow === r });
            }
            return out;
        }
        // Context mode: ±3 lines around every changed block
        const keep = new Uint8Array(total);
        for (const b of blocks) {
            const from = Math.max(0, b.startRow - 3);
            const to = Math.min(total, b.endRow + 3);
            for (let r = from; r < to; r++) keep[r] = 1;
        }
        const out: VisibleItem[] = [];
        let r = 0;
        while (r < total) {
            if (keep[r]) {
                const b = rowToBlock[r];
                out.push({ kind: "row", rowIndex: r, blockIndex: b, isBlockStart: b >= 0 && blocks[b].startRow === r });
                r++;
            } else {
                const startRow = r;
                while (r < total && !keep[r]) r++;
                const endRow = r;
                const count = endRow - startRow;
                if (count <= 1 || expandedFolds.has(startRow)) {
                    for (let k = startRow; k < endRow; k++) {
                        out.push({ kind: "row", rowIndex: k, blockIndex: -1, isBlockStart: false });
                    }
                } else {
                    out.push({ kind: "fold", startRow, endRow, count });
                }
            }
        }
        return out;
    }, [compare, rowFilter, rowToBlock, blocks, expandedFolds]);

    const rowToVisibleIndex = useMemo(() => {
        const map = new Map<number, number>();
        visibleItems.forEach((item, idx) => {
            if (item.kind === "row") map.set(item.rowIndex, idx);
        });
        return map;
    }, [visibleItems]);

    // Search matches by row index
    const searchRowMatches = useMemo(() => {
        if (!compare || !searchQuery) return [];
        const needle = searchQuery.toLowerCase();
        const rows: number[] = [];
        compare.rows.forEach(([l, r], idx) => {
            const lt = l !== null ? (compare.left[l] ?? "").toLowerCase() : "";
            const rt = r !== null ? (compare.right[r] ?? "").toLowerCase() : "";
            if (lt.includes(needle) || rt.includes(needle)) rows.push(idx);
        });
        return rows;
    }, [compare, searchQuery]);

    const [at, setAt] = useState(-1);
    const [targetVisibleIdx, setTargetVisibleIdx] = useState(-1);
    const [scrollKey, setScrollKey] = useState(0);

    const jumpToRow = useCallback(
        (rowIndex: number) => {
            const vIdx = rowToVisibleIndex.get(rowIndex);
            if (vIdx !== undefined) {
                setTargetVisibleIdx(vIdx);
                setScrollKey((k) => k + 1);
            }
        },
        [rowToVisibleIndex]
    );

    const selectBlock = useCallback(
        (nextBlock: number) => {
            if (!blocks.length) {
                setAt(-1);
                return;
            }
            const clamped = Math.max(0, Math.min(blocks.length - 1, nextBlock));
            setAt(clamped);
            const vIdx = rowToVisibleIndex.get(blocks[clamped].startRow);
            if (vIdx !== undefined) {
                setTargetVisibleIdx(vIdx);
                setScrollKey((k) => k + 1);
            }
        },
        [blocks, rowToVisibleIndex]
    );

    const go = useCallback(
        (step: number) => {
            if (!blocks.length) return;
            selectBlock((at < 0 ? 0 : at) + step);
        },
        [blocks.length, at, selectBlock]
    );

    useEffect(() => {
        if (!blocks.length) {
            setAt(-1);
            setTargetVisibleIdx(-1);
            return;
        }
        const next = at >= 0 ? Math.min(at, blocks.length - 1) : 0;
        setAt(next);
        const vIdx = rowToVisibleIndex.get(blocks[next].startRow);
        if (vIdx !== undefined) setTargetVisibleIdx(vIdx);
    }, [blocks, rowToVisibleIndex]);

    // Merge actions (Copy block left -> right or right -> left)
    // A side that isn't UTF-8 shows � where it isn't: saved, every such byte would become one.
    const canEditLeft = !left.readOnly && compare?.leftUtf8 !== false;
    const canEditRight = !right.readOnly && compare?.rightUtf8 !== false;

    const copyBlock = useCallback(
        (blockIndex: number, direction: "toRight" | "toLeft") => {
            if (!compare || blockIndex < 0 || blockIndex >= blocks.length) return;
            if (direction === "toRight" && !canEditRight) return;
            if (direction === "toLeft" && !canEditLeft) return;
            const block = blocks[blockIndex];
            const curLeft = leftLines ?? compare.left;
            const curRight = rightLines ?? compare.right;
            setHistory((h) => [...h, { left: leftLines, right: rightLines }]);
            if (direction === "toRight") {
                const replacement = curLeft.slice(block.leftRange[0], block.leftRange[1]);
                const nextRight = [...curRight.slice(0, block.rightRange[0]), ...replacement, ...curRight.slice(block.rightRange[1])];
                setRightLines(nextRight);
            } else {
                const replacement = curRight.slice(block.rightRange[0], block.rightRange[1]);
                const nextLeft = [...curLeft.slice(0, block.leftRange[0]), ...replacement, ...curLeft.slice(block.leftRange[1])];
                setLeftLines(nextLeft);
            }
        },
        [compare, blocks, leftLines, rightLines, canEditLeft, canEditRight]
    );

    const commitLineEdit = useCallback(
        (side: 0 | 1, line: number, value: string) => {
            setEditingCell(null);
            if (!compare) return;
            const cur = side === 0 ? (leftLines ?? compare.left) : (rightLines ?? compare.right);
            if (cur[line] === value) return;
            setHistory((h) => [...h, { left: leftLines, right: rightLines }]);
            const next = [...cur];
            next[line] = value;
            if (side === 0) setLeftLines(next);
            else setRightLines(next);
        },
        [compare, leftLines, rightLines]
    );

    const undo = useCallback(() => {
        setHistory((h) => {
            if (!h.length) return h;
            const last = h[h.length - 1];
            setLeftLines(last.left);
            setRightLines(last.right);
            return h.slice(0, -1);
        });
    }, []);

    // The latest save, for a toast's Save anyway clicked after more edits.
    const saveRef = useRef<(side: 0 | 1, overwrite?: boolean) => Promise<void>>(async () => {});
    const saveSide = useCallback(
        async (side: 0 | 1, overwrite = false) => {
            const lines = side === 0 ? leftLines : rightLines;
            const target = side === 0 ? left : right;
            if (!lines || target.readOnly) return;
            const meta = diskMeta.current;
            const content = serializeLines(lines, side === 0 ? (meta?.leftCrlf ?? false) : (meta?.rightCrlf ?? false), side === 0 ? (meta?.leftNewline ?? true) : (meta?.rightNewline ?? true));
            setSavingSide(side);
            try {
                diskHash.current[side] = await writeFileContent(target.path, content, overwrite ? undefined : (diskHash.current[side] ?? undefined));
                if (side === 0) setLeftLines(null);
                else setRightLines(null);
                toast.success(`Saved ${nameOf(target)}`);
                run();
            } catch (e) {
                // Changed on disk since it was compared: saving over that anyway is a choice, as editors offer.
                const changed = String(e).includes("since it was compared");
                toast.error(`Couldn't save ${nameOf(target)}: ${String(e)}`, changed ? { action: { label: "Save anyway", onClick: () => void saveRef.current(side, true) } } : undefined);
            } finally {
                setSavingSide(null);
            }
        },
        [leftLines, rightLines, left, right, run]
    );
    saveRef.current = saveSide;

    // What closing would lose: edits not saved yet.
    useEffect(() => {
        unsaved = leftLines !== null || rightLines !== null;
    }, [leftLines, rightLines]);
    useEffect(() => () => void (unsaved = false), []);

    // Ctrl+S while a line is edited: the edit goes in as its field loses focus, and then the save.
    const saveSoon = useRef(false);
    useEffect(() => {
        if (!saveSoon.current || editingCell) return;
        saveSoon.current = false;
        if (leftLines) void saveSide(0);
        if (rightLines) void saveSide(1);
    }, [editingCell, leftLines, rightLines, saveSide]);

    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (editingCell) {
                if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
                    e.preventDefault();
                    saveSoon.current = true;
                    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
                }
                return;
            }
            if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f") {
                e.preventDefault();
                setSearchOpen(true);
                requestAnimationFrame(() => searchInputRef.current?.focus());
                return;
            }
            // In the find box, Ctrl+Z is the box's own.
            const typing = e.target instanceof Element && e.target.matches("input, textarea");
            if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z" && history.length > 0 && !typing) {
                e.preventDefault();
                undo();
                return;
            }
            if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
                if (leftLines || rightLines) {
                    e.preventDefault();
                    if (leftLines) void saveSide(0);
                    if (rightLines) void saveSide(1);
                }
                return;
            }
            if (!e.altKey) return;
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                go(e.key === "ArrowDown" ? 1 : -1);
            } else if (e.key === "ArrowRight" && at >= 0) {
                e.preventDefault();
                copyBlock(at, "toRight");
            } else if (e.key === "ArrowLeft" && at >= 0) {
                e.preventDefault();
                copyBlock(at, "toLeft");
            }
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [editingCell, history.length, undo, leftLines, rightLines, saveSide, go, at, copyBlock]);

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
    const totalRows = compare.rows.length;

    return (
        <>
            {/* Per-side header with dirty state & Save buttons */}
            <div className="flex shrink-0 border-b bg-chrome/60 text-xs">
                {[left, right].map((side, n) => {
                    const dirty = n === 0 ? leftLines !== null : rightLines !== null;
                    const canEdit = n === 0 ? canEditLeft : canEditRight;
                    return (
                        <div key={n} className={cn("flex min-w-0 flex-1 items-center gap-2 px-3.5 py-1.5", n === 1 && "border-l")}>
                            <span className={cn("size-2 shrink-0 rounded-full", n === 0 ? "bg-red-500/70" : "bg-emerald-500/70")} />
                            <span className="truncate font-medium text-foreground" title={side.path}>
                                {nameOf(side)}
                            </span>
                            {side.label && <span className="truncate text-muted-foreground">{baseName(side.path)}</span>}
                            {dirty && <span className="rounded bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-medium text-amber-600 dark:text-amber-400">Modified</span>}
                            {!canEdit && !side.readOnly && (
                                <span className="shrink-0 rounded bg-foreground/[0.06] px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground" title="Its text isn't UTF-8: it shows � where Glint can't read it, so it isn't edited or saved here">
                                    Read-only: not UTF-8
                                </span>
                            )}
                            <div className="ml-auto flex items-center gap-1">
                                {dirty && canEdit && (
                                    <Button
                                        variant="default"
                                        size="sm"
                                        className="h-6 gap-1 px-2 text-xs"
                                        disabled={savingSide === n}
                                        onClick={() => void saveSide(n as 0 | 1)}
                                        title="Save changes to disk (Ctrl+S)"
                                    >
                                        {savingSide === n ? <Loader2 className="size-3 animate-spin" /> : <Save className="size-3" />} Save
                                    </Button>
                                )}
                            </div>
                        </div>
                    );
                })}
            </div>

            {/* Diff toolbar */}
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
                    className="hidden xl:inline"
                    title={compare.engine === "difftastic" ? "Difftastic compared them by the code's structure: a change of layout alone isn't one" : "Compared line by line with intra-line word & character highlighting"}
                >
                    · {compare.engine === "difftastic" ? `Difftastic${compare.language ? `, as ${compare.language}` : ""}` : "line by line"} in {formatDuration(compare.elapsed)}
                </span>
                {compare.engine === "lines" && difftastic && !difftastic.origin && (
                    <Button
                        variant="ghost"
                        size="sm"
                        className="h-6 px-1.5 text-xs"
                        disabled={installing}
                        onClick={() => void installTool("difftastic")}
                        title="Difftastic compares code by its structure: what changed in it, not its layout. It installs with winget."
                    >
                        {installing ? <Loader2 className="size-3 animate-spin" /> : <Download className="size-3" />} Difftastic
                    </Button>
                )}

                {/* View filter: All / Context / Diffs */}
                <div role="group" aria-label="Rows to show" className="ml-2 flex rounded-md bg-foreground/[0.05] p-0.5">
                    {(
                        [
                            ["all", "All"],
                            ["context", "Context"],
                            ["diffs", "Diffs"],
                        ] as [RowFilter, string][]
                    ).map(([key, label]) => (
                        <button
                            key={key}
                            type="button"
                            aria-pressed={rowFilter === key}
                            onClick={() => {
                                setExpandedFolds(new Set());
                                setRowFilter(key);
                            }}
                            className={cn(
                                "h-5 rounded-[3px] px-2 text-[11px] font-medium transition-colors hover:text-foreground",
                                rowFilter === key && "bg-background text-primary shadow-xs dark:bg-foreground/[0.12]"
                            )}
                        >
                            {label}
                        </button>
                    ))}
                </div>

                {/* Ignore Whitespace & Case toggles */}
                <div className="flex items-center gap-1">
                    <button
                        type="button"
                        onClick={() => setIgnoreWhitespace((w) => (w === "none" ? "trailing" : w === "trailing" ? "all" : "none"))}
                        title={
                            ignoreWhitespace === "none"
                                ? "Whitespace: comparing all whitespace (click to ignore trailing)"
                                : ignoreWhitespace === "trailing"
                                  ? "Whitespace: ignoring trailing whitespace (click to ignore all whitespace)"
                                  : "Whitespace: ignoring all whitespace (click to compare all)"
                        }
                        className={cn(
                            "h-6 rounded px-1.5 text-[11px] font-medium transition-colors hover:bg-foreground/[0.06] hover:text-foreground",
                            ignoreWhitespace !== "none" && "bg-primary/15 text-primary"
                        )}
                    >
                        {ignoreWhitespace === "none" ? "WS: Exact" : ignoreWhitespace === "trailing" ? "WS: Trim" : "WS: Ignore"}
                    </button>
                    <button
                        type="button"
                        aria-pressed={ignoreCase}
                        onClick={() => setIgnoreCase((c) => !c)}
                        title="Ignore case differences"
                        className={cn(
                            "flex h-6 items-center gap-1 rounded px-1.5 text-[11px] font-medium transition-colors hover:bg-foreground/[0.06] hover:text-foreground",
                            ignoreCase && "bg-primary/15 text-primary"
                        )}
                    >
                        <CaseSensitive className="size-3.5" />
                    </button>
                    {withDifftastic && (
                        <button
                            type="button"
                            aria-pressed={!lineMode && ignoreWhitespace === "none" && !ignoreCase && !leftLines && !rightLines}
                            onClick={() => setLineMode((m) => !m)}
                            title={lineMode ? "Using line-by-line diff (click for Difftastic structural diff)" : "Using Difftastic structural diff (click for line-by-line diff)"}
                            className={cn(
                                "flex h-6 items-center gap-1 rounded px-1.5 text-[11px] font-medium transition-colors hover:bg-foreground/[0.06] hover:text-foreground",
                                !lineMode && ignoreWhitespace === "none" && !ignoreCase && !leftLines && !rightLines && "bg-primary/15 text-primary"
                            )}
                        >
                            <Sparkles className="size-3" /> AST
                        </button>
                    )}
                </div>

                <div className="ml-auto flex items-center gap-1">
                    {history.length > 0 && (
                        <Button variant="ghost" size="sm" className="h-6 gap-1 px-1.5 text-xs" onClick={undo} title="Undo last merge or edit (Ctrl+Z)">
                            <RotateCcw className="size-3" /> Undo
                        </Button>
                    )}
                    <Button
                        variant="ghost"
                        size="icon"
                        className={cn("size-7", searchOpen && "bg-primary/15 text-primary")}
                        onClick={() => {
                            setSearchOpen((o) => {
                                const next = !o;
                                if (next) requestAnimationFrame(() => searchInputRef.current?.focus());
                                else setSearchQuery("");
                                return next;
                            });
                        }}
                        title="Find in files (Ctrl+F)"
                    >
                        <Search className="size-3.5" />
                    </Button>
                    {blocks.length > 0 && <span className="tabular-nums">{at >= 0 ? `${at + 1} of ${blocks.length}` : ""}</span>}
                    <Button variant="ghost" size="icon" className="size-7" disabled={!blocks.length || at <= 0} onClick={() => go(-1)} title="Previous change (Alt+Up)">
                        <ChevronUp />
                    </Button>
                    <Button variant="ghost" size="icon" className="size-7" disabled={!blocks.length || at >= blocks.length - 1} onClick={() => go(1)} title="Next change (Alt+Down)">
                        <ChevronDown />
                    </Button>
                </div>
            </div>

            {/* Inline search bar */}
            {searchOpen && (
                <div className="flex h-8 shrink-0 items-center gap-2 border-b bg-chrome/40 px-3.5 text-xs">
                    <Search className="size-3.5 text-muted-foreground" />
                    <input
                        ref={searchInputRef}
                        type="text"
                        data-own-escape
                        value={searchQuery}
                        onChange={(e) => {
                            setSearchQuery(e.target.value);
                            setSearchIdx(0);
                        }}
                        onKeyDown={(e) => {
                            if (e.key === "Enter" && searchRowMatches.length > 0) {
                                e.preventDefault();
                                const next = (searchIdx + (e.shiftKey ? -1 : 1) + searchRowMatches.length) % searchRowMatches.length;
                                setSearchIdx(next);
                                jumpToRow(searchRowMatches[next]);
                            } else if (e.key === "Escape") {
                                e.preventDefault();
                                setSearchOpen(false);
                                setSearchQuery("");
                            }
                        }}
                        placeholder="Find text in either file…"
                        className="h-6 w-64 rounded border bg-background px-2 text-xs text-foreground outline-none focus:border-primary"
                    />
                    {searchQuery && (
                        <span className="tabular-nums text-muted-foreground">
                            {searchRowMatches.length ? `${Math.min(searchIdx + 1, searchRowMatches.length)} of ${plural(searchRowMatches.length, "matching row")}` : "No matches"}
                        </span>
                    )}
                    <Button
                        variant="ghost"
                        size="icon"
                        className="size-6"
                        disabled={!searchRowMatches.length}
                        onClick={() => {
                            const next = (searchIdx - 1 + searchRowMatches.length) % searchRowMatches.length;
                            setSearchIdx(next);
                            jumpToRow(searchRowMatches[next]);
                        }}
                        title="Previous match (Shift+Enter)"
                    >
                        <ChevronUp className="size-3.5" />
                    </Button>
                    <Button
                        variant="ghost"
                        size="icon"
                        className="size-6"
                        disabled={!searchRowMatches.length}
                        onClick={() => {
                            const next = (searchIdx + 1) % searchRowMatches.length;
                            setSearchIdx(next);
                            jumpToRow(searchRowMatches[next]);
                        }}
                        title="Next match (Enter)"
                    >
                        <ChevronDown className="size-3.5" />
                    </Button>
                    <Button
                        variant="ghost"
                        size="icon"
                        className="ml-auto size-6"
                        onClick={() => {
                            setSearchOpen(false);
                            setSearchQuery("");
                        }}
                        title="Close find bar (Esc)"
                    >
                        <X className="size-3.5" />
                    </Button>
                </div>
            )}

            <div className="flex min-h-0 flex-1">
                <VirtualList
                    count={visibleItems.length}
                    rowHeight={20}
                    scrollToIndex={targetVisibleIdx}
                    scrollKey={scrollKey}
                    className="min-h-0 flex-1 overflow-auto font-mono text-xs leading-5"
                    role="table"
                    aria-label="Side by side"
                >
                    {(vIdx) => {
                        const item = visibleItems[vIdx];
                        if (item.kind === "fold") {
                            return (
                                <button
                                    type="button"
                                    onClick={() => setExpandedFolds((prev) => new Set(prev).add(item.startRow))}
                                    className="flex h-5 w-full items-center justify-center gap-1.5 bg-muted/40 text-[11px] font-sans text-muted-foreground transition-colors hover:bg-muted/80 hover:text-foreground"
                                >
                                    <UnfoldVertical className="size-3" />
                                    {formatCount(item.count)} unchanged {item.count === 1 ? "line" : "lines"} — click to expand
                                </button>
                            );
                        }
                        const i = item.rowIndex;
                        const [l, r] = compare.rows[i];
                        const bIdx = item.blockIndex;
                        const isCurrentBlock = bIdx >= 0 && bIdx === at;
                        return (
                            <div
                                role="row"
                                onClick={() => bIdx >= 0 && setAt(bIdx)}
                                className={cn("group/row flex h-5", isCurrentBlock && "bg-primary/[0.04] outline outline-1 -outline-offset-1 outline-primary/40")}
                            >
                                <Half
                                    line={l}
                                    text={l !== null ? compare.left[l] : undefined}
                                    spans={l !== null ? compare.leftChanges[l] : undefined}
                                    searchQuery={searchQuery}
                                    side={0}
                                    editable={canEditLeft && l !== null}
                                    editing={editingCell?.side === 0 && editingCell.line === l}
                                    onStartEdit={() => l !== null && setEditingCell({ side: 0, line: l })}
                                    onCommitEdit={(val) => l !== null && commitLineEdit(0, l, val)}
                                    onCancelEdit={() => setEditingCell(null)}
                                />
                                {/* Center 2-way merge gutter */}
                                <div className="flex w-9 shrink-0 select-none items-center justify-center gap-0.5 border-x bg-chrome/40">
                                    {bIdx >= 0 && (item.isBlockStart || isCurrentBlock) && (
                                        <>
                                            {canEditRight && (
                                                <button
                                                    type="button"
                                                    onClick={(e) => {
                                                        e.stopPropagation();
                                                        copyBlock(bIdx, "toRight");
                                                    }}
                                                    title="Copy change to right (Alt+Right)"
                                                    className="flex size-4 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-primary/20 hover:text-primary"
                                                >
                                                    <ArrowRight className="size-3" />
                                                </button>
                                            )}
                                            {canEditLeft && (
                                                <button
                                                    type="button"
                                                    onClick={(e) => {
                                                        e.stopPropagation();
                                                        copyBlock(bIdx, "toLeft");
                                                    }}
                                                    title="Copy change to left (Alt+Left)"
                                                    className="flex size-4 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-primary/20 hover:text-primary"
                                                >
                                                    <ArrowLeft className="size-3" />
                                                </button>
                                            )}
                                        </>
                                    )}
                                </div>
                                <Half
                                    line={r}
                                    text={r !== null ? compare.right[r] : undefined}
                                    spans={r !== null ? compare.rightChanges[r] : undefined}
                                    searchQuery={searchQuery}
                                    side={1}
                                    editable={canEditRight && r !== null}
                                    editing={editingCell?.side === 1 && editingCell.line === r}
                                    onStartEdit={() => r !== null && setEditingCell({ side: 1, line: r })}
                                    onCommitEdit={(val) => r !== null && commitLineEdit(1, r, val)}
                                    onCancelEdit={() => setEditingCell(null)}
                                />
                            </div>
                        );
                    }}
                </VirtualList>

                {/* Diff Minimap Overview Strip */}
                {totalRows > 0 && (
                    <div
                        role="presentation"
                        title="Diff minimap — click to jump to change"
                        onClick={(e) => {
                            if (!blocks.length) return;
                            const rect = e.currentTarget.getBoundingClientRect();
                            const ratio = Math.max(0, Math.min(1, (e.clientY - rect.top) / rect.height));
                            const targetRow = Math.round(ratio * totalRows);
                            let best = 0;
                            let bestDist = Infinity;
                            blocks.forEach((b, idx) => {
                                const mid = (b.startRow + b.endRow) / 2;
                                const d = Math.abs(mid - targetRow);
                                if (d < bestDist) {
                                    bestDist = d;
                                    best = idx;
                                }
                            });
                            selectBlock(best);
                        }}
                        className="relative w-3 shrink-0 cursor-pointer select-none border-l bg-chrome/50"
                    >
                        {blocks.map((b, idx) => {
                            const topPct = (b.startRow / totalRows) * 100;
                            const heightPct = Math.max(0.6, ((b.endRow - b.startRow) / totalRows) * 100);
                            const hasLeft = b.leftRange[1] > b.leftRange[0];
                            const hasRight = b.rightRange[1] > b.rightRange[0];
                            const color = hasLeft && hasRight ? "bg-amber-500/80" : hasLeft ? "bg-red-500/80" : "bg-emerald-500/80";
                            return (
                                <div
                                    key={idx}
                                    style={{ top: `${topPct}%`, height: `${heightPct}%` }}
                                    className={cn("absolute inset-x-0.5 rounded-[1px]", color, idx === at && "ring-1 ring-primary")}
                                />
                            );
                        })}
                    </div>
                )}
            </div>
        </>
    );
}

type PictureMode = "side" | "swipe" | "onion" | "diff";

function PicturesView({ left, right }: { left: CompareSide; right: CompareSide }) {
    const [mode, setMode] = useState<PictureMode>("side");
    const [amount, setAmount] = useState(50);
    const [tolerance, setTolerance] = useState(5);
    const [zoom, setZoom] = useState<number>(1);
    const [pan, setPan] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
    const [dims, setDims] = useState<[string, string]>(["", ""]);
    const [diffStats, setDiffStats] = useState<{ changed: number; total: number } | null>(null);
    const imgElements = useRef<[HTMLImageElement | null, HTMLImageElement | null]>([null, null]);
    const canvasRef = useRef<HTMLCanvasElement | null>(null);
    const dragRef = useRef<{ startX: number; startY: number; panX: number; panY: number } | null>(null);

    const src = useMemo(() => [convertFileSrc(left.path), convertFileSrc(right.path)], [left.path, right.path]);

    const computePixelDiff = useCallback(() => {
        const [imgA, imgB] = imgElements.current;
        const canvas = canvasRef.current;
        if (!imgA || !imgB || !canvas || !imgA.naturalWidth || !imgB.naturalWidth) return;
        const w = Math.max(imgA.naturalWidth, imgB.naturalWidth);
        const h = Math.max(imgA.naturalHeight, imgB.naturalHeight);
        // Cap offscreen comparison resolution to avoid excessive memory on huge photos while preserving crispness
        const scale = Math.min(1, 2048 / Math.max(w, h));
        const cw = Math.max(1, Math.round(w * scale));
        const ch = Math.max(1, Math.round(h * scale));
        canvas.width = cw;
        canvas.height = ch;

        const offA = document.createElement("canvas");
        const offB = document.createElement("canvas");
        offA.width = cw;
        offA.height = ch;
        offB.width = cw;
        offB.height = ch;
        const ctxA = offA.getContext("2d");
        const ctxB = offB.getContext("2d");
        const ctxOut = canvas.getContext("2d");
        if (!ctxA || !ctxB || !ctxOut) return;

        ctxA.drawImage(imgA, 0, 0, Math.round(imgA.naturalWidth * scale), Math.round(imgA.naturalHeight * scale));
        ctxB.drawImage(imgB, 0, 0, Math.round(imgB.naturalWidth * scale), Math.round(imgB.naturalHeight * scale));
        const dataA = ctxA.getImageData(0, 0, cw, ch).data;
        const dataB = ctxB.getImageData(0, 0, cw, ch).data;
        const outImg = ctxOut.createImageData(cw, ch);
        const out = outImg.data;
        const thresh = Math.round((tolerance / 100) * 255);
        let changed = 0;
        const total = cw * ch;

        for (let i = 0; i < dataA.length; i += 4) {
            const dr = Math.abs(dataA[i] - dataB[i]);
            const dg = Math.abs(dataA[i + 1] - dataB[i + 1]);
            const db = Math.abs(dataA[i + 2] - dataB[i + 2]);
            const da = Math.abs(dataA[i + 3] - dataB[i + 3]);
            if (dr > thresh || dg > thresh || db > thresh || da > thresh) {
                changed++;
                out[i] = 255;
                out[i + 1] = 45;
                out[i + 2] = 85;
                out[i + 3] = 240;
            } else {
                const lum = Math.round(0.299 * dataA[i] + 0.587 * dataA[i + 1] + 0.114 * dataA[i + 2]);
                out[i] = lum;
                out[i + 1] = lum;
                out[i + 2] = lum;
                out[i + 3] = Math.round(dataA[i + 3] * 0.28);
            }
        }
        ctxOut.putImageData(outImg, 0, 0);
        setDiffStats({ changed, total });
    }, [tolerance]);

    useEffect(() => {
        if (mode === "diff") computePixelDiff();
    }, [mode, tolerance, dims, computePixelDiff]);

    const loaded = (n: 0 | 1) => (e: React.SyntheticEvent<HTMLImageElement>) => {
        imgElements.current[n] = e.currentTarget;
        const { naturalWidth: w, naturalHeight: h } = e.currentTarget;
        setDims((d) => (n === 0 ? [`${w} × ${h}`, d[1]] : [d[0], `${w} × ${h}`]));
    };

    const imgStyle: React.CSSProperties = {
        transform: zoom === 1 && pan.x === 0 && pan.y === 0 ? undefined : `translate(${pan.x}px, ${pan.y}px) scale(${zoom})`,
        transformOrigin: "center center",
    };
    const img = "absolute inset-0 m-auto max-h-full max-w-full object-contain transition-transform duration-75";
    const checker = "relative min-h-0 flex-1 overflow-hidden bg-[conic-gradient(var(--color-muted)_25%,transparent_0_50%,var(--color-muted)_0_75%,transparent_0)] bg-[length:16px_16px]";

    return (
        <>
            <div className="flex h-9 shrink-0 items-center gap-3 border-b px-3.5 text-xs text-muted-foreground">
                <div role="group" aria-label="Show them" className="flex rounded-md bg-foreground/[0.05] p-0.5">
                    {(
                        [
                            ["side", "Side by side"],
                            ["swipe", "Swipe"],
                            ["onion", "Onion skin"],
                            ["diff", "Pixel diff"],
                        ] as [PictureMode, string][]
                    ).map(([key, label]) => (
                        <button
                            key={key}
                            type="button"
                            aria-pressed={mode === key}
                            onClick={() => setMode(key)}
                            className={cn(
                                "h-6 rounded-[4px] px-2 font-medium transition-colors hover:text-foreground",
                                mode === key && "bg-background text-primary shadow-xs dark:bg-foreground/[0.12]"
                            )}
                        >
                            {label}
                        </button>
                    ))}
                </div>
                {(mode === "swipe" || mode === "onion") && (
                    <input
                        type="range"
                        min={0}
                        max={100}
                        value={amount}
                        onChange={(e) => setAmount(Number(e.target.value))}
                        aria-label={mode === "swipe" ? "Where the swipe is" : "How much of the second shows"}
                        className="w-40 accent-primary"
                    />
                )}
                {mode === "diff" && (
                    <div className="flex items-center gap-2">
                        <span>Tolerance {tolerance}%</span>
                        <input
                            type="range"
                            min={0}
                            max={50}
                            value={tolerance}
                            onChange={(e) => setTolerance(Number(e.target.value))}
                            aria-label="Pixel difference tolerance"
                            className="w-32 accent-primary"
                        />
                        {diffStats && (
                            <span className={cn("font-medium", diffStats.changed === 0 ? "text-emerald-600 dark:text-emerald-400" : "text-red-600 dark:text-red-400")}>
                                {diffStats.changed === 0
                                    ? "Identical pixels"
                                    : `${ ((diffStats.changed / diffStats.total) * 100).toFixed(2) }% (${formatCount(diffStats.changed)} px) differ`}
                            </span>
                        )}
                    </div>
                )}
                <div className="ml-auto flex items-center gap-1">
                    <Button variant="ghost" size="icon" className="size-6" onClick={() => setZoom((z) => Math.max(0.25, +(z - 0.25).toFixed(2)))} title="Zoom out">
                        <ZoomOut className="size-3.5" />
                    </Button>
                    <button
                        type="button"
                        onClick={() => {
                            setZoom(1);
                            setPan({ x: 0, y: 0 });
                        }}
                        title="Reset zoom and pan"
                        className="h-6 rounded px-1.5 tabular-nums hover:bg-foreground/[0.06] hover:text-foreground"
                    >
                        {Math.round(zoom * 100)}%
                    </button>
                    <Button variant="ghost" size="icon" className="size-6" onClick={() => setZoom((z) => Math.min(8, +(z + 0.25).toFixed(2)))} title="Zoom in">
                        <ZoomIn className="size-3.5" />
                    </Button>
                    <span className="ml-2 tabular-nums">
                        {dims[0] || "…"} · {dims[1] || "…"}
                    </span>
                </div>
            </div>
            {/* Hidden preloaders so both natural dimensions and pixel data are always available */}
            <div className="hidden">
                <img src={src[0]} crossOrigin="anonymous" alt="" onLoad={loaded(0)} />
                <img src={src[1]} crossOrigin="anonymous" alt="" onLoad={loaded(1)} />
            </div>
            <div
                className="flex min-h-0 flex-1 select-none"
                onWheel={(e) => {
                    if (e.ctrlKey || zoom > 1) {
                        e.preventDefault();
                        const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15;
                        setZoom((z) => Math.max(0.25, Math.min(8, +(z * factor).toFixed(2))));
                    }
                }}
                onMouseDown={(e) => {
                    if (e.button !== 0) return;
                    dragRef.current = { startX: e.clientX, startY: e.clientY, panX: pan.x, panY: pan.y };
                }}
                onMouseMove={(e) => {
                    if (!dragRef.current) return;
                    setPan({
                        x: dragRef.current.panX + (e.clientX - dragRef.current.startX),
                        y: dragRef.current.panY + (e.clientY - dragRef.current.startY),
                    });
                }}
                onMouseUp={() => {
                    dragRef.current = null;
                }}
                onMouseLeave={() => {
                    dragRef.current = null;
                }}
            >
                {mode === "side" ? (
                    <div className="flex min-h-0 flex-1">
                        {[0, 1].map((n) => (
                            <div key={n} className={cn(checker, n === 1 && "border-l")}>
                                <img src={src[n]} alt={baseName(n === 0 ? left.path : right.path)} className={img} style={imgStyle} draggable={false} />
                            </div>
                        ))}
                    </div>
                ) : mode === "diff" ? (
                    <div className={checker}>
                        <canvas ref={canvasRef} className={img} style={imgStyle} />
                    </div>
                ) : (
                    <div className={checker}>
                        <img src={src[0]} alt={baseName(left.path)} className={img} style={imgStyle} draggable={false} />
                        <img
                            src={src[1]}
                            alt={baseName(right.path)}
                            className={img}
                             draggable={false}
                            style={{
                                ...imgStyle,
                                ...(mode === "swipe" ? { clipPath: `inset(0 0 0 ${amount}%)` } : { opacity: amount / 100 }),
                            }}
                        />
                        {mode === "swipe" && <div className="pointer-events-none absolute inset-y-0 w-0.5 bg-primary shadow" style={{ left: `${amount}%` }} />}
                    </div>
                )}
            </div>
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
    const [offsetInput, setOffsetInput] = useState("");

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

    const prev = useCallback(async () => {
        if (shown <= 0) return;
        setBusy(true);
        try {
            const before = Math.floor(shown / ROW) * ROW;
            const at = await prevByteDifference(left.path, right.path, before);
            if (at === null) toast.message("No earlier differences before this one", { id: "bytes" });
            else setShown(at);
        } catch (e) {
            setError(String(e));
        } finally {
            setBusy(false);
        }
    }, [left.path, right.path, shown]);

    const next = useCallback(async () => {
        setBusy(true);
        try {
            const at = await nextByteDifference(left.path, right.path, (Math.floor(Math.max(0, shown) / ROW) + 1) * ROW);
            if (at === null) toast.message("No more differences after this one", { id: "bytes" });
            else setShown(at);
        } catch (e) {
            setError(String(e));
        } finally {
            setBusy(false);
        }
    }, [left.path, right.path, shown]);

    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (!e.altKey || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
            e.preventDefault();
            if (e.key === "ArrowDown") void next();
            else void prev();
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [next, prev]);

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
    const maxSize = Math.max(sizes[0], sizes[1]);
    const rows = Math.ceil(maxSize / ROW);
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
            <span className="font-medium text-foreground">
                {shown >= 0 ? `At 0x${hex(shown, 8)}` : `First difference at 0x${hex(first, 8)}`}
            </span>
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
                <div className="ml-auto flex items-center gap-1.5">
                    <input
                        type="text"
                        value={offsetInput}
                        onChange={(e) => setOffsetInput(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key !== "Enter") return;
                            const raw = offsetInput.trim().toLowerCase();
                            const parsed = raw.startsWith("0x") ? parseInt(raw.slice(2), 16) : /^[0-9a-f]+$/.test(raw) && /[a-f]/.test(raw) ? parseInt(raw, 16) : parseInt(raw, 10);
                            if (!Number.isNaN(parsed) && parsed >= 0 && parsed < maxSize) {
                                setShown(parsed);
                            }
                        }}
                        placeholder="Go to 0x…"
                        aria-label="Go to byte offset"
                        className="h-6 w-24 rounded border bg-background px-2 font-mono text-[11px] text-foreground outline-none focus:border-primary"
                    />
                    <Button
                        variant="ghost"
                        size="sm"
                        className="h-7 px-2 text-xs"
                        disabled={first === null || first === undefined || busy || shown <= (first ?? 0)}
                        onClick={() => void prev()}
                        title="Previous difference (Alt+Up)"
                    >
                        {busy ? <Loader2 className="size-3.5 animate-spin" /> : <ChevronUp className="size-3.5" />} Prev
                    </Button>
                    <Button
                        variant="ghost"
                        size="sm"
                        className="h-7 px-2 text-xs"
                        disabled={first === null || first === undefined || busy}
                        onClick={() => void next()}
                        title="Next difference (Alt+Down)"
                    >
                        {busy ? <Loader2 className="size-3.5 animate-spin" /> : <ChevronDown className="size-3.5" />} Next
                    </Button>
                </div>
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
