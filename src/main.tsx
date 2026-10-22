import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import { QuickSearch } from "./components/QuickSearch";
import "./index.css";

// The quick search window (Settings → Quick search) loads the same page.
const Root = getCurrentWindow().label === "quick" ? QuickSearch : App;

// Release builds: hide the webview's own context menu (Reload, Inspect…) except where text can be copied.
if (!import.meta.env.DEV) {
  document.addEventListener("contextmenu", (e) => {
    if (!(e.target as Element).closest("input, textarea, .select-text")) e.preventDefault();
  });
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
