import { useEffect, useRef, useState } from "react";
import { Toaster, toast } from "sonner";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { FolderOpen, History as HistoryIcon, Loader2, Settings as SettingsIcon, Sparkles, TextSearch } from "lucide-react";
import logo from "./assets/logo.svg";
import { cancelSearch, RgMatch, searchBatched } from "./lib/ripgrep";
import { HistoryItem, Mode, SearchOptions, useStore } from "./lib/store";
import { archiveOf, closedNotice, closedOf, currentPath, defaultTextSearchFolder, openRequested, refreshIndex, revealInApp } from "./lib/explorer";
import { THIS_PC } from "./lib/paths";
import { replaceText, toRegExp } from "./lib/text";
import { checkForUpdates, greetUpdate, useUpdater } from "./lib/updater";
import { openItem, quickConfigure, setStartWithWindows } from "./lib/fs";
import { cn, firstNotice } from "./lib/utils";
import { useTheme } from "./hooks/useTheme";
import { RipgrepCard, SearchBar } from "./components/Search";
import { Results } from "./components/Results";
import { SettingsView } from "./components/SettingsView";
import { openShortcuts, ShortcutsDialog } from "./components/ShortcutsDialog";
import { Home, HistoryPopover, HISTORY_POPOVER_ID } from "./components/History";
import { WindowControls } from "./components/WindowControls";
import { AddressBar } from "./components/explorer/AddressBar";
import { BatchRenameDialog } from "./components/explorer/BatchRenameDialog";
import { CleanupDialog } from "./components/explorer/CleanupDialog";
import { CompareFilesDialog } from "./components/explorer/CompareFilesDialog";
import { DuplicatesDialog } from "./components/explorer/DuplicatesDialog";
import { Explorer } from "./components/explorer/Explorer";
import { HistoryDialog } from "./components/explorer/HistoryDialog";
import { LocksDialog } from "./components/explorer/LocksDialog";
import { TimelineDialog } from "./components/explorer/TimelineDialog";
import { XRayDialog } from "./components/explorer/XRayDialog";
import { Button } from "./components/ui/button";

export interface SearchRun {
  id: string;
  query: string;
  path: string;
  options: SearchOptions;
  limit: number | null;
  /** Searched again skipping nothing. */
  everything?: boolean;
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
            "flex h-7 items-center gap-1.5 rounded-[5px] px-2.5 text-sm text-muted-foreground transition-colors hover:text-foreground max-sm:px-2 [&_svg]:size-4",
            mode === m && "bg-background font-medium text-foreground shadow-xs dark:bg-foreground/[0.12]"
          )}
        >
          <Icon className={mode === m ? "text-primary" : undefined} />
          <span className="hidden lg:inline">{label}</span>
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
  const replacing = useRef(false);
  const unlisten = useRef<(() => void) | undefined>(undefined);
  const settingsRef = useRef<HTMLDialogElement>(null);

  const openSettings = () => {
    const dialog = settingsRef.current;
    if (dialog && !dialog.open) {
      dialog.showModal();
      // Its content, not the close button that would get a focus ring for nothing.
      dialog.querySelector<HTMLElement>("[data-settings-body]")?.focus();
    }
    // The service may have stopped, restarted or finished a drive since the last look.
    refreshIndex();
  };

  /** Settings at About, where an available update installs. */
  const openAbout = () => {
    openSettings();
    document.getElementById("settings-about")?.scrollIntoView({ block: "start" });
  };

  /** Text search in `folder` (the open folder by default). */
  const searchText = (folder?: string) => {
    // ripgrep reads files on disk: an archive's contents aren't any.
    if (!folder && archiveOf()) {
      toast.message("Text search can't look inside archives", { id: "archive-text", description: "It searches files on disk. Copy what you want to search out of the archive first." });
      return;
    }
    // Nor files Windows keeps from this account.
    if (!folder && closedOf()) return closedNotice();
    const target = folder ?? currentPath();
    const state = useStore.getState();
    if (target && target !== THIS_PC) {
      state.setPath(target);
    } else if (!state.path.trim()) {
      const fallback = defaultTextSearchFolder();
      if (fallback) state.setPath(fallback);
    }
    setMode("text");
  };

  // Settings → Quick search, at start and on every change. A shortcut another app took is said once
  // at start (it stays taken), and every time it's chosen in Settings.
  const { quickShortcut, keepInTray, startWithWindows } = settings;
  const configured = useRef(false);
  // The shortcut another app holds, which Settings notes on its row.
  const [quickTaken, setQuickTaken] = useState<string | null>(null);
  useEffect(() => {
    const atStart = !configured.current;
    configured.current = true;
    quickConfigure(quickShortcut, keepInTray).then(
      () => setQuickTaken(null),
      (e) => {
        setQuickTaken(quickShortcut);
        if (!atStart || firstNotice("quick-shortcut-taken", quickShortcut ?? "")) toast.error("Couldn't set the quick search shortcut", { description: String(e) });
      }
    );
  }, [quickShortcut, keepInTray]);
  useEffect(() => {
    // Not without the tray: Glint would start where nothing shows it.
    setStartWithWindows(startWithWindows && keepInTray).catch(() => {});
  }, [startWithWindows, keepInTray]);

  // Painted: the window can show (it's made hidden, so it never flashes white first).
  useEffect(() => {
    let frame = requestAnimationFrame(() => (frame = requestAnimationFrame(() => void invoke("app_ready"))));
    return () => cancelAnimationFrame(frame);
  }, []);

  useEffect(() => {
    void greetUpdate();
    checkForUpdates();
    const timer = setInterval(checkForUpdates, UPDATE_CHECK_MS);
    return () => {
      clearInterval(timer);
      unlisten.current?.();
    };
  }, []);

  // Folders handed over by File Explorer or a later launch of the app open in new tabs, in front of
  // Text mode and Settings.
  useEffect(() => {
    const open = () => openRequested().then((opened) => opened && settingsRef.current?.close());
    const stop = listen("open-requested", open);
    // Any that came in before the listener was ready.
    stop.then(open);
    return () => {
      stop.then((f) => f());
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const ctrl = e.ctrlKey || e.metaKey;
      // Esc in a field leaves it first; the next one stops the search.
      if (e.key === "Escape" && searching && mode === "text" && !(e.target as Element).closest("dialog, [popover], input, textarea")) {
        cancelSearch();
      } else if (e.key === "F1" || (ctrl && e.key === "/")) {
        e.preventDefault();
        // In front of everything, Settings included.
        settingsRef.current?.close();
        openShortcuts();
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

  const search = async (query: string, path: string, limit = useStore.getState().settings.maxResults, everything = false) => {
    if (!query || !path) return;
    const { options, settings, addToHistory } = useStore.getState();
    const id = crypto.randomUUID();
    const isCurrent = () => runId.current === id;
    runId.current = id;
    unlisten.current?.();
    unlisten.current = undefined;

    setRun({ id, query, path, options, limit, everything });
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

    const { stop, folder } = await searchBatched(
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
      { ...options, exclusions: settings.exclusions, maxResults: limit, everything }
    );
    // A newer search may have started while this one was spinning up.
    if (!isCurrent()) return stop();
    unlisten.current = stop;
    // Paths show relative to the folder as searched (c:/proj\ or %USERPROFILE% typed in the box).
    if (folder && folder !== path) setRun((r) => (r?.id === id ? { ...r, path: folder } : r));
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
    const { query, path, setPath } = useStore.getState();
    if (!query) return;
    let target = path.trim();
    if (!target) {
      target = defaultTextSearchFolder();
      if (target) setPath(target);
    }
    if (!target) {
      toast.message("Choose a folder to search in");
      return;
    }
    search(query, target);
  };

  const pickHistory = (item: HistoryItem) => {
    const { setQuery, setPath } = useStore.getState();
    setQuery(item.query);
    setPath(item.path);
    search(item.query, item.path);
  };

  // In the editor from Settings, else with the file's app as File Explorer opens it.
  const openFile = (path: string) =>
    (settings.editorPath.trim() ? openPath(path, settings.editorPath.trim()) : openItem(path)).catch((e) => toast.error("Couldn't open the file", { description: String(e) }));

  const replaceAll = async (replacement: string) => {
    // One at a time: a second Enter mid-way would replace in what the first just wrote.
    if (!run || searching || replacing.current) return;
    const files = [...new Set(matches.map((m) => m.data.path?.text ?? ""))].filter(Boolean);
    if (!files.length) return;

    try {
      toRegExp(run.query, run.options);
    } catch (e) {
      toast.error("This pattern can't be used for replacing", { description: String(e) });
      return;
    }
    replacing.current = true;
    try {
      const confirmed = !settings.confirmBeforeReplace || await confirm(
        `Replace every match of “${run.query}” with “${replacement}” in ${files.length} ${files.length === 1 ? "file" : "files"}?\n\nThis can't be undone.`,
        { title: "Replace all", kind: "warning", okLabel: "Replace all", cancelLabel: "Cancel" }
      );
      if (!confirmed) return;

      const toastId = toast.loading(`Replacing in ${files.length} files…`);
      let changed = 0;
      let failed = 0;
      for (const path of files) {
        try {
          const content = await invoke<string>("read_file_content", { path });
          const next = replaceText(content, run.query, run.options, replacement);
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
    } finally {
      replacing.current = false;
    }
    // What's left to replace, unless a newer search started meanwhile.
    if (runId.current === run.id) search(run.query, run.path, run.limit);
  };

  // With auto-preview on, the first result is shown until the user picks another.
  const current = selected >= 0 ? selected : settings.autoOpenPreview && matches.length ? 0 : -1;

  return (
    <div className="flex h-full flex-col">
      {/* The window's title bar (it has no system one): drag it by any empty spot, double-click to maximize. */}
      <header data-tauri-drag-region="deep" className="flex h-11 shrink-0 items-center gap-2 border-b bg-chrome pl-3">
        <img src={logo} alt="" draggable={false} className="size-[18px] shrink-0 max-sm:hidden" />
        {/* Text keeps the folder it searched last; the search bar fills an empty one when it opens. */}
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
        {mode === "text" && <RipgrepCard />}
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
            onReveal={revealInApp}
            onShowAll={() => search(run.query, run.path, null, run.everything)}
            onSearchAgain={(everything) => search(run.query, run.path, run.limit, everything)}
            onClose={closeResults}
          />
        ) : (
          <Home onPick={pickHistory} />
        )}
      </div>

      {/* Over either mode: Text's results open X-Ray and the locks dialog too. */}
      <LocksDialog />
      <DuplicatesDialog />
      <CleanupDialog />
      <CompareFilesDialog />
      <TimelineDialog />
      <HistoryDialog />
      <BatchRenameDialog />
      <XRayDialog />
      <ShortcutsDialog />
      <HistoryPopover onPick={pickHistory} />
      <dialog
        ref={settingsRef}
        onClick={(e) => e.target === e.currentTarget && e.currentTarget.close()}
        className="m-auto h-[min(85vh,46rem)] w-[min(92vw,54rem)] overflow-hidden rounded-xl border bg-chrome p-0 text-foreground shadow-2xl backdrop:bg-black/50"
      >
        <SettingsView onClose={() => settingsRef.current?.close()} quickTaken={quickTaken} />
        {/* The dialog sits above everything else, toasts included: its own show them over it. */}
        <Toaster theme={settings.theme} position="bottom-right" richColors />
      </dialog>
      <Toaster theme={settings.theme} position="bottom-right" richColors />
    </div>
  );
}

export default App;
