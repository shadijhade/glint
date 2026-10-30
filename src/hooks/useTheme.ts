import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { hslOf, textOn } from '@/lib/color';
import { tileLayout } from '@/lib/grid';
import { useStore, type Settings } from '@/lib/store';

const FONT_PX: Record<Settings['fontSize'], number> = { small: 14, medium: 16, large: 18 };

/** Height of one results row; scales with the text size setting. */
export const rowHeight = (size: Settings['fontSize']) => Math.round(FONT_PX[size] * 1.75);

/** Pixels in a rem at the text size setting. */
export const remPx = (size: Settings['fontSize']) => FONT_PX[size];

/** A grid tile at the tile size setting, in CSS pixels (see `tileLayout`); scales with the text size too. */
export const gridTile = (size: Settings['fontSize'], tiles: Settings['tileSize']) => tileLayout(FONT_PX[size], tiles);

interface Accent {
    accent: string;
    light: [string, string, string];
    dark: [string, string, string];
}

/** Applies the theme (incl. live system changes), the accent color and text size to <html>. */
export function useTheme() {
    const { theme, fontSize, accent } = useStore((s) => s.settings);

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

    // Windows' accent (Settings → Appearance → Accent color): Windows 11 fills with its darker first
    // shade in light mode and its lighter second one in dark mode. Read again whenever Glint comes back
    // to the front, as it may have been changed in Windows' settings meanwhile.
    useEffect(() => {
        const root = document.documentElement;
        if (accent !== 'windows') return root.removeAttribute('data-accent');
        let live = true;
        const setAccentVars = (a: Accent) => {
            if (!live) return;
            for (const [mode, hex] of [['light', a.dark[0]], ['dark', a.light[1]]]) {
                root.style.setProperty(`--windows-accent-${mode}`, hslOf(hex));
                root.style.setProperty(`--windows-accent-${mode}-text`, textOn(hex));
            }
            root.setAttribute('data-accent', 'windows');
        };
        const apply = () =>
            invoke<Accent>('accent_colors').then(
                setAccentVars,
                () => root.removeAttribute('data-accent')
            );
        apply();
        window.addEventListener('focus', apply);
        const unlisten = listen<Accent>('windows-accent-changed', (ev) => setAccentVars(ev.payload));
        return () => {
            live = false;
            window.removeEventListener('focus', apply);
            void unlisten.then((stop) => stop());
        };
    }, [accent]);
}

/** The main window's frame and backdrop in Glint's theme: Mica Alt behind the title bar and the
 *  sidebar on Windows 11 (Settings → Appearance → Backdrop), which the page then lets show through. */
export function useBackdrop() {
    const { theme, backdrop } = useStore((s) => s.settings);
    useEffect(() => {
        let live = true;
        invoke<boolean>('appearance', { theme, backdrop: backdrop === 'mica' }).then(
            (on) => live && document.documentElement.toggleAttribute('data-backdrop', on),
            () => live && document.documentElement.removeAttribute('data-backdrop')
        );
        return () => {
            live = false;
        };
    }, [theme, backdrop]);
}
