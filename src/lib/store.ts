import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { toast } from 'sonner'
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
    /** A code pattern, by ast-grep, rather than text. */
    code: boolean;
    /** The text inside documents (PDF, Office, OpenDocument, EPUB) rather than files' own (doctext.rs). */
    documents: boolean;
    /** With documents: pictures' text too, read by Windows' OCR (ocr.rs). */
    ocr: boolean;
    /** Documents' and notes' passages ranked by meaning (meaning.rs) rather than matched. */
    meaning: boolean;
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
    /** Windows 11's Mica behind the title bar and the sidebar, or a solid window. */
    backdrop: 'mica' | 'solid';
    /** Glint's pink, or the accent color chosen in Windows. */
    accent: 'glint' | 'windows';
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
    // The sidebar's folder tree: drives, Quick access and pinned folders open to show their folders, and
    // (if asked) it opens down to the folder shown, as File Explorer's "Expand to open folder"
    folderTree: boolean;
    treeFollows: boolean;
    listThumbnails: boolean; // small thumbnails of pictures and videos in the list's icon column
    tileSize: TileSize; // how big the grid's tiles are, in every tab (Ctrl+wheel, Ctrl+= and Ctrl+-)
    confirmDelete: boolean; // ask before moving to the Recycle Bin (permanent deletes always ask)
    sort: Sort;
    // Find files (fd)
    findRespectIgnore: boolean;
    findMaxResults: number | null;
    findExclusions: string[];
    // Quick search: a shortcut from anywhere (null: off), the tray, and starting with Windows
    quickShortcut: string | null;
    keepInTray: boolean;
    startWithWindows: boolean;
    // Compare panes: what's inside subfolders too, and files' contents
    compareSubfolders: boolean;
    compareContents: boolean;
    // Widths dragged in the list's header and at the preview pane's edge, in pixels (unset: the defaults)
    columns: Partial<Record<Column, number>>;
    previewWidth: number | null;
    // Git repositories (their roots) whose own settings name programs for status to run (filter
    // drivers), trusted to run them
    gitTrusted: string[];
    // The terminal pane: its shell ("" picks the first found: PowerShell 7, Windows PowerShell, cmd),
    // its height in pixels (null: the default), and whether it and the folder shown follow each other
    terminalShell: string;
    terminalHeight: number | null;
    terminalFollow: boolean;
    // The disk-usage map's Growth colors: what grew in the last this many days (1, 7 or 30)
    growthDays: number;
    // The YARA rules files are checked with: a .yar file, or a folder of them ("" until chosen)
    yaraRules: string;
    // A folder's README.md above its list, and whether it's open (or folded to its title)
    showReadme: boolean;
    readmeOpen: boolean;
    // Folder history shared with zoxide, when it's installed: its folders rank in Ctrl+K and quick
    // search, and the folders opened here go to it
    zoxide: boolean;
}

/** The list's columns whose width can be dragged (the name takes what's left). */
export type Column = 'date' | 'type' | 'size'

export const DEFAULT_SETTINGS: Settings = {
    theme: 'system',
    backdrop: 'mica',
    accent: 'glint',
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
    folderTree: true,
    treeFollows: false,
    listThumbnails: true,
    tileSize: 'medium',
    confirmDelete: false,
    sort: { key: 'name', desc: false },
    findRespectIgnore: false,
    findMaxResults: 50000,
    findExclusions: ['node_modules', '.git'],
    quickShortcut: 'Ctrl+Alt+F',
    keepInTray: false,
    startWithWindows: false,
    compareSubfolders: false,
    compareContents: false,
    columns: {},
    previewWidth: null,
    gitTrusted: [],
    terminalShell: '',
    terminalHeight: null,
    terminalFollow: true,
    growthDays: 7,
    yaraRules: '',
    showReadme: true,
    readmeOpen: false,
    zoxide: false,
};

/**
 * A remote connection (remote/), as its folders are reached at `remote:<id>`: no secret, which Windows
 * Credential Manager keeps. `id` is made from the first name it's given, and kept through renames.
 */
export interface RemoteConnection {
    id: string;
    name: string;
    kind: "s3" | "webdav" | "sftp" | "rclone";
    /** S3: the service's address (Amazon's, from the region, when empty); WebDAV: the server's URL. */
    endpoint: string;
    region: string;
    bucket: string;
    /** S3's access key ID; WebDAV's and SFTP's user name. */
    user: string;
    /** SFTP: the server (a name ~/.ssh/config knows will do), its port (22 when 0), and a key file ("" for ssh's own choice). */
    host?: string;
    port?: number;
    key?: string;
    /** The folder on it to start at. */
    root: string;
    /** Pictures in the grid get thumbnails, which downloads each one shown (up to 16 MB): off unless asked for. */
    thumbnails?: boolean;
    /** rclone: the remote's name in rclone's configuration, and its type there ("drive", "onedrive"…). */
    rclone?: string;
    service?: string;
}

/**
 * A folder rule (rules.rs): what it looks at (a folder, its subfolders if asked), what it takes (names,
 * sizes, age), and what it does. Off until it's turned on, after a look at what it would do.
 */
export interface FolderRule {
    id: string;
    name: string;
    on: boolean;
    folder: string;
    subfolders: boolean;
    /** Patterns with * and ?, separated by ";" or "," ("*.pdf; invoice*"): empty for any name. */
    names: string;
    /** Bytes (0: no bound). */
    minSize: number;
    maxSize: number;
    /** Last changed at least this many days ago (0: any). */
    olderThanDays: number;
    action: "move" | "rename" | "recycle" | "unblock" | "extract" | "sync";
    /** Move: where to ({year}, {month}, {day} of the file's date may be in it). Sync: the place on a remote of rclone's ("gdrive:Backups"). */
    target: string;
    /** Rename: the new name, of {name}, {ext}, {date}, {time}, {year}, {month}, {day}. */
    pattern: string;
    /** Sync: "copy" (what's new or changed here goes there), "mirror" (there becomes what's here) or "both" (both ways). */
    sync?: "copy" | "mirror" | "both";
    /** Sync: also every this many minutes (0: when files change, and at start). */
    every?: number;
}

/** A name search kept in the sidebar: what was typed, and the folder it searches (THIS_PC: every drive). */
export interface SavedSearch {
    query: string;
    folder: string;
}

interface AppState {
    query: string;
    path: string;
    history: HistoryItem[];
    options: SearchOptions;
    settings: Settings;
    mode: Mode;
    pinned: string[];
    savedSearches: SavedSearch[];
    remotes: RemoteConnection[];
    rules: FolderRule[];
    lastFolder: string | null;
    session: Session | null;
    /** Folders opened, for ranking in the command palette. */
    visits: Record<string, Visit>;
    setQuery: (query: string) => void;
    setPath: (path: string) => void;
    addToHistory: (query: string, path: string) => void;
    removeFromHistory: (query: string, path: string) => void;
    setOption: (key: keyof SearchOptions, value: boolean) => void;
    setSettings: (settings: Partial<Settings>) => void;
    setMode: (mode: Mode) => void;
    togglePin: (path: string) => void;
    /** Keeps the search in the sidebar, or takes it out if it's there. */
    toggleSavedSearch: (search: SavedSearch) => void;
    /** Adds a connection, or changes the one with its id. */
    saveRemote: (remote: RemoteConnection) => void;
    removeRemote: (id: string) => void;
    /** Adds a rule, or changes the one with its id. */
    saveRule: (rule: FolderRule) => void;
    removeRule: (id: string) => void;
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
            options: { caseSensitive: false, wholeWord: false, regex: false, code: false, documents: false, ocr: false, meaning: false },
            settings: DEFAULT_SETTINGS,
            mode: 'files',
            pinned: [],
            savedSearches: [],
            remotes: [],
            rules: [],
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
            setOption: (key, value) => set((state) => ({
                // Code patterns and documents are searches of their own: turning one on turns the other off.
                // Meaning is a documents search, so it turns documents on, and off with them.
                options: {
                    ...state.options,
                    [key]: value,
                    ...(value && key === 'code' && { documents: false, meaning: false }),
                    ...(value && key === 'documents' && { code: false }),
                    ...(value && key === 'meaning' && { documents: true, code: false }),
                    ...(!value && key === 'documents' && { meaning: false }),
                }
            })),
            setSettings: (newSettings) => set((state) => ({
                settings: { ...state.settings, ...newSettings }
            })),
            setMode: (mode) => set({ mode }),
            togglePin: (path) => set((state) => ({
                pinned: state.pinned.includes(path) ? state.pinned.filter(p => p !== path) : [...state.pinned, path]
            })),
            toggleSavedSearch: (search) => set((state) => {
                const same = (s: SavedSearch) => s.query === search.query && s.folder.toLowerCase() === search.folder.toLowerCase();
                return { savedSearches: state.savedSearches.some(same) ? state.savedSearches.filter((s) => !same(s)) : [...state.savedSearches, search] };
            }),
            saveRemote: (remote) => set((state) => ({
                remotes: state.remotes.some((r) => r.id === remote.id) ? state.remotes.map((r) => (r.id === remote.id ? remote : r)) : [...state.remotes, remote],
            })),
            removeRemote: (id) => set((state) => ({ remotes: state.remotes.filter((r) => r.id !== id) })),
            saveRule: (rule) => set((state) => ({
                rules: state.rules.some((r) => r.id === rule.id) ? state.rules.map((r) => (r.id === rule.id ? rule : r)) : [...state.rules, rule],
            })),
            removeRule: (id) => set((state) => ({ rules: state.rules.filter((r) => r.id !== id) })),
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

/** Clears the searches kept for Home, or the folders the command palette ranks, with Undo. */
export function clearWithUndo(what: 'history' | 'visits') {
    const before = useStore.getState()[what]
    useStore.setState(what === 'history' ? { history: [] } : { visits: {} })
    toast.success(what === 'history' ? 'Cleared the search history' : 'Cleared the folders remembered', {
        action: { label: 'Undo', onClick: () => useStore.setState({ [what]: before }) },
    })
}
