import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { type FileEntry, isArchived } from "@/lib/entries";
import { kindOf } from "@/lib/fileTypes";
import { thumbStep } from "@/lib/grid";
import { isHeld, useThumbnail } from "@/lib/thumbs";
import { cn } from "@/lib/utils";

interface ThumbnailProps {
    entry: FileEntry;
    /** The square it fits in, CSS pixels: its parent, which is `relative`. */
    box: number;
    /** The tab it's shown in. */
    lane: string;
    /** Ask Windows for a thumbnail; otherwise (and until one is in, or if there's none) the file's icon. */
    enabled: boolean;
    /** Sits on the bottom of its square, as in Explorer, so names line up under pictures of any shape. */
    bottom?: boolean;
    iconClassName?: string;
    /** The icon's size in CSS pixels, when it's not set by `iconClassName`. */
    iconSize?: number;
}

/** A thumbnail once decoded: its file, and its natural size in CSS pixels for the square it was asked for. */
interface Picture {
    path: string;
    url: string;
    width: number;
    height: number;
    box: number;
}

/** The latest picture decoded of each file, at whatever size: shown while a tile's new size comes in. */
const latest = new Map<string, Picture>();
function decoded(picture: Picture) {
    latest.delete(picture.path);
    latest.set(picture.path, picture);
    if (latest.size > 5000) latest.delete(latest.keys().next().value!);
}

/**
 * How big to draw `picture` in a `box` square: one device pixel to one, so a small one isn't blown up,
 * except a stand-in from another size that filled its square: it fills this one, as the new one will.
 */
function fit(picture: Picture, box: number, standIn: boolean) {
    const long = Math.max(picture.width, picture.height);
    const scale = standIn && long >= picture.box - 2 ? box / long : Math.min(1, box / picture.width, box / picture.height);
    return { width: picture.width * scale, height: picture.height * scale };
}

/** An SVG the page draws itself, up to this size. */
const MAX_SVG = 4 << 20;

/**
 * A file's thumbnail from Windows' thumbnail cache, or its type icon. An SVG is drawn by the page itself,
 * crisp at any size, whatever thumbnail handler Windows has for SVG (none, as it comes): as a picture,
 * an SVG in an <img> runs nothing and loads nothing else.
 */
export function Thumbnail({ entry, box, lane, enabled, bottom, iconClassName, iconSize }: ThumbnailProps) {
    const vector = enabled && entry.ext === "svg" && !isArchived(entry) && entry.size <= MAX_SVG;
    const fromWindows = useThumbnail(entry, thumbStep(box, devicePixelRatio), lane, enabled && !vector);
    const url = vector ? convertFileSrc(entry.path) : fromWindows;
    // The icon stays until the picture is decoded, so it never blinks out first: decoded here, or by any
    // tile (the browser has it at hand then, as when a zoom or a resize makes the tiles anew). This tile
    // keeps its own, whatever the other pane decodes of the file meanwhile.
    const [own, setOwn] = useState<Picture | null>(null);
    const mine = own?.path === entry.path ? own : undefined;
    const last = enabled ? latest.get(entry.path) : undefined;
    const drawn = url ? ([mine, last].find((p) => p?.url === url) ?? null) : null;
    if (drawn && drawn !== own) setOwn(drawn);
    // Until this size's is in and decoded, the picture of another size (zoomed from), unless it's far smaller.
    const prior = !drawn && url !== null ? (mine ?? last) : undefined;
    const standIn = prior && prior.box * 4 >= box && isHeld(prior.url) ? prior : null;
    const kind = kindOf(entry);
    const place = cn("pointer-events-none absolute inset-x-0 mx-auto max-h-full max-w-full", bottom ? "bottom-0 rounded-[3px] shadow-sm" : "inset-y-0 my-auto");
    return (
        <>
            {!drawn && !standIn && <kind.icon className={cn("shrink-0", kind.tone, iconClassName)} style={iconSize ? { width: iconSize, height: iconSize } : undefined} />}
            {standIn && <img key={standIn.url} src={standIn.url} alt="" draggable={false} style={fit(standIn, box, true)} className={place} />}
            {url && (
                <img
                    key={url}
                    src={url}
                    alt=""
                    draggable={false}
                    onLoad={(e) => {
                        if (drawn) return;
                        const img = e.currentTarget;
                        // A drawing has no pixels of its own to keep to: it fills its square, its shape kept.
                        const ratio = img.naturalWidth / img.naturalHeight || 1;
                        const size = vector ? (ratio >= 1 ? { width: box, height: box / ratio } : { width: box * ratio, height: box }) : { width: img.naturalWidth / devicePixelRatio, height: img.naturalHeight / devicePixelRatio };
                        const picture = { path: entry.path, url, ...size, box };
                        img.decode()
                            .catch(() => {})
                            .finally(() => {
                                decoded(picture);
                                setOwn(picture);
                            });
                    }}
                    onError={() => vector && setOwn(null)}
                    style={drawn ? fit(drawn, box, false) : undefined}
                    className={cn(place, !drawn && "invisible")}
                />
            )}
        </>
    );
}
