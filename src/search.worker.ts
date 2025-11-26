import { RgMatch } from "./lib/ripgrep";
import { DisplayItem } from "./types";

let totalEventsProcessed = 0;
let lastFile: string | null = null;
let buffer: RgMatch[] = [];
let displayItemsBuffer: DisplayItem[] = [];

// Batch updates to main thread to avoid flooding messaging
const BATCH_SIZE = 500;
let batchTimeout: number | null = null;

const flush = () => {
    if (buffer.length > 0 || displayItemsBuffer.length > 0) {
        self.postMessage({
            type: 'results',
            matches: buffer,
            displayItems: displayItemsBuffer
        });
        buffer = [];
        displayItemsBuffer = [];
    }
    batchTimeout = null;
};

self.onmessage = (e: MessageEvent) => {
    const { type, payload } = e.data;

    if (type === 'reset') {
        totalEventsProcessed = 0;
        lastFile = null;
        buffer = [];
        displayItemsBuffer = [];
        if (batchTimeout) {
            clearTimeout(batchTimeout);
            batchTimeout = null;
        }
    } else if (type === 'data') {
        // payload is string[] from Rust batch
        const lines = payload as string[];
        for (const l of lines) {
            if (l.trim()) {
                try {
                    const parsed = JSON.parse(l) as RgMatch;

                    // We only care about matches for the display items list
                    // But we might want to pass all events (like summary) back to main thread if needed
                    // For now, let's pass everything to results buffer, but only process matches for display

                    buffer.push(parsed);
                    const currentIndex = totalEventsProcessed;
                    totalEventsProcessed++;

                    if (parsed.type === 'match') {
                        const file = parsed.data.path?.text || "";
                        if (file !== lastFile) {
                            displayItemsBuffer.push({ type: 'header', file, matchCount: -1 });
                            lastFile = file;
                        }
                        displayItemsBuffer.push({ type: 'match', match: parsed, originalIndex: currentIndex });
                    }
                } catch (e) {
                    // ignore parse errors
                }
            }
        }

        // Schedule flush
        if (!batchTimeout) {
            if (buffer.length >= BATCH_SIZE) {
                flush();
            } else {
                batchTimeout = setTimeout(flush, 50) as unknown as number;
            }
        } else if (buffer.length >= BATCH_SIZE) {
            clearTimeout(batchTimeout);
            flush();
        }
    } else if (type === 'flush') {
        flush();
    }
};
