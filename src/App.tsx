import { useState, useEffect, useRef } from 'react';
import { Search } from './components/Search';
import { Results } from './components/Results';
import { SearchStats } from './components/SearchStats';
import { Preview } from './components/Preview';
import { searchBatched, checkRgInstalled, cancelSearch, RgMatch } from './lib/ripgrep';
import { useStore } from './lib/store';
import { AlertTriangle, Telescope } from 'lucide-react';
import { Toaster, toast } from "sonner";
import * as opener from "@tauri-apps/plugin-opener";
import { invoke } from "@tauri-apps/api/core";
import { useTheme } from "./hooks/useTheme";
import { DisplayItem } from "./types";
import { Sidebar } from "./components/Sidebar";
import { Footer } from "./components/Footer";
import { Settings } from "./components/Settings";

function App() {
  useTheme(); // Initialize theme system
  const [results, setResults] = useState<RgMatch[]>([]);
  const [displayItems, setDisplayItems] = useState<DisplayItem[]>([]);
  const [searchedQuery, setSearchedQuery] = useState("");
  const [isSearching, setIsSearching] = useState(false);
  const [rgInstalled, setRgInstalled] = useState<boolean | null>(null);
  const [searchDuration, setSearchDuration] = useState(0);
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const startTimeRef = useRef<number>(0);
  const { addToHistory, options, settings } = useStore();
  const workerRef = useRef<Worker | null>(null);
  const unlistenRef = useRef<(() => void) | null>(null);
  const gotSummaryRef = useRef(false);

  useEffect(() => {
    checkRgInstalled().then(setRgInstalled);

    // Initialize worker
    workerRef.current = new Worker(new URL('./search.worker.ts', import.meta.url), { type: 'module' });

    workerRef.current.onmessage = (e) => {
      const { type, matches, displayItems: newDisplayItems } = e.data;
      if (type === 'results') {
        if (matches && matches.length > 0) {
          setResults(prev => [...prev, ...matches]);

          // Check for summary to get precise duration
          const summary = matches.find((m: RgMatch) => m.type === 'summary');
          if (summary?.data?.elapsed_total?.secs !== undefined) {
            const { secs, nanos } = summary.data.elapsed_total;
            setSearchDuration(secs * 1000 + nanos / 1_000_000);
            gotSummaryRef.current = true;
          }
        }
        if (newDisplayItems && newDisplayItems.length > 0) {
          setDisplayItems(prev => [...prev, ...newDisplayItems]);
        }
      }
    };

    return () => {
      if (unlistenRef.current) {
        unlistenRef.current();
      }
      if (workerRef.current) {
        workerRef.current.terminate();
      }
    };
  }, []);

  const handleOpenFile = async (filePath: string) => {
    try {
      await opener.openPath(filePath);
      toast.success("Opened successfully", {
        description: filePath.split(/[\\/]/).pop(),
      });
    } catch (error) {
      console.error("Failed to open file:", error);
      toast.error("Failed to open path", {
        description: error instanceof Error ? error.message : "Unknown error occurred",
      });
    }
  };

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (results.length === 0) return;

      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelectedIndex((prev) => Math.min(prev + 1, results.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelectedIndex((prev) => Math.max(prev - 1, 0));
      } else if (e.key === "Enter" && selectedIndex >= 0) {
        e.preventDefault();
        const match = results[selectedIndex];
        if (match && match.data.path?.text) {
          handleOpenFile(match.data.path.text);
        }
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [results, selectedIndex]);

  const handleSearch = async (query: string, path: string) => {
    if (unlistenRef.current) {
      unlistenRef.current();
      unlistenRef.current = null;
    }

    if (query.trim() && path.trim()) {
      addToHistory(query, path);
    }

    startTimeRef.current = performance.now();
    gotSummaryRef.current = false;
    setIsSearching(true);
    setResults([]);
    setDisplayItems([]);
    setSearchDuration(0);


    try {
      const unlisten = await searchBatched(
        query,
        path,
        (lines) => {
          // Forward raw lines to worker
          workerRef.current?.postMessage({ type: 'data', payload: lines });
        },
        () => {
          // Flush remaining worker buffer
          workerRef.current?.postMessage({ type: 'flush' });

          setIsSearching(false);
          if (!gotSummaryRef.current) {
            setSearchDuration(performance.now() - startTimeRef.current);
          }
          unlistenRef.current = null;
        },
        (error) => {
          console.error("Search error:", error);
          setIsSearching(false);
          setSearchDuration(performance.now() - startTimeRef.current);
        },
        { ...options, exclusions: settings.exclusions }
      );
      unlistenRef.current = unlisten;
    } catch (e) {
      console.error(e);
      setSearchDuration(performance.now() - startTimeRef.current);
      setIsSearching(false);
    }
  };

  const handleStop = async () => {
    await cancelSearch();
    if (unlistenRef.current) {
      unlistenRef.current();
      unlistenRef.current = null;
    }
    setIsSearching(false);
    setSearchDuration(performance.now() - startTimeRef.current);
  };

  const handleReplace = async (replaceText: string) => {
    if (!searchedQuery) return;

    const matchesByFile = new Map<string, RgMatch[]>();
    results.forEach(r => {
      if (r.type === 'match' && r.data.path?.text) {
        const path = r.data.path.text;
        if (!matchesByFile.has(path)) {
          matchesByFile.set(path, []);
        }
        matchesByFile.get(path)?.push(r);
      }
    });

    let replacedCount = 0;
    let errorCount = 0;

    const toastId = toast.loading("Replacing...");

    for (const [filePath] of matchesByFile) {
      try {
        const content = await invoke<string>("read_file_content", { path: filePath });

        let newContent = content;
        if (options.regex) {
          const flags = options.caseSensitive ? 'g' : 'gi';
          newContent = content.replace(new RegExp(searchedQuery, flags), replaceText);
        } else {
          if (options.wholeWord) {
            const flags = options.caseSensitive ? 'g' : 'gi';
            const escaped = searchedQuery.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
            newContent = content.replace(new RegExp(`\\b${escaped}\\b`, flags), replaceText);
          } else {
            if (options.caseSensitive) {
              newContent = content.split(searchedQuery).join(replaceText);
            } else {
              const escaped = searchedQuery.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
              newContent = content.replace(new RegExp(escaped, 'gi'), replaceText);
            }
          }
        }

        if (newContent !== content) {
          await invoke("write_file_content", { path: filePath, content: newContent });
          replacedCount++;
        }
      } catch (e) {
        console.error(`Failed to replace in ${filePath}`, e);
        errorCount++;
      }
    }

    toast.dismiss(toastId);

    if (replacedCount > 0) {
      toast.success(`Replaced in ${replacedCount} files` + (errorCount > 0 ? ` (${errorCount} failed)` : ""));
      handleSearch(searchedQuery, useStore.getState().path);
    } else if (errorCount > 0) {
      toast.error(`Failed to replace in ${errorCount} files`);
    } else {
      toast.info("No occurrences replaced");
    }
  };

  const [activeView, setActiveView] = useState<'search' | 'settings'>('search');
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);

  // Handle Sidebar Navigation
  const handleNavigate = (view: 'search' | 'settings') => {
    setActiveView(view);
    if (view === 'settings') setIsSettingsOpen(true);
    if (view === 'search') {
      // Just focus search or do nothing special
    }
  };

  if (rgInstalled === false) {
    return (
      <div className="min-h-screen bg-black text-white flex items-center justify-center p-4">
        <div className="bg-zinc-900 border border-red-900/50 p-6 rounded-xl max-w-md text-center space-y-4">
          <AlertTriangle className="w-12 h-12 text-red-500 mx-auto" />
          <h2 className="text-xl font-bold">Ripgrep not found</h2>
          <p className="text-zinc-400">
            Please install ripgrep to use this application.
            <br />
            <code>cargo install ripgrep</code>
          </p>
        </div>
        <Toaster position="top-right" richColors />
      </div>
    );
  }

  const selectedMatch = selectedIndex >= 0 ? results[selectedIndex] : null;

  return (
    <div className="flex h-screen bg-white dark:bg-black text-foreground font-sans selection:bg-pink-500/30 transition-colors duration-300 overflow-hidden">
      {/* Background */}
      <div className="fixed inset-0 bg-[radial-gradient(ellipse_at_top_right,_var(--tw-gradient-stops))] from-indigo-100 via-zinc-50 to-white dark:from-zinc-950 dark:via-zinc-900 dark:to-black -z-10 transition-colors duration-300">
        <div className="absolute inset-0 bg-[url('https://grainy-gradients.vercel.app/noise.svg')] opacity-20 brightness-100 contrast-150 mix-blend-overlay"></div>
        <div className="absolute inset-0 bg-gradient-to-tr from-pink-500/5 via-transparent to-blue-500/5 dark:from-pink-500/10 dark:to-blue-500/10 pointer-events-none"></div>
      </div>

      {/* Sidebar */}
      <Sidebar
        activeView={activeView}
        onNavigate={handleNavigate}
        onHistorySelect={(q, p) => {
          setSearchedQuery(q);
          useStore.getState().setQuery(q);
          useStore.getState().setPath(p);
          handleSearch(q, p);
        }}
      />

      {/* Main Content */}
      <div className="flex-1 flex flex-col min-w-0 relative">
        {/* Header / Search Area */}
        <div className="shrink-0 p-6 pb-2">
          <Search
            onSearch={handleSearch}
            onStop={handleStop}
            isSearching={isSearching}
            onReplace={handleReplace}
          />
          <div className="mt-4">
            <SearchStats results={results} isSearching={isSearching} duration={searchDuration} />
          </div>
        </div>

        {/* Content Area */}
        <div className="flex-1 min-h-0 p-6 pt-2 flex gap-4 overflow-hidden">
          {results.length === 0 && !isSearching ? (
            <div className="flex-1 flex flex-col items-center justify-center text-center opacity-50 select-none relative overflow-hidden">
              <div className="w-96 h-96 bg-gradient-to-tr from-pink-500/10 to-purple-500/10 rounded-full blur-3xl absolute -z-10 animate-pulse"></div>
              <div className="relative">
                <div className="absolute inset-0 bg-pink-500/20 blur-xl rounded-full animate-pulse"></div>
                <Telescope className="w-32 h-32 text-zinc-300 dark:text-zinc-800 mb-8 relative z-10" strokeWidth={1} />
              </div>
              <h2 className="text-3xl font-bold text-zinc-400 dark:text-zinc-600 mb-3 tracking-tight">Ready to Explore</h2>
              <p className="text-zinc-400 dark:text-zinc-600 max-w-md text-lg leading-relaxed">
                Enter a search term and path to begin exploring your codebase with <span className="text-pink-500 font-medium">lightning speed</span>.
              </p>
            </div>
          ) : (
            <>
              <div className="flex-1 min-w-0 border border-zinc-200 dark:border-zinc-800 rounded-2xl bg-white/40 dark:bg-zinc-900/40 backdrop-blur-md overflow-hidden shadow-sm">
                <Results
                  results={results}
                  displayItems={displayItems}
                  query={searchedQuery}
                  selectedIndex={selectedIndex}
                  onOpenFile={handleOpenFile}
                  onSelect={setSelectedIndex}
                />
              </div>
              <div className="flex-1 min-w-0 border border-zinc-200 dark:border-zinc-800 rounded-2xl bg-white/40 dark:bg-zinc-900/40 backdrop-blur-md overflow-hidden shadow-sm">
                <Preview
                  filePath={selectedMatch?.data.path?.text || null}
                  lineNumber={selectedMatch?.data.line_number}
                />
              </div>
            </>
          )}
        </div>

        {/* Footer */}
        <Footer />
      </div>

      {/* Modals */}
      <Settings
        isOpen={isSettingsOpen}
        onOpenChange={(open) => { setIsSettingsOpen(open); if (!open) setActiveView('search'); }}
      />

      <Toaster position="top-right" richColors />
    </div>
  );
}

export default App;
