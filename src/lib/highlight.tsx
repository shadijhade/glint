import { type ComponentProps, lazy, Suspense } from "react";
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

/** react-syntax-highlighter's Prism, as before. Until it has loaded (a moment, once) the code shows in
 *  the same place as plain text. */
export function SyntaxHighlighter({ children, ...props }: ComponentProps<typeof Prism>) {
    return (
        <Suspense fallback={<pre className={props.className}>{children}</pre>}>
            <Highlighter {...props}>{children}</Highlighter>
        </Suspense>
    );
}
