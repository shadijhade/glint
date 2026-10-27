/*
  Client entry. The page is complete without any of this: every hidden state in the stylesheet is gated
  on .motion-ok, set before first paint by the boot script in Base.astro. This file only adds behaviour,
  in two steps: what the first frame needs (the flash's timing, the buttons, the search) is here, and
  the rest (GSAP, smooth scrolling, the choreography, the field of light) arrives in experience.ts once
  that frame is on screen, so no script stands between a visitor and the first paint.
*/
import type Lenis from "lenis";
import type { Field } from "./field";
import { playIntro } from "./intro";
import { initSearch } from "./search";
import { initUI } from "./ui";

export interface App {
  root: HTMLElement;
  motionOk: boolean;
  /** Smooth scrolling, once experience.ts has started it (wheel and trackpad only). */
  lenis: Lenis | null;
  /** The scroll offset, kept current by the scroll itself (reading window.scrollY mid-frame forces layout). */
  scrollY: number;
  /** Smoothed scroll velocity in px/s, positive downwards. */
  velocity: number;
  /** Settles when the opening flash lifts, or at once if there is none. */
  introDone: Promise<void>;
  field: Field | null;
  paused(): boolean;
  scrollTo(target: string | HTMLElement | number): void;
}

const root = document.documentElement;
const motionOk = root.classList.contains("motion-ok");

const app: App = {
  root,
  motionOk,
  lenis: null,
  scrollY: window.scrollY,
  velocity: 0,
  introDone: Promise.resolve(),
  field: null,
  paused: () => root.classList.contains("motion-paused"),
  scrollTo(target) {
    const el = typeof target === "string" ? document.querySelector<HTMLElement>(target) : target;
    if (app.lenis) app.lenis.scrollTo(el ?? 0, { duration: 1.5, easing: (t: number) => 1 - Math.pow(1 - t, 4) });
    else if (typeof el === "number") window.scrollTo({ top: el, behavior: motionOk ? "smooth" : "auto" });
    else el?.scrollIntoView({ behavior: motionOk ? "smooth" : "auto" });
  },
};

app.introDone = playIntro(app);
window.addEventListener("scroll", () => (app.scrollY = window.scrollY), { passive: true });

initUI(app);
initSearch(app);

requestAnimationFrame(() => setTimeout(() => void import("./experience").then(({ startExperience }) => startExperience(app)), 0));
