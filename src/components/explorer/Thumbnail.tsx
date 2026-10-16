import { useState } from "react";
import type { FileEntry } from "@/lib/entries";
import { kindOf } from "@/lib/fileTypes";
import { thumbStep } from "@/lib/grid";
import { useThumbnail } from "@/lib/thumbs";
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
}

/** A file's thumbnail from Windows' thumbnail cache, or its type icon. */
export function Thumbnail({ entry, box, lane, enabled, bottom, iconClassName }: ThumbnailProps) {
    const url = useThumbnail(entry, thumbStep(box, devicePixelRatio), lane, enabled);
    // The icon stays until the picture is decoded, so it never blinks out first. The picture is drawn
    // one device pixel to one: a small one isn't blown up.
    const [shown, setShown] = useState<{ url: string; width: number; height: number } | null>(null);
    const drawn = url && shown?.url === url ? shown : null;
    const kind = kindOf(entry);
    return (
        <>
            {!drawn && <kind.icon className={cn("shrink-0", kind.tone, iconClassName)} />}
            {url && (
                <img
                    src={url}
                    alt=""
                    draggable={false}
                    onLoad={(e) => {
                        const img = e.currentTarget;
                        const [width, height] = [img.naturalWidth / devicePixelRatio, img.naturalHeight / devicePixelRatio];
                        const fit = Math.min(1, box / width, box / height);
                        img.decode()
                            .catch(() => {})
                            .finally(() => setShown({ url, width: width * fit, height: height * fit }));
                    }}
                    style={drawn ? { width: drawn.width, height: drawn.height } : undefined}
                    className={cn(
                        "pointer-events-none absolute inset-x-0 mx-auto max-h-full max-w-full",
                        bottom ? "bottom-0 rounded-[3px] shadow-sm" : "inset-y-0 my-auto",
                        !drawn && "invisible"
                    )}
                />
            )}
        </>
    );
}
