//! A recording's waveform under its player (`Waveform` of Waveform.tsx): how loud each stretch of it is, and
//! where it is playing.
//!
//! The TypeScript decoded the whole file in the page (`decodeAudioData`) and kept each of 480 bars' loudest
//! sample. Here Media Foundation's source reader decodes it, on a thread of its own, to 16-bit sound at
//! 22,050 Hz (a tenth of the data a recording usually has), and [`Peaks`] keeps the loudest sample of every
//! 1,024 frames, which [`Peaks::finish`] folds into the 480 bars. Recordings over [`MAX_BYTES`] aren't decoded.

use gpui_kit::{canvas, fill, point, px, size, Bounds, Hsla, IntoElement, Pixels, Styled as _};
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

/// Recordings up to this size are decoded for their waveform (`MAX_BYTES`).
pub const MAX_BYTES: u64 = 80 * 1024 * 1024;
/// How many bars the waveform has (`BARS`).
pub const BARS: usize = 480;
/// Frames of sound the loudest sample is kept for, before the blocks are folded into bars.
const BLOCK: usize = 1024;

/// The loudest sample of stretches of a recording, as it is decoded.
pub struct Peaks {
    channels: usize,
    in_block: usize,
    loudest: f32,
    blocks: Vec<f32>,
}

impl Peaks {
    /// For sound of `channels` channels (only the first two count, as in the TypeScript).
    pub fn new(channels: usize) -> Self {
        Peaks {
            channels: channels.max(1),
            in_block: 0,
            loudest: 0.,
            blocks: Vec::new(),
        }
    }

    /// One sample, 0 to 1 (its size, not its sign), of `channel` in the frame being read. Samples come in frame
    /// order: channel 0, 1, … then the next frame.
    pub fn sample(&mut self, channel: usize, level: f32) {
        if channel < 2 && level > self.loudest {
            self.loudest = level;
        }
        if channel + 1 == self.channels {
            self.in_block += 1;
            if self.in_block == BLOCK {
                self.blocks.push(self.loudest);
                self.loudest = 0.;
                self.in_block = 0;
            }
        }
    }

    /// The bars: the loudest block in each of `bars` equal stretches, as a fraction of the loudest of all. Empty
    /// when no sound was read.
    pub fn finish(mut self, bars: usize) -> Vec<f32> {
        if self.in_block > 0 {
            self.blocks.push(self.loudest);
        }
        let n = self.blocks.len();
        if n == 0 || bars == 0 {
            return Vec::new();
        }
        let folded: Vec<f32> = (0..bars)
            .map(|bar| {
                let lo = bar * n / bars;
                let hi = ((bar + 1) * n / bars).max(lo + 1).min(n);
                self.blocks[lo..hi].iter().copied().fold(0., f32::max)
            })
            .collect();
        // `reduce(max) || 1`: a silent recording stays flat.
        let top = folded.iter().copied().fold(0., f32::max);
        let top = if top > 0. { top } else { 1. };
        folded.into_iter().map(|v| v / top).collect()
    }
}

/// How loud one sample of `bits` bits is, 0 to 1 (PCM integers; 8-bit sound is unsigned).
fn level(bytes: &[u8], bits: usize) -> f32 {
    match (bits, bytes) {
        (8, [b, ..]) => (f32::from(*b) - 128.).abs() / 128.,
        (16, [lo, hi, ..]) => f32::from(i16::from_le_bytes([*lo, *hi])).abs() / 32768.,
        (24, [a, b, c, ..]) => (i32::from_le_bytes([0, *a, *b, *c]) >> 8).unsigned_abs() as f32 / 8_388_608.,
        (32, [a, b, c, d, ..]) => (i32::from_le_bytes([*a, *b, *c, *d]) as f64).abs() as f32 / 2_147_483_648.,
        _ => 0.,
    }
}

/// The recording's waveform: [`BARS`] bars from 0 to 1. On a thread that has nothing else of COM's or Media
/// Foundation's.
pub fn peaks_of(path: &str) -> Result<Vec<f32>, String> {
    // SAFETY: COM and Media Foundation are started and stopped here, on this thread, and every object made in
    // between is dropped before they stop.
    unsafe {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = match MFStartup(MF_VERSION, MFSTARTUP_LITE) {
            Ok(()) => {
                let result = decode(path);
                let _ = MFShutdown();
                result
            }
            Err(e) => Err(e.message()),
        };
        if com {
            CoUninitialize();
        }
        result
    }
}

/// Reads the sound through a source reader into [`Peaks`].
///
/// # Safety
/// Media Foundation is started on this thread.
unsafe fn decode(path: &str) -> Result<Vec<f32>, String> {
    let failed = |what: &'static str| move |e: windows::core::Error| format!("{what}: {}", e.message());
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), None).map_err(failed("Windows can't read this"))?;
    let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    reader
        .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
        .map_err(failed("Media Foundation"))?;
    reader
        .SetStreamSelection(stream, true)
        .map_err(failed("This has no sound"))?;
    // 16-bit stereo at 22,050 Hz if the converter can; else whatever PCM the decoder gives.
    let wanted = |small: bool| -> Result<IMFMediaType, windows::core::Error> {
        let kind = MFCreateMediaType()?;
        kind.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        kind.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        if small {
            kind.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            kind.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 22_050)?;
            kind.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        }
        Ok(kind)
    };
    let set = |small: bool| wanted(small).and_then(|kind| reader.SetCurrentMediaType(stream, None, &kind));
    if set(true).is_err() {
        set(false).map_err(failed("Windows can't decode this"))?;
    }
    let current = reader.GetCurrentMediaType(stream).map_err(failed("Media Foundation"))?;
    let channels = current.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2).max(1) as usize;
    let bits = current.GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE).unwrap_or(16) as usize;
    if ![8, 16, 24, 32].contains(&bits) {
        return Err("Windows decoded it to a format the waveform doesn't read".into());
    }
    let width = bits / 8;
    let mut peaks = Peaks::new(channels);
    let mut counter = 0usize;
    loop {
        let (mut flags, mut sample) = (0u32, None);
        reader
            .ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))
            .map_err(failed("Windows stopped decoding"))?;
        if let Some(sample) = sample {
            let buffer = sample.ConvertToContiguousBuffer().map_err(failed("Media Foundation"))?;
            let (mut at, mut length) = (std::ptr::null_mut(), 0u32);
            buffer
                .Lock(&mut at, None, Some(&mut length))
                .map_err(failed("Media Foundation"))?;
            if !at.is_null() {
                let bytes = std::slice::from_raw_parts(at, length as usize);
                for one in bytes.chunks_exact(width) {
                    peaks.sample(counter % channels, level(one, bits));
                    counter += 1;
                }
            }
            let _ = buffer.Unlock();
        }
        if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 || flags & (MF_SOURCE_READERF_ERROR.0 as u32) != 0 {
            break;
        }
    }
    let bars = peaks.finish(BARS);
    if bars.is_empty() {
        Err("There was no sound to draw".into())
    } else {
        Ok(bars)
    }
}

/// The bars as a canvas to put in a box: each bar's height is its loudness, those before `played` (0 to 1) are in
/// `played_color` and the rest in `rest_color`. Bars run from the end edge in right to left.
pub fn bars(peaks: Arc<Vec<f32>>, played: f64, rtl: bool, played_color: Hsla, rest_color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window, _| {
            let count = peaks.len();
            if count == 0 {
                return;
            }
            let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let step = width / count as f32;
            for (i, loudness) in peaks.iter().enumerate() {
                let h = (loudness * height * 0.95).max(1.);
                let w = (step - 0.5).max(1.);
                let from = if rtl {
                    width - (i as f32 + 1.) * step
                } else {
                    i as f32 * step
                };
                let colour = if (i as f64 / count as f64) <= played {
                    played_color
                } else {
                    rest_color
                };
                window.paint_quad(fill(
                    Bounds {
                        origin: point(bounds.origin.x + px(from), bounds.origin.y + px((height - h) / 2.)),
                        size: size(px(w), px(h)),
                    },
                    colour,
                ));
            }
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_are_the_loudest_sample_of_each_stretch_as_a_fraction_of_the_loudest() {
        let mut peaks = Peaks::new(1);
        // Four blocks: quiet, loud, quieter, silent.
        for (block, loudness) in [0.1f32, 0.8, 0.4, 0.].iter().enumerate() {
            for frame in 0..BLOCK {
                peaks.sample(0, if frame == 7 { *loudness } else { 0.01 * block as f32 });
            }
        }
        let bars = peaks.finish(4);
        assert_eq!(bars.len(), 4);
        assert_eq!(bars[1], 1.);
        assert!((bars[0] - 0.125).abs() < 1e-6);
        assert!((bars[2] - 0.5).abs() < 1e-6);
        // The last block's frames were 0.03 each: a quiet floor, not silence.
        assert!((bars[3] - 0.03 / 0.8).abs() < 1e-6);
    }

    #[test]
    fn only_the_first_two_channels_count_and_the_louder_one_wins() {
        let mut peaks = Peaks::new(3);
        // One block where the third channel is loudest, one where the second channel is.
        for _ in 0..BLOCK {
            peaks.sample(0, 0.1);
            peaks.sample(1, 0.05);
            peaks.sample(2, 1.0);
        }
        for _ in 0..BLOCK {
            peaks.sample(0, 0.2);
            peaks.sample(1, 0.5);
            peaks.sample(2, 0.);
        }
        let bars = peaks.finish(2);
        assert!((bars[0] - 0.2).abs() < 1e-6, "{bars:?}: the third channel isn't heard");
        assert_eq!(bars[1], 1.);
    }

    #[test]
    fn fewer_blocks_than_bars_repeat_and_silence_stays_flat() {
        let mut peaks = Peaks::new(1);
        for frame in 0..BLOCK * 2 {
            peaks.sample(0, if frame < BLOCK { 0.5 } else { 0.25 });
        }
        assert_eq!(peaks.finish(4), [1., 1., 0.5, 0.5]);
        let mut silence = Peaks::new(2);
        for _ in 0..100 {
            silence.sample(0, 0.);
            silence.sample(1, 0.);
        }
        assert_eq!(silence.finish(3), [0., 0., 0.]);
        assert!(Peaks::new(2).finish(3).is_empty());
    }

    #[test]
    fn samples_of_each_depth_are_read_as_a_fraction_of_full_scale() {
        assert_eq!(level(&[0x00, 0x80], 16), 1.);
        assert_eq!(level(&[0x00, 0x40], 16), 0.5);
        assert_eq!(level(&[0xFF, 0xFF], 16), 1. / 32768.);
        assert_eq!(level(&[0x00], 8), 1.);
        assert_eq!(level(&[0xC0], 8), 0.5);
        assert_eq!(level(&[0, 0, 0x40], 24), 0.5);
        assert_eq!(level(&[0, 0, 0, 0x40], 32), 0.5);
        assert_eq!(level(&[1], 16), 0.);
        assert_eq!(level(&[1, 2], 12), 0.);
    }

    /// Windows' own sample sounds, read for real: Media Foundation decodes them and they have sound in them.
    #[test]
    #[ignore = "needs Windows' Media Foundation and C:\\Windows\\Media"]
    fn a_real_wav_has_a_waveform() {
        let Some(path) = first_wav() else {
            return;
        };
        let bars = peaks_of(&path).expect("the sample sound decodes");
        assert_eq!(bars.len(), BARS);
        assert!(bars.iter().all(|v| (0. ..=1.).contains(v)));
        assert_eq!(bars.iter().copied().fold(0., f32::max), 1.);
    }

    fn first_wav() -> Option<String> {
        std::fs::read_dir(r"C:\Windows\Media")
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")))
            .map(|p| p.to_string_lossy().into_owned())
    }
}
