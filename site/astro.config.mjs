// @ts-check
import { fileURLToPath } from "node:url";
import { defineConfig } from "astro/config";

export default defineConfig({
  site: "https://glint.rafd.tech",
  trailingSlash: "always",
  compressHTML: true,
  build: {
    format: "directory",
    // A few KB of CSS is cheaper inline than as a render-blocking request.
    inlineStylesheets: "always",
  },
  devToolbar: { enabled: false },
  vite: {
    build: {
      // Keep the WebGL field and GSAP in their own long-cacheable chunks.
      assetsInlineLimit: 2048,
    },
    // The screenshots are the README's, in the app's repository one folder up.
    server: { fs: { allow: [fileURLToPath(new URL("..", import.meta.url))] } },
  },
});
