import { createElement, type CSSProperties, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ImageOff } from "lucide-react";
import { goTo } from "@/lib/explorer";
import { archiveExtract, markdownHtml } from "@/lib/fs";
import { useGit } from "@/lib/git";
import { languageNamed, SyntaxHighlighter } from "@/lib/highlight";
import { safeHref } from "@/lib/markdown";
import { linkedPath, parentPath, samePath, volumeOf } from "@/lib/paths";
import { cn } from "@/lib/utils";

/** Tags shown as such, about the ones GitHub allows in Markdown; any other shows only what's in it. */
const TAGS = new Set(
    "p div span h1 h2 h3 h4 h5 h6 ul ol li dl dt dd blockquote code kbd samp var em i strong b u s del ins mark small sub sup abbr cite dfn q time br hr wbr table caption thead tbody tfoot tr th td details summary figure figcaption section ruby rt rp".split(" ")
);
/** Old tags, as their current ones. */
const RENAMED: Record<string, string> = { tt: "code", strike: "del", center: "div" };
/** Tags dropped with what's in them: what could run, fetch or take input, and what isn't text. */
const DROPPED = new Set(
    "script style iframe frame frameset object embed applet form textarea select button option template noscript title meta link base svg math audio video canvas source track xmp noembed noframes plaintext".split(" ")
);
const EMPTY = new Set(["hr", "br", "wbr"]);
/** comrak's classes that the stylesheet uses. */
const CLASSES = new Set(["task-list-item", "contains-task-list", "footnotes", "footnote-ref", "footnote-backref"]);
/** Where the line breaks between tags aren't text (React refuses text there). */
const NO_TEXT = new Set(["table", "thead", "tbody", "tfoot", "tr"]);

interface Ctx {
    folder: string;
    root: string;
    /** The Markdown is in an archive: its pictures are extracted from it to be shown. */
    archived: boolean;
    follow: (href: string) => void;
}

/** A width or height as written on a picture: pixels or a percentage. */
const size = (value: string | null) => (value && /^\d+(\.\d+)?%?$/.test(value.trim()) ? value.trim() : undefined);

/**
 * A picture the Markdown shows: from beside it, on the same drive or share (one elsewhere could be a
 * server Windows would sign in to, telling it who you are), or written in it (data:). Pictures from
 * the web aren't fetched for a preview.
 */
function Picture({ src, alt, title, width, height, float, ctx }: { src: string; alt: string; title: string; width?: string; height?: string; float?: string; ctx: Ctx }) {
    const data = /^data:image\//i.test(src);
    const path = data ? null : linkedPath(src, ctx.folder, ctx.root);
    const near = !!path && volumeOf(path).toLowerCase() === volumeOf(ctx.folder).toLowerCase();
    const [url, setUrl] = useState<string | null>(data ? src : near && !ctx.archived ? convertFileSrc(path) : null);
    const [failed, setFailed] = useState(false);
    useEffect(() => {
        if (!ctx.archived || !near || !path) return;
        let stale = false;
        archiveExtract(path, "preview").then(
            (copy) => !stale && setUrl(convertFileSrc(copy)),
            () => !stale && setFailed(true)
        );
        return () => {
            stale = true;
        };
    }, [ctx.archived, near, path]);
    if (failed || (!url && !(ctx.archived && near)))
        return (
            <span
                className="md-missing"
                title={
                    /^https?:/i.test(src)
                        ? `A picture from the web, not loaded in previews: ${src}`
                        : path && !near
                          ? `A picture on another drive or share, not loaded in previews: ${path}`
                          : `Not found: ${path ?? src}`
                }
            >
                <ImageOff aria-hidden />
                {alt || "Picture"}
            </span>
        );
    if (!url) return <span className="md-missing">{alt || "Picture"}</span>;
    const style: CSSProperties | undefined = float ? { float: float as "left" | "right", margin: float === "left" ? "0 1em 0.5em 0" : "0 0 0.5em 1em" } : undefined;
    return <img src={url} alt={alt} title={title || undefined} width={width} height={height} style={style} draggable={false} onError={() => setFailed(true)} />;
}

/** One node of the HTML as an element, if it's an allowed tag, with only the allowed attributes. */
function build(node: ChildNode, key: number, ctx: Ctx): ReactNode {
    if (node.nodeType === Node.TEXT_NODE) {
        const parent = node.parentElement?.localName ?? "";
        return NO_TEXT.has(parent) && !node.textContent?.trim() ? null : node.textContent;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return null;
    const el = node as Element;
    const tag = RENAMED[el.localName] ?? el.localName;
    if (DROPPED.has(tag)) return null;
    const attr = (name: string) => el.getAttribute(name) ?? "";
    const kids = () => (el.childNodes.length ? Array.from(el.childNodes, (child, i) => build(child, i, ctx)) : undefined);
    // Ids (and old <a name>s) go in data-anchor, so they can't clash with the app's or between two
    // previews of one file.
    const id = el.id || (tag === "a" ? attr("name") : "");
    const anchor = id ? { "data-anchor": id } : {};
    const className = [...el.classList].filter((c) => CLASSES.has(c)).join(" ") || undefined;
    const align = /^(left|right|center|justify)$/i.exec(el.localName === "center" ? "center" : attr("align"))?.[0].toLowerCase();
    const title = attr("title") || undefined;
    switch (tag) {
        case "a": {
            // A heading's own "link to here".
            if (el.classList.contains("anchor") && el.hasAttribute("data-heading-content")) return null;
            const href = safeHref(attr("href"));
            const go = (e: React.MouseEvent) => {
                // The app's page never follows a link itself.
                e.preventDefault();
                if (e.button <= 1 && href) ctx.follow(href);
            };
            return (
                <a key={key} {...anchor} className={className} href={href ?? undefined} title={title ?? href ?? undefined} draggable={false} onClick={go} onAuxClick={go}>
                    {kids()}
                </a>
            );
        }
        case "img":
            // Keyed by its address too, so a picture changed in place loads anew.
            return (
                <Picture
                    key={`${key}:${attr("src")}`}
                    src={attr("src")}
                    alt={attr("alt")}
                    title={attr("title")}
                    width={size(el.getAttribute("width"))}
                    height={size(el.getAttribute("height"))}
                    float={align === "left" || align === "right" ? align : undefined}
                    ctx={ctx}
                />
            );
        case "picture":
            // Its <source>s are dropped: its <img> shows.
            return <span key={key}>{kids()}</span>;
        case "pre": {
            const code = el.querySelector("code");
            const language = /\blanguage-(\S+)/.exec(code?.className ?? "")?.[1];
            return (
                <SyntaxHighlighter key={key} language={language ? languageNamed(language) : "text"} useInlineStyles={false} PreTag="pre" CodeTag="code">
                    {(code ?? el).textContent?.replace(/\n$/, "") ?? ""}
                </SyntaxHighlighter>
            );
        }
        case "input":
            return attr("type").toLowerCase() === "checkbox" ? <input key={key} type="checkbox" checked={el.hasAttribute("checked")} disabled readOnly /> : null;
        case "ol":
            return (
                <ol key={key} start={Number(attr("start")) || undefined} className={className}>
                    {kids()}
                </ol>
            );
        case "details":
            return (
                <details key={key} open={el.hasAttribute("open")} {...anchor}>
                    {kids()}
                </details>
            );
        case "th":
        case "td":
            return createElement(
                tag,
                { key, title, colSpan: Number(attr("colspan")) || undefined, rowSpan: Number(attr("rowspan")) || undefined, style: align ? { textAlign: align } : undefined },
                kids()
            );
        default:
            if (!TAGS.has(tag)) return <span key={key}>{kids()}</span>;
            return createElement(tag, { key, ...anchor, className, title, style: align ? { textAlign: align } : undefined }, EMPTY.has(tag) ? undefined : kids());
    }
}

/** The repository a folder is in, from the Git view of a tab showing it. */
const useRepoRoot = (folder: string) => useGit((s) => Object.values(s).find((r) => r.view && samePath(r.folder, folder))?.view?.root);

/**
 * Markdown as it reads: comrak's HTML (GitHub's flavor), built into elements from an allowlist of tags
 * and attributes rather than put in the page, so nothing in a file can run. Links to the web open in
 * the browser, links to files and folders open here, and links within the file scroll to their heading.
 */
export function MarkdownView({ text, file, archived, className }: { text: string; file: string; archived: boolean; className?: string }) {
    const [html, setHtml] = useState<string | { error: string } | null>(null);
    useEffect(() => {
        let stale = false;
        markdownHtml(text).then(
            (result) => !stale && setHtml(result),
            (e) => !stale && setHtml({ error: String(e) })
        );
        return () => {
            stale = true;
        };
    }, [text]);
    const box = useRef<HTMLDivElement>(null);
    const folder = parentPath(file);
    const repo = useRepoRoot(folder);
    const root = repo ?? `${volumeOf(file)}\\`;
    const content = useMemo(() => {
        if (typeof html !== "string") return null;
        const ctx: Ctx = {
            folder,
            root,
            archived,
            follow(href) {
                if (href.startsWith("#")) {
                    let id = href.slice(1);
                    try {
                        id = decodeURIComponent(id);
                    } catch {
                        // As written.
                    }
                    const at = (name: string) => box.current?.querySelector(`[data-anchor="${CSS.escape(name)}"]`);
                    // GitHub's own links to a heading carry its "user-content-" prefix.
                    (at(id) ?? at(id.replace(/^user-content-/, "")) ?? at(id.toLowerCase()))?.scrollIntoView({ block: "start" });
                } else if (/^(https?|mailto):/i.test(href)) void openUrl(href);
                else {
                    const path = linkedPath(href, folder, root);
                    if (path) void goTo(path);
                }
            },
        };
        const body = new DOMParser().parseFromString(html, "text/html").body;
        return Array.from(body.childNodes, (node, i) => build(node, i, ctx));
    }, [html, folder, root, archived]);
    if (html && typeof html !== "string") return <p className="text-sm text-muted-foreground">{html.error}</p>;
    return (
        <div ref={box} className={cn("markdown", className)}>
            {content}
        </div>
    );
}
