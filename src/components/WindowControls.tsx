import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** Segoe Fluent Icons (Windows 11) and Segoe MDL2 Assets (Windows 10) glyphs: Windows' own caption buttons use them. */
const GLYPHS = { minimize: "", maximize: "", restore: "", close: "" };

/**
 * Minimize, maximize and close for the window, which draws its own title bar. They look and behave
 * like Windows 11's, and fade while another window is in front, as a native title bar does.
 */
export function WindowControls() {
    const [maximized, setMaximized] = useState(false);
    const maximize = useRef<HTMLButtonElement>(null);
    // The pointer on the maximize button, which Windows' snap layouts take over (see snap.rs): "hover", "press" or "".
    const [snap, setSnap] = useState("");

    useEffect(() => {
        const win = getCurrentWindow();
        const update = () => win.isMaximized().then(setMaximized, () => {});
        update();
        const unlisten = [
            win.onResized(update),
            win.onFocusChanged(({ payload: focused }) => document.documentElement.toggleAttribute("data-window-inactive", !focused)),
            listen<string>("snap-button", ({ payload }) => setSnap(payload)),
        ];
        return () => {
            for (const stop of unlisten) stop.then((f) => f());
        };
    }, []);

    // Windows' snap layouts window goes over the button, in physical pixels: again as the window or its scale changes.
    useEffect(() => {
        const button = maximize.current!;
        const place = () => {
            const r = button.getBoundingClientRect();
            const k = window.devicePixelRatio;
            const [x, y] = [Math.round(r.left * k), Math.round(r.top * k)];
            invoke("snap_place", { rect: { x, y, width: Math.round(r.right * k) - x, height: Math.round(r.bottom * k) - y } }).catch(() => {});
        };
        place();
        const observer = new ResizeObserver(place);
        observer.observe(button, { box: "device-pixel-content-box" });
        window.addEventListener("resize", place);
        return () => {
            observer.disconnect();
            window.removeEventListener("resize", place);
            invoke("snap_place", { rect: null }).catch(() => {});
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
                ref={maximize}
                type="button"
                tabIndex={-1}
                data-snap={snap || undefined}
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
