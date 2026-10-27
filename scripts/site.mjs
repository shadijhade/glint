// Puts the website together in _site/ (docs/releasing.md#the-website): site/ as it is, with the README's
// screenshots and the app's icons, so the page and the README show the same pictures. The Pages workflow
// publishes _site/; open _site/index.html to look at it first.
//
// Usage: npm run site

import { cpSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const out = join(root, "_site");
rmSync(out, { recursive: true, force: true });
cpSync(join(root, "site"), out, { recursive: true });
cpSync(join(root, "screenshots"), join(out, "screenshots"), { recursive: true });
cpSync(join(root, "src", "assets", "logo.svg"), join(out, "logo.svg"));
cpSync(join(root, "src-tauri", "icons", "128x128.png"), join(out, "icon.png"));
console.log(`✓ ${join(out, "index.html")}`);
