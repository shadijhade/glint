import { Heart, Github } from "lucide-react";

export function Footer() {
    return (
        <footer className="shrink-0 py-4 px-6 border-t border-zinc-200 dark:border-zinc-800/50 bg-white/30 dark:bg-zinc-950/30 backdrop-blur-sm flex items-center justify-between transition-colors duration-300">
            <div className="flex items-center gap-1.5 text-xs text-zinc-500 dark:text-zinc-400 font-medium">
                <span>Created and maintained with</span>
                <Heart className="w-3.5 h-3.5 text-pink-500 fill-pink-500 animate-pulse" />
                <span>by Shadi Sadi</span>
            </div>

            <a
                href="https://github.com/shadijhade/shadui-ripgrep"
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-2 text-xs text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 transition-colors"
            >
                <Github className="w-4 h-4" />
                <span>Source Code</span>
            </a>
        </footer>
    );
}
