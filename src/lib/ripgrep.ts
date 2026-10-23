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
        /** A Documents search's match: where it is in the document ("p. 3", "slide 2", "Sales, row 7"). */
        label?: string;
        elapsed_total?: { human: string; nanos: number; secs: number };
        stats?: {
            elapsed?: { human: string; nanos: number; secs: number };
            searches?: number;
            searches_with_match?: number;
            bytes_searched?: number;
            bytes_printed?: number;
            matched_lines?: number;
            matches?: number;
            /** A Documents search's documents that couldn't be read (damaged, locked by a password, too big or too slow). */
            unreadable?: number;
        };
    };
}

/** What Documents search reads (doctext.rs's DOCUMENTS), and pictures, whose text OCR reads (ocr.rs's PICTURES). */
const DOCUMENT_EXTENSIONS = new Set(["pdf", "docx", "docm", "dotx", "dotm", "pptx", "pptm", "ppsx", "potx", "xlsx", "xlsm", "xlsb", "xls", "ods", "odt", "ott", "odp", "epub"]);
const PICTURE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "jpe", "jfif", "bmp", "gif", "tif", "tiff", "webp", "heic", "heif", "avif", "jxr"]);

/** Whether Documents search reads this file (a document, or a picture with OCR): its app opens it, not the text editor. */
export const isDocument = (path: string) => {
    const ext = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
    return DOCUMENT_EXTENSIONS.has(ext) || PICTURE_EXTENSIONS.has(ext);
};

/** A document's lines `from` to `to` (1-based) as Documents search read them, each with where it is. */
export const docLines = (path: string, from: number, to: number) => invoke<[string, string][]>("doc_lines", { path, from, to });

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
    /** The text inside documents, read by Glint's workers (doctext.rs), and with `ocr`, pictures' too. */
    documents?: boolean;
    ocr?: boolean;
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
        const text = { query, path, caseSensitive: !!options.caseSensitive, wholeWord: !!options.wholeWord, regex: !!options.regex, exclusions, everything: !!options.everything, ocr: !!options.ocr };
        const folder = options.code
            ? await invoke<string>("run_ast_grep", { request: { query, path, rewrite: options.rewrite || null, exclusions, everything: !!options.everything }, searchId, maxResults: options.maxResults })
            : await invoke<string>(options.documents ? "run_doc_search" : "run_ripgrep_batched", { request: text, searchId, maxResults: options.maxResults });
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
