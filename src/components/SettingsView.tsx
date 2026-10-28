import { ReactNode, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Archive, Binary, ClipboardCopy, Download, ExternalLink, Folder, Heart, Image, Keyboard, Loader2, Lock, Plus, RefreshCw, RotateCcw, ShieldCheck, Sparkles, Type, X, Zap } from "lucide-react";
import logo from "@/assets/logo.svg";
import { disableIndex, enableIndex, refreshIndex, serviceOutdated, useExplorer } from "@/lib/explorer";
import { formatCount, formatDuration, formatSize } from "@/lib/format";
import { diagnostics, type ExplorerIntegration, explorerIntegration, type IndexVolume, type McpSetup, mcpSetup, setExplorerIntegration, type ToolProgress, type ToolStatus } from "@/lib/fs";
import { TILE_SIZES, type TileSize } from "@/lib/grid";
import { clearWithUndo, DEFAULT_SETTINGS, useStore } from "@/lib/store";
import { knownShells, useTerminals } from "@/lib/terminal";
import { cancelInstall, describeProgress, installTool, refreshTools, removeTool, useTool, useTools } from "@/lib/tools";
import { checkForUpdates, installUpdate, useUpdater } from "@/lib/updater";
import { cn } from "@/lib/utils";
import { cancelMeaning, installMeaning, refreshMeaning, removeMeaning, useMeaning } from "@/lib/meaning";
import { refreshZoxide } from "@/lib/zoxide";
import { refreshOpeners, useOpeners } from "@/lib/openers";
import { openShortcuts } from "./ShortcutsDialog";
import { installHint } from "./ToolNeeded";
import { Button } from "./ui/button";
import { Input } from "./ui/input";

const REPO_URL = "https://github.com/shadijhade/glint";

const PRESETS = [
    { label: "Folders", icon: Folder, items: ["node_modules", ".git", "dist", "build", "bin", "obj", ".vs", ".idea", "__pycache__", ".cache", "vendor", "packages", ".nuget", "coverage", ".next", "target"] },
    { label: "Binaries", icon: Binary, items: ["*.exe", "*.dll", "*.so", "*.dylib", "*.pdb", "*.lib", "*.a", "*.o", "*.class", "*.pyc"] },
    { label: "Images", icon: Image, items: ["*.png", "*.jpg", "*.jpeg", "*.gif", "*.ico", "*.svg", "*.webp", "*.bmp", "*.tiff", "*.psd"] },
    { label: "Archives", icon: Archive, items: ["*.zip", "*.rar", "*.7z", "*.tar", "*.gz", "*.bz2", "*.xz", "*.iso"] },
    { label: "Fonts", icon: Type, items: ["*.woff", "*.woff2", "*.ttf", "*.eot", "*.otf"] },
    { label: "Lock files", icon: Lock, items: ["package-lock.json", "yarn.lock", "pnpm-lock.yaml", "Cargo.lock", "Gemfile.lock", "composer.lock", "poetry.lock"] },
];

/** A section's id, from its title: "settings-about" for About. */
const sectionId = (title: string) => `settings-${title.toLowerCase().replace(/[^a-z]+/g, "-").replace(/^-|-$/g, "")}`;

/** The sections in order, as the column on the left lists them (title, and a shorter name). */
const SECTIONS: [string, string][] = [
    ["Appearance", "Appearance"],
    ["Files", "Files"],
    ["Instant search", "Instant search"],
    ["Find files by name", "Find files"],
    ["Quick search", "Quick search"],
    ["AI agents", "AI agents"],
    ["Search text (ripgrep)", "Search text"],
    ["Text search skips", "Text search skips"],
    ["Terminal", "Terminal"],
    ["Check", "Check"],
    ["Tools", "Tools"],
    ["History", "History"],
    ["About", "About"],
];

/** How AI agents that speak the Model Context Protocol start Glint's server (mcp.rs), for each to be told. */
const AGENTS: [string, string, keyof McpSetup][] = [
    ["Claude Code", "Run this in a terminal", "claudeCode"],
    ["VS Code", "Run this in PowerShell: Copilot's agent mode can then use Glint", "vscode"],
    ["Claude Desktop, Cursor and others", "Add this to the mcpServers in their settings", "json"],
];

function AgentsSection() {
    const [setup, setSetup] = useState<McpSetup | null>(null);
    useEffect(() => {
        mcpSetup().then(setSetup, () => setSetup(null));
    }, []);
    const copy = (text: string, what: string) =>
        navigator.clipboard.writeText(text).then(
            () => toast.success(`Copied ${what}`, { duration: 1500 }),
            () => toast.error(`Couldn't copy ${what}`)
        );
    return (
        <Section title="AI agents">
            <p className="px-4 py-3 text-xs text-muted-foreground">
                AI agents that speak the Model Context Protocol can ask Glint to find files by name on every drive in milliseconds (with instant search on), measure folders, list the biggest files, say what was moved or deleted lately and search text inside files.
                Glint's tools only read. Once told how, the agent starts Glint for itself, and sees the names and text it asks for.
            </p>
            {AGENTS.map(([label, hint, key]) => (
                <div key={key} className="px-4 py-3">
                    <div className="flex items-center justify-between gap-4">
                        <div className="min-w-0">
                            <div className="text-sm">{label}</div>
                            <div className="mt-0.5 text-xs text-muted-foreground">{hint}</div>
                        </div>
                        <Button variant="outline" size="sm" disabled={!setup} onClick={() => setup && copy(setup[key], `${label}'s setup`)}>
                            <ClipboardCopy /> Copy
                        </Button>
                    </div>
                    {setup && <pre className="mt-2 select-text overflow-x-auto rounded-sm bg-muted px-2 py-1.5 font-mono text-[0.6875rem] text-muted-foreground">{setup[key]}</pre>}
                </div>
            ))}
        </Section>
    );
}

function Section({ title, action, children }: { title: string; action?: ReactNode; children: ReactNode }) {
    return (
        <section id={sectionId(title)} className="scroll-mt-5">
            <div className="mb-2 flex h-7 items-center justify-between px-1">
                <h3 className="text-sm font-semibold">{title}</h3>
                {action}
            </div>
            <div className="divide-y rounded-lg border bg-card">{children}</div>
        </section>
    );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
    return (
        // In a narrow window, what's chosen goes under the label rather than squeezing it.
        <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2 px-4 py-3">
            <div className="min-w-48 flex-1">
                <div className="text-sm">{label}</div>
                {hint && <div className="mt-0.5 text-xs text-muted-foreground">{hint}</div>}
            </div>
            <div className="flex shrink-0 flex-wrap items-center gap-1.5">{children}</div>
        </div>
    );
}

/** A labelled on/off row; the whole row toggles. */
function SwitchRow({ label, hint, checked, disabled, onChange }: { label: string; hint?: string; checked: boolean; disabled?: boolean; onChange: (value: boolean) => void }) {
    return (
        <label className={cn("flex items-center justify-between gap-6 px-4 py-3", disabled ? "cursor-default" : "cursor-pointer")}>
            <div className="min-w-0">
                <div className={cn("text-sm", disabled && "text-muted-foreground")}>{label}</div>
                {hint && <div className="mt-0.5 text-xs text-muted-foreground">{hint}</div>}
            </div>
            <input
                type="checkbox"
                role="switch"
                checked={checked}
                disabled={disabled}
                onChange={(e) => onChange(e.target.checked)}
                className="relative h-5 w-9 shrink-0 cursor-pointer appearance-none rounded-full bg-input transition-colors before:absolute before:left-0.5 before:top-0.5 before:size-4 before:rounded-full before:bg-white before:shadow-xs before:transition-transform checked:bg-primary checked:before:translate-x-4 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default disabled:opacity-50"
            />
        </label>
    );
}

function Segmented<T extends string | number | null>({ value, options, onChange }: { value: T; options: [T, string][]; onChange: (value: T) => void }) {
    return (
        <div className="inline-flex rounded-md bg-foreground/[0.05] p-0.5">
            {options.map(([option, label]) => (
                <button
                    key={label}
                    type="button"
                    aria-pressed={option === value}
                    onClick={() => onChange(option)}
                    className={cn(
                        "rounded-sm px-2.5 py-1 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground",
                        option === value && "bg-background text-foreground shadow-xs dark:bg-foreground/[0.12]"
                    )}
                >
                    {label}
                </button>
            ))}
        </div>
    );
}

/** Add-a-pattern input plus removable chips. */
function PatternEditor({ value, onChange, placeholder }: { value: string[]; onChange: (value: string[]) => void; placeholder: string }) {
    const [pattern, setPattern] = useState("");
    const add = () => {
        const p = pattern.trim();
        if (p && !value.includes(p)) onChange([...value, p]);
        setPattern("");
    };
    return (
        <>
            <div className="flex gap-2">
                <Input
                    value={pattern}
                    onChange={(e) => setPattern(e.target.value)}
                    onKeyDown={(e) => e.key === "Enter" && add()}
                    placeholder={placeholder}
                    className="font-mono placeholder:font-sans"
                />
                <Button size="sm" variant="outline" onClick={add} disabled={!pattern.trim()}>
                    <Plus /> Add
                </Button>
            </div>
            <div className="flex max-h-44 flex-wrap gap-1.5 overflow-y-auto">
                {value.length === 0 && <p className="text-sm text-muted-foreground">Nothing is excluded.</p>}
                {value.map((ex) => (
                    <span key={ex} className="inline-flex items-center gap-1 rounded-md border bg-background py-0.5 pl-2 pr-0.5 font-mono text-xs">
                        {ex}
                        <button
                            type="button"
                            onClick={() => onChange(value.filter((e) => e !== ex))}
                            className="grid size-5 place-items-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground"
                            aria-label={`Stop excluding ${ex}`}
                        >
                            <X className="size-3" />
                        </button>
                    </span>
                ))}
            </div>
        </>
    );
}

const copyDiagnostics = () =>
    diagnostics()
        .then((text) => navigator.clipboard.writeText(text))
        .then(
            () => toast.success("Diagnostics copied", { description: "Paste them into your bug report." }),
            (e) => toast.error("Couldn't copy diagnostics", { description: String(e) })
        );

function describeVolume(v: IndexVolume) {
    if (v.state === "scanning") return `${v.volume} reading${v.progressTotal ? ` ${Math.floor((100 * v.progress) / v.progressTotal)}%` : ""}…`;
    if (v.state === "error") return `${v.volume} unavailable${v.error ? ` (${v.error})` : ""}`;
    return `${v.volume} ${formatCount(v.files + v.folders)} items`;
}

/** Turning the MFT index service on and off, and how it's doing. */
function InstantSearch() {
    const index = useExplorer((s) => s.index);
    const [working, setWorking] = useState(false);
    useEffect(() => {
        refreshIndex();
    }, []);
    const run = async (action: () => Promise<void>) => {
        setWorking(true);
        try {
            await action();
        } finally {
            setWorking(false);
        }
    };
    const state = index?.state ?? "off";
    const volumes = index?.volumes ?? [];
    const outdated = serviceOutdated(index);
    const details = volumes
        .map((v) => (v.state === "ready" ? `${v.volume} ${formatSize(v.memory)} in memory, built from ${v.source === "scan" ? "the MFT" : "its snapshot"} in ${formatDuration(v.buildMs)}` : `${v.volume} ${v.error ?? v.state}`))
        .join("\n");
    const hint = {
        on: volumes.length ? volumes.map(describeVolume).join(" · ") : "Starting…",
        stopped: "Installed, but the service isn't running. Turning it on starts it again.",
        noAccess: "Another account on this PC turned it on. Allow yours too to use it.",
        untrusted: index?.error ?? "Something else is using the service's name, so it isn't trusted.",
        unsupported: "Needs Windows and NTFS drives.",
        off: "Searches whole drives by name in milliseconds, and shows every folder's size as it opens, like Everything and WizTree.",
    }[state];
    const enable = { on: "Update", stopped: "Start", noAccess: "Allow me", untrusted: "Turn on", unsupported: "Turn on", off: "Turn on" }[state];
    return (
        <Section title="Instant search">
            <div className="flex items-center justify-between gap-6 px-4 py-3" title={details || undefined}>
                <div className="flex min-w-0 items-start gap-3">
                    <Zap className={cn("mt-0.5 size-4 shrink-0", state === "on" ? "text-primary" : "text-muted-foreground")} />
                    <div className="min-w-0">
                        <div className="text-sm">{state === "on" ? (outdated ? "On · an update for the service is ready" : "On") : "Index file names and folder sizes"}</div>
                        <div className="mt-0.5 text-xs text-muted-foreground">{hint}</div>
                    </div>
                </div>
                <div className="flex shrink-0 items-center gap-1.5">
                    {working && <Loader2 className="size-4 animate-spin text-muted-foreground" />}
                    {(state !== "on" || outdated) && state !== "unsupported" && (
                        <Button size="sm" disabled={working} onClick={() => run(enableIndex)} title="Windows asks for administrator approval">
                            <ShieldCheck /> {enable}
                        </Button>
                    )}
                    {(state === "on" || state === "stopped") && (
                        <Button variant="outline" size="sm" disabled={working} onClick={() => run(disableIndex)} title="Windows asks for administrator approval">
                            Turn off
                        </Button>
                    )}
                </div>
            </div>
            <p className="px-4 py-3 text-xs text-muted-foreground">
                Turning it on asks for administrator approval once: a small Windows service reads your NTFS drives' file tables, follows their change
                journals, and answers only your account and administrators. Like Everything, it knows the name, size and date of every file on
                those drives, including files your account can't open. Turning it off removes the service and its index files.
            </p>
        </Section>
    );
}

/** One program Glint drives: its version, where it came from, and Install, Stop or Remove. */
function ToolRow({ tool, progress }: { tool: ToolStatus; progress?: ToolProgress }) {
    const where = { bundled: "Comes with Glint", downloaded: "Downloaded by Glint", installed: "Installed on this PC" }[tool.origin ?? "bundled"];
    const outdated = tool.origin === "downloaded" && tool.pinned && tool.version !== tool.pinned;
    return (
        <Row label={tool.version ? `${tool.name} ${tool.version}` : tool.name} hint={`${tool.usedFor} · ${tool.origin ? where : "Not installed"}`}>
            {progress ? (
                <>
                    <span className="max-w-56 truncate text-xs text-muted-foreground" title={describeProgress(progress)}>
                        {describeProgress(progress)}
                    </span>
                    <Button variant="outline" size="sm" onClick={() => cancelInstall(tool.id)}>
                        Stop
                    </Button>
                </>
            ) : (
                <>
                    {(!tool.origin || outdated) && (
                        <Button size="sm" onClick={() => void installTool(tool.id)} title={installHint(tool)}>
                            <Download /> {tool.origin ? `Update to ${tool.pinned}` : "Install"}
                        </Button>
                    )}
                    {tool.removable && (
                        <Button variant="outline" size="sm" onClick={() => void removeTool(tool.id)} title={tool.path ?? undefined}>
                            Remove
                        </Button>
                    )}
                </>
            )}
        </Row>
    );
}

/** The code editors found on this PC, to open files with: picking one fills in its path. */
function EditorPicker({ value, onPick }: { value: string; onPick: (path: string) => void }) {
    const editors = useOpeners((s) => s.list).filter((o) => !o.terminal);
    useEffect(() => refreshOpeners(), []);
    if (!editors.length) return null;
    const chosen = editors.find((e) => e.path.toLowerCase() === value.trim().toLowerCase());
    return (
        <select
            value={chosen?.id ?? ""}
            onChange={(e) => {
                const editor = editors.find((o) => o.id === e.target.value);
                if (editor) onPick(editor.path);
            }}
            aria-label="An editor found on this PC"
            className="h-8 rounded-md border bg-background px-2 text-sm outline-hidden focus:border-primary"
        >
            <option value="" disabled>
                Found on this PC…
            </option>
            {editors.map((e) => (
                <option key={e.id} value={e.id}>
                    {e.name}
                </option>
            ))}
        </select>
    );
}

/** Meaning search's model, among the tools: downloaded or not, and Download, Stop or Remove. */
function MeaningRow() {
    const { installed, progress } = useMeaning();
    useEffect(() => {
        void refreshMeaning();
    }, []);
    return (
        <Row label="Meaning model (potion-base-8M)" hint={`Meaning search in Text: passages ranked by what they're about (Model2Vec, 30 MB, MIT) · ${installed ? "Downloaded by Glint" : "Not downloaded"}`}>
            {progress ? (
                <>
                    <span className="max-w-56 truncate text-xs text-muted-foreground">{describeProgress(progress)}</span>
                    <Button variant="outline" size="sm" onClick={cancelMeaning}>
                        Stop
                    </Button>
                </>
            ) : installed ? (
                <Button variant="outline" size="sm" onClick={() => void removeMeaning()}>
                    Remove
                </Button>
            ) : (
                <Button size="sm" onClick={() => void installMeaning()} title="Downloads it from Hugging Face and checks it against its pinned checksum">
                    <Download /> Download
                </Button>
            )}
        </Row>
    );
}

/** The programs Glint drives: the ones that come with it, and the optional ones it installs when a feature needs them. */
function ToolsSection() {
    const tools = useTools((s) => s.tools);
    const busy = useTools((s) => s.busy);
    const [checking, setChecking] = useState(false);
    const check = async () => {
        setChecking(true);
        await refreshTools();
        setChecking(false);
    };
    useEffect(() => {
        void check();
    }, []);
    const list = Object.values(tools);
    return (
        <Section
            title="Tools"
            action={
                <Button variant="ghost" size="sm" className="h-7" disabled={checking} onClick={() => void check()} title="Look for each tool again">
                    {checking ? <Loader2 className="animate-spin" /> : <RefreshCw />} Refresh
                </Button>
            }
        >
            {!list.length && <p className="px-4 py-3 text-sm text-muted-foreground">Looking for them…</p>}
            {list.map((tool) => (
                <ToolRow key={tool.id} tool={tool} progress={busy[tool.id]} />
            ))}
            <MeaningRow />
            <p className="px-4 py-3 text-xs text-muted-foreground">
                ripgrep, fd and pdu come with Glint. Other tools install when a feature needs them, for your account only: with winget, or from the
                tool's own GitHub release, which Glint checks against the checksum it knows for it before unpacking anything.
            </p>
        </Section>
    );
}

/** The terminal under the files (Ctrl+`): the shell new ones start, and whether they follow the folder shown. */
function TerminalSection() {
    const shells = useTerminals((s) => s.shells);
    const settings = useStore((s) => s.settings);
    const set = useStore((s) => s.setSettings);
    useEffect(() => {
        void knownShells();
    }, []);
    return (
        <Section title="Terminal">
            <Row label="Shell" hint="New terminals start it. Ctrl+` opens one under the files, in the folder shown">
                <select
                    value={shells.find((s) => s.id === settings.terminalShell)?.id ?? shells[0]?.id ?? ""}
                    onChange={(e) => set({ terminalShell: e.target.value })}
                    className="h-8 rounded-md border bg-background px-2 text-sm outline-hidden focus:border-primary"
                >
                    {shells.map((shell) => (
                        <option key={shell.id} value={shell.id}>
                            {shell.name}
                        </option>
                    ))}
                </select>
            </Row>
            <SwitchRow
                label="Follow the folder shown"
                hint="A cd in the terminal opens that folder here, and a folder opened here is cd'd to while the shell waits at its prompt"
                checked={settings.terminalFollow}
                onChange={(terminalFollow) => set({ terminalFollow })}
            />
        </Section>
    );
}

/** Checking files: the YARA rules they're checked with. */
function CheckSection() {
    const rules = useStore((s) => s.settings.yaraRules);
    const set = useStore((s) => s.setSettings);
    const browse = async () => {
        const picked = await open({ directory: true, title: "A folder of YARA rules (.yar, .yara)", defaultPath: rules || undefined });
        if (typeof picked === "string") set({ yaraRules: picked });
    };
    return (
        <Section title="Check">
            <Row label="YARA rules" hint="A folder of .yar and .yara files (its subfolders too), for Check with YARA rules: YARA-X compiles them each time">
                <Input className="w-52" value={rules} onChange={(e) => set({ yaraRules: e.target.value })} placeholder="None chosen" />
                <Button variant="outline" size="sm" onClick={() => void browse()}>
                    Browse…
                </Button>
            </Row>
        </Section>
    );
}

/** File Explorer's "Open in Glint", and opening folders here when they're double-clicked. */
function FileExplorerRows() {
    const [state, setState] = useState<ExplorerIntegration | null>(null);
    const [working, setWorking] = useState(false);
    useEffect(() => {
        explorerIntegration().then(setState).catch(() => {});
    }, []);
    const change = async (menu: boolean, byDefault: boolean) => {
        setWorking(true);
        try {
            setState(await setExplorerIntegration(menu, byDefault));
        } catch (e) {
            toast.error("Couldn't change File Explorer's settings", { description: String(e) });
        } finally {
            setWorking(false);
        }
    };
    const { available = false, menu = false, byDefault = false } = state ?? {};
    const disabled = !available || working;
    const installed = "Needs Glint installed with its setup program or Scoop";
    return (
        <>
            <SwitchRow
                label="Open in Glint from File Explorer"
                hint={available ? "On the right-click menu of folders, drives and empty space in a folder (Show more options on Windows 11)" : installed}
                checked={menu}
                disabled={disabled}
                onChange={(on) => change(on, on && byDefault)}
            />
            <SwitchRow
                label="Open folders in Glint by default"
                hint={available ? "Double-clicking a folder in File Explorer or on the desktop opens it here. Drives, This PC, the Recycle Bin and libraries stay in File Explorer." : installed}
                checked={byDefault}
                disabled={disabled}
                onChange={(on) => change(menu || on, on)}
            />
        </>
    );
}

/** A new version with its Install button, or a way to look for one. */
function UpdateRow() {
    const { available, status, error, progress } = useUpdater();
    if (available) {
        const installing = progress !== null;
        return (
            <Row label={`Version ${available.version} is available`} hint={available.update ? "Installing it restarts the app" : "Download the installer from GitHub"}>
                <Button variant="ghost" size="sm" onClick={() => openUrl(available.url)}>
                    What's new
                </Button>
                <Button size="sm" disabled={installing} onClick={installUpdate}>
                    {installing ? <Loader2 className="animate-spin" /> : <Sparkles />}
                    {installing ? `Downloading ${progress}%` : available.update ? "Install" : "Download"}
                </Button>
            </Row>
        );
    }
    // Offline, a fetch fails with words like "Failed to fetch": say that plainly.
    const offline = /fetch|network|sending request|dns|timed? ?out|connect/i.test(error ?? "");
    const hint = { idle: undefined, checking: "Checking…", latest: "You have the latest version", failed: offline ? "Couldn't reach GitHub to check. Are you online?" : `Couldn't check: ${error}` }[status];
    return (
        <Row label="Updates" hint={hint}>
            <Button variant="outline" size="sm" disabled={status === "checking"} onClick={() => checkForUpdates(true)}>
                {status === "checking" ? <Loader2 className="animate-spin" /> : <RefreshCw />} Check for updates
            </Button>
        </Row>
    );
}

interface SettingsViewProps {
    onClose: () => void;
    /** The quick search shortcut, when another app holds it. */
    quickTaken: string | null;
    /** The section to open at ("settings-about"); the top otherwise. */
    section?: string;
}

export function SettingsView({ onClose, quickTaken, section }: SettingsViewProps) {
    const { settings, setSettings: set, history } = useStore();
    const visited = useStore((s) => Object.keys(s.visits).length);
    const zoxide = useTool("zoxide");
    const [version, setVersion] = useState("");
    // The section at the top of the scroll, marked on the left.
    const content = useRef<HTMLDivElement>(null);
    const [current, setCurrent] = useState(SECTIONS[0][0]);
    const onScroll = () => {
        const box = content.current!;
        // Scrolled to the end: the last section, which may never reach the top.
        if (box.scrollTop + box.clientHeight >= box.scrollHeight - 2) return setCurrent(SECTIONS[SECTIONS.length - 1][0]);
        const top = box.getBoundingClientRect().top + 40;
        const passed = SECTIONS.filter(([title]) => (document.getElementById(sectionId(title))?.getBoundingClientRect().top ?? Infinity) <= top);
        setCurrent((passed[passed.length - 1] ?? SECTIONS[0])[0]);
    };

    useEffect(() => {
        getVersion().then(setVersion).catch(() => {});
        // Mounted as it opens: its content takes the keyboard (not the close button, which would get
        // a focus ring for nothing), at the section asked for.
        content.current?.focus();
        if (section) document.getElementById(section)?.scrollIntoView({ block: "start" });
    }, []);

    const browseDefault = async () => {
        const dir = await open({ directory: true, defaultPath: settings.defaultSearchPath || undefined });
        if (typeof dir === "string") set({ defaultSearchPath: dir });
    };

    const reset = async () => {
        if (await confirm("Reset all settings to their defaults? Your search history is kept.", { title: "Reset settings", kind: "warning", okLabel: "Reset", cancelLabel: "Cancel" })) {
            set(DEFAULT_SETTINGS);
        }
    };

    return (
        <div className="flex h-full flex-col">
            <div className="flex h-12 shrink-0 items-center justify-between border-b px-5">
                <h2 className="font-semibold">Settings</h2>
                <Button variant="ghost" size="icon" onClick={onClose} title="Close (Esc)" aria-label="Close settings">
                    <X />
                </Button>
            </div>

            <div className="flex min-h-0 flex-1">
            <nav aria-label="Settings sections" className="hidden w-40 shrink-0 flex-col gap-0.5 overflow-y-auto border-r p-2 md:flex">
                {SECTIONS.map(([title, name]) => (
                    <button
                        key={title}
                        type="button"
                        aria-current={current === title || undefined}
                        onClick={() => {
                            document.getElementById(sectionId(title))?.scrollIntoView({ block: "start", behavior: "smooth" });
                            setCurrent(title);
                        }}
                        className={cn(
                            "rounded-md px-2.5 py-1.5 text-left text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
                            current === title && "bg-accent font-medium text-foreground"
                        )}
                    >
                        {name}
                    </button>
                ))}
            </nav>
            <div ref={content} data-settings-body tabIndex={-1} onScroll={onScroll} className="min-w-0 flex-1 space-y-6 overflow-y-auto px-5 py-5 outline-hidden">
                <Section title="Appearance">
                    <Row label="Theme">
                        <Segmented value={settings.theme} onChange={(theme) => set({ theme })} options={[["light", "Light"], ["dark", "Dark"], ["system", "System"]]} />
                    </Row>
                    <Row
                        label="Backdrop"
                        hint={settings.backdrop === "mica" && !document.documentElement.hasAttribute("data-backdrop") ? "Mica needs Windows 11: the window stays solid here" : "Windows 11's Mica behind the title bar and the sidebar, as in File Explorer"}
                    >
                        <Segmented value={settings.backdrop} onChange={(backdrop) => set({ backdrop })} options={[["mica", "Mica"], ["solid", "Solid"]]} />
                    </Row>
                    <Row label="Accent color" hint="Glint's pink, or the color chosen in Windows' Settings → Personalization → Colors">
                        <Segmented value={settings.accent} onChange={(accent) => set({ accent })} options={[["glint", "Glint"], ["windows", "Windows"]]} />
                    </Row>
                    <Row label="Text size">
                        <Segmented value={settings.fontSize} onChange={(fontSize) => set({ fontSize })} options={[["small", "Small"], ["medium", "Medium"], ["large", "Large"]]} />
                    </Row>
                    <SwitchRow label="Line numbers in preview" checked={settings.showLineNumbers} onChange={(showLineNumbers) => set({ showLineNumbers })} />
                </Section>

                <Section title="Files">
                    <SwitchRow
                        label="Reopen my tabs"
                        hint="Each pane's tabs, as they were when the app closed"
                        checked={settings.restoreTabs}
                        onChange={(restoreTabs) => set({ restoreTabs })}
                    />
                    <Row label={settings.restoreTabs ? "Otherwise open" : "Open on start"}>
                        <Segmented value={settings.startFolder} onChange={(startFolder) => set({ startFolder })} options={[["home", "Home"], ["thispc", "This PC"], ["last", "Last folder"]]} />
                    </Row>
                    <SwitchRow label="Show hidden items" hint="Files and folders Windows marks as hidden · Ctrl+H" checked={settings.showHidden} onChange={(showHidden) => set({ showHidden })} />
                    <SwitchRow label="Folders first" checked={settings.foldersFirst} onChange={(foldersFirst) => set({ foldersFirst })} />
                    <SwitchRow label="Preview pane" hint="Alt+P" checked={settings.showPreviewPane} onChange={(showPreviewPane) => set({ showPreviewPane })} />
                    <SwitchRow
                        label="Thumbnails in the list"
                        hint="Pictures and videos show a small one in place of their icon. The grid view (Ctrl+Shift+2) shows them for every file Windows has one of."
                        checked={settings.listThumbnails}
                        onChange={(listThumbnails) => set({ listThumbnails })}
                    />
                    <SwitchRow
                        label="A folder's README above the list"
                        hint="Its README.md, folded to its title until you open it"
                        checked={settings.showReadme}
                        onChange={(showReadme) => set({ showReadme })}
                    />
                    <Row label="Tile size in the grid" hint="Ctrl+wheel, Ctrl+= and Ctrl+- in the grid">
                        <Segmented value={settings.tileSize} onChange={(tileSize) => set({ tileSize })} options={TILE_SIZES.map(({ size, label }): [TileSize, string] => [size, label])} />
                    </Row>
                    <SwitchRow
                        label="Ask before moving to the Recycle Bin"
                        hint="Deleting permanently (Shift+Del) always asks"
                        checked={settings.confirmDelete}
                        onChange={(confirmDelete) => set({ confirmDelete })}
                    />
                    <FileExplorerRows />
                </Section>

                <InstantSearch />

                <Section title="Find files by name">
                    <Row label="Result limit" hint="Stops after this many files and folders">
                        <Segmented value={settings.findMaxResults} onChange={(findMaxResults) => set({ findMaxResults })} options={[[10000, "10k"], [50000, "50k"], [200000, "200k"], [null, "None"]]} />
                    </Row>
                    <SwitchRow
                        label="Skip what .gitignore lists"
                        hint="Leaves out build output and dependencies in code folders. Searches with fd: only fd reads .gitignore files."
                        checked={settings.findRespectIgnore}
                        onChange={(findRespectIgnore) => set({ findRespectIgnore })}
                    />
                    <div className="space-y-3 p-4">
                        <p className="text-xs text-muted-foreground">Folders and patterns name searches skip. Hidden items are searched only while they're shown.</p>
                        <PatternEditor value={settings.findExclusions} onChange={(findExclusions) => set({ findExclusions })} placeholder="Add a folder or pattern, e.g. node_modules or *.tmp" />
                    </div>
                </Section>

                <Section title="Quick search">
                    <Row
                        label="Shortcut"
                        hint={
                            quickTaken && quickTaken === settings.quickShortcut
                                ? `Another app already uses ${quickTaken}, so it doesn't open quick search: pick another`
                                : "From anywhere: a search box over every drive. Enter opens, Alt+Enter shows it in Glint"
                        }
                    >
                        <Segmented
                            value={settings.quickShortcut}
                            onChange={(quickShortcut) => set({ quickShortcut })}
                            options={[[null, "Off"], ["Ctrl+Alt+F", "Ctrl+Alt+F"], ["Ctrl+Alt+Space", "Ctrl+Alt+Space"], ["Ctrl+Shift+Space", "Ctrl+Shift+Space"]]}
                        />
                    </Row>
                    <SwitchRow
                        label="Keep running in the tray"
                        hint="Closing the window leaves Glint in the notification area, so the shortcut works anytime"
                        checked={settings.keepInTray}
                        onChange={(keepInTray) => set({ keepInTray })}
                    />
                    <SwitchRow
                        label="Start with Windows"
                        hint="In the tray, when you sign in"
                        checked={settings.startWithWindows && settings.keepInTray}
                        disabled={!settings.keepInTray}
                        onChange={(startWithWindows) => set({ startWithWindows })}
                    />
                </Section>

                <AgentsSection />

                <Section title="Search text (ripgrep)">
                    <Row label="Default folder" hint="Used when no folder is filled in">
                        <Input className="w-52" value={settings.defaultSearchPath} onChange={(e) => set({ defaultSearchPath: e.target.value })} placeholder="None" />
                        <Button variant="outline" size="sm" onClick={browseDefault}>Browse…</Button>
                    </Row>
                    <Row label="Result limit" hint="Stops searching after this many matching lines">
                        <Segmented value={settings.maxResults} onChange={(maxResults) => set({ maxResults })} options={[[1000, "1k"], [10000, "10k"], [50000, "50k"], [null, "None"]]} />
                    </Row>
                    <SwitchRow label="Preview the first result" hint="Show the first match as soon as results arrive" checked={settings.autoOpenPreview} onChange={(autoOpenPreview) => set({ autoOpenPreview })} />
                    <SwitchRow label="Confirm before replacing" checked={settings.confirmBeforeReplace} onChange={(confirmBeforeReplace) => set({ confirmBeforeReplace })} />
                    <Row label="Open files with" hint="A program name or path, e.g. code or notepad++ (results open at their line), or an editor found on this PC">
                        <Input className="w-52" value={settings.editorPath} onChange={(e) => set({ editorPath: e.target.value })} placeholder="System default" />
                        <EditorPicker value={settings.editorPath} onPick={(editorPath) => set({ editorPath })} />
                    </Row>
                </Section>

                <Section
                    title="Text search skips"
                    action={settings.exclusions.length > 0 && <Button variant="ghost" size="sm" className="h-7" onClick={() => set({ exclusions: [] })}>Remove all</Button>}
                >
                    <div className="space-y-4 p-4">
                        <p className="text-xs text-muted-foreground">
                            Glob patterns every text search skips. Files ignored by .gitignore, hidden files and binary files are skipped too.
                        </p>
                        <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
                            {PRESETS.map(({ label, icon: Icon, items }) => {
                                const active = items.filter((item) => settings.exclusions.includes(item)).length;
                                const all = active === items.length;
                                return (
                                    <button
                                        key={label}
                                        type="button"
                                        aria-pressed={all}
                                        title={items.join("  ")}
                                        onClick={() => set({
                                            exclusions: all
                                                ? settings.exclusions.filter((ex) => !items.includes(ex))
                                                : [...settings.exclusions, ...items.filter((item) => !settings.exclusions.includes(item))],
                                        })}
                                        className={cn(
                                            "flex items-center gap-2 rounded-md border px-2.5 py-2 text-left text-sm transition-colors hover:bg-accent [&_svg]:size-4",
                                            all ? "border-primary/40 bg-primary/10" : active > 0 && "border-dashed"
                                        )}
                                    >
                                        <Icon className={all ? "text-primary" : "text-muted-foreground"} />
                                        {label}
                                        <span className="ml-auto text-xs tabular-nums text-muted-foreground">{active}/{items.length}</span>
                                    </button>
                                );
                            })}
                        </div>
                        <PatternEditor value={settings.exclusions} onChange={(exclusions) => set({ exclusions })} placeholder="Add a pattern: folder, *.log, or file name" />
                    </div>
                </Section>

                <TerminalSection />

                <CheckSection />

                <ToolsSection />

                <Section title="History">
                    <SwitchRow label="Save search history" checked={settings.saveSearchHistory} onChange={(saveSearchHistory) => set({ saveSearchHistory })} />
                    <Row label="Keep up to">
                        <Segmented value={settings.maxHistoryItems} onChange={(maxHistoryItems) => set({ maxHistoryItems })} options={[[100, "100"], [500, "500"], [1000, "1,000"], [5000, "5,000"]]} />
                    </Row>
                    <Row label={`${history.length.toLocaleString()} ${history.length === 1 ? "search" : "searches"} in the history`}>
                        <Button variant="outline" size="sm" onClick={() => clearWithUndo("history")} disabled={!history.length}>Clear history</Button>
                    </Row>
                    <Row label={`${visited.toLocaleString()} ${visited === 1 ? "folder" : "folders"} remembered`} hint="The command palette (Ctrl+K) offers the folders you use most first">
                        <Button variant="outline" size="sm" onClick={() => clearWithUndo("visits")} disabled={!visited}>Clear folders</Button>
                    </Row>
                    <SwitchRow
                        label="Share folder history with zoxide"
                        hint={
                            zoxide?.path
                                ? "Ctrl+K and quick search rank the folders you cd to in a terminal too, and z there finds the folders you open here"
                                : "zoxide, the smarter cd for terminals, isn't installed: Tools above installs it"
                        }
                        checked={settings.zoxide}
                        disabled={!zoxide?.path && !settings.zoxide}
                        onChange={(on) => {
                            set({ zoxide: on });
                            void refreshZoxide();
                        }}
                    />
                </Section>

                <Section title="About">
                    <div className="flex items-center gap-3 px-4 py-4">
                        <img src={logo} alt="" className="size-11" />
                        <div className="min-w-0 flex-1">
                            <div className="font-semibold">Glint</div>
                            <div className="text-xs text-muted-foreground">
                                {version && `Version ${version} · `}Made with <Heart className="inline size-3 fill-pink-500 text-pink-500" /> by Shadi Saadi
                            </div>
                            <div className="text-xs text-muted-foreground">Powered by ripgrep, fd, pdu and an NTFS index</div>
                        </div>
                        <Button variant="outline" size="sm" onClick={() => openUrl(REPO_URL)}>
                            <ExternalLink /> GitHub
                        </Button>
                    </div>
                    <UpdateRow />
                    <Row label="Keyboard shortcuts" hint="Every one on a sheet: F1 or Ctrl+/ anywhere">
                        <Button
                            variant="outline"
                            size="sm"
                            onClick={() => {
                                // Settings sits above everything: it goes first.
                                onClose();
                                openShortcuts();
                            }}
                        >
                            <Keyboard /> Show
                        </Button>
                    </Row>
                    <Row label="Diagnostics" hint="Versions, tools and instant search's state, to paste into a bug report">
                        <Button variant="outline" size="sm" onClick={copyDiagnostics}>
                            <ClipboardCopy /> Copy
                        </Button>
                    </Row>
                    <Row label="Reset settings" hint="Restore every option above to its default">
                        <Button variant="outline" size="sm" onClick={reset}>
                            <RotateCcw /> Reset
                        </Button>
                    </Row>
                </Section>
            </div>
            </div>
        </div>
    );
}
