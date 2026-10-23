import { useEffect, useMemo, useState } from "react";
import { ArrowDown, ArrowUp, Check, Copy, Database, FileCode, Hash, Search, Table2, ToggleLeft, Type } from "lucide-react";
import { toast } from "sonner";
import type { FileEntry } from "@/lib/entries";
import { formatCount, formatSize } from "@/lib/format";
import { inspectSheet, inspectSqlite, inspectTabular, readTextPreview, type SheetPreview as Sheet, type SqliteInfo, type TablePreview } from "@/lib/fs";
import { languageFor, SyntaxHighlighter } from "@/lib/highlight";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { BigIcon, Waiting } from "./PreviewState";

const SQLITE_EXTS = new Set(["sqlite", "sqlite3", "db", "db3", "s3db", "sl3"]);
const TABULAR_EXTS = new Set(["csv", "tsv", "jsonl", "ndjson"]);
const SHEET_EXTS = new Set(["xlsx", "xlsm", "xltx", "xltm", "xlam", "xlsb", "xls", "xla", "xlt", "ods", "parquet"]);

export function isSqliteCandidate(entry: FileEntry): boolean {
    return !entry.dir && SQLITE_EXTS.has(entry.ext);
}

export function isTabularCandidate(entry: FileEntry): boolean {
    return !entry.dir && TABULAR_EXTS.has(entry.ext);
}

/** Workbooks (Excel's, OpenDocument's) and Parquet files, read as tables without Office (sheet.rs). */
export function isSheetCandidate(entry: FileEntry): boolean {
    return !entry.dir && SHEET_EXTS.has(entry.ext);
}

function CopySqlButton({ text }: { text: string }) {
    const [copied, setCopied] = useState(false);
    const copy = () => {
        navigator.clipboard.writeText(text).then(
            () => {
                setCopied(true);
                setTimeout(() => setCopied(false), 1400);
            },
            () => toast.error("Couldn't copy SQL")
        );
    };
    return (
        <button
            type="button"
            onClick={copy}
            title="Copy DDL statement"
            className="rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        >
            {copied ? <Check className="size-3.5 text-emerald-500" /> : <Copy className="size-3.5" />}
        </button>
    );
}

export function SqlitePreview({ entry }: { entry: FileEntry }) {
    const [state, setState] = useState<{ info: SqliteInfo } | { error: string } | null>(null);
    const [tab, setTab] = useState<"tables" | "indexes" | "other">("tables");
    const [query, setQuery] = useState("");

    useEffect(() => {
        let stale = false;
        setState(null);
        inspectSqlite(entry.path).then(
            (info) => !stale && setState({ info }),
            (e) => !stale && setState({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified]);

    const grouped = useMemo(() => {
        if (!state || "error" in state) return { tables: [], indexes: [], other: [] };
        const q = query.trim().toLowerCase();
        const match = (name: string, sql: string, tbl: string) =>
            !q || name.toLowerCase().includes(q) || sql.toLowerCase().includes(q) || tbl.toLowerCase().includes(q);
        const tables = state.info.objects.filter((o) => o.kind === "table" && match(o.name, o.sql, o.tableName));
        const indexes = state.info.objects.filter((o) => o.kind === "index" && match(o.name, o.sql, o.tableName));
        const other = state.info.objects.filter((o) => o.kind !== "table" && o.kind !== "index" && match(o.name, o.sql, o.tableName));
        return { tables, indexes, other };
    }, [state, query]);

    if (!state) return <Waiting what="Reading the database…" />;
    if ("error" in state) return <BigIcon entry={entry} note={state.error} />;

    const { info } = state;
    const totalTables = info.objects.filter((o) => o.kind === "table").length;
    const totalIndexes = info.objects.filter((o) => o.kind === "index").length;
    const totalOther = info.objects.length - totalTables - totalIndexes;
    const activeItems = tab === "tables" ? grouped.tables : tab === "indexes" ? grouped.indexes : grouped.other;

    return (
        <div className="flex h-full flex-col text-xs">
            {/* Database header telemetry */}
            <div className="space-y-2.5 border-b bg-muted/20 px-3.5 py-3">
                <div className="flex flex-wrap items-center gap-1.5">
                    <span className="inline-flex items-center gap-1 rounded-md bg-sky-500/15 px-2 py-0.5 font-semibold text-sky-600 dark:text-sky-400">
                        <Database className="size-3" /> SQLite {info.sqliteVersion}
                    </span>
                    <span className="rounded-md bg-muted px-2 py-0.5 font-medium text-foreground">{info.encoding}</span>
                    <span
                        className={cn(
                            "rounded-md px-2 py-0.5 font-medium",
                            info.journalMode === "WAL"
                                ? "bg-emerald-500/15 text-emerald-700 dark:text-emerald-400"
                                : "bg-muted text-muted-foreground"
                        )}
                    >
                        {info.journalMode} mode
                    </span>
                    <span className="ml-auto font-mono text-[0.625rem] text-muted-foreground tabular-nums">{info.elapsedMs.toFixed(2)} ms</span>
                </div>

                <div className="grid grid-cols-2 gap-x-4 gap-y-1 text-[0.6875rem]">
                    <div className="flex justify-between gap-2">
                        <span className="text-muted-foreground">Page size</span>
                        <span className="font-mono tabular-nums">{formatSize(info.pageSize)}</span>
                    </div>
                    <div className="flex justify-between gap-2">
                        <span className="text-muted-foreground">Pages</span>
                        <span className="font-mono tabular-nums">{formatCount(info.pageCount)}</span>
                    </div>
                    <div className="flex justify-between gap-2">
                        <span className="text-muted-foreground">Freelist</span>
                        <span className="font-mono tabular-nums">
                            {formatCount(info.freelistPages)} ({formatSize(info.freelistPages * info.pageSize)})
                        </span>
                    </div>
                    <div className="flex justify-between gap-2">
                        <span className="text-muted-foreground">User version</span>
                        <span className="font-mono tabular-nums">{info.userVersion}</span>
                    </div>
                </div>
            </div>

            {/* Sub-navigation & Search */}
            <div className="flex flex-wrap items-center gap-1 border-b bg-muted/10 px-2.5 py-1.5">
                <button
                    type="button"
                    onClick={() => setTab("tables")}
                    className={cn(
                        "rounded-md px-2 py-1 font-medium transition-colors",
                        tab === "tables" ? "bg-primary/15 text-primary" : "text-muted-foreground hover:bg-muted/60 hover:text-foreground"
                    )}
                >
                    Tables ({totalTables})
                </button>
                <button
                    type="button"
                    onClick={() => setTab("indexes")}
                    className={cn(
                        "rounded-md px-2 py-1 font-medium transition-colors",
                        tab === "indexes" ? "bg-primary/15 text-primary" : "text-muted-foreground hover:bg-muted/60 hover:text-foreground"
                    )}
                >
                    Indexes ({totalIndexes})
                </button>
                {totalOther > 0 && (
                    <button
                        type="button"
                        onClick={() => setTab("other")}
                        className={cn(
                            "rounded-md px-2 py-1 font-medium transition-colors",
                            tab === "other" ? "bg-primary/15 text-primary" : "text-muted-foreground hover:bg-muted/60 hover:text-foreground"
                        )}
                    >
                        Views & Triggers ({totalOther})
                    </button>
                )}
                <div className="relative ml-auto min-w-[7rem] flex-1">
                    <Search className="pointer-events-none absolute left-2 top-1/2 size-3 -translate-y-1/2 text-muted-foreground" />
                    <input
                        type="text"
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        placeholder="Filter schema…"
                        className="h-6 w-full rounded border bg-background pl-6 pr-2 text-[0.6875rem] outline-hidden focus:border-primary"
                    />
                </div>
            </div>

            {/* Schema objects */}
            <div className="min-h-0 flex-1 space-y-2.5 overflow-auto p-3 select-text">
                {activeItems.length === 0 ? (
                    <p className="py-6 text-center text-muted-foreground">No matching schema objects found.</p>
                ) : (
                    activeItems.map((obj) => (
                        <div key={`${obj.kind}:${obj.name}`} className="overflow-hidden rounded-md border bg-background/50">
                            <div className="flex items-center gap-2 border-b bg-muted/20 px-2.5 py-1.5">
                                <span className="font-mono font-semibold text-foreground">{obj.name}</span>
                                {obj.tableName && obj.tableName !== obj.name && (
                                    <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-[0.625rem] text-muted-foreground">
                                        on {obj.tableName}
                                    </span>
                                )}
                                <span className="ml-auto font-mono text-[0.625rem] text-muted-foreground tabular-nums">
                                    page #{obj.rootPage}
                                </span>
                                {obj.sql && <CopySqlButton text={obj.sql} />}
                            </div>
                            {obj.sql ? (
                                <pre className="overflow-x-auto whitespace-pre-wrap p-2.5 font-mono text-[0.6875rem] leading-relaxed text-muted-foreground">
                                    {obj.sql}
                                </pre>
                            ) : (
                                <p className="p-2.5 text-[0.6875rem] italic text-muted-foreground">Internal auto-index (no SQL statement)</p>
                            )}
                        </div>
                    ))
                )}
            </div>
        </div>
    );
}

/** CSV, TSV and JSON-lines files: their first rows as a table, or their text. */
export function TabularPreview({ entry }: { entry: FileEntry }) {
    const [state, setState] = useState<{ table: TablePreview } | { error: string } | null>(null);
    useEffect(() => {
        let stale = false;
        setState(null);
        inspectTabular(entry.path).then(
            (table) => !stale && setState({ table }),
            (e) => !stale && setState({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified]);
    if (!state) return <Waiting what="Reading the table…" />;
    if ("error" in state) return <BigIcon entry={entry} note={state.error} />;
    return <TableView key={entry.path} entry={entry} table={state.table} raw />;
}

/** A workbook's sheet (with its others to pick from), or a Parquet file with its columns' types, as a table. */
export function SheetPreview({ entry }: { entry: FileEntry }) {
    const [sheet, setSheet] = useState<number | undefined>(undefined);
    const [state, setState] = useState<Sheet | { error: string } | null>(null);
    useEffect(() => {
        let stale = false;
        inspectSheet(entry.path, sheet).then(
            (read) => !stale && setState(read),
            (e) => !stale && setState({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified, sheet]);
    if (!state) return <Waiting what="Reading the table…" />;
    if ("error" in state) return <BigIcon entry={entry} note={state.error} />;
    const picker = state.sheets.length > 1 && (
        <div role="tablist" aria-label="Sheets" className="flex min-w-0 max-w-full gap-0.5 overflow-x-auto">
            {state.sheets.map((name, i) => (
                <button
                    key={name}
                    type="button"
                    role="tab"
                    aria-selected={i === state.sheet}
                    onClick={() => setSheet(i)}
                    className={cn(
                        "h-6 shrink-0 rounded px-2 text-[0.6875rem] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground",
                        i === state.sheet && "bg-background font-medium text-primary shadow-xs dark:bg-foreground/[0.12]"
                    )}
                >
                    {name}
                </button>
            ))}
        </div>
    );
    return <TableView key={`${entry.path}:${state.sheet}`} entry={entry} table={state.table} total={state.totalRows} types={state.types} extra={picker} />;
}

/**
 * A table's rows, filtered and sorted by a click on a column; `raw` offers the file's text instead,
 * `types` are its columns' types (a Parquet file's), and `extra` goes in its header (a workbook's sheets).
 */
function TableView({ entry, table, total, types, extra, raw }: { entry: FileEntry; table: TablePreview; total?: number; types?: string[]; extra?: React.ReactNode; raw?: boolean }) {
    const showLineNumbers = useStore((s) => s.settings.showLineNumbers);
    const [mode, setMode] = useState<"grid" | "raw">("grid");
    const [rawText, setRawText] = useState<{ text: string; truncated: boolean } | null>(null);
    const [query, setQuery] = useState("");
    const [sortCol, setSortCol] = useState<{ index: number; desc: boolean } | null>(null);

    useEffect(() => {
        if (mode !== "raw" || rawText) return;
        let stale = false;
        readTextPreview(entry.path, 256 * 1024).then(
            (res) => !stale && setRawText({ text: res.text, truncated: res.truncated }),
            () => {}
        );
        return () => {
            stale = true;
        };
    }, [mode, rawText, entry.path]);

    const rows = useMemo(() => {
        const q = query.trim().toLowerCase();
        const filtered = q ? table.rows.filter((r) => r.some((cell) => cell.toLowerCase().includes(q))) : table.rows;
        if (!sortCol) return filtered;
        const { index, desc } = sortCol;
        const colKind = table.columns[index]?.kind ?? "text";
        const dir = desc ? -1 : 1;
        return [...filtered].sort((a, b) => {
            const va = a[index] ?? "";
            const vb = b[index] ?? "";
            if (colKind === "number") {
                const na = Number(va);
                const nb = Number(vb);
                if (!Number.isNaN(na) && !Number.isNaN(nb)) return (na - nb) * dir;
            }
            return va.localeCompare(vb, undefined, { numeric: true, sensitivity: "base" }) * dir;
        });
    }, [table, query, sortCol]);

    const ColumnTypeIcon = ({ kind }: { kind: "number" | "boolean" | "text" }) => {
        if (kind === "number") return <Hash className="size-3 shrink-0 text-amber-500" />;
        if (kind === "boolean") return <ToggleLeft className="size-3 shrink-0 text-emerald-500" />;
        return <Type className="size-3 shrink-0 text-sky-500" />;
    };

    return (
        <div className="flex h-full flex-col text-xs">
            {/* Telemetry & view mode header */}
            <div className="flex flex-wrap items-center gap-2 border-b bg-muted/20 px-3 py-2">
                <span className="inline-flex items-center gap-1 rounded-md bg-emerald-500/15 px-2 py-0.5 font-semibold text-emerald-700 dark:text-emerald-400">
                    <Table2 className="size-3" /> {table.format}
                </span>
                <span className="text-muted-foreground">
                    <b className="font-semibold text-foreground">{formatCount(table.columns.length)}</b> cols ·{" "}
                    <b className="font-semibold text-foreground">{formatCount(total ?? table.totalRowsSampled)}</b>
                    {table.truncated && total === undefined ? "+" : ""} rows
                </span>
                <span className="font-mono text-[0.625rem] text-muted-foreground tabular-nums">{table.elapsedMs.toFixed(2)} ms</span>

                {extra}
                <div className="ml-auto flex items-center gap-1">
                    {mode === "grid" && (
                        <div className="relative w-36">
                            <Search className="pointer-events-none absolute left-2 top-1/2 size-3 -translate-y-1/2 text-muted-foreground" />
                            <input
                                type="text"
                                value={query}
                                onChange={(e) => setQuery(e.target.value)}
                                placeholder="Filter rows…"
                                className="h-6 w-full rounded border bg-background pl-6 pr-2 text-[0.6875rem] outline-hidden focus:border-primary"
                            />
                        </div>
                    )}
                    {raw && (
                        <Button
                            variant="outline"
                            size="sm"
                            className="h-6 px-2 text-[0.6875rem]"
                            onClick={() => setMode((m) => (m === "grid" ? "raw" : "grid"))}
                            title={mode === "grid" ? "Switch to raw text view" : "Switch to structured table grid"}
                        >
                            {mode === "grid" ? <FileCode /> : <Table2 />}
                            {mode === "grid" ? "Raw" : "Table"}
                        </Button>
                    )}
                </div>
            </div>

            {mode === "raw" ? (
                <div className="min-h-0 flex-1 overflow-auto select-text">
                    {rawText ? (
                        <SyntaxHighlighter
                            language={languageFor(entry.ext)}
                            useInlineStyles={false}
                            showLineNumbers={showLineNumbers}
                            wrapLines
                            lineProps={{ className: "block pl-3 pr-4" }}
                            className="min-w-max py-2 font-mono text-[0.8125rem] leading-5"
                        >
                            {rawText.text}
                        </SyntaxHighlighter>
                    ) : (
                        <Waiting />
                    )}
                </div>
            ) : (
                <div className="min-h-0 flex-1 overflow-auto select-text">
                    <table className="w-full border-collapse text-left font-mono text-[0.6875rem]">
                        <thead className="sticky top-0 z-10 bg-muted/90 backdrop-blur-xs">
                            <tr className="border-b">
                                <th className="w-9 border-r px-2 py-1.5 text-right font-normal text-muted-foreground select-none">#</th>
                                {table.columns.map((col, idx) => {
                                    const sorted = sortCol?.index === idx;
                                    return (
                                        <th
                                            key={idx}
                                            onClick={() =>
                                                setSortCol((prev) =>
                                                    prev?.index === idx
                                                        ? prev.desc
                                                            ? null
                                                            : { index: idx, desc: true }
                                                        : { index: idx, desc: false }
                                                )
                                            }
                                            className="max-w-[16rem] cursor-pointer border-r px-2.5 py-1.5 font-semibold text-foreground transition-colors select-none hover:bg-muted"
                                        >
                                            <div className="flex items-center gap-1.5">
                                                <ColumnTypeIcon kind={col.kind} />
                                                <span className="truncate" title={types?.[idx] ? `${col.name} · ${types[idx]}` : col.name}>
                                                    {col.name}
                                                </span>
                                                {types?.[idx] && <span className="shrink-0 text-[0.625rem] font-normal text-muted-foreground">{types[idx]}</span>}
                                                {sorted &&
                                                    (sortCol.desc ? (
                                                        <ArrowDown className="ml-auto size-3 shrink-0 text-primary" />
                                                    ) : (
                                                        <ArrowUp className="ml-auto size-3 shrink-0 text-primary" />
                                                    ))}
                                            </div>
                                        </th>
                                    );
                                })}
                            </tr>
                        </thead>
                        <tbody className="divide-y divide-border/50">
                            {rows.map((row, rIdx) => (
                                <tr key={rIdx} className="hover:bg-muted/30">
                                    <td className="border-r bg-muted/10 px-2 py-1 text-right text-[0.625rem] text-muted-foreground tabular-nums select-none">
                                        {rIdx + 1}
                                    </td>
                                    {table.columns.map((col, cIdx) => {
                                        const val = row[cIdx] ?? "";
                                        return (
                                            <td
                                                key={cIdx}
                                                title={val}
                                                className={cn(
                                                    "max-w-[16rem] truncate border-r px-2.5 py-1",
                                                    col.kind === "number" && "text-right tabular-nums text-amber-700 dark:text-amber-400",
                                                    col.kind === "boolean" && "text-emerald-700 dark:text-emerald-400"
                                                )}
                                            >
                                                {val}
                                            </td>
                                        );
                                    })}
                                </tr>
                            ))}
                        </tbody>
                    </table>
                    {table.truncated && (
                        <p className="border-t bg-muted/10 px-3 py-2 text-[0.6875rem] text-muted-foreground">
                            {total === undefined
                                ? `Showing the first ${formatCount(table.rows.length)} rows sampled from the file header.`
                                : `Showing the first ${formatCount(table.rows.length)} rows of ${formatCount(total)}. Open it to see the rest.`}
                        </p>
                    )}
                </div>
            )}
        </div>
    );
}
