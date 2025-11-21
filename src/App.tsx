import { useState, useEffect, useRef } from 'react';
import { Search } from './components/Search';
import { Results } from './components/Results';
import { SearchStats } from './components/SearchStats';
import { Preview } from './components/Preview';
import { search, checkRgInstalled, RgMatch } from './lib/ripgrep';
import { useStore } from './lib/store';
import { AlertTriangle } from 'lucide-react';
import { Child } from "@tauri-apps/plugin-shell";
import { Toaster, toast } from "sonner";
import * as opener from "@tauri-apps/plugin-opener";

function App() {
  const [results, setResults] = useState<RgMatch[]>([]);
  const [searchedQuery, setSearchedQuery] = useState("");
  const [isSearching, setIsSearching] = useState(false);
  const [rgInstalled, setRgInstalled] = useState<boolean | null>(null);
  const [searchDuration, setSearchDuration] = useState(0);
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const startTimeRef = useRef<number>(0);
  const { addToHistory, options } = useStore();
  const currentProcess = useRef<Child | null>(null);

  useEffect(() => {
    checkRgInstalled().then(setRgInstalled);

    return () => {
      if (currentProcess.current) {
        currentProcess.current.kill();
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
    if (currentProcess.current) {
      await currentProcess.current.kill();
      currentProcess.current = null;
    }

    setIsSearching(true);
    setResults([]);
    setSearchedQuery(query);
    setSearchDuration(0);
    setSelectedIndex(-1);
    startTimeRef.current = performance.now();
    addToHistory(query, path);

    try {
      const child = await search(
        query,
        path,
        (events) => {
          setResults((prev) => [...prev, ...events]);
        },
        () => {
          setIsSearching(false);
          setSearchDuration(performance.now() - startTimeRef.current);
          currentProcess.current = null;
        },
        options
      );
      currentProcess.current = child;
    } catch (e) {
      console.error(e);
      setIsSearching(false);
      setSearchDuration(performance.now() - startTimeRef.current);
    }
  };

  const handleStop = async () => {
    if (currentProcess.current) {
      await currentProcess.current.kill();
      currentProcess.current = null;
      setIsSearching(false);
      setSearchDuration(performance.now() - startTimeRef.current);
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
    <div className="min-h-screen bg-black text-foreground font-sans selection:bg-pink-500/30">
      <div className="fixed inset-0 bg-[radial-gradient(ellipse_at_top,_var(--tw-gradient-stops))] from-zinc-900 via-black to-black -z-10"></div>

      <div className="container mx-auto pt-10 px-4 pb-4 flex flex-col h-screen">
        <div className="text-center mb-6 space-y-0 shrink-0">
          <h1 className="text-4xl md:text-6xl pb-4 text-left font-bold bg-clip-text text-transparent bg-gradient-to-r from-white to-zinc-500 tracking-tight">
            ShadUI Ripgrep
          </h1>
          <p className="text-zinc-500 text-lg text-left">
            Blazing fast text search
          </p>
        </div>

        <div className="shrink-0 mb-4">
          <Search onSearch={handleSearch} onStop={handleStop} isSearching={isSearching} />
          <SearchStats results={results} isSearching={isSearching} duration={searchDuration} />
        </div>

        <div className="flex-1 min-h-0 flex gap-4">
          <div className="flex-1 min-w-0 border rounded-xl bg-zinc-900/50 backdrop-blur-sm overflow-hidden">
            <Results
              results={results}
              query={searchedQuery}
              selectedIndex={selectedIndex}
              onOpenFile={handleOpenFile}
            />
          </div>
          <div className="flex-1 min-w-0 border rounded-xl bg-zinc-900/50 backdrop-blur-sm overflow-hidden">
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
