/*
  The page's controls: the latest release's details on every installer link, copy buttons, the phone
  menu, in-page links, the pause control, and the header's state.
*/
import { SITE } from "../content/site";
import type { App } from "./main";

export function initUI(app: App) {
  release();
  copyButtons();
  menu(app);
  links(app);
  motionToggle(app);
  header(app);
}

interface Asset {
  name: string;
  size: number;
  browser_download_url: string;
}

/* The links work as written (releases/latest/download/Glint-setup.exe); the latest release adds its
   version and size, and an offline installer when it has one. */
function release() {
  fetch(SITE.latestApi, { headers: { Accept: "application/vnd.github+json" } })
    .then((r) => (r.ok ? r.json() : null))
    .then((rel: { tag_name: string; html_url: string; assets: Asset[] } | null) => {
      if (!rel) return;
      const assets = rel.assets ?? [];
      const setup = assets.find((a) => a.name === "Glint-setup.exe") ?? assets.find((a) => a.name.endsWith("_x64-setup.exe"));
      if (!setup) return;
      const version = rel.tag_name.replace(/^v/, "");
      document.querySelectorAll<HTMLAnchorElement>("[data-download]").forEach((a) => (a.href = setup.browser_download_url));
      document.querySelectorAll("[data-release]").forEach((s) => (s.textContent = `Version ${version} · installer ${Math.round(setup.size / 1048576)} MB · `));
      document.querySelectorAll<HTMLAnchorElement>("[data-notes]").forEach((a) => (a.href = rel.html_url));
      const offline = assets.find((a) => a.name.endsWith("_offline-setup.exe"));
      if (offline) {
        document.querySelectorAll<HTMLAnchorElement>("[data-offline-link]").forEach((a) => (a.href = offline.browser_download_url));
        document.querySelectorAll<HTMLElement>("[data-offline]").forEach((el) => (el.hidden = false));
      }
    })
    .catch(() => {});
}

function copyButtons() {
  document.querySelectorAll<HTMLButtonElement>("[data-copy]").forEach((button) => {
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(button.dataset.copy ?? "");
        button.classList.add("is-copied");
        setTimeout(() => button.classList.remove("is-copied"), 1600);
      } catch {
        /* No clipboard access: the command is on screen to select. */
      }
    });
  });
}

function menu(app: App) {
  const toggle = document.querySelector<HTMLButtonElement>("[data-menu-toggle]");
  const panel = document.querySelector<HTMLElement>("[data-menu]");
  if (!toggle || !panel) return;
  const set = (open: boolean) => {
    panel.hidden = !open;
    toggle.setAttribute("aria-expanded", String(open));
    app.root.classList.toggle("menu-open", open);
    if (open) app.lenis?.stop();
    else app.lenis?.start();
  };
  toggle.addEventListener("click", () => set(panel.hidden));
  panel.querySelectorAll("a").forEach((a) => a.addEventListener("click", () => set(false)));
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !panel.hidden) {
      set(false);
      toggle.focus();
    }
  });
}

function links(app: App) {
  document.querySelectorAll<HTMLAnchorElement>("a[data-nav-link]").forEach((a) => {
    const href = a.getAttribute("href") ?? "";
    if (!href.startsWith("#")) return;
    a.addEventListener("click", (e) => {
      const target = document.querySelector<HTMLElement>(href);
      if (!target) return;
      e.preventDefault();
      app.scrollTo(href === "#top" ? 0 : target);
      history.replaceState(null, "", href === "#top" ? location.pathname : href);
    });
  });
}

/* The pause control stops every autonomous motion (the field, ambient loops) and remembers the choice. */
function motionToggle(app: App) {
  const buttons = document.querySelectorAll<HTMLButtonElement>("[data-motion-toggle]");
  const sync = () => buttons.forEach((b) => b.setAttribute("aria-pressed", String(app.paused())));
  sync();
  buttons.forEach((b) =>
    b.addEventListener("click", () => {
      const pause = !app.paused();
      app.root.classList.toggle("motion-paused", pause);
      app.root.classList.toggle("motion-live", !pause);
      try {
        localStorage.setItem("glint-motion", pause ? "off" : "on");
      } catch {
        /* Private mode: the choice lasts until the page closes. */
      }
      sync();
    }),
  );
}

function header(app: App) {
  const el = document.querySelector<HTMLElement>("[data-header]");
  if (!el) return;
  let scrolled = false;
  const update = () => {
    const now = app.scrollY > 24;
    if (now !== scrolled) el.classList.toggle("is-scrolled", (scrolled = now));
  };
  update();
  window.addEventListener("scroll", update, { passive: true });
  app.lenis?.on("scroll", update);
}
