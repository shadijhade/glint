//! Many small reads at once through a Windows 11 I/O ring: the slices asked for (a file's first and last
//! 4 KB, a photo's header) are queued by the hundred and the kernel works through them together, which
//! is what an NVMe drive needs to go at full speed: one thread reading one file after another leaves it
//! mostly idle. Windows 10 has no I/O rings: `read_many` says so (None), and callers read the usual way.
//!
//! The ring's functions are looked up when first needed, never linked: linking them makes Glint.exe import
//! api-ms-win-core-ioring-l1-1-0.dll, which Windows 10 doesn't have, and Windows then refuses to start Glint
//! at all (issue #6). The installer and MSIX scripts refuse to pack a Glint.exe that imports anything
//! Windows 10 lacks (scripts/lib/windows-imports.mjs), so the next function like these is caught before it ships.

use std::path::Path;

/// One file's slices: (offset, length) each.
pub struct Ask<'a> {
    pub path: &'a str,
    pub slices: Vec<(u64, u32)>,
}

/// What came back for one file: its identity (volume serial, file index, for telling hard links apart)
/// and each slice's bytes (shorter at the end of the file); None when it couldn't be opened.
pub struct Got {
    pub id: Option<(u32, u64)>,
    pub slices: Vec<Vec<u8>>,
}

/// Files opened (and reads queued) at a time.
const BATCH: usize = 128;

/// Reads every file's slices through an I/O ring; None where Windows has none.
pub fn read_many(asks: &[Ask]) -> Option<Vec<Option<Got>>> {
    read_many_opts(asks, true)
}

/// Reads every file's slices through a thread-reused I/O ring; skips `GetFileInformationByHandle` when `!want_id`.
#[cfg(windows)]
pub fn read_many_opts(asks: &[Ask], want_id: bool) -> Option<Vec<Option<Got>>> {
    use std::cell::RefCell;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, S_OK};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, HIORING, IORING_BUFFER_REF,
        IORING_BUFFER_REF_0, IORING_CQE, IORING_CREATE_ADVISORY_FLAGS_NONE, IORING_CREATE_FLAGS, IORING_CREATE_REQUIRED_FLAGS_NONE, IORING_HANDLE_REF, IORING_HANDLE_REF_0,
        IORING_REF_RAW, IORING_VERSION_3, IOSQE_FLAGS_NONE, OPEN_EXISTING,
    };
    use windows::core::PCWSTR;

    struct CachedRing {
        ring: HIORING,
        queue: u32,
        close: ring_fns::Close,
    }
    impl Drop for CachedRing {
        fn drop(&mut self) {
            unsafe {
                let _ = (self.close)(self.ring);
            }
        }
    }
    thread_local! {
        static RING: RefCell<Option<CachedRing>> = const { RefCell::new(None) };
    }

    let api = ring_api()?;
    let cached = RING.with(|slot| slot.borrow_mut().take()).or_else(|| {
        let queue = (BATCH * 2).next_power_of_two() as u32;
        let flags = IORING_CREATE_FLAGS {
            Required: IORING_CREATE_REQUIRED_FLAGS_NONE,
            Advisory: IORING_CREATE_ADVISORY_FLAGS_NONE,
        };
        let mut ring = HIORING::default();
        if unsafe { (api.create)(IORING_VERSION_3, flags, queue, queue * 2, &mut ring) }.is_err() {
            return None;
        }
        Some(CachedRing { ring, queue, close: api.close })
    })?;

    let ring = cached.ring;
    let queue = cached.queue;
    let mut ring_healthy = true;
    let mut out: Vec<Option<Got>> = Vec::with_capacity(asks.len());
    for batch in asks.chunks(BATCH) {
        // Open them all, then queue every slice of every one.
        let handles: Vec<Option<HANDLE>> = batch
            .iter()
            .map(|ask| {
                let wide: Vec<u16> = Path::new(ask.path).as_os_str().encode_wide_null();
                unsafe { CreateFileW(PCWSTR(wide.as_ptr()), GENERIC_READ.0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, None) }.ok()
            })
            .collect();
        let mut buffers: Vec<Vec<Vec<u8>>> = batch.iter().map(|ask| ask.slices.iter().map(|&(_, len)| vec![0u8; len as usize]).collect()).collect();
        let mut queued = 0u32;
        let mut failed = vec![false; batch.len()];
        for (f, ask) in batch.iter().enumerate() {
            let Some(handle) = handles[f] else { continue };
            for (s, &(offset, len)) in ask.slices.iter().enumerate() {
                let file = IORING_HANDLE_REF { Kind: IORING_REF_RAW, Handle: IORING_HANDLE_REF_0 { Handle: handle } };
                let data = IORING_BUFFER_REF { Kind: IORING_REF_RAW, Buffer: IORING_BUFFER_REF_0 { Address: buffers[f][s].as_mut_ptr().cast() } };
                // Which file and slice, packed in the entry's user data.
                let tag = (f << 8) | s;
                if unsafe { (api.build_read)(ring, file, data, len, offset, tag, IOSQE_FLAGS_NONE) }.is_err() {
                    failed[f] = true;
                    continue;
                }
                queued += 1;
                if queued >= queue {
                    // The queue is full: let it drain before queueing more.
                    let _ = unsafe { (api.submit)(ring, queued, u32::MAX, std::ptr::null_mut()) };
                    drain(api, ring, &mut buffers, &mut failed, &mut queued);
                }
            }
        }
        if queued > 0 {
            let _ = unsafe { (api.submit)(ring, queued, u32::MAX, std::ptr::null_mut()) };
            drain(api, ring, &mut buffers, &mut failed, &mut queued);
        }
        if queued > 0 {
            ring_healthy = false;
        }
        for (f, handle) in handles.into_iter().enumerate() {
            let got = handle.filter(|_| !failed[f]).map(|handle| {
                let id = if want_id {
                    let mut info = BY_HANDLE_FILE_INFORMATION::default();
                    unsafe { GetFileInformationByHandle(handle, &mut info) }
                        .ok()
                        .map(|_| (info.dwVolumeSerialNumber, (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64))
                        .filter(|&(_, index)| index != 0)
                } else {
                    None
                };
                Got { id, slices: std::mem::take(&mut buffers[f]) }
            });
            if let Some(handle) = handle {
                unsafe {
                    let _ = CloseHandle(handle);
                }
            }
            out.push(got);
        }
    }
    if ring_healthy {
        RING.with(|slot| *slot.borrow_mut() = Some(cached));
    }
    return Some(out);

    /// Takes every completion there is, trimming each buffer to what was read (an error empties it).
    fn drain(api: &RingApi, ring: HIORING, buffers: &mut [Vec<Vec<u8>>], failed: &mut [bool], queued: &mut u32) {
        let mut cqe = IORING_CQE::default();
        while *queued > 0 && unsafe { (api.pop)(ring, &mut cqe) } == S_OK {
            *queued -= 1;
            let (f, s) = (cqe.UserData >> 8, cqe.UserData & 0xFF);
            if let Some(buffer) = buffers.get_mut(f).and_then(|b| b.get_mut(s)) {
                if cqe.ResultCode.is_ok() {
                    buffer.truncate(cqe.Information);
                } else {
                    buffer.clear();
                    failed[f] = true;
                }
            }
        }
    }
}

/// The I/O ring's functions (Windows 11 and later), as ioringapi.h declares them.
#[cfg(windows)]
mod ring_fns {
    use windows::core::HRESULT;
    use windows::Win32::Storage::FileSystem::{HIORING, IORING_BUFFER_REF, IORING_CQE, IORING_CREATE_FLAGS, IORING_HANDLE_REF, IORING_SQE_FLAGS, IORING_VERSION};
    pub type Create = unsafe extern "system" fn(IORING_VERSION, IORING_CREATE_FLAGS, u32, u32, *mut HIORING) -> HRESULT;
    pub type BuildRead = unsafe extern "system" fn(HIORING, IORING_HANDLE_REF, IORING_BUFFER_REF, u32, u64, usize, IORING_SQE_FLAGS) -> HRESULT;
    pub type Submit = unsafe extern "system" fn(HIORING, u32, u32, *mut u32) -> HRESULT;
    pub type Pop = unsafe extern "system" fn(HIORING, *mut IORING_CQE) -> HRESULT;
    pub type Close = unsafe extern "system" fn(HIORING) -> HRESULT;
    pub type Found = unsafe extern "system" fn() -> isize;
}

#[cfg(windows)]
struct RingApi {
    create: ring_fns::Create,
    build_read: ring_fns::BuildRead,
    submit: ring_fns::Submit,
    pop: ring_fns::Pop,
    close: ring_fns::Close,
}

/// The I/O ring's functions, found once; None on Windows 10, which has none.
#[cfg(windows)]
fn ring_api() -> Option<&'static RingApi> {
    static API: std::sync::LazyLock<Option<RingApi>> = std::sync::LazyLock::new(|| unsafe {
        use ring_fns::*;
        use std::mem::transmute;
        use windows::core::{s, w};
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
        let dll = LoadLibraryExW(w!("api-ms-win-core-ioring-l1-1-0.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
        Some(RingApi {
            create: transmute::<Found, Create>(GetProcAddress(dll, s!("CreateIoRing"))?),
            build_read: transmute::<Found, BuildRead>(GetProcAddress(dll, s!("BuildIoRingReadFile"))?),
            submit: transmute::<Found, Submit>(GetProcAddress(dll, s!("SubmitIoRing"))?),
            pop: transmute::<Found, Pop>(GetProcAddress(dll, s!("PopIoRingCompletion"))?),
            close: transmute::<Found, Close>(GetProcAddress(dll, s!("CloseIoRing"))?),
        })
    });
    API.as_ref()
}

#[cfg(not(windows))]
pub fn read_many_opts(_asks: &[Ask], _want_id: bool) -> Option<Vec<Option<Got>>> {
    None
}

#[cfg(windows)]
trait EncodeWideNull {
    fn encode_wide_null(&self) -> Vec<u16>;
}

#[cfg(windows)]
impl EncodeWideNull for std::ffi::OsStr {
    fn encode_wide_null(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain([0]).collect()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn reads_many_files_slices_at_once() {
        let dir = std::env::temp_dir().join(format!("glint-ioring-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut paths = Vec::new();
        for i in 0..300u32 {
            let path = dir.join(format!("{i}.bin"));
            let bytes: Vec<u8> = (0..10_000u32).map(|b| (b.wrapping_mul(7) ^ i) as u8).collect();
            std::fs::write(&path, &bytes).unwrap();
            paths.push((path.to_string_lossy().into_owned(), bytes));
        }
        let asks: Vec<Ask> = paths.iter().map(|(p, _)| Ask { path: p, slices: vec![(0, 4096), (10_000 - 4096, 4096), (9_990, 4096)] }).collect();
        let mut missing = asks.iter().map(|a| Ask { path: a.path, slices: a.slices.clone() }).collect::<Vec<_>>();
        missing.push(Ask { path: r"C:\nowhere\at\all.bin", slices: vec![(0, 16)] });
        // Windows 11 (build 22000) brought I/O rings: there, finding none means the lookup is broken.
        let Some(got) = read_many(&missing) else {
            assert!(windows_build() < 22_000, "Windows build {} has I/O rings, but none was found", windows_build());
            return;
        };
        for ((_, bytes), got) in paths.iter().zip(&got) {
            let got = got.as_ref().expect("opened");
            assert_eq!(got.slices[0], bytes[..4096]);
            assert_eq!(got.slices[1], bytes[10_000 - 4096..]);
            assert_eq!(got.slices[2], bytes[9_990..], "short at the end of the file");
            assert!(got.id.is_some());
        }
        assert!(got.last().unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// This Windows' build number, from `ver` ("Microsoft Windows [Version 10.0.26100.1]").
    fn windows_build() -> u32 {
        let out = std::process::Command::new("cmd").args(["/c", "ver"]).output().expect("ver");
        let text = String::from_utf8_lossy(&out.stdout);
        let version = text.rsplit("Version ").next().unwrap_or_default();
        version.split('.').nth(2).and_then(|b| b.trim_end_matches(|c: char| !c.is_ascii_digit()).parse().ok()).expect("a build number")
    }
}
