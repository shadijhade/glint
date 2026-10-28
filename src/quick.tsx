import React from "react";
import ReactDOM from "react-dom/client";
import { QuickSearch } from "./components/QuickSearch";
import "./index.css";

// The quick search window (Settings → Quick search) is a page of its own, with only what it needs:
// it never loads the rest of the app.

// Release builds: hide the webview's own context menu (Reload, Inspect…) except where text can be copied.
if (!import.meta.env.DEV) {
  document.addEventListener("contextmenu", (e) => {
    if (!(e.target as Element).closest("input, textarea, .select-text")) e.preventDefault();
  });
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QuickSearch />
  </React.StrictMode>,
);
