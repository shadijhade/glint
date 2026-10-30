// Draws the installer's artwork (src-tauri/installer/art): the picture beside the welcome and finish
// pages and the one in every other page's header, at each common scaling from 100% to 300%, so neither
// is stretched (installer/look.nsh picks the one for the screen). Edge draws each from the HTML
// below, with the logo from src/assets/logo.svg; Windows' System.Drawing then makes the 24-bit BMPs that
// NSIS takes.
//
// Usage: node scripts/installer-art.mjs

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(import.meta.dirname, "..");
const out = join(root, "src-tauri", "installer", "art");
const edge = ["ProgramFiles(x86)", "ProgramFiles"].map((v) => join(process.env[v] ?? "", "Microsoft", "Edge", "Application", "msedge.exe")).find(existsSync);
if (!edge) throw new Error("Microsoft Edge wasn't found");

// The pictures' sizes in pixels at each scaling, so neither is ever stretched: MUI's controls in dialog
// units (the picture 109 × 193, the header's 100 × 35) in Segoe UI 9, the installer's font, as Windows
// turns them into pixels (its font's average letter width and height there, which don't grow in step
// with the scaling: 7 × 15 at 100%, 8 × 20 at 125%, 10 × 25 at 150%… 20 × 48 at 300%).
const SIZES = {
    100: { sidebar: [191, 362], header: [175, 66] },
    125: { sidebar: [218, 483], header: [200, 88] },
    150: { sidebar: [273, 603], header: [250, 109] },
    175: { sidebar: [327, 724], header: [300, 131] },
    200: { sidebar: [354, 772], header: [325, 140] },
    225: { sidebar: [409, 893], header: [375, 162] },
    250: { sidebar: [463, 989], header: [425, 179] },
    300: { sidebar: [545, 1158], header: [500, 210] },
};

const logo = `data:image/svg+xml;base64,${readFileSync(join(root, "src", "assets", "logo.svg")).toString("base64")}`;
const font = "font-family: 'Segoe UI Variable Display', 'Segoe UI', sans-serif;";

// Each is drawn in a box of the picture's size (Edge's window can be wider than asked).
const box = (w, h, style, content) => `<body style="margin: 0; overflow: hidden; background: #fff">
<div style="width: ${w}px; height: ${h}px; overflow: hidden; box-sizing: border-box; ${font} ${style}">${content}</div></body>`;

/** The welcome and finish pages' picture: the logo glowing on Glint's dark purple, its name and what it is. */
const sidebar = (w, h) => box(w, h, `
    background: radial-gradient(circle at 50% ${h * 0.215 + w * 0.25}px, rgb(236 72 153 / 0.32), rgb(236 72 153 / 0) ${w * 0.6}px),
        linear-gradient(172deg, #2f1247 0%, #1f0c31 55%, #140720 100%);
    display: flex; flex-direction: column; align-items: center; padding-top: ${h * 0.215}px; color: #fff`, `
    <img src="${logo}" style="width: ${w * 0.5}px; height: ${w * 0.5}px; filter: drop-shadow(0 ${w * 0.02}px ${w * 0.06}px rgb(0 0 0 / 0.35))">
    <div style="margin-top: ${w * 0.09}px; font-size: ${w * 0.215}px; font-weight: 600; letter-spacing: -0.01em; line-height: 1">Glint</div>
    <div style="margin-top: ${w * 0.065}px; font-size: ${w * 0.074}px; line-height: 1.4; color: #dccbf2; text-align: center">The fast file explorer<br>for Windows</div>`);

/** The header's: the logo and the name, on MUI's white header. */
const header = (w, h) => box(w, h, `background: #fff; display: flex; align-items: center; padding-left: ${h * 0.2}px`, `
    <img src="${logo}" style="width: ${h * 0.62}px; height: ${h * 0.62}px">
    <div style="margin-left: ${h * 0.14}px; font-size: ${h * 0.4}px; font-weight: 600; letter-spacing: -0.01em; color: #1b1b1f">Glint</div>`);

const work = mkdtempSync(join(tmpdir(), "glint-art-"));
mkdirSync(out, { recursive: true });
const pngs = [];
for (const [scale, size] of Object.entries(SIZES)) {
    for (const [name, draw] of [["sidebar", sidebar], ["header", header]]) {
        const [w, h] = size[name];
        const html = join(work, `${name}-${scale}.html`);
        const png = join(work, `${name}-${scale}.png`);
        writeFileSync(html, `<!doctype html><meta charset="utf-8">${draw(w, h)}`);
        // Text smoothed in grey (--disable-lcd-text): ClearType's coloured edges belong to the screen it was
        // drawn for, not in a picture.
        spawnSync(edge, ["--headless=new", "--disable-gpu", "--disable-lcd-text", "--hide-scrollbars", "--force-device-scale-factor=1", `--user-data-dir=${join(work, "profile")}`,
            `--window-size=${w},${h}`, `--screenshot=${png}`, pathToFileURL(html).href], { stdio: "ignore", timeout: 60_000 });
        // msedge.exe can hand the page to a browser process of its own and end first.
        for (let waited = 0; !existsSync(png) && waited < 30_000; waited += 250) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 250);
        if (!existsSync(png)) throw new Error(`Edge didn't draw ${name}-${scale}`);
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 500);
        pngs.push([png, join(out, `${name}-${scale}.bmp`), w, h]);
    }
}
// PNG to a 24-bit BMP of exactly that size.
const convert = pngs.map(([png, bmp, w, h]) =>
    `$i = [Drawing.Image]::FromFile('${png}'); $b = New-Object Drawing.Bitmap ${w}, ${h}, ([Drawing.Imaging.PixelFormat]::Format24bppRgb); ` +
    `$g = [Drawing.Graphics]::FromImage($b); $g.DrawImage($i, (New-Object Drawing.Rectangle 0, 0, ${w}, ${h})); $b.Save('${bmp}', [Drawing.Imaging.ImageFormat]::Bmp); $g.Dispose(); $b.Dispose(); $i.Dispose()`).join("\n");
const ps = spawnSync("powershell", ["-NoProfile", "-Command", `Add-Type -AssemblyName System.Drawing\n${convert}`], { stdio: "inherit" });
rmSync(work, { recursive: true, force: true });
if (ps.status) throw new Error("Couldn't write the BMPs");
console.log(`Drew ${pngs.length} pictures into ${out}`);
