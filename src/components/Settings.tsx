import * as Dialog from '@radix-ui/react-dialog';
import { X, Plus } from 'lucide-react';
import { useStore } from '@/lib/store';
import { useState } from 'react';

interface SettingsProps {
    isOpen: boolean;
    onOpenChange: (open: boolean) => void;
}

export function Settings({ isOpen, onOpenChange }: SettingsProps) {
    const { settings, setSettings } = useStore();
    const [newExclusion, setNewExclusion] = useState('');

    const handleAddExclusion = () => {
        if (newExclusion && !settings.exclusions.includes(newExclusion)) {
            setSettings({ exclusions: [...settings.exclusions, newExclusion] });
            setNewExclusion('');
        }
    };

    const handleRemoveExclusion = (ex: string) => {
        setSettings({ exclusions: settings.exclusions.filter(e => e !== ex) });
    };

    return (
        <Dialog.Root open={isOpen} onOpenChange={onOpenChange}>
            <Dialog.Portal>
                <Dialog.Overlay className="fixed inset-0 bg-black/50 backdrop-blur-sm data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 z-50" />
                <Dialog.Content className="fixed left-[50%] top-[50%] z-50 grid w-full max-w-lg translate-x-[-50%] translate-y-[-50%] gap-4 border border-zinc-300 dark:border-zinc-800 bg-white dark:bg-zinc-950 p-6 shadow-lg duration-200 data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[state=closed]:slide-out-to-left-1/2 data-[state=closed]:slide-out-to-top-[48%] data-[state=open]:slide-in-from-left-1/2 data-[state=open]:slide-in-from-top-[48%] sm:rounded-lg transition-colors">
                    <div className="flex flex-col space-y-1.5 text-center sm:text-left">
                        <Dialog.Title className="text-lg font-semibold leading-none tracking-tight text-zinc-900 dark:text-zinc-100 transition-colors">
                            Settings
                        </Dialog.Title>
                        <Dialog.Description className="text-sm text-zinc-600 dark:text-zinc-500 transition-colors">
                            Configure application preferences.
                        </Dialog.Description>
                    </div>

                    <div className="grid gap-6 py-4 max-h-[60vh] overflow-y-auto pr-2">
                        {/* Theme & Editor */}
                        <div className="space-y-4">
                            <h3 className="text-sm font-medium text-zinc-900 dark:text-zinc-100 border-b border-zinc-200 dark:border-zinc-800 pb-2">General</h3>
                            <div className="grid grid-cols-4 items-center gap-4">
                                <label className="text-right text-sm font-medium text-zinc-700 dark:text-zinc-300 transition-colors">
                                    Theme
                                </label>
                                <select
                                    value={settings.theme}
                                    onChange={(e) => setSettings({ theme: e.target.value as any })}
                                    className="col-span-3 flex h-9 w-full rounded-md border border-zinc-300 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1 text-sm shadow-sm transition-colors text-zinc-900 dark:text-zinc-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-pink-500 cursor-pointer hover:bg-zinc-50 dark:hover:bg-zinc-800/80"
                                >
                                    <option value="dark">Dark</option>
                                    <option value="light">Light</option>
                                    <option value="system">System</option>
                                </select>
                            </div>

                            <div className="grid grid-cols-4 items-center gap-4">
                                <label className="text-right text-sm font-medium text-zinc-700 dark:text-zinc-300 transition-colors">
                                    Editor Path
                                </label>
                                <input
                                    value={settings.editorPath}
                                    onChange={(e) => setSettings({ editorPath: e.target.value })}
                                    placeholder="code, notepad, etc. (Default: system default)"
                                    className="col-span-3 flex h-9 w-full rounded-md border border-zinc-300 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1 text-sm shadow-sm transition-colors text-zinc-900 dark:text-zinc-100 placeholder:text-zinc-400 dark:placeholder:text-zinc-500 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-pink-500"
                                />
                            </div>
                        </div>

                        {/* Search Defaults */}
                        <div className="space-y-4">
                            <h3 className="text-sm font-medium text-zinc-900 dark:text-zinc-100 border-b border-zinc-200 dark:border-zinc-800 pb-2">Search Defaults</h3>
                            <div className="grid grid-cols-4 items-center gap-4">
                                <label className="text-right text-sm font-medium text-zinc-700 dark:text-zinc-300 transition-colors">
                                    Default Path
                                </label>
                                <input
                                    value={settings.defaultSearchPath}
                                    onChange={(e) => setSettings({ defaultSearchPath: e.target.value })}
                                    placeholder="Default search directory"
                                    className="col-span-3 flex h-9 w-full rounded-md border border-zinc-300 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1 text-sm shadow-sm transition-colors text-zinc-900 dark:text-zinc-100 placeholder:text-zinc-400 dark:placeholder:text-zinc-500 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-pink-500"
                                />
                            </div>
                            <div className="grid grid-cols-4 items-center gap-4">
                                <label className="text-right text-sm font-medium text-zinc-700 dark:text-zinc-300 transition-colors">
                                    Max Results
                                </label>
                                <input
                                    type="number"
                                    value={settings.maxResults === null ? '' : settings.maxResults}
                                    onChange={(e) => {
                                        const val = e.target.value;
                                        setSettings({ maxResults: val === '' ? null : parseInt(val) });
                                    }}
                                    placeholder="No limit"
                                    className="col-span-3 flex h-9 w-full rounded-md border border-zinc-300 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1 text-sm shadow-sm transition-colors text-zinc-900 dark:text-zinc-100 placeholder:text-zinc-400 dark:placeholder:text-zinc-500 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-pink-500"
                                />
                            </div>
                            <div className="grid grid-cols-4 items-center gap-4">
                                <label className="text-right text-sm font-medium text-zinc-700 dark:text-zinc-300 transition-colors">
                                    Debounce (ms)
                                </label>
                                <input
                                    type="number"
                                    value={settings.searchDelay}
                                    onChange={(e) => setSettings({ searchDelay: parseInt(e.target.value) || 300 })}
                                    className="col-span-3 flex h-9 w-full rounded-md border border-zinc-300 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1 text-sm shadow-sm transition-colors text-zinc-900 dark:text-zinc-100 placeholder:text-zinc-400 dark:placeholder:text-zinc-500 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-pink-500"
                                />
                            </div>
                        </div>

                        {/* Display Preferences */}
                        <div className="space-y-4">
                            <h3 className="text-sm font-medium text-zinc-900 dark:text-zinc-100 border-b border-zinc-200 dark:border-zinc-800 pb-2">Display</h3>
                            <div className="grid grid-cols-4 items-center gap-4">
                                <label className="text-right text-sm font-medium text-zinc-700 dark:text-zinc-300 transition-colors">
                                    Font Size
                                </label>
                                <select
                                    value={settings.fontSize}
                                    onChange={(e) => setSettings({ fontSize: e.target.value as any })}
                                    className="col-span-3 flex h-9 w-full rounded-md border border-zinc-300 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1 text-sm shadow-sm transition-colors text-zinc-900 dark:text-zinc-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-pink-500 cursor-pointer hover:bg-zinc-50 dark:hover:bg-zinc-800/80"
                                >
                                    <option value="small">Small</option>
                                    <option value="medium">Medium</option>
                                    <option value="large">Large</option>
                                </select>
                            </div>
                            <div className="grid grid-cols-4 items-center gap-4">
                                <label className="text-right text-sm font-medium text-zinc-700 dark:text-zinc-300 transition-colors">
                                    Preview Lines
                                </label>
                                <input
                                    type="number"
                                    value={settings.previewLines}
                                    onChange={(e) => setSettings({ previewLines: parseInt(e.target.value) || 5 })}
                                    className="col-span-3 flex h-9 w-full rounded-md border border-zinc-300 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1 text-sm shadow-sm transition-colors text-zinc-900 dark:text-zinc-100 placeholder:text-zinc-400 dark:placeholder:text-zinc-500 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-pink-500"
                                />
                            </div>
                            <div className="grid grid-cols-4 items-center gap-4">
                                <div className="col-start-2 col-span-3 flex items-center space-x-2">
                                    <input
                                        type="checkbox"
                                        id="showLineNumbers"
                                        checked={settings.showLineNumbers}
                                        onChange={(e) => setSettings({ showLineNumbers: e.target.checked })}
                                        className="rounded border-zinc-300 text-pink-500 focus:ring-pink-500"
                                    />
                                    <label htmlFor="showLineNumbers" className="text-sm font-medium text-zinc-700 dark:text-zinc-300">
                                        Show Line Numbers
                                    </label>
                                </div>
                            </div>
                        </div>

                        {/* Behavior */}
                        <div className="space-y-4">
                            <h3 className="text-sm font-medium text-zinc-900 dark:text-zinc-100 border-b border-zinc-200 dark:border-zinc-800 pb-2">Behavior</h3>
                            <div className="grid grid-cols-4 items-center gap-4">
                                <div className="col-start-2 col-span-3 space-y-2">
                                    <div className="flex items-center space-x-2">
                                        <input
                                            type="checkbox"
                                            id="autoOpenPreview"
                                            checked={settings.autoOpenPreview}
                                            onChange={(e) => setSettings({ autoOpenPreview: e.target.checked })}
                                            className="rounded border-zinc-300 text-pink-500 focus:ring-pink-500"
                                        />
                                        <label htmlFor="autoOpenPreview" className="text-sm font-medium text-zinc-700 dark:text-zinc-300">
                                            Auto Open Preview
                                        </label>
                                    </div>
                                    <div className="flex items-center space-x-2">
                                        <input
                                            type="checkbox"
                                            id="confirmBeforeReplace"
                                            checked={settings.confirmBeforeReplace}
                                            onChange={(e) => setSettings({ confirmBeforeReplace: e.target.checked })}
                                            className="rounded border-zinc-300 text-pink-500 focus:ring-pink-500"
                                        />
                                        <label htmlFor="confirmBeforeReplace" className="text-sm font-medium text-zinc-700 dark:text-zinc-300">
                                            Confirm Before Replace
                                        </label>
                                    </div>
                                    <div className="flex items-center space-x-2">
                                        <input
                                            type="checkbox"
                                            id="clearResultsOnNewSearch"
                                            checked={settings.clearResultsOnNewSearch}
                                            onChange={(e) => setSettings({ clearResultsOnNewSearch: e.target.checked })}
                                            className="rounded border-zinc-300 text-pink-500 focus:ring-pink-500"
                                        />
                                        <label htmlFor="clearResultsOnNewSearch" className="text-sm font-medium text-zinc-700 dark:text-zinc-300">
                                            Clear Results on New Search
                                        </label>
                                    </div>
                                </div>
                            </div>
                        </div>

                        {/* History */}
                        <div className="space-y-4">
                            <h3 className="text-sm font-medium text-zinc-900 dark:text-zinc-100 border-b border-zinc-200 dark:border-zinc-800 pb-2">History</h3>
                            <div className="grid grid-cols-4 items-center gap-4">
                                <label className="text-right text-sm font-medium text-zinc-700 dark:text-zinc-300 transition-colors">
                                    Max Items
                                </label>
                                <input
                                    type="number"
                                    value={settings.maxHistoryItems}
                                    onChange={(e) => setSettings({ maxHistoryItems: parseInt(e.target.value) || 100 })}
                                    className="col-span-3 flex h-9 w-full rounded-md border border-zinc-300 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1 text-sm shadow-sm transition-colors text-zinc-900 dark:text-zinc-100 placeholder:text-zinc-400 dark:placeholder:text-zinc-500 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-pink-500"
                                />
                            </div>
                            <div className="grid grid-cols-4 items-center gap-4">
                                <div className="col-start-2 col-span-3 flex items-center space-x-2">
                                    <input
                                        type="checkbox"
                                        id="saveSearchHistory"
                                        checked={settings.saveSearchHistory}
                                        onChange={(e) => setSettings({ saveSearchHistory: e.target.checked })}
                                        className="rounded border-zinc-300 text-pink-500 focus:ring-pink-500"
                                    />
                                    <label htmlFor="saveSearchHistory" className="text-sm font-medium text-zinc-700 dark:text-zinc-300">
                                        Save Search History
                                    </label>
                                </div>
                            </div>
                        </div>

                        {/* Exclusions */}
                        <div className="space-y-4">
                            <h3 className="text-sm font-medium text-zinc-900 dark:text-zinc-100 border-b border-zinc-200 dark:border-zinc-800 pb-2">Exclusions</h3>
                            <div className="grid grid-cols-4 items-start gap-4">
                                <label className="text-right text-sm font-medium text-zinc-700 dark:text-zinc-300 pt-2 transition-colors">
                                    Patterns
                                </label>
                                <div className="col-span-3 flex flex-col gap-2">
                                    <div className="flex gap-2">
                                        <input
                                            value={newExclusion}
                                            onChange={(e) => setNewExclusion(e.target.value)}
                                            placeholder="Add pattern (e.g. node_modules)"
                                            className="flex h-9 w-full rounded-md border border-zinc-300 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1 text-sm shadow-sm transition-colors text-zinc-900 dark:text-zinc-100 placeholder:text-zinc-400 dark:placeholder:text-zinc-500 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-pink-500"
                                            onKeyDown={(e) => e.key === 'Enter' && handleAddExclusion()}
                                        />
                                        <button
                                            onClick={handleAddExclusion}
                                            className="p-2 bg-zinc-200 hover:bg-zinc-300 dark:bg-zinc-800 dark:hover:bg-zinc-700 rounded-md text-zinc-700 dark:text-zinc-300 transition-colors"
                                        >
                                            <Plus className="w-4 h-4" />
                                        </button>
                                    </div>
                                    <div className="flex flex-wrap gap-2 mt-2 max-h-32 overflow-y-auto">
                                        {settings.exclusions.map((ex) => (
                                            <div key={ex} className="flex items-center gap-1 bg-zinc-100 dark:bg-zinc-800/50 border border-zinc-300 dark:border-zinc-800 rounded-full px-3 py-1 text-xs text-zinc-700 dark:text-zinc-300 transition-colors">
                                                {ex}
                                                <button onClick={() => handleRemoveExclusion(ex)} className="hover:text-red-400 dark:hover:text-red-400 hover:text-red-500 ml-1">
                                                    <X className="w-3 h-3" />
                                                </button>
                                            </div>
                                        ))}
                                    </div>
                                </div>
                            </div>
                        </div>
                    </div>

                    <Dialog.Close className="absolute right-4 top-4 rounded-sm opacity-70 ring-offset-background transition-opacity hover:opacity-100 focus:outline-none disabled:pointer-events-none data-[state=open]:bg-accent data-[state=open]:text-muted-foreground">
                        <X className="h-4 w-4 text-zinc-400" />
                        <span className="sr-only">Close</span>
                    </Dialog.Close>
                </Dialog.Content>
            </Dialog.Portal>
        </Dialog.Root>
    );
}
