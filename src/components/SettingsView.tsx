import { ReactNode, useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Archive, Binary, ExternalLink, Folder, Heart, Image, Lock, Plus, RotateCcw, Sparkles, Type, X } from "lucide-react";
import logo from "@/assets/logo.svg";
import { DEFAULT_SETTINGS, useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Button } from "./ui/button";
import { Input } from "./ui/input";

const REPO_URL = "https://github.com/shadijhade/shadui-ripgrep";

const PRESETS = [
    { label: "Folders", icon: Folder, items: ["node_modules", ".git", "dist", "build", "bin", "obj", ".vs", ".idea", "__pycache__", ".cache", "vendor", "packages", ".nuget", "coverage", ".next", "target"] },
    { label: "Binaries", icon: Binary, items: ["*.exe", "*.dll", "*.so", "*.dylib", "*.pdb", "*.lib", "*.a", "*.o", "*.class", "*.pyc"] },
    { label: "Images", icon: Image, items: ["*.png", "*.jpg", "*.jpeg", "*.gif", "*.ico", "*.svg", "*.webp", "*.bmp", "*.tiff", "*.psd"] },
    { label: "Archives", icon: Archive, items: ["*.zip", "*.rar", "*.7z", "*.tar", "*.gz", "*.bz2", "*.xz", "*.iso"] },
    { label: "Fonts", icon: Type, items: ["*.woff", "*.woff2", "*.ttf", "*.eot", "*.otf"] },
    { label: "Lock files", icon: Lock, items: ["package-lock.json", "yarn.lock", "pnpm-lock.yaml", "Cargo.lock", "Gemfile.lock", "composer.lock", "poetry.lock"] },
];

function Section({ title, action, children }: { title: string; action?: ReactNode; children: ReactNode }) {
    return (
        <section>
            <div className="mb-2 flex h-7 items-center justify-between px-1">
                <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">{title}</h3>
                {action}
            </div>
            <div className="divide-y rounded-lg border bg-card">{children}</div>
        </section>
    );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
    return (
        <div className="flex items-center justify-between gap-6 px-4 py-3">
            <div className="min-w-0">
                <div className="text-sm">{label}</div>
                {hint && <div className="mt-0.5 text-xs text-muted-foreground">{hint}</div>}
            </div>
            <div className="flex shrink-0 items-center gap-1.5">{children}</div>
        </div>
    );
}

/** A labelled on/off row; the whole row toggles. */
function SwitchRow({ label, hint, checked, onChange }: { label: string; hint?: string; checked: boolean; onChange: (value: boolean) => void }) {
    return (
        <label className="flex cursor-pointer items-center justify-between gap-6 px-4 py-3">
            <div className="min-w-0">
                <div className="text-sm">{label}</div>
                {hint && <div className="mt-0.5 text-xs text-muted-foreground">{hint}</div>}
            </div>
            <input
                type="checkbox"
                role="switch"
                checked={checked}
                onChange={(e) => onChange(e.target.checked)}
                className="relative h-5 w-9 shrink-0 cursor-pointer appearance-none rounded-full bg-input transition-colors before:absolute before:left-0.5 before:top-0.5 before:size-4 before:rounded-full before:bg-white before:shadow-xs before:transition-transform checked:bg-primary checked:before:translate-x-4 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring/40"
            />
        </label>
    );
}

function Segmented<T extends string | number | null>({ value, options, onChange }: { value: T; options: [T, string][]; onChange: (value: T) => void }) {
    return (
        <div className="inline-flex rounded-md bg-muted p-0.5">
            {options.map(([option, label]) => (
                <button
                    key={label}
                    type="button"
                    aria-pressed={option === value}
                    onClick={() => onChange(option)}
                    className={cn(
                        "rounded-sm px-2.5 py-1 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground",
                        option === value && "bg-card text-foreground shadow-xs"
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
                    className="font-mono"
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

interface SettingsViewProps {
    onClose: () => void;
    update: { version: string; url: string } | null;
}

export function SettingsView({ onClose, update }: SettingsViewProps) {
    const { settings, setSettings: set, history, clearHistory } = useStore();
    const [version, setVersion] = useState("");

    useEffect(() => {
        getVersion().then(setVersion).catch(() => {});
    }, []);

    const browseDefault = async () => {
        const dir = await open({ directory: true, defaultPath: settings.defaultSearchPath || undefined });
        if (typeof dir === "string") set({ defaultSearchPath: dir });
    };

    const reset = async () => {
        if (await confirm("Reset all settings to their defaults? Your search history is kept.", { title: "Reset settings", kind: "warning" })) {
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

            <div className="flex-1 space-y-6 overflow-y-auto px-5 py-5">
                <Section title="Appearance">
                    <Row label="Theme">
                        <Segmented value={settings.theme} onChange={(theme) => set({ theme })} options={[["light", "Light"], ["dark", "Dark"], ["system", "System"]]} />
                    </Row>
                    <Row label="Text size">
                        <Segmented value={settings.fontSize} onChange={(fontSize) => set({ fontSize })} options={[["small", "Small"], ["medium", "Medium"], ["large", "Large"]]} />
                    </Row>
                    <SwitchRow label="Line numbers in preview" checked={settings.showLineNumbers} onChange={(showLineNumbers) => set({ showLineNumbers })} />
                </Section>

                <Section title="Files">
                    <Row label="Open on start">
                        <Segmented value={settings.startFolder} onChange={(startFolder) => set({ startFolder })} options={[["home", "Home"], ["thispc", "This PC"], ["last", "Last folder"]]} />
                    </Row>
                    <SwitchRow label="Show hidden items" hint="Files and folders Windows marks as hidden · Ctrl+H" checked={settings.showHidden} onChange={(showHidden) => set({ showHidden })} />
                    <SwitchRow label="Folders first" checked={settings.foldersFirst} onChange={(foldersFirst) => set({ foldersFirst })} />
                    <SwitchRow label="Preview pane" hint="Alt+P" checked={settings.showPreviewPane} onChange={(showPreviewPane) => set({ showPreviewPane })} />
                    <SwitchRow
                        label="Ask before moving to the Recycle Bin"
                        hint="Deleting permanently (Shift+Del) always asks"
                        checked={settings.confirmDelete}
                        onChange={(confirmDelete) => set({ confirmDelete })}
                    />
                </Section>

                <Section title="Find files by name (fd)">
                    <Row label="Result limit" hint="Stops after this many files and folders">
                        <Segmented value={settings.findMaxResults} onChange={(findMaxResults) => set({ findMaxResults })} options={[[10000, "10k"], [50000, "50k"], [200000, "200k"], [null, "None"]]} />
                    </Row>
                    <SwitchRow
                        label="Skip what .gitignore lists"
                        hint="Leaves out build output and dependencies in code folders"
                        checked={settings.findRespectIgnore}
                        onChange={(findRespectIgnore) => set({ findRespectIgnore })}
                    />
                    <div className="space-y-3 p-4">
                        <p className="text-xs text-muted-foreground">Folders and patterns name searches skip. Hidden items are searched only while they're shown.</p>
                        <PatternEditor value={settings.findExclusions} onChange={(findExclusions) => set({ findExclusions })} placeholder="Add a folder or pattern, e.g. node_modules or *.tmp" />
                    </div>
                </Section>

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
                    <Row label="Open files with" hint="A program name or path, e.g. code or notepad++">
                        <Input className="w-52" value={settings.editorPath} onChange={(e) => set({ editorPath: e.target.value })} placeholder="System default" />
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

                <Section title="History">
                    <SwitchRow label="Save search history" checked={settings.saveSearchHistory} onChange={(saveSearchHistory) => set({ saveSearchHistory })} />
                    <Row label="Keep up to">
                        <Segmented value={settings.maxHistoryItems} onChange={(maxHistoryItems) => set({ maxHistoryItems })} options={[[100, "100"], [500, "500"], [1000, "1,000"], [5000, "5,000"]]} />
                    </Row>
                    <Row label={`${history.length.toLocaleString()} saved ${history.length === 1 ? "search" : "searches"}`}>
                        <Button variant="outline" size="sm" onClick={clearHistory} disabled={!history.length}>Clear history</Button>
                    </Row>
                </Section>

                <Section title="About">
                    <div className="flex items-center gap-3 px-4 py-4">
                        <img src={logo} alt="" className="size-11" />
                        <div className="min-w-0 flex-1">
                            <div className="font-semibold">ShadUI Ripgrep</div>
                            <div className="text-xs text-muted-foreground">
                                {version && `Version ${version} · `}Made with <Heart className="inline size-3 fill-pink-500 text-pink-500" /> by Shadi Sadi
                            </div>
                            <div className="text-xs text-muted-foreground">Powered by ripgrep, fd and dust</div>
                        </div>
                        {update && (
                            <Button size="sm" onClick={() => openUrl(update.url)}>
                                <Sparkles /> Get {update.version}
                            </Button>
                        )}
                        <Button variant="outline" size="sm" onClick={() => openUrl(REPO_URL)}>
                            <ExternalLink /> GitHub
                        </Button>
                    </div>
                    <Row label="Reset settings" hint="Restore every option above to its default">
                        <Button variant="outline" size="sm" onClick={reset}>
                            <RotateCcw /> Reset
                        </Button>
                    </Row>
                </Section>
            </div>
        </div>
    );
}
