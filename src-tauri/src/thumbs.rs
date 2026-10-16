//! Thumbnails for the grid view (and the list's icons), from Windows' own thumbnail cache: the one
//! Explorer fills, read through IShellItemImageFactory. Only what's on screen is asked for, and it's
//! answered in two steps, so a slow file never holds up a fast one:
//!
//! 1. Lookups: the thumbnail as Windows has it cached (SIIGBF_INCACHEONLY), about a millisecond each.
//! 2. Making: for those Windows hasn't seen yet (or has only at a smaller size), the file type's
//!    thumbnail handler decodes the file (20–35 ms for a photo), on lower-priority threads; Windows
//!    caches the result for next time.
//!
//! Both run on worker threads of their own (each a COM apartment), never on the UI thread or the async
//! runtime. Requests wait in their tab's lane and are dropped when that tab scrolls them out of view or
//! moves on, and every thumbnail goes back to the webview as one binary buffer, kept in memory here too.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Once};
use tauri::async_runtime::{channel, Sender};
use tauri::ipc::Response;
use tauri::State;

/// What an answer holds (its first byte).
const FOUND: u8 = 0;
const NONE: u8 = 1;
/// Where a thumbnail came from (its second byte).
const FROM_MEMORY: u8 = 0;
const FROM_CACHE: u8 = 1;
const MADE: u8 = 2;

/// Thumbnails kept here once made: about 800 tiles' worth at the grid's medium size and 100 at its
/// biggest, some ten screenfuls either way (bigger tiles, fewer on screen). The UI keeps the latest ones
/// too, and Windows' cache has them all a few milliseconds away.
const MEMORY_BUDGET: usize = 32 << 20;

/// The sizes Windows' thumbnail cache keeps thumbnails at (thumbcache_16.db … thumbcache_2560.db).
const CACHE_SIZES: [u32; 9] = [16, 32, 48, 96, 256, 768, 1280, 1920, 2560];

/// Whether a lookup's answer is a smaller size Windows has cached in place of the one asked for. It
/// answers from whichever size it has, the list's 32 px icon for a tile, say, and those come at exactly
/// their cache size, so a picture that's small itself (it comes at its own size) isn't taken for one.
fn from_smaller_size(width: u32, height: u32, asked: u32) -> bool {
    let long = width.max(height);
    long < asked && CACHE_SIZES.contains(&long)
}

/// A thumbnail of a file as it is now (a changed file gets a new one), at one size.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct Key {
    /// Lowercase: Windows paths ignore case.
    path: String,
    size: u32,
    modified: i64,
    bytes: u64,
}

/// An answer: a BMP, or that the file has none.
#[derive(Debug)]
struct Thumb {
    found: bool,
    /// For none: that the file has none for good, rather than none yet (an online-only file Windows
    /// has no thumbnail of, or a handler that timed out): only those are remembered.
    lasting: bool,
    source: u8,
    /// Milliseconds the shell spent on it.
    ms: f32,
    bmp: Vec<u8>,
}

impl Thumb {
    fn none(lasting: bool) -> Arc<Thumb> {
        Arc::new(Thumb { found: false, lasting, source: FROM_CACHE, ms: 0.0, bmp: Vec::new() })
    }

    /// The answer as the UI reads it: an 8-byte header (u8 FOUND or NONE, u8 where it came from, u8 1
    /// when it's for good, u8 0, f32 milliseconds the shell took), then the BMP.
    fn encode(&self, source: u8) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.bmp.len());
        out.extend([if self.found { FOUND } else { NONE }, source, self.lasting as u8, 0]);
        out.extend(self.ms.to_le_bytes());
        out.extend(&self.bmp);
        out
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Lookup,
    Make,
}

/// Someone waiting for a thumbnail: a tab, which can drop the requests it no longer needs.
struct Waiter<R> {
    lane: String,
    /// The UI numbers its requests, so dropping the old ones never drops one made since.
    seq: u32,
    reply: R,
}

struct Job<R> {
    path: String,
    size: u32,
    /// Every waiter's file is online-only: making a thumbnail would download it, so only Windows'
    /// cache is asked.
    cached_only: bool,
    phase: Phase,
    running: bool,
    waiters: Vec<Waiter<R>>,
    /// The smaller one Windows has cached, while a thumbnail at the size asked is made: the answer if
    /// that fails.
    smaller: Option<Thumb>,
}

/// What a worker found.
#[derive(Debug)]
enum Outcome {
    Found { bmp: Vec<u8>, ms: f32 },
    /// Windows doesn't have it cached (only from a lookup).
    NotCached,
    /// Windows has it cached only at a smaller size (only from a lookup; see `from_smaller_size`).
    Smaller { bmp: Vec<u8>, ms: f32 },
    /// No thumbnail; `lasting` unless it may work next time (a timeout, a busy surrogate).
    Nothing { lasting: bool },
}

/// The next thing for a worker to do.
#[derive(Debug, PartialEq)]
struct Task {
    key: Key,
    path: String,
    size: u32,
    phase: Phase,
}

/// Thumbnails kept in memory, the least recently used dropped first once over budget.
struct Memory {
    map: HashMap<Key, (Arc<Thumb>, u64)>,
    order: BTreeMap<u64, Key>,
    tick: u64,
    bytes: usize,
    budget: usize,
}

impl Memory {
    fn new(budget: usize) -> Self {
        Memory { map: HashMap::new(), order: BTreeMap::new(), tick: 0, bytes: 0, budget }
    }

    fn cost(thumb: &Thumb) -> usize {
        thumb.bmp.len() + 128
    }

    fn get(&mut self, key: &Key) -> Option<Arc<Thumb>> {
        let (thumb, tick) = self.map.get_mut(key)?;
        self.order.remove(tick);
        self.tick += 1;
        *tick = self.tick;
        self.order.insert(self.tick, key.clone());
        Some(thumb.clone())
    }

    fn put(&mut self, key: Key, thumb: Arc<Thumb>) {
        self.tick += 1;
        self.bytes += Self::cost(&thumb);
        if let Some((old, tick)) = self.map.insert(key.clone(), (thumb, self.tick)) {
            self.bytes -= Self::cost(&old);
            self.order.remove(&tick);
        }
        self.order.insert(self.tick, key);
        while self.bytes > self.budget {
            let Some((_, oldest)) = self.order.pop_first() else { break };
            if let Some((gone, _)) = self.map.remove(&oldest) {
                self.bytes -= Self::cost(&gone);
            }
        }
    }
}

/// The requests waiting or being worked on, and the thumbnails made. `R` is how a waiter is answered.
struct Queue<R> {
    jobs: HashMap<Key, Job<R>>,
    lookups: VecDeque<Key>,
    makes: VecDeque<Key>,
    memory: Memory,
}

impl<R> Queue<R> {
    fn new(budget: usize) -> Self {
        Queue { jobs: HashMap::new(), lookups: VecDeque::new(), makes: VecDeque::new(), memory: Memory::new(budget) }
    }

    /// Adds a waiter for `key`: to the job for it if there's one (several panes can show one
    /// folder), otherwise as a new lookup.
    fn want(&mut self, key: Key, path: String, cached_only: bool, waiter: Waiter<R>) {
        if let Some(job) = self.jobs.get_mut(&key) {
            job.cached_only &= cached_only;
            job.waiters.push(waiter);
            return;
        }
        let size = key.size;
        self.lookups.push_back(key.clone());
        self.jobs.insert(key, Job { path, size, cached_only, phase: Phase::Lookup, running: false, waiters: vec![waiter], smaller: None });
    }

    /// The oldest lookup, or for a thread that makes thumbnails (`makes`), the oldest one to make
    /// once no lookup waits. Requests dropped meanwhile are skipped.
    fn next(&mut self, makes: bool) -> Option<Task> {
        loop {
            let (key, phase) = match self.lookups.pop_front() {
                Some(key) => (key, Phase::Lookup),
                None if makes => (self.makes.pop_front()?, Phase::Make),
                None => return None,
            };
            if let Some(job) = self.jobs.get_mut(&key).filter(|job| !job.running && job.phase == phase) {
                job.running = true;
                return Some(Task { path: job.path.clone(), size: job.size, key, phase });
            }
        }
    }

    /// Records what a worker found for `key`. Returns the answer and who gets it, or None when the job
    /// goes on to be made (or nobody waits any more).
    fn finish(&mut self, key: &Key, outcome: Outcome) -> Option<(Arc<Thumb>, Vec<R>)> {
        let job = self.jobs.get_mut(key)?;
        job.running = false;
        if matches!(outcome, Outcome::NotCached | Outcome::Smaller { .. }) && !job.cached_only && !job.waiters.is_empty() {
            if let Outcome::Smaller { bmp, ms } = outcome {
                job.smaller = Some(Thumb { found: true, lasting: true, source: FROM_CACHE, ms, bmp });
            }
            job.phase = Phase::Make;
            self.makes.push_back(key.clone());
            return None;
        }
        let thumb = match outcome {
            Outcome::Found { bmp, ms } => {
                let source = if job.phase == Phase::Lookup { FROM_CACHE } else { MADE };
                let thumb = Arc::new(Thumb { found: true, lasting: true, source, ms, bmp });
                self.memory.put(key.clone(), thumb.clone());
                thumb
            }
            // Online-only files stay as they are until Windows has a thumbnail of them (making one would
            // download the file), at whatever size it has; that's not kept for once they're downloaded.
            Outcome::Smaller { bmp, ms } => Arc::new(Thumb { found: true, lasting: true, source: FROM_CACHE, ms, bmp }),
            Outcome::NotCached => Thumb::none(false),
            // Making a bigger one failed: the smaller one Windows has will do (kept unless it may work later).
            Outcome::Nothing { lasting } => match job.smaller.take() {
                Some(smaller) => {
                    let thumb = Arc::new(smaller);
                    if lasting {
                        self.memory.put(key.clone(), thumb.clone());
                    }
                    thumb
                }
                None => {
                    let none = Thumb::none(lasting);
                    if lasting {
                        self.memory.put(key.clone(), none.clone());
                    }
                    none
                }
            },
        };
        let job = self.jobs.remove(key)?;
        Some((thumb, job.waiters.into_iter().map(|w| w.reply).collect()))
    }

    /// Drops the waits of `lane` for `paths` (lowercase; all of its waits when None), up to request
    /// number `up_to`. Jobs nobody waits for any more go, unless a worker has them already (they're
    /// finished and kept for next time). Returns the replies dropped.
    fn cancel(&mut self, lane: &str, paths: Option<&HashSet<String>>, up_to: u32) -> Vec<R> {
        let mut dropped = Vec::new();
        self.jobs.retain(|key, job| {
            if paths.is_some_and(|p| !p.contains(&key.path)) {
                return true;
            }
            let mut kept = Vec::with_capacity(job.waiters.len());
            for waiter in job.waiters.drain(..) {
                if waiter.lane == lane && waiter.seq <= up_to {
                    dropped.push(waiter.reply);
                } else {
                    kept.push(waiter);
                }
            }
            job.waiters = kept;
            job.running || !job.waiters.is_empty()
        });
        dropped
    }
}

type Reply = Sender<Arc<Thumb>>;

struct Shared {
    queue: Mutex<Queue<Reply>>,
    work: Condvar,
    started: Once,
}

/// The thumbnail service: its queue, memory and worker threads.
pub struct Thumbs(Arc<Shared>);

impl Default for Thumbs {
    fn default() -> Self {
        Thumbs(Arc::new(Shared { queue: Mutex::new(Queue::new(MEMORY_BUDGET)), work: Condvar::new(), started: Once::new() }))
    }
}

impl Thumbs {
    /// Starts the worker threads a moment after the app opens (once its first folder is up), so the
    /// first thumbnails don't wait for the shell to load its thumbnail cache: about 50 ms, once.
    pub fn warm_up(&self) {
        #[cfg(windows)]
        {
            let shared = self.0.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(1));
                start(&shared);
            });
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue<Reply>> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Threads that only look thumbnails up (always free for the next screenful), and threads that make
/// them, at lower priority, and look up too when there's nothing to make.
fn workers() -> (usize, usize) {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    (2, (cores / 2).clamp(2, 8))
}

#[cfg(windows)]
fn start(shared: &Arc<Shared>) {
    shared.started.call_once(|| {
        let (lookups, makers) = workers();
        for n in 0..lookups + makers {
            let shared = shared.clone();
            let makes = n >= lookups;
            let _ = std::thread::Builder::new().name(format!("thumbs-{n}")).spawn(move || work(&shared, makes));
        }
    });
}

#[cfg(windows)]
fn work(shared: &Shared, makes: bool) {
    win::init_thread(makes);
    win::warm_up();
    loop {
        let task = {
            let mut queue = shared.lock();
            loop {
                if let Some(task) = queue.next(makes) {
                    break task;
                }
                queue = shared.work.wait(queue).unwrap_or_else(|e| e.into_inner());
            }
        };
        // A bug here mustn't take the thread down with its job left running (its tiles would wait for good).
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| win::outcome(&task.path, task.size, task.phase == Phase::Lookup)))
            .unwrap_or(Outcome::Nothing { lasting: false });
        let answer = shared.lock().finish(&task.key, outcome);
        match answer {
            Some((thumb, replies)) => {
                for reply in replies {
                    let _ = reply.try_send(thumb.clone());
                }
            }
            // On to a thread that makes thumbnails.
            None => shared.work.notify_all(),
        }
    }
}

/// One item's thumbnail at `size` device pixels (the UI asks for a few steps, so sizes are shared),
/// as binary: see `Thumb::encode`. `modified` and `bytes` are the file's, from its listing, so a file
/// that changed isn't answered from memory. Waits in `lane` (the tab asking) as request `seq`, until
/// it's answered or dropped ("cancelled").
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn thumbnail(state: State<'_, Thumbs>, path: String, size: u32, modified: f64, bytes: f64, cached_only: bool, lane: String, seq: u32) -> Result<Response, String> {
    let key = Key { path: path.to_lowercase(), size: size.clamp(16, 1024), modified: modified as i64, bytes: bytes as u64 };
    if !cfg!(windows) {
        return Ok(Response::new(Thumb::none(true).encode(FROM_CACHE)));
    }
    // A file in an archive: the thumbnail is made of a temp copy of it (extracted once, within a
    // budget); one that can't be extracted has none.
    let asked = path.clone();
    let path = match crate::fsx::blocking(move || Ok(crate::archive::thumbnail_source(&asked))).await? {
        None => path,
        Some(Ok(copy)) => copy,
        Some(Err(_)) => return Ok(Response::new(Thumb::none(true).encode(FROM_CACHE))),
    };
    let shared = &state.0;
    let mut answer = {
        let mut queue = shared.lock();
        if let Some(thumb) = queue.memory.get(&key) {
            return Ok(Response::new(thumb.encode(FROM_MEMORY)));
        }
        let (reply, answer) = channel(1);
        queue.want(key, path, cached_only, Waiter { lane, seq, reply });
        answer
    };
    #[cfg(windows)]
    start(shared);
    shared.work.notify_one();
    match answer.recv().await {
        Some(thumb) => Ok(Response::new(thumb.encode(thumb.source))),
        None => Err("cancelled".into()),
    }
}

/// Drops `lane`'s requests up to number `up_to` for `paths` (they scrolled out of view), or all of
/// them (its folder changed, or the tab closed).
#[tauri::command]
pub async fn cancel_thumbnails(state: State<'_, Thumbs>, lane: String, paths: Option<Vec<String>>, up_to: u32) -> Result<(), String> {
    let paths: Option<HashSet<String>> = paths.map(|p| p.iter().map(|p| p.to_lowercase()).collect());
    // Dropping the replies answers their requests with "cancelled".
    let dropped = state.0.lock().cancel(&lane, paths.as_ref(), up_to);
    drop(dropped);
    Ok(())
}

/// A thumbnail's pixels: top-down rows of straight-alpha BGRA.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// A 32-bit BMP of `pixels` (BITMAPV4HEADER with an alpha mask, top-down), which the webview shows as
/// it is: no compression to spend time on, and nothing to decode but a header.
pub fn encode_bmp(pixels: &Pixels) -> Vec<u8> {
    const HEADERS: u32 = 14 + 108;
    let image = pixels.width * pixels.height * 4;
    let mut out = Vec::with_capacity((HEADERS + image) as usize);
    out.extend(b"BM");
    out.extend((HEADERS + image).to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(HEADERS.to_le_bytes());
    out.extend(108u32.to_le_bytes());
    out.extend((pixels.width as i32).to_le_bytes());
    // Negative: rows run top to bottom.
    out.extend((-(pixels.height as i32)).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    // BI_BITFIELDS, so the masks below (alpha included) are what counts.
    out.extend(3u32.to_le_bytes());
    out.extend(image.to_le_bytes());
    out.extend(2835i32.to_le_bytes());
    out.extend(2835i32.to_le_bytes());
    out.extend([0u8; 8]);
    for mask in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000] {
        out.extend(mask.to_le_bytes());
    }
    // LCS_sRGB, then endpoints and gamma, unused for it.
    out.extend(0x7352_4742u32.to_le_bytes());
    out.extend([0u8; 48]);
    out.extend(&pixels.bgra);
    out
}

/// The shell hands out premultiplied alpha (what AlphaBlend wants), and some thumbnails, JPEGs' among
/// them, with alpha 0 everywhere. Turns either into what a browser shows: straight alpha, opaque when
/// the bitmap has no alpha at all.
pub fn straighten_alpha(bgra: &mut [u8]) {
    let pixels = bgra.as_chunks_mut::<4>().0;
    if pixels.iter().all(|p| p[3] == 0) {
        pixels.iter_mut().for_each(|p| p[3] = 255);
        return;
    }
    // Premultiplied colors never exceed their alpha; a bitmap where one does has straight alpha already.
    if pixels.iter().any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]) {
        return;
    }
    for p in pixels.iter_mut() {
        let a = p[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

#[cfg(windows)]
mod win {
    use super::{encode_bmp, from_smaller_size, straighten_alpha, Outcome, Pixels};
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;
    use windows::core::{HRESULT, HSTRING, PWSTR};
    use windows::Win32::Foundation::{E_FAIL, SIZE};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IBindCtx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    use windows::Win32::UI::Shell::{
        AssocQueryStringW, IShellItem, IShellItemImageFactory, ISharedBitmap, IThumbnailCache, LocalThumbnailCache, SHCreateItemFromParsingName,
        ASSOCF_NONE, ASSOCSTR_SHELLEXTENSION, SIIGBF_INCACHEONLY, SIIGBF_THUMBNAILONLY, WTSAT_RGB, WTS_EXTRACTINPROC, WTS_SCALETOREQUESTEDSIZE,
    };

    /// Not in the thumbnail cache (STG_E_FILENOTFOUND), or being made elsewhere (WTS_E_EXTRACTIONPENDING,
    /// E_PENDING).
    const NOT_CACHED: [HRESULT; 3] = [HRESULT(0x8003_0002_u32 as i32), HRESULT(0x8004_B205_u32 as i32), HRESULT(0x8000_000A_u32 as i32)];
    /// Worth trying again later: the handler timed out (WTS_E_EXTRACTIONTIMEDOUT) or its surrogate
    /// process wasn't there (WTS_E_SURROGATEUNAVAILABLE).
    const TRANSIENT: [HRESULT; 2] = [HRESULT(0x8004_B201_u32 as i32), HRESULT(0x8004_B202_u32 as i32)];
    /// IThumbnailProvider, the key a file type's thumbnail handler is registered under.
    const THUMBNAIL_PROVIDER: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
    /// Windows' own Photo Thumbnail Provider (PhotoMetadataHandler.dll, decoding with WIC): JPEG, GIF,
    /// TIFF, WebP, HEIC, camera RAW…
    const PHOTO_PROVIDER: &str = "{C7657C4A-9F68-40fa-A4DF-96BC08EB3551}";
    /// What Windows' own codecs decode. WebP, HEIC, AVIF and RAW decoders are add-ons (from the Store,
    /// or a camera maker or codec pack), so those stay in the isolated process.
    const BUILT_IN: [&str; 14] = [".jpg", ".jpeg", ".jpe", ".jfif", ".png", ".gif", ".tif", ".tiff", ".bmp", ".dib", ".ico", ".jxr", ".wdp", ".hdp"];

    /// Each worker thread is a single-threaded COM apartment, as thumbnail handlers expect; the ones
    /// that decode files run below normal priority, so the window stays responsive. They wait for work
    /// without a message loop, which an apartment can do when nothing calls into it from outside: every
    /// object here is made, used and let go on its own thread, COM pumps messages itself during calls
    /// out (to the isolated process), and without OLE1 DDE there's no window for broadcasts to reach.
    pub fn init_thread(makes: bool) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            if makes {
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
            }
        }
    }

    /// Lookups (in the cache only: nothing is made) of a program and of a photo Windows ships, for what
    /// the first ones load, per process and per thread: the thumbnail cache, and what photos need. A
    /// first screenful from the cache comes about a third sooner for it.
    pub fn warm_up() {
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        for file in [r"explorer.exe", r"Web\Wallpaper\Windows\img0.jpg"] {
            let _ = from_shell(&format!(r"{windows}\{file}"), 32, true);
        }
    }

    pub fn outcome(path: &str, size: u32, lookup: bool) -> Outcome {
        let started = Instant::now();
        let made = if lookup {
            from_shell(path, size, true)
        } else if photo_provider(path) {
            made_here(path, size)
        } else {
            from_shell(path, size, false)
        };
        let ms = || (started.elapsed().as_secs_f64() * 1000.0) as f32;
        match made {
            Ok(pixels) if lookup && from_smaller_size(pixels.width, pixels.height, size) => Outcome::Smaller { bmp: encode_bmp(&pixels), ms: ms() },
            Ok(pixels) => Outcome::Found { bmp: encode_bmp(&pixels), ms: ms() },
            Err(e) if lookup && NOT_CACHED.contains(&e.code()) => Outcome::NotCached,
            Err(e) => Outcome::Nothing { lasting: !TRANSIENT.contains(&e.code()) },
        }
    }

    /// The item's thumbnail through IShellItemImageFactory, at most `size` pixels on each side: only
    /// from Windows' thumbnail cache with `cached_only` (a millisecond or two, at a smaller size when
    /// that's all the cache has), otherwise made by the file type's thumbnail handler where Windows runs
    /// it (an isolated process, usually, which makes about 50 a second however many ask), and cached.
    /// Never an icon: the UI has its own for those.
    pub fn from_shell(path: &str, size: u32, cached_only: bool) -> windows::core::Result<Pixels> {
        unsafe {
            let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path), None::<&IBindCtx>)?;
            let flags = if cached_only { SIIGBF_THUMBNAILONLY | SIIGBF_INCACHEONLY } else { SIIGBF_THUMBNAILONLY };
            let side = size as i32;
            let bitmap = factory.GetImage(SIZE { cx: side, cy: side }, flags)?;
            let pixels = read(bitmap, false);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            pixels.ok_or_else(|| E_FAIL.into())
        }
    }

    /// A photo's thumbnail made on this thread by Windows' own photo provider (WTS_EXTRACTINPROC), and
    /// cached by Windows as if Explorer had made it. One thread makes as many a second as the
    /// isolated process does; each thread more adds as many again.
    pub fn made_here(path: &str, size: u32) -> windows::core::Result<Pixels> {
        thread_local! {
            static CACHE: std::cell::OnceCell<Option<IThumbnailCache>> = const { std::cell::OnceCell::new() };
        }
        unsafe {
            let Some(cache) = CACHE.with(|c| c.get_or_init(|| CoCreateInstance(&LocalThumbnailCache, None, CLSCTX_INPROC_SERVER).ok()).clone()) else {
                return from_shell(path, size, false);
            };
            let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None::<&IBindCtx>)?;
            let mut shared: Option<ISharedBitmap> = None;
            cache.GetThumbnail(&item, size, WTS_EXTRACTINPROC | WTS_SCALETOREQUESTEDSIZE, Some(&mut shared), None, None)?;
            let shared = shared.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
            // The bitmap stays the shared bitmap's.
            let opaque = shared.GetFormat().is_ok_and(|format| format == WTSAT_RGB);
            read(shared.GetSharedBitmap()?, opaque).ok_or_else(|| E_FAIL.into())
        }
    }

    /// Whether files like `path` get their thumbnails from Windows' photo provider and its built-in
    /// codecs, which is safe to run in this process. Every other handler (Office's, a cloud drive's, a
    /// codec pack's) runs where Windows puts it. Asked once per extension: by the extension's
    /// associations, and by the app the person chose to open it with, which could bring a handler of
    /// its own.
    pub fn photo_provider(path: &str) -> bool {
        static KNOWN: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
        let Some(ext) = Path::new(path).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())) else { return false };
        if !BUILT_IN.contains(&ext.as_str()) {
            return false;
        }
        let known = KNOWN.get_or_init(Default::default);
        if let Some(&photo) = known.lock().unwrap_or_else(|e| e.into_inner()).get(&ext) {
            return photo;
        }
        let photo = handler(&ext).is_some_and(|h| h.eq_ignore_ascii_case(PHOTO_PROVIDER))
            && user_choice(&ext).and_then(|prog_id| handler(&prog_id)).is_none_or(|h| h.eq_ignore_ascii_case(PHOTO_PROVIDER));
        known.lock().unwrap_or_else(|e| e.into_inner()).insert(ext, photo);
        photo
    }

    /// The thumbnail handler registered for `assoc` (an extension or a ProgID), as a CLSID string.
    fn handler(assoc: &str) -> Option<String> {
        let mut buf = [0u16; 64];
        let mut len = buf.len() as u32;
        let hr = unsafe { AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_SHELLEXTENSION, &HSTRING::from(assoc), &HSTRING::from(THUMBNAIL_PROVIDER), Some(PWSTR(buf.as_mut_ptr())), &mut len) };
        hr.is_ok().then(|| String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]))
    }

    /// The ProgID of the app chosen in Windows' "Open with" for `ext`, if one was.
    fn user_choice(ext: &str) -> Option<String> {
        let key = HSTRING::from(format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{ext}\UserChoice"));
        let mut buf = [0u16; 260];
        let mut bytes = (buf.len() * 2) as u32;
        let found = unsafe { RegGetValueW(HKEY_CURRENT_USER, &key, &HSTRING::from("ProgId"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut bytes)) };
        // A name too long for the buffer fails with the length it needs, so it's read only once found.
        if found.is_err() {
            return None;
        }
        let units = (bytes as usize / 2).min(buf.len());
        let text = String::from_utf16_lossy(&buf[..units]).trim_end_matches('\0').to_string();
        (!text.is_empty()).then_some(text)
    }

    /// The bitmap's pixels as straight-alpha BGRA (`opaque` when it's known to have no alpha).
    unsafe fn read(bitmap: HBITMAP, opaque: bool) -> Option<Pixels> {
        let mut info = BITMAP::default();
        if GetObjectW(HGDIOBJ(bitmap.0), size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut _)) == 0 {
            return None;
        }
        let (width, height) = (info.bmWidth, info.bmHeight.abs());
        if width <= 0 || height <= 0 || width > 4096 || height > 4096 {
            return None;
        }
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; width as usize * height as usize * 4];
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(dc, bitmap, 0, height as u32, Some(bgra.as_mut_ptr() as *mut _), &mut header, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        if lines != height {
            return None;
        }
        if opaque {
            bgra.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
        } else {
            straighten_alpha(&mut bgra);
        }
        Some(Pixels { width: width as u32, height: height as u32, bgra })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(path: &str) -> Key {
        Key { path: path.into(), size: 96, modified: 1, bytes: 2 }
    }

    fn waiter(lane: &str, seq: u32, reply: u32) -> Waiter<u32> {
        Waiter { lane: lane.into(), seq, reply }
    }

    fn found() -> Outcome {
        Outcome::Found { bmp: vec![1, 2, 3], ms: 1.5 }
    }

    #[test]
    fn looks_up_first_then_makes_what_windows_lacks() {
        let mut q: Queue<u32> = Queue::new(1 << 20);
        q.want(key("a"), "A".into(), false, waiter("tab1", 1, 10));
        q.want(key("b"), "B".into(), false, waiter("tab1", 2, 20));
        // Only threads that make thumbnails take what has to be made; lookups go first for everyone.
        let a = q.next(false).unwrap();
        assert_eq!((a.path.as_str(), a.phase), ("A", Phase::Lookup));
        assert!(q.finish(&a.key, Outcome::NotCached).is_none(), "on to be made");
        let b = q.next(false).unwrap();
        assert_eq!((b.path.as_str(), b.phase), ("B", Phase::Lookup));
        assert!(q.next(false).is_none(), "a lookup thread doesn't make thumbnails");
        let make = q.next(true).unwrap();
        assert_eq!((make.path.as_str(), make.phase), ("A", Phase::Make));
        let (thumb, replies) = q.finish(&make.key, found()).unwrap();
        assert_eq!((thumb.found, thumb.source, replies), (true, MADE, vec![10]));
        // Kept in memory for the next request.
        assert_eq!(q.memory.get(&key("a")).unwrap().bmp, [1, 2, 3]);
        let (thumb, replies) = q.finish(&b.key, found()).unwrap();
        assert_eq!((thumb.source, replies), (FROM_CACHE, vec![20]));
        assert!(q.jobs.is_empty());
    }

    #[test]
    fn shares_one_job_between_panes() {
        let mut q: Queue<u32> = Queue::new(1 << 20);
        q.want(key("a"), "A".into(), false, waiter("left", 1, 1));
        q.want(key("a"), "A".into(), false, waiter("right", 2, 2));
        let task = q.next(false).unwrap();
        assert!(q.next(true).is_none(), "one job for both");
        // One pane scrolls away; the other still waits for it.
        assert_eq!(q.cancel("left", None, 9), [1]);
        let (_, replies) = q.finish(&task.key, found()).unwrap();
        assert_eq!(replies, [2]);
    }

    #[test]
    fn drops_requests_that_scrolled_away() {
        let mut q: Queue<u32> = Queue::new(1 << 20);
        q.want(key("a"), "A".into(), false, waiter("tab1", 1, 1));
        q.want(key("b"), "B".into(), false, waiter("tab1", 2, 2));
        q.want(key("c"), "C".into(), false, waiter("tab2", 3, 3));
        let only: HashSet<String> = ["a".to_string()].into();
        assert_eq!(q.cancel("tab1", Some(&only), 5), [1]);
        // A request made after the cancel was sent survives it.
        q.want(key("b"), "B".into(), false, waiter("tab1", 7, 4));
        assert_eq!(q.cancel("tab1", None, 5), [2]);
        assert_eq!(q.next(false).unwrap().path, "B");
        assert_eq!(q.next(false).unwrap().path, "C");
        assert!(q.next(true).is_none());
    }

    #[test]
    fn finishes_what_a_worker_started_even_when_dropped() {
        let mut q: Queue<u32> = Queue::new(1 << 20);
        q.want(key("a"), "A".into(), false, waiter("tab1", 1, 1));
        let task = q.next(true).unwrap();
        assert!(q.cancel("tab1", None, 1).len() == 1 && q.jobs.contains_key(&key("a")));
        // Nobody waits: a lookup that missed isn't made, but a thumbnail found is kept.
        assert_eq!(q.finish(&task.key, Outcome::NotCached).map(|(_, r)| r), Some(vec![]));
        q.want(key("b"), "B".into(), false, waiter("tab1", 2, 2));
        let task = q.next(true).unwrap();
        q.cancel("tab1", None, 2);
        q.finish(&task.key, found());
        assert!(q.memory.get(&key("b")).is_some());
    }

    #[test]
    fn makes_what_windows_has_only_at_a_smaller_size() {
        let mut q: Queue<u32> = Queue::new(1 << 20);
        q.want(key("a"), "A".into(), false, waiter("tab1", 1, 1));
        let task = q.next(false).unwrap();
        // Windows has the list's little one: a tile's is made at its own size.
        assert!(q.finish(&task.key, Outcome::Smaller { bmp: vec![9], ms: 1.0 }).is_none(), "on to be made");
        let make = q.next(true).unwrap();
        assert_eq!((make.path.as_str(), make.phase), ("A", Phase::Make));
        let (thumb, replies) = q.finish(&make.key, found()).unwrap();
        assert_eq!((thumb.source, thumb.bmp.as_slice(), replies), (MADE, &[1, 2, 3][..], vec![1]));
        // Making it fails: the smaller one will do.
        q.want(key("b"), "B".into(), false, waiter("tab1", 2, 2));
        let task = q.next(false).unwrap();
        q.finish(&task.key, Outcome::Smaller { bmp: vec![9], ms: 1.0 });
        let make = q.next(true).unwrap();
        let (thumb, _) = q.finish(&make.key, Outcome::Nothing { lasting: true }).unwrap();
        assert_eq!((thumb.found, thumb.source, thumb.bmp.as_slice()), (true, FROM_CACHE, &[9][..]));
        assert!(q.memory.get(&key("b")).is_some_and(|t| t.found));
        // Online-only files aren't made (that would download them), and nothing's kept for later.
        q.want(key("c"), "C".into(), true, waiter("tab1", 3, 3));
        let task = q.next(false).unwrap();
        let (thumb, replies) = q.finish(&task.key, Outcome::Smaller { bmp: vec![9], ms: 1.0 }).unwrap();
        assert_eq!((thumb.found, thumb.source, replies), (true, FROM_CACHE, vec![3]));
        assert!(q.memory.get(&key("c")).is_none());
        // Nobody waits any more: not kept either, so the next tile to ask has it made.
        q.want(key("d"), "D".into(), false, waiter("tab1", 4, 4));
        let task = q.next(false).unwrap();
        q.cancel("tab1", None, 4);
        assert_eq!(q.finish(&task.key, Outcome::Smaller { bmp: vec![9], ms: 1.0 }).map(|(_, r)| r), Some(vec![]));
        assert!(q.memory.get(&key("d")).is_none());
    }

    #[test]
    fn tells_smaller_cached_sizes_from_small_pictures() {
        // Windows' 32 and 256 px versions, for a 120 px tile and a 320 px one.
        assert!(from_smaller_size(32, 20, 120) && from_smaller_size(256, 160, 320) && from_smaller_size(60, 96, 120));
        // What was asked for, and pictures that are that small themselves.
        assert!(!from_smaller_size(120, 75, 120) && !from_smaller_size(200, 150, 320) && !from_smaller_size(96, 60, 96));
    }

    #[test]
    fn online_only_files_are_only_looked_up() {
        let mut q: Queue<u32> = Queue::new(1 << 20);
        q.want(key("a"), "A".into(), true, waiter("tab1", 1, 1));
        let task = q.next(true).unwrap();
        let (thumb, replies) = q.finish(&task.key, Outcome::NotCached).unwrap();
        assert_eq!((thumb.found, thumb.lasting, replies), (false, false, vec![1]));
        // Not remembered: once the file is downloaded (or Windows has its thumbnail), it's asked again.
        assert!(q.memory.get(&key("a")).is_none());
        // Someone who may download it (the same file, not online-only any more) gets it made.
        q.want(key("a"), "A".into(), true, waiter("tab1", 2, 2));
        q.want(key("a"), "A".into(), false, waiter("tab1", 3, 3));
        let task = q.next(true).unwrap();
        assert!(q.finish(&task.key, Outcome::NotCached).is_none());
    }

    #[test]
    fn remembers_files_without_thumbnails_unless_it_may_work_later() {
        let mut q: Queue<u32> = Queue::new(1 << 20);
        q.want(key("a"), "A".into(), false, waiter("tab1", 1, 1));
        let task = q.next(true).unwrap();
        let (none, _) = q.finish(&task.key, Outcome::Nothing { lasting: true }).unwrap();
        assert!(none.lasting && !q.memory.get(&key("a")).unwrap().found);
        q.want(key("b"), "B".into(), false, waiter("tab1", 2, 2));
        let task = q.next(true).unwrap();
        let (none, _) = q.finish(&task.key, Outcome::Nothing { lasting: false }).unwrap();
        assert!(!none.lasting && q.memory.get(&key("b")).is_none());
    }

    #[test]
    fn memory_drops_the_least_recently_used() {
        let thumb = |n: usize| Arc::new(Thumb { found: true, lasting: true, source: MADE, ms: 0.0, bmp: vec![0; n] });
        let mut memory = Memory::new(3 * (100 + 128));
        memory.put(key("a"), thumb(100));
        memory.put(key("b"), thumb(100));
        memory.put(key("c"), thumb(100));
        assert!(memory.get(&key("a")).is_some());
        memory.put(key("d"), thumb(100));
        assert!(memory.get(&key("b")).is_none(), "b was used least recently");
        assert!(memory.get(&key("a")).is_some() && memory.get(&key("c")).is_some() && memory.get(&key("d")).is_some());
        assert_eq!(memory.bytes, 3 * 228);
        // Replacing an entry doesn't count it twice.
        memory.put(key("a"), thumb(10));
        assert_eq!(memory.bytes, 2 * 228 + 138);
    }

    #[test]
    fn encodes_answers() {
        let thumb = Thumb { found: true, lasting: true, source: MADE, ms: 2.5, bmp: vec![b'B', b'M'] };
        let bytes = thumb.encode(FROM_MEMORY);
        assert_eq!(&bytes[..4], [FOUND, FROM_MEMORY, 1, 0]);
        assert_eq!(f32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2.5);
        assert_eq!(&bytes[8..], b"BM");
        assert_eq!(Thumb::none(true).encode(FROM_CACHE), [NONE, FROM_CACHE, 1, 0, 0, 0, 0, 0]);
        // None yet: the UI asks again next time.
        assert_eq!(Thumb::none(false).encode(FROM_CACHE)[2], 0);
    }

    #[test]
    fn encodes_bmps_the_webview_reads() {
        let bmp = encode_bmp(&Pixels { width: 2, height: 1, bgra: vec![1, 2, 3, 255, 4, 5, 6, 128] });
        let u32_at = |at: usize| u32::from_le_bytes(bmp[at..at + 4].try_into().unwrap());
        let i32_at = |at: usize| i32::from_le_bytes(bmp[at..at + 4].try_into().unwrap());
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!((u32_at(2) as usize, u32_at(10), u32_at(14)), (bmp.len(), 122, 108));
        assert_eq!((i32_at(18), i32_at(22), u32_at(30)), (2, -1, 3));
        assert_eq!((u32_at(54), u32_at(66)), (0x00FF_0000, 0xFF00_0000));
        assert_eq!(&bmp[122..], [1, 2, 3, 255, 4, 5, 6, 128]);
    }

    #[test]
    fn straightens_alpha() {
        // No alpha at all: opaque.
        let mut none = vec![10, 20, 30, 0, 40, 50, 60, 0];
        straighten_alpha(&mut none);
        assert_eq!(none, [10, 20, 30, 255, 40, 50, 60, 255]);
        // Premultiplied: colors divided back out.
        let mut pre = vec![64, 32, 0, 128, 0, 0, 0, 0, 255, 255, 255, 255];
        straighten_alpha(&mut pre);
        assert_eq!(pre, [128, 64, 0, 128, 0, 0, 0, 0, 255, 255, 255, 255]);
        // Already straight (a color above its alpha): left alone.
        let mut straight = vec![200, 10, 10, 100];
        straighten_alpha(&mut straight);
        assert_eq!(straight, [200, 10, 10, 100]);
    }

    /// Windows' answers for real files: a wallpaper Windows ships (made once, then from its cache), and
    /// files that have no thumbnail.
    #[cfg(windows)]
    #[test]
    fn reads_windows_thumbnails() {
        win::init_thread(false);
        let wallpaper = r"C:\Windows\Web\Wallpaper\Windows\img0.jpg";
        if std::path::Path::new(wallpaper).exists() {
            let made = win::outcome(wallpaper, 96, false);
            let Outcome::Found { bmp, .. } = made else { panic!("no thumbnail of {wallpaper}: {made:?}") };
            let (width, height) = (i32::from_le_bytes(bmp[18..22].try_into().unwrap()), -i32::from_le_bytes(bmp[22..26].try_into().unwrap()));
            assert!(width == 96 && (40..=96).contains(&height), "{width}x{height}");
            assert!(matches!(win::outcome(wallpaper, 96, true), Outcome::Found { .. }), "cached now");
        }
        assert!(matches!(win::outcome(r"C:\Windows\win.ini", 96, false), Outcome::Nothing { lasting: true }));
        // Only photo types are made in this process; text files have no handler, nameless ones no type.
        assert!(!win::photo_provider(r"C:\Windows\win.ini") && !win::photo_provider(r"C:\Windows\no-extension"));
        assert!(matches!(win::outcome(r"C:\no\such\file.jpg", 96, true), Outcome::Nothing { lasting: true }));
    }
}
