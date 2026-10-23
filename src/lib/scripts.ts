import { create } from "zustand";
import { Download, FolderCog, ScrollText } from "lucide-react";
import { toast } from "sonner";
import type { MenuItem } from "@/components/Menu";
import { runLine } from "./cd";
import { navigate } from "./explorer";
import { listScripts, type Script, scriptsFolder } from "./fs";
import { formatCount } from "./format";
import { runInTerminal } from "./terminal";
import { installTool, useTools } from "./tools";

/** The scripts in Glint's Scripts folder: read at start, when the window is back in front, and when that folder changes on screen. */
export const useScripts = create<{ folder: string | null; list: Script[] }>(() => ({ folder: null, list: [] }));

export function refreshScripts() {
    listScripts().then(
        ({ folder, scripts }) => {
            const now = useScripts.getState();
            if (now.folder !== folder || JSON.stringify(now.list) !== JSON.stringify(scripts)) useScripts.setState({ folder, list: scripts });
        },
        () => {}
    );
}

/** About what cmd's prompt takes on one line; what's chosen goes on it. */
const LONGEST = 8000;

/** Runs `script` in a new terminal tab in `cwd`, given `paths` (what's chosen, or the folder) as its arguments. */
export function runScript(script: Script, cwd: string, paths: string[]) {
    if (script.missing) return void installTool(script.missing);
    const length = paths.reduce((n, p) => n + p.length + 3, 0);
    if (length > LONGEST)
        return void toast.error(`Too much for “${script.name}”'s command line`, {
            description: `The ${formatCount(paths.length)} paths take ${formatCount(length)} characters, and a command line holds about ${formatCount(LONGEST)}. Choose fewer, or run it on their folder.`,
        });
    void runInTerminal(cwd, (shell) => runLine(shell, script.program, [...script.args, ...paths]), script.name);
}

/** The Scripts menu: each script, run on `paths` in `cwd`, then the folder they're in. */
export function scriptItems(cwd: string, paths: string[]): MenuItem[] {
    const tools = useTools.getState().tools;
    const { list } = useScripts.getState();
    return [
        ...list.map(
            (script): MenuItem =>
                script.missing
                    ? { label: `${script.name}: install ${tools[script.missing]?.name ?? script.missing} to run it`, icon: Download, onSelect: () => void installTool(script.missing!) }
                    : { label: script.name, icon: ScrollText, onSelect: () => runScript(script, cwd, paths) }
        ),
        list.length > 0 && "separator",
        { label: "Open the Scripts folder", icon: FolderCog, onSelect: () => void openScriptsFolder() },
    ];
}

/** Shows the Scripts folder, made with a script to start from the first time. */
export async function openScriptsFolder() {
    try {
        const folder = await scriptsFolder();
        refreshScripts();
        await navigate(folder);
    } catch (e) {
        toast.error("Couldn't open the Scripts folder", { description: String(e) });
    }
}
