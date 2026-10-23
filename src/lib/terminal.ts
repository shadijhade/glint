import { Channel, invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { cdCommand } from "./cd";
import { useStore } from "./store";

/** A shell the terminal pane can run (term.rs). */
export interface Shell {
    id: string;
    name: string;
    path: string;
}

/** What happens in a session besides its output: its prompt said where it is, or it ended. */
export type TermEvent = { kind: "cwd"; path: string } | { kind: "exit"; code: number };

export const termShells = () => invoke<Shell[]>("term_shells");

/** Starts `shell` in `cwd`; its output comes to `onOutput`, the rest to `onEvent`. Resolves to its id. */
export function termSpawn(shell: string, cwd: string, cols: number, rows: number, onOutput: (data: Uint8Array) => void, onEvent: (event: TermEvent) => void) {
    const channel = new Channel<ArrayBuffer | TermEvent>();
    channel.onmessage = (message) => (message instanceof ArrayBuffer ? onOutput(new Uint8Array(message)) : onEvent(message));
    return invoke<number>("term_spawn", { shell, cwd, cols, rows, onOutput: channel });
}

export const termWrite = (id: number, data: string) => invoke("term_write", { id, data });
export const termResize = (id: number, cols: number, rows: number) => invoke("term_resize", { id, cols, rows });
export const termClose = (id: number) => invoke("term_close", { id });
export const clipboardText = () => invoke<string | null>("clipboard_text");

/** What both terminal renderers offer (ghostty-web keeps xterm.js's API). */
export interface TermLike {
    cols: number;
    rows: number;
    options: { theme?: Record<string, string>; fontSize?: number };
    open(parent: HTMLElement): void;
    write(data: string | Uint8Array): void;
    paste(data: string): void;
    focus(): void;
    dispose(): void;
    loadAddon(addon: unknown): void;
    hasSelection(): boolean;
    getSelection(): string;
    clearSelection(): void;
    onData(listener: (data: string) => void): { dispose(): void };
    onTitleChange(listener: (title: string) => void): { dispose(): void };
    attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean): void;
}

export interface FitLike {
    fit(): void;
}

export interface Renderer {
    /** "Ghostty" (its VT parser, from Zig, as WebAssembly) or "xterm.js". */
    name: string;
    create(options: Record<string, unknown>): TermLike;
    fit(): FitLike;
    /** Keys `ours` returns true for are the app's: the shell never gets them. */
    keys(term: TermLike, ours: (event: KeyboardEvent) => boolean): void;
    /** New colors for a terminal already open. */
    recolor(term: TermLike, theme: Record<string, string>): void;
}

let renderer: Promise<Renderer> | null = null;

/** The terminal renderer, loaded the first time a terminal opens: ghostty-web, or xterm.js if that can't start. */
export function loadRenderer(): Promise<Renderer> {
    renderer ??= (async () => {
        try {
            const ghostty = await import("ghostty-web");
            await ghostty.init();
            return {
                name: "Ghostty",
                create: (options) => new ghostty.Terminal(options) as unknown as TermLike,
                fit: () => new ghostty.FitAddon() as unknown as FitLike,
                // ghostty-web skips the keys its handler returns true for: the reverse of xterm.js.
                keys: (term, ours) => term.attachCustomKeyEventHandler(ours),
                // Its emulator fixes the colors in each cell as it's written, so a terminal keeps the ones it opened with.
                recolor: () => {},
            };
        } catch (e) {
            console.warn("ghostty-web didn't start; xterm.js draws the terminal", e);
            const [xterm, fit] = await Promise.all([import("@xterm/xterm"), import("@xterm/addon-fit"), import("@xterm/xterm/css/xterm.css")]);
            return {
                name: "xterm.js",
                create: (options) => new xterm.Terminal(options) as unknown as TermLike,
                fit: () => new fit.FitAddon() as unknown as FitLike,
                keys: (term, ours) => term.attachCustomKeyEventHandler((e) => !ours(e)),
                recolor: (term, theme) => {
                    term.options.theme = theme;
                },
            };
        }
    })();
    return renderer;
}

/** A terminal tab in the pane. */
export interface TermTab {
    key: string;
    shell: string;
    /** Where its prompt last said it is (where it started, till then). */
    cwd: string;
    title: string;
    /** How it ended (null: still running). */
    exited: number | null;
    /** What's typed at its prompt once its shell first says where it is (a project's task, a script), Enter included. */
    run?: string;
}

interface TerminalState {
    open: boolean;
    tabs: TermTab[];
    active: string | null;
    shells: Shell[];
}

export const useTerminals = create<TerminalState>(() => ({ open: false, tabs: [], active: null, shells: [] }));
const set = useTerminals.setState;
const get = useTerminals.getState;

/** A running session, as its view keeps it: what the pane's following needs of it. */
export interface Live {
    id: number | null;
    shell: string;
    /** At its prompt with nothing typed since: a cd can go in. */
    idle: boolean;
    send(data: string): void;
    focus(): void;
}

/** Sessions by tab key. */
export const live = new Map<string, Live>();

let shellsAsked: Promise<Shell[]> | null = null;

/** The shells on this PC, asked once. */
export function knownShells() {
    shellsAsked ??= termShells().then(
        (shells) => {
            set({ shells });
            return shells;
        },
        () => []
    );
    return shellsAsked;
}

/** The shell new terminals use: the one chosen in Settings, else the first found. */
export async function defaultShell() {
    const shells = await knownShells();
    const chosen = useStore.getState().settings.terminalShell;
    return shells.find((s) => s.id === chosen)?.id ?? shells[0]?.id ?? "cmd";
}

let next = 0;

/** A new terminal tab in `cwd`, with `shell` (the default one otherwise), shown. */
export async function newTerminal(cwd: string, shell?: string) {
    const id = shell ?? (await defaultShell());
    const name = get().shells.find((s) => s.id === id)?.name ?? id;
    const tab: TermTab = { key: `t${++next}`, shell: id, cwd, title: name, exited: null };
    set((s) => ({ tabs: [...s.tabs, tab], active: tab.key, open: true }));
}

/**
 * Runs `line` (as `shell` takes it, built for it with `runLine`) in a new terminal tab in `cwd`, named
 * `title`, shown: typed at its prompt once its shell is there. Not in WSL, whose tools aren't Windows'.
 */
export async function runInTerminal(cwd: string, line: (shell: string) => string, title: string) {
    const shells = await knownShells();
    const preferred = await defaultShell();
    const shell = preferred === "wsl" ? (shells.find((s) => s.id !== "wsl")?.id ?? "cmd") : preferred;
    const tab: TermTab = { key: `t${++next}`, shell, cwd, title, exited: null, run: line(shell) };
    set((s) => ({ tabs: [...s.tabs, tab], active: tab.key, open: true }));
}

export function closeTerminal(key: string) {
    const s = get();
    const at = s.tabs.findIndex((t) => t.key === key);
    const tabs = s.tabs.filter((t) => t.key !== key);
    const active = s.active === key ? (tabs[Math.min(at, tabs.length - 1)]?.key ?? null) : s.active;
    set({ tabs, active, open: s.open && tabs.length > 0 });
}

export function updateTerminal(key: string, change: Partial<TermTab>) {
    set((s) => ({ tabs: s.tabs.map((t) => (t.key === key ? { ...t, ...change } : t)) }));
}

/** Shows or hides the pane (Ctrl+`); shown with none, it starts one in `cwd`. */
export function toggleTerminal(cwd: string) {
    const s = get();
    if (s.open) return set({ open: false });
    if (!s.tabs.length) return void newTerminal(cwd);
    set({ open: true });
    if (s.active) setTimeout(() => live.get(s.active!)?.focus(), 0);
}

/** Takes the active terminal to `folder`, if it's at its prompt with nothing typed. */
export function cdActive(folder: string) {
    const s = get();
    const tab = s.tabs.find((t) => t.key === s.active);
    const session = tab && live.get(tab.key);
    if (!tab || !session?.idle || tab.exited !== null) return;
    if (tab.cwd.replace(/\\+$/, "").toLowerCase() === folder.replace(/\\+$/, "").toLowerCase()) return;
    session.idle = false;
    session.send(cdCommand(tab.shell, folder));
}
