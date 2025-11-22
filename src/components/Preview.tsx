import { useEffect, useState, useRef } from "react";
import { FileText, Loader2 } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { vscDarkPlus, vs } from 'react-syntax-highlighter/dist/esm/styles/prism';
import { useStore } from '@/lib/store';

interface PreviewProps {
    filePath: string | null;
    lineNumber?: number;
}

const getLanguage = (filePath: string) => {
    const ext = filePath.split('.').pop()?.toLowerCase();
    switch (ext) {
        case 'ts': case 'tsx': return 'typescript';
        case 'js': case 'jsx': return 'javascript';
        case 'rs': return 'rust';
        case 'py': return 'python';
        case 'css': return 'css';
        case 'html': return 'html';
        case 'json': return 'json';
        case 'md': return 'markdown';
        case 'yml': case 'yaml': return 'yaml';
        case 'sql': return 'sql';
        case 'go': return 'go';
        case 'java': return 'java';
        case 'c': case 'cpp': case 'h': return 'cpp';
        case 'sh': return 'bash';
        case 'toml': return 'toml';
        default: return 'text';
    }
};

export function Preview({ filePath, lineNumber }: PreviewProps) {
    const { settings } = useStore();
    const isDark = settings.theme === 'dark' || (settings.theme === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
    const [content, setContent] = useState<string>("");
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const scrollRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        if (!filePath) {
            setContent("");
            return;
        }

        const fetchContent = async () => {
            setLoading(true);
            setError(null);
            try {
                const text = await invoke<string>("read_file_content", { path: filePath });
                setContent(text);
            } catch (err) {
                console.error(err);
                setError(typeof err === 'string' ? err : "Failed to read file");
            } finally {
                setLoading(false);
            }
        };

        const timeoutId = setTimeout(fetchContent, 200); // Debounce
        return () => clearTimeout(timeoutId);
    }, [filePath]);

    // Scroll to line number when content loads
    useEffect(() => {
        if (content && lineNumber) {
            // Give syntax highlighter a moment to render
            setTimeout(() => {
                const element = document.getElementById(`line-${lineNumber}`);
                if (element) {
                    element.scrollIntoView({ behavior: "smooth", block: "center" });
                }
            }, 100);
        }
    }, [content, lineNumber]);

    if (!filePath) {
        return (
            <div className="h-full flex flex-col items-center justify-center text-zinc-600 dark:text-zinc-500 gap-2 bg-zinc-100/30 dark:bg-zinc-900/30 border-l border-zinc-300 dark:border-zinc-800 transition-colors duration-300">
                <FileText className="w-12 h-12 opacity-20" />
                <p>Select a file to preview</p>
            </div>
        );
    }

    return (
        <div className="h-full flex flex-col bg-zinc-100/30 dark:bg-zinc-900/30 border-l border-zinc-300 dark:border-zinc-800 overflow-hidden transition-colors duration-300">
            <div className="p-3 border-b border-zinc-300 dark:border-zinc-800 bg-white/50 dark:bg-zinc-900/50 backdrop-blur-sm flex items-center justify-between shrink-0 transition-colors duration-300">
                <span className="text-sm font-medium text-zinc-700 dark:text-zinc-300 truncate transition-colors duration-300" title={filePath}>
                    {filePath.split(/[\\/]/).pop()}
                </span>
                {loading && <Loader2 className="w-4 h-4 animate-spin text-pink-500" />}
            </div>

            <div className="flex-1 overflow-auto relative bg-[#f8f8f8] dark:bg-[#1e1e1e] transition-colors duration-300" ref={scrollRef}>
                {error ? (
                    <div className="text-red-400 p-4">{error}</div>
                ) : (
                    <SyntaxHighlighter
                        language={getLanguage(filePath)}
                        style={isDark ? vscDarkPlus : vs}
                        showLineNumbers={true}
                        wrapLines={true}
                        customStyle={{ margin: 0, padding: '1rem', fontSize: '12px', lineHeight: '1.5', background: 'transparent' }}
                        lineNumberStyle={{ minWidth: '3em', paddingRight: '1em', color: isDark ? '#6e7681' : '#858585', textAlign: 'right' }}
                        lineProps={(lineNumberProp) => {
                            const style: React.CSSProperties = { display: 'block' };
                            if (lineNumberProp === lineNumber) {
                                style.backgroundColor = 'rgba(236, 72, 153, 0.2)'; // Pink-500 with opacity
                                style.borderLeft = '2px solid #ec4899';
                            }
                            return {
                                id: `line-${lineNumberProp}`,
                                style
                            };
                        }}
                    >
                        {content}
                    </SyntaxHighlighter>
                )}
            </div>
        </div>
    );
}
