/*
  Where every point of the field stands in each chapter, in CSS pixels of the viewport. A point keeps
  its index (and its seeded randomness) in every scene, so it flows from one shape into the next:

    0  the drive      every name on a disk, as a turning galaxy (the hero)
    1  the list       rows of names and sizes (Open, Proof)
    2  the map        C:\Windows's folders as a treemap (Size)
    3  the index      a ring; the names that match the search fall into it (Find)
    4  the horizon    a floor of light to the horizon (Everything else, Still Windows)
    5  the mark       Glint's own logo, in its colours (Get Glint)
*/
import { SIZE } from "../content/site";
import { squarify } from "./treemap";

export const SCENES = 6;

export interface LogoSample {
  /** Points inside the mark, 0..1 across its box. */
  x: Float32Array;
  y: Float32Array;
  /** Their colours, 0..1. */
  rgb: Float32Array;
  count: number;
}

export interface Layout {
  pos: Float32Array[];
  /** The drive's centre and radius, which the shader turns the galaxy around. */
  center: [number, number];
  radius: number;
  /** The ring's centre, where the names that match the search gather. */
  ring: [number, number];
}

/** Deterministic randomness (mulberry32): a resize rebuilds the same field. */
export function rng(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const gaussOf = (r: () => number) => () => Math.sqrt(-2 * Math.log(r() || 1e-9)) * Math.cos(2 * Math.PI * r());

/** Each point's seed: x random, y size, z phase, w brightness. */
export function seeds(n: number): Float32Array {
  const r = rng(11);
  const out = new Float32Array(n * 4);
  for (let i = 0; i < n; i++) {
    out[i * 4] = r();
    out[i * 4 + 1] = Math.pow(r(), 2.2);
    out[i * 4 + 2] = r();
    out[i * 4 + 3] = 0.25 + 0.75 * Math.pow(r(), 1.6);
  }
  return out;
}

/** Each point's own colour: mostly the folder's golds and the sparkle's pinks, a little starlight. */
export function colors(n: number): Float32Array {
  const r = rng(23);
  const palette = [
    [1.0, 0.85, 0.42],
    [1.0, 0.72, 0.18],
    [1.0, 0.56, 0.82],
    [0.93, 0.29, 0.6],
    [0.86, 0.8, 1.0],
    [1.0, 1.0, 1.0],
  ];
  const weights = [0.3, 0.14, 0.24, 0.08, 0.14, 0.1];
  const out = new Float32Array(n * 3);
  for (let i = 0; i < n; i++) {
    let u = r();
    let k = 0;
    while (k < weights.length - 1 && u > weights[k]) u -= weights[k++];
    out.set(palette[k], i * 3);
  }
  return out;
}

export function layout(n: number, vw: number, vh: number, logo: LogoSample | null): Layout {
  const wide = vw >= 900;
  const pos = Array.from({ length: SCENES }, () => new Float32Array(n * 2));

  // 0. The drive: three arms, a bright core, and the rest of the sky.
  const cx = wide ? vw * 0.73 : vw * 0.5;
  // On a phone the copy fills the hero, so the drive sits high and smaller, above the headline.
  const cy = wide ? vh * 0.48 : vh * 0.2;
  const R = wide ? Math.min(vw * 0.42, vh * 0.66) : Math.max(vw * 0.52, 190);
  {
    const r = rng(101);
    const g = gaussOf(r);
    const p = pos[0];
    for (let i = 0; i < n; i++) {
      const u = r();
      let x: number;
      let y: number;
      if (u < 0.12) {
        x = cx + g() * R * 0.07;
        y = cy + g() * R * 0.045;
      } else if (u < 0.26) {
        x = r() * vw;
        y = r() * vh;
      } else {
        const arm = Math.floor(r() * 3);
        const t = Math.pow(r(), 0.7);
        const rad = R * (0.08 + 0.92 * t);
        const ang = (arm * 2 * Math.PI) / 3 + t * 3.6 + g() * 0.28 * (1.1 - t);
        x = cx + Math.cos(ang) * rad + g() * R * 0.03;
        y = cy + Math.sin(ang) * rad * 0.58 + g() * R * 0.025;
      }
      p[i * 2] = x;
      p[i * 2 + 1] = y;
    }
  }

  // 1. The list: rows of names across the whole screen, a size at the end of some, dust between.
  // It sits behind the page's own list, so it stays a texture, not a second list.
  {
    const r = rng(202);
    const g = gaussOf(r);
    const p = pos[1];
    const x0 = vw * 0.04;
    const x1 = vw * 0.96;
    const y0 = vh * 0.08;
    const y1 = vh * 0.94;
    const rowH = Math.min(30, Math.max(18, (y1 - y0) / 26));
    const rows = Math.floor((y1 - y0) / rowH);
    const name = Array.from({ length: rows }, () => 0.22 + 0.42 * r());
    const size = Array.from({ length: rows }, () => (r() < 0.7 ? 0.05 + 0.2 * r() : 0));
    const w = x1 - x0;
    for (let i = 0; i < n; i++) {
      const u = r();
      if (u < 0.3) {
        p[i * 2] = r() * vw;
        p[i * 2 + 1] = r() * vh;
        continue;
      }
      const row = Math.floor(r() * rows);
      const y = y0 + row * rowH + g() * 0.9;
      const inSize = size[row] > 0 && r() < 0.28;
      p[i * 2] = inSize ? x1 - r() * size[row] * w : x0 + r() * name[row] * w;
      p[i * 2 + 1] = y;
    }
  }

  // 2. The map: C:\Windows's folders as tiles across the whole screen, lit along their edges. The page's
  // card draws the same map up close; this is the room around it.
  {
    const r = rng(303);
    const g = gaussOf(r);
    const p = pos[2];
    const box = { x: vw * 0.03, y: vh * 0.06, w: vw * 0.94, h: vh * 0.88 };
    const tiles = squarify(SIZE.folders, (f) => f.gb, box);
    const weights = tiles.map((t) => Math.pow(t.w * t.h, 0.75) + (t.w + t.h) * 6);
    const total = weights.reduce((a, b) => a + b, 0);
    const cum: number[] = [];
    weights.reduce((acc, w, k) => (cum[k] = acc + w / total), 0);
    for (let i = 0; i < n; i++) {
      if (r() < 0.12) {
        p[i * 2] = r() * vw;
        p[i * 2 + 1] = r() * vh;
        continue;
      }
      const u = r();
      let k = cum.findIndex((c) => u <= c);
      if (k < 0) k = tiles.length - 1;
      const t = tiles[k];
      const gap = 5;
      const x = t.x + gap;
      const y = t.y + gap;
      const w = Math.max(1, t.w - 2 * gap);
      const h = Math.max(1, t.h - 2 * gap);
      if (r() < 0.9) {
        // Along the edge.
        const along = r() * 2 * (w + h);
        if (along < w) [p[i * 2], p[i * 2 + 1]] = [x + along, y];
        else if (along < w + h) [p[i * 2], p[i * 2 + 1]] = [x + w, y + along - w];
        else if (along < 2 * w + h) [p[i * 2], p[i * 2 + 1]] = [x + w - (along - w - h), y + h];
        else [p[i * 2], p[i * 2 + 1]] = [x, y + h - (along - 2 * w - h)];
        p[i * 2] += g() * 0.7;
        p[i * 2 + 1] += g() * 0.7;
      } else {
        p[i * 2] = x + r() * w;
        p[i * 2 + 1] = y + r() * h;
      }
    }
  }

  // 3. The index: a ring around the search, a soft cloud in it, dust beyond.
  const rx = wide ? vw * 0.73 : vw * 0.5;
  const ry = vh * 0.5;
  {
    const r = rng(404);
    const g = gaussOf(r);
    const p = pos[3];
    const RR = wide ? Math.min(vw * 0.22, vh * 0.38) : vw * 0.42;
    for (let i = 0; i < n; i++) {
      const u = r();
      if (u < 0.62) {
        const a = r() * Math.PI * 2;
        const rad = RR * (1 + g() * 0.045);
        p[i * 2] = rx + Math.cos(a) * rad;
        p[i * 2 + 1] = ry + Math.sin(a) * rad;
      } else if (u < 0.8) {
        p[i * 2] = rx + g() * RR * 0.32;
        p[i * 2 + 1] = ry + g() * RR * 0.32;
      } else {
        p[i * 2] = r() * vw;
        p[i * 2 + 1] = r() * vh;
      }
    }
  }

  // 4. The horizon: a floor of light running away to a bright line, and stars above it.
  {
    const r = rng(505);
    const g = gaussOf(r);
    const p = pos[4];
    const horizon = vh * 0.66;
    for (let i = 0; i < n; i++) {
      const u = r();
      if (u < 0.16) {
        p[i * 2] = r() * vw;
        p[i * 2 + 1] = r() * horizon * 0.96;
        continue;
      }
      if (u < 0.26) {
        p[i * 2] = r() * vw;
        p[i * 2 + 1] = horizon + g() * 1.6;
        continue;
      }
      // Lines across (depth) and lines towards the vanishing point.
      const across = r() < 0.5;
      const depth = across ? Math.floor(r() * 14) / 14 : r();
      const v = Math.pow(depth, 2.1);
      const lane = across ? r() * 2 - 1 : (Math.floor(r() * 25) / 24) * 2 - 1;
      p[i * 2] = vw * 0.5 + lane * vw * (0.16 + v * 1.25);
      p[i * 2 + 1] = horizon + v * vh * 0.5 + g() * 0.6;
    }
  }

  // 5. The mark: Glint's logo, drawn by the points that fall inside it.
  {
    const p = pos[5];
    if (!logo || !logo.count) p.set(pos[0]);
    else {
      // Above the download, in the space Get.astro leaves for it.
      const r = rng(606);
      const S = wide ? Math.min(vw * 0.3, vh * 0.44) : Math.min(vw * 0.5, vh * 0.24);
      const lx = vw * 0.5 - S / 2;
      const ly = (wide ? vh * 0.29 : vh * 0.19) - S / 2;
      for (let i = 0; i < n; i++) {
        if (r() < 0.14) {
          p[i * 2] = r() * vw;
          p[i * 2 + 1] = r() * vh;
          continue;
        }
        const k = Math.floor(r() * logo.count);
        p[i * 2] = lx + (logo.x[k] + (r() - 0.5) / 256) * S;
        p[i * 2 + 1] = ly + (logo.y[k] + (r() - 0.5) / 256) * S;
      }
    }
  }

  return { pos, center: [cx, cy], radius: R, ring: [rx, ry] };
}

/** The mark's colour at each point's place in scene 5 (the rest keep their own). */
export function logoColors(n: number, own: Float32Array, logo: LogoSample | null): Float32Array {
  const out = new Float32Array(own);
  if (!logo || !logo.count) return out;
  const r = rng(606);
  for (let i = 0; i < n; i++) {
    if (r() < 0.14) {
      r();
      r();
      continue;
    }
    const k = Math.floor(r() * logo.count);
    r();
    r();
    out[i * 3] = logo.rgb[k * 3];
    out[i * 3 + 1] = logo.rgb[k * 3 + 1];
    out[i * 3 + 2] = logo.rgb[k * 3 + 2];
  }
  return out;
}

/** Sample the mark: every opaque pixel of the logo drawn at 256 px, with its colour. */
export async function sampleLogo(url: string): Promise<LogoSample | null> {
  try {
    const img = new Image();
    img.decoding = "async";
    img.src = url;
    await img.decode();
    const size = 256;
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = size;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) return null;
    ctx.drawImage(img, 0, 0, size, size);
    const data = ctx.getImageData(0, 0, size, size).data;
    const xs: number[] = [];
    const ys: number[] = [];
    const rgb: number[] = [];
    for (let y = 0; y < size; y++) {
      for (let x = 0; x < size; x++) {
        const o = (y * size + x) * 4;
        if (data[o + 3] < 160) continue;
        xs.push(x / size);
        ys.push(y / size);
        rgb.push(data[o] / 255, data[o + 1] / 255, data[o + 2] / 255);
      }
    }
    return { x: Float32Array.from(xs), y: Float32Array.from(ys), rgb: Float32Array.from(rgb), count: xs.length };
  } catch {
    return null;
  }
}
