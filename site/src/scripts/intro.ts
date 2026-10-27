/*
  The opening flash. The animation itself is CSS (Intro.astro); this aims the mark's flight at the
  header's mark, lifts the curtain on time, and lets any key, click, wheel or touch skip it.
*/
import type { App } from "./main";

const LIFT_MS = 1450; // when the curtain starts to lift, and the hero rises under it
const GONE_MS = 2200; // when the curtain is gone

export function playIntro(app: App): Promise<void> {
  const root = app.root;
  if (!root.classList.contains("intro")) {
    root.classList.add("intro-done");
    return Promise.resolve();
  }

  const mark = document.querySelector<HTMLElement>("[data-intro-mark]");
  const target = document.querySelector<HTMLElement>(".brand-mark");
  if (mark && target) {
    const m = mark.getBoundingClientRect();
    const t = target.getBoundingClientRect();
    mark.style.setProperty("--fly-x", `${t.left + t.width / 2 - (m.left + m.width / 2)}px`);
    mark.style.setProperty("--fly-y", `${t.top + t.height / 2 - (m.top + m.height / 2)}px`);
    mark.style.setProperty("--fly-s", `${t.width / m.width}`);
  }

  return new Promise((resolve) => {
    const events = ["pointerdown", "keydown", "wheel", "touchstart"] as const;
    let lifted = false;
    const lift = () => {
      if (lifted) return;
      lifted = true;
      events.forEach((e) => window.removeEventListener(e, skip));
      root.classList.add("intro-done");
      resolve();
    };
    const gone = () => root.classList.remove("intro", "intro-skip");
    const skip = () => {
      root.classList.add("intro-skip");
      lift();
      setTimeout(gone, 400);
    };
    events.forEach((e) => window.addEventListener(e, skip, { once: true, passive: true }));
    setTimeout(lift, LIFT_MS);
    setTimeout(gone, GONE_MS);
  });
}
