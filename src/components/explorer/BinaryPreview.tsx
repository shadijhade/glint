import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { ClipboardCopy, Cpu, Layers, Search, ShieldCheck, Zap } from "lucide-react";
import type { FileEntry } from "@/lib/entries";
import { kindOf } from "@/lib/fileTypes";
import { formatCount, formatDate, formatDuration, formatSize } from "@/lib/format";
import { inspectPe, type PeInfo } from "@/lib/fs";
import { cn } from "@/lib/utils";

const PE_EXTS = new Set(["exe", "dll", "sys", "ocx", "drv", "cpl", "scr", "efi", "pyd", "node", "mui"]);

export const isPeCandidate = (entry: FileEntry) => !entry.dir && PE_EXTS.has(entry.ext);

const hex = (n: number, pad = 8) => `0x${n.toString(16).toUpperCase().padStart(pad, "0")}`;

type Tab = "overview" | "imports" | "exports";

export function BinaryPreview({ entry }: { entry: FileEntry }) {
    const [info, setInfo] = useState<PeInfo | null>(null);
    const [failed, setFailed] = useState(false);
    const [tab, setTab] = useState<Tab>("overview");
    const [query, setQuery] = useState("");

    useEffect(() => {
        let stale = false;
        setInfo(null);
        setFailed(false);
        inspectPe(entry.path).then(
            (res) => {
                if (!stale) setInfo(res);
            },
            () => {
                if (!stale) setFailed(true);
            }
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified]);

    const filteredImports = useMemo(() => {
        if (!info) return [];
        const q = query.trim().toLowerCase();
        if (!q) return info.imports;
        return info.imports
            .map((m) => {
                if (m.module.toLowerCase().includes(q)) return m;
                const symbols = m.symbols.filter((s) => s.toLowerCase().includes(q));
                return symbols.length ? { ...m, symbols } : null;
            })
            .filter((m): m is NonNullable<typeof m> => m !== null);
    }, [info, query]);

    const filteredExports = useMemo(() => {
        if (!info) return [];
        const q = query.trim().toLowerCase();
        if (!q) return info.exports;
        return info.exports.filter((e) => e.name.toLowerCase().includes(q) || `#${e.ordinal}` === q);
    }, [info, query]);

    if (failed) {
        const kind = kindOf(entry);
        return (
            <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center text-sm text-muted-foreground">
                <kind.icon className={cn("size-16 stroke-[1.25]", kind.tone)} />
                <p>No binary header preview for this file</p>
            </div>
        );
    }

    if (!info) return null;

    const maxSectionBytes = Math.max(1, ...info.sections.map((s) => Math.max(s.rawSize, s.virtualSize)));
    const copySymbol = (text: string) => {
        navigator.clipboard.writeText(text).then(
            () => toast.success(`Copied “${text}”`, { duration: 1400 }),
            () => {}
        );
    };

    return (
        <div className="flex h-full min-h-0 flex-col select-text text-xs">
            <div className="flex flex-wrap items-center gap-1.5 border-b bg-chrome/50 px-3 py-2">
                <span className="inline-flex items-center gap-1 rounded-md bg-primary/15 px-2 py-0.5 font-semibold text-primary">
                    <Cpu className="size-3" /> {info.machine}
                </span>
                <span className="rounded-md border bg-background px-2 py-0.5 font-medium text-foreground">
                    {info.format} · {info.isDll ? "DLL" : "Executable"}
                </span>
                <span className="rounded-md border bg-background px-2 py-0.5 text-muted-foreground">{info.subsystem}</span>
                {info.isSigned && (
                    <span
                        className="rounded-md border bg-background px-2 py-0.5 text-muted-foreground"
                        title="A digital signature is attached. Whether it's valid and who signed it: Properties → Digital Signatures."
                    >
                        Signature attached
                    </span>
                )}
                {info.isDotnet && (
                    <span className="rounded-md bg-violet-500/15 px-2 py-0.5 font-medium text-violet-600 dark:text-violet-400">.NET CLR</span>
                )}
                <span className="ml-auto flex items-center gap-1 text-[0.6875rem] text-muted-foreground tabular-nums" title="Parsed directly from the PE header">
                    <Zap className="size-3 text-primary" /> {formatDuration(info.elapsedMs)}
                </span>
            </div>

            <div className="flex shrink-0 items-center gap-1 border-b px-3 py-1.5">
                <button
                    type="button"
                    onClick={() => setTab("overview")}
                    className={cn(
                        "rounded-md px-2 py-1 font-medium transition-colors",
                        tab === "overview" ? "bg-primary/15 text-primary" : "text-muted-foreground hover:bg-accent hover:text-foreground"
                    )}
                >
                    Sections ({info.sections.length})
                </button>
                <button
                    type="button"
                    onClick={() => setTab("imports")}
                    className={cn(
                        "rounded-md px-2 py-1 font-medium transition-colors",
                        tab === "imports" ? "bg-primary/15 text-primary" : "text-muted-foreground hover:bg-accent hover:text-foreground"
                    )}
                >
                    Imports ({info.imports.length})
                </button>
                <button
                    type="button"
                    onClick={() => setTab("exports")}
                    className={cn(
                        "rounded-md px-2 py-1 font-medium transition-colors",
                        tab === "exports" ? "bg-primary/15 text-primary" : "text-muted-foreground hover:bg-accent hover:text-foreground"
                    )}
                >
                    Exports ({formatCount(info.exportCount)})
                </button>
            </div>

            {tab !== "overview" && (
                <div className="flex shrink-0 items-center gap-2 border-b px-3 py-1.5">
                    <Search className="size-3.5 shrink-0 text-muted-foreground" />
                    <input
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        placeholder={tab === "imports" ? "Filter imported DLLs or functions…" : "Filter exported symbols…"}
                        spellCheck={false}
                        className="h-6 min-w-0 flex-1 bg-transparent text-xs outline-hidden placeholder:text-muted-foreground"
                    />
                </div>
            )}

            <div className="min-h-0 flex-1 overflow-y-auto p-3">
                {tab === "overview" && (
                    <div className="flex flex-col gap-3">
                        {info.features.length > 0 && (
                            <div className="flex flex-wrap items-center gap-1.5">
                                <ShieldCheck className="size-3.5 shrink-0 text-muted-foreground" />
                                {info.features.map((f) => (
                                    <span key={f} className="rounded-sm border bg-muted/50 px-1.5 py-0.5 text-[0.6875rem] text-muted-foreground">
                                        {f}
                                    </span>
                                ))}
                            </div>
                        )}

                        <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 rounded-lg border bg-card/50 p-2.5">
                            {info.timestampMs && (
                                <>
                                    <dt className="text-muted-foreground">Compiled</dt>
                                    <dd className="truncate tabular-nums">{formatDate(info.timestampMs)}</dd>
                                </>
                            )}
                            <dt className="text-muted-foreground">Entry point</dt>
                            <dd className="font-mono">{hex(info.entryPoint)}</dd>
                            <dt className="text-muted-foreground">Image base</dt>
                            <dd className="font-mono">{hex(info.imageBase, 12)}</dd>
                            <dt className="text-muted-foreground">Target OS / Linker</dt>
                            <dd className="tabular-nums">
                                Windows {info.osVersion} · Linker {info.linkerVersion}
                            </dd>
                            {info.checksum !== 0 && (
                                <>
                                    <dt className="text-muted-foreground">PE checksum</dt>
                                    <dd className="font-mono">{hex(info.checksum)}</dd>
                                </>
                            )}
                        </dl>

                        <div className="flex flex-col gap-1.5">
                            <div className="flex items-center gap-1.5 font-medium text-foreground">
                                <Layers className="size-3.5 text-primary" /> PE Sections
                            </div>
                            {info.sections.map((s, idx) => {
                                const pct = Math.max(2, Math.round((Math.max(s.rawSize, s.virtualSize) / maxSectionBytes) * 100));
                                return (
                                    <div key={`${s.name}-${idx}`} className="rounded-md border bg-card/40 px-2.5 py-1.5">
                                        <div className="flex items-center justify-between gap-2">
                                            <span className="font-mono font-semibold text-foreground">{s.name || "(unnamed)"}</span>
                                            <div className="flex items-center gap-1">
                                                {s.flags.map((fl) => (
                                                    <span
                                                        key={fl}
                                                        className={cn(
                                                            "rounded-xs px-1 py-0.2 font-mono text-[0.625rem]",
                                                            fl === "X" ? "bg-amber-500/15 text-amber-700 dark:text-amber-400" : "bg-muted text-muted-foreground"
                                                        )}
                                                    >
                                                        {fl}
                                                    </span>
                                                ))}
                                            </div>
                                        </div>
                                        <div className="mt-1 flex items-center justify-between text-[0.6875rem] text-muted-foreground tabular-nums">
                                            <span>
                                                Raw {formatSize(s.rawSize)} · Virtual {formatSize(s.virtualSize)}
                                            </span>
                                            <span className="font-mono">RVA {hex(s.virtualAddress, 6)}</span>
                                        </div>
                                        <div className="mt-1 h-1 w-full overflow-hidden rounded-full bg-muted">
                                            <div className="h-full rounded-full bg-primary/70" style={{ width: `${pct}%` }} />
                                        </div>
                                    </div>
                                );
                            })}
                        </div>
                    </div>
                )}

                {tab === "imports" && (
                    <div className="flex flex-col gap-2">
                        {filteredImports.length === 0 ? (
                            <p className="py-8 text-center text-muted-foreground">No matching imported modules or functions.</p>
                        ) : (
                            filteredImports.map((imp) => (
                                <div key={imp.module} className="rounded-lg border bg-card/40 p-2.5">
                                    <div className="flex items-center justify-between gap-2">
                                        <span className="font-mono font-semibold text-foreground">{imp.module}</span>
                                        <span className="text-[0.6875rem] text-muted-foreground tabular-nums">
                                            {formatCount(imp.totalCount)} {imp.totalCount === 1 ? "symbol" : "symbols"}
                                        </span>
                                    </div>
                                    {imp.symbols.length > 0 && (
                                        <div className="mt-1.5 flex flex-wrap gap-1">
                                            {imp.symbols.map((sym, k) => (
                                                <button
                                                    key={`${sym}-${k}`}
                                                    type="button"
                                                    onClick={() => copySymbol(sym)}
                                                    title="Click to copy symbol name"
                                                    className="max-w-full truncate rounded-xs border bg-background/80 px-1.5 py-0.5 font-mono text-[0.6875rem] text-muted-foreground transition-colors hover:border-primary/40 hover:text-foreground"
                                                >
                                                    {sym}
                                                </button>
                                            ))}
                                            {imp.totalCount > imp.symbols.length && (
                                                <span className="px-1 py-0.5 text-[0.6875rem] text-muted-foreground">
                                                    +{formatCount(imp.totalCount - imp.symbols.length)} more
                                                </span>
                                            )}
                                        </div>
                                    )}
                                </div>
                            ))
                        )}
                    </div>
                )}

                {tab === "exports" && (
                    <div className="flex flex-col gap-1">
                        {filteredExports.length === 0 ? (
                            <p className="py-8 text-center text-muted-foreground">
                                {info.exportCount === 0 ? "This binary exports no symbols." : "No matching exported symbols."}
                            </p>
                        ) : (
                            <>
                                {filteredExports.map((exp) => (
                                    <div
                                        key={`${exp.ordinal}-${exp.name}`}
                                        className="group flex items-center justify-between gap-2 rounded-md border bg-card/40 px-2.5 py-1.5 hover:bg-accent/40"
                                    >
                                        <div className="flex min-w-0 items-center gap-2">
                                            <span className="shrink-0 font-mono text-[0.6875rem] text-muted-foreground">#{exp.ordinal}</span>
                                            <span className="truncate font-mono font-medium text-foreground" title={exp.name}>
                                                {exp.name}
                                            </span>
                                        </div>
                                        <div className="flex shrink-0 items-center gap-1.5">
                                            <span className="font-mono text-[0.6875rem] text-muted-foreground">{hex(exp.rva, 6)}</span>
                                            <button
                                                type="button"
                                                onClick={() => copySymbol(exp.name)}
                                                title="Copy symbol name"
                                                className="opacity-0 transition-opacity group-hover:opacity-100 hover:text-foreground"
                                            >
                                                <ClipboardCopy className="size-3" />
                                            </button>
                                        </div>
                                    </div>
                                ))}
                                {info.exportCount > info.exports.length && !query && (
                                    <p className="pt-2 text-center text-[0.6875rem] text-muted-foreground">
                                        Showing first {formatCount(info.exports.length)} of {formatCount(info.exportCount)} exports.
                                    </p>
                                )}
                            </>
                        )}
                    </div>
                )}
            </div>
        </div>
    );
}
