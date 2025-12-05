import { motion, AnimatePresence } from "framer-motion";
import { Activity, Clock, FileText, Zap, AlertTriangle } from "lucide-react";
import { RgMatch } from "@/lib/ripgrep";
import { useMemo } from "react";
import { useStore } from "@/lib/store";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";

interface SearchStatsProps {
    results: RgMatch[];
    isSearching: boolean;
    duration: number; // in milliseconds
    onRerunSearch?: () => void;
}

export function SearchStats({ results, isSearching, duration, onRerunSearch }: SearchStatsProps) {
    const derivedStats = useMemo(() => {
        const matches = results.filter((r) => r.type === "match");
        const totalMatches = matches.length;
        const uniqueFiles = new Set(matches.map(m => m.data.path?.text)).size;
        return { totalMatches, uniqueFiles };
    }, [results]);

    const { settings, setSettings } = useStore();
    const { maxResults } = settings;
    const limitReached = maxResults !== null && derivedStats.totalMatches >= maxResults;

    const handleShowAll = () => {
        setSettings({ maxResults: null });
        onRerunSearch?.();
    };

    if (!results.length && !isSearching) return null;

    return (
        <AnimatePresence mode="wait">
            <motion.div
                initial={{ opacity: 0, y: -10 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -10 }}
                className="flex items-center gap-4 text-sm text-muted-foreground bg-zinc-100/50 dark:bg-zinc-900/50 px-4 py-2 rounded-full border border-zinc-200 dark:border-zinc-800 backdrop-blur-sm shadow-sm inline-flex"
            >
                {/* Status */}
                <div className="flex items-center gap-2">
                    {isSearching ? (
                        <Activity className="w-4 h-4 animate-pulse text-blue-500" />
                    ) : limitReached ? (
                        <AlertTriangle className="w-4 h-4 text-yellow-500" />
                    ) : (
                        <Zap className="w-4 h-4 text-green-500" />
                    )}
                    <span className="font-medium text-zinc-900 dark:text-zinc-100">
                        {isSearching ? "Scanning" : limitReached ? "Limit Reached" : "Done"}
                    </span>
                </div>

                <Separator orientation="vertical" className="h-4" />

                {/* Counts */}
                <div className="flex items-center gap-2">
                    <span className="font-mono font-medium text-pink-600 dark:text-pink-400">
                        {derivedStats.totalMatches}
                    </span>
                    <span>matches</span>
                </div>

                <Separator orientation="vertical" className="h-4" />

                <div className="flex items-center gap-2">
                    <FileText className="w-4 h-4" />
                    <span className="font-mono font-medium text-zinc-900 dark:text-zinc-100">
                        {derivedStats.uniqueFiles}
                    </span>
                    <span>files</span>
                </div>

                <Separator orientation="vertical" className="h-4" />

                {/* Time */}
                <div className="flex items-center gap-2">
                    <Clock className="w-4 h-4" />
                    <span className="font-mono text-xs">
                        {duration > 0 ? (
                            duration < 1 ? `<1ms` :
                                duration < 1000 ? `${duration.toFixed(0)}ms` :
                                    `${(duration / 1000).toFixed(2)}s`
                        ) : "--"}
                    </span>
                </div>

                {limitReached && !isSearching && (
                    <>
                        <Separator orientation="vertical" className="h-4" />
                        <Badge
                            variant="secondary"
                            className="text-xs cursor-pointer hover:bg-yellow-100 dark:hover:bg-yellow-900/30 text-yellow-600 dark:text-yellow-400 border-yellow-200 dark:border-yellow-800"
                            onClick={handleShowAll}
                        >
                            Show all
                        </Badge>
                    </>
                )}
            </motion.div>
        </AnimatePresence>
    );
}
