import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "@/lib/store";
import { Folder, StopCircle, CaseSensitive, WholeWord, Regex, Replace, ArrowUp, CornerDownLeft } from "lucide-react";
import { cn } from "@/lib/utils";
import { useState, useEffect, useRef } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";

interface SearchProps {
    onSearch: (query: string, path: string) => void;
    onStop: () => void;
    isSearching: boolean;
    onReplace: (replaceText: string) => void;
}

export function Search({ onSearch, onStop, isSearching, onReplace }: SearchProps) {
    const { query, setQuery, path, setPath, options, setOption, settings, setSearchFocused } = useStore();
    const [isReplaceOpen, setIsReplaceOpen] = useState(false);
    const [replaceText, setReplaceText] = useState("");
    const textareaRef = useRef<HTMLTextAreaElement>(null);

    // Auto-resize textarea
    useEffect(() => {
        if (textareaRef.current) {
            textareaRef.current.style.height = "auto";
            textareaRef.current.style.height = textareaRef.current.scrollHeight + "px";
        }
    }, [query]);

    // Set default search path on mount if path is empty
    useEffect(() => {
        if (!path && settings.defaultSearchPath) {
            setPath(settings.defaultSearchPath);
        }
    }, []);

    const handleSubmit = () => {
        if (query && path && !isSearching) {
            onSearch(query, path);
        }
    };

    const handleKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            handleSubmit();
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
        <div className="w-full max-w-4xl mx-auto relative z-50">
            <div className={cn(
                "relative bg-zinc-100 dark:bg-zinc-900 rounded-3xl border border-zinc-200 dark:border-zinc-800 shadow-sm transition-all duration-300",
                "focus-within:ring-2 focus-within:ring-pink-500/20 focus-within:border-pink-500/50 focus-within:shadow-xl focus-within:scale-[1.01]"
            )}>
                {/* Header: Path & Options */}
                <div className="flex items-center justify-between px-4 pt-3 pb-2">
                    <Badge
                        variant="secondary"
                        className="cursor-pointer hover:bg-zinc-200 dark:hover:bg-zinc-800 font-normal text-zinc-500 dark:text-zinc-400 px-3 py-1.5 h-auto text-xs gap-1.5 rounded-lg border border-transparent hover:border-zinc-300 dark:hover:border-zinc-700 transition-all select-none"
                        onClick={handleBrowse}
                    >
                        <Folder className="w-3.5 h-3.5" />
                        {path ? <span className="truncate max-w-[200px]">{path}</span> : "Select location..."}
                    </Badge>

                    <div className="flex items-center gap-1">
                        <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => setOption('caseSensitive', !options.caseSensitive)}
                            className={cn("h-7 w-7 p-0 rounded-lg", options.caseSensitive ? "bg-pink-100 text-pink-600 dark:bg-pink-900/30 dark:text-pink-400" : "text-zinc-500")}
                            title="Case Sensitive"
                        >
                            <CaseSensitive className="w-4 h-4" />
                        </Button>
                        <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => setOption('wholeWord', !options.wholeWord)}
                            className={cn("h-7 w-7 p-0 rounded-lg", options.wholeWord ? "bg-pink-100 text-pink-600 dark:bg-pink-900/30 dark:text-pink-400" : "text-zinc-500")}
                            title="Whole Word"
                        >
                            <WholeWord className="w-4 h-4" />
                        </Button>
                        <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => setOption('regex', !options.regex)}
                            className={cn("h-7 w-7 p-0 rounded-lg", options.regex ? "bg-pink-100 text-pink-600 dark:bg-pink-900/30 dark:text-pink-400" : "text-zinc-500")}
                            title="Regex"
                        >
                            <Regex className="w-4 h-4" />
                        </Button>
                        <Separator orientation="vertical" className="h-4 mx-1" />
                        <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => setIsReplaceOpen(!isReplaceOpen)}
                            className={cn("h-7 w-7 p-0 rounded-lg", isReplaceOpen ? "bg-zinc-200 dark:bg-zinc-800 text-zinc-900 dark:text-zinc-100" : "text-zinc-500")}
                            title="Toggle Replace"
                        >
                            <Replace className="w-4 h-4" />
                        </Button>
                    </div>
                </div>

                {/* Input Area */}
                <div className="px-2 pb-2 flex gap-2 items-end relative">
                    <div className="flex-1 min-w-0 relative">
                        <Textarea
                            ref={textareaRef}
                            value={query}
                            onChange={(e) => setQuery(e.target.value)}
                            onKeyDown={handleKeyDown}
                            onFocus={() => setSearchFocused(true)}
                            onBlur={() => setSearchFocused(false)}
                            placeholder="What do you want to find?"
                            className="min-h-[44px] max-h-[200px] w-full border-0 focus-visible:ring-0 resize-none bg-transparent shadow-none p-3 text-base placeholder:text-zinc-400 dark:placeholder:text-zinc-600 relative z-10"
                            disabled={isSearching}
                        />

                        {/* Command Hints Overlay */}
                        {query && !isSearching && (
                            <div className="absolute right-0 bottom-3 z-20 hidden md:flex items-center gap-3 pointer-events-none px-2 animate-in fade-in zoom-in-95 duration-200">
                                <span className="flex items-center gap-1 text-[10px] bg-white/50 dark:bg-black/50 backdrop-blur-sm px-1.5 py-0.5 rounded text-zinc-400 border border-black/5 dark:border-white/5">
                                    <CornerDownLeft className="w-3 h-3" />
                                    to search
                                </span>
                                <span className="flex items-center gap-1 text-[10px] bg-white/50 dark:bg-black/50 backdrop-blur-sm px-1.5 py-0.5 rounded text-zinc-400 border border-black/5 dark:border-white/5">
                                    <span className="font-medium">⇧ ↵</span>
                                    new line
                                </span>
                            </div>
                        )}

                        {isReplaceOpen && (
                            <div className="mt-2 relative animate-in slide-in-from-top-1 fade-in duration-200 z-10">
                                <Textarea
                                    value={replaceText}
                                    onChange={(e) => setReplaceText(e.target.value)}
                                    placeholder="Replace with..."
                                    className="min-h-[44px] w-full border-0 bg-zinc-50 dark:bg-zinc-950/50 rounded-xl focus-visible:ring-0 resize-none shadow-none p-3 text-sm"
                                    disabled={isSearching}
                                />
                                <Button
                                    size="sm"
                                    variant="secondary"
                                    onClick={() => onReplace(replaceText)}
                                    disabled={!query || !path || isSearching}
                                    className="absolute right-2 bottom-2 h-7 text-xs bg-zinc-200 hover:bg-zinc-300 dark:bg-zinc-800 dark:hover:bg-zinc-700"
                                >
                                    Replace All
                                </Button>
                            </div>
                        )}
                    </div>

                    <Button
                        size="icon"
                        onClick={isSearching ? onStop : handleSubmit}
                        disabled={(!query || !path) && !isSearching}
                        className={cn(
                            "rounded-2xl h-12 w-12 shrink-0 transition-all mb-1 relative z-30",
                            isSearching
                                ? "bg-red-500 hover:bg-red-600 text-white shadow-lg shadow-red-500/20"
                                : "bg-gradient-to-br from-pink-500 to-purple-600 text-white hover:from-pink-600 hover:to-purple-700 shadow-lg shadow-pink-500/30 hover:shadow-pink-500/40 hover:scale-105 active:scale-95"
                        )}
                    >
                        {isSearching ? <StopCircle className="h-6 w-6" /> : <ArrowUp className="h-6 w-6" />}
                    </Button>
                </div>
            </div>
        </div>
    );
}
