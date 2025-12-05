import { Command, Child } from "@tauri-apps/plugin-shell";

export interface RgMatch {
    type: "match" | "context" | "begin" | "end" | "summary";
    data: {
        path?: { text: string };
        lines?: { text: string };
        line_number?: number;
        absolute_offset?: number;
        submatches?: Array<{ match: { text: string }; start: number; end: number }>;
        elapsed_total?: { human: string; nanos: number; secs: number };
        stats?: {
            elapsed?: { human: string; nanos: number; secs: number };
            searches?: number;
            searches_with_match?: number;
            bytes_searched?: number;
            bytes_printed?: number;
            matched_lines?: number;
            matches?: number;
        };
    };
}

export async function checkRgInstalled(): Promise<boolean> {
    try {
        const command = Command.sidecar("binaries/rg", ["--version"]);
        const output = await command.execute();
        return output.code === 0;
    } catch (e) {
        console.error("Failed to check rg version", e);
        return false;
    }
}

export interface SearchOptions {
    caseSensitive?: boolean;
    wholeWord?: boolean;
    regex?: boolean;
    exclusions?: string[];
    maxResults?: number | null;
}

export async function search(
    query: string,
    path: string,
    onEvent: (events: RgMatch[]) => void,
    onFinished: () => void,
    options: SearchOptions = {}
) {
    const args = ["--json"];

    // Limit line length to prevent massive memory usage on minified files
    args.push("--max-columns");
    args.push("150");

    if (options.caseSensitive) {
        args.push("--case-sensitive");
    } else {
        args.push("--smart-case");
    }

    if (options.wholeWord) {
        args.push("--word-regexp");
    }

    if (!options.regex) {
        args.push("--fixed-strings");
    }

    if (options.exclusions) {
        for (const exclusion of options.exclusions) {
            args.push("--glob");
            args.push(`!${exclusion}`);
        }
    }

    args.push(query);
    args.push(path);

    const command = Command.sidecar("binaries/rg", args);

    let buffer: RgMatch[] = [];
    let lastEmit = Date.now();
    let totalMatches = 0;
    const BATCH_SIZE = 1000;
    const BATCH_INTERVAL = 100;
    const MAX_MATCHES = options.maxResults || 20000;
    let childProcess: Child | null = null;

    const flush = () => {
        if (buffer.length > 0) {
            onEvent([...buffer]);
            buffer = [];
            lastEmit = Date.now();
        }
    };

    command.on("close", () => {
        flush();
        onFinished();
    });

    command.on("error", (error) => {
        console.error(`command error: "${error}"`);
        flush();
        onFinished();
    });

    command.stdout.on("data", (line) => {
        try {
            const lines = line.split("\n");
            for (const l of lines) {
                if (l.trim()) {
                    try {
                        const parsed = JSON.parse(l) as RgMatch;

                        // Count matches
                        if (parsed.type === "match") {
                            totalMatches++;
                        }

                        buffer.push(parsed);

                        if (
                            buffer.length >= BATCH_SIZE ||
                            Date.now() - lastEmit >= BATCH_INTERVAL
                        ) {
                            flush();
                        }

                        if (totalMatches >= MAX_MATCHES) {
                            if (childProcess) {
                                childProcess.kill();
                            }
                            flush();
                            onFinished();
                            return;
                        }
                    } catch (e) {
                        // ignore parse errors
                    }
                }
            }
        } catch (e) {
            console.error("Failed to parse rg output", e);
        }
    });

    const child = await command.spawn();
    childProcess = child;
    return child;
}

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ... existing imports

// ... existing code

export async function searchBatched(
    query: string,
    path: string,
    searchId: string,
    onData: (lines: string[]) => void,
    onFinished: () => void,
    onError: (error: string) => void,
    options: SearchOptions = {}
) {
    const args: string[] = ["--json"];

    // Limit line length to prevent massive memory usage on minified files
    args.push("--max-columns");
    args.push("150");

    // Suppress error messages (like Access Denied)
    args.push("--no-messages");
    args.push("--stats");

    // Note: We don't use --max-count here because it limits per-file, not total.
    // The total limit is handled Rust-side via the maxResults parameter.

    if (options.caseSensitive) {
        args.push("--case-sensitive");
    } else {
        args.push("--smart-case");
    }

    if (options.wholeWord) {
        args.push("--word-regexp");
    }

    // escape regex Helper
    const escapeRegex = (str: string) => str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

    let finalQuery = query;

    // Smart Multiline Handling
    if (query.includes('\n')) {
        args.push("--multiline");
        args.push("--multiline-dotall"); // Allow . to match newlines if enabled

        // If not already a regex search, treat it as a literal but handle line ending differences
        // Browsers/Textareas normalize to \n, but Windows files have \r\n. 
        // We escape the query and replace \n with \r?\n to match both.
        if (!options.regex) {
            finalQuery = escapeRegex(query).replace(/\n/g, '\\r?\\n');
            // We are forcing a "regex" search now, so we must NOT pass --fixed-strings
        } else {
            // If it IS a regex search provided by user, they are on their own for line endings, 
            // but we still enable multiline mode.
        }
    } else {
        // Single line behavior
        if (!options.regex) {
            args.push("--fixed-strings");
        }
    }

    if (options.exclusions) {
        for (const exclusion of options.exclusions) {
            args.push("--glob");
            args.push(`!${exclusion}`);
        }
    }

    args.push(finalQuery);
    args.push(path);

    const unlisten = await listen<{ id: string; type: string; lines: string[] }>("search-event", (event) => {
        const { id, type, lines } = event.payload;
        if (id !== searchId) return;

        if (type === "data") {
            onData(lines);
        } else if (type === "finished") {
            onFinished();
        } else if (type === "error") {
            onError(lines[0] || "Unknown error");
        }
    });

    try {
        await invoke("run_ripgrep_batched", { args, searchId, maxResults: options.maxResults });
    } catch (e) {
        onError(String(e));
    }

    return unlisten;
}

export async function cancelSearch() {
    try {
        await invoke("cancel_search");
    } catch (e) {
        console.error("Failed to cancel search", e);
    }
}
