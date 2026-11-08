import { Channel, invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { create } from "zustand";
import type { ToolProgress } from "./fs";

/**
 * Meaning search's model (meaning.rs): Model2Vec's potion-base-8M, 30 MB, downloaded from Hugging Face the
 * first time it's wanted (pinned by revision and SHA-256). `installed` is null until asked.
 */
export const useMeaning = create<{ installed: boolean | null; multilingual: boolean; progress: ToolProgress | null }>(() => ({ installed: null, multilingual: false, progress: null }));
const set = useMeaning.setState;

export async function refreshMeaning() {
    const models = await invoke<{ english: boolean; multilingual: boolean }>("meaning_models").catch(() => ({ english: false, multilingual: false }));
    set({ installed: models.english || models.multilingual, multilingual: models.multilingual });
}

/** The 101-language model (potion-multilingual-128M, 512 MB): meaning search in Arabic, Chinese, Spanish and 98 more. */
export async function installMultilingual() {
    if (useMeaning.getState().progress) return;
    set({ progress: { stage: "Starting", done: 0, total: 0 } });
    const channel = new Channel<ToolProgress>();
    channel.onmessage = (progress) => set({ progress });
    try {
        await invoke("meaning_install", { onProgress: channel, variant: "multilingual" });
        set({ installed: true, multilingual: true });
        toast.success("The 101-language meaning model is downloaded", { description: "Meaning search reads Arabic, Chinese, Spanish and 98 more languages now." });
    } catch (e) {
        if (e !== "cancelled") toast.error("Couldn't download the 101-language model", { description: String(e) });
    } finally {
        set({ progress: null });
    }
}

export async function removeMultilingual() {
    try {
        await invoke("meaning_remove", { variant: "multilingual" });
        await refreshMeaning();
        toast.success("The 101-language model is removed");
    } catch (e) {
        toast.error("Couldn't remove the 101-language model", { description: String(e) });
    }
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
