import { create } from 'zustand'
import { persist } from 'zustand/middleware'

interface SearchHistoryItem {
    query: string;
    path: string;
    timestamp: number;
}

interface SearchOptions {
    caseSensitive: boolean;
    wholeWord: boolean;
    regex: boolean;
}

interface Settings {
    theme: 'dark' | 'light' | 'system';
    editorPath: string;
    exclusions: string[];
}

interface AppState {
    query: string;
    path: string;
    history: SearchHistoryItem[];
    options: SearchOptions;
    settings: Settings;
    setQuery: (query: string) => void;
    setPath: (path: string) => void;
    addToHistory: (query: string, path: string) => void;
    removeFromHistory: (query: string, path: string) => void;
    clearHistory: () => void;
    setOption: (key: keyof SearchOptions, value: boolean) => void;
    setSettings: (settings: Partial<Settings>) => void;
}

export const useStore = create<AppState>()(
    persist(
        (set) => ({
            query: '',
            path: '', // Default to empty, will prompt user or use current dir
            history: [],
            options: {
                caseSensitive: false,
                wholeWord: false,
                regex: false,
            },
            settings: {
                theme: 'dark',
                editorPath: '',
                exclusions: ['node_modules', '.git', 'dist', 'build'],
            },
            setQuery: (query) => set({ query }),
            setPath: (path) => set({ path }),
            addToHistory: (query, path) => set((state) => {
                const newHistory = [
                    { query, path, timestamp: Date.now() },
                    ...state.history.filter(h => h.query !== query || h.path !== path)
                ].slice(0, 1000); // Keep last 1000
                return { history: newHistory };
            }),
            removeFromHistory: (query: string, path: string) => set((state) => ({
                history: state.history.filter(h => h.query !== query || h.path !== path)
            })),
            clearHistory: () => set({ history: [] }),
            setOption: (key, value) => set((state) => ({
                options: { ...state.options, [key]: value }
            })),
            setSettings: (newSettings) => set((state) => ({
                settings: { ...state.settings, ...newSettings }
            })),
        }),
        {
            name: 'shadui-ripgrep-storage',
        }
    )
)
