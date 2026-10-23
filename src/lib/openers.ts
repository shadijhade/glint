import { create } from "zustand";
import { Code, SquareTerminal } from "lucide-react";
import { toast } from "sonner";
import { listOpeners, type Opener, openIn } from "./fs";

/** The editors and terminals found on this PC, for Open in…: read at start and when the window is back in front. */
export const useOpeners = create<{ list: Opener[] }>(() => ({ list: [] }));

export function refreshOpeners() {
    listOpeners().then(
        (list) => {
            const now = useOpeners.getState().list;
            if (JSON.stringify(now) !== JSON.stringify(list)) useOpeners.setState({ list });
        },
        () => {}
    );
}

/** Open in…'s items for `path`: each editor, and each terminal (a file's folder in it). */
export function openInItems(path: string) {
    return useOpeners.getState().list.map((o) => ({
        label: o.name,
        icon: o.terminal ? SquareTerminal : Code,
        onSelect: () => void openIn(o.id, path).catch((e) => toast.error(`Couldn't open it in ${o.name}`, { description: String(e) })),
    }));
}
