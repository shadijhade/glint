import { useEffect, useState } from "react";
import { Download, Play } from "lucide-react";
import type { MenuItem } from "@/components/Menu";
import { projectTasks, type TaskGroup } from "./fs";
import { baseName } from "./paths";
import { runInTerminal } from "./terminal";
import { installTool, useTools } from "./tools";

/** What each folder could run, as last read (a folder opened again shows its Run menu at once). */
const known = new Map<string, TaskGroup[]>();

/** What `folder` and the folders above it (to its repository's root) can run; read again each time it's opened. */
export function useProjectTasks(folder: string | null) {
    const [groups, setGroups] = useState<TaskGroup[]>(() => (folder ? (known.get(folder.toLowerCase()) ?? []) : []));
    // A tool installed since (just, uv) makes its tasks runnable.
    const tools = useTools((s) => s.tools);
    useEffect(() => {
        if (!folder) return setGroups([]);
        setGroups(known.get(folder.toLowerCase()) ?? []);
        let stale = false;
        projectTasks(folder).then(
            (got) => {
                known.set(folder.toLowerCase(), got);
                if (!stale) setGroups(got);
            },
            () => {}
        );
        return () => {
            stale = true;
        };
    }, [folder, tools]);
    return groups;
}

/** The tasks of `folder` known so far (Ctrl+K's, which doesn't wait). */
export const knownTasks = (folder: string) => known.get(folder.toLowerCase()) ?? [];

/** Runs a task in the terminal pane, in its project's folder, typed as it'd be typed. */
export const runTask = (group: TaskGroup, command: string) => void runInTerminal(group.folder, () => `${command}\r`, command);

/** The Run menu: each project's tasks under its name, nearest first; an install for what a project needs that isn't there. */
export function taskMenu(groups: TaskGroup[]): MenuItem[] {
    return groups.flatMap((group): MenuItem[] => [
        { heading: `${baseName(group.folder) || group.folder} · ${group.source}` },
        ...(group.missing
            ? [{ label: `Install ${group.missing} to run these`, icon: Download, onSelect: () => void installTool(group.missing!) }]
            : group.tasks.map((task) => ({ label: task.detail ? `${task.name}: ${task.detail}` : task.name, icon: Play, onSelect: () => runTask(group, task.command) }))),
    ]);
}
