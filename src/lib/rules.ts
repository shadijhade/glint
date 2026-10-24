import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { baseName } from "./paths";
import { type FolderRule, useStore } from "./store";

/** What a rule did to one file, or (in a preview) would: `action` is then what, in words, and `to` where it'd go. */
export interface RuleDone {
    id: number;
    at: number;
    rule: string;
    action: string;
    from: string;
    to: string | null;
    error: string | null;
    undone: boolean;
}

export const rulesPreview = (rule: FolderRule) => invoke<RuleDone[]>("rules_preview", { rule });
/** Does what `rule` says to what's in its folder now: how many it did. */
export const rulesApply = (rule: FolderRule) => invoke<number>("rules_apply", { rule });
export const rulesLog = () => invoke<RuleDone[]>("rules_log");
export const rulesClearLog = () => invoke("rules_clear_log");
export const rulesUndo = (id: number) => invoke("rules_undo", { id });

const DID: Record<string, string> = { move: "moved", rename: "renamed", recycle: "put in the Recycle Bin", unblock: "unblocked", extract: "extracted" };

/** What a rule did, in a few words: "moved “a.pdf”". */
export const didWhat = (done: RuleDone) => `${DID[done.action] ?? done.action} “${baseName(done.from)}”`;

export async function undoRule(done: RuleDone) {
    try {
        await rulesUndo(done.id);
        toast.success(`Took back: ${didWhat(done)}`, { duration: 2500 });
    } catch (e) {
        toast.error("Couldn't take it back", { description: String(e) });
    }
}

/** Tells Rust the rules (at start and whenever they change), and says what they do as they do it, with Undo. */
export function initRules() {
    const push = (rules: FolderRule[]) => invoke("rules_set", { rules }).catch(() => {});
    useStore.subscribe((s, before) => {
        if (s.rules !== before.rules) void push(s.rules);
    });
    void listen<RuleDone>("rule-did", ({ payload: done }) => {
        if (done.error) toast.error(`Rule “${done.rule}” couldn't do it: ${baseName(done.from)}`, { description: done.error });
        else toast.success(`Rule “${done.rule}”: ${didWhat(done)}`, { description: done.to ?? undefined, action: { label: "Undo", onClick: () => void undoRule(done) } });
    });
    return push(useStore.getState().rules);
}

/** A new rule's id. */
export const ruleId = () => `rule-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
