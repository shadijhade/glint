import { Plus, X, RotateCcw, Monitor, Sun, Moon, Eye, Search, FileText, History, Info, ExternalLink, Zap, LucideIcon } from 'lucide-react';
import { useStore } from '@/lib/store';
import { useState, memo, useCallback } from 'react';
import { motion } from 'framer-motion';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Switch } from '@/components/ui/switch';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Separator } from '@/components/ui/separator';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Badge } from '@/components/ui/badge';
import { cn } from '@/lib/utils';

// Memoized Section component with Card
const Section = memo(({ title, icon: Icon, children }: { title: string; icon: LucideIcon; children: React.ReactNode }) => (
    <motion.div
        initial={{ opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        className="mb-4"
    >
        <Card className="border-zinc-200 dark:border-zinc-800 bg-white/70 dark:bg-zinc-900/70 backdrop-blur-md shadow-sm">
            <CardHeader className="pb-3">
                <div className="flex items-center gap-3">
                    <div className="p-2 rounded-xl bg-pink-500/10 border border-pink-500/20 text-pink-500">
                        <Icon className="w-4 h-4" />
                    </div>
                    <CardTitle className="text-base text-zinc-900 dark:text-zinc-100">{title}</CardTitle>
                </div>
            </CardHeader>
            <CardContent className="space-y-4">
                {children}
            </CardContent>
        </Card>
    </motion.div>
));
Section.displayName = 'Section';

// Memoized SettingRow component
const SettingRow = memo(({ label, description, children }: { label: string; description?: string; children: React.ReactNode }) => (
    <div>
        <div className="flex items-center justify-between gap-4 py-2">
            <div className="flex-1 min-w-0">
                <p className="text-sm font-medium text-zinc-900 dark:text-zinc-100">{label}</p>
                {description && <p className="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">{description}</p>}
            </div>
            <div className="shrink-0">{children}</div>
        </div>
        <Separator className="mt-2 opacity-50" />
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
        <ScrollArea className="h-full w-full">
            <div className="p-8 max-w-4xl mx-auto space-y-6 pb-20">
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
                                    variant={settings.theme === value ? "secondary" : "ghost"}
                                    size="sm"
                                    onClick={() => setSettings({ theme: value as 'light' | 'dark' | 'system' })}
                                    className={cn(
                                        "gap-2 h-8",
                                        settings.theme === value && "bg-zinc-200 dark:bg-zinc-700 hover:bg-zinc-300 dark:hover:bg-zinc-600"
                                    )}
                                >
                                    <ThemeIcon className="w-3.5 h-3.5" />
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
                            <SelectTrigger className="w-32 h-8">
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
                            className="w-56 h-8"
                        />
                    </SettingRow>

                    <SettingRow label="Maximum Results" description="Limit the number of search results">
                        <div className="flex items-center gap-2">
                            {[500, 1000, 5000].map(limit => (
                                <Button
                                    key={limit}
                                    variant={settings.maxResults === limit ? "secondary" : "ghost"}
                                    size="sm"
                                    onClick={() => setSettings({ maxResults: limit })}
                                    className="h-8 text-xs"
                                >
                                    {limit >= 1000 ? `${limit / 1000}k` : limit}
                                </Button>
                            ))}
                            <Button
                                variant={settings.maxResults === null ? "secondary" : "ghost"}
                                size="sm"
                                onClick={() => setSettings({ maxResults: null })}
                                className="h-8 text-xs"
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
                            className="w-24 text-right h-8"
                        />
                    </SettingRow>

                    <SettingRow label="Clear Results on New Search" description="Automatically clear previous results">
                        <Switch
                            checked={settings.clearResultsOnNewSearch}
                            onCheckedChange={(v) => setSettings({ clearResultsOnNewSearch: v })}
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
                            className="w-24 text-right h-8"
                        />
                    </SettingRow>

                    <SettingRow label="Auto-open Preview" description="Automatically show preview when selecting a result">
                        <Switch
                            checked={settings.autoOpenPreview}
                            onCheckedChange={(v) => setSettings({ autoOpenPreview: v })}
                        />
                    </SettingRow>
                </Section>

                {/* Editor Section */}
                <Section title="Editor" icon={FileText}>
                    <SettingRow label="Editor Path" description="Command to open files (e.g., code, notepad)">
                        <Input
                            value={settings.editorPath}
                            onChange={(e) => setSettings({ editorPath: e.target.value })}
                            placeholder="code, notepad"
                            className="w-56 h-8"
                        />
                    </SettingRow>

                    <SettingRow label="Confirm Before Replace" description="Show confirmation dialog before replacing">
                        <Switch
                            checked={settings.confirmBeforeReplace}
                            onCheckedChange={(v) => setSettings({ confirmBeforeReplace: v })}
                        />
                    </SettingRow>
                </Section>

                {/* Exclusions Section */}
                <Section title="Exclusions" icon={X}>
                    <div className="space-y-5">
                        <p className="text-sm text-zinc-500 dark:text-zinc-400">
                            Patterns to exclude from search results. Use folder names, file patterns (*.ext), or specific files.
                        </p>

                        {/* Quick Add Presets */}
                        <div className="space-y-3">
                            <div className="flex items-center gap-2">
                                <span className="text-xs font-semibold text-zinc-500 dark:text-zinc-400 uppercase tracking-wider">Quick Add</span>
                                <Separator className="flex-1" />
                            </div>

                            {/* Preset Categories */}
                            <div className="grid grid-cols-2 gap-2">
                                {[
                                    {
                                        label: '📁 Folders',
                                        items: ['node_modules', '.git', 'dist', 'build', 'bin', 'obj', '.vs', '.idea', '__pycache__', '.cache', 'vendor', 'packages', '.nuget', 'coverage', '.next', 'target'],
                                        color: 'bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-500/20 hover:bg-blue-500/20'
                                    },
                                    {
                                        label: '⚙️ Binaries',
                                        items: ['*.exe', '*.dll', '*.so', '*.dylib', '*.pdb', '*.lib', '*.a', '*.o', '*.class', '*.pyc'],
                                        color: 'bg-red-500/10 text-red-600 dark:text-red-400 border-red-500/20 hover:bg-red-500/20'
                                    },
                                    {
                                        label: '🖼️ Images',
                                        items: ['*.png', '*.jpg', '*.jpeg', '*.gif', '*.ico', '*.svg', '*.webp', '*.bmp', '*.tiff', '*.psd'],
                                        color: 'bg-green-500/10 text-green-600 dark:text-green-400 border-green-500/20 hover:bg-green-500/20'
                                    },
                                    {
                                        label: '📦 Archives',
                                        items: ['*.zip', '*.rar', '*.7z', '*.tar', '*.gz', '*.bz2', '*.xz', '*.iso'],
                                        color: 'bg-orange-500/10 text-orange-600 dark:text-orange-400 border-orange-500/20 hover:bg-orange-500/20'
                                    },
                                    {
                                        label: '🔤 Fonts',
                                        items: ['*.woff', '*.woff2', '*.ttf', '*.eot', '*.otf'],
                                        color: 'bg-purple-500/10 text-purple-600 dark:text-purple-400 border-purple-500/20 hover:bg-purple-500/20'
                                    },
                                    {
                                        label: '🔒 Lock Files',
                                        items: ['package-lock.json', 'yarn.lock', 'pnpm-lock.yaml', 'Cargo.lock', 'Gemfile.lock', 'composer.lock', 'poetry.lock'],
                                        color: 'bg-amber-500/10 text-amber-600 dark:text-amber-400 border-amber-500/20 hover:bg-amber-500/20'
                                    },
                                ].map((category) => {
                                    const hasAll = category.items.every(item => settings.exclusions.includes(item));
                                    const hasSome = category.items.some(item => settings.exclusions.includes(item));
                                    return (
                                        <Button
                                            key={category.label}
                                            variant="outline"
                                            size="sm"
                                            onClick={() => {
                                                if (hasAll) {
                                                    // Remove all from this category
                                                    setSettings({
                                                        exclusions: settings.exclusions.filter(ex => !category.items.includes(ex))
                                                    });
                                                } else {
                                                    // Add all new from this category
                                                    const newItems = category.items.filter(item => !settings.exclusions.includes(item));
                                                    setSettings({ exclusions: [...settings.exclusions, ...newItems] });
                                                }
                                            }}
                                            className={cn(
                                                "h-9 justify-start gap-2 transition-all",
                                                hasAll ? category.color : hasSome ? "border-dashed " + category.color : "border-dashed"
                                            )}
                                        >
                                            <span>{category.label}</span>
                                            <Badge variant="secondary" className="text-[10px] h-4 px-1">
                                                {category.items.filter(item => settings.exclusions.includes(item)).length}/{category.items.length}
                                            </Badge>
                                        </Button>
                                    );
                                })}
                            </div>
                        </div>

                        <Separator />

                        {/* Custom Pattern Input */}
                        <div className="space-y-3">
                            <div className="flex items-center gap-2">
                                <span className="text-xs font-semibold text-zinc-500 dark:text-zinc-400 uppercase tracking-wider">Custom Pattern</span>
                                <Separator className="flex-1" />
                            </div>
                            <div className="flex gap-2">
                                <Input
                                    value={newExclusion}
                                    onChange={(e) => setNewExclusion(e.target.value)}
                                    placeholder="folder, *.ext, or filename"
                                    className="flex-1 h-9"
                                    onKeyDown={(e) => e.key === 'Enter' && handleAddExclusion()}
                                />
                                <Button
                                    onClick={handleAddExclusion}
                                    disabled={!newExclusion}
                                    className="bg-pink-500 hover:bg-pink-600 h-9"
                                >
                                    <Plus className="w-4 h-4 mr-2" />
                                    Add
                                </Button>
                            </div>
                            <p className="text-[10px] text-zinc-400 dark:text-zinc-500">
                                Examples: <code className="bg-zinc-100 dark:bg-zinc-800 px-1 rounded">node_modules</code> (folder),
                                <code className="bg-zinc-100 dark:bg-zinc-800 px-1 rounded ml-1">*.log</code> (file type),
                                <code className="bg-zinc-100 dark:bg-zinc-800 px-1 rounded ml-1">thumbs.db</code> (file)
                            </p>
                        </div>

                        <Separator />

                        {/* Current Exclusions */}
                        <div className="space-y-3">
                            <div className="flex items-center justify-between">
                                <div className="flex items-center gap-2">
                                    <span className="text-xs font-semibold text-zinc-500 dark:text-zinc-400 uppercase tracking-wider">Active Exclusions</span>
                                    <Badge variant="secondary" className="text-[10px] h-5">{settings.exclusions.length}</Badge>
                                </div>
                                {settings.exclusions.length > 0 && (
                                    <Button
                                        variant="ghost"
                                        size="sm"
                                        onClick={() => setSettings({ exclusions: [] })}
                                        className="h-6 text-xs text-red-500 hover:text-red-600 hover:bg-red-500/10"
                                    >
                                        Clear All
                                    </Button>
                                )}
                            </div>
                            <div className="flex flex-wrap gap-2 max-h-48 overflow-y-auto p-1">
                                {settings.exclusions.length === 0 ? (
                                    <p className="text-sm text-zinc-400 dark:text-zinc-500 italic">No exclusions configured</p>
                                ) : (
                                    settings.exclusions.map((ex) => {
                                        // Determine type for icon/color
                                        const isFileType = ex.startsWith('*.');
                                        const isFolder = !ex.includes('.') && !ex.includes('*');
                                        return (
                                            <Badge
                                                key={ex}
                                                variant="secondary"
                                                className={cn(
                                                    "pl-2 pr-1 py-1 gap-1.5 transition-colors",
                                                    isFolder ? "bg-blue-500/10 text-blue-700 dark:text-blue-300 border border-blue-500/20" :
                                                        isFileType ? "bg-orange-500/10 text-orange-700 dark:text-orange-300 border border-orange-500/20" :
                                                            "bg-zinc-100 dark:bg-zinc-800"
                                                )}
                                            >
                                                <span className="text-xs">
                                                    {isFolder ? '📁' : isFileType ? '📄' : '📋'}
                                                </span>
                                                {ex}
                                                <button
                                                    onClick={() => handleRemoveExclusion(ex)}
                                                    className="p-0.5 rounded-full hover:bg-red-500/20 hover:text-red-500 transition-colors"
                                                >
                                                    <X className="w-3 h-3" />
                                                </button>
                                            </Badge>
                                        );
                                    })
                                )}
                            </div>
                        </div>
                    </div>
                </Section>

                {/* History Section */}
                <Section title="History" icon={History}>
                    <SettingRow label="Save Search History" description="Remember your search queries">
                        <Switch
                            checked={settings.saveSearchHistory}
                            onCheckedChange={(v) => setSettings({ saveSearchHistory: v })}
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
                            className="w-24 text-right h-8"
                        />
                    </SettingRow>

                    <div className="pt-4 mt-2">
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
                    <div className="flex items-center gap-4 pb-4">
                        <div className="w-14 h-14 rounded-2xl bg-gradient-to-br from-pink-500 to-purple-600 flex items-center justify-center shadow-lg shadow-pink-500/20">
                            <Zap className="w-8 h-8 text-white fill-current" />
                        </div>
                        <div>
                            <h3 className="text-lg font-bold text-zinc-900 dark:text-white">ShadUI Ripgrep Pro</h3>
                            <p className="text-sm text-zinc-500 dark:text-zinc-400">Lightning-fast code search</p>
                        </div>
                    </div>

                    <div className="space-y-3 pt-2 border-t border-zinc-200 dark:border-zinc-800">
                        <Button
                            variant="ghost"
                            onClick={() => openUrl('https://github.com/shadijhade/shadui-ripgrep')}
                            className="w-full justify-between mt-2"
                        >
                            <span>View on GitHub</span>
                            <ExternalLink className="w-4 h-4" />
                        </Button>

                        <div className="pt-2">
                            <Button
                                variant="outline"
                                onClick={handleResetSettings}
                                className="w-full text-zinc-600 dark:text-zinc-400"
                            >
                                <RotateCcw className="w-4 h-4 mr-2" />
                                Reset All Settings to Defaults
                            </Button>
                        </div>
                    </div>
                </Section>
            </div>
        </ScrollArea>
    );
}
