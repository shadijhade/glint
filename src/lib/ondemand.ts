import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { create } from "zustand";

/** A remote shown in File Explorer on demand (ondemand.rs): its connection, and its folder there. */
export interface OnDemand {
    id: string;
    folder: string;
}

export const useOnDemand = create<{ mounts: OnDemand[] }>(() => ({ mounts: [] }));

export const refreshOnDemand = () => invoke<OnDemand[]>("ondemand_list").then((mounts) => useOnDemand.setState({ mounts }), () => {});

/** Shows a remote in File Explorer, every app's Open dialog too: names come as folders open, files as they're read. */
export async function showOnDemand(id: string, name: string) {
    try {
        const folder = await invoke<string>("ondemand_mount", { id, name });
        await refreshOnDemand();
        toast.success(`“${name}” is in File Explorer`, {
            description: "Its files download when an app opens them, while Glint runs. It's read-only there.",
            action: { label: "Open", onClick: () => void openPath(folder) },
        });
    } catch (e) {
        toast.error("Couldn't show it in File Explorer", { description: String(e) });
    }
}

export async function hideOnDemand(id: string) {
    try {
        await invoke("ondemand_unmount", { id });
        await refreshOnDemand();
    } catch (e) {
        toast.error("Couldn't take it out of File Explorer", { description: String(e) });
    }
}
