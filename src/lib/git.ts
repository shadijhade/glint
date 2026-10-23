import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { refreshedByHand, useExplorer, visibleTabs } from "./explorer";
import { cancelGit, type GitView, gitView, gitWatch } from "./fs";
import { isFolderPath, samePath } from "./paths";
import { useStore } from "./store";

/** A tab's Git: its folder's view (null: not in a repository), while it's being worked out too. */
export interface GitRun {
    folder: string;
    view: GitView | null;
    running: boolean;
}

/** Each tab's Git, by tab id; a tab not in a repository (or showing a search, an archive, This PC) has none. */
export const useGit = create<Record<string, GitRun>>(() => ({}));
const runs = useGit.getState;

function save(tab: string, run: GitRun | null) {
    useGit.setState((s) => {
        const next = { ...s };
        if (run) next[tab] = run;
        else delete next[tab];
        return next;
    });
}

/** Asks for Git in a tab's folder; a view of the same folder stays on screen meanwhile. */
export async function refreshGit(tab: string, folder: string, force = false) {
    const before = runs()[tab];
    const same = before && samePath(before.folder, folder);
    save(tab, { folder, view: same ? before.view : null, running: true });
    try {
        const view = await gitView(folder, tab, useStore.getState().settings.gitTrusted, force);
        if (samePath(runs()[tab]?.folder ?? "", folder)) save(tab, view ? { folder, view, running: false } : null);
    } catch {
        // Stopped for a newer request, or the folder went: what was shown stays until that one answers.
        if (samePath(runs()[tab]?.folder ?? "", folder) && runs()[tab]?.running) save(tab, same && before.view ? { ...before, running: false } : null);
    }
    watchShown();
}

/** Takes Git off a tab (it closed, or shows something that isn't a folder on disk). */
function forget(tab: string) {
    if (!runs()[tab]) return;
    void cancelGit(tab);
    save(tab, null);
}

/** What Git says of the item at `path`, from its folder's view in any tab (undefined: nothing, or not known). */
export function useGitMark(path: string) {
    return useGit((s) => {
        const folder = path.replace(/[\\/][^\\/]*$/, "");
        const run = Object.values(s).find((r) => r.view && samePath(r.folder, folder));
        return run?.view?.marks[path.slice(folder.length + 1).toLowerCase()];
    });
}

/** Trusts a repository whose own settings name programs for status to run, and shows its status. */
export function trustRepository(root: string) {
    const { settings, setSettings } = useStore.getState();
    if (!settings.gitTrusted.some((r) => samePath(r, root))) setSettings({ gitTrusted: [...settings.gitTrusted, root] });
    for (const [tab, run] of Object.entries(runs())) if (run.view && samePath(run.view.root, root)) void refreshGit(tab, run.folder);
}

let watched = "";

/** Watches the repositories of the tabs on screen, and only those. */
function watchShown() {
    const s = useExplorer.getState();
    const roots = [...new Set(visibleTabs(s).flatMap((tab) => (runs()[tab]?.view ? [runs()[tab]!.view!.root] : [])))].sort();
    const key = roots.join("\n").toLowerCase();
    if (key === watched) return;
    watched = key;
    gitWatch(roots).catch(() => {});
}

// Git follows each tab's listing: a folder opened (or read again) is looked at; F5 runs status even
// where it waits for it. Search results, archives, This PC, the Recycle Bin and closed tabs have none.
useExplorer.subscribe((s, prev) => {
    for (const [tab, t] of Object.entries(s.tabs)) {
        const before = prev.tabs[tab];
        if (t.listing === before?.listing && t.find === before?.find) continue;
        if (!t.listing || t.find || t.listing.archive || t.listing.fromIndex || !isFolderPath(t.path)) {
            forget(tab);
            continue;
        }
        const force = refreshedByHand.delete(tab);
        void refreshGit(tab, t.path, force);
    }
    for (const tab of Object.keys(runs())) if (!s.tabs[tab]) forget(tab);
    if (s.panes !== prev.panes || s.activePane !== prev.activePane || s.dual !== prev.dual) watchShown();
});

// A repository on screen changed (a save, git add, a checkout): its tabs look again.
void listen<string>("git-changed", (event) => {
    for (const [tab, run] of Object.entries(runs())) if (run.view && samePath(run.view.root, event.payload)) void refreshGit(tab, run.folder);
});
