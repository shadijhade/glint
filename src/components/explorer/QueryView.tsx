import { useEffect, useRef, useState } from "react";
import { Loader2, Play } from "lucide-react";
import type { FileEntry } from "@/lib/entries";
import { type SqlAnswer, sqlRun, type SqlTables, sqlTables } from "@/lib/fs";
import { sqlName, useQueryDraft } from "@/lib/preview";
import { samePath } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { TableView } from "./DataPreview";
import { BigIcon, Waiting } from "./PreviewState";

const firstRows = (table: string) => `SELECT * FROM ${sqlName(table)} LIMIT 100`;

/**
 * SQL over a table file or a database, reading only (sql.rs): a CSV, TSV, JSON-lines or Parquet file
 * is the table `data`, a workbook's sheets are tables by their names. Ctrl+Enter runs; the first 1,000
 * rows of the answer show in the table view.
 */
export function QueryView({ entry }: { entry: FileEntry }) {
    const draft = useQueryDraft((s) => (s.draft && samePath(s.draft.path, entry.path) ? s.draft.sql : null));
    const [tables, setTables] = useState<SqlTables | { error: string } | null>(null);
    const [sql, setSql] = useState("");
    const [answer, setAnswer] = useState<(SqlAnswer & { at: number }) | { error: string } | null>(null);
    const [running, setRunning] = useState(false);
    const runs = useRef(0);

    const run = (text: string) => {
        const mine = ++runs.current;
        setRunning(true);
        sqlRun(entry.path, text).then(
            (result) => mine === runs.current && setAnswer({ ...result, at: mine }),
            (e) => mine === runs.current && setAnswer({ error: String(e) })
        ).finally(() => mine === runs.current && setRunning(false));
    };

    // The file is loaded (a database opened), then the first table's rows (or the query asked) run.
    useEffect(() => {
        let stale = false;
        setTables(null);
        setAnswer(null);
        sqlTables(entry.path).then(
            (found) => {
                if (stale) return;
                setTables(found);
                const first = draft ?? (found.tables.length ? firstRows(found.tables[0]) : "");
                setSql(first);
                if (first) run(first);
            },
            (e) => !stale && setTables({ error: String(e) })
        );
        return () => {
            stale = true;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [entry.path, entry.modified, draft]);

    if (!tables) return <Waiting what="Loading it for SQL…" />;
    if ("error" in tables) return <BigIcon entry={entry} note={tables.error} />;

    return (
        <div className="flex h-full flex-col">
            <div className="shrink-0 space-y-1.5 border-b p-2">
                <textarea
                    value={sql}
                    onChange={(e) => setSql(e.target.value)}
                    onKeyDown={(e) => {
                        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
                            e.preventDefault();
                            run(sql);
                        }
                    }}
                    rows={3}
                    spellCheck={false}
                    aria-label="SQL"
                    placeholder="SELECT * FROM data"
                    className="fluent-field block w-full resize-y px-2 py-1.5 font-mono text-xs leading-5 outline-hidden"
                />
                <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Button size="sm" className="h-6 px-2 text-xs" onClick={() => run(sql)} title="Run it (Ctrl+Enter)">
                        {running ? <Loader2 className="animate-spin" /> : <Play />} Run
                    </Button>
                    <div className="flex min-w-0 flex-1 gap-0.5 overflow-x-auto" role="list" aria-label="Tables">
                        {tables.tables.map((table) => (
                            <button
                                key={table}
                                type="button"
                                role="listitem"
                                onClick={() => {
                                    setSql(firstRows(table));
                                    run(firstRows(table));
                                }}
                                title={`Its first rows: ${firstRows(table)}`}
                                className="h-6 shrink-0 rounded px-1.5 font-mono text-[0.6875rem] hover:bg-muted hover:text-foreground"
                            >
                                {table}
                            </button>
                        ))}
                    </div>
                    {tables.loadMs >= 1 && <span className="shrink-0 tabular-nums">loaded in {Math.round(tables.loadMs)} ms</span>}
                </div>
            </div>
            {answer && "error" in answer ? (
                <pre className={cn("m-2 whitespace-pre-wrap rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 font-mono text-xs text-destructive")}>{answer.error}</pre>
            ) : answer ? (
                <div className="min-h-0 flex-1">
                    <TableView
                        key={answer.at}
                        entry={entry}
                        table={{ format: "SQL", delimiter: "", columns: answer.columns, rows: answer.rows, totalRowsSampled: answer.rows.length, truncated: answer.truncated, elapsedMs: answer.elapsedMs }}
                        more="The first 1,000 rows of the answer: LIMIT and OFFSET show others."
                    />
                </div>
            ) : (
                running && <Waiting />
            )}
        </div>
    );
}
