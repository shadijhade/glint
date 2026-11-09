import { useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { invoke } from "@tauri-apps/api/core";
import { FileType } from "lucide-react";
import { type FileEntry, isArchived, isClosed, isOnlineOnly, isRecycled } from "@/lib/entries";
import { type ActionOutput, badOutput, offerActionUndo, runAction } from "@/lib/actions";
import { formatCount, plural } from "@/lib/format";
import { parentPath } from "@/lib/paths";
import { useTool } from "@/lib/tools";
import { ToolNeeded } from "../ToolNeeded";
import { Modal, ModalHeader } from "../Modal";
import { Choices, RunFooter, RunProgress, RunResult, useActionRun, WhereChooser } from "./ActionParts";

/** What pandoc reads (docconvert.rs's reader). */
const READS = new Set("docx odt md markdown mdown mkd html htm xhtml epub tex latex rtf rst org textile ipynb typ fb2".split(" "));
const extOf = (name: string) => (name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "");
const onDisk = (e: FileEntry) => !isArchived(e) && !isClosed(e) && !isRecycled(e) && !isOnlineOnly(e);
/** A document Convert document takes. */
export const canConvertDoc = (e: FileEntry) => onDisk(e) && !e.dir && READS.has(extOf(e.name));
/** A Typst file, for Export as PDF. */
export const isTypst = (e: FileEntry) => onDisk(e) && !e.dir && extOf(e.name) === "typ";

type Target = "docx" | "pdf" | "markdown" | "html" | "odt" | "epub" | "pptx" | "latex" | "rtf" | "text" | "rst" | "org" | "typst" | "ipynb";
const TARGETS: { value: Target; label: string; hint: string; ext: string[] }[] = [
    { value: "docx", label: "Word", hint: ".docx", ext: ["docx"] },
    { value: "pdf", label: "PDF", hint: "Typeset by Typst", ext: ["pdf"] },
    { value: "markdown", label: "Markdown", hint: ".md", ext: ["md", "markdown", "mdown", "mkd"] },
    { value: "html", label: "Web page", hint: ".html, pictures inside", ext: ["html", "htm", "xhtml"] },
    { value: "odt", label: "OpenDocument", hint: ".odt", ext: ["odt"] },
    { value: "epub", label: "E-book", hint: ".epub", ext: ["epub"] },
    { value: "pptx", label: "PowerPoint", hint: "Each heading a slide", ext: ["pptx"] },
    { value: "latex", label: "LaTeX", hint: ".tex", ext: ["tex", "latex"] },
    { value: "rtf", label: "Rich text", hint: ".rtf", ext: ["rtf"] },
    { value: "text", label: "Plain text", hint: ".txt", ext: ["txt"] },
    { value: "rst", label: "reStructuredText", hint: ".rst", ext: ["rst"] },
    { value: "org", label: "Org", hint: ".org", ext: ["org"] },
    { value: "typst", label: "Typst", hint: ".typ", ext: ["typ"] },
    { value: "ipynb", label: "Notebook", hint: ".ipynb", ext: ["ipynb"] },
];

const useDocConvert = create<{ files: FileEntry[] | null; target: Target }>(() => ({ files: null, target: "docx" }));

/** Opens Convert document on the documents among `entries`, `target` picked (PDF for Typst files, else Word, or Markdown for Word files). */
export function openDocConvert(entries: FileEntry[], target?: Target) {
    const files = entries.filter(canConvertDoc);
    if (!files.length) return void toast.message("Convert document takes Word, OpenDocument, Markdown, HTML, EPUB, LaTeX, RTF and other documents");
    const exts = new Set(files.map((f) => extOf(f.name)));
    const pick = target ?? (exts.size === 1 && exts.has("typ") ? "pdf" : exts.size === 1 && exts.has("docx") ? "markdown" : "docx");
    useDocConvert.setState({ files, target: pick });
}

/** Export as PDF for Typst files: typeset by Typst, beside each. */
export async function exportTypstPdf(entries: FileEntry[]) {
    const files = entries.filter(isTypst).map((e) => e.path);
    if (!files.length) return;
    try {
        const report = await runAction("doc_convert", { files, target: "pdf", output: { kind: "beside", suffix: "" } }, () => {});
        if (report.made.length) offerActionUndo(report, `PDF export of ${plural(report.made.length, "file")}`);
        if (report.failed.length) toast.error("Couldn't export it", { description: report.failed[0][1] });
    } catch (e) {
        toast.error("Couldn't export it", { description: String(e) });
    }
}

const close = () => useDocConvert.setState({ files: null });

export function DocConvertDialog() {
    const files = useDocConvert((s) => s.files);
    const target = useDocConvert((s) => s.target);
    if (!files) return null;
    return <DocConvertModal key={files.map((f) => f.path).join("\n")} files={files} first={target} />;
}

function DocConvertModal({ files, first }: { files: FileEntry[]; first: Target }) {
    const [target, setTarget] = useState<Target>(first);
    const [output, setOutput] = useState<ActionOutput>({ kind: "beside", suffix: "" });
    const run = useActionRun();
    const pandoc = useTool("pandoc");
    const typst = useTool("typst");
    const exts = new Set(files.map((f) => extOf(f.name)));
    // A format every chosen file is already in is no choice.
    const options = TARGETS.map((t) => ({ value: t.value, label: t.label, hint: t.hint, disabled: [...exts].every((e) => t.ext.includes(e)) }));
    const ready = !!pandoc?.origin && (target !== "pdf" || !!typst?.origin);
    const dirs = new Set(files.map((f) => parentPath(f.path).toLowerCase()));
    const title = files.length === 1 ? files[0].name : `${formatCount(files.length)} documents`;
    const fromWord = exts.has("docx") || exts.has("odt");

    const start = () =>
        void run.start(
            files.length,
            (p) => runAction("doc_convert", { files: files.map((f) => f.path), target, output }, p),
            (r) => `conversion of ${plural(r.made.length, "document")}`
        );

    return (
        <Modal label="Convert document" onClose={close} onSubmit={() => ready && !run.started && start()} className="max-h-[88vh] max-w-xl">
            <ModalHeader icon={FileType} title={`Convert ${title}`} subtitle={dirs.size === 1 ? parentPath(files[0].path) : `In ${formatCount(dirs.size)} folders`} onClose={close} />
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                {!run.started && (
                    <>
                        <ToolNeeded id="pandoc" why="pandoc converts documents on this PC, reading only the document it's given." icon={FileType} />
                        {target === "pdf" && <ToolNeeded id="typst" why="Typst typesets the PDF, without LaTeX or Word." icon={FileType} />}
                        <section className="space-y-1.5">
                            <h3 className="text-xs font-semibold text-muted-foreground">Into</h3>
                            <Choices label="Into" columns={3} value={target} onChange={setTarget} options={options} />
                        </section>
                        <p className="text-xs text-muted-foreground">
                            Text, headings, lists, tables, pictures, footnotes and links come across{fromWord ? "; Word's exact page layout doesn't (an exact copy of a Word file is Word's Save as PDF)" : ""}.
                            {target === "pptx" && " Each heading becomes a slide."}
                            {(target === "markdown" || target === "latex" || target === "rst" || target === "org" || target === "typst" || target === "text") && " Pictures go into a folder beside each."}
                            {" "}A document written right to left (Arabic, Hebrew) stays right to left.
                        </p>
                        <WhereChooser value={output} onChange={setOutput} noun="document" note="A name that's taken gets “(2)”, and Ctrl+Z takes it all back." />
                    </>
                )}
                {run.running && run.progress && <RunProgress progress={run.progress} verb="Converting" />}
                <RunResult report={run.report} error={run.error} noun="file" failedWhat="Couldn't convert" sizes={false} />
            </div>
            <RunFooter run={run} onClose={close} onStart={start} onStop={() => void invoke("doc_cancel")} startLabel="Convert" startIcon={FileType} disabled={!ready || badOutput(output) || !!options.find((o) => o.value === target)?.disabled} />
        </Modal>
    );
}
