import { useEffect, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { invoke } from "@tauri-apps/api/core";
import { ArrowDown, ArrowUp, FileStack, FileText, Images, KeyRound, LockOpen, Minimize2, RotateCw, Scissors, Stamp, X } from "lucide-react";
import { type FileEntry, isArchived, isClosed, isOnlineOnly, isRecycled } from "@/lib/entries";
import { type ActionOutput, badOutput, runAction } from "@/lib/actions";
import { formatCount, plural } from "@/lib/format";
import { parentPath } from "@/lib/paths";
import { refreshTools, useTool } from "@/lib/tools";
import { ToolNeeded } from "../ToolNeeded";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";
import { RunFooter, RunProgress, RunResult, useActionRun, WhereChooser } from "./ActionParts";

export type PdfMode = "merge" | "split" | "keep" | "remove" | "rotate" | "optimize" | "images" | "encrypt" | "decrypt" | "stamp";

/** A PDF on disk, there to be read. */
export const isPdfFile = (e: FileEntry) => !e.dir && e.ext === "pdf" && !isArchived(e) && !isClosed(e) && !isRecycled(e) && !isOnlineOnly(e);

const usePdf = create<{ files: FileEntry[] | null; mode: PdfMode }>(() => ({ files: null, mode: "optimize" }));

/** Opens PDF tools on the PDFs among `entries`, at `mode`. */
export function openPdfTools(entries: FileEntry[], mode: PdfMode) {
    const files = entries.filter(isPdfFile);
    if (!files.length) return void toast.message("No PDFs chosen");
    usePdf.setState({ files, mode: mode === "merge" && files.length < 2 ? "split" : mode });
}

const close = () => usePdf.setState({ files: null });

const MODES: { mode: PdfMode; label: string; icon: typeof FileText; hint: string }[] = [
    { mode: "merge", label: "Merge", icon: FileStack, hint: "The PDFs as one, in the order below" },
    { mode: "split", label: "Split", icon: Scissors, hint: "Into parts of so many pages" },
    { mode: "keep", label: "Keep pages", icon: FileText, hint: "A new PDF of only the pages listed" },
    { mode: "remove", label: "Take out pages", icon: X, hint: "A new PDF without the pages listed" },
    { mode: "rotate", label: "Turn pages", icon: RotateCw, hint: "Pages turned a quarter or half" },
    { mode: "optimize", label: "Make smaller", icon: Minimize2, hint: "What's repeated or unused taken out" },
    { mode: "images", label: "Pictures", icon: Images, hint: "Every picture in it, as files" },
    { mode: "encrypt", label: "Add a password", icon: KeyRound, hint: "Needed to open it (AES-256)" },
    { mode: "decrypt", label: "Take the password off", icon: LockOpen, hint: "With the password it has" },
    { mode: "stamp", label: "Stamp", icon: Stamp, hint: "Text across every page: DRAFT, COPY…" },
];

/** pdfcpu's page lists: "1-3, 5, 8-", "even", "odd", "l" for the last, "!2" to leave one out. */
const validPages = (pages: string) =>
    pages.trim() !== "" &&
    pages.split(",").every((p) => {
        const q = p.trim().replace(/^!/, "");
        return q === "even" || q === "odd" || q === "l" || (/^[\dl-]+$/.test(q) && (q.match(/-/g) ?? []).length <= 1);
    });

export function PdfDialog() {
    const files = usePdf((s) => s.files);
    const mode = usePdf((s) => s.mode);
    if (!files) return null;
    return <PdfModal key={files.map((f) => f.path).join("\n")} files={files} initial={mode} />;
}

function PdfModal({ files, initial }: { files: FileEntry[]; initial: PdfMode }) {
    const tool = useTool("pdfcpu");
    const [mode, setMode] = useState<PdfMode>(initial);
    const [order, setOrder] = useState(files);
    const [name, setName] = useState("Merged");
    const [every, setEvery] = useState(1);
    const [pages, setPages] = useState("");
    const [degrees, setDegrees] = useState(90);
    const [password, setPassword] = useState("");
    const [again, setAgain] = useState("");
    const [text, setText] = useState("DRAFT");
    const [opacity, setOpacity] = useState(0.3);
    const [diagonal, setDiagonal] = useState(true);
    const [count, setCount] = useState<number | null>(null);
    const [output, setOutput] = useState<ActionOutput>({ kind: "beside", suffix: "" });
    const run = useActionRun();
    const ready = !!tool?.origin;

    useEffect(() => {
        void refreshTools();
    }, []);
    useEffect(() => {
        if (ready && files.length === 1) invoke<{ pages: number }>("pdf_info", { path: files[0].path }).then((i) => setCount(i.pages || null), () => {});
    }, [ready, files]);
    // Parts and pictures are many files: into a folder named after the PDF by default.
    useEffect(() => {
        if (mode === "split" || mode === "images") setOutput({ kind: "folder", name: files.length === 1 ? `${files[0].name.replace(/\.pdf$/i, "")} ${mode === "split" ? "parts" : "pictures"}` : mode === "split" ? "Parts" : "Pictures" });
        else setOutput({ kind: "beside", suffix: "" });
    }, [mode, files]);

    const folders = new Set(files.map((f) => parentPath(f.path).toLowerCase()));
    const title = files.length === 1 ? files[0].name : `${formatCount(files.length)} PDFs`;
    const subtitle = (folders.size === 1 ? parentPath(files[0].path) : `In ${formatCount(folders.size)} folders`) + (count ? ` · ${plural(count, "page")}` : "");
    const pagesNeeded = mode === "keep" || mode === "remove";
    const pagesBad = (pagesNeeded && !validPages(pages)) || (mode === "rotate" && pages.trim() !== "" && !validPages(pages));
    const passwordBad = (mode === "encrypt" && (!password || password !== again)) || (mode === "decrypt" && !password);
    const nameBad = mode === "merge" && (!name.trim() || /[\\/:*?"<>|]|[. ]$/.test(name));

    const task = () => {
        switch (mode) {
            case "merge":
                return { kind: "merge", name: name.trim() };
            case "split":
                return { kind: "split", every };
            case "keep":
                return { kind: "keep", pages };
            case "remove":
                return { kind: "remove", pages };
            case "rotate":
                return { kind: "rotate", pages, degrees };
            case "encrypt":
                return { kind: "encrypt", password };
            case "decrypt":
                return { kind: "decrypt", password };
            case "stamp":
                return { kind: "stamp", text, opacity, diagonal };
            default:
                return { kind: mode };
        }
    };

    const start = () => {
        const list = mode === "merge" ? order : files;
        void run.start(
            mode === "merge" ? 1 : list.length,
            (p) =>
                runAction(
                    "pdf_run",
                    {
                        files: list.map((f) => f.path),
                        task: task(),
                        output: mode === "merge" && output.kind === "replace" ? { kind: "beside", suffix: "" } : output,
                    },
                    p
                ),
            (r) => `${MODES.find((m) => m.mode === mode)!.label.toLowerCase()} (${plural(r.made.length, "file")})`
        );
    };

    const move = (i: number, by: number) =>
        setOrder((o) => {
            const next = [...o];
            const [it] = next.splice(i, 1);
            next.splice(Math.max(0, Math.min(next.length, i + by)), 0, it);
            return next;
        });

    const current = MODES.find((m) => m.mode === mode)!;
    return (
        <Modal label={`PDF tools: ${title}`} onClose={close} className="max-h-[88vh] max-w-2xl">
            <ModalHeader icon={FileText} title={title} subtitle={subtitle} onClose={close} />
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                {tool && !tool.origin && <ToolNeeded id="pdfcpu" why="pdfcpu merges, splits, shrinks and locks PDFs on this PC: nothing is uploaded anywhere." icon={FileText} />}

                {!run.started && (
                    <>
                        <div role="radiogroup" aria-label="What to do" className="grid grid-cols-5 gap-1.5">
                            {MODES.map((m) => (
                                <button
                                    key={m.mode}
                                    type="button"
                                    role="radio"
                                    aria-checked={mode === m.mode}
                                    disabled={m.mode === "merge" && files.length < 2}
                                    title={m.hint}
                                    onClick={() => setMode(m.mode)}
                                    className={`flex flex-col items-center gap-1 rounded-md border px-1 py-2 text-center text-[0.6875rem] font-medium transition-colors hover:bg-accent disabled:opacity-40 ${mode === m.mode ? "border-primary bg-primary/10" : ""}`}
                                >
                                    <m.icon className="size-4" />
                                    {m.label}
                                </button>
                            ))}
                        </div>
                        <p className="text-xs text-muted-foreground">{current.hint}.</p>

                        {mode === "merge" && (
                            <>
                                <ol className="max-h-56 space-y-1 overflow-y-auto rounded-md border p-1.5">
                                    {order.map((f, i) => (
                                        <li key={f.path} className="flex items-center gap-2 rounded px-1.5 py-1 hover:bg-accent/50">
                                            <span className="w-5 text-end text-xs tabular-nums text-muted-foreground">{i + 1}</span>
                                            <span className="min-w-0 flex-1 truncate" title={f.path}>
                                                {f.name}
                                            </span>
                                            <Button variant="ghost" size="icon" className="size-6" disabled={i === 0} onClick={() => move(i, -1)} aria-label="Earlier">
                                                <ArrowUp className="size-3.5" />
                                            </Button>
                                            <Button variant="ghost" size="icon" className="size-6" disabled={i === order.length - 1} onClick={() => move(i, 1)} aria-label="Later">
                                                <ArrowDown className="size-3.5" />
                                            </Button>
                                        </li>
                                    ))}
                                </ol>
                                <label className="flex items-center gap-3">
                                    <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Named</span>
                                    <input value={name} onChange={(e) => setName(e.target.value)} aria-label="Name" className={`h-8 min-w-0 flex-1 rounded-md border bg-background px-2 ${nameBad ? "border-destructive" : ""}`} />
                                    <span className="text-muted-foreground">.pdf</span>
                                </label>
                            </>
                        )}

                        {mode === "split" && (
                            <label className="flex items-center gap-3">
                                <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Each part</span>
                                <input type="number" min={1} value={every} onChange={(e) => setEvery(Math.max(1, Number(e.target.value) || 1))} aria-label="Pages in each part" className="h-8 w-24 rounded-md border bg-background px-2" />
                                <span className="text-muted-foreground">{every === 1 ? "page: a PDF a page" : "pages"}</span>
                            </label>
                        )}

                        {(pagesNeeded || mode === "rotate") && (
                            <label className="flex items-center gap-3">
                                <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Pages</span>
                                <input
                                    value={pages}
                                    onChange={(e) => setPages(e.target.value)}
                                    placeholder={mode === "rotate" ? "All of them" : "1-3, 5, 8-"}
                                    aria-label="Pages"
                                    spellCheck={false}
                                    className={`h-8 min-w-0 flex-1 rounded-md border bg-background px-2 ${pages && pagesBad ? "border-destructive" : ""}`}
                                />
                            </label>
                        )}
                        {(pagesNeeded || mode === "rotate") && <p className="ps-[5.75rem] text-xs text-muted-foreground">Numbers and ranges with commas: 1-3, 5, 8- (to the end); even, odd, l (the last page).</p>}

                        {mode === "rotate" && (
                            <label className="flex items-center gap-3">
                                <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Turn</span>
                                <select value={degrees} onChange={(e) => setDegrees(Number(e.target.value))} aria-label="Turn" className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2">
                                    <option value={90}>A quarter clockwise</option>
                                    <option value={180}>Upside down</option>
                                    <option value={270}>A quarter anticlockwise</option>
                                </select>
                            </label>
                        )}

                        {(mode === "encrypt" || mode === "decrypt") && (
                            <div className="space-y-2">
                                <label className="flex items-center gap-3">
                                    <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Password</span>
                                    <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} aria-label="Password" autoComplete="new-password" className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2" />
                                </label>
                                {mode === "encrypt" && (
                                    <label className="flex items-center gap-3">
                                        <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Again</span>
                                        <input type="password" value={again} onChange={(e) => setAgain(e.target.value)} aria-label="The password again" autoComplete="new-password" className={`h-8 min-w-0 flex-1 rounded-md border bg-background px-2 ${again && again !== password ? "border-destructive" : ""}`} />
                                    </label>
                                )}
                                {mode === "encrypt" && <p className="ps-[5.75rem] text-xs text-muted-foreground">Nobody can open it without this password, Glint included: keep it somewhere safe.</p>}
                            </div>
                        )}

                        {mode === "stamp" && (
                            <div className="space-y-2">
                                <label className="flex items-center gap-3">
                                    <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Text</span>
                                    <input value={text} onChange={(e) => setText(e.target.value)} aria-label="Stamp text" className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2" />
                                </label>
                                <label className="flex items-center gap-3">
                                    <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Strength</span>
                                    <input type="range" min={0.1} max={1} step={0.05} value={opacity} onChange={(e) => setOpacity(Number(e.target.value))} className="min-w-0 flex-1 accent-primary" />
                                    <span className="w-10 text-end tabular-nums">{Math.round(opacity * 100)}%</span>
                                </label>
                                <label className="flex items-center gap-2 ps-[5.75rem]">
                                    <input type="checkbox" checked={diagonal} onChange={(e) => setDiagonal(e.target.checked)} className="accent-primary" /> Across, corner to corner
                                </label>
                            </div>
                        )}

                        <WhereChooser value={output} onChange={setOutput} noun="PDF" replace={mode !== "merge" && mode !== "split" && mode !== "images"} />
                    </>
                )}

                {run.running && run.progress && <RunProgress progress={run.progress} verb="Working on" />}
                <RunResult report={run.report} error={run.error} failedWhat="Couldn't do it" sizes={mode === "optimize"} />
            </div>
            <RunFooter
                run={run}
                onClose={close}
                onStart={start}
                onStop={() => void invoke("pdf_cancel")}
                startLabel={current.label}
                startIcon={current.icon}
                disabled={!ready || badOutput(output) || pagesBad || passwordBad || nameBad || (mode === "stamp" && !text.trim())}
            />
        </Modal>
    );
}
