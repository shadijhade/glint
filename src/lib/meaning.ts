import { Channel, invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { create } from "zustand";
import type { ToolProgress } from "./fs";

/**
 * Meaning search's model (meaning.rs): Model2Vec's potion-base-8M, 30 MB, downloaded from Hugging Face the
 * first time it's wanted (pinned by revision and SHA-256). `installed` is null until asked.
 */
export const useMeaning = create<{ installed: boolean | null; progress: ToolProgress | null }>(() => ({ installed: null, progress: null }));
const set = useMeaning.setState;

export async function refreshMeaning() {
    set({ installed: await invoke<boolean>("meaning_status").catch(() => false) });
}

/** Downloads the model, with its progress on the card and in Settings. */
export async function installMeaning() {
    if (useMeaning.getState().progress) return;
    set({ progress: { stage: "Starting", done: 0, total: 0 } });
    const channel = new Channel<ToolProgress>();
    channel.onmessage = (progress) => set({ progress });
    try {
        await invoke("meaning_install", { onProgress: channel });
        set({ installed: true });
        toast.success("The meaning model is downloaded", { description: "Meaning search ranks passages by what they're about." });
    } catch (e) {
        if (e !== "cancelled") toast.error("Couldn't download the meaning model", { description: String(e) });
    } finally {
        set({ progress: null });
    }
}

export const cancelMeaning = () => void invoke("tool_cancel_install", { id: "meaning" });

export async function removeMeaning() {
    try {
        await invoke("meaning_remove");
        set({ installed: false });
        toast.success("The meaning model is removed");
    } catch (e) {
        toast.error("Couldn't remove the meaning model", { description: String(e) });
    }
}
