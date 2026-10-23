import { useEffect, useMemo, useState } from "react";
import { BookOpen, ChevronRight, X } from "lucide-react";
import { type FileEntry, isArchived, isListOnly, isLocked, isOnlineOnly } from "@/lib/entries";
import { readTextPreview } from "@/lib/fs";
import { markdownTitle } from "@/lib/markdown";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { MarkdownView } from "./MarkdownView";

/** The names a folder's README goes by, in the order one is picked. */
const NAMES = ["readme.md", "readme.markdown", "readme.mdown", "readme.mkd", "readme.mkdn"];

/** A folder's README, if it has one that can be read without downloading it (an online-only file would be). */
export function readmeOf(entries: FileEntry[]) {
    let best: FileEntry | undefined;
    let rank = NAMES.length;
    for (const entry of entries) {
        const at = entry.dir ? -1 : NAMES.indexOf(entry.lower);
        if (at >= 0 && at < rank && !isOnlineOnly(entry) && !isLocked(entry) && !isListOnly(entry)) [best, rank] = [entry, at];
    }
    return best;
}

/**
 * A folder's README above its list, folded to one line (its title) until it's opened, then as it reads,
 * up to half the pane; opened or folded stays from folder to folder.
 */
export function ReadmeCard({ entry }: { entry: FileEntry }) {
    const open = useStore((s) => s.settings.readmeOpen);
    const { setSettings } = useStore.getState();
    const [text, setText] = useState<string | null>(null);
    useEffect(() => {
        let stale = false;
        readTextPreview(entry.path, 256 * 1024).then(
            (result) => !stale && setText(result.binary ? "" : result.text),
            () => !stale && setText("")
        );
        return () => {
            stale = true;
        };
    }, [entry.path, entry.modified]);
    const title = useMemo(() => (text ? markdownTitle(text) : ""), [text]);
    if (text === "") return null;
    return (
        <div className={cn("flex shrink-0 flex-col border-b", open && "max-h-[50%]")}>
            <div className="flex h-8 shrink-0 items-center gap-1 pl-2 pr-1.5 text-xs">
                <button
                    type="button"
                    aria-expanded={open}
                    onClick={() => setSettings({ readmeOpen: !open })}
                    title={open ? "Fold the README" : "Show the README"}
                    className="flex min-w-0 flex-1 items-center gap-1.5 self-stretch rounded-[4px] px-1 text-left hover:text-foreground"
                >
                    <ChevronRight className={cn("size-3.5 shrink-0 text-muted-foreground transition-transform", open && "rotate-90")} />
                    <BookOpen className="size-3.5 shrink-0 text-primary" />
                    <span className="shrink-0 font-medium">{entry.name}</span>
                    {title && <span className="truncate text-muted-foreground">{title}</span>}
                </button>
                <button
                    type="button"
                    onClick={() => setSettings({ showReadme: false })}
                    title="Don't show READMEs above the list (Settings → Files)"
                    aria-label="Don't show READMEs"
                    className="grid size-5 shrink-0 place-items-center rounded-[4px] text-muted-foreground hover:bg-accent hover:text-foreground"
                >
                    <X className="size-3.5" />
                </button>
            </div>
            {open && text && (
                <div className="min-h-0 overflow-auto bg-card px-5 pb-4 pt-2 select-text">
                    <MarkdownView text={text} file={entry.path} archived={isArchived(entry)} />
                </div>
            )}
        </div>
    );
}
