import { Fragment, useLayoutEffect, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, ArrowUp, ChevronRight, type LucideIcon, Monitor, RotateCw, Search as SearchIcon, X } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { clearFind, goBack, goForward, goTo, goUp, middleClickOpens, navigate, refresh, runFind, tabName, updateTab, useExplorer, useTab } from "@/lib/explorer";
import { driveName } from "@/lib/fs";
import { crumbs, samePath, THIS_PC } from "@/lib/paths";
import { cn } from "@/lib/utils";

export const FIND_BOX_ID = "find-box";

function NavButton({ icon: Icon, label, className, ...props }: { icon: LucideIcon; label: string } & React.ComponentProps<"button">) {
    return (
        <button
            type="button"
            title={label}
            aria-label={label}
            className={cn(
                "grid size-8 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-35 [&_svg]:size-4",
                className
            )}
            {...props}
        >
            <Icon />
        </button>
    );
}

function AddressInput({ path }: { path: string }) {
    const [text, setText] = useState(path === THIS_PC ? "This PC" : path);
    const ref = useRef<HTMLInputElement>(null);
    useLayoutEffect(() => {
        ref.current?.focus();
        ref.current?.select();
    }, []);
    const close = () => useExplorer.setState({ editingAddress: false });
    // A file opens its folder with it selected; a typo keeps the box open to fix it.
    const go = async () => {
        if (await goTo(text)) close();
    };
    return (
        <input
            ref={ref}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
                if (e.key === "Enter") go();
                else if (e.key === "Escape") close();
            }}
            onBlur={close}
            spellCheck={false}
            aria-label="Address"
            className="h-full min-w-0 flex-1 bg-transparent px-3 text-sm outline-hidden"
        />
    );
}

/** Back/forward/up, the breadcrumb address bar (click it to type a path) and the search box, for the active tab. */
export function AddressBar() {
    const { path, back, forward, loading, filter, find } = useTab(
        useShallow((t) => ({ path: t.path, back: t.back.length > 0, forward: t.forward.length > 0, loading: t.loading, filter: t.filter, find: !!t.find }))
    );
    const { drives, editing } = useExplorer(useShallow((s) => ({ drives: s.drives, editing: s.editingAddress })));
    const trailRef = useRef<HTMLDivElement>(null);
    const trail = crumbs(path);

    // Long paths keep their end in view; the start scrolls away to the left.
    useLayoutEffect(() => {
        const el = trailRef.current;
        if (el) el.scrollLeft = el.scrollWidth;
    }, [path, drives, editing]);

    const rootName = (root: string) => {
        const drive = drives.find((d) => samePath(d.path, root));
        return drive ? driveName(drive) : root;
    };
    const folderName = tabName(path, drives);
    const startEditing = () => useExplorer.setState({ editingAddress: true });

    const onFindKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
        if (e.key === "Enter") {
            e.preventDefault();
            runFind(filter);
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
                <NavButton icon={ArrowRight} label="Forward (Alt+Right)" onClick={goForward} disabled={!forward} />
                <NavButton icon={ArrowUp} label="Up (Alt+Up)" onClick={goUp} disabled={path === THIS_PC} />
                <NavButton icon={RotateCw} label="Refresh (F5)" onClick={refresh} className={cn(loading && "[&_svg]:animate-spin")} />
            </div>

            {/* In the title bar, but a click here types a path or picks a crumb rather than moving the window. */}
            <div data-tauri-drag-region="false" className="fluent-field flex h-8 min-w-0 flex-1 items-center">
                {editing ? (
                    <AddressInput path={path} />
                ) : (
                    <div ref={trailRef} className="flex h-full min-w-0 flex-1 items-center overflow-x-auto px-1 [scrollbar-width:none]">
                        <button onClick={() => navigate(THIS_PC)} onMouseDown={middleClickOpens(THIS_PC)} className="flex h-6 shrink-0 items-center gap-1.5 rounded-[4px] px-1.5 text-sm hover:bg-accent" title="This PC">
                            <Monitor className="size-4 text-muted-foreground" />
                            {path === THIS_PC && "This PC"}
                        </button>
                        {trail.map((crumb, i) => (
                            <Fragment key={crumb.path}>
                                <ChevronRight className="size-3.5 shrink-0 text-muted-foreground/60" />
                                <button
                                    onClick={() => navigate(crumb.path)}
                                    onMouseDown={middleClickOpens(crumb.path)}
                                    className={cn("h-6 shrink-0 rounded-[4px] px-1.5 text-sm hover:bg-accent", i === trail.length - 1 && "font-medium")}
                                >
                                    {i === 0 ? rootName(crumb.path) : crumb.name}
                                </button>
                            </Fragment>
                        ))}
                        <div onClick={startEditing} className="h-full min-w-10 flex-1 cursor-text" title="Type a path (Ctrl+L)" />
                    </div>
                )}
            </div>

            <div data-tauri-drag-region="false" className="relative w-[clamp(11rem,22vw,19rem)] shrink-0">
                <SearchIcon className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                <input
                    id={FIND_BOX_ID}
                    value={filter}
                    onChange={(e) => updateTab({ filter: e.target.value })}
                    onKeyDown={onFindKeyDown}
                    spellCheck={false}
                    placeholder={`Search ${folderName}`}
                    aria-label={`Search ${folderName}. Typing filters; Enter searches subfolders too.`}
                    title="Typing filters this folder · Enter searches subfolders by name, with the instant search index or fd (* and ? work, words all must match)"
                    className="fluent-field h-8 w-full pl-8 pr-8 text-sm outline-hidden placeholder:text-muted-foreground"
                />
                {(filter || find) && (
                    <button onClick={clearFind} className="absolute right-2 top-1/2 -translate-y-1/2 rounded-sm text-muted-foreground hover:text-foreground" title="Clear (Esc)" aria-label="Clear search">
                        <X className="size-4" />
                    </button>
                )}
            </div>
        </div>
    );
}
