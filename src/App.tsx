import { useState, useEffect, useRef } from 'react';
import { Search } from './components/Search';
import { Results } from './components/Results';
import { SearchStats } from './components/SearchStats';
import { Preview } from './components/Preview';
import { searchBatched, checkRgInstalled, cancelSearch, RgMatch } from './lib/ripgrep';
import { useStore } from './lib/store';
import { AlertTriangle } from 'lucide-react';
import { Toaster, toast } from "sonner";
import * as opener from "@tauri-apps/plugin-opener";
import { invoke } from "@tauri-apps/api/core";
import { useTheme } from "./hooks/useTheme";
import { DisplayItem } from "./types";

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

  useEffect(() => {
    checkRgInstalled().then(setRgInstalled);

    // Initialize worker
    workerRef.current = new Worker(new URL('./search.worker.ts', import.meta.url), { type: 'module' });

    workerRef.current.onmessage = (e) => {
      const { type, matches, displayItems: newDisplayItems } = e.data;
      if (type === 'results') {
        if (matches && matches.length > 0) {
          setResults(prev => [...prev, ...matches]);
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

    // Cancel any running search on the backend
    await cancelSearch();

    setIsSearching(true);
    setResults([]);
    setDisplayItems([]);
    setSearchedQuery(query);
    setSearchDuration(0);
    setSelectedIndex(-1);
    startTimeRef.current = performance.now();
    addToHistory(query, path);

    // Reset worker state
    workerRef.current?.postMessage({ type: 'reset' });

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
          setSearchDuration(performance.now() - startTimeRef.current);
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
    <div className="min-h-screen bg-white dark:bg-black text-foreground font-sans selection:bg-pink-500/30 transition-colors duration-300">
      <div className="fixed inset-0 bg-[radial-gradient(ellipse_at_top,_var(--tw-gradient-stops))] from-slate-100 via-white to-slate-50 dark:from-zinc-900 dark:via-black dark:to-black -z-10 transition-colors duration-300"></div>

      <div className="container mx-auto pt-10 px-4 pb-4 flex flex-col h-screen">
        <div className="text-center mb-6 space-y-0 shrink-0">
          <h1 className="text-4xl md:text-6xl pb-4 text-left font-bold bg-clip-text text-transparent bg-gradient-to-r from-zinc-900 to-zinc-600 dark:from-white dark:to-zinc-500 tracking-tight transition-colors duration-300">
            ShadUI Ripgrep
          </h1>
          <p className="text-zinc-600 dark:text-zinc-500 text-lg text-left transition-colors duration-300">
            Blazing fast text search
          </p>
        </div>

        <div className="shrink-0 mb-4">
          <Search onSearch={handleSearch} onStop={handleStop} isSearching={isSearching} onReplace={handleReplace} />
          <SearchStats results={results} isSearching={isSearching} duration={searchDuration} />
        </div>

        <div className="flex-1 min-h-0 flex gap-4">
          <div className="flex-1 min-w-0 border border-zinc-300 dark:border-zinc-800 rounded-xl bg-white/50 dark:bg-zinc-900/50 backdrop-blur-sm overflow-hidden">
            <Results
              results={results}
              displayItems={displayItems}
              query={searchedQuery}
              selectedIndex={selectedIndex}
              onOpenFile={handleOpenFile}
              onSelect={setSelectedIndex}
            />
          </div>
          <div className="flex-1 min-w-0 border border-zinc-300 dark:border-zinc-800 rounded-xl bg-white/50 dark:bg-zinc-900/50 backdrop-blur-sm overflow-hidden">
            <Preview
              filePath={selectedMatch?.data.path?.text || null}
              lineNumber={selectedMatch?.data.line_number}
            />
          </div>
        </div>
      </div>
      <Toaster position="top-right" richColors />
    </div>
  );
}

export default App;
