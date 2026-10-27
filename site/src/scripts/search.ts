/*
  03 Find: a search over 9,474 of Windows' own file names, in the visitor's browser, on every keystroke.
  Words match anywhere in a name (all of them must); * and ? are wildcards over the whole name, as in
  Glint. Each search is timed, and the field of light is told how much of it matched.
*/
import type { App } from "./main";

interface Entry {
  name: string;
  lower: string;
  folder: number;
}

const SHOW = 8;

export function initSearch(_app: App) {
  const root = document.querySelector<HTMLElement>("[data-search]");
  const input = root?.querySelector<HTMLInputElement>("[data-search-input]");
  const status = root?.querySelector<HTMLElement>("[data-search-status]");
  const list = root?.querySelector<HTMLElement>("[data-search-results]");
  if (!root || !input || !status || !list) return;

  let entries: Entry[] | null = null;
  let folders: string[] = [];
  let loading: Promise<void> | null = null;

  const load = () =>
    (loading ??= fetch("/data/names.txt")
      .then((r) => r.text())
      .then((text) => {
        const out: Entry[] = [];
        // Either line ending: a Windows checkout of the repository may have turned the file's into CRLF.
        for (const line of text.split(/\r?\n/)) {
          if (!line) continue;
          if (line.startsWith(">")) folders.push(line.slice(1));
          else out.push({ name: line, lower: line.toLowerCase(), folder: folders.length - 1 });
        }
        entries = out;
      })
      .catch(() => {
        loading = null;
      }));

  const run = () => {
    if (!entries) {
      void load().then(() => entries && run());
      return;
    }
    const query = input.value;
    const match = compile(query);
    if (!match) {
      status.innerHTML = `Type a name: <span class="status-count">${entries.length.toLocaleString("en-US")}</span> to search`;
      list.innerHTML = "";
      tell(query, 0, false);
      return;
    }
    const t0 = performance.now();
    let count = 0;
    const shown: { e: Entry; span: [number, number] }[] = [];
    for (const e of entries) {
      const span = match(e.lower);
      if (!span) continue;
      if (count < SHOW) shown.push({ e, span });
      count++;
    }
    const ms = performance.now() - t0;
    status.innerHTML = `<span class="status-count">${count.toLocaleString("en-US")}</span> of ${entries.length.toLocaleString("en-US")} names<span class="status-time"> · ${ms < 0.1 ? "under 0.1" : ms.toFixed(2)} ms, in your browser</span>`;
    list.innerHTML = shown.length
      ? shown.map(({ e, span }) => row(e, span, folders[e.folder] ?? "")).join("")
      : `<li class="result-empty">No name has that. Try *.sys or shell.</li>`;
    tell(query, count / entries.length, true);
  };

  input.addEventListener("input", run);
  input.addEventListener("focus", () => void load(), { once: true });
  root.querySelectorAll<HTMLButtonElement>("[data-search-try]").forEach((b) =>
    b.addEventListener("click", () => {
      input.value = b.dataset.searchTry ?? "";
      run();
      input.focus({ preventScroll: true });
    }),
  );

  // The names come when the chapter is near, then the first query runs for real, timed.
  const io = new IntersectionObserver(
    (seen) => {
      if (!seen.some((s) => s.isIntersecting)) return;
      io.disconnect();
      void load().then(run);
    },
    { rootMargin: "700px 0px" },
  );
  io.observe(root);
}

type Matcher = (lower: string) => [number, number] | null;

function compile(query: string): Matcher | null {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return null;
  type Part = { kind: "sub"; sub: string } | { kind: "glob"; re: RegExp };
  const parts = terms.map((t): Part =>
    /[*?]/.test(t)
      ? { kind: "glob", re: new RegExp(`^${t.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".")}$`) }
      : { kind: "sub", sub: t },
  );
  return (lower) => {
    let first: [number, number] | null = null;
    for (const p of parts) {
      if (p.kind === "sub") {
        const at = lower.indexOf(p.sub);
        if (at < 0) return null;
        first ??= [at, at + p.sub.length];
      } else {
        if (!p.re.test(lower)) return null;
        first ??= [0, 0];
      }
    }
    return first;
  };
}

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c] ?? c);

function row(e: Entry, [a, b]: [number, number], folder: string) {
  const name = b > a ? `${esc(e.name.slice(0, a))}<mark>${esc(e.name.slice(a, b))}</mark>${esc(e.name.slice(b))}` : esc(e.name);
  return `<li><span class="result-name">${name}</span><span class="result-folder mono">${esc(folder)}</span></li>`;
}

/* The field lights a share of its points to match, chosen by the query, so each query has its own. The
   field may start after the first search: it says so, and hears the last one again. */
let last: CustomEvent | null = null;
window.addEventListener("glint:field", () => last && window.dispatchEvent(new CustomEvent(last.type, { detail: last.detail })));

function tell(query: string, frac: number, active: boolean) {
  let h = 2166136261;
  for (let i = 0; i < query.length; i++) h = Math.imul(h ^ query.charCodeAt(i), 16777619);
  const hash = ((h >>> 0) % 10007) / 10007;
  last = new CustomEvent("glint:query", { detail: { hash, frac: Math.min(0.3, Math.max(frac ? 0.004 : 0, frac * 3)), active } });
  window.dispatchEvent(last);
}
