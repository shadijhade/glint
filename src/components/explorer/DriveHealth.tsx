import { useEffect } from "react";
import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, HeartPulse, ShieldAlert, Thermometer } from "lucide-react";
import { formatCount, formatSize, plural } from "@/lib/format";
import { cn } from "@/lib/utils";
import { Modal, ModalHeader } from "../Modal";

/** One physical disk's health (drivehealth.rs). */
export interface DriveHealth {
    disk: number;
    model: string;
    bus: string;
    letters: string[];
    verdict: "good" | "warning" | "bad";
    reasons: string[];
    temperature: number | null;
    temperatureWarning: number | null;
    wear: number | null;
    powerOnHours: number | null;
    powerCycles: number | null;
    unsafeShutdowns: number | null;
    mediaErrors: number | null;
    reallocated: number | null;
    pending: number | null;
    spare: number | null;
    spareThreshold: number | null;
    written: number | null;
    predictsFailure: boolean;
    tooHot: boolean;
    readable: boolean;
}

const useHealth = create<{ disks: DriveHealth[]; at: number; open: DriveHealth | null }>(() => ({ disks: [], at: 0, open: null }));

/** Reads the drives' health, at most once a minute. */
function refreshHealth() {
    const { at } = useHealth.getState();
    if (Date.now() - at < 60_000) return;
    useHealth.setState({ at: Date.now() });
    invoke<DriveHealth[]>("drive_health").then(
        (disks) => useHealth.setState({ disks }),
        () => {}
    );
}

/** The health of the disk `drivePath` ("C:\") is on, read when This PC shows. */
export function useDriveHealth(drivePath: string): DriveHealth | undefined {
    const disks = useHealth((s) => s.disks);
    useEffect(refreshHealth, []);
    return disks.find((d) => d.readable && d.letters.some((l) => l.toLowerCase() === drivePath.toLowerCase()));
}

/** Disks whose health isn't good, for the banner at the top of This PC. */
export function useFailingDisks() {
    const disks = useHealth((s) => s.disks);
    useEffect(refreshHealth, []);
    return disks.filter((d) => d.readable && d.verdict !== "good");
}

const BUS: Record<string, string> = { nvme: "NVMe", sata: "SATA", usb: "USB", sd: "SD card", raid: "RAID", virtual: "Virtual" };

/** A drive tile's health: a dot and its temperature; a click shows the details. */
export function DriveHealthBadge({ health }: { health: DriveHealth }) {
    const tone = health.verdict === "bad" ? "text-destructive" : health.verdict === "warning" ? "text-amber-600 dark:text-amber-400" : "text-emerald-600 dark:text-emerald-400";
    const label = health.verdict === "bad" ? "Failing" : health.verdict === "warning" ? "Check" : "Healthy";
    return (
        <button
            type="button"
            title={[`${health.model}: ${label.toLowerCase()}`, ...health.reasons, health.temperature != null ? `${health.temperature} °C` : "", health.wear != null ? `${health.wear}% of its life used` : ""].filter(Boolean).join("\n")}
            onClick={(e) => {
                e.stopPropagation();
                useHealth.setState({ open: health });
            }}
            className={cn("flex shrink-0 items-center gap-1 rounded px-1 py-0.5 text-[0.6875rem] font-medium hover:bg-background", tone)}
        >
            {health.verdict === "good" ? <HeartPulse className="size-3" /> : <AlertTriangle className="size-3" />}
            {health.verdict === "good" && health.temperature != null ? `${health.temperature} °C` : label}
        </button>
    );
}

/** The banner on This PC when a disk's health isn't good. */
export function DriveHealthBanner() {
    const failing = useFailingDisks();
    if (!failing.length) return null;
    return (
        <div className="mt-6 space-y-2">
            {failing.map((d) => (
                <button
                    key={d.disk}
                    type="button"
                    onClick={() => useHealth.setState({ open: d })}
                    className={cn(
                        "flex w-full items-start gap-3 rounded-lg border px-4 py-3 text-start text-sm",
                        d.verdict === "bad" ? "border-destructive/40 bg-destructive/10" : "border-amber-500/40 bg-amber-500/10"
                    )}
                >
                    <ShieldAlert className={cn("mt-0.5 size-4 shrink-0", d.verdict === "bad" ? "text-destructive" : "text-amber-600 dark:text-amber-400")} />
                    <span className="min-w-0 flex-1">
                        <b className="font-semibold">
                            {d.model}
                            {d.letters.length > 0 && ` (${d.letters.map((l) => l.replace("\\", "")).join(", ")})`}
                            {d.verdict === "bad" ? " may be failing" : " needs a look"}
                        </b>
                        <span className="block text-xs text-muted-foreground">
                            {d.reasons.join(" · ")}
                            {d.verdict === "bad" && ". Copy what matters off it soon."}
                        </span>
                    </span>
                </button>
            ))}
        </div>
    );
}

/** A disk's health in full: what the drive and Windows report. */
export function DriveHealthDialog() {
    const health = useHealth((s) => s.open);
    if (!health) return null;
    const close = () => useHealth.setState({ open: null });
    const days = health.powerOnHours != null ? Math.floor(health.powerOnHours / 24) : null;
    const rows: [string, string][] = [];
    if (health.temperature != null) rows.push(["Temperature", `${health.temperature} °C${health.temperatureWarning ? ` (warns at ${health.temperatureWarning} °C)` : ""}`]);
    if (health.wear != null) rows.push(["Life used", `${health.wear}% of what it's rated for`]);
    if (health.spare != null) rows.push(["Spare blocks", `${health.spare}% left${health.spareThreshold ? ` (warns below ${health.spareThreshold}%)` : ""}`]);
    if (health.written != null) rows.push(["Written to it", formatSize(health.written)]);
    if (health.powerOnHours != null) rows.push(["Hours on", `${formatCount(health.powerOnHours)} hours${days ? ` (about ${plural(days, "day")})` : ""}`]);
    if (health.powerCycles != null) rows.push(["Times turned on", formatCount(health.powerCycles)]);
    if (health.unsafeShutdowns != null) rows.push(["Power lost while on", formatCount(health.unsafeShutdowns)]);
    if (health.mediaErrors != null) rows.push(["Errors it couldn't fix", formatCount(health.mediaErrors)]);
    if (health.reallocated != null) rows.push(["Sectors moved aside", formatCount(health.reallocated)]);
    if (health.pending != null) rows.push(["Sectors waiting to be checked", formatCount(health.pending)]);
    const verdict = health.verdict === "bad" ? "May be failing" : health.verdict === "warning" ? "Needs a look" : "Healthy";
    return (
        <Modal label="Drive health" onClose={close} className="max-w-md">
            <ModalHeader
                icon={health.verdict === "good" ? HeartPulse : ShieldAlert}
                title={health.model || `Disk ${health.disk}`}
                subtitle={[BUS[health.bus] ?? "", health.letters.length ? `Holds ${health.letters.map((l) => l.replace("\\", "")).join(", ")}` : ""].filter(Boolean).join(" · ")}
                onClose={close}
            />
            <div className="space-y-3 p-4 text-sm">
                <p className={cn("flex items-center gap-2 font-medium", health.verdict === "bad" ? "text-destructive" : health.verdict === "warning" ? "text-amber-600 dark:text-amber-400" : "text-emerald-600 dark:text-emerald-400")}>
                    {health.verdict === "good" ? <HeartPulse className="size-4" /> : <AlertTriangle className="size-4" />} {verdict}
                </p>
                {health.reasons.length > 0 && (
                    <ul className="list-disc space-y-0.5 ps-5 text-xs">
                        {health.reasons.map((r) => (
                            <li key={r}>{r}</li>
                        ))}
                    </ul>
                )}
                <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-xs">
                    {rows.map(([k, v]) => (
                        <div key={k} className="contents">
                            <dt className="text-muted-foreground">{k}</dt>
                            <dd className="tabular-nums">{v}</dd>
                        </div>
                    ))}
                </dl>
                <p className="flex items-start gap-1.5 text-xs text-muted-foreground">
                    <Thermometer className="mt-0.5 size-3.5 shrink-0" /> Read from the drive itself, as Windows passes it on. Drives report differently: a drive that says it's healthy can still fail, so keep backups.
                </p>
            </div>
        </Modal>
    );
}
