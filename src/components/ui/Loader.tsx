import { Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";

interface LoaderProps {
    className?: string;
    size?: number;
}

export function Loader({ className, size = 24 }: LoaderProps) {
    return (
        <Loader2
            className={cn("animate-spin text-pink-500", className)}
            size={size}
        />
    );
}
