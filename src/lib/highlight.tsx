import { type ComponentProps, lazy, memo, Suspense } from "react";
import type Prism from "react-syntax-highlighter/dist/esm/prism-light";

// Only these grammars are bundled (highlighter.ts); anything else previews as plain text.
export const LANGUAGE_NAMES = ["bash", "c", "cpp", "csharp", "css", "go", "java", "javascript", "json", "jsx", "markdown", "markup", "powershell", "python", "rust", "sql", "toml", "tsx", "typescript", "yaml"] as const;
type Language = (typeof LANGUAGE_NAMES)[number];

const LANGUAGE_BY_EXT: Record<string, Language> = {
    sh: "bash", bash: "bash", c: "c", h: "c", cc: "cpp", cpp: "cpp", hpp: "cpp", cs: "csharp", css: "css", scss: "css",
    go: "go", java: "java", js: "javascript", mjs: "javascript", cjs: "javascript", json: "json", jsx: "jsx",
    md: "markdown", html: "markup", htm: "markup", xml: "markup", svg: "markup", xaml: "markup", csproj: "markup",
    vue: "markup", ps1: "powershell", py: "python", rs: "rust", sql: "sql", toml: "toml", tsx: "tsx", ts: "typescript",
    mts: "typescript", yml: "yaml", yaml: "yaml",
};

export const languageFor = (ext: string) => LANGUAGE_BY_EXT[ext] ?? "text";

const ALIASES: Record<string, Language> = { "c++": "cpp", "c#": "csharp", shell: "bash", zsh: "bash", console: "bash", pwsh: "powershell", ps: "powershell" };

/** A code block's language as Markdown names it (```rust, ```ts, ```c++…). */
export function languageNamed(name: string) {
    const lower = name.toLowerCase();
    return (LANGUAGE_NAMES as readonly string[]).includes(lower) ? lower : (ALIASES[lower] ?? languageFor(lower));
}

// Prism and its grammars (96 kB) load the first time code is highlighted, not with the app.
const Highlighter = lazy(() => import("./highlighter"));

type Props = ComponentProps<typeof Prism>;

/** The same code shown the same way: an object of line props by what it holds, as a preview makes a new one each render. */
const same = (a: Props, b: Props) =>
    (Object.keys({ ...a, ...b }) as (keyof Props)[]).every((key) =>
        key === "lineProps" && typeof a.lineProps === "object" && typeof b.lineProps === "object" ? JSON.stringify(a.lineProps) === JSON.stringify(b.lineProps) : a[key] === b[key]
    );

/**
 * react-syntax-highlighter's Prism, as before. Until it has loaded (a moment, once) the code shows in the
 * same place as plain text. Drawn again only when the code or how it's shown changes: Prism goes through
 * every line of it (up to 2,000) each time, and a preview is drawn again with each key the list moves by.
 */
export const SyntaxHighlighter = memo(function SyntaxHighlighter({ children, ...props }: Props) {
    return (
        <Suspense fallback={<pre className={props.className}>{children}</pre>}>
            <Highlighter {...props}>{children}</Highlighter>
        </Suspense>
    );
}, same);
