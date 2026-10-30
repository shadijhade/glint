import type { FileEntry } from "@/lib/entries";
import { EntryPreview } from "./FilePreview";

/** Space-bar preview of the focused item, large. Arrows move through the list underneath; Space or Esc closes. */
export function QuickLook({ entry, onClose }: { entry: FileEntry; onClose: () => void }) {
    return (
        <div
            aria-modal="true"
            // The list underneath keeps its arrow keys (see modalOpen).
            data-quicklook
            aria-label={`Quick Look: ${entry.name}`}
            role="dialog"
            className="glint-fade fixed inset-0 z-40 flex items-center justify-center bg-black/55 p-4 backdrop-blur-xs sm:p-8"
            onMouseDown={(e) => e.target === e.currentTarget && onClose()}
        >
            <div className="glint-rise flex h-full max-h-[56rem] w-full max-w-[80rem] overflow-hidden rounded-xl border bg-card shadow-2xl">
                <div key={entry.path} className="glint-quick-swap flex min-h-0 flex-1 flex-col">
                    <EntryPreview entry={entry} onClose={onClose} />
                </div>
            </div>
        </div>
    );
}
