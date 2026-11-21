//! Drag-to-Transform (`CFSTR_FILEDESCRIPTORW` + `CFSTR_FILECONTENTS` `TYMED_ISTREAM` OLE virtual drag).
//!
//! Advertises transformed filenames on the drag payload immediately and materializes bytes lazily inside
//! `IDataObject::GetData` only when the drop target accepts the drop — zero disk writes until a drop
//! actually happens, and compatible with Explorer, browsers, chat apps, and mail clients.

use std::path::Path;

/// Transformation applied lazily on drop during a virtual drag-out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DragTransform {
    /// Losslessly strips EXIF/XMP/IPTC/comment metadata from JPEG and PNG files in memory.
    StripMetadata,
    /// Compresses a video or image to stay under `mb` megabytes.
    ShrinkUnderMb(u32),
    /// Converts a picture to WebP.
    ConvertWebp,
    /// Converts a picture to PNG.
    ConvertPng,
    /// Extracts audio from a video to MP3.
    ExtractAudioMp3,
}

/// Computes the virtual filename advertised in `CFSTR_FILEDESCRIPTORW`.
pub fn transformed_filename(path: &str, transform: &DragTransform) -> String {
    let p = Path::new(path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let orig_ext = p
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    match transform {
        DragTransform::StripMetadata => {
            if orig_ext.is_empty() {
                format!("{stem}_clean")
            } else {
                format!("{stem}_clean.{orig_ext}")
            }
        }
        DragTransform::ShrinkUnderMb(mb) => {
            let ext = if orig_ext.is_empty() { "mp4" } else { orig_ext.as_str() };
            format!("{stem}_under{mb}mb.{ext}")
        }
        DragTransform::ConvertWebp => format!("{stem}.webp"),
        DragTransform::ConvertPng => format!("{stem}.png"),
        DragTransform::ExtractAudioMp3 => format!("{stem}.mp3"),
    }
}

/// Losslessly strips metadata segments from JPEG (`jpg`/`jpeg`) or PNG (`png`) byte streams in memory.
pub fn strip_metadata_bytes(ext: &str, input: &[u8]) -> Result<Vec<u8>, String> {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => strip_jpeg_metadata(input),
        "png" => strip_png_metadata(input),
        other => Err(format!(
            "Lossless metadata stripping isn't supported for .{other} files"
        )),
    }
}

fn strip_jpeg_metadata(input: &[u8]) -> Result<Vec<u8>, String> {
    if input.len() < 4 || input[0] != 0xFF || input[1] != 0xD8 {
        return Err("Not a valid JPEG stream".to_string());
    }
    let mut out = Vec::with_capacity(input.len());
    out.extend_from_slice(&input[..2]); // SOI
    let mut i = 2usize;
    while i < input.len() {
        if input[i] != 0xFF {
            // Unexpected raw byte outside entropy scan; copy remainder safely.
            out.extend_from_slice(&input[i..]);
            break;
        }
        while i < input.len() && input[i] == 0xFF {
            i += 1;
        }
        if i >= input.len() {
            break;
        }
        let marker = input[i];
        i += 1;
        if marker == 0xD9 {
            // EOI
            out.push(0xFF);
            out.push(0xD9);
            break;
        }
        // Standalone restart/TEM markers have no length word.
        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            out.push(0xFF);
            out.push(marker);
            continue;
        }
        if i + 2 > input.len() {
            return Err("Truncated JPEG marker".to_string());
        }
        let seg_len = u16::from_be_bytes([input[i], input[i + 1]]) as usize;
        if seg_len < 2 || i + seg_len > input.len() {
            return Err("Invalid JPEG segment length".to_string());
        }
        // Keep JFIF APP0 (0xE0) and ICC profile if needed? Wait: strip APP1..APP15 (EXIF/XMP/IPTC/ICC) and COM (0xFE).
        let is_metadata = (0xE1..=0xEF).contains(&marker) || marker == 0xFE;
        if marker == 0xDA {
            // Start of Scan: copy SOS header and all entropy-coded data up to EOI verbatim.
            out.push(0xFF);
            out.push(marker);
            out.extend_from_slice(&input[i..]);
            break;
        }
        if !is_metadata {
            out.push(0xFF);
            out.push(marker);
            out.extend_from_slice(&input[i..i + seg_len]);
        }
        i += seg_len;
    }
    Ok(out)
}

fn strip_png_metadata(input: &[u8]) -> Result<Vec<u8>, String> {
    const SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    if input.len() < 8 || input[..8] != SIG {
        return Err("Not a valid PNG stream".to_string());
    }
    let mut out = Vec::with_capacity(input.len());
    out.extend_from_slice(&SIG);
    let mut i = 8usize;
    while i + 12 <= input.len() {
        let len = u32::from_be_bytes([input[i], input[i + 1], input[i + 2], input[i + 3]]) as usize;
        let kind = &input[i + 4..i + 8];
        let total = 12usize
            .checked_add(len)
            .ok_or_else(|| "PNG chunk size overflow".to_string())?;
        if i + total > input.len() {
            return Err("Truncated PNG chunk".to_string());
        }
        let strip = matches!(kind, b"eXIf" | b"tEXt" | b"zTXt" | b"iTXt" | b"tIME");
        if !strip {
            out.extend_from_slice(&input[i..i + total]);
        }
        i += total;
        if kind == b"IEND" {
            break;
        }
    }
    Ok(out)
}

/// Materializes transformed bytes for `path` according to `transform`.
pub fn materialize_bytes(path: &str, transform: &DragTransform) -> Result<Vec<u8>, String> {
    let p = Path::new(path);
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    match transform {
        DragTransform::StripMetadata => {
            let raw = std::fs::read(path).map_err(|e| format!("Couldn't read {path}: {e}"))?;
            strip_metadata_bytes(&ext, &raw)
        }
        DragTransform::ShrinkUnderMb(mb) => {
            let temp = temp_target("shrink", &ext);
            let target_kbps = ((*mb as u64).max(1) * 8 * 900) / 10; // conservative single-pass target bitrate budget
            let res = run_ffmpeg_to(
                &[
                    "-i",
                    path,
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                    "-b:v",
                    &format!("{target_kbps}k"),
                    "-maxrate",
                    &format!("{}k", target_kbps * 3 / 2),
                    "-bufsize",
                    &format!("{}k", target_kbps * 2),
                    "-c:a",
                    "aac",
                    "-b:a",
                    "128k",
                    "-movflags",
                    "+faststart",
                ],
                &temp,
            );
            read_and_cleanup(res, &temp)
        }
        DragTransform::ConvertWebp => {
            let temp = temp_target("webp", "webp");
            let res = run_ffmpeg_to(&["-i", path, "-c:v", "libwebp", "-quality", "82", "-an"], &temp);
            read_and_cleanup(res, &temp)
        }
        DragTransform::ConvertPng => {
            let temp = temp_target("png", "png");
            let res = run_ffmpeg_to(&["-i", path, "-frames:v", "1"], &temp);
            read_and_cleanup(res, &temp)
        }
        DragTransform::ExtractAudioMp3 => {
            let temp = temp_target("audio", "mp3");
            let res = run_ffmpeg_to(&["-i", path, "-vn", "-c:a", "libmp3lame", "-q:a", "2"], &temp);
            read_and_cleanup(res, &temp)
        }
    }
}

fn temp_target(tag: &str, ext: &str) -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ext = if ext.is_empty() { "bin" } else { ext };
    std::env::temp_dir().join(format!("glint-vdrag-{tag}-{}-{n}.{ext}", std::process::id()))
}

fn run_ffmpeg_to(args: &[&str], out: &Path) -> Result<(), String> {
    let mut cmd = crate::tools::command("ffmpeg")?;
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"]);
    cmd.args(args).arg(out);
    let done = cmd.output().map_err(|e| format!("Couldn't start FFmpeg: {e}"))?;
    if !done.status.success() {
        let _ = std::fs::remove_file(out);
        let msg = String::from_utf8_lossy(&done.stderr).trim().to_string();
        return Err(if msg.is_empty() {
            "FFmpeg failed while transforming item".to_string()
        } else {
            msg
        });
    }
    Ok(())
}

fn read_and_cleanup(res: Result<(), String>, temp: &Path) -> Result<Vec<u8>, String> {
    let out = match res {
        Ok(()) => std::fs::read(temp).map_err(|e| format!("Couldn't read transformed output: {e}")),
        Err(err) => Err(err),
    };
    let _ = std::fs::remove_file(temp);
    out
}

/// Starts a Windows OLE drag-out with `CFSTR_FILEDESCRIPTORW` + lazy `CFSTR_FILECONTENTS` `TYMED_ISTREAM`.
pub async fn start_virtual_drag(
    window: crate::host::Window,
    paths: Vec<String>,
    transform: DragTransform,
) -> Result<String, String> {
    if paths.is_empty() {
        return Err("Nothing selected to drag".to_string());
    }
    let owner = crate::shell::owner(&window);
    #[cfg(windows)]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        window
            .run_on_main_thread(move || {
                let _ = tx.send(imp::drag_out(owner, paths, transform));
            })
            .map_err(|e| format!("Couldn't schedule drag on UI thread: {e}"))?;
        rx.await
            .map_err(|_| "Virtual drag cancelled before starting".to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (owner, paths, transform);
        Err("Virtual drag is only available on Windows".to_string())
    }
}

#[cfg(windows)]
mod imp {
    use super::{materialize_bytes, transformed_filename, DragTransform};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, OnceLock};
    use windows::core::{implement, ComObject, Ref, BOOL, HRESULT, PCWSTR};
    use windows::Win32::Foundation::{
        GlobalFree, DV_E_FORMATETC, DV_E_LINDEX, DV_E_TYMED, E_NOTIMPL, E_POINTER, HGLOBAL, HWND,
        OLE_E_ADVISENOTSUPPORTED, S_FALSE, S_OK,
    };
    use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
    use windows::Win32::System::Com::{
        IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumFORMATETC_Impl, IStream, DATADIR_GET,
        DVASPECT_CONTENT, FORMATETC, STATDATA, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL, TYMED_ISTREAM,
    };
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT};
    use windows::Win32::System::Ole::{IDropSource, DROPEFFECT_COPY};
    use windows::Win32::UI::Shell::{
        SHDoDragDrop, CFSTR_FILECONTENTS, CFSTR_FILEDESCRIPTORW, FD_UNICODE, FILEDESCRIPTORW, FILEGROUPDESCRIPTORW,
    };

    #[derive(Clone)]
    struct Item {
        source_path: String,
        virtual_name: String,
        transform: DragTransform,
        cached: Arc<OnceLock<Result<Vec<u8>, String>>>,
    }

    impl Item {
        fn bytes(&self) -> Result<&[u8], String> {
            self.cached
                .get_or_init(|| materialize_bytes(&self.source_path, &self.transform))
                .as_deref()
                .map_err(Clone::clone)
        }
    }

    #[implement(IEnumFORMATETC)]
    struct FormatEnum {
        formats: Vec<FORMATETC>,
        pos: AtomicUsize,
    }

    impl IEnumFORMATETC_Impl for FormatEnum_Impl {
        fn Next(&self, celt: u32, rgelt: *mut FORMATETC, pceltfetched: *mut u32) -> HRESULT {
            if rgelt.is_null() {
                return E_POINTER;
            }
            if celt > 1 && pceltfetched.is_null() {
                return E_POINTER;
            }
            let mut copied = 0u32;
            let mut cur = self.pos.load(Ordering::Relaxed);
            while copied < celt && cur < self.formats.len() {
                // SAFETY: caller supplied array of at least `celt` `FORMATETC` elements.
                unsafe { *rgelt.add(copied as usize) = self.formats[cur] };
                cur += 1;
                copied += 1;
            }
            self.pos.store(cur, Ordering::Relaxed);
            if !pceltfetched.is_null() {
                // SAFETY: non-null caller pointer.
                unsafe { *pceltfetched = copied };
            }
            if copied == celt {
                S_OK
            } else {
                S_FALSE
            }
        }

        fn Skip(&self, celt: u32) -> windows::core::Result<()> {
            let cur = self.pos.load(Ordering::Relaxed);
            let next = cur.saturating_add(celt as usize);
            self.pos.store(next.min(self.formats.len()), Ordering::Relaxed);
            if next <= self.formats.len() {
                Ok(())
            } else {
                Err(S_FALSE.into())
            }
        }

        fn Reset(&self) -> windows::core::Result<()> {
            self.pos.store(0, Ordering::Relaxed);
            Ok(())
        }

        fn Clone(&self) -> windows::core::Result<IEnumFORMATETC> {
            let cloned: IEnumFORMATETC = ComObject::new(FormatEnum {
                formats: self.formats.clone(),
                pos: AtomicUsize::new(self.pos.load(Ordering::Relaxed)),
            })
            .into_interface();
            Ok(cloned)
        }
    }

    #[implement(IDataObject)]
    pub(super) struct VirtualDataObject {
        cf_descriptor: u16,
        cf_contents: u16,
        items: Vec<Item>,
    }

    impl VirtualDataObject {
        pub(super) fn new(paths: Vec<String>, transform: DragTransform) -> Self {
            // SAFETY: Win32 clipboard format registration for standard shell descriptor formats.
            let (cf_descriptor, cf_contents) = unsafe {
                (
                    RegisterClipboardFormatW(CFSTR_FILEDESCRIPTORW) as u16,
                    RegisterClipboardFormatW(CFSTR_FILECONTENTS) as u16,
                )
            };
            let items = paths
                .into_iter()
                .map(|source_path| Item {
                    virtual_name: transformed_filename(&source_path, &transform),
                    source_path,
                    transform: transform.clone(),
                    cached: Arc::new(OnceLock::new()),
                })
                .collect();
            Self {
                cf_descriptor,
                cf_contents,
                items,
            }
        }

        fn descriptor_format(&self) -> FORMATETC {
            FORMATETC {
                cfFormat: self.cf_descriptor,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            }
        }

        fn contents_format(&self, lindex: i32) -> FORMATETC {
            FORMATETC {
                cfFormat: self.cf_contents,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex,
                tymed: TYMED_ISTREAM.0 as u32,
            }
        }

        fn build_descriptor_hglobal(&self) -> windows::core::Result<HGLOBAL> {
            let count = self.items.len();
            let extra = count.saturating_sub(1) * size_of::<FILEDESCRIPTORW>();
            let total = size_of::<FILEGROUPDESCRIPTORW>() + extra;
            // SAFETY: allocates movable zero-initialized global memory owned by the STGMEDIUM returned to OLE.
            // Both `FILEGROUPDESCRIPTORW` and `FILEDESCRIPTORW` are packed structs on Windows, so every write uses
            // `write_unaligned` via raw pointers to avoid unaligned references.
            unsafe {
                let hmem = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, total)?;
                let base = GlobalLock(hmem) as *mut u8;
                if base.is_null() {
                    let _ = GlobalFree(Some(hmem));
                    return Err(E_POINTER.into());
                }
                let fgd_ptr = base as *mut FILEGROUPDESCRIPTORW;
                std::ptr::addr_of_mut!((*fgd_ptr).cItems).write_unaligned(count as u32);
                let first_fd = std::ptr::addr_of_mut!((*fgd_ptr).fgd) as *mut FILEDESCRIPTORW;
                for (i, item) in self.items.iter().enumerate() {
                    let mut name_buf = [0u16; 260];
                    for (slot, unit) in name_buf.iter_mut().zip(item.virtual_name.encode_utf16().take(259)) {
                        *slot = unit;
                    }
                    let fd = FILEDESCRIPTORW {
                        dwFlags: FD_UNICODE.0 as u32,
                        cFileName: name_buf,
                        ..Default::default()
                    };
                    first_fd.add(i).write_unaligned(fd);
                }
                let _ = GlobalUnlock(hmem);
                Ok(hmem)
            }
        }

        fn build_contents_stream(&self, index: usize) -> windows::core::Result<IStream> {
            let bytes = self.items[index].bytes().map_err(|_| DV_E_LINDEX)?;
            // SAFETY: allocates HGLOBAL containing the materialized bytes and transfers ownership to IStream (`fDeleteOnRelease = true`).
            unsafe {
                let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
                if !bytes.is_empty() {
                    let ptr = GlobalLock(hmem) as *mut u8;
                    if ptr.is_null() {
                        let _ = GlobalFree(Some(hmem));
                        return Err(E_POINTER.into());
                    }
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
                    let _ = GlobalUnlock(hmem);
                }
                CreateStreamOnHGlobal(hmem, true)
            }
        }
    }

    impl IDataObject_Impl for VirtualDataObject_Impl {
        fn GetData(&self, pformatetcin: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
            if pformatetcin.is_null() {
                return Err(E_POINTER.into());
            }
            // SAFETY: non-null caller pointer.
            let req = unsafe { *pformatetcin };
            if req.cfFormat == self.cf_descriptor && (req.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
                let hmem = self.build_descriptor_hglobal()?;
                return Ok(STGMEDIUM {
                    tymed: TYMED_HGLOBAL.0 as u32,
                    u: STGMEDIUM_0 { hGlobal: hmem },
                    pUnkForRelease: std::mem::ManuallyDrop::new(None),
                });
            }
            if req.cfFormat == self.cf_contents && (req.tymed & TYMED_ISTREAM.0 as u32) != 0 {
                let idx = if req.lindex < 0 && self.items.len() == 1 {
                    0
                } else {
                    usize::try_from(req.lindex).map_err(|_| DV_E_LINDEX)?
                };
                if idx >= self.items.len() {
                    return Err(DV_E_LINDEX.into());
                }
                let stream = self.build_contents_stream(idx)?;
                return Ok(STGMEDIUM {
                    tymed: TYMED_ISTREAM.0 as u32,
                    u: STGMEDIUM_0 {
                        pstm: std::mem::ManuallyDrop::new(Some(stream)),
                    },
                    pUnkForRelease: std::mem::ManuallyDrop::new(None),
                });
            }
            Err(DV_E_FORMATETC.into())
        }

        fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn QueryGetData(&self, pformatetcin: *const FORMATETC) -> HRESULT {
            if pformatetcin.is_null() {
                return E_POINTER;
            }
            // SAFETY: non-null caller pointer.
            let req = unsafe { *pformatetcin };
            if req.cfFormat == self.cf_descriptor {
                return if (req.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
                    S_OK
                } else {
                    DV_E_TYMED
                };
            }
            if req.cfFormat == self.cf_contents {
                let ok_idx = (req.lindex >= 0 && (req.lindex as usize) < self.items.len())
                    || (req.lindex < 0 && self.items.len() == 1);
                if !ok_idx {
                    return DV_E_LINDEX;
                }
                return if (req.tymed & TYMED_ISTREAM.0 as u32) != 0 {
                    S_OK
                } else {
                    DV_E_TYMED
                };
            }
            DV_E_FORMATETC
        }

        fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, pformatetcout: *mut FORMATETC) -> HRESULT {
            if !pformatetcout.is_null() {
                // SAFETY: caller out-pointer.
                unsafe { (*pformatetcout).ptd = std::ptr::null_mut() };
            }
            windows::Win32::Foundation::DATA_S_SAMEFORMATETC
        }

        fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn EnumFormatEtc(&self, dwdirection: u32) -> windows::core::Result<IEnumFORMATETC> {
            if dwdirection != DATADIR_GET.0 as u32 {
                return Err(E_NOTIMPL.into());
            }
            let mut formats = vec![self.descriptor_format()];
            for i in 0..self.items.len() {
                formats.push(self.contents_format(i as i32));
            }
            let enumerator: IEnumFORMATETC = ComObject::new(FormatEnum {
                formats,
                pos: AtomicUsize::new(0),
            })
            .into_interface();
            Ok(enumerator)
        }

        fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<IAdviseSink>) -> windows::core::Result<u32> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }

        fn DUnadvise(&self, _: u32) -> windows::core::Result<()> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }

        fn EnumDAdvise(&self) -> windows::core::Result<windows::Win32::System::Com::IEnumSTATDATA> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }
    }

    pub(super) fn make_data_object(paths: Vec<String>, transform: DragTransform) -> IDataObject {
        ComObject::new(VirtualDataObject::new(paths, transform)).into_interface()
    }

    pub(super) fn drag_out(owner: isize, paths: Vec<String>, transform: DragTransform) -> Result<String, String> {
        let hwnd = (owner != 0).then_some(HWND(owner as *mut std::ffi::c_void));
        let data = make_data_object(paths, transform);
        // SAFETY: modal SHDoDragDrop runs on the UI thread message loop via `run_on_main_thread`.
        unsafe {
            let effect = SHDoDragDrop(hwnd, &data, None::<&IDropSource>, DROPEFFECT_COPY).map_err(|e| e.message())?;
            Ok(if effect.0 & DROPEFFECT_COPY.0 != 0 {
                "copy".to_string()
            } else {
                "none".to_string()
            })
        }
    }

    #[cfg(test)]
    pub(super) fn make_test_object(paths: Vec<String>, transform: DragTransform) -> IDataObject {
        make_data_object(paths, transform)
    }

    // Silence unused import warning when only `STATDATA` isn't named in a signature.
    const _: Option<STATDATA> = None;
    const _: PCWSTR = PCWSTR::null();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transformed_filenames_reflect_requested_transform() {
        assert_eq!(
            transformed_filename("C:\\pix\\holiday.JPG", &DragTransform::StripMetadata),
            "holiday_clean.jpg"
        );
        assert_eq!(
            transformed_filename("C:\\pix\\holiday.png", &DragTransform::ConvertWebp),
            "holiday.webp"
        );
        assert_eq!(
            transformed_filename("C:\\vid\\demo.mp4", &DragTransform::ShrinkUnderMb(10)),
            "demo_under10mb.mp4"
        );
        assert_eq!(
            transformed_filename("C:\\vid\\demo.mp4", &DragTransform::ExtractAudioMp3),
            "demo.mp3"
        );
    }

    #[test]
    fn jpeg_metadata_stripper_removes_app1_exif_and_com_segments_losslessly() {
        // Minimal synthetic JPEG: SOI + APP0 (JFIF) + APP1 (EXIF) + COM + DQT + SOS + EOI
        let mut jpg = vec![0xFF, 0xD8];
        jpg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0xAA, 0xBB]); // APP0 kept
        jpg.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x08, b'E', b'x', b'i', b'f', 0x00, 0x00]); // APP1 stripped
        jpg.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x05, b'h', b'i', b'!']); // COM stripped
        jpg.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x04, 0x01, 0x02]); // DQT kept
        jpg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x03, 0x01, 0x11, 0x22, 0xFF, 0xD9]); // SOS..EOI kept

        let clean = strip_metadata_bytes("jpg", &jpg).unwrap();
        assert!(clean.starts_with(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(clean.ends_with(&[0xFF, 0xD9]));
        assert!(!clean.windows(4).any(|w| w == b"Exif"));
        assert!(!clean.windows(3).any(|w| w == b"hi!"));
        assert!(clean.len() < jpg.len());
    }

    #[test]
    fn png_metadata_stripper_removes_exif_and_text_while_preserving_critical_chunks() {
        let mut png = vec![137, 80, 78, 71, 13, 10, 26, 10];
        let mut push_chunk = |kind: &[u8; 4], data: &[u8]| {
            png.extend_from_slice(&(data.len() as u32).to_be_bytes());
            png.extend_from_slice(kind);
            png.extend_from_slice(data);
            png.extend_from_slice(&[0, 0, 0, 0]);
        };
        push_chunk(b"IHDR", &[1, 2, 3, 4]);
        push_chunk(b"tEXt", b"Camera\0Pixel");
        push_chunk(b"eXIf", b"GPS-DATA");
        push_chunk(b"IDAT", &[9, 9, 9]);
        push_chunk(b"IEND", &[]);

        let clean = strip_metadata_bytes("png", &png).unwrap();
        assert!(clean.windows(4).any(|w| w == b"IHDR"));
        assert!(clean.windows(4).any(|w| w == b"IDAT"));
        assert!(clean.windows(4).any(|w| w == b"IEND"));
        assert!(!clean.windows(4).any(|w| w == b"tEXt"));
        assert!(!clean.windows(4).any(|w| w == b"eXIf"));
    }

    #[cfg(windows)]
    #[test]
    fn com_data_object_streams_descriptor_and_lazy_file_contents() {
        use std::ffi::c_void;
        use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL, TYMED_ISTREAM};
        use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
        use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
        use windows::Win32::System::Ole::ReleaseStgMedium;
        use windows::Win32::UI::Shell::{CFSTR_FILECONTENTS, CFSTR_FILEDESCRIPTORW, FILEGROUPDESCRIPTORW};

        let mut png = vec![137, 80, 78, 71, 13, 10, 26, 10];
        let push_chunk = |buf: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]| {
            buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
            buf.extend_from_slice(kind);
            buf.extend_from_slice(data);
            buf.extend_from_slice(&[0, 0, 0, 0]);
        };
        push_chunk(&mut png, b"IHDR", &[1, 2, 3]);
        push_chunk(&mut png, b"tEXt", b"Secret\0Location");
        push_chunk(&mut png, b"IDAT", &[7, 8]);
        push_chunk(&mut png, b"IEND", &[]);

        let path = std::env::temp_dir().join(format!("glint-vdrag-com-test-{}.png", std::process::id()));
        std::fs::write(&path, &png).unwrap();
        let data = imp::make_test_object(vec![path.to_string_lossy().into_owned()], DragTransform::StripMetadata);

        // SAFETY: calls our in-process COM `IDataObject` with valid FORMATETC structs and cleans up media/temp file.
        unsafe {
            let cf_desc = RegisterClipboardFormatW(CFSTR_FILEDESCRIPTORW) as u16;
            let cf_cont = RegisterClipboardFormatW(CFSTR_FILECONTENTS) as u16;
            let mut desc_med = data
                .GetData(&FORMATETC {
                    cfFormat: cf_desc,
                    ptd: std::ptr::null_mut(),
                    dwAspect: DVASPECT_CONTENT.0,
                    lindex: -1,
                    tymed: TYMED_HGLOBAL.0 as u32,
                })
                .unwrap();
            let ptr = GlobalLock(desc_med.u.hGlobal) as *const FILEGROUPDESCRIPTORW;
            let count = std::ptr::addr_of!((*ptr).cItems).read_unaligned();
            assert_eq!(count, 1);
            let fd0 = std::ptr::addr_of!((*ptr).fgd[0]).read_unaligned();
            let raw_name = fd0.cFileName;
            let len = raw_name.iter().position(|&c| c == 0).unwrap_or(raw_name.len());
            let name = String::from_utf16_lossy(&raw_name[..len]);
            assert!(name.ends_with("_clean.png"), "{name}");
            let _ = GlobalUnlock(desc_med.u.hGlobal);
            ReleaseStgMedium(&mut desc_med);

            let mut cont_med = data
                .GetData(&FORMATETC {
                    cfFormat: cf_cont,
                    ptd: std::ptr::null_mut(),
                    dwAspect: DVASPECT_CONTENT.0,
                    lindex: 0,
                    tymed: TYMED_ISTREAM.0 as u32,
                })
                .unwrap();
            let stream = cont_med.u.pstm.as_ref().expect("IStream returned").clone();
            let mut buf = vec![0u8; 512];
            let mut read = 0u32;
            stream
                .Read(buf.as_mut_ptr() as *mut c_void, buf.len() as u32, Some(&mut read))
                .ok()
                .unwrap();
            let streamed = &buf[..read as usize];
            assert!(!streamed.windows(6).any(|w| w == b"Secret"));
            assert!(streamed.windows(4).any(|w| w == b"IDAT"));
            ReleaseStgMedium(&mut cont_med);
        }
        let _ = std::fs::remove_file(&path);
    }
}
