/** How often and how recently a folder was opened. */
export interface Visit {
    count: number;
    last: number;
}

const HOUR = 3_600_000;

/** zoxide-style rank: the number of visits, weighted by how recent the last one was. */
export function frecency(visit: Visit, now = Date.now()) {
    const age = now - visit.last;
    const weight = age < HOUR ? 4 : age < 24 * HOUR ? 2 : age < 7 * 24 * HOUR ? 0.5 : 0.25;
    return visit.count * weight;
}

const isBoundary = (text: string, i: number) => i === 0 || /[\s\\/._\-()[\]]/.test(text[i - 1]) || (text[i] !== text[i].toLowerCase() && text[i - 1] === text[i - 1].toLowerCase());

/**
 * Scores `text` for `query` typed as an in-order subsequence ("dl" finds "Downloads"), or null when
 * it doesn't match. Letters at word starts and runs of consecutive letters score higher, so the best
 * match comes first. Typed with a typo ("stetings"), it's a close match (`typoDistance`), below them all.
 * `tight` (for a path, where letters typed turn up anywhere): only letters close together match, as
 * zf matches a path.
 */
export function fuzzyScore(query: string, text: string, tight = false): number | null {
    const q = query.toLowerCase().replace(/\s+/g, "");
    if (!q) return 0;
    const lower = text.toLowerCase();
    // The letters in order from `start`, each as early as it comes: the score, and where it ended.
    const from = (start: number) => {
        let score = 0;
        let at = start;
        let streak = 0;
        for (const c of q) {
            const found = lower.indexOf(c, at);
            if (found < 0) return null;
            streak = found === at ? streak + 1 : 0;
            score += 1 + (isBoundary(text, found) ? 3 : 0) + 2 * streak - 0.05 * (found - at);
            at = found + 1;
        }
        return { score, end: at };
    };
    let best: number | null = null;
    if (!tight) best = from(0)?.score ?? null;
    else {
        // From each place the first letter is, the best match that stays close together.
        const head = [...q][0];
        for (let start = lower.indexOf(head); start >= 0; start = lower.indexOf(head, start + 1)) {
            const match = from(start);
            if (!match) break;
            if (match.end - start <= 3 * q.length + 2 && (best === null || match.score > best)) best = match.score;
        }
    }
    if (best === null) {
        const edits = typoDistance(query, text);
        return edits === null ? null : -1 - edits;
    }
    // Among equal matches, the shorter text wins.
    return best - 0.01 * text.length;
}

/** Character indices in `text` that match `query`'s letters in order (preferring word boundaries and consecutive runs), or null. */
export function fuzzyIndices(query: string, text: string, tight = false): number[] | null {
    const q = query.toLowerCase().replace(/\s+/g, "");
    if (!q) return null;
    const lower = text.toLowerCase();
    const from = (start: number) => {
        let score = 0;
        let at = start;
        let streak = 0;
        const indices: number[] = [];
        for (const c of q) {
            const found = lower.indexOf(c, at);
            if (found < 0) return null;
            indices.push(found);
            streak = found === at ? streak + 1 : 0;
            score += 1 + (isBoundary(text, found) ? 3 : 0) + 2 * streak - 0.05 * (found - at);
            at = found + 1;
        }
        return { score, end: at, indices };
    };
    const head = [...q][0];
    let best: { score: number; indices: number[] } | null = null;
    for (let start = lower.indexOf(head); start >= 0; start = lower.indexOf(head, start + 1)) {
        const match = from(start);
        if (!match) break;
        if ((!tight || match.end - start <= 3 * q.length + 2) && (best === null || match.score > best.score)) {
            best = { score: match.score, indices: match.indices };
        }
    }
    return best?.indices ?? null;
}

/** How many edits a word may be off by in a close match: one from 4 letters, two from 8. A shorter word, or a number, must be there as typed. */
export const editsAllowed = (word: string) => {
    const letters = [...word].length;
    return letters < 4 || /^\p{N}+$/u.test(word) ? 0 : letters < 8 ? 1 : 2;
};

/** u: a capital, l: another letter, d: a digit, o: anything else. */
function charKind(c: string): "u" | "l" | "d" | "o" {
    const code = c.charCodeAt(0);
    if (code < 128) return code >= 65 && code <= 90 ? "u" : code >= 97 && code <= 122 ? "l" : code >= 48 && code <= 57 ? "d" : "o";
    return /\p{Lu}/u.test(c) ? "u" : /\p{L}/u.test(c) ? "l" : /\p{N}/u.test(c) ? "d" : "o";
}

/** Where a name's words start: its start, after anything but a letter or digit, between letters and digits, and at a capital after a small letter. */
function wordStarts(chars: string[]): number[] {
    const starts: number[] = [];
    let previous: ReturnType<typeof charKind> = "o";
    chars.forEach((c, i) => {
        const kind = charKind(c);
        const letter = (k: string) => k === "u" || k === "l";
        if (i === 0 || previous === "o" || (letter(previous) && kind === "d") || (previous === "d" && letter(kind)) || (previous === "l" && kind === "u")) starts.push(i);
        previous = kind;
    });
    return starts;
}

/** The fewest edits (a letter added, dropped or changed, or two neighbours swapped) that make `word` a prefix of `text`. */
function prefixDistance(word: string[], text: string[]): number {
    const m = word.length;
    let before: number[] = new Array(m + 1).fill(0);
    let last = Array.from({ length: m + 1 }, (_, i) => i);
    let best = last[m];
    for (let j = 1; j <= text.length; j++) {
        const column: number[] = [j];
        for (let i = 1; i <= m; i++) {
            let d = Math.min(last[i] + 1, column[i - 1] + 1, last[i - 1] + (word[i - 1] === text[j - 1] ? 0 : 1));
            if (i > 1 && j > 1 && word[i - 1] === text[j - 2] && word[i - 2] === text[j - 1]) d = Math.min(d, before[i - 2] + 1);
            column.push(d);
        }
        best = Math.min(best, column[m]);
        [before, last] = [last, column];
    }
    return best;
}

/**
 * How many typos `query` is from `name`: each word of it within an edit of the start of a word of the
 * name (two from 8 letters; two letters swapped are one edit, and the first letter must be right or
 * swapped with the second), and shorter words and numbers in it as typed. The edits in all, or null
 * when it isn't close. As the instant search service finds close matches (query.rs).
 */
export function typoDistance(query: string, name: string): number | null {
    const chars = [...name];
    // One character each (a lowercase form can be longer), so positions stay the name's.
    const lower = chars.map((c) => [...c.toLowerCase()][0] ?? c);
    const flat = name.toLowerCase();
    const starts = wordStarts(chars);
    let total = 0;
    for (const term of query.toLowerCase().split(/\s+/).filter(Boolean)) {
        const k = editsAllowed(term);
        if (k === 0) {
            if (!flat.includes(term)) return null;
            continue;
        }
        const word = [...term];
        let best: number | null = null;
        for (const s of starts) {
            if (lower[s] !== word[0] && lower[s] !== word[1]) continue;
            const d = prefixDistance(word, lower.slice(s, s + word.length + k));
            if (d <= k && (best === null || d < best)) best = d;
        }
        if (best === null) return null;
        total += best;
    }
    return total;
}
