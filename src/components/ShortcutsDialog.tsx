import { create } from "zustand";
import { Keyboard, X } from "lucide-react";
import { useStore } from "@/lib/store";
import { Modal } from "./Modal";
import { Button } from "./ui/button";
import { Kbd } from "./ui/kbd";

const useShortcuts = create<{ open: boolean }>(() => ({ open: false }));

/** F1, Ctrl+/ or the palette: every shortcut on one sheet. */
export const openShortcuts = () => useShortcuts.setState({ open: true });
const close = () => useShortcuts.setState({ open: false });

/** The sheet's groups, with the quick search shortcut chosen in Settings (none when it's off). */
const groups = (quick: string | null): [string, [string, string][]][] => [
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
            ["Ctrl+Shift+R", "Rename several, with find and replace"],
            ["Delete", "To the Recycle Bin (Shift: for good)"],
            ["Ctrl+Z", "Undo"],
            ["Ctrl+A", "Select all"],
            ["Ctrl+C X V", "Copy, cut, paste"],
            ["Ctrl+Shift+C", "Copy the path"],
            ["Ctrl+Shift+N", "New folder"],
            ["Shift+F10", "The menu of what's selected (or the menu key)"],
            ["Alt+Enter", "Properties"],
            ["Ctrl+H", "Hidden items"],
            ["Alt+P", "Preview pane"],
            ["Ctrl+Shift+2 / 6", "Grid / list"],
            ["Ctrl+= − 0", "Grid: larger tiles, smaller, the usual size"],
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
        "Text",
        [
            ["Enter", "Search, or open the result picked"],
            ["↓ ↑", "From the search box on through the results"],
            ["Shift+Enter", "A new line: search for several lines at once"],
            ["Alt+C / W / R", "Match case, whole words, regular expression"],
            ["Ctrl+H", "Replace"],
            ["Esc", "Leave the search box; again, stop the search"],
        ],
    ],
    [
        "Anywhere",
        [
            ["Ctrl+Shift+F", "Search inside the files here"],
            ["Ctrl+Shift+E", "Back to Files from Text"],
            ...(quick ? ([[quick, "Quick search, from any app"]] as [string, string][]) : []),
            ["Ctrl+,", "Settings"],
            ["F1", "This sheet"],
        ],
    ],
];

export function ShortcutsDialog() {
    const open = useShortcuts((s) => s.open);
    const quick = useStore((s) => s.settings.quickShortcut);
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
                {groups(quick).map(([title, keys], i) => (
                    <section key={title} className={i === 0 ? "md:row-span-3" : undefined}>
                        <h3 className="mb-2 text-xs font-semibold text-muted-foreground">{title}</h3>
                        <dl className="grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-1.5 text-sm">
                            {keys.map(([key, what]) => (
                                <div key={key} className="contents">
                                    <dt className="text-right">
                                        <Kbd className="text-xs text-foreground">{key}</Kbd>
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
