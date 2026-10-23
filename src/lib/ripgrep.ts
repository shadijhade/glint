import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";

/** The replace box: open or not, and what it says (a code search previews it on each match). */
export const useRewrite = create<{ open: boolean; text: string }>(() => ({ open: false, text: "" }));

export interface RgMatch {
    type: "match" | "context" | "begin" | "end" | "summary";
    data: {
        path?: { text: string };
        lines?: { text: string };
        line_number?: number;
        absolute_offset?: number;
        submatches?: Array<{ match: { text: string }; start: number; end: number }>;
        /** A code search with a rewrite: what this match becomes. */
        replacement?: string;
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
    /** Skipping nothing: hidden files, what .gitignore files list and the exclusions too. */
    everything?: boolean;
    /** A code pattern, searched by ast-grep (codesearch.rs), and what each match becomes. */
    code?: boolean;
    rewrite?: string | null;
}

/** A code search's rewrite made in its files, by ast-grep: how many matches it rewrote. */
export const astGrepApply = (query: string, path: string, rewrite: string, options: SearchOptions) =>
    invoke<number>("ast_grep_apply", { request: { query, path, rewrite, exclusions: options.exclusions ?? [], everything: !!options.everything } });

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
        // ripgrep's (or ast-grep's) arguments are built in Rust from these options. The folder comes back as searched.
        const exclusions = options.exclusions ?? [];
        const folder = options.code
            ? await invoke<string>("run_ast_grep", { request: { query, path, rewrite: options.rewrite || null, exclusions, everything: !!options.everything }, searchId, maxResults: options.maxResults })
            : await invoke<string>("run_ripgrep_batched", {
                  request: { query, path, caseSensitive: !!options.caseSensitive, wholeWord: !!options.wholeWord, regex: !!options.regex, exclusions, everything: !!options.everything },
                  searchId,
                  maxResults: options.maxResults,
              });
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
