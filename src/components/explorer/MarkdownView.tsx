import { createElement, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ImageOff } from "lucide-react";
import { goTo } from "@/lib/explorer";
import { archiveExtract, markdownHtml } from "@/lib/fs";
import { useGit } from "@/lib/git";
import { languageNamed, SyntaxHighlighter } from "@/lib/highlight";
import { linkedPath, parentPath, samePath, volumeOf } from "@/lib/paths";
import { cn } from "@/lib/utils";

/** Tags comrak writes that are shown as such; any other shows only what's in it. */
const TAGS = new Set(["p", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "blockquote", "code", "em", "strong", "del", "table", "thead", "tbody", "tr", "th", "td", "hr", "br", "sup", "section"]);
const EMPTY = new Set(["hr", "br"]);
/** comrak's classes that the stylesheet uses. */
const CLASSES = new Set(["task-list-item", "contains-task-list", "footnotes", "footnote-ref", "footnote-backref"]);
/** Where the line breaks between tags aren't text (React refuses text there). */
const NO_TEXT = new Set(["table", "thead", "tbody", "tr"]);

interface Ctx {
    folder: string;
    root: string;
    /** The Markdown is in an archive: its pictures are extracted from it to be shown. */
    archived: boolean;
    follow: (href: string) => void;
}

/**
 * A picture the Markdown shows: from beside it, on the same drive or share (one elsewhere could be a
 * server Windows would sign in to, telling it who you are), or written in it (data:). Pictures from
 * the web aren't fetched for a preview.
 */
function Picture({ src, alt, title, ctx }: { src: string; alt: string; title: string; ctx: Ctx }) {
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
    return <img src={url} alt={alt} title={title || undefined} draggable={false} onError={() => setFailed(true)} />;
}

/** One node of comrak's HTML as an element, if it's one of the allowed tags with the allowed attributes. */
function build(node: ChildNode, key: number, ctx: Ctx): ReactNode {
    if (node.nodeType === Node.TEXT_NODE) {
        const parent = node.parentElement?.localName ?? "";
        return NO_TEXT.has(parent) && !node.textContent?.trim() ? null : node.textContent;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return null;
    const el = node as Element;
    const tag = el.localName;
    const attr = (name: string) => el.getAttribute(name) ?? "";
    const kids = () => (el.childNodes.length ? Array.from(el.childNodes, (child, i) => build(child, i, ctx)) : undefined);
    // Ids go in data-anchor, so two previews of one file (or an id like "root") don't clash with the app's.
    const anchor = el.id ? { "data-anchor": el.id } : {};
    const className = [...el.classList].filter((c) => CLASSES.has(c)).join(" ") || undefined;
    switch (tag) {
        case "a": {
            // A heading's own "link to here".
            if (el.classList.contains("anchor")) return null;
            const href = attr("href");
            const go = (e: React.MouseEvent) => {
                // The app's page never follows a link itself.
                e.preventDefault();
                if (e.button <= 1 && href) ctx.follow(href);
            };
            return (
                <a key={key} {...anchor} className={className} href={href || undefined} title={attr("title") || href || undefined} draggable={false} onClick={go} onAuxClick={go}>
                    {kids()}
                </a>
            );
        }
        case "img":
            // Keyed by its address too, so a picture changed in place loads anew.
            return <Picture key={`${key}:${attr("src")}`} src={attr("src")} alt={attr("alt")} title={attr("title")} ctx={ctx} />;
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
            return <input key={key} type="checkbox" checked={el.hasAttribute("checked")} disabled readOnly />;
        case "ol":
            return (
                <ol key={key} start={Number(attr("start")) || undefined} className={className}>
                    {kids()}
                </ol>
            );
        case "th":
        case "td": {
            const align = attr("align");
            return createElement(tag, { key, style: /^(left|right|center)$/.test(align) ? { textAlign: align } : undefined }, kids());
        }
        default:
            if (!TAGS.has(tag)) return <span key={key}>{kids()}</span>;
            return createElement(tag, { key, ...anchor, className }, EMPTY.has(tag) ? undefined : kids());
    }
}

/** The repository a folder is in, from the Git view of a tab showing it. */
const useRepoRoot = (folder: string) => useGit((s) => Object.values(s).find((r) => r.view && samePath(r.folder, folder))?.view?.root);

/**
 * Markdown as it reads: comrak's HTML (GitHub's flavor, raw HTML shown as text), built into elements
 * from an allowlist rather than put in the page, so nothing in a file can run. Links to the web open in
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
                    box.current?.querySelector(`[data-anchor="${CSS.escape(id)}"]`)?.scrollIntoView({ block: "start" });
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
