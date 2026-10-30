import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { ChevronRight, Folder, FolderOpen, Loader2 } from "lucide-react";
import { middleClickOpens, navigate, useExplorer } from "@/lib/explorer";
import { type FileEntry, isHidden } from "@/lib/entries";
import { joinPath, parentPath, samePath } from "@/lib/paths";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";

/** A folder in the tree: its name, and whether it has folders of its own (null: not known yet). */
interface Subfolder {
    name: string;
    children: boolean | null;
}

type Kids = Subfolder[] | "loading" | { error: string };

/** The sidebar's folder tree: which folders are open, and what's in each, by path in lower case. */
const useTree = create<{ open: Record<string, boolean>; kids: Record<string, Kids> }>(() => ({ open: {}, kids: {} }));

/** A path as the tree keys it: lower case, a drive as "c:\" and no other trailing backslash. */
const keyOf = (path: string) => {
    const lower = path.toLowerCase();
    return /^[a-z]:\\?$/.test(lower) ? `${lower.slice(0, 2)}\\` : lower.replace(/[\\/]+$/, "");
};

/** Reads (again) the folders in `path`. */
async function load(path: string) {
    const key = keyOf(path);
    if (!useTree.getState().kids[key]) useTree.setState((s) => ({ kids: { ...s.kids, [key]: "loading" } }));
    try {
        const found = await invoke<Subfolder[]>("list_subfolders", { path, hidden: useStore.getState().settings.showHidden });
        useTree.setState((s) => ({ kids: { ...s.kids, [key]: found } }));
    } catch (e) {
        useTree.setState((s) => ({ kids: { ...s.kids, [key]: { error: String(e) } } }));
    }
}

/** Opens or closes a folder in the tree; opening reads what's in it again. */
export function toggleFolder(path: string, open = !useTree.getState().open[keyOf(path)]) {
    useTree.setState((s) => ({ open: { ...s.open, [keyOf(path)]: open } }));
    if (open) void load(path);
}

/**
 * Opens the tree down to `path` (File Explorer's "Expand to open folder"), below the closest of the
 * sidebar's places it's in (`roots`: Home, a pinned folder, its drive), so it shows once; and scrolls
 * the sidebar to it.
 */
export async function revealInTree(path: string, roots: string[]) {
    const lower = keyOf(path);
    const root = roots
        .map(keyOf)
        .filter((r) => lower !== r && lower.startsWith(r.endsWith("\\") ? r : `${r}\\`))
        .sort((a, b) => b.length - a.length)[0];
    if (!root) return;
    const chain: string[] = [];
    for (let p = parentPath(path); p && keyOf(p).length >= root.length && !chain.includes(p); p = parentPath(p)) {
        chain.unshift(p);
        if (keyOf(p) === root) break;
    }
    for (const p of chain) {
        if (!useTree.getState().open[keyOf(p)]) {
            useTree.setState((s) => ({ open: { ...s.open, [keyOf(p)]: true } }));
            await load(p);
        }
    }
    requestAnimationFrame(() => document.querySelector("nav [data-tree-path][aria-current=page]")?.scrollIntoView({ block: "nearest" }));
}

/** What a folder's listing says is in it, for the tree (folders it already knew keep what it knew of them). */
export function treeSaw(path: string, entries: FileEntry[]) {
    const key = keyOf(path);
    const { open, kids } = useTree.getState();
    if (!open[key]) return;
    const hidden = useStore.getState().settings.showHidden;
    const before = Array.isArray(kids[key]) ? (kids[key] as Subfolder[]) : [];
    const known = new Map(before.map((k) => [k.name.toLowerCase(), k.children]));
    const now = entries.filter((e) => e.dir && (hidden || !isHidden(e))).map((e) => ({ name: e.name, children: known.get(e.lower) ?? null }));
    if (now.length === before.length && now.every((k, i) => k.name === before[i].name)) return;
    useTree.setState((s) => ({ kids: { ...s.kids, [key]: now } }));
}

/** Hidden folders shown or not: what's open is read again. */
export function treeHiddenChanged() {
    const { open } = useTree.getState();
    useTree.setState({ kids: {} });
    for (const [key, isOpen] of Object.entries(open)) if (isOpen) void load(key);
}

export const isTreeOpen = (path: string) => !!useTree.getState().open[keyOf(path)];

/** How far in a folder `depth` levels below a place is: past the place's own icon, a step a level. */
const indentOf = (depth: number) => 38 + depth * 14;

/** The arrow by a place or a folder in the tree that opens it (none where it's known to hold no folders). */
export function TreeArrow({ path, has = null, className, left }: { path: string; has?: boolean | null; className?: string; left?: number }) {
    const open = useTree((s) => !!s.open[keyOf(path)]);
    const loading = useTree((s) => s.kids[keyOf(path)] === "loading");
    if (has === false) return null;
    return (
        <button
            type="button"
            tabIndex={-1}
            aria-hidden
            onClick={(e) => {
                e.stopPropagation();
                toggleFolder(path);
            }}
            style={left === undefined ? undefined : { left }}
            className={cn(
                "absolute top-1/2 z-10 hidden size-4 -translate-y-1/2 place-items-center rounded-sm text-muted-foreground opacity-0 transition-opacity duration-100 group-hover/tree:opacity-100 hover:bg-background hover:text-foreground md:grid",
                open && "opacity-100",
                className
            )}
        >
            {loading ? <Loader2 className="size-3 animate-spin" /> : <ChevronRight className={cn("size-3.5 transition-transform duration-100", open && "rotate-90")} />}
        </button>
    );
}

/** The folders in `path`, while it's open in the tree, each a row of its own (and its folders, while open). */
export function TreeKids({ path, depth, current, onContextMenu }: { path: string; depth: number; current: string; onContextMenu: (e: React.MouseEvent, path: string) => void }) {
    const open = useTree((s) => !!s.open[keyOf(path)]);
    const kids = useTree((s) => s.kids[keyOf(path)]);
    if (!open || !kids || kids === "loading") return null;
    if (!Array.isArray(kids))
        return (
            <p role="none" className="hidden truncate py-1 text-xs text-muted-foreground md:block" style={{ paddingLeft: indentOf(depth) }} title={kids.error}>
                Can't look inside
            </p>
        );
    return (
        <div role="group" className="hidden md:block">
            {kids.map((kid) => (
                <TreeRow key={kid.name} path={joinPath(path, kid.name)} name={kid.name} has={kid.children} depth={depth} parent={path} current={current} onContextMenu={onContextMenu} />
            ))}
        </div>
    );
}

function TreeRow({ path, name, has, depth, parent, current, onContextMenu }: { path: string; name: string; has: boolean | null; depth: number; parent: string; current: string; onContextMenu: (e: React.MouseEvent, path: string) => void }) {
    const open = useTree((s) => !!s.open[keyOf(path)]);
    const dropping = useExplorer((s) => s.dropTarget === path);
    const active = samePath(path, current);
    const indent = indentOf(depth);
    return (
        <>
            <div className="group/tree relative">
                <TreeArrow path={path} has={has} left={indent - 18} />
                <button
                    data-drop={path}
                    data-tree-path={path}
                    data-tree-parent={parent}
                    data-tree-has={has === false ? "no" : "yes"}
                    role="treeitem"
                    aria-level={depth + 1}
                    aria-expanded={has === false ? undefined : open}
                    aria-current={active ? "page" : undefined}
                    onClick={() => navigate(path)}
                    onMouseDown={middleClickOpens(path)}
                    onContextMenu={(e) => onContextMenu(e, path)}
                    title={path}
                    style={{ paddingLeft: indent }}
                    className={cn(
                        "relative flex w-full items-center gap-2 rounded-md py-1 pr-2 text-left text-[0.8125rem] text-foreground/85 transition-colors duration-100 hover:bg-accent",
                        active && "bg-accent text-foreground before:absolute before:left-0 before:top-1/2 before:h-3.5 before:w-[3px] before:-translate-y-1/2 before:rounded-full before:bg-primary",
                        dropping && "bg-primary/20 outline-2 -outline-offset-2 outline-primary"
                    )}
                >
                    {open ? <FolderOpen className="size-4 shrink-0 fill-amber-400/80 text-amber-500" /> : <Folder className="size-4 shrink-0 fill-amber-400/80 text-amber-500" />}
                    <span className="truncate">{name}</span>
                </button>
            </div>
            <TreeKids path={path} depth={depth + 1} current={current} onContextMenu={onContextMenu} />
        </>
    );
}

/**
 * The sidebar's keys for its places and the tree's folders: Up and Down move between rows, Right opens
 * a folder (or goes into it), Left closes it (or goes up to its parent).
 */
export function onTreeKey(e: React.KeyboardEvent<HTMLElement>) {
    const target = e.target as HTMLElement;
    if (!["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(e.key) || !target.matches("[data-nav-row], [data-tree-path]")) return;
    const rows = [...e.currentTarget.querySelectorAll<HTMLElement>("[data-nav-row], [data-tree-path]")].filter((r) => r.offsetParent !== null);
    const at = rows.indexOf(target);
    const path = target.dataset.treePath ?? target.dataset.navTree;
    const go = (row: HTMLElement | undefined) => {
        if (!row) return;
        e.preventDefault();
        row.focus();
    };
    if (e.key === "ArrowDown") return go(rows[at + 1]);
    if (e.key === "ArrowUp") return go(rows[at - 1]);
    if (!path) return;
    const open = isTreeOpen(path);
    if (e.key === "ArrowRight") {
        if (target.dataset.treeHas === "no") return;
        e.preventDefault();
        if (!open) toggleFolder(path, true);
        else go(rows[at + 1]?.dataset.treeParent && samePath(rows[at + 1].dataset.treeParent!, path) ? rows[at + 1] : undefined);
    } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        if (open) toggleFolder(path, false);
        else if (target.dataset.treeParent) go(rows.find((r) => samePath(r.dataset.treePath ?? r.dataset.navTree ?? "", target.dataset.treeParent!)));
    }
}
