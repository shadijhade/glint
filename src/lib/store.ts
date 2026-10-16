import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { Sort } from './entries'
import type { TileSize } from './grid'
import { frecency, type Visit } from './rank'

export interface HistoryItem {
    query: string;
    path: string;
    timestamp: number;
}

export interface SearchOptions {
    caseSensitive: boolean;
    wholeWord: boolean;
    regex: boolean;
}

export type Mode = 'files' | 'text';

/** How a tab shows a folder: a list with columns, or a grid of thumbnails. */
export type ViewMode = 'list' | 'grid';

/** The tabs open in each pane when the app closed, to reopen them. */
export interface Session {
    dual: boolean;
    activePane: number;
    /** Each tab's folder and view (`views` came later: missing means lists). */
    panes: { paths: string[]; active: number; views?: ViewMode[] }[];
}

export interface Settings {
    theme: 'dark' | 'light' | 'system';
    fontSize: 'small' | 'medium' | 'large';
    showLineNumbers: boolean;
    defaultSearchPath: string;
    maxResults: number | null;
    autoOpenPreview: boolean;
    confirmBeforeReplace: boolean;
    editorPath: string; // program used to open files; empty = system default
    exclusions: string[];
    saveSearchHistory: boolean;
    maxHistoryItems: number;
    // Files
    startFolder: 'home' | 'thispc' | 'last';
    restoreTabs: boolean; // reopen last time's tabs and panes; startFolder when there are none
    showHidden: boolean;
    foldersFirst: boolean;
    showPreviewPane: boolean;
    listThumbnails: boolean; // small thumbnails of pictures and videos in the list's icon column
    tileSize: TileSize; // how big the grid's tiles are, in every tab (Ctrl+wheel, Ctrl+= and Ctrl+-)
    confirmDelete: boolean; // ask before moving to the Recycle Bin (permanent deletes always ask)
    sort: Sort;
    // Find files (fd)
    findRespectIgnore: boolean;
    findMaxResults: number | null;
    findExclusions: string[];
}

export const DEFAULT_SETTINGS: Settings = {
    theme: 'dark',
    fontSize: 'medium',
    showLineNumbers: true,
    defaultSearchPath: '',
    maxResults: 10000,
    autoOpenPreview: true,
    confirmBeforeReplace: true,
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
    saveSearchHistory: true,
    maxHistoryItems: 1000,
    startFolder: 'home',
    restoreTabs: true,
    showHidden: false,
    foldersFirst: true,
    showPreviewPane: true,
    listThumbnails: true,
    tileSize: 'medium',
    confirmDelete: false,
    sort: { key: 'name', desc: false },
    findRespectIgnore: false,
    findMaxResults: 50000,
    findExclusions: ['node_modules', '.git'],
};

interface AppState {
    query: string;
    path: string;
    history: HistoryItem[];
    options: SearchOptions;
    settings: Settings;
    mode: Mode;
    pinned: string[];
    lastFolder: string | null;
    session: Session | null;
    /** Folders opened, for ranking in the command palette. */
    visits: Record<string, Visit>;
    setQuery: (query: string) => void;
    setPath: (path: string) => void;
    addToHistory: (query: string, path: string) => void;
    removeFromHistory: (query: string, path: string) => void;
    clearHistory: () => void;
    setOption: (key: keyof SearchOptions, value: boolean) => void;
    setSettings: (settings: Partial<Settings>) => void;
    setMode: (mode: Mode) => void;
    togglePin: (path: string) => void;
    setLastFolder: (path: string) => void;
    setSession: (session: Session) => void;
    recordVisit: (path: string) => void;
}

const MAX_VISITS = 300;

export const useStore = create<AppState>()(
    persist(
        (set) => ({
            query: '',
            path: '',
            history: [],
            options: { caseSensitive: false, wholeWord: false, regex: false },
            settings: DEFAULT_SETTINGS,
            mode: 'files',
            pinned: [],
            lastFolder: null,
            session: null,
            visits: {},
            setQuery: (query) => set({ query }),
            setPath: (path) => set({ path }),
            addToHistory: (query, path) => set((state) => {
                if (!state.settings.saveSearchHistory) return {};
                const history = [
                    { query, path, timestamp: Date.now() },
                    ...state.history.filter(h => h.query !== query || h.path !== path),
                ].slice(0, Math.max(10, state.settings.maxHistoryItems || 0)); // never let a bad value wipe history
                return { history };
            }),
            removeFromHistory: (query, path) => set((state) => ({
                history: state.history.filter(h => h.query !== query || h.path !== path)
            })),
            clearHistory: () => set({ history: [] }),
            setOption: (key, value) => set((state) => ({
                options: { ...state.options, [key]: value }
            })),
            setSettings: (newSettings) => set((state) => ({
                settings: { ...state.settings, ...newSettings }
            })),
            setMode: (mode) => set({ mode }),
            togglePin: (path) => set((state) => ({
                pinned: state.pinned.includes(path) ? state.pinned.filter(p => p !== path) : [...state.pinned, path]
            })),
            setLastFolder: (lastFolder) => set({ lastFolder }),
            setSession: (session) => set({ session }),
            recordVisit: (path) => set((state) => {
                const now = Date.now();
                const visits = { ...state.visits, [path]: { count: (state.visits[path]?.count ?? 0) + 1, last: now } };
                const paths = Object.keys(visits);
                // Forget the least used folders once there are many, as zoxide ages its database.
                if (paths.length > MAX_VISITS) {
                    paths.sort((a, b) => frecency(visits[a], now) - frecency(visits[b], now));
                    for (const old of paths.slice(0, paths.length - MAX_VISITS)) delete visits[old];
                }
                return { visits };
            }),
        }),
        {
            name: 'shadui-ripgrep-storage',
            // Deep-merge settings so options added in newer versions get their defaults.
            merge: (persisted, current) => {
                const p = (persisted ?? {}) as Partial<AppState>;
                return { ...current, ...p, settings: { ...current.settings, ...p.settings } };
            },
        }
    )
)
