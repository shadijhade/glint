import { useEffect, useRef } from "react";
import { useShallow } from "zustand/react/shallow";
import { ChevronDown, Link2, Plus, SquareTerminal, X } from "lucide-react";
import { isTyping } from "@/lib/cd";
import { currentPath, navigate, useExplorer } from "@/lib/explorer";
import { baseName, samePath, THIS_PC } from "@/lib/paths";
import { useStore } from "@/lib/store";
import {
    cdActive,
    clipboardText,
    closeTerminal,
    type FitLike,
    knownShells,
    live,
    loadRenderer,
    newTerminal,
    type Renderer,
    type TermLike,
    type TermTab,
    termClose,
    termResize,
    termSpawn,
    termWrite,
    updateTerminal,
    useTerminals,
} from "@/lib/terminal";
import { cn } from "@/lib/utils";
import { useMenu } from "../Menu";
import { ResizeHandle } from "../ResizeHandle";

/** The folder on disk a terminal starts in or follows: the active pane's, or home from This PC. */
export function terminalFolder() {
    const path = currentPath();
    if (path !== THIS_PC) return path;
    return useExplorer.getState().known.find((k) => k.id === "home")?.path ?? "C:\\";
}

/** Windows Terminal's Campbell colors, and One Half Light for the light theme. */
const DARK = ["#0c0c0c", "#c50f1f", "#13a10e", "#c19c00", "#0037da", "#881798", "#3a96dd", "#cccccc", "#767676", "#e74856", "#16c60c", "#f9f1a5", "#3b78ff", "#b4009e", "#61d6d6", "#f2f2f2"];
const LIGHT = ["#383a42", "#e45649", "#50a14f", "#c18401", "#0184bc", "#a626a4", "#0997b3", "#fafafa", "#4f525e", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff"];
const NAMES = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

let probe: CanvasRenderingContext2D | null = null;
/** Any CSS color (oklch included) as #rrggbb, as the terminal's canvas takes it. */
function hex(color: string) {
    if (!probe) {
        const canvas = document.createElement("canvas");
        canvas.width = canvas.height = 1;
        probe = canvas.getContext("2d", { willReadFrequently: true });
    }
    if (!probe) return "#000000";
    probe.clearRect(0, 0, 1, 1);
    probe.fillStyle = "#000";
    probe.fillStyle = color;
    probe.fillRect(0, 0, 1, 1);
    const [r, g, b] = probe.getImageData(0, 0, 1, 1).data;
    return `#${[r, g, b].map((n) => n.toString(16).padStart(2, "0")).join("")}`;
}

/** The terminal's colors, from the app's: its background and text, the primary color for the cursor. */
function theme() {
    const style = getComputedStyle(document.documentElement);
    // The theme's variables hold bare HSL parts ("0 0% 14%").
    const css = (name: string) => hex(`hsl(${style.getPropertyValue(name).trim() || "0 0% 0%"})`);
    const dark = document.documentElement.classList.contains("dark");
    const ansi = dark ? DARK : LIGHT;
    const colors: Record<string, string> = {
        background: css("--card"),
        foreground: css("--foreground"),
        cursor: css("--primary"),
        cursorAccent: css("--card"),
        selectionBackground: dark ? "#3b4b6b" : "#c8d7f5",
    };
    NAMES.forEach((name, n) => {
        colors[name] = ansi[n];
        colors[`bright${name[0].toUpperCase()}${name.slice(1)}`] = ansi[n + 8];
    });
    return colors;
}

/** One terminal: its shell in a pseudo console, drawn by Ghostty's emulator (or xterm.js). */
function TerminalView({ tab, visible }: { tab: TermTab; visible: boolean }) {
    const ref = useRef<HTMLDivElement>(null);
    const name = useTerminals((s) => s.shells.find((x) => x.id === tab.shell)?.name ?? tab.shell);

    useEffect(() => {
        let disposed = false;
        let renderer: Renderer | null = null;
        let term: TermLike | null = null;
        let fit: FitLike | null = null;
        let id: number | null = null;
        let said = tab.cwd;
        const session = {
            id: null as number | null,
            shell: tab.shell,
            idle: false,
            send: (data: string) => {
                if (id !== null) void termWrite(id, data);
            },
            focus: () => term?.focus(),
        };
        live.set(tab.key, session);
        const copy = () => {
            if (!term?.hasSelection()) return false;
            void navigator.clipboard.writeText(term.getSelection());
            term.clearSelection();
            return true;
        };

        (async () => {
            renderer = await loadRenderer();
            if (disposed || !ref.current) return;
            term = renderer.create({ fontFamily: "'Cascadia Mono', 'Cascadia Code', Consolas, monospace", fontSize: 13, cursorBlink: true, scrollback: 5000, theme: theme() });
            fit = renderer.fit();
            term.loadAddon(fit);
            term.open(ref.current);
            fit.fit();
            renderer.keys(term, (e) => {
                if (e.type !== "keydown") return false;
                const ctrl = e.ctrlKey && !e.altKey && !e.metaKey;
                const key = e.key.toLowerCase();
                // Ctrl+` hides the pane; Ctrl+C copies what's selected (else it's ^C); Ctrl+V pastes.
                if (ctrl && e.code === "Backquote") return true;
                if (ctrl && key === "c" && (e.shiftKey || term?.hasSelection())) {
                    copy();
                    e.preventDefault();
                    return true;
                }
                if (ctrl && key === "v") {
                    e.preventDefault();
                    void clipboardText().then((text) => text && term?.paste(text));
                    return true;
                }
                return false;
            });
            term.onData((data) => {
                if (isTyping(data)) session.idle = false;
                if (id !== null) void termWrite(id, data);
            });
            id = await termSpawn(
                tab.shell,
                tab.cwd,
                term.cols,
                term.rows,
                (bytes) => term?.write(bytes),
                (event) => {
                    if (event.kind === "exit") {
                        updateTerminal(tab.key, { exited: event.code });
                        // Said dimmed, and no cursor left blinking.
                        term?.write(`\r\n\x1b[2m[${name} ended${event.code ? ` with ${event.code}` : ""}.]\x1b[0m\r\n\x1b[?25l`);
                        return;
                    }
                    const moved = !samePath(said, event.path);
                    said = event.path;
                    session.idle = true;
                    updateTerminal(tab.key, { cwd: event.path });
                    const s = useTerminals.getState();
                    if (!useStore.getState().settings.terminalFollow || !s.open || s.active !== tab.key) return;
                    const shown = currentPath();
                    // The shell went somewhere: the pane follows. The pane went somewhere while the shell was busy: now it follows.
                    if (moved && !samePath(shown, event.path)) void navigate(event.path);
                    else if (!moved && shown !== THIS_PC && !samePath(shown, event.path)) cdActive(shown);
                }
            );
            session.id = id;
            if (disposed) void termClose(id);
            else if (useTerminals.getState().active === tab.key) term.focus();
        })().catch((e) => term?.write(`\r\nCouldn't start ${name}: ${e}\r\n`));

        const observer = new ResizeObserver(() => {
            if (!fit || !term || !ref.current?.offsetParent) return;
            fit.fit();
            if (id !== null) void termResize(id, term.cols, term.rows);
        });
        if (ref.current) observer.observe(ref.current);
        // The app's theme switched: the terminal's colors follow.
        const recolor = new MutationObserver(() => {
            if (term) renderer?.recolor(term, theme());
        });
        recolor.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
        return () => {
            disposed = true;
            observer.disconnect();
            recolor.disconnect();
            live.delete(tab.key);
            if (id !== null) void termClose(id);
            term?.dispose();
        };
        // A tab keeps its shell and where it started.
    }, [tab.key]);

    useEffect(() => {
        if (visible) live.get(tab.key)?.focus();
    }, [visible, tab.key]);

    return (
        <div
            ref={ref}
            data-terminal
            // Keys typed here are the shell's, not the file list's (Ctrl+` aside, which hides the pane).
            onKeyDown={(e) => {
                if (!(e.ctrlKey && e.code === "Backquote")) e.stopPropagation();
            }}
            // Relative: ghostty-web's hidden input box sits at its top left.
            className={cn("relative min-h-0 flex-1 overflow-hidden px-2 pt-1 caret-transparent", !visible && "hidden")}
        />
    );
}

/** The terminal pane, under the file panes: terminal tabs, each a shell, following the folder shown. */
export function TerminalPanel() {
    const { open, tabs, active, shells } = useTerminals(useShallow((s) => ({ open: s.open, tabs: s.tabs, active: s.active, shells: s.shells })));
    const height = useStore((s) => s.settings.terminalHeight) ?? 260;
    const follow = useStore((s) => s.settings.terminalFollow);
    const setSettings = useStore((s) => s.setSettings);
    const folder = useExplorer((s) => {
        const t = s.tabs[(s.panes[s.activePane] ?? s.panes[0]).active];
        return t && !t.find && !t.listing?.archive ? t.path : THIS_PC;
    });
    const menu = useMenu();
    const drag = useRef({ from: 0, now: 0, panel: null as HTMLElement | null });

    useEffect(() => {
        void knownShells();
    }, []);
    // The folder shown went somewhere: the terminal follows, if it's at its prompt.
    useEffect(() => {
        if (open && follow && folder !== THIS_PC) cdActive(folder);
    }, [folder, follow, open, active]);

    if (!tabs.length) return null;
    const pickShell = (e: React.MouseEvent<HTMLButtonElement>) => {
        const at = e.currentTarget.getBoundingClientRect();
        menu.open(
            { clientX: at.left, clientY: at.bottom + 4 },
            shells.map((shell) => ({ label: shell.name, icon: SquareTerminal, onSelect: () => void newTerminal(terminalFolder(), shell.id) }))
        );
    };

    return (
        <div className={cn("relative flex shrink-0 flex-col border-t bg-card", !open && "hidden")} style={{ height }}>
            <ResizeHandle
                across
                label="Terminal height"
                className="-top-1"
                onStart={(bar) => {
                    const panel = bar.parentElement!;
                    drag.current = { from: panel.getBoundingClientRect().height, now: 0, panel };
                }}
                onDrag={(up) => {
                    const d = drag.current;
                    d.now = Math.round(Math.min(window.innerHeight - 220, Math.max(96, d.from + up)));
                    if (d.panel) d.panel.style.height = `${d.now}px`;
                }}
                onEnd={() => {
                    if (drag.current.now) setSettings({ terminalHeight: drag.current.now });
                }}
                onReset={() => setSettings({ terminalHeight: null })}
            />
            <div className="flex h-8 shrink-0 items-center gap-0.5 border-b bg-chrome/60 px-1.5 text-xs">
                <div role="tablist" aria-label="Terminals" className="flex min-w-0 items-center gap-0.5 overflow-x-auto">
                    {tabs.map((tab) => {
                        const shell = shells.find((s) => s.id === tab.shell)?.name ?? tab.shell;
                        return (
                            <div
                                key={tab.key}
                                role="tab"
                                aria-selected={tab.key === active}
                                title={`${shell} · ${tab.cwd}`}
                                onMouseDown={(e) => {
                                    if (e.button === 1) {
                                        e.preventDefault();
                                        closeTerminal(tab.key);
                                    }
                                }}
                                onClick={() => useTerminals.setState({ active: tab.key })}
                                className={cn(
                                    "group flex h-6 max-w-48 shrink-0 cursor-default items-center gap-1.5 rounded-[4px] pr-0.5 pl-2 text-muted-foreground hover:bg-foreground/[0.05] hover:text-foreground",
                                    tab.key === active && "bg-background text-foreground shadow-xs dark:bg-foreground/[0.1]",
                                    tab.exited !== null && "opacity-60"
                                )}
                            >
                                <SquareTerminal className="size-3.5 shrink-0" />
                                <span className="truncate">{baseName(tab.cwd) || tab.cwd}</span>
                                <button
                                    type="button"
                                    aria-label="Close this terminal"
                                    title="Close (ends its shell)"
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        closeTerminal(tab.key);
                                    }}
                                    className="grid size-4 place-items-center rounded-sm opacity-0 group-hover:opacity-100 hover:bg-foreground/10 aria-selected:opacity-100"
                                >
                                    <X className="size-3" />
                                </button>
                            </div>
                        );
                    })}
                </div>
                <button type="button" onClick={() => void newTerminal(terminalFolder())} title="New terminal here" className="grid size-6 shrink-0 place-items-center rounded-[4px] text-muted-foreground hover:bg-foreground/[0.05] hover:text-foreground">
                    <Plus className="size-3.5" />
                </button>
                {shells.length > 1 && (
                    <button type="button" onClick={pickShell} title="New terminal with another shell" className="grid h-6 w-4 shrink-0 place-items-center rounded-[4px] text-muted-foreground hover:bg-foreground/[0.05] hover:text-foreground">
                        <ChevronDown className="size-3" />
                    </button>
                )}
                <div className="ml-auto flex shrink-0 items-center gap-0.5">
                    <button
                        type="button"
                        aria-pressed={follow}
                        onClick={() => setSettings({ terminalFollow: !follow })}
                        title={follow ? "Following: a cd in the terminal opens that folder here, and a folder opened here is cd'd to (when the shell is at its prompt). Click to stop." : "Not following: click so the terminal and the folder shown go where the other goes"}
                        className={cn("flex h-6 items-center gap-1 rounded-[4px] px-1.5 text-muted-foreground hover:bg-foreground/[0.05] hover:text-foreground", follow && "text-primary")}
                    >
                        <Link2 className="size-3.5" /> Follow
                    </button>
                    <button type="button" onClick={() => useTerminals.setState({ open: false })} title="Hide the terminal (Ctrl+`): its shells keep running" className="grid size-6 place-items-center rounded-[4px] text-muted-foreground hover:bg-foreground/[0.05] hover:text-foreground">
                        <ChevronDown className="size-4" />
                    </button>
                </div>
            </div>
            {tabs.map((tab) => (
                <TerminalView key={tab.key} tab={tab} visible={open && tab.key === active} />
            ))}
            {menu.element}
        </div>
    );
}
