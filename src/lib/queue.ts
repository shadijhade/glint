import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { toast } from "sonner";
import { plural } from "./format";
import { baseName, parentPath } from "./paths";

/** A job in the copy queue (queue.rs), as Rust tells it. */
export interface QueueJob {
    id: number;
    kind: "copy" | "move";
    sources: string[];
    dest: string;
    verify: boolean;
    state: "waiting" | "planning" | "copying" | "checking" | "paused" | "asking" | "done" | "cancelled" | "failed";
    files: number;
    filesDone: number;
    bytes: number;
    bytesDone: number;
    /** Files read back and found the same as their originals. */
    verified: number;
    current: string | null;
    /** Bytes a second, lately. */
    speed: number;
    started: number | null;
    finished: number | null;
    errors: { path: string; error: string }[];
    /** The name that's taken, while it asks what to do. */
    conflict: string | null;
    made: { from: string; to: string }[];
}

export const finished = (job: QueueJob) => job.state === "done" || job.state === "cancelled" || job.state === "failed";

/** The jobs, and whether the panel is open. */
export const useQueue = create<{ jobs: QueueJob[]; open: boolean }>(() => ({ jobs: [], open: false }));

/** What's done once a job finishes: Undo offered, and the folder it went into read again. Set by explorer.ts. */
let onFinished: (job: QueueJob) => void = () => {};
export const whenFinished = (handler: (job: QueueJob) => void) => {
    onFinished = handler;
};

let listening = false;
/** Listens to Rust's word of the jobs (once). */
export function listenQueue() {
    if (listening) return;
    listening = true;
    void listen<QueueJob[]>("copy-queue", ({ payload }) => {
        const before = new Map(useQueue.getState().jobs.map((j) => [j.id, j]));
        useQueue.setState({ jobs: payload });
        for (const job of payload) {
            const was = before.get(job.id);
            if (finished(job) && was && !finished(was)) onFinished(job);
        }
    });
}

/** Queues a copy or a move of `sources` into `dest`, and shows the queue. */
export async function queueFiles(kind: "copy" | "move", sources: string[], dest: string, verify: boolean) {
    listenQueue();
    try {
        await invoke<number>("queue_add", { kind, sources, dest, verify });
        useQueue.setState({ open: true });
    } catch (e) {
        toast.error(`Couldn't queue the ${kind === "move" ? "move" : "copy"}`, { description: String(e) });
    }
}

export const queuePause = (id: number) => invoke("queue_pause", { id });
export const queueResume = (id: number) => invoke("queue_resume", { id });
export const queueCancel = (id: number) => invoke("queue_cancel", { id });
export const queueAnswer = (id: number, answer: "replace" | "skip" | "both", all: boolean) => invoke("queue_answer", { id, answer, all });
export const queueClear = () => invoke("queue_clear");

/** A job in a few words: "Copying 3 items to “Backup”". */
export function jobTitle(job: QueueJob) {
    const what = job.sources.length === 1 ? `“${baseName(job.sources[0])}”` : plural(job.sources.length, "item");
    const verb = finished(job) ? (job.kind === "move" ? "Moved" : "Copied") : job.kind === "move" ? "Moving" : "Copying";
    return `${verb} ${what} to “${baseName(job.dest) || job.dest}”`;
}

/** Where a job's items came from, for its tooltip. */
export const jobFrom = (job: QueueJob) => [...new Set(job.sources.map(parentPath))].join(", ");
