import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { languageOf, setLanguage } from "./lib/i18n";
import { useStore } from "./lib/store";

// The interface's language before anything's drawn; a change of it opens the window again in it.
setLanguage(languageOf(useStore.getState().settings.language));
useStore.subscribe((now, before) => {
  if (now.settings.language !== before.settings.language) location.reload();
});

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
