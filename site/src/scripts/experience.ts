/*
  Everything that moves, loaded once the first frame is on screen (main.ts): GSAP and ScrollTrigger,
  Lenis for the wheel, the choreography (motion.ts), and, once the page has gone idle, the field of light.
*/
import { gsap } from "gsap";
import { ScrollTrigger } from "gsap/ScrollTrigger";
import Lenis from "lenis";
import type { App } from "./main";
import { initMotion } from "./motion";

gsap.registerPlugin(ScrollTrigger);

export function startExperience(app: App) {
  const finePointer = window.matchMedia("(hover: hover) and (pointer: fine)").matches;

  /* Smooth scrolling is for the wheel. On a touch screen the browser's own scroll is smooth already and
     runs off the main thread, and Lenis's touch listeners can't be passive. */
  if (app.motionOk && finePointer) {
    const lenis = new Lenis({ lerp: 0.09, wheelMultiplier: 0.95 });
    lenis.on("scroll", ScrollTrigger.update);
    lenis.on("scroll", ({ scroll }: { scroll: number }) => (app.scrollY = scroll));
    gsap.ticker.add((time) => lenis.raf(time * 1000));
    gsap.ticker.lagSmoothing(0);
    app.lenis = lenis;
    // The page holds still while the flash plays.
    if (app.root.classList.contains("intro") && !app.root.classList.contains("intro-done")) {
      lenis.stop();
      void app.introDone.then(() => lenis.start());
    }
  }

  let lastY = app.scrollY;
  gsap.ticker.add((_time, deltaMs) => {
    const dt = Math.max(1, deltaMs) / 1000;
    app.velocity += ((app.scrollY - lastY) / dt - app.velocity) * Math.min(1, dt * 8);
    lastY = app.scrollY;
  });

  initMotion(app);

  if (app.motionOk) {
    if ("requestIdleCallback" in window) requestIdleCallback(() => startField(app, finePointer), { timeout: 1600 });
    else setTimeout(() => startField(app, finePointer), 500);
  }
}

/* The field of light: WebGL2 only, with motion only. Until it draws, the still picture stands in. */
function startField(app: App, finePointer: boolean) {
  const canvas = document.querySelector<HTMLCanvasElement>("[data-field]");
  if (!canvas || !("WebGL2RenderingContext" in window)) return;
  void import("./field").then(({ createField }) => {
    const field = createField(canvas);
    if (!field) return;
    app.field = field;
    gsap.ticker.add((time) => {
      if (!app.paused()) field.tick(time, app.scrollY, app.velocity);
    });
    void app.introDone.then(() => field.gather());
    if (finePointer) {
      window.addEventListener("pointermove", (e) => field.setPointer(e.clientX, e.clientY, true), { passive: true });
      document.addEventListener("pointerleave", () => field.setPointer(-9999, -9999, false));
    }
    ScrollTrigger.addEventListener("refresh", () => field.measure());
    window.addEventListener("glint:query", (e) => {
      const { hash, frac, active } = (e as CustomEvent<{ hash: number; frac: number; active: boolean }>).detail;
      field.setQuery(hash, frac, active);
    });
    // The search may have run before the field was there.
    window.dispatchEvent(new Event("glint:field"));
  });
}
