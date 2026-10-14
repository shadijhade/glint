import { Disc, Download, FileText, Film, Folder, HardDrive, House, Image, type LucideIcon, Monitor, Music, Server, Usb } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { type KnownFolder, navigate, useExplorer } from "@/lib/explorer";
import { type Drive, driveName } from "@/lib/fs";
import { formatSize } from "@/lib/format";
import { baseName, samePath, THIS_PC } from "@/lib/paths";
import { useStore } from "@/lib/store";
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
                    <button onClick={onTitleClick} className={cn("text-xs font-semibold uppercase tracking-wide text-muted-foreground hover:text-foreground", active && "text-primary")}>
                        {title}
                    </button>
                ) : (
                    <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">{title}</span>
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

function Place({ icon: Icon, iconClass, label, path, active, title, onContextMenu, children }: PlaceProps) {
    const dropping = useExplorer((s) => s.dropTarget === path);
    return (
        <button
            data-drop={path}
            onClick={() => navigate(path)}
            onContextMenu={(e) => onContextMenu(e, path)}
            title={title ?? path}
            aria-current={active ? "page" : undefined}
            className={cn(
                "flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left text-sm text-foreground/90 transition-colors hover:bg-accent",
                active && "bg-primary/10 text-foreground hover:bg-primary/15",
                dropping && "bg-primary/20 outline-2 -outline-offset-2 outline-primary"
            )}
        >
            <Icon className={cn("size-4 shrink-0 text-muted-foreground", active && "text-primary", iconClass)} />
            <div className="min-w-0 flex-1">
                <div className="truncate">{label}</div>
                {children}
            </div>
        </button>
    );
}

export function Sidebar({ onContextMenu }: { onContextMenu: (e: React.MouseEvent, path: string) => void }) {
    const { path, known, drives } = useExplorer(useShallow((s) => ({ path: s.path, known: s.known, drives: s.drives })));
    const pinned = useStore((s) => s.pinned);
    const here = (p: string) => path !== THIS_PC && samePath(p, path);

    return (
        <nav aria-label="Places" className="hidden w-56 shrink-0 flex-col gap-4 overflow-y-auto border-r bg-card/40 p-2 pt-3 md:flex">
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
