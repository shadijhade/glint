import { useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { AlertTriangle, CalendarDays, Loader2, Mail, Paperclip, User } from "lucide-react";
import type { FileEntry } from "@/lib/entries";
import { formatSize } from "@/lib/format";
import type { SmallKind } from "@/lib/smallKinds";


/** The file's bytes, read through the page's own file access. */
async function bytes(entry: FileEntry) {
    const response = await fetch(convertFileSrc(entry.path));
    if (!response.ok) throw new Error(`Couldn't read it (${response.status})`);
    return response.arrayBuffer();
}

const text = async (entry: FileEntry) => new TextDecoder().decode(await bytes(entry));

/** Runs `load` for the entry, with its loading and error states. */
function useLoaded<T>(entry: FileEntry, load: () => Promise<T>) {
    const [value, setValue] = useState<T | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        let live = true;
        setValue(null);
        setError(null);
        load()
            .then((v) => live && setValue(v))
            .catch((e) => live && setError(String(e?.message ?? e)));
        return () => {
            live = false;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [entry.path, entry.modified]);
    return { value, error };
}

function Loading() {
    return (
        <div className="flex h-full items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="size-4 animate-spin" /> Reading…
        </div>
    );
}

function Failed({ error }: { error: string }) {
    return (
        <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-sm">
            <AlertTriangle className="size-6 text-amber-500" />
            <p className="max-w-sm text-muted-foreground">{error}</p>
        </div>
    );
}

export function SmallPreview({ entry, kind }: { entry: FileEntry; kind: SmallKind }) {
    if (kind === "contact") return <Contacts entry={entry} />;
    if (kind === "calendar") return <Calendar entry={entry} />;
    if (kind === "email" || kind === "outlook") return <Email entry={entry} outlook={kind === "outlook"} />;
    if (kind === "design") return <DesignPicture entry={entry} />;
    if (kind === "graphviz") return <Graphviz entry={entry} />;
    return <MermaidFile entry={entry} />;
}

/** vCard and iCalendar lines, unfolded (a line starting with a space or tab continues the one before). */
function lines(source: string) {
    return source.replace(/\r?\n[ \t]/g, "").split(/\r?\n/);
}

/** One property: its name, its parameters (TYPE=WORK…) and its value, unescaped. */
function property(line: string) {
    const colon = line.indexOf(":");
    if (colon < 0) return null;
    const [name, ...params] = line.slice(0, colon).split(";");
    const value = line.slice(colon + 1).replace(/\\n/gi, "\n").replace(/\\([,;\\])/g, "$1");
    return { name: name.toUpperCase().replace(/^ITEM\d+\./, ""), params: params.join(";").toUpperCase(), value };
}

type Card = { name: string; rows: [string, string][]; photo?: string };

export function readContacts(source: string): Card[] {
    const cards: Card[] = [];
    let card: Card | null = null;
    for (const line of lines(source)) {
        const p = property(line);
        if (!p) continue;
        if (p.name === "BEGIN" && p.value.toUpperCase() === "VCARD") card = { name: "", rows: [] };
        else if (p.name === "END" && card) {
            cards.push(card);
            card = null;
        } else if (card) {
            const kind = /WORK/.test(p.params) ? " (work)" : /HOME/.test(p.params) ? " (home)" : /CELL/.test(p.params) ? " (mobile)" : "";
            if (p.name === "FN") card.name = p.value;
            else if (p.name === "N" && !card.name) card.name = p.value.split(";").filter(Boolean).reverse().join(" ");
            else if (p.name === "TEL") card.rows.push([`Phone${kind}`, p.value.replace(/^tel:/i, "")]);
            else if (p.name === "EMAIL") card.rows.push([`Email${kind}`, p.value]);
            else if (p.name === "ORG") card.rows.push(["Organization", p.value.split(";").filter(Boolean).join(", ")]);
            else if (p.name === "TITLE") card.rows.push(["Title", p.value]);
            else if (p.name === "ADR") card.rows.push([`Address${kind}`, p.value.split(";").filter(Boolean).join(", ")]);
            else if (p.name === "URL") card.rows.push(["Website", p.value]);
            else if (p.name === "BDAY") card.rows.push(["Birthday", p.value]);
            else if (p.name === "NOTE") card.rows.push(["Note", p.value]);
            else if (p.name === "PHOTO" && /^data:image\//.test(p.value)) card.photo = p.value;
            else if (p.name === "PHOTO" && /ENCODING=B|BASE64/.test(p.params)) card.photo = `data:image/${/PNG/.test(p.params) ? "png" : "jpeg"};base64,${p.value}`;
        }
    }
    return cards;
}

function Contacts({ entry }: { entry: FileEntry }) {
    const { value, error } = useLoaded(entry, async () => readContacts(await text(entry)));
    if (error) return <Failed error={error} />;
    if (!value) return <Loading />;
    if (!value.length) return <Failed error="No contacts in it." />;
    return (
        <div className="h-full space-y-3 overflow-y-auto p-4 text-sm">
            {value.slice(0, 500).map((card, i) => (
                <div key={i} className="flex gap-3 rounded-lg border p-3">
                    {card.photo ? <img src={card.photo} alt="" className="size-12 shrink-0 rounded-full object-cover" /> : <User className="size-12 shrink-0 rounded-full bg-muted p-2.5 text-muted-foreground" />}
                    <div className="min-w-0 flex-1">
                        <p className="truncate font-semibold">{card.name || "(no name)"}</p>
                        <dl className="mt-1 grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-xs">
                            {card.rows.map(([label, v], j) => (
                                <div key={j} className="contents">
                                    <dt className="text-muted-foreground">{label}</dt>
                                    <dd className="min-w-0 whitespace-pre-wrap break-words">{v}</dd>
                                </div>
                            ))}
                        </dl>
                    </div>
                </div>
            ))}
            {value.length > 500 && <p className="text-xs text-muted-foreground">And {value.length - 500} more.</p>}
        </div>
    );
}

type Event = { summary: string; start: string; end: string; location: string; description: string; organizer: string; attendees: string[] };

/** 20261109T143000Z (or a date alone) as the PC's local time. */
function icsDate(value: string) {
    const m = /^(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2})(\d{2})(Z)?)?/.exec(value);
    if (!m) return value;
    const [, y, mo, d, h, mi, s, z] = m;
    if (!h) return new Date(+y, +mo - 1, +d).toLocaleDateString(undefined, { dateStyle: "full" });
    const date = z ? new Date(Date.UTC(+y, +mo - 1, +d, +h, +mi, +s)) : new Date(+y, +mo - 1, +d, +h, +mi, +s);
    return date.toLocaleString(undefined, { dateStyle: "full", timeStyle: "short" });
}

export function readEvents(source: string): Event[] {
    const events: Event[] = [];
    let event: Event | null = null;
    for (const line of lines(source)) {
        const p = property(line);
        if (!p) continue;
        if (p.name === "BEGIN" && p.value.toUpperCase() === "VEVENT") event = { summary: "", start: "", end: "", location: "", description: "", organizer: "", attendees: [] };
        else if (p.name === "END" && p.value.toUpperCase() === "VEVENT" && event) {
            events.push(event);
            event = null;
        } else if (event) {
            const who = (v: string) => v.replace(/^mailto:/i, "");
            const cn = /CN="?([^";:]+)"?/i.exec(line.slice(0, line.indexOf(":")))?.[1];
            if (p.name === "SUMMARY") event.summary = p.value;
            else if (p.name === "DTSTART") event.start = icsDate(p.value);
            else if (p.name === "DTEND") event.end = icsDate(p.value);
            else if (p.name === "LOCATION") event.location = p.value;
            else if (p.name === "DESCRIPTION") event.description = p.value;
            else if (p.name === "ORGANIZER") event.organizer = cn ?? who(p.value);
            else if (p.name === "ATTENDEE") event.attendees.push(cn ?? who(p.value));
        }
    }
    return events;
}

function Calendar({ entry }: { entry: FileEntry }) {
    const { value, error } = useLoaded(entry, async () => readEvents(await text(entry)));
    if (error) return <Failed error={error} />;
    if (!value) return <Loading />;
    if (!value.length) return <Failed error="No events in it." />;
    return (
        <div className="h-full space-y-3 overflow-y-auto p-4 text-sm">
            {value.slice(0, 200).map((ev, i) => (
                <div key={i} className="rounded-lg border p-3">
                    <p className="flex items-center gap-2 font-semibold">
                        <CalendarDays className="size-4 shrink-0 text-primary" /> {ev.summary || "(no title)"}
                    </p>
                    <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-xs">
                        {(
                            [
                                ["When", ev.start + (ev.end ? ` – ${ev.end}` : "")],
                                ["Where", ev.location],
                                ["Organizer", ev.organizer],
                                ["Invited", ev.attendees.join(", ")],
                            ] as [string, string][]
                        )
                            .filter(([, v]) => v)
                            .map(([label, v]) => (
                                <div key={label} className="contents">
                                    <dt className="text-muted-foreground">{label}</dt>
                                    <dd className="min-w-0 break-words">{v}</dd>
                                </div>
                            ))}
                    </dl>
                    {ev.description && <p className="mt-2 whitespace-pre-wrap text-xs text-muted-foreground">{ev.description}</p>}
                </div>
            ))}
        </div>
    );
}

type Attachment = { name: string; size: number; data: () => Promise<Uint8Array> };
type Message = { subject: string; from: string; to: string; cc: string; date: string; html: string; text: string; attachments: Attachment[]; remote: boolean };

const people = (list?: { name?: string; address?: string }[] | null) => (list ?? []).map((p) => (p.name ? `${p.name}${p.address ? ` <${p.address}>` : ""}` : (p.address ?? ""))).join(", ");

/** An email's HTML for a sandboxed frame: no scripts, nothing from the internet (no tracking pictures), its own pictures inline. */
function safeHtml(html: string, inline: Map<string, string>) {
    const withPictures = html.replace(/(["'(])cid:([^"')]+)/gi, (all, q: string, id: string) => (inline.has(id) ? `${q}${inline.get(id)}` : all));
    const policy = `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:">`;
    return `<!doctype html><html><head><meta charset="utf-8">${policy}<base target="_blank"><style>body{font:14px system-ui,sans-serif;margin:12px;word-wrap:break-word}img{max-width:100%;height:auto}</style></head><body>${withPictures}</body></html>`;
}

const toBase64 = (data: Uint8Array) => {
    let binary = "";
    for (let i = 0; i < data.length; i += 0x8000) binary += String.fromCharCode(...data.subarray(i, i + 0x8000));
    return btoa(binary);
};

async function readEmail(entry: FileEntry): Promise<Message> {
    const PostalMime = (await import("postal-mime")).default;
    const email = await PostalMime.parse(await bytes(entry));
    const inline = new Map<string, string>();
    for (const a of email.attachments) {
        const id = a.contentId?.replace(/^<|>$/g, "");
        if (id && a.mimeType.startsWith("image/")) inline.set(id, `data:${a.mimeType};base64,${toBase64(new Uint8Array(a.content as ArrayBuffer))}`);
    }
    const html = email.html ?? "";
    return {
        subject: email.subject ?? "",
        from: people(email.from ? [email.from] : []),
        to: people(email.to),
        cc: people(email.cc),
        date: email.date ? new Date(email.date).toLocaleString(undefined, { dateStyle: "full", timeStyle: "short" }) : "",
        html: html ? safeHtml(html, inline) : "",
        text: email.text ?? "",
        remote: /<img[^>]+src=["']?https?:/i.test(html),
        attachments: email.attachments
            .filter((a) => a.disposition !== "inline" || !a.contentId)
            .map((a) => ({ name: a.filename ?? "attachment", size: (a.content as ArrayBuffer).byteLength, data: async () => new Uint8Array(a.content as ArrayBuffer) })),
    };
}

async function readOutlook(entry: FileEntry): Promise<Message> {
    const MsgReader = (await import("@kenjiuno/msgreader")).default;
    const reader = new MsgReader(await bytes(entry));
    const data = reader.getFileData();
    const recipients = (kind: string) => (data.recipients ?? []).filter((r) => (r.recipType ?? "to") === kind).map((r) => (r.name && r.email && r.name !== r.email ? `${r.name} <${r.email}>` : (r.name ?? r.email ?? ""))).join(", ");
    const when = data.messageDeliveryTime ?? data.clientSubmitTime;
    const rawHtml = data.bodyHtml ?? (data.html ? new TextDecoder().decode(data.html) : "");
    return {
        subject: data.subject ?? "",
        from: data.senderName ? `${data.senderName}${data.senderEmail ? ` <${data.senderEmail}>` : ""}` : (data.senderEmail ?? ""),
        to: recipients("to"),
        cc: recipients("cc"),
        date: when ? new Date(when).toLocaleString(undefined, { dateStyle: "full", timeStyle: "short" }) : "",
        html: rawHtml ? safeHtml(rawHtml, new Map()) : "",
        text: data.body ?? "",
        remote: /<img[^>]+src=["']?https?:/i.test(rawHtml),
        attachments: (data.attachments ?? [])
            .filter((a) => !a.attachmentHidden)
            .map((a) => ({ name: a.fileName ?? a.name ?? "attachment", size: a.contentLength ?? 0, data: async () => reader.getAttachment(a).content })),
    };
}

/** Saves an attachment to a temp folder of Glint's and opens it with its app. */
async function openAttachment(a: Attachment) {
    const path = await invoke<string>("attachment_save", await a.data(), { headers: { name: encodeURIComponent(a.name) } });
    await invoke("open_item", { path });
}

function Email({ entry, outlook }: { entry: FileEntry; outlook: boolean }) {
    const { value, error } = useLoaded(entry, () => (outlook ? readOutlook(entry) : readEmail(entry)));
    const [failed, setFailed] = useState<string | null>(null);
    if (error) return <Failed error={outlook ? `Glint couldn't read this Outlook message: ${error}` : error} />;
    if (!value) return <Loading />;
    return (
        <div className="flex h-full min-h-0 flex-col text-sm">
            <div className="space-y-0.5 border-b p-3">
                <p className="flex items-center gap-2 font-semibold">
                    <Mail className="size-4 shrink-0 text-primary" /> {value.subject || "(no subject)"}
                </p>
                <dl className="grid grid-cols-[auto_1fr] gap-x-3 text-xs">
                    {(
                        [
                            ["From", value.from],
                            ["To", value.to],
                            ["Cc", value.cc],
                            ["Date", value.date],
                        ] as [string, string][]
                    )
                        .filter(([, v]) => v)
                        .map(([label, v]) => (
                            <div key={label} className="contents">
                                <dt className="text-muted-foreground">{label}</dt>
                                <dd className="min-w-0 truncate" title={v}>
                                    {v}
                                </dd>
                            </div>
                        ))}
                </dl>
                {value.attachments.length > 0 && (
                    <div className="flex flex-wrap gap-1 pt-1.5">
                        {value.attachments.map((a, i) => (
                            <button
                                key={i}
                                type="button"
                                title="Open it with its app"
                                onClick={() => openAttachment(a).catch((e) => setFailed(String(e)))}
                                className="flex max-w-56 items-center gap-1 rounded-md border px-2 py-0.5 text-xs hover:bg-accent"
                            >
                                <Paperclip className="size-3 shrink-0" />
                                <span className="truncate">{a.name}</span>
                                <span className="shrink-0 text-muted-foreground">{formatSize(a.size)}</span>
                            </button>
                        ))}
                    </div>
                )}
                {failed && <p className="text-xs text-destructive">{failed}</p>}
                {value.remote && <p className="pt-1 text-xs text-muted-foreground">Pictures from the internet aren't loaded, so nobody learns it was opened.</p>}
            </div>
            {value.html ? (
                <iframe title="The message" sandbox="allow-popups" srcDoc={value.html} className="min-h-0 w-full flex-1 bg-white" />
            ) : (
                <pre className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap p-3 font-sans text-sm">{value.text}</pre>
            )}
        </div>
    );
}

/** The picture a Krita, OpenRaster, Procreate or Sketch file keeps of itself. */
const DESIGN_PICTURES = ["mergedimage.png", "Thumbnail.png", "QuickLook/Thumbnail.png", "previews/preview.png", "preview.png"];

function DesignPicture({ entry }: { entry: FileEntry }) {
    const { value, error } = useLoaded(entry, async () => {
        const { BlobReader, BlobWriter, ZipReader } = await import("@zip.js/zip.js");
        const zip = new ZipReader(new BlobReader(new Blob([await bytes(entry)])));
        try {
            const entries = await zip.getEntries();
            const found = DESIGN_PICTURES.map((name) => entries.find((e) => e.filename.toLowerCase() === name.toLowerCase())).find((e) => e && !e.directory);
            if (!found || found.directory) throw new Error("It keeps no picture of itself to show.");
            return URL.createObjectURL(await found.getData(new BlobWriter("image/png")));
        } finally {
            await zip.close();
        }
    });
    useEffect(() => () => void (value && URL.revokeObjectURL(value)), [value]);
    if (error) return <Failed error={error} />;
    if (!value) return <Loading />;
    return (
        <div className="flex h-full items-center justify-center p-3">
            <img src={value} alt={entry.name} className="max-h-full max-w-full rounded-sm object-contain" draggable={false} />
        </div>
    );
}

function Graphviz({ entry }: { entry: FileEntry }) {
    const { value, error } = useLoaded(entry, async () => {
        const { Graphviz } = await import("@hpcc-js/wasm-graphviz");
        const graphviz = await Graphviz.load();
        const raw = new Uint8Array(await bytes(entry));
        // .dot is also Word 97's template extension: those are compound files.
        if (raw[0] === 0xd0 && raw[1] === 0xcf && raw[2] === 0x11 && raw[3] === 0xe0) throw new Error("This .dot file is a Word template, which Word opens.");
        return graphviz.dot(new TextDecoder().decode(raw));
    });
    if (error) return <Failed error={`Graphviz couldn't draw it: ${error}`} />;
    if (!value) return <Loading />;
    return <div className="h-full overflow-auto bg-white p-3 [&_svg]:mx-auto [&_svg]:h-auto [&_svg]:max-w-full" dangerouslySetInnerHTML={{ __html: value }} />;
}

let mermaidCount = 0;

function MermaidFile({ entry }: { entry: FileEntry }) {
    const { value, error } = useLoaded(entry, async () => {
        const mermaid = (await import("mermaid")).default;
        mermaid.initialize({ startOnLoad: false, securityLevel: "strict", theme: document.documentElement.classList.contains("dark") ? "dark" : "default" });
        const { svg } = await mermaid.render(`glint-mermaid-file-${++mermaidCount}`, await text(entry));
        return svg;
    });
    if (error) return <Failed error={`Mermaid couldn't draw it: ${error}`} />;
    if (!value) return <Loading />;
    return <div className="h-full overflow-auto p-3 [&_svg]:mx-auto [&_svg]:h-auto [&_svg]:max-w-full" dangerouslySetInnerHTML={{ __html: value }} />;
}
