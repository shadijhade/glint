//! Thumbnails for tiles and rows: the service that asks glint-core for them, keeps the decoded pictures, and
//! the little pieces a tile draws with. Ported from src/lib/thumbs.ts and src/components/explorer/Thumbnail.tsx.
//!
//! # What it does, and what the TypeScript called it
//!
//! | TypeScript                          | Here                                                              |
//! |-------------------------------------|-------------------------------------------------------------------|
//! | `useThumbnail` + `watch`            | [`Thumbnails::show`]: a view says which thumbnails it shows now   |
//! | the microtask `settleWants`         | the diff inside `show`: new asks go out, dropped ones are cancelled |
//! | `held` (48 MB) + `heldUrls`         | the held pictures, an LRU by decoded bytes                        |
//! | `latest` (5,000) + `isHeld`         | `latest`, an LRU by file; a stand-in must still be held           |
//! | `thumbnail` / `cancelThumbnails`    | [`ThumbSource`] (glint-core in the app, a fake in tests)          |
//! | `decodeThumb` + `URL.createObjectURL` | the answer's 8-byte header, then the BMP decoded into a `RenderImage` |
//! | `dropThumbnails(lane)`              | [`Thumbnails::forget_lane`] (also on the explorer's `TabClosed`)  |
//! | `timeThumbnails` + `useThumbRuns`   | [`Thumbnails::time`] and [`Thumbnails::run`] ([`ThumbRun`])       |
//! | `Thumbnail` (component)             | [`Thumbnails::look`] says what to draw, [`thumbnail_square`] draws it |
//!
//! # Using it from a view (the file list reuses this)
//!
//! ```ignore
//! let thumbs = thumbnail::thumbnails(cx);                       // Entity<Thumbnails>, made on first use
//! cx.observe(&thumbs, |_, _, cx| cx.notify()).detach();         // redraw when a picture arrives
//! // As the visible items change (not every frame: when the range or the rows change):
//! let wants: Vec<Want> = visible.filter_map(|e| Want::for_entry(e, box_px, window.scale_factor())).collect();
//! thumbs.update(cx, |t, cx| t.show(cx.entity_id_of_this_view, "tab3", wants, cx));
//! // While drawing each item:
//! let look = thumbs.read(cx).look(entry, box_px, window.scale_factor());
//! thumbnail_square(&look, kind_of(entry), box_px, icon_px, true, cx)
//! // Once per frame, somewhere with a window: free what was let go of from the GPU's atlas.
//! for image in thumbs.update(cx, |t, _| t.take_evicted()) { window.drop_image(image).ok(); }
//! // And when the view goes away: thumbs.update(cx, |t, cx| t.release(view_id, cx));
//! ```
//!
//! **Why `show` takes the whole set.** A tile in the browser asked as it mounted and let go as it unmounted.
//! A GPUI view re-renders its visible range every frame, so it hands over the set it shows and this module
//! works out what is new (ask), what is gone (cancel, unless another view still shows it) and what stays
//! (touch, so it is the last to be evicted). A thumbnail that came back empty but may come later (a handler
//! that timed out, an online-only file) is asked again only when it leaves the set and comes back, as the
//! TypeScript did when the tile remounted.
//!
//! **Pictures.** The answer is an 8-byte header and a 32-bit top-down BMP (`Thumb::encode`, `encode_bmp`).
//! The BMP is decoded on a background thread straight to the BGRA pixels GPUI draws (no PNG anywhere), and the
//! `RenderImage` is shared by every tile that shows it.
//!
//! NOTE(coordinator): `RenderImage::new` takes `image::Frame`s, and `image` is not a dependency of glint-ui,
//! so the BMP goes through `gpui::Image::to_image_data` (GPUI's own BMP decoder: one pass over the pixels).
//! With `image = "0.25"` in Cargo.toml the pixels could be wrapped in a `Frame` directly; see `decode_bmp`.

use crate::explorer::{self, ExplorerEvent};
use crate::kit::prelude::*;
use crate::theme;
use glint_model::entries::{is_archived, is_list_only, is_locked, is_online_only, FileEntry};
use glint_model::file_types::Kind;
use glint_model::grid::thumb_step;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

/// A decoded picture's budget: about 1,100 tiles' worth at the grid's medium size and 150 at its biggest, ten
/// screenfuls or more either way. Pictures on screen are always kept (`HELD_BUDGET`).
const HELD_BUDGET: usize = 48 << 20;

/// How many files' latest picture (at whatever size) is remembered as a stand-in for a tile's new size
/// (`latest` in the TypeScript, which kept 5,000).
const LATEST_FILES: usize = 5000;

/// What a file with no thumbnail costs to remember: a little, so a folder of them can't grow this for ever.
const NONE_COST: usize = 64;

/// An SVG the grid draws itself, up to this size (`MAX_SVG`): Windows has no thumbnail handler for SVG as it comes.
const MAX_SVG: u64 = 4 << 20;

/// The width an SVG is first drawn at, only to learn its shape (see [`draw_vector`]).
const PROBE_WIDTH: i32 = 64;

// ---- The source ---------------------------------------------------------------------------------------

/// A boxed future, as the explorer's `Fut`.
pub type Fut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// One thumbnail asked of the source (`ThumbRequest` in fs.ts).
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub path: String,
    /// Device pixels, the most on either side.
    pub size: u32,
    /// The file's date and size from its listing: a file that changed gets a new thumbnail.
    pub modified: i64,
    pub bytes: u64,
    /// Only from Windows' cache (an online-only file, which making a thumbnail would download).
    pub cached_only: bool,
    /// The tab asking, and the request's number in it.
    pub lane: String,
    pub seq: u32,
}

/// Where thumbnails come from: glint-core in the app, a fake in tests.
pub trait ThumbSource {
    /// One item's thumbnail as core answers it (the 8-byte header, then a BMP), or the reason it failed.
    /// `Err("cancelled")` once [`ThumbSource::cancel`] dropped it.
    fn thumbnail(&self, ask: Ask) -> Fut<Result<Vec<u8>, String>>;

    /// Drops `lane`'s requests up to number `up_to`, for `paths` or (None) all of them
    /// (`cancelThumbnails`). Never waits.
    fn cancel(&self, lane: &str, paths: Option<Vec<String>>, up_to: u32);
}

/// glint-core's thumbnail service (`glint_core::thumbs`), through the app handle that registered its `Thumbs`.
pub struct CoreThumbs {
    app: glint_core::host::AppHandle,
}

impl CoreThumbs {
    pub fn new(app: glint_core::host::AppHandle) -> Self {
        CoreThumbs { app }
    }
}

impl ThumbSource for CoreThumbs {
    fn thumbnail(&self, ask: Ask) -> Fut<Result<Vec<u8>, String>> {
        use glint_core::host::Manager as _;
        let app = self.app.clone();
        let running = crate::core_bridge::run(async move {
            let state = app.state::<glint_core::thumbs::Thumbs>();
            glint_core::thumbs::thumbnail(
                state,
                ask.path,
                ask.size,
                ask.modified as f64,
                ask.bytes as f64,
                ask.cached_only,
                ask.lane,
                ask.seq,
            )
            .await
            .map(glint_core::host::ipc::Response::into_bytes)
        });
        // Two layers: the core task may panic (outer Err), the thumbnail may not come (inner Err).
        Box::pin(async move { running.await.and_then(|answer| answer) })
    }

    fn cancel(&self, lane: &str, paths: Option<Vec<String>>, up_to: u32) {
        use glint_core::host::Manager as _;
        let app = self.app.clone();
        let lane = lane.to_string();
        // It only drops the waits (a lock and a few moves), but it is core's, so it runs on core's runtime.
        drop(glint_core::host::async_runtime::spawn(async move {
            let state = app.state::<glint_core::thumbs::Thumbs>();
            let _ = glint_core::thumbs::cancel_thumbnails(state, lane, paths, up_to).await;
        }));
    }
}

/// A source that never has a thumbnail, for an app whose core isn't up (and for tests that don't care).
pub struct NoThumbs;

impl ThumbSource for NoThumbs {
    fn thumbnail(&self, _ask: Ask) -> Fut<Result<Vec<u8>, String>> {
        Box::pin(async { Err("no thumbnails here".to_string()) })
    }

    fn cancel(&self, _lane: &str, _paths: Option<Vec<String>>, _up_to: u32) {}
}

// ---- The answer ---------------------------------------------------------------------------------------

/// Where a thumbnail came from (the answer's second byte): core's memory, Windows' cache, or made from the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Memory,
    Cache,
    Made,
}

/// The 8-byte header of an answer (`decodeThumb`, `Thumb::encode`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Header {
    /// A BMP follows.
    pub found: bool,
    /// For none: that the file has none for good, rather than none yet.
    pub lasting: bool,
    pub from: Origin,
    /// Milliseconds the shell spent on it.
    pub ms: f32,
}

/// Reads an answer's header. `None` for fewer than 8 bytes (nothing core sends).
pub fn parse_header(answer: &[u8]) -> Option<Header> {
    let head = answer.get(..8)?;
    Some(Header {
        found: head[0] == 0 && answer.len() > 8,
        lasting: head[2] == 1,
        from: match head[1] {
            0 => Origin::Memory,
            2 => Origin::Made,
            _ => Origin::Cache,
        },
        ms: f32::from_le_bytes([head[4], head[5], head[6], head[7]]),
    })
}

/// Decodes the answer's BMP (the bytes after the header) into a picture GPUI draws. `None` when it isn't one.
fn decode_bmp(bmp: Vec<u8>, renderer: SvgRenderer) -> Option<Arc<RenderImage>> {
    Image::from_bytes(ImageFormat::Bmp, bmp).to_image_data(renderer).ok()
}

/// A decoded picture's size in pixels.
fn pixels_of(image: &RenderImage) -> (u32, u32) {
    let size = image.size(0);
    (
        u32::try_from(size.width.0).unwrap_or(0),
        u32::try_from(size.height.0).unwrap_or(0),
    )
}

// ---- Keys, wants, looks -------------------------------------------------------------------------------

/// A file as it is now, at a size: a file that changes (or is downloaded, from online-only) gets a new
/// thumbnail (`thumbKey`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    /// Lowercase: Windows paths ignore case.
    path: String,
    step: u32,
    modified: i64,
    bytes: u64,
    cloud: bool,
}

/// One thumbnail a view shows (or is about to).
#[derive(Clone, Debug, PartialEq)]
pub struct Want {
    pub path: String,
    pub modified: i64,
    pub bytes: u64,
    pub cached_only: bool,
    /// The size to ask for, in device pixels (`glint_model::grid::thumb_step`).
    pub step: u32,
    /// The square it fits in, CSS pixels: a stand-in from another size must not be far smaller.
    pub css_box: f32,
    /// An SVG file the service draws itself instead of asking Windows ([`is_vector`]).
    pub vector: bool,
}

impl Want {
    /// What a tile of `css_box` pixels (at a window `scale` of device pixels to one) asks for `entry`; `None`
    /// for what has no thumbnail to ask for: a folder, an item in an archive Glint can't read (locked) or one
    /// that only lists (`enabled` in the TypeScript's tile).
    pub fn for_entry(entry: &FileEntry, css_box: f32, scale: f32) -> Option<Want> {
        if !is_eligible(entry) {
            return None;
        }
        Some(Want {
            path: entry.path.clone(),
            modified: entry.modified,
            bytes: entry.size,
            cached_only: is_online_only(entry),
            step: thumb_step(css_box, scale),
            css_box,
            vector: is_vector(entry),
        })
    }

    fn key(&self) -> Key {
        Key {
            path: self.path.to_lowercase(),
            step: self.step,
            modified: self.modified,
            bytes: self.bytes,
            cloud: self.cached_only,
        }
    }
}

/// Whether Windows can have a thumbnail of this item at all (the TypeScript's `enabled`). An SVG counts: the
/// service draws it ([`is_vector`]) rather than asking Windows, which has no handler for SVG as it comes.
pub fn is_eligible(entry: &FileEntry) -> bool {
    !entry.dir && !is_locked(entry) && !is_list_only(entry)
}

/// Whether the service draws this file itself: an SVG of up to 4 MB, not inside an archive (`vector` in
/// Thumbnail.tsx). Unlike the web page, which would have fetched an online-only file through its placeholder (and so
/// downloaded it), a cloud file that isn't on this PC is not drawn: it asks Windows' cache like any other file.
pub fn is_vector(entry: &FileEntry) -> bool {
    is_eligible(entry) && entry.ext == "svg" && !is_archived(entry) && !is_online_only(entry) && entry.size <= MAX_SVG
}

/// Draws the SVG file at `path` crisp for a square of `step` device pixels: parsed once, then rasterized at the
/// size that fits the square with its shape kept (the web page drew it into an <img>, which fills the square the
/// same way). `None` for a file that can't be read or isn't a drawing, which keeps its icon.
fn draw_vector(path: &str, step: u32, renderer: SvgRenderer) -> Option<Arc<RenderImage>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    // One byte past the limit tells a file that grew since it was listed.
    std::fs::File::open(path)
        .ok()?
        .take(MAX_SVG + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).ok()? > MAX_SVG {
        return None;
    }
    let svg = renderer.parse_svg(&bytes).ok()?;
    let at = |width: i32| SvgSize::Size(size(DevicePixels(width), DevicePixels(width)));
    // The drawing's shape is not known to the caller, and a very tall one drawn at the square's full width would
    // be huge: a small drawing first gives the shape.
    let probe = renderer.render_parsed(&svg, at(PROBE_WIDTH)).ok()?;
    let (probe_w, probe_h) = pixels_of(&probe);
    if probe_w == 0 {
        return None;
    }
    let step = i32::try_from(step).ok()?;
    // Wider than tall (or square): the square's width. Taller: the width at which its height is the square's.
    let width = if probe_h <= probe_w {
        step
    } else {
        let tall = f64::from(probe_h) / f64::from(probe_w);
        // Rounded; at least one pixel, at most the square.
        ((f64::from(step) / tall).round() as i32).clamp(1, step)
    };
    renderer.render_parsed(&svg, at(width)).ok()
}

/// A picture, sized for the square it goes in, in CSS pixels.
#[derive(Clone)]
pub struct Drawn {
    pub image: Arc<RenderImage>,
    pub width: f32,
    pub height: f32,
    /// How much of the picture shows, 0 to 1: under 1 for the moment after a file's first picture came in, while it
    /// fades in over its icon ([`Thumbnails::look_arriving`]); always 1 from [`Thumbnails::look`].
    pub shown: f32,
}

/// How long a file's first picture takes to fade in over its icon.
const ARRIVE: std::time::Duration = std::time::Duration::from_millis(150);

/// What a tile draws in its square.
#[derive(Clone)]
pub enum Look {
    /// The file's kind icon: no thumbnail yet, or none at all. It stays until the picture is in, so a tile
    /// never blinks out first.
    Icon,
    /// This size's picture.
    Picture(Drawn),
    /// Another size's picture, until this size's is in (a zoom), filling the square as the new one will.
    StandIn(Drawn),
}

impl Look {
    /// A picture that is still fading in: the view drawing it must draw again next frame.
    pub fn arriving(&self) -> bool {
        matches!(self, Look::Picture(drawn) if drawn.shown < 1.)
    }
}

/// How big to draw a picture of `w` x `h` CSS pixels, asked for a `picture_box` square, in a `box_` square: one
/// device pixel to one, so a small one isn't blown up, except a stand-in from another size that filled its
/// square: it fills this one, as the new one will (`fit`). `w` and `h` are the picture's natural size.
pub fn fit(w: f32, h: f32, picture_box: f32, box_: f32, stand_in: bool) -> (f32, f32) {
    let long = w.max(h);
    if long.is_nan() || box_.is_nan() || long <= 0.0 || box_ <= 0.0 {
        return (0.0, 0.0);
    }
    let scale = if stand_in && long >= picture_box - 2.0 {
        box_ / long
    } else {
        1f32.min(box_ / w).min(box_ / h)
    };
    (w * scale, h * scale)
}

// ---- An LRU --------------------------------------------------------------------------------------------

/// A map that remembers how recently each entry was used.
struct Lru<K, V> {
    map: HashMap<K, (V, u64)>,
    order: BTreeMap<u64, K>,
    tick: u64,
}

impl<K: Hash + Eq + Clone, V> Lru<K, V> {
    fn new() -> Self {
        Lru {
            map: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
        }
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn get(&self, key: &K) -> Option<&V> {
        self.map.get(key).map(|(value, _)| value)
    }

    fn contains(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    /// Makes `key` the most recently used.
    fn touch(&mut self, key: &K) {
        if let Some((_, tick)) = self.map.get_mut(key) {
            self.order.remove(tick);
            self.tick += 1;
            *tick = self.tick;
            self.order.insert(self.tick, key.clone());
        }
    }

    fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.tick += 1;
        let old = self.map.insert(key.clone(), (value, self.tick));
        if let Some((_, tick)) = &old {
            self.order.remove(tick);
        }
        self.order.insert(self.tick, key);
        old.map(|(value, _)| value)
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        let (value, tick) = self.map.remove(key)?;
        self.order.remove(&tick);
        Some(value)
    }

    /// Removes the least recently used entry `keep` doesn't want kept.
    fn pop_oldest(&mut self, keep: impl Fn(&K, &V) -> bool) -> Option<(K, V)> {
        let victim = self
            .order
            .values()
            .find(|key| self.map.get(*key).is_some_and(|(value, _)| !keep(key, value)))
            .cloned()?;
        let value = self.remove(&victim)?;
        Some((victim, value))
    }
}

// ---- The service ---------------------------------------------------------------------------------------

/// A thumbnail on hand: its picture (or none, for a file that has none), where it was asked for, what it costs.
struct Held {
    /// When the picture came in, for a file that had none of any size before it: it fades in from then. None for
    /// a picture that took another size's place (a zoom), which is simply there.
    since: Option<std::time::Instant>,
    image: Option<Arc<RenderImage>>,
    /// The picture's size in device pixels.
    px: (u32, u32),
    /// The CSS square it was asked for.
    css_box: f32,
    cost: usize,
}

/// A file's latest picture, whatever its size: the key it is held under, and the square it was asked for.
struct Latest {
    key: Key,
}

/// A request out at the source.
struct Request {
    seq: u32,
    lane: String,
    /// The burst it was timed in ([`ThumbRun`]), if its lane was being timed.
    run: Option<u64>,
    /// What was asked, to ask again exactly (a path's case matters inside an archive).
    want: Want,
    /// How often it was asked again after being cancelled while a view still showed it.
    retries: u8,
}

/// A thumbnail cancelled while a view still shows it is asked again this many times at the most, so a source
/// that keeps cancelling can't make a loop.
const MAX_RETRIES: u8 = 3;

/// What one view shows now.
struct Shown {
    lane: String,
    keys: HashSet<Key>,
}

/// A tab's latest burst of thumbnails, from its first request to its last answer (`ThumbRun`): the status bar
/// reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct ThumbRun {
    /// The folder or search shown (`Tab::view_key`) and the view: another one's thumbnails don't count.
    pub scope: String,
    /// When the burst's first request went out.
    pub started: Option<Instant>,
    pub pending: usize,
    /// Thumbnails that came, and from where: core's memory, Windows' cache, or made from the file.
    pub found: usize,
    pub memory: usize,
    pub cache: usize,
    pub made: usize,
    /// Milliseconds the shell spent making them, all told.
    pub made_ms: f64,
    /// From the first request to the last answer, ms.
    pub elapsed_ms: f64,
    id: u64,
}

impl ThumbRun {
    fn start_burst(&mut self) {
        self.started = Some(Instant::now());
        self.pending = 0;
        self.found = 0;
        self.memory = 0;
        self.cache = 0;
        self.made = 0;
        self.made_ms = 0.0;
        self.elapsed_ms = 0.0;
    }
}

/// The thumbnail service: asks the source for what views show, keeps what came, and says what to draw.
pub struct Thumbnails {
    source: Rc<dyn ThumbSource>,
    held: Lru<Key, Held>,
    held_bytes: usize,
    budget: usize,
    latest: Lru<String, Latest>,
    /// How many views show each thumbnail right now (those are never evicted).
    showing: HashMap<Key, u32>,
    viewers: HashMap<EntityId, Shown>,
    wanted: HashMap<Key, Request>,
    /// SVGs being drawn here (they are not asked of the source, so they are not in `wanted`).
    drawing: HashSet<Key>,
    seq: u32,
    runs: HashMap<String, ThumbRun>,
    run_ids: u64,
    /// Pictures let go of that the GPU's atlas may still hold: a view with a window frees them.
    evicted: Vec<Arc<RenderImage>>,
    _subscriptions: Vec<Subscription>,
}

/// Something worth redrawing for happened (a picture arrived, a burst ended). Views can observe the entity
/// instead; this is for the ones that want to know it was this.
pub struct Arrived;

impl EventEmitter<Arrived> for Thumbnails {}

/// The one service, in a GPUI `Global`.
struct ThumbnailsHandle(Entity<Thumbnails>);

impl Global for ThumbnailsHandle {}

/// The thumbnail service of the app, made on first use over glint-core (or over [`NoThumbs`] before core is up).
pub fn thumbnails(cx: &mut App) -> Entity<Thumbnails> {
    if let Some(handle) = cx.try_global::<ThumbnailsHandle>() {
        return handle.0.clone();
    }
    let source: Rc<dyn ThumbSource> = match cx.try_global::<crate::core_bridge::Core>() {
        Some(core) => Rc::new(CoreThumbs::new(core.app.clone())),
        None => Rc::new(NoThumbs),
    };
    init_with(cx, source)
}

/// [`thumbnails`] over a source of your own (tests, tools). Replaces the service already there.
pub fn init_with(cx: &mut App, source: Rc<dyn ThumbSource>) -> Entity<Thumbnails> {
    let service = cx.new(|cx| Thumbnails::new(source, cx));
    cx.set_global(ThumbnailsHandle(service.clone()));
    service
}

impl Thumbnails {
    /// A service over `source`. A tab that closes lets go of its thumbnails.
    pub fn new(source: Rc<dyn ThumbSource>, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(explorer) = explorer::try_explorer(cx) {
            subscriptions.push(cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| {
                if let ExplorerEvent::TabClosed { tab, .. } = event {
                    this.forget_lane(&tab.to_string(), cx);
                }
            }));
        }
        Thumbnails {
            source,
            held: Lru::new(),
            held_bytes: 0,
            budget: HELD_BUDGET,
            latest: Lru::new(),
            showing: HashMap::new(),
            viewers: HashMap::new(),
            wanted: HashMap::new(),
            drawing: HashSet::new(),
            seq: 0,
            runs: HashMap::new(),
            run_ids: 0,
            evicted: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    // ---- What a view shows ------------------------------------------------------------------------

    /// `viewer` shows exactly `wants` now, for tab `lane`. New thumbnails are asked for, ones it no longer shows
    /// are cancelled unless another view shows them, and the ones it still shows are kept fresh (`watch`, and
    /// the let-go, and `settleWants`).
    pub fn show(&mut self, viewer: EntityId, lane: &str, wants: Vec<Want>, cx: &mut Context<Self>) {
        let mut new: HashMap<Key, Want> = HashMap::with_capacity(wants.len());
        for want in wants {
            new.insert(want.key(), want);
        }
        let old = self.viewers.remove(&viewer);
        let (old_lane, old_keys) = match old {
            Some(old) => (old.lane, old.keys),
            None => (String::new(), HashSet::new()),
        };
        let same_lane = old_lane == lane;

        // Counts first: who shows what after this call.
        for key in &old_keys {
            self.stop_showing(key);
        }
        for key in new.keys() {
            *self.showing.entry(key.clone()).or_insert(0) += 1;
        }

        // Dropped: nobody shows them any more. Only the requests made up to now are cancelled, so the new asks
        // below (a zoomed tile's other size of the same file) are not.
        let mut cancels: HashMap<String, Vec<String>> = HashMap::new();
        for key in old_keys.iter().filter(|key| !same_lane || !new.contains_key(*key)) {
            if self.showing.contains_key(key) {
                continue;
            }
            if let Some(request) = self.wanted.remove(key) {
                self.count(&request, None);
                cancels.entry(request.lane).or_default().push(key.path.clone());
            }
        }
        let up_to = self.seq;
        for (cancelled_lane, paths) in cancels {
            self.source.cancel(&cancelled_lane, Some(paths), up_to);
        }

        // New: held ones are touched, the rest asked for.
        let fresh_for_lane = !same_lane;
        let mut to_ask = Vec::new();
        for (key, want) in &new {
            if self.held.contains(key) {
                self.held.touch(key);
                continue;
            }
            let is_new = fresh_for_lane || !old_keys.contains(key);
            if is_new && !self.wanted.contains_key(key) && !self.drawing.contains(key) {
                to_ask.push((key.clone(), want.clone()));
            }
        }
        // Closest to the top first would be better, but the view hands over the set in the order it drew it.
        to_ask.sort_by(|a, b| a.1.path.cmp(&b.1.path));
        self.viewers.insert(
            viewer,
            Shown {
                lane: lane.to_string(),
                keys: new.into_keys().collect(),
            },
        );
        for (key, want) in to_ask {
            self.ask(key, want, lane, 0, cx);
        }
        self.evict();
    }

    /// `viewer` is gone (its view was released): what only it showed is cancelled.
    pub fn release(&mut self, viewer: EntityId, cx: &mut Context<Self>) {
        self.show(viewer, "", Vec::new(), cx);
        self.viewers.remove(&viewer);
    }

    /// Everything a tab waits for is dropped, and its views start over with their next `show`: its folder
    /// changed, or the tab closed (`dropThumbnails`).
    pub fn forget_lane(&mut self, lane: &str, cx: &mut Context<Self>) {
        self.runs.remove(lane);
        let gone: Vec<Key> = self
            .wanted
            .iter()
            .filter(|(_, request)| request.lane == lane)
            .map(|(key, _)| key.clone())
            .collect();
        for key in gone {
            self.wanted.remove(&key);
        }
        for shown in self.viewers.values_mut().filter(|shown| shown.lane == lane) {
            // Their sets are forgotten so that the next `show` asks for everything again.
            let keys = std::mem::take(&mut shown.keys);
            for key in keys {
                if let Some(n) = self.showing.get_mut(&key) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        self.showing.remove(&key);
                    }
                }
            }
        }
        self.source.cancel(lane, None, self.seq);
        cx.notify();
    }

    fn stop_showing(&mut self, key: &Key) {
        if let Some(n) = self.showing.get_mut(key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.showing.remove(key);
            }
        }
    }

    // ---- Asking and answering ---------------------------------------------------------------------

    fn ask(&mut self, key: Key, want: Want, lane: &str, retries: u8, cx: &mut Context<Self>) {
        if want.vector {
            self.draw(key, &want, cx);
            return;
        }
        self.seq = self.seq.wrapping_add(1).max(1);
        let seq = self.seq;
        let run = self.runs.get_mut(lane).map(|run| {
            if run.pending == 0 {
                run.start_burst();
            }
            run.pending += 1;
            run.id
        });
        self.wanted.insert(
            key.clone(),
            Request {
                seq,
                lane: lane.to_string(),
                run,
                want: want.clone(),
                retries,
            },
        );
        let running = self.source.thumbnail(Ask {
            path: want.path.clone(),
            size: want.step,
            modified: want.modified,
            bytes: want.bytes,
            cached_only: want.cached_only,
            lane: lane.to_string(),
            seq,
        });
        let renderer = cx.svg_renderer();
        let css_box = want.css_box;
        cx.spawn(async move |this, cx| {
            let answer = running.await;
            // The BMP is decoded off the UI thread.
            let decoded = cx
                .background_executor()
                .spawn(async move {
                    let bytes = answer?;
                    let header =
                        parse_header(&bytes).ok_or_else(|| "a thumbnail answer too short to read".to_string())?;
                    let image = if header.found {
                        decode_bmp(bytes.get(8..).map(<[u8]>::to_vec).unwrap_or_default(), renderer)
                    } else {
                        None
                    };
                    Ok::<_, String>((header, image))
                })
                .await;
            this.update(cx, |this, cx| this.answered(key, seq, css_box, decoded, cx))
                .ok();
        })
        .detach();
    }

    /// Draws an SVG file on a background thread (`Thumbnail.tsx` gave it to the browser as an <img>): no source,
    /// no lane, and so no part in the status bar's thumbnail run.
    fn draw(&mut self, key: Key, want: &Want, cx: &mut Context<Self>) {
        self.drawing.insert(key.clone());
        let renderer = cx.svg_renderer();
        let (path, step, css_box) = (want.path.clone(), want.step, want.css_box);
        cx.spawn(async move |this, cx| {
            let drawn = cx
                .background_executor()
                .spawn(async move { draw_vector(&path, step, renderer) })
                .await;
            this.update(cx, |this, cx| {
                this.drawing.remove(&key);
                // A drawing that failed keeps the file's icon, as the web page did when its <img> errored.
                if !this.held.contains(&key) {
                    this.hold(key, drawn, css_box);
                }
                this.evict();
                cx.emit(Arrived);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn answered(
        &mut self,
        key: Key,
        seq: u32,
        css_box: f32,
        answer: Result<(Header, Option<Arc<RenderImage>>), String>,
        cx: &mut Context<Self>,
    ) {
        let current = self.wanted.get(&key).map(|request| request.seq);
        match answer {
            Ok((header, image)) => {
                if let Some(request) = self.wanted.remove(&key) {
                    self.count(&request, Some((&header, image.is_some())));
                }
                // A file with no thumbnail yet (online-only, or its handler timed out) is asked again next time
                // it is shown.
                if !self.held.contains(&key) && (image.is_some() || header.lasting) {
                    self.hold(key, image, css_box);
                }
            }
            Err(error) => {
                if current != Some(seq) {
                    return;
                }
                let request = self.wanted.remove(&key);
                if let Some(request) = &request {
                    self.count(request, None);
                }
                if error == "cancelled" {
                    // Dropped with the tab that asked (it closed or moved on) while a view in another pane
                    // waits: asked again, in that view's tab.
                    if let Some(request) = request.filter(|request| request.retries < MAX_RETRIES) {
                        let lane = self
                            .viewers
                            .values()
                            .find(|shown| shown.keys.contains(&key))
                            .map(|shown| shown.lane.clone());
                        if let Some(lane) = lane {
                            self.ask(key, request.want, &lane, request.retries + 1, cx);
                        }
                    }
                } else {
                    // A thumbnail that failed isn't asked for again.
                    self.hold(key, None, css_box);
                }
            }
        }
        self.evict();
        cx.emit(Arrived);
        cx.notify();
    }

    /// Keeps a picture (`hold`), as the file's latest.
    fn hold(&mut self, key: Key, image: Option<Arc<RenderImage>>, css_box: f32) {
        let px = image.as_deref().map_or((0, 0), pixels_of);
        let bytes = (px.0 as usize).saturating_mul(px.1 as usize).saturating_mul(4);
        let cost = if bytes == 0 { NONE_COST } else { bytes };
        // The file's first picture fades in over its icon; one that replaces another size's doesn't.
        let since = (image.is_some() && !self.latest.contains(&key.path)).then(std::time::Instant::now);
        if image.is_some() {
            self.latest.insert(key.path.clone(), Latest { key: key.clone() });
        }
        self.held_bytes += cost;
        if let Some(old) = self.held.insert(
            key,
            Held {
                since,
                image,
                px,
                css_box,
                cost,
            },
        ) {
            self.held_bytes = self.held_bytes.saturating_sub(old.cost);
            self.evicted.extend(old.image);
        }
    }

    /// Lets go of the least recently shown pictures while over budget; the ones on screen stay.
    fn evict(&mut self) {
        while self.held_bytes > self.budget {
            let showing = &self.showing;
            let Some((_, gone)) = self.held.pop_oldest(|key, _| showing.contains_key(key)) else {
                break;
            };
            self.held_bytes = self.held_bytes.saturating_sub(gone.cost);
            self.evicted.extend(gone.image);
        }
        while self.latest.len() > LATEST_FILES {
            let Some(_) = self.latest.pop_oldest(|_, _| false) else {
                break;
            };
        }
    }

    /// The pictures let go of since the last call. A view with a window frees each from the GPU's atlas with
    /// `window.drop_image`, once per frame.
    pub fn take_evicted(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.evicted)
    }

    // ---- What to draw -----------------------------------------------------------------------------

    /// What a tile of `css_box` pixels draws for `entry` at a window `scale` (device pixels to one): this
    /// size's picture, else another size's as a stand-in while it comes (unless far smaller), else the icon.
    pub fn look(&self, entry: &FileEntry, css_box: f32, scale: f32) -> Look {
        self.look_at(entry, css_box, scale, false)
    }

    /// [`Thumbnails::look`], with a file's first picture fading in for a moment after it comes ([`Drawn::shown`]
    /// under 1). The view that draws it must keep drawing while [`Look::arriving`] says so
    /// (`window.request_animation_frame()`), or the picture would stay half there.
    pub fn look_arriving(&self, entry: &FileEntry, css_box: f32, scale: f32) -> Look {
        self.look_at(entry, css_box, scale, true)
    }

    fn look_at(&self, entry: &FileEntry, css_box: f32, scale: f32, arriving: bool) -> Look {
        if !is_eligible(entry) {
            return Look::Icon;
        }
        let key = Key {
            path: entry.path.to_lowercase(),
            step: thumb_step(css_box, scale),
            modified: entry.modified,
            bytes: entry.size,
            cloud: is_online_only(entry),
        };
        match self.held.get(&key) {
            // This size's picture is in.
            Some(Held {
                image: Some(image),
                px,
                since,
                ..
            }) => {
                let (w, h) = (px.0 as f32 / scale, px.1 as f32 / scale);
                let (width, height) = fit(w, h, css_box, css_box, false);
                let shown = match since.filter(|_| arriving) {
                    Some(since) => (since.elapsed().as_secs_f32() / ARRIVE.as_secs_f32()).min(1.),
                    None => 1.,
                };
                Look::Picture(Drawn {
                    image: image.clone(),
                    width,
                    height,
                    shown,
                })
            }
            // It has none, for good: the icon.
            Some(Held { image: None, .. }) => Look::Icon,
            None => self.stand_in(&key, css_box, scale),
        }
    }

    fn stand_in(&self, key: &Key, css_box: f32, scale: f32) -> Look {
        let Some(latest) = self.latest.get(&key.path) else {
            return Look::Icon;
        };
        // Not far smaller than the square (a picture a quarter of its size or less would be blown up).
        match self.held.get(&latest.key) {
            Some(Held {
                image: Some(image),
                px,
                css_box: asked,
                ..
            }) if asked * 4.0 >= css_box => {
                let (w, h) = (px.0 as f32 / scale, px.1 as f32 / scale);
                let (width, height) = fit(w, h, *asked, css_box, true);
                Look::StandIn(Drawn {
                    image: image.clone(),
                    width,
                    height,
                    shown: 1.,
                })
            }
            _ => Look::Icon,
        }
    }

    // ---- Timing -----------------------------------------------------------------------------------

    /// A view (grid, or list with thumbnails) opens `scope` in tab `lane`: its thumbnails are timed from here
    /// (`timeThumbnails`).
    pub fn time(&mut self, lane: &str, scope: &str, cx: &mut Context<Self>) {
        if self.runs.get(lane).is_some_and(|run| run.scope == scope) {
            return;
        }
        self.run_ids += 1;
        let mut run = ThumbRun {
            scope: scope.to_string(),
            started: None,
            pending: 0,
            found: 0,
            memory: 0,
            cache: 0,
            made: 0,
            made_ms: 0.0,
            elapsed_ms: 0.0,
            id: self.run_ids,
        };
        run.start_burst();
        self.runs.insert(lane.to_string(), run);
        cx.notify();
    }

    /// A tab's latest burst (`useThumbRuns`), for the status bar.
    pub fn run(&self, lane: &str) -> Option<&ThumbRun> {
        self.runs.get(lane)
    }

    /// Counts an answer in its burst; `None` for a request dropped (scrolled away), which ends the burst
    /// without counting (`count`).
    fn count(&mut self, request: &Request, answer: Option<(&Header, bool)>) {
        let Some(id) = request.run else { return };
        let Some(run) = self.runs.get_mut(&request.lane).filter(|run| run.id == id) else {
            return;
        };
        run.pending = run.pending.saturating_sub(1);
        if let Some((header, has_picture)) = answer {
            run.elapsed_ms = run.started.map_or(0.0, |at| at.elapsed().as_secs_f64() * 1000.0);
            if has_picture {
                run.found += 1;
                match header.from {
                    Origin::Memory => run.memory += 1,
                    Origin::Cache => run.cache += 1,
                    Origin::Made => {
                        run.made += 1;
                        run.made_ms += f64::from(header.ms);
                    }
                }
            }
        }
    }

    // ---- Introspection (tests, tools) -------------------------------------------------------------

    /// How many thumbnails (pictures and "none"s) are on hand.
    pub fn held_count(&self) -> usize {
        self.held.len()
    }

    /// How many requests are out (SVGs being drawn included).
    pub fn pending_count(&self) -> usize {
        self.wanted.len() + self.drawing.len()
    }

    /// The decoded bytes held, against the budget.
    pub fn held_bytes(&self) -> usize {
        self.held_bytes
    }

    /// Changes the budget (tests).
    pub fn set_budget(&mut self, bytes: usize) {
        self.budget = bytes;
    }
}

// ---- Drawing --------------------------------------------------------------------------------------------

/// The folder icon's fill: the Lucide folder's outline filled with `fill`, as a picture to put under the outlined
/// icon (the web drew `fill-amber-400/80` on it; the kit's icons are strokes only). The picture is drawn once per
/// colour and kept, so a frame costs a lookup.
pub fn folder_fill(fill: Hsla) -> Img {
    thread_local! {
        static FILLS: std::cell::RefCell<HashMap<[u8; 4], Arc<Image>>> = std::cell::RefCell::new(HashMap::new());
    }
    let rgba = gpui_kit::Rgba::from(fill);
    // A colour channel is 0..=1, so the product is 0..=255.
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    let key = [byte(rgba.r), byte(rgba.g), byte(rgba.b), byte(rgba.a)];
    let image = FILLS.with(|fills| {
        fills
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                #[cfg(test)]
                FILLS_MADE.with(|made| made.set(made.get() + 1));
                Arc::new(Image::from_bytes(ImageFormat::Svg, folder_fill_svg(key).into_bytes()))
            })
            .clone()
    });
    img(image)
}

// How many fill pictures were made on this thread (tests).
#[cfg(test)]
thread_local! {
    static FILLS_MADE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The outline of lucide's `folder` (24 x 24), filled with `rgba` (red, green, blue, alpha as bytes), drawn at 48 px
/// (GPUI rasterizes at twice that) so it stays sharp when scaled down.
fn folder_fill_svg(rgba: [u8; 4]) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"48\" height=\"48\" viewBox=\"0 0 24 24\">         <path d=\"M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z\"          fill=\"#{:02x}{:02x}{:02x}\" fill-opacity=\"{:.3}\"/></svg>",
        rgba[0],
        rgba[1],
        rgba[2],
        f32::from(rgba[3]) / 255.
    )
}

/// A kind's icon, `icon_px` square: the kit's outlined icon in the kind's colour, and under it the kind's fill when
/// it has one (the folder's amber).
pub fn kind_icon_filled(kind: &Kind, icon_px: Pixels, cx: &App) -> Div {
    let tone = theme::kind_tone(kind, cx);
    let filled = kind.icon == glint_model::file_types::Icon::Folder;
    div()
        .relative()
        .flex_none()
        .size(icon_px)
        .when_some(tone.fill.filter(|_| filled), |tile, fill| {
            tile.child(folder_fill(fill).absolute().inset_0().size_full())
        })
        .child(icon(kind_icon(kind.icon)).size(icon_px).text_color(tone.color))
}

/// A tile's thumbnail square (the TypeScript's `Thumbnail` inside its `relative grid place-items-center` box):
/// the kind's icon, `icon_px` square, until the picture is in; then the picture, centred and (with `bottom`)
/// sitting on the bottom of its square as in Explorer, so names line up under pictures of any shape.
pub fn thumbnail_square(look: &Look, kind: &Kind, box_px: f32, icon_px: f32, bottom: bool, cx: &App) -> Div {
    let square = div()
        .relative()
        .flex_none()
        .size(px(box_px))
        .flex()
        .items_center()
        .justify_center();
    match look {
        Look::Icon => square.child(kind_icon_filled(kind, px(icon_px), cx)),
        Look::Picture(drawn) | Look::StandIn(drawn) => {
            // A picture that has just come in fades in over the icon it takes the place of.
            let arriving = drawn.shown < 1.;
            let square = if arriving {
                square.child(
                    div()
                        .opacity(1. - drawn.shown)
                        .child(kind_icon_filled(kind, px(icon_px), cx)),
                )
            } else {
                square
            };
            // A tile's own picture is a small print: rounded, lifted off the sheet. A row's stays a plain square
            // with its corners just taken off; a stand-in (a kind's icon as a picture) gets neither.
            let print = !matches!(look, Look::StandIn(_));
            let picture = img(drawn.image.clone())
                .w(px(drawn.width))
                .h(px(drawn.height))
                .flex_none()
                .when(print && bottom, |picture| {
                    picture
                        .rounded(theme::size::RADIUS)
                        .shadow(theme::elevation::raised(cx))
                })
                .when(print && !bottom, |picture| picture.rounded(px(2.)));
            square.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .justify_center()
                    .when(bottom, |layer| layer.items_end())
                    .when(!bottom, |layer| layer.items_center())
                    .when(arriving, |layer| layer.opacity(drawn.shown))
                    .child(picture),
            )
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::explorer::fake::file;
    use crate::testing;
    use glint_core::thumbs::{encode_bmp, Pixels};
    use std::cell::RefCell;

    #[gpui_kit::test]
    fn the_folder_fill_is_a_picture_made_once_per_colour_that_decodes_with_its_alpha(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let _guard = testing::lock();
        cx.update(|cx| {
            let amber = gpui_kit::hsla(0.12, 0.9, 0.6, 0.8);
            // Same colour, same picture: the cache hands back the one made.
            let made = FILLS_MADE.with(|made| made.get());
            let _ = folder_fill(amber);
            let _ = folder_fill(amber);
            assert_eq!(FILLS_MADE.with(|made| made.get()), made + 1, "made once for a colour");
            // The SVG is one GPUI's renderer reads, at the size it was drawn.
            let svg = folder_fill_svg([0xf5, 0xa6, 0x23, 0xcc]);
            let image = gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Svg, svg.into_bytes());
            let drawn = image.to_image_data(cx.svg_renderer()).expect("the fill decodes");
            let size = drawn.size(0);
            assert_eq!((size.width.0, size.height.0), (96, 96));
        });
    }

    /// The answer core sends: the 8-byte header, then the BMP.
    pub(crate) fn answer(width: u32, height: u32, bgra: [u8; 4], lasting: bool, from: u8) -> Vec<u8> {
        let pixels = Pixels {
            width,
            height,
            bgra: bgra
                .iter()
                .copied()
                .cycle()
                .take((width * height * 4) as usize)
                .collect(),
        };
        let mut out = vec![0, from, lasting as u8, 0];
        out.extend(1.5f32.to_le_bytes());
        out.extend(encode_bmp(&pixels));
        out
    }

    /// An answer saying the file has no thumbnail.
    pub(crate) fn none(lasting: bool) -> Vec<u8> {
        let mut out = vec![1, 1, lasting as u8, 0];
        out.extend(0f32.to_le_bytes());
        out
    }

    /// A cancel as the source saw it: the lane, the paths (None for all), the request number.
    pub(crate) type Cancel = (String, Option<Vec<String>>, u32);

    /// A source answering from a table, counting asks and cancels, and holding answers back on request.
    #[derive(Default)]
    pub(crate) struct Fake {
        pub answers: RefCell<HashMap<String, Vec<u8>>>,
        pub asked: RefCell<Vec<Ask>>,
        pub cancelled: RefCell<Vec<Cancel>>,
        pub held_back: RefCell<HashSet<String>>,
        /// Every ask comes back cancelled.
        pub cancels_all: std::cell::Cell<bool>,
    }

    impl ThumbSource for Rc<Fake> {
        fn thumbnail(&self, ask: Ask) -> Fut<Result<Vec<u8>, String>> {
            self.asked.borrow_mut().push(ask.clone());
            let answer = self.answers.borrow().get(&ask.path).cloned();
            let held = self.held_back.borrow().contains(&ask.path);
            let cancelled = self.cancels_all.get();
            Box::pin(async move {
                if cancelled {
                    return Err("cancelled".to_string());
                }
                if held {
                    // Never answers (until the test is over).
                    std::future::pending::<()>().await;
                }
                Ok(answer.unwrap_or_else(|| none(true)))
            })
        }

        fn cancel(&self, lane: &str, paths: Option<Vec<String>>, up_to: u32) {
            self.cancelled.borrow_mut().push((lane.to_string(), paths, up_to));
        }
    }

    fn want(path: &str) -> Want {
        Want {
            path: path.into(),
            modified: 1,
            bytes: 2,
            cached_only: false,
            step: 96,
            css_box: 96.0,
            vector: false,
        }
    }

    fn settle(cx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the thumbnail never arrived");
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn service(cx: &mut TestAppContext, fake: &Rc<Fake>) -> (VisualTestContext, Entity<Thumbnails>) {
        cx.update(testing::init);
        let (visual, _) = testing::open_window(cx, |_, cx| cx.new(|_| Host));
        let source: Rc<dyn ThumbSource> = Rc::new(fake.clone());
        let service = cx.update(|cx| init_with(cx, source));
        (visual, service)
    }

    #[test]
    fn the_header_is_read_as_core_writes_it() {
        let mut bytes = answer(2, 2, [1, 2, 3, 255], true, 2);
        let header = parse_header(&bytes);
        assert_eq!(
            header,
            Some(Header {
                found: true,
                lasting: true,
                from: Origin::Made,
                ms: 1.5
            })
        );
        // A header alone is "none".
        bytes.truncate(8);
        assert_eq!(parse_header(&bytes).map(|h| h.found), Some(false));
        assert_eq!(
            parse_header(&none(false)).map(|h| (h.found, h.lasting)),
            Some((false, false))
        );
        assert_eq!(parse_header(&[0, 1, 2]), None, "fewer than 8 bytes are no answer");
        assert_eq!(parse_header(&none(true)).map(|h| h.lasting), Some(true));
    }

    #[gpui_kit::test]
    fn a_bmp_from_core_decodes_to_bgra_pixels_the_size_it_was_made(cx: &mut TestAppContext) {
        let bytes = answer(3, 2, [10, 20, 30, 255], true, 1);
        let renderer = cx.update(|cx| cx.svg_renderer());
        let image = decode_bmp(bytes[8..].to_vec(), renderer).expect("the BMP decodes");
        assert_eq!(pixels_of(&image), (3, 2));
        let pixels = image.as_bytes(0).expect("one frame");
        // BGRA in, BGRA out: a 3 x 2 picture is 6 pixels of the same colour, opaque.
        assert_eq!(pixels.len(), 3 * 2 * 4);
        for pixel in pixels.as_chunks::<4>().0 {
            assert_eq!(*pixel, [10, 20, 30, 255]);
        }
    }

    #[gpui_kit::test]
    fn straight_alpha_in_the_bmp_stays_straight(cx: &mut TestAppContext) {
        let bytes = answer(1, 1, [10, 20, 30, 128], true, 1);
        let renderer = cx.update(|cx| cx.svg_renderer());
        let image = decode_bmp(bytes[8..].to_vec(), renderer).expect("the BMP decodes");
        assert_eq!(image.as_bytes(0), Some(&[10u8, 20, 30, 128][..]));
    }

    #[test]
    fn a_picture_fits_its_square_without_being_blown_up() {
        // Smaller than the square: kept at its own size. Bigger: shrunk to fit, shape kept.
        assert_eq!(fit(40.0, 20.0, 96.0, 96.0, false), (40.0, 20.0));
        assert_eq!(fit(192.0, 96.0, 96.0, 96.0, false), (96.0, 48.0));
        // A stand-in that filled its square fills the new one, bigger or smaller.
        assert_eq!(fit(96.0, 48.0, 96.0, 128.0, true), (128.0, 64.0));
        assert_eq!(fit(96.0, 48.0, 96.0, 64.0, true), (64.0, 32.0));
        // A stand-in that didn't fill its square (a small picture) is kept at its size.
        assert_eq!(fit(30.0, 20.0, 96.0, 128.0, true), (30.0, 20.0));
        assert_eq!(fit(0.0, 0.0, 96.0, 96.0, false), (0.0, 0.0));
    }

    #[test]
    fn only_files_windows_can_make_thumbnails_of_are_asked_for() {
        assert!(Want::for_entry(&file("C:\\x", "a.jpg", 5, 1), 96.0, 1.0).is_some());
        let folder = crate::explorer::fake::folder("C:\\x", "dir");
        assert!(Want::for_entry(&folder, 96.0, 1.0).is_none());
        let asked = Want::for_entry(&file("C:\\x", "a.jpg", 5, 7), 96.0, 1.25).expect("a file");
        assert_eq!((asked.step, asked.modified, asked.bytes), (120, 7, 5));
    }

    #[test]
    fn the_lru_evicts_the_oldest_that_is_not_kept() {
        let mut lru = Lru::new();
        for n in 0..4 {
            lru.insert(n, n * 10);
        }
        lru.touch(&0);
        // 1 is the oldest now; keep it, and 2 goes.
        assert_eq!(lru.pop_oldest(|k, _| *k == 1), Some((2, 20)));
        assert_eq!(lru.pop_oldest(|_, _| false), Some((1, 10)));
        assert_eq!(lru.len(), 2);
        assert_eq!(lru.pop_oldest(|_, _| true), None, "everything left is kept");
        assert_eq!(lru.insert(0, 1), Some(0));
        assert_eq!(lru.get(&0), Some(&1));
    }

    #[gpui_kit::test]
    fn asked_once_answered_decoded_and_drawn_instead_of_the_icon(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Rc::new(Fake::default());
        fake.answers
            .borrow_mut()
            .insert("C:\\x\\a.jpg".into(), answer(48, 32, [9, 9, 9, 255], true, 1));
        let (mut visual, service) = service(cx, &fake);
        let entry = file("C:\\x", "a.jpg", 2, 1);
        let viewer = service.entity_id();

        service.update(&mut visual, |t, cx| {
            t.show(viewer, "tab1", vec![want("C:\\x\\a.jpg")], cx)
        });
        // The icon stays until the picture is in.
        assert!(matches!(
            service.read_with(&visual, |t, _| t.look(&entry, 96.0, 1.0)),
            Look::Icon
        ));
        settle(&mut visual, |cx| service.read_with(cx, |t, _| t.held_count() == 1));
        match service.read_with(&visual, |t, _| t.look(&entry, 96.0, 1.0)) {
            Look::Picture(drawn) => assert_eq!((drawn.width, drawn.height), (48.0, 32.0)),
            _ => panic!("the picture should be drawn"),
        }
        // Showing the same set again asks nothing more.
        service.update(&mut visual, |t, cx| {
            t.show(viewer, "tab1", vec![want("C:\\x\\a.jpg")], cx)
        });
        assert_eq!(fake.asked.borrow().len(), 1);
        assert_eq!(fake.asked.borrow()[0].lane, "tab1");
        assert_eq!(fake.asked.borrow()[0].size, 96);
    }

    #[gpui_kit::test]
    fn what_scrolled_away_is_cancelled_and_what_another_view_shows_is_not(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Rc::new(Fake::default());
        for name in ["a", "b", "c"] {
            fake.held_back.borrow_mut().insert(format!("C:\\x\\{name}.jpg"));
        }
        let (mut visual, service) = service(cx, &fake);
        let (one, two) = (EntityId::from(1u64), EntityId::from(2u64));
        service.update(&mut visual, |t, cx| {
            t.show(one, "tab1", vec![want("C:\\x\\a.jpg"), want("C:\\x\\b.jpg")], cx);
            t.show(two, "tab2", vec![want("C:\\x\\b.jpg")], cx);
        });
        assert_eq!(fake.asked.borrow().len(), 2, "b is asked once for both views");
        // The first view scrolls on: a is let go of (cancelled), b is still shown by the second view, c is new.
        service.update(&mut visual, |t, cx| t.show(one, "tab1", vec![want("C:\\x\\c.jpg")], cx));
        let cancelled = fake.cancelled.borrow().clone();
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].0, "tab1");
        assert_eq!(cancelled[0].1, Some(vec!["c:\\x\\a.jpg".to_string()]));
        assert_eq!(
            service.read_with(&visual, |t, _| t.pending_count()),
            2,
            "b and c are out"
        );
        // The last view letting go cancels the rest.
        service.update(&mut visual, |t, cx| {
            t.release(one, cx);
            t.release(two, cx);
        });
        assert_eq!(service.read_with(&visual, |t, _| t.pending_count()), 0);
        assert!(fake.cancelled.borrow().len() >= 3);
    }

    #[gpui_kit::test]
    fn a_source_that_keeps_cancelling_is_asked_a_few_times_not_for_ever(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Rc::new(Fake::default());
        fake.cancels_all.set(true);
        let (mut visual, service) = service(cx, &fake);
        let viewer = EntityId::from(13u64);
        service.update(&mut visual, |t, cx| {
            t.show(viewer, "tab1", vec![want("C:\\x\\a.jpg")], cx)
        });
        settle(&mut visual, |cx| service.read_with(cx, |t, _| t.pending_count() == 0));
        // The first ask and MAX_RETRIES more.
        assert_eq!(fake.asked.borrow().len(), 1 + usize::from(MAX_RETRIES));
    }

    #[gpui_kit::test]
    fn a_tabs_folder_changing_cancels_all_it_waits_for_and_asks_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Rc::new(Fake::default());
        fake.held_back.borrow_mut().insert("C:\\x\\a.jpg".into());
        let (mut visual, service) = service(cx, &fake);
        let viewer = EntityId::from(7u64);
        service.update(&mut visual, |t, cx| {
            t.show(viewer, "tab1", vec![want("C:\\x\\a.jpg")], cx)
        });
        service.update(&mut visual, |t, cx| t.forget_lane("tab1", cx));
        assert_eq!(
            fake.cancelled.borrow().last().map(|c| (c.0.clone(), c.1.clone())),
            Some(("tab1".into(), None))
        );
        assert_eq!(service.read_with(&visual, |t, _| t.pending_count()), 0);
        // The same set shown again is asked for again: the view started over.
        service.update(&mut visual, |t, cx| {
            t.show(viewer, "tab1", vec![want("C:\\x\\a.jpg")], cx)
        });
        assert_eq!(fake.asked.borrow().len(), 2);
    }

    #[gpui_kit::test]
    fn a_file_without_a_thumbnail_keeps_its_icon_and_is_only_asked_again_when_it_comes_back(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Rc::new(Fake::default());
        // "None yet" (not lasting) for a, "none for good" for b.
        fake.answers.borrow_mut().insert("C:\\x\\a.txt".into(), none(false));
        fake.answers.borrow_mut().insert("C:\\x\\b.txt".into(), none(true));
        let (mut visual, service) = service(cx, &fake);
        let viewer = EntityId::from(3u64);
        let both = || vec![want("C:\\x\\a.txt"), want("C:\\x\\b.txt")];
        service.update(&mut visual, |t, cx| t.show(viewer, "tab1", both(), cx));
        settle(&mut visual, |cx| service.read_with(cx, |t, _| t.pending_count() == 0));
        assert_eq!(
            service.read_with(&visual, |t, _| t.held_count()),
            1,
            "only the lasting none is remembered"
        );
        // Still shown: nothing is asked again however often the view redraws.
        service.update(&mut visual, |t, cx| t.show(viewer, "tab1", both(), cx));
        assert_eq!(fake.asked.borrow().len(), 2);
        // Scrolled away and back: a (none yet) is asked again, b (none for good) is not.
        service.update(&mut visual, |t, cx| t.show(viewer, "tab1", vec![], cx));
        service.update(&mut visual, |t, cx| t.show(viewer, "tab1", both(), cx));
        assert_eq!(fake.asked.borrow().len(), 3);
        assert_eq!(fake.asked.borrow()[2].path, "C:\\x\\a.txt");
    }

    #[gpui_kit::test]
    fn another_sizes_picture_stands_in_while_a_zoom_comes_and_is_dropped_when_far_smaller(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Rc::new(Fake::default());
        fake.answers
            .borrow_mut()
            .insert("C:\\x\\a.jpg".into(), answer(96, 64, [1, 2, 3, 255], true, 1));
        let (mut visual, service) = service(cx, &fake);
        let entry = file("C:\\x", "a.jpg", 2, 1);
        let viewer = EntityId::from(5u64);
        service.update(&mut visual, |t, cx| {
            t.show(viewer, "tab1", vec![want("C:\\x\\a.jpg")], cx)
        });
        settle(&mut visual, |cx| service.read_with(cx, |t, _| t.held_count() == 1));
        // The tile grows to 128: this size isn't in, the 96 one stands in, filling the new square.
        match service.read_with(&visual, |t, _| t.look(&entry, 128.0, 1.0)) {
            Look::StandIn(drawn) => assert_eq!((drawn.width, drawn.height), (128.0, 128.0 * 64.0 / 96.0)),
            _ => panic!("the 96 px picture should stand in"),
        }
        // A square more than four times the size gets the icon instead.
        assert!(matches!(
            service.read_with(&visual, |t, _| t.look(&entry, 400.0, 1.0)),
            Look::Icon
        ));
    }

    #[gpui_kit::test]
    fn pictures_over_the_budget_go_oldest_first_and_the_ones_shown_stay(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Rc::new(Fake::default());
        for name in ["a", "b", "c"] {
            fake.answers
                .borrow_mut()
                .insert(format!("C:\\x\\{name}.jpg"), answer(16, 16, [1, 2, 3, 255], true, 1));
        }
        let (mut visual, service) = service(cx, &fake);
        // Room for two 16 x 16 pictures (1,024 bytes each).
        service.update(&mut visual, |t, _| t.set_budget(2 * 1024));
        let viewer = EntityId::from(9u64);
        for name in ["a", "b", "c"] {
            let path = format!("C:\\x\\{name}.jpg");
            service.update(&mut visual, |t, cx| t.show(viewer, "tab1", vec![want(&path)], cx));
            settle(&mut visual, |cx| service.read_with(cx, |t, _| t.pending_count() == 0));
        }
        // c is shown: a (the oldest) went, and b is still there.
        let entry = |name: &str| file("C:\\x", name, 2, 1);
        assert!(matches!(
            service.read_with(&visual, |t, _| t.look(&entry("c.jpg"), 96.0, 1.0)),
            Look::Picture(_)
        ));
        assert!(service.read_with(&visual, |t, _| t.held_bytes() <= 2 * 1024));
        assert_eq!(service.read_with(&visual, |t, _| t.held_count()), 2);
        // What went is handed over to be freed from the GPU's atlas.
        let evicted = service.update(&mut visual, |t, _| t.take_evicted());
        assert_eq!(evicted.len(), 1);
    }

    #[gpui_kit::test]
    fn a_burst_is_timed_from_its_first_ask_to_its_last_answer(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Rc::new(Fake::default());
        fake.answers
            .borrow_mut()
            .insert("C:\\x\\a.jpg".into(), answer(8, 8, [1, 2, 3, 255], true, 2));
        let (mut visual, service) = service(cx, &fake);
        service.update(&mut visual, |t, cx| t.time("tab1", "dir:C:\\x:grid", cx));
        let viewer = EntityId::from(11u64);
        service.update(&mut visual, |t, cx| {
            t.show(viewer, "tab1", vec![want("C:\\x\\a.jpg")], cx)
        });
        assert_eq!(
            service.read_with(&visual, |t, _| t.run("tab1").map(|r| r.pending)),
            Some(1)
        );
        settle(&mut visual, |cx| service.read_with(cx, |t, _| t.pending_count() == 0));
        let run = service
            .read_with(&visual, |t, _| t.run("tab1").cloned())
            .expect("a run");
        assert_eq!((run.found, run.made, run.pending), (1, 1, 0));
        assert!((run.made_ms - 1.5).abs() < 1e-6 && run.elapsed_ms >= 0.0);
        // The same scope again keeps the burst; another starts a new one.
        service.update(&mut visual, |t, cx| t.time("tab1", "dir:C:\\x:grid", cx));
        assert_eq!(
            service.read_with(&visual, |t, _| t.run("tab1").map(|r| r.found)),
            Some(1)
        );
        service.update(&mut visual, |t, cx| t.time("tab1", "dir:C:\\y:grid", cx));
        assert_eq!(
            service.read_with(&visual, |t, _| t.run("tab1").map(|r| r.found)),
            Some(0)
        );
    }
}
