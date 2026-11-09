import { useEffect, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { FolderInput, Laptop, Loader2, Monitor, Radar, RefreshCw, Send, Smartphone, Square } from "lucide-react";
import { revealInApp } from "@/lib/explorer";
import { formatSize, plural } from "@/lib/format";
import { baseName } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";

interface Info {
    alias: string;
    deviceModel?: string | null;
    deviceType?: string | null;
    fingerprint: string;
    port: number;
    protocol: string;
}

interface Peer {
    info: Info;
    ip: string;
}

interface SendProgress {
    files: number;
    totalFiles: number;
    bytes: number;
    totalBytes: number;
    current: string;
}

const useNearby = create<{ open: boolean; paths: string[] }>(() => ({ open: false, paths: [] }));

/** Opens Nearby: the LocalSend apps on this network, to send `paths` to (or to receive from). */
export const openNearby = (paths: string[] = []) => useNearby.setState({ open: true, paths });
const close = () => useNearby.setState({ open: false });

export function NearbyDialog() {
    const open = useNearby((s) => s.open);
    const paths = useNearby((s) => s.paths);
    if (!open) return null;
    return <NearbyModal paths={paths} />;
}

const iconOf = (type?: string | null) => (type === "mobile" ? Smartphone : type === "laptop" ? Laptop : Monitor);

function NearbyModal({ paths }: { paths: string[] }) {
    const [peers, setPeers] = useState<Peer[] | null>(null);
    const [scanning, setScanning] = useState(false);
    const [sending, setSending] = useState<{ to: string; progress: SendProgress | null } | null>(null);
    const [receiving, setReceiving] = useState<number | null>(null);
    const [received, setReceived] = useState<{ from: string; path: string }[]>([]);

    const scan = async () => {
        setScanning(true);
        try {
            setPeers(await invoke<Peer[]>("nearby_scan", { millis: 3500 }));
        } catch (e) {
            toast.error("Couldn't look for devices", { description: String(e) });
            setPeers([]);
        } finally {
            setScanning(false);
        }
    };

    useEffect(() => {
        void scan();
        const stop = listen<{ from: string; path: string }>("nearby-received", (e) => setReceived((r) => [e.payload, ...r].slice(0, 50)));
        return () => {
            void stop.then((u) => u());
            void invoke("nearby_receive", { on: false });
        };
    }, []);

    const receive = async (on: boolean) => {
        try {
            setReceiving((await invoke<number | null>("nearby_receive", { on })) ?? null);
        } catch (e) {
            toast.error("Couldn't start receiving", { description: String(e) });
        }
    };

    const send = async (peer: Peer) => {
        const channel = new Channel<SendProgress>();
        channel.onmessage = (p) => setSending({ to: peer.info.fingerprint, progress: p });
        setSending({ to: peer.info.fingerprint, progress: null });
        try {
            const sent = await invoke<number>("nearby_send", { peer, paths, onProgress: channel });
            toast.success(sent ? `Sent ${plural(sent, "file")} to ${peer.info.alias}` : `${peer.info.alias} already had them`);
        } catch (e) {
            if (String(e) !== "cancelled") toast.error(`Couldn't send to ${peer.info.alias}`, { description: String(e) });
        } finally {
            setSending(null);
        }
    };

    return (
        <Modal label="Nearby" onClose={close} className="max-h-[88vh] max-w-lg">
            <ModalHeader icon={Radar} title="Phones and computers nearby" subtitle={paths.length ? `To send ${paths.length === 1 ? `“${baseName(paths[0])}”` : plural(paths.length, "item")}` : "Through LocalSend, on this network"} onClose={close}>
                <Button variant="outline" size="sm" className="h-7" disabled={scanning} onClick={() => void scan()}>
                    {scanning ? <Loader2 className="animate-spin" /> : <RefreshCw />} Look again
                </Button>
            </ModalHeader>
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                <p className="text-xs text-muted-foreground">
                    Devices with the LocalSend app open (Android, iPhone, Mac, Linux, Windows), on the same Wi-Fi or network. Nothing goes through the internet; the other device asks its person before it takes anything.
                </p>
                <div className="space-y-1">
                    {peers === null || (scanning && !peers.length) ? (
                        <p className="flex items-center gap-2 py-4 text-muted-foreground">
                            <Loader2 className="size-4 animate-spin" /> Looking on this network…
                        </p>
                    ) : peers.length === 0 ? (
                        <p className="py-4 text-muted-foreground">No device answered. Open LocalSend on the phone (same Wi-Fi), then look again.</p>
                    ) : (
                        peers.map((peer) => {
                            const Icon = iconOf(peer.info.deviceType);
                            const busy = sending?.to === peer.info.fingerprint;
                            const progress = busy ? sending?.progress : null;
                            return (
                                <div key={peer.info.fingerprint} className="flex items-center gap-3 rounded-md border px-3 py-2">
                                    <Icon className="size-5 shrink-0 text-primary" />
                                    <div className="min-w-0 flex-1">
                                        <div className="truncate font-medium">{peer.info.alias}</div>
                                        <div className="truncate text-xs text-muted-foreground">
                                            {busy && progress ? `${progress.files} of ${progress.totalFiles} · ${formatSize(progress.bytes)} of ${formatSize(progress.totalBytes)}` : busy ? "Waiting for them to say yes…" : `${peer.info.deviceModel ?? peer.info.deviceType ?? "Device"} · ${peer.ip}`}
                                        </div>
                                        {busy && progress && progress.totalBytes > 0 && (
                                            <div className="mt-1 h-1 overflow-hidden rounded-full bg-muted">
                                                <div className="h-full bg-primary transition-[width]" style={{ width: `${Math.round((100 * progress.bytes) / progress.totalBytes)}%` }} />
                                            </div>
                                        )}
                                    </div>
                                    {paths.length > 0 &&
                                        (busy ? (
                                            <Button variant="outline" size="sm" onClick={() => void invoke("nearby_cancel")}>
                                                <Square className="size-3! fill-current" /> Stop
                                            </Button>
                                        ) : (
                                            <Button size="sm" disabled={!!sending} onClick={() => void send(peer)}>
                                                <Send /> Send
                                            </Button>
                                        ))}
                                </div>
                            );
                        })
                    )}
                </div>

                <section className="space-y-2 rounded-md border p-3">
                    <label className="flex items-center gap-2">
                        <input type="checkbox" checked={receiving !== null} onChange={(e) => void receive(e.target.checked)} className="accent-primary" />
                        <span className="font-medium">Let them send to this PC while this is open</span>
                    </label>
                    <p className={cn("text-xs text-muted-foreground", receiving === null && "opacity-70")}>
                        {receiving !== null
                            ? `This PC shows up in LocalSend as “Glint on this PC” (port ${receiving}). What's sent goes into Downloads\\LocalSend. If Windows asks, allow Glint on private networks.`
                            : "This PC shows up in their LocalSend lists, and what they send goes into Downloads\\LocalSend."}
                    </p>
                    {received.length > 0 && (
                        <ul className="space-y-0.5 text-xs">
                            {received.map((r) => (
                                <li key={r.path} className="flex items-center gap-2">
                                    <span className="min-w-0 flex-1 truncate" title={r.path}>
                                        {baseName(r.path)} <span className="text-muted-foreground">from {r.from}</span>
                                    </span>
                                    <Button
                                        variant="ghost"
                                        size="sm"
                                        className="h-6 px-2 text-xs"
                                        onClick={() => {
                                            close();
                                            void revealInApp(r.path);
                                        }}
                                    >
                                        <FolderInput className="size-3.5" /> Show
                                    </Button>
                                </li>
                            ))}
                        </ul>
                    )}
                </section>
            </div>
        </Modal>
    );
}
