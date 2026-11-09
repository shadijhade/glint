import { defineConfig } from "vite";
import react, { reactCompilerPreset } from "@vitejs/plugin-react";
import babel from "@rolldown/plugin-babel";
import tailwindcss from "@tailwindcss/vite";
import path from "path";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  // React Compiler memoizes components and hooks by itself, so a store change re-renders only what
  // reads it (components that break the rules of React are left as written).
  plugins: [react(), babel({ presets: [reactCompilerPreset()] }), tailwindcss()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
        protocol: "ws",
        host,
        port: 1421,
      }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
    },
  },
  build: {
    // ghostty-web carries its WebAssembly inline (637 kB), loaded only when a terminal opens.
    chunkSizeWarningLimit: 700,
    rolldownOptions: {
      // The quick search window is a page of its own (quick.html), loading only what it shows.
      input: {
        main: path.resolve(import.meta.dirname, "index.html"),
        quick: path.resolve(import.meta.dirname, "quick.html"),
        // Export as PDF draws a Markdown file in a hidden window of its own (print.rs).
        print: path.resolve(import.meta.dirname, "print.html"),
      },
      output: {
        codeSplitting: {
          groups: [
            // What every page needs, React itself included: a group takes the modules it imports along,
            // so React left out of this one would land with whichever took it first (the highlighter,
            // which would then load with the app).
            { name: "vendor", test: /[\\/]node_modules[\\/](react|react-dom|scheduler|@tauri-apps|sonner|zustand)[\\/]/, priority: 2 },
            // Prism and its grammars, loaded the first time code is highlighted (highlight.tsx).
            { name: "syntax", test: /[\\/]node_modules[\\/](react-syntax-highlighter|refractor|prismjs)[\\/]/, priority: 1 },
          ],
        },
      },
    },
  },
}));
