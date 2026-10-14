import { useEffect, useRef, useState } from "react";
import { Toaster, toast } from "sonner";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { confirm } from "@tauri-apps/plugin-dialog";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import { FolderOpen, History as HistoryIcon, Settings as SettingsIcon, Sparkles, TextSearch } from "lucide-react";
import logo from "./assets/logo.svg";
import { cancelSearch, RgMatch, searchBatched } from "./lib/ripgrep";
import { HistoryItem, Mode, SearchOptions, useStore } from "./lib/store";
import { navigate, revealInApp, useExplorer } from "./lib/explorer";
import { THIS_PC } from "./lib/paths";
import { toRegExp } from "./lib/text";
import { cn } from "./lib/utils";
import { useTheme } from "./hooks/useTheme";
import { SearchBar } from "./components/Search";
import { Results } from "./components/Results";
import { SettingsView } from "./components/SettingsView";
import { Home, HistoryPopover, HISTORY_POPOVER_ID } from "./components/History";
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

const RELEASES_API = "https://api.github.com/repos/shadijhade/shadui-ripgrep/releases/latest";

const MODES: { mode: Mode; icon: typeof FolderOpen; label: string; title: string }[] = [
  { mode: "files", icon: FolderOpen, label: "Files", title: "Browse files (Ctrl+Shift+E)" },
  { mode: "text", icon: TextSearch, label: "Text", title: "Search inside files with ripgrep (Ctrl+Shift+F)" },
];

function ModeSwitch({ mode, onChange }: { mode: Mode; onChange: (mode: Mode) => void }) {
  return (
    <div role="tablist" aria-label="Mode" className="flex shrink-0 rounded-lg bg-muted p-0.5">
      {MODES.map(({ mode: m, icon: Icon, label, title }) => (
        <button
          key={m}
          role="tab"
          aria-selected={mode === m}
          title={title}
          onClick={() => onChange(m)}
          className={cn(
            "flex h-8 items-center gap-1.5 rounded-md px-2.5 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground [&_svg]:size-4",
            mode === m && "bg-card text-foreground shadow-xs"
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
  const [update, setUpdate] = useState<{ version: string; url: string } | null>(null);
  const runId = useRef("");
  const unlisten = useRef<(() => void) | undefined>(undefined);
  const settingsRef = useRef<HTMLDialogElement>(null);

  const openSettings = () => {
    const dialog = settingsRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  };

  /** Text search in `folder` (the open folder by default). */
  const searchText = (folder?: string) => {
    const target = folder ?? useExplorer.getState().path;
    if (target && target !== THIS_PC) useStore.getState().setPath(target);
    setMode("text");
  };

  useEffect(() => {
    Promise.all([getVersion(), fetch(RELEASES_API).then((r) => r.json())])
      .then(([current, latest]) => {
        const version = String(latest.tag_name ?? "").replace(/^v/, "");
        const isNewer = /^\d+(\.\d+)*$/.test(version) && version.localeCompare(current, undefined, { numeric: true }) > 0;
        if (isNewer) setUpdate({ version, url: latest.html_url });
      })
      .catch(() => {}); // offline or rate-limited: no update badge
    return () => unlisten.current?.();
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

  const goHome = () => {
    if (mode === "files") {
      navigate(THIS_PC);
      return;
    }
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
      <header className={cn("flex shrink-0 gap-2.5 border-b bg-card px-3 py-2.5", mode === "text" ? "items-start" : "items-center")}>
        <button
          onClick={goHome}
          className="shrink-0 rounded-lg transition-opacity hover:opacity-90 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring/50"
          title={mode === "files" ? "This PC" : "Start screen"}
          aria-label={mode === "files" ? "This PC" : "Start screen"}
        >
          <img src={logo} alt="" className="size-9" />
        </button>
        <div className={cn("shrink-0", mode === "text" && "pt-0.5")}>
          <ModeSwitch mode={mode} onChange={setMode} />
        </div>
        {mode === "files" ? (
          <AddressBar />
        ) : (
          <SearchBar
            searching={searching}
            canReplace={!searching && matches.length > 0}
            onSearch={searchCurrent}
            onStop={cancelSearch}
            onReplace={replaceAll}
          />
        )}
        <div className={cn("flex shrink-0 items-center gap-0.5", mode === "text" && "pt-0.5")}>
          {update && (
            <Button variant="ghost" size="sm" className="text-primary hover:text-primary" onClick={() => openUrl(update.url)} title={`Version ${update.version} is available`}>
              <Sparkles /> Update
            </Button>
          )}
          {mode === "text" && (
            <Button variant="ghost" size="icon" popoverTarget={HISTORY_POPOVER_ID} title="Recent searches" aria-label="Recent searches">
              <HistoryIcon />
            </Button>
          )}
          <Button variant="ghost" size="icon" onClick={openSettings} title="Settings (Ctrl+,)" aria-label="Settings">
            <SettingsIcon />
          </Button>
        </div>
      </header>

      <Explorer active={mode === "files"} onSearchText={searchText} />

      <div className={mode === "text" ? "flex min-h-0 flex-1 flex-col" : "hidden"}>
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
          />
        ) : (
          <Home onPick={pickHistory} />
        )}
      </div>

      <HistoryPopover onPick={pickHistory} />
      <dialog
        ref={settingsRef}
        onClick={(e) => e.target === e.currentTarget && e.currentTarget.close()}
        className="m-auto h-[min(85vh,46rem)] w-[min(92vw,44rem)] overflow-hidden rounded-xl border bg-background p-0 text-foreground shadow-2xl backdrop:bg-black/50"
      >
        <SettingsView onClose={() => settingsRef.current?.close()} update={update} />
      </dialog>
      <Toaster theme={settings.theme} position="bottom-right" richColors />
    </div>
  );
}

export default App;
