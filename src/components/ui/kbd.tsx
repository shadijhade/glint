import * as React from "react"
import { cn } from "@/lib/utils"

/** A key, as shortcuts are shown everywhere in the app. */
export function Kbd({ className, ...props }: React.ComponentProps<"kbd">) {
    return <kbd className={cn("whitespace-nowrap rounded-sm border bg-card px-1.5 py-0.5 font-sans text-[0.6875rem] text-muted-foreground", className)} {...props} />
}
