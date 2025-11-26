import { Command, Child } from "@tauri-apps/plugin-shell";

export interface RgMatch {
    type: "match" | "context" | "begin" | "end" | "summary";
    data: {
        path?: { text: string };
        lines?: { text: string };
        line_number?: number;
        absolute_offset?: number;
        submatches?: Array<{ match: { text: string }; start: number; end: number }>;
        elapsed_total?: { human: string; nanos: number; seconds: number };
        stats?: {
            elapsed?: { human: string; nanos: number; seconds: number };
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
    const MAX_MATCHES = 20000;
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

    const unlisten = await listen<{ type: string; lines: string[] }>("search-event", (event) => {
        const { type, lines } = event.payload;
        if (type === "data") {
            onData(lines);
        } else if (type === "finished") {
            onFinished();
        } else if (type === "error") {
            onError(lines[0] || "Unknown error");
        }
    });

    try {
        await invoke("run_ripgrep_batched", { args });
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
