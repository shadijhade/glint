import { useEffect, useState } from "react";
import { FileText, Loader2 } from "lucide-react";
import { Command } from "@tauri-apps/plugin-shell";

interface PreviewProps {
    filePath: string | null;
    lineNumber?: number;
}

export function Preview({ filePath, lineNumber }: PreviewProps) {
    const [content, setContent] = useState<string>("");
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        if (!filePath) {
            setContent("");
            return;
        }

        const fetchContent = async () => {
            setLoading(true);
            setError(null);
            try {
                // Use PowerShell to read file content
                const command = Command.create("powershell", [
                    "-Command",
                    `Get-Content -Path '${filePath}' -Raw`
                ]);
                const output = await command.execute();

                if (output.code === 0) {
                    setContent(output.stdout);
                } else {
                    setError(output.stderr || "Failed to read file");
                }
            } catch (err) {
                console.error(err);
                setError("Failed to execute read command");
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
            const element = document.getElementById(`line-${lineNumber}`);
            if (element) {
                element.scrollIntoView({ behavior: "smooth", block: "center" });
            }
        }
    }, [content, lineNumber]);

    if (!filePath) {
        return (
            <div className="h-full flex flex-col items-center justify-center text-zinc-500 gap-2 bg-zinc-900/30 border-l border-zinc-800">
                <FileText className="w-12 h-12 opacity-20" />
                <p>Select a file to preview</p>
            </div>
        );
    }

    return (
        <div className="h-full flex flex-col bg-zinc-900/30 border-l border-zinc-800 overflow-hidden">
            <div className="p-3 border-b border-zinc-800 bg-zinc-900/50 backdrop-blur-sm flex items-center justify-between shrink-0">
                <span className="text-sm font-medium text-zinc-300 truncate" title={filePath}>
                    {filePath.split(/[\\/]/).pop()}
                </span>
                {loading && <Loader2 className="w-4 h-4 animate-spin text-pink-500" />}
            </div>

            <div className="flex-1 overflow-auto p-4 font-mono text-xs text-zinc-300 bg-zinc-950/50">
                {error ? (
                    <div className="text-red-400 p-4">{error}</div>
                ) : (
                    <pre className="whitespace-pre-wrap">
                        {content.split('\n').map((line, i) => {
                            const currentLine = i + 1;
                            const isMatch = currentLine === lineNumber;
                            return (
                                <div
                                    key={i}
                                    id={`line-${currentLine}`}
                                    className={`${isMatch ? "bg-pink-500/20 text-pink-200" : ""} px-2 py-0.5 rounded`}
                                >
                                    <span className="text-zinc-600 select-none w-8 inline-block text-right mr-4 border-r border-zinc-800 pr-2">
                                        {currentLine}
                                    </span>
                                    {line}
                                </div>
                            );
                        })}
                    </pre>
                )}
            </div>
        </div>
    );
}
