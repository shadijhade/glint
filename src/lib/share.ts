import { create } from "zustand";
import { toast } from "sonner";
import { revealInApp } from "./explorer";
import { receiveStart, sendStart, shareStop } from "./fs";
import { baseName, samePath } from "./paths";

/** A file or folder offered to another PC with sendme (share.rs), from Glint's start to its Stop. */
export interface Sending {
    key: number;
    path: string;
    /** Its run's id, once sendme has started. */
    id: number | null;
    /** What receives it, once sendme has read it through; and as a QR code (an SVG). */
    ticket?: string;
    size?: string;
    qr?: string;
    /** Why it ended by itself. */
    ended?: string;
}

/** What's being received, from a ticket into a folder. */
export interface Receiving {
    key: number;
    id: number | null;
    folder: string;
    files?: number;
    size?: string;
    /** Its size in bytes, and how much is here. */
    total?: number;
    bytes: number;
    /** Where it went. */
    done?: string;
    failed?: string;
}

interface ShareState {
    /** What the dialog shows, while it's open. */
    open: "send" | "receive" | null;
    sends: Sending[];
    receive: Receiving | null;
    /** Where Receive puts what comes. */
    into: string;
}

export const useShare = create<ShareState>(() => ({ open: null, sends: [], receive: null, into: "" }));
const set = useShare.setState;
const get = useShare.getState;
let next = 0;

export const closeShare = () => set({ open: null });
export const showShare = (open: "send" | "receive") => set({ open });

/** How many sends are on: the title bar says so while there are. */
export const useSharing = () => useShare((s) => s.sends.filter((x) => !x.ended).length);

const patchSend = (key: number, patch: Partial<Sending>) => set((s) => ({ sends: s.sends.map((x) => (x.key === key ? { ...x, ...patch } : x)) }));

/** Offers `path` to another PC: the dialog opens on it, its ticket showing once sendme has read it. */
export async function sendToPC(path: string) {
    set({ open: "send" });
    if (get().sends.some((s) => samePath(s.path, path) && !s.ended)) return;
    const key = ++next;
    set((s) => ({ sends: [...s.sends, { key, path, id: null }] }));
    try {
        const id = await sendStart(path, (e) => patchSend(key, e.kind === "ready" ? { ticket: e.ticket, size: e.size, qr: e.qr } : { ended: e.message }));
        // Stopped while it started.
        if (!get().sends.some((s) => s.key === key)) return void shareStop(id);
        patchSend(key, { id });
    } catch (e) {
        patchSend(key, { ended: String(e) });
    }
}

/** Stops a send (its ticket stops working), or takes away one that ended. */
export function stopSend(key: number) {
    const send = get().sends.find((s) => s.key === key);
    if (send?.id != null && !send.ended) void shareStop(send.id);
    set((s) => ({ sends: s.sends.filter((x) => x.key !== key) }));
}

/** Opens Receive, into `folder` (unless something's coming already). */
export function openReceive(folder: string) {
    const running = get().receive;
    set({ open: "receive", ...(running && !running.done && !running.failed ? {} : { into: folder, receive: null }) });
}

/** Receives what `ticket` offers into the folder Receive was opened on. */
export async function receive(ticket: string) {
    const key = ++next;
    const folder = get().into;
    const patch = (change: Partial<Receiving>) => set((s) => (s.receive?.key === key ? { receive: { ...s.receive, ...change } } : {}));
    set({ receive: { key, id: null, folder, bytes: 0 } });
    try {
        const id = await receiveStart(folder, ticket, (e) => {
            if (e.kind === "getting") patch({ files: e.files, size: e.size, total: e.bytes });
            else if (e.kind === "progress") patch({ bytes: e.bytes });
            else if (e.kind === "failed") patch({ failed: e.message });
            else {
                patch({ done: e.path });
                if (get().open !== "receive") toast.success(`Received “${baseName(e.path)}”`, { action: { label: "Show", onClick: () => void revealInApp(e.path) } });
            }
        });
        if (get().receive?.key !== key) return void shareStop(id);
        patch({ id });
    } catch (e) {
        patch({ failed: String(e) });
    }
}

/** Stops what's coming (what came so far goes), or clears what's done for the next. */
export function stopReceive() {
    const r = get().receive;
    if (r?.id != null && !r.done && !r.failed) void shareStop(r.id);
    set({ receive: null });
}
