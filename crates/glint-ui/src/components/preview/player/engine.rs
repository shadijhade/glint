//! What the player asks of the thing that plays, and what it hears back; the numbers it works out around that.
//!
//! The real engine is Media Foundation's media engine ([`super::mf`]); a test uses a fake ([`super::fake`]).
//! Both are an [`Engine`]: the view sends it [`Command`]s (never waiting for an answer) and reads the
//! [`Event`]s it sends back on a channel, and takes the newest picture with [`Engine::take_frame`].

use gpui_kit::RenderImage;
use std::sync::Arc;

/// Video, or sound alone (the TypeScript's `VideoPreview` and `AudioPreview`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Pictures and sound.
    Video,
    /// Sound only: the engine makes no frames.
    Audio,
}

/// What the view asks the engine to do. Asking never waits.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// `play()`.
    Play,
    /// `pause()`.
    Pause,
    /// `currentTime = seconds`.
    Seek(f64),
    /// `volume`, 0 to 1.
    Volume(f64),
    /// `muted`.
    Muted(bool),
    /// `playbackRate`.
    Rate(f64),
    /// Pause by itself on reaching this time (the Trim bar's end: `stopAtEnd`), or stop doing so.
    StopAt(Option<f64>),
    /// The biggest box, in device pixels, a picture is wanted in: the engine scales to it on the GPU, so what is
    /// copied out grows with the preview, not with the video.
    Frame {
        /// Width in device pixels.
        width: u32,
        /// Height in device pixels.
        height: u32,
    },
}

/// What the engine learned about the file once it could read its start (`loadedmetadata`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Facts {
    /// Seconds; 0 when it isn't known (a stream).
    pub duration: f64,
    /// The video's size in pixels, 0 for sound.
    pub width: u32,
    /// The video's height in pixels.
    pub height: u32,
    /// There is a picture to show.
    pub has_video: bool,
    /// There is sound.
    pub has_audio: bool,
}

/// What the engine tells the view, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The file is open: its length and shape (`loadedmetadata`, `durationchange`).
    Loaded(Facts),
    /// Where it is, in seconds (`timeupdate`; sent about twenty times a second while it plays, and at once after
    /// a seek).
    Time(f64),
    /// It plays, or it stopped (`playing`, `pause`).
    Playing(bool),
    /// It reached the end (`ended`).
    Ended,
    /// It is waiting for data (`waiting`), or has it again.
    Waiting(bool),
    /// A new picture is ready: [`Engine::take_frame`] has it.
    Frame,
    /// It can't be played: the reason in the engine's words (`error`). Windows lacks the codec, or the file is
    /// damaged.
    Failed(String),
}

/// A picture of the video, already scaled to the size it is drawn at, in the BGRA GPUI draws.
#[derive(Clone)]
pub struct Frame {
    /// The pixels.
    pub image: Arc<RenderImage>,
    /// Its size in device pixels.
    pub width: u32,
    /// Its height in device pixels.
    pub height: u32,
}

/// Something that plays a file. The view owns one; letting go of it stops the sound and frees everything.
pub trait Engine {
    /// Asks it to do something. Never waits.
    fn send(&self, command: Command);
    /// The newest picture, once: asking again gives nothing until the next [`Event::Frame`].
    fn take_frame(&self) -> Option<Frame>;
}

/// An engine and the channel its events come on.
pub struct Opened {
    /// The engine.
    pub engine: Box<dyn Engine>,
    /// Its events.
    pub events: async_channel::Receiver<Event>,
}

// ---- Numbers ---------------------------------------------------------------------------------------------

/// The speeds the speed button goes through (Chromium's own menu, less 0.25 and 1.75).
pub const SPEEDS: [f64; 6] = [0.5, 0.75, 1., 1.25, 1.5, 2.];

/// The speed after `rate` in [`SPEEDS`], and the first after the last.
pub fn next_speed(rate: f64) -> f64 {
    SPEEDS
        .iter()
        .copied()
        .find(|speed| *speed > rate + 1e-9)
        .unwrap_or(SPEEDS[0])
}

/// `1×`, `1.25×`: a speed as the button says it.
pub fn speed_label(rate: f64) -> String {
    let text = format!("{rate:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text}×")
}

/// 3:07, or 1:02:09 (`clock` of FilePreview.tsx, from milliseconds).
pub fn clock_ms(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// [`clock_ms`] of a time in seconds (negative and not-a-number times are 0).
pub fn clock(seconds: f64) -> String {
    if seconds.is_finite() && seconds > 0. {
        clock_ms((seconds * 1000.) as u64)
    } else {
        clock_ms(0)
    }
}

/// How far across a track of `width` the pointer at `x` is (0 to 1), from the track's `start` edge; the track
/// runs from the right edge in right to left.
pub fn across(x: f32, left: f32, width: f32, rtl: bool) -> f64 {
    if width <= 0. {
        return 0.;
    }
    let fraction = f64::from(((x - left) / width).clamp(0., 1.));
    if rtl {
        1. - fraction
    } else {
        fraction
    }
}

/// The size a video of `native` pixels is copied out at to fill `area` (device pixels) without going past it or
/// past its own size: what [`super::super::picture::fit`] draws a picture at. At least 2 x 2.
pub fn frame_size(native: (u32, u32), area: (u32, u32)) -> (u32, u32) {
    let (w, h) = (native.0 as f32, native.1 as f32);
    if w <= 0. || h <= 0. || area.0 == 0 || area.1 == 0 {
        return (0, 0);
    }
    let scale = (area.0 as f32 / w).min(area.1 as f32 / h).min(1.);
    (((w * scale).round() as u32).max(2), ((h * scale).round() as u32).max(2))
}

/// What Media Foundation's error code means (`MF_MEDIA_ENGINE_ERR`), in the words the player shows underneath
/// the can't-play card's own.
pub fn error_text(code: usize) -> &'static str {
    match code {
        1 => "The playback was stopped",
        2 => "The file couldn't be read",
        3 => "Windows couldn't decode it",
        4 => "Windows has no codec for it",
        5 => "It is protected",
        _ => "Windows couldn't play it",
    }
}

/// A file's path as a `file:` address for the engine to tell its type by (spaces and non-ASCII are escaped).
pub fn file_url(path: &str) -> String {
    let mut url = String::from("file:///");
    let slashed = path.replace('\\', "/");
    // A network share, \\server\share, is file://server/share.
    let rest = slashed.strip_prefix("//").map(|share| {
        url = String::from("file://");
        share.to_string()
    });
    for byte in rest.unwrap_or(slashed).bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            url.push(char::from(byte));
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_the_typescript_wrote_them() {
        assert_eq!(clock_ms(0), "0:00");
        assert_eq!(clock_ms(187_000), "3:07");
        assert_eq!(clock_ms(3_729_000), "1:02:09");
        assert_eq!(clock_ms(59_999), "0:59");
        assert_eq!(clock(f64::NAN), "0:00");
        assert_eq!(clock(-4.), "0:00");
        assert_eq!(clock(61.5), "1:01");
    }

    #[test]
    fn the_speed_button_goes_round() {
        assert_eq!(next_speed(1.), 1.25);
        assert_eq!(next_speed(2.), 0.5);
        assert_eq!(next_speed(0.9), 1.);
        assert_eq!(speed_label(1.), "1×");
        assert_eq!(speed_label(1.25), "1.25×");
        assert_eq!(speed_label(0.5), "0.5×");
    }

    #[test]
    fn a_pointer_across_a_track_gives_a_fraction_that_mirrors_in_arabic() {
        assert_eq!(across(150., 100., 200., false), 0.25);
        assert_eq!(across(150., 100., 200., true), 0.75);
        assert_eq!(across(50., 100., 200., false), 0.);
        assert_eq!(across(900., 100., 200., false), 1.);
        assert_eq!(across(5., 0., 0., false), 0.);
    }

    #[test]
    fn a_frame_is_copied_out_at_the_size_it_is_drawn_never_bigger_than_the_video() {
        assert_eq!(frame_size((1920, 1080), (960, 960)), (960, 540));
        assert_eq!(frame_size((320, 240), (1600, 1600)), (320, 240));
        assert_eq!(frame_size((1080, 1920), (500, 500)), (281, 500));
        assert_eq!(frame_size((0, 0), (500, 500)), (0, 0));
        assert_eq!(frame_size((100, 100), (0, 500)), (0, 0));
        assert_eq!(frame_size((4000, 10), (100, 100)), (100, 2));
    }

    #[test]
    fn a_path_becomes_an_address_with_what_needs_escaping_escaped() {
        assert_eq!(
            file_url(r"C:\Windows\Media\a b.wav"),
            "file:///C:/Windows/Media/a%20b.wav"
        );
        assert_eq!(file_url(r"\\nas\share\x.mp4"), "file://nas/share/x.mp4");
        assert_eq!(
            file_url("C:\\موسيقى\\a.mp3"),
            "file:///C:/%D9%85%D9%88%D8%B3%D9%8A%D9%82%D9%89/a.mp3"
        );
    }

    #[test]
    fn media_foundation_errors_have_words() {
        assert_eq!(error_text(4), "Windows has no codec for it");
        assert_eq!(error_text(99), "Windows couldn't play it");
    }
}
