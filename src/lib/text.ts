import type { RgMatch } from "./ripgrep";
import type { SearchOptions } from "./store";

export const escapeRegex = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** JS version of an rg search, for Replace All. Mirrors rg's --smart-case so it never replaces more than was found. */
export function toRegExp(query: string, options: SearchOptions) {
    let source = options.regex ? query : escapeRegex(query).replace(/\n/g, "\\r?\\n");
    if (options.wholeWord) source = `\\b(?:${source})\\b`;
    const ignoreCase = !options.caseSensitive && query === query.toLowerCase();
    // Over several lines, ^ and $ still mean a line's start and end, as they do in rg.
    return new RegExp(source, "g" + (ignoreCase ? "i" : "") + (query.includes("\n") ? "ms" : ""));
}

/**
 * Replace All in one file's text, the way rg found the matches: line by line, so ^, $ and \s stay
 * within a line and line ends never change, unless the query itself spans lines.
 */
export function replaceText(content: string, query: string, options: SearchOptions, replacement: string) {
    const pattern = toRegExp(query, options);
    // Literal searches insert the replacement verbatim; regex searches allow $1-style groups.
    const replace = (text: string) => (options.regex ? text.replace(pattern, replacement) : text.replace(pattern, () => replacement));
    if (query.includes("\n")) return replace(content);
    return content
        .split(/(\r?\n)/)
        .map((part, i) => (i % 2 ? part : replace(part)))
        .join("");
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** Splits a match line into plain/hit parts using rg's UTF-8 byte offsets, trimmed for a single-line row. */
export function segments(m: RgMatch) {
    // ponytail: only the first 2000 chars are rendered; hits beyond that in minified lines aren't shown.
    const bytes = encoder.encode((m.data.lines?.text ?? "").slice(0, 2000));
    const parts: { text: string; hit: boolean }[] = [];
    let pos = 0;
    for (const { start, end } of m.data.submatches ?? []) {
        if (end > bytes.length) break;
        parts.push({ text: decoder.decode(bytes.subarray(pos, start)), hit: false });
        parts.push({ text: decoder.decode(bytes.subarray(start, end)), hit: true });
        pos = end;
    }
    parts.push({ text: decoder.decode(bytes.subarray(pos)).trimEnd(), hit: false });
    // Drop indentation and keep the first hit visible on long lines.
    const lead = parts[0].text.trimStart();
    parts[0].text = parts.length > 1 && lead.length > 40 ? "…" + lead.slice(-30) : lead;
    return parts;
}
