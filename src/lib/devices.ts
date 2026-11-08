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
