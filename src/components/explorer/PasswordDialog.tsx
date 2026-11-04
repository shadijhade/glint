import { useState } from "react";
import { KeyRound, Loader2 } from "lucide-react";
import { usePasswordAsk, useUnlocks } from "@/lib/archives";
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
        <form onSubmit={(e) => void submit(e)} className="flex w-full max-w-72 flex-col gap-1.5 text-left">
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
