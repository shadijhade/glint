import { RgMatch } from "@/lib/ripgrep";
import { AutoSizer } from "./AutoSizer";
import { VirtualList, VirtualListHandle } from "./VirtualList";
import {
    ContextMenu,
    ContextMenuContent,
    ContextMenuItem,
    ContextMenuTrigger,
    ContextMenuSeparator,
} from "@/components/ui/context-menu";
import { FileText, FileCode2, FileImage, FileArchive, FileAudio, FileVideo, Download, Copy, ExternalLink, FolderOpen, Search as SearchIcon, AlertTriangle } from "lucide-react";
import { Loader } from "@/components/ui/Loader";
import { useRef, useEffect, useState, useMemo, useDeferredValue } from "react";
import { cn } from "@/lib/utils";
import { save } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { DisplayItem } from "../types";
import { Button } from "@/components/ui/button";
import { useStore } from "@/lib/store";

interface ResultsProps {
    results: RgMatch[];
    displayItems: DisplayItem[];
    query: string;
    selectedIndex: number;
    onOpenFile: (filePath: string) => void;
    onSelect?: (index: number) => void;
    limitReached?: boolean;
    onRerunSearch?: (newLimit: number | null) => void;
}

// Helper function to get icon based on file extension
const getFileIcon = (filePath: string) => {
    const ext = filePath.split('.').pop()?.toLowerCase() || '';

    const iconMap: Record<string, typeof FileText> = {
        // Code files
        'ts': FileCode2,
        'tsx': FileCode2,
        'js': FileCode2,
        'jsx': FileCode2,
        'py': FileCode2,
        'rs': FileCode2,
        'go': FileCode2,
        'java': FileCode2,
        'cpp': FileCode2,
        'c': FileCode2,
        'cs': FileCode2,

        // Images
        'png': FileImage,
        'jpg': FileImage,
        'jpeg': FileImage,
        'gif': FileImage,
        'svg': FileImage,
        'webp': FileImage,

        // Archives
        'zip': FileArchive,
        'rar': FileArchive,
        '7z': FileArchive,
        'tar': FileArchive,
        'gz': FileArchive,

        // Audio
        'mp3': FileAudio,
        'wav': FileAudio,
        'flac': FileAudio,
        'ogg': FileAudio,

        // Video
        'mp4': FileVideo,
        'avi': FileVideo,
        'mkv': FileVideo,
        'mov': FileVideo,
    };

    return iconMap[ext] || FileText;
};

export function Results({ results, displayItems: propDisplayItems, query, selectedIndex, onOpenFile, onSelect, limitReached, onRerunSearch }: ResultsProps) {
    const { settings, setSettings } = useStore();
    const listRef = useRef<VirtualListHandle>(null);
    const [isExporting, setIsExporting] = useState(false);
    const [liveQuery, setLiveQuery] = useState("");
    const deferredLiveQuery = useDeferredValue(liveQuery);
    const [activeFileTypes, setActiveFileTypes] = useState<string[]>([]);
    const [showLimitOptions, setShowLimitOptions] = useState(false);

    // Count matches (excluding summary/context types)
    const matchCount = useMemo(() => {
        return results.filter(r => r.type === 'match').length;
    }, [results]);

    // Handle limit change and re-run search
    const handleLimitChange = (newLimit: number | null) => {
        setSettings({ maxResults: newLimit });
        setShowLimitOptions(false);
        onRerunSearch?.(newLimit);
    };

    // Calculate top 3 file types
    const topExtensions = useMemo(() => {
        const counts = new Map<string, number>();
        results.forEach(r => {
            if (r.type === 'match' && r.data.path?.text) {
                const parts = r.data.path.text.split('.');
                const ext = parts.length > 1 ? parts.pop()?.toLowerCase() || '' : 'no-ext';
                counts.set(ext, (counts.get(ext) || 0) + 1);
            }
        });
        return Array.from(counts.entries())
            .sort((a, b) => b[1] - a[1])
            .slice(0, 3)
            .map(([ext]) => ext);
    }, [results]);

    // Group and Filter Logic
    const displayItems = useMemo(() => {
        if (!deferredLiveQuery && activeFileTypes.length === 0) {
            return propDisplayItems;
        }

        // Only compute matchesWithIndex when we need to filter
        const matchesWithIndex = results
            .map((r, i) => ({ ...r, originalIndex: i }))
            .filter((r): r is RgMatch & { originalIndex: number } => r.type === "match");

        // Filter matches
        const filtered = matchesWithIndex.filter((m) => {
            const text = m.data.lines?.text || "";
            const path = m.data.path?.text || "";

            // File Type Filter
            if (activeFileTypes.length > 0) {
                const parts = path.split('.');
                const ext = parts.length > 1 ? parts.pop()?.toLowerCase() || '' : 'no-ext';
                if (!activeFileTypes.includes(ext)) {
                    return false;
                }
            }

            // Query Filter
            if (deferredLiveQuery) {
                const lowerQuery = deferredLiveQuery.toLowerCase();
                return text.toLowerCase().includes(lowerQuery) ||
                    path.toLowerCase().includes(lowerQuery);
            }

            return true;
        });

        // Group by file
        const grouped = new Map<string, { match: RgMatch, originalIndex: number }[]>();
        filtered.forEach(item => {
            const path = item.data.path?.text || "";
            if (!grouped.has(path)) grouped.set(path, []);
            grouped.get(path)?.push({ match: item, originalIndex: item.originalIndex });
        });

        // Flatten
        const items: DisplayItem[] = [];
        for (const [file, fileMatches] of grouped) {
            items.push({ type: 'header', file, matchCount: fileMatches.length });
            fileMatches.forEach(({ match, originalIndex }) => {
                items.push({ type: 'match', match, originalIndex });
            });
        }
        return items;
    }, [results, propDisplayItems, deferredLiveQuery, activeFileTypes]);

    // Find the display index corresponding to the selected match
    const scrollIndex = useMemo(() => {
        if (selectedIndex < 0) return -1;
        // We need to find the display item that corresponds to the selectedIndex in the original results array
        return displayItems.findIndex(item => item.type === 'match' && item.originalIndex === selectedIndex);
    }, [displayItems, selectedIndex]);

    useEffect(() => {
        if (scrollIndex >= 0 && listRef.current) {
            listRef.current.scrollToItem(scrollIndex);
        }
    }, [scrollIndex]);

    const handleExport = async () => {
        // We need matchesWithIndex for export if we want to export everything
        // But matchesWithIndex is not available here if we didn't compute it.
        // We can compute it on demand for export.

        const matchesToExport = results
            .map((r, i) => ({ ...r, originalIndex: i }))
            .filter((r): r is RgMatch & { originalIndex: number } => r.type === "match");

        if (matchesToExport.length === 0) return;
        setIsExporting(true);

        try {
            const filePath = await save({
                filters: [
                    {
                        name: 'JSON',
                        extensions: ['json'],
                    },
                    {
                        name: 'CSV',
                        extensions: ['csv'],
                    },
                ],
            });

            if (!filePath) {
                setIsExporting(false);
                return;
            }

            let content = "";
            if (filePath.endsWith('.json')) {
                content = JSON.stringify(matchesToExport, null, 2);
            } else {
                // CSV format: File,Line,Content
                content = "File,Line,Content\n" + matchesToExport.map(m => {
                    const file = m.data.path?.text || "";
                    const line = m.data.line_number;
                    const text = (m.data.lines?.text || "").replace(/"/g, '""'); // Escape quotes
                    return `"${file}",${line},"${text.trim()}"`;
                }).join("\n");
            }

            await invoke("write_file_content", { path: filePath, content: content });
            toast.success("Export successful", { description: `Saved to ${filePath}` });

        } catch (error) {
            console.error("Export failed:", error);
            toast.error("Export failed", { description: error instanceof Error ? error.message : "Unknown error" });
        } finally {
            setIsExporting(false);
        }
    };

    const HighlightedText = ({ text, highlight }: { text: string; highlight: string }) => {
        if (!highlight.trim()) {
            return <span>{text}</span>;
        }
        const parts = text.split(new RegExp(`(${highlight})`, 'gi'));
        return (
            <span>
                {parts.map((part, i) =>
                    part.toLowerCase() === highlight.toLowerCase() ? (
                        <span key={i} className="bg-pink-500/30 text-pink-700 dark:text-pink-200 rounded px-0.5 font-bold border border-pink-500/50">
                            {part}
                        </span>
                    ) : (
                        <span key={i}>{part}</span>
                    )
                )}
            </span>
        );
    };

    const Row = ({ index, style }: { index: number; style: React.CSSProperties }) => {
        const item = displayItems[index];
        if (!item) return null;

        if (item.type === 'header') {
            const FileIcon = getFileIcon(item.file);
            const fileName = item.file.split(/[\\/]/).pop() || item.file;
            const matchCountDisplay = item.matchCount >= 0 ? item.matchCount : "";

            return (
                <div style={style} className="px-4 py-1">
                    <div className="flex items-center gap-2 p-2 bg-zinc-100/80 dark:bg-zinc-800/80 rounded-lg border border-zinc-200 dark:border-zinc-700/50 backdrop-blur-sm h-full">
                        <div className="p-1 bg-white dark:bg-zinc-900 rounded text-pink-500 shrink-0">
                            <FileIcon className="h-4 w-4" />
                        </div>
                        <span className="font-semibold text-sm text-zinc-900 dark:text-zinc-100 truncate flex-1" title={item.file}>
                            {fileName}
                        </span>
                        {matchCountDisplay !== "" && (
                            <span className="text-xs text-zinc-500 font-mono bg-zinc-200 dark:bg-zinc-900 px-1.5 py-0.5 rounded shrink-0">
                                {matchCountDisplay}
                            </span>
                        )}
                    </div>
                </div>
            );
        }

        // Match Item
        const match = item.match;
        const data = match.data;
        const lineContent = data.lines?.text?.trim() || "";
        const lineNumber = data.line_number;
        const isSelected = item.originalIndex === selectedIndex;

        // Truncate line content
        const truncatedContent =
            lineContent.length > 150
                ? lineContent.substring(0, 150) + "..."
                : lineContent;

        return (
            <div style={style} className="px-4 py-1">
                <ContextMenu>
                    <ContextMenuTrigger>
                        <div
                            onClick={() => onSelect?.(item.originalIndex)}
                            className={cn(
                                "flex items-center gap-3 rounded-lg border p-2 pl-8 shadow-sm transition-all cursor-pointer h-full",
                                isSelected
                                    ? "bg-pink-50 dark:bg-zinc-800 border-pink-500/50 shadow-pink-500/10"
                                    : "bg-white/40 dark:bg-zinc-900/40 border-zinc-200 dark:border-zinc-800 hover:bg-white/60 dark:hover:bg-zinc-800/60 hover:border-pink-300 dark:hover:border-zinc-700"
                            )}
                        >
                            <span className="shrink-0 text-zinc-400 dark:text-zinc-600 select-none min-w-[3ch] text-right font-mono text-xs">
                                {lineNumber}
                            </span>
                            <code className="break-all text-zinc-700 dark:text-zinc-300 flex-1 font-mono text-xs leading-relaxed truncate">
                                <HighlightedText text={truncatedContent} highlight={query} />
                            </code>
                        </div>
                    </ContextMenuTrigger>
                    <ContextMenuContent className="w-64">
                        <ContextMenuItem onClick={() => {
                            navigator.clipboard.writeText(match.data.path?.text || "");
                            toast.success("Copied path to clipboard");
                        }}>
                            <Copy className="mr-2 h-4 w-4" /> Copy Path
                        </ContextMenuItem>
                        <ContextMenuItem onClick={() => {
                            navigator.clipboard.writeText(lineContent);
                            toast.success("Copied content to clipboard");
                        }}>
                            <FileText className="mr-2 h-4 w-4" /> Copy Content
                        </ContextMenuItem>
                        <ContextMenuSeparator />
                        <ContextMenuItem onClick={() => onOpenFile(match.data.path?.text || "")}>
                            <ExternalLink className="mr-2 h-4 w-4" /> Open File
                        </ContextMenuItem>
                        <ContextMenuItem onClick={() => onOpenFile(match.data.path?.text || "")}>
                            <FolderOpen className="mr-2 h-4 w-4" /> Reveal in Explorer
                        </ContextMenuItem>
                    </ContextMenuContent>
                </ContextMenu>
            </div>
        );
    };

    return (
        <div className="h-full w-full flex-1 overflow-hidden bg-transparent flex flex-col">
            {displayItems.length === 0 ? (
                <div className="flex h-full flex-col items-center justify-center text-muted-foreground gap-4">
                    <div className="p-6 bg-zinc-100 dark:bg-zinc-900/50 rounded-full border border-zinc-300 dark:border-zinc-800 transition-colors duration-300">
                        <FileText className="h-12 w-12 opacity-20" />
                    </div>
                    <p className="text-zinc-600 dark:text-zinc-500 transition-colors duration-300">No results found</p>
                </div>
            ) : (
                <>
                    <div className="shrink-0 px-4 py-3 border-b border-zinc-300 dark:border-zinc-800 bg-white/80 dark:bg-zinc-900/80 backdrop-blur-md flex flex-col gap-3 z-10 transition-colors duration-300">
                        <div className="flex justify-between items-center">
                            <div className="flex flex-col gap-2">
                                <div className="flex items-center gap-2">
                                    <p className="text-sm text-zinc-600 dark:text-zinc-400 font-medium transition-colors duration-300">
                                        Found <span className="text-pink-500 font-bold">{matchCount}</span> matches
                                    </p>
                                    {limitReached && (
                                        <div className="relative">
                                            <button
                                                onClick={() => setShowLimitOptions(!showLimitOptions)}
                                                className="flex items-center gap-1.5 px-2 py-1 bg-amber-500/10 hover:bg-amber-500/20 border border-amber-500/30 rounded-lg text-amber-600 dark:text-amber-400 text-xs font-medium transition-colors"
                                                title="Search hit the limit. Click to adjust."
                                            >
                                                <AlertTriangle className="w-3.5 h-3.5" />
                                                <span>Limit reached</span>
                                            </button>
                                            {showLimitOptions && (
                                                <div className="absolute top-full left-0 mt-1 p-2 bg-white dark:bg-zinc-900 border border-zinc-200 dark:border-zinc-700 rounded-lg shadow-lg z-50 min-w-[160px]">
                                                    <p className="text-xs text-zinc-500 dark:text-zinc-400 mb-2 px-1">Continue with:</p>
                                                    <div className="flex flex-col gap-1">
                                                        {[500, 1000, 5000].map(limit => (
                                                            <button
                                                                key={limit}
                                                                onClick={() => handleLimitChange(limit)}
                                                                className={cn(
                                                                    "px-3 py-1.5 text-xs font-medium rounded-md transition-colors text-left",
                                                                    settings.maxResults === limit
                                                                        ? "bg-pink-500 text-white"
                                                                        : "hover:bg-zinc-100 dark:hover:bg-zinc-800 text-zinc-700 dark:text-zinc-300"
                                                                )}
                                                            >
                                                                {limit.toLocaleString()} results
                                                            </button>
                                                        ))}
                                                        <div className="border-t border-zinc-200 dark:border-zinc-700 my-1" />
                                                        <button
                                                            onClick={() => handleLimitChange(null)}
                                                            className={cn(
                                                                "px-3 py-1.5 text-xs font-medium rounded-md transition-colors text-left",
                                                                settings.maxResults === null
                                                                    ? "bg-pink-500 text-white"
                                                                    : "hover:bg-zinc-100 dark:hover:bg-zinc-800 text-zinc-700 dark:text-zinc-300"
                                                            )}
                                                        >
                                                            Remove limit
                                                        </button>
                                                    </div>
                                                </div>
                                            )}
                                        </div>
                                    )}
                                </div>
                                {topExtensions.length > 0 && (
                                    <div className="flex gap-2">
                                        {topExtensions.map(ext => (
                                            <Button
                                                key={ext}
                                                variant={activeFileTypes.includes(ext) ? "default" : "outline"}
                                                size="sm"
                                                onClick={() => setActiveFileTypes(prev =>
                                                    prev.includes(ext) ? prev.filter(e => e !== ext) : [...prev, ext]
                                                )}
                                                className={cn(
                                                    "h-6 text-xs px-2",
                                                    activeFileTypes.includes(ext)
                                                        ? "bg-pink-500 hover:bg-pink-600 text-white border-transparent"
                                                        : "text-zinc-500 dark:text-zinc-400 border-zinc-200 dark:border-zinc-700 hover:bg-zinc-100 dark:hover:bg-zinc-800"
                                                )}
                                            >
                                                .{ext}
                                            </Button>
                                        ))}
                                    </div>
                                )}
                            </div>
                            <button
                                onClick={handleExport}
                                disabled={isExporting}
                                className="flex items-center gap-2 px-3 py-1.5 bg-zinc-200 hover:bg-zinc-300 dark:bg-zinc-800 dark:hover:bg-zinc-700 rounded-lg text-xs font-medium text-zinc-700 dark:text-zinc-300 transition-colors border border-zinc-300 dark:border-zinc-700"
                            >
                                {isExporting ? <Loader size={14} /> : <Download className="w-3.5 h-3.5" />}
                                {isExporting ? "Exporting..." : "Export"}
                            </button>
                        </div>
                        <div className="relative">
                            <SearchIcon className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-zinc-400" />
                            <input
                                type="text"
                                placeholder="Filter results..."
                                value={liveQuery}
                                onChange={(e) => setLiveQuery(e.target.value)}
                                className="w-full pl-9 pr-4 py-2 bg-white dark:bg-zinc-950 border border-zinc-200 dark:border-zinc-800 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-pink-500/20 transition-all"
                            />
                        </div>
                    </div>
                    <div className="flex-1 min-h-0 p-2">
                        <AutoSizer>
                            {({ height, width }) => (
                                <VirtualList
                                    ref={listRef}
                                    height={height}
                                    width={width}
                                    itemCount={displayItems.length}
                                    itemSize={50}
                                >
                                    {Row}
                                </VirtualList>
                            )}
                        </AutoSizer>
                    </div>
                </>
            )}
        </div>
    );
}
