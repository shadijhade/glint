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

/// The pictures a transform can read: Clean EXIF works on JPEG and PNG bytes, the rest goes through FFmpeg.
fn is_clean_picture(ext: &str) -> bool {
    matches!(ext, "jpg" | "jpeg" | "png")
}

fn is_picture(ext: &str) -> bool {
    matches!(ext, "jpg" | "jpeg" | "png" | "bmp" | "gif" | "tif" | "tiff")
}

fn is_video(ext: &str) -> bool {
    matches!(ext, "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v")
}

/// Most files a drag-out offers a transform for: each is made when the drop reads it, one after another.
pub const MAX_TRANSFORMED: usize = 10;

/// The transforms a drag of `paths` can deliver, in the order the strip shows them. Pictures offer Clean EXIF
/// (JPEG and PNG only) and WebP; videos offer under 10 MB and the audio as MP3. Empty when the drag is not all
/// pictures or all videos (or is too long), and then the drag is the plain one.
pub fn transforms_for(paths: &[String]) -> Vec<DragTransform> {
    if paths.is_empty() || paths.len() > MAX_TRANSFORMED {
        return Vec::new();
    }
    let exts: Vec<String> = paths
        .iter()
        .map(|p| {
            Path::new(p)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default()
        })
        .collect();
    if exts.iter().all(|e| is_picture(e)) {
        let mut out = Vec::new();
        if exts.iter().all(|e| is_clean_picture(e)) {
            out.push(DragTransform::StripMetadata);
        }
        out.push(DragTransform::ConvertWebp);
        out
    } else if exts.iter().all(|e| is_video(e)) {
        vec![DragTransform::ShrinkUnderMb(10), DragTransform::ExtractAudioMp3]
    } else {
        Vec::new()
    }
}

/// Who is told when the choice changes.
type Listener = std::sync::Arc<dyn Fn(usize) + Send + Sync>;

/// What a drag-out delivers, chosen while the drag is under way. Option 0 is the files as they are; the
/// others are the transforms [`transforms_for`] offered. The drag's data object reads [`DragSwitch::selected`]
/// every time a target asks for data, so a change takes effect for the very next drop. Tab in the drag cycles
/// it ([`DragSwitch::cycle`]); the window's strip shows it and sets it by hovering ([`DragSwitch::select`]).
pub struct DragSwitch {
    options: Vec<Option<DragTransform>>,
    selected: std::sync::atomic::AtomicUsize,
    on_change: std::sync::Mutex<Option<Listener>>,
}

impl DragSwitch {
    /// A switch over `transforms`, on the plain files to begin with.
    pub fn new(transforms: Vec<DragTransform>) -> std::sync::Arc<Self> {
        let mut options = vec![None];
        options.extend(transforms.into_iter().map(Some));
        std::sync::Arc::new(Self {
            options,
            selected: std::sync::atomic::AtomicUsize::new(0),
            on_change: std::sync::Mutex::new(None),
        })
    }

    /// Every choice: the plain files (`None`), then the transforms.
    pub fn options(&self) -> &[Option<DragTransform>] {
        &self.options
    }

    /// The index of what a drop delivers now.
    pub fn selected(&self) -> usize {
        self.selected.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The transform a drop applies now; `None` for the files as they are.
    pub fn current(&self) -> Option<&DragTransform> {
        self.options.get(self.selected()).and_then(Option::as_ref)
    }

    /// Delivers option `index`. False (nothing changed) when there is no such option or it is the one chosen.
    pub fn select(&self, index: usize) -> bool {
        if index >= self.options.len() || self.selected.swap(index, std::sync::atomic::Ordering::Relaxed) == index {
            return false;
        }
        let listener = self.on_change.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(listener) = listener {
            listener(index);
        }
        true
    }

    /// The next choice (the previous one when `back`), round to the plain files again.
    pub fn cycle(&self, back: bool) {
        let n = self.options.len();
        let now = self.selected();
        self.select(if back { (now + n - 1) % n } else { (now + 1) % n });
    }

    /// Tells `listener` (from any thread) the index chosen whenever it changes.
    pub fn on_change(&self, listener: impl Fn(usize) + Send + Sync + 'static) {
        *self.on_change.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(listener));
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

/// Starts the drag-out of `paths` as the shell does it (the files themselves, with its drag card) and lets
/// `switch` change what a drop delivers while it runs: Tab cycles it (QueryContinueDrag reads the key, since
/// OLE's modal loop takes keyboard messages away from every window). Resolves like `shell::start_drag`:
/// "copy", "move", "link", "none", or "released" when the button was already up.
pub async fn start_switchable_drag(
    window: crate::host::Window,
    paths: Vec<String>,
    copy_only: bool,
    switch: std::sync::Arc<DragSwitch>,
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
                let _ = tx.send(imp::drag_switchable(owner, paths, copy_only, switch));
            })
            .map_err(|e| format!("Couldn't schedule drag on UI thread: {e}"))?;
        rx.await.map_err(|_| "Drag cancelled before starting".to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (owner, paths, copy_only, switch);
        Err("Dragging is only available on Windows".to_string())
    }
}

#[cfg(windows)]
mod imp {
    use super::{materialize_bytes, transformed_filename, DragSwitch, DragTransform};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, OnceLock};
    use windows::core::{implement, ComObject, Ref, BOOL, HRESULT, PCWSTR};
    use windows::Win32::Foundation::{
        GlobalFree, DATA_S_SAMEFORMATETC, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS,
        DV_E_FORMATETC, DV_E_LINDEX, DV_E_TYMED, E_NOTIMPL, E_POINTER, HGLOBAL, HWND, OLE_E_ADVISENOTSUPPORTED,
        S_FALSE, S_OK,
    };
    use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
    use windows::Win32::System::Com::{
        IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumFORMATETC_Impl, IStream, DATADIR_GET,
        DVASPECT_CONTENT, FORMATETC, STATDATA, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL, TYMED_ISTREAM,
    };
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT};
    use windows::Win32::System::Ole::{
        IDropSource, IDropSource_Impl, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE,
    };
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON, VK_TAB};
    use windows::Win32::UI::Shell::{
        SHDoDragDrop, CFSTR_FILECONTENTS, CFSTR_FILEDESCRIPTORW, CFSTR_FILENAMEA, CFSTR_FILENAMEW, CFSTR_SHELLIDLIST,
        FD_UNICODE, FILEDESCRIPTORW, FILEGROUPDESCRIPTORW,
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

    // ---- The drag whose delivery changes while it runs -------------------------------------------------

    /// The formats that name the dragged files themselves. While a transform is chosen they are hidden, so a
    /// target takes the transformed bytes (the file descriptor and contents) and not the originals.
    fn file_formats() -> [u16; 4] {
        // SAFETY: registering the shell's standard clipboard format names.
        unsafe {
            [
                CF_HDROP.0,
                RegisterClipboardFormatW(CFSTR_SHELLIDLIST) as u16,
                RegisterClipboardFormatW(CFSTR_FILENAMEW) as u16,
                RegisterClipboardFormatW(CFSTR_FILENAMEA) as u16,
            ]
        }
    }

    /// Every format `en` lists.
    fn collect_formats(en: &IEnumFORMATETC) -> Vec<FORMATETC> {
        let mut out = Vec::new();
        loop {
            let mut one = [FORMATETC::default()];
            let mut got = 0u32;
            // SAFETY: a one-element buffer and a fetched-count out-pointer, both valid for the call.
            let hr = unsafe { en.Next(&mut one, Some(&mut got as *mut u32)) };
            if hr != S_OK || got != 1 {
                break;
            }
            out.push(one[0]);
        }
        out
    }

    /// The shell's data object for the files (`inner`) and one virtual data object per transform. Every call
    /// goes to what `switch` has chosen now; the drag-image and drop-description formats the shell's helpers
    /// keep on the data go to `inner` whichever is chosen.
    #[implement(IDataObject)]
    struct SwitchData {
        inner: IDataObject,
        virtuals: Vec<IDataObject>,
        switch: Arc<DragSwitch>,
        blocked: [u16; 4],
    }

    impl SwitchData {
        fn virtual_now(&self) -> Option<&IDataObject> {
            self.switch.selected().checked_sub(1).and_then(|i| self.virtuals.get(i))
        }

        fn is_file_format(&self, cf: u16) -> bool {
            self.blocked.contains(&cf)
        }
    }

    impl IDataObject_Impl for SwitchData_Impl {
        fn GetData(&self, pformatetcin: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
            if pformatetcin.is_null() {
                return Err(E_POINTER.into());
            }
            // SAFETY: non-null caller pointer.
            let req = unsafe { *pformatetcin };
            let Some(chosen) = self.virtual_now() else {
                // SAFETY: the shell's own data object, asked as OLE asked us.
                return unsafe { self.inner.GetData(&req) };
            };
            // SAFETY: as above.
            match unsafe { chosen.GetData(&req) } {
                Err(e) if e.code() == DV_E_FORMATETC && !self.is_file_format(req.cfFormat) => {
                    // SAFETY: as above.
                    unsafe { self.inner.GetData(&req) }
                }
                other => other,
            }
        }

        fn GetDataHere(&self, pformatetc: *const FORMATETC, pmedium: *mut STGMEDIUM) -> windows::core::Result<()> {
            if pformatetc.is_null() || pmedium.is_null() {
                return Err(E_POINTER.into());
            }
            // SAFETY: non-null caller pointer.
            let req = unsafe { *pformatetc };
            if self.virtual_now().is_some() && self.is_file_format(req.cfFormat) {
                return Err(E_NOTIMPL.into());
            }
            // SAFETY: the pointers are the caller's own, passed straight on.
            unsafe { self.inner.GetDataHere(pformatetc, pmedium) }
        }

        fn QueryGetData(&self, pformatetcin: *const FORMATETC) -> HRESULT {
            if pformatetcin.is_null() {
                return E_POINTER;
            }
            // SAFETY: non-null caller pointer.
            let req = unsafe { *pformatetcin };
            let Some(chosen) = self.virtual_now() else {
                // SAFETY: the shell's own data object.
                return unsafe { self.inner.QueryGetData(&req) };
            };
            // SAFETY: as above.
            let hr = unsafe { chosen.QueryGetData(&req) };
            if hr == S_OK {
                S_OK
            } else if self.is_file_format(req.cfFormat) {
                DV_E_FORMATETC
            } else {
                // SAFETY: as above.
                unsafe { self.inner.QueryGetData(&req) }
            }
        }

        fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, pformatetcout: *mut FORMATETC) -> HRESULT {
            if !pformatetcout.is_null() {
                // SAFETY: caller out-pointer.
                unsafe { (*pformatetcout).ptd = std::ptr::null_mut() };
            }
            DATA_S_SAMEFORMATETC
        }

        fn SetData(
            &self,
            pformatetc: *const FORMATETC,
            pmedium: *const STGMEDIUM,
            frelease: BOOL,
        ) -> windows::core::Result<()> {
            if pformatetc.is_null() || pmedium.is_null() {
                return Err(E_POINTER.into());
            }
            // The drag image and the drop description the shell's helpers keep on the data live on its own object.
            // SAFETY: the caller's pointers, passed straight on.
            unsafe { self.inner.SetData(pformatetc, pmedium, frelease.as_bool()) }
        }

        fn EnumFormatEtc(&self, dwdirection: u32) -> windows::core::Result<IEnumFORMATETC> {
            // SAFETY: the shell's own data object.
            let plain = unsafe { self.inner.EnumFormatEtc(dwdirection) };
            let (Some(chosen), true) = (self.virtual_now(), dwdirection == DATADIR_GET.0 as u32) else {
                return plain;
            };
            // What the virtual files offer, then whatever else the shell's data carries but the file formats.
            let mut formats = Vec::new();
            // SAFETY: the virtual data object made here.
            if let Ok(en) = unsafe { chosen.EnumFormatEtc(dwdirection) } {
                formats.extend(collect_formats(&en));
            }
            if let Ok(en) = &plain {
                formats.extend(
                    collect_formats(en)
                        .into_iter()
                        .filter(|f| !self.is_file_format(f.cfFormat)),
                );
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

    /// The drag's drop source: it ends the drag as the default source does (Escape cancels, the button's release
    /// drops) and reads Tab for the switch. OLE's modal loop takes the keyboard messages away from every window and
    /// calls `QueryContinueDrag` on each key event and mouse move, so that call is where Tab can be read; the key
    /// state OLE passes holds only modifiers and buttons, so the key itself is asked of the system.
    #[implement(IDropSource)]
    struct Source {
        switch: Arc<DragSwitch>,
        tab_was_down: Cell<bool>,
    }

    /// MK_LBUTTON and MK_RBUTTON in the key state OLE passes.
    const MK_BUTTONS: u32 = 0x01 | 0x02;
    const MK_SHIFT: u32 = 0x04;

    impl IDropSource_Impl for Source_Impl {
        fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> HRESULT {
            if fescapepressed.as_bool() {
                return DRAGDROP_S_CANCEL;
            }
            if grfkeystate.0 & MK_BUTTONS == 0 {
                return DRAGDROP_S_DROP;
            }
            // SAFETY: reads a key's state; the low bit says it went down since the last call, which catches a tap
            // that was over before this call came.
            let state = unsafe { GetAsyncKeyState(VK_TAB.0 as i32) };
            let down = state < 0 || state & 1 != 0;
            if down && !self.tab_was_down.get() {
                self.switch.cycle(grfkeystate.0 & MK_SHIFT != 0);
            }
            self.tab_was_down.set(state < 0);
            S_OK
        }

        fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }

    /// Runs on the UI thread, like `shell::start_drag`: the shell's drag of `paths` that `switch` can turn into
    /// a transformed one.
    pub(super) fn drag_switchable(
        owner: isize,
        paths: Vec<String>,
        copy_only: bool,
        switch: Arc<DragSwitch>,
    ) -> Result<String, String> {
        // SAFETY: reading the mouse buttons' state.
        let held = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0 };
        if !held {
            // Released while this request was on its way: dragging now would drop at once, under the pointer.
            return Ok("released".into());
        }
        let inner = crate::shell::drag_data(&paths)?;
        let virtuals = switch
            .options()
            .iter()
            .flatten()
            .map(|transform| make_data_object(paths.clone(), transform.clone()))
            .collect();
        let data: IDataObject = ComObject::new(SwitchData {
            inner,
            virtuals,
            switch: switch.clone(),
            blocked: file_formats(),
        })
        .into_interface();
        let source: IDropSource = ComObject::new(Source {
            switch,
            tab_was_down: Cell::new(false),
        })
        .into_interface();
        let allowed = if copy_only {
            DROPEFFECT_COPY
        } else {
            DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK
        };
        let hwnd = (owner != 0).then_some(HWND(owner as *mut std::ffi::c_void));
        // SAFETY: modal SHDoDragDrop on the UI thread's message loop, started through `run_on_main_thread`.
        let effect = unsafe { SHDoDragDrop(hwnd, &data, Some(&source), allowed) }.map_err(|e| e.message())?;
        Ok(if effect.0 & DROPEFFECT_MOVE.0 != 0 {
            "move"
        } else if effect.0 & DROPEFFECT_COPY.0 != 0 {
            "copy"
        } else if effect.0 & DROPEFFECT_LINK.0 != 0 {
            "link"
        } else {
            "none"
        }
        .to_string())
    }

    /// Tests only: the switching data object over `inner`, as a drag would hold it.
    #[cfg(test)]
    pub(super) fn make_switch_object(inner: IDataObject, paths: Vec<String>, switch: Arc<DragSwitch>) -> IDataObject {
        let virtuals = switch
            .options()
            .iter()
            .flatten()
            .map(|t| make_data_object(paths.clone(), t.clone()))
            .collect();
        ComObject::new(SwitchData {
            inner,
            virtuals,
            switch,
            blocked: file_formats(),
        })
        .into_interface()
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

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn pictures_and_videos_offer_their_own_transforms() {
        assert_eq!(
            transforms_for(&paths(&["C:\\p\\a.JPG", "C:\\p\\b.png"])),
            vec![DragTransform::StripMetadata, DragTransform::ConvertWebp]
        );
        // A GIF can't be cleaned losslessly, but it can be a WebP.
        assert_eq!(
            transforms_for(&paths(&["C:\\p\\a.jpg", "C:\\p\\b.gif"])),
            vec![DragTransform::ConvertWebp]
        );
        assert_eq!(
            transforms_for(&paths(&["C:\\v\\clip.MP4"])),
            vec![DragTransform::ShrinkUnderMb(10), DragTransform::ExtractAudioMp3]
        );
        // Mixed, other kinds, nothing and too many offer none: the drag stays the plain one.
        assert!(transforms_for(&paths(&["C:\\p\\a.jpg", "C:\\v\\clip.mp4"])).is_empty());
        assert!(transforms_for(&paths(&["C:\\d\\notes.txt"])).is_empty());
        assert!(transforms_for(&[]).is_empty());
        let many: Vec<String> = (0..=MAX_TRANSFORMED).map(|i| format!("C:\\p\\{i}.jpg")).collect();
        assert!(transforms_for(&many).is_empty());
        // An Arabic name is no different.
        assert_eq!(transforms_for(&paths(&["C:\\p\\صورة.jpg"])).len(), 2);
    }

    #[test]
    fn the_switch_starts_plain_cycles_both_ways_and_tells_its_listener() {
        let switch = DragSwitch::new(vec![DragTransform::StripMetadata, DragTransform::ConvertWebp]);
        assert_eq!(
            (switch.selected(), switch.options().len(), switch.current()),
            (0, 3, None)
        );
        let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = heard.clone();
        switch.on_change(move |i| sink.lock().unwrap().push(i));
        switch.cycle(false);
        assert_eq!(switch.current(), Some(&DragTransform::StripMetadata));
        switch.cycle(false);
        switch.cycle(false);
        assert_eq!(switch.selected(), 0, "round to the plain files again");
        switch.cycle(true);
        assert_eq!(
            switch.current(),
            Some(&DragTransform::ConvertWebp),
            "Shift+Tab goes back"
        );
        // Choosing what is already chosen, or what isn't there, changes and tells nothing.
        assert!(!switch.select(2));
        assert!(!switch.select(9));
        assert!(switch.select(1));
        assert_eq!(*heard.lock().unwrap(), vec![1, 2, 0, 2, 1]);
    }

    #[cfg(windows)]
    #[test]
    fn the_data_object_follows_the_switch_between_the_files_and_the_transformed_copy() {
        use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL, TYMED_ISTREAM};
        use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
        use windows::Win32::System::Ole::{OleInitialize, CF_HDROP};
        use windows::Win32::UI::Shell::{CFSTR_FILECONTENTS, CFSTR_FILEDESCRIPTORW};

        // SAFETY: OLE on this test's thread, as the shell's data objects need.
        unsafe {
            let _ = OleInitialize(None);
        }
        let file = std::env::temp_dir().join(format!("glint-switch-test-{}.png", std::process::id()));
        std::fs::write(&file, b"png").unwrap();
        let path = file.to_string_lossy().into_owned();
        let inner = crate::shell::drag_data(std::slice::from_ref(&path)).expect("the shell's data object");
        let switch = DragSwitch::new(vec![DragTransform::StripMetadata]);
        let data = imp::make_switch_object(inner, vec![path], switch.clone());
        let format = |cf: u16, lindex: i32, tymed: i32| FORMATETC {
            cfFormat: cf,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex,
            tymed: tymed as u32,
        };
        // SAFETY: querying our own in-process data object with valid formats.
        unsafe {
            let descriptor = RegisterClipboardFormatW(CFSTR_FILEDESCRIPTORW) as u16;
            let contents = RegisterClipboardFormatW(CFSTR_FILECONTENTS) as u16;
            let has = |cf: u16, lindex: i32, tymed: i32| data.QueryGetData(&format(cf, lindex, tymed)).is_ok();
            let listed = || {
                let en = data.EnumFormatEtc(1).unwrap();
                let mut seen = Vec::new();
                let mut one = [FORMATETC::default()];
                let mut got = 0u32;
                while en.Next(&mut one, Some(&mut got as *mut u32)).0 == 0 && got == 1 {
                    seen.push(one[0].cfFormat);
                }
                seen
            };
            // Plain: the files, no virtual copy.
            assert!(has(CF_HDROP.0, -1, TYMED_HGLOBAL.0));
            assert!(listed().contains(&CF_HDROP.0));
            // Transformed: the copy, and the files hidden so a target can't take them instead.
            assert!(switch.select(1));
            assert!(!has(CF_HDROP.0, -1, TYMED_HGLOBAL.0));
            assert!(has(descriptor, -1, TYMED_HGLOBAL.0) && has(contents, 0, TYMED_ISTREAM.0));
            assert!(data.GetData(&format(CF_HDROP.0, -1, TYMED_HGLOBAL.0)).is_err());
            let seen = listed();
            assert!(seen.contains(&descriptor) && seen.contains(&contents) && !seen.contains(&CF_HDROP.0));
            // And back.
            assert!(switch.select(0));
            assert!(has(CF_HDROP.0, -1, TYMED_HGLOBAL.0));
        }
        let _ = std::fs::remove_file(&file);
    }
}
