import { Home, Settings as SettingsIcon, Search as SearchIcon, ChevronDown, ChevronUp, Trash2, X, Sparkles, History, Zap } from "lucide-react";
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
                        variant={activeView === view ? "secondary" : "ghost"}
                        onClick={() => onNavigate(view)}
                        className={cn(
                            "w-full justify-start gap-3 px-4 py-6 rounded-xl transition-all duration-300 relative overflow-hidden",
                            activeView === view
                                ? "bg-zinc-100 dark:bg-zinc-800 text-zinc-900 dark:text-zinc-100 font-semibold"
                                : "text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200"
                        )}
                    >
                        {activeView === view && (
                            <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1 h-8 bg-pink-500 rounded-r-full" />
                        )}
                        <Icon className={cn("w-5 h-5", activeView === view ? "text-pink-500" : "opacity-70")} />
                        <span className="text-sm">{label}</span>
                    </Button>
                </TooltipTrigger>
                <TooltipContent side="right">
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
                <div className="px-4 pb-4 flex flex-col gap-2">
                    <Button
                        variant="ghost"
                        onClick={() => setIsHistoryExpanded(!isHistoryExpanded)}
                        className="w-full justify-between items-center px-4 py-2 h-auto hover:bg-zinc-200/50 dark:hover:bg-zinc-800/50"
                    >
                        <div className="flex items-center gap-2 text-zinc-500 dark:text-zinc-400">
                            <History className="w-4 h-4" />
                            <span className="text-xs font-semibold uppercase tracking-wider">Recent</span>
                        </div>
                        <div className="flex items-center gap-2">
                            <Badge variant="secondary" className="text-[10px] h-5 px-1.5">{history.length}</Badge>
                            {isHistoryExpanded ? <ChevronDown className="w-3 h-3 text-zinc-400" /> : <ChevronUp className="w-3 h-3 text-zinc-400" />}
                        </div>
                    </Button>

                    <AnimatePresence>
                        {isHistoryExpanded && (
                            <motion.div
                                initial={{ height: 0, opacity: 0 }}
                                animate={{ height: "auto", opacity: 1 }}
                                exit={{ height: 0, opacity: 0 }}
                                transition={{ duration: 0.2 }}
                                className="overflow-hidden flex flex-col gap-2"
                            >
                                <div className="px-1">
                                    <div className="relative">
                                        <SearchIcon className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-zinc-400" />
                                        <Input
                                            value={historySearch}
                                            onChange={(e) => setHistorySearch(e.target.value)}
                                            placeholder="Find..."
                                            className="h-8 pl-8 pr-8 text-xs bg-white dark:bg-zinc-900 border-zinc-200 dark:border-zinc-800"
                                        />
                                        {historySearch && (
                                            <button
                                                onClick={() => setHistorySearch("")}
                                                className="absolute right-2.5 top-1/2 -translate-y-1/2 text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-300"
                                            >
                                                <X className="w-3 h-3" />
                                            </button>
                                        )}
                                    </div>
                                </div>

                                <Separator className="my-1" />

                                <div ref={listContainerRef} className="pt-1">
                                    {filteredHistory.length === 0 ? (
                                        <div className="text-center py-8 text-zinc-400 text-xs">
                                            No recent searches
                                        </div>
                                    ) : (
                                        <VirtualList
                                            height={listHeight}
                                            width={220} // Approximate width of sidebar content area
                                            itemCount={filteredHistory.length}
                                            itemSize={() => 40}
                                        >
                                            {({ index, style }) => {
                                                const item = filteredHistory[index];
                                                return (
                                                    <div style={style} className="px-1 py-1">
                                                        <div className="group flex items-center justify-between w-full h-full rounded-lg hover:bg-zinc-100 dark:hover:bg-zinc-800 cursor-pointer transition-all border border-transparent hover:border-zinc-200 dark:hover:border-zinc-700/50 pr-2">
                                                            <div
                                                                onClick={() => onHistorySelect(item.query, item.path)}
                                                                className="flex-1 min-w-0 flex flex-col justify-center px-3"
                                                            >
                                                                <span className="text-sm font-medium text-zinc-700 dark:text-zinc-300 truncate">{item.query}</span>
                                                                <span className="text-[10px] text-zinc-400 truncate">{item.path}</span>
                                                            </div>
                                                            <Button
                                                                variant="ghost"
                                                                size="icon"
                                                                onClick={(e) => {
                                                                    e.stopPropagation();
                                                                    removeFromHistory(item.query, item.path);
                                                                }}
                                                                className="h-6 w-6 opacity-0 group-hover:opacity-100 text-zinc-400 hover:text-red-500 hover:bg-red-50 dark:hover:bg-red-900/20"
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

                                {history.length > 0 && (
                                    <Button
                                        variant="ghost"
                                        size="sm"
                                        onClick={clearHistory}
                                        className="w-full text-xs text-red-500 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-900/10 h-8"
                                    >
                                        <Trash2 className="w-3 h-3 mr-2" />
                                        Clear History
                                    </Button>
                                )}
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
