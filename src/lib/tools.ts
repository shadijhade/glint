import { useEffect } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { formatSize } from "./format";
import { toolCancelInstall, toolInstall, type ToolProgress, toolRemove, type ToolStatus, toolsStatus } from "./fs";

/**
 * The programs Glint drives: rg, fd and pdu come with it, the optional ones (difftastic, ast-grep…)
 * install on demand, with winget or from their GitHub release (tools.rs).
 */
interface ToolsState {
    /** By id; empty until the first `refreshTools`. */
    tools: Record<string, ToolStatus>;
    /** Installs and removals running now, with how far along they are. */
    busy: Record<string, ToolProgress>;
}

export const useTools = create<ToolsState>(() => ({ tools: {}, busy: {} }));
const set = useTools.setState;
const get = useTools.getState;

let asked: Promise<void> | null = null;

/** Asks where each tool is and its version (a few hundred ms: each program is run with --version). */
export function refreshTools(): Promise<void> {
    asked ??= toolsStatus()
        .then((list) => set({ tools: Object.fromEntries(list.map((tool) => [tool.id, tool])) }))
        .catch(() => {})
        .finally(() => (asked = null));
    return asked;
}

/** A tool's status once known, asking for every tool's the first time one is wanted. */
export function useTool(id: string): ToolStatus | undefined {
    useEffect(() => {
        if (!Object.keys(get().tools).length) void refreshTools();
    }, []);
    return useTools((s) => s.tools[id]);
}

const busy = (id: string, progress: ToolProgress | null) =>
    set((s) => {
        const next = { ...s.busy };
        if (progress) next[id] = progress;
        else delete next[id];
        return { busy: next };
    });

/** Installs a tool, with its progress in Settings and on the card that asked; resolves to whether it's ready. */
export async function installTool(id: string): Promise<boolean> {
    if (get().busy[id]) return false;
    const name = get().tools[id]?.name ?? id;
    busy(id, { stage: "Starting", done: 0, total: 0 });
    try {
        const tool = await toolInstall(id, (progress) => busy(id, progress));
        set((s) => ({ tools: { ...s.tools, [id]: tool } }));
        toast.success(`${name} is installed`, { description: tool.version ? `Version ${tool.version}` : undefined });
        return true;
    } catch (e) {
        if (e !== "cancelled") toast.error(`Couldn't install ${name}`, { description: String(e) });
        return false;
    } finally {
        busy(id, null);
    }
}

export const cancelInstall = (id: string) => void toolCancelInstall(id);

export async function removeTool(id: string) {
    const name = get().tools[id]?.name ?? id;
    busy(id, { stage: "Removing", done: 0, total: 0 });
    try {
        const tool = await toolRemove(id);
        set((s) => ({ tools: { ...s.tools, [id]: tool } }));
        toast.success(`${name} is removed`);
    } catch (e) {
        toast.error(`Couldn't remove ${name}`, { description: String(e) });
    } finally {
        busy(id, null);
    }
}

/** "Downloading 2.1 MB of 7.4 MB…", or what winget is doing. */
export function describeProgress(progress: ToolProgress) {
    if (!progress.total) return progress.stage.endsWith("…") ? progress.stage : `${progress.stage.replace(/\.+$/, "")}…`;
    return `${progress.stage} ${formatSize(progress.done)} of ${formatSize(progress.total)}…`;
}
