import { useEffect, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { MarkdownView } from "./components/explorer/MarkdownView";
import { readTextPreview } from "./lib/fs";
import "./index.css";

// Export as PDF (print.rs): a hidden window of its own draws a Markdown file exactly as the preview does
// (math, Mermaid, its pictures), always light, then tells Glint it's ready for WebView2 to print to PDF.

const params = new URLSearchParams(location.search);
const path = params.get("path") ?? "";
const id = Number(params.get("id") ?? "0");

function Print() {
    const [text, setText] = useState<string | null>(null);
    const box = useRef<HTMLDivElement>(null);
    useEffect(() => {
        readTextPreview(path, 32 * 1024 * 1024).then(
            (read) => setText(read.text),
            (e) => setText(`Couldn't read ${path}: ${e}`)
        );
    }, []);
    // Ready once the Markdown is in the page and its diagrams are drawn (or 10 seconds have gone by).
    useEffect(() => {
        if (text === null) return;
        const started = Date.now();
        const timer = setInterval(() => {
            const el = box.current;
            const drawn = !!el && el.childElementCount > 0 && el.querySelector(".markdown")?.childElementCount !== 0;
            const diagrams = el ? Array.from(el.querySelectorAll("[data-mermaid], .mermaid")) : [];
            const pending = diagrams.some((d) => !d.querySelector("svg, img"));
            if ((drawn && !pending) || Date.now() - started > 10_000) {
                clearInterval(timer);
                setTimeout(() => void invoke("print_ready", { id }), 250);
            }
        }, 100);
        return () => clearInterval(timer);
    }, [text]);
    return (
        <div ref={box} className="mx-auto max-w-3xl bg-white px-2 py-4 text-black">
            {text !== null && <MarkdownView text={text} file={path} archived={false} />}
        </div>
    );
}

document.documentElement.classList.remove("dark");
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<Print />);
