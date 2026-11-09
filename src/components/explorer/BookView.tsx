import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { AlertTriangle, ChevronLeft, ChevronRight, ListTree, Loader2, Minus, Plus, Square, Volume2 } from "lucide-react";
import type { FileEntry } from "@/lib/entries";
import { cn } from "@/lib/utils";

/** The e-books the reader opens (foliate-js, vendored in src/vendor/foliate). */
export const BOOK_EXTS = new Set(["epub", "mobi", "azw3", "azw", "fb2", "fbz", "cbz"]);

type TocItem = { label: string; href: string; subitems?: TocItem[] };
type FoliateView = HTMLElement & {
    open(file: File): Promise<void>;
    init(options: { lastLocation?: string; showTextStart?: boolean }): Promise<void>;
    close(): void;
    prev(): Promise<void>;
    next(): Promise<void>;
    goTo(target: string): Promise<void>;
    book: { toc?: TocItem[]; metadata?: { title?: unknown; language?: string | string[] }; dir?: string };
    renderer: HTMLElement & { setStyles?(css: string): void; getContents?(): { doc: Document }[] };
};

const PLACES = "glint-book-places";
const SIZE = "glint-book-size";

/** Where each book was left: its CFI, the last 200 books. */
function places(): Record<string, string> {
    try {
        return JSON.parse(localStorage.getItem(PLACES) ?? "{}");
    } catch {
        return {};
    }
}

function remember(path: string, cfi: string | undefined) {
    if (!cfi) return;
    try {
        const all = places();
        delete all[path.toLowerCase()];
        all[path.toLowerCase()] = cfi;
        const keys = Object.keys(all);
        for (const old of keys.slice(0, Math.max(0, keys.length - 200))) delete all[old];
        localStorage.setItem(PLACES, JSON.stringify(all));
    } catch {
        /* private storage: the book opens at its start next time */
    }
}

function startingSize() {
    try {
        return Number(localStorage.getItem(SIZE)) || 100;
    } catch {
        return 100;
    }
}

/** The book's pages styled for reading in Glint: its text size, and Glint's light or dark. */
function css(size: number, dark: boolean) {
    return `
        html { color-scheme: ${dark ? "dark" : "light"}; font-size: ${size}%; }
        ${dark ? "html, body { background: transparent !important; color: #e6e6e6 !important; } a:link { color: #8ab4f8; }" : ""}
        p, li, blockquote, dd { line-height: 1.5; hyphens: auto; }
        img, svg { max-width: 100%; height: auto; }
    `;
}

const titleOf = (title: unknown) => (typeof title === "string" ? title : title && typeof title === "object" ? String(Object.values(title as Record<string, string>)[0] ?? "") : "");

/** An e-book in the preview: its pages, contents, text size and the place you stopped at, and Read aloud with Windows' own voices. */
export default function BookView({ entry }: { entry: FileEntry }) {
    const box = useRef<HTMLDivElement>(null);
    const view = useRef<FoliateView | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [ready, setReady] = useState(false);
    const [title, setTitle] = useState("");
    const [toc, setToc] = useState<TocItem[]>([]);
    const [showToc, setShowToc] = useState(false);
    const [section, setSection] = useState("");
    const [fraction, setFraction] = useState(0);
    const [size, setSize] = useState(startingSize);
    const [speaking, setSpeaking] = useState(false);
    const dark = document.documentElement.classList.contains("dark");

    useEffect(() => {
        let live = true;
        let opened: FoliateView | null = null;
        setReady(false);
        setError(null);
        (async () => {
            await import("@/vendor/foliate/view.js");
            const response = await fetch(convertFileSrc(entry.path));
            const file = new File([await response.blob()], entry.name.toLowerCase());
            if (!live || !box.current) return;
            opened = document.createElement("foliate-view") as FoliateView;
            opened.style.cssText = "display:block;width:100%;height:100%";
            box.current.replaceChildren(opened);
            view.current = opened;
            await opened.open(file);
            if (!live) return;
            setTitle(titleOf(opened.book.metadata?.title));
            setToc(opened.book.toc ?? []);
            opened.renderer.setStyles?.(css(startingSize(), document.documentElement.classList.contains("dark")));
            opened.addEventListener("relocate", (e) => {
                const detail = (e as CustomEvent<{ fraction?: number; cfi?: string; tocItem?: { label?: string } }>).detail;
                setFraction(detail.fraction ?? 0);
                setSection(detail.tocItem?.label?.trim() ?? "");
                remember(entry.path, detail.cfi);
            });
            const last = places()[entry.path.toLowerCase()];
            await opened.init({ lastLocation: last, showTextStart: !last });
            if (live) setReady(true);
        })().catch((e) => {
            if (!live) return;
            const text = String(e?.message ?? e);
            setError(/not supported|PDF/.test(text) ? "Glint can't read this kind of book." : /encrypt|DRM/i.test(text) ? "This book has DRM, which only its store's app can open." : text);
        });
        return () => {
            live = false;
            speechSynthesis.cancel();
            try {
                opened?.close();
            } catch {
                /* already closed */
            }
            opened?.remove();
            view.current = null;
        };
    }, [entry.path, entry.modified]);

    useEffect(() => {
        view.current?.renderer.setStyles?.(css(size, dark));
        try {
            localStorage.setItem(SIZE, String(size));
        } catch {
            /* the size resets next time */
        }
    }, [size, dark]);

    // Read aloud: the pages on screen's chapter, with a voice on this PC in the book's language (Edge's online voices are left out).
    const readAloud = () => {
        if (speaking) {
            speechSynthesis.cancel();
            return setSpeaking(false);
        }
        const doc = view.current?.renderer.getContents?.()[0]?.doc;
        const text = doc?.body?.innerText?.trim();
        if (!text) return;
        const lang = [view.current?.book.metadata?.language].flat()[0] ?? doc?.documentElement.lang ?? "";
        const voices = speechSynthesis.getVoices().filter((v) => v.localService);
        const voice = voices.find((v) => lang && v.lang.toLowerCase().startsWith(lang.toLowerCase().slice(0, 2))) ?? voices[0];
        const utterance = new SpeechSynthesisUtterance(text.slice(0, 32_000));
        if (voice) utterance.voice = voice;
        utterance.onend = utterance.onerror = () => setSpeaking(false);
        speechSynthesis.cancel();
        speechSynthesis.speak(utterance);
        setSpeaking(true);
    };

    if (error)
        return (
            <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-sm">
                <AlertTriangle className="size-6 text-amber-500" />
                <p className="max-w-sm">{error}</p>
            </div>
        );
    return (
        <div
            className="flex h-full min-h-0 flex-col"
            tabIndex={0}
            onKeyDown={(e) => {
                if (e.key === "ArrowLeft" || e.key === "PageUp") void view.current?.prev();
                else if (e.key === "ArrowRight" || e.key === "PageDown" || e.key === " ") void view.current?.next();
                else return;
                e.preventDefault();
            }}
        >
            <div className="flex items-center gap-1 border-b px-2 py-1 text-xs">
                <button type="button" title="Contents" aria-label="Contents" disabled={!toc.length} onClick={() => setShowToc((s) => !s)} className={cn("rounded p-1 hover:bg-accent disabled:opacity-40", showToc && "bg-accent")}>
                    <ListTree className="size-4" />
                </button>
                <span className="min-w-0 flex-1 truncate text-muted-foreground" title={title}>
                    {section || title || entry.name}
                </span>
                <span className="tabular-nums text-muted-foreground">{Math.round(fraction * 100)}%</span>
                <button type="button" title="Smaller text" aria-label="Smaller text" onClick={() => setSize((s) => Math.max(70, s - 10))} className="rounded p-1 hover:bg-accent">
                    <Minus className="size-3.5" />
                </button>
                <button type="button" title="Bigger text" aria-label="Bigger text" onClick={() => setSize((s) => Math.min(200, s + 10))} className="rounded p-1 hover:bg-accent">
                    <Plus className="size-3.5" />
                </button>
                <button type="button" title={speaking ? "Stop reading aloud" : "Read this chapter aloud, with a voice on this PC"} aria-label={speaking ? "Stop reading aloud" : "Read aloud"} onClick={readAloud} className="rounded p-1 hover:bg-accent">
                    {speaking ? <Square className="size-3.5 fill-current" /> : <Volume2 className="size-4" />}
                </button>
            </div>
            <div className="relative flex min-h-0 flex-1">
                {showToc && (
                    <nav className="w-56 shrink-0 overflow-y-auto border-e p-1 text-xs" aria-label="Contents">
                        <Toc items={toc} onPick={(href) => void view.current?.goTo(href)} />
                    </nav>
                )}
                <button type="button" aria-label="Previous page" onClick={() => void view.current?.prev()} className="z-10 w-7 shrink-0 text-muted-foreground hover:bg-accent/50">
                    <ChevronLeft className="mx-auto size-4" />
                </button>
                <div ref={box} className="min-w-0 flex-1" />
                <button type="button" aria-label="Next page" onClick={() => void view.current?.next()} className="z-10 w-7 shrink-0 text-muted-foreground hover:bg-accent/50">
                    <ChevronRight className="mx-auto size-4" />
                </button>
                {!ready && (
                    <div className="absolute inset-0 flex items-center justify-center gap-2 text-sm text-muted-foreground">
                        <Loader2 className="size-4 animate-spin" /> Opening the book…
                    </div>
                )}
            </div>
        </div>
    );
}

function Toc({ items, onPick, depth = 0 }: { items: TocItem[]; onPick: (href: string) => void; depth?: number }) {
    return (
        <ul>
            {items.map((item, i) => (
                <li key={`${item.href}-${i}`}>
                    <button type="button" onClick={() => onPick(item.href)} className="block w-full truncate rounded px-2 py-1 text-start hover:bg-accent" style={{ paddingInlineStart: `${0.5 + depth * 0.75}rem` }} title={item.label}>
                        {item.label.trim()}
                    </button>
                    {item.subitems?.length ? <Toc items={item.subitems} onPick={onPick} depth={depth + 1} /> : null}
                </li>
            ))}
        </ul>
    );
}
