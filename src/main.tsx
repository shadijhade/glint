import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";

// Release builds: hide the webview's own context menu (Reload, Inspect…) except where text can be copied.
if (!import.meta.env.DEV) {
  document.addEventListener("contextmenu", (e) => {
    if (!(e.target as Element).closest("input, textarea, .select-text")) e.preventDefault();
  });
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
