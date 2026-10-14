const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

/** 1536 → "1.5 KB", 12345678 → "11.8 MB". */
export function formatSize(bytes: number) {
    if (bytes < 1024) return `${bytes} B`;
    let value = bytes;
    let unit = 0;
    while (value >= 1024 && unit < UNITS.length - 1) {
        value /= 1024;
        unit++;
    }
    return `${value < 10 ? value.toFixed(1) : Math.round(value).toLocaleString()} ${UNITS[unit]}`;
}

export const formatCount = (n: number) => n.toLocaleString();

/** 0.42 → "0.4 ms", 12.3 → "12 ms", 1234 → "1.23 s". */
export function formatDuration(ms: number) {
    if (ms < 10) return `${ms.toFixed(1)} ms`;
    if (ms < 1000) return `${Math.round(ms)} ms`;
    return `${(ms / 1000).toFixed(2)} s`;
}

const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const dateFormat = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric" });
const DAY = 86_400_000;

/** "Today 9:31 PM", "Yesterday 8:02 AM", otherwise "Oct 3, 2026 4:15 PM". */
export function formatDate(ms: number, now = Date.now()) {
    if (!ms) return "";
    const date = new Date(ms);
    const today = new Date(now);
    today.setHours(0, 0, 0, 0);
    const time = timeFormat.format(date);
    if (ms >= today.getTime() && ms < today.getTime() + DAY) return `Today ${time}`;
    if (ms >= today.getTime() - DAY && ms < today.getTime()) return `Yesterday ${time}`;
    return `${dateFormat.format(date)} ${time}`;
}

export const formatFullDate = (ms: number) => (ms ? new Date(ms).toLocaleString(undefined, { dateStyle: "full", timeStyle: "medium" }) : "");
