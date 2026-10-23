import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { formatDuration } from "./format";
import { baseName } from "./paths";

export interface DefenderThreat {
    /** As Defender names it, e.g. "Virus:DOS/EICAR_Test_File". */
    name: string;
    /** Where it found it. */
    resources: string[];
}

export interface DefenderReport {
    clean: boolean;
    threats: DefenderThreat[];
    elapsedMs: number;
}

/** A custom scan of a file or folder by Microsoft Defender (defender.rs); rejects with "cancelled" when stopped. */
const defenderScan = (path: string) => invoke<DefenderReport>("defender_scan", { path });
const cancelDefenderScan = () => invoke("cancel_defender_scan");

let scanning = false;

/**
 * Scans `paths` with Microsoft Defender, one after another, telling how it went in a toast with Stop.
 * What it finds, Defender deals with as its settings say.
 */
export async function scanWithDefender(paths: string[]) {
    if (!paths.length) return;
    if (scanning) {
        toast.info("Microsoft Defender is already scanning");
        return;
    }
    scanning = true;
    const what = paths.length === 1 ? `“${baseName(paths[0]) || paths[0]}”` : `${paths.length} items`;
    let stopped = false;
    const stop = () => {
        stopped = true;
        void cancelDefenderScan();
    };
    const id = toast.loading(`Microsoft Defender is scanning ${what}…`, { duration: Infinity, action: { label: "Stop", onClick: stop } });
    const started = performance.now();
    const threats: DefenderThreat[] = [];
    try {
        for (const path of paths) {
            if (stopped) break;
            threats.push(...(await defenderScan(path)).threats);
        }
        const took = formatDuration(performance.now() - started);
        if (stopped) toast.info(`Stopped scanning ${what}`, { id, duration: 3000, action: undefined });
        else if (!threats.length) toast.success(`No threats found in ${what}`, { id, description: `Microsoft Defender · ${took}`, duration: 6000, action: undefined });
        else
            toast.error(`Microsoft Defender found ${threats.length === 1 ? "a threat" : `${threats.length} threats`} in ${what}`, {
                id,
                description: `${threats.map((t) => `${t.name}${t.resources.length ? ` (${t.resources.map((r) => baseName(r) || r).join(", ")})` : ""}`).join("; ")}. It dealt with ${threats.length === 1 ? "it" : "them"} as its settings say: Windows Security's Protection history lists ${threats.length === 1 ? "it" : "them"}.`,
                duration: Infinity,
                action: undefined,
            });
    } catch (e) {
        if (stopped || String(e) === "cancelled") toast.info(`Stopped scanning ${what}`, { id, duration: 3000, action: undefined });
        else toast.error("Couldn't scan with Microsoft Defender", { id, description: String(e), duration: 10000, action: undefined });
    } finally {
        scanning = false;
    }
}
