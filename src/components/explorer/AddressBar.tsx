import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Check,
    ChevronRight,
    Ellipsis,
    File,
    FileArchive,
    Folder,
    Loader2,
    type LucideIcon,
    Monitor,
    RotateCw,
    Search as SearchIcon,
    Trash2,
    Cloud,
    X,
} from "lucide-react";
import { toast } from "sonner";
import { useShallow } from "zustand/react/shallow";
import { type FileEntry, isArchiveFile, isHidden } from "@/lib/entries";
import { clearFind, goBack, goForward, goTo, goUp, middleClickOpens, navigate, refresh, runFind, tabName, updateTab, useExplorer, useTab } from "@/lib/explorer";
import { driveName, listDir } from "@/lib/fs";
import { useGit } from "@/lib/git";
import { crumbs, isRecycleBin, isRemotePath, RECYCLE_BIN, samePath, THIS_PC } from "@/lib/paths";
import { remoteName, remotePath } from "@/lib/remotes";
import { editsAllowed, typoDistance } from "@/lib/rank";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";
import { GitChip } from "./GitChip";
import { DRIVE_ICONS, KNOWN_ICONS } from "./Sidebar";

export const FIND_BOX_ID = "find-box";

/** The search box's filters, as it offers them when empty: what choosing one types, an example, and what it means. */
const FILTERS: [string, string, string][] = [
    ["ext:", "ext:pdf", "of a type (ext:jpg,png for more)"],
    ["size:>", "size:>10mb", "bigger than (size:<1kb smaller)"],
    ["dm:", "dm:7d", "changed in the last 7 days (dm:>1y before)"],
    ["file:", "file:", "files only"],
    ["folder:", "folder:", "folders only"],
];

/** What photos, videos and songs say of themselves (media.rs reads it): as you type for what's here, by the search on Enter. */
const PROP_FILTERS: [string, string, string][] = [
    ["width:>", "width:>1920", "wider than (height:<1080 too), photos and videos"],
    ["taken:", "taken:2024", "taken in 2024 (taken:2024-06, taken:>2023, taken:7d)"],
    ["camera:", "camera:pixel", "taken with that camera"],
    ["duration:>", "duration:>5m", "longer than 5 minutes (1:30, 90s), videos and songs"],
    ["artist:", "artist:beatles", "songs by (album: too)"],
];

/** Offered too in a Git repository: it filters what's on screen (Enter doesn't take it into subfolders). */
const GIT_FILTER: [string, string, string] = ["git:", "git:changed", "changed in Git (git:new, git:staged…), here"];

/** Whether a search holds a git: filter, which works on the folder shown only. */
const hasGitFilter = (text: string) => /(^|\s)git:/i.test(text);

function NavButton({ icon: Icon, label, className, ...props }: { icon: LucideIcon; label: string } & React.ComponentProps<"button">) {
    return (
        <button
            type="button"
            title={label}
            aria-label={label}
            className={cn(
                "grid size-8 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-40 max-sm:size-7 [&_svg]:size-4",
                className
            )}
            {...props}
        >
            <Icon />
        </button>
    );
}

interface Suggestion {
    name: string;
    detail?: string;
    path: string;
    dir: boolean;
    archive?: boolean;
    icon: LucideIcon;
    iconClass?: string;
}

function AddressInput({ path }: { path: string }) {
    const [text, setText] = useState(path === THIS_PC ? "This PC" : isRecycleBin(path) ? "Recycle Bin" : path);
    const [dirEntries, setDirEntries] = useState<{ dir: string; entries: FileEntry[] } | null>(null);
    const [activeIndex, setActiveIndex] = useState(-1);
    const ref = useRef<HTMLInputElement>(null);
    const cacheRef = useRef<Map<string, FileEntry[]>>(new Map());
    const showHidden = useStore((s) => s.settings.showHidden);
    const { drives, known } = useExplorer(useShallow((s) => ({ drives: s.drives, known: s.known })));

    useLayoutEffect(() => {
        ref.current?.focus();
        ref.current?.select();
    }, []);

    // Split typed path into parent directory to list and filename prefix to filter.
    const { parentDir, prefix } = useMemo(() => {
        const raw = text.trim();
        if (!raw || raw.toLowerCase() === "this pc") return { parentDir: "", prefix: "" };
        const lastSep = Math.max(raw.lastIndexOf("\\"), raw.lastIndexOf("/"));
        if (lastSep < 0) return { parentDir: "", prefix: raw.toLowerCase() };
        const dir = raw.slice(0, lastSep + 1);
        const sub = raw.slice(lastSep + 1).toLowerCase();
        return { parentDir: dir, prefix: sub };
    }, [text]);

    useEffect(() => {
        if (!parentDir || !(/^[A-Za-z]:[\\/]/.test(parentDir) || /^\\\\[^\\]+\\[^\\]+/.test(parentDir))) {
            setDirEntries(null);
            return;
        }
        const key = parentDir.replace(/[\\/]+$/, "\\").toLowerCase();
        const cached = cacheRef.current.get(key);
        if (cached) {
            setDirEntries({ dir: parentDir, entries: cached });
            return;
        }
        let cancelled = false;
        listDir(parentDir)
            .then((listing) => {
                if (cancelled) return;
                cacheRef.current.set(key, listing.entries);
                setDirEntries({ dir: parentDir, entries: listing.entries });
            })
            .catch(() => {
                if (!cancelled) setDirEntries(null);
            });
        return () => {
            cancelled = true;
        };
    }, [parentDir]);

    const suggestions = useMemo<Suggestion[]>(() => {
        if (!parentDir) {
            if (!prefix) return [];
            const items: Suggestion[] = [];
            for (const d of drives) {
                const label = driveName(d);
                if (d.path.toLowerCase().startsWith(prefix) || label.toLowerCase().includes(prefix)) {
                    items.push({ name: label, detail: d.path, path: d.path, dir: true, icon: DRIVE_ICONS[d.kind] });
                }
            }
            for (const k of known) {
                if (k.name.toLowerCase().includes(prefix) || k.path.toLowerCase().includes(prefix)) {
                    items.push({ name: k.name, detail: k.path, path: k.path, dir: true, icon: KNOWN_ICONS[k.id] });
                }
            }
            return items.slice(0, 10);
        }
        if (!dirEntries) return [];
        const starts: Suggestion[] = [];
        const contains: Suggestion[] = [];
        // Typed with a typo ("Dowloads"): names a letter or two off, fewest first, after the rest.
        const close: { item: Suggestion; edits: number }[] = [];
        const typos = editsAllowed(prefix) > 0;
        for (const e of dirEntries.entries) {
            if (!showHidden && isHidden(e)) continue;
            const archive = isArchiveFile(e);
            const browsable = e.dir || archive;
            const icon = e.dir ? Folder : archive ? FileArchive : File;
            const iconClass = e.dir
                ? "fill-amber-400/80 text-amber-500"
                : archive
                  ? "text-amber-600 dark:text-amber-500"
                  : "text-muted-foreground";
            const item = { name: e.name, path: e.path, dir: browsable, archive, icon, iconClass };
            if (!prefix || e.lower.startsWith(prefix)) {
                starts.push(item);
            } else if (e.lower.includes(prefix)) {
                contains.push(item);
            } else if (typos) {
                const edits = typoDistance(prefix, e.name);
                if (edits !== null) close.push({ item, edits });
            }
            if (starts.length + contains.length >= 48) break;
        }
        const rank = (a: Suggestion, b: Suggestion) => (a.dir === b.dir ? 0 : a.dir ? -1 : 1);
        starts.sort(rank);
        contains.sort(rank);
        close.sort((a, b) => a.edits - b.edits || rank(a.item, b.item));
        return [...starts, ...contains, ...close.map((c) => c.item)].slice(0, 10);
    }, [parentDir, prefix, dirEntries, drives, known, showHidden]);

    useEffect(() => {
        setActiveIndex(-1);
    }, [text]);

    const close = () => useExplorer.setState({ editingAddress: false });

    const go = async (target = text) => {
        if (await goTo(target)) close();
    };

    const completeWith = (item: Suggestion) => {
        const sep = text.includes("/") && !text.includes("\\") ? "/" : "\\";
        const next = item.dir && !item.archive ? (item.path.endsWith(sep) ? item.path : item.path + sep) : item.path;
        setText(next);
        setActiveIndex(-1);
        requestAnimationFrame(() => {
            const el = ref.current;
            if (el) {
                el.focus();
                el.setSelectionRange(next.length, next.length);
            }
        });
    };

    return (
        <div className="relative flex h-full min-w-0 flex-1 items-center">
            <input
                ref={ref}
                value={text}
                onChange={(e) => setText(e.target.value)}
                onKeyDown={(e) => {
                    if (e.key === "ArrowDown" && suggestions.length) {
                        e.preventDefault();
                        setActiveIndex((i) => (i + 1) % suggestions.length);
                    } else if (e.key === "ArrowUp" && suggestions.length) {
                        e.preventDefault();
                        setActiveIndex((i) => (i <= 0 ? suggestions.length - 1 : i - 1));
                    } else if (e.key === "Tab" && suggestions.length) {
                        e.preventDefault();
                        completeWith(suggestions[activeIndex >= 0 ? activeIndex : 0]);
                    } else if (e.key === "Enter") {
                        e.preventDefault();
                        if (activeIndex >= 0 && suggestions[activeIndex]) void go(suggestions[activeIndex].path);
                        else void go();
                    } else if (e.key === "Escape") {
                        close();
                    }
                }}
                onBlur={close}
                spellCheck={false}
                aria-label="Address"
                aria-autocomplete="list"
                className="h-full min-w-0 flex-1 bg-transparent px-3 text-sm outline-hidden"
            />
            {suggestions.length > 0 && (
                <div
                    onMouseDown={(e) => e.preventDefault()}
                    data-overlay
                    className="glint-pop absolute inset-x-0 top-[calc(100%+4px)] z-50 max-h-72 overflow-y-auto rounded-md border bg-popover p-1 text-popover-foreground shadow-lg"
                >
                    <div className="flex items-center justify-between px-2 py-1 text-[0.6875rem] text-muted-foreground">
                        <span>Path suggestions</span>
                        <span>Tab to complete · Enter to open</span>
                    </div>
                    {suggestions.map((item, idx) => {
                        const Icon = item.icon;
                        return (
                            <button
                                key={item.path}
                                type="button"
                                onClick={() => void go(item.path)}
                                onMouseEnter={() => setActiveIndex(idx)}
                                className={cn(
                                    "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm transition-colors",
                                    idx === activeIndex ? "bg-accent text-accent-foreground" : "hover:bg-accent/60"
                                )}
                            >
                                <Icon className={cn("size-4 shrink-0 text-muted-foreground", item.iconClass)} />
                                <span className="truncate font-medium">{item.name}</span>
                                {item.detail && <span className="ml-auto truncate pl-3 font-mono text-xs text-muted-foreground">{item.detail}</span>}
                            </button>
                        );
                    })}
                </div>
            )}
        </div>
    );
}

interface FlyoutTarget {
    /** Folder whose subfolders are shown (`THIS_PC` for roots/known folders). */
    folder: string;
    /** Currently active child segment along the breadcrumb trail, if any. */
    activeChild?: string;
    left: number;
    top: number;
}

/** Back/forward/up, the breadcrumb address bar (click it to type a path) and the search box, for the active tab. */
export function AddressBar() {
    const { tab, path, back, forward, loading, filter, find, archive, activeListing } = useTab(
        useShallow((t) => ({
            tab: t.id,
            path: t.path,
            back: t.back.length > 0,
            forward: t.forward.length > 0,
            loading: t.loading,
            filter: t.filter,
            find: !!t.find,
            archive: t.listing?.archive,
            activeListing: t.listing,
        }))
    );
    // The archives on the way are marked, so it's clear where the folders on disk end.
    const isArchive = (crumb: string) => !!archive && (samePath(crumb, archive.path) || samePath(crumb, archive.outer));
    const { drives, known, editing } = useExplorer(useShallow((s) => ({ drives: s.drives, known: s.known, editing: s.editingAddress })));
    const showHidden = useStore((s) => s.settings.showHidden);
    const trailRef = useRef<HTMLDivElement>(null);
    const crumbsRef = useRef<HTMLDivElement>(null);
    const flyoutRef = useRef<HTMLDivElement>(null);
    // The flyout opened last: a slow folder's list mustn't replace the one asked for after it.
    const flyoutSeq = useRef(0);
    const flyoutFilterRef = useRef<HTMLInputElement>(null);
    const [flyout, setFlyout] = useState<FlyoutTarget | null>(null);
    const [flyoutItems, setFlyoutItems] = useState<Suggestion[]>([]);
    const [flyoutLoading, setFlyoutLoading] = useState(false);
    const [flyoutFilter, setFlyoutFilter] = useState("");
    // The search box, focused and empty, shows what it can do.
    const [findFocused, setFindFocused] = useState(false);
    const inRepository = useGit((s) => !!s[tab]?.view);
    const trail = crumbs(path);

    // A path too long for the bar keeps its end, as the window narrows too: the crumbs are laid out
    // from the end (wrapping onto a line out of view), so it's the folders above the ones that fit that
    // go, into a "…" menu before them, as in Explorer.
    const [folded, setFolded] = useState(0);
    // Counted for the path before, until it's counted again for this one (never all of it).
    const hidden = Math.max(0, Math.min(folded, trail.length - 1));
    useLayoutEffect(() => {
        const [bar, box] = [trailRef.current, crumbsRef.current];
        if (!bar || !box) return;
        const count = () => {
            const top = box.getBoundingClientRect().top + 1;
            setFolded([...box.children].filter((el) => el.getBoundingClientRect().top > top).length);
        };
        count();
        // The bar's width follows the window's, not the "…" coming and going.
        const observer = new ResizeObserver(count);
        observer.observe(bar);
        return () => observer.disconnect();
    }, [path, drives, editing, folded]);

    const rootName = (root: string) => {
        if (isRemotePath(root)) return remoteName(root);
        const drive = drives.find((d) => samePath(d.path, root));
        return drive ? driveName(drive) : root;
    };
    // A drive by its name; the Recycle Bin, a place, by its own.
    const crumbName = (i: number) => (i === 0 && !isRecycleBin(trail[0].path) ? rootName(trail[0].path) : trail[i].name);
    /** A crumb in the "…" menu: the drive's own icon for the first. */
    const crumbItem = (crumb: { path: string }, i: number): Suggestion => {
        const drive = i === 0 ? drives.find((d) => samePath(d.path, crumb.path)) : undefined;
        if (drive) return { name: crumbName(i), path: crumb.path, dir: true, icon: DRIVE_ICONS[drive.kind] };
        if (isArchive(crumb.path)) return { name: crumbName(i), path: crumb.path, dir: true, archive: true, icon: FileArchive, iconClass: "text-amber-600 dark:text-amber-500" };
        return { name: crumbName(i), path: crumb.path, dir: true, icon: Folder, iconClass: "fill-amber-400/80 text-amber-500" };
    };
    const folderName = tabName(path, drives);
    const startEditing = () => useExplorer.setState({ editingAddress: true });

    /** The subfolders of `folder`, or with `given`, those (the folders the "…" stands for). */
    const openFlyout = async (e: React.MouseEvent<HTMLButtonElement>, folder: string, activeChild?: string, given?: Suggestion[]) => {
        e.stopPropagation();
        const seq = ++flyoutSeq.current;
        const rect = e.currentTarget.getBoundingClientRect();
        const popover = flyoutRef.current;
        setFlyout({ folder, activeChild, left: rect.left, top: rect.bottom + 4 });
        setFlyoutFilter("");

        if (popover && !popover.matches(":popover-open")) {
            popover.showPopover();
        }

        if (given) {
            setFlyoutLoading(false);
            setFlyoutItems(given);
            return;
        }

        if (folder === THIS_PC) {
            setFlyoutLoading(false);
            const roots: Suggestion[] = [
                ...drives.map((d) => ({ name: driveName(d), detail: d.fs, path: d.path, dir: true, icon: DRIVE_ICONS[d.kind] })),
                ...known.map((k) => ({ name: k.name, path: k.path, dir: true, icon: KNOWN_ICONS[k.id] })),
                { name: "Recycle Bin", path: RECYCLE_BIN, dir: true, icon: Trash2 },
                ...useStore.getState().remotes.map((r) => ({ name: r.name, path: remotePath(r), dir: true, icon: Cloud })),
            ];
            setFlyoutItems(roots);
            return;
        }

        const toSuggestions = (entries: FileEntry[]): Suggestion[] =>
            entries
                .filter((item) => (item.dir || isArchiveFile(item)) && (showHidden || !isHidden(item)))
                .map((item) => ({
                    name: item.name,
                    path: item.path,
                    dir: true,
                    archive: !item.dir,
                    icon: item.dir ? Folder : FileArchive,
                    iconClass: item.dir ? "fill-amber-400/80 text-amber-500" : "text-amber-600 dark:text-amber-500",
                }));

        // Zero-IPC fast path when clicking the trailing chevron for the currently open directory.
        if (samePath(folder, path) && activeListing) {
            setFlyoutLoading(false);
            setFlyoutItems(toSuggestions(activeListing.entries));
            return;
        }

        setFlyoutLoading(true);
        const items = await listDir(folder).then((res) => toSuggestions(res.entries), () => []);
        if (seq !== flyoutSeq.current) return;
        setFlyoutItems(items);
        setFlyoutLoading(false);
    };

    useLayoutEffect(() => {
        const popover = flyoutRef.current;
        if (!flyout || !popover || !popover.matches(":popover-open")) return;
        const { width, height } = popover.getBoundingClientRect();
        popover.style.left = `${Math.max(4, Math.min(flyout.left, window.innerWidth - width - 4))}px`;
        popover.style.top = `${Math.max(4, Math.min(flyout.top, window.innerHeight - height - 4))}px`;
        if (flyoutItems.length > 8) {
            flyoutFilterRef.current?.focus();
        } else {
            popover.querySelector<HTMLButtonElement>("button[data-flyout-item]")?.focus();
        }
    }, [flyout, flyoutItems, flyoutLoading]);

    const visibleFlyoutItems = useMemo(() => {
        const q = flyoutFilter.trim().toLowerCase();
        if (!q) return flyoutItems;
        return flyoutItems.filter((i) => i.name.toLowerCase().includes(q));
    }, [flyoutItems, flyoutFilter]);

    const chevron = (i: number) => (
        <button
            type="button"
            onClick={(e) => void openFlyout(e, trail[i].path, trail[i + 1]?.path)}
            title={`Subfolders of ${crumbName(i)}`}
            aria-label={`Subfolders of ${crumbName(i)}`}
            className="grid size-5 shrink-0 place-items-center rounded-[4px] text-muted-foreground/70 transition-colors hover:bg-accent hover:text-foreground"
        >
            <ChevronRight className="size-3.5" />
        </button>
    );

    const onFlyoutKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
        if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
        e.preventDefault();
        const buttons = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("button[data-flyout-item]")];
        if (!buttons.length) return;
        const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const next = e.key === "ArrowDown" ? (at + 1) % buttons.length : (at - 1 + buttons.length) % buttons.length;
        buttons[next]?.focus();
    };

    const onFindKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
        if (e.key === "Enter") {
            e.preventDefault();
            // Git's marks are the folder shown's: a search of its subfolders couldn't ask them.
            if (hasGitFilter(filter)) toast.message("git: filters this folder, as you type", { id: "git-filter", description: "Enter searches subfolders by name, where Git's marks don't reach." });
            else runFind(filter);
        } else if (e.key === "Escape") {
            // Like Explorer: clear the search and go back to the list.
            if (filter || find) clearFind();
            e.currentTarget.blur();
        } else if (e.key === "ArrowDown") {
            // Hand the arrow to the list: leave the box and move to the first item.
            e.preventDefault();
            e.currentTarget.blur();
            window.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown" }));
        }
    };

    return (
        <div className="flex min-w-0 flex-1 items-center gap-2">
            <div className="flex shrink-0 items-center">
                <NavButton icon={ArrowLeft} label="Back (Alt+Left)" onClick={goBack} disabled={!back} />
                {/* A narrow window shows Forward only when there's somewhere to go. */}
                <NavButton icon={ArrowRight} label="Forward (Alt+Right)" onClick={goForward} disabled={!forward} className={cn(!forward && "max-sm:hidden")} />
                <NavButton icon={ArrowUp} label="Up (Alt+Up)" onClick={goUp} disabled={path === THIS_PC} />
                <NavButton icon={RotateCw} label="Refresh (F5)" onClick={refresh} className={cn("hidden md:grid", loading && "[&_svg]:animate-spin")} />
            </div>

            {/* In the title bar, but a click here types a path or picks a crumb rather than moving the window. */}
            <div data-tauri-drag-region="false" className="fluent-field flex h-8 min-w-0 flex-1 items-center">
                {editing ? (
                    <AddressInput path={path} />
                ) : (
                    // A narrow bar leaves This PC out, for the folder you're in.
                    <div ref={trailRef} className="@container flex h-full min-w-0 flex-1 items-center overflow-hidden px-1">
                        <button
                            type="button"
                            onClick={() => navigate(THIS_PC)}
                            onMouseDown={middleClickOpens(THIS_PC)}
                            className={cn("flex h-6 shrink-0 items-center gap-1.5 rounded-[4px] px-1.5 text-sm hover:bg-accent", path !== THIS_PC && "@max-[16rem]:hidden")}
                            title="This PC"
                        >
                            <Monitor className="size-4 text-muted-foreground" />
                            {path === THIS_PC && "This PC"}
                        </button>
                        <button
                            type="button"
                            onClick={(e) => void openFlyout(e, THIS_PC, trail[0]?.path)}
                            title="Drives and places"
                            aria-label="Drives and places"
                            className={cn(
                                "grid size-5 shrink-0 place-items-center rounded-[4px] text-muted-foreground/70 transition-colors hover:bg-accent hover:text-foreground",
                                path !== THIS_PC && "@max-[16rem]:hidden"
                            )}
                        >
                            <ChevronRight className="size-3.5" />
                        </button>
                        {hidden > 0 && (
                            // Narrower still, only the folder you're in.
                            <div className="flex shrink-0 items-center @max-[9rem]:hidden">
                                <button
                                    type="button"
                                    onClick={(e) => void openFlyout(e, "", undefined, trail.slice(0, hidden).map(crumbItem))}
                                    title="Folders above"
                                    aria-label="Folders above"
                                    className="grid h-6 shrink-0 place-items-center rounded-[4px] px-1.5 text-muted-foreground hover:bg-accent hover:text-foreground"
                                >
                                    <Ellipsis className="size-4" />
                                </button>
                                {chevron(hidden - 1)}
                            </div>
                        )}
                        <div ref={crumbsRef} className="flex h-6 min-w-12 flex-row-reverse flex-wrap justify-end overflow-hidden">
                            {trail.map((crumb, i) => (
                                // Laid out from the end; in the page (and to Tab), from the root.
                                <div key={crumb.path} style={{ order: trail.length - i }} className={cn("flex items-center", i === trail.length - 1 ? "min-w-0 max-w-full" : "shrink-0", i < hidden && "invisible")}>
                                    <button
                                        type="button"
                                        onClick={() => navigate(crumb.path)}
                                        onMouseDown={middleClickOpens(crumb.path)}
                                        title={isArchive(crumb.path) ? "An archive, opened like a folder (read-only)" : isRecycleBin(crumb.path) ? "What you deleted, to restore or delete for good" : i === 0 && isRemotePath(crumb.path) ? "A remote connection, opened read-only" : crumb.path}
                                        className={cn("flex h-6 min-w-0 items-center gap-1 rounded-[4px] px-1.5 text-sm hover:bg-accent", i === trail.length - 1 && "font-medium")}
                                    >
                                        {isArchive(crumb.path) && <FileArchive className="size-3.5 shrink-0 text-amber-600 dark:text-amber-500" />}
                                        {i === 0 && isRemotePath(crumb.path) && <Cloud className="size-3.5 shrink-0 text-sky-500" />}
                                        <span className="truncate">{crumbName(i)}</span>
                                    </button>
                                    {/* What's in the Recycle Bin isn't a folder to go into. */}
                                    {!isRecycleBin(crumb.path) && chevron(i)}
                                </div>
                            ))}
                        </div>
                        <GitChip tab={tab} path={path} />
                        <div onClick={startEditing} className="h-full min-w-4 flex-1 cursor-text" title="Type a path (Ctrl+L)" />
                    </div>
                )}
            </div>

            <div
                ref={flyoutRef}
                popover="auto"
                role="menu"
                onKeyDown={onFlyoutKeyDown}
                className="fixed inset-auto m-0 w-64 rounded-md border bg-popover p-1 text-popover-foreground shadow-lg"
            >
                {flyoutItems.length > 8 && (
                    <div className="mb-1 border-b pb-1">
                        <input
                            ref={flyoutFilterRef}
                            value={flyoutFilter}
                            onChange={(e) => setFlyoutFilter(e.target.value)}
                            placeholder="Filter folders…"
                            spellCheck={false}
                            className="h-7 w-full rounded-sm bg-muted/50 px-2 text-xs outline-hidden placeholder:text-muted-foreground"
                        />
                    </div>
                )}
                <div className="max-h-72 overflow-y-auto">
                    {flyoutLoading ? (
                        <div className="flex items-center justify-center gap-2 py-4 text-xs text-muted-foreground">
                            <Loader2 className="size-3.5 animate-spin" /> Loading folders…
                        </div>
                    ) : visibleFlyoutItems.length === 0 ? (
                        <div className="px-2 py-3 text-center text-xs text-muted-foreground">
                            {flyoutFilter ? "No matching folders" : "No subfolders"}
                        </div>
                    ) : (
                        visibleFlyoutItems.map((item) => {
                            const Icon = item.icon;
                            const active = !!flyout?.activeChild && samePath(item.path, flyout.activeChild);
                            return (
                                <button
                                    key={item.path}
                                    type="button"
                                    role="menuitem"
                                    data-flyout-item
                                    onMouseDown={middleClickOpens(item.path)}
                                    onClick={() => {
                                        flyoutRef.current?.hidePopover();
                                        void navigate(item.path);
                                    }}
                                    className={cn(
                                        "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm transition-colors hover:bg-accent focus-visible:bg-accent focus-visible:outline-hidden",
                                        active && "bg-primary/10 font-medium text-primary"
                                    )}
                                >
                                    <Icon className={cn("size-4 shrink-0 text-muted-foreground", item.iconClass)} />
                                    <span className="flex-1 truncate">{item.name}</span>
                                    {item.detail && <span className="text-[0.6875rem] text-muted-foreground">{item.detail}</span>}
                                    {active && <Check className="size-3.5 shrink-0 text-primary" />}
                                </button>
                            );
                        })
                    )}
                </div>
            </div>

            {/* In a narrow window, just its magnifier until it's clicked into or holds a search. */}
            <div
                data-tauri-drag-region="false"
                className={cn(
                    "relative w-[clamp(8rem,22vw,19rem)] shrink-0 transition-[width] duration-150 max-sm:w-[calc(2rem+2px)] max-sm:focus-within:w-[min(16rem,50vw)]",
                    (filter || find) && "max-sm:w-[min(16rem,50vw)]"
                )}
            >
                <SearchIcon className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                <input
                    id={FIND_BOX_ID}
                    value={filter}
                    onChange={(e) => updateTab({ filter: e.target.value })}
                    onKeyDown={onFindKeyDown}
                    onFocus={() => setFindFocused(true)}
                    onBlur={() => setFindFocused(false)}
                    spellCheck={false}
                    placeholder={`Search ${folderName}`}
                    aria-label={`Search ${folderName}. Typing filters; Enter searches subfolders too.`}
                    title="Typing filters this folder · Enter searches subfolders by name, with the instant search index or fd (* and ? work, words all must match). Filters: ext:pdf, size:>10mb, dm:7d (changed in the last 7 days), dm:>1y, file:, folder:, and for photos, videos and songs width:>1920, height:, taken:2024, camera:, duration:>5m, artist:, album:"
                    className={cn("fluent-field h-8 w-full pl-8 text-sm outline-hidden placeholder:text-muted-foreground", filter || find ? "pr-8" : "pr-8 max-sm:pr-0 max-sm:focus:pr-2")}
                />
                {(filter || find) && (
                    <button onClick={clearFind} className="absolute right-2 top-1/2 -translate-y-1/2 rounded-sm text-muted-foreground hover:text-foreground" title="Clear (Esc)" aria-label="Clear search">
                        <X className="size-4" />
                    </button>
                )}
                {findFocused && !filter && !find && (
                    // Mouse down keeps the focus in the box, which each filter starts.
                    <div data-overlay className="glint-pop absolute right-0 top-full z-50 mt-1 w-[max(100%,19rem)] rounded-md border bg-popover p-2.5 text-xs text-popover-foreground shadow-lg" onMouseDown={(e) => e.preventDefault()}>
                        <p className="text-muted-foreground">Typing filters this folder. Enter searches its subfolders too.</p>
                        {[
                            { title: "", filters: inRepository ? [...FILTERS, GIT_FILTER] : FILTERS },
                            { title: "What photos, videos and songs say (read from the files):", filters: PROP_FILTERS },
                        ].map(({ title, filters }) => (
                            <div key={title} className="mt-1.5 flex flex-col">
                                {title && <p className="px-1.5 pb-0.5 pt-1 text-muted-foreground">{title}</p>}
                                {filters.map(([start, example, meaning]) => (
                                    <button key={start} type="button" tabIndex={-1} onClick={() => updateTab({ filter: start })} className="flex items-baseline gap-2 rounded-sm px-1.5 py-1 text-left hover:bg-accent">
                                        <code className="shrink-0 font-mono text-[0.6875rem] text-primary">{example}</code>
                                        <span className="truncate text-muted-foreground">{meaning}</span>
                                    </button>
                                ))}
                            </div>
                        ))}
                    </div>
                )}
            </div>
        </div>
    );
}

