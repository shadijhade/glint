import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Braces, ChevronRight, ClipboardCopy, Filter, Square } from "lucide-react";
import { toast } from "sonner";
import { type FileEntry, isArchived } from "@/lib/entries";
import { formatCount } from "@/lib/format";
import { archiveExtract, type JqAnswer, jqQuery, jqStop } from "@/lib/fs";
import { initiallyOpen, isContainer, jqPath, type JsonValue, PAGE, preview, sizeOf, type TreeRow, treeRows } from "@/lib/jsonTree";
import { useTool } from "@/lib/tools";
import { cn } from "@/lib/utils";
import { ToolNeeded } from "../ToolNeeded";
import { Button } from "../ui/button";
import { Waiting } from "./PreviewState";

const ROW = 22;
/** Containers opened at most by one Alt+click (opening everything under a big one would be millions of rows). */
const OPEN_ALL = 2000;

const keyText = (key: string) => (/^[A-Za-z_$][\w$-]*$/.test(key) ? key : JSON.stringify(key));

function Leaf({ value }: { value: JsonValue }) {
    if (value === null) return <span className="italic text-muted-foreground">null</span>;
    if (typeof value === "string") return <span className="text-[color:var(--tk-string)]">{JSON.stringify(value.length > 400 ? value.slice(0, 400) + "…" : value)}</span>;
    if (typeof value === "boolean") return <span className="text-[color:var(--tk-keyword)]">{String(value)}</span>;
    return <span className="text-[color:var(--tk-number)]">{String(value)}</span>;
}

function copy(text: string, what: string) {
    navigator.clipboard.writeText(text).then(
        () => toast.success(`Copied the ${what}`),
        () => toast.error(`Couldn't copy the ${what}`)
    );
}

/**
 * A JSON, YAML or TOML file as a tree of its values, with a jq filter over it, run by jaq (installed on
 * demand). Both run in a process of their own (jq.rs), so a file or filter that never ends or eats memory
 * is stopped there, not here. Big objects and arrays show 500 items at a time; Alt+click opens
 * everything under one.
 */
export function DataTree({ entry, format }: { entry: FileEntry; format: "json" | "yaml" | "toml" }) {
    const [typed, setTyped] = useState("");
    const [filter, setFilter] = useState(".");
    const [answer, setAnswer] = useState<JqAnswer | { error: string } | null>(null);
    const [running, setRunning] = useState(false);
    const [open, setOpen] = useState<Map<string, number>>(new Map());
    const [selected, setSelected] = useState<string | null>(null);
    const { path, modified } = entry;
    const archived = isArchived(entry);
    // A filter needs jaq; the file as it is doesn't.
    const jaq = useTool("jaq");
    const needsJaq = filter !== "." && !jaq?.origin;
    useEffect(() => {
        if (needsJaq) return;
        let stale = false;
        setRunning(true);
        (async () => jqQuery(archived ? await archiveExtract(path, "preview") : path, format, filter))()
            .then(
                (result) => {
                    if (stale) return;
                    setAnswer(result);
                    setOpen(initiallyOpen(result.values));
                    setSelected(null);
                },
                (e) => !stale && setAnswer({ error: String(e) === "stopped" ? "Stopped." : String(e) })
            )
            .finally(() => !stale && setRunning(false));
        return () => {
            stale = true;
        };
    }, [path, modified, archived, format, filter, needsJaq]);

    const values = answer && "values" in answer ? answer.values : [];
    const rows = useMemo(() => treeRows(values, open), [values, open]);
    const at = rows.findIndex((r) => r.id === selected);
    const row = at >= 0 ? rows[at] : undefined;

    const box = useRef<HTMLDivElement>(null);
    const [view, setView] = useState({ top: 0, height: 400 });
    useLayoutEffect(() => {
        const el = box.current;
        if (!el) return;
        const observer = new ResizeObserver(() => setView({ top: el.scrollTop, height: el.clientHeight }));
        observer.observe(el);
        return () => observer.disconnect();
    }, [answer]);
    const first = Math.max(0, Math.floor(view.top / ROW) - 10);
    const last = Math.min(rows.length, Math.ceil((view.top + view.height) / ROW) + 10);

    const toggle = (r: TreeRow) =>
        setOpen((o) => {
            const next = new Map(o);
            if (next.has(r.id)) next.delete(r.id);
            else next.set(r.id, PAGE);
            return next;
        });
    const openAll = (r: TreeRow) =>
        setOpen((o) => {
            const next = new Map(o);
            let budget = OPEN_ALL;
            const walk = (value: JsonValue, id: string, path: string) => {
                if (!isContainer(value) || budget-- <= 0) return;
                next.set(id, Math.max(next.get(id) ?? 0, PAGE));
                const prefix = id.slice(0, id.indexOf(":") + 1);
                const children: [string | number, JsonValue][] = Array.isArray(value) ? value.slice(0, PAGE).map((v, i) => [i, v]) : Object.entries(value).slice(0, PAGE);
                for (const [key, child] of children) walk(child, prefix + jqPath(path, key), jqPath(path, key));
            };
            walk(r.value, r.id, r.path);
            return next;
        });
    const more = (r: TreeRow) =>
        setOpen((o) => {
            const parent = r.id.replace(/\u0000more$/, "");
            return new Map(o).set(parent, (o.get(parent) ?? PAGE) + PAGE);
        });

    const select = (index: number) => {
        const target = rows[Math.max(0, Math.min(rows.length - 1, index))];
        if (!target) return;
        setSelected(target.id);
        const el = box.current;
        const top = Math.max(0, Math.min(rows.length - 1, index)) * ROW;
        if (el && top < el.scrollTop) el.scrollTop = top;
        else if (el && top + ROW > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW - el.clientHeight;
    };
    const onKey = (e: React.KeyboardEvent) => {
        const key = e.key;
        if (key === "ArrowDown") select(at + 1);
        else if (key === "ArrowUp") select(at - 1);
        else if (key === "Home") select(0);
        else if (key === "End") select(rows.length - 1);
        else if (key === "PageDown" || key === "PageUp") select(at + (key === "PageDown" ? 1 : -1) * Math.max(1, Math.floor(view.height / ROW) - 1));
        else if (key === "ArrowRight" && row && isContainer(row.value) && !row.more) {
            if (!row.open) toggle(row);
            else select(at + 1);
        } else if (key === "ArrowLeft" && row) {
            if (row.open) toggle(row);
            else for (let i = at - 1; i >= 0; i--) if (rows[i].depth < row.depth) return void (e.preventDefault(), select(i));
        } else if ((key === "Enter" || key === " ") && row) {
            if (row.more) more(row);
            else if (isContainer(row.value)) toggle(row);
        } else if (key === "c" && (e.ctrlKey || e.metaKey) && row && !row.more) copy(JSON.stringify(row.value, null, 2), "value");
        else return;
        e.preventDefault();
    };

    const run = (text: string) => {
        setTyped(text);
        setFilter(text.trim() || ".");
    };
    const many = values.length > 1;

    return (
        <div className="flex h-full flex-col">
            <form
                className="flex shrink-0 items-center gap-1.5 border-b px-2 py-1.5"
                onSubmit={(e) => {
                    e.preventDefault();
                    run(typed);
                }}
            >
                <span className="pl-1 font-mono text-xs font-medium text-muted-foreground" title="A jq filter, run by jaq">
                    jq
                </span>
                <input
                    value={typed}
                    onChange={(e) => setTyped(e.target.value)}
                    onKeyDown={(e) => {
                        if (e.key === "Enter") {
                            e.preventDefault();
                            run(typed);
                        } else if (e.key === "Escape" && (typed || filter !== ".")) {
                            e.preventDefault();
                            e.stopPropagation();
                            run("");
                        }
                    }}
                    placeholder=".items[] | select(.size > 1000) | .name"
                    spellCheck={false}
                    aria-label="jq filter"
                    className="fluent-field h-7 min-w-0 flex-1 px-2 font-mono text-xs outline-hidden"
                />
                {running ? (
                    <Button type="button" variant="ghost" size="sm" onClick={() => void jqStop()}>
                        <Square className="size-3! fill-current" /> Stop
                    </Button>
                ) : (
                    <Button type="submit" variant="ghost" size="sm" title="Run the filter (Enter); Esc clears it">
                        Run
                    </Button>
                )}
            </form>
            {needsJaq && (
                <ToolNeeded
                    id="jaq"
                    icon={Braces}
                    why="Filters run in jaq, jq rewritten in Rust: a 5 MB program from its GitHub release, checked before it's used."
                    className="m-2 shrink-0"
                />
            )}
            {answer && "error" in answer && answer.error && (
                <pre className={cn("max-h-40 shrink-0 overflow-auto border-b px-3 py-2 font-mono text-xs leading-[1.2] text-destructive", values.length && "bg-destructive/5")}>{answer.error}</pre>
            )}
            {!answer ? (
                <Waiting />
            ) : (
                <div
                    ref={box}
                    role="tree"
                    tabIndex={0}
                    aria-label="Values"
                    onKeyDown={onKey}
                    onScroll={(e) => setView({ top: e.currentTarget.scrollTop, height: e.currentTarget.clientHeight })}
                    className="min-h-0 flex-1 overflow-auto outline-hidden select-text"
                >
                    <div className="relative min-w-max" style={{ height: rows.length * ROW + 8 }}>
                        {rows.slice(first, last).map((r, i) => {
                            const top = (first + i) * ROW + 4;
                            const indent = 6 + r.depth * 16;
                            if (r.more !== undefined)
                                return (
                                    <button
                                        key={r.id}
                                        type="button"
                                        onClick={() => more(r)}
                                        style={{ top, paddingLeft: indent + 18 }}
                                        className={cn("absolute left-0 flex h-[22px] items-center pr-3 text-xs text-primary hover:underline", r.id === selected && "bg-accent")}
                                    >
                                        {formatCount(r.more)} more… (show {formatCount(Math.min(PAGE, r.more))})
                                    </button>
                                );
                            const container = isContainer(r.value);
                            return (
                                <div
                                    key={r.id}
                                    role="treeitem"
                                    aria-expanded={container ? r.open : undefined}
                                    aria-selected={r.id === selected}
                                    style={{ top, paddingLeft: indent }}
                                    onClick={(e) => {
                                        setSelected(r.id);
                                        if (container) (e.altKey && !r.open ? openAll : toggle)(r);
                                    }}
                                    className={cn(
                                        "absolute inset-x-0 flex h-[22px] items-center gap-1 whitespace-pre pr-4 font-mono text-[0.8125rem] leading-[22px]",
                                        r.id === selected ? "bg-accent" : "hover:bg-accent/60"
                                    )}
                                >
                                    {container ? (
                                        <ChevronRight className={cn("size-3.5 shrink-0 text-muted-foreground transition-transform", r.open && "rotate-90")} />
                                    ) : (
                                        <span className="w-3.5 shrink-0" />
                                    )}
                                    {r.key !== null ? (
                                        <span className={typeof r.key === "number" ? "text-muted-foreground" : "text-[color:var(--tk-function)]"}>
                                            {typeof r.key === "number" ? r.key : keyText(r.key)}
                                            <span className="text-muted-foreground">: </span>
                                        </span>
                                    ) : (
                                        many && <span className="text-muted-foreground">#{r.root + 1} </span>
                                    )}
                                    {container ? (
                                        <span className="text-muted-foreground">
                                            {r.open ? `${Array.isArray(r.value) ? "[" : "{"}${formatCount(sizeOf(r.value))}${Array.isArray(r.value) ? "]" : "}"}` : preview(r.value)}
                                        </span>
                                    ) : (
                                        <Leaf value={r.value} />
                                    )}
                                </div>
                            );
                        })}
                    </div>
                </div>
            )}
            <div className="flex h-8 shrink-0 items-center gap-1 border-t px-2 text-xs text-muted-foreground">
                {row && !row.more ? (
                    <>
                        <code className="min-w-0 flex-1 truncate font-mono text-foreground" title={row.path}>
                            {many ? `#${row.root + 1} ` : ""}
                            {row.path}
                        </code>
                        <Button variant="ghost" size="sm" className="h-6 px-1.5 text-xs" onClick={() => copy(row.path, "path")} title="Copy its jq path">
                            <ClipboardCopy /> Path
                        </Button>
                        <Button variant="ghost" size="sm" className="h-6 px-1.5 text-xs" onClick={() => copy(JSON.stringify(row.value, null, 2), "value")} title="Copy it as JSON (Ctrl+C)">
                            <ClipboardCopy /> Value
                        </Button>
                        {!many && row.path !== "." && (
                            <Button variant="ghost" size="sm" className="h-6 px-1.5 text-xs" onClick={() => run(row.path)} title="Filter to it">
                                <Filter /> Filter
                            </Button>
                        )}
                    </>
                ) : (
                    <span className="truncate">
                        {answer && "values" in answer
                            ? `${formatCount(values.length)} ${values.length === 1 ? "value" : "values"}${answer.truncated ? " (stopped there: there were more)" : ""} · ${filter === "." ? "read" : "jaq"} in ${answer.elapsed < 10 ? answer.elapsed.toFixed(1) : Math.round(answer.elapsed)} ms`
                            : running
                              ? "Running…"
                              : ""}
                    </span>
                )}
            </div>
        </div>
    );
}
