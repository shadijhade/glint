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
 * match comes first.
 */
export function fuzzyScore(query: string, text: string): number | null {
    const q = query.toLowerCase().replace(/\s+/g, "");
    if (!q) return 0;
    const lower = text.toLowerCase();
    let score = 0;
    let at = 0;
    let streak = 0;
    for (const c of q) {
        const found = lower.indexOf(c, at);
        if (found < 0) return null;
        streak = found === at ? streak + 1 : 0;
        score += 1 + (isBoundary(text, found) ? 3 : 0) + 2 * streak - 0.05 * (found - at);
        at = found + 1;
    }
    // Among equal matches, the shorter text wins.
    return score - 0.01 * text.length;
}
