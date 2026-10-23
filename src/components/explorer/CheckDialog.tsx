import { useMemo } from "react";
import { create } from "zustand";
import { open as pickPath } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { AlertTriangle, ExternalLink, FolderInput, FolderOpen, KeyRound, Loader2, ScanSearch, ShieldCheck, Square } from "lucide-react";
import { revealInApp } from "@/lib/explorer";
import { formatCount, formatDuration } from "@/lib/format";
import { cancelCheck, type CheckReport, type Finding, openItem as openPath, secretsScan, yaraScan } from "@/lib/fs";
import { baseName, parentPath } from "@/lib/paths";
import { useStore } from "@/lib/store";
import { refreshTools, useTool, useTools } from "@/lib/tools";
import { cn } from "@/lib/utils";
import { ToolNeeded } from "../ToolNeeded";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";

type Kind = "yara" | "secrets";

interface CheckState {
    open: boolean;
    kind: Kind;
    /** What's checked: a file, or a folder and what's in it. */
    target: string;
    running: boolean;
    report: CheckReport | null;
    /** What failed; "noTool" and "noRules" ask for what's missing. */
    error: string | null;
    /** What was asked for after the check (Zip anyway, when it found secrets in what's zipped). */
    then: { label: string; run: () => void } | null;
}

const useCheck = create<CheckState>(() => ({ open: false, kind: "secrets", target: "", running: false, report: null, error: null, then: null }));

const TOOL: Record<Kind, string> = { yara: "yara-x", secrets: "kingfisher" };

/** Whether a tool is installed, asking where the tools are the first time. */
async function ready(id: string) {
    if (!Object.keys(useTools.getState().tools).length) await refreshTools();
    return !!useTools.getState().tools[id]?.origin;
}

let asked = 0;

async function run(kind: Kind, target: string, then: CheckState["then"] = null) {
    const id = ++asked;
    useCheck.setState({ open: true, kind, target, running: true, report: null, error: null, then });
    const rules = useStore.getState().settings.yaraRules;
    const missing = !(await ready(TOOL[kind])) ? "noTool" : kind === "yara" && !rules ? "noRules" : null;
    if (missing) return useCheck.setState({ running: false, error: missing });
    try {
        const report = kind === "yara" ? await yaraScan(rules, target) : await secretsScan(target);
        if (id === asked) useCheck.setState({ running: false, report });
    } catch (e) {
        if (id === asked) useCheck.setState({ running: false, error: String(e) === "cancelled" ? "Stopped" : String(e) });
    }
}

/** Checks `target` (a file, or a folder and what's in it) with the YARA rules chosen in Settings. */
export const checkWithYara = (target: string) => void run("yara", target);

/** Looks for passwords, keys and tokens left in `target`, with Kingfisher (offline). */
export const checkForSecrets = (target: string) => void run("secrets", target);

/**
 * Before `paths` leave the PC (zipped, shared): with Kingfisher installed, looks for secrets in them
 * first, and asks before going on when it finds any; otherwise goes straight on. Skip in its toast
 * stops the check and goes on.
 */
export async function secretsCheckFirst(paths: string[], label: string, go: () => void) {
    if (!paths.length || !(await ready("kingfisher"))) return go();
    const id = toast.loading("Looking for passwords and keys first…", { duration: Infinity, action: { label: "Skip", onClick: () => void cancelCheck() } });
    const started = performance.now();
    const findings: Finding[] = [];
    try {
        for (const path of paths) findings.push(...(await secretsScan(path)).findings);
    } catch {
        // Skipped (or Kingfisher failed): it's an offer, not a gate.
        toast.dismiss(id);
        return go();
    }
    toast.dismiss(id);
    if (!findings.length) return go();
    asked++;
    const target = paths.length === 1 ? paths[0] : parentPath(paths[0]);
    useCheck.setState({ open: true, kind: "secrets", target, running: false, report: { findings, elapsedMs: performance.now() - started }, error: null, then: { label, run: go } });
}

function close() {
    if (useCheck.getState().running) void cancelCheck();
    asked++;
    useCheck.setState({ open: false });
}

/** Where a finding is, from the folder checked. */
function relative(path: string, target: string) {
    const base = target.replace(/\\+$/, "");
    return path.toLowerCase().startsWith(base.toLowerCase() + "\\") ? path.slice(base.length + 1) : baseName(path) || path;
}

/** What a check found (YARA rules matched, or secrets), by file. */
export function CheckDialog() {
    const { open, kind, target, running, report, error, then } = useCheck();
    const rules = useStore((s) => s.settings.yaraRules);
    const setSettings = useStore((s) => s.setSettings);
    const tool = useTool(TOOL[kind]);
    const byFile = useMemo(() => {
        const files = new Map<string, Finding[]>();
        for (const f of report?.findings ?? []) files.set(f.path, [...(files.get(f.path) ?? []), f]);
        return [...files];
    }, [report]);
    if (!open) return null;

    const name = baseName(target) || target;
    const secrets = kind === "secrets";
    const chooseRules = async () => {
        const picked = await pickPath({ directory: true, title: "A folder of YARA rules (.yar, .yara)", defaultPath: rules || undefined });
        if (typeof picked !== "string") return;
        setSettings({ yaraRules: picked });
        void run("yara", target, then);
    };
    const count = report?.findings.length ?? 0;

    return (
        <Modal label={secrets ? "Secrets" : "YARA rules"} onClose={close} className="max-h-[86vh] max-w-3xl">
            <ModalHeader
                icon={secrets ? KeyRound : ScanSearch}
                title={secrets ? `Passwords and keys in “${name}”` : `YARA rules in “${name}”`}
                subtitle={target}
                onClose={close}
                onRefresh={running ? undefined : () => void run(kind, target, then)}
                refreshing={running}
            >
                {running && (
                    <Button variant="outline" size="sm" className="h-7" onClick={() => void cancelCheck()}>
                        <Square className="size-3! fill-current" /> Stop
                    </Button>
                )}
            </ModalHeader>

            <div className="min-h-[14rem] flex-1 overflow-y-auto p-4 text-xs select-text">
                {running && (
                    <div className="flex h-48 flex-col items-center justify-center gap-2.5 text-sm text-muted-foreground">
                        <Loader2 className="size-6 animate-spin text-primary" />
                        <p>{secrets ? `Kingfisher is looking through “${name}”…` : `YARA-X is checking “${name}” with ${baseName(rules) || rules}…`}</p>
                    </div>
                )}
                {!running && error === "noTool" && (
                    <div className="space-y-3">
                        <ToolNeeded
                            id={TOOL[kind]}
                            why={secrets ? "Kingfisher, MongoDB's secret scanner, finds passwords, keys and tokens left in files. It runs on this PC only." : "YARA-X (VirusTotal's) checks files against YARA rules."}
                            icon={secrets ? KeyRound : ScanSearch}
                        />
                        {tool?.origin && (
                            <Button size="sm" onClick={() => void run(kind, target, then)}>
                                Check now
                            </Button>
                        )}
                    </div>
                )}
                {!running && error === "noRules" && (
                    <div className="flex h-44 flex-col items-center justify-center gap-2 text-center text-sm">
                        <ScanSearch className="size-7 stroke-[1.25] text-muted-foreground" />
                        <p className="font-medium">Which YARA rules?</p>
                        <p className="max-w-md text-xs text-muted-foreground">A folder of .yar and .yara files, such as one of the community collections; it's kept in Settings → Check.</p>
                        <Button size="sm" className="mt-1" onClick={() => void chooseRules()}>
                            <FolderOpen /> Choose a rules folder
                        </Button>
                    </div>
                )}
                {!running && error && error !== "noTool" && error !== "noRules" && (
                    <div className="flex h-44 flex-col items-center justify-center gap-2 text-center text-sm">
                        <AlertTriangle className="size-6 text-destructive" />
                        <p className="font-medium">{error === "Stopped" ? "Stopped" : "Couldn't check"}</p>
                        {error !== "Stopped" && <p className="max-w-lg text-xs text-muted-foreground">{error}</p>}
                    </div>
                )}
                {!running && !error && report && count === 0 && (
                    <div className="flex h-44 flex-col items-center justify-center gap-2 text-center text-sm text-muted-foreground">
                        <ShieldCheck className="size-7 stroke-[1.25] text-emerald-500" />
                        <p className="font-medium text-foreground">{secrets ? "No passwords or keys found" : "No rule matched"}</p>
                        <p className="text-xs">{secrets ? "Kingfisher looked through the files as they are (not Git's history), and nothing left the PC." : `Checked with ${rules}.`}</p>
                    </div>
                )}
                {!running && !error && count > 0 && (
                    <div className="space-y-2">
                        {secrets && (
                            <p className="rounded-md bg-amber-500/10 px-3 py-2 text-amber-800 dark:text-amber-300">
                                These look like passwords, keys or tokens. Anyone who gets these files can use them: take them out, or change them where they work.
                            </p>
                        )}
                        <div className="divide-y rounded-lg border bg-card">
                            {byFile.map(([path, found]) => (
                                <div key={path} className="group px-3 py-2">
                                    <div className="flex items-center gap-2">
                                        <span className="min-w-0 flex-1 truncate font-medium text-foreground" title={path}>
                                            {relative(path, target)}
                                        </span>
                                        <Button variant="ghost" size="icon" className="size-6" title="Open the file" onClick={() => void openPath(path)}>
                                            <ExternalLink className="size-3.5" />
                                        </Button>
                                        <Button
                                            variant="ghost"
                                            size="icon"
                                            className="size-6"
                                            title="Show it in its folder"
                                            onClick={() => {
                                                close();
                                                void revealInApp(path);
                                            }}
                                        >
                                            <FolderInput className="size-3.5" />
                                        </Button>
                                    </div>
                                    <ul className="mt-1 space-y-0.5">
                                        {found.map((f, i) => (
                                            <li key={i} className="flex items-baseline gap-2 text-muted-foreground">
                                                {f.line > 0 && <span className="w-14 shrink-0 tabular-nums">line {f.line}</span>}
                                                <span className="min-w-0 flex-1 truncate" title={f.title || f.rule}>
                                                    {secrets ? f.title || f.rule : f.rule}
                                                    {!secrets && f.title && <span> · {f.title}</span>}
                                                </span>
                                                {f.value && <code className="shrink-0 rounded bg-muted px-1.5 font-mono text-[0.6875rem] text-foreground">{f.value}</code>}
                                                {f.confidence && (
                                                    <span className={cn("shrink-0 rounded px-1.5 text-[0.625rem] font-medium uppercase", f.confidence === "high" ? "bg-destructive/15 text-destructive" : "bg-muted")}>{f.confidence}</span>
                                                )}
                                            </li>
                                        ))}
                                    </ul>
                                </div>
                            ))}
                        </div>
                    </div>
                )}
            </div>

            <div className="flex items-center justify-between gap-3 border-t bg-muted/20 px-4 py-2.5 text-xs text-muted-foreground">
                <span className="truncate">
                    {report && !running && `${formatCount(count)} ${count === 1 ? "finding" : "findings"}${byFile.length > 1 ? ` in ${formatCount(byFile.length)} files` : ""} · ${formatDuration(report.elapsedMs)}`}
                    {!secrets && rules && (
                        <button type="button" className="ml-2 underline underline-offset-2 hover:text-foreground" onClick={() => void chooseRules()}>
                            Other rules…
                        </button>
                    )}
                </span>
                <div className="flex gap-2">
                    {then && count > 0 && !running && (
                        <Button
                            variant="outline"
                            size="sm"
                            onClick={() => {
                                const go = then.run;
                                close();
                                go();
                            }}
                        >
                            {then.label}
                        </Button>
                    )}
                    <Button variant={then && count > 0 ? "default" : "outline"} size="sm" onClick={close}>
                        {then && count > 0 ? "Cancel" : "Close"}
                    </Button>
                </div>
            </div>
        </Modal>
    );
}
