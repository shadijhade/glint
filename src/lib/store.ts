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
    // Search defaults
    defaultSearchPath: string;
    maxResults: number | null;
    searchDelay: number; // ms debounce delay
    // Display preferences
    fontSize: 'small' | 'medium' | 'large';
    showLineNumbers: boolean;
    previewLines: number; // lines of context in preview
    // Behavior
    autoOpenPreview: boolean;
    confirmBeforeReplace: boolean;
    clearResultsOnNewSearch: boolean;
    // History
    maxHistoryItems: number;
    saveSearchHistory: boolean;
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
    // UI State
    isSearchFocused: boolean;
    setSearchFocused: (focused: boolean) => void;
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
                exclusions: [
                    // Folders
                    'node_modules', '.git', 'dist', 'build', 'bin', 'obj', '.vs', '.idea',
                    '__pycache__', '.cache', 'vendor', 'packages', '.nuget', 'coverage',
                    // Binary/Executable files
                    '*.exe', '*.dll', '*.so', '*.dylib', '*.pdb', '*.lib', '*.a', '*.o',
                    // Images
                    '*.png', '*.jpg', '*.jpeg', '*.gif', '*.ico', '*.svg', '*.webp', '*.bmp',
                    // Archives
                    '*.zip', '*.rar', '*.7z', '*.tar', '*.gz',
                    // Other binary formats
                    '*.pdf', '*.woff', '*.woff2', '*.ttf', '*.eot',
                    // Lock files
                    'package-lock.json', 'yarn.lock', 'pnpm-lock.yaml',
                ],
                // Search defaults
                defaultSearchPath: '',
                maxResults: 10000,
                searchDelay: 300,
                // Display preferences
                fontSize: 'medium',
                showLineNumbers: true,
                previewLines: 5,
                // Behavior
                autoOpenPreview: true,
                confirmBeforeReplace: true,
                clearResultsOnNewSearch: true,
                // History
                maxHistoryItems: 1000,
                saveSearchHistory: true,
            },
            setQuery: (query) => set({ query }),
            setPath: (path) => set({ path }),
            addToHistory: (query, path) => set((state) => {
                // Check if history saving is enabled
                if (!state.settings.saveSearchHistory) {
                    return {}; // Don't save to history
                }
                const newHistory = [
                    { query, path, timestamp: Date.now() },
                    ...state.history.filter(h => h.query !== query || h.path !== path)
                ].slice(0, state.settings.maxHistoryItems); // Use setting for max items
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
            isSearchFocused: false,
            setSearchFocused: (focused) => set({ isSearchFocused: focused }),
        }),
        {
            name: 'shadui-ripgrep-storage',
        }
    )
)
