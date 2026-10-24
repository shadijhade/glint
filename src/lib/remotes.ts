import { invoke } from "@tauri-apps/api/core";
import { REMOTE, remoteIdOf } from "./paths";
import { type RemoteConnection, useStore } from "./store";

/** Tells Rust the connections (at start, and whenever they change); resolves once it has them. */
export function initRemotes() {
    const push = (connections: RemoteConnection[]) => invoke("remote_configure", { connections }).catch(() => {});
    useStore.subscribe((s, before) => {
        if (s.remotes !== before.remotes) void push(s.remotes);
    });
    return push(useStore.getState().remotes);
}

/** A connection's folder at its top. */
export const remotePath = (remote: RemoteConnection) => REMOTE + remote.id;

export const remoteOfPath = (path: string) => {
    const id = remoteIdOf(path).toLowerCase();
    return id ? useStore.getState().remotes.find((r) => r.id === id) : undefined;
};

/** What a remote path's top is called: its connection's name (or its id, for one that's gone). */
export const remoteName = (path: string) => remoteOfPath(path)?.name ?? remoteIdOf(path);

/** A new connection's id, from its name: lower-case letters and digits, unlike the others'. */
export function remoteId(name: string) {
    const base = name.toLowerCase().normalize("NFKD").replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 32) || "remote";
    const taken = new Set(useStore.getState().remotes.map((r) => r.id));
    let id = base;
    for (let n = 2; taken.has(id); n++) id = `${base}-${n}`;
    return id;
}

/** Where a connection goes, in a few words (the sidebar's tooltip). */
export function remoteWhere(r: RemoteConnection) {
    const folder = r.root.trim() ? `, in ${r.root.trim()}` : "";
    return r.kind === "s3" ? `S3 bucket “${r.bucket}” at ${r.endpoint.trim() || `Amazon S3 (${r.region.trim() || "us-east-1"})`}${folder}` : `WebDAV at ${r.endpoint}${folder}`;
}

export const remoteSecretSet = (id: string, secret: string) => invoke("remote_secret_set", { id, secret });
export const remoteSecretDelete = (id: string) => invoke("remote_secret_delete", { id });
export const remoteSecretSaved = (id: string) => invoke<boolean>("remote_secret_saved", { id });
/** Tries a connection as it's typed, with `secret` (or the one kept, when it's null): what's at its top, or why it didn't connect. */
export const remoteTest = (connection: RemoteConnection, secret: string | null) => invoke<string>("remote_test", { connection, secret });
