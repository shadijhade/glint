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
import { FileText, Folder, FileCode2, FileImage, FileArchive, FileAudio, FileVideo, Download, Copy, ExternalLink, FolderOpen } from "lucide-react";
import { useRef, useEffect, useState } from "react";
import { cn } from "@/lib/utils";
import { save } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";

interface ResultsProps {
    results: RgMatch[];
    query: string;
    selectedIndex: number;
    onOpenFile: (filePath: string) => void;
    onSelect?: (index: number) => void;
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

export function Results({ results, query, selectedIndex, onOpenFile, onSelect }: ResultsProps) {
    // Filter only match events for the list
    const matches = results.filter((r) => r.type === "match");
    const listRef = useRef<VirtualListHandle>(null);
    const [isExporting, setIsExporting] = useState(false);

    useEffect(() => {
        if (selectedIndex >= 0 && listRef.current) {
            listRef.current.scrollToItem(selectedIndex);
        }
    }, [selectedIndex]);

    const handleExport = async () => {
        if (matches.length === 0) return;
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
                content = JSON.stringify(matches, null, 2);
            } else {
                // CSV format: File,Line,Content
                content = "File,Line,Content\n" + matches.map(m => {
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
                        <span key={i} className="bg-pink-500/30 text-pink-200 rounded px-0.5 font-bold border border-pink-500/50">
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
        const match = matches[index];
        if (!match) return null;

        const data = match.data;
        const filePath = data.path?.text || "";
        const lineContent = data.lines?.text?.trim() || "";
        const lineNumber = data.line_number;
        const isSelected = index === selectedIndex;

        const FileIcon = getFileIcon(filePath);
        const fileName = filePath.split(/[\\/]/).pop() || filePath;
        const folderPath = filePath.substring(0, filePath.lastIndexOf(fileName));

        // Truncate line content
        const truncatedContent =
            lineContent.length > 150
                ? lineContent.substring(0, 150) + "..."
                : lineContent;

        return (
            <div style={style} className="px-4 py-2">
                <ContextMenu>
                    <ContextMenuTrigger>
                        <div
                            onClick={() => onSelect?.(index)}
                            className={cn(
                                "flex flex-col gap-2 rounded-xl border p-4 shadow-lg transition-all backdrop-blur-sm group cursor-pointer",
                                isSelected
                                    ? "bg-pink-50 dark:bg-zinc-800 border-pink-500/50 shadow-pink-500/10 translate-x-2"
                                    : "bg-white/60 dark:bg-zinc-900/60 border-zinc-300 dark:border-zinc-800 hover:bg-white/80 dark:hover:bg-zinc-800/80 hover:border-pink-400 dark:hover:border-zinc-700 hover:shadow-xl hover:-translate-y-0.5"
                            )}
                        >

                            <div className="flex items-center justify-between gap-3 min-w-0 pb-2 border-b border-zinc-300/50 dark:border-zinc-800/50">
                                <div className="flex items-center gap-2 overflow-hidden flex-1 min-w-0">
                                    <div className="p-1.5 bg-pink-100 dark:bg-zinc-800 rounded-lg text-pink-500 shrink-0 transition-colors duration-300">
                                        <FileIcon className="h-4 w-4" />
                                    </div>
                                    <div className="flex flex-col min-w-0 flex-1">
                                        <span
                                            className="font-semibold text-sm text-zinc-900 dark:text-zinc-100 hover:text-pink-500 dark:hover:text-pink-400 cursor-pointer truncate transition-colors"
                                            onClick={() => onOpenFile(filePath)}
                                            title={fileName}
                                        >
                                            {fileName}
                                        </span>
                                        {folderPath && (
                                            <span className="text-[10px] text-zinc-500 dark:text-zinc-500 text-zinc-600 truncate font-mono transition-colors duration-300" title={folderPath}>
                                                {folderPath}
                                            </span>
                                        )}
                                    </div>
                                </div>
                                <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity bg-white dark:bg-zinc-900/90 rounded-lg p-1 border border-zinc-300 dark:border-zinc-800">
                                    <button
                                        className="p-1.5 flex items-center justify-center text-zinc-400 hover:text-pink-400 hover:bg-zinc-800 transition-all rounded-md"
                                        onClick={() => onOpenFile(filePath)}
                                        title="Open file"
                                    >
                                        <FileText className="h-3.5 w-3.5" />
                                    </button>
                                    <button
                                        className="p-1.5 flex items-center justify-center text-zinc-400 hover:text-pink-400 hover:bg-zinc-800 transition-all rounded-md"
                                        onClick={() => onOpenFile(folderPath || filePath)}
                                        title="Open folder"
                                    >
                                        <Folder className="h-3.5 w-3.5" />
                                    </button>
                                </div>
                            </div>
                            {/* Line content section */}
                            <div className="flex items-start gap-3 mt-1 bg-zinc-50 dark:bg-black/40 rounded-lg p-2 border border-zinc-300 dark:border-zinc-800/50 font-mono text-xs transition-colors duration-300">
                                <span className="shrink-0 text-zinc-400 dark:text-zinc-600 select-none min-w-[3ch] text-right border-r border-zinc-300 dark:border-zinc-800 pr-2 mr-1 transition-colors duration-300">
                                    {lineNumber}
                                </span>
                                <code className="break-all text-zinc-700 dark:text-zinc-300 flex-1 leading-relaxed transition-colors duration-300">
                                    <HighlightedText text={truncatedContent} highlight={query} />
                                </code>
                            </div>
                        </div >
                    </ContextMenuTrigger >
                    <ContextMenuContent className="w-64">
                        <ContextMenuItem onClick={() => {
                            navigator.clipboard.writeText(filePath);
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
                        <ContextMenuItem onClick={() => onOpenFile(filePath)}>
                            <ExternalLink className="mr-2 h-4 w-4" /> Open File
                        </ContextMenuItem>
                        <ContextMenuItem onClick={() => onOpenFile(folderPath || filePath)}>
                            <FolderOpen className="mr-2 h-4 w-4" /> Reveal in Explorer
                        </ContextMenuItem>
                    </ContextMenuContent>
                </ContextMenu >
            </div >
        );
    };

    return (
        <div className="h-full w-full flex-1 overflow-hidden bg-transparent">
            {matches.length === 0 ? (
                <div className="flex h-full flex-col items-center justify-center text-muted-foreground gap-4">
                    <div className="p-6 bg-zinc-100 dark:bg-zinc-900/50 rounded-full border border-zinc-300 dark:border-zinc-800 transition-colors duration-300">
                        <FileText className="h-12 w-12 opacity-20" />
                    </div>
                    <p className="text-zinc-600 dark:text-zinc-500 transition-colors duration-300">No results found</p>
                </div>
            ) : (
                <div className="h-full flex flex-col">
                    <div className="shrink-0 px-6 py-3 border-b border-zinc-300 dark:border-zinc-800 bg-white/80 dark:bg-zinc-900/80 backdrop-blur-md flex justify-between items-center z-10 transition-colors duration-300">
                        <p className="text-sm text-zinc-600 dark:text-zinc-400 font-medium transition-colors duration-300">
                            Found <span className="text-pink-500 font-bold">{matches.length}</span> matches for "<span className="text-zinc-900 dark:text-zinc-200 transition-colors duration-300">{query}</span>"
                        </p>
                        <button
                            onClick={handleExport}
                            disabled={isExporting}
                            className="flex items-center gap-2 px-3 py-1.5 bg-zinc-200 hover:bg-zinc-300 dark:bg-zinc-800 dark:hover:bg-zinc-700 rounded-lg text-xs font-medium text-zinc-700 dark:text-zinc-300 transition-colors border border-zinc-300 dark:border-zinc-700"
                        >
                            <Download className="w-3.5 h-3.5" />
                            {isExporting ? "Exporting..." : "Export"}
                        </button>
                    </div>
                    <div className="flex-1 min-h-0 p-2">
                        <AutoSizer>
                            {({ height, width }) => (
                                <VirtualList
                                    ref={listRef}
                                    height={height}
                                    width={width}
                                    itemCount={matches.length}
                                    itemSize={130}
                                >
                                    {Row}
                                </VirtualList>
                            )}
                        </AutoSizer>
                    </div>
                </div>
            )}
        </div>
    );
}
