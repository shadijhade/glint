import { FilePlus, FileText, FolderPlus } from "lucide-react";
import { newItem, newOfKind } from "@/lib/explorer";
import { newKinds } from "@/lib/newKinds";
import type { MenuItem } from "../Menu";

/** What New makes in the open folder: a folder (when asked), a text document, and the kinds of file Windows' New menu has (Word, Excel, a zip…). */
export function newMenuItems(folder: boolean): MenuItem[] {
    const kinds = newKinds();
    return [
        folder && { label: "Folder", icon: FolderPlus, shortcut: "Ctrl+Shift+N", onSelect: () => void newItem(true) },
        { label: "Text document", icon: FileText, onSelect: () => void newItem(false) },
        kinds.length > 0 && "separator",
        ...kinds.map((kind): MenuItem => ({ label: kind.name, icon: FilePlus, image: kind.icon ?? undefined, onSelect: () => void newOfKind(kind) })),
    ];
}
