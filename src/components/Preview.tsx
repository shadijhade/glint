import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ExternalLink, FileSearch, FolderOpen } from "lucide-react";
import { languageFor, SyntaxHighlighter } from "@/lib/highlight";
import { RgMatch } from "@/lib/ripgrep";
import { useStore } from "@/lib/store";
import { Button } from "./ui/button";

const BLOCK = 100; // the preview loads 300-line windows aligned to 100-line blocks, so nearby matches reuse it

interface PreviewProps {
    match?: RgMatch;
    onOpen: (path: string) => void;
    onReveal: (path: string) => void;
}

export function Preview({ match, onOpen, onReveal }: PreviewProps) {
    const showLineNumbers = useStore((s) => s.settings.showLineNumbers);
    const [chunk, setChunk] = useState<{ path: string; start: number; text: string } | null>(null);
    const [error, setError] = useState("");
    const codeRef = useRef<HTMLDivElement>(null);

    const path = match?.data.path?.text;
    const line = match?.data.line_number ?? 1;
    const start = Math.max(1, (Math.floor((line - 1) / BLOCK) - 1) * BLOCK + 1);

    useEffect(() => {
        if (!path) return;
        let stale = false;
        invoke<string>("read_file_chunk", { path, startLine: start, endLine: start + 3 * BLOCK - 1 })
            .then((text) => {
                if (stale) return;
                setChunk({ path, start, text });
                setError("");
            })
            .catch((e) => {
                if (!stale) setError(String(e));
            });
        return () => {
            stale = true;
        };
    }, [path, start]);

    useEffect(() => {
        codeRef.current?.querySelector(".hl-line")?.scrollIntoView({ block: "center" });
    }, [chunk, line]);

    if (!path) {
        return (
            <div className="flex flex-1 flex-col items-center justify-center gap-2 text-sm text-muted-foreground">
                <FileSearch className="size-6" /> Select a result to preview it
            </div>
        );
    }

    const name = path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1);
    const current = chunk?.path === path ? chunk : null;
    const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "";

    return (
        <div className="flex min-w-0 flex-1 flex-col">
            <div className="flex h-10 shrink-0 items-center gap-2 border-b bg-chrome pl-3 pr-1.5">
                <span className="truncate text-sm" title={path}>
                    <span className="font-medium">{name}</span>
                    <span className="text-muted-foreground"> · line {line}</span>
                </span>
                <div className="ml-auto flex shrink-0">
                    <Button variant="ghost" size="icon" className="size-7" onClick={() => onReveal(path)} title="Show in folder">
                        <FolderOpen />
                    </Button>
                    <Button variant="ghost" size="icon" className="size-7" onClick={() => onOpen(path)} title="Open file (Enter)">
                        <ExternalLink />
                    </Button>
                </div>
            </div>
            <div ref={codeRef} className="min-h-0 flex-1 select-text overflow-auto bg-card">
                {error ? (
                    <p className="p-4 text-sm text-destructive">{error}</p>
                ) : current && (
                    <SyntaxHighlighter
                        language={languageFor(ext)}
                        useInlineStyles={false}
                        showLineNumbers={showLineNumbers}
                        startingLineNumber={current.start}
                        wrapLines
                        lineProps={(n: number) => ({ className: n === line ? "block pl-3 pr-4 hl-line" : "block pl-3 pr-4" })}
                        className="min-w-max py-2 font-mono text-[0.8125rem] leading-5"
                    >
                        {current.text}
                    </SyntaxHighlighter>
                )}
            </div>
        </div>
    );
}
