//! 3D models: STL, OBJ, PLY, glTF/GLB and 3MF, drawn and turned (`ModelView` in ModelView.tsx).
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                          | Here                                                        |
//! |-----------------------------------------------------|-------------------------------------------------------------|
//! | `MAX_BYTES`, "It's too big to draw here"            | [`MAX_BYTES`]; a bigger file is not read                    |
//! | `STLLoader`, `OBJLoader`, `PLYLoader`               | `model3d_load.rs`: [`load_stl`], [`load_obj`], [`load_ply`] |
//! | `GLTFLoader`, `ThreeMFLoader`                       | `model3d_scene.rs`: [`load_gltf`], [`load_3mf`]             |
//! | `meshOf` (normals when the file has none)           | `model3d_mesh.rs`: one [`Mesh`], [`Shading`]                |
//! | the `WebGLRenderer`, lights, `OrbitControls`        | `model3d_raster.rs`: [`render`] and [`Camera`], on the CPU  |
//! | "N triangles · drag to turn, wheel to zoom"         | the line over the bottom of the picture                     |
//! | "Drawing it…", the error line                       | [`waiting`]; the file's icon with the reason                |
//!
//! # How it works
//!
//! The file is read and turned into a [`Mesh`] on a background thread. The mesh is drawn by [`render`] on a
//! background thread too, into a picture the size of the pane (in device pixels), shown as a GPUI image; the UI
//! thread only builds the `RenderImage` from the finished pixels. Drawing is *latest wins*: while one picture is
//! being drawn, newer requests wait, and when it arrives the view looks at what it wants now and starts the next
//! draw if that has changed. While the model is being turned (a drag, the wheel, a key) the picture is drawn at
//! half size from at most 150 000 triangles, and when it has been still for [`SETTLE`] (at once on letting go of
//! the button) the full picture is drawn, with smooth edges.
//!
//! **Controls** (`OrbitControls`): drag to turn, wheel to zoom, right-drag (or Shift-drag) to move it across, and
//! double-click to start again; the picture fits the pane whatever shape it is. Beyond the TypeScript, for
//! keyboard-only use: the arrow keys turn it, `+` and `-` zoom, `0` or Home starts again, and Page Up and Page
//! Down (Quick Look's scroll keys) turn it an eighth of a turn about the vertical axis. In Quick Look the
//! arrows are Quick Look's own (next and previous item); Shift with an arrow turns the model there.
//!
//! **Pictures are let go of.** A picture that is replaced is queued in [`RETIRED`] and dropped from GPUI's atlas by
//! [`release_retired`], which every [`ModelView`] frame calls and the preview frame should call too (see the
//! wiring notes in the report), so a view that goes away doesn't leave its last picture on the GPU.
//!
//! Not ported: the damping (the model coasting after a drag), the hemisphere light and the shine of
//! `MeshStandardMaterial` (the light here is a plain headlight), materials, textures and animations, and the
//! GPU: a very big model turns at half size and with fewer triangles while dragging. NOTE(port): the TypeScript
//! said "1 triangles" for one triangle; the wording is kept.

#[path = "model3d_load.rs"]
pub mod model3d_load;
#[path = "model3d_mesh.rs"]
pub mod model3d_mesh;
#[path = "model3d_raster.rs"]
pub mod model3d_raster;
#[path = "model3d_scene.rs"]
pub mod model3d_scene;

use super::text::{big_icon, redraw_after_delay, waiting};
use crate::kit::prelude::*;
use crate::{actions, t, theme};
pub use model3d_load::{load_obj, load_ply, load_stl};
pub use model3d_mesh::{LoadError, Mesh, Shading, MAX_BYTES};
pub use model3d_raster::{render, Camera, Params};
pub use model3d_scene::{load_3mf, load_gltf};

use glint_model::entries::FileEntry;
use glint_model::format::format_count;
use glint_model::paths::{join_path, parent_path};
use gpui_kit::{
    canvas, img, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, RenderImage, ScrollEvent, Size,
    StyledImage as _, Task,
};
use image::Frame;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the model has to be still before the full picture is drawn.
pub const SETTLE: Duration = Duration::from_millis(220);
/// The wheel's step (a notch up zooms in by this much).
const ZOOM_STEP: f32 = 1.15;
/// A key's turn.
const KEY_TURN: f32 = std::f32::consts::PI / 12.;
/// Page Up and Page Down.
const PAGE_TURN: f32 = std::f32::consts::FRAC_PI_4;
/// The most triangles drawn while the model is being turned.
const DRAFT_TRIANGLES: usize = 150_000;
/// Models with more triangles than this are drawn without smoothing the edges.
const SMOOTH_TRIANGLES: usize = 400_000;
/// The most pixels of the picture; a bigger pane is drawn smaller and stretched.
const MAX_PIXELS: f32 = 3_000_000.;

thread_local! {
    /// Pictures that have been replaced, until a frame can let them go of the GPU's atlas.
    static RETIRED: RefCell<Vec<Arc<RenderImage>>> = const { RefCell::new(Vec::new()) };
}

/// Lets pictures that are no longer shown go of the GPU's atlas.
pub fn release_retired(window: &mut Window) {
    for image in RETIRED.with(|retired| std::mem::take(&mut *retired.borrow_mut())) {
        window.drop_image(image).ok();
    }
}

/// How many pictures wait to be let go of.
pub fn retired_count() -> usize {
    RETIRED.with(|retired| retired.borrow().len())
}

// ---- Reading ----------------------------------------------------------------------------------------------

/// Reads a file whole: the path in, the bytes out. glTF files ask for their buffers through it too.
pub type ModelSource = Arc<dyn Fn(&str) -> Result<Vec<u8>, LoadError> + Send + Sync>;

/// The file system, refusing what is past [`MAX_BYTES`].
pub fn fs_source() -> ModelSource {
    Arc::new(|path| {
        let size = std::fs::metadata(path)
            .map_err(|e| LoadError::Unreadable(e.to_string()))?
            .len();
        if size > MAX_BYTES {
            return Err(LoadError::TooBig);
        }
        std::fs::read(path).map_err(|e| LoadError::Unreadable(e.to_string()))
    })
}

/// Reads and parses the model at `path`. The format is the extension's, except that a `.glb`'s magic wins; as in
/// the TypeScript anything that isn't STL, PLY, OBJ or 3MF is read as glTF.
pub fn load_model(path: &str, ext: &str, source: &ModelSource) -> Result<Mesh, LoadError> {
    let bytes = source(path)?;
    match ext {
        "stl" => load_stl(&bytes),
        "ply" => load_ply(&bytes),
        "obj" => load_obj(&bytes),
        "3mf" => load_3mf(&bytes),
        _ => {
            let folder = parent_path(path);
            load_gltf(&bytes, &|name: &str| {
                source(&join_path(&folder, &name.replace('/', "\\"))).map_err(|e| match e {
                    LoadError::Unreadable(reason) => reason,
                    other => format!("{other:?}"),
                })
            })
        }
    }
}

/// The words for why a model isn't drawn.
pub fn describe(error: &LoadError) -> SharedString {
    match error {
        LoadError::TooBig => t!("It's too big to draw here"),
        LoadError::Empty => t!("There is nothing in this file to draw"),
        LoadError::Damaged(_) => t!("This isn't a model Glint can read"),
        LoadError::Compressed => t!("This model is compressed in a way Glint can't draw yet"),
        LoadError::Unreadable(_) => t!("Couldn't read this file"),
    }
}

// ---- The view ---------------------------------------------------------------------------------------------

/// The numbers of a model's box, three significant figures at most: "2", "12.5", "0.125".
fn short(v: f32) -> String {
    let places = if v >= 100. {
        0
    } else if v >= 10. {
        1
    } else if v >= 1. {
        2
    } else {
        3
    };
    let text = format!("{v:.places$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

/// A box's size as "W × H × D".
pub fn size_text(size: [f32; 3]) -> String {
    format!("{} × {} × {}", short(size[0]), short(size[1]), short(size[2]))
}

struct Ready {
    mesh: Arc<Mesh>,
    triangles: usize,
}

enum Load {
    Reading,
    Failed(SharedString),
    Ready(Ready),
}

/// What a picture was drawn for.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Key {
    camera: Camera,
    width: u32,
    height: u32,
    /// Full size and smooth edges, as against the draft drawn while turning.
    full: bool,
    color: [u8; 3],
}

struct Shown {
    image: Arc<RenderImage>,
    key: Key,
}

struct Drag {
    /// Moving the model across rather than turning it.
    pan: bool,
    last: Point<Pixels>,
}

/// A 3D model on screen.
pub struct ModelView {
    entry: FileEntry,
    source: ModelSource,
    load: Load,
    started: Instant,
    generation: u64,
    camera: Camera,
    drag: Option<Drag>,
    /// The model is being turned: pictures are drafts until it has been still for [`SETTLE`].
    interacting: bool,
    settle_epoch: u64,
    /// The space the picture takes, measured while drawing (a frame later it is used).
    area: Rc<Cell<Size<Pixels>>>,
    shown: Option<Shown>,
    drawing: Option<Key>,
    draws: usize,
    focus: FocusHandle,
    _read: Task<()>,
    _draw: Task<()>,
    _settle: Task<()>,
}

impl ModelView {
    /// The model of `entry`; reading starts at once, off the UI thread.
    pub fn new(entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(entry, fs_source(), window, cx)
    }

    /// [`ModelView::new`] reading through `source` (tests pass fakes).
    pub fn with_source(entry: FileEntry, source: ModelSource, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = ModelView {
            entry,
            source,
            load: Load::Reading,
            started: Instant::now(),
            generation: 0,
            camera: Camera::home(),
            drag: None,
            interacting: false,
            settle_epoch: 0,
            area: Rc::new(Cell::new(Size::default())),
            shown: None,
            drawing: None,
            draws: 0,
            focus: cx.focus_handle(),
            _read: Task::ready(()),
            _draw: Task::ready(()),
            _settle: Task::ready(()),
        };
        this.read(cx);
        this
    }

    /// The file shown.
    pub fn entry(&self) -> &FileEntry {
        &self.entry
    }

    /// The mesh, once read.
    pub fn mesh(&self) -> Option<&Mesh> {
        match &self.load {
            Load::Ready(ready) => Some(&ready.mesh),
            _ => None,
        }
    }

    /// How many triangles it has.
    pub fn triangles(&self) -> Option<usize> {
        self.mesh().map(Mesh::triangles)
    }

    /// Why it isn't drawn, if it isn't.
    pub fn failure(&self) -> Option<&str> {
        match &self.load {
            Load::Failed(why) => Some(why),
            _ => None,
        }
    }

    /// Where it is looked at from.
    pub fn camera(&self) -> Camera {
        self.camera
    }

    /// The picture on screen and its size in pixels.
    pub fn picture(&self) -> Option<(&Arc<RenderImage>, (u32, u32))> {
        self.shown.as_ref().map(|s| (&s.image, (s.key.width, s.key.height)))
    }

    /// Whether the picture on screen is the full one (not the draft drawn while turning).
    pub fn is_full(&self) -> bool {
        self.shown.as_ref().is_some_and(|s| s.key.full)
    }

    /// How many pictures have been drawn.
    pub fn draws(&self) -> usize {
        self.draws
    }

    /// Reads the file again (it changed on disk).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.read(cx);
    }

    /// Back to the first view (double-click).
    pub fn reset_view(&mut self, cx: &mut Context<Self>) {
        self.camera = Camera::home();
        self.touch(cx);
    }

    /// Page Up and Page Down in Quick Look: turns it an eighth of a turn. Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        if self.mesh().is_none() {
            return false;
        }
        self.camera = self.camera.turned(if down { -PAGE_TURN } else { PAGE_TURN }, 0.);
        self.touch(cx);
        true
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.retire();
        self.drawing = None;
        self._draw = Task::ready(());
        self.camera = Camera::home();
        self.started = Instant::now();
        if self.entry.size > MAX_BYTES {
            self.load = Load::Failed(describe(&LoadError::TooBig));
            cx.notify();
            return;
        }
        self.load = Load::Reading;
        let (path, ext, source) = (self.entry.path.clone(), self.entry.ext.clone(), self.source.clone());
        self._read = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { load_model(&path, &ext, &source) })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.load = match result {
                    Ok(mesh) => Load::Ready(Ready {
                        triangles: mesh.triangles(),
                        mesh: Arc::new(mesh),
                    }),
                    Err(error) => Load::Failed(describe(&error)),
                };
                cx.notify();
            })
            .ok();
        });
        redraw_after_delay(cx);
        cx.notify();
    }

    /// The picture on screen goes to the retired list, to be let go of the GPU.
    fn retire(&mut self) {
        if let Some(shown) = self.shown.take() {
            RETIRED.with(|retired| retired.borrow_mut().push(shown.image));
        }
    }

    // ---- Drawing --------------------------------------------------------------------------------------

    /// The size of the picture to draw for a pane of `area`, in device pixels (and the draft's half of it).
    fn pixels(area: Size<Pixels>, scale: f32) -> (u32, u32) {
        let (mut w, mut h) = (f32::from(area.width) * scale, f32::from(area.height) * scale);
        if w * h > MAX_PIXELS {
            let k = (MAX_PIXELS / (w * h)).sqrt();
            (w, h) = (w * k, h * k);
        }
        (w.round().max(0.) as u32, h.round().max(0.) as u32)
    }

    /// Starts the picture the view wants now, unless it has it or one is being drawn (when that arrives the view
    /// looks again).
    fn want(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Load::Ready(ready) = &self.load else { return };
        let mesh = ready.mesh.clone();
        let (w, h) = Self::pixels(self.area.get(), window.scale_factor());
        if w == 0 || h == 0 || self.drawing.is_some() {
            return;
        }
        let full = !self.interacting;
        let (width, height) = if full { (w, h) } else { ((w / 2).max(1), (h / 2).max(1)) };
        let color = model_color(cx);
        let key = Key {
            camera: self.camera,
            width,
            height,
            full,
            color,
        };
        if self.shown.as_ref().is_some_and(|s| s.key == key) {
            return;
        }
        self.drawing = Some(key);
        let generation = self.generation;
        let params = Params {
            width,
            height,
            supersample: if full && mesh.triangles() <= SMOOTH_TRIANGLES {
                2
            } else {
                1
            },
            color,
            max_triangles: if full { usize::MAX } else { DRAFT_TRIANGLES },
            threads: 0,
        };
        self._draw = cx.spawn(async move |this, cx| {
            let pixels = cx
                .background_spawn(async move {
                    let mut image = render(&mesh, &key.camera, &params);
                    model3d_raster::to_bgra(&mut image);
                    image
                })
                .await;
            this.update(cx, |this, cx| this.drawn(key, generation, pixels, cx)).ok();
        });
    }

    fn drawn(&mut self, key: Key, generation: u64, pixels: image::RgbaImage, cx: &mut Context<Self>) {
        self.drawing = None;
        if generation != self.generation {
            return;
        }
        let image = Arc::new(RenderImage::new(vec![Frame::new(pixels)]));
        self.retire();
        self.shown = Some(Shown { image, key });
        self.draws += 1;
        cx.notify();
    }

    // ---- Turning it -----------------------------------------------------------------------------------

    /// The model moved: draw drafts until it has been still for a moment.
    fn touch(&mut self, cx: &mut Context<Self>) {
        self.interacting = true;
        self.settle_epoch += 1;
        let epoch = self.settle_epoch;
        self._settle = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            this.update(cx, |this, cx| {
                if this.settle_epoch == epoch && this.drag.is_none() {
                    this.interacting = false;
                    cx.notify();
                }
            })
            .ok();
        });
        cx.notify();
    }

    /// The model stopped moving: the full picture, now.
    fn settle(&mut self, cx: &mut Context<Self>) {
        self.settle_epoch += 1;
        self._settle = Task::ready(());
        if self.interacting {
            self.interacting = false;
            cx.notify();
        }
    }

    fn aspect(&self) -> f32 {
        let area = self.area.get();
        f32::from(area.width).max(1.) / f32::from(area.height).max(1.)
    }

    fn pointer_down(&mut self, event: &MouseDownEvent, pan: bool, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if event.click_count == 2 && !pan {
            self.drag = None;
            self.reset_view(cx);
            return;
        }
        self.drag = Some(Drag {
            pan: pan || event.modifiers.shift,
            last: event.position,
        });
        cx.stop_propagation();
    }

    fn pointer_moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else { return };
        if !event.dragging() {
            self.drag = None;
            self.settle(cx);
            return;
        }
        let (dx, dy) = (
            f32::from(event.position.x - drag.last.x),
            f32::from(event.position.y - drag.last.y),
        );
        drag.last = event.position;
        let height = f32::from(self.area.get().height).max(1.);
        if drag.pan {
            // The model follows the pointer, so the camera's target goes the other way.
            let k = model3d_raster::radii_per_pixel(self.aspect(), height, self.camera.zoom);
            self.camera = self.camera.panned(-dx * k, dy * k);
        } else {
            // A drag the full height of the pane is a full turn (`rotateLeft(2π * dx / clientHeight)`).
            let k = std::f32::consts::TAU / height;
            self.camera = self.camera.turned(-dx * k, dy * k);
        }
        self.touch(cx);
    }

    fn pointer_up(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            self.settle(cx);
        }
    }

    /// The wheel: up zooms in.
    fn wheel(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let up = event.delta.pixel_delta(px(16.)).y > px(0.);
        self.camera = self.camera.zoomed(if up { ZOOM_STEP } else { 1. / ZOOM_STEP });
        self.touch(cx);
    }

    /// The arrows turn it, `+` and `-` zoom, `0` and Home start again, Page Up and Page Down turn it further.
    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let m = event.keystroke.modifiers;
        if m.control || m.alt || m.platform || self.mesh().is_none() {
            return;
        }
        match event.keystroke.key.as_str() {
            "left" => self.camera = self.camera.turned(KEY_TURN, 0.),
            "right" => self.camera = self.camera.turned(-KEY_TURN, 0.),
            "up" => self.camera = self.camera.turned(0., -KEY_TURN),
            "down" => self.camera = self.camera.turned(0., KEY_TURN),
            "+" | "=" | "plus" => self.camera = self.camera.zoomed(ZOOM_STEP),
            "-" | "minus" => self.camera = self.camera.zoomed(1. / ZOOM_STEP),
            "0" | "home" => self.camera = Camera::home(),
            "pageup" => return self.key_page(false, cx),
            "pagedown" => return self.key_page(true, cx),
            _ => return,
        }
        self.touch(cx);
        cx.stop_propagation();
    }

    fn key_page(&mut self, down: bool, cx: &mut Context<Self>) {
        self.page(down, cx);
        cx.stop_propagation();
    }
}

impl Drop for ModelView {
    fn drop(&mut self) {
        self.retire();
    }
}

impl Focusable for ModelView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The model's colour: the theme's muted foreground with a touch of the accent, lit from the camera.
fn model_color(cx: &App) -> [u8; 3] {
    let t = theme::tokens(cx);
    let base = gpui_kit::Rgba::from(t.muted_foreground);
    let accent = gpui_kit::Rgba::from(t.primary);
    let mix = |a: f32, b: f32| ((a * 0.72 + b * 0.28) * 255.).round().clamp(0., 255.) as u8;
    [mix(base.r, accent.r), mix(base.g, accent.g), mix(base.b, accent.b)]
}

impl Render for ModelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_retired(window);
        let t = theme::tokens(cx).clone();
        let root = col().id("model-view").size_full().min_h_0().text_start();
        let triangles = match &self.load {
            Load::Reading => {
                return root.child(waiting(self.started, Some(t!("Drawing it…")), cx));
            }
            Load::Failed(why) => return root.child(big_icon(&self.entry.path, Some(why.clone()), cx)),
            Load::Ready(ready) => ready.triangles,
        };
        self.want(window, cx);

        let view = cx.entity();
        let id = view.entity_id();
        let area_cell = self.area.clone();
        let measuring = canvas(
            move |bounds, window, _| {
                if area_cell.get() != bounds.size {
                    area_cell.set(bounds.size);
                    // Not `cx.notify` here: GPUI drops a redraw asked for during a draw. After this frame, it counts.
                    window.on_next_frame(move |_, cx| cx.notify(id));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let picture = self.shown.as_ref().map(|shown| {
            img(shown.image.clone())
                .object_fit(ObjectFit::Fill)
                .absolute()
                .size_full()
        });

        let size = self.mesh().map(|m| size_text(m.size())).unwrap_or_default();
        let facts = t!(
            "{n} triangles · {size} · drag to turn, wheel to zoom",
            n = format_count(triangles),
            size = size
        );
        let name = self.entry.name.clone();
        let surface = div()
            .id("model-canvas")
            .role(Role::Image)
            .aria_label(t!("3D model: {name}", name = name))
            .aria_description(t!(
                "Drag to turn, wheel to zoom, right-drag to move, double-click to start again. Arrow keys turn it, plus and minus zoom, zero starts again."
            ))
            .test_support()
            .track_focus(&self.focus)
            .tab_stop(true)
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .cursor_grab()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)))
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                this.wheel(event, cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| this.pointer_down(event, false, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| this.pointer_down(event, true, window, cx)),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.pointer_moved(event, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.pointer_up(cx)))
            .on_mouse_up(MouseButton::Right, cx.listener(|this, _, _, cx| this.pointer_up(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.pointer_up(cx)))
            .on_mouse_up_out(MouseButton::Right, cx.listener(|this, _, _, cx| this.pointer_up(cx)))
            .child(measuring)
            .children(picture)
            .child(
                div()
                    .id("model-facts")
                    .role(Role::Status)
                    .aria_label(facts.clone())
                    .test_support()
                    .absolute()
                    .bottom(space(2.))
                    .left_0()
                    .right_0()
                    .px(space(2.))
                    .text_center()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(facts),
            );
        root.on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
            this.page(true, cx);
        }))
        .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
            this.page(false, cx);
        }))
        .child(surface)
    }
}

#[cfg(test)]
mod tests {
    use super::model3d_load::tests::{cube_ascii_stl, cube_binary_stl, cube_obj};
    use super::*;
    use crate::explorer::fake::file;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{point, ScrollDelta, TestAppContext, VisualTestContext};
    use std::collections::HashMap;

    fn source_of(files: Vec<(&str, Vec<u8>)>) -> ModelSource {
        let files: HashMap<String, Vec<u8>> = files.into_iter().map(|(p, b)| (p.to_string(), b)).collect();
        Arc::new(move |path| {
            files
                .get(path)
                .cloned()
                .ok_or_else(|| LoadError::Unreadable(format!("no {path}")))
        })
    }

    fn open(cx: &mut TestAppContext, entry: FileEntry, source: ModelSource) -> (Entity<ModelView>, VisualTestContext) {
        cx.update(testing::init);
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| ModelView::with_source(entry, source, window, cx))
        });
        (view, visual)
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..600 {
            cx.run_until_parked();
            cx.update(|window, cx| window.render_frame(cx));
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("it never settled");
    }

    /// Until a full picture is on screen.
    fn drawn(view: &Entity<ModelView>, visual: &mut VisualTestContext) {
        settle(visual, |cx| view.read_with(cx, |v, _| v.is_full()));
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn pixel(view: &Entity<ModelView>, cx: &VisualTestContext, x: u32, y: u32) -> [u8; 4] {
        view.read_with(cx, |v, _| {
            let (image, (w, _)) = v.picture().expect("a picture");
            let bytes = image.as_bytes(0).expect("pixels");
            let at = ((y * w + x) * 4) as usize;
            [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
        })
    }

    fn middle(view: &Entity<ModelView>, cx: &VisualTestContext) -> [u8; 4] {
        let (w, h) = view.read_with(cx, |v, _| v.picture().expect("a picture").1);
        pixel(view, cx, w / 2, h / 2)
    }

    fn canvas_middle(visual: &mut VisualTestContext) -> Point<Pixels> {
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.find("model-canvas").bounds().center()
        })
    }

    fn arabic_cube() -> (FileEntry, ModelSource) {
        let bytes = cube_binary_stl();
        let entry = file("C:\\مجسمات", "مكعب.stl", bytes.len() as u64, 1);
        let source = source_of(vec![("C:\\مجسمات\\مكعب.stl", bytes)]);
        (entry, source)
    }

    #[gpui_kit::test]
    fn a_model_is_read_and_drawn_off_the_ui_thread_and_says_what_it_is(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let (entry, source) = arabic_cube();
        let (view, mut visual) = open(cx, entry, source);
        drawn(&view, &mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.triangles()), Some(12));
        assert!(
            has(&mut visual, "3D model: مكعب.stl"),
            "named with the Arabic file name"
        );
        assert!(has(
            &mut visual,
            "12 triangles · 2 × 2 × 2 · drag to turn, wheel to zoom"
        ));
        // The picture is as big as the pane in device pixels, the cube is in its middle and the corners show the background.
        let (w, h) = view.read_with(&visual, |v, _| v.picture().expect("a picture").1);
        let (area, scale) =
            visual.update(|window, _| (window.find("model-canvas").bounds().size, window.scale_factor()));
        assert_eq!(w, (f32::from(area.width) * scale).round() as u32);
        assert_eq!(h, (f32::from(area.height) * scale).round() as u32);
        assert_eq!(middle(&view, &visual)[3], 255, "the cube is in the middle");
        assert_eq!(pixel(&view, &visual, 0, 0)[3], 0, "and the corner is clear");
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn every_format_the_pane_chooses_this_view_for_is_read(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        for (name, bytes) in [
            ("a.stl", cube_ascii_stl().into_bytes()),
            ("a.obj", cube_obj().into_bytes()),
        ] {
            let entry = file("C:\\m", name, bytes.len() as u64, 1);
            let source = source_of(vec![(&format!("C:\\m\\{name}"), bytes)]);
            let (view, mut visual) = open(cx, entry, source);
            drawn(&view, &mut visual);
            assert_eq!(view.read_with(&visual, |v, _| v.triangles()), Some(12), "{name}");
        }
    }

    #[gpui_kit::test]
    fn the_wheel_zooms_with_a_draft_first_and_the_full_picture_when_it_is_still(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (entry, source) = arabic_cube();
        let (view, mut visual) = open(cx, entry, source);
        drawn(&view, &mut visual);
        let full = view.read_with(&visual, |v, _| v.picture().expect("a picture").1);
        let before = view.read_with(&visual, |v, _| v.draws());

        visual.update(|window, cx| window.scroll("model-canvas", ScrollDelta::Lines(point(0., 1.)), cx));
        assert!(view.read_with(&visual, |v, _| v.camera().zoom > 1.));
        // While it is moving the picture is half the size; the full one waits for it to be still.
        settle(&mut visual, |cx| view.read_with(cx, |v, _| v.draws() > before));
        assert!(!view.read_with(&visual, |v, _| v.is_full()));
        let draft = view.read_with(&visual, |v, _| v.picture().expect("a picture").1);
        assert_eq!(draft, ((full.0 / 2).max(1), (full.1 / 2).max(1)));
        visual.executor().advance_clock(SETTLE + Duration::from_millis(50));
        drawn(&view, &mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.picture().expect("a picture").1), full);

        // Wheel down zooms out again, and never past the limits.
        for _ in 0..80 {
            visual.update(|window, cx| window.scroll("model-canvas", ScrollDelta::Lines(point(0., -1.)), cx));
        }
        assert_eq!(
            view.read_with(&visual, |v, _| v.camera().zoom),
            model3d_raster::MIN_ZOOM
        );
        // The pictures that were replaced wait to be let go of the GPU, and the next frame does it.
        visual.update(|window, cx| window.render_frame(cx));
        assert_eq!(retired_count(), 0);
    }

    #[gpui_kit::test]
    fn dragging_turns_the_model_and_the_picture_changes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (entry, source) = arabic_cube();
        let (view, mut visual) = open(cx, entry, source);
        drawn(&view, &mut visual);
        let camera = view.read_with(&visual, |v, _| v.camera());
        let first = view.read_with(&visual, |v, _| {
            v.picture().expect("a picture").0.as_bytes(0).map(<[u8]>::to_vec)
        });
        let from = canvas_middle(&mut visual);

        // Dragging right turns the model to the right: the camera's yaw goes down. Down tips its top toward us.
        visual.update(|window, cx| window.drag(from, point(from.x + px(120.), from.y + px(40.)), cx));
        let turned = view.read_with(&visual, |v, _| v.camera());
        assert!(turned.yaw < camera.yaw, "{turned:?} against {camera:?}");
        assert!(turned.pitch > camera.pitch);
        // Letting go draws the full picture at once, and it is another picture.
        drawn(&view, &mut visual);
        let second = view.read_with(&visual, |v, _| {
            v.picture().expect("a picture").0.as_bytes(0).map(<[u8]>::to_vec)
        });
        assert_ne!(first, second);
        assert!(view.read_with(&visual, |v, _| v.is_full()));
        assert_eq!(middle(&view, &visual)[3], 255);
    }

    #[gpui_kit::test]
    fn a_double_click_starts_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (entry, source) = arabic_cube();
        let (view, mut visual) = open(cx, entry, source);
        drawn(&view, &mut visual);
        visual.update(|window, cx| window.scroll("model-canvas", ScrollDelta::Lines(point(0., 3.)), cx));
        let from = canvas_middle(&mut visual);
        visual.update(|window, cx| window.drag(from, point(from.x + px(80.), from.y), cx));
        assert_ne!(view.read_with(&visual, |v, _| v.camera()), Camera::home());
        visual.update(|window, cx| window.double_click("model-canvas", cx));
        assert_eq!(view.read_with(&visual, |v, _| v.camera()), Camera::home());
        visual.executor().advance_clock(SETTLE + Duration::from_millis(50));
        drawn(&view, &mut visual);
    }

    #[gpui_kit::test]
    fn the_keys_turn_zoom_and_reset_it_without_the_mouse(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (entry, source) = arabic_cube();
        let (view, mut visual) = open(cx, entry, source);
        drawn(&view, &mut visual);
        let focus = view.read_with(&visual, |v, _| v.focus.clone());
        visual.update(|window, cx| window.focus(&focus, cx));
        let home = view.read_with(&visual, |v, _| v.camera());
        let press = |visual: &mut VisualTestContext, key: &str| {
            visual.update(|window, cx| window.press(key, cx));
            view.read_with(visual, |v, _| v.camera())
        };
        assert!(press(&mut visual, "left").yaw > home.yaw);
        assert!(press(&mut visual, "right").yaw <= home.yaw + 1e-5);
        assert!(press(&mut visual, "up").pitch < home.pitch);
        assert!(press(&mut visual, "down").pitch >= home.pitch - 1e-5);
        assert!(press(&mut visual, "=").zoom > 1.);
        assert_eq!(press(&mut visual, "0"), Camera::home());
        assert!(press(&mut visual, "-").zoom < 1.);
        assert_eq!(press(&mut visual, "home"), Camera::home());
        // A key it doesn't use changes nothing.
        assert_eq!(press(&mut visual, "x"), Camera::home());
        // Quick Look's page keys turn it an eighth of a turn.
        let yaw = view.update(&mut visual, |v, cx| {
            assert!(v.page(true, cx));
            v.camera().yaw
        });
        assert!((yaw - Camera::home().turned(-std::f32::consts::FRAC_PI_4, 0.).yaw).abs() < 1e-5);
        visual.executor().advance_clock(SETTLE + Duration::from_millis(50));
        drawn(&view, &mut visual);
    }

    #[gpui_kit::test]
    fn a_model_too_big_to_draw_says_so_without_reading_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut entry = file("C:\\m", "huge.stl", 1, 1);
        entry.size = MAX_BYTES + 1;
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = asked.clone();
        let source: ModelSource = Arc::new(move |_| {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Vec::new())
        });
        let (view, mut visual) = open(cx, entry, source);
        visual.run_until_parked();
        assert!(has(&mut visual, "It's too big to draw here"));
        assert_eq!(
            view.read_with(&visual, |v, _| v.failure().map(str::to_string))
                .as_deref(),
            Some("It's too big to draw here")
        );
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[gpui_kit::test]
    fn what_cannot_be_read_or_drawn_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let failing = |name: &str, bytes: Vec<u8>, why: &str, cx: &mut TestAppContext| {
            let entry = file("C:\\m", name, bytes.len() as u64, 1);
            let source = source_of(vec![(&format!("C:\\m\\{name}"), bytes)]);
            let (view, mut visual) = open(cx, entry, source);
            settle(&mut visual, |cx| view.read_with(cx, |v, _| v.failure().is_some()));
            assert!(has(&mut visual, why), "{name}: {why}");
        };
        failing(
            "junk.stl",
            b"hello there, I am not a model".to_vec(),
            "This isn't a model Glint can read",
            cx,
        );
        failing(
            "nothing.obj",
            b"# no faces\nv 1 2 3\n".to_vec(),
            "There is nothing in this file to draw",
            cx,
        );
        failing("مفقود.ply", Vec::new(), "This isn't a model Glint can read", cx);
        failing(
            "packed.gltf",
            br#"{"asset":{"version":"2.0"},"extensionsRequired":["KHR_draco_mesh_compression"]}"#.to_vec(),
            "This model is compressed in a way Glint can't draw yet",
            cx,
        );
        // A file that isn't there.
        let entry = file("C:\\m", "gone.3mf", 10, 1);
        let (view, mut visual) = open(cx, entry, source_of(vec![]));
        settle(&mut visual, |cx| view.read_with(cx, |v, _| v.failure().is_some()));
        assert!(has(&mut visual, "Couldn't read this file"));
    }

    #[gpui_kit::test]
    fn a_file_on_disk_with_an_arabic_name_is_read_through_the_file_system(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let folder = std::env::temp_dir().join(format!("glint-model3d-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("a folder");
        let path = folder.join("نموذج ثلاثي.stl");
        std::fs::write(&path, cube_binary_stl()).expect("a file");
        let path_text = path.to_string_lossy().to_string();
        let entry = file(&folder.to_string_lossy(), "نموذج ثلاثي.stl", 684, 1);
        assert_eq!(entry.path, path_text);
        let (view, mut visual) = open(cx, entry, fs_source());
        drawn(&view, &mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.triangles()), Some(12));
        assert!(has(&mut visual, "3D model: نموذج ثلاثي.stl"));
        std::fs::remove_dir_all(&folder).ok();
        testing::set_rtl(false);
    }

    #[test]
    fn a_models_numbers_are_short_and_the_sources_refuse_big_files() {
        assert_eq!(short(2.), "2");
        assert_eq!(short(12.5), "12.5");
        assert_eq!(short(0.125), "0.125");
        assert_eq!(short(1234.4), "1234");
        assert_eq!(short(0.), "0");
        assert_eq!(size_text([2., 2.5, 100.]), "2 × 2.5 × 100");
        let missing = fs_source()("C:\\this\\does\\not\\exist.stl");
        assert!(matches!(missing, Err(LoadError::Unreadable(_))));
    }

    #[test]
    fn the_pane_is_drawn_at_its_device_size_and_never_past_the_pixel_limit() {
        use gpui_kit::size;
        assert_eq!(ModelView::pixels(size(px(400.), px(300.)), 1.25), (500, 375));
        assert_eq!(ModelView::pixels(size(px(0.), px(300.)), 1.), (0, 300));
        let (w, h) = ModelView::pixels(size(px(4000.), px(3000.)), 2.);
        assert!((w as f32) * (h as f32) <= MAX_PIXELS * 1.001);
        assert!((w as f32 / h as f32 - 4. / 3.).abs() < 0.01);
    }
}
