import { useEffect, useMemo, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { AlertTriangle, ArrowRight, CheckCircle2, Loader2, PenLine, Regex, RotateCcw, Sparkles, X } from "lucide-react";
import type { FileEntry } from "@/lib/entries";
import { refresh } from "@/lib/explorer";
import { formatCount } from "@/lib/format";
import { batchRename, type RenamePair } from "@/lib/fs";
import { baseName, joinPath, parentPath } from "@/lib/paths";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";

type CaseMode = "none" | "lower" | "upper" | "title" | "snake" | "kebab";

interface BatchRenameState {
    open: boolean;
    items: FileEntry[];
    existingNames: Set<string>;
    lastUndo: RenamePair[] | null;
}

const useBatchRenameStore = create<BatchRenameState>(() => ({
    open: false,
    items: [],
    existingNames: new Set(),
    lastUndo: null,
}));

export function openBatchRename(items: FileEntry[], folderEntries: FileEntry[] = []) {
    if (!items.length) return;
    const selectedSet = new Set(items.map((i) => i.path.toLowerCase()));
    const existingNames = new Set(
        folderEntries
            .filter((e) => !selectedSet.has(e.path.toLowerCase()))
            .map((e) => e.name.toLowerCase())
    );
    useBatchRenameStore.setState({ open: true, items, existingNames });
}

function closeBatchRename() {
    useBatchRenameStore.setState({ open: false });
}

function splitStemExt(entry: FileEntry): { stem: string; ext: string } {
    if (entry.dir) return { stem: entry.name, ext: "" };
    const dot = entry.name.lastIndexOf(".");
    if (dot <= 0) return { stem: entry.name, ext: "" };
    return {
        stem: entry.name.slice(0, dot),
        ext: entry.name.slice(dot + 1),
    };
}

function transformCase(text: string, mode: CaseMode): string {
    if (mode === "none" || !text) return text;
    if (mode === "lower") return text.toLowerCase();
    if (mode === "upper") return text.toUpperCase();
    const words = text
        .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
        .split(/[\s_\-.]+/)
        .filter(Boolean);
    if (!words.length) return text;
    if (mode === "title") {
        return words.map((w) => w.charAt(0).toUpperCase() + w.slice(1).toLowerCase()).join(" ");
    }
    if (mode === "snake") {
        return words.map((w) => w.toLowerCase()).join("_");
    }
    if (mode === "kebab") {
        return words.map((w) => w.toLowerCase()).join("-");
    }
    return text;
}

function formatIsoDate(ms: number): string {
    if (!ms) return "1970-01-01";
    const d = new Date(ms);
    const y = d.getFullYear();
    const m = String(d.getMonth() + 1).padStart(2, "0");
    const day = String(d.getDate()).padStart(2, "0");
    return `${y}-${m}-${day}`;
}

const RESERVED_DOS = new Set([
    "CON", "PRN", "AUX", "NUL",
    "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
]);

function validateTargetName(name: string): string | null {
    if (!name || name === "." || name === "..") return "Name cannot be empty";
    if (name.endsWith(" ") || name.endsWith(".")) return "Cannot end with space or period";
    if (/[<>:"/\\|?*\x00-\x1F]/.test(name)) return "Contains invalid Windows characters";
    const stem = name.split(".")[0]?.toUpperCase() ?? "";
    if (RESERVED_DOS.has(stem)) return "Reserved Windows device name";
    return null;
}

const TOKENS = [
    { token: "{name}", desc: "Original stem" },
    { token: "{ext}", desc: "Extension" },
    { token: "{n}", desc: "1, 2, 3…" },
    { token: "{nn}", desc: "01, 02, 03…" },
    { token: "{nnn}", desc: "001, 002…" },
    { token: "{date}", desc: "YYYY-MM-DD" },
    { token: "{parent}", desc: "Folder name" },
];

export function BatchRenameDialog() {
    const { open, items, existingNames, lastUndo } = useBatchRenameStore();
    const [findText, setFindText] = useState("");
    const [replaceText, setReplaceText] = useState("");
    const [useRegex, setUseRegex] = useState(false);
    const [caseSensitive, setCaseSensitive] = useState(false);
    const [pattern, setPattern] = useState("{name}");
    const [caseMode, setCaseMode] = useState<CaseMode>("none");
    const [startNum, setStartNum] = useState(1);
    const [stepNum, setStepNum] = useState(1);
    const [running, setRunning] = useState(false);

    useEffect(() => {
        if (!open) return;
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") {
                e.preventDefault();
                e.stopPropagation();
                closeBatchRename();
            }
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [open]);

    const resetRules = () => {
        setFindText("");
        setReplaceText("");
        setPattern("{name}");
        setCaseMode("none");
        setStartNum(1);
        setStepNum(1);
    };

    const simulation = useMemo(() => {
        let regexError: string | null = null;
        let matcher: RegExp | null = null;
        if (findText) {
            try {
                const flags = caseSensitive ? "g" : "gi";
                const source = useRegex ? findText : findText.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
                matcher = new RegExp(source, flags);
            } catch (e) {
                regexError = e instanceof Error ? e.message : String(e);
            }
        }

        const draft = items.map((entry, idx) => {
            const { stem: origStem, ext } = splitStemExt(entry);
            let stem = origStem;
            if (matcher) {
                // A regex replacement may use $1 and the like; plain text is taken as it is, $ included.
                stem = useRegex ? stem.replace(matcher, replaceText) : stem.replace(matcher, () => replaceText);
            }
            const seq = startNum + idx * stepNum;
            const parent = baseName(parentPath(entry.path));
            const dateStr = formatIsoDate(entry.modified);

            const patternHasExt = pattern.includes("{ext}");
            // Replacer functions, so a $ in a name (price$&tax.txt) stays a $.
            let builtStem = pattern
                .replace(/\{name\}/g, () => stem)
                .replace(/\{ext\}/g, () => ext)
                .replace(/\{nnn\}/g, () => String(seq).padStart(3, "0"))
                .replace(/\{nn\}/g, () => String(seq).padStart(2, "0"))
                .replace(/\{n\}/g, () => String(seq))
                .replace(/\{date\}/g, () => dateStr)
                .replace(/\{parent\}/g, () => parent);

            builtStem = transformCase(builtStem, caseMode);
            const newName = patternHasExt
                ? builtStem
                : ext
                  ? `${builtStem}.${ext}`
                  : builtStem;

            return {
                entry,
                newName,
                newPath: joinPath(parentPath(entry.path), newName),
                changed: newName !== entry.name,
            };
        });

        // Count target occurrences (case-insensitive per parent folder) to spot collisions
        const counts = new Map<string, number>();
        for (const row of draft) {
            const key = row.newPath.toLowerCase();
            counts.set(key, (counts.get(key) ?? 0) + 1);
        }

        const rows = draft.map((row) => {
            const syntaxErr = validateTargetName(row.newName);
            const key = row.newPath.toLowerCase();
            const batchCollision = (counts.get(key) ?? 0) > 1;
            const diskCollision = row.changed && existingNames.has(row.newName.toLowerCase());
            const conflict =
                syntaxErr ??
                (batchCollision
                    ? "Duplicate target name in batch"
                    : diskCollision
                      ? "Already exists in folder"
                      : null);
            return { ...row, conflict };
        });

        const changedCount = rows.filter((r) => r.changed).length;
        const conflictCount = rows.filter((r) => r.conflict !== null).length;

        return { rows, regexError, changedCount, conflictCount };
    }, [items, existingNames, findText, replaceText, useRegex, caseSensitive, pattern, caseMode, startNum, stepNum]);

    if (!open) return null;

    const applyBatch = async () => {
        if (simulation.regexError || simulation.conflictCount > 0 || simulation.changedCount === 0) return;
        const ops: RenamePair[] = simulation.rows
            .filter((r) => r.changed)
            .map((r) => ({ from: r.entry.path, to: r.newPath }));
        setRunning(true);
        try {
            const res = await batchRename(ops);
            useBatchRenameStore.setState({ open: false, lastUndo: res.undoOps });
            refresh();
            toast.success(`Renamed ${formatCount(res.renamed)} ${res.renamed === 1 ? "item" : "items"} in ${res.elapsedMs.toFixed(1)} ms`, {
                action: res.undoOps.length
                    ? {
                          label: "Undo",
                          onClick: () => {
                              batchRename(res.undoOps)
                                  .then((u) => {
                                      useBatchRenameStore.setState({ lastUndo: null });
                                      refresh();
                                      toast.success(`Reverted ${formatCount(u.renamed)} names`);
                                  })
                                  .catch((e) => toast.error("Couldn't undo batch rename", { description: String(e) }));
                          },
                      }
                    : undefined,
            });
        } catch (e) {
            toast.error("Batch rename failed", { description: String(e) });
        } finally {
            setRunning(false);
        }
    };

    const undoPrevious = async () => {
        if (!lastUndo?.length) return;
        setRunning(true);
        try {
            const res = await batchRename(lastUndo);
            useBatchRenameStore.setState({ open: false, lastUndo: null });
            refresh();
            toast.success(`Reverted ${formatCount(res.renamed)} names in ${res.elapsedMs.toFixed(1)} ms`);
        } catch (e) {
            toast.error("Couldn't undo previous batch", { description: String(e) });
        } finally {
            setRunning(false);
        }
    };

    return (
        <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 backdrop-blur-[1px] animate-in fade-in duration-100"
            onMouseDown={(e) => e.target === e.currentTarget && !running && closeBatchRename()}
        >
            <div
                role="dialog"
                aria-modal="true"
                aria-label="Rename items"
                className="flex max-h-[88vh] w-full max-w-4xl flex-col overflow-hidden rounded-lg border bg-popover text-popover-foreground shadow-2xl"
            >
                {/* Header */}
                <div className="flex items-center gap-2.5 border-b px-4 py-3">
                    <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
                        <PenLine className="size-4" />
                    </div>
                    <div className="min-w-0 flex-1">
                        <h2 className="truncate text-sm font-semibold">
                            Rename {formatCount(items.length)} {items.length === 1 ? "item" : "items"}
                        </h2>
                        <p className="truncate text-xs text-muted-foreground">
                            Two-phase cycle-safe transaction with live conflict & regex simulation
                        </p>
                    </div>
                    {lastUndo && lastUndo.length > 0 && (
                        <Button variant="outline" size="sm" onClick={undoPrevious} disabled={running} title="Undo the last batch rename">
                            <RotateCcw className="size-3.5" /> Undo last batch ({lastUndo.length})
                        </Button>
                    )}
                    <Button variant="ghost" size="sm" onClick={resetRules} disabled={running}>
                        Reset
                    </Button>
                    <Button variant="ghost" size="icon" className="size-7" onClick={closeBatchRename} disabled={running} title="Close (Esc)">
                        <X className="size-4" />
                    </Button>
                </div>

                {/* Rule controls */}
                <div className="grid gap-3 border-b bg-muted/20 p-4 text-xs sm:grid-cols-2">
                    {/* Find & Replace */}
                    <div className="space-y-2 rounded-md border bg-card p-3">
                        <div className="flex items-center justify-between">
                            <span className="font-semibold text-foreground">1. Find & Replace in Name</span>
                            <div className="flex items-center gap-1">
                                <button
                                    type="button"
                                    onClick={() => setUseRegex((v) => !v)}
                                    title="Use regular expressions (supports $1, $2 capture groups)"
                                    className={cn(
                                        "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-medium transition-colors",
                                        useRegex ? "bg-primary/15 text-primary" : "text-muted-foreground hover:bg-muted"
                                    )}
                                >
                                    <Regex className="size-3" /> Regex
                                </button>
                                <button
                                    type="button"
                                    onClick={() => setCaseSensitive((v) => !v)}
                                    title="Match case"
                                    className={cn(
                                        "rounded px-1.5 py-0.5 font-mono text-[11px] font-medium transition-colors",
                                        caseSensitive ? "bg-primary/15 text-primary" : "text-muted-foreground hover:bg-muted"
                                    )}
                                >
                                    Aa
                                </button>
                            </div>
                        </div>
                        <div className="grid grid-cols-2 gap-2">
                            <input
                                type="text"
                                value={findText}
                                onChange={(e) => setFindText(e.target.value)}
                                placeholder={useRegex ? "Regex (e.g. ^IMG_(\\d+))" : "Find text…"}
                                className="h-7 rounded border bg-background px-2 font-mono text-xs outline-none focus:border-primary"
                            />
                            <input
                                type="text"
                                value={replaceText}
                                onChange={(e) => setReplaceText(e.target.value)}
                                placeholder={useRegex ? "Replace (e.g. Photo_$1)" : "Replace with…"}
                                className="h-7 rounded border bg-background px-2 font-mono text-xs outline-none focus:border-primary"
                            />
                        </div>
                        {simulation.regexError && (
                            <p className="text-[11px] text-destructive">{simulation.regexError}</p>
                        )}
                    </div>

                    {/* Pattern Template & Case */}
                    <div className="space-y-2 rounded-md border bg-card p-3">
                        <div className="flex items-center justify-between gap-2">
                            <span className="font-semibold text-foreground">2. Pattern Template & Case</span>
                            <div className="flex items-center gap-1.5 text-[11px]">
                                <span className="text-muted-foreground">Start #</span>
                                <input
                                    type="number"
                                    value={startNum}
                                    onChange={(e) => setStartNum(Number(e.target.value) || 0)}
                                    className="h-6 w-12 rounded border bg-background px-1.5 text-center font-mono outline-none focus:border-primary"
                                />
                                <span className="text-muted-foreground">Step</span>
                                <input
                                    type="number"
                                    value={stepNum}
                                    onChange={(e) => setStepNum(Number(e.target.value) || 1)}
                                    className="h-6 w-12 rounded border bg-background px-1.5 text-center font-mono outline-none focus:border-primary"
                                />
                            </div>
                        </div>
                        <div className="grid grid-cols-[1fr_auto] gap-2">
                            <input
                                type="text"
                                value={pattern}
                                onChange={(e) => setPattern(e.target.value)}
                                placeholder="{name}_{nn}"
                                className="h-7 rounded border bg-background px-2 font-mono text-xs outline-none focus:border-primary"
                            />
                            <select
                                value={caseMode}
                                onChange={(e) => setCaseMode(e.target.value as CaseMode)}
                                className="h-7 rounded border bg-background px-2 text-xs outline-none focus:border-primary"
                            >
                                <option value="none">Keep Case</option>
                                <option value="lower">lowercase</option>
                                <option value="upper">UPPERCASE</option>
                                <option value="title">Title Case</option>
                                <option value="snake">snake_case</option>
                                <option value="kebab">kebab-case</option>
                            </select>
                        </div>
                        <div className="flex flex-wrap gap-1">
                            {TOKENS.map((t) => (
                                <button
                                    key={t.token}
                                    type="button"
                                    title={t.desc}
                                    onClick={() => setPattern((p) => `${p}${t.token}`)}
                                    className="rounded border bg-muted/40 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                                >
                                    {t.token}
                                </button>
                            ))}
                        </div>
                    </div>
                </div>

                {/* Live preview matrix */}
                <div className="min-h-[16rem] flex-1 overflow-y-auto">
                    <table className="w-full border-collapse text-left text-xs">
                        <thead className="sticky top-0 z-10 border-b bg-muted/90 backdrop-blur-xs">
                            <tr>
                                <th className="w-10 px-3 py-2 text-right font-normal text-muted-foreground">#</th>
                                <th className="w-[44%] px-3 py-2 font-semibold text-foreground">Current Name</th>
                                <th className="w-6 px-1 py-2" />
                                <th className="px-3 py-2 font-semibold text-foreground">New Name (Live Preview)</th>
                            </tr>
                        </thead>
                        <tbody className="divide-y">
                            {simulation.rows.map((row, idx) => (
                                <tr
                                    key={row.entry.path}
                                    className={cn(
                                        "transition-colors",
                                        row.conflict
                                            ? "bg-destructive/10"
                                            : row.changed
                                              ? "bg-emerald-500/5 hover:bg-emerald-500/10"
                                              : "hover:bg-muted/30"
                                    )}
                                >
                                    <td className="px-3 py-1.5 text-right font-mono text-[11px] text-muted-foreground tabular-nums">
                                        {idx + 1}
                                    </td>
                                    <td className="max-w-[18rem] truncate px-3 py-1.5 font-mono text-muted-foreground" title={row.entry.name}>
                                        {row.entry.name}
                                    </td>
                                    <td className="px-1 py-1.5 text-center">
                                        <ArrowRight className={cn("size-3.5", row.changed ? "text-primary" : "text-muted-foreground/30")} />
                                    </td>
                                    <td className="px-3 py-1.5 font-mono">
                                        <div className="flex items-center gap-2">
                                            <span
                                                className={cn(
                                                    "truncate",
                                                    row.conflict
                                                        ? "font-semibold text-destructive"
                                                        : row.changed
                                                          ? "font-semibold text-emerald-600 dark:text-emerald-400"
                                                          : "text-muted-foreground"
                                                )}
                                                title={row.newName}
                                            >
                                                {row.newName}
                                            </span>
                                            {row.conflict && (
                                                <span className="ml-auto inline-flex shrink-0 items-center gap-1 rounded bg-destructive/15 px-1.5 py-0.5 font-sans text-[10px] font-medium text-destructive">
                                                    <AlertTriangle className="size-3" /> {row.conflict}
                                                </span>
                                            )}
                                        </div>
                                    </td>
                                </tr>
                            ))}
                        </tbody>
                    </table>
                </div>

                {/* Footer */}
                <div className="flex items-center justify-between border-t bg-muted/20 px-4 py-2.5 text-xs">
                    <div className="flex items-center gap-3">
                        {simulation.conflictCount > 0 ? (
                            <span className="inline-flex items-center gap-1.5 font-medium text-destructive">
                                <AlertTriangle className="size-3.5" />
                                {formatCount(simulation.conflictCount)} {simulation.conflictCount === 1 ? "conflict" : "conflicts"} detected — resolve before renaming
                            </span>
                        ) : simulation.changedCount > 0 ? (
                            <span className="inline-flex items-center gap-1.5 font-medium text-emerald-600 dark:text-emerald-400">
                                <CheckCircle2 className="size-3.5" />
                                Ready to rename {formatCount(simulation.changedCount)} of {formatCount(items.length)} items: all of them, or none if one can't be
                            </span>
                        ) : (
                            <span className="text-muted-foreground">Modify Find/Replace, Pattern, or Case above to preview changes</span>
                        )}
                    </div>
                    <div className="flex items-center gap-2">
                        <Button variant="outline" size="sm" onClick={closeBatchRename} disabled={running}>
                            Cancel
                        </Button>
                        <Button
                            size="sm"
                            onClick={applyBatch}
                            disabled={running || !!simulation.regexError || simulation.conflictCount > 0 || simulation.changedCount === 0}
                        >
                            {running ? <Loader2 className="size-3.5 animate-spin" /> : <Sparkles className="size-3.5" />}
                            Rename {formatCount(simulation.changedCount)} {simulation.changedCount === 1 ? "item" : "items"}
                        </Button>
                    </div>
                </div>
            </div>
        </div>
    );
}
