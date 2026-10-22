import { Disc, Download, FileText, Film, Folder, HardDrive, House, Image, type LucideIcon, Monitor, Music, Search, Server, Usb, X } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { type KnownFolder, middleClickOpens, navigate, runSavedSearch, useExplorer, useTab } from "@/lib/explorer";
import { type Drive, driveName } from "@/lib/fs";
import { formatSize } from "@/lib/format";
import { baseName, inArchivePath, samePath, THIS_PC } from "@/lib/paths";
import { type SavedSearch, useStore } from "@/lib/store";
import { cn } from "@/lib/utils";

export const KNOWN_ICONS: Record<KnownFolder["id"], LucideIcon> = {
    home: House,
    desktop: Monitor,
    documents: FileText,
    downloads: Download,
    pictures: Image,
    music: Music,
    videos: Film,
};

export const DRIVE_ICONS: Record<Drive["kind"], LucideIcon> = {
    fixed: HardDrive,
    removable: Usb,
    network: Server,
    cdrom: Disc,
    ram: HardDrive,
    unknown: HardDrive,
};

/** Used-space bar, red when nearly full like Explorer's. */
export function CapacityBar({ drive, className }: { drive: Drive; className?: string }) {
    if (!drive.total) return null;
    const used = (drive.total - drive.free) / drive.total;
    return (
        <div className={cn("h-1 overflow-hidden rounded-full bg-muted", className)}>
            <div className={cn("h-full rounded-full", used > 0.9 ? "bg-destructive" : "bg-primary/70")} style={{ width: `${Math.round(used * 100)}%` }} />
        </div>
    );
}

export const freeOf = (drive: Drive) => (drive.total ? `${formatSize(drive.free)} free of ${formatSize(drive.total)}` : "");

function Section({ title, onTitleClick, active, children }: { title: string; onTitleClick?: () => void; active?: boolean; children: React.ReactNode }) {
    return (
        <section>
            <h2 className="mb-1 px-2">
                {onTitleClick ? (
                    <button onClick={onTitleClick} className={cn("text-xs font-semibold text-muted-foreground hover:text-foreground", active && "text-foreground")}>
                        {title}
                    </button>
                ) : (
                    <span className="text-xs font-semibold text-muted-foreground">{title}</span>
                )}
            </h2>
            <div className="space-y-px">{children}</div>
        </section>
    );
}

interface PlaceProps {
    icon: LucideIcon;
    iconClass?: string;
    label: string;
    path: string;
    active: boolean;
    title?: string;
    onContextMenu: (e: React.MouseEvent, path: string) => void;
    children?: React.ReactNode;
}

const PLACE = "relative flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left text-sm text-foreground/90 transition-colors hover:bg-accent";

function Place({ icon: Icon, iconClass, label, path, active, title, onContextMenu, children }: PlaceProps) {
    const dropping = useExplorer((s) => s.dropTarget === path);
    return (
        <button
            // A pinned folder in an archive takes no drops: archives are read-only.
            data-drop={inArchivePath(path) ? undefined : path}
            onClick={() => navigate(path)}
            onMouseDown={middleClickOpens(path)}
            onContextMenu={(e) => onContextMenu(e, path)}
            title={title ?? path}
            aria-current={active ? "page" : undefined}
            className={cn(
                PLACE,
                // Windows 11's navigation selection: a gray fill and a short accent pill at the left edge.
                active &&
                    "bg-accent text-foreground before:absolute before:left-0 before:top-1/2 before:h-4 before:w-[3px] before:-translate-y-1/2 before:rounded-full before:bg-primary",
                dropping && "bg-primary/20 outline-2 -outline-offset-2 outline-primary"
            )}
        >
            <Icon className={cn("size-4 shrink-0 text-muted-foreground", active && "text-foreground", iconClass)} />
            <div className="min-w-0 flex-1">
                <div className="truncate">{label}</div>
                {children}
            </div>
        </button>
    );
}

/** A saved search: one click runs it again; the cross takes it out of the sidebar. */
function SavedSearchItem({ search }: { search: SavedSearch }) {
    const toggle = useStore((s) => s.toggleSavedSearch);
    return (
        <div className="group relative">
            <button onClick={() => void runSavedSearch(search)} title={`Search for “${search.query}” in ${search.folder || "every drive"}`} className={cn(PLACE, "pr-7")}>
                <Search className="size-4 shrink-0 text-muted-foreground" />
                <div className="min-w-0 flex-1">
                    <div className="truncate">{search.query}</div>
                    <div className="truncate text-xs text-muted-foreground">{search.folder === THIS_PC ? "All drives" : baseName(search.folder)}</div>
                </div>
            </button>
            <button
                onClick={() => toggle(search)}
                title="Remove from the sidebar"
                aria-label={`Remove the saved search “${search.query}”`}
                className="absolute right-1 top-1/2 -translate-y-1/2 rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:bg-background hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
            >
                <X className="size-3.5" />
            </button>
        </div>
    );
}

export function Sidebar({ onContextMenu }: { onContextMenu: (e: React.MouseEvent, path: string) => void }) {
    const path = useTab((t) => t.path);
    const { known, drives } = useExplorer(useShallow((s) => ({ known: s.known, drives: s.drives })));
    const pinned = useStore((s) => s.pinned);
    const saved = useStore((s) => s.savedSearches);
    const here = (p: string) => path !== THIS_PC && samePath(p, path);

    return (
        <nav aria-label="Places" className="hidden w-56 shrink-0 flex-col gap-4 overflow-y-auto border-r p-2 pt-3 md:flex">
            <Section title="Quick access">
                {known.map((k) => (
                    <Place key={k.id} icon={KNOWN_ICONS[k.id]} label={k.name} path={k.path} active={here(k.path)} onContextMenu={onContextMenu} />
                ))}
            </Section>
            {pinned.length > 0 && (
                <Section title="Pinned">
                    {pinned.map((p) => (
                        <Place key={p} icon={Folder} iconClass="fill-amber-400/80 text-amber-500" label={baseName(p)} path={p} active={here(p)} onContextMenu={onContextMenu} />
                    ))}
                </Section>
            )}
            {saved.length > 0 && (
                <Section title="Searches">
                    {saved.map((s) => (
                        <SavedSearchItem key={`${s.folder}\n${s.query}`} search={s} />
                    ))}
                </Section>
            )}
            <Section title="This PC" onTitleClick={() => navigate(THIS_PC)} active={path === THIS_PC}>
                {drives.map((d) => (
                    <Place key={d.path} icon={DRIVE_ICONS[d.kind]} label={driveName(d)} path={d.path} active={here(d.path)} title={freeOf(d) || d.path} onContextMenu={onContextMenu}>
                        <CapacityBar drive={d} className="mt-1" />
                    </Place>
                ))}
            </Section>
        </nav>
    );
}
