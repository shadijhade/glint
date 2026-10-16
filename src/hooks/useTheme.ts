import { useEffect } from 'react';
import { useStore, Settings } from '@/lib/store';

const FONT_PX: Record<Settings['fontSize'], number> = { small: 14, medium: 16, large: 18 };

/** Height of one results row; scales with the text size setting. */
export const rowHeight = (size: Settings['fontSize']) => Math.round(FONT_PX[size] * 1.75);

/** Pixels in a rem at the text size setting. */
export const remPx = (size: Settings['fontSize']) => FONT_PX[size];

/**
 * A grid tile, in CSS pixels: the narrowest it gets (tiles widen to fill the row), its height, the
 * square its thumbnail or icon fits in, and the grid's side padding. Scales with the text size.
 */
export function gridTile(size: Settings['fontSize']) {
    const rem = FONT_PX[size];
    return { width: Math.round(rem * 7.5), height: Math.round(rem * 10.25), thumb: rem * 6, pad: rem * 0.5 };
}

export type GridTile = ReturnType<typeof gridTile>;

/** Applies the theme (incl. live system changes) and text size to <html>. */
export function useTheme() {
    const { theme, fontSize } = useStore((s) => s.settings);

    useEffect(() => {
        const media = window.matchMedia('(prefers-color-scheme: dark)');
        const apply = () => document.documentElement.classList.toggle('dark', theme === 'dark' || (theme === 'system' && media.matches));
        apply();
        if (theme !== 'system') return;
        media.addEventListener('change', apply);
        return () => media.removeEventListener('change', apply);
    }, [theme]);

    useEffect(() => {
        document.documentElement.style.fontSize = `${FONT_PX[fontSize]}px`;
    }, [fontSize]);
}
