import { useState } from "react";
import { KeyRound, Loader2 } from "lucide-react";
import { useCompressAsk, usePasswordAsk, useUnlocks } from "@/lib/archives";
import { reload } from "@/lib/explorer";
import { archiveUnlock } from "@/lib/fs";
import { baseName } from "@/lib/paths";
import { Modal, ModalHeader } from "../Modal";
import { Button } from "../ui/button";

/**
 * The password for what `path` leads to in an archive (a file in it, or an archive whose list of files
 * is encrypted), tried as it's given: kept in memory, until Glint closes, once it opens it. By default
 * the folder shown is read again after, without the locks.
 */
export function PasswordForm({ path, autoFocus, onUnlocked = () => void reload() }: { path: string; autoFocus?: boolean; onUnlocked?: () => void }) {
    const [password, setPassword] = useState("");
    const [error, setError] = useState("");
    const [busy, setBusy] = useState(false);
    const submit = async (e: React.FormEvent) => {
        e.preventDefault();
        if (!password || busy) return;
        setBusy(true);
        setError("");
        try {
            await archiveUnlock(path, password);
            useUnlocks.setState((s) => ({ count: s.count + 1 }));
            onUnlocked();
        } catch (err) {
            setError(String(err));
            setBusy(false);
        }
    };
    return (
        <form onSubmit={(e) => void submit(e)} className="flex w-full max-w-72 flex-col gap-1.5 text-start">
            <div className="flex gap-1.5">
                <input
                    type="password"
                    autoFocus={autoFocus}
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    placeholder="Password"
                    aria-label="Password"
                    className="fluent-field h-8 min-w-0 flex-1 px-2.5 text-sm outline-hidden placeholder:text-muted-foreground"
                />
                <Button type="submit" size="sm" className="h-8 gap-1.5" disabled={!password || busy}>
                    {busy ? <Loader2 className="size-3.5 animate-spin" /> : <KeyRound className="size-3.5" />} Open
                </Button>
            </div>
            {error && (
                <p role="alert" className="text-xs text-destructive">
                    {error}
                </p>
            )}
        </form>
    );
}

/**
 * Compress with a password: AES-256 in a zip (its file names show) or a 7z (its file names are encrypted
 * too). The password is asked twice, and never kept.
 */
export function CompressPasswordDialog() {
    const { run, format: asked, what } = useCompressAsk();
    const [format, setFormat] = useState<"zip" | "7z">(asked);
    const [password, setPassword] = useState("");
    const [again, setAgain] = useState("");
    if (!run) return null;
    const close = () => {
        useCompressAsk.setState({ run: null });
        setPassword("");
        setAgain("");
    };
    const mismatch = again.length > 0 && again !== password;
    const submit = (e: React.FormEvent) => {
        e.preventDefault();
        if (!password || password !== again) return;
        run(format, password);
        close();
    };
    return (
        <Modal label={`Compress ${what} with a password`} onClose={close} className="max-w-sm">
            <ModalHeader icon={KeyRound} title={`Compress ${what} with a password`} subtitle="AES-256. Glint doesn't keep the password: without it, nothing in the archive can be read." onClose={close} />
            <form onSubmit={submit} className="flex flex-col gap-3 p-4 text-sm">
                <div className="flex gap-1.5" role="radiogroup" aria-label="Format">
                    {(["7z", "zip"] as const).map((f) => (
                        <Button key={f} type="button" size="sm" variant={format === f ? "default" : "outline"} aria-checked={format === f} role="radio" onClick={() => setFormat(f)}>
                            {f === "7z" ? "7z (names hidden too)" : "zip (names show)"}
                        </Button>
                    ))}
                </div>
                <input type="password" autoFocus value={password} onChange={(e) => setPassword(e.target.value)} placeholder="Password" aria-label="Password" className="fluent-field h-8 px-2.5 outline-hidden" />
                <input type="password" value={again} onChange={(e) => setAgain(e.target.value)} placeholder="The password again" aria-label="The password again" className="fluent-field h-8 px-2.5 outline-hidden" />
                {mismatch && (
                    <p role="alert" className="text-xs text-destructive">
                        The two aren't the same.
                    </p>
                )}
                <div className="flex justify-end gap-2">
                    <Button type="button" variant="ghost" size="sm" onClick={close}>
                        Cancel
                    </Button>
                    <Button type="submit" size="sm" disabled={!password || password !== again}>
                        Compress
                    </Button>
                </div>
            </form>
        </Modal>
    );
}

/** Asks for an archive's password when opening it failed for want of one (its list of files is encrypted). */
export function PasswordDialog() {
    const { path, retry } = usePasswordAsk();
    if (!path) return null;
    const close = () => usePasswordAsk.setState({ path: null, retry: undefined });
    return (
        <Modal label={`The password for “${baseName(path)}”`} onClose={close} className="max-w-sm">
            <ModalHeader icon={KeyRound} title={`“${baseName(path)}” is password-protected`} subtitle="Its password stays in memory until Glint closes." onClose={close} />
            <div className="p-4">
                <PasswordForm
                    path={path}
                    autoFocus
                    onUnlocked={() => {
                        close();
                        retry?.();
                    }}
                />
            </div>
        </Modal>
    );
}
