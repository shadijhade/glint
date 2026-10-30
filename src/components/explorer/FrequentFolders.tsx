import { useMemo } from "react";
import { Folder } from "lucide-react";
import { middleClickOpens, navigate } from "@/lib/explorer";
import { baseName, isFolderPath, parentPath } from "@/lib/paths";
import { frecency } from "@/lib/rank";
import { useStore } from "@/lib/store";

/** The most folders shown. */
const MOST = 8;

/** Above Recent: the folders opened most (and most lately), as File Explorer's Home has them, each a click away. */
export function FrequentFolders({ onContextMenu }: { onContextMenu: (e: React.MouseEvent, path: string) => void }) {
    const visits = useStore((s) => s.visits);
    const folders = useMemo(() => {
        const now = Date.now();
        return Object.entries(visits)
            .filter(([path]) => isFolderPath(path))
            .sort(([, a], [, b]) => frecency(b, now) - frecency(a, now))
            .slice(0, MOST)
            .map(([path]) => path);
    }, [visits]);
    if (!folders.length) return null;
    return (
        <section aria-label="Frequent folders" className="shrink-0 border-b px-3 py-2.5">
            <h2 className="mb-1.5 text-xs font-semibold text-muted-foreground">Frequent folders</h2>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(11rem,1fr))] gap-1">
                {folders.map((path) => (
                    <button
                        key={path}
                        type="button"
                        data-drop={path}
                        onClick={() => navigate(path)}
                        onMouseDown={middleClickOpens(path)}
                        onContextMenu={(e) => onContextMenu(e, path)}
                        title={path}
                        className="flex min-w-0 items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-accent"
                    >
                        <Folder className="size-5 shrink-0 fill-amber-400/80 text-amber-500" />
                        <span className="min-w-0">
                            <span className="block truncate text-sm">{baseName(path) || path}</span>
                            <span className="block truncate text-[0.6875rem] text-muted-foreground">{parentPath(path) || path}</span>
                        </span>
                    </button>
                ))}
            </div>
        </section>
    );
}
