import { useEffect, useRef, useState } from "react";
import type { FileEntry } from "@/lib/entries";
import { nativePreviewClose, nativePreviewPlace, nativePreviewShow, type PixelRect } from "@/lib/fs";

/** What can cover the preview: menus, dialogs, the palette, toasts, dropdowns. */
const OVERLAYS = ':popover-open, dialog[open], [aria-modal="true"], [data-sonner-toast], [data-overlay]';

/** A preview that wants Windows' previewer window; Quick Look's (`over`) win over the pane's, then the newest. */
interface Claim {
    over: boolean;
    start: () => void;
    stop: () => void;
}
const claims: Claim[] = [];
let current: Claim | undefined;
let ids = 0;

/** Hands the window to the preview that should have it now. */
function reassign() {
    const ranked = [...claims].sort((a, b) => Number(a.over) - Number(b.over));
    const next = ranked[ranked.length - 1];
    if (next === current) return;
    current?.stop();
    current = next;
    current?.start();
}

/**
 * A file Glint can't show itself, as Windows' own previewer for its type shows it (Word, Excel and
 * PowerPoint documents, fonts…): drawn by that previewer in a window over this box (native_preview.rs),
 * hidden while a menu, dialog or toast covers it. Without one, or where it's blocked, `children` (the
 * icon) stays, with the reason (unless `quiet`: the children show something of their own).
 */
export function ShellPreview({ entry, over, quiet, children }: { entry: FileEntry; over?: boolean; quiet?: boolean; children: React.ReactNode }) {
    const box = useRef<HTMLDivElement>(null);
    const [note, setNote] = useState("");

    useEffect(() => {
        const id = ++ids;
        let frame = 0;
        let last = "";
        // Where the box is, in the window's physical pixels; null when it's out of sight or covered.
        const place = (): PixelRect | null => {
            const el = box.current;
            const r = el?.getBoundingClientRect();
            if (!el || !r || r.width < 8 || r.height < 8) return null;
            for (const o of document.querySelectorAll<HTMLElement>(OVERLAYS)) {
                if (o.contains(el)) continue;
                const b = o.getBoundingClientRect();
                if (b.width && b.height && b.left < r.right && b.right > r.left && b.top < r.bottom && b.bottom > r.top) return null;
            }
            const d = devicePixelRatio;
            const [x, y] = [Math.round(r.left * d), Math.round(r.top * d)];
            return { x, y, width: Math.round(r.right * d) - x, height: Math.round(r.bottom * d) - y };
        };
        const follow = () => {
            const at = place();
            const key = JSON.stringify(at);
            if (key !== last) {
                last = key;
                void nativePreviewPlace(id, at);
            }
            frame = requestAnimationFrame(follow);
        };
        const claim: Claim = {
            over: !!over,
            start: () => {
                const at = place();
                last = JSON.stringify(at);
                const dark = document.documentElement.classList.contains("dark");
                void nativePreviewShow(id, entry.path, at, dark, (event) => setNote(event.type === "unavailable" ? event.reason : ""));
                frame = requestAnimationFrame(follow);
            },
            stop: () => {
                cancelAnimationFrame(frame);
                void nativePreviewClose(id);
            },
        };
        claims.push(claim);
        reassign();
        return () => {
            claims.splice(claims.indexOf(claim), 1);
            if (current === claim) {
                claim.stop();
                current = undefined;
            }
            reassign();
        };
    }, [entry.path, entry.modified, over]);

    return (
        <div ref={box} className="flex h-full flex-col">
            <div className="min-h-0 flex-1">{children}</div>
            {note && !quiet && <p className="px-6 pb-6 text-center text-sm text-muted-foreground">{note}</p>}
        </div>
    );
}
