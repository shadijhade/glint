import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { toast } from "sonner";
import { openUrl } from "@tauri-apps/plugin-opener";
import { AlertTriangle, CheckCircle2, Copy, Download, ExternalLink, FolderOpen, Loader2, Network, Send, Shield, ShieldAlert, Square, Wifi, X } from "lucide-react";
import { currentPath, revealInApp } from "@/lib/explorer";
import { formatSize, plural } from "@/lib/format";
import { type LanAddress, lanAddresses, lanOpenSettings, type LanReach } from "@/lib/fs";
import { baseName, samePath } from "@/lib/paths";
import {
    closeShare,
    openNetworkShare,
    openReceive,
    receive,
    type Receiving,
    type Sending,
    serve,
    type Serving,
    type ShareView,
    sendToPC,
    showShare,
    stopReceive,
    stopSend,
    stopServing,
    useShare,
    useSharing,
} from "@/lib/share";
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

/** Shares `path` on this network (its choices first), after the same look. */
export const offerOnNetwork = (path: string) => void secretsCheckFirst([path], "Share anyway", () => openNetworkShare(path));

const copy = (text: string, what: string) => void navigator.clipboard.writeText(text).then(() => toast.success(`Copied the ${what}`, { duration: 1500 }), () => toast.error(`Couldn't copy the ${what}`));

/** In the title bar while something's sent or shared: the way back to it, and Stop. */
export function SharingButton() {
    const { count, view } = useSharing();
    if (!count) return null;
    const label = count === 1 && view === "send" ? "Sending" : count > 1 ? `Sharing ${count}` : "Sharing";
    return (
        <Button variant="ghost" size="sm" className="text-primary hover:text-primary" onClick={() => showShare(view)} title="What you're sending or sharing: its ticket or link, and Stop">
            {view === "send" ? <Send /> : <Wifi />} {label}
        </Button>
    );
}

export function ShareDialog() {
    const open = useShare((s) => s.open);
    if (!open) return null;
    return <ShareModal open={open} />;
}

const VIEWS: { view: ShareView; tab: string; title: string; subtitle: string }[] = [
    { view: "send", tab: "Send", title: "Send to another PC", subtitle: "With sendme: PC to PC, encrypted end to end" },
    { view: "receive", tab: "Receive", title: "Receive from another PC", subtitle: "With sendme: from the ticket it shows" },
    { view: "network", tab: "On this network", title: "Share on this network", subtitle: "With dufs: a link for a phone's browser" },
];

function ShareModal({ open }: { open: ShareView }) {
    const { title, subtitle } = VIEWS.find((v) => v.view === open)!;
    return (
        <Modal label={title} onClose={closeShare} className="max-h-[88vh] max-w-xl">
            <ModalHeader icon={open === "network" ? Wifi : open === "send" ? Send : Download} title={title} subtitle={subtitle} onClose={closeShare}>
                <div role="tablist" aria-label="Send, receive or share" className="flex shrink-0 rounded-md border p-0.5 text-xs">
                    {VIEWS.map(({ view, tab }) => (
                        <button
                            key={view}
                            type="button"
                            role="tab"
                            aria-selected={open === view}
                            onClick={() => (view === "receive" ? openReceive(useShare.getState().into || currentPath()) : showShare(view))}
                            className={cn("rounded px-2 py-0.5 whitespace-nowrap transition-colors", open === view ? "bg-primary/15 text-primary" : "text-muted-foreground hover:text-foreground")}
                        >
                            {tab}
                        </button>
                    ))}
                </div>
            </ModalHeader>
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                {open === "network" ? (
                    <>
                        <ToolNeeded id="dufs" icon={Wifi} why="dufs serves a folder to browsers on this network: a phone opens its link, or scans its QR code, to see and download what's in it." />
                        <NetworkView />
                    </>
                ) : (
                    <>
                        <ToolNeeded
                            id="sendme"
                            icon={Send}
                            why="sendme sends files and folders straight to another PC, encrypted end to end, with no account and no size limit. The other PC needs it too."
                        />
                        {open === "send" ? <SendView /> : <ReceiveView />}
                    </>
                )}
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

/** A card's head: what it is, where, and its Stop (or Remove, once it's ended). */
function Head({ path, detail, ended, stop, stopLabel }: { path: string; detail?: string; ended?: string; stop: () => void; stopLabel: string }) {
    return (
        <div className="flex items-center gap-2">
            <div className="min-w-0 flex-1">
                <p className="truncate font-medium">
                    {baseName(path) || path}
                    {detail && <span className="font-normal text-muted-foreground"> · {detail}</span>}
                </p>
                <p className="truncate text-xs text-muted-foreground" title={path}>
                    {path}
                </p>
            </div>
            <Button variant="outline" size="sm" className="h-7" onClick={stop}>
                {ended ? (
                    <>
                        <X /> Remove
                    </>
                ) : (
                    <>
                        <Square className="size-3! fill-current" /> {stopLabel}
                    </>
                )}
            </Button>
        </div>
    );
}

const Ended = ({ message }: { message: string }) => (
    <p className="flex items-start gap-2 text-xs text-destructive">
        <AlertTriangle className="mt-0.5 size-3.5 shrink-0" /> {message}
    </p>
);

const Qr = ({ svg, alt, className }: { svg: string; alt: string; className?: string }) => (
    <img src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`} alt={alt} className={cn("shrink-0 rounded-md bg-white p-1", className)} />
);

function SendCard({ send }: { send: Sending }) {
    return (
        <section className="space-y-2.5 rounded-lg border p-3">
            <Head path={send.path} detail={send.size} ended={send.ended} stop={() => stopSend(send.key)} stopLabel="Stop sending" />
            {send.ended ? (
                <Ended message={send.ended} />
            ) : !send.ticket ? (
                <p className="flex items-center gap-2 text-muted-foreground">
                    <Loader2 className="size-4 animate-spin text-primary" /> Reading it through for its ticket…
                </p>
            ) : (
                <div className="flex gap-3">
                    {send.qr && <Qr svg={send.qr} alt="The ticket as a QR code" className="size-32" />}
                    <div className="flex min-w-0 flex-1 flex-col gap-2">
                        <textarea
                            readOnly
                            value={send.ticket}
                            aria-label="Ticket"
                            onFocus={(e) => e.currentTarget.select()}
                            className="min-h-20 flex-1 resize-none rounded-md border bg-muted/40 p-2 font-mono text-[0.6875rem] leading-snug break-all outline-hidden focus:border-primary"
                        />
                        <Button size="sm" className="self-start" onClick={() => copy(send.ticket!, "ticket")}>
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

const LENGTHS = [
    { label: "15 minutes", minutes: 15 },
    { label: "An hour", minutes: 60 },
    { label: "4 hours", minutes: 240 },
    { label: "When I stop it", minutes: 0 },
];

/** The choices made last time, for the next (while Glint runs). */
let last = { uploads: false, minutes: 60 };

function NetworkView() {
    const { offer, serving } = useShare(useShallow((s) => ({ offer: s.offer, serving: s.serving })));
    const asked = !!offer && !serving.some((s) => samePath(s.path, offer) && !s.ended);
    return (
        <>
            {asked && <ServeForm path={offer} />}
            {serving.map((s) => (
                <ServingCard key={s.key} serving={s} />
            ))}
            {!asked && !serving.length && <p className="py-6 text-center text-muted-foreground">Right-click a folder or a file and choose Share on this network, or find it in Ctrl+K.</p>}
            <p className="text-xs text-muted-foreground">
                Anyone on this network who has the link can see and download all that's in it, and it isn't encrypted on its way, so share on networks you trust. The first time, Windows asks
                whether dufs may accept connections: allow it, or other devices can't open the link. A share says so when Windows keeps them out.
            </p>
        </>
    );
}

function ServeForm({ path }: { path: string }) {
    const tool = useTool("dufs");
    const [addresses, setAddresses] = useState<LanAddress[] | null>(null);
    const [address, setAddress] = useState("");
    const [choice, setChoice] = useState(last);
    useEffect(() => {
        lanAddresses().then(
            (list) => {
                setAddresses(list);
                setAddress(list[0]?.address ?? "");
            },
            () => setAddresses([])
        );
    }, []);
    const choose = (patch: Partial<typeof last>) => setChoice((c) => (last = { ...c, ...patch }));
    return (
        <section className="space-y-3 rounded-lg border p-3">
            <div className="min-w-0">
                <p className="truncate font-medium">{baseName(path) || path}</p>
                <p className="truncate text-xs text-muted-foreground" title={path}>
                    {path}
                </p>
            </div>
            {addresses?.length === 0 && <p className="text-xs text-destructive">This PC isn't on a network.</p>}
            {addresses && addresses.length > 1 && (
                <label className="flex items-center gap-3">
                    <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Network</span>
                    <select value={address} onChange={(e) => setAddress(e.target.value)} aria-label="Network" className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2 text-sm outline-hidden focus:border-primary">
                        {addresses.map((a) => (
                            <option key={a.address} value={a.address}>
                                {a.name} · {a.address}
                            </option>
                        ))}
                    </select>
                </label>
            )}
            <label className="flex items-center gap-3">
                <span className="w-20 shrink-0 text-xs font-semibold text-muted-foreground">Stop it</span>
                <select
                    value={choice.minutes}
                    onChange={(e) => choose({ minutes: Number(e.target.value) })}
                    aria-label="Stop it"
                    className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2 text-sm outline-hidden focus:border-primary"
                >
                    {LENGTHS.map((l) => (
                        <option key={l.minutes} value={l.minutes}>
                            {l.minutes ? `After ${l.label.toLowerCase()}` : l.label}
                        </option>
                    ))}
                </select>
            </label>
            <label className="flex items-start gap-2 pl-[5.75rem]">
                <input type="checkbox" checked={choice.uploads} onChange={(e) => choose({ uploads: e.target.checked })} className="mt-0.5 accent-primary" />
                <span>
                    Let them add files to it
                    <span className="block text-xs text-muted-foreground">Whoever has the link can then save files into it, but not replace or delete what's there.</span>
                </span>
            </label>
            <Button disabled={!tool?.origin || !addresses?.length} onClick={() => void serve(path, { uploads: choice.uploads, minutes: choice.minutes, address: address || null })}>
                <Wifi /> Share it
            </Button>
        </section>
    );
}

/** Now, a second at a time while `on`: for a countdown, and how long ago. */
function useNow(on: boolean) {
    const [now, setNow] = useState(Date.now);
    useEffect(() => {
        if (!on) return;
        setNow(Date.now());
        const tick = setInterval(() => setNow(Date.now()), 1000);
        return () => clearInterval(tick);
    }, [on]);
    return now;
}

const ago = (ms: number) => (ms < 60_000 ? `${Math.max(1, Math.round(ms / 1000))} s ago` : `${Math.round(ms / 60_000)} min ago`);

function ServingCard({ serving: s }: { serving: Serving }) {
    const now = useNow(!s.ended && (!!s.ends || !!s.last));
    const left = s.ends ? Math.max(0, s.ends - now) : null;
    return (
        <section className="space-y-2.5 rounded-lg border p-3">
            <Head path={s.path} detail={s.uploads ? "they can add files" : "read-only"} ended={s.ended} stop={() => stopServing(s.key)} stopLabel="Stop sharing" />
            {s.ended ? (
                <Ended message={s.ended} />
            ) : !s.url ? (
                <p className="flex items-center gap-2 text-muted-foreground">
                    <Loader2 className="size-4 animate-spin text-primary" /> Starting…
                </p>
            ) : (
                <>
                    {s.reach && s.address && <ReachNote reach={s.reach} address={s.address} />}
                    <div className="flex gap-3">
                        {s.qr && <Qr svg={s.qr} alt="The link as a QR code" className="size-36" />}
                        <div className="flex min-w-0 flex-1 flex-col gap-2">
                            <p className="font-mono text-xs break-all select-all" aria-label="Link">
                                {s.url}
                            </p>
                            <div className="flex flex-wrap gap-2">
                                <Button size="sm" onClick={() => copy(s.url!, "link")}>
                                    <Copy /> Copy the link
                                </Button>
                                <Button size="sm" variant="outline" onClick={() => void openUrl(s.url!)} title="Open it in your browser here, as others see it">
                                    <ExternalLink /> Open
                                </Button>
                            </div>
                            <p className="text-xs text-muted-foreground tabular-nums">
                                {left == null
                                    ? "Shared till you stop it."
                                    : `Stops at ${new Date(s.ends!).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}, in ${left < 60_000 ? "under a minute" : plural(Math.ceil(left / 60_000), "minute")}.`}
                            </p>
                            <p className="text-xs text-muted-foreground">
                                {s.last
                                    ? `${plural(s.visits, "request")} from ${plural(s.from.length, "device")}; the last ${ago(now - s.last.at)}: ${s.last.request}${s.last.status >= 400 ? ` (${s.last.status})` : ""}`
                                    : "Nobody's opened it yet."}
                            </p>
                        </div>
                    </div>
                </>
            )}
        </section>
    );
}

/** What keeps other devices from opening a share's link, as Windows has it, and where that's changed: nothing when nothing does. Glint only opens those pages. */
function ReachNote({ reach, address }: { reach: LanReach; address: string }) {
    if (reach.firewall !== "blocked" && reach.firewall !== "unset") return null;
    const kind = reach.network ?? "private";
    const isPublic = reach.network === "public";
    const where = isPublic ? ", and Windows has this network as public" : "";
    const said =
        reach.firewall === "blocked"
            ? `Other devices can't open the link: Windows Firewall blocks dufs on ${kind} networks${where}.`
            : `Other devices can't open the link yet: Windows Firewall hasn't been told dufs may take connections on ${kind} networks${where}.`;
    const fix =
        reach.firewall === "blocked"
            ? isPublic
                ? "If it's your home or work network, set it to private and share again. Or let dufs in on public networks, in the firewall's allowed apps."
                : `Let dufs in on ${kind} networks, in the firewall's allowed apps.`
            : isPublic
              ? "If Windows asks, allow it with Public networks ticked. Or, if it's your home or work network, set it to private and share again."
              : "If Windows asks, allow it. Or let dufs in, in the firewall's allowed apps.";
    const open = (page: "network" | "firewall") => void lanOpenSettings(page, address).catch((e) => toast.error(String(e)));
    return (
        <div className="space-y-2 rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-xs" role="status">
            <p className="flex items-start gap-2 font-medium">
                <ShieldAlert className="mt-0.5 size-3.5 shrink-0 text-amber-600 dark:text-amber-500" /> {said}
            </p>
            <p className="text-muted-foreground">{fix}</p>
            <div className="flex flex-wrap gap-2">
                {isPublic && (
                    <Button size="sm" variant="outline" onClick={() => open("network")} title="Settings for this network, where it's set as public or private">
                        <Network /> Network settings
                    </Button>
                )}
                <Button size="sm" variant="outline" onClick={() => open("firewall")} title="Windows Firewall's allowed apps (changing them asks for administrator approval)">
                    <Shield /> Allowed apps
                </Button>
            </div>
        </div>
    );
}
