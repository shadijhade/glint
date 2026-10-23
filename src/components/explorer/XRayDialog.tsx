import { useEffect, useState } from "react";
import { create } from "zustand";
import { confirm } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import {
    Activity,
    AlertTriangle,
    Check,
    CheckCircle2,
    Copy,
    Cpu,
    Fingerprint,
    HardDrive,
    Layers,
    Loader2,
    ShieldAlert,
    ShieldCheck,
    Trash2,
} from "lucide-react";
import { formatCount, formatSize } from "@/lib/format";
import { cancelXRay, type FileXRayReport, inspectFileXRay, removeNtfsStream, signable } from "@/lib/fs";
import { baseName } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";
import { describeSignature, TONES, useSignature } from "./SignatureChip";

interface XRayState {
    open: boolean;
    path: string;
    loading: boolean;
    /** Hashing the whole file (asked for), rather than its first 256 MB. */
    full: boolean;
    report: FileXRayReport | null;
    error: string | null;
}

const useXRayStore = create<XRayState>(() => ({
    open: false,
    path: "",
    loading: false,
    full: false,
    report: null,
    error: null,
}));

let activeXRayId = 0;

export async function inspectXRay(path: string, full = false) {
    if (!path) return;
    const reqId = ++activeXRayId;
    useXRayStore.setState({ open: true, path, loading: true, full, report: null, error: null });
    try {
        const report = await inspectFileXRay(path, full);
        if (reqId === activeXRayId) {
            useXRayStore.setState({ report, loading: false });
        }
    } catch (e) {
        if (reqId === activeXRayId) {
            useXRayStore.setState({ error: String(e), loading: false });
        }
    }
}

function closeXRay() {
    activeXRayId++;
    cancelXRay().catch(() => {});
    useXRayStore.setState({ open: false });
}

function CopyHashButton({ value, label }: { value: string; label: string }) {
    const [copied, setCopied] = useState(false);
    const copy = () => {
        navigator.clipboard.writeText(value).then(
            () => {
                setCopied(true);
                toast.success(`Copied ${label}`, { duration: 1300 });
                setTimeout(() => setCopied(false), 1400);
            },
            () => toast.error(`Couldn't copy ${label}`)
        );
    };
    return (
        <button
            type="button"
            onClick={copy}
            title={`Copy ${label}`}
            className="rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        >
            {copied ? <Check className="size-3.5 text-emerald-500" /> : <Copy className="size-3.5" />}
        </button>
    );
}

/** Whether the file's signature holds, and who signed it, as Windows checks it offline. */
function SignatureSection({ path }: { path: string }) {
    const signature = useSignature(path);
    const described = signature && describeSignature(signature);
    return (
        <div className="space-y-2 rounded-lg border bg-card p-3.5">
            <div className="flex flex-wrap items-center gap-2">
                <span className="flex items-center gap-1.5 font-semibold text-foreground">
                    <ShieldCheck className="size-3.5 text-primary" />
                    Signature
                </span>
                {described ? (
                    <span className={cn("rounded px-2 py-0.5 font-medium", TONES[described.tone])}>{described.text}</span>
                ) : (
                    <span className="inline-flex items-center gap-1 text-muted-foreground">
                        <Loader2 className="size-3 animate-spin" /> Checking…
                    </span>
                )}
            </div>
            {described && described.details.length > 0 && (
                <ul className="space-y-0.5 pl-5 text-muted-foreground">
                    {described.details.map((d) => (
                        <li key={d} className="list-disc">
                            {d}
                        </li>
                    ))}
                </ul>
            )}
        </div>
    );
}

export function XRayDialog() {
    const { open, path, loading, full, report, error } = useXRayStore();
    const [expectedHash, setExpectedHash] = useState("");
    const [removingStream, setRemovingStream] = useState<string | null>(null);
    // A checksum pasted for one file isn't compared with the next one opened.
    useEffect(() => {
        if (open) setExpectedHash("");
    }, [open, path]);


    if (!open) return null;

    // SHA-256 and BLAKE3 are both 64 hex digits: a pasted one is compared with each.
    const cleanExpected = expectedHash.trim().toLowerCase();
    const matchedAlgo =
        report && cleanExpected
            ? cleanExpected === report.sha256.toLowerCase()
                ? "SHA-256"
                : cleanExpected === report.blake3.toLowerCase()
                  ? "BLAKE3"
                  : cleanExpected === report.crc32.toLowerCase()
                    ? "CRC32"
                    : null
            : null;

    const handleRemoveStream = async (streamName: string) => {
        const name = baseName(path) || path;
        const zone = streamName.toLowerCase() === "zone.identifier";
        const ok = await confirm(
            zone
                ? `Unblock “${name}”?\n\nWindows marked it as downloaded from the internet. Unblocking removes that mark, so Windows (SmartScreen, Office's Protected View) stops warning you before it opens. Only unblock files you trust.`
                : `Delete the hidden stream “:${streamName}” from “${name}”?\n\nWhat another program stored in it is gone for good.`,
            { title: zone ? "Unblock file" : "Delete stream", kind: "warning", okLabel: zone ? "Unblock" : "Delete", cancelLabel: "Cancel" }
        );
        if (!ok) return;
        setRemovingStream(streamName);
        try {
            await removeNtfsStream(path, streamName);
            toast.success(zone ? `Unblocked “${name}”` : `Deleted the stream “:${streamName}”`);
            await inspectXRay(path);
        } catch (e) {
            toast.error(zone ? `Couldn't unblock “${name}”` : "Couldn't delete the stream", { description: String(e) });
        } finally {
            setRemovingStream(null);
        }
    };

    const maxBucket = report ? Math.max(1, ...report.byteSpectrum) : 1;

    return (
        <Modal label="File X-Ray" onClose={closeXRay} className="max-h-[88vh] max-w-3xl">
            <ModalHeader icon={Fingerprint} title={`File X-Ray: ${baseName(path) || path}`} subtitle={path} onClose={closeXRay} onRefresh={() => inspectXRay(path)} refreshing={loading}>
                {report && <span className="text-xs text-muted-foreground tabular-nums">{report.elapsedMs.toFixed(1)} ms</span>}
            </ModalHeader>

            {/* Body */}
            <div className="min-h-[18rem] flex-1 space-y-4 overflow-y-auto p-4 text-xs select-text">
                {loading && (
                    <div className="flex h-64 flex-col items-center justify-center gap-2.5 text-sm text-muted-foreground">
                        <Loader2 className="size-6 animate-spin text-primary" />
                        <p>{full ? "Hashing the whole file…" : "Reading the file…"}</p>
                    </div>
                )}

                {!loading && error && (
                    <div className="flex h-64 flex-col items-center justify-center gap-2 text-center text-sm">
                        <AlertTriangle className="size-6 text-destructive" />
                        <p className="font-medium">Couldn't X-Ray this item</p>
                        <p className="max-w-md text-xs text-muted-foreground">{error}</p>
                    </div>
                )}

                {!loading && !error && report && (
                    <>
                        {/* Downloaded from the internet: Windows' mark (Zone.Identifier), and Unblock. */}
                        {report.zoneInfo && (
                            <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-amber-500/30 bg-amber-500/10 p-3">
                                <div className="space-y-1">
                                    <div className="flex items-center gap-1.5 font-semibold text-amber-600 dark:text-amber-400">
                                        <ShieldAlert className="size-4 shrink-0" />
                                        Marked as downloaded · {report.zoneInfo.zoneLabel}
                                    </div>
                                    {report.zoneInfo.hostUrl && (
                                        <p className="max-w-lg truncate font-mono text-[0.6875rem] text-muted-foreground" title={report.zoneInfo.hostUrl}>
                                            Source URL: {report.zoneInfo.hostUrl}
                                        </p>
                                    )}
                                    {report.zoneInfo.referrerUrl && (
                                        <p className="max-w-lg truncate font-mono text-[0.6875rem] text-muted-foreground" title={report.zoneInfo.referrerUrl}>
                                            Referrer: {report.zoneInfo.referrerUrl}
                                        </p>
                                    )}
                                </div>
                                <Button
                                    size="sm"
                                    variant="outline"
                                    disabled={removingStream !== null}
                                    onClick={() => handleRemoveStream("Zone.Identifier")}
                                    className="border-amber-500/40 bg-background hover:bg-amber-500/20"
                                >
                                    {removingStream === "Zone.Identifier" ? (
                                        <Loader2 className="size-3.5 animate-spin" />
                                    ) : (
                                        <ShieldCheck className="size-3.5 text-emerald-500" />
                                    )}
                                    Unblock
                                </Button>
                            </div>
                        )}

                        {signable(path) && <SignatureSection path={path} />}

                        {/* 1. Checksums, and comparing one with a published one */}
                        <div className="space-y-2.5 rounded-lg border bg-card p-3.5">
                            <div className="flex flex-wrap items-center justify-between gap-2">
                                <div className="flex items-center gap-1.5 font-semibold text-foreground">
                                    <Fingerprint className="size-3.5 text-primary" />
                                    Checksums
                                    {!report.hashesComplete && (
                                        <span className="rounded bg-amber-500/15 px-1.5 py-0.5 text-[0.625rem] font-normal text-amber-600 dark:text-amber-400">
                                            Of the first 256 MB ·{" "}
                                            <button type="button" onClick={() => inspectXRay(path, true)} className="font-medium underline underline-offset-2 hover:text-amber-700 dark:hover:text-amber-300">
                                                Hash the whole file
                                            </button>
                                        </span>
                                    )}
                                </div>
                                <div className="flex items-center gap-2">
                                    <input
                                        type="text"
                                        value={expectedHash}
                                        onChange={(e) => setExpectedHash(e.target.value)}
                                        placeholder="Paste a checksum to compare"
                                        title="A SHA-256, BLAKE3 or CRC32 checksum, as a download page publishes it"
                                        className="h-6 w-64 rounded border bg-background px-2 font-mono text-[0.6875rem] outline-hidden focus:border-primary"
                                    />
                                    {cleanExpected &&
                                        (matchedAlgo ? (
                                            <span className="inline-flex items-center gap-1 rounded bg-emerald-500/15 px-2 py-0.5 font-semibold text-emerald-600 dark:text-emerald-400">
                                                <CheckCircle2 className="size-3" /> {matchedAlgo} matches
                                            </span>
                                        ) : report.hashesComplete ? (
                                            <span className="inline-flex items-center gap-1 rounded bg-destructive/15 px-2 py-0.5 font-semibold text-destructive">
                                                <AlertTriangle className="size-3" /> Doesn't match
                                            </span>
                                        ) : (
                                            <span className="inline-flex items-center gap-1 rounded bg-amber-500/15 px-2 py-0.5 font-medium text-amber-600 dark:text-amber-400">
                                                Not checked: hash the whole file first
                                            </span>
                                        ))}
                                </div>
                            </div>

                            <div className="divide-y rounded-md border bg-muted/15 font-mono text-[0.6875rem]">
                                {[
                                    { label: "SHA-256", value: report.sha256 },
                                    { label: "BLAKE3", value: report.blake3 },
                                    { label: "CRC32", value: report.crc32 },
                                ].map((row) => {
                                    const isMatch = matchedAlgo === row.label;
                                    return (
                                        <div
                                            key={row.label}
                                            className={cn(
                                                "flex items-center gap-3 px-2.5 py-1.5",
                                                isMatch && "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
                                            )}
                                        >
                                            <span className="w-24 shrink-0 font-sans text-xs font-medium text-muted-foreground">
                                                {row.label}
                                            </span>
                                            <span className="min-w-0 flex-1 truncate">{row.value}</span>
                                            <CopyHashButton value={row.value} label={row.label} />
                                        </div>
                                    );
                                })}
                            </div>
                        </div>

                        {/* 2. Entropy, and which byte values the file is made of */}
                        <div className="space-y-3 rounded-lg border bg-card p-3.5">
                            <div className="flex flex-wrap items-center justify-between gap-2">
                                <div className="flex items-center gap-1.5 font-semibold text-foreground">
                                    <Activity className="size-3.5 text-primary" />
                                    Entropy and byte values
                                </div>
                                <span className="rounded-md bg-primary/10 px-2 py-0.5 font-mono text-[0.6875rem] font-semibold text-primary tabular-nums">
                                    H = {report.entropy.toFixed(3)} / 8.000 bits/B
                                </span>
                            </div>

                            <div className="space-y-1">
                                <div className="flex justify-between text-[0.6875rem]">
                                    <span className="font-medium text-foreground">{report.entropyLabel}</span>
                                    <span className="text-muted-foreground tabular-nums">
                                        {Math.round((report.entropy / 8) * 100)}% of the maximum
                                    </span>
                                </div>
                                <div className="h-2 w-full overflow-hidden rounded-full bg-muted">
                                    {/* One color: a reading isn't good or bad. */}
                                    <div className="h-full bg-primary/70 transition-all" style={{ width: `${Math.min(100, (report.entropy / 8) * 100)}%` }} />
                                </div>
                            </div>

                            {/* 16-Bucket Byte Value Histogram (0x00..0xFF) */}
                            <div className="space-y-1 pt-1">
                                <div className="flex h-16 items-end gap-1 rounded border bg-muted/20 px-2 pt-2 pb-1">
                                    {report.byteSpectrum.map((pct, idx) => {
                                        const hPct = Math.max(3, Math.round((pct / maxBucket) * 100));
                                        const rangeLabel = `0x${(idx * 16).toString(16).toUpperCase().padStart(2, "0")}–0x${(idx * 16 + 15).toString(16).toUpperCase().padStart(2, "0")}`;
                                        return (
                                            <div
                                                key={idx}
                                                title={`${rangeLabel}: ${pct.toFixed(1)}% of bytes`}
                                                className="flex h-full flex-1 flex-col items-center justify-end"
                                            >
                                                <div
                                                    className="w-full rounded-t-[2px] bg-primary/75 transition-all hover:bg-primary"
                                                    style={{ height: `${hPct}%` }}
                                                />
                                            </div>
                                        );
                                    })}
                                </div>
                                <div className="flex justify-between px-1 font-mono text-[0.625rem] text-muted-foreground">
                                    <span>0x00 (NUL)</span>
                                    <span>0x20–0x7E (ASCII: {report.asciiPct.toFixed(1)}%)</span>
                                    <span>0x80–0xFF (High: {report.highBytePct.toFixed(1)}%)</span>
                                </div>
                            </div>
                        </div>

                        {/* 3. Size on disk, and the file's NTFS record */}
                        <div className="grid gap-3 sm:grid-cols-2">
                            <div className="space-y-2 rounded-lg border bg-card p-3.5">
                                <div className="flex items-center gap-1.5 font-semibold text-foreground">
                                    <HardDrive className="size-3.5 text-primary" />
                                    Size on disk
                                </div>
                                <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[0.6875rem]">
                                    <dt className="text-muted-foreground">Logical size</dt>
                                    <dd className="text-right font-mono tabular-nums">
                                        {formatSize(report.logicalSize)} ({formatCount(report.logicalSize)} B)
                                    </dd>
                                    <dt className="text-muted-foreground">Allocated on disk</dt>
                                    <dd className="text-right font-mono tabular-nums">
                                        {formatSize(report.allocatedSize)} ({formatCount(report.allocatedSize)} B)
                                    </dd>
                                    <dt className="text-muted-foreground">NTFS hardlinks</dt>
                                    <dd className="text-right font-mono tabular-nums">{report.hardlinkCount}</dd>
                                </dl>
                            </div>

                            <div className="space-y-2 rounded-lg border bg-card p-3.5">
                                <div className="flex items-center gap-1.5 font-semibold text-foreground">
                                    <Cpu className="size-3.5 text-primary" />
                                    File record
                                </div>
                                <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[0.6875rem]">
                                    <dt className="text-muted-foreground">MFT File ID</dt>
                                    <dd className="text-right font-mono tabular-nums">{report.fileIndex}</dd>
                                    <dt className="text-muted-foreground">Volume Serial</dt>
                                    <dd className="text-right font-mono tabular-nums">{report.volumeSerial}</dd>
                                    <dt className="text-muted-foreground">Attributes</dt>
                                    <dd className="text-right">
                                        {report.attributes.length ? report.attributes.join(", ") : "Normal"}
                                    </dd>
                                </dl>
                            </div>
                        </div>

                        {/* 4. Hidden streams (NTFS alternate data streams) */}
                        <div className="space-y-2 rounded-lg border bg-card p-3.5">
                            <div className="flex items-center justify-between">
                                <div className="flex items-center gap-1.5 font-semibold text-foreground">
                                    <Layers className="size-3.5 text-primary" />
                                    Hidden streams ({report.streams.length})
                                </div>
                                <span className="text-[0.6875rem] text-muted-foreground">NTFS alternate data streams</span>
                            </div>

                            {report.streams.length === 0 ? (
                                <p className="py-2 text-[0.6875rem] text-muted-foreground">
                                    None: the file has only its own content.
                                </p>
                            ) : (
                                <div className="divide-y rounded-md border bg-muted/10">
                                    {report.streams.map((s) => (
                                        <div key={s.name} className="space-y-1.5 p-2.5">
                                            <div className="flex items-center gap-2">
                                                <span className="font-mono font-semibold text-foreground">:{s.cleanName}</span>
                                                <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-[0.625rem] text-muted-foreground tabular-nums">
                                                    {formatSize(s.size)}
                                                </span>
                                                <Button
                                                    variant="ghost"
                                                    size="sm"
                                                    disabled={removingStream !== null}
                                                    onClick={() => handleRemoveStream(s.cleanName)}
                                                    className="ml-auto h-6 px-2 text-[0.6875rem] text-destructive hover:bg-destructive/10 hover:text-destructive"
                                                    title={`Delete the stream :${s.cleanName}`}
                                                >
                                                    {removingStream === s.cleanName ? (
                                                        <Loader2 className="size-3 animate-spin" />
                                                    ) : (
                                                        <Trash2 className="size-3" />
                                                    )}
                                                    Delete
                                                </Button>
                                            </div>
                                            {s.preview && (
                                                <pre className="max-h-28 overflow-auto rounded border bg-background/70 p-2 font-mono text-[0.625rem] text-muted-foreground">
                                                    {s.preview}
                                                </pre>
                                            )}
                                        </div>
                                    ))}
                                </div>
                            )}
                        </div>
                    </>
                )}
            </div>

            {/* Footer */}
            <div className="flex items-center justify-between border-t bg-muted/20 px-4 py-2.5 text-xs text-muted-foreground">
                <span>{report ? (report.hashesComplete ? "SHA-256, BLAKE3 and CRC32 of the whole file" : "SHA-256, BLAKE3 and CRC32 of the first 256 MB") : ""}</span>
                <Button variant="outline" size="sm" onClick={closeXRay}>
                    Close
                </Button>
            </div>
        </Modal>
    );
}
