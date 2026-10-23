import SyntaxHighlighter from "react-syntax-highlighter/dist/esm/prism-light";
import bash from "react-syntax-highlighter/dist/esm/languages/prism/bash";
import c from "react-syntax-highlighter/dist/esm/languages/prism/c";
import cpp from "react-syntax-highlighter/dist/esm/languages/prism/cpp";
import csharp from "react-syntax-highlighter/dist/esm/languages/prism/csharp";
import css from "react-syntax-highlighter/dist/esm/languages/prism/css";
import go from "react-syntax-highlighter/dist/esm/languages/prism/go";
import java from "react-syntax-highlighter/dist/esm/languages/prism/java";
import javascript from "react-syntax-highlighter/dist/esm/languages/prism/javascript";
import json from "react-syntax-highlighter/dist/esm/languages/prism/json";
import jsx from "react-syntax-highlighter/dist/esm/languages/prism/jsx";
import markdown from "react-syntax-highlighter/dist/esm/languages/prism/markdown";
import markup from "react-syntax-highlighter/dist/esm/languages/prism/markup";
import powershell from "react-syntax-highlighter/dist/esm/languages/prism/powershell";
import python from "react-syntax-highlighter/dist/esm/languages/prism/python";
import rust from "react-syntax-highlighter/dist/esm/languages/prism/rust";
import sql from "react-syntax-highlighter/dist/esm/languages/prism/sql";
import toml from "react-syntax-highlighter/dist/esm/languages/prism/toml";
import tsx from "react-syntax-highlighter/dist/esm/languages/prism/tsx";
import typescript from "react-syntax-highlighter/dist/esm/languages/prism/typescript";
import yaml from "react-syntax-highlighter/dist/esm/languages/prism/yaml";

// Only these grammars are bundled; anything else previews as plain text.
const LANGUAGES = { bash, c, cpp, csharp, css, go, java, javascript, json, jsx, markdown, markup, powershell, python, rust, sql, toml, tsx, typescript, yaml };
Object.entries(LANGUAGES).forEach(([name, grammar]) => SyntaxHighlighter.registerLanguage(name, grammar));

const LANGUAGE_BY_EXT: Record<string, keyof typeof LANGUAGES> = {
    sh: "bash", bash: "bash", c: "c", h: "c", cc: "cpp", cpp: "cpp", hpp: "cpp", cs: "csharp", css: "css", scss: "css",
    go: "go", java: "java", js: "javascript", mjs: "javascript", cjs: "javascript", json: "json", jsx: "jsx",
    md: "markdown", html: "markup", htm: "markup", xml: "markup", svg: "markup", xaml: "markup", csproj: "markup",
    vue: "markup", ps1: "powershell", py: "python", rs: "rust", sql: "sql", toml: "toml", tsx: "tsx", ts: "typescript",
    mts: "typescript", yml: "yaml", yaml: "yaml",
};

export const languageFor = (ext: string) => LANGUAGE_BY_EXT[ext] ?? "text";

const ALIASES: Record<string, keyof typeof LANGUAGES> = { "c++": "cpp", "c#": "csharp", shell: "bash", zsh: "bash", console: "bash", pwsh: "powershell", ps: "powershell" };

/** A code block's language as Markdown names it (```rust, ```ts, ```c++…). */
export function languageNamed(name: string) {
    const lower = name.toLowerCase();
    return Object.prototype.hasOwnProperty.call(LANGUAGES, lower) ? lower : (ALIASES[lower] ?? languageFor(lower));
}

export { SyntaxHighlighter };
