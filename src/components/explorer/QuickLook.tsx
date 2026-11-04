import { useState } from "react";
import type { FileEntry } from "@/lib/entries";
import { cn } from "@/lib/utils";
import { EntryPreview } from "./FilePreview";

/** Space-bar preview of the focused item, large. Arrows move through the list underneath; Space or Esc closes. */
export function QuickLook({ entry, onClose }: { entry: FileEntry; onClose: () => void }) {
    const [maximized, setMaximized] = useState(false);
    return (
        <div
            aria-modal="true"
            // The list underneath keeps its arrow keys (see modalOpen).
            data-quicklook
            aria-label={`Quick Look: ${entry.name}`}
            role="dialog"
            className={cn(
                "glint-fade fixed inset-0 z-40 flex items-center justify-center bg-black/55 backdrop-blur-xs transition-[padding] duration-150",
                maximized ? "p-1.5 sm:p-2.5" : "p-4 sm:p-8"
            )}
            onMouseDown={(e) => e.target === e.currentTarget && onClose()}
        >
            <div
                className={cn(
                    "glint-rise flex h-full w-full overflow-hidden rounded-xl border bg-card shadow-2xl transition-[max-width,max-height] duration-150",
                    !maximized && "max-h-[56rem] max-w-[80rem]"
                )}
            >
                <div key={entry.path} className="glint-quick-swap flex min-h-0 min-w-0 flex-1 flex-col">
                    <EntryPreview entry={entry} onClose={onClose} maximized={maximized} onToggleMaximize={() => setMaximized((m) => !m)} />
                </div>
            </div>
        </div>
    );
}
