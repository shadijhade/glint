import { useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { AlertTriangle, ArrowLeft, CheckCircle2, ChevronRight, Cloud, ExternalLink, Globe, KeyRound, Loader2, LockOpen, Search, Server } from "lucide-react";
import {
    forProvider,
    helpOf,
    LOCKED,
    offered,
    onSignInPage,
    POPULAR,
    rcloneAuthorize,
    rcloneAuthorizeCancel,
    rcloneConfigAnswer,
    rcloneConfigStart,
    rcloneDelete,
    type RcloneProvider,
    rcloneProviders,
    type RcloneQuestion,
    type RcloneRemote,
    rcloneRemotes,
    type RcloneStatus,
    rcloneStatus,
    rcloneUnlock,
    remoteNameFor,
    serviceName,
} from "@/lib/rclone";
import { remoteId, remoteTest } from "@/lib/remotes";
import { refreshTools } from "@/lib/tools";
import { type RemoteConnection, useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { ToolNeeded } from "../ToolNeeded";
import { Button } from "../ui/button";

const INPUT = "h-8 w-full rounded-md border bg-background px-2 text-sm outline-hidden focus:border-primary";

/** Services reached over a network protocol rather than a company's cloud: a server's icon. */
const SERVERS = new Set(["sftp", "ftp", "smb", "webdav", "http", "hdfs", "seafile"]);
export const serviceIcon = (kind: string) => (SERVERS.has(kind) ? Server : Cloud);

/** A connection to one of rclone's remotes, named `name` in Glint. */
export const connectionFor = (name: string, remote: RcloneRemote | { name: string; kind: string }): RemoteConnection => ({
    id: remoteId(name),
    name,
    kind: "rclone",
    rclone: remote.name,
    service: remote.kind,
    endpoint: "",
    region: "",
    bucket: "",
    user: "",
    root: "",
});

/** Text with its web addresses as links (rclone's help points at its guides). */
function Linked({ text }: { text: string }) {
    const parts = text.split(/(https?:\/\/[^\s)]+)/g);
    return (
        <>
            {parts.map((part, i) =>
                i % 2 ? (
                    <button key={i} type="button" onClick={() => void openUrl(part)} className="text-primary underline-offset-2 hover:underline">
                        {part}
                    </button>
                ) : (
                    part
                )
            )}
        </>
    );
}

/** One of rclone's questions (or a service's setting), as the field that answers it. */
function QuestionField({ question, value, onChange, provider, autoFocus }: { question: RcloneQuestion; value: string; onChange: (value: string) => void; provider?: string; autoFocus?: boolean }) {
    const { label, more } = helpOf(question);
    const examples = question.examples.filter((e) => !provider || forProvider(e.provider, provider));
    const id = `rclone-${question.name}`;
    let field: React.ReactNode;
    if (question.kind === "bool") {
        const yes = value === "true";
        field = (
            <div role="radiogroup" aria-labelledby={`${id}-label`} className="flex gap-1.5">
                {[
                    ["true", examples.find((e) => e.value === "true")?.help || "Yes"],
                    ["false", examples.find((e) => e.value === "false")?.help || "No"],
                ].map(([answer, text]) => (
                    <button
                        key={answer}
                        type="button"
                        role="radio"
                        aria-checked={(answer === "true") === yes}
                        onClick={() => onChange(answer)}
                        className={cn("rounded-md border px-3 py-1 text-sm transition-colors hover:bg-accent", (answer === "true") === yes && "border-primary bg-primary/10")}
                    >
                        {text.split("\n")[0]}
                    </button>
                ))}
            </div>
        );
    } else if (examples.length > 0 && question.exclusive) {
        field = (
            <select id={id} autoFocus={autoFocus} value={value} onChange={(e) => onChange(e.target.value)} className={INPUT}>
                {!examples.some((e) => e.value === value) && <option value={value}>{value || "Choose…"}</option>}
                {examples.map((e) => (
                    <option key={e.value} value={e.value}>
                        {e.help ? `${e.help.split("\n")[0]}${e.help.split("\n")[0] === e.value ? "" : ` (${e.value})`}` : e.value}
                    </option>
                ))}
            </select>
        );
    } else {
        field = (
            <>
                <input
                    id={id}
                    autoFocus={autoFocus}
                    type={question.password ? "password" : "text"}
                    value={value}
                    onChange={(e) => onChange(e.target.value)}
                    placeholder={question.default && !question.password ? question.default : question.required ? "Needed" : "Optional"}
                    list={examples.length ? `${id}-examples` : undefined}
                    spellCheck={false}
                    autoComplete="off"
                    className={INPUT}
                />
                {examples.length > 0 && (
                    <datalist id={`${id}-examples`}>
                        {examples.map((e) => (
                            <option key={e.value} value={e.value}>
                                {e.help.split("\n")[0]}
                            </option>
                        ))}
                    </datalist>
                )}
            </>
        );
    }
    return (
        <div className="space-y-1">
            <label htmlFor={id} id={`${id}-label`} className="block text-xs font-semibold text-muted-foreground">
                <Linked text={label} />
                {question.required && <span className="text-destructive"> *</span>}
            </label>
            {field}
            {more && (
                <p className="max-h-28 overflow-y-auto whitespace-pre-line text-[0.6875rem] text-muted-foreground">
                    <Linked text={more} />
                </p>
            )}
        </div>
    );
}

type Phase =
    | { at: "checking" }
    | { at: "install" }
    | { at: "unlock"; error?: string }
    | { at: "choose" }
    | { at: "settings"; provider: RcloneProvider }
    | { at: "asking"; provider: RcloneProvider; step: { state: string; question: RcloneQuestion; error: string } }
    | { at: "browser"; provider: RcloneProvider; url: string | null }
    | { at: "working"; provider: RcloneProvider | null; what: string }
    | { at: "failed"; provider: RcloneProvider | null; error: string }
    | { at: "ready"; connection: RemoteConnection; said: string };

/**
 * Connecting a cloud through rclone, in the connection dialog: rclone first (installed, unlocked), then a
 * service (or a remote rclone has already), its first settings, and rclone's own questions one at a
 * time, a browser sign-in where the service has one, until Glint can open it.
 */
export function CloudSetup({ onDone, onCancel }: { onDone: (connection: RemoteConnection) => void; onCancel: () => void }) {
    const [phase, setPhase] = useState<Phase>({ at: "checking" });
    const [status, setStatus] = useState<RcloneStatus | null>(null);
    const [providers, setProviders] = useState<RcloneProvider[]>([]);
    const [remotes, setRemotes] = useState<RcloneRemote[]>([]);
    const [query, setQuery] = useState("");
    const [name, setName] = useState("");
    const [values, setValues] = useState<Record<string, string>>({});
    const [answer, setAnswer] = useState("");
    const [password, setPassword] = useState("");
    const [own, setOwn] = useState(false);
    const connections = useStore((s) => s.remotes);
    // The remote being made in rclone's configuration: taken out again if this stops before it works.
    const making = useRef<string | null>(null);
    const done = useRef(false);

    const check = async () => {
        setPhase({ at: "checking" });
        try {
            const found = await rcloneStatus();
            setStatus(found);
            if (!found.installed) return setPhase({ at: "install" });
            if (found.locked) return setPhase({ at: "unlock" });
            const [all, have] = await Promise.all([rcloneProviders(), rcloneRemotes()]);
            setProviders(all);
            setRemotes(have);
            setPhase({ at: "choose" });
        } catch (e) {
            setPhase(String(e).startsWith(LOCKED) ? { at: "unlock" } : { at: "failed", provider: null, error: String(e) });
        }
    };

    useEffect(() => {
        void check();
        // Its page, once rclone has it ready: opened in the browser.
        const unlisten = onSignInPage((url) => {
            setPhase((p) => (p.at === "browser" ? { ...p, url } : p));
            void openUrl(url).catch(() => {});
        });
        return () => {
            void unlisten.then((stop) => stop());
            void rcloneAuthorizeCancel();
            if (making.current && !done.current) void rcloneDelete(making.current).catch(() => {});
        };
    }, []);

    // Back to the tools' list once rclone is installed from here.
    const installed = async () => {
        await refreshTools();
        await check();
    };

    const unlock = async () => {
        setPhase({ at: "working", provider: null, what: "Unlocking rclone's configuration…" });
        try {
            await rcloneUnlock(password);
            setPassword("");
            await check();
        } catch (e) {
            setPhase({ at: "unlock", error: String(e) });
        }
    };

    const choose = (provider: RcloneProvider) => {
        const service = serviceName(provider.name, providers);
        const taken = new Set(connections.map((c) => c.name.toLowerCase()));
        let shown = service;
        for (let n = 2; taken.has(shown.toLowerCase()); n++) shown = `${service} ${n}`;
        setName(shown);
        setValues(Object.fromEntries(provider.options.filter((o) => o.default && o.kind !== "bool").map((o) => [o.name, ""])));
        setOwn(false);
        setPhase({ at: "settings", provider });
    };

    /** Its settings to give rclone first: those filled in, for the service chosen (S3's provider). */
    const shownOptions = (provider: RcloneProvider) => {
        const s3 = values.provider ?? "";
        return provider.options.filter(
            (o) =>
                o.name !== "token" &&
                !o.name.startsWith("config_") &&
                (!provider.oauth || own || (o.name !== "client_id" && o.name !== "client_secret")) &&
                (o.name === "provider" || !o.provider || forProvider(o.provider, s3))
        );
    };

    const missing = (provider: RcloneProvider) => shownOptions(provider).some((o) => o.required && !(values[o.name] ?? "").trim());

    /** Where rclone's steps lead: its next question, a browser sign-in, or done (then a test). */
    const follow = async (provider: RcloneProvider, remote: string, step: { state: string; question: RcloneQuestion | null; error: string }): Promise<void> => {
        const question = step.question;
        if (!question) {
            if (step.state) return setPhase({ at: "failed", provider, error: "rclone stopped before it was set up" });
            return test(provider, remote);
        }
        // A browser on this PC: Glint opens the page (rclone authorize), and the sign-in is the answer.
        if (question.name === "config_is_local") return follow(provider, remote, await rcloneConfigAnswer(remote, step.state, "false"));
        if (question.name === "config_token") {
            setPhase({ at: "browser", provider, url: null });
            let token: string;
            try {
                token = await rcloneAuthorize(question.help);
            } catch (e) {
                if (String(e) === "cancelled") return;
                return setPhase({ at: "failed", provider, error: String(e) });
            }
            setPhase({ at: "working", provider, what: `Signed in. Finishing ${serviceName(provider.name, providers)}…` });
            return follow(provider, remote, await rcloneConfigAnswer(remote, step.state, token));
        }
        setAnswer(question.default);
        setPhase({ at: "asking", provider, step: { state: step.state, question, error: step.error } });
    };

    const start = async (provider: RcloneProvider) => {
        const remote = remoteNameFor(name, remotes.map((r) => r.name));
        making.current = remote;
        const parameters = Object.fromEntries(
            shownOptions(provider)
                .map((o) => [o.name, (values[o.name] ?? "").trim()] as const)
                .filter(([, v]) => v !== "")
        );
        setPhase({ at: "working", provider, what: `Setting up ${serviceName(provider.name, providers)}…` });
        try {
            await follow(provider, remote, await rcloneConfigStart(remote, provider.name, parameters));
        } catch (e) {
            setPhase({ at: "failed", provider, error: String(e) });
        }
    };

    const reply = async (provider: RcloneProvider, state: string) => {
        const remote = making.current;
        if (!remote) return;
        setPhase({ at: "working", provider, what: "Asking rclone…" });
        try {
            await follow(provider, remote, await rcloneConfigAnswer(remote, state, answer));
        } catch (e) {
            setPhase({ at: "failed", provider, error: String(e) });
        }
    };

    /** Tries the remote as Glint will open it: what's at its top. */
    const test = async (provider: RcloneProvider | null, remote: string, shown = name) => {
        const connection = connectionFor(shown.trim() || remote, { name: remote, kind: provider?.name ?? remotes.find((r) => r.name === remote)?.kind ?? "" });
        setPhase({ at: "working", provider, what: "Looking inside…" });
        try {
            setPhase({ at: "ready", connection, said: await remoteTest(connection, null) });
        } catch (e) {
            setPhase({ at: "failed", provider, error: String(e) });
        }
    };

    /** Back to the services, the half-made remote taken out of rclone's configuration. */
    const startOver = async () => {
        void rcloneAuthorizeCancel();
        if (making.current) await rcloneDelete(making.current).catch(() => {});
        making.current = null;
        await check();
    };

    const finish = (connection: RemoteConnection) => {
        done.current = true;
        onDone(connection);
    };

    const known = new Set(connections.filter((c) => c.kind === "rclone").map((c) => c.rclone?.toLowerCase()));
    const unused = remotes.filter((r) => !known.has(r.name.toLowerCase()));
    const listed = useMemo(() => {
        const words = query.trim().toLowerCase();
        const all = providers.filter(offered).filter((p) => !words || `${serviceName(p.name, providers)} ${p.description} ${p.name}`.toLowerCase().includes(words));
        const rank = (p: RcloneProvider) => (POPULAR.includes(p.name) ? POPULAR.indexOf(p.name) : POPULAR.length);
        return all.sort((a, b) => rank(a) - rank(b) || serviceName(a.name, providers).localeCompare(serviceName(b.name, providers)));
    }, [providers, query]);

    const body = (() => {
        switch (phase.at) {
            case "checking":
                return <Working what="Looking for rclone…" />;
            case "install":
                return (
                    <div className="space-y-3">
                        <p>
                            Glint reaches Google Drive, OneDrive, Dropbox, Box, MEGA, S3 and 60 more clouds through <strong>rclone</strong>, a free program that's been doing it for years. It's
                            not on this PC yet.
                        </p>
                        <ToolNeeded id="rclone" icon={Cloud} why="rclone signs in to your clouds and reads them for Glint." />
                        <Button variant="outline" size="sm" onClick={() => void installed()}>
                            I've installed it
                        </Button>
                    </div>
                );
            case "unlock":
                return (
                    <form
                        className="space-y-3"
                        onSubmit={(e) => {
                            e.preventDefault();
                            void unlock();
                        }}
                    >
                        <p>
                            Your rclone configuration is encrypted. Its password unlocks it until Glint closes{status?.config ? ` (${status.config})` : ""}.
                        </p>
                        <input type="password" autoFocus value={password} onChange={(e) => setPassword(e.target.value)} aria-label="rclone's configuration password" className={INPUT} />
                        {phase.error && <Failure error={phase.error} />}
                        <Button size="sm" type="submit" disabled={!password}>
                            <LockOpen /> Unlock
                        </Button>
                    </form>
                );
            case "choose":
                return (
                    <div className="space-y-4">
                        {unused.length > 0 && (
                            <section className="space-y-1.5">
                                <h3 className="text-xs font-semibold text-muted-foreground">Already in rclone{status?.version ? ` ${status.version}` : ""}</h3>
                                <div className="grid grid-cols-2 gap-1.5">
                                    {unused.map((r) => {
                                        const Icon = serviceIcon(r.kind);
                                        return (
                                            <button
                                                key={r.name}
                                                type="button"
                                                onClick={() => {
                                                    setName(r.name);
                                                    void test(null, r.name, r.name);
                                                }}
                                                className="flex items-center gap-2 rounded-md border px-2 py-1.5 text-left transition-colors hover:bg-accent"
                                            >
                                                <Icon className="size-4 shrink-0 text-primary" />
                                                <span className="min-w-0">
                                                    <span className="block truncate font-medium">{r.name}</span>
                                                    <span className="block truncate text-[0.6875rem] text-muted-foreground">{serviceName(r.kind, providers)}</span>
                                                </span>
                                            </button>
                                        );
                                    })}
                                </div>
                            </section>
                        )}
                        <section className="space-y-1.5">
                            <h3 className="text-xs font-semibold text-muted-foreground">{unused.length ? "Or connect another" : "Which cloud?"}</h3>
                            <div className="relative">
                                <Search className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                                <input autoFocus value={query} onChange={(e) => setQuery(e.target.value)} placeholder={`Search ${listed.length} services`} aria-label="Search services" className={cn(INPUT, "pl-7")} />
                            </div>
                            <div className="grid max-h-72 grid-cols-2 gap-1 overflow-y-auto">
                                {listed.map((p) => {
                                    const Icon = serviceIcon(p.name);
                                    const shown = serviceName(p.name, providers);
                                    return (
                                        <button
                                            key={p.name}
                                            type="button"
                                            onClick={() => choose(p)}
                                            title={p.description}
                                            className="flex items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-accent"
                                        >
                                            <Icon className="size-4 shrink-0 text-muted-foreground" />
                                            <span className="min-w-0 flex-1">
                                                <span className="block truncate">{shown}</span>
                                                {!p.description.toLowerCase().includes(shown.toLowerCase()) && <span className="block truncate text-[0.6875rem] text-muted-foreground">{p.description}</span>}
                                            </span>
                                            {p.oauth && <Globe className="size-3.5 shrink-0 text-muted-foreground" aria-label="Signed in to in your browser" />}
                                        </button>
                                    );
                                })}
                            </div>
                        </section>
                    </div>
                );
            case "settings": {
                const provider = phase.provider;
                const options = shownOptions(provider);
                const hasOwn = provider.oauth && provider.options.some((o) => o.name === "client_id");
                return (
                    <form
                        className="space-y-3"
                        onSubmit={(e) => {
                            e.preventDefault();
                            if (name.trim() && !missing(provider)) void start(provider);
                        }}
                    >
                        <div className="space-y-1">
                            <label htmlFor="cloud-name" className="block text-xs font-semibold text-muted-foreground">
                                Its name in Glint
                            </label>
                            <input id="cloud-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} className={INPUT} />
                        </div>
                        {options.map((o) => (
                            <QuestionField
                                key={o.name}
                                question={o}
                                provider={o.name === "provider" ? undefined : values.provider}
                                value={values[o.name] ?? (o.kind === "bool" ? o.default : "")}
                                onChange={(v) => setValues((all) => ({ ...all, [o.name]: v }))}
                            />
                        ))}
                        {hasOwn && (
                            <label className="flex items-start gap-2 text-sm">
                                <input type="checkbox" checked={own} onChange={(e) => setOwn(e.target.checked)} className="mt-0.5 accent-primary" />
                                <span>
                                    My own app (client ID)
                                    <span className="block text-[0.6875rem] text-muted-foreground">
                                        Optional: rclone signs in as its own app otherwise.{provider.name === "drive" ? " Google is retiring rclone's shared one for Drive, so yours keeps working." : ""}
                                    </span>
                                </span>
                            </label>
                        )}
                        {provider.oauth && (
                            <p className="flex items-start gap-2 text-xs text-muted-foreground">
                                <Globe className="mt-0.5 size-3.5 shrink-0" /> You sign in on {serviceName(provider.name, providers)}'s own page, in your browser: Glint never sees your password. rclone keeps
                                the sign-in in its configuration.
                            </p>
                        )}
                    </form>
                );
            }
            case "asking": {
                const { question, error } = phase.step;
                return (
                    <form
                        className="space-y-3"
                        onSubmit={(e) => {
                            e.preventDefault();
                            void reply(phase.provider, phase.step.state);
                        }}
                    >
                        <p className="text-xs text-muted-foreground">rclone asks:</p>
                        <QuestionField question={question} value={answer} onChange={setAnswer} autoFocus />
                        {error && <Failure error={error} />}
                    </form>
                );
            }
            case "browser":
                return (
                    <div className="space-y-3">
                        <p className="flex items-start gap-2">
                            <Loader2 className="mt-0.5 size-4 shrink-0 animate-spin text-primary" />
                            <span>
                                Sign in to {serviceName(phase.provider.name, providers)} in your browser, and allow rclone there. This carries on by itself once you have.
                            </span>
                        </p>
                        {phase.url && (
                            <Button variant="outline" size="sm" onClick={() => void openUrl(phase.url!)}>
                                <ExternalLink /> Open the sign-in page again
                            </Button>
                        )}
                    </div>
                );
            case "working":
                return <Working what={phase.what} />;
            case "failed":
                return (
                    <div className="space-y-3">
                        <Failure error={phase.error} />
                        {phase.error.startsWith(LOCKED) && (
                            <Button size="sm" variant="outline" onClick={() => setPhase({ at: "unlock" })}>
                                <KeyRound /> Unlock it
                            </Button>
                        )}
                    </div>
                );
            case "ready":
                return (
                    <div className="space-y-3">
                        <p className="flex items-start gap-2">
                            <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-emerald-500" />
                            <span>{phase.said}</span>
                        </p>
                        <div className="space-y-1">
                            <label htmlFor="cloud-shown" className="block text-xs font-semibold text-muted-foreground">
                                Its name in Glint
                            </label>
                            <input
                                id="cloud-shown"
                                value={phase.connection.name}
                                onChange={(e) => setPhase({ ...phase, connection: { ...phase.connection, name: e.target.value } })}
                                className={INPUT}
                            />
                        </div>
                        <p className="text-xs text-muted-foreground">
                            It opens read-only, like Glint's other remotes: what you open or copy out is downloaded. Folder rules can keep a folder of yours in sync with it.
                        </p>
                    </div>
                );
        }
    })();

    const back = phase.at === "settings" || phase.at === "failed" || phase.at === "asking" || phase.at === "browser" || phase.at === "ready";
    return (
        <>
            <div className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4 text-sm">{body}</div>
            <div className="flex shrink-0 items-center gap-2 border-t px-4 py-3">
                {back && (
                    <Button variant="ghost" size="sm" onClick={() => void startOver()}>
                        <ArrowLeft /> Other clouds
                    </Button>
                )}
                <div className="flex-1" />
                <Button variant="ghost" size="sm" onClick={onCancel}>
                    Cancel
                </Button>
                {phase.at === "settings" && (
                    <Button size="sm" disabled={!name.trim() || missing(phase.provider)} onClick={() => void start(phase.provider)}>
                        {phase.provider.oauth ? (
                            <>
                                <Globe /> Sign in
                            </>
                        ) : (
                            <>
                                Connect <ChevronRight />
                            </>
                        )}
                    </Button>
                )}
                {phase.at === "asking" && (
                    <Button size="sm" onClick={() => void reply(phase.provider, phase.step.state)}>
                        Continue <ChevronRight />
                    </Button>
                )}
                {phase.at === "ready" && (
                    <Button size="sm" disabled={!phase.connection.name.trim()} onClick={() => finish({ ...phase.connection, name: phase.connection.name.trim(), id: remoteId(phase.connection.name.trim()) })}>
                        Add and open
                    </Button>
                )}
            </div>
        </>
    );
}

function Working({ what }: { what: string }) {
    return (
        <p className="flex items-center gap-2 text-muted-foreground" role="status">
            <Loader2 className="size-4 shrink-0 animate-spin text-primary" /> {what}
        </p>
    );
}

function Failure({ error }: { error: string }) {
    return (
        <p className="flex items-start gap-2 text-xs text-destructive" role="alert">
            <AlertTriangle className="mt-0.5 size-3.5 shrink-0" /> {error}
        </p>
    );
}
