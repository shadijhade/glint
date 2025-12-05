import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "@/lib/store";
import { Search as SearchIcon, Folder, StopCircle, CaseSensitive, WholeWord, Regex, Replace, Play } from "lucide-react";
import { Loader } from "@/components/ui/Loader";
import { cn } from "@/lib/utils";
import { useState, useEffect } from "react";


interface SearchProps {
    onSearch: (query: string, path: string) => void;
    onStop: () => void;
    isSearching: boolean;
    onReplace: (replaceText: string) => void;
}

export function Search({ onSearch, onStop, isSearching, onReplace }: SearchProps) {
    const { query, setQuery, path, setPath, options, setOption, settings } = useStore();
    const [isReplaceOpen, setIsReplaceOpen] = useState(false);
    const [replaceText, setReplaceText] = useState("");

    // Set default search path on mount if path is empty
    useEffect(() => {
        if (!path && settings.defaultSearchPath) {
            setPath(settings.defaultSearchPath);
        }
    }, []); // Only run on mount

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
                {/* Search Input */}
                <div className="relative group">
                    <input
                        type="text"
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        placeholder="Search pattern..."
                        className="w-full pl-6 pr-32 py-4 bg-white/90 dark:bg-zinc-900/80 border border-zinc-300 dark:border-zinc-800 rounded-xl text-zinc-900 dark:text-zinc-100 placeholder:text-zinc-400 dark:placeholder:text-zinc-500 focus:outline-none focus:ring-2 focus:ring-pink-500/50 focus:border-transparent transition-all backdrop-blur-sm"
                        disabled={isSearching}
                    />
                    <div className="absolute right-4 top-1/2 -translate-y-1/2 flex items-center gap-1">
                        <button
                            type="button"
                            onClick={() => setOption('caseSensitive', !options.caseSensitive)}
                            className={cn(
                                "p-1.5 rounded-md transition-colors",
                                options.caseSensitive ? "bg-pink-500/20 text-pink-400" : "text-zinc-600 hover:text-zinc-900 hover:bg-zinc-200 dark:text-zinc-500 dark:hover:text-zinc-300 dark:hover:bg-zinc-800"
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
                                options.wholeWord ? "bg-pink-500/20 text-pink-400" : "text-zinc-600 hover:text-zinc-900 hover:bg-zinc-200 dark:text-zinc-500 dark:hover:text-zinc-300 dark:hover:bg-zinc-800"
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
                                options.regex ? "bg-pink-500/20 text-pink-400" : "text-zinc-600 hover:text-zinc-900 hover:bg-zinc-200 dark:text-zinc-500 dark:hover:text-zinc-300 dark:hover:bg-zinc-800"
                            )}
                            title="Regular Expression"
                        >
                            <Regex className="w-4 h-4" />
                        </button>
                        <div className="w-px h-4 bg-zinc-300 dark:bg-zinc-800 mx-1" />
                        <SearchIcon className="w-5 h-5 text-zinc-600 dark:text-zinc-500 group-focus-within:text-pink-500 transition-colors" />
                    </div>
                </div>

                {/* Replace Input */}
                {isReplaceOpen && (
                    <div className="relative group animate-in slide-in-from-top-2 fade-in duration-200">
                        <input
                            type="text"
                            value={replaceText}
                            onChange={(e) => setReplaceText(e.target.value)}
                            placeholder="Replace with..."
                            className="w-full pl-6 pr-32 py-4 bg-white/90 dark:bg-zinc-900/80 border border-zinc-300 dark:border-zinc-800 rounded-xl text-zinc-900 dark:text-zinc-100 placeholder:text-zinc-400 dark:placeholder:text-zinc-500 focus:outline-none focus:ring-2 focus:ring-pink-500/50 focus:border-transparent transition-all backdrop-blur-sm"
                            disabled={isSearching}
                        />
                        <div className="absolute right-4 top-1/2 -translate-y-1/2">
                            <button
                                type="button"
                                onClick={() => onReplace(replaceText)}
                                disabled={!query || !path || isSearching}
                                className="px-4 py-1.5 bg-pink-600 hover:bg-pink-500 rounded-lg text-white text-sm font-medium transition-all disabled:opacity-50"
                            >
                                Replace All
                            </button>
                        </div>
                    </div>
                )}

                <div className="flex gap-3">
                    <div className="relative flex-1 group">
                        <input
                            type="text"
                            value={path}
                            onChange={(e) => setPath(e.target.value)}
                            placeholder="Search directory..."
                            className="w-full px-6 py-3 bg-white/90 dark:bg-zinc-900/80 border border-zinc-300 dark:border-zinc-800 rounded-xl text-zinc-900 dark:text-zinc-100 placeholder:text-zinc-400 dark:placeholder:text-zinc-500 focus:outline-none focus:ring-2 focus:ring-pink-500/50 focus:border-transparent transition-all backdrop-blur-sm"
                            disabled={isSearching}
                        />
                        <Folder className="absolute right-4 top-1/2 -translate-y-1/2 w-4 h-4 text-zinc-600 dark:text-zinc-500 group-focus-within:text-pink-500 transition-colors" />
                    </div>

                    <button
                        type="button"
                        onClick={() => setIsReplaceOpen(!isReplaceOpen)}
                        className={cn(
                            "px-4 py-3 bg-zinc-200 hover:bg-zinc-300 border border-zinc-300 dark:bg-zinc-800 dark:hover:bg-zinc-700 dark:border-zinc-700 rounded-xl text-foreground transition-all",
                            isReplaceOpen && "bg-zinc-300 border-zinc-400 dark:bg-zinc-700 dark:border-zinc-600"
                        )}
                        title="Toggle Replace"
                    >
                        <Replace className="w-5 h-5" />
                    </button>

                    <button
                        type="button"
                        onClick={handleBrowse}
                        disabled={isSearching}
                        className="px-6 py-3 bg-zinc-200 hover:bg-zinc-300 border border-zinc-300 dark:bg-zinc-800 dark:hover:bg-zinc-700 dark:border-zinc-700 rounded-xl text-foreground transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                    >
                        Browse
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
                            className="px-8 py-3 bg-gradient-to-r from-pink-600 to-purple-600 hover:from-pink-500 hover:to-purple-500 rounded-xl text-white font-medium transition-all disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:from-pink-600 disabled:hover:to-purple-600 flex items-center gap-2 shadow-lg shadow-pink-500/20 hover:shadow-pink-500/40"
                        >
                            {isSearching ? <Loader size={18} className="text-white" /> : <Play className="w-4 h-4 fill-current" />}
                            Search
                        </button>
                    )}
                </div>
            </form>


        </div>
    );
}
