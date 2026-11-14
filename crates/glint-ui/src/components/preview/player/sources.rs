//! Where the player gets what takes work: Media Foundation and glint-core in the app, fakes in tests.
//!
//! The same seam as `preview::sources` (whose `extract` is borrowed from it): each method returns a future the
//! view awaits in a GPUI task, so the UI thread never waits. [`PlayerSources::core`] is the app's;
//! [`init_with`] sets others for a test.

use super::engine::{Kind, Opened};
use super::mf;
use super::scrub::{frames_of, Frames};
use crate::components::preview::picture::sniff;
use crate::components::preview::sources::{sources, Ask, Fut};
use crate::core_bridge;
use gpui_kit::{App, Global, Image, RenderImage};
use std::rc::Rc;
use std::sync::Arc;

/// Opens a file in an engine.
pub type Open = Rc<dyn Fn(&str, Kind) -> Opened>;

/// A video's scrub frames, for a path and a width in device pixels.
pub type FramesFn = Rc<dyn Fn(&str, u32) -> Fut<Option<Arc<Frames>>>>;

/// Everything the player asks of the outside world.
#[derive(Clone)]
pub struct PlayerSources {
    /// Opens a file in an engine (Media Foundation's, or a fake).
    pub open: Open,
    /// A recording's waveform: [`super::wave::BARS`] bars from 0 to 1, or nothing when it can't be decoded.
    pub peaks: Ask<Option<Vec<f32>>>,
    /// A song's cover, decoded.
    pub cover: Ask<Option<Arc<RenderImage>>>,
    /// A file in an archive (or on a remote), copied out to a temp file to be played: its path.
    pub extract: Ask<Result<String, String>>,
    /// A video's scrub frames at a width in device pixels (`video_frames`).
    pub frames: FramesFn,
}

/// Runs `work` on a thread of its own and hands back its answer: for work that is neither core's nor GPUI's
/// (decoding a whole recording, a cover).
fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Fut<Option<T>> {
    let (answer, heard) = async_channel::bounded(1);
    let started = std::thread::Builder::new()
        .name("glint-player-work".into())
        .spawn(move || {
            let _ = answer.send_blocking(work());
        });
    Box::pin(async move {
        started.ok()?;
        heard.recv().await.ok()
    })
}

impl PlayerSources {
    /// Media Foundation, glint-core, and `preview::sources`' extraction.
    pub fn core(cx: &mut App) -> Self {
        let renderer = cx.svg_renderer();
        let extract = sources(cx).extract;
        PlayerSources {
            open: Rc::new(mf::open),
            peaks: Rc::new(|path| {
                let path = path.to_string();
                let running = off_thread(move || super::wave::peaks_of(&path).ok());
                Box::pin(async move { running.await.flatten() })
            }),
            cover: Rc::new(move |path| {
                let running = core_bridge::run(glint_core::media::media_details(path.to_string()));
                let renderer = renderer.clone();
                Box::pin(async move {
                    let cover = running.await.ok()?.ok()??.cover?;
                    off_thread(move || decode_cover(cover, renderer)).await.flatten()
                })
            }),
            extract,
            frames: Rc::new(|path, width| {
                let running = core_bridge::run(glint_core::scrub::video_frames(
                    path.to_string(),
                    super::scrub::COUNT as u32,
                    width,
                ));
                Box::pin(async move {
                    let video = running.await.ok()?.ok()?;
                    off_thread(move || frames_of(video)).await.flatten().map(Arc::new)
                })
            }),
        }
    }
}

/// The sources in use, in a GPUI global.
struct Shared(PlayerSources);

impl Global for Shared {}

/// Sets the sources the player uses (tests; the app uses [`PlayerSources::core`] without this).
pub fn init_with(cx: &mut App, sources: PlayerSources) {
    cx.set_global(Shared(sources));
}

/// The sources in use: the app's unless [`init_with`] set others.
pub fn player_sources(cx: &mut App) -> PlayerSources {
    if let Some(shared) = cx.try_global::<Shared>() {
        return shared.0.clone();
    }
    let core = PlayerSources::core(cx);
    init_with(cx, core);
    cx.global::<Shared>().0.clone()
}

// ---- Covers ----------------------------------------------------------------------------------------------

/// A song's cover (a JPEG, a PNG… as the file holds it) as a picture; `None` for anything else.
fn decode_cover(bytes: Vec<u8>, renderer: gpui_kit::SvgRenderer) -> Option<Arc<RenderImage>> {
    let format = sniff(&bytes, "")?;
    Image::from_bytes(format, bytes).to_image_data(renderer).ok()
}
