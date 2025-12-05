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
import { SettingsView } from "./components/SettingsView";

function App() {
  useTheme(); // Initialize theme system
  const [results, setResults] = useState<RgMatch[]>([]);
  const [displayItems, setDisplayItems] = useState<DisplayItem[]>([]);
  const [searchedQuery, setSearchedQuery] = useState("");
  const [isSearching, setIsSearching] = useState(false);
  const [rgInstalled, setRgInstalled] = useState<boolean | null>(null);
  const [searchDuration, setSearchDuration] = useState(0);
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const [limitReached, setLimitReached] = useState(false);
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

  // Auto-select first result when search completes if setting is enabled
  useEffect(() => {
    if (!isSearching && results.length > 0 && settings.autoOpenPreview && selectedIndex < 0) {
      // Find the first match (not summary or other types)
      const firstMatchIndex = results.findIndex(r => r.type === 'match');
      if (firstMatchIndex >= 0) {
        setSelectedIndex(firstMatchIndex);
      }
    }
  }, [isSearching, results.length, settings.autoOpenPreview]);

  // Detect if search hit the limit
  useEffect(() => {
    if (!isSearching && results.length > 0 && settings.maxResults !== null) {
      const matchCount = results.filter(r => r.type === 'match').length;
      // Consider limit reached if we got exactly maxResults matches (or close to it)
      setLimitReached(matchCount >= settings.maxResults);
    }
  }, [isSearching, results.length, settings.maxResults]);

  // Font size class mapping
  const fontSizeClass = {
    small: 'text-sm',
    medium: 'text-base',
    large: 'text-lg',
  }[settings.fontSize] || 'text-base';

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
    setSearchedQuery(query); // Set searched query early

    // Reset worker state before new search
    workerRef.current?.postMessage({ type: 'reset' });

    // Always clear results when starting a new search
    setResults([]);
    setDisplayItems([]);
    setSearchDuration(0);
    setLimitReached(false);
    setSelectedIndex(-1); // Reset selection for new search


    try {
      const currentStore = useStore.getState();
      const currentOptions = currentStore.options;
      const currentSettings = currentStore.settings;
      const searchId = crypto.randomUUID();

      const unlisten = await searchBatched(
        query,
        path,
        searchId,
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
        { ...currentOptions, exclusions: currentSettings.exclusions, maxResults: currentSettings.maxResults }
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

  const handleRerunSearch = () => {
    const { query, path } = useStore.getState();
    if (query && path) {
      handleSearch(query, path);
    }
  };

  const handleReplace = async (replaceText: string) => {
    if (!searchedQuery) return;

    // Confirm before replace if setting is enabled
    if (settings.confirmBeforeReplace) {
      const confirmed = window.confirm(
        `Are you sure you want to replace all occurrences of "${searchedQuery}" with "${replaceText}"?`
      );
      if (!confirmed) return;
    }

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

  // Handle Sidebar Navigation
  const handleNavigate = (view: 'search' | 'settings') => {
    setActiveView(view);
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
    <div className={`flex h-screen bg-white dark:bg-black text-foreground font-sans selection:bg-pink-500/30 transition-colors duration-300 overflow-hidden ${fontSizeClass}`}>
      {/* Background */}
      <div className="fixed inset-0 bg-white dark:bg-zinc-950 -z-10 transition-colors duration-300">
        <div className="absolute inset-0 bg-[radial-gradient(circle_500px_at_50%_200px,#fbcfe8,transparent)] dark:bg-[radial-gradient(circle_500px_at_50%_200px,#3f2c35,transparent)] opacity-40 blur-3xl"></div>
        <div className="absolute top-0 right-0 w-full h-full bg-[radial-gradient(circle_800px_at_100%_0%,#e9d5ff,transparent)] dark:bg-[radial-gradient(circle_800px_at_100%_0%,#271b33,transparent)] opacity-30 blur-3xl"></div>
        <div className="absolute bottom-0 left-0 w-full h-full bg-[radial-gradient(circle_800px_at_0%_100%,#f0f9ff,transparent)] dark:bg-[radial-gradient(circle_800px_at_0%_100%,#182836,transparent)] opacity-30 blur-3xl"></div>
        <div className="absolute inset-0 bg-[url('https://grainy-gradients.vercel.app/noise.svg')] opacity-10 brightness-100 contrast-150 mix-blend-overlay"></div>
      </div>

      {/* Sidebar */}
      {/* Sidebar */}
      <div className="transition-all duration-300">
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
      </div>

      {/* Main Content */}
      <div className="flex-1 flex flex-col min-w-0 relative">
        {activeView === 'settings' ? (
          <SettingsView />
        ) : (
          <>
            {/* Header / Stats Area */}
            <div className="shrink-0 px-6 pt-4 pb-2">
              <SearchStats results={results} isSearching={isSearching} duration={searchDuration} onRerunSearch={handleRerunSearch} />
            </div>

            {/* Content Area */}
            <div className="flex-1 min-h-0 p-6 pt-2 pb-0 flex gap-4 overflow-hidden relative">
              {results.length === 0 && !isSearching ? (
                <div className="flex-1 flex flex-col items-center justify-center text-center opacity-50 select-none relative overflow-hidden mb-20 animate-in fade-in duration-500">
                  <div className="w-96 h-96 bg-gradient-to-tr from-pink-500/10 to-purple-500/10 rounded-full blur-3xl absolute -z-10 animate-pulse"></div>
                  <div className="relative">
                    <div className="absolute inset-0 bg-pink-500/20 blur-xl rounded-full animate-pulse"></div>
                    <Telescope className="w-32 h-32 text-zinc-300 dark:text-zinc-800 mb-8 relative z-10" strokeWidth={1} />
                  </div>
                  <h2 className="text-3xl font-bold text-zinc-400 dark:text-zinc-600 mb-3 tracking-tight">Ready to Explore</h2>
                  <p className="text-zinc-400 dark:text-zinc-600 max-w-md text-lg leading-relaxed mb-8">
                    Enter a search term and path below to begin.
                  </p>

                  {/* Quick Actions / Recent */}
                  <div className="flex flex-col gap-4 items-center">
                    {useStore.getState().history.length > 0 && (
                      <div className="flex flex-wrap gap-2 justify-center max-w-lg">
                        {useStore.getState().history.slice(0, 3).map((item, i) => (
                          <button
                            key={i}
                            onClick={() => {
                              setSearchedQuery(item.query);
                              useStore.getState().setQuery(item.query);
                              useStore.getState().setPath(item.path);
                              handleSearch(item.query, item.path);
                            }}
                            className="text-xs px-3 py-1.5 rounded-full bg-zinc-100 dark:bg-zinc-800/50 text-zinc-500 hover:bg-pink-100 hover:text-pink-600 dark:hover:bg-pink-900/20 dark:hover:text-pink-400 transition-colors border border-transparent hover:border-pink-200 dark:hover:border-pink-800"
                          >
                            {item.query}
                          </button>
                        ))}
                      </div>
                    )}

                    <div className="flex gap-4 text-xs text-zinc-400/70 mt-4">
                      <span className="flex items-center gap-1"><kbd className="bg-zinc-200 dark:bg-zinc-800 px-1 rounded">↵</kbd> Search</span>
                      <span className="flex items-center gap-1"><kbd className="bg-zinc-200 dark:bg-zinc-800 px-1 rounded">⇧ ↵</kbd> New line</span>
                    </div>
                  </div>
                </div>
              ) : (
                <>
                  <div className="flex-1 min-w-0 border border-zinc-200 dark:border-zinc-800 rounded-2xl bg-white/40 dark:bg-zinc-900/40 backdrop-blur-md overflow-hidden shadow-sm mb-24">
                    <Results
                      results={results}
                      displayItems={displayItems}
                      query={searchedQuery}
                      selectedIndex={selectedIndex}
                      onOpenFile={handleOpenFile}
                      onSelect={setSelectedIndex}
                      limitReached={limitReached}
                      onRerunSearch={handleRerunSearch}
                    />
                  </div>
                  <div className="flex-1 min-w-0 border border-zinc-200 dark:border-zinc-800 rounded-2xl bg-white/40 dark:bg-zinc-900/40 backdrop-blur-md overflow-hidden shadow-sm mb-24">
                    <Preview
                      filePath={selectedMatch?.data.path?.text || null}
                      lineNumber={selectedMatch?.data.line_number}
                    />
                  </div>
                </>
              )}
            </div>

            {/* Bottom Search Area */}
            <div className="absolute bottom-0 left-0 right-0 p-6 bg-gradient-to-t from-white via-white to-transparent dark:from-black dark:via-black dark:to-transparent z-50">
              <Search
                onSearch={handleSearch}
                onStop={handleStop}
                isSearching={isSearching}
                onReplace={handleReplace}
              />
              {import.meta.env.DEV && (
                <div className="flex gap-2 mt-2 justify-end absolute top-full">
                  <button
                    onClick={async () => {
                      const path = "C:\\Repos";
                      // Trigger heavy search
                      handleSearch("Class", path);
                      // Wait brief moment
                      await new Promise(r => setTimeout(r, 50));
                      // Trigger empty/small search
                      handleSearch("ZXZXZXZX_UNIQUE_STRING", path);
                    }}
                    className="px-2 py-1 text-xs bg-red-500/10 text-red-500 rounded hover:bg-red-500/20"
                  >
                    Run Race Test
                  </button>
                </div>
              )}
              <Footer />
            </div>
          </>
        )}
      </div>

      <Toaster position="top-right" richColors />
    </div>
  );
}

export default App;
