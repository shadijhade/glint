/** Links a Markdown preview follows: within the file, to the web or an e-mail, or to a file or folder by path. */
export function safeHref(href: string): string | null {
    const value = href.trim();
    if (!value) return null;
    if (/^(https?|mailto):/i.test(value) || value.startsWith("#")) return value;
    // Any other scheme (javascript:, data:, file:…) goes nowhere; C:\… is a path.
    if (/^[a-z][a-z\d+.-]*:/i.test(value) && !/^[a-z]:[\\/]/i.test(value)) return null;
    return value;
}

/** A line of Markdown as plain text: links as their text, without pictures, emphasis, code marks or HTML tags. */
function plain(line: string) {
    return line
        .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
        .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
        .replace(/<[^>]*>/g, "")
        .replace(/[*_`~]+/g, "")
        .replace(/\s+#+\s*$/, "")
        .replace(/&nbsp;/g, " ")
        .trim();
}

/** A Markdown document's title: its first top heading (# Title, Title over ===, or <h1>), else its first line of text. */
export function markdownTitle(text: string) {
    const lines = text.split(/\r?\n/, 300);
    for (let i = 0; i < lines.length; i++) {
        const heading = /^ {0,3}#\s+(.*)$/.exec(lines[i]) ?? /<h1\b[^>]*>(.*?)(?:<\/h1>|$)/i.exec(lines[i]);
        const title = heading ? plain(heading[1]) : /^ {0,3}=+\s*$/.test(lines[i + 1] ?? "") ? plain(lines[i]) : "";
        if (title) return title;
    }
    for (const line of lines) {
        const title = plain(line.replace(/^ {0,3}(?:#{1,6}|>|[-*+]|\d+[.)])\s+/, ""));
        if (/\p{L}/u.test(title) && !/^[-=|:\s]+$/.test(title)) return title;
    }
    return "";
}
