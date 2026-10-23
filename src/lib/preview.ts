import { create } from "zustand";

/** A file's other views: as written (Markdown's source, a tree's text), what Git says changed, its bytes, or SQL over it. */
export type View = "file" | "source" | "changes" | "hex" | "query";

/** Which view the preview shows, kept while moving from file to file (where a file has no such view, it's shown as it is). */
export const usePreviewView = create<{ view: View }>(() => ({ view: "file" }));

/** A query for the Query view to start with, for one file (a table's rows, from its database's schema). */
export const useQueryDraft = create<{ draft: { path: string; sql: string } | null }>(() => ({ draft: null }));

/** Shows a file's Query view with `sql` in it, run. */
export function showQuery(path: string, sql: string) {
    useQueryDraft.setState({ draft: { path, sql } });
    usePreviewView.setState({ view: "query" });
}

/** A table's name as SQL takes it: as it is when it's a plain word, else in double quotes. */
export const sqlName = (name: string) => (/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) ? name : `"${name.replace(/"/g, '""')}"`);
