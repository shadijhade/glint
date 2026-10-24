import { useEffect, useState } from "react";
import { create } from "zustand";
import { open as pickPath } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { AlertTriangle, ArrowRight, FolderOpen, Loader2, Plus, Trash2, Undo2, Wand2 } from "lucide-react";
import { formatDate, plural } from "@/lib/format";
import { baseName } from "@/lib/paths";
import { didWhat, ruleId, type RuleDone, rulesApply, rulesClearLog, rulesLog, rulesPreview, undoRule } from "@/lib/rules";
import { type FolderRule, useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";

const useRulesDialog = create<{ open: boolean; tab: "rules" | "log"; editing: FolderRule | null }>(() => ({ open: false, tab: "rules", editing: null }));

const blank = (folder = ""): FolderRule => ({
    id: ruleId(),
    name: "",
    on: false,
    folder,
    subfolders: false,
    names: "",
    minSize: 0,
    maxSize: 0,
    olderThanDays: 0,
    action: "move",
    target: "",
    pattern: "{date} {name}{ext}",
});

/** Folder rules: the list, or (given a folder) a new rule for it. */
export function openRules(folder?: string) {
    useRulesDialog.setState({ open: true, tab: "rules", editing: folder ? blank(folder) : null });
}
const close = () => useRulesDialog.setState({ open: false, editing: null });

const ACTIONS: { action: FolderRule["action"]; label: string }[] = [
    { action: "move", label: "Move it into a folder" },
    { action: "rename", label: "Rename it" },
    { action: "unblock", label: "Unblock it (take off Windows' downloaded-from-the-internet mark)" },
    { action: "extract", label: "Extract it (an archive) into a folder next to it" },
    { action: "recycle", label: "Put it in the Recycle Bin" },
];

/** A rule in a few words: "In Downloads, *.pdf: move into D:\Docs\{year}". */
function summary(rule: FolderRule) {
    const what = ACTIONS.find((a) => a.action === rule.action)?.label.split(" (")[0] ?? rule.action;
    const where = rule.action === "move" ? ` ${rule.target}` : rule.action === "rename" ? ` to ${rule.pattern}` : "";
    const age = rule.olderThanDays ? `, unchanged ${plural(rule.olderThanDays, "day")}` : "";
    return `In ${baseName(rule.folder) || rule.folder}${rule.subfolders ? " and below" : ""}, ${rule.names.trim() || "every file"}${age}: ${what.toLowerCase()}${where}`;
}

export function RulesDialog() {
    const { open, tab, editing } = useRulesDialog();
    if (!open) return null;
    return (
        <Modal label="Folder rules" onClose={close} className="max-h-[88vh] max-w-2xl">
            <ModalHeader icon={Wand2} title="Folder rules" subtitle="Files that come into a folder, sorted out as they come, while Glint runs" onClose={close}>
                <div role="tablist" aria-label="Rules or what they did" className="flex shrink-0 rounded-md border p-0.5 text-xs">
                    {(["rules", "log"] as const).map((t) => (
                        <button
                            key={t}
                            type="button"
                            role="tab"
                            aria-selected={tab === t}
                            onClick={() => useRulesDialog.setState({ tab: t, editing: null })}
                            className={cn("rounded px-2 py-0.5 transition-colors", tab === t ? "bg-primary/15 text-primary" : "text-muted-foreground hover:text-foreground")}
                        >
                            {t === "rules" ? "Rules" : "What they did"}
                        </button>
                    ))}
                </div>
            </ModalHeader>
            <div className="min-h-0 flex-1 overflow-y-auto p-4 text-sm">{tab === "log" ? <Log /> : editing ? <Editor key={editing.id} start={editing} /> : <List />}</div>
        </Modal>
    );
}

function List() {
    const rules = useStore((s) => s.rules);
    return (
        <div className="space-y-3">
            {rules.length === 0 && (
                <p className="text-muted-foreground">
                    A rule watches a folder, such as Downloads, and does something with the files that come into it: moves PDFs into Documents\Invoices\{"{year}"}, renames scans by their date,
                    unblocks installers, extracts archives, or clears out what's been there a month. It's off until you turn it on, after a look at what it would do.
                </p>
            )}
            {rules.map((rule) => (
                <div key={rule.id} className="flex items-center gap-3 rounded-lg border px-3 py-2">
                    <input
                        type="checkbox"
                        role="switch"
                        aria-label={`${rule.name}: on`}
                        checked={rule.on}
                        // Turning one on goes through its preview first; turning it off is at once.
                        onChange={(e) => (e.target.checked ? useRulesDialog.setState({ editing: rule }) : useStore.getState().saveRule({ ...rule, on: false }))}
                        className="size-4 accent-primary"
                    />
                    <div className="min-w-0 flex-1">
                        <p className="truncate font-medium">{rule.name || "A rule"}</p>
                        <p className="truncate text-xs text-muted-foreground" title={summary(rule)}>
                            {summary(rule)}
                        </p>
                    </div>
                    <Button size="sm" variant="outline" className="h-7" onClick={() => useRulesDialog.setState({ editing: rule })}>
                        Edit
                    </Button>
                </div>
            ))}
            <Button size="sm" onClick={() => useRulesDialog.setState({ editing: blank() })}>
                <Plus /> New rule
            </Button>
        </div>
    );
}

const INPUT = "h-8 w-full rounded-md border bg-background px-2 text-sm outline-hidden focus:border-primary";

function Field({ label, children, hint }: { label: string; children: React.ReactNode; hint?: string }) {
    return (
        <label className="flex items-start gap-3">
            <span className="w-28 shrink-0 pt-1.5 text-xs font-semibold text-muted-foreground">{label}</span>
            <span className="min-w-0 flex-1 space-y-1">
                {children}
                {hint && <span className="block text-[0.6875rem] text-muted-foreground">{hint}</span>}
            </span>
        </label>
    );
}

const megabytes = (bytes: number) => (bytes ? String(Math.round((bytes / (1 << 20)) * 100) / 100) : "");
const bytesOf = (text: string) => Math.round((Number(text.replace(",", ".")) || 0) * (1 << 20));

function Editor({ start }: { start: FolderRule }) {
    const [rule, setRule] = useState(start);
    const [preview, setPreview] = useState<{ running: boolean; found?: RuleDone[]; error?: string } | null>(null);
    const known = useStore((s) => s.rules.some((r) => r.id === start.id));
    const change = (patch: Partial<FolderRule>) => {
        setRule((r) => ({ ...r, ...patch }));
        setPreview(null);
    };
    const named = { ...rule, name: rule.name.trim() || (baseName(rule.folder) ? `In ${baseName(rule.folder)}` : "A rule") };
    const look = async () => {
        setPreview({ running: true });
        try {
            setPreview({ running: false, found: await rulesPreview(named) });
        } catch (e) {
            setPreview({ running: false, error: String(e) });
        }
    };
    // A rule that's off (asked to turn on, from the list): what it would do, at once.
    useEffect(() => {
        if (known && !start.on) void look();
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    const save = (on: boolean) => {
        useStore.getState().saveRule({ ...named, on });
        useRulesDialog.setState({ editing: null });
    };
    const turnOnWithThese = async () => {
        save(true);
        try {
            const did = await rulesApply({ ...named, on: true });
            toast.success(`Rule “${named.name}” is on`, { description: `It did it to ${plural(did, "file")} there now; what comes later goes the same way.` });
        } catch (e) {
            toast.error("Couldn't do it to what's there", { description: String(e) });
        }
    };
    const browse = async (key: "folder" | "target") => {
        const picked = await pickPath({ directory: true, defaultPath: rule[key] || undefined });
        if (typeof picked === "string") change({ [key]: picked });
    };
    const found = preview?.found ?? [];
    return (
        <div className="space-y-3">
            <Field label="Name">
                <input autoFocus value={rule.name} onChange={(e) => change({ name: e.target.value })} placeholder="PDFs to Documents" aria-label="Name" className={INPUT} />
            </Field>
            <Field label="Folder">
                <span className="flex gap-1.5">
                    <input value={rule.folder} onChange={(e) => change({ folder: e.target.value })} placeholder="C:\Users\you\Downloads" spellCheck={false} aria-label="Folder" className={INPUT} />
                    <Button size="sm" variant="outline" className="h-8" onClick={() => void browse("folder")} aria-label="Choose the folder">
                        <FolderOpen />
                    </Button>
                </span>
            </Field>
            <label className="flex items-center gap-2 pl-[7.75rem]">
                <input type="checkbox" checked={rule.subfolders} onChange={(e) => change({ subfolders: e.target.checked })} className="accent-primary" />
                <span>Its subfolders too</span>
            </label>
            <Field label="Names" hint="Patterns with * and ?, separated by ; (empty for every file).">
                <input value={rule.names} onChange={(e) => change({ names: e.target.value })} placeholder="*.pdf; invoice*" spellCheck={false} aria-label="Names" className={INPUT} />
            </Field>
            <Field label="Size" hint="In MB; either can be empty.">
                <span className="flex items-center gap-2">
                    <input value={megabytes(rule.minSize)} onChange={(e) => change({ minSize: bytesOf(e.target.value) })} placeholder="at least" inputMode="decimal" aria-label="At least (MB)" className={INPUT} />
                    <span className="text-muted-foreground">to</span>
                    <input value={megabytes(rule.maxSize)} onChange={(e) => change({ maxSize: bytesOf(e.target.value) })} placeholder="at most" inputMode="decimal" aria-label="At most (MB)" className={INPUT} />
                </span>
            </Field>
            <Field label="Unchanged for" hint="Days since it last changed (empty: as soon as it comes). Rules with an age are also checked every hour.">
                <input
                    value={rule.olderThanDays ? String(rule.olderThanDays) : ""}
                    onChange={(e) => change({ olderThanDays: Number(e.target.value.replace(/\D/g, "")) || 0 })}
                    placeholder="0"
                    inputMode="numeric"
                    aria-label="Unchanged for (days)"
                    className={INPUT}
                />
            </Field>
            <Field label="Then">
                <select value={rule.action} onChange={(e) => change({ action: e.target.value as FolderRule["action"] })} aria-label="Then" className={INPUT}>
                    {ACTIONS.map((a) => (
                        <option key={a.action} value={a.action}>
                            {a.label}
                        </option>
                    ))}
                </select>
            </Field>
            {rule.action === "move" && (
                <Field label="Into" hint="{year}, {month} and {day} of the file's date can make folders. A name that's taken gets “(2)”: nothing's replaced.">
                    <span className="flex gap-1.5">
                        <input value={rule.target} onChange={(e) => change({ target: e.target.value })} placeholder="D:\Documents\Invoices\{year}" spellCheck={false} aria-label="Into" className={INPUT} />
                        <Button size="sm" variant="outline" className="h-8" onClick={() => void browse("target")} aria-label="Choose where">
                            <FolderOpen />
                        </Button>
                    </span>
                </Field>
            )}
            {rule.action === "rename" && (
                <Field label="New name" hint="Of {name}, {ext}, {date}, {time}, {year}, {month} and {day} (the file's date). A name that's taken gets “(2)”.">
                    <input value={rule.pattern} onChange={(e) => change({ pattern: e.target.value })} spellCheck={false} aria-label="New name" className={INPUT} />
                </Field>
            )}
            <div className="flex flex-wrap items-center gap-2 border-t pt-3">
                <Button size="sm" variant="outline" disabled={!rule.folder.trim() || !!preview?.running} onClick={() => void look()}>
                    {preview?.running ? <Loader2 className="animate-spin" /> : <Wand2 />} What would it do?
                </Button>
                {known && (
                    <Button
                        size="sm"
                        variant="ghost"
                        className="text-destructive hover:text-destructive"
                        onClick={() => {
                            useStore.getState().removeRule(rule.id);
                            useRulesDialog.setState({ editing: null });
                        }}
                    >
                        <Trash2 /> Delete the rule
                    </Button>
                )}
                <div className="flex-1" />
                <Button size="sm" variant="ghost" onClick={() => useRulesDialog.setState({ editing: null })}>
                    Cancel
                </Button>
                <Button size="sm" variant="outline" disabled={!rule.folder.trim()} onClick={() => save(start.on && known)} title="Keep it as it is: on stays on, off stays off">
                    Save
                </Button>
            </div>
            {preview?.error && (
                <p className="flex items-start gap-2 text-xs text-destructive">
                    <AlertTriangle className="mt-0.5 size-3.5 shrink-0" /> {preview.error}
                </p>
            )}
            {preview?.found && (
                <div className="space-y-2 rounded-lg border p-3">
                    <p className="font-medium">{found.length ? `${plural(found.length, "file")} there now it would take${found.length >= 500 ? " (the first 500)" : ""}:` : "Nothing there now that it takes: it would wait for what comes."}</p>
                    {found.length > 0 && (
                        <ul className="max-h-56 space-y-0.5 overflow-y-auto text-xs">
                            {found.map((f) => (
                                <li key={f.from} className="flex min-w-0 items-center gap-1.5">
                                    <span className="max-w-[45%] shrink-0 truncate" title={f.from}>
                                        {f.from.slice(rule.folder.replace(/\\+$/, "").length + 1) || baseName(f.from)}
                                    </span>
                                    <ArrowRight className="size-3 shrink-0 text-muted-foreground" />
                                    <span className="truncate text-muted-foreground" title={f.to ?? f.action}>
                                        {f.to ?? f.action}
                                    </span>
                                </li>
                            ))}
                        </ul>
                    )}
                    <div className="flex flex-wrap gap-2 pt-1">
                        <Button size="sm" onClick={() => save(true)}>
                            Turn it on: for what comes from now
                        </Button>
                        {found.length > 0 && (
                            <Button size="sm" variant="outline" onClick={() => void turnOnWithThese()}>
                                Turn it on, and do it to {found.length === 1 ? "this one" : `these ${found.length}`} now
                            </Button>
                        )}
                    </div>
                    <p className="text-[0.6875rem] text-muted-foreground">It runs while Glint runs. All it does is in What they did, each with Undo.</p>
                </div>
            )}
        </div>
    );
}

function Log() {
    const [log, setLog] = useState<RuleDone[] | null>(null);
    const read = () => rulesLog().then((l) => setLog([...l].reverse()), () => setLog([]));
    useEffect(() => {
        void read();
    }, []);
    if (!log) return <Loader2 className="mx-auto size-5 animate-spin text-primary" />;
    if (!log.length) return <p className="py-6 text-center text-muted-foreground">Rules haven't done anything yet.</p>;
    return (
        <div className="space-y-2">
            <ul className="space-y-1">
                {log.map((done) => (
                    <li key={done.id} className={cn("flex items-center gap-3 rounded-md px-2 py-1.5 hover:bg-accent/50", done.undone && "opacity-60")}>
                        <div className="min-w-0 flex-1">
                            <p className="truncate">
                                <span className="text-muted-foreground">{formatDate(done.at)} · {done.rule}:</span> {didWhat(done)}
                                {done.undone && <span className="text-muted-foreground"> (taken back)</span>}
                            </p>
                            {done.error ? (
                                <p className="truncate text-xs text-destructive">{done.error}</p>
                            ) : (
                                done.to && (
                                    <p className="truncate text-xs text-muted-foreground" title={done.to}>
                                        {done.to}
                                    </p>
                                )
                            )}
                        </div>
                        {!done.error && !done.undone && !!done.to && (
                            <Button size="sm" variant="outline" className="h-7" onClick={() => void undoRule(done).then(read)}>
                                <Undo2 /> Undo
                            </Button>
                        )}
                    </li>
                ))}
            </ul>
            <Button size="sm" variant="ghost" onClick={() => void rulesClearLog().then(read)}>
                Clear this list
            </Button>
        </div>
    );
}
