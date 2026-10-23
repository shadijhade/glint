import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "path";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

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
      output: {
        manualChunks(id) {
          if (id.includes("react-syntax-highlighter") || id.includes("refractor") || id.includes("prismjs")) {
            return "syntax";
          }
          if (id.includes("lucide-react")) {
            return "icons";
          }
          if (id.includes("node_modules") && (id.includes("react-dom") || id.includes("@tauri-apps") || id.includes("sonner") || id.includes("zustand"))) {
            return "vendor";
          }
        },
      },
    },
  },
}));
