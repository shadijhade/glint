import { create } from "zustand";
import { toast } from "sonner";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { operationsDone, operationsRunning } from "./explorer";
import { stopTools } from "./fs";
import { firstNotice } from "./utils";

const RELEASES = "https://github.com/shadijhade/glint/releases";
const LATEST_RELEASE_API = "https://api.github.com/repos/shadijhade/glint/releases/latest";

export interface AvailableUpdate {
    version: string;
    /** The release page: what's new, and the installer for copies that don't update themselves. */
    url: string;
    /** Set when this copy can install it: copies the setup program installed (see installed_by_setup in lib.rs). */
    update?: Update;
}

interface UpdaterState {
    available: AvailableUpdate | null;
    /** The last check: running, found nothing newer ("latest"), or failed. */
    status: "idle" | "checking" | "latest" | "failed";
    error: string | null;
    /** Percent of the update downloaded while it installs, null otherwise. */
    progress: number | null;
}

export const useUpdater = create<UpdaterState>(() => ({ available: null, status: "idle", error: null, progress: null }));
const get = useUpdater.getState;
const set = useUpdater.setState;

/**
 * The first start after an update says so once, with the release's notes a click away. A fresh
 * install has no earlier settings and says nothing; versions before 0.6 kept no version, so an
 * update from one of them counts by its settings.
 */
export async function greetUpdate() {
    const version = await getVersion().catch(() => "");
    let before: string | null = null;
    try {
        before = localStorage.getItem("last-version") ?? ("returning" in document.documentElement.dataset ? "earlier" : null);
        localStorage.setItem("last-version", version);
    } catch {
        return;
    }
    if (!version || !before || before === version) return;
    toast.success(`Updated to Glint ${version}`, {
        description: "See what's new in this version.",
        action: { label: "What's new", onClick: () => void openUrl(`${RELEASES}/tag/v${version}`) },
        duration: 10_000,
    });
}

/** A newer version, from the latest release's update manifest (latest.json), or else from GitHub's latest-release API. */
async function findUpdate(): Promise<AvailableUpdate | null> {
    try {
        const update = await check();
        return update && { version: update.version, url: `${RELEASES}/tag/v${update.version}`, update };
    } catch {
        // No updater in this copy (Scoop, portable, a dev build), or the latest release has no manifest.
    }
    const [current, response] = await Promise.all([getVersion(), fetch(LATEST_RELEASE_API)]);
    if (!response.ok) throw new Error(`GitHub answered ${response.status} ${response.statusText}`);
    const latest = await response.json();
    const version = String(latest.tag_name ?? "").replace(/^v/, "");
    const newer = /^\d+(\.\d+)*$/.test(version) && version.localeCompare(current, undefined, { numeric: true }) > 0;
    return newer ? { version, url: String(latest.html_url) } : null;
}

/**
 * Looks for a newer version. On its own (at start and twice a day) it says so once per version; asked from
 * Settings, the outcome shows there.
 */
export async function checkForUpdates(manual = false) {
    const { available, status } = get();
    if (available || status === "checking") return;
    set({ status: "checking", error: null });
    try {
        const found = await findUpdate();
        set({ available: found, status: found ? "idle" : "latest" });
        if (found && !manual && firstNotice("update-notice", found.version)) {
            toast.info("Update available", {
                description: found.update ? `Version ${found.version} is ready. Installing it restarts the app.` : `Version ${found.version} is out.`,
                action: { label: found.update ? "Install" : "Download", onClick: () => void installUpdate() },
                duration: 15_000,
            });
        }
    } catch (e) {
        set({ status: "failed", error: String(e) });
    }
}

/**
 * Installs the available update: the app closes, and the installer puts in the new version and opens it
 * again. Copies that don't update themselves open the release page instead.
 */
export async function installUpdate() {
    const { available, progress } = get();
    if (!available || progress !== null) return;
    const { version, url, update } = available;
    if (!update) return void openUrl(url);

    set({ progress: 0 });
    const id = toast.loading(`Downloading version ${version}…`);
    let total = 0;
    let received = 0;
    try {
        await update.download((event) => {
            if (event.event === "Started") total = event.data.contentLength ?? 0;
            if (event.event !== "Progress" || !total) return;
            received += event.data.chunkLength;
            const percent = Math.min(100, Math.floor((100 * received) / total));
            if (percent === get().progress) return;
            set({ progress: percent });
            toast.loading(`Downloading version ${version}… ${percent}%`, { id });
        });
        // Closing the app would cut a copy or move short, so those finish first.
        if (operationsRunning()) toast.loading("Installing once the file operation finishes…", { id });
        await operationsDone();
        toast.loading(`Installing version ${version}…`, { id });
        // The installer replaces rg, fd and dust too, which it can't while one of them is running.
        await stopTools().catch(() => {});
        await update.install(); // on Windows the app exits here, and the installer reopens it
    } catch (e) {
        set({ progress: null });
        toast.error("Couldn't install the update", {
            id,
            description: String(e),
            action: { label: "Download", onClick: () => void openUrl(url) },
        });
    }
}
