import { Fragment, useLayoutEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { ArrowLeft, ArrowRight, ArrowUp, ChevronRight, type LucideIcon, Monitor, RotateCw, Search as SearchIcon, X } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { clearFind, goBack, goForward, goUp, navigate, refresh, revealInApp, runFind, useExplorer } from "@/lib/explorer";
import { driveName, resolvePath } from "@/lib/fs";
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
    const go = async () => {
        const value = text.trim();
        if (!value || /^this pc$/i.test(value)) {
            close();
            navigate(THIS_PC);
            return;
        }
        try {
            const target = await resolvePath(value);
            close();
            // A file opens its folder with it selected.
            if (target.dir) navigate(target.path);
            else revealInApp(target.path);
        } catch (e) {
            toast.error(String(e));
        }
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

/** Back/forward/up, the breadcrumb address bar (click it to type a path) and the search box. */
export function AddressBar() {
    const { path, back, forward, loading, drives, filter, find, editing } = useExplorer(
        useShallow((s) => ({ path: s.path, back: s.back, forward: s.forward, loading: s.loading, drives: s.drives, filter: s.filter, find: s.find, editing: s.editingAddress }))
    );
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
    const folderName = path === THIS_PC ? "This PC" : trail.length > 1 ? trail[trail.length - 1].name : rootName(path);
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
                <NavButton icon={ArrowLeft} label="Back (Alt+Left)" onClick={goBack} disabled={!back.length} />
                <NavButton icon={ArrowRight} label="Forward (Alt+Right)" onClick={goForward} disabled={!forward.length} />
                <NavButton icon={ArrowUp} label="Up (Alt+Up)" onClick={goUp} disabled={path === THIS_PC} />
                <NavButton icon={RotateCw} label="Refresh (F5)" onClick={refresh} className={cn(loading && "[&_svg]:animate-spin")} />
            </div>

            <div className="flex h-9 min-w-0 flex-1 items-center rounded-lg border bg-background shadow-xs transition-shadow focus-within:border-ring/60 focus-within:ring-2 focus-within:ring-ring/15">
                {editing ? (
                    <AddressInput path={path} />
                ) : (
                    <div ref={trailRef} className="flex h-full min-w-0 flex-1 items-center overflow-x-auto px-1 [scrollbar-width:none]">
                        <button onClick={() => navigate(THIS_PC)} className="flex h-7 shrink-0 items-center gap-1.5 rounded-md px-1.5 text-sm hover:bg-accent" title="This PC">
                            <Monitor className="size-4 text-muted-foreground" />
                            {path === THIS_PC && "This PC"}
                        </button>
                        {trail.map((crumb, i) => (
                            <Fragment key={crumb.path}>
                                <ChevronRight className="size-3.5 shrink-0 text-muted-foreground/60" />
                                <button
                                    onClick={() => navigate(crumb.path)}
                                    className={cn("h-7 shrink-0 rounded-md px-1.5 text-sm hover:bg-accent", i === trail.length - 1 && "font-medium")}
                                >
                                    {i === 0 ? rootName(crumb.path) : crumb.name}
                                </button>
                            </Fragment>
                        ))}
                        <div onClick={startEditing} className="h-full min-w-10 flex-1 cursor-text" title="Type a path (Ctrl+L)" />
                    </div>
                )}
            </div>

            <div className="relative w-[clamp(11rem,22vw,19rem)] shrink-0">
                <SearchIcon className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                <input
                    id={FIND_BOX_ID}
                    value={filter}
                    onChange={(e) => useExplorer.setState({ filter: e.target.value })}
                    onKeyDown={onFindKeyDown}
                    spellCheck={false}
                    placeholder={`Search ${folderName}`}
                    aria-label={`Search ${folderName}. Typing filters; Enter searches subfolders too.`}
                    title="Typing filters this folder · Enter searches subfolders with fd (* and ? work, words all must match)"
                    className="h-9 w-full rounded-lg border bg-background pl-8 pr-8 text-sm shadow-xs outline-hidden transition-shadow placeholder:text-muted-foreground focus:border-ring/60 focus:ring-2 focus:ring-ring/15"
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
