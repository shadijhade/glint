import { useEffect, useMemo, useState } from "react";
import { Tag } from "lucide-react";
import { COLOR_TAGS, setTags, tagColor, useTags, useTagsEdit, wantTags } from "@/lib/tags";
import { Modal, ModalHeader } from "../Modal";
import { Button } from "../ui/button";

/** The tags of the items `paths`, read from the files, each with a dot of its color. */
export function TagDots({ path }: { path: string }) {
    const tags = useTags((s) => s.byPath[path.toLowerCase()]);
    useEffect(() => wantTags(path), [path]);
    if (!tags?.length) return null;
    return (
        <span className="flex shrink-0 items-center gap-0.5" title={`Tags: ${tags.join(", ")}`} aria-label={`Tags: ${tags.join(", ")}`}>
            {tags.slice(0, 5).map((t) => (
                <span key={t} className="size-2 rounded-full ring-1 ring-background" style={{ background: tagColor(t) }} />
            ))}
        </span>
    );
}

/**
 * Edit tags…: the tags the items share, as text to change (commas between them), with the color tags
 * and the ones used lately a click away. What's taken off comes off every item; what's added goes on all.
 */
export function TagsDialog() {
    const paths = useTagsEdit((s) => s.paths);
    const byPath = useTags((s) => s.byPath);
    const recent = useTags((s) => s.recent);
    const shared = useMemo(() => {
        if (!paths?.length) return [];
        const lists = paths.map((p) => byPath[p.toLowerCase()] ?? []);
        return lists[0].filter((t) => lists.every((l) => l.some((x) => x.toLowerCase() === t.toLowerCase())));
    }, [paths, byPath]);
    const [text, setText] = useState("");
    useEffect(() => {
        if (paths) {
            paths.forEach(wantTags);
            setText(shared.join(", "));
        }
        // Only as it opens.
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [paths]);
    if (!paths) return null;
    const close = () => useTagsEdit.setState({ paths: null });
    const typed = text.split(",").map((t) => t.trim()).filter(Boolean);
    const has = (tag: string) => typed.some((t) => t.toLowerCase() === tag.toLowerCase());
    const toggle = (tag: string) => setText((has(tag) ? typed.filter((t) => t.toLowerCase() !== tag.toLowerCase()) : [...typed, tag]).join(", "));
    const save = (e: React.FormEvent) => {
        e.preventDefault();
        const removed = shared.filter((t) => !has(t));
        void setTags(paths, typed, removed);
        close();
    };
    const offered = [...COLOR_TAGS.map((t) => t.name), ...recent.filter((t) => !COLOR_TAGS.some((c) => c.name.toLowerCase() === t.toLowerCase()))].slice(0, 16);
    return (
        <Modal label="Tags" onClose={close} className="max-w-md">
            <ModalHeader icon={Tag} title={paths.length === 1 ? `Tags of “${paths[0].split("\\").pop()}”` : `Tags of ${paths.length} items`} subtitle="Kept in the files themselves: they go where the files go, on NTFS and ReFS drives." onClose={close} />
            <form onSubmit={save} className="flex flex-col gap-3 p-4 text-sm">
                <input autoFocus value={text} onChange={(e) => setText(e.target.value)} placeholder="Work, Taxes 2026, Red" aria-label="Tags, with commas between them" className="fluent-field h-8 px-2.5 outline-hidden" />
                <div className="flex flex-wrap gap-1.5">
                    {offered.map((tag) => (
                        <button
                            key={tag}
                            type="button"
                            onClick={() => toggle(tag)}
                            aria-pressed={has(tag)}
                            className="flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs transition-colors hover:bg-accent aria-pressed:border-primary aria-pressed:bg-primary/10"
                        >
                            <span className="size-2 rounded-full" style={{ background: tagColor(tag) }} />
                            {tag}
                        </button>
                    ))}
                </div>
                <div className="flex justify-end gap-2">
                    <Button type="button" variant="ghost" size="sm" onClick={close}>
                        Cancel
                    </Button>
                    <Button type="submit" size="sm">
                        Save
                    </Button>
                </div>
            </form>
        </Modal>
    );
}
