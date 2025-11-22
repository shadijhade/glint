import * as Dialog from '@radix-ui/react-dialog';
import { X, Settings as SettingsIcon, Plus } from 'lucide-react';
import { useStore } from '@/lib/store';
import { useState } from 'react';

export function Settings() {
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
        <Dialog.Root>
            <Dialog.Trigger asChild>
                <button
                    className="px-4 py-3 bg-zinc-200 hover:bg-zinc-300 border border-zinc-300 dark:bg-zinc-800 dark:hover:bg-zinc-700 dark:border-zinc-700 rounded-xl text-zinc-900 dark:text-zinc-100 transition-all"
                    title="Settings"
                >
                    <SettingsIcon className="w-5 h-5" />
                </button>
            </Dialog.Trigger>
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

                    <div className="grid gap-4 py-4">
                        <div className="grid grid-cols-4 items-center gap-4">
                            <label className="text-right text-sm font-medium text-zinc-700 dark:text-zinc-300 transition-colors">
                                Theme
                            </label>
                            <select
                                value={settings.theme}
                                onChange={(e) => setSettings({ theme: e.target.value as any })}
                                className="col-span-3 flex h-9 w-full rounded-md border border-zinc-300 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1 text-sm shadow-sm transition-colors text-zinc-900 dark:text-zinc-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-pink-500 cursor-pointer hover:bg-zinc-50 dark:hover:bg-zinc-800/80"
                                style={{
                                    backgroundImage: `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' fill='none' viewBox='0 0 20 20'%3E%3Cpath stroke='%236b7280' stroke-linecap='round' stroke-linejoin='round' stroke-width='1.5' d='M6 8l4 4 4-4'/%3E%3C/svg%3E")`,
                                    backgroundPosition: 'right 0.5rem center',
                                    backgroundRepeat: 'no-repeat',
                                    backgroundSize: '1.5em 1.5em',
                                    paddingRight: '2.5rem',
                                    appearance: 'none',
                                }}
                            >
                                <option value="dark" className="bg-white dark:bg-zinc-900 text-zinc-900 dark:text-zinc-100 py-2">Dark</option>
                                <option value="light" className="bg-white dark:bg-zinc-900 text-zinc-900 dark:text-zinc-100 py-2">Light</option>
                                <option value="system" className="bg-white dark:bg-zinc-900 text-zinc-900 dark:text-zinc-100 py-2">System</option>
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

                        <div className="grid grid-cols-4 items-start gap-4">
                            <label className="text-right text-sm font-medium text-zinc-700 dark:text-zinc-300 pt-2 transition-colors">
                                Exclusions
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

                    <Dialog.Close className="absolute right-4 top-4 rounded-sm opacity-70 ring-offset-background transition-opacity hover:opacity-100 focus:outline-none disabled:pointer-events-none data-[state=open]:bg-accent data-[state=open]:text-muted-foreground">
                        <X className="h-4 w-4 text-zinc-400" />
                        <span className="sr-only">Close</span>
                    </Dialog.Close>
                </Dialog.Content>
            </Dialog.Portal>
        </Dialog.Root>
    );
}
