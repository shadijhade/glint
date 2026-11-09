import { Channel, invoke } from "@tauri-apps/api/core";
import { offerUndo, rememberUndo } from "./explorer";
import { fileOp } from "./fs";
import { baseName, parentPath } from "./paths";
import { plural } from "./format";

/** Where what a file action makes goes (actions.rs). */
export type ActionOutput = { kind: "beside"; suffix: string } | { kind: "folder"; name: string } | { kind: "replace" };

/** How far an action is: jobs done of all, the one just finished, and how far the one under way is (0–1). */
export interface ActionProgress {
    done: number;
    total: number;
    current: string;
    part: number;
}

/** What a file action did (actions.rs's Report). */
export interface ActionReport {
    made: string[];
    recycled: { path: string; bin: string }[];
    failed: [string, string][];
    before: number;
    after: number;
    stopped: boolean;
    elapsedMs: number;
}

/** Runs a file action's command with a progress channel. */
export function runAction(command: string, args: Record<string, unknown>, onProgress: (p: ActionProgress) => void) {
    const channel = new Channel<ActionProgress>();
    channel.onmessage = onProgress;
    return invoke<ActionReport>(command, { ...args, onProgress: channel });
}

/** Offers Undo for what an action made: what it made recycled, then the files it replaced restored. */
export function offerActionUndo(report: ActionReport, what: string) {
    if (!report.made.length) return;
    const made = report.made;
    const back = report.recycled;
    const step = rememberUndo(what, async () => {
        await fileOp({ kind: "delete", paths: made, permanent: false, confirm: false });
        if (back.length) await fileOp({ kind: "restore", items: back.map((r) => ({ path: r.bin, folder: parentPath(r.path), name: baseName(r.path) })) });
    });
    offerUndo(what.charAt(0).toUpperCase() + what.slice(1), step);
}

/** "3 files", for reports. */
export const madeCount = (report: ActionReport, noun = "file") => plural(report.made.length, noun);

const NOT_IN_NAMES = /[\\/:*?"<>|]|[. ]$/;

/** Whether a suffix or folder name can't be used. */
export function badOutput(output: ActionOutput) {
    if (output.kind === "beside") return NOT_IN_NAMES.test(output.suffix);
    if (output.kind === "folder") return !output.name.trim() || NOT_IN_NAMES.test(output.name);
    return false;
}

/** Export as PDF for Markdown files (print.rs): each drawn as the preview draws it, a PDF beside it. */
export async function exportMarkdownPdf(paths: string[], onDone: (report: ActionReport) => void) {
    const report = await runAction("markdown_pdf", { files: paths, output: { kind: "beside", suffix: "" } }, () => {});
    if (report.made.length) offerActionUndo(report, `PDF export of ${plural(report.made.length, "file")}`);
    onDone(report);
    return report;
}
