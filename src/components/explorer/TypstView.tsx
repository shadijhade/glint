import { useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { AlertTriangle, FileType, Loader2 } from "lucide-react";
import type { FileEntry } from "@/lib/entries";
import { useTool } from "@/lib/tools";
import { ToolNeeded } from "../ToolNeeded";

/** A .typ file typeset by Typst (docconvert.rs's typst_preview: its PDF, kept in Glint's cache), drawn by `render`. */
export function TypstView({ entry, render }: { entry: FileEntry; render: (src: string) => React.ReactNode }) {
    const tool = useTool("typst");
    const ready = !!tool?.origin;
    const [pdf, setPdf] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        if (!ready) return;
        let live = true;
        setPdf(null);
        setError(null);
        invoke<string>("typst_preview", { path: entry.path })
            .then((p) => live && setPdf(p))
            .catch((e) => live && setError(String(e)));
        return () => {
            live = false;
        };
    }, [entry.path, entry.modified, ready]);
    if (tool && !ready)
        return (
            <div className="p-4">
                <ToolNeeded id="typst" why="Typst typesets .typ files: their preview here, and Export as PDF." icon={FileType} />
            </div>
        );
    if (error)
        return (
            <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-sm">
                <AlertTriangle className="size-6 text-amber-500" />
                <p className="font-medium">Typst couldn't typeset it</p>
                <pre className="max-h-48 max-w-full overflow-auto whitespace-pre-wrap text-start text-xs text-muted-foreground">{error}</pre>
            </div>
        );
    if (!pdf)
        return (
            <div className="flex h-full items-center justify-center gap-2 text-sm text-muted-foreground">
                <Loader2 className="size-4 animate-spin" /> Typesetting…
            </div>
        );
    return <>{render(convertFileSrc(pdf))}</>;
}
