import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

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
): Promise<{ stop: () => void; folder?: string }> {
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
        // ripgrep's arguments are built in Rust from these options. The folder comes back as searched.
        const request = { query, path, caseSensitive: !!options.caseSensitive, wholeWord: !!options.wholeWord, regex: !!options.regex, exclusions: options.exclusions ?? [] };
        const folder = await invoke<string>("run_ripgrep_batched", { request, searchId, maxResults: options.maxResults });
        return { stop: unlisten, folder };
    } catch (e) {
        onError(String(e));
    }
    return { stop: unlisten };
}

export async function cancelSearch() {
    try {
        await invoke("cancel_search");
    } catch (e) {
        console.error("Failed to cancel search", e);
    }
}
