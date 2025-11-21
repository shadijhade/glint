import { Command } from "@tauri-apps/plugin-shell";

export interface RgMatch {
    type: "match" | "context" | "begin" | "end" | "summary";
    data: {
        path?: { text: string };
        lines?: { text: string };
        line_number?: number;
        absolute_offset?: number;
        submatches?: Array<{ match: { text: string }; start: number; end: number }>;
        elapsed_total?: { human: string; nanos: number; seconds: number };
        stats?: {
            elapsed?: { human: string; nanos: number; seconds: number };
            searches?: number;
            searches_with_match?: number;
            bytes_searched?: number;
            bytes_printed?: number;
            matched_lines?: number;
            matches?: number;
        };
    };
}

export async function checkRgInstalled(): Promise<boolean> {
    try {
        const command = Command.sidecar("binaries/rg", ["--version"]);
        const output = await command.execute();
        return output.code === 0;
    } catch (e) {
        console.error("Failed to check rg version", e);
        return false;
    }
}

export interface SearchOptions {
    caseSensitive?: boolean;
    wholeWord?: boolean;
    regex?: boolean;
}

export async function search(
    query: string,
    path: string,
    onEvent: (events: RgMatch[]) => void,
    onFinished: () => void,
    options: SearchOptions = {}
) {
    const args = ["--json"];

    if (options.caseSensitive) {
        args.push("--case-sensitive");
    } else {
        args.push("--smart-case");
    }

    if (options.wholeWord) {
        args.push("--word-regexp");
    }

    if (!options.regex) {
        args.push("--fixed-strings");
    }

    args.push(query);
    args.push(path);

    const command = Command.sidecar("binaries/rg", args);

    let buffer: RgMatch[] = [];
    let lastEmit = Date.now();
    const BATCH_SIZE = 1000;
    const BATCH_INTERVAL = 100;

    const flush = () => {
        if (buffer.length > 0) {
            onEvent([...buffer]);
            buffer = [];
            lastEmit = Date.now();
        }
    };

    command.on("close", () => {
        flush();
        onFinished();
    });

    command.on("error", (error) => {
        console.error(`command error: "${error}"`);
        flush();
        onFinished();
    });

    command.stdout.on("data", (line) => {
        try {
            const lines = line.split("\n");
            for (const l of lines) {
                if (l.trim()) {
                    try {
                        const parsed = JSON.parse(l) as RgMatch;
                        buffer.push(parsed);

                        if (
                            buffer.length >= BATCH_SIZE ||
                            Date.now() - lastEmit >= BATCH_INTERVAL
                        ) {
                            flush();
                        }
                    } catch (e) {
                        // ignore parse errors
                    }
                }
            }
        } catch (e) {
            console.error("Failed to parse rg output", e);
        }
    });

    const child = await command.spawn();
    return child;
}
