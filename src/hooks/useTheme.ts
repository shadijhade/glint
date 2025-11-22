import { useEffect } from 'react';
import { useStore } from '@/lib/store';

type Theme = 'light' | 'dark' | 'system';

export function useTheme() {
    const { settings, setSettings } = useStore();
    const theme = settings.theme as Theme;

    useEffect(() => {
        const root = window.document.documentElement;

        // Remove existing theme classes
        root.classList.remove('light', 'dark');

        // Determine the effective theme
        let effectiveTheme: 'light' | 'dark';

        if (theme === 'system') {
            // Check system preference
            const systemTheme = window.matchMedia('(prefers-color-scheme: dark)').matches
                ? 'dark'
                : 'light';
            effectiveTheme = systemTheme;
        } else {
            effectiveTheme = theme;
        }

        // Apply theme class
        root.classList.add(effectiveTheme);
    }, [theme]);

    // Listen for system theme changes when in system mode
    useEffect(() => {
        if (theme !== 'system') return;

        const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');

        const handleChange = (e: MediaQueryListEvent) => {
            const root = window.document.documentElement;
            root.classList.remove('light', 'dark');
            root.classList.add(e.matches ? 'dark' : 'light');
        };

        mediaQuery.addEventListener('change', handleChange);
        return () => mediaQuery.removeEventListener('change', handleChange);
    }, [theme]);

    const setTheme = (newTheme: Theme) => {
        setSettings({ theme: newTheme });
    };

    return { theme, setTheme };
}
