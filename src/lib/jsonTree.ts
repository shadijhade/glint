/** A number as written, where a JavaScript number would change it: a big integer, 1.50, 1e400. */
export class JsonNumber {
    readonly text: string;
    constructor(text: string) {
        this.text = text;
    }
    toString() {
        return this.text;
    }
    /** As written, in JSON.stringify. */
    toJSON() {
        return (JSON as unknown as { rawJSON(text: string): unknown }).rawJSON(this.text);
    }
}

export type JsonValue = null | boolean | number | string | JsonNumber | JsonValue[] | { [key: string]: JsonValue };

type Reviver = (this: unknown, key: string, value: unknown, context?: { source?: string }) => unknown;

/** JSON text as values, its numbers kept as written where JavaScript's would differ. */
export function parseJson(text: string): JsonValue {
    const exact: Reviver = (_key, value, context) => {
        if (typeof value !== "number" || !context?.source) return value;
        return String(value) === context.source ? value : new JsonNumber(context.source);
    };
    return JSON.parse(text, exact as Parameters<typeof JSON.parse>[1]) as JsonValue;
}

export const isContainer = (value: JsonValue): value is JsonValue[] | { [key: string]: JsonValue } =>
    value !== null && typeof value === "object" && !(value instanceof JsonNumber);

const KEYS = new WeakMap<object, string[]>();

/** An object's keys, worked out once. */
function keysOf(value: { [key: string]: JsonValue }) {
    let keys = KEYS.get(value);
    if (!keys) KEYS.set(value, (keys = Object.keys(value)));
    return keys;
}

export const sizeOf = (value: JsonValue) => (Array.isArray(value) ? value.length : isContainer(value) ? keysOf(value).length : 0);

/** A step in a jq path: `.name`, `.["two words"]` or `[3]`. */
export const jqStep = (key: string | number) => (typeof key === "number" ? `[${key}]` : /^[A-Za-z_][A-Za-z0-9_]*$/.test(key) ? `.${key}` : `.[${JSON.stringify(key)}]`);

export const jqPath = (parent: string, key: string | number) => {
    const step = jqStep(key);
    return parent === "." ? (step.startsWith(".") ? step : `.${step}`) : parent + step;
};

export interface TreeRow {
    /** Unique among the rows: the value's number, then its jq path in it. */
    id: string;
    /** Its jq path within its value (`.` for a value itself). */
    path: string;
    /** Which value it's in (a filter can give several). */
    root: number;
    depth: number;
    /** Its key or index; null for a value itself. */
    key: string | number | null;
    value: JsonValue;
    open: boolean;
    /** A row standing for the rest of a big object or array, under `id`'s parent: how many more there are. */
    more?: number;
}

/** How many of an open object's or array's children show at first, and with each "more". */
export const PAGE = 500;

/**
 * The rows to show: each value, and in each open object or array its children, a page or more at a
 * time (`open` holds how many show, by row id).
 */
export function treeRows(values: JsonValue[], open: Map<string, number>): TreeRow[] {
    const rows: TreeRow[] = [];
    const visit = (value: JsonValue, key: string | number | null, path: string, root: number, depth: number) => {
        const id = `${root}:${path}`;
        const shown = isContainer(value) ? open.get(id) : undefined;
        rows.push({ id, path, root, depth, key, value, open: shown !== undefined });
        if (shown === undefined || !isContainer(value)) return;
        const size = sizeOf(value);
        const count = Math.min(size, shown);
        if (Array.isArray(value)) for (let i = 0; i < count; i++) visit(value[i], i, jqPath(path, i), root, depth + 1);
        else {
            const keys = keysOf(value);
            for (let i = 0; i < count; i++) visit(value[keys[i]], keys[i], jqPath(path, keys[i]), root, depth + 1);
        }
        if (count < size) rows.push({ id: `${id}\u0000more`, path, root, depth: depth + 1, key: null, value, open: false, more: size - count });
    };
    values.forEach((value, i) => visit(value, null, ".", i, 0));
    return rows;
}

/** Which objects and arrays start open: the values themselves, and what's in a small one. */
export function initiallyOpen(values: JsonValue[]) {
    const open = new Map<string, number>();
    values.forEach((value, root) => {
        if (!isContainer(value)) return;
        open.set(`${root}:.`, PAGE);
        if (values.length > 1 || sizeOf(value) > 20) return;
        const entries: [string | number, JsonValue][] = Array.isArray(value) ? value.map((v, i) => [i, v]) : Object.entries(value);
        for (const [key, child] of entries) if (isContainer(child) && sizeOf(child) <= 50) open.set(`${root}:${jqPath(".", key)}`, PAGE);
    });
    return open;
}

/** A value in one line, for a row: strings quoted, objects and arrays as their first few items. */
export function preview(value: JsonValue, room = 80): string {
    if (value === null) return "null";
    if (typeof value === "string") return JSON.stringify(value.length > room ? value.slice(0, room) + "…" : value);
    if (!isContainer(value)) return String(value);
    const array = Array.isArray(value);
    const parts: string[] = [];
    let used = 2;
    const items: [string | null, JsonValue][] = array ? value.slice(0, 20).map((v) => [null, v]) : keysOf(value).slice(0, 20).map((k) => [k, value[k]]);
    for (const [key, item] of items) {
        const text = (key === null ? "" : `${/^[A-Za-z_][A-Za-z0-9_]*$/.test(key) ? key : JSON.stringify(key)}: `) + (isContainer(item) ? (Array.isArray(item) ? "[…]" : "{…}") : preview(item, 24));
        if (used + text.length > room) break;
        parts.push(text);
        used += text.length + 2;
    }
    const rest = sizeOf(value) > parts.length ? (parts.length ? ", …" : "…") : "";
    return array ? `[${parts.join(", ")}${rest}]` : `{${parts.join(", ")}${rest}}`;
}
