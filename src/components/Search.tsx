import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "@/lib/store";
import { Search as SearchIcon, Folder, StopCircle, CaseSensitive, WholeWord, Regex, History as HistoryIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import { useState } from "react";
import { History } from "./History";

interface SearchProps {
    onSearch: (query: string, path: string) => void;
    onStop: () => void;
    isSearching: boolean;
}

export function Search({ onSearch, onStop, isSearching }: SearchProps) {
    const { query, setQuery, path, setPath, options, setOption } = useStore();
    const [isHistoryOpen, setIsHistoryOpen] = useState(false);

    const handleSubmit = (e: React.FormEvent) => {
        e.preventDefault();
        if (query && path) {
            onSearch(query, path);
        }
    };

    const handleBrowse = async () => {
        const selected = await open({
            directory: true,
            multiple: false,
            defaultPath: path || undefined,
        });
        if (selected) {
            setPath(selected as string);
        }
    };

    return (
        <div className="w-full max-w-3xl mx-auto">
            <form onSubmit={handleSubmit} className="space-y-4">
                <div className="relative group">
                    <input
                        type="text"
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        placeholder="Search pattern..."
                        className="w-full pl-6 pr-32 py-4 bg-zinc-900/80 border border-zinc-800 rounded-xl text-zinc-100 placeholder:text-zinc-500 focus:outline-none focus:ring-2 focus:ring-pink-500/50 focus:border-transparent transition-all backdrop-blur-sm"
                        disabled={isSearching}
                    />
                    <div className="absolute right-4 top-1/2 -translate-y-1/2 flex items-center gap-1">
                        <button
                            type="button"
                            onClick={() => setOption('caseSensitive', !options.caseSensitive)}
                            className={cn(
                                "p-1.5 rounded-md transition-colors",
                                options.caseSensitive ? "bg-pink-500/20 text-pink-400" : "text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800"
                            )}
                            title="Case Sensitive"
                        >
                            <CaseSensitive className="w-4 h-4" />
                        </button>
                        <button
                            type="button"
                            onClick={() => setOption('wholeWord', !options.wholeWord)}
                            className={cn(
                                "p-1.5 rounded-md transition-colors",
                                options.wholeWord ? "bg-pink-500/20 text-pink-400" : "text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800"
                            )}
                            title="Whole Word"
                        >
                            <WholeWord className="w-4 h-4" />
                        </button>
                        <button
                            type="button"
                            onClick={() => setOption('regex', !options.regex)}
                            className={cn(
                                "p-1.5 rounded-md transition-colors",
                                options.regex ? "bg-pink-500/20 text-pink-400" : "text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800"
                            )}
                            title="Regular Expression"
                        >
                            <Regex className="w-4 h-4" />
                        </button>
                        <div className="w-px h-4 bg-zinc-800 mx-1" />
                        <SearchIcon className="w-5 h-5 text-zinc-500 group-focus-within:text-pink-500 transition-colors" />
                    </div>
                </div>

                <div className="flex gap-3">
                    <div className="relative flex-1 group">
                        <input
                            type="text"
                            value={path}
                            onChange={(e) => setPath(e.target.value)}
                            placeholder="Search directory..."
                            className="w-full px-6 py-3 bg-zinc-900/80 border border-zinc-800 rounded-xl text-zinc-100 placeholder:text-zinc-500 focus:outline-none focus:ring-2 focus:ring-pink-500/50 focus:border-transparent transition-all backdrop-blur-sm"
                            disabled={isSearching}
                        />
                        <Folder className="absolute right-4 top-1/2 -translate-y-1/2 w-4 h-4 text-zinc-500 group-focus-within:text-pink-500 transition-colors" />
                    </div>
                    <button
                        type="button"
                        onClick={handleBrowse}
                        disabled={isSearching}
                        className="px-6 py-3 bg-zinc-800 hover:bg-zinc-700 border border-zinc-700 rounded-xl text-foreground transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                    >
                        Browse
                    </button>
                    <button
                        type="button"
                        onClick={() => setIsHistoryOpen(true)}
                        disabled={isSearching}
                        className="px-4 py-3 bg-zinc-800 hover:bg-zinc-700 border border-zinc-700 rounded-xl text-foreground transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                        title="Search History"
                    >
                        <HistoryIcon className="w-5 h-5" />
                    </button>
                    {isSearching ? (
                        <button
                            type="button"
                            onClick={onStop}
                            className="px-8 py-3 bg-red-600 hover:bg-red-500 rounded-xl text-white font-medium transition-all flex items-center gap-2"
                        >
                            <StopCircle className="w-4 h-4" />
                            Stop
                        </button>
                    ) : (
                        <button
                            type="submit"
                            disabled={!query || !path}
                            className="px-8 py-3 bg-gradient-to-r from-pink-600 to-purple-600 hover:from-pink-500 hover:to-purple-500 rounded-xl text-white font-medium transition-all disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:from-pink-600 disabled:hover:to-purple-600"
                        >
                            Search
                        </button>
                    )}
                </div>
            </form>

            <History
                isOpen={isHistoryOpen}
                onClose={() => setIsHistoryOpen(false)}
                onSelect={(q, p) => {
                    setQuery(q);
                    setPath(p);
                    onSearch(q, p);
                }}
            />
        </div>
    );
}
