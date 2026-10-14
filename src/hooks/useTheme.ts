import { useEffect } from 'react';
import { useStore, Settings } from '@/lib/store';

const FONT_PX: Record<Settings['fontSize'], number> = { small: 14, medium: 16, large: 18 };

/** Height of one results row; scales with the text size setting. */
export const rowHeight = (size: Settings['fontSize']) => Math.round(FONT_PX[size] * 1.75);

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
