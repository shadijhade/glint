import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"

const buttonVariants = cva(
    "inline-flex shrink-0 items-center justify-center gap-1.5 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-4 [&_svg]:shrink-0",
    {
        variants: {
            variant: {
                default: "bg-primary text-primary-foreground shadow-xs hover:bg-primary/90",
                outline: "border bg-card hover:bg-accent",
                ghost: "text-muted-foreground hover:bg-accent hover:text-foreground",
                destructive: "text-destructive hover:bg-destructive/10",
            },
            size: {
                default: "h-9 px-3.5",
                sm: "h-8 px-2.5 text-xs",
                icon: "size-8",
            },
        },
        defaultVariants: {
            variant: "default",
            size: "default",
        },
    }
)

export type ButtonProps = React.ComponentProps<"button"> & VariantProps<typeof buttonVariants>

export function Button({ className, variant, size, type = "button", ...props }: ButtonProps) {
    return <button type={type} className={cn(buttonVariants({ variant, size }), className)} {...props} />
}
