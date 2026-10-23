/** An asciinema recording, ready to play: its terminal's size, and what it printed when. */
export interface Cast {
    cols: number;
    rows: number;
    title: string;
    /** Output, at seconds from the start (long pauses cut short). */
    events: { t: number; data: string }[];
    duration: number;
}

/**
 * An asciinema .cast file: a line of JSON saying the terminal's size, then a line per event, `[time,
 * code, data]`, the time from the start (version 2) or since the last event (version 3). Only output
 * ("o") is played. Pauses longer than the file's idle_time_limit (else `idle` seconds) are cut to it, as
 * asciinema's player does when asked. Null when it isn't one.
 */
export function parseCast(text: string, idle = 3): Cast | null {
    const lines = text.split(/\r?\n/);
    let header: { version?: number; width?: number; height?: number; term?: { cols?: number; rows?: number }; title?: string; idle_time_limit?: number };
    try {
        header = JSON.parse(lines[0] ?? "");
    } catch {
        return null;
    }
    if (typeof header !== "object" || !header || (header.version !== 2 && header.version !== 3)) return null;
    const relative = header.version === 3;
    const cols = Math.max(1, Math.min(1000, (relative ? header.term?.cols : header.width) ?? 80));
    const rows = Math.max(1, Math.min(500, (relative ? header.term?.rows : header.height) ?? 24));
    const limit = header.idle_time_limit && header.idle_time_limit > 0 ? header.idle_time_limit : idle;
    const events: Cast["events"] = [];
    let clock = 0;
    let last = 0;
    for (const line of lines.slice(1)) {
        if (!line.trim() || line.startsWith("#")) continue;
        let event: unknown;
        try {
            event = JSON.parse(line);
        } catch {
            continue;
        }
        if (!Array.isArray(event) || typeof event[0] !== "number") continue;
        const at = relative ? last + event[0] : event[0];
        // The pause before it, cut short.
        clock += Math.min(Math.max(0, at - last), limit);
        last = at;
        if (event[1] === "o" && typeof event[2] === "string") events.push({ t: clock, data: event[2] });
    }
    return { cols, rows, title: header.title ?? "", events, duration: clock };
}

/** The output up to `seconds` in, all at once (to show a moment of it), and how many events that is. */
export function outputUntil(cast: Cast, seconds: number): { data: string; count: number } {
    let count = 0;
    while (count < cast.events.length && cast.events[count].t <= seconds) count++;
    return { data: cast.events.slice(0, count).map((e) => e.data).join(""), count };
}

/** 1:05 */
export const clock = (seconds: number) => `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, "0")}`;
