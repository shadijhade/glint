import { invoke } from "@tauri-apps/api/core";

/** A kind of file Windows' New menu makes (newmenu.rs): a Word document, a zip… */
export interface NewKind {
    /** Its extension, with the dot. */
    ext: string;
    /** What the menu says: "Microsoft Word Document". */
    name: string;
    /** Its icon as File Explorer shows it (a PNG's data URL). */
    icon: string | null;
    /** Its app makes it with a command of its own (and may open a window): not renamed right away. */
    byApp: boolean;
}

let kinds: NewKind[] = [];
let asked = 0;
let asking: Promise<void> | null = null;

/** Reads them (again, when it's been a while: an app installed since then shows up). */
export function loadNewKinds() {
    if (asking || Date.now() - asked < 30_000) return asking ?? Promise.resolve();
    asking = invoke<NewKind[]>("new_kinds")
        .then((found) => {
            kinds = found;
            asked = Date.now();
        })
        .catch(() => {})
        .finally(() => (asking = null));
    return asking;
}

/** File Explorer's New menu kinds on this PC, as last read; each look starts another read in the background. */
export function newKinds(): NewKind[] {
    void loadNewKinds();
    return kinds;
}

/** Makes one in `folder`: its path, or null when its app hasn't made it within a few seconds. */
export const newFile = (folder: string, ext: string) => invoke<string | null>("new_file", { folder, ext });
