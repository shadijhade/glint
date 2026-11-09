import { useEffect, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { invoke } from "@tauri-apps/api/core";
import { Database, FileCog, Sigma } from "lucide-react";
import { type FileEntry, isArchived, isClosed, isOnlineOnly, isRecycled } from "@/lib/entries";
import { type ActionOutput, badOutput, runAction } from "@/lib/actions";
import { formatCount, formatDuration, plural } from "@/lib/format";
import { parentPath } from "@/lib/paths";
import { refreshTools, useTool } from "@/lib/tools";
import { cn } from "@/lib/utils";
import { ToolNeeded } from "../ToolNeeded";
import { Modal, ModalHeader } from "../Modal";
import { Choices, RunFooter, RunProgress, RunResult, useActionRun, WhereChooser } from "./ActionParts";
import { BigIcon, Waiting } from "./PreviewState";
import { QueryView } from "./QueryView";

/** What DuckDB reads itself (duck.rs's READS). */
export const DUCK_READS = new Set(["csv", "tsv", "tab", "psv", "txt", "json", "jsonl", "ndjson", "parquet"]);
export const isDataFile = (e: FileEntry) => !e.dir && DUCK_READS.has(e.ext) && e.ext !== "txt" && !isArchived(e) && !isClosed(e) && !isRecycled(e) && !isOnlineOnly(e);

const DUCK_WHY = "DuckDB reads data files of any size where they are, in seconds, and describes every column. Nothing is uploaded.";

interface ColumnProfile {
    name: string;
    kind: string;
    duckType: string;
    count: number;
    nullPercent: number;
    distinct: number;
    min: string;
    max: string;
    mean: string;
    std: string;
    q25: string;
    q50: string;
    q75: string;
}

interface Profile {
    rows: number;
    columns: ColumnProfile[];
    elapsedMs: number;
}

/** Profile: each column of a data file described from all of its rows (DuckDB's SUMMARIZE). */
export function ProfileView({ entry }: { entry: FileEntry }) {
    const duck = useTool("duckdb");
    const [profile, setProfile] = useState<Profile | { error: string } | null>(null);
    const ready = !!duck?.origin;
    useEffect(() => {
        if (!ready) return;
        let stale = false;
        setProfile(null);
        invoke<Profile>("duck_profile", { path: entry.path }).then(
            (p) => !stale && setProfile(p),
            (e) => !stale && setProfile({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [ready, entry.path, entry.modified]);

    if (duck && !duck.origin) return <div className="p-4"><ToolNeeded id="duckdb" why={DUCK_WHY} icon={Database} /></div>;
    if (!profile) return <Waiting what="Reading every row…" />;
    if ("error" in profile) return <BigIcon entry={entry} note={profile.error} />;
    return (
        <div className="h-full overflow-auto p-2 text-xs">
            <p className="px-1 pb-2 text-muted-foreground">
                {plural(profile.rows, "row")} · {plural(profile.columns.length, "column")} · read by DuckDB in {formatDuration(profile.elapsedMs)}
            </p>
            <table className="w-full border-collapse">
                <thead className="sticky top-0 bg-background text-start text-muted-foreground">
                    <tr className="[&>th]:px-2 [&>th]:py-1 [&>th]:text-start [&>th]:font-medium">
                        <th>Column</th>
                        <th>Type</th>
                        <th title="Rows without a value">Missing</th>
                        <th title="About how many different values">Distinct</th>
                        <th>Smallest</th>
                        <th>Largest</th>
                        <th>Mean</th>
                        <th title="Lower quartile, median, upper quartile">Quartiles</th>
                    </tr>
                </thead>
                <tbody>
                    {profile.columns.map((c) => (
                        <tr key={c.name} className="border-t [&>td]:max-w-48 [&>td]:truncate [&>td]:px-2 [&>td]:py-1">
                            <td className="font-medium" title={c.name}>
                                {c.name}
                            </td>
                            <td className="font-mono text-muted-foreground" title={c.duckType}>
                                {c.duckType}
                            </td>
                            <td>
                                <div className="flex items-center gap-1.5">
                                    <div className="h-1.5 w-12 overflow-hidden rounded-full bg-muted">
                                        <div className={cn("h-full rounded-full", c.nullPercent > 20 ? "bg-amber-500" : "bg-primary")} style={{ width: `${Math.min(100, c.nullPercent)}%` }} />
                                    </div>
                                    <span className="tabular-nums">{c.nullPercent.toFixed(c.nullPercent && c.nullPercent < 1 ? 2 : 0)}%</span>
                                </div>
                            </td>
                            <td className="tabular-nums">{formatCount(c.distinct)}</td>
                            <td title={c.min}>{c.min}</td>
                            <td title={c.max}>{c.max}</td>
                            <td className="tabular-nums" title={c.std ? `Standard deviation ${c.std}` : undefined}>
                                {c.mean}
                            </td>
                            <td className="tabular-nums">{c.q50 ? `${c.q25} · ${c.q50} · ${c.q75}` : ""}</td>
                        </tr>
                    ))}
                </tbody>
            </table>
        </div>
    );
}

const useTogether = create<{ files: FileEntry[] | null }>(() => ({ files: null }));
const useDataConvert = create<{ files: FileEntry[] | null }>(() => ({ files: null }));

/** Opens Query together on the data files among `entries`: each a table named after it. */
export function queryTogether(entries: FileEntry[]) {
    const files = entries.filter(isDataFile);
    if (files.length < 2) return void toast.message("Choose two or more CSV, JSON or Parquet files");
    useTogether.setState({ files });
}

/** Opens Convert data on the data files among `entries`. */
export function convertData(entries: FileEntry[]) {
    const files = entries.filter(isDataFile);
    if (!files.length) return void toast.message("No CSV, JSON or Parquet files chosen");
    useDataConvert.setState({ files });
}

export function DataDialogs() {
    const together = useTogether((s) => s.files);
    const converting = useDataConvert((s) => s.files);
    return (
        <>
            {together && <TogetherModal key={together.map((f) => f.path).join("\n")} files={together} />}
            {converting && <ConvertModal key={converting.map((f) => f.path).join("\n")} files={converting} />}
        </>
    );
}

function TogetherModal({ files }: { files: FileEntry[] }) {
    const duck = useTool("duckdb");
    const close = () => useTogether.setState({ files: null });
    useEffect(() => {
        void refreshTools();
    }, []);
    return (
        <Modal label="Query together" onClose={close} className="h-[85vh] max-w-5xl">
            <ModalHeader icon={Sigma} title={`${formatCount(files.length)} files, queried together`} subtitle={files.map((f) => f.name).join(", ")} onClose={close} />
            <div className="min-h-0 flex-1">
                {duck && !duck.origin ? (
                    <div className="p-4">
                        <ToolNeeded id="duckdb" why={DUCK_WHY} icon={Database} />
                    </div>
                ) : duck ? (
                    <QueryView entry={files[0]} paths={files.map((f) => f.path)} />
                ) : (
                    <Waiting />
                )}
            </div>
        </Modal>
    );
}

type DataFormat = "csv" | "parquet" | "jsonl";

function ConvertModal({ files }: { files: FileEntry[] }) {
    const duck = useTool("duckdb");
    const [format, setFormat] = useState<DataFormat>(files.every((f) => f.ext === "parquet") ? "csv" : "parquet");
    const [output, setOutput] = useState<ActionOutput>({ kind: "beside", suffix: "" });
    const run = useActionRun();
    const close = () => useDataConvert.setState({ files: null });
    useEffect(() => {
        void refreshTools();
    }, []);
    const folders = new Set(files.map((f) => parentPath(f.path).toLowerCase()));
    const title = files.length === 1 ? files[0].name : `${formatCount(files.length)} data files`;
    const start = () =>
        void run.start(
            files.length,
            (p) => runAction("duck_convert", { files: files.map((f) => f.path), format, output }, p),
            (r) => `conversion of ${plural(r.made.length, "data file")}`
        );
    return (
        <Modal label={`Convert data: ${title}`} onClose={close} className="max-h-[88vh] max-w-xl">
            <ModalHeader icon={FileCog} title={title} subtitle={folders.size === 1 ? parentPath(files[0].path) : `In ${formatCount(folders.size)} folders`} onClose={close} />
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                {duck && !duck.origin && <ToolNeeded id="duckdb" why={DUCK_WHY} icon={Database} />}
                {!run.started && (
                    <>
                        <section className="space-y-1.5">
                            <h3 className="text-xs font-semibold text-muted-foreground">Into</h3>
                            <Choices
                                label="Format"
                                columns={3}
                                value={format}
                                onChange={setFormat}
                                options={[
                                    { value: "parquet", label: "Parquet", hint: "Columns, Zstandard: far smaller and faster" },
                                    { value: "csv", label: "CSV", hint: "Opens in Excel and everything" },
                                    { value: "jsonl", label: "JSON lines", hint: "An object a line, for code" },
                                ]}
                            />
                        </section>
                        <WhereChooser value={output} onChange={setOutput} noun="file" />
                    </>
                )}
                {run.running && run.progress && <RunProgress progress={run.progress} verb="Converting" />}
                <RunResult report={run.report} error={run.error} failedWhat="Couldn't convert" />
            </div>
            <RunFooter run={run} onClose={close} onStart={start} onStop={() => void invoke("duck_cancel")} startLabel="Convert" startIcon={FileCog} disabled={!duck?.origin || badOutput(output)} />
        </Modal>
    );
}
