/*
  The choreography. Everything here only adds motion to a page that is complete without it: reveals,
  count-ups, the blink replayed slowly, the list that becomes a map, the gallery that runs sideways,
  and the band of light across the footer. Chapter highlighting runs with or without motion.
*/
import { gsap } from "gsap";
import { ScrollTrigger } from "gsap/ScrollTrigger";
import type { App } from "./main";

export function initMotion(app: App) {
  ScrollTrigger.config({ ignoreMobileResize: true });
  cardGlow();
  if (app.motionOk) {
    reveals(app);
    counts(app);
    restOutOfSight();
  }
  // Every ScrollTrigger measures the page as it's made, so they wait until the first frame is on screen,
  // and each gets its own task: none of them holds back the first paint or makes one long task.
  const later = app.motionOk ? [activeChapters, listing, blink, sizes, gallery, footerSweep] : [activeChapters];
  const next = () => {
    const make = later.shift();
    if (!make) return;
    make();
    setTimeout(next, 0);
  };
  requestAnimationFrame(() => setTimeout(next, 0));
}

/* ---------- Reveals: once, as each thing comes into view; the hero as the flash lifts ---------- */
function reveals(app: App) {
  const io = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        e.target.classList.add("is-in");
        io.unobserve(e.target);
      }
    },
    { rootMargin: "0px 0px -10% 0px", threshold: 0.01 },
  );
  document.querySelectorAll("[data-reveal], [data-lines], [data-proof-row]").forEach((el) => {
    if (!el.closest("[data-hero-step]")) io.observe(el);
  });

  void app.introDone.then(() => {
    document.querySelectorAll<HTMLElement>("[data-hero-step]").forEach((step, i) => {
      step.style.transitionDelay = `${i * 80}ms`;
      step.querySelectorAll<HTMLElement>(".line-inner").forEach((inner, j) => (inner.style.transitionDelay = `${i * 80 + j * 90}ms`));
      requestAnimationFrame(() => step.classList.add("is-in"));
    });
  });
}

/* ---------- Numbers count up to what the page says, once the flash has lifted ---------- */
function counts(app: App) {
  const io = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        io.unobserve(e.target);
        const el = e.target as HTMLElement;
        const to = Number(el.dataset.count);
        const decimals = Number(el.dataset.decimals ?? 0);
        const n = { v: 0 };
        gsap.to(n, { v: to, duration: 1.8, delay: 0.15, ease: "power3.out", onUpdate: () => (el.textContent = n.v.toFixed(decimals)) });
      }
    },
    { rootMargin: "0px 0px -12% 0px" },
  );
  const all = document.querySelectorAll<HTMLElement>("[data-count]");
  all.forEach((el) => (el.textContent = (0).toFixed(Number(el.dataset.decimals ?? 0))));
  void app.introDone.then(() => all.forEach((el) => io.observe(el)));
}

/* ---------- 01 Open: System32's rows arrive, and the stopwatch settles on 12 ---------- */
function listing() {
  const el = document.querySelector<HTMLElement>("[data-listing]");
  const watch = el?.querySelector<HTMLElement>("[data-stopwatch]");
  if (!el || !watch) return;
  watch.textContent = "0.0";
  ScrollTrigger.create({
    trigger: el,
    start: "top 72%",
    once: true,
    onEnter: () => {
      el.classList.add("is-open");
      const n = { v: 0 };
      gsap.to(n, { v: 12, duration: 0.9, ease: "power2.out", onUpdate: () => (watch.textContent = n.v < 11.95 ? n.v.toFixed(1) : "12") });
    },
  });
}

/* ---------- 01 Open: 400 ms replayed eight times slower; the eyelids close at 100 ---------- */
function blink() {
  const fig = document.querySelector<HTMLElement>("[data-blink]");
  if (!fig) return;
  const sweep = fig.querySelector<HTMLElement>("[data-blink-sweep]");
  const marks = [...fig.querySelectorAll<HTMLElement>("[data-blink-mark]")];
  const lids = fig.querySelectorAll<HTMLElement>(".lid");
  const SECONDS = 3.2; // 400 ms, eight times slower
  let tl: gsap.core.Timeline | null = null;
  const play = () => {
    tl?.kill();
    marks.forEach((m) => m.classList.remove("is-in"));
    gsap.set(lids, { scaleY: 0 });
    tl = gsap.timeline();
    tl.set(sweep, { left: "0%", opacity: 1 });
    tl.to(sweep, { left: "100%", duration: SECONDS, ease: "none" }, 0);
    for (const m of marks) tl.call(() => m.classList.add("is-in"), [], (parseFloat(m.style.left) / 100) * SECONDS);
    const at = (100 / 400) * SECONDS;
    tl.to(lids, { scaleY: 1, duration: 0.14, ease: "power2.in" }, at);
    tl.to(lids, { scaleY: 0, duration: 0.32, ease: "power2.out" }, at + 0.2);
    tl.to(sweep, { opacity: 0, duration: 0.5 }, SECONDS);
  };
  ScrollTrigger.create({ trigger: fig, start: "top 72%", once: true, onEnter: play });
  fig.querySelector("[data-blink-replay]")?.addEventListener("click", play);
}

/* ---------- 02 Size: each bar of the list becomes its tile of the map ---------- */
function sizes() {
  const fig = document.querySelector<HTMLElement>("[data-sizes]");
  if (!fig) return;
  const blocks = [...fig.querySelectorAll<SVGRectElement>("rect.block")].map((r) => ({
    r,
    a: (r.dataset.bar ?? "").split(" ").map(Number),
    b: (r.dataset.tile ?? "").split(" ").map(Number),
  }));
  const rowText = fig.querySelectorAll<SVGTextElement>(".row-name, .row-size");
  const tileText = fig.querySelectorAll<SVGTextElement>(".tile-label");
  const ease = gsap.parseEase("power3.inOut");
  const attrs = ["x", "y", "width", "height"] as const;
  const draw = (p: number) => {
    blocks.forEach(({ r, a, b }, i) => {
      const e = ease(Math.min(1, Math.max(0, (p * 1.3 - i * 0.022) / 1)));
      attrs.forEach((k, j) => r.setAttribute(k, (a[j] + (b[j] - a[j]) * e).toFixed(2)));
    });
    const words = Math.max(0, 1 - p * 3.2);
    rowText.forEach((t) => (t.style.opacity = String(words)));
    const labels = Math.min(1, Math.max(0, (p - 0.72) / 0.28));
    tileText.forEach((t) => (t.style.opacity = String(labels)));
    fig.classList.toggle("is-map", p > 0.5);
  };
  ScrollTrigger.create({ trigger: fig, start: "center 72%", end: "center 30%", scrub: 0.6, onUpdate: (self) => draw(self.progress) });
}

/* ---------- 04 Everything else: pinned, the track runs sideways ---------- */
function gallery() {
  const mm = gsap.matchMedia();
  mm.add("(min-width: 901px)", () => {
    const section = document.querySelector<HTMLElement>("[data-gallery]");
    const track = section?.querySelector<HTMLElement>("[data-track]");
    const bar = section?.querySelector<HTMLElement>("[data-track-progress]");
    if (!section || !track) return;
    const distance = () => Math.max(0, track.scrollWidth - window.innerWidth);
    const tween = gsap.to(track, { x: () => -distance(), ease: "none" });
    const st = ScrollTrigger.create({
      trigger: section,
      start: "top top",
      end: () => `+=${distance()}`,
      pin: true,
      scrub: 0.8,
      animation: tween,
      invalidateOnRefresh: true,
      anticipatePin: 1,
      onUpdate: (self) => bar && (bar.style.transform = `scaleX(${self.progress})`),
    });
    return () => {
      st.kill();
      tween.kill();
      gsap.set(track, { clearProps: "transform" });
    };
  });
}

/* ---------- The footer: a band of light crosses the name as the page ends ---------- */
function footerSweep() {
  const word = document.querySelector<HTMLElement>("[data-footer-word]");
  if (!word) return;
  gsap.fromTo(word, { "--sweep": "-30%" }, { "--sweep": "130%", ease: "none", scrollTrigger: { trigger: word, start: "top bottom", end: "bottom 70%", scrub: 0.6 } });
}

/* ---------- The chapter on screen lights its link in the header and its mark on the rail ---------- */
function activeChapters() {
  const set = (id: string) => {
    document.querySelectorAll<HTMLElement>("[data-chapter-link]").forEach((a) => a.classList.toggle("is-active", a.dataset.chapterLink === id));
    document.querySelectorAll<HTMLElement>("[data-rail-link]").forEach((a) => a.classList.toggle("is-active", a.dataset.railLink === id));
  };
  document.querySelectorAll<HTMLElement>("[data-chapter]").forEach((ch) => {
    ScrollTrigger.create({ trigger: ch, start: "top center", end: "bottom center", onToggle: (self) => self.isActive && set(ch.dataset.chapter ?? "") });
  });
  set("top");
}

/* ---------- Cards: a soft light follows the pointer ---------- */
function cardGlow() {
  if (!window.matchMedia("(hover: hover) and (pointer: fine)").matches) return;
  document.querySelectorAll<HTMLElement>("[data-tilt]").forEach((card) => {
    card.addEventListener(
      "pointermove",
      (e) => {
        const r = card.getBoundingClientRect();
        card.style.setProperty("--mx", `${(((e.clientX - r.left) / r.width) * 100).toFixed(1)}%`);
        card.style.setProperty("--my", `${(((e.clientY - r.top) / r.height) * 100).toFixed(1)}%`);
      },
      { passive: true },
    );
  });
}

/* ---------- Ambient loops rest while their chapter is off screen ---------- */
function restOutOfSight() {
  const io = new IntersectionObserver(
    (entries) => {
      for (const e of entries) e.target.classList.toggle("is-resting", !e.isIntersecting);
    },
    { rootMargin: "120px 0px" },
  );
  document.querySelectorAll("[data-chapter], [data-footer]").forEach((el) => io.observe(el));
}
