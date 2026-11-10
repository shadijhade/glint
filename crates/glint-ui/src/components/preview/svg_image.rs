//! SVG drawn as a GPUI image, cached by its source and size: the pipeline diagrams and math share
//! (docs/gpui/previews-native.md sections 4-7).
//!
//! Mermaid ([`super::diagram`]), Graphviz and RaTeX ([`super::math`]) all end in an SVG string. GPUI rasterises an
//! SVG with resvg, so the same three steps serve all of them:
//!
//! 1. **Ask** ([`request`]) for a picture by [`Key`] (what it is, its source, its theme, its size). A picture
//!    already made comes straight back. A picture being made is waited for. Otherwise a job starts on a
//!    background thread and the view that asked is woken when it finishes (never from inside a draw: the
//!    wake-up comes from the foreground task that collects the job).
//! 2. **Make** ([`rasterize`], on the background thread): parse the SVG with GPUI's own renderer (so what fails
//!    here fails there, and is reported), pick a scale that keeps the picture under [`MAX_PIXELS`], and render it
//!    to a `RenderImage` at twice that for sharp text.
//! 3. **Draw** ([`picture`]) the image at its natural size, shrinking to the width it has; or, when it could not
//!    be made, the [`error_banner`] with the diagnostic and the source: never an empty box.
//!
//! The cache is bounded by count ([`MAX_ENTRIES`]) and by pixels held ([`MAX_BYTES`]); the least recently used
//! picture goes first and is handed back to the window by [`release_retired`] so the GPU's atlas lets go of it.
//! Failures are cached too (a source that cannot be drawn is not tried again each frame), at no pixel cost.
//!
//! A job carries a [`Ctl`]: a deadline, and a flag set when every view that waited for it has gone
//! ([`forget`]), so work that has not started is skipped and work that can be stopped (Mermaid's operation
//! control) is.
//!
//! Not ported: nothing from the TypeScript, which put the SVG in an `<img>` (`data:image/svg+xml`); the text of
//! a Mermaid SVG is drawn by resvg with the system fonts here, not by the page's.

use crate::kit::prelude::*;
use crate::{t, theme};
use gpui_kit::{img, EntityId, ImageSource, ObjectFit, RenderImage, StyledImage as _, SvgRenderer, SvgSize};
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// The most pictures (and failures) kept.
pub const MAX_ENTRIES: usize = 64;
/// The most pixel memory the kept pictures may hold (BGRA, 4 bytes a pixel).
pub const MAX_BYTES: usize = 96 << 20;
/// The most pixels one picture may have; a bigger SVG is drawn smaller (about 32 MB).
pub const MAX_PIXELS: f64 = 8_000_000.;
/// The longest side, in drawn pixels, one picture may have.
pub const MAX_SIDE: f64 = 6000.;
/// How much sharper than its logical size a picture is rasterised (GPUI's own factor).
const SHARPNESS: f32 = 2.;
/// The scale of the throwaway render that measures an SVG.
const PROBE: f32 = 0.25;
/// The most lines and characters of source an error banner shows.
const BANNER_LINES: usize = 40;
const BANNER_CHARS: usize = 6000;

/// What makes two pictures the same: what drew them, from what, in which theme, at what size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    /// A hash of the kind and the source.
    pub hash: u64,
    /// The size asked for, as `f32` bits (0 where the size is the SVG's own).
    pub size_bits: u32,
    /// The dark colour set.
    pub dark: bool,
}

impl Key {
    /// The key of a `kind` ("mermaid", "dot", "math", "display-math") drawn from `source`, at `size` (the font
    /// size for math, 0 otherwise), in the light or dark colour set.
    pub fn new(kind: &str, source: &str, size: f32, dark: bool) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        kind.hash(&mut hasher);
        source.hash(&mut hasher);
        Key {
            hash: hasher.finish(),
            size_bits: size.to_bits(),
            dark,
        }
    }
}

/// A picture made: the image, and the size it is drawn at (logical pixels) and its natural size.
#[derive(Clone)]
pub struct Drawn {
    /// The raster, at twice the logical size.
    pub image: Arc<RenderImage>,
    /// The width it is drawn at, in logical pixels.
    pub width: f32,
    /// The height it is drawn at, in logical pixels.
    pub height: f32,
    /// How much smaller than the SVG's own size it is drawn (1 unless it was too big).
    pub scale: f32,
    /// Where the text's baseline is, in drawn pixels from the top: set for a formula, so it can sit on the line
    /// of the text around it.
    pub baseline: Option<f32>,
}

impl Drawn {
    /// The SVG's own width in its units.
    pub fn natural_width(&self) -> f32 {
        self.width / self.scale
    }

    /// The SVG's own height in its units.
    pub fn natural_height(&self) -> f32 {
        self.height / self.scale
    }
}

/// How a job ended: the picture, or why there is none.
pub type Outcome = Result<Drawn, String>;

// ---- Making ---------------------------------------------------------------------------------------------

/// Parses `svg` and rasterises it. The diagnostic is resvg's, in words.
pub fn rasterize(renderer: &SvgRenderer, svg: &str) -> Outcome {
    let parsed = renderer.parse_svg(svg.as_bytes()).map_err(|error| error.to_string())?;
    // A small render learns the SVG's size (the parsed tree is opaque here), so the real one can be sized.
    let probe = renderer
        .render_parsed(&parsed, SvgSize::ScaleFactor(PROBE))
        .map_err(|error| error.to_string())?;
    let probed = probe.size(0);
    let unit = PROBE * SHARPNESS;
    let (guess_w, guess_h) = (
        probed.width.0.max(1) as f64 / unit as f64,
        probed.height.0.max(1) as f64 / unit as f64,
    );
    let area = guess_w * guess_h * (SHARPNESS * SHARPNESS) as f64;
    let mut scale = 1.0_f64;
    if area > MAX_PIXELS {
        scale = scale.min((MAX_PIXELS / area).sqrt());
    }
    let side = guess_w.max(guess_h) * SHARPNESS as f64;
    if side > MAX_SIDE {
        scale = scale.min(MAX_SIDE / side);
    }
    let scale = scale as f32;
    let image = renderer
        .render_parsed(&parsed, SvgSize::ScaleFactor(scale))
        .map_err(|error| error.to_string())?;
    let size = image.size(0);
    if size.width.0 <= 0 || size.height.0 <= 0 {
        return Err("The picture has no size".to_string());
    }
    Ok(Drawn {
        width: size.width.0 as f32 / SHARPNESS,
        height: size.height.0 as f32 / SHARPNESS,
        scale,
        baseline: None,
        image,
    })
}

// ---- Jobs -----------------------------------------------------------------------------------------------

/// What stops an engine that can be stopped.
type StopHook = Box<dyn Fn() + Send>;

/// What a job knows about the world around it: its time, and whether anyone still wants it.
#[derive(Clone)]
pub struct Ctl {
    cancelled: Arc<AtomicBool>,
    hook: Arc<Mutex<Option<StopHook>>>,
    deadline: Instant,
}

impl Ctl {
    /// A control whose deadline is `budget` from now.
    pub fn new(budget: Duration) -> Self {
        Ctl {
            cancelled: Arc::new(AtomicBool::new(false)),
            hook: Arc::new(Mutex::new(None)),
            deadline: Instant::now() + budget,
        }
    }

    /// Whether nobody wants the picture any more.
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// When the job must be done by.
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// The time left (zero when it is past).
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Runs `stop` if the job is cancelled while it works (at once, if it already is): for engines with a
    /// handle that can interrupt them.
    pub fn on_cancel(&self, stop: impl Fn() + Send + 'static) {
        if self.cancelled() {
            stop();
            return;
        }
        if let Ok(mut hook) = self.hook.lock() {
            *hook = Some(Box::new(stop));
        }
    }

    /// Says nobody wants the job any more: [`Ctl::cancelled`] turns true and the engine's stop hook runs.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Ok(hook) = self.hook.lock() {
            if let Some(stop) = hook.as_ref() {
                stop();
            }
        }
    }
}

enum Slot {
    Making(Ctl),
    Done(Arc<Outcome>, usize),
}

#[derive(Default)]
struct Inner {
    slots: HashMap<Key, Slot>,
    /// Least recently used first.
    order: VecDeque<Key>,
    bytes: usize,
    /// The views to wake when a picture arrives.
    waiting: HashMap<Key, Vec<EntityId>>,
    /// Images that left the cache, to be let go of the GPU's atlas.
    evicted: Vec<Arc<RenderImage>>,
}

impl Inner {
    fn touch(&mut self, key: Key) {
        self.order.retain(|k| *k != key);
        self.order.push_back(key);
    }

    fn trim(&mut self) {
        while self.order.len() > MAX_ENTRIES || self.bytes > MAX_BYTES {
            // Never drop a picture being made: it is not in the count of what is held.
            let Some(at) = self
                .order
                .iter()
                .position(|key| matches!(self.slots.get(key), Some(Slot::Done(..))))
            else {
                break;
            };
            let Some(key) = self.order.remove(at) else { break };
            if let Some(Slot::Done(outcome, bytes)) = self.slots.remove(&key) {
                self.bytes = self.bytes.saturating_sub(bytes);
                if let Ok(drawn) = outcome.as_ref() {
                    self.evicted.push(drawn.image.clone());
                }
            }
        }
    }
}

static CACHE: OnceLock<Mutex<Inner>> = OnceLock::new();

fn cache() -> MutexGuard<'static, Inner> {
    CACHE
        .get_or_init(|| Mutex::new(Inner::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The answer to [`request`].
pub enum Lookup {
    /// A job is making it; the view is woken when it is done.
    Making,
    /// Made (or failed).
    Done(Arc<Outcome>),
}

/// The picture for `key`, if it exists; otherwise starts `work` on a background thread (once, however many ask)
/// and wakes the entity `wake` when it is done. `work` gets GPUI's SVG renderer and the job's [`Ctl`]; a panic
/// in it is an error, not a crash.
pub fn request<W>(cx: &mut App, key: Key, wake: EntityId, budget: Duration, work: W) -> Lookup
where
    W: FnOnce(&SvgRenderer, &Ctl) -> Outcome + Send + 'static,
{
    let ctl = {
        let mut inner = cache();
        match inner.slots.get(&key) {
            Some(Slot::Done(outcome, _)) => {
                let outcome = outcome.clone();
                inner.touch(key);
                return Lookup::Done(outcome);
            }
            Some(Slot::Making(_)) => {
                let waiting = inner.waiting.entry(key).or_default();
                if !waiting.contains(&wake) {
                    waiting.push(wake);
                }
                return Lookup::Making;
            }
            None => {}
        }
        let ctl = Ctl::new(budget);
        inner.slots.insert(key, Slot::Making(ctl.clone()));
        inner.waiting.insert(key, vec![wake]);
        ctl
    };
    let renderer = cx.svg_renderer();
    let job_ctl = ctl.clone();
    cx.spawn(async move |cx| {
        let outcome = cx
            .background_executor()
            .spawn(async move {
                if job_ctl.cancelled() {
                    return Err("Cancelled".to_string());
                }
                catch_unwind(AssertUnwindSafe(|| work(&renderer, &job_ctl)))
                    .unwrap_or_else(|_| Err("The drawing stopped unexpectedly".to_string()))
            })
            .await;
        cx.update(|cx| finish(cx, key, outcome));
    })
    .detach();
    Lookup::Making
}

fn finish(cx: &mut App, key: Key, outcome: Outcome) {
    let woken = {
        let mut inner = cache();
        let woken = inner.waiting.remove(&key).unwrap_or_default();
        // A job that was cancelled (nobody wanted it when it stopped) leaves no trace; whoever asks again
        // starts afresh, and the views that did ask meanwhile are woken to do so.
        if matches!(&outcome, Err(reason) if reason == "Cancelled") {
            inner.slots.remove(&key);
            drop(inner);
            for id in woken {
                cx.notify(id);
            }
            return;
        }
        let bytes = outcome.as_ref().map_or(0, |drawn| {
            drawn.image.size(0).width.0.max(0) as usize * drawn.image.size(0).height.0.max(0) as usize * 4
        });
        inner.bytes += bytes;
        inner.slots.insert(key, Slot::Done(Arc::new(outcome), bytes));
        inner.touch(key);
        inner.trim();
        woken
    };
    for id in woken {
        cx.notify(id);
    }
}

/// The view `wake` is gone (or shows something else): jobs only it waited for are cancelled.
pub fn forget(wake: EntityId) {
    let mut inner = cache();
    let mut orphaned = Vec::new();
    for (key, waiting) in inner.waiting.iter_mut() {
        waiting.retain(|id| *id != wake);
        if waiting.is_empty() {
            orphaned.push(*key);
        }
    }
    for key in orphaned {
        if let Some(Slot::Making(ctl)) = inner.slots.get(&key) {
            ctl.cancel();
        }
    }
}

/// The picture for `key` if it is already made (a finished one only; nothing is started).
pub fn peek(key: &Key) -> Option<Arc<Outcome>> {
    match cache().slots.get(key) {
        Some(Slot::Done(outcome, _)) => Some(outcome.clone()),
        _ => None,
    }
}

/// Lets pictures that left the cache go of the GPU's atlas. Call once per frame from a view that has the window.
pub fn release_retired(window: &mut Window) {
    let evicted = std::mem::take(&mut cache().evicted);
    for image in evicted {
        window.drop_image(image).ok();
    }
}

/// How many pictures and failures are kept, and the pixel memory they hold (for tests and the status line).
pub fn held() -> (usize, usize) {
    let inner = cache();
    (
        inner
            .slots
            .values()
            .filter(|slot| matches!(slot, Slot::Done(..)))
            .count(),
        inner.bytes,
    )
}

/// Empties the cache (tests; and a theme or font change that makes every picture stale).
pub fn clear() {
    let mut inner = cache();
    let gone: Vec<Arc<RenderImage>> = inner
        .slots
        .values()
        .filter_map(|slot| match slot {
            Slot::Done(outcome, _) => outcome.as_ref().as_ref().ok().map(|drawn| drawn.image.clone()),
            Slot::Making(_) => None,
        })
        .collect();
    let Inner {
        slots,
        order,
        bytes,
        evicted,
        ..
    } = &mut *inner;
    slots.retain(|_, slot| matches!(slot, Slot::Making(_)));
    order.retain(|key| slots.contains_key(key));
    *bytes = 0;
    evicted.extend(gone);
}

// ---- Drawing --------------------------------------------------------------------------------------------

/// The picture as an element named `name` (for the accessibility tree) with the element id `id`: at its drawn size,
/// shrinking to the width it has and keeping its shape.
pub fn picture(id: impl Into<ElementId>, drawn: &Drawn, name: SharedString) -> impl IntoElement {
    let ratio = if drawn.height > 0. {
        drawn.width / drawn.height
    } else {
        1.
    };
    div()
        .id(id)
        .role(Role::Image)
        .aria_label(name)
        .test_support()
        .w(px(drawn.width))
        .max_w_full()
        .min_w_0()
        .aspect_ratio(ratio)
        .child(
            img(ImageSource::Render(drawn.image.clone()))
                .size_full()
                .object_fit(ObjectFit::Contain),
        )
}

/// A source shown as a monospace block, cut to a sensible length.
pub fn source_block(id: ElementId, source: &str, cx: &App) -> impl IntoElement {
    let t = theme::tokens(cx);
    let mut shown = String::new();
    for (count, line) in source.lines().enumerate() {
        if count >= BANNER_LINES || shown.len() + line.len() > BANNER_CHARS {
            shown.push('…');
            break;
        }
        if count > 0 {
            shown.push('\n');
        }
        shown.push_str(line);
    }
    div()
        .id(id)
        .role(Role::Group)
        .aria_label(t!("Source"))
        .test_support()
        .w_full()
        .min_w_0()
        .p(space(2.5))
        .rounded(theme::size::RADIUS_SM)
        .bg(t.muted)
        .font_family(theme::mono_font(cx))
        .text_size(theme::size::XS)
        .text_color(t.foreground)
        .text_start()
        .child(shown)
}

/// What shows when a picture could not be made: the diagnostic in a tinted banner, then the source as written.
/// `what` is the whole sentence, with the diagnostic in it. `prefix` and `at` (where in the document it sits) make
/// the ids of the two parts, so two banners in one document are two elements for the accessibility tree.
pub fn error_banner(prefix: &'static str, at: u64, what: SharedString, source: &str, cx: &App) -> Div {
    let t = theme::tokens(cx);
    col()
        .w_full()
        .min_w_0()
        .gap(space(1.5))
        .child(
            div()
                .id(ElementId::NamedInteger(prefix.into(), at))
                .role(Role::Status)
                .aria_label(what.clone())
                .test_support()
                .w_full()
                .min_w_0()
                .px(space(2.5))
                .py(space(1.5))
                .rounded(theme::size::RADIUS_SM)
                .bg(t.warning.opacity(0.14))
                .border_1()
                .border_color(t.warning.opacity(0.4))
                .text_size(theme::size::XS)
                .text_color(t.foreground)
                .text_start()
                .child(what),
        )
        .child(source_block(
            ElementId::NamedInteger(format!("{prefix}-source").into(), at),
            source,
            cx,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::TestAppContext;

    const SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20"><rect width="40" height="20" fill="#336699"/></svg>"##;

    fn renderer(cx: &mut TestAppContext) -> SvgRenderer {
        cx.update(|cx| cx.svg_renderer())
    }

    #[gpui_kit::test]
    fn a_small_svg_is_drawn_at_its_own_size(cx: &mut TestAppContext) {
        let drawn = rasterize(&renderer(cx), SQUARE).expect("the svg draws");
        assert_eq!((drawn.width, drawn.height), (40., 20.));
        assert_eq!(drawn.scale, 1.);
        assert_eq!((drawn.natural_width(), drawn.natural_height()), (40., 20.));
        // Twice the pixels for sharpness.
        assert_eq!(drawn.image.size(0).width.0, 80);
    }

    #[gpui_kit::test]
    fn a_huge_svg_is_drawn_smaller_but_keeps_its_shape(cx: &mut TestAppContext) {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="12000" height="6000" viewBox="0 0 12000 6000"><rect width="12000" height="6000" fill="#336699"/></svg>"##;
        let drawn = rasterize(&renderer(cx), svg).expect("the svg draws");
        assert!(drawn.scale < 0.6, "{}", drawn.scale);
        assert!(
            (drawn.width / drawn.height - 2.).abs() < 0.02,
            "{} x {}",
            drawn.width,
            drawn.height
        );
        let size = drawn.image.size(0);
        assert!((size.width.0 as f64) * (size.height.0 as f64) <= MAX_PIXELS * 1.05);
        assert!(
            (drawn.natural_width() - 12000.).abs() < 60.,
            "{}",
            drawn.natural_width()
        );
    }

    #[gpui_kit::test]
    fn a_broken_svg_says_why(cx: &mut TestAppContext) {
        let failure = rasterize(&renderer(cx), "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect").err();
        assert!(failure.is_some_and(|reason| !reason.is_empty()));
        assert!(rasterize(&renderer(cx), "not an svg at all").is_err());
    }

    #[test]
    fn keys_tell_kind_source_theme_and_size_apart() {
        let base = Key::new("mermaid", "graph TD;", 0., false);
        assert_eq!(base, Key::new("mermaid", "graph TD;", 0., false));
        assert_ne!(base, Key::new("dot", "graph TD;", 0., false));
        assert_ne!(base, Key::new("mermaid", "graph LR;", 0., false));
        assert_ne!(base, Key::new("mermaid", "graph TD;", 0., true));
        assert_ne!(base, Key::new("mermaid", "graph TD;", 16., false));
    }

    #[gpui_kit::test]
    fn the_cache_makes_once_and_keeps_what_it_made(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        clear();
        let (_visual, view) = testing::open_window(cx, |_, cx| cx.new(|_| Probe));
        let id = view.entity_id();
        let key = Key::new("test", "a-cache-test-source", 0., false);
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ask = |cx: &mut TestAppContext, runs: &Arc<std::sync::atomic::AtomicUsize>| {
            let runs = runs.clone();
            cx.update(|cx| {
                request(cx, key, id, Duration::from_secs(5), move |renderer, _| {
                    runs.fetch_add(1, Ordering::SeqCst);
                    rasterize(renderer, SQUARE)
                })
            })
        };
        assert!(matches!(ask(cx, &runs), Lookup::Making));
        // A second ask while it is being made joins the first.
        assert!(matches!(ask(cx, &runs), Lookup::Making));
        for _ in 0..400 {
            cx.run_until_parked();
            if peek(&key).is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(peek(&key).is_some_and(|outcome| outcome.is_ok()));
        assert!(matches!(ask(cx, &runs), Lookup::Done(_)));
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        clear();
    }

    #[gpui_kit::test]
    fn a_panic_and_a_failure_are_kept_as_failures_not_crashes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        clear();
        let (_visual, view) = testing::open_window(cx, |_, cx| cx.new(|_| Probe));
        let id = view.entity_id();
        let key = Key::new("test", "a-panicking-source", 0., false);
        cx.update(|cx| request(cx, key, id, Duration::from_secs(5), |_, _| panic!("boom")));
        for _ in 0..400 {
            cx.run_until_parked();
            if peek(&key).is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let outcome = peek(&key).expect("the failure is kept");
        assert!(outcome.is_err());
        clear();
    }

    #[test]
    fn the_cache_stays_within_its_count() {
        let _guard = testing::lock();
        clear();
        {
            let mut inner = cache();
            for n in 0..(MAX_ENTRIES + 20) {
                let key = Key::new("test", &format!("entry {n}"), 0., false);
                inner.slots.insert(key, Slot::Done(Arc::new(Err("none".into())), 0));
                inner.touch(key);
                inner.trim();
            }
        }
        assert_eq!(held().0, MAX_ENTRIES);
        // The newest are the ones kept.
        assert!(peek(&Key::new("test", &format!("entry {}", MAX_ENTRIES + 19), 0., false)).is_some());
        assert!(peek(&Key::new("test", "entry 0", 0., false)).is_none());
        clear();
    }

    /// Runs a job that, once under way, makes the views in `leaving` forget it, and reports whether it was
    /// cancelled by then and whether its stop hook ran.
    fn job_whose_views_leave(
        cx: &mut TestAppContext,
        key: Key,
        waiting: &[EntityId],
        leaving: Vec<EntityId>,
    ) -> (bool, bool) {
        // (The job runs on the test's own thread, so it does the leaving itself.)
        let seen = Arc::new(Mutex::new((false, false)));
        for wake in waiting {
            let (seen, leaving) = (seen.clone(), leaving.clone());
            cx.update(|cx| {
                request(cx, key, *wake, Duration::from_secs(30), move |_, ctl| {
                    let hook = Arc::new(AtomicBool::new(false));
                    let flag = hook.clone();
                    ctl.on_cancel(move || flag.store(true, Ordering::SeqCst));
                    for wake in leaving {
                        forget(wake);
                    }
                    if let Ok(mut seen) = seen.lock() {
                        *seen = (ctl.cancelled(), hook.load(Ordering::SeqCst));
                    }
                    if ctl.cancelled() {
                        Err("Cancelled".to_string())
                    } else {
                        Err("It went on".to_string())
                    }
                })
            });
        }
        cx.run_until_parked();
        let seen = seen.lock().map_or((false, false), |seen| *seen);
        seen
    }

    #[gpui_kit::test]
    fn a_job_nobody_waits_for_any_more_is_cancelled_and_leaves_no_trace(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        clear();
        let (_visual, only) = testing::open_window(cx, |_, cx| cx.new(|_| Probe));
        let key = Key::new("test", "a-job-nobody-wants", 0., false);
        let (cancelled, hook_ran) = job_whose_views_leave(cx, key, &[only.entity_id()], vec![only.entity_id()]);
        assert!(cancelled && hook_ran, "cancelled {cancelled}, hook {hook_ran}");
        // It left nothing behind: not a picture, not a failure; the next ask starts afresh.
        assert!(!cache().slots.contains_key(&key));
        assert!(peek(&key).is_none());
        clear();
    }

    #[gpui_kit::test]
    fn a_job_another_view_still_waits_for_goes_on(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        clear();
        let (_visual, first) = testing::open_window(cx, |_, cx| cx.new(|_| Probe));
        let (_visual2, second) = testing::open_window(cx, |_, cx| cx.new(|_| Probe));
        let key = Key::new("test", "a-job-someone-wants", 0., false);
        let (cancelled, hook_ran) = job_whose_views_leave(
            cx,
            key,
            &[first.entity_id(), second.entity_id()],
            vec![first.entity_id()],
        );
        assert!(!cancelled && !hook_ran);
        // It finished (as a failure, the way this job ends) and is kept for the one that waited.
        assert!(peek(&key).is_some_and(|outcome| outcome.is_err()));
        clear();
    }

    struct Probe;

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
}
