//! RAR archives (RAR 4 and 5, volumes followed), read with UnRAR, RARLAB's own C++ library, through
//! unrar_sys. UnRAR only ever runs in test mode here: it decompresses and checks each file and hands
//! the bytes to a callback, and never writes a file or makes a link itself.
//!
//! Its callback pushes data while Glint's extraction pulls it (`each` gets a reader), so a pass runs
//! UnRAR on a thread of its own and sends the bytes over a short channel. Stop, or a reader that
//! goes away, makes the callback tell UnRAR to give up.

use super::tree::{Item, ENCRYPTED, UNREADABLE};
use super::Failure;
use std::collections::HashMap;
use std::ffi::c_void;
use std::io::{self, Read};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
// unrar_sys builds UnRAR and links it; its constants are right, but its structs aren't (see `ffi`).
extern crate unrar_sys;
use unrar_sys as codes;

/// UnRAR's DLL interface as its dll.hpp (UnRAR 7.1) declares it. Its structs are byte-packed
/// (`#pragma pack(1)`), which unrar_sys's own aren't: every field after the first pointer lands 4
/// bytes off there, and UnRAR then writes through a pointer half made of another field.
#[allow(non_snake_case)]
mod ffi {
    use std::ffi::c_void;

    pub type Callback = extern "system" fn(msg: u32, user: isize, p1: isize, p2: isize) -> i32;

    #[repr(C, packed)]
    pub struct OpenData {
        pub arc_name: *const u8,
        pub arc_name_w: *const u16,
        pub open_mode: u32,
        pub open_result: u32,
        pub cmt_buf: *mut u8,
        pub cmt_buf_size: u32,
        pub cmt_size: u32,
        pub cmt_state: u32,
        pub flags: u32,
        pub callback: Option<Callback>,
        pub user_data: isize,
        pub op_flags: u32,
        pub cmt_buf_w: *mut u16,
        pub reserved: [u32; 25],
    }

    #[repr(C, packed)]
    pub struct Header {
        pub arc_name: [u8; 1024],
        pub arc_name_w: [u16; 1024],
        pub file_name: [u8; 1024],
        pub file_name_w: [u16; 1024],
        pub flags: u32,
        pub pack_size: u32,
        pub pack_size_high: u32,
        pub unp_size: u32,
        pub unp_size_high: u32,
        pub host_os: u32,
        pub file_crc: u32,
        pub file_time: u32,
        pub unp_ver: u32,
        pub method: u32,
        pub file_attr: u32,
        pub cmt_buf: *mut u8,
        pub cmt_buf_size: u32,
        pub cmt_size: u32,
        pub cmt_state: u32,
        pub dict_size: u32,
        pub hash_type: u32,
        pub hash: [u8; 32],
        pub redir_type: u32,
        pub redir_name: *mut u16,
        pub redir_name_size: u32,
        pub dir_target: u32,
        pub mtime_low: u32,
        pub mtime_high: u32,
        pub ctime_low: u32,
        pub ctime_high: u32,
        pub atime_low: u32,
        pub atime_high: u32,
        pub arc_name_ex: *mut u16,
        pub arc_name_ex_size: u32,
        pub file_name_ex: *mut u16,
        pub file_name_ex_size: u32,
        pub reserved: [u32; 982],
    }

    // As dll.hpp lays them out.
    const _: () = assert!(std::mem::size_of::<OpenData>() == 176 && std::mem::size_of::<Header>() == 10244);

    extern "system" {
        pub fn RAROpenArchiveEx(data: *mut OpenData) -> *mut c_void;
        pub fn RARCloseArchive(handle: *mut c_void) -> i32;
        pub fn RARReadHeaderEx(handle: *mut c_void, header: *mut Header) -> i32;
        pub fn RARProcessFileW(handle: *mut c_void, operation: i32, dest_path: *const u16, dest_name: *const u16) -> i32;
    }
}

/// The longest link target read (Windows' paths stop well before).
const MAX_TARGET: usize = 32 << 10;
/// Pieces of data waiting between UnRAR's thread and the reader: a few of UnRAR's blocks (up to
/// 4 MB each).
const QUEUED: usize = 4;

/// How a file's bytes come out.
#[derive(Debug, Clone)]
enum Data {
    /// UnRAR decompresses it (a file, or nothing for a folder).
    Unrar,
    /// A symlink or junction: a small file holding where it points, as zips' and tars' links are.
    Inline(Box<[u8]>),
    /// A hard link or a file copy: its target's bytes, by the target's index.
    LinkTo(usize),
}

pub struct RarSource {
    /// The archive's first volume, which UnRAR reads the others from.
    first: PathBuf,
    data: Vec<Data>,
    problems: Vec<Option<Failure>>,
    /// In a solid archive, everything before an entry is decompressed to reach it.
    solid: bool,
    before: Vec<u64>,
    packed: u64,
    unpacked: u64,
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

fn text(units: &[u16]) -> String {
    String::from_utf16_lossy(&units[..units.iter().position(|&u| u == 0).unwrap_or(units.len())])
}

/// What an UnRAR error code means.
fn failure(code: i32, missing: &Option<String>) -> Failure {
    match code {
        codes::ERAR_MISSING_PASSWORD | codes::ERAR_BAD_PASSWORD => Failure::Encrypted,
        codes::ERAR_EOPEN if missing.is_some() => Failure::Unreadable(format!("it goes on in the next volume (“{}”), which isn't there", missing.as_deref().unwrap_or_default())),
        codes::ERAR_BAD_DATA => Failure::Damaged("its data doesn't match its checksum".into()),
        codes::ERAR_BAD_ARCHIVE | codes::ERAR_UNKNOWN_FORMAT => Failure::Damaged("it isn't a RAR archive UnRAR can read".into()),
        codes::ERAR_NO_MEMORY => Failure::Bomb("it needs more memory to unpack than there is".into()),
        codes::ERAR_EOPEN | codes::ERAR_EREAD => Failure::Io("the archive can't be read".into()),
        other => Failure::Damaged(format!("UnRAR error {other}")),
    }
}

/// What UnRAR's callback reports to: the channel to the reader (None while listing), Stop, and a
/// volume it asked for that isn't there.
struct Feed {
    tx: Option<SyncSender<Piece>>,
    stop: Arc<AtomicBool>,
    /// Whether the data now coming is an entry being served (else it's read through, in a solid archive).
    serving: bool,
    missing: Option<String>,
}

enum Piece {
    /// The next entry served: its place in the pass's list.
    Begin(usize),
    Data(Vec<u8>),
    /// The entry served is done, well or not.
    End(Option<Failure>),
    /// Bytes decompressed to reach what's wanted.
    Through(u64),
    /// The pass is over: why it stopped early, if it did.
    Done(Option<Failure>),
}

extern "system" fn callback(msg: u32, user: isize, p1: isize, p2: isize) -> i32 {
    // Safety: `user` is the `Feed` the archive was opened with, alive until it's closed.
    let feed = unsafe { &mut *(user as *mut Feed) };
    match msg {
        codes::UCM_PROCESSDATA => {
            if feed.stop.load(Ordering::Relaxed) {
                return -1;
            }
            let Some(tx) = &feed.tx else { return 1 };
            // Safety: UnRAR passes the address and size of the data it just unpacked.
            let data = unsafe { std::slice::from_raw_parts(p1 as *const u8, p2.max(0) as usize) };
            let piece = if feed.serving { Piece::Data(data.to_vec()) } else { Piece::Through(data.len() as u64) };
            if tx.send(piece).is_err() {
                return -1;
            }
            1
        }
        codes::UCM_CHANGEVOLUMEW if p2 == codes::RAR_VOL_NOTIFY => 1,
        codes::UCM_CHANGEVOLUMEW => {
            // Safety: UnRAR passes the name of the volume it looked for, NUL-ended; it's read up to
            // the NUL (and no further than a path can be).
            let start = p1 as *const u16;
            let mut len = 0;
            while len < 2048 && unsafe { *start.add(len) } != 0 {
                len += 1;
            }
            let name = text(unsafe { std::slice::from_raw_parts(start, len) });
            feed.missing = Some(Path::new(&name).file_name().map_or(name.clone(), |n| n.to_string_lossy().into_owned()));
            -1
        }
        codes::UCM_CHANGEVOLUME if p2 == codes::RAR_VOL_NOTIFY => 1,
        // No passwords yet; nor the narrow-string versions of these.
        _ => -1,
    }
}

/// A header as UnRAR fills it in, with its name and link target as strings (copied out of the
/// packed struct).
struct Header {
    name: String,
    target: String,
    flags: u32,
    size: u64,
    packed: u64,
    host_os: u32,
    attrs: u32,
    redir_type: u32,
    filetime: u64,
}

/// An archive opened with UnRAR, closed when dropped.
struct Opened {
    handle: *mut c_void,
    flags: u32,
    feed: Box<Feed>,
    /// Room for a long name (UnRAR 7's FileNameEx) and a link's target.
    name: Vec<u16>,
    target: Vec<u16>,
}

impl Opened {
    fn open(path: &Path, mode: u32, feed: Feed) -> Result<Opened, Failure> {
        let wide_name = wide(path);
        let mut feed = Box::new(feed);
        // Safety: an all-zero OpenData is a valid one (null pointers, no callback).
        let mut data: ffi::OpenData = unsafe { std::mem::zeroed() };
        data.arc_name_w = wide_name.as_ptr();
        data.open_mode = mode;
        data.callback = Some(callback);
        data.user_data = &mut *feed as *mut Feed as isize;
        // Safety: `data` and the name it points to live through the call; UnRAR copies the name.
        let handle = unsafe { ffi::RAROpenArchiveEx(&mut data) };
        let result = data.open_result as i32;
        if handle.is_null() || result != 0 {
            if !handle.is_null() {
                unsafe { ffi::RARCloseArchive(handle) };
            }
            return Err(match result {
                codes::ERAR_MISSING_PASSWORD | codes::ERAR_BAD_PASSWORD => Failure::EncryptedList,
                code => failure(code, &feed.missing),
            });
        }
        Ok(Opened { handle, flags: data.flags, feed, name: vec![0; MAX_TARGET], target: vec![0; MAX_TARGET] })
    }

    fn feed(&mut self) -> &mut Feed {
        &mut self.feed
    }

    /// The next header; None at the end.
    fn next(&mut self) -> Result<Option<Header>, i32> {
        // Safety: an all-zero Header is a valid one; boxed, as it's 10 KB.
        let mut raw: Box<ffi::Header> = Box::new(unsafe { std::mem::zeroed() });
        self.name.fill(0);
        self.target.fill(0);
        raw.file_name_ex = self.name.as_mut_ptr();
        raw.file_name_ex_size = self.name.len() as u32;
        raw.redir_name = self.target.as_mut_ptr();
        raw.redir_name_size = self.target.len() as u32;
        // Safety: the header and the buffers it points to live through the call.
        match unsafe { ffi::RARReadHeaderEx(self.handle, &mut *raw) } {
            codes::ERAR_SUCCESS => {}
            codes::ERAR_END_ARCHIVE => return Ok(None),
            code => return Err(code),
        }
        // Fields are copied out: the struct is packed, so they can't be borrowed.
        let short_name = raw.file_name_w;
        let long_name = text(&self.name);
        let low_high = |low: u32, high: u32| (low as u64) | ((high as u64) << 32);
        Ok(Some(Header {
            name: if long_name.is_empty() { text(&short_name) } else { long_name },
            target: text(&self.target),
            flags: raw.flags,
            size: low_high(raw.unp_size, raw.unp_size_high),
            packed: low_high(raw.pack_size, raw.pack_size_high),
            host_os: raw.host_os,
            attrs: raw.file_attr,
            redir_type: raw.redir_type,
            filetime: low_high(raw.mtime_low, raw.mtime_high),
        }))
    }

    /// Skips the current file (a seek, or in a solid archive a decompression), or tests it (its
    /// data to the callback, its checksum checked).
    fn process(&mut self, operation: i32) -> i32 {
        unsafe { ffi::RARProcessFileW(self.handle, operation, std::ptr::null(), std::ptr::null()) }
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        unsafe { ffi::RARCloseArchive(self.handle) };
    }
}

/// The first volume of the set `path` is in: "name.part1.rar" (with as many digits) for a later
/// part of a set with new-style names, "name.rar" for an old-style ".r00". `path` itself otherwise.
fn first_volume(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = name.to_lowercase();
    if let Some(stem) = lower.strip_suffix(".rar") {
        if let Some(at) = stem.rfind(".part") {
            let digits = &stem[at + 5..];
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                let first = format!("{}{}1.rar", &name[..at + 5], "0".repeat(digits.len() - 1));
                let candidate = path.with_file_name(first);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    } else if lower.len() > 4 && lower.as_bytes()[lower.len() - 4] == b'.' && lower.as_bytes()[lower.len() - 3] == b'r' && lower[lower.len() - 2..].bytes().all(|b| b.is_ascii_digit()) {
        let candidate = path.with_file_name(format!("{}.rar", &name[..name.len() - 4]));
        if candidate.is_file() {
            return candidate;
        }
    }
    path.to_path_buf()
}

/// Lists the archive (from its first volume, when `path` is a later one and the first is there).
pub fn open(path: &Path, max_entries: usize) -> Result<(RarSource, Vec<Item>), Failure> {
    let first = first_volume(path);
    let mut archive = Opened::open(&first, codes::RAR_OM_LIST, Feed { tx: None, stop: Arc::new(AtomicBool::new(false)), serving: false, missing: None })?;
    let solid = archive.flags & codes::ROADF_SOLID != 0;
    let mut items: Vec<Item> = Vec::new();
    let mut data = Vec::new();
    let mut problems: Vec<Option<Failure>> = Vec::new();
    let mut targets = Vec::new();
    let mut packed = 0u64;
    // Whether the last file read goes on into the next volume.
    let mut goes_on = false;
    loop {
        let header = match archive.next() {
            Ok(Some(header)) => header,
            Ok(None) => break,
            // A volume that isn't there ends the listing, and the file going on into it says so.
            Err(codes::ERAR_EOPEN) if archive.feed().missing.is_some() => {
                if goes_on {
                    let last = items.len() - 1;
                    items[last].attrs |= UNREADABLE;
                    problems[last] = Some(failure(codes::ERAR_EOPEN, &archive.feed().missing));
                }
                break;
            }
            Err(code) if items.is_empty() => return Err(failure(code, &archive.feed().missing)),
            // Damage past the first files: what was read so far lists.
            Err(_) => break,
        };
        goes_on = header.flags & codes::RHDF_SPLITAFTER != 0;
        if items.len() >= max_entries {
            return Err(Failure::TooManyEntries(items.len() + 1));
        }
        let flags = header.flags;
        packed = packed.saturating_add(header.packed);
        let modified = super::read::filetime_ms(header.filetime).unwrap_or(0);
        // Windows' attributes from Windows; from elsewhere, only whether nobody may write it.
        let attrs = if header.host_os == 3 { if header.attrs & 0o222 == 0 { 0x1 } else { 0 } } else { header.attrs & 0x27 };
        let mut item = Item { path: header.name, dir: flags & codes::RHDF_DIRECTORY != 0, size: header.size, modified, attrs };
        let target = header.target;
        let (entry, problem) = match header.redir_type {
            1..=3 if !target.is_empty() => {
                item.size = target.len() as u64;
                (Data::Inline(target.into_bytes().into_boxed_slice()), None)
            }
            4 | 5 => {
                targets.push((items.len(), target.replace('/', "\\")));
                (Data::LinkTo(usize::MAX), None)
            }
            // An older RAR's Unix symlink keeps its target as its data, which UnRAR reads only to
            // make the link, never to hand it on.
            1..=3 => (Data::Unrar, Some(Failure::Unreadable("it's a symbolic link from an older RAR, whose target UnRAR doesn't pass on".into()))),
            _ if flags & codes::RHDF_ENCRYPTED != 0 => (Data::Unrar, Some(Failure::Encrypted)),
            _ if flags & codes::RHDF_SPLITBEFORE != 0 => (Data::Unrar, Some(Failure::Unreadable("it begins in the volume before this one, which isn't there".into()))),
            _ => (Data::Unrar, None),
        };
        if problem.is_some() {
            item.attrs |= if matches!(problem, Some(Failure::Encrypted)) { ENCRYPTED } else { UNREADABLE };
        }
        items.push(item);
        data.push(entry);
        problems.push(problem);
        let code = archive.process(codes::RAR_SKIP);
        if code != 0 {
            // The file goes on in a volume that isn't there.
            let why = failure(code, &archive.feed().missing);
            let last = items.len() - 1;
            items[last].attrs |= UNREADABLE;
            problems[last] = Some(why);
            break;
        }
    }
    // Hard links and copies read their target: the last file before them with that name.
    let by_name: HashMap<String, usize> = items.iter().enumerate().filter(|(_, i)| !i.dir).map(|(n, i)| (i.path.to_lowercase(), n)).collect();
    for (link, target) in targets {
        match by_name.get(&target.to_lowercase()).filter(|&&t| t != link && matches!(data[t], Data::Unrar) && problems[t].is_none()) {
            Some(&t) => {
                data[link] = Data::LinkTo(t);
                items[link].size = items[t].size;
            }
            None => {
                problems[link] = Some(Failure::Damaged(format!("it's a link to “{target}”, which isn't in the archive")));
                items[link].attrs |= UNREADABLE;
            }
        }
    }
    let mut before = Vec::with_capacity(items.len());
    let mut sum = 0u64;
    for item in &items {
        before.push(sum);
        sum = sum.saturating_add(item.size);
    }
    let unpacked = sum;
    Ok((RarSource { first, data, problems, solid, before, packed, unpacked }, items))
}

impl RarSource {
    pub fn cost(&self, item: usize, size: u64) -> u64 {
        if self.solid {
            self.before.get(item).copied().unwrap_or(0).saturating_add(size)
        } else {
            size
        }
    }

    pub fn block_of(&self, _item: usize) -> Option<usize> {
        self.solid.then_some(0)
    }

    pub fn ratio(&self) -> u64 {
        self.unpacked / self.packed.max(1)
    }

    pub fn problem(&self, item: usize) -> Option<Failure> {
        self.problems.get(item).cloned().flatten()
    }

    /// Links' targets from memory; the rest in passes through the archive in order, each header
    /// served at most once a pass (a hard link and its target take two).
    pub fn read_each(&self, items: &[usize], each: &mut dyn FnMut(usize, &mut dyn Read) -> bool, skipped: &mut dyn FnMut(u64) -> bool) -> Vec<(usize, Failure)> {
        let mut failed = Vec::new();
        // What each wanted item reads: (the header to serve, the item).
        let mut wanted: Vec<(usize, usize)> = Vec::new();
        for &item in items {
            match (self.problem(item), self.data.get(item)) {
                (Some(problem), _) => failed.push((item, problem)),
                (None, Some(Data::Inline(bytes))) => {
                    if !each(item, &mut &bytes[..]) {
                        failed.push((item, Failure::Cancelled));
                    }
                }
                (None, Some(&Data::LinkTo(target))) => wanted.push((target, item)),
                (None, Some(Data::Unrar)) => wanted.push((item, item)),
                (None, None) => failed.push((item, Failure::NotFound)),
            }
        }
        wanted.sort_unstable();
        while !wanted.is_empty() {
            // One of each header now; the rest (a header wanted twice) next time.
            let mut pass = Vec::new();
            let mut again = Vec::new();
            for w in wanted {
                if pass.last().is_some_and(|&(h, _)| h == w.0) {
                    again.push(w);
                } else {
                    pass.push(w);
                }
            }
            if !self.pass(&pass, each, skipped, &mut failed) {
                failed.extend(again.into_iter().map(|(_, i)| (i, Failure::Cancelled)));
                break;
            }
            wanted = again;
        }
        failed
    }

    /// One pass through the archive for `pass` (headers in order, each once). False when stopped.
    fn pass(&self, pass: &[(usize, usize)], each: &mut dyn FnMut(usize, &mut dyn Read) -> bool, skipped: &mut dyn FnMut(u64) -> bool, failed: &mut Vec<(usize, Failure)>) -> bool {
        let (tx, rx) = sync_channel(QUEUED);
        let stop = Arc::new(AtomicBool::new(false));
        let headers: Vec<usize> = pass.iter().map(|&(h, _)| h).collect();
        let (first, solid, worker_stop) = (self.first.clone(), self.solid, stop.clone());
        let worker = std::thread::spawn(move || serve(&first, &headers, solid, tx, worker_stop));
        let mut served = vec![false; pass.len()];
        let mut go = true;
        loop {
            match rx.recv() {
                Ok(Piece::Begin(k)) => {
                    served[k] = true;
                    let mut reader = Pieces { rx: &rx, chunk: Vec::new(), at: 0, ended: false };
                    let keep = each(pass[k].1, &mut reader);
                    // Done early (a preview's first bytes): UnRAR stops at its next block rather
                    // than decompressing the rest for nobody.
                    if !keep {
                        stop.store(true, Ordering::SeqCst);
                    }
                    // Whatever the reader left of the entry: its end (and its checksum).
                    while !reader.ended {
                        match rx.recv() {
                            Ok(Piece::End(_)) | Err(_) => reader.ended = true,
                            Ok(_) => {}
                        }
                    }
                    if !keep {
                        go = false;
                        break;
                    }
                }
                Ok(Piece::Through(n)) => {
                    if !skipped(n) {
                        go = false;
                        break;
                    }
                }
                Ok(Piece::Done(why)) => {
                    if let Some(why) = why {
                        failed.extend(pass.iter().zip(&served).filter(|(_, &s)| !s).map(|(&(_, i), _)| (i, why.clone())));
                        served.iter_mut().for_each(|s| *s = true);
                    }
                    break;
                }
                Ok(Piece::Data(_) | Piece::End(_)) => {}
                Err(_) => break,
            }
        }
        // Stopped, or done: UnRAR gives up at its next block once nobody listens.
        stop.store(true, Ordering::SeqCst);
        drop(rx);
        let _ = worker.join();
        if !go {
            failed.extend(pass.iter().zip(&served).filter(|(_, &s)| !s).map(|(&(_, i), _)| (i, Failure::Cancelled)));
        }
        go
    }
}

/// UnRAR's side of a pass: tests each header wanted, in order, sending its bytes; reads through
/// (a solid archive) or skips the rest.
fn serve(first: &Path, headers: &[usize], solid: bool, tx: SyncSender<Piece>, stop: Arc<AtomicBool>) {
    let feed = Feed { tx: Some(tx.clone()), stop: stop.clone(), serving: false, missing: None };
    let mut archive = match Opened::open(first, codes::RAR_OM_EXTRACT, feed) {
        Ok(a) => a,
        Err(e) => {
            let _ = tx.send(Piece::Done(Some(e)));
            return;
        }
    };
    let Some(&last) = headers.last() else {
        let _ = tx.send(Piece::Done(None));
        return;
    };
    let mut next = 0;
    for index in 0..=last {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match archive.next() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(code) => {
                let why = failure(code, &archive.feed().missing);
                let _ = tx.send(Piece::Done(Some(why)));
                return;
            }
        }
        let wanted = headers.get(next) == Some(&index);
        let code = if wanted {
            if tx.send(Piece::Begin(next)).is_err() {
                return;
            }
            next += 1;
            archive.feed().serving = true;
            let code = archive.process(codes::RAR_TEST);
            archive.feed().serving = false;
            let why = (code != 0).then(|| failure(code, &archive.feed().missing));
            if tx.send(Piece::End(why)).is_err() {
                return;
            }
            0
        } else if solid {
            // Read through: what's wanted later depends on it. Its bytes count for progress.
            archive.process(codes::RAR_TEST)
        } else {
            archive.process(codes::RAR_SKIP)
        };
        if code != 0 {
            let _ = tx.send(Piece::Done(Some(failure(code, &archive.feed().missing))));
            return;
        }
    }
    let _ = tx.send(Piece::Done(None));
}

/// An entry's bytes as UnRAR's thread sends them, failing at the end if UnRAR found it damaged.
struct Pieces<'a> {
    rx: &'a Receiver<Piece>,
    chunk: Vec<u8>,
    at: usize,
    ended: bool,
}

impl Read for Pieces<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.at >= self.chunk.len() {
            if self.ended {
                return Ok(0);
            }
            match self.rx.recv() {
                Ok(Piece::Data(data)) => {
                    self.chunk = data;
                    self.at = 0;
                }
                Ok(Piece::End(None)) => self.ended = true,
                Ok(Piece::End(Some(why))) => {
                    self.ended = true;
                    return Err(io::Error::new(io::ErrorKind::InvalidData, why.message()));
                }
                Ok(_) => {}
                Err(_) => {
                    self.ended = true;
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "UnRAR stopped before the end of the file"));
                }
            }
        }
        let n = buf.len().min(self.chunk.len() - self.at);
        buf[..n].copy_from_slice(&self.chunk[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_first_volume() {
        let dir = std::env::temp_dir().join(format!("glint-rar-volumes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["set.part01.rar", "set.part02.rar", "old.rar", "old.r00", "alone.part2.rar"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        assert_eq!(first_volume(&dir.join("set.part02.rar")), dir.join("set.part01.rar"));
        assert_eq!(first_volume(&dir.join("old.r00")), dir.join("old.rar"));
        assert_eq!(first_volume(&dir.join("alone.part2.rar")), dir.join("alone.part2.rar"), "no first part: this one");
        assert_eq!(first_volume(&dir.join("old.rar")), dir.join("old.rar"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
