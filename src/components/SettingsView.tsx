import { Plus, X, RotateCcw, Monitor, Sun, Moon, Eye, Search, FileText, History, Info, ExternalLink, Zap, LucideIcon } from 'lucide-react';
import { useStore } from '@/lib/store';
import { useState, memo, useCallback } from 'react';
import { motion } from 'framer-motion';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Switch } from '@/components/ui/switch';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

// Memoized Section component - defined outside to prevent re-creation
const Section = memo(({ title, icon: Icon, children }: { title: string; icon: LucideIcon; children: React.ReactNode }) => (
    <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        className="bg-white/60 dark:bg-zinc-900/60 backdrop-blur-xl rounded-2xl border border-zinc-200 dark:border-zinc-800 p-6 shadow-sm"
    >
        <div className="flex items-center gap-3 mb-6">
            <div className="p-2 rounded-xl bg-gradient-to-br from-pink-500/10 to-purple-500/10 border border-pink-500/20">
                <Icon className="w-5 h-5 text-pink-500" />
            </div>
            <h2 className="text-lg font-semibold text-zinc-900 dark:text-white">{title}</h2>
        </div>
        <div className="space-y-4">{children}</div>
    </motion.div>
));
Section.displayName = 'Section';

// Memoized SettingRow component
const SettingRow = memo(({ label, description, children }: { label: string; description?: string; children: React.ReactNode }) => (
    <div className="flex items-center justify-between gap-4 py-2">
        <div className="flex-1 min-w-0">
            <p className="text-sm font-medium text-zinc-900 dark:text-zinc-100">{label}</p>
            {description && <p className="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">{description}</p>}
        </div>
        <div className="shrink-0">{children}</div>
    </div>
));
SettingRow.displayName = 'SettingRow';

export function SettingsView() {
    const settings = useStore((state) => state.settings);
    const setSettings = useStore((state) => state.setSettings);
    const history = useStore((state) => state.history);
    const clearHistory = useStore((state) => state.clearHistory);
    const [newExclusion, setNewExclusion] = useState('');

    const handleAddExclusion = useCallback(() => {
        if (newExclusion && !settings.exclusions.includes(newExclusion)) {
            setSettings({ exclusions: [...settings.exclusions, newExclusion] });
            setNewExclusion('');
        }
    }, [newExclusion, settings.exclusions, setSettings]);

    const handleRemoveExclusion = useCallback((ex: string) => {
        setSettings({ exclusions: settings.exclusions.filter(e => e !== ex) });
    }, [settings.exclusions, setSettings]);

    const handleResetSettings = useCallback(() => {
        setSettings({
            theme: 'dark',
            editorPath: '',
            exclusions: ['node_modules', '.git', 'dist', 'build'],
            defaultSearchPath: '',
            maxResults: 10000,
            searchDelay: 300,
            fontSize: 'medium',
            showLineNumbers: true,
            previewLines: 5,
            autoOpenPreview: true,
            confirmBeforeReplace: true,
            clearResultsOnNewSearch: true,
            maxHistoryItems: 1000,
            saveSearchHistory: true,
        });
    }, [setSettings]);

    return (
        <div className="flex-1 overflow-auto p-6 scrollbar-thin">
            <div className="max-w-3xl mx-auto space-y-6">
                {/* Header */}
                <motion.div
                    initial={{ opacity: 0, y: -20 }}
                    animate={{ opacity: 1, y: 0 }}
                    className="mb-8"
                >
                    <h1 className="text-3xl font-bold text-zinc-900 dark:text-white mb-2">Settings</h1>
                    <p className="text-zinc-500 dark:text-zinc-400">Configure your ShadUI Ripgrep experience</p>
                </motion.div>

                {/* Appearance Section */}
                <Section title="Appearance" icon={Monitor}>
                    <SettingRow label="Theme" description="Choose your preferred color scheme">
                        <div className="flex items-center gap-2">
                            {[
                                { value: 'light', icon: Sun, label: 'Light' },
                                { value: 'dark', icon: Moon, label: 'Dark' },
                                { value: 'system', icon: Monitor, label: 'System' },
                            ].map(({ value, icon: ThemeIcon, label }) => (
                                <Button
                                    key={value}
                                    variant={settings.theme === value ? "default" : "outline"}
                                    size="sm"
                                    onClick={() => setSettings({ theme: value as 'light' | 'dark' | 'system' })}
                                    className={cn(
                                        "gap-2",
                                        settings.theme === value && "bg-pink-500 hover:bg-pink-600"
                                    )}
                                >
                                    <ThemeIcon className="w-4 h-4" />
                                    {label}
                                </Button>
                            ))}
                        </div>
                    </SettingRow>

                    <SettingRow label="Font Size" description="Adjust the interface font size">
                        <Select
                            value={settings.fontSize}
                            onValueChange={(v) => setSettings({ fontSize: v as 'small' | 'medium' | 'large' })}
                        >
                            <SelectTrigger className="w-32">
                                <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                                <SelectItem value="small">Small</SelectItem>
                                <SelectItem value="medium">Medium</SelectItem>
                                <SelectItem value="large">Large</SelectItem>
                            </SelectContent>
                        </Select>
                    </SettingRow>

                    <SettingRow label="Show Line Numbers" description="Display line numbers in preview">
                        <Switch
                            checked={settings.showLineNumbers}
                            onCheckedChange={(v) => setSettings({ showLineNumbers: v })}
                            className="data-[state=checked]:bg-pink-500"
                        />
                    </SettingRow>
                </Section>

                {/* Search Behavior Section */}
                <Section title="Search Behavior" icon={Search}>
                    <SettingRow label="Default Search Path" description="Pre-fill this path when starting a new search">
                        <Input
                            value={settings.defaultSearchPath}
                            onChange={(e) => setSettings({ defaultSearchPath: e.target.value })}
                            placeholder="C:\Projects"
                            className="w-48"
                        />
                    </SettingRow>

                    <SettingRow label="Maximum Results" description="Limit the number of search results">
                        <div className="flex items-center gap-2">
                            {[500, 1000, 5000, 10000].map(limit => (
                                <Button
                                    key={limit}
                                    variant={settings.maxResults === limit ? "default" : "outline"}
                                    size="sm"
                                    onClick={() => setSettings({ maxResults: limit })}
                                    className={cn(
                                        "min-w-[60px]",
                                        settings.maxResults === limit && "bg-pink-500 hover:bg-pink-600"
                                    )}
                                >
                                    {limit >= 1000 ? `${limit / 1000}k` : limit}
                                </Button>
                            ))}
                            <Button
                                variant={settings.maxResults === null ? "default" : "outline"}
                                size="sm"
                                onClick={() => setSettings({ maxResults: null })}
                                className={cn(
                                    settings.maxResults === null && "bg-pink-500 hover:bg-pink-600"
                                )}
                            >
                                No limit
                            </Button>
                        </div>
                    </SettingRow>

                    <SettingRow label="Search Delay" description="Debounce delay in milliseconds">
                        <Input
                            type="number"
                            value={settings.searchDelay}
                            onChange={(e) => setSettings({ searchDelay: Number(e.target.value) })}
                            min={0}
                            max={2000}
                            step={50}
                            className="w-24 text-right"
                        />
                    </SettingRow>

                    <SettingRow label="Clear Results on New Search" description="Automatically clear previous results">
                        <Switch
                            checked={settings.clearResultsOnNewSearch}
                            onCheckedChange={(v) => setSettings({ clearResultsOnNewSearch: v })}
                            className="data-[state=checked]:bg-pink-500"
                        />
                    </SettingRow>
                </Section>

                {/* Preview Section */}
                <Section title="Preview" icon={Eye}>
                    <SettingRow label="Context Lines" description="Number of lines to show around matches">
                        <Input
                            type="number"
                            value={settings.previewLines}
                            onChange={(e) => setSettings({ previewLines: Number(e.target.value) })}
                            min={1}
                            max={20}
                            className="w-24 text-right"
                        />
                    </SettingRow>

                    <SettingRow label="Auto-open Preview" description="Automatically show preview when selecting a result">
                        <Switch
                            checked={settings.autoOpenPreview}
                            onCheckedChange={(v) => setSettings({ autoOpenPreview: v })}
                            className="data-[state=checked]:bg-pink-500"
                        />
                    </SettingRow>
                </Section>

                {/* Editor Section */}
                <Section title="Editor" icon={FileText}>
                    <SettingRow label="Editor Path" description="Command to open files (e.g., code, notepad)">
                        <Input
                            value={settings.editorPath}
                            onChange={(e) => setSettings({ editorPath: e.target.value })}
                            placeholder="code, notepad, etc."
                            className="w-48"
                        />
                    </SettingRow>

                    <SettingRow label="Confirm Before Replace" description="Show confirmation dialog before replacing">
                        <Switch
                            checked={settings.confirmBeforeReplace}
                            onCheckedChange={(v) => setSettings({ confirmBeforeReplace: v })}
                            className="data-[state=checked]:bg-pink-500"
                        />
                    </SettingRow>
                </Section>

                {/* Exclusions Section */}
                <Section title="Exclusions" icon={X}>
                    <div className="space-y-4">
                        <p className="text-sm text-zinc-500 dark:text-zinc-400">
                            Patterns to exclude from search results (e.g., node_modules, .git)
                        </p>
                        <div className="flex gap-2">
                            <Input
                                value={newExclusion}
                                onChange={(e) => setNewExclusion(e.target.value)}
                                placeholder="Add exclusion pattern..."
                                className="flex-1"
                                onKeyDown={(e) => e.key === 'Enter' && handleAddExclusion()}
                            />
                            <Button
                                onClick={handleAddExclusion}
                                disabled={!newExclusion}
                                className="bg-pink-500 hover:bg-pink-600"
                            >
                                <Plus className="w-4 h-4 mr-2" />
                                Add
                            </Button>
                        </div>
                        <div className="flex flex-wrap gap-2">
                            {settings.exclusions.map((ex) => (
                                <span
                                    key={ex}
                                    className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-full text-sm bg-zinc-100 dark:bg-zinc-800 text-zinc-700 dark:text-zinc-300 border border-zinc-200 dark:border-zinc-700"
                                >
                                    {ex}
                                    <button
                                        onClick={() => handleRemoveExclusion(ex)}
                                        className="p-0.5 rounded-full hover:bg-red-500/10 hover:text-red-500 transition-colors"
                                    >
                                        <X className="w-3 h-3" />
                                    </button>
                                </span>
                            ))}
                        </div>
                    </div>
                </Section>

                {/* History Section */}
                <Section title="History" icon={History}>
                    <SettingRow label="Save Search History" description="Remember your search queries">
                        <Switch
                            checked={settings.saveSearchHistory}
                            onCheckedChange={(v) => setSettings({ saveSearchHistory: v })}
                            className="data-[state=checked]:bg-pink-500"
                        />
                    </SettingRow>

                    <SettingRow label="Maximum History Items" description="Number of searches to remember">
                        <Input
                            type="number"
                            value={settings.maxHistoryItems}
                            onChange={(e) => setSettings({ maxHistoryItems: Number(e.target.value) })}
                            min={10}
                            max={10000}
                            step={10}
                            className="w-24 text-right"
                        />
                    </SettingRow>

                    <div className="pt-4 border-t border-zinc-200 dark:border-zinc-800">
                        <div className="flex items-center justify-between">
                            <div>
                                <p className="text-sm font-medium text-zinc-900 dark:text-zinc-100">Clear Search History</p>
                                <p className="text-xs text-zinc-500 dark:text-zinc-400">
                                    {history.length} {history.length === 1 ? 'item' : 'items'} saved
                                </p>
                            </div>
                            <Button
                                variant="ghost"
                                onClick={clearHistory}
                                disabled={history.length === 0}
                                className="text-red-500 hover:text-red-600 hover:bg-red-500/10"
                            >
                                Clear All
                            </Button>
                        </div>
                    </div>
                </Section>

                {/* About Section */}
                <Section title="About" icon={Info}>
                    <div className="flex items-center gap-4 pb-4 border-b border-zinc-200 dark:border-zinc-800">
                        <div className="w-14 h-14 rounded-2xl bg-gradient-to-br from-pink-500 to-purple-600 flex items-center justify-center shadow-lg shadow-pink-500/20">
                            <Zap className="w-8 h-8 text-white fill-current" />
                        </div>
                        <div>
                            <h3 className="text-lg font-bold text-zinc-900 dark:text-white">ShadUI Ripgrep Pro</h3>
                            <p className="text-sm text-zinc-500 dark:text-zinc-400">Lightning-fast code search</p>
                        </div>
                    </div>

                    <div className="space-y-3 pt-2">
                        <Button
                            variant="ghost"
                            onClick={() => openUrl('https://github.com/shadijhade/shadui-ripgrep')}
                            className="w-full justify-between"
                        >
                            <span>View on GitHub</span>
                            <ExternalLink className="w-4 h-4" />
                        </Button>

                        <div className="pt-4 border-t border-zinc-200 dark:border-zinc-800">
                            <Button
                                variant="ghost"
                                onClick={handleResetSettings}
                                className="w-full text-zinc-600 dark:text-zinc-400"
                            >
                                <RotateCcw className="w-4 h-4 mr-2" />
                                Reset All Settings to Defaults
                            </Button>
                        </div>
                    </div>
                </Section>

                {/* Bottom Spacer */}
                <div className="h-6" />
            </div>
        </div>
    );
}
