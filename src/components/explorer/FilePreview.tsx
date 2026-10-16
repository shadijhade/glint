import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Cloud, ExternalLink, Files, FolderOpen, HardDrive, X } from "lucide-react";
import { type FileEntry, isOnlineOnly } from "@/lib/entries";
import { calcSizes, navigate, useExplorer } from "@/lib/explorer";
import { isKnownType, kindOf, previewKind, typeLabel } from "@/lib/fileTypes";
import { formatCount, formatFullDate, formatSize } from "@/lib/format";
import { listDir, readTextPreview } from "@/lib/fs";
import { languageFor, SyntaxHighlighter } from "@/lib/highlight";
import { baseName, parentPath } from "@/lib/paths";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";

const TEXT_BYTES = 256 * 1024;
const TEXT_LINES = 2000;

function Centered({ children, className }: { children: React.ReactNode; className?: string }) {
    return <div className={cn("flex h-full flex-col items-center justify-center gap-3 p-6 text-center text-sm text-muted-foreground", className)}>{children}</div>;
}

function BigIcon({ entry, note }: { entry: FileEntry; note?: string }) {
    const kind = kindOf(entry);
    return (
        <Centered>
            <kind.icon className={cn("size-16 stroke-[1.25]", kind.tone)} />
            {note && <p>{note}</p>}
        </Centered>
    );
}

function ImagePreview({ entry, src }: { entry: FileEntry; src: string }) {
    const [dimensions, setDimensions] = useState("");
    const [failed, setFailed] = useState(false);
    if (failed) return <BigIcon entry={entry} note="This image can't be shown here" />;
    return (
        <Centered className="gap-2 p-4">
            <img
                src={src}
                alt={entry.name}
                draggable={false}
                onLoad={(e) => setDimensions(`${e.currentTarget.naturalWidth} × ${e.currentTarget.naturalHeight}`)}
                onError={() => setFailed(true)}
                className="max-h-[calc(100%-1.5rem)] max-w-full rounded-sm object-contain shadow-sm [background:repeating-conic-gradient(hsl(var(--muted))_0%_25%,transparent_0%_50%)_50%/16px_16px]"
            />
            {dimensions && <span className="text-xs tabular-nums">{dimensions} pixels</span>}
        </Centered>
    );
}

function TextPreview({ entry }: { entry: FileEntry }) {
    const showLineNumbers = useStore((s) => s.settings.showLineNumbers);
    const [state, setState] = useState<{ text: string; truncated: boolean; binary: boolean } | { error: string } | null>(null);
    useEffect(() => {
        let stale = false;
        readTextPreview(entry.path, TEXT_BYTES).then(
            (result) => !stale && setState(result),
            (e) => !stale && setState({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified]);

    if (!state) return null;
    if ("error" in state) return <p className="p-4 text-sm text-destructive">{state.error}</p>;
    if (state.binary) return <BigIcon entry={entry} note="No preview for this kind of file" />;
    if (!state.text) return <Centered>This file is empty.</Centered>;
    const lines = state.text.split("\n");
    const cut = lines.length > TEXT_LINES;
    return (
        <div className="min-h-full select-text">
            <SyntaxHighlighter
                language={languageFor(entry.ext)}
                useInlineStyles={false}
                showLineNumbers={showLineNumbers}
                wrapLines
                lineProps={{ className: "block pl-3 pr-4" }}
                className="min-w-max py-2 font-mono text-[0.8125rem] leading-5"
            >
                {cut ? lines.slice(0, TEXT_LINES).join("\n") : state.text}
            </SyntaxHighlighter>
            {(cut || state.truncated) && <p className="px-4 pb-3 text-xs text-muted-foreground">Preview shows the beginning of the file. Open it to see the rest.</p>}
        </div>
    );
}

function FolderPreview({ entry }: { entry: FileEntry }) {
    const size = useExplorer((s) => s.sizes[parentPath(entry.path)]?.sizes.get(entry.name));
    const [counts, setCounts] = useState<{ folders: number; files: number; bytes: number } | { error: string } | null>(null);
    useEffect(() => {
        let stale = false;
        listDir(entry.path).then(
            ({ entries }) => {
                if (stale) return;
                const files = entries.filter((e) => !e.dir);
                setCounts({ folders: entries.length - files.length, files: files.length, bytes: files.reduce((sum, e) => sum + e.size, 0) });
            },
            (e) => !stale && setCounts({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified]);

    return (
        <Centered>
            <FolderOpen className="size-16 fill-amber-400/80 stroke-[1.25] text-amber-500" />
            {counts &&
                ("error" in counts ? (
                    <p>{counts.error}</p>
                ) : (
                    <p>
                        {formatCount(counts.folders)} {counts.folders === 1 ? "folder" : "folders"}, {formatCount(counts.files)} {counts.files === 1 ? "file" : "files"}
                        {counts.files > 0 && <span className="block text-xs">{formatSize(counts.bytes)} in files directly inside</span>}
                    </p>
                ))}
            {size !== undefined ? (
                <p className="text-foreground">
                    <b className="font-semibold">{formatSize(size)}</b> in total
                </p>
            ) : (
                parentPath(entry.path) === useExplorer.getState().path && (
                    <Button variant="outline" size="sm" onClick={calcSizes}>
                        <HardDrive /> Calculate folder sizes
                    </Button>
                )
            )}
        </Centered>
    );
}

function Details({ entry }: { entry: FileEntry }) {
    const rows: [string, string][] = [
        ["Type", typeLabel(entry)],
        ...(!entry.dir ? [["Size", `${formatSize(entry.size)} (${formatCount(entry.size)} bytes)`] as [string, string]] : []),
        ["Modified", formatFullDate(entry.modified)],
        ["Location", parentPath(entry.path)],
    ];
    return (
        <dl className="grid shrink-0 grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 border-t px-4 py-3 text-xs">
            {rows.map(([label, value]) => (
                <div key={label} className="contents">
                    <dt className="text-muted-foreground">{label}</dt>
                    <dd className="select-text truncate" title={value}>
                        {value}
                    </dd>
                </div>
            ))}
        </dl>
    );
}

/** One item's preview with its name, actions and details; Quick Look shows it large, with a close button. */
export function EntryPreview({ entry, onClose }: { entry: FileEntry; onClose?: () => void }) {
    const kind = previewKind(entry);
    const src = convertFileSrc(entry.path);
    let body: React.ReactNode;
    if (entry.dir) body = <FolderPreview entry={entry} />;
    else if (isOnlineOnly(entry))
        body = (
            <Centered>
                <Cloud className="size-14 stroke-[1.25] text-sky-500" />
                <p>This file is online-only. Opening it downloads it.</p>
            </Centered>
        );
    else if (kind === "image") body = <ImagePreview entry={entry} src={src} />;
    else if (kind === "video")
        body = (
            <Centered className="p-3">
                <video src={src} controls className="max-h-full max-w-full rounded-sm" />
            </Centered>
        );
    else if (kind === "audio")
        body = (
            <Centered>
                <BigIcon entry={entry} />
                <audio src={src} controls className="w-full max-w-sm" />
            </Centered>
        );
    else if (kind === "pdf") body = <iframe src={src} title={entry.name} className="h-full w-full bg-white" />;
    // Unknown extensions get a text preview attempt; the backend spots binary files.
    else if (kind === "text" || !isKnownType(entry)) body = <TextPreview entry={entry} />;
    else body = <BigIcon entry={entry} />;

    const open = () => (entry.dir ? navigate(entry.path) : openPath(entry.path).catch((e) => toast.error(`Couldn't open “${entry.name}”`, { description: String(e) })));

    return (
        <div className="flex min-w-0 flex-1 flex-col">
            <div className="flex h-10 shrink-0 items-center gap-2 border-b pl-3 pr-1.5">
                <span className="truncate text-sm font-medium" title={entry.path}>
                    {entry.name}
                </span>
                <div className="ml-auto flex shrink-0">
                    <Button variant="ghost" size="icon" className="size-7" onClick={() => revealItemInDir(entry.path)} title="Show in File Explorer">
                        <FolderOpen />
                    </Button>
                    <Button variant="ghost" size="icon" className="size-7" onClick={open} title="Open (Enter)">
                        <ExternalLink />
                    </Button>
                    {onClose && (
                        <Button variant="ghost" size="icon" className="size-7" onClick={onClose} title="Close (Space or Esc)" aria-label="Close">
                            <X />
                        </Button>
                    )}
                </div>
            </div>
            <div className="min-h-0 flex-1 overflow-auto bg-card">{body}</div>
            <Details entry={entry} />
        </div>
    );
}

/** Waits briefly so arrowing quickly through a list doesn't load every file on the way. */
function useSettled<T>(value: T, ms = 70) {
    const [settled, setSettled] = useState(value);
    useEffect(() => {
        const timer = setTimeout(() => setSettled(value), ms);
        return () => clearTimeout(timer);
    }, [value, ms]);
    return settled;
}

interface FolderSummary {
    path: string;
    items: number;
    /** Everything inside, once sized (by the index or dust). */
    total?: number;
}

export function FilePreview({ selected, folder }: { selected: FileEntry[]; folder: FolderSummary }) {
    const entry = useSettled(selected.length === 1 ? selected[0] : null);
    if (selected.length > 1) {
        const files = selected.filter((e) => !e.dir);
        return (
            <Centered>
                <Files className="size-14 stroke-[1.25] text-primary" />
                <p className="text-foreground">{formatCount(selected.length)} items selected</p>
                <p>
                    {formatCount(selected.length - files.length)} folders, {formatCount(files.length)} files
                    {files.length > 0 && <span className="block">{formatSize(files.reduce((sum, e) => sum + e.size, 0))} in files</span>}
                </p>
            </Centered>
        );
    }
    if (!entry || !selected.length) {
        return (
            <Centered>
                <FolderOpen className="size-14 fill-amber-400/80 stroke-[1.25] text-amber-500" />
                <p className="font-medium text-foreground">{baseName(folder.path) || "This PC"}</p>
                <p>
                    {formatCount(folder.items)} {folder.items === 1 ? "item" : "items"}
                    {folder.total !== undefined && ` · ${formatSize(folder.total)} in total`}
                </p>
                <p className="text-xs">Select a file to preview it</p>
            </Centered>
        );
    }
    return <EntryPreview key={entry.path} entry={entry} />;
}
