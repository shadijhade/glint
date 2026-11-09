import { useEffect, useState } from "react";
import type { FileEntry } from "@/lib/entries";
import { readTextPreview } from "@/lib/fs";
import { languageNamed, SyntaxHighlighter } from "@/lib/highlight";
import { MarkdownView } from "./MarkdownView";
import { BigIcon, Waiting } from "./PreviewState";

/** The most of a notebook read for its preview. */
const MAX_BYTES = 32 * 1024 * 1024;

interface Cell {
    cell_type: "markdown" | "code" | "raw";
    source: string | string[];
    execution_count?: number | null;
    outputs?: Output[];
}

interface Output {
    output_type: "stream" | "execute_result" | "display_data" | "error";
    text?: string | string[];
    data?: Record<string, string | string[]>;
    ename?: string;
    evalue?: string;
}

const joined = (text: string | string[] | undefined) => (Array.isArray(text) ? text.join("") : (text ?? ""));

/** A Jupyter notebook as it reads: Markdown cells rendered, code highlighted, outputs (text and pictures) under each; nothing in it runs. */
export function NotebookView({ entry }: { entry: FileEntry }) {
    const [state, setState] = useState<{ cells: Cell[]; language: string } | { error: string } | null>(null);
    useEffect(() => {
        let stale = false;
        readTextPreview(entry.path, MAX_BYTES).then(
            ({ text, truncated }) => {
                if (stale) return;
                if (truncated) return setState({ error: "This notebook is too big to preview" });
                try {
                    const doc = JSON.parse(text);
                    const language = String(doc?.metadata?.kernelspec?.language ?? doc?.metadata?.language_info?.name ?? "python");
                    setState({ cells: Array.isArray(doc?.cells) ? doc.cells : [], language });
                } catch {
                    setState({ error: "It isn't a notebook Glint can read" });
                }
            },
            (e) => !stale && setState({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified]);

    if (!state) return <Waiting />;
    if ("error" in state) return <BigIcon entry={entry} note={state.error} />;
    const language = languageNamed(state.language);
    return (
        <div className="h-full space-y-3 overflow-y-auto p-3 text-sm">
            {state.cells.map((cell, i) =>
                cell.cell_type === "markdown" ? (
                    <MarkdownView key={i} text={joined(cell.source)} file={entry.path} archived={false} className="px-1" />
                ) : cell.cell_type === "code" ? (
                    <div key={i} className="space-y-1">
                        <div className="flex gap-2">
                            <span className="w-10 shrink-0 pt-1.5 text-end font-mono text-[0.6875rem] text-muted-foreground">[{cell.execution_count ?? " "}]</span>
                            <div className="min-w-0 flex-1 overflow-x-auto rounded-md border bg-muted/30">
                                <SyntaxHighlighter language={language} useInlineStyles={false} className="px-3 py-2 font-mono text-[0.8125rem] leading-5">
                                    {joined(cell.source)}
                                </SyntaxHighlighter>
                            </div>
                        </div>
                        {(cell.outputs ?? []).map((out, j) => (
                            <div key={j} className="ms-12 min-w-0">
                                <OutputView output={out} />
                            </div>
                        ))}
                    </div>
                ) : (
                    <pre key={i} className="whitespace-pre-wrap rounded-md border px-3 py-2 font-mono text-xs text-muted-foreground">
                        {joined(cell.source)}
                    </pre>
                )
            )}
        </div>
    );
}

/** A cell's output: a picture as a picture (SVG too, as an image, so nothing in it runs), else its text. */
function OutputView({ output }: { output: Output }) {
    if (output.output_type === "error")
        return (
            <pre className="whitespace-pre-wrap rounded-md bg-destructive/10 px-3 py-1.5 font-mono text-xs text-destructive">
                {output.ename}: {output.evalue}
            </pre>
        );
    const data = output.data ?? {};
    for (const type of ["image/png", "image/jpeg", "image/gif"]) {
        if (data[type]) return <img src={`data:${type};base64,${joined(data[type]).replace(/\s/g, "")}`} alt="Output" className="max-w-full rounded-sm bg-white" />;
    }
    if (data["image/svg+xml"]) return <img src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(joined(data["image/svg+xml"]))}`} alt="Output" className="max-w-full rounded-sm bg-white" />;
    const text = output.output_type === "stream" ? joined(output.text) : joined(data["text/plain"]);
    if (!text) return null;
    return <pre className="max-h-80 overflow-auto whitespace-pre-wrap px-1 font-mono text-xs text-muted-foreground">{text}</pre>;
}
