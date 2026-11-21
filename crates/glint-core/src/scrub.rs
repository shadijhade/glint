//! Scrubbing a video in the grid: the pointer run across its tile shows the video at that point. The
//! frames come from Windows' own Media Foundation (the codecs File Explorer and Photos use), read at
//! evenly spaced points, and made the tile's size, in the BGRA the window draws.

use crate::fsx::blocking;

/// A video's frames for scrubbing, evenly spaced: each `width` x `height` pixels of opaque BGRA rows.
pub struct VideoFrames {
    pub width: u32,
    pub height: u32,
    /// The video's length in seconds, 0 when it doesn't say.
    pub seconds: f64,
    pub frames: Vec<Vec<u8>>,
}

/// `count` frames of the video at `path`, evenly spaced, each `width` pixels wide.
pub async fn video_frames(path: String, count: u32, width: u32) -> Result<VideoFrames, String> {
    blocking(move || frames(&path, count.clamp(1, 24), width.clamp(32, 512))).await
}

#[cfg(windows)]
fn frames(path: &str, count: u32, width: u32) -> Result<VideoFrames, String> {
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Media::MediaFoundation::*;
    // Looked up, not linked (mfapi.rs): Windows' N editions have no Media Foundation.
    use crate::mfapi::{MFCreateAttributes, MFCreateMediaType, MFCreateSourceReaderFromURL, MFShutdown, MFStartup};
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    let failed = |what: &'static str| move |e: windows::core::Error| format!("{what}: {}", e.message());
    unsafe {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(failed("Media Foundation didn't start"))?;
        let result = (|| -> Result<VideoFrames, String> {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 1).map_err(failed("Media Foundation"))?;
            let attributes = attributes.ok_or("Media Foundation")?;
            // Decoded straight to RGB, whatever the video's own format.
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .map_err(failed("Media Foundation"))?;
            let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
            let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &attributes)
                .map_err(failed("Windows can't read this video"))?;
            let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let wanted = MFCreateMediaType().map_err(failed("Media Foundation"))?;
            wanted
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(failed("Media Foundation"))?;
            wanted
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(failed("Media Foundation"))?;
            reader
                .SetCurrentMediaType(stream, None, &wanted)
                .map_err(failed("Windows can't decode this video"))?;
            let current = reader.GetCurrentMediaType(stream).map_err(failed("Media Foundation"))?;
            let size = current
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map_err(failed("Media Foundation"))?;
            let (src_w, src_h) = ((size >> 32) as usize, (size & 0xFFFF_FFFF) as usize);
            if src_w == 0 || src_h == 0 {
                return Err("The video has no picture".into());
            }
            // A negative stride: the rows come bottom up.
            let stride = current
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map(|s| s as i32)
                .unwrap_or((src_w * 4) as i32);
            let duration: u64 = reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .ok()
                .and_then(|v| u64::try_from(&v).ok())
                .unwrap_or(0);
            let (out_w, out_h) = (width as usize, ((width as usize * src_h) / src_w).max(1));
            let mut frames: Vec<Vec<u8>> = Vec::with_capacity(count as usize);
            for i in 0..count {
                // The middle of each stretch, so the first frame isn't a black lead-in.
                let at = (duration as f64 * (i as f64 + 0.5) / count as f64) as i64;
                let _ = reader.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(at));
                let mut frame = None;
                for _ in 0..8 {
                    let (mut flags, mut stamp, mut sample) = (0u32, 0i64, None);
                    if reader
                        .ReadSample(stream, 0, None, Some(&mut flags), Some(&mut stamp), Some(&mut sample))
                        .is_err()
                    {
                        break;
                    }
                    if let Some(sample) = sample {
                        frame = Some(sample);
                        break;
                    }
                    if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                        break;
                    }
                }
                let pixels = match frame {
                    Some(sample) => {
                        let buffer = sample.ConvertToContiguousBuffer().map_err(failed("Media Foundation"))?;
                        let (mut at, mut len) = (std::ptr::null_mut(), 0u32);
                        buffer
                            .Lock(&mut at, None, Some(&mut len))
                            .map_err(failed("Media Foundation"))?;
                        let data = std::slice::from_raw_parts(at, len as usize);
                        let scaled = scale(data, src_w, src_h, stride, out_w, out_h);
                        let _ = buffer.Unlock();
                        scaled
                    }
                    // Past what can be read: the last frame again.
                    None => frames.last().cloned().unwrap_or_else(|| vec![0; out_w * out_h * 4]),
                };
                frames.push(pixels);
            }
            Ok(VideoFrames {
                width: out_w as u32,
                height: out_h as u32,
                seconds: duration as f64 / 10_000_000.0,
                frames,
            })
        })();
        let _ = MFShutdown();
        if com {
            CoUninitialize();
        }
        result
    }
}

#[cfg(not(windows))]
fn frames(_path: &str, _count: u32, _width: u32) -> Result<VideoFrames, String> {
    Err("Needs Windows".into())
}

/// BGRX rows (`stride` bytes apart, bottom up when negative) to `out_w`×`out_h` opaque BGRA, each pixel the
/// average of the source pixels it covers (a box filter: smooth, and quick at these sizes).
fn scale(src: &[u8], src_w: usize, src_h: usize, stride: i32, out_w: usize, out_h: usize) -> Vec<u8> {
    let row = |y: usize| -> usize {
        if stride < 0 {
            (src_h - 1 - y) * (-stride) as usize
        } else {
            y * stride as usize
        }
    };
    let mut out = vec![0u8; out_w * out_h * 4];
    for oy in 0..out_h {
        let (y0, y1) = (
            oy * src_h / out_h,
            ((oy + 1) * src_h / out_h).max(oy * src_h / out_h + 1).min(src_h),
        );
        for ox in 0..out_w {
            let (x0, x1) = (
                ox * src_w / out_w,
                ((ox + 1) * src_w / out_w).max(ox * src_w / out_w + 1).min(src_w),
            );
            let (mut b, mut g, mut r, mut n) = (0u32, 0u32, 0u32, 0u32);
            // At most a 4×4 sample of what it covers keeps big frames quick.
            let (sy, sx) = (((y1 - y0) / 4).max(1), ((x1 - x0) / 4).max(1));
            let mut y = y0;
            while y < y1 {
                let base = row(y);
                let mut x = x0;
                while x < x1 {
                    if let Some(p) = src.get(base + x * 4..base + x * 4 + 3) {
                        b += p[0] as u32;
                        g += p[1] as u32;
                        r += p[2] as u32;
                        n += 1;
                    }
                    x += sx;
                }
                y += sy;
            }
            let at = (oy * out_w + ox) * 4;
            if let (Some(r), Some(g), Some(b)) = (r.checked_div(n), g.checked_div(n), b.checked_div(n)) {
                out[at] = b as u8;
                out[at + 1] = g as u8;
                out[at + 2] = r as u8;
            }
            out[at + 3] = 255;
        }
    }
    out
}

/// Formats seconds into compact `MMSS` for generated clip filenames.
fn mmss(secs: f64) -> String {
    let whole = secs.max(0.0).floor() as u64;
    format!("{:02}{:02}", whole / 60, whole % 60)
}

/// Output format for Shift+drag quick clip slicing on a video tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickClipFormat {
    Mp4Copy,
    Gif,
}

/// Computes a non-clobbering sibling path for a quick clip from `start_sec..end_sec`.
pub fn clip_output_path(input: &str, start_sec: f64, end_sec: f64, format: QuickClipFormat) -> Result<String, String> {
    if !start_sec.is_finite() || !end_sec.is_finite() || start_sec < 0.0 || end_sec <= start_sec + 0.05 {
        return Err("Select at least a short range before slicing a clip".to_string());
    }
    let src = std::path::Path::new(input);
    let parent = src.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
    let src_ext = src
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "mp4".to_string());
    let ext = match format {
        QuickClipFormat::Gif => "gif",
        QuickClipFormat::Mp4Copy => match src_ext.as_str() {
            "mp4" | "m4v" | "mov" | "mkv" | "webm" => src_ext.as_str(),
            _ => "mp4",
        },
    };
    let base = format!("{stem}_clip_{}-{}", mmss(start_sec), mmss(end_sec));
    let first = parent.join(format!("{base}.{ext}"));
    if !first.exists() {
        return Ok(first.to_string_lossy().into_owned());
    }
    for n in 2..=999 {
        let candidate = parent.join(format!("{base}_{n}.{ext}"));
        if !candidate.exists() {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    Err("Too many similarly named clips already exist in this folder".to_string())
}

/// Converts top-down tight `BGRA` pixels into a Windows bottom-up 32bpp `CF_DIB` payload (`BITMAPINFOHEADER` + pixels).
pub fn bgra_to_dib_v3(width: u32, height: u32, bgra: &[u8]) -> Result<Vec<u8>, String> {
    let row_bytes = (width as usize)
        .checked_mul(4)
        .ok_or_else(|| "Frame dimensions overflow".to_string())?;
    let expected = row_bytes
        .checked_mul(height as usize)
        .ok_or_else(|| "Frame dimensions overflow".to_string())?;
    if width == 0 || height == 0 || bgra.len() != expected {
        return Err("Invalid frame buffer dimensions".to_string());
    }
    let mut dib = Vec::with_capacity(40 + expected);
    dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
    dib.extend_from_slice(&(width as i32).to_le_bytes()); // biWidth
    dib.extend_from_slice(&(height as i32).to_le_bytes()); // biHeight (>0 = bottom-up DIB)
    dib.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    dib.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    dib.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    dib.extend_from_slice(&(expected as u32).to_le_bytes()); // biSizeImage
    dib.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    dib.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant
    for y in (0..height as usize).rev() {
        let row = &bgra[y * row_bytes..(y + 1) * row_bytes];
        dib.extend_from_slice(row);
    }
    Ok(dib)
}

/// Decodes one video frame at `at_seconds` into tightly packed top-down `BGRA` pixels.
/// Pass `max_width == 0` to keep full native video resolution.
pub async fn frame_bgra_at(path: String, at_seconds: f64, max_width: u32) -> Result<(u32, u32, Vec<u8>), String> {
    blocking(move || frame_at_sync(&path, at_seconds, max_width)).await
}

#[cfg(windows)]
fn frame_at_sync(path: &str, at_seconds: f64, max_width: u32) -> Result<(u32, u32, Vec<u8>), String> {
    use crate::mfapi::{MFCreateAttributes, MFCreateMediaType, MFCreateSourceReaderFromURL, MFShutdown, MFStartup};
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    // SAFETY: COM & dynamically loaded MF APIs are initialized on this worker thread and torn down before return.
    unsafe {
        let co = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let mf = MFStartup(MF_VERSION, 0).is_ok();
        let res = (|| -> Result<(u32, u32, Vec<u8>), String> {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 1).map_err(|e| e.to_string())?;
            if let Some(a) = &attrs {
                let _ = a.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1);
            }
            let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), attrs.as_ref())
                .map_err(|e| format!("Couldn't open video frame reader: {e}"))?;
            let rgb = MFCreateMediaType().map_err(|e| e.to_string())?;
            rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| e.to_string())?;
            rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(|e| e.to_string())?;
            reader
                .SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &rgb)
                .map_err(|e| format!("Couldn't select RGB32 video format: {e}"))?;
            let media_type = reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                .map_err(|e| e.to_string())?;
            let packed = media_type.GetUINT64(&MF_MT_FRAME_SIZE).map_err(|e| e.to_string())?;
            let src_w = (packed >> 32) as u32;
            let src_h = (packed & 0xFFFF_FFFF) as u32;
            if src_w == 0 || src_h == 0 {
                return Err("Video has empty dimensions".to_string());
            }
            let stride = media_type
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map(|s| s as i32)
                .unwrap_or((src_w * 4) as i32);

            let pos_100ns = (at_seconds.max(0.0) * 10_000_000.0).round() as i64;
            let prop = PROPVARIANT::from(pos_100ns);
            let _ = reader.SetCurrentPosition(&GUID::zeroed(), &prop);

            for _ in 0..8 {
                let mut flags = 0u32;
                let mut sample: Option<IMFSample> = None;
                reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                        0,
                        None,
                        Some(&mut flags),
                        None,
                        Some(&mut sample),
                    )
                    .map_err(|e| format!("Couldn't decode frame: {e}"))?;
                if (flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                    break;
                }
                let Some(sample) = sample else {
                    continue;
                };
                let buf = sample.ConvertToContiguousBuffer().map_err(|e| e.to_string())?;
                let mut ptr = std::ptr::null_mut();
                let mut cur_len = 0u32;
                buf.Lock(&mut ptr, None, Some(&mut cur_len))
                    .map_err(|e| e.to_string())?;
                let bytes = std::slice::from_raw_parts(ptr, cur_len as usize);
                let (out_w, out_h) = if max_width > 0 && src_w > max_width {
                    let h = ((u64::from(src_h) * u64::from(max_width)) / u64::from(src_w)).max(1) as u32;
                    (max_width, h)
                } else {
                    (src_w, src_h)
                };
                let bgra = scale(
                    bytes,
                    src_w as usize,
                    src_h as usize,
                    stride,
                    out_w as usize,
                    out_h as usize,
                );
                let _ = buf.Unlock();
                return Ok((out_w, out_h, bgra));
            }
            Err("No video frame was returned at this timestamp".to_string())
        })();
        if mf {
            let _ = MFShutdown();
        }
        if co {
            CoUninitialize();
        }
        res
    }
}

#[cfg(not(windows))]
fn frame_at_sync(_path: &str, _at_seconds: f64, _max_width: u32) -> Result<(u32, u32, Vec<u8>), String> {
    Err("Video frame decode requires Windows Media Foundation".to_string())
}

/// Decodes a full-resolution frame from `path` at `at_seconds` and places it on the clipboard as `CF_DIB`.
pub async fn copy_frame_to_clipboard(window: crate::host::Window, path: String, at_seconds: f64) -> Result<(), String> {
    let owner = crate::shell::owner(&window);
    blocking(move || copy_frame_sync(owner, &path, at_seconds)).await
}

#[cfg(windows)]
fn copy_frame_sync(owner: isize, path: &str, at_seconds: f64) -> Result<(), String> {
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::CF_DIB;

    let (w, h, bgra) = frame_at_sync(path, at_seconds, 0)?;
    let dib = bgra_to_dib_v3(w, h, &bgra)?;

    let hwnd = (owner != 0).then_some(HWND(owner as *mut std::ffi::c_void));
    // SAFETY: Standard Win32 clipboard sequence with retries for clipboard-viewer contention.
    unsafe {
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(hwnd).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        if !opened {
            return Err("The clipboard is held by another program".to_string());
        }
        let put_result = (|| -> Result<(), String> {
            EmptyClipboard().map_err(|e| e.to_string())?;
            let hmem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, dib.len()).map_err(|e| e.to_string())?;
            let ptr = GlobalLock(hmem) as *mut u8;
            if ptr.is_null() {
                let _ = GlobalFree(Some(hmem));
                return Err("Out of memory allocating clipboard frame".to_string());
            }
            std::ptr::copy_nonoverlapping(dib.as_ptr(), ptr, dib.len());
            let _ = GlobalUnlock(hmem);
            if let Err(e) = SetClipboardData(u32::from(CF_DIB.0), Some(HANDLE(hmem.0))) {
                let _ = GlobalFree(Some(hmem));
                return Err(e.to_string());
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        put_result
    }
}

#[cfg(not(windows))]
fn copy_frame_sync(_owner: isize, _path: &str, _at_seconds: f64) -> Result<(), String> {
    Err("Clipboard DIB requires Windows".to_string())
}

/// Slices a sibling MP4 (stream copy) or palette-optimized GIF between `start_sec` and `end_sec`.
pub async fn slice_clip(path: String, start_sec: f64, end_sec: f64, format: QuickClipFormat) -> Result<String, String> {
    blocking(move || slice_clip_sync(&path, start_sec, end_sec, format)).await
}

fn slice_clip_sync(path: &str, start_sec: f64, end_sec: f64, format: QuickClipFormat) -> Result<String, String> {
    let out = clip_output_path(path, start_sec, end_sec, format)?;
    let mut cmd = crate::tools::command("ffmpeg")?;
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"]);
    cmd.args([
        "-ss",
        &format!("{:.3}", start_sec.max(0.0)),
        "-to",
        &format!("{:.3}", end_sec),
        "-i",
        path,
    ]);
    match format {
        QuickClipFormat::Mp4Copy => {
            cmd.args([
                "-map",
                "0:v?",
                "-map",
                "0:a?",
                "-c",
                "copy",
                "-avoid_negative_ts",
                "make_zero",
            ]);
            if out.to_ascii_lowercase().ends_with(".mp4") || out.to_ascii_lowercase().ends_with(".mov") {
                cmd.args(["-movflags", "+faststart"]);
            }
        }
        QuickClipFormat::Gif => {
            cmd.args([
                "-filter_complex",
                "fps=15,scale=640:-2:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=sierra2_4a",
                "-loop",
                "0",
                "-an",
            ]);
        }
    }
    cmd.arg(&out);
    let output = cmd.output().map_err(|e| format!("Couldn't start FFmpeg: {e}"))?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&out);
        let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if msg.is_empty() {
            "FFmpeg couldn't slice that clip".to_string()
        } else {
            msg
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_to_dib_v3_writes_bottom_up_32bpp_header_and_pixels() {
        // 2x2 image: top row red/green, bottom row blue/white in BGRA
        let bgra = vec![
            0, 0, 255, 255, 0, 255, 0, 255, // y=0
            255, 0, 0, 255, 255, 255, 255, 255, // y=1
        ];
        let dib = bgra_to_dib_v3(2, 2, &bgra).unwrap();
        assert_eq!(dib.len(), 40 + 16);
        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
        assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 2);
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
        // Bottom row (y=1) appears first in positive-height DIB
        assert_eq!(&dib[40..48], &[255, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!(&dib[48..56], &[0, 0, 255, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn clip_output_path_validates_range_and_picks_expected_extension() {
        assert!(clip_output_path("C:\\v\\demo.mp4", 5.0, 5.0, QuickClipFormat::Mp4Copy).is_err());
        let mp4 = clip_output_path("C:\\v\\demo.mov", 12.0, 75.0, QuickClipFormat::Mp4Copy).unwrap();
        assert!(mp4.ends_with("demo_clip_0012-0115.mov"), "{mp4}");
        let gif = clip_output_path("C:\\v\\demo.mkv", 2.0, 6.0, QuickClipFormat::Gif).unwrap();
        assert!(gif.ends_with("demo_clip_0002-0006.gif"), "{gif}");
    }

    #[test]
    fn scales_bgrx_to_opaque_bgra_either_way_up() {
        // 2×2: top row blue, bottom row red (BGRX).
        let top_down = [255, 0, 0, 0, 255, 0, 0, 0, 0, 0, 255, 0, 0, 0, 255, 0];
        let out = scale(&top_down, 2, 2, 8, 1, 2);
        assert_eq!(out, [255, 0, 0, 255, 0, 0, 255, 255], "blue then red, as BGRA");
        let out = scale(&top_down, 2, 2, -8, 1, 2);
        assert_eq!(out, [0, 0, 255, 255, 255, 0, 0, 255], "bottom up: red first");
    }
}
