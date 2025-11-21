import { useStore } from "@/lib/store";
import { Clock, Trash2, Search, X } from "lucide-react";
import { motion, AnimatePresence } from "framer-motion";

interface HistoryProps {
    isOpen: boolean;
    onClose: () => void;
    onSelect: (query: string, path: string) => void;
}

export function History({ isOpen, onClose, onSelect }: HistoryProps) {
    const { history, clearHistory } = useStore();

    const formatTime = (timestamp: number) => {
        const date = new Date(timestamp);
        return date.toLocaleString();
    };

    return (
        <AnimatePresence>
            {isOpen && (
                <>
                    <motion.div
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        exit={{ opacity: 0 }}
                        onClick={onClose}
                        className="fixed inset-0 bg-black/60 backdrop-blur-sm z-40"
                    />
                    <motion.div
                        initial={{ x: "100%" }}
                        animate={{ x: 0 }}
                        exit={{ x: "100%" }}
                        transition={{ type: "spring", damping: 20 }}
                        className="fixed right-0 top-0 bottom-0 w-96 bg-zinc-900 border-l border-zinc-800 z-50 shadow-2xl flex flex-col"
                    >
                        <div className="p-4 border-b border-zinc-800 flex items-center justify-between">
                            <div className="flex items-center gap-2 text-zinc-100 font-semibold">
                                <Clock className="w-5 h-5 text-pink-500" />
                                <h2>Search History</h2>
                            </div>
                            <button
                                onClick={onClose}
                                className="p-2 hover:bg-zinc-800 rounded-lg text-zinc-400 hover:text-white transition-colors"
                            >
                                <X className="w-5 h-5" />
                            </button>
                        </div>

                        <div className="flex-1 overflow-y-auto p-4 space-y-3">
                            {history.length === 0 ? (
                                <div className="text-center text-zinc-500 py-10">
                                    <Clock className="w-12 h-12 mx-auto mb-3 opacity-20" />
                                    <p>No search history yet</p>
                                </div>
                            ) : (
                                history.map((item, index) => (
                                    <div
                                        key={index}
                                        onClick={() => {
                                            onSelect(item.query, item.path);
                                            onClose();
                                        }}
                                        className="p-3 rounded-xl bg-zinc-800/50 border border-zinc-800 hover:border-pink-500/50 hover:bg-zinc-800 cursor-pointer group transition-all"
                                    >
                                        <div className="flex items-start justify-between mb-1">
                                            <span className="font-medium text-zinc-200 group-hover:text-pink-400 transition-colors">
                                                {item.query}
                                            </span>
                                            <span className="text-[10px] text-zinc-500">
                                                {formatTime(item.timestamp)}
                                            </span>
                                        </div>
                                        <div className="flex items-center gap-1 text-xs text-zinc-500 truncate">
                                            <Search className="w-3 h-3" />
                                            <span className="truncate">{item.path}</span>
                                        </div>
                                    </div>
                                ))
                            )}
                        </div>

                        {history.length > 0 && (
                            <div className="p-4 border-t border-zinc-800">
                                <button
                                    onClick={clearHistory}
                                    className="w-full flex items-center justify-center gap-2 p-3 rounded-xl bg-red-500/10 text-red-400 hover:bg-red-500/20 transition-colors text-sm font-medium"
                                >
                                    <Trash2 className="w-4 h-4" />
                                    Clear History
                                </button>
                            </div>
                        )}
                    </motion.div>
                </>
            )}
        </AnimatePresence>
    );
}
