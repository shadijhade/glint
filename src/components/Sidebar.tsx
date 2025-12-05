import { Home, Settings as SettingsIcon, Search as SearchIcon, ChevronUp, Trash2, X, Sparkles, History, Zap } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { cn } from "@/lib/utils";
import { useStore } from "@/lib/store";
import { useState, useMemo, useRef, useEffect } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { VirtualList } from "./VirtualList";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import { Input } from "@/components/ui/input";

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
    const [updateInfo, setUpdateInfo] = useState<{ update_available: boolean; latest_version: string; url: string } | null>(null);
    const [isUpdateDismissed, setIsUpdateDismissed] = useState(false);

    useEffect(() => {
        invoke<{ update_available: boolean; latest_version: string; url: string }>("check_update")
            .then((info) => {
                if (info.update_available) {
                    setUpdateInfo(info);
                }
            })
            .catch((err) => console.error("Failed to check for updates:", err));
    }, []);

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
        <TooltipProvider>
            <Tooltip>
                <TooltipTrigger asChild>
                    <Button
                        variant="ghost"
                        onClick={() => onNavigate(view)}
                        className={cn(
                            "w-full justify-start gap-3 px-4 py-6 rounded-xl transition-all duration-300 relative overflow-hidden group",
                            activeView === view
                                ? "bg-gradient-to-r from-pink-500/15 to-purple-500/10 dark:from-pink-500/20 dark:to-purple-500/15 text-pink-600 dark:text-pink-400 font-semibold shadow-lg shadow-pink-500/10 border border-pink-500/20"
                                : "text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 hover:bg-zinc-100/80 dark:hover:bg-zinc-800/50"
                        )}
                    >
                        {activeView === view && (
                            <>
                                <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1 h-8 bg-gradient-to-b from-pink-500 to-purple-500 rounded-r-full shadow-lg shadow-pink-500/50" />
                                <div className="absolute inset-0 bg-gradient-to-r from-pink-500/5 to-transparent opacity-0 group-hover:opacity-100 transition-opacity" />
                            </>
                        )}
                        <div className={cn(
                            "p-1.5 rounded-lg transition-all duration-300",
                            activeView === view
                                ? "bg-pink-500/20 dark:bg-pink-500/30"
                                : "bg-transparent group-hover:bg-zinc-200/50 dark:group-hover:bg-zinc-700/50"
                        )}>
                            <Icon className={cn(
                                "w-4 h-4 transition-all duration-300",
                                activeView === view
                                    ? "text-pink-500 dark:text-pink-400"
                                    : "text-zinc-500 dark:text-zinc-400 group-hover:text-zinc-700 dark:group-hover:text-zinc-300"
                            )} />
                        </div>
                        <span className="text-sm">{label}</span>
                        {activeView === view && (
                            <div className="ml-auto w-2 h-2 rounded-full bg-pink-500 animate-pulse shadow-lg shadow-pink-500/50" />
                        )}
                    </Button>
                </TooltipTrigger>
                <TooltipContent side="right" className="bg-zinc-900 text-white border-zinc-800">
                    <p>{label}</p>
                </TooltipContent>
            </Tooltip>
        </TooltipProvider>
    );

    return (
        <div className="w-64 h-full flex flex-col bg-zinc-50/50 dark:bg-zinc-950/50 backdrop-blur-xl border-r border-zinc-200 dark:border-zinc-800 transition-colors duration-300">
            {/* Header */}
            <div className="p-6 shrink-0">
                <div className="flex items-center gap-3 mb-8">
                    <div className="w-10 h-10 rounded-xl bg-gradient-to-br from-pink-500 to-purple-600 flex items-center justify-center shadow-lg shadow-pink-500/20">
                        <Zap className="w-6 h-6 text-white fill-current" />
                    </div>
                    <div>
                        <h1 className="font-bold text-lg text-zinc-900 dark:text-white leading-tight tracking-tight">
                            ShadUI
                        </h1>
                        <Badge variant="outline" className="text-[10px] h-4 px-1 py-0 border-pink-500/30 text-pink-500">
                            PRO
                        </Badge>
                    </div>
                </div>

                <nav className="space-y-2">
                    <NavItem view="search" icon={Home} label="Search" />
                    <NavItem view="settings" icon={SettingsIcon} label="Settings" />
                </nav>
            </div>

            {/* Middle Spacer */}
            <div className="flex-1" />

            {/* Bottom Section */}
            <div className="flex flex-col min-h-0 container-snap">
                {/* History Section */}
                <div className="px-3 pb-4 flex flex-col gap-2">
                    <Button
                        variant="ghost"
                        onClick={() => setIsHistoryExpanded(!isHistoryExpanded)}
                        className={cn(
                            "w-full justify-between items-center px-3 py-2.5 h-auto rounded-xl transition-all duration-200",
                            isHistoryExpanded
                                ? "bg-gradient-to-r from-pink-500/10 to-purple-500/5 border border-pink-500/20"
                                : "hover:bg-zinc-100 dark:hover:bg-zinc-800/50"
                        )}
                    >
                        <div className="flex items-center gap-2.5">
                            <div className={cn(
                                "p-1.5 rounded-lg transition-all",
                                isHistoryExpanded
                                    ? "bg-pink-500/20 text-pink-500"
                                    : "bg-zinc-100 dark:bg-zinc-800 text-zinc-500"
                            )}>
                                <History className="w-3.5 h-3.5" />
                            </div>
                            <span className={cn(
                                "text-xs font-semibold uppercase tracking-wider transition-colors",
                                isHistoryExpanded ? "text-pink-600 dark:text-pink-400" : "text-zinc-500 dark:text-zinc-400"
                            )}>Recent</span>
                        </div>
                        <div className="flex items-center gap-2">
                            {history.length > 0 && (
                                <Badge className="text-[10px] h-5 px-1.5 bg-pink-500/10 text-pink-600 dark:text-pink-400 border-pink-500/20 hover:bg-pink-500/20">
                                    {history.length}
                                </Badge>
                            )}
                            <div className={cn(
                                "p-1 rounded-md transition-all",
                                isHistoryExpanded ? "bg-pink-500/10 rotate-180" : ""
                            )}>
                                <ChevronUp className={cn(
                                    "w-3.5 h-3.5 transition-colors",
                                    isHistoryExpanded ? "text-pink-500" : "text-zinc-400"
                                )} />
                            </div>
                        </div>
                    </Button>

                    <AnimatePresence>
                        {isHistoryExpanded && (
                            <motion.div
                                initial={{ height: 0, opacity: 0 }}
                                animate={{ height: "auto", opacity: 1 }}
                                exit={{ height: 0, opacity: 0 }}
                                transition={{ duration: 0.25, ease: "easeOut" }}
                                className="overflow-hidden"
                            >
                                <div className="bg-white/50 dark:bg-zinc-900/50 rounded-xl border border-zinc-200 dark:border-zinc-800 p-2 backdrop-blur-sm">
                                    {/* Search Input */}
                                    <div className="relative mb-2">
                                        <SearchIcon className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-zinc-400" />
                                        <Input
                                            value={historySearch}
                                            onChange={(e) => setHistorySearch(e.target.value)}
                                            placeholder="Filter searches..."
                                            className="h-8 pl-8 pr-8 text-xs bg-zinc-50 dark:bg-zinc-800/50 border-zinc-200 dark:border-zinc-700 rounded-lg focus:ring-pink-500/20 focus:border-pink-500/50"
                                        />
                                        {historySearch && (
                                            <button
                                                onClick={() => setHistorySearch("")}
                                                className="absolute right-2 top-1/2 -translate-y-1/2 p-0.5 rounded text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors"
                                            >
                                                <X className="w-3 h-3" />
                                            </button>
                                        )}
                                    </div>

                                    {/* History List */}
                                    <div ref={listContainerRef}>
                                        {filteredHistory.length === 0 ? (
                                            <div className="flex flex-col items-center justify-center py-8 text-center">
                                                <div className="p-3 rounded-full bg-zinc-100 dark:bg-zinc-800 mb-3">
                                                    <SearchIcon className="w-5 h-5 text-zinc-400" />
                                                </div>
                                                <p className="text-xs text-zinc-500 dark:text-zinc-400 font-medium">
                                                    {historySearch ? "No matches found" : "No recent searches"}
                                                </p>
                                                <p className="text-[10px] text-zinc-400 dark:text-zinc-500 mt-1">
                                                    {historySearch ? "Try a different filter" : "Your searches will appear here"}
                                                </p>
                                            </div>
                                        ) : (
                                            <VirtualList
                                                height={listHeight}
                                                width={220}
                                                itemCount={filteredHistory.length}
                                                itemSize={() => 56}
                                            >
                                                {({ index, style }) => {
                                                    const item = filteredHistory[index];
                                                    const folderName = item.path.split(/[\\/]/).pop() || item.path;

                                                    return (
                                                        <div style={style} className="px-0.5 py-1">
                                                            <div
                                                                className="group relative flex items-center gap-2 w-full h-full rounded-lg bg-zinc-50 dark:bg-zinc-800/30 hover:bg-gradient-to-r hover:from-pink-50 hover:to-purple-50 dark:hover:from-pink-950/30 dark:hover:to-purple-950/20 cursor-pointer transition-all duration-200 border border-transparent hover:border-pink-200 dark:hover:border-pink-800/50 px-2.5 overflow-hidden"
                                                            >
                                                                {/* Left accent line on hover */}
                                                                <div className="absolute left-0 top-1/2 -translate-y-1/2 w-0.5 h-6 bg-gradient-to-b from-pink-500 to-purple-500 rounded-r opacity-0 group-hover:opacity-100 transition-opacity" />

                                                                {/* Play icon */}
                                                                <div
                                                                    onClick={() => onHistorySelect(item.query, item.path)}
                                                                    className="shrink-0 p-1.5 rounded-md bg-zinc-100 dark:bg-zinc-700/50 group-hover:bg-pink-500 transition-all duration-200"
                                                                >
                                                                    <SearchIcon className="w-3 h-3 text-zinc-500 group-hover:text-white transition-colors" />
                                                                </div>

                                                                {/* Content */}
                                                                <div
                                                                    onClick={() => onHistorySelect(item.query, item.path)}
                                                                    className="flex-1 min-w-0 flex flex-col justify-center py-1"
                                                                >
                                                                    <span className="text-xs font-semibold text-zinc-700 dark:text-zinc-200 truncate group-hover:text-pink-600 dark:group-hover:text-pink-400 transition-colors">
                                                                        {item.query}
                                                                    </span>
                                                                    <div className="flex items-center gap-1.5 mt-0.5">
                                                                        <span className="text-[10px] text-zinc-400 dark:text-zinc-500 truncate max-w-[100px]" title={item.path}>
                                                                            📁 {folderName}
                                                                        </span>
                                                                    </div>
                                                                </div>

                                                                {/* Delete button */}
                                                                <Button
                                                                    variant="ghost"
                                                                    size="icon"
                                                                    onClick={(e) => {
                                                                        e.stopPropagation();
                                                                        removeFromHistory(item.query, item.path);
                                                                    }}
                                                                    className="shrink-0 h-6 w-6 opacity-0 group-hover:opacity-100 text-zinc-400 hover:text-red-500 hover:bg-red-50 dark:hover:bg-red-900/30 rounded-md transition-all"
                                                                >
                                                                    <Trash2 className="w-3 h-3" />
                                                                </Button>
                                                            </div>
                                                        </div>
                                                    );
                                                }}
                                            </VirtualList>
                                        )}
                                    </div>

                                    {/* Footer Actions */}
                                    {history.length > 0 && (
                                        <>
                                            <Separator className="my-2" />
                                            <Button
                                                variant="ghost"
                                                size="sm"
                                                onClick={clearHistory}
                                                className="w-full text-xs text-zinc-500 hover:text-red-500 hover:bg-red-50 dark:hover:bg-red-900/20 h-7 rounded-lg transition-colors"
                                            >
                                                <Trash2 className="w-3 h-3 mr-1.5" />
                                                Clear all history
                                            </Button>
                                        </>
                                    )}
                                </div>
                            </motion.div>
                        )}
                    </AnimatePresence>
                </div>

                {/* Update Card / Promo */}
                <AnimatePresence>
                    {updateInfo && !isHistoryExpanded && !isUpdateDismissed && (
                        <motion.div
                            initial={{ opacity: 0, y: 20, scale: 0.95 }}
                            animate={{ opacity: 1, y: 0, scale: 1 }}
                            exit={{ opacity: 0, y: 20, scale: 0.95 }}
                            className="p-4 pt-0 relative"
                        >
                            <div className="p-4 rounded-xl bg-gradient-to-br from-indigo-500 to-purple-600 text-white shadow-lg relative overflow-hidden group">
                                <Button
                                    size="icon"
                                    variant="ghost"
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        setIsUpdateDismissed(true);
                                    }}
                                    className="absolute top-1 right-1 h-6 w-6 text-white/50 hover:text-white hover:bg-white/10"
                                >
                                    <X className="w-3 h-3" />
                                </Button>

                                <div className="flex items-center gap-2 mb-2">
                                    <Sparkles className="w-4 h-4 animate-pulse" />
                                    <span className="text-xs font-bold uppercase tracking-wider">Update</span>
                                </div>
                                <h3 className="text-sm font-semibold mb-1">New Version {updateInfo.latest_version}</h3>
                                <Button
                                    size="sm"
                                    className="w-full mt-2 bg-white text-purple-600 hover:bg-white/90 border-0 h-7 text-xs"
                                    onClick={() => openUrl(updateInfo.url)}
                                >
                                    Download
                                </Button>
                            </div>
                        </motion.div>
                    )}
                </AnimatePresence>
            </div>
        </div>
    );
}
