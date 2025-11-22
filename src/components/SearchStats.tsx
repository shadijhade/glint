import { motion, AnimatePresence } from "framer-motion";
import { Activity, Clock, FileText, Zap, BarChart3 } from "lucide-react";
import { RgMatch } from "@/lib/ripgrep";
import { useMemo } from "react";

interface SearchStatsProps {
    results: RgMatch[];
    isSearching: boolean;
    duration: number; // in milliseconds
}

export function SearchStats({ results, isSearching, duration }: SearchStatsProps) {
    const stats = useMemo(() => {
        const matches = results.filter((r) => r.type === "match");
        const totalMatches = matches.length;

        // Count unique files
        const uniqueFiles = new Set(matches.map(m => m.data.path?.text)).size;

        // Calculate file type distribution
        const fileTypes = matches.reduce((acc, curr) => {
            const path = curr.data.path?.text || "";
            const ext = path.split('.').pop()?.toLowerCase() || 'unknown';
            acc[ext] = (acc[ext] || 0) + 1;
            return acc;
        }, {} as Record<string, number>);

        const topFileType = Object.entries(fileTypes).sort((a, b) => b[1] - a[1])[0];

        return {
            totalMatches,
            uniqueFiles,
            topFileType: topFileType ? { ext: topFileType[0], count: topFileType[1] } : null
        };
    }, [results]);

    if (!results.length && !isSearching) return null;

    return (
        <AnimatePresence mode="wait">
            <motion.div
                initial={{ opacity: 0, y: 20 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: 20 }}
                className="w-full max-w-4xl mx-auto mt-4"
            >
                <div className="grid grid-cols-1 md:grid-cols-4 gap-3">
                    {/* Status Card */}
                    <div className="bg-white/50 dark:bg-zinc-900/50 border border-zinc-300 dark:border-zinc-800/50 rounded-xl p-3 backdrop-blur-md flex items-center gap-3 relative overflow-hidden group transition-colors duration-300">
                        <div className={`p-2 rounded-lg ${isSearching ? 'bg-blue-500/20 text-blue-400' : 'bg-green-500/20 text-green-400'}`}>
                            {isSearching ? (
                                <Activity className="w-4 h-4 animate-pulse" />
                            ) : (
                                <Zap className="w-4 h-4" />
                            )}
                        </div>
                        <div>
                            <p className="text-xs text-zinc-500 dark:text-zinc-500 text-zinc-600 font-medium uppercase tracking-wider transition-colors duration-300">Status</p>
                            <p className="text-sm font-semibold text-zinc-900 dark:text-zinc-200 transition-colors duration-300">
                                {isSearching ? "Scanning..." : "Complete"}
                            </p>
                        </div>
                        {isSearching && (
                            <motion.div
                                className="absolute bottom-0 left-0 h-0.5 bg-blue-500"
                                initial={{ width: "0%" }}
                                animate={{ width: "100%" }}
                                transition={{ repeat: Infinity, duration: 1.5, ease: "linear" }}
                            />
                        )}
                    </div>

                    {/* Time Card */}
                    <div className="bg-white/50 dark:bg-zinc-900/50 border border-zinc-300 dark:border-zinc-800/50 rounded-xl p-3 backdrop-blur-md flex items-center gap-3 transition-colors duration-300">
                        <div className="p-2 rounded-lg bg-orange-500/20 text-orange-400">
                            <Clock className="w-4 h-4" />
                        </div>
                        <div>
                            <p className="text-xs text-zinc-600 dark:text-zinc-500 font-medium uppercase tracking-wider transition-colors duration-300">Time Taken</p>
                            <p className="text-sm font-semibold text-zinc-900 dark:text-zinc-200 font-mono transition-colors duration-300">
                                {duration > 0 ? `${(duration / 1000).toFixed(2)}s` : "--"}
                            </p>
                        </div>
                    </div>

                    {/* Files Card */}
                    <div className="bg-white/50 dark:bg-zinc-900/50 border border-zinc-300 dark:border-zinc-800/50 rounded-xl p-3 backdrop-blur-md flex items-center gap-3 transition-colors duration-300">
                        <div className="p-2 rounded-lg bg-purple-500/20 text-purple-400">
                            <FileText className="w-4 h-4" />
                        </div>
                        <div>
                            <p className="text-xs text-zinc-600 dark:text-zinc-500 font-medium uppercase tracking-wider transition-colors duration-300">Files Found</p>
                            <div className="flex items-baseline gap-1">
                                <p className="text-sm font-semibold text-zinc-900 dark:text-zinc-200 transition-colors duration-300">
                                    {stats.uniqueFiles}
                                </p>
                                <span className="text-[10px] text-zinc-600 dark:text-zinc-500 transition-colors duration-300">files</span>
                            </div>
                        </div>
                    </div>

                    {/* Insights Card */}
                    <div className="bg-white/50 dark:bg-zinc-900/50 border border-zinc-300 dark:border-zinc-800/50 rounded-xl p-3 backdrop-blur-md flex items-center gap-3 transition-colors duration-300">
                        <div className="p-2 rounded-lg bg-pink-500/20 text-pink-400">
                            <BarChart3 className="w-4 h-4" />
                        </div>
                        <div>
                            <p className="text-xs text-zinc-600 dark:text-zinc-500 font-medium uppercase tracking-wider transition-colors duration-300">Top Type</p>
                            {stats.topFileType ? (
                                <div className="flex items-baseline gap-1">
                                    <p className="text-sm font-semibold text-zinc-900 dark:text-zinc-200 uppercase transition-colors duration-300">
                                        .{stats.topFileType.ext}
                                    </p>
                                    <span className="text-[10px] text-zinc-600 dark:text-zinc-500 transition-colors duration-300">
                                        ({Math.round((stats.topFileType.count / stats.totalMatches) * 100)}%)
                                    </span>
                                </div>
                            ) : (
                                <p className="text-sm font-semibold text-zinc-900 dark:text-zinc-200 transition-colors duration-300">--</p>
                            )}
                        </div>
                    </div>
                </div>
            </motion.div>
        </AnimatePresence>
    );
}
