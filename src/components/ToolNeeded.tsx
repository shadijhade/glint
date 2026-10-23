import type { LucideIcon } from "lucide-react";
import { Download, Wrench } from "lucide-react";
import type { ToolStatus } from "@/lib/fs";
import { cancelInstall, describeProgress, installTool, useTool, useTools } from "@/lib/tools";
import { cn } from "@/lib/utils";
import { Button } from "./ui/button";

/** What Install will do, for its tooltip. */
export function installHint(tool: ToolStatus) {
    return tool.source === "release" ? `Downloads ${tool.name} ${tool.pinned ?? ""} from its GitHub release and checks it`.replace("  ", " ") : `Installs ${tool.name} with winget`;
}

/**
 * Shown where a feature needs a tool that isn't installed: says why, and installs it with one click (with
 * its progress, and Stop). Nothing shows once the tool is there.
 */
export function ToolNeeded({ id, why, icon: Icon = Wrench, className }: { id: string; why: string; icon?: LucideIcon; className?: string }) {
    const tool = useTool(id);
    const progress = useTools((s) => s.busy[id]);
    if (!tool || tool.origin) return null;
    return (
        <div className={cn("flex items-center gap-3 rounded-lg border bg-primary/5 px-4 py-3", className)}>
            <Icon className="size-4 shrink-0 text-primary" />
            <span className="min-w-0 flex-1 text-sm">{why}</span>
            {progress ? (
                <>
                    <span className="truncate text-xs tabular-nums text-muted-foreground">{describeProgress(progress)}</span>
                    <Button size="sm" variant="outline" onClick={() => cancelInstall(id)}>
                        Stop
                    </Button>
                </>
            ) : (
                <Button size="sm" onClick={() => void installTool(id)} title={installHint(tool)}>
                    <Download /> Install {tool.name}
                </Button>
            )}
        </div>
    );
}
