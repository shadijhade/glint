import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { FileEntry } from "@/lib/entries";
import { Waiting } from "./PreviewState";

/** Narrower than this, a page would be too small to read scaled down, so the text flows to the pane's width instead. */
const NARROW = 520;

/**
 * Around each page, and the pages as paper. A document that doesn't say its page size (some converters
 * leave it out) gets Word's: A4 with 2.54 cm margins. Reflowed, the text keeps small margins only.
 */
const FRAME = `
:host { display: block; }
.docx-wrapper { background: transparent !important; padding: 12px 0 !important; align-items: center; }
.docx-wrapper > section.docx { box-shadow: 0 1px 4px rgba(0,0,0,.25) !important; margin-bottom: 12px !important; }
.docx-wrapper > section.docx:not([style]) { width: 21cm; min-height: 29.7cm; padding: 2.54cm; box-sizing: border-box; }
.flow .docx-wrapper { padding: 0 !important; }
.flow .docx-wrapper > section.docx { width: auto !important; min-height: 0 !important; padding: 16px 20px !important; margin: 0 !important; box-shadow: none !important; }
`;

/**
 * A Word document laid out (docx-preview), without Office: as pages scaled to the pane's width, or as
 * text flowing to it when the pane is narrow. In a shadow root, so its styles stay its own. Nothing
 * loads from the internet (the window's rules block it), embedded HTML isn't shown, and links open in
 * the browser.
 */
export default function DocxView({ entry, fallback }: { entry: FileEntry; fallback: React.ReactNode }) {
    const host = useRef<HTMLDivElement>(null);
    const [state, setState] = useState<"loading" | "done" | "failed">("loading");
    const [narrow, setNarrow] = useState<boolean | null>(null);

    // Pages or flowing text, by the pane's width (laid out again when it crosses over).
    useEffect(() => {
        const box = host.current!;
        const measure = () => setNarrow(box.clientWidth > 0 && box.clientWidth < NARROW);
        measure();
        const resize = new ResizeObserver(measure);
        resize.observe(box);
        return () => resize.disconnect();
    }, []);

    useEffect(() => {
        if (narrow === null) return;
        let stale = false;
        const box = host.current!;
        const root = box.shadowRoot ?? box.attachShadow({ mode: "open" });
        root.replaceChildren();
        const frame = document.createElement("style");
        frame.textContent = FRAME;
        const styles = document.createElement("div");
        const body = document.createElement("div");
        if (narrow) body.className = "flow";
        root.append(frame, styles, body);
        setState("loading");

        // Pages are as wide as on paper; a narrower pane sees them scaled down.
        const fit = () => {
            const wrapper = body.querySelector<HTMLElement>(".docx-wrapper");
            const page = body.querySelector<HTMLElement>("section.docx");
            if (narrow || !wrapper || !page) return;
            wrapper.style.zoom = "";
            const scale = Math.min(1, (box.clientWidth - 24) / page.offsetWidth);
            if (scale > 0 && scale < 1) wrapper.style.zoom = String(scale);
        };
        const resize = new ResizeObserver(fit);
        resize.observe(box);

        (async () => {
            try {
                const [{ renderAsync }, data] = await Promise.all([
                    import("docx-preview"),
                    fetch(convertFileSrc(entry.path)).then((r) => {
                        if (!r.ok) throw new Error(r.statusText);
                        return r.arrayBuffer();
                    }),
                ]);
                if (stale) return;
                await renderAsync(data, body, styles, {
                    className: "docx",
                    inWrapper: true,
                    breakPages: !narrow,
                    ignoreWidth: narrow,
                    ignoreHeight: narrow,
                    ignoreLastRenderedPageBreak: true,
                    useBase64URL: true,
                    renderHeaders: !narrow,
                    renderFooters: !narrow,
                    renderFootnotes: true,
                    renderEndnotes: true,
                    renderComments: false,
                    renderChanges: false,
                    // HTML embedded in a document isn't shown.
                    renderAltChunks: false,
                });
                if (stale) return;
                fit();
                setState("done");
            } catch {
                if (!stale) setState("failed");
            }
        })();

        const onClick = (e: Event) => {
            const link = (e.target as Element | null)?.closest?.("a");
            if (!link) return;
            e.preventDefault();
            const href = link.getAttribute("href") ?? "";
            if (href.startsWith("#")) root.getElementById(decodeURIComponent(href.slice(1)))?.scrollIntoView({ block: "start" });
            else if (/^(https?|mailto):/i.test(href)) void openUrl(href);
        };
        root.addEventListener("click", onClick);
        return () => {
            stale = true;
            resize.disconnect();
            root.removeEventListener("click", onClick);
        };
    }, [entry.path, entry.modified, narrow]);

    if (state === "failed") return <>{fallback}</>;
    return (
        <div className="relative min-h-full select-text bg-background">
            {state === "loading" && <Waiting what="Laying out the document" />}
            <div ref={host} />
        </div>
    );
}
