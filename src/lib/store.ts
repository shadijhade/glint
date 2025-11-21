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

interface AppState {
    query: string;
    path: string;
    history: SearchHistoryItem[];
    theme: 'dark' | 'light' | 'system';
    options: SearchOptions;
    setQuery: (query: string) => void;
    setPath: (path: string) => void;
    addToHistory: (query: string, path: string) => void;
    setTheme: (theme: 'dark' | 'light' | 'system') => void;
    clearHistory: () => void;
    setOption: (key: keyof SearchOptions, value: boolean) => void;
}

export const useStore = create<AppState>()(
    persist(
        (set) => ({
            query: '',
            path: '', // Default to empty, will prompt user or use current dir
            history: [],
            theme: 'dark',
            options: {
                caseSensitive: false,
                wholeWord: false,
                regex: false,
            },
            setQuery: (query) => set({ query }),
            setPath: (path) => set({ path }),
            addToHistory: (query, path) => set((state) => {
                const newHistory = [
                    { query, path, timestamp: Date.now() },
                    ...state.history.filter(h => h.query !== query || h.path !== path)
                ].slice(0, 50); // Keep last 50
                return { history: newHistory };
            }),
            setTheme: (theme) => set({ theme }),
            clearHistory: () => set({ history: [] }),
            setOption: (key, value) => set((state) => ({
                options: { ...state.options, [key]: value }
            })),
        }),
        {
            name: 'shadui-ripgrep-storage',
        }
    )
)
