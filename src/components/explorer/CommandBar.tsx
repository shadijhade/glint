import { ClipboardPaste, Copy, Eye, EyeOff, FolderPlus, HardDrive, Loader2, type LucideIcon, PanelRight, PenLine, Scissors, SquareTerminal, TextSearch, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { useShallow } from "zustand/react/shallow";
import type { FileEntry } from "@/lib/entries";
import { calcSizes, copyToClipboard, newItem, paste, stopSizes, useExplorer } from "@/lib/explorer";
import { openTerminal } from "@/lib/fs";
import { THIS_PC } from "@/lib/paths";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Button } from "../ui/button";

function Tool({ icon: Icon, label, pressed, className, ...props }: { icon: LucideIcon; label: string; pressed?: boolean } & React.ComponentProps<"button">) {
    return (
        <button
            type="button"
            title={label}
            aria-label={label}
            aria-pressed={pressed}
            className={cn(
                "grid size-8 shrink-0 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-35 [&_svg]:size-4",
                pressed && "bg-primary/15 text-primary hover:bg-primary/20 hover:text-primary",
                className
            )}
            {...props}
        >
            <Icon />
        </button>
    );
}

const Divider = () => <span className="mx-1 h-5 w-px shrink-0 bg-border" />;

interface CommandBarProps {
    selected: FileEntry[];
    onRename: () => void;
    onDelete: () => void;
    onSearchText: () => void;
}

export function CommandBar({ selected, onRename, onDelete, onSearchText }: CommandBarProps) {
    const { path, find, sizing } = useExplorer(useShallow((s) => ({ path: s.path, find: s.find, sizing: !!s.sizes[s.path]?.running })));
    const { showHidden, showPreviewPane } = useStore(useShallow((s) => ({ showHidden: s.settings.showHidden, showPreviewPane: s.settings.showPreviewPane })));
    const setSettings = useStore((s) => s.setSettings);
    const inFolder = path !== THIS_PC;
    const paths = selected.map((e) => e.path);
    const terminal = () => openTerminal(path).catch((e) => toast.error("Couldn't open a terminal", { description: String(e) }));

    return (
        <div className="flex h-10 shrink-0 items-center gap-0.5 overflow-hidden border-b bg-card/40 px-2">
            <Button variant="ghost" size="sm" onClick={() => newItem(true)} disabled={!inFolder || !!find} title="New folder (Ctrl+Shift+N)">
                <FolderPlus /> New folder
            </Button>
            <Divider />
            <Tool icon={Scissors} label="Cut (Ctrl+X)" disabled={!paths.length} onClick={() => copyToClipboard(paths, true)} />
            <Tool icon={Copy} label="Copy (Ctrl+C)" disabled={!paths.length} onClick={() => copyToClipboard(paths, false)} />
            <Tool icon={ClipboardPaste} label="Paste (Ctrl+V)" disabled={!inFolder || !!find} onClick={() => paste()} />
            <Tool icon={PenLine} label="Rename (F2)" disabled={selected.length !== 1} onClick={onRename} />
            <Tool icon={Trash2} label="Delete (Del) · Shift+Del deletes permanently" disabled={!paths.length} onClick={onDelete} />
            <Divider />
            <Tool icon={SquareTerminal} label="Open in Terminal" disabled={!inFolder} onClick={terminal} />

            <div className="ml-auto flex shrink-0 items-center gap-0.5">
                <Button
                    variant="ghost"
                    size="sm"
                    onClick={sizing ? stopSizes : calcSizes}
                    disabled={!inFolder || !!find}
                    title={sizing ? "Stop calculating" : "Size every folder here: from the instant search index when it has them, otherwise measured with dust"}
                    className="hidden sm:inline-flex"
                >
                    {sizing ? <Loader2 className="animate-spin" /> : <HardDrive />} {sizing ? "Stop" : "Folder sizes"}
                </Button>
                <Button variant="ghost" size="sm" onClick={onSearchText} disabled={!inFolder} title="Search inside the files in this folder (ripgrep) · Ctrl+Shift+F" className="hidden lg:inline-flex">
                    <TextSearch /> Search text
                </Button>
                <Divider />
                <Tool icon={showHidden ? Eye : EyeOff} label="Show hidden items (Ctrl+H)" pressed={showHidden} onClick={() => setSettings({ showHidden: !showHidden })} />
                <Tool icon={PanelRight} label="Preview pane (Alt+P)" pressed={showPreviewPane} onClick={() => setSettings({ showPreviewPane: !showPreviewPane })} />
            </div>
        </div>
    );
}
