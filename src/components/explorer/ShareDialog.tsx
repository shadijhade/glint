import { useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { toast } from "sonner";
import { AlertTriangle, CheckCircle2, Copy, Download, FolderOpen, Loader2, Send, Square, X } from "lucide-react";
import { currentPath, revealInApp } from "@/lib/explorer";
import { formatSize, plural } from "@/lib/format";
import { baseName } from "@/lib/paths";
import { closeShare, openReceive, receive, type Receiving, type Sending, sendToPC, showShare, stopReceive, stopSend, useShare, useSharing } from "@/lib/share";
import { useTool } from "@/lib/tools";
import { cn } from "@/lib/utils";
import { ToolNeeded } from "../ToolNeeded";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";
import { secretsCheckFirst } from "./CheckDialog";

/** A ticket as the sending PC shows it, or its whole "sendme receive …" line (share.rs checks it again). */
const TICKET = /^(sendme rec(?:eive|v)\s+)?blob[a-z2-7]{56,}$/i;

/** Sends `path` to another PC, after a look for passwords and keys in it (when Kingfisher's there). */
export const offerToPC = (path: string) => void secretsCheckFirst([path], "Send anyway", () => void sendToPC(path));

/** In the title bar while something's being sent: the way back to its ticket, and Stop. */
export function SharingButton() {
    const sending = useSharing();
    if (!sending) return null;
    return (
        <Button variant="ghost" size="sm" className="text-primary hover:text-primary" onClick={() => showShare("send")} title="What you're sending to another PC: its ticket, and Stop">
            <Send /> Sending{sending > 1 ? ` ${sending}` : ""}
        </Button>
    );
}

export function ShareDialog() {
    const open = useShare((s) => s.open);
    if (!open) return null;
    return <ShareModal open={open} />;
}

function ShareModal({ open }: { open: "send" | "receive" }) {
    const title = open === "send" ? "Send to another PC" : "Receive from another PC";
    return (
        <Modal label={title} onClose={closeShare} className="max-h-[88vh] max-w-xl">
            <ModalHeader icon={open === "send" ? Send : Download} title={title} subtitle="With sendme: encrypted end to end, straight between the two PCs where they can reach each other" onClose={closeShare}>
                <div role="tablist" aria-label="Send or receive" className="flex shrink-0 rounded-md border p-0.5 text-xs">
                    {(["send", "receive"] as const).map((mode) => (
                        <button
                            key={mode}
                            type="button"
                            role="tab"
                            aria-selected={open === mode}
                            onClick={() => (mode === "send" ? showShare("send") : openReceive(useShare.getState().into || currentPath()))}
                            className={cn("rounded px-2 py-0.5 transition-colors", open === mode ? "bg-primary/15 text-primary" : "text-muted-foreground hover:text-foreground")}
                        >
                            {mode === "send" ? "Send" : "Receive"}
                        </button>
                    ))}
                </div>
            </ModalHeader>
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                <ToolNeeded
                    id="sendme"
                    icon={Send}
                    why="sendme sends files and folders straight to another PC, encrypted end to end, with no account and no size limit. The other PC needs it too."
                />
                {open === "send" ? <SendView /> : <ReceiveView />}
            </div>
        </Modal>
    );
}

function SendView() {
    const sends = useShare((s) => s.sends);
    if (!sends.length) return <p className="py-6 text-center text-muted-foreground">Right-click a file or a folder and choose Send to another PC, or find it in Ctrl+K.</p>;
    return (
        <>
            {sends.map((send) => (
                <SendCard key={send.key} send={send} />
            ))}
            <p className="text-xs text-muted-foreground">
                On the other PC, choose Receive from another PC in Glint (in Tools, or Ctrl+K) and paste the ticket, or run <code className="font-mono">sendme receive</code> with it. Anyone with the ticket
                can download what it offers while it's offered, so give it only to whom it's for. It's offered till you stop it or close Glint; keep it as it is till then.
            </p>
        </>
    );
}

function SendCard({ send }: { send: Sending }) {
    const copy = () => void navigator.clipboard.writeText(send.ticket!).then(() => toast.success("Copied the ticket", { duration: 1500 }), () => toast.error("Couldn't copy the ticket"));
    return (
        <section className="space-y-2.5 rounded-lg border p-3">
            <div className="flex items-center gap-2">
                <div className="min-w-0 flex-1">
                    <p className="truncate font-medium">
                        {baseName(send.path) || send.path}
                        {send.size && <span className="font-normal text-muted-foreground"> · {send.size}</span>}
                    </p>
                    <p className="truncate text-xs text-muted-foreground" title={send.path}>
                        {send.path}
                    </p>
                </div>
                <Button variant="outline" size="sm" className="h-7" onClick={() => stopSend(send.key)}>
                    {send.ended ? (
                        <>
                            <X /> Remove
                        </>
                    ) : (
                        <>
                            <Square className="size-3! fill-current" /> Stop sending
                        </>
                    )}
                </Button>
            </div>
            {send.ended ? (
                <p className="flex items-start gap-2 text-xs text-destructive">
                    <AlertTriangle className="mt-0.5 size-3.5 shrink-0" /> {send.ended}
                </p>
            ) : !send.ticket ? (
                <p className="flex items-center gap-2 text-muted-foreground">
                    <Loader2 className="size-4 animate-spin text-primary" /> Reading it through for its ticket…
                </p>
            ) : (
                <div className="flex gap-3">
                    {send.qr && <img src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(send.qr)}`} alt="The ticket as a QR code" className="size-32 shrink-0 rounded-md bg-white p-1" />}
                    <div className="flex min-w-0 flex-1 flex-col gap-2">
                        <textarea
                            readOnly
                            value={send.ticket}
                            aria-label="Ticket"
                            onFocus={(e) => e.currentTarget.select()}
                            className="min-h-20 flex-1 resize-none rounded-md border bg-muted/40 p-2 font-mono text-[0.6875rem] leading-snug break-all outline-hidden focus:border-primary"
                        />
                        <Button size="sm" className="self-start" onClick={copy}>
                            <Copy /> Copy the ticket
                        </Button>
                    </div>
                </div>
            )}
        </section>
    );
}

function ReceiveView() {
    const { receiving, into } = useShare(useShallow((s) => ({ receiving: s.receive, into: s.into })));
    const tool = useTool("sendme");
    const [ticket, setTicket] = useState("");
    if (receiving) return <ReceiveProgress receiving={receiving} />;
    const valid = TICKET.test(ticket.trim());
    return (
        <form
            className="space-y-3"
            onSubmit={(e) => {
                e.preventDefault();
                if (valid) void receive(ticket);
            }}
        >
            <label className="block space-y-1.5">
                <span className="text-xs font-semibold text-muted-foreground">The ticket the sending PC shows</span>
                <textarea
                    autoFocus
                    value={ticket}
                    onChange={(e) => setTicket(e.target.value)}
                    onKeyDown={(e) => {
                        // Enter receives; a ticket has no lines.
                        if (e.key === "Enter" && !e.shiftKey) {
                            e.preventDefault();
                            e.currentTarget.form?.requestSubmit();
                        }
                    }}
                    rows={4}
                    placeholder="blob…"
                    spellCheck={false}
                    aria-label="Ticket"
                    className={cn("w-full resize-none rounded-md border bg-background p-2 font-mono text-xs break-all outline-hidden focus:border-primary", ticket.trim() && !valid && "border-destructive")}
                />
            </label>
            {ticket.trim() && !valid && <p className="text-xs text-destructive">That isn't a whole ticket: it's the long line starting with “blob”.</p>}
            <p className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
                <FolderOpen className="size-3.5 shrink-0" /> Into
                <span className="truncate text-foreground" title={into}>
                    {into}
                </span>
            </p>
            <Button type="submit" disabled={!valid || !tool?.origin}>
                <Download /> Receive
            </Button>
        </form>
    );
}

function ReceiveProgress({ receiving: r }: { receiving: Receiving }) {
    if (r.done) {
        const path = r.done;
        return (
            <div className="flex items-start gap-3 py-2">
                <CheckCircle2 className="mt-0.5 size-5 shrink-0 text-emerald-500" />
                <div className="min-w-0 flex-1 space-y-2">
                    <p className="font-medium">Received “{baseName(path)}”</p>
                    <p className="truncate text-xs text-muted-foreground" title={path}>
                        {path}
                    </p>
                    <div className="flex gap-2">
                        <Button
                            size="sm"
                            onClick={() => {
                                closeShare();
                                void revealInApp(path);
                            }}
                        >
                            <FolderOpen /> Show
                        </Button>
                        <Button size="sm" variant="outline" onClick={stopReceive}>
                            Receive another
                        </Button>
                    </div>
                </div>
            </div>
        );
    }
    if (r.failed)
        return (
            <div className="flex items-start gap-3 py-2">
                <AlertTriangle className="mt-0.5 size-5 shrink-0 text-destructive" />
                <div className="min-w-0 flex-1 space-y-2">
                    <p className="font-medium">Couldn't receive it</p>
                    <p className="text-xs break-words text-muted-foreground">{r.failed}</p>
                    <Button size="sm" variant="outline" onClick={stopReceive}>
                        Try again
                    </Button>
                </div>
            </div>
        );
    const percent = r.total ? Math.min(100, Math.round((100 * r.bytes) / r.total)) : 0;
    return (
        <div className="space-y-2 py-4">
            <div className="flex items-center gap-2 text-muted-foreground">
                <Loader2 className="size-4 shrink-0 animate-spin text-primary" />
                <span className="min-w-0 flex-1 truncate">{r.files == null ? "Reaching the other PC…" : `Receiving ${plural(r.files, "file")}, ${r.size}`}</span>
                <Button variant="outline" size="sm" className="h-7" onClick={stopReceive}>
                    <Square className="size-3! fill-current" /> Stop
                </Button>
            </div>
            {r.total != null && (
                <>
                    <div className="h-1.5 overflow-hidden rounded-full bg-muted">
                        <div className="h-full rounded-full bg-primary transition-[width]" style={{ width: `${percent}%` }} />
                    </div>
                    <p className="text-xs tabular-nums text-muted-foreground">
                        {formatSize(Math.min(r.bytes, r.total))} of {r.size}, into {r.folder}
                    </p>
                </>
            )}
        </div>
    );
}
