import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";
import type { FileEntry } from "@/lib/entries";
import { kindOf } from "@/lib/fileTypes";
import { cn } from "@/lib/utils";

/** The preview pane's states, as every kind of preview shows them: what's in the middle of it. */
export function Centered({ children, className }: { children: React.ReactNode; className?: string }) {
    return <div className={cn("flex h-full flex-col items-center justify-center gap-3 p-6 text-center text-sm text-muted-foreground", className)}>{children}</div>;
}

/** The file's kind, large, with why it isn't shown (or nothing to show). */
export function BigIcon({ entry, note }: { entry: FileEntry; note?: string }) {
    const kind = kindOf(entry);
    return (
        <Centered>
            <kind.icon className={cn("size-16 stroke-[1.25]", kind.tone)} />
            {note && <p>{note}</p>}
        </Centered>
    );
}

/** True once `ms` have passed: a spinner only for what takes a moment. */
function useLater(ms: number) {
    const [late, setLate] = useState(false);
    useEffect(() => {
        const timer = setTimeout(() => setLate(true), ms);
        return () => clearTimeout(timer);
    }, [ms]);
    return late;
}

/** What's still being read: nothing at first, a spinner once it takes a moment. */
export function Waiting({ what }: { what?: string }) {
    const late = useLater(300);
    return late ? (
        <Centered>
            <Loader2 className="size-5 animate-spin text-primary" />
            {what && <p>{what}</p>}
        </Centered>
    ) : null;
}
