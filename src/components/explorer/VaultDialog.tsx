import { useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { invoke } from "@tauri-apps/api/core";
import { Lock, LockOpen } from "lucide-react";
import { type FileEntry, isArchived, isClosed, isOnlineOnly, isRecycled } from "@/lib/entries";
import { type ActionOutput, badOutput, runAction } from "@/lib/actions";
import { navigate } from "@/lib/explorer";
import { formatCount, plural } from "@/lib/format";
import { parentPath } from "@/lib/paths";
import { Modal, ModalHeader } from "../Modal";
import { Choices, RunFooter, RunProgress, RunResult, useActionRun, WhereChooser } from "./ActionParts";

const onDisk = (e: FileEntry) => !isArchived(e) && !isClosed(e) && !isRecycled(e) && !isOnlineOnly(e);
/** What Encrypt takes: files and folders on disk. */
export const canEncrypt = (e: FileEntry) => onDisk(e) && !e.name.toLowerCase().endsWith(".age");
/** An age file, to decrypt. */
export const isAgeFile = (e: FileEntry) => onDisk(e) && !e.dir && e.name.toLowerCase().endsWith(".age");

type Mode = "encrypt" | "decrypt";
const useVault = create<{ files: FileEntry[] | null; mode: Mode }>(() => ({ files: null, mode: "encrypt" }));

/** Opens Encrypt (or Decrypt) on what among `entries` it takes. */
export function openVault(entries: FileEntry[], mode: Mode) {
    const files = entries.filter(mode === "encrypt" ? canEncrypt : isAgeFile);
    if (!files.length) return void toast.message(mode === "encrypt" ? "Nothing chosen to encrypt" : "No .age files chosen");
    useVault.setState({ files, mode });
}

const close = () => useVault.setState({ files: null });

export function VaultDialog() {
    const files = useVault((s) => s.files);
    const mode = useVault((s) => s.mode);
    if (!files) return null;
    return <VaultModal key={files.map((f) => f.path).join("\n")} files={files} mode={mode} />;
}

function VaultModal({ files, mode }: { files: FileEntry[]; mode: Mode }) {
    const [lock, setLock] = useState<"passphrase" | "keys">("passphrase");
    const [password, setPassword] = useState("");
    const [again, setAgain] = useState("");
    const [keys, setKeys] = useState("");
    const [open, setOpen] = useState(true);
    const [output, setOutput] = useState<ActionOutput>({ kind: "beside", suffix: "" });
    const run = useActionRun();

    const recipients = keys
        .split(/[\s,]+/)
        .map((k) => k.trim())
        .filter(Boolean);
    const keysBad = lock === "keys" && (recipients.length === 0 || recipients.some((k) => !/^age1[0-9a-z]{58}$/.test(k)));
    const passwordBad = mode === "decrypt" ? !password : lock === "passphrase" && (!password || password !== again);
    const folders = new Set(files.map((f) => parentPath(f.path).toLowerCase()));
    const title = files.length === 1 ? files[0].name : `${formatCount(files.length)} items`;
    const short = mode === "encrypt" && lock === "passphrase" && password.length > 0 && password.length < 10;

    const start = async () => {
        const task = mode === "decrypt" ? { kind: "decrypt", password } : { kind: "encrypt", password: lock === "passphrase" ? password : "", recipients: lock === "keys" ? recipients : [] };
        const done = await run.start(
            files.length,
            (p) => runAction("vault_run", { files: files.map((f) => f.path), task, output }, p),
            (r) => `${mode === "encrypt" ? "encryption" : "decryption"} of ${plural(r.made.length, "item")}`
        );
        // A folder that was encrypted comes back as a .tar.zst: Glint opens it like a folder.
        if (done && mode === "decrypt" && open && done.made.length === 1) {
            close();
            void navigate(done.made[0].toLowerCase().endsWith(".tar.zst") ? done.made[0] : parentPath(done.made[0]), done.made[0].toLowerCase().endsWith(".tar.zst") ? undefined : { select: done.made[0] });
        }
    };

    return (
        <Modal label={mode === "encrypt" ? "Encrypt" : "Decrypt"} onClose={close} onSubmit={() => !passwordBad && !keysBad && !run.started && void start()} className="max-h-[88vh] max-w-xl">
            <ModalHeader icon={mode === "encrypt" ? Lock : LockOpen} title={`${mode === "encrypt" ? "Encrypt" : "Decrypt"} ${title}`} subtitle={folders.size === 1 ? parentPath(files[0].path) : `In ${formatCount(folders.size)} folders`} onClose={close} />
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
                {!run.started && mode === "encrypt" && (
                    <>
                        <p className="text-muted-foreground">
                            Each becomes an .age file (a folder is packed first, as .tar.zst.age), readable on any system with age or rage. Without the passphrase or the key, nobody can open it: Glint neither.
                        </p>
                        <Choices
                            label="Locked with"
                            value={lock}
                            onChange={setLock}
                            options={[
                                { value: "passphrase", label: "A passphrase", hint: "Typed to open it" },
                                { value: "keys", label: "Someone's age key", hint: "Only they can open it" },
                            ]}
                        />
                        {lock === "passphrase" ? (
                            <div className="space-y-2">
                                <label className="flex items-center gap-3">
                                    <span className="w-24 shrink-0 text-xs font-semibold text-muted-foreground">Passphrase</span>
                                    <input type="password" autoFocus value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="new-password" aria-label="Passphrase" className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2" />
                                </label>
                                <label className="flex items-center gap-3">
                                    <span className="w-24 shrink-0 text-xs font-semibold text-muted-foreground">Again</span>
                                    <input type="password" value={again} onChange={(e) => setAgain(e.target.value)} autoComplete="new-password" aria-label="The passphrase again" className={`h-8 min-w-0 flex-1 rounded-md border bg-background px-2 ${again && again !== password ? "border-destructive" : ""}`} />
                                </label>
                                {short && <p className="ps-[6.75rem] text-xs text-amber-700 dark:text-amber-400">A few words are much harder to guess than a short password.</p>}
                            </div>
                        ) : (
                            <label className="block space-y-1">
                                <span className="text-xs font-semibold text-muted-foreground">Their age public keys, one a line (age1…)</span>
                                <textarea value={keys} onChange={(e) => setKeys(e.target.value)} rows={3} spellCheck={false} aria-label="Age keys" className={`w-full rounded-md border bg-background px-2 py-1 font-mono text-xs ${keys && keysBad ? "border-destructive" : ""}`} />
                            </label>
                        )}
                        <WhereChooser value={output} onChange={setOutput} noun="item" note="In their place, the originals go to the Recycle Bin (empty it to be rid of them), and Ctrl+Z takes it back." />
                    </>
                )}
                {!run.started && mode === "decrypt" && (
                    <>
                        <label className="flex items-center gap-3">
                            <span className="w-24 shrink-0 text-xs font-semibold text-muted-foreground">Passphrase</span>
                            <input type="password" autoFocus value={password} onChange={(e) => setPassword(e.target.value)} aria-label="Passphrase" className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2" />
                        </label>
                        {files.length === 1 && (
                            <label className="flex items-center gap-2 ps-[6.75rem]">
                                <input type="checkbox" checked={open} onChange={(e) => setOpen(e.target.checked)} className="accent-primary" /> Show it once it's decrypted
                            </label>
                        )}
                        <WhereChooser value={output} onChange={setOutput} noun="file" replace />
                    </>
                )}
                {run.running && run.progress && <RunProgress progress={run.progress} verb={mode === "encrypt" ? "Encrypting" : "Decrypting"} />}
                <RunResult report={run.report} error={run.error} failedWhat={mode === "encrypt" ? "Couldn't encrypt" : "Couldn't decrypt"} sizes={false} />
            </div>
            <RunFooter
                run={run}
                onClose={close}
                onStart={() => void start()}
                onStop={() => void invoke("vault_cancel")}
                startLabel={mode === "encrypt" ? "Encrypt" : "Decrypt"}
                startIcon={mode === "encrypt" ? Lock : LockOpen}
                disabled={passwordBad || keysBad || badOutput(output)}
            />
        </Modal>
    );
}
