import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { escapeRegex } from "./text";

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

export interface SearchOptions {
    caseSensitive?: boolean;
    wholeWord?: boolean;
    regex?: boolean;
    exclusions?: string[];
    maxResults?: number | null;
}

export async function searchBatched(
    query: string,
    path: string,
    searchId: string,
    onData: (lines: string[]) => void,
    onFinished: () => void,
    onError: (error: string) => void,
    options: SearchOptions = {}
) {
    // --no-messages hides per-file errors (e.g. Access Denied); pattern errors still reach stderr.
    // The total result limit is enforced Rust-side (--max-count is per file).
    const args: string[] = ["--json", "--no-messages", "--stats"];

    args.push(options.caseSensitive ? "--case-sensitive" : "--smart-case");

    if (options.wholeWord) {
        args.push("--word-regexp");
    }

    let finalQuery = query;

    if (query.includes('\n')) {
        args.push("--multiline", "--multiline-dotall");
        // Textareas normalize to \n but files may use \r\n, so literal searches match both.
        if (!options.regex) {
            finalQuery = escapeRegex(query).replace(/\n/g, '\\r?\\n');
        }
    } else if (!options.regex) {
        args.push("--fixed-strings");
    }

    for (const exclusion of options.exclusions ?? []) {
        args.push("--glob", `!${exclusion}`);
    }

    // "--" so queries starting with "-" aren't parsed as flags.
    args.push("--", finalQuery, path);

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
