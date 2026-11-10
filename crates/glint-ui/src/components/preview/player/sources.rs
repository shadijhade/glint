//! Where the player gets what takes work: Media Foundation and glint-core in the app, fakes in tests.
//!
//! The same seam as `preview::sources` (whose `extract` is borrowed from it): each method returns a future the
//! view awaits in a GPUI task, so the UI thread never waits. [`PlayerSources::core`] is the app's;
//! [`init_with`] sets others for a test.

use super::engine::{Kind, Opened};
use super::mf;
use super::scrub::{parse_frames, Frames};
use crate::components::preview::picture::sniff;
use crate::components::preview::sources::{sources, Ask, Fut};
use crate::core_bridge;
use gpui_kit::{App, Global, Image, RenderImage};
use std::rc::Rc;
use std::sync::Arc;

/// What speech search heard in a recording: the language, each part with the millisecond it begins at, and which
/// parts say the words that were searched for (`Heard`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    /// "en", "ar"…
    pub lang: String,
    /// `(milliseconds, text)`.
    pub parts: Vec<(u64, String)>,
    /// Indexes into `parts`.
    pub hits: Vec<usize>,
}

/// Opens a file in an engine.
pub type Open = Rc<dyn Fn(&str, Kind) -> Opened>;

/// What speech search heard in a recording, for a path and the words to mark.
pub type HeardFn = Rc<dyn Fn(&str, Option<&str>) -> Fut<Option<Heard>>>;

/// A video's scrub frames, for a path and a width in device pixels.
pub type FramesFn = Rc<dyn Fn(&str, u32) -> Fut<Option<Arc<Frames>>>>;

/// Everything the player asks of the outside world.
#[derive(Clone)]
pub struct PlayerSources {
    /// Opens a file in an engine (Media Foundation's, or a fake).
    pub open: Open,
    /// A recording's waveform: [`super::wave::BARS`] bars from 0 to 1, or nothing when it can't be decoded.
    pub peaks: Ask<Option<Vec<f32>>>,
    /// What speech search heard in a recording, with the parts that say the words (second argument) marked.
    pub heard: HeardFn,
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
            heard: Rc::new(|path, words| {
                let running = core_bridge::run(glint_core::speech::speech_transcript(
                    path.to_string(),
                    words.map(str::to_string),
                ));
                Box::pin(async move {
                    let heard = running.await.ok()?.ok()??;
                    Some(Heard {
                        lang: heard.lang,
                        parts: heard.parts,
                        hits: heard.hits,
                    })
                })
            }),
            cover: Rc::new(move |path| {
                let running = core_bridge::run(glint_core::media::media_details(path.to_string()));
                let renderer = renderer.clone();
                Box::pin(async move {
                    let url = running.await.ok()?.ok()??.cover?;
                    off_thread(move || decode_cover(&url, renderer)).await.flatten()
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
                    let bytes = running.await.ok()?.ok()?.into_bytes();
                    off_thread(move || parse_frames(&bytes)).await.flatten().map(Arc::new)
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

/// A `data:image/jpeg;base64,…` address as a picture.
fn decode_cover(url: &str, renderer: gpui_kit::SvgRenderer) -> Option<Arc<RenderImage>> {
    let bytes = cover_bytes(url)?;
    let format = sniff(&bytes, "")?;
    Image::from_bytes(format, bytes).to_image_data(renderer).ok()
}

/// The bytes inside a `data:…;base64,…` address.
fn cover_bytes(url: &str) -> Option<Vec<u8>> {
    base64_decode(url.strip_prefix("data:")?.split_once(";base64,")?.1)
}

/// Standard base64, with or without padding, ignoring line breaks; `None` for anything else.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut bits, mut count) = (0u32, 0u32);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        bits = bits << 6 | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_is_decoded_with_or_without_padding() {
        assert_eq!(base64_decode("TWFu").as_deref(), Some(&b"Man"[..]));
        assert_eq!(base64_decode("TWE=").as_deref(), Some(&b"Ma"[..]));
        assert_eq!(base64_decode("TQ").as_deref(), Some(&b"M"[..]));
        assert_eq!(base64_decode("TW\r\nFu").as_deref(), Some(&b"Man"[..]));
        assert_eq!(base64_decode("").as_deref(), Some(&b""[..]));
        assert!(base64_decode("T!Fu").is_none());
    }

    #[test]
    fn a_cover_that_is_not_a_data_address_is_nothing() {
        assert!(cover_bytes("https://x/y.jpg").is_none());
        assert_eq!(cover_bytes("data:image/png;base64,TWFu").as_deref(), Some(&b"Man"[..]));
    }
}
