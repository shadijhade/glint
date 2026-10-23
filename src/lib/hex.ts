/** hexyl's classes of bytes, which the hex view colors: zero, ASCII text, whitespace, other ASCII, and the rest. */
export type ByteClass = "null" | "text" | "space" | "control" | "high";

export function byteClass(byte: number): ByteClass {
    if (byte === 0) return "null";
    if (byte === 0x20 || (byte >= 0x09 && byte <= 0x0d)) return "space";
    if (byte > 0x20 && byte < 0x7f) return "text";
    return byte < 0x80 ? "control" : "high";
}

/** A byte in the text column, as hexyl draws it: ⋄ for zero, _ for whitespace but a space, • for other ASCII, × above it. */
export function byteChar(byte: number): string {
    switch (byteClass(byte)) {
        case "null":
            return "⋄";
        case "space":
            return byte === 0x20 ? " " : "_";
        case "text":
            return String.fromCharCode(byte);
        case "control":
            return "•";
        default:
            return "×";
    }
}

export const hex = (n: number, width: number) => n.toString(16).toUpperCase().padStart(width, "0");

/** An offset typed to go to: 0x1F00, 1F00h, or 1F00 (hexadecimal, having a letter), or 8192 (decimal). */
export function parseOffset(text: string): number | null {
    const value = text.trim().replace(/[_\s]/g, "");
    const prefixed = /^0x([0-9a-f]+)$/i.exec(value) ?? /^([0-9a-f]+)h$/i.exec(value);
    if (prefixed) return parseInt(prefixed[1], 16);
    if (/^\d+$/.test(value)) return Number(value);
    if (/^[0-9a-f]+$/i.test(value)) return parseInt(value, 16);
    return null;
}
