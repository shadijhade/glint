/** "333 71% 51%", as the color tokens in index.css are written, for #rrggbb. */
export function hslOf(hex: string) {
    const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
    const max = Math.max(r, g, b), min = Math.min(r, g, b), l = (max + min) / 2, d = max - min;
    const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
    const h = d === 0 ? 0 : max === r ? ((g - b) / d + 6) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
    return `${Math.round(h * 60)} ${Math.round(s * 100)}% ${Math.round(l * 100)}%`;
}

/** White or near-black text on #rrggbb, whichever reads better (WCAG contrast), as a color token. */
export function textOn(hex: string) {
    const lum = (c: number) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
    const [r, g, b] = [1, 3, 5].map((i) => lum(parseInt(hex.slice(i, i + 2), 16) / 255));
    const y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    // Near-black is #121212 (luminance 0.0063).
    return 1.05 / (y + 0.05) >= (y + 0.05) / 0.0563 ? "0 0% 100%" : "0 0% 7%";
}
