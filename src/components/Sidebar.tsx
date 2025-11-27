import { Home, Settings as SettingsIcon, Search as SearchIcon, ChevronDown, ChevronUp, Clock, Trash2, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { useStore } from "@/lib/store";
import { useState, useMemo, useRef, useEffect } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { VirtualList } from "./VirtualList";

interface SidebarProps {
    activeView: 'search' | 'settings';
    onNavigate: (view: 'search' | 'settings') => void;
    onHistorySelect: (query: string, path: string) => void;
}

export function Sidebar({ activeView, onNavigate, onHistorySelect }: SidebarProps) {
    const { history, clearHistory, removeFromHistory } = useStore();
    const [isHistoryExpanded, setIsHistoryExpanded] = useState(false);
    const [historySearch, setHistorySearch] = useState("");
    const listContainerRef = useRef<HTMLDivElement>(null);
    const [listHeight, setListHeight] = useState(200);

    const filteredHistory = useMemo(() => {
        if (!historySearch) return history;
        const lowerSearch = historySearch.toLowerCase();
        return history.filter(item =>
            item.query.toLowerCase().includes(lowerSearch) ||
            item.path.toLowerCase().includes(lowerSearch)
        );
    }, [history, historySearch]);

    useEffect(() => {
        if (isHistoryExpanded && listContainerRef.current) {
            setListHeight(300);
        }
    }, [isHistoryExpanded]);

    const NavItem = ({ view, icon: Icon, label }: { view: 'search' | 'settings', icon: any, label: string }) => (
        <button
            onClick={() => onNavigate(view)}
            className={cn(
                "w-full flex items-center gap-3 px-4 py-3 rounded-xl transition-all duration-300 group relative overflow-hidden",
                activeView === view
                    ? "bg-pink-500/10 text-pink-600 dark:text-pink-400"
                    : "text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800/50 hover:text-zinc-900 dark:hover:text-zinc-200"
            )}
        >
            {activeView === view && (
                <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1 h-8 bg-pink-500 rounded-r-full" />
            )}
            <Icon className={cn("w-5 h-5 transition-transform group-hover:scale-110", activeView === view && "fill-current")} />
            <span className="font-medium">{label}</span>
        </button>
    );

    return (
        <div className="w-64 h-full flex flex-col bg-white/50 dark:bg-zinc-950/50 backdrop-blur-xl border-r border-zinc-200 dark:border-zinc-800/50 transition-colors duration-300">
            <div className="p-6 shrink-0">
                <div className="flex items-center gap-3 mb-8">
                    <div className="w-10 h-10 rounded-xl bg-gradient-to-br from-pink-500 to-purple-600 flex items-center justify-center shadow-lg shadow-pink-500/20">
                        <SearchIcon className="w-6 h-6 text-white" />
                    </div>
                    <div>
                        <h1 className="font-bold text-lg text-zinc-900 dark:text-white leading-tight">
                            ShadUI
                        </h1>
                        <p className="text-xs text-zinc-500 dark:text-zinc-400 font-medium">
                            Ripgrep Pro
                        </p>
                    </div>
                </div>

                <nav className="space-y-2">
                    <NavItem view="search" icon={Home} label="Search" />
                    <NavItem view="settings" icon={SettingsIcon} label="Settings" />
                </nav>
            </div>

            <div className="mt-auto flex flex-col min-h-0">
                <div className="px-4 pb-4 flex flex-col gap-2">
                    <button
                        onClick={() => setIsHistoryExpanded(!isHistoryExpanded)}
                        className="w-full flex items-center justify-between px-4 py-2 text-xs font-semibold text-zinc-500 dark:text-zinc-400 uppercase tracking-wider hover:text-zinc-900 dark:hover:text-zinc-200 transition-colors"
                    >
                        <div className="flex items-center gap-2">
                            <Clock className="w-4 h-4" />
                            <span>Recent ({history.length})</span>
                        </div>
                        {isHistoryExpanded ? <ChevronDown className="w-4 h-4" /> : <ChevronUp className="w-4 h-4" />}
                    </button>

                    <AnimatePresence>
                        {isHistoryExpanded && (
                            <motion.div
                                initial={{ height: 0, opacity: 0 }}
                                animate={{ height: "auto", opacity: 1 }}
                                exit={{ height: 0, opacity: 0 }}
                                transition={{ duration: 0.2 }}
                                className="overflow-hidden flex flex-col gap-2"
                            >
                                <div className="px-2">
                                    <div className="relative">
                                        <SearchIcon className="absolute left-2 top-1/2 -translate-y-1/2 w-3 h-3 text-zinc-400" />
                                        <input
                                            value={historySearch}
                                            onChange={(e) => setHistorySearch(e.target.value)}
                                            placeholder="Filter history..."
                                            className="w-full bg-zinc-100 dark:bg-zinc-800/50 border border-zinc-200 dark:border-zinc-700 rounded-lg pl-7 pr-2 py-1 text-xs focus:outline-none focus:ring-1 focus:ring-pink-500"
                                        />
                                        {historySearch && (
                                            <button
                                                onClick={() => setHistorySearch("")}
                                                className="absolute right-2 top-1/2 -translate-y-1/2 text-zinc-400 hover:text-zinc-600"
                                            >
                                                <X className="w-3 h-3" />
                                            </button>
                                        )}
                                    </div>
                                </div>

                                <div ref={listContainerRef} className="border-t border-zinc-100 dark:border-zinc-800/50 pt-2">
                                    {filteredHistory.length === 0 ? (
                                        <div className="text-center py-4 text-zinc-400 text-xs">
                                            No matching history
                                        </div>
                                    ) : (
                                        <VirtualList
                                            height={listHeight}
                                            width={220} // Approximate width of sidebar content area
                                            itemCount={filteredHistory.length}
                                            itemSize={36}
                                        >
                                            {({ index, style }) => {
                                                const item = filteredHistory[index];
                                                return (
                                                    <div style={style} className="px-1">
                                                        <div className="group flex items-center justify-between w-full rounded-lg hover:bg-zinc-100 dark:hover:bg-zinc-800/50 transition-colors pr-2">
                                                            <button
                                                                onClick={() => onHistorySelect(item.query, item.path)}
                                                                className="flex-1 text-left px-3 py-2 text-sm text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 truncate flex items-center gap-2"
                                                                title={`${item.query} in ${item.path}`}
                                                            >
                                                                <span className="truncate">{item.query}</span>
                                                            </button>
                                                            <button
                                                                onClick={(e) => {
                                                                    e.stopPropagation();
                                                                    removeFromHistory(item.query, item.path);
                                                                }}
                                                                className="opacity-0 group-hover:opacity-100 p-1 text-zinc-400 hover:text-red-500 transition-all"
                                                                title="Remove from history"
                                                            >
                                                                <Trash2 className="w-3 h-3" />
                                                            </button>
                                                        </div>
                                                    </div>
                                                );
                                            }}
                                        </VirtualList>
                                    )}
                                </div>

                                {history.length > 0 && (
                                    <button
                                        onClick={clearHistory}
                                        className="w-full flex items-center justify-center gap-2 px-4 py-2 text-xs text-red-500 hover:bg-red-500/10 rounded-lg transition-colors"
                                    >
                                        <Trash2 className="w-3 h-3" />
                                        Clear All
                                    </button>
                                )}
                            </motion.div>
                        )}
                    </AnimatePresence>
                </div>

                {!isHistoryExpanded && (
                    <div className="p-6 pt-0">
                        <div className="p-4 rounded-2xl bg-gradient-to-br from-pink-500/10 to-purple-500/10 border border-pink-500/20 dark:border-pink-500/10">
                            <h3 className="text-sm font-semibold text-pink-700 dark:text-pink-300 mb-1">
                                Pro Tip
                            </h3>
                            <p className="text-xs text-zinc-600 dark:text-zinc-400 leading-relaxed">
                                Use Regex mode for powerful pattern matching across your codebase.
                            </p>
                        </div>
                    </div>
                )}
            </div>
        </div>
    );
}
