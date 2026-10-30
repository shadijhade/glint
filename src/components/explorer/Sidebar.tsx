import { Database, Disc, Download, FileText, Film, Folder, HardDrive, House, Image, type LucideIcon, Monitor, Music, Plus, Search, Server, SquareTerminal, Trash2, Unplug, Usb, X } from "lucide-react";
import { toast } from "sonner";
import { useEffect, useState } from "react";
import { type CloudSpace, rcloneAbout } from "@/lib/rclone";
import { remotePath, remoteWhere } from "@/lib/remotes";
import { serviceIcon } from "./CloudSetup";
import { addRemote } from "./RemoteDialog";
import { useShallow } from "zustand/react/shallow";
import { type KnownFolder, middleClickOpens, navigate, refreshDrives, runSavedSearch, useExplorer, useTab } from "@/lib/explorer";
import { type Drive, driveName, ejectDrive } from "@/lib/fs";
import { formatSize } from "@/lib/format";
import { baseName, inArchivePath, isRemotePath, RECYCLE_BIN, samePath, THIS_PC } from "@/lib/paths";
import { type RemoteConnection, type SavedSearch, useStore } from "@/lib/store";
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
    wsl: SquareTerminal,
    unknown: HardDrive,
};

/** Used-space bar, red when nearly full like Explorer's. */
export function CapacityBar({ drive, className }: { drive: Pick<Drive, "total" | "free">; className?: string }) {
    if (!drive.total) return null;
    const used = (drive.total - drive.free) / drive.total;
    return (
        <div className={cn("h-1 overflow-hidden rounded-full bg-muted", className)}>
            <div className={cn("h-full rounded-full", used > 0.9 ? "bg-destructive" : "bg-primary/70")} style={{ width: `${Math.round(used * 100)}%` }} />
        </div>
    );
}

export const freeOf = (drive: Pick<Drive, "total" | "free">) => (drive.total ? `${formatSize(drive.free)} free of ${formatSize(drive.total)}` : "");

/** How full each cloud is, as its service last said (rclone's about), by connection: asked again after ten minutes. */
const spaces = new Map<string, { at: number; space: CloudSpace | null }>();

/** How full a cloud of rclone's is, where its service says: its total and what's free. */
function useCloudSpace(r: RemoteConnection) {
    const [space, setSpace] = useState(() => spaces.get(r.id)?.space ?? null);
    useEffect(() => {
        if (r.kind !== "rclone" || !r.rclone) return;
        const known = spaces.get(r.id);
        if (known && Date.now() - known.at < 10 * 60_000) return setSpace(known.space);
        let live = true;
        rcloneAbout(r.rclone, r.root).then(
            (found) => {
                spaces.set(r.id, { at: Date.now(), space: found });
                if (live) setSpace(found);
            },
            () => {}
        );
        return () => {
            live = false;
        };
    }, [r.id, r.kind, r.rclone, r.root]);
    if (!space?.total) return null;
    return { total: space.total, free: space.free ?? Math.max(0, space.total - (space.used ?? 0)) };
}

/** A remote connection in the sidebar: a cloud with how full it is, where its service says. */
function RemotePlace({ remote, active, onContextMenu }: { remote: RemoteConnection; active: boolean; onContextMenu: (e: React.MouseEvent, path: string) => void }) {
    const space = useCloudSpace(remote);
    const icon = remote.kind === "rclone" ? serviceIcon(remote.service ?? "") : remote.kind === "s3" ? Database : remote.kind === "sftp" ? SquareTerminal : Server;
    const where = remoteWhere(remote);
    return (
        <Place icon={icon} label={remote.name} path={remotePath(remote)} active={active} title={space ? `${where}\n${freeOf(space)}` : where} onContextMenu={onContextMenu}>
            {space && <CapacityBar drive={space} className="mt-1" />}
        </Place>
    );
}

function Section({ title, onTitleClick, active, children }: { title: string; onTitleClick?: () => void; active?: boolean; children: React.ReactNode }) {
    return (
        <section>
            <h2 className="mb-1 hidden px-2 md:block">
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

// Below md, icons only: the sidebar is a rail, each place's name in its tooltip.
const PLACE = "relative flex w-full items-center justify-center gap-2.5 rounded-md px-2 py-1.5 text-left text-sm text-foreground/90 transition-all duration-100 hover:bg-accent active:scale-[0.985] md:justify-start";

// Windows 11's navigation selection: a gray fill and a short accent pill at the left edge.
const ACTIVE = "bg-accent text-foreground before:absolute before:left-0 before:top-1/2 before:h-4 before:w-[3px] before:-translate-y-1/2 before:rounded-full before:bg-primary";

function Place({ icon: Icon, iconClass, label, path, active, title, onContextMenu, children }: PlaceProps) {
    const dropping = useExplorer((s) => s.dropTarget === path);
    return (
        <button
            // A pinned folder in an archive takes no drops, nor does a remote's: they're read-only.
            data-drop={inArchivePath(path) || isRemotePath(path) ? undefined : path}
            onClick={() => navigate(path)}
            onMouseDown={middleClickOpens(path)}
            onContextMenu={(e) => onContextMenu(e, path)}
            title={`${label}\n${title ?? path}`}
            aria-label={label}
            aria-current={active ? "page" : undefined}
            className={cn(
                PLACE,
                active && ACTIVE,
                dropping && "scale-[1.01] bg-primary/20 outline-2 -outline-offset-2 outline-primary"
            )}
        >
            <Icon className={cn("size-4 shrink-0 text-muted-foreground", active && "text-foreground", iconClass)} />
            <div className="hidden min-w-0 flex-1 md:block">
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
            <button onClick={() => void runSavedSearch(search)} title={`Search for “${search.query}” in ${search.folder || "every drive"}`} className={cn(PLACE, "md:pr-7")}>
                <Search className="size-4 shrink-0 text-muted-foreground" />
                <div className="hidden min-w-0 flex-1 md:block">
                    <div className="truncate">{search.query}</div>
                    <div className="truncate text-xs text-muted-foreground">{search.folder === THIS_PC ? "All drives" : baseName(search.folder)}</div>
                </div>
            </button>
            <button
                onClick={() => toggle(search)}
                title="Remove from the sidebar"
                aria-label={`Remove the saved search “${search.query}”`}
                className="absolute right-1 top-1/2 hidden -translate-y-1/2 rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:bg-background hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100 md:block"
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
    const remotes = useStore((s) => s.remotes);
    const here = (p: string) => path !== THIS_PC && samePath(p, path);

    return (
        <nav aria-label="Places" className="flex w-12 shrink-0 flex-col gap-4 overflow-y-auto border-r p-1.5 pt-3 md:w-56 md:p-2 md:pt-3">
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
                {/* The rail's way to This PC, where the section's title isn't shown. */}
                <button
                    onClick={() => navigate(THIS_PC)}
                    title="This PC"
                    aria-label="This PC"
                    aria-current={path === THIS_PC ? "page" : undefined}
                    className={cn(PLACE, "md:hidden", path === THIS_PC && ACTIVE)}
                >
                    <Monitor className="size-4 shrink-0 text-muted-foreground" />
                </button>
                {drives.map((d) => {
                    const ejectable = d.kind === "removable" || d.kind === "cdrom";
                    return (
                        <div key={d.path} className="group/drive relative">
                            <Place
                                icon={DRIVE_ICONS[d.kind]}
                                iconClass={d.kind === "wsl" ? "text-amber-500" : undefined}
                                label={driveName(d)}
                                path={d.path}
                                active={here(d.path)}
                                title={freeOf(d) || d.path}
                                onContextMenu={onContextMenu}
                            >
                                <CapacityBar drive={d} className="mt-1" />
                            </Place>
                            {ejectable && (
                                <button
                                    type="button"
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        ejectDrive(d.path)
                                            .then(() => {
                                                toast.success(`Safely ejected ${driveName(d)}`);
                                                void refreshDrives();
                                            })
                                            .catch((err) => toast.error(`Couldn't eject ${driveName(d)}`, { description: String(err) }));
                                    }}
                                    title={`Safely eject ${driveName(d)}`}
                                    aria-label={`Eject ${driveName(d)}`}
                                    className="absolute right-1 top-1.5 hidden rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:bg-background hover:text-foreground group-hover/drive:opacity-100 md:block"
                                >
                                    <Unplug className="size-3.5" />
                                </button>
                            )}
                        </div>
                    );
                })}
                <Place
                    icon={Trash2}
                    label="Recycle Bin"
                    path={RECYCLE_BIN}
                    active={here(RECYCLE_BIN)}
                    title="What you deleted, to restore or delete for good. Files dropped here are deleted."
                    onContextMenu={onContextMenu}
                />
            </Section>
            <Section title="Remote">
                {remotes.map((r) => (
                    <RemotePlace key={r.id} remote={r} active={here(remotePath(r))} onContextMenu={onContextMenu} />
                ))}
                <button
                    onClick={() => addRemote()}
                    title="Add a remote connection: a cloud (Google Drive, OneDrive, Dropbox and 60 more), an S3 bucket, a WebDAV server or an SFTP server"
                    aria-label="Add a remote connection"
                    className={cn(PLACE, "text-muted-foreground")}
                >
                    <Plus className="size-4 shrink-0" />
                    <span className="hidden truncate md:inline">Add a connection</span>
                </button>
            </Section>
        </nav>
    );
}
