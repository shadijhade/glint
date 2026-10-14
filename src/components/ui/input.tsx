import * as React from "react"
import { cn } from "@/lib/utils"

export function Input({ className, ...props }: React.ComponentProps<"input">) {
    return (
        <input
            spellCheck={false}
            className={cn(
                "h-8 w-full rounded-md border border-input bg-background px-2.5 text-sm outline-hidden transition-colors placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/20 disabled:opacity-50",
                className
            )}
            {...props}
        />
    )
}
