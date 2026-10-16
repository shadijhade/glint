import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** Segoe Fluent Icons (Windows 11) and Segoe MDL2 Assets (Windows 10) glyphs: Windows' own caption buttons use them. */
const GLYPHS = { minimize: "", maximize: "", restore: "", close: "" };

/**
 * Minimize, maximize and close for the window, which draws its own title bar. They look and behave
 * like Windows 11's, and fade while another window is in front, as a native title bar does.
 */
export function WindowControls() {
    const [maximized, setMaximized] = useState(false);

    useEffect(() => {
        const win = getCurrentWindow();
        const update = () => win.isMaximized().then(setMaximized, () => {});
        update();
        const unlisten = [
            win.onResized(update),
            win.onFocusChanged(({ payload: focused }) => document.documentElement.toggleAttribute("data-window-inactive", !focused)),
        ];
        return () => {
            for (const stop of unlisten) stop.then((f) => f());
        };
    }, []);

    const win = getCurrentWindow();
    // Not in the tab order, like Windows' own caption buttons.
    return (
        <div role="group" aria-label="Window" className="flex h-full shrink-0 self-start">
            <button type="button" tabIndex={-1} className="caption-button" onClick={() => win.minimize()} title="Minimize" aria-label="Minimize">
                {GLYPHS.minimize}
            </button>
            <button
                type="button"
                tabIndex={-1}
                className="caption-button"
                onClick={() => win.toggleMaximize()}
                title={maximized ? "Restore down" : "Maximize"}
                aria-label={maximized ? "Restore down" : "Maximize"}
            >
                {maximized ? GLYPHS.restore : GLYPHS.maximize}
            </button>
            <button type="button" tabIndex={-1} className="caption-button caption-close" onClick={() => win.close()} title="Close" aria-label="Close">
                {GLYPHS.close}
            </button>
        </div>
    );
}
