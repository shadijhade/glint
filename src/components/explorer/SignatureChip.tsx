import { useEffect, useState } from "react";
import { Loader2, ShieldAlert, ShieldCheck, ShieldQuestion, ShieldX, type LucideIcon } from "lucide-react";
import { formatFullDate } from "@/lib/format";
import { type Signature, verifySignature } from "@/lib/fs";
import { cn } from "@/lib/utils";

type Tone = "good" | "bad" | "warn" | "plain";

/** What a signature check found, in words: a short line, and the rest for a tooltip or a list. */
export function describeSignature(s: Signature): { text: string; tone: Tone; details: string[] } {
    const by = s.signer ? ` by ${s.signer}` : "";
    const details = [
        s.issuer && `Its certificate was issued by ${s.issuer}`,
        s.catalog && "Signed in a Windows catalog, as Windows' own files are",
        s.signedAt && `Signed ${formatFullDate(s.signedAt)}`,
        s.state !== "valid" && s.detail,
        "Checked offline: certificates aren't looked up as revoked",
    ].filter((d): d is string => !!d);
    switch (s.state) {
        case "valid":
            return { text: `Signed${by}`, tone: "good", details };
        case "unsigned":
            return { text: "Not signed", tone: "plain", details: ["No digital signature, in the file or in a Windows catalog"] };
        case "invalid":
            return { text: `Signature doesn't hold: changed after it was signed${by}`, tone: "bad", details };
        case "untrusted":
            return { text: `Signed${by}, but not with a certificate Windows trusts`, tone: "warn", details };
        case "expired":
            return { text: `Signed${by}, with a certificate that expired (and no timestamp)`, tone: "warn", details };
        case "revoked":
            return { text: `Signed${by}, with a certificate that was revoked`, tone: "bad", details };
        default:
            return { text: "Couldn't check the signature", tone: "plain", details };
    }
}

const ICONS: Record<Tone, LucideIcon> = { good: ShieldCheck, bad: ShieldX, warn: ShieldAlert, plain: ShieldQuestion };
export const TONES: Record<Tone, string> = {
    good: "bg-emerald-500/15 text-emerald-700 dark:text-emerald-400",
    bad: "bg-destructive/15 text-destructive",
    warn: "bg-amber-500/15 text-amber-800 dark:text-amber-400",
    plain: "border bg-background text-muted-foreground",
};

/** A file's signature, checked when it's shown (null while Windows checks it). */
export function useSignature(path: string | null): Signature | null {
    const [signature, setSignature] = useState<Signature | null>(null);
    useEffect(() => {
        let stale = false;
        setSignature(null);
        if (path)
            verifySignature(path).then(
                (s) => !stale && setSignature(s),
                () => !stale && setSignature({ state: "unknown", catalog: false, signer: null, issuer: null, signedAt: null, detail: null })
            );
        return () => {
            stale = true;
        };
    }, [path]);
    return signature;
}

/** A chip saying whether the file's signature holds, and who signed it. */
export function SignatureChip({ path, className }: { path: string; className?: string }) {
    const signature = useSignature(path);
    if (!signature) {
        return (
            <span className={cn("inline-flex items-center gap-1 rounded-md border bg-background px-2 py-0.5 text-muted-foreground", className)}>
                <Loader2 className="size-3 animate-spin" /> Checking the signature…
            </span>
        );
    }
    const { text, tone, details } = describeSignature(signature);
    const Icon = ICONS[tone];
    return (
        <span className={cn("inline-flex min-w-0 items-center gap-1 rounded-md px-2 py-0.5 font-medium", TONES[tone], className)} title={[text, ...details].join("\n")}>
            <Icon className="size-3 shrink-0" />
            <span className="truncate">{text}</span>
        </span>
    );
}
