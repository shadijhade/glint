//! The stage: how a pane's view gives way to the next.
//!
//! The zoom (Ctrl and the wheel, a pinch) changes how a folder is laid out, never which folder it is: from the
//! list a notch in is the grid at its smallest tiles, then bigger tiles; a notch out is the way back. Between
//! the list and the grid every item flies from where the one drew it to where the other does (its picture out
//! of its row onto its tile, and back), so the eye keeps hold of each file. The tiles' sizes grow into each
//! other about the pointer.
//!
//! Everything else is quiet. Opening a folder is a small step toward it (the view grows a little about the
//! folder's row or tile as the folder's files come up), going up a small step back, and another tab or a search
//! comes forward as the old view gives way where it is.
//!
//! **How.** Nothing is laid out twice and nothing is drawn to a picture. The view that leaves is what the last
//! frame painted for it, kept as a list ([`Window::snapshot_painted`]) and painted again on its way out; the view
//! that arrives is laid out once, where it will rest, and takes the pointer and the keys from its first frame.
//! Both are drawn under a paint transform ([`Window::with_paint_transform`], Glint's addition to the vendored
//! GPUI): the GPU places and sizes their quads and glyphs, at the cost of drawing them. An item on its way
//! between the list and the grid is the arriving view's own, painted under a transform of its own ([`flown`]).
//!
//! **Who says what.** The workspace sees what each pane shows change and gives its stage a [`Cue`] with a
//! [`Plan`]; the stage works the plan out against its own bounds when it paints ([`Plan::flight`]) and flies it.
//! Under reduced motion (Windows' "Animation effects" switch, and tests) a stage is its child and nothing more.

use crate::kit::prelude::*;
use crate::theme;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How much a view grows as its pane goes into a folder (and shrinks coming out): a step, not a flight.
const STEP: f32 = 1.06;
/// How much a view gives when it goes somewhere unrelated, and when there is nowhere to go.
const NUDGE: f32 = 1.035;

/// How many times as long flights take: one, unless a debug build is asked to slow them for a look
/// (`GLINT_SLOW_MOTION=8`).
fn slowed() -> f32 {
    static BY: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *BY.get_or_init(|| {
        std::env::var("GLINT_SLOW_MOTION")
            .ok()
            .filter(|_| cfg!(debug_assertions))
            .and_then(|by| by.parse::<f32>().ok())
            .filter(|by| by.is_finite() && *by >= 1.)
            .unwrap_or(1.)
    })
}

/// Where the camera goes as one view gives way to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flight {
    /// How many times larger the view that leaves is drawn by the end: above one flies in, below one out.
    pub scale: f32,
    /// A point of the view that leaves (window pixels)…
    pub from: Point<Pixels>,
    /// …and the point of the view that arrives it ends on.
    pub to: Point<Pixels>,
    /// How long it takes.
    pub duration: Duration,
    /// The part of the flight (0 to 1) by which the view that leaves has cleared.
    pub gone_by: f32,
    /// The part of the flight by which the view that arrives is wholly there.
    pub here_by: f32,
    /// How much of the view that arrives is there at the start (it is readable from its first frame).
    pub dim: f32,
    /// There and back: the view gives by `scale` and returns, and nothing leaves (nowhere to go).
    pub bump: bool,
}

/// A moment of a flight: how each view is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moment {
    /// The view that leaves: where, and how opaque. None once it has cleared (and in a bump).
    pub leaving: Option<(PaintTransform, f32)>,
    /// The view that arrives.
    pub arriving: (PaintTransform, f32),
}

/// `t` (0 to 1) eased in and out: no speed at either end.
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

impl Flight {
    /// The flight at `t`, the part of its time gone (0 to 1; later is its end).
    pub fn at(&self, t: f32) -> Moment {
        let t = t.clamp(0., 1.);
        if self.bump {
            // Out to the limit by the middle, back by the end.
            let give = (t * std::f32::consts::PI).sin().max(0.);
            let scale = 1. + (self.scale - 1.) * give;
            return Moment {
                leaving: None,
                arriving: (PaintTransform::scale_about(scale, self.from), 1.),
            };
        }
        let eased = theme::motion::glide(t);
        // The scale grows by equal ratios in equal times, as a zoom does; the place moves with it so that one
        // point of the window stays still throughout.
        let (scale, along) = if (self.scale - 1.).abs() < 1e-3 {
            (1., eased)
        } else {
            let scale = self.scale.powf(eased);
            (scale, (scale - 1.) / (self.scale - 1.))
        };
        let at = point(
            self.from.x + (self.to.x - self.from.x) * along,
            self.from.y + (self.to.y - self.from.y) * along,
        );
        let placed = |scale: f32, anchor: Point<Pixels>| PaintTransform {
            scale,
            translate: point(at.x - anchor.x * scale, at.y - anchor.y * scale),
        };
        let leaving = 1. - smooth(t / self.gone_by.max(0.01));
        let arriving = self.dim + (1. - self.dim) * smooth(t / self.here_by.max(0.01));
        Moment {
            leaving: (leaving > 0.004).then(|| (placed(scale, self.from), leaving)),
            arriving: (placed(scale / self.scale, self.to), arriving),
        }
    }
}

/// Where the row or tile of something in a view is drawn, asked once the view has been laid out.
pub type Spot = Rc<dyn Fn(&Window, &App) -> Option<Bounds<Pixels>>>;

/// What a pane's view is giving way to, as the workspace knows it.
#[derive(Clone)]
pub enum Plan {
    /// Into a folder (or an archive, or a drive): its row or tile was drawn here in the view that leaves. None
    /// when it wasn't on screen: the step is about the middle.
    Dive(Option<Bounds<Pixels>>),
    /// Out to a folder above: the spot says where the folder left is drawn there.
    Rise(Option<Spot>),
    /// Somewhere unrelated, or the same place seen another way: the old view gives way where it is.
    Swap,
    /// The same items laid out another way (the list for the grid): the old view clears where it is while the
    /// new one's items fly to their places from where the old one drew them (the view flies them: [`Morph`]).
    Morph,
    /// The same things `ratio` times larger (smaller, under one), about the pointer if it is over them.
    Resize { ratio: f32, pivot: Option<Point<Pixels>> },
    /// Nowhere to go: the view gives a little (`inward`: toward the eye, as if to go in) and comes back.
    Bump { inward: bool, pivot: Option<Point<Pixels>> },
}

impl Plan {
    /// The plan's name, for the frame log.
    pub fn name(&self) -> &'static str {
        match self {
            Plan::Dive(Some(_)) => "dive",
            Plan::Dive(None) => "dive from the middle",
            Plan::Rise(_) => "rise",
            Plan::Swap => "swap",
            Plan::Morph => "morph",
            Plan::Resize { .. } => "resize",
            Plan::Bump { .. } => "bump",
        }
    }

    /// Whether the view that was there is shown on its way out.
    fn leaves(&self) -> bool {
        !matches!(self, Plan::Bump { .. })
    }

    /// The flight this plan asks for on a stage drawn at `stage`. Called while the stage paints: the view that
    /// arrives has been laid out, so a [`Plan::Rise`] can ask where the folder left is in it.
    pub fn flight(&self, stage: Bounds<Pixels>, window: &Window, cx: &App) -> Flight {
        let middle = stage.center();
        // A spot counts for as much of it as is on the stage.
        let on_stage = |spot: Bounds<Pixels>| Some(spot.intersect(&stage)).filter(|spot| !spot.is_empty());
        let inside = |at: Point<Pixels>| Some(at).filter(|at| stage.contains(at));
        match self {
            Plan::Dive(spot) => {
                // About the folder gone into: the step is toward it.
                let about = spot.and_then(on_stage).map_or(middle, |spot| spot.center());
                Flight {
                    scale: STEP,
                    from: about,
                    to: about,
                    duration: theme::motion::DIVE,
                    gone_by: 0.6,
                    here_by: 0.7,
                    dim: 0.35,
                    bump: false,
                }
            }
            Plan::Rise(spot) => {
                // About the folder left, where the view that arrives draws it.
                let spot = spot.as_ref().and_then(|spot| spot(window, cx)).and_then(on_stage);
                let about = spot.map_or(middle, |spot| spot.center());
                Flight {
                    scale: 1. / STEP,
                    from: about,
                    to: about,
                    duration: theme::motion::DIVE,
                    gone_by: 0.6,
                    here_by: 0.7,
                    dim: 0.35,
                    bump: false,
                }
            }
            // Neither view moves: the old one clears over the new one, whose items are on their way.
            Plan::Morph => Flight {
                scale: 1.,
                from: middle,
                to: middle,
                duration: theme::motion::MORPH,
                // The old view is out of the way before the items are half way: they are what is watched.
                gone_by: 0.4,
                here_by: 0.01,
                dim: 1.,
                bump: false,
            },
            Plan::Swap => Flight {
                scale: NUDGE,
                from: middle,
                to: middle,
                duration: theme::motion::BASE,
                gone_by: 0.6,
                here_by: 1.,
                dim: 0.35,
                bump: false,
            },
            Plan::Resize { ratio, pivot } => {
                let about = pivot.and_then(inside).unwrap_or(middle);
                Flight {
                    scale: ratio.clamp(0.25, 4.),
                    from: about,
                    to: about,
                    duration: theme::motion::BASE + theme::motion::BASE / 4,
                    // The two sizes are on screen together for as short a time as reads as one growing.
                    gone_by: 0.5,
                    here_by: 0.6,
                    dim: 0.2,
                    bump: false,
                }
            }
            Plan::Bump { inward, pivot } => {
                let about = pivot.and_then(inside).unwrap_or(middle);
                Flight {
                    scale: if *inward { NUDGE } else { 1. / NUDGE },
                    from: about,
                    to: about,
                    duration: theme::motion::SLOW,
                    gone_by: 0.,
                    here_by: 0.,
                    dim: 1.,
                    bump: true,
                }
            }
        }
    }
}

/// What a stage is told when its pane goes somewhere: a plan, and a number that is new each time. A stage that
/// sees a number it has not seen flies the plan; the same cue given again (every frame until the next) does
/// nothing.
#[derive(Clone)]
pub struct Cue {
    /// Counts the pane's cues.
    pub serial: u64,
    /// How the view gives way.
    pub plan: Plan,
}

/// The flight under way.
struct Flying {
    flight: Flight,
    began: Instant,
    /// What the view that leaves painted in its last frame.
    ghost: Option<PaintSnapshot>,
}

/// What a stage keeps from frame to frame. Whoever draws the stage holds it ([`Stage`] is made anew each frame):
/// not GPUI's element state, which is kept by an element's place in the tree, and a pane's place changes (the
/// preview pane beside it comes and goes), which would forget the view that is leaving just as it leaves.
#[derive(Clone, Default)]
pub struct Stagehand(Rc<std::cell::RefCell<Kept>>);

#[derive(Default)]
struct Kept {
    /// The cue acted on last, if any.
    serial: Option<u64>,
    /// What the view painted in the last frame (its own part: not what was leaving then).
    painted: Option<Range<PaintPosition>>,
    flying: Option<Flying>,
}

/// A pane's view on its stage. See the module.
pub struct Stage {
    hand: Stagehand,
    cue: Option<Cue>,
    child: AnyElement,
}

/// `child` (which fills the stage) on a stage that flies the cues it is given. `hand` is the stage's own, kept by
/// whoever draws it: it holds what the stage remembers between frames.
pub fn stage(hand: &Stagehand, cue: Option<Cue>, child: impl IntoElement) -> Stage {
    Stage {
        hand: hand.clone(),
        cue,
        child: child.into_any_element(),
    }
}

impl IntoElement for Stage {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Stage {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // The stage is its child's box.
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let now = cx.background_executor().now();
        let still = cx.reduce_motion();
        // Taken out while the child paints (a stage inside this one has its own), put back at the end.
        let mut kept = self.hand.0.take();
        if let Some(cue) = self.cue.take() {
            let last = kept.serial.replace(cue.serial);
            // A cue not seen before, for a stage that has shown something to leave (not its first frame).
            if last != Some(cue.serial) && kept.painted.is_some() && !still {
                // The view that was here leaves as the last frame painted it.
                let ghost = cue
                    .plan
                    .leaves()
                    .then(|| kept.painted.clone())
                    .flatten()
                    .and_then(|painted| window.snapshot_painted(painted))
                    .filter(|ghost| !ghost.is_empty());
                let flight = cue.plan.flight(bounds, window, cx);
                crate::perf::say(|| {
                    format!(
                        "flight scale={:.2} from={:?} to={:?} ghost={} bump={}",
                        flight.scale,
                        flight.from,
                        flight.to,
                        ghost.is_some(),
                        flight.bump
                    )
                });
                kept.flying = Some(Flying {
                    flight,
                    began: now,
                    ghost,
                });
            }
        }
        if still {
            kept.flying = None;
        }
        // Where the flight is now, and whether this is its last frame.
        let moment = kept.flying.as_ref().map(|flying| {
            let gone = now.saturating_duration_since(flying.began).as_secs_f32();
            let t = gone / (flying.flight.duration.as_secs_f32() * slowed()).max(0.001);
            (flying.flight.at(t), t >= 1.)
        });
        let from = window.paint_position();
        let mut to = None;
        match moment {
            None => self.child.paint(window, cx),
            Some((moment, _)) => {
                let ghost = kept.flying.as_ref().and_then(|flying| flying.ghost.clone());
                let child = &mut self.child;
                // Neither view is drawn outside the stage, however large it grows.
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let (transform, opacity) = moment.arriving;
                    window.with_paint_transform(Some(transform), |window| {
                        window.with_element_opacity(Some(opacity), |window| child.paint(window, cx))
                    });
                    to = Some(window.paint_position());
                    // Over it, what leaves: the folder's files come out from under their tile.
                    if let (Some(ghost), Some((transform, opacity))) = (ghost, moment.leaving) {
                        window.with_paint_transform(Some(transform), |window| window.paint_snapshot(&ghost, opacity));
                    }
                });
            }
        }
        // What leaves next is the view itself, not what was leaving with it.
        kept.painted = Some(from..to.unwrap_or_else(|| window.paint_position()));
        match moment {
            Some((_, true)) => kept.flying = None,
            Some(_) => window.request_animation_frame(),
            None => {}
        }
        self.hand.0.replace(kept);
    }
}

/// Where a pane's views draw the row or tile of a path (the view on screen answers), for whoever opens
/// something out of an item: Quick Look grows out of the file it shows.
pub type Spotter = Rc<dyn Fn(&str, &Window, &App) -> Option<Bounds<Pixels>>>;

/// What a view that takes over from another is asked to show in the same place, so the zoom stays on what it
/// was over: an item, and how far below the top of the view's rows the pointer was on it (pixels; its middle
/// goes there). With no pointer on an item, the first one the old view showed: it goes at the top.
pub type Anchor = (String, Option<f32>);

/// Where a view drew the picture of a path's item, on screen or not.
type Pictures = Rc<dyn Fn(&str, &App) -> Option<Bounds<Pixels>>>;

/// The items of a view on their way from where another view drew them: the list has given way to the grid (or
/// the grid to the list), and each item's picture flies from its row to its tile. The view that arrives keeps
/// one while it lasts and draws its items through [`flown`].
#[derive(Clone)]
pub struct Morph {
    /// Where the view that left drew an item's picture (off screen too: where it would have been).
    from: Pictures,
    began: Instant,
    /// What `from` said, by path: the view that left doesn't change, and asking it reads its rows.
    asked: Rc<std::cell::RefCell<std::collections::HashMap<String, Option<Bounds<Pixels>>>>>,
}

impl Morph {
    /// A morph that begins at `now`, from where `from` says each item was.
    pub fn new(from: impl Fn(&str, &App) -> Option<Bounds<Pixels>> + 'static, now: Instant) -> Self {
        Self {
            from: Rc::new(from),
            began: now,
            asked: Rc::default(),
        }
    }

    /// How far along it is at `now` (0 to 1, eased), or None once it is over.
    pub fn progress(&self, now: Instant) -> Option<f32> {
        let gone = now.saturating_duration_since(self.began).as_secs_f32();
        let t = gone / (theme::motion::MORPH.as_secs_f32() * slowed()).max(0.001);
        (t < 1.).then(|| theme::motion::glide(t))
    }

    /// Where the view that left drew the picture of `path`.
    pub fn from(&self, path: &str, cx: &App) -> Option<Bounds<Pixels>> {
        if let Some(known) = self.asked.borrow().get(path) {
            return *known;
        }
        let spot = (self.from)(path, cx);
        self.asked.borrow_mut().insert(path.to_string(), spot);
        spot
    }
}

/// An element painted on its way to where it is laid out. See [`flown`].
pub struct Flown {
    child: AnyElement,
    from: Bounds<Pixels>,
    picture: Option<(Pixels, Pixels)>,
    progress: f32,
}

/// `child` painted on its way from `from` to its place: at `progress` 0 its picture is drawn over `from` (as
/// small, or as large), at 1 the child is where it is laid out, as it is. The picture is the square `picture`
/// says (how far below the child's top it starts, and its side; centered across), or the whole child. Paint
/// only: the child is laid out once, where it rests, and is clicked there throughout.
pub fn flown(child: impl IntoElement, from: Bounds<Pixels>, picture: Option<(Pixels, Pixels)>, progress: f32) -> Flown {
    Flown {
        child: child.into_any_element(),
        from,
        picture,
        progress,
    }
}

impl Flown {
    /// The transform a child laid out at `bounds` is painted under. Nothing comes from further off than
    /// `reach`: an item far down the list arrives from the edge, not from a mile below it.
    fn transform(&self, bounds: Bounds<Pixels>, reach: Pixels) -> PaintTransform {
        let picture = self.picture.map_or(bounds, |(top, side)| {
            Bounds::new(
                point(bounds.center().x - side / 2., bounds.top() + top),
                size(side, side),
            )
        });
        let left = 1. - self.progress.clamp(0., 1.);
        let (to, from) = (picture.center(), self.from.center());
        let off = |from: Pixels, to: Pixels| (from - to).clamp(-reach, reach) * left;
        let small = (self.from.size.height / picture.size.height.max(px(1.))).clamp(0.05, 20.);
        // The size goes by equal ratios in equal times, as a zoom does.
        let scale = small.powf(left);
        let at = point(to.x + off(from.x, to.x), to.y + off(from.y, to.y));
        PaintTransform {
            scale,
            translate: point(at.x - to.x * scale, at.y - to.y * scale),
        }
    }
}

impl IntoElement for Flown {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Flown {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // It is its child's box.
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let transform = self.transform(bounds, window.viewport_size().height);
        let child = &mut self.child;
        window.with_paint_transform(Some(transform), |window| child.paint(window, cx));
    }
}

/// Each pane's spotter, once its views are made.
#[derive(Default)]
struct Spotters([Option<Spotter>; 2]);

impl Global for Spotters {}

/// Pane `pane`'s views are made: `spotter` says where they draw things.
pub fn set_spotter(cx: &mut App, pane: usize, spotter: Spotter) {
    if let Some(slot) = cx.default_global::<Spotters>().0.get_mut(pane) {
        *slot = Some(spotter);
    }
}

/// Where pane `pane` draws the row or tile of `path` now, if it shows it.
pub fn spot_of(pane: usize, path: &str, window: &Window, cx: &App) -> Option<Bounds<Pixels>> {
    let spotter = cx.try_global::<Spotters>()?.0.get(pane)?.clone()?;
    spotter(path, window, cx)
}

/// The wheel's turning for one notch of zoom: a mouse wheel's notch. Touchpads and fine-grained wheels add up
/// to it.
pub const NOTCH: f32 = 100.;
/// A pause longer than this ends a spin of the wheel: the count starts over.
const SPIN_ENDS_AFTER: Duration = Duration::from_millis(400);
/// How far apart the fingers go for a pinch to count as a notch: half as far again.
const PINCH_NOTCH: f32 = 1.5;

/// Counts the wheel's turning (with Ctrl) and a pinch's spreading into notches of zoom: from the list to the
/// grid's smallest tiles, on to its biggest, and back. Where the zoom ends (the list going out, the biggest
/// tiles going in) the stage gives a little, once a spin.
#[derive(Default)]
pub struct Notches {
    turned: f32,
    last: Option<(Instant, bool)>,
    /// Whether the spin under way has said that the zoom ends here.
    pub said: bool,
}

/// What a movement of the wheel or the fingers came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turn {
    /// Whole notches: above zero toward bigger and into, below toward smaller and out.
    pub steps: i64,
    /// Whether this movement began a spin: the first after a pause, or the first the other way.
    pub fresh: bool,
}

impl Notches {
    /// A wheel event's turning, in the count's measure: a notch (three lines) is one [`NOTCH`].
    pub fn of_wheel(delta: &ScrollDelta) -> f32 {
        match delta {
            // Multiplied before dividing so that a whole notch is exactly one step.
            ScrollDelta::Lines(lines) => lines.y * NOTCH / 3.,
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y),
        }
    }

    /// A pinch event's spreading (`delta`: 0.1 is a tenth further apart) in the count's measure.
    pub fn of_pinch(delta: f32) -> f32 {
        if !delta.is_finite() || delta <= -0.95 {
            return 0.;
        }
        (1. + delta).ln() / PINCH_NOTCH.ln() * NOTCH
    }

    /// Adds a movement at `now` and takes the whole notches out of the count.
    pub fn turn(&mut self, delta: f32, now: Instant) -> Turn {
        if delta == 0. || !delta.is_finite() {
            return Turn { steps: 0, fresh: false };
        }
        let inward = delta > 0.;
        let fresh = self.last.is_none_or(|(last, was_inward)| {
            now.saturating_duration_since(last) > SPIN_ENDS_AFTER || was_inward != inward
        });
        if fresh {
            self.turned = 0.;
            self.said = false;
        }
        self.last = Some((now, inward));
        self.turned += delta;
        let steps = (self.turned / NOTCH).trunc();
        self.turned -= steps * NOTCH;
        // Whole notches only, and few of them, so the cast is exact.
        Turn {
            steps: steps.clamp(-16., 16.) as i64,
            fresh,
        }
    }
}

/// The step from `above` toward `below`: the folder in `above` that `below` is, or is in. None unless `below`
/// is under `above` (This PC is above every drive and share).
pub fn step_toward(above: &str, below: &str) -> Option<String> {
    use glint_model::paths::{parent_path, same_path, THIS_PC};
    if below == THIS_PC || same_path(above, below) {
        return None;
    }
    let mut step = below.to_string();
    // A path has fewer steps than letters: the walk up ends.
    for _ in 0..below.len() + 1 {
        let parent = parent_path(&step);
        let top = parent == THIS_PC;
        if (top && above == THIS_PC) || (!top && same_path(&parent, above)) {
            return Some(step);
        }
        if top {
            return None;
        }
        step = parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Point<Pixels>, b: Point<Pixels>) -> bool {
        (f32::from(a.x) - f32::from(b.x)).abs() < 0.01 && (f32::from(a.y) - f32::from(b.y)).abs() < 0.01
    }

    fn a_flight(scale: f32) -> Flight {
        Flight {
            scale,
            from: point(px(200.), px(300.)),
            to: point(px(500.), px(400.)),
            duration: Duration::from_millis(260),
            gone_by: 0.5,
            here_by: 0.6,
            dim: 0.25,
            bump: false,
        }
    }

    #[test]
    fn a_flight_starts_with_the_old_view_as_it_was_and_ends_with_the_new_one_as_it_is() {
        for scale in [4., 0.25, 1., 1.035] {
            let flight = a_flight(scale);
            let (start, end) = (flight.at(0.), flight.at(1.));
            // The view that leaves starts where it was, wholly there.
            let (leaving, opacity) = start.leaving.expect("the old view is there at the start");
            assert!(
                leaving.is_identity() && (opacity - 1.).abs() < 1e-3,
                "{leaving:?} {opacity}"
            );
            // The view that arrives ends where it is laid out, wholly there, and the other has cleared.
            assert!(end.arriving.0.is_identity(), "{:?}", end.arriving.0);
            assert!((end.arriving.1 - 1.).abs() < 1e-3 && end.leaving.is_none());
            // It is readable from its first frame.
            assert!((start.arriving.1 - 0.25).abs() < 1e-3);
        }
    }

    #[test]
    fn one_camera_moves_both_views() {
        // At every moment the old view's `from` and the new view's `to` are drawn at the same place, and the old
        // view is `scale` times as large as the new one: the tile and the folder are one place.
        for scale in [4., 0.25, 1.] {
            let flight = a_flight(scale);
            for step in 0..=10 {
                let moment = flight.at(step as f32 / 10.);
                let (arriving, _) = moment.arriving;
                if let Some((leaving, _)) = moment.leaving {
                    assert!(near(leaving.apply(flight.from), arriving.apply(flight.to)), "at {step}");
                    assert!((leaving.scale / arriving.scale - scale).abs() < 1e-3, "at {step}");
                }
            }
        }
    }

    #[test]
    fn an_item_flies_from_where_the_old_view_drew_it_to_where_it_is_laid_out() {
        // A tile's cell at (300, 200), 100 wide; its picture a 64 px square 11 px down, centered across. The
        // row's picture it comes from: a 20 px square at (12, 500).
        let cell = Bounds::new(point(px(300.), px(200.)), size(px(100.), px(130.)));
        let picture = Bounds::new(point(px(318.), px(211.)), size(px(64.), px(64.)));
        let from = Bounds::new(point(px(12.), px(500.)), size(px(20.), px(20.)));
        let at = |progress: f32| flown(div(), from, Some((px(11.), px(64.))), progress).transform(cell, px(2000.));
        // At the start the tile's picture is drawn over the row's, as small.
        let start = at(0.).apply_bounds(picture);
        assert!(near(start.origin, from.origin), "{start:?}");
        assert!((f32::from(start.size.width) - 20.).abs() < 0.01, "{start:?}");
        // At the end the tile is where it is laid out, as it is.
        assert!(at(1.).is_identity(), "{:?}", at(1.));
        // On the way it is between the two, in place and in size.
        let middle = at(0.5).apply_bounds(picture);
        assert!(middle.size.width > px(20.) && middle.size.width < px(64.));
        assert!(middle.center().x > from.center().x && middle.center().x < picture.center().x);
        // Nothing comes from further off than the reach: a row a mile down the list arrives from the edge.
        let far = Bounds::new(point(px(12.), px(90_000.)), size(px(20.), px(20.)));
        let begun = flown(div(), far, None, 0.)
            .transform(picture, px(800.))
            .apply_bounds(picture);
        assert!(
            (f32::from(begun.center().y) - (f32::from(picture.center().y) + 800.)).abs() < 0.01,
            "{begun:?}"
        );
    }

    #[test]
    fn a_morph_clears_the_old_view_where_it_is_over_the_new_one() {
        let flight = Flight {
            scale: 1.,
            from: point(px(400.), px(300.)),
            to: point(px(400.), px(300.)),
            gone_by: 0.5,
            here_by: 0.01,
            dim: 1.,
            ..a_flight(1.)
        };
        for step in 0..=10 {
            let moment = flight.at(step as f32 / 10.);
            // The new view is in its place and wholly there throughout: its items do the moving.
            assert!(moment.arriving.0.is_identity() && (moment.arriving.1 - 1.).abs() < 1e-3);
            if let Some((leaving, _)) = moment.leaving {
                assert!(leaving.is_identity());
            }
        }
        assert!(flight.at(0.6).leaving.is_none(), "the old view has cleared by then");
        // It is over in its time, and eased.
        let morph = Morph::new(|_, _| None, Instant::now());
        let began = morph.began;
        assert_eq!(morph.progress(began), Some(0.));
        let along = morph.progress(began + theme::motion::MORPH / 2).expect("half way");
        assert!(along > 0.5 && along < 1., "{along}");
        assert_eq!(morph.progress(began + theme::motion::MORPH), None);
    }

    #[test]
    fn a_dive_starts_with_the_folder_small_on_its_tile_and_never_turns_back() {
        let flight = a_flight(4.);
        let start = flight.at(0.).arriving.0;
        // A quarter of its size, its middle on the tile.
        assert!((start.scale - 0.25).abs() < 1e-3);
        assert!(near(start.apply(flight.to), flight.from));
        let mut last = 0.;
        for step in 0..=20 {
            let scale = flight.at(step as f32 / 20.).arriving.0.scale;
            assert!(scale + 1e-4 >= last, "{scale} after {last}");
            last = scale;
        }
    }

    #[test]
    fn a_bump_gives_and_comes_back_with_nothing_leaving() {
        let flight = Flight {
            scale: NUDGE,
            bump: true,
            ..a_flight(1.)
        };
        assert!(flight.at(0.).arriving.0.is_identity() && flight.at(1.).arriving.0.is_identity());
        let middle = flight.at(0.5);
        assert!((middle.arriving.0.scale - NUDGE).abs() < 1e-3 && middle.leaving.is_none());
        // The point it gives about stays put.
        assert!(near(middle.arriving.0.apply(flight.from), flight.from));
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
        Bounds::new(
            point(ScaledPixels(x), ScaledPixels(y)),
            size(ScaledPixels(width), ScaledPixels(height)),
        )
    }

    /// A red quad at `bounds` with rounded corners and a border, clipped by the whole of a big window.
    fn quad(bounds: Bounds<ScaledPixels>) -> Quad {
        Quad {
            bounds,
            content_mask: ContentMask {
                bounds: rect(0., 0., 1000., 1000.),
            },
            background: gpui_kit::red().into(),
            corner_radii: Corners::all(ScaledPixels(8.)),
            border_widths: Edges::all(ScaledPixels(1.)),
            ..Default::default()
        }
    }

    // The vendored GPUI's paint transform (docs/gpui/VENDOR.md), which every flight is drawn with.
    #[test]
    fn a_paint_transform_places_sizes_and_clips_what_is_painted_under_it() {
        let mut scene = Scene::default();
        // Twice the size about (100, 100), and nothing outside the first 400 pixels.
        scene.push_transform(
            2.,
            point(ScaledPixels(-100.), ScaledPixels(-100.)),
            rect(0., 0., 400., 400.),
        );
        scene.insert_primitive(quad(rect(100., 100., 50., 50.)));
        // Inside it, half the size about the same point: where it would be with neither.
        scene.push_transform(
            0.5,
            point(ScaledPixels(50.), ScaledPixels(50.)),
            rect(0., 0., 1000., 1000.),
        );
        scene.insert_primitive(quad(rect(100., 100., 50., 50.)));
        scene.pop_transform();
        // Moved wholly outside the clip, it is not painted at all.
        scene.insert_primitive(quad(rect(300., 300., 50., 50.)));
        scene.pop_transform();
        assert!(!scene.is_transformed());
        scene.insert_primitive(quad(rect(100., 100., 50., 50.)));

        assert_eq!(scene.quads.len(), 3);
        let doubled = &scene.quads[0];
        assert_eq!(doubled.bounds, rect(100., 100., 100., 100.));
        assert_eq!(doubled.corner_radii, Corners::all(ScaledPixels(16.)));
        assert_eq!(doubled.border_widths, Edges::all(ScaledPixels(2.)));
        // Its own clip, transformed, and no more than the clip where the transform began.
        assert_eq!(doubled.content_mask.bounds, rect(0., 0., 400., 400.));
        assert_eq!(scene.quads[1].bounds, rect(100., 100., 50., 50.));
        assert_eq!(scene.quads[1].corner_radii, Corners::all(ScaledPixels(8.)));
        assert_eq!(scene.quads[2].bounds, rect(100., 100., 50., 50.));
        assert_eq!(scene.quads[2].content_mask.bounds, rect(0., 0., 1000., 1000.));
    }

    #[test]
    fn what_a_frame_painted_is_painted_again_under_a_transform_and_fainter() {
        let mut first = Scene::default();
        first.insert_primitive(quad(rect(10., 10., 20., 20.)));
        first.insert_primitive(quad(rect(40., 10., 20., 20.)));
        let snapshot = first.snapshot(0..2).expect("both are in the scene");
        assert!(!snapshot.is_empty() && first.snapshot(1..5).is_none());

        let mut second = Scene::default();
        second.push_transform(
            3.,
            point(ScaledPixels(0.), ScaledPixels(0.)),
            rect(0., 0., 1000., 1000.),
        );
        second.paint_snapshot(&snapshot, 0.5);
        second.pop_transform();
        assert_eq!(second.quads.len(), 2);
        assert_eq!(second.quads[0].bounds, rect(30., 30., 60., 60.));
        assert_eq!(second.quads[1].bounds, rect(120., 30., 60., 60.));
        let alpha = second.quads[0].background.as_solid().map(|color| color.a);
        assert_eq!(alpha, Some(0.5));
        // The scene it came from is as it was.
        assert_eq!(first.quads[0].bounds, rect(10., 10., 20., 20.));
    }

    #[test]
    fn a_transform_then_its_inverse_is_nothing_and_a_move_alone_keeps_the_size() {
        let about = point(px(300.), px(200.));
        let zoom = PaintTransform::scale_about(2.5, about);
        // The point it is about stays put.
        assert!(near(zoom.apply(about), about));
        let back = zoom.then(zoom.inverse().expect("a scale of 2.5 can be undone"));
        assert!(back.is_identity(), "{back:?}");
        assert!(PaintTransform::scale_about(0., about).inverse().is_none());
        let moved = PaintTransform::moved(point(px(12.), px(-4.)));
        assert!(!moved.is_identity() && moved.is_drawable() && moved.scale == 1.);
        assert!(!PaintTransform::scale_about(f32::NAN, about).is_drawable());
    }

    #[test]
    fn the_wheel_counts_whole_notches_and_a_pause_or_a_turn_back_begins_a_spin() {
        let mut notches = Notches::default();
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        // A mouse wheel's notch is three lines: one step, and the first movement begins a spin.
        let notch = Notches::of_wheel(&ScrollDelta::Lines(point(0., 3.)));
        assert_eq!(notches.turn(notch, at(0)), Turn { steps: 1, fresh: true });
        assert_eq!(notches.turn(notch, at(100)), Turn { steps: 1, fresh: false });
        // A fine wheel adds up to a notch.
        assert_eq!(notches.turn(40., at(150)).steps, 0);
        assert_eq!(notches.turn(40., at(200)).steps, 0);
        assert_eq!(notches.turn(40., at(250)).steps, 1);
        // The other way is a spin of its own, and what was left of the last one is dropped.
        assert_eq!(notches.turn(-notch, at(300)), Turn { steps: -1, fresh: true });
        // So is whatever comes after a pause.
        assert_eq!(notches.turn(-notch, at(900)), Turn { steps: -1, fresh: true });
        assert_eq!(notches.turn(0., at(950)), Turn { steps: 0, fresh: false });
    }

    #[test]
    fn a_pinch_half_as_far_again_is_a_notch_either_way() {
        assert!((Notches::of_pinch(0.5) - NOTCH).abs() < 0.01);
        assert!((Notches::of_pinch(1. / 1.5 - 1.) + NOTCH).abs() < 0.01);
        assert_eq!(Notches::of_pinch(0.), 0.);
        assert_eq!(Notches::of_pinch(f32::NAN), 0.);
        assert_eq!(Notches::of_pinch(-1.), 0.);
    }

    #[test]
    fn the_step_toward_a_folder_is_the_folder_of_the_one_above_that_holds_it() {
        assert_eq!(step_toward(r"C:\a", r"C:\a\b\c").as_deref(), Some(r"C:\a\b"));
        assert_eq!(step_toward(r"C:\a", r"C:\a\b").as_deref(), Some(r"C:\a\b"));
        assert_eq!(step_toward(r"c:\A\", r"C:\a\b").as_deref(), Some(r"C:\a\b"));
        assert_eq!(step_toward(r"C:\", r"C:\a\b").as_deref(), Some(r"C:\a"));
        // This PC is above the drives.
        assert_eq!(step_toward("", r"C:\a\b").as_deref(), Some(r"C:\"));
        // Not under it, the same folder, or the other way round: no step.
        assert_eq!(step_toward(r"C:\a", r"C:\b\c"), None);
        assert_eq!(step_toward(r"C:\a", r"C:\a"), None);
        assert_eq!(step_toward(r"C:\a\b", r"C:\a"), None);
        assert_eq!(step_toward(r"C:\a", ""), None);
    }
}
