import { useEffect } from "react";
import { create } from "zustand";
import { Keyboard, X } from "lucide-react";
import { Modal } from "./Modal";
import { Button } from "./ui/button";

const useShortcuts = create<{ open: boolean }>(() => ({ open: false }));

/** F1, Ctrl+/ or the palette: every shortcut on one sheet. */
export const openShortcuts = () => useShortcuts.setState({ open: true });
const close = () => useShortcuts.setState({ open: false });

const GROUPS: [string, [string, string][]][] = [
    [
        "Files",
        [
            ["Enter", "Open"],
            ["Space", "Quick Look"],
            ["Ctrl+K", "Command palette: folders and commands"],
            ["Ctrl+F", "Search this folder; Enter searches subfolders too"],
            ["Ctrl+L", "Type a path"],
            ["Alt+← → ↑", "Back, forward, up"],
            ["F2", "Rename (several selected: all at once)"],
            ["Delete", "To the Recycle Bin (Shift: for good)"],
            ["Ctrl+Z", "Undo"],
            ["Ctrl+C X V", "Copy, cut, paste"],
            ["Ctrl+Shift+C", "Copy the path"],
            ["Ctrl+Shift+N", "New folder"],
            ["Alt+Enter", "Properties"],
            ["Ctrl+H", "Hidden items"],
            ["Alt+P", "Preview pane"],
            ["Ctrl+Shift+2 / 6", "Grid / list"],
            ["F5", "Refresh"],
        ],
    ],
    [
        "Tabs and panes",
        [
            ["Ctrl+T", "New tab"],
            ["Ctrl+W", "Close the tab"],
            ["Ctrl+Shift+T", "Reopen the last closed tab"],
            ["Ctrl+Tab", "Next tab (Shift: previous)"],
            ["Ctrl+1…9", "Go to a tab"],
            ["Ctrl+Enter", "Open in a new tab"],
            ["Ctrl+Shift+D", "Two panes"],
            ["Ctrl+Shift+M", "Compare the panes"],
            ["F6", "The other pane"],
        ],
    ],
    [
        "Search and more",
        [
            ["Ctrl+Shift+F", "Search inside the files here"],
            ["Ctrl+Shift+E", "Back to Files from Text"],
            ["Ctrl+Alt+F", "Quick search, from any app"],
            ["Ctrl+,", "Settings"],
            ["F1", "This sheet"],
        ],
    ],
];

export function ShortcutsDialog() {
    const open = useShortcuts((s) => s.open);
    useEffect(() => {
        if (!open) return;
        const onKey = (e: KeyboardEvent) => {
            if (e.key !== "Escape") return;
            e.stopPropagation();
            close();
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [open]);
    if (!open) return null;
    return (
        <Modal label="Keyboard shortcuts" onClose={close} className="max-h-[86vh] max-w-4xl">
            <div className="flex h-12 shrink-0 items-center gap-2.5 border-b px-4">
                <Keyboard className="size-4 text-primary" />
                <h2 className="flex-1 text-sm font-semibold">Keyboard shortcuts</h2>
                <Button variant="ghost" size="icon" className="size-7" onClick={close} title="Close (Esc)" aria-label="Close">
                    <X className="size-4" />
                </Button>
            </div>
            <div className="grid gap-x-8 gap-y-6 overflow-y-auto p-5 md:grid-cols-[3fr_2fr]">
                {GROUPS.map(([title, keys], i) => (
                    <section key={title} className={i === 0 ? "md:row-span-2" : undefined}>
                        <h3 className="mb-2 text-xs font-semibold text-muted-foreground">{title}</h3>
                        <dl className="grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-1.5 text-sm">
                            {keys.map(([key, what]) => (
                                <div key={key} className="contents">
                                    <dt className="text-right">
                                        <kbd className="whitespace-nowrap rounded-sm border bg-card px-1.5 py-0.5 font-sans text-xs">{key}</kbd>
                                    </dt>
                                    <dd className="text-muted-foreground">{what}</dd>
                                </div>
                            ))}
                        </dl>
                    </section>
                ))}
            </div>
        </Modal>
    );
}
