//! Scrubbing a video in the grid: the pointer run across its tile shows the video at that point. The
//! frames come from Windows' own Media Foundation (the codecs File Explorer and Photos use), read at
//! evenly spaced points, made the tile's size, and handed to the page as RGBA in one buffer.

use crate::fsx::blocking;

/// `count` frames of the video at `path`, evenly spaced, each `width` pixels wide: a buffer of
/// u32 count, u32 width, u32 height, f64 seconds of video, then each frame's RGBA rows.
#[tauri::command]
pub async fn video_frames(path: String, count: u32, width: u32) -> Result<tauri::ipc::Response, String> {
    blocking(move || frames(&path, count.clamp(1, 24), width.clamp(32, 512)).map(tauri::ipc::Response::new)).await
}

#[cfg(windows)]
fn frames(path: &str, count: u32, width: u32) -> Result<Vec<u8>, String> {
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    let failed = |what: &'static str| move |e: windows::core::Error| format!("{what}: {}", e.message());
    unsafe {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(failed("Media Foundation didn't start"))?;
        let result = (|| -> Result<Vec<u8>, String> {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 1).map_err(failed("Media Foundation"))?;
            let attributes = attributes.ok_or("Media Foundation")?;
            // Decoded straight to RGB, whatever the video's own format.
            attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(failed("Media Foundation"))?;
            let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
            let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &attributes).map_err(failed("Windows can't read this video"))?;
            let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let wanted = MFCreateMediaType().map_err(failed("Media Foundation"))?;
            wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(failed("Media Foundation"))?;
            wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(failed("Media Foundation"))?;
            reader.SetCurrentMediaType(stream, None, &wanted).map_err(failed("Windows can't decode this video"))?;
            let current = reader.GetCurrentMediaType(stream).map_err(failed("Media Foundation"))?;
            let size = current.GetUINT64(&MF_MT_FRAME_SIZE).map_err(failed("Media Foundation"))?;
            let (src_w, src_h) = ((size >> 32) as usize, (size & 0xFFFF_FFFF) as usize);
            if src_w == 0 || src_h == 0 {
                return Err("The video has no picture".into());
            }
            // A negative stride: the rows come bottom up.
            let stride = current.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or((src_w * 4) as i32);
            let duration: u64 = reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .ok()
                .and_then(|v| u64::try_from(&v).ok())
                .unwrap_or(0);
            let (out_w, out_h) = (width as usize, ((width as usize * src_h) / src_w).max(1));
            let mut out = Vec::with_capacity(20 + count as usize * out_w * out_h * 4);
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&(out_w as u32).to_le_bytes());
            out.extend_from_slice(&(out_h as u32).to_le_bytes());
            out.extend_from_slice(&(duration as f64 / 10_000_000.0).to_le_bytes());
            let mut last: Option<Vec<u8>> = None;
            for i in 0..count {
                // The middle of each stretch, so the first frame isn't a black lead-in.
                let at = (duration as f64 * (i as f64 + 0.5) / count as f64) as i64;
                let _ = reader.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(at));
                let mut frame = None;
                for _ in 0..8 {
                    let (mut flags, mut stamp, mut sample) = (0u32, 0i64, None);
                    if reader.ReadSample(stream, 0, None, Some(&mut flags), Some(&mut stamp), Some(&mut sample)).is_err() {
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
                        buffer.Lock(&mut at, None, Some(&mut len)).map_err(failed("Media Foundation"))?;
                        let data = std::slice::from_raw_parts(at, len as usize);
                        let scaled = scale(data, src_w, src_h, stride, out_w, out_h);
                        let _ = buffer.Unlock();
                        scaled
                    }
                    // Past what can be read: the last frame again.
                    None => last.clone().unwrap_or_else(|| vec![0; out_w * out_h * 4]),
                };
                out.extend_from_slice(&pixels);
                last = Some(pixels);
            }
            Ok(out)
        })();
        let _ = MFShutdown();
        if com {
            CoUninitialize();
        }
        result
    }
}

#[cfg(not(windows))]
fn frames(_path: &str, _count: u32, _width: u32) -> Result<Vec<u8>, String> {
    Err("Needs Windows".into())
}

/// BGRX rows (`stride` bytes apart, bottom up when negative) to `out_w`×`out_h` RGBA, each pixel the
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
        let (y0, y1) = (oy * src_h / out_h, ((oy + 1) * src_h / out_h).max(oy * src_h / out_h + 1).min(src_h));
        for ox in 0..out_w {
            let (x0, x1) = (ox * src_w / out_w, ((ox + 1) * src_w / out_w).max(ox * src_w / out_w + 1).min(src_w));
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
                out[at] = r as u8;
                out[at + 1] = g as u8;
                out[at + 2] = b as u8;
            }
            out[at + 3] = 255;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_bgrx_to_rgba_either_way_up() {
        // 2×2: top row blue, bottom row red (BGRX).
        let top_down = [255, 0, 0, 0, 255, 0, 0, 0, 0, 0, 255, 0, 0, 0, 255, 0];
        let out = scale(&top_down, 2, 2, 8, 1, 2);
        assert_eq!(out, [0, 0, 255, 255, 255, 0, 0, 255], "blue then red, as RGBA");
        let out = scale(&top_down, 2, 2, -8, 1, 2);
        assert_eq!(out, [255, 0, 0, 255, 0, 0, 255, 255], "bottom up: red first");
    }
}
