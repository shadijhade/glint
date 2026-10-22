const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];
const numberFormat = new Intl.NumberFormat();
const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const dateFormat = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric" });
const fullDateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "full", timeStyle: "medium" });
const DAY = 86_400_000;

/** 1536 → "1.5 KB", 12345678 → "11.8 MB". */
export function formatSize(bytes: number) {
    if (bytes < 1024) return `${bytes} B`;
    let value = bytes;
    let unit = 0;
    while (value >= 1024 && unit < UNITS.length - 1) {
        value /= 1024;
        unit++;
    }
    return `${value < 10 ? value.toFixed(1) : numberFormat.format(Math.round(value))} ${UNITS[unit]}`;
}

export const formatCount = (n: number) => numberFormat.format(n);

/** 1 → "1 item", 3 → "3 items". */
export const plural = (n: number, word: string) => `${formatCount(n)} ${word}${n === 1 ? "" : "s"}`;

/** 0.42 → "0.4 ms", 12.3 → "12 ms", 1234 → "1.23 s". */
export function formatDuration(ms: number) {
    if (ms < 10) return `${ms.toFixed(1)} ms`;
    if (ms < 1000) return `${Math.round(ms)} ms`;
    return `${(ms / 1000).toFixed(2)} s`;
}

let cachedTodayStart = 0;
let cachedTodayEnd = 0;
const shortDateCache = new Map<number, string>();
const fullDateCache = new Map<number, string>();
const MAX_CACHE_SIZE = 4096;

function todayStartFor(now: number): number {
    if (now >= cachedTodayStart && now < cachedTodayEnd) {
        return cachedTodayStart;
    }
    const d = new Date(now);
    d.setHours(0, 0, 0, 0);
    const start = d.getTime();
    // Only update the global day cache and flush relative "Today/Yesterday" labels when `now` is real wall-clock time.
    if (Math.abs(now - Date.now()) < 60_000) {
        cachedTodayStart = start;
        cachedTodayEnd = start + DAY;
        shortDateCache.clear();
    }
    return start;
}

/** "Today 9:31 PM", "Yesterday 8:02 AM", otherwise "Oct 3, 2026 4:15 PM". */
export function formatDate(ms: number, now?: number) {
    if (!ms) return "";
    if (now === undefined) {
        const nowMs = Date.now();
        const todayStart = todayStartFor(nowMs);
        const hit = shortDateCache.get(ms);
        if (hit !== undefined) return hit;
        const formatted = formatDateWithToday(ms, todayStart);
        if (shortDateCache.size >= MAX_CACHE_SIZE) shortDateCache.clear();
        shortDateCache.set(ms, formatted);
        return formatted;
    }
    const d = new Date(now);
    d.setHours(0, 0, 0, 0);
    return formatDateWithToday(ms, d.getTime());
}

function formatDateWithToday(ms: number, todayStart: number): string {
    const date = new Date(ms);
    const time = timeFormat.format(date);
    if (ms >= todayStart && ms < todayStart + DAY) return `Today ${time}`;
    if (ms >= todayStart - DAY && ms < todayStart) return `Yesterday ${time}`;
    return `${dateFormat.format(date)} ${time}`;
}

export function formatFullDate(ms: number): string {
    if (!ms) return "";
    const hit = fullDateCache.get(ms);
    if (hit !== undefined) return hit;
    const formatted = fullDateFormat.format(new Date(ms));
    if (fullDateCache.size >= MAX_CACHE_SIZE) fullDateCache.clear();
    fullDateCache.set(ms, formatted);
    return formatted;
}
