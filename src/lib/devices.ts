import { invoke } from "@tauri-apps/api/core";
import { remoteId } from "./remotes";
import { type RemoteConnection, useStore } from "./store";

/** A folder this PC shares with its paired PCs (devices.rs): the name they see, and where it is here. */
export interface Share {
    name: string;
    path: string;
}

/** A PC let in to see the shared folders. */
export interface DevicePeer {
    id: string;
    name: string;
    paired: number;
}

export interface DevicesStatus {
    /** This PC's name, as the others see it. */
    name: string;
    sharing: boolean;
    shares: Share[];
    allowed: DevicePeer[];
}

export const devicesStatus = () => invoke<DevicesStatus>("devices_status");
export const devicesConfigure = (sharing: boolean, shares: Share[]) => invoke<DevicesStatus>("devices_configure", { sharing, shares });
export const devicesForget = (id: string) => invoke<DevicesStatus>("devices_forget", { id });
/** This PC's pairing code: its key and a one-time secret, good for ten minutes. */
export const devicesPairCode = () => invoke<string>("devices_pair_code");

/** Enters another PC's pairing code: that PC then shows in the sidebar's Remote section, to browse. */
export async function pairDevice(code: string) {
    const paired = await invoke<{ id: string; name: string }>("devices_pair", { code });
    const { remotes, saveRemote } = useStore.getState();
    const known = remotes.find((r) => r.kind === "device" && r.host === paired.id);
    const remote: RemoteConnection = known
        ? { ...known, name: paired.name }
        : { id: remoteId(paired.name), name: paired.name, kind: "device", host: paired.id, endpoint: "", region: "", bucket: "", user: "", root: "", thumbnails: true };
    saveRemote(remote);
    return remote;
}

/** How a send to a paired PC is going (devices.rs). */
export interface SendProgress {
    files: number;
    totalFiles: number;
    bytes: number;
    totalBytes: number;
    current: string;
}

/** Sends files and folders to a paired PC: they land in its Downloads, in a folder named after this PC. */
export async function sendToDevice(device: RemoteConnection, paths: string[]) {
    const { Channel } = await import("@tauri-apps/api/core");
    const { toast } = await import("sonner");
    const { formatSize } = await import("./format");
    const id = toast.loading(`Sending to ${device.name}…`);
    const channel = new Channel<SendProgress>();
    channel.onmessage = (p) =>
        toast.loading(`Sending to ${device.name}: ${p.files} of ${p.totalFiles}`, { id, description: `${formatSize(p.bytes)} of ${formatSize(p.totalBytes)}${p.current ? ` · ${p.current}` : ""}` });
    try {
        const landed = await invoke<string[]>("devices_send", { id: device.host, paths, onProgress: channel });
        toast.success(`Sent ${landed.length === 1 ? "it" : `${landed.length} files`} to ${device.name}`, { id, description: `In its Downloads folder` });
    } catch (e) {
        toast.error(`Couldn't send to ${device.name}`, { id, description: String(e) });
    }
}

/** Files arriving from paired PCs: one notice a burst, with Show for the last. */
export function listenForDeviceFiles(reveal: (path: string) => void) {
    let timer = 0;
    let batch: { from: string; path: string }[] = [];
    const flush = async () => {
        const { toast } = await import("sonner");
        const last = batch[batch.length - 1];
        const from = [...new Set(batch.map((b) => b.from))].join(", ");
        toast.success(batch.length === 1 ? `${from} sent you “${last.path.split("\\").pop()}”` : `${from} sent you ${batch.length} files`, {
            description: "In Downloads",
            duration: 10000,
            action: { label: "Show", onClick: () => reveal(last.path) },
        });
        batch = [];
    };
    return import("@tauri-apps/api/event").then(({ listen }) =>
        listen<{ from: string; path: string }>("device-received", (e) => {
            batch.push(e.payload);
            clearTimeout(timer);
            timer = window.setTimeout(() => void flush(), 800);
        })
    );
}
