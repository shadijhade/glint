import { useEffect, useRef, useState } from "react";
import { Toaster, toast } from "sonner";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { FolderOpen, History as HistoryIcon, Loader2, Settings as SettingsIcon, Sparkles, TextSearch } from "lucide-react";
import logo from "./assets/logo.svg";
import { cancelSearch, RgMatch, searchBatched } from "./lib/ripgrep";
import { HistoryItem, Mode, SearchOptions, useStore } from "./lib/store";
import { currentPath, revealInApp } from "./lib/explorer";
import { THIS_PC } from "./lib/paths";
import { toRegExp } from "./lib/text";
import { checkForUpdates, useUpdater } from "./lib/updater";
import { cn } from "./lib/utils";
import { useTheme } from "./hooks/useTheme";
import { SearchBar } from "./components/Search";
import { Results } from "./components/Results";
import { SettingsView } from "./components/SettingsView";
import { Home, HistoryPopover, HISTORY_POPOVER_ID } from "./components/History";
import { WindowControls } from "./components/WindowControls";
import { AddressBar } from "./components/explorer/AddressBar";
import { Explorer } from "./components/explorer/Explorer";
import { Button } from "./components/ui/button";

export interface SearchRun {
  id: string;
  query: string;
  path: string;
  options: SearchOptions;
  limit: number | null;
}

/** How often a long-running session looks for a new version again (it also looks at start). */
const UPDATE_CHECK_MS = 12 * 60 * 60 * 1000;

const MODES: { mode: Mode; icon: typeof FolderOpen; label: string; title: string }[] = [
  { mode: "files", icon: FolderOpen, label: "Files", title: "Browse files (Ctrl+Shift+E)" },
  { mode: "text", icon: TextSearch, label: "Text", title: "Search inside files with ripgrep (Ctrl+Shift+F)" },
];

function ModeSwitch({ mode, onChange }: { mode: Mode; onChange: (mode: Mode) => void }) {
  return (
    <div role="tablist" aria-label="Mode" className="flex shrink-0 rounded-md bg-foreground/[0.05] p-0.5">
      {MODES.map(({ mode: m, icon: Icon, label, title }) => (
        <button
          key={m}
          role="tab"
          aria-selected={mode === m}
          title={title}
          onClick={() => onChange(m)}
          className={cn(
            "flex h-7 items-center gap-1.5 rounded-[5px] px-2.5 text-sm text-muted-foreground transition-colors hover:text-foreground [&_svg]:size-4",
            mode === m && "bg-background font-medium text-foreground shadow-xs dark:bg-foreground/[0.12]"
          )}
        >
          <Icon className={mode === m ? "text-primary" : undefined} />
          <span className="hidden sm:inline">{label}</span>
        </button>
      ))}
    </div>
  );
}

function App() {
  useTheme();
  const settings = useStore((s) => s.settings);
  const mode = useStore((s) => s.mode);
  const setMode = useStore((s) => s.setMode);
  const [run, setRun] = useState<SearchRun | null>(null);
  const [matches, setMatches] = useState<RgMatch[]>([]);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState("");
  const [duration, setDuration] = useState(0);
  const [filesSearched, setFilesSearched] = useState<number | null>(null);
  const [selected, setSelected] = useState(-1);
  const update = useUpdater((s) => s.available);
  const updating = useUpdater((s) => s.progress !== null);
  const runId = useRef("");
  const unlisten = useRef<(() => void) | undefined>(undefined);
  const settingsRef = useRef<HTMLDialogElement>(null);

  const openSettings = () => {
    const dialog = settingsRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  };

  /** Settings at About, where an available update installs. */
  const openAbout = () => {
    openSettings();
    document.getElementById("settings-about")?.scrollIntoView({ block: "start" });
  };

  /** Text search in `folder` (the open folder by default). */
  const searchText = (folder?: string) => {
    const target = folder ?? currentPath();
    if (target && target !== THIS_PC) useStore.getState().setPath(target);
    setMode("text");
  };

  useEffect(() => {
    checkForUpdates();
    const timer = setInterval(checkForUpdates, UPDATE_CHECK_MS);
    return () => {
      clearInterval(timer);
      unlisten.current?.();
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const ctrl = e.ctrlKey || e.metaKey;
      if (e.key === "Escape" && searching && mode === "text" && !(e.target as Element).closest("dialog, [popover]")) {
        cancelSearch();
      } else if (ctrl && e.key === ",") {
        e.preventDefault();
        openSettings();
      } else if (ctrl && e.shiftKey && e.key.toLowerCase() === "f") {
        e.preventDefault();
        if (mode === "files") searchText();
      } else if (ctrl && e.shiftKey && e.key.toLowerCase() === "e") {
        e.preventDefault();
        setMode("files");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [searching, mode]);

  const search = async (query: string, path: string, limit = useStore.getState().settings.maxResults) => {
    if (!query || !path) return;
    const { options, settings, addToHistory } = useStore.getState();
    const id = crypto.randomUUID();
    const isCurrent = () => runId.current === id;
    runId.current = id;
    unlisten.current?.();
    unlisten.current = undefined;

    setRun({ id, query, path, options, limit });
    setMatches([]);
    setSelected(-1);
    setError("");
    setDuration(0);
    setFilesSearched(null);
    setSearching(true);
    addToHistory(query, path);

    const started = performance.now();
    let elapsed = 0;
    const finish = () => {
      if (!isCurrent()) return;
      setSearching(false);
      setDuration(elapsed || performance.now() - started);
    };

    const stop = await searchBatched(
      query,
      path,
      id,
      (lines) => {
        if (!isCurrent()) return;
        const batch: RgMatch[] = [];
        for (const line of lines) {
          try {
            const event: RgMatch = JSON.parse(line);
            if (event.type === "match") {
              batch.push(event);
            } else if (event.type === "summary") {
              const total = event.data.elapsed_total;
              if (total) elapsed = total.secs * 1000 + total.nanos / 1e6;
              setFilesSearched(event.data.stats?.searches ?? null);
            }
          } catch {
            // not a JSON line
          }
        }
        if (batch.length) setMatches((prev) => prev.concat(batch));
      },
      finish,
      (message) => {
        if (isCurrent()) setError(message);
        finish();
      },
      { ...options, exclusions: settings.exclusions, maxResults: limit }
    );
    // A newer search may have started while this one was spinning up.
    if (isCurrent()) unlisten.current = stop;
    else stop();
  };

  /** Back from text search results to the start screen (recent searches). */
  const closeResults = () => {
    if (searching) cancelSearch();
    runId.current = "";
    unlisten.current?.();
    unlisten.current = undefined;
    setRun(null);
    setMatches([]);
    setSearching(false);
  };

  const searchCurrent = () => {
    const { query, path } = useStore.getState();
    search(query, path);
  };

  const pickHistory = (item: HistoryItem) => {
    const { setQuery, setPath } = useStore.getState();
    setQuery(item.query);
    setPath(item.path);
    search(item.query, item.path);
  };

  const openFile = (path: string) =>
    openPath(path, settings.editorPath.trim() || undefined).catch((e) => toast.error("Couldn't open the file", { description: String(e) }));

  // Text search results open their folder here, with the file selected.
  const reveal = (path: string) => {
    setMode("files");
    revealInApp(path);
  };

  const replaceAll = async (replacement: string) => {
    if (!run || searching) return;
    const files = [...new Set(matches.map((m) => m.data.path?.text ?? ""))].filter(Boolean);
    if (!files.length) return;

    let pattern: RegExp;
    try {
      pattern = toRegExp(run.query, run.options);
    } catch (e) {
      toast.error("This pattern can't be used for replacing", { description: String(e) });
      return;
    }
    const confirmed = !settings.confirmBeforeReplace || await confirm(
      `Replace every match of “${run.query}” with “${replacement}” in ${files.length} ${files.length === 1 ? "file" : "files"}?\n\nThis can't be undone.`,
      { title: "Replace all", kind: "warning" }
    );
    if (!confirmed) return;

    const toastId = toast.loading(`Replacing in ${files.length} files…`);
    let changed = 0;
    let failed = 0;
    for (const path of files) {
      try {
        const content = await invoke<string>("read_file_content", { path });
        // Literal searches insert the replacement verbatim; regex searches allow $1-style groups.
        const next = run.options.regex ? content.replace(pattern, replacement) : content.replace(pattern, () => replacement);
        if (next !== content) {
          await invoke("write_file_content", { path, content: next });
          changed++;
        }
      } catch (e) {
        console.error(`Replace failed in ${path}`, e);
        failed++;
      }
    }
    toast.dismiss(toastId);
    const summary = `Replaced in ${changed} ${changed === 1 ? "file" : "files"}`;
    if (failed) toast.error(`${summary}, ${failed} failed`);
    else toast.success(summary);
    search(run.query, run.path, run.limit);
  };

  // With auto-preview on, the first result is shown until the user picks another.
  const current = selected >= 0 ? selected : settings.autoOpenPreview && matches.length ? 0 : -1;

  return (
    <div className="flex h-full flex-col">
      {/* The window's title bar (it has no system one): drag it by any empty spot, double-click to maximize. */}
      <header data-tauri-drag-region="deep" className="flex h-11 shrink-0 items-center gap-2 border-b bg-chrome pl-3">
        <img src={logo} alt="" draggable={false} className="size-[18px] shrink-0" />
        <ModeSwitch mode={mode} onChange={setMode} />
        {mode === "files" ? <AddressBar /> : <div className="min-w-0 flex-1" />}
        <div className="flex shrink-0 items-center gap-0.5">
          {update && (
            <Button variant="ghost" size="sm" className="text-primary hover:text-primary" onClick={openAbout} title={`Version ${update.version} is available`}>
              {updating ? <Loader2 className="animate-spin" /> : <Sparkles />} Update
            </Button>
          )}
          {mode === "text" && (
            <Button variant="ghost" size="icon" className="size-8" popoverTarget={HISTORY_POPOVER_ID} title="Recent searches" aria-label="Recent searches">
              <HistoryIcon />
            </Button>
          )}
          <Button variant="ghost" size="icon" className="size-8" onClick={openSettings} title="Settings (Ctrl+,)" aria-label="Settings">
            <SettingsIcon />
          </Button>
        </div>
        <WindowControls />
      </header>

      <Explorer active={mode === "files"} onSearchText={searchText} onOpenSettings={openSettings} />

      <div className={mode === "text" ? "flex min-h-0 flex-1 flex-col" : "hidden"}>
        {/* Only while Text is on screen: its shortcuts (Ctrl+F, Ctrl+K, Ctrl+H) are Files' otherwise. */}
        {mode === "text" && (
          <div className="flex shrink-0 border-b bg-chrome px-3 py-2.5">
            <SearchBar
              searching={searching}
              canReplace={!searching && matches.length > 0}
              onSearch={searchCurrent}
              onStop={cancelSearch}
              onReplace={replaceAll}
            />
          </div>
        )}
        {run ? (
          <Results
            key={run.id}
            active={mode === "text"}
            run={run}
            matches={matches}
            searching={searching}
            error={error}
            duration={duration}
            filesSearched={filesSearched}
            selected={current}
            onSelect={setSelected}
            onOpen={openFile}
            onReveal={reveal}
            onShowAll={() => search(run.query, run.path, null)}
            onClose={closeResults}
          />
        ) : (
          <Home onPick={pickHistory} />
        )}
      </div>

      <HistoryPopover onPick={pickHistory} />
      <dialog
        ref={settingsRef}
        onClick={(e) => e.target === e.currentTarget && e.currentTarget.close()}
        className="m-auto h-[min(85vh,46rem)] w-[min(92vw,44rem)] overflow-hidden rounded-xl border bg-chrome p-0 text-foreground shadow-2xl backdrop:bg-black/50"
      >
        <SettingsView onClose={() => settingsRef.current?.close()} />
      </dialog>
      <Toaster theme={settings.theme} position="bottom-right" richColors />
    </div>
  );
}

export default App;
