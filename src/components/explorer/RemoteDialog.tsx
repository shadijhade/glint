import { useEffect, useState } from "react";
import { create } from "zustand";
import { toast } from "sonner";
import { confirm } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, CheckCircle2, Cloud, Loader2, PlugZap, Server, Trash2 } from "lucide-react";
import { leaveRemote, navigate } from "@/lib/explorer";
import { remoteId, remotePath, remoteSecretDelete, remoteSecretSaved, remoteSecretSet, remoteTest } from "@/lib/remotes";
import { type RemoteConnection, useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";
import { Modal, ModalHeader } from "../Modal";

const useRemoteDialog = create<{ open: boolean; editing: RemoteConnection | null }>(() => ({ open: false, editing: null }));

/** Add a remote connection (the sidebar's Remote section, Ctrl+K). */
export const addRemote = () => useRemoteDialog.setState({ open: true, editing: null });
export const editRemote = (remote: RemoteConnection) => useRemoteDialog.setState({ open: true, editing: remote });
const close = () => useRemoteDialog.setState({ open: false, editing: null });

/** Removes a connection (asking first): its secret leaves Credential Manager, its tabs go to This PC. Nothing on the remote changes. */
export async function removeRemote(remote: RemoteConnection) {
    const sure = await confirm(`Remove the connection “${remote.name}”? Its secret goes from Credential Manager; nothing on it changes.`, {
        title: "Remove the connection",
        kind: "warning",
        okLabel: "Remove",
    });
    if (!sure) return false;
    await remoteSecretDelete(remote.id).catch(() => {});
    useStore.getState().removeRemote(remote.id);
    leaveRemote(remote.id);
    return true;
}

export function RemoteDialog() {
    const { open, editing } = useRemoteDialog();
    if (!open) return null;
    return <RemoteForm key={editing?.id ?? "new"} editing={editing} />;
}

const BLANK: RemoteConnection = { id: "", name: "", kind: "s3", endpoint: "", region: "", bucket: "", user: "", root: "" };

const KINDS = [
    { kind: "s3" as const, label: "S3-compatible storage", hint: "Amazon S3, Cloudflare R2, Backblaze B2, MinIO…", icon: Cloud },
    { kind: "webdav" as const, label: "WebDAV", hint: "Nextcloud, a NAS, dufs…", icon: Server },
];

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

const INPUT = "h-8 w-full rounded-md border bg-background px-2 text-sm outline-hidden focus:border-primary";

function RemoteForm({ editing }: { editing: RemoteConnection | null }) {
    const [form, setForm] = useState<RemoteConnection>(editing ?? BLANK);
    const [secret, setSecret] = useState("");
    const [kept, setKept] = useState(false);
    const [test, setTest] = useState<{ running: boolean; ok?: string; error?: string } | null>(null);
    useEffect(() => {
        if (editing) remoteSecretSaved(editing.id).then(setKept, () => {});
    }, [editing]);
    const change = (patch: Partial<RemoteConnection>) => {
        setForm((f) => ({ ...f, ...patch }));
        setTest(null);
    };
    const s3 = form.kind === "s3";
    const ready = !!form.name.trim() && (s3 ? !!form.bucket.trim() : !!form.endpoint.trim());
    const connection = (): RemoteConnection => ({
        ...form,
        id: editing?.id ?? remoteId(form.name),
        name: form.name.trim(),
        endpoint: form.endpoint.trim(),
        region: form.region.trim(),
        bucket: form.bucket.trim(),
        user: form.user.trim(),
        root: form.root.trim(),
    });
    // What's typed, or (editing, nothing typed) the secret kept.
    const typed = () => (secret ? secret : editing && kept ? null : "");

    const runTest = async () => {
        setTest({ running: true });
        try {
            setTest({ running: false, ok: await remoteTest(connection(), typed()) });
        } catch (e) {
            setTest({ running: false, error: String(e) });
        }
    };

    const save = async () => {
        if (!ready) return;
        const made = connection();
        try {
            if (secret) await remoteSecretSet(made.id, secret);
        } catch (e) {
            return void toast.error("Couldn't keep the secret in Credential Manager", { description: String(e) });
        }
        useStore.getState().saveRemote(made);
        close();
        if (!editing) void navigate(remotePath(made));
    };

    const title = editing ? `Edit “${editing.name}”` : "Add a remote connection";
    return (
        <Modal label={title} onClose={close} onSubmit={() => void save()} className="max-h-[88vh] max-w-xl">
            <ModalHeader icon={PlugZap} title={title} subtitle="Its folders open read-only; what you open or copy is downloaded" onClose={close} />
            <div className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4 text-sm">
                {!editing && (
                    <div role="radiogroup" aria-label="Kind" className="grid grid-cols-2 gap-1.5">
                        {KINDS.map(({ kind, label, hint, icon: Icon }) => (
                            <button
                                key={kind}
                                type="button"
                                role="radio"
                                aria-checked={form.kind === kind}
                                onClick={() => change({ kind })}
                                className={cn("flex items-start gap-2 rounded-md border px-2 py-1.5 text-left transition-colors hover:bg-accent", form.kind === kind && "border-primary bg-primary/10 hover:bg-primary/15")}
                            >
                                <Icon className="mt-0.5 size-4 shrink-0 text-primary" />
                                <span className="min-w-0">
                                    <span className="block font-medium">{label}</span>
                                    <span className="block truncate text-[0.6875rem] text-muted-foreground">{hint}</span>
                                </span>
                            </button>
                        ))}
                    </div>
                )}
                <Field label="Name">
                    <input autoFocus value={form.name} onChange={(e) => change({ name: e.target.value })} placeholder={s3 ? "Photos backup" : "My NAS"} aria-label="Name" className={INPUT} />
                </Field>
                {s3 ? (
                    <>
                        <Field label="Address" hint="Empty for Amazon S3. R2: https://<account>.r2.cloudflarestorage.com · B2: https://s3.<region>.backblazeb2.com">
                            <input value={form.endpoint} onChange={(e) => change({ endpoint: e.target.value })} placeholder="Amazon S3" spellCheck={false} aria-label="Address" className={INPUT} />
                        </Field>
                        <Field label="Region" hint="R2 takes “auto”.">
                            <input value={form.region} onChange={(e) => change({ region: e.target.value })} placeholder="us-east-1" spellCheck={false} aria-label="Region" className={INPUT} />
                        </Field>
                        <Field label="Bucket">
                            <input value={form.bucket} onChange={(e) => change({ bucket: e.target.value })} spellCheck={false} aria-label="Bucket" className={INPUT} />
                        </Field>
                        <Field label="Access key ID" hint="Empty for a public bucket.">
                            <input value={form.user} onChange={(e) => change({ user: e.target.value })} spellCheck={false} autoComplete="off" aria-label="Access key ID" className={INPUT} />
                        </Field>
                        <Field label="Secret access key">
                            <input
                                type="password"
                                value={secret}
                                onChange={(e) => {
                                    setSecret(e.target.value);
                                    setTest(null);
                                }}
                                placeholder={kept ? "Kept in Credential Manager: type to change it" : ""}
                                autoComplete="off"
                                aria-label="Secret access key"
                                className={INPUT}
                            />
                        </Field>
                        <Field label="Folder in it" hint="Where it opens; empty for the bucket's top.">
                            <input value={form.root} onChange={(e) => change({ root: e.target.value })} placeholder="photos/2024" spellCheck={false} aria-label="Folder in it" className={INPUT} />
                        </Field>
                    </>
                ) : (
                    <>
                        <Field label="Address">
                            <input
                                value={form.endpoint}
                                onChange={(e) => change({ endpoint: e.target.value })}
                                placeholder="https://cloud.example.com/remote.php/dav/files/you/"
                                spellCheck={false}
                                aria-label="Address"
                                className={INPUT}
                            />
                        </Field>
                        <Field label="User name" hint="Empty for a server that doesn't ask.">
                            <input value={form.user} onChange={(e) => change({ user: e.target.value })} spellCheck={false} autoComplete="off" aria-label="User name" className={INPUT} />
                        </Field>
                        <Field label="Password">
                            <input
                                type="password"
                                value={secret}
                                onChange={(e) => {
                                    setSecret(e.target.value);
                                    setTest(null);
                                }}
                                placeholder={kept ? "Kept in Credential Manager: type to change it" : ""}
                                autoComplete="off"
                                aria-label="Password"
                                className={INPUT}
                            />
                        </Field>
                        <Field label="Folder on it" hint="Where it opens; empty for the address's own.">
                            <input value={form.root} onChange={(e) => change({ root: e.target.value })} placeholder="Photos" spellCheck={false} aria-label="Folder on it" className={INPUT} />
                        </Field>
                    </>
                )}
                <label className="flex items-start gap-2 pl-[7.75rem]">
                    <input type="checkbox" checked={!!form.thumbnails} onChange={(e) => change({ thumbnails: e.target.checked })} className="mt-0.5 accent-primary" />
                    <span>
                        Thumbnails of pictures in the grid
                        <span className="block text-[0.6875rem] text-muted-foreground">Downloads each picture shown (up to 16 MB){s3 ? ", which a paid bucket charges for" : ""}.</span>
                    </span>
                </label>
                {test && (
                    <p className={cn("flex items-start gap-2 text-xs", test.error ? "text-destructive" : "text-muted-foreground")} role="status">
                        {test.running ? (
                            <>
                                <Loader2 className="mt-0.5 size-3.5 shrink-0 animate-spin text-primary" /> Connecting…
                            </>
                        ) : test.error ? (
                            <>
                                <AlertTriangle className="mt-0.5 size-3.5 shrink-0" /> {test.error}
                            </>
                        ) : (
                            <>
                                <CheckCircle2 className="mt-0.5 size-3.5 shrink-0 text-emerald-500" /> {test.ok}
                            </>
                        )}
                    </p>
                )}
                <p className="text-xs text-muted-foreground">Glint only reads it: nothing there is changed. {s3 ? "The secret access key" : "The password"} stays in Windows Credential Manager, never in Glint's settings.</p>
            </div>
            <div className="flex shrink-0 items-center gap-2 border-t px-4 py-3">
                <Button variant="outline" size="sm" disabled={!ready || !!test?.running} onClick={() => void runTest()}>
                    <PlugZap /> Test
                </Button>
                {editing && (
                    <Button
                        variant="ghost"
                        size="sm"
                        className="text-destructive hover:text-destructive"
                        onClick={() =>
                            void removeRemote(editing).then((gone) => {
                                if (gone) close();
                            })
                        }
                    >
                        <Trash2 /> Remove
                    </Button>
                )}
                <div className="flex-1" />
                <Button variant="ghost" size="sm" onClick={close}>
                    Cancel
                </Button>
                <Button size="sm" disabled={!ready} onClick={() => void save()}>
                    {editing ? "Save" : "Add and open"}
                </Button>
            </div>
        </Modal>
    );
}
