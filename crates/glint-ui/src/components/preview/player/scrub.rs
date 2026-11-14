//! Scrubbing a video over its tile in the grid: with the pointer on the tile, the video at the point the pointer
//! is at across it (ten frames, read as the pointer arrives), with where that is below (`VideoScrub` of
//! VideoScrub.tsx).
//!
//! glint-core's `video_frames` reads the frames with Media Foundation's source reader, in the BGRA GPUI draws;
//! [`frames_of`] makes each a picture, on a background thread. The frames of the last few videos are kept
//! ([`BUDGET`] bytes of them) so running the pointer over a tile again is at once.
//!
//! **Using it.** A tile that shows a video puts a [`Scrub`] over itself (`cx.new(|cx| Scrub::new(path, width,
//! cx))`, with `width` from [`Scrub::width_for`]): it fills the tile, draws nothing until the pointer is on it
//! and the frames are in, and takes the pointer's moves itself (the tile's own clicks and drags still reach the
//! tile). Frames it drew are let go of the GPU's atlas when the pointer leaves, and by [`super::release_retired`]
//! if the tile goes away first.

use super::engine::clock;
use super::sources::player_sources;
use super::{retire, PlayerSources};
use crate::kit::prelude::*;
use crate::{t, theme};
use gpui_kit::{canvas, img, Bounds, MouseMoveEvent, ObjectFit, Pixels, RenderImage, StyledImage as _, Task};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// How many frames are read across a video (`COUNT`).
pub const COUNT: usize = 10;
/// How many bytes of frames are kept for videos scrubbed before (the TypeScript kept 24 videos).
const BUDGET: usize = 48 << 20;

/// The frames read for a video at a width.
pub struct Frames {
    /// How many pictures.
    pub count: usize,
    /// Each one's size in pixels.
    pub width: u32,
    /// And height.
    pub height: u32,
    /// The video's length in seconds, 0 when it didn't say.
    pub seconds: f64,
    /// The pictures, first to last.
    pub images: Vec<Arc<RenderImage>>,
}

impl Frames {
    /// The picture for the pointer `at` (0 to 1) across the tile: the frame whose tenth it falls in.
    pub fn at(&self, at: f32) -> Option<&Arc<RenderImage>> {
        let index = ((at.clamp(0., 1.) * self.count as f32).floor() as usize).min(self.count.saturating_sub(1));
        self.images.get(index)
    }

    fn cost(&self) -> usize {
        self.count * self.width as usize * self.height as usize * 4
    }
}

/// `video_frames`' answer as pictures; `None` when there are none, or one isn't the size they say.
pub fn frames_of(video: glint_core::scrub::VideoFrames) -> Option<Frames> {
    let glint_core::scrub::VideoFrames {
        width,
        height,
        seconds,
        frames,
    } = video;
    let count = frames.len();
    let one = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if count == 0 || count > 64 || one == 0 || frames.iter().any(|frame| frame.len() != one) {
        return None;
    }
    let images = frames
        .into_iter()
        .map(|bgra| super::mf::bgra_image(width, height, bgra))
        .collect::<Option<Vec<_>>>()?;
    Some(Frames {
        count,
        width,
        height,
        seconds: if seconds.is_finite() && seconds > 0. {
            seconds
        } else {
            0.
        },
        images,
    })
}

thread_local! {
    /// Frames read before, newest last: (lowercased path and width, frames).
    static CACHE: RefCell<Vec<(String, Arc<Frames>)>> = const { RefCell::new(Vec::new()) };
}

fn key(path: &str, width: u32) -> String {
    format!("{}\n{width}", path.to_lowercase())
}

fn cached(path: &str, width: u32) -> Option<Arc<Frames>> {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let at = cache.iter().position(|(k, _)| *k == key(path, width))?;
        let found = cache.remove(at);
        let frames = found.1.clone();
        cache.push(found);
        Some(frames)
    })
}

fn remember(path: &str, width: u32, frames: Arc<Frames>) {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.retain(|(k, _)| *k != key(path, width));
        cache.push((key(path, width), frames));
        let mut total: usize = cache.iter().map(|(_, f)| f.cost()).sum();
        while total > BUDGET && cache.len() > 1 {
            total -= cache.remove(0).1.cost();
        }
    });
}

/// The tile's overlay.
pub struct Scrub {
    path: String,
    width: u32,
    sources: PlayerSources,
    frames: Option<Arc<Frames>>,
    /// The pointer across the tile, 0 to 0.999; none while it is off the tile.
    at: Option<f32>,
    hovering: bool,
    /// Whether a frame of `frames` is (or may be) in the GPU's atlas.
    drawn: bool,
    area: Rc<Cell<Bounds<Pixels>>>,
    loading: Option<Task<()>>,
}

impl Scrub {
    /// The width to ask for on a tile `box_px` wide (logical pixels) on a screen of `scale`: the tile's width in
    /// device pixels, at most two device pixels to a logical one.
    pub fn width_for(box_px: f32, scale: f32) -> u32 {
        (box_px * scale.min(2.)).round().max(1.) as u32
    }

    /// An overlay for the video at `path`, to be read at `width` pixels across.
    pub fn new(path: String, width: u32, cx: &mut Context<Self>) -> Self {
        Scrub {
            path,
            width,
            sources: player_sources(cx),
            frames: None,
            at: None,
            hovering: false,
            drawn: false,
            area: Rc::new(Cell::new(Bounds::default())),
            loading: None,
        }
    }

    /// Where the pointer is across the tile, if it is on it.
    pub fn at(&self) -> Option<f32> {
        self.at
    }

    /// The frames, once they are in.
    pub fn frames(&self) -> Option<&Arc<Frames>> {
        self.frames.as_ref()
    }

    /// The pointer came onto the tile: the frames are read (at once when they were before).
    pub(super) fn enter(&mut self, cx: &mut Context<Self>) {
        self.hovering = true;
        if self.frames.is_some() || self.loading.is_some() {
            return;
        }
        if let Some(frames) = cached(&self.path, self.width) {
            self.frames = Some(frames);
            return;
        }
        let running = (self.sources.frames)(&self.path, self.width);
        let (path, width) = (self.path.clone(), self.width);
        self.loading = Some(cx.spawn(async move |this, cx| {
            let frames = running.await;
            this.update(cx, |this, cx| {
                this.loading = None;
                if let Some(frames) = frames {
                    remember(&path, width, frames.clone());
                    this.frames = Some(frames);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The pointer left: nothing is drawn, and the pictures are let go of the atlas by the next frame.
    pub(super) fn leave(&mut self, cx: &mut Context<Self>) {
        self.hovering = false;
        if self.at.take().is_some() {
            cx.notify();
        }
    }

    fn moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let area = self.area.get();
        let width = f32::from(area.size.width);
        if !self.hovering || width <= 0. {
            return;
        }
        let across = ((f32::from(event.position.x) - f32::from(area.origin.x)) / width).clamp(0., 0.999);
        // Mirrored in Arabic, the tile's start is at the right.
        let across = if crate::i18n::is_rtl() { 0.999 - across } else { across };
        if self.at != Some(across) {
            self.at = Some(across);
            cx.notify();
        }
    }
}

impl Drop for Scrub {
    fn drop(&mut self) {
        if self.drawn {
            if let Some(frames) = &self.frames {
                frames.images.iter().cloned().for_each(retire);
            }
        }
    }
}

impl Render for Scrub {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        // Off the tile: what was drawn goes back from the atlas.
        if self.at.is_none() && self.drawn {
            if let Some(frames) = &self.frames {
                for image in &frames.images {
                    window.drop_image(image.clone()).ok();
                }
            }
            self.drawn = false;
        }
        let area = self.area.clone();
        let id = cx.entity_id();
        let measuring = canvas(
            move |bounds, window, _| {
                if area.get() != bounds {
                    area.set(bounds);
                    // Not `cx.notify` here: GPUI drops a redraw asked for during a draw.
                    window.on_next_frame(move |_, cx| cx.notify(id));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let shown = self.at.and_then(|at| self.frames.clone().map(|frames| (at, frames)));
        let overlay = shown.map(|(at, frames)| {
            let image = frames.at(at).cloned();
            self.drawn |= image.is_some();
            let label = (frames.seconds > 0.).then(|| clock(f64::from(at) * frames.seconds));
            div()
                .absolute()
                .size_full()
                .children(image.map(|image| {
                    img(image)
                        .object_fit(ObjectFit::Contain)
                        .absolute()
                        .size_full()
                        .rounded(theme::size::RADIUS_SM)
                }))
                .child(
                    div()
                        .absolute()
                        .bottom(space(1.))
                        .left(space(1.))
                        .right(space(1.))
                        .h(px(2.))
                        .rounded_full()
                        .bg(gpui_kit::black().opacity(0.3))
                        .child(div().h_full().w(gpui_kit::relative(at)).rounded_full().bg(t.primary)),
                )
                .children(label.map(|label| {
                    let tag = div()
                        .id("scrub-time")
                        .role(Role::Status)
                        .aria_label(t!("At {time}", time = label.clone()))
                        .test_support()
                        .absolute()
                        .top(space(1.))
                        .rounded(theme::size::RADIUS_SM)
                        .bg(gpui_kit::black().opacity(0.55))
                        .px(space(1.))
                        .text_size(theme::size::XXS)
                        .text_color(gpui_kit::white())
                        .child(label);
                    // `end-1`: the end edge, which is the left one in Arabic.
                    if crate::i18n::is_rtl() {
                        tag.left(space(1.))
                    } else {
                        tag.right(space(1.))
                    }
                }))
        });
        div()
            .id("video-scrub")
            .test_support()
            .absolute()
            .size_full()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered {
                    this.enter(cx);
                } else {
                    this.leave(cx);
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.moved(event, cx)))
            .child(measuring)
            .children(overlay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(count: u32, width: u32, height: u32, seconds: f64) -> glint_core::scrub::VideoFrames {
        glint_core::scrub::VideoFrames {
            width,
            height,
            seconds,
            // BGRA: red grows with the frame.
            frames: (0..count)
                .map(|frame| [30, 20, frame as u8 * 10, 255].repeat((width * height) as usize))
                .collect(),
        }
    }

    #[test]
    fn the_frames_core_answers_become_pictures_as_they_are() {
        let frames = frames_of(video(10, 4, 2, 125.5)).expect("frames");
        assert_eq!((frames.count, frames.width, frames.height), (10, 4, 2));
        assert_eq!(frames.seconds, 125.5);
        assert_eq!(frames.images.len(), 10);
        // Frame 1 is RGBA (10, 20, 30, 255): GPUI's BGRA has blue first.
        assert_eq!(&frames.images[1].as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
    }

    #[test]
    fn frames_of_the_wrong_size_or_none_at_all_are_nothing() {
        let mut short = video(2, 2, 2, 1.);
        short.frames[1].pop();
        assert!(frames_of(short).is_none());
        assert!(frames_of(video(0, 2, 2, 1.)).is_none());
        assert!(frames_of(video(2, 0, 2, 1.)).is_none());
        let mut huge = video(2, 2, 2, 1.);
        huge.width = u32::MAX;
        assert!(frames_of(huge).is_none());
    }

    #[test]
    fn the_pointer_picks_the_tenth_it_is_in() {
        let frames = frames_of(video(10, 2, 2, 0.)).expect("frames");
        assert_eq!(frames.seconds, 0.);
        let index = |at: f32| {
            frames
                .images
                .iter()
                .position(|i| Arc::ptr_eq(i, frames.at(at).unwrap()))
        };
        assert_eq!(index(0.), Some(0));
        assert_eq!(index(0.099), Some(0));
        assert_eq!(index(0.1), Some(1));
        assert_eq!(index(0.999), Some(9));
        assert_eq!(index(5.), Some(9));
    }

    #[test]
    fn the_cache_keeps_the_newest_within_its_budget() {
        let small = |n: u32| Arc::new(frames_of(video(2, 2, 2, n as f64)).expect("frames"));
        remember("C:\\a.mp4", 64, small(1));
        remember("C:\\B.mp4", 64, small(2));
        assert_eq!(cached("c:\\b.MP4", 64).map(|f| f.seconds), Some(2.));
        assert!(cached("C:\\a.mp4", 128).is_none(), "another width is another read");
        assert_eq!(cached("C:\\a.mp4", 64).map(|f| f.seconds), Some(1.));
    }

    #[test]
    fn a_tiles_width_is_its_device_pixels_up_to_double() {
        assert_eq!(Scrub::width_for(200., 1.), 200);
        assert_eq!(Scrub::width_for(200., 1.25), 250);
        assert_eq!(Scrub::width_for(200., 3.), 400);
    }
}
