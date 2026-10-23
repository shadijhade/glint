import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink, GitBranch } from "lucide-react";
import { formatCount, formatDate } from "@/lib/format";
import { type RepoCard as Card, repoCard } from "@/lib/fs";
import { useTool } from "@/lib/tools";
import { ToolNeeded } from "../ToolNeeded";

/** GitHub's colors for the languages most repositories have; others take one of the rest. */
const COLORS: Record<string, string> = {
    rust: "#dea584",
    typescript: "#3178c6",
    tsx: "#6aa5e8",
    javascript: "#f1e05a",
    jsx: "#f1e05a",
    python: "#3572a5",
    go: "#00add8",
    java: "#b07219",
    kotlin: "#a97bff",
    c: "#555555",
    "c++": "#f34b7d",
    cpp: "#f34b7d",
    "c#": "#178600",
    csharp: "#178600",
    html: "#e34c26",
    css: "#663399",
    scss: "#c6538c",
    shell: "#89e051",
    powershell: "#012456",
    ruby: "#701516",
    php: "#4f5d95",
    swift: "#f05138",
    dart: "#00b4ab",
    lua: "#000080",
    vue: "#41b883",
    svelte: "#ff3e00",
    zig: "#ec915c",
    markdown: "#083fa1",
};
const OTHERS = ["#8b5cf6", "#14b8a6", "#f59e0b", "#ec4899", "#64748b", "#22c55e"];
const colorOf = (language: string, i: number) => COLORS[language.toLowerCase()] ?? OTHERS[i % OTHERS.length];

/** Cards read this session, by folder: a folder previewed again shows its card at once (Rust keeps them too, by commit). */
const read = new Map<string, Card | null>();

/** A Git repository's card in its folder's preview: what onefetch and tokei say of it, each installed on demand. */
export function RepoCard({ path }: { path: string }) {
    const [card, setCard] = useState<Card | null | undefined>(() => read.get(path.toLowerCase()));
    const onefetch = useTool("onefetch");
    const tokei = useTool("tokei");
    useEffect(() => {
        let stale = false;
        repoCard(path).then(
            (got) => {
                read.set(path.toLowerCase(), got);
                if (!stale) setCard(got);
            },
            () => !stale && setCard(null)
        );
        return () => {
            stale = true;
        };
        // Read again once a tool is installed.
    }, [path, onefetch?.origin, tokei?.origin]);
    if (!card) return null;

    const shown = card.languages.slice(0, 6);
    const authors = card.authors.slice(0, 3);
    const lines = card.lines.slice(0, 3);
    const site = card.url?.replace(/^git@([^:]+):/, "https://$1/").replace(/\.git$/, "");
    return (
        <div className="w-full max-w-sm space-y-2 rounded-lg border bg-card/60 p-3 text-left text-xs">
            {card.onefetch && card.name && (
                <>
                    <div className="flex items-center gap-1.5">
                        <GitBranch className="size-3.5 shrink-0 text-primary" />
                        <span className="truncate text-sm font-medium text-foreground">{card.name}</span>
                        {card.version && <span className="shrink-0 rounded bg-muted px-1.5 font-mono text-[0.625rem]">{card.version}</span>}
                        {card.license && <span className="shrink-0 rounded bg-muted px-1.5 text-[0.625rem]">{card.license}</span>}
                    </div>
                    {card.description && <p className="text-muted-foreground">{card.description}</p>}
                    <p className="text-muted-foreground">
                        {[
                            card.branch,
                            card.head,
                            card.commits !== null && `${formatCount(card.commits)} ${card.commits === 1 ? "commit" : "commits"}`,
                            card.lastChange && `changed ${formatDate(Date.parse(card.lastChange)).toLowerCase()}`,
                        ]
                            .filter(Boolean)
                            .join(" · ")}
                    </p>
                    {shown.length > 0 && (
                        <div className="space-y-1">
                            <div className="flex h-1.5 overflow-hidden rounded-full bg-muted" aria-hidden>
                                {shown.map(([language, share], i) => (
                                    <span key={language} style={{ width: `${share}%`, background: colorOf(language, i) }} />
                                ))}
                            </div>
                            <div className="flex flex-wrap gap-x-2.5 gap-y-0.5 text-muted-foreground">
                                {shown.map(([language, share], i) => (
                                    <span key={language} className="flex items-center gap-1">
                                        <span className="size-2 rounded-full" style={{ background: colorOf(language, i) }} />
                                        {language} {share >= 1 ? Math.round(share) : "<1"}%
                                    </span>
                                ))}
                            </div>
                        </div>
                    )}
                    {authors.length > 0 && (
                        <p className="truncate text-muted-foreground" title={card.authors.map((a) => `${a.name}: ${formatCount(a.commits)} commits`).join("\n")}>
                            {authors.map((a) => `${a.name} ${a.share}%`).join(" · ")}
                            {card.contributors !== null && card.contributors > authors.length && ` · ${formatCount(card.contributors - authors.length)} more`}
                        </p>
                    )}
                </>
            )}
            {card.total && card.total.code > 0 && (
                <p className="text-muted-foreground" title={card.lines.map((l) => `${l.language}: ${formatCount(l.code)} lines of code, ${formatCount(l.comments)} of comments`).join("\n")}>
                    <b className="font-medium text-foreground">{formatCount(card.total.code)}</b> lines of code
                    {lines.length > 0 && ` · ${lines.map((l) => `${l.language} ${formatCount(l.code)}`).join(" · ")}`}
                </p>
            )}
            {card.onefetch && site?.startsWith("https://") && (
                <button type="button" onClick={() => void openUrl(site)} className="flex max-w-full items-center gap-1 text-primary hover:underline" title="Open it in the browser">
                    <span className="truncate">{site.replace(/^https:\/\//, "")}</span>
                    <ExternalLink className="size-3 shrink-0" />
                </button>
            )}
            {!card.onefetch && <ToolNeeded id="onefetch" why="onefetch says what this repository is: its languages, who works on it, when it last changed." icon={GitBranch} className="px-3 py-2" />}
            {!card.tokei && <ToolNeeded id="tokei" why="tokei counts its lines of code, by language." className="px-3 py-2" />}
        </div>
    );
}
