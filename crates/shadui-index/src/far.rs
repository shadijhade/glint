//! Marks the record itself doesn't show (docs/gpui/marks/PLAN.md 6.4, and 14 item 6 for why): a `glint.*`
//! stream that left the record, or a record whose attribute list did. `Tree::far` lists them; they are read here
//! **through the file system**, never from the volume's clusters (a stream just written may still be in Windows'
//! cache).
//!
//! The service runs as LocalSystem, so **no path is ever walked**: an account that can write a folder could swap
//! part of a path for a link while the service looks, and send it to open somewhere else. An item is opened by its
//! file reference on the drive (`OpenFileById` on the drive's root), a link is not followed, and:
//! 1. its stream listing is asked on a handle that wants attributes alone (no sharing is needed, so nothing another
//!    program holds is tripped over). Nearly every far record has no `glint.*` stream and ends there;
//! 2. only an item that has one is opened again, for reading, and walked with `BackupRead`: only the three
//!    `glint.*` streams are read, each to its cap, everything else (the file's own data first) is sought over.

use crate::record::{comment_text, label_text, tags_text, COMMENT_KEPT};
use crate::tree::{make_ref, Marked, Tree};
use crate::win::Handle;
use std::ffi::c_void;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::IntoRawHandle;
use std::sync::RwLock;
use windows::Win32::Foundation::{ERROR_HANDLE_EOF, ERROR_MORE_DATA, HANDLE};
use windows::Win32::Storage::FileSystem::{
    BackupRead, BackupSeek, FileIdType, FileStreamInfo, GetFileInformationByHandleEx, OpenFileById, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};

/// The most records one pass of [`drain`] takes.
pub const BATCH: usize = 256;

const FILE_READ_ATTRIBUTES: u32 = 0x80;
const GENERIC_READ: u32 = 0x8000_0000;
/// How much of a label and of the tags is read (a comment's is [`COMMENT_KEPT`]).
const LABEL_READ: u64 = 256;
const TAGS_READ: u64 = 4096;
/// The streams that are marks, with their cap, in the order label, tags, comment.
const MARKS: [(&str, u64); 3] = [(":glint.label:$DATA", LABEL_READ), (":glint.tags:$DATA", TAGS_READ), (":glint.comment:$DATA", COMMENT_KEPT as u64)];

/// The root of drive `letter` (`X:\`) itself, opened for its attributes alone with every share mode.
fn root_handle(letter: char) -> Option<Handle> {
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(format!("{letter}:\\"))
        .ok()?;
    Some(Handle(HANDLE(file.into_raw_handle())))
}

/// Opens the item with file reference `reference` on the drive `root` is of: no path, a link not followed,
/// every share mode, and nothing fetched for a file whose data is kept somewhere else (a cloud folder's). A
/// reference whose sequence is 0 would match any file that has the number, so it opens nothing.
fn open_ref(root: &Handle, reference: u64, access: u32) -> Option<Handle> {
    if reference >> 48 == 0 {
        return None;
    }
    let id = FILE_ID_DESCRIPTOR {
        dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: FileIdType,
        // The reference's 64 bits as they are.
        Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: i64::from_ne_bytes(reference.to_ne_bytes()) },
    };
    // SAFETY: `id` lives through the call and `root` is an open handle.
    let handle = unsafe {
        OpenFileById(
            root.0,
            &id,
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL,
        )
    }
    .ok()?;
    Some(Handle(handle))
}

/// Which of [label, tags, comment] an item has a stream for, from its stream listing; `None` when the file
/// system can't say.
fn listing(file: &Handle) -> Option<[bool; 3]> {
    let mut size = 4096usize;
    while size <= 256 * 1024 {
        // A Vec<u64> keeps the buffer aligned to 8 bytes, as the call wants.
        let mut buf = vec![0u64; size / 8];
        // SAFETY: the buffer is `size` bytes long and the handle is open for the call.
        let got = unsafe { GetFileInformationByHandleEx(file.0, FileStreamInfo, buf.as_mut_ptr().cast(), size as u32) };
        match got {
            Ok(()) => {
                // SAFETY: the same buffer seen as bytes: u64 has no padding and u8 no alignment.
                let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), size) };
                return Some(walk_listing(bytes));
            }
            Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() => size *= 4,
            // An item with no streams at all.
            Err(e) if e.code() == ERROR_HANDLE_EOF.to_hresult() => return Some([false; 3]),
            Err(_) => return None,
        }
    }
    None
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at.checked_add(4)?)?.try_into().ok().map(u32::from_le_bytes)
}

/// The name of a stream from its UTF-16LE bytes (a stray byte or a lone surrogate is a replacement character).
fn stream_name(raw: &[u8]) -> String {
    let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    String::from_utf16_lossy(&units)
}

/// Which mark a stream's full name (`:glint.label:$DATA`, in any ASCII case) is.
fn mark_of(name: &str) -> Option<usize> {
    MARKS.iter().position(|(m, _)| name.eq_ignore_ascii_case(m))
}

/// Walks `FILE_STREAM_INFO` entries (next offset u32, name length in bytes u32, two sizes of 8 bytes, the name),
/// every offset and length checked against `bytes`.
fn walk_listing(bytes: &[u8]) -> [bool; 3] {
    let mut found = [false; 3];
    let mut off = 0usize;
    while let (Some(next), Some(len)) = (le32(bytes, off), off.checked_add(4).and_then(|at| le32(bytes, at))) {
        let name = off
            .checked_add(24)
            .and_then(|start| bytes.get(start..start.checked_add(usize::try_from(len).ok()?)?));
        let Some(name) = name else { break };
        if let Some(i) = mark_of(&stream_name(name)) {
            found[i] = true;
        }
        // An entry that doesn't move on ends the walk, so a lying offset can't loop it.
        match usize::try_from(next).ok().filter(|&n| n > 0).and_then(|n| off.checked_add(n)) {
            Some(n) => off = n,
            None => break,
        }
    }
    found
}

/// A `WIN32_STREAM_ID`'s fixed part is 20 bytes: id, attributes, size (8), name size.
const HEADER: usize = 20;
/// A stream's full name is 263 characters at most; a header that claims more is lying.
const NAME_MAX: usize = 1024;
/// The most streams one item is walked through.
const STREAMS_MAX: usize = 10_000;
/// `BACKUP_ALTERNATE_DATA`: a named stream.
const ALTERNATE_DATA: u32 = 4;

/// What a stream header says: its kind, the size of its data, the size of its name.
#[derive(Debug, PartialEq)]
struct Header {
    id: u32,
    size: u64,
    name: usize,
}

/// The header at the start of `bytes`; `None` when it is cut short, its size is negative or its name is longer
/// than any name can be.
fn header(bytes: &[u8]) -> Option<Header> {
    let id = le32(bytes, 0)?;
    let size = i64::from_le_bytes(bytes.get(8..16)?.try_into().ok()?);
    let name = usize::try_from(le32(bytes, 16)?).ok().filter(|&n| n <= NAME_MAX)?;
    Some(Header { id, size: u64::try_from(size).ok()?, name })
}

/// Where a walk reads a file's streams from: `BackupRead` and `BackupSeek`, or a fake.
trait Backup {
    /// Reads up to `buf.len()` bytes; 0 at the end or when it fails.
    fn read(&mut self, buf: &mut [u8]) -> usize;
    /// Moves over up to `n` bytes, never out of the stream it is in; how many it moved (0: it can't).
    fn seek(&mut self, n: u64) -> u64;
}

/// Fills `buf` as far as the source goes: how many bytes it got.
fn fill(src: &mut dyn Backup, buf: &mut [u8]) -> usize {
    let mut got = 0;
    while got < buf.len() {
        match buf.get_mut(got..).map(|rest| src.read(rest)) {
            Some(n) if n > 0 => got += n,
            _ => break,
        }
    }
    got
}

/// The marks in a file's streams: the three `glint.*` ones read to their caps, everything else sought over. A
/// header that lies (cut short, a name past the end, a size past what can be moved over) ends the walk and what
/// was read before it stands.
fn collect(src: &mut dyn Backup) -> Marked {
    let mut bytes: [Vec<u8>; 3] = Default::default();
    'streams: for _ in 0..STREAMS_MAX {
        let mut head = [0u8; HEADER];
        if fill(src, &mut head) < HEADER {
            break;
        }
        let Some(h) = header(&head) else { break };
        let mut name = vec![0u8; h.name];
        if fill(src, &mut name) < h.name {
            break;
        }
        let mut left = h.size;
        if let Some(i) = Some(h.id).filter(|&id| id == ALTERNATE_DATA).and_then(|_| mark_of(&stream_name(&name))) {
            let want = usize::try_from(left.min(MARKS[i].1)).unwrap_or(0);
            let mut data = vec![0u8; want];
            let got = fill(src, &mut data);
            data.truncate(got);
            bytes[i] = data;
            if got < want {
                break;
            }
            left -= got as u64;
        }
        while left > 0 {
            let moved = src.seek(left);
            if moved == 0 {
                break 'streams;
            }
            left -= moved.min(left);
        }
    }
    let field = |text: Option<String>| text.unwrap_or_default().into_boxed_str();
    Marked { label: field(label_text(&bytes[0])), tags: field(tags_text(&bytes[1])), comment: field(comment_text(&bytes[2])) }
}

/// An item opened for reading, walked by `BackupRead`. Dropping it frees the read's context.
struct Reading {
    file: Handle,
    context: *mut c_void,
}

impl Backup for Reading {
    fn read(&mut self, buf: &mut [u8]) -> usize {
        let mut got = 0u32;
        // SAFETY: the handle is open, `got` is ours, `context` is only ever Windows' own.
        let ok = unsafe { BackupRead(self.file.0, buf, &mut got, false, false, &mut self.context) };
        if ok.is_ok() { (got as usize).min(buf.len()) } else { 0 }
    }

    fn seek(&mut self, n: u64) -> u64 {
        let (mut low, mut high) = (0u32, 0u32);
        // SAFETY: the handle is open, the two outputs are ours, `context` is only ever Windows' own.
        // It answers false when it moved less than asked, having set what it did move.
        let _ = unsafe {
            BackupSeek(
                self.file.0,
                u32::try_from(n & 0xFFFF_FFFF).unwrap_or(0),
                u32::try_from(n >> 32).unwrap_or(0),
                &mut low,
                &mut high,
                &mut self.context,
            )
        };
        u64::from(high) << 32 | u64::from(low)
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        if !self.context.is_null() {
            let mut got = 0u32;
            // SAFETY: the abort call frees the context a read made; nothing is read into the empty buffer.
            let _ = unsafe { BackupRead(self.file.0, &mut [], &mut got, true, false, &mut self.context) };
        }
    }
}

/// The marks of the item with file reference `reference` on the drive `root` is of (`tree::make_ref`): its
/// three streams as `record`'s readers read them, each field empty when it has none. `None` when it can't be
/// opened (gone, another file's now, or kept from the service): its record is then left as it is.
pub fn read_ref(root: &Handle, reference: u64) -> Option<Marked> {
    let probe = open_ref(root, reference, FILE_READ_ATTRIBUTES)?;
    // Nearly every far record ends here, and nothing was held that another program could trip over.
    if listing(&probe)? == [false; 3] {
        return Some(Marked::default());
    }
    drop(probe);
    let file = open_ref(root, reference, GENERIC_READ)?;
    Some(collect(&mut Reading { file, context: std::ptr::null_mut() }))
}

/// One pass over drive `letter`'s far records: takes up to [`BATCH`] of them out of `Tree::far`, reads each
/// by its reference (`read_ref`) while no lock is held (a search never waits for the disk), and gives the tree
/// what was read (`Tree::set_marks`). Returns how many records it took: 0 when none is far, or when the drive's
/// root can't be opened. `stop` ends it early.
pub fn drain(tree: &RwLock<Tree>, letter: char, stop: &(dyn Fn() -> bool + Sync)) -> usize {
    let Some(root) = root_handle(letter) else { return 0 };
    drain_with(tree, stop, &|reference| read_ref(&root, reference))
}

fn drain_with(tree: &RwLock<Tree>, stop: &(dyn Fn() -> bool + Sync), read: &dyn Fn(u64) -> Option<Marked>) -> usize {
    let write = || tree.write().unwrap_or_else(|e| e.into_inner());
    let (taken, jobs) = {
        let mut tree = write();
        let numbers: Vec<u32> = tree.far.iter().copied().take(BATCH).collect();
        let mut jobs = Vec::new();
        for &n in &numbers {
            tree.far.remove(&n);
            if let Some(seq) = tree.rec(n).filter(|r| r.used()).map(|r| r.seq()) {
                jobs.push((n, make_ref(n, seq)));
            }
        }
        (numbers.len(), jobs)
    };
    // No lock is held while the disk is read.
    let mut done = Vec::with_capacity(jobs.len());
    let mut unread = Vec::new();
    for (n, reference) in &jobs {
        if stop() {
            unread.push(*n);
        } else if let Some(marks) = read(*reference) {
            done.push((*n, *reference, marks));
        }
    }
    if !done.is_empty() || !unread.is_empty() {
        let mut tree = write();
        for (n, reference, marks) in done {
            // Gone, or its number is another file's by now: what was read is not this record's.
            if tree.rec(n).is_none_or(|r| !r.used() || make_ref(n, r.seq()) != reference) {
                continue;
            }
            let again = tree.far.contains(&n);
            tree.set_marks(n, marks);
            if again {
                tree.far.insert(n);
            }
        }
        tree.far.extend(unread);
    }
    taken
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{RecordState, ROOT};
    use std::os::windows::io::AsRawHandle;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;
    use std::time::Instant;
    use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};

    /// A folder of its own under the temp folder, removed when dropped.
    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Dir {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!("shadui-far-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
            std::fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }
        fn path(&self, name: &str) -> String {
            self.0.join(name).to_string_lossy().into_owned()
        }
        /// Whether the drive keeps streams.
        fn streams(&self) -> bool {
            std::fs::write(self.0.join("probe.txt:glint.probe"), b"x").is_ok()
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn marks(label: &str, tags: &str, comment: &str) -> Marked {
        Marked { label: label.into(), tags: tags.into(), comment: comment.into() }
    }

    /// The root of the drive the temp folder is on.
    fn root() -> Handle {
        let temp = std::env::temp_dir().to_string_lossy().into_owned();
        root_handle(temp.chars().next().unwrap()).unwrap()
    }

    /// A path's own file reference (a link is not followed), as the tree would hold it.
    fn reference(path: &str) -> u64 {
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
            .unwrap();
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the handle is open and `info` is ours.
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.unwrap();
        u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow)
    }

    fn read(item: &str) -> Option<Marked> {
        read_ref(&root(), reference(item))
    }

    fn three(p: &str) {
        std::fs::write(format!("{p}:glint.label"), b"Red").unwrap();
        std::fs::write(format!("{p}:glint.tags"), b"a\r\nb\r\n").unwrap();
        std::fs::write(format!("{p}:glint.comment"), b"hello").unwrap();
    }

    #[test]
    fn a_file_and_a_folder_with_the_three_streams_read_back_by_their_reference() {
        let dir = Dir::new();
        if !dir.streams() {
            return;
        }
        std::fs::write(dir.path("f.txt"), b"data").unwrap();
        std::fs::create_dir(dir.path("sub")).unwrap();
        for item in ["f.txt", "sub"] {
            let p = dir.path(item);
            three(&p);
            assert_eq!(read(&p), Some(marks("Red", "a\nb", "hello")), "{item}");
        }
    }

    #[test]
    fn a_file_with_no_glint_stream_is_empty_marks_without_being_opened_for_reading() {
        let dir = Dir::new();
        let p = dir.path("f.txt");
        std::fs::write(&p, b"data").unwrap();
        let reference = reference(&p);
        // Held with no sharing at all: a read would be refused, a look at its attributes is not.
        let _held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&p).unwrap();
        assert_eq!(read_ref(&root(), reference), Some(Marked::default()));
        std::fs::create_dir(dir.path("sub")).unwrap();
        assert_eq!(read(&dir.path("sub")), Some(Marked::default()));
    }

    #[test]
    fn a_deleted_file_and_made_up_references_read_as_none() {
        let dir = Dir::new();
        let p = dir.path("f.txt");
        std::fs::write(&p, b"data").unwrap();
        let reference = reference(&p);
        let root = root();
        assert!(read_ref(&root, reference).is_some());
        std::fs::remove_file(&p).unwrap();
        assert_eq!(read_ref(&root, reference), None);
        assert_eq!(read_ref(&root, 0), None);
        assert_eq!(read_ref(&root, u64::MAX), None);
        // The right number with a sequence that is not its own.
        let other = (reference & 0xFFFF_FFFF_FFFF) | (reference >> 48 ^ 0x55) << 48;
        assert_eq!(read_ref(&root, other), None);
    }

    #[test]
    fn eighty_other_streams_with_long_names_still_leave_the_three_readable() {
        let dir = Dir::new();
        if !dir.streams() {
            return;
        }
        let p = dir.path("f.txt");
        std::fs::write(&p, b"data").unwrap();
        for i in 0..80 {
            std::fs::write(format!("{p}:{}{i}", "x".repeat(58)), b"y").unwrap();
        }
        three(&p);
        assert_eq!(read(&p), Some(marks("Red", "a\nb", "hello")));
    }

    #[test]
    fn a_marked_file_held_with_no_sharing_is_none_and_reads_once_it_is_let_go() {
        let dir = Dir::new();
        if !dir.streams() {
            return;
        }
        let p = dir.path("f.txt");
        std::fs::write(&p, b"data").unwrap();
        three(&p);
        let reference = reference(&p);
        let root = root();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&p).unwrap();
        assert_eq!(read_ref(&root, reference), None);
        drop(held);
        assert_eq!(read_ref(&root, reference), Some(marks("Red", "a\nb", "hello")));
    }

    #[test]
    fn a_long_comment_is_cut_on_a_character_and_the_files_own_data_is_sought_over() {
        let dir = Dir::new();
        if !dir.streams() {
            return;
        }
        let p = dir.path("f.txt");
        std::fs::write(&p, vec![7u8; 3 * 1024 * 1024]).unwrap();
        std::fs::write(format!("{p}:glint.comment"), "é".repeat(5000)).unwrap();
        let (reference, root) = (reference(&p), root());
        let start = Instant::now();
        let comment = read_ref(&root, reference).unwrap().comment;
        println!("a read over 3 MB of data: {:?}", start.elapsed());
        assert!(comment.len() <= COMMENT_KEPT && comment.len() > COMMENT_KEPT - 2 && comment.chars().all(|c| c == 'é'));
        std::fs::write(format!("{p}:glint.label"), "  Important \r\nsecond\n").unwrap();
        assert_eq!(&*read_ref(&root, reference).unwrap().label, "Important");
        std::fs::write(format!("{p}:glint.tags"), "tag\n".repeat(25_000)).unwrap();
        let tags = read_ref(&root, reference).unwrap().tags;
        assert!(tags.len() <= TAGS_READ as usize && tags.lines().count() <= 64);
    }

    #[test]
    fn a_stream_in_another_case_is_the_label_and_other_streams_are_not_marks() {
        let dir = Dir::new();
        if !dir.streams() {
            return;
        }
        let p = dir.path("f.txt");
        std::fs::write(&p, b"glint.label").unwrap();
        std::fs::write(format!("{p}:Zone.Identifier"), b"[ZoneTransfer]").unwrap();
        std::fs::write(format!("{p}:glint.other"), b"x").unwrap();
        assert_eq!(read(&p), Some(Marked::default()));
        std::fs::write(format!("{p}:Glint.Label"), b"Blue").unwrap();
        assert_eq!(&*read(&p).unwrap().label, "Blue");
    }

    /// A stream as `BackupRead` gives it: header, name, data.
    fn piece(id: u32, name: &str, size: i64, data: &[u8]) -> Vec<u8> {
        let name: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut out = [id.to_le_bytes().to_vec(), 0u32.to_le_bytes().to_vec(), size.to_le_bytes().to_vec()].concat();
        out.extend((name.len() as u32).to_le_bytes());
        out.extend(name);
        out.extend(data);
        out
    }

    fn glint(name: &str, data: &[u8]) -> Vec<u8> {
        piece(ALTERNATE_DATA, &format!(":{name}:$DATA"), data.len() as i64, data)
    }

    /// Bytes read as a file's streams are.
    struct Fake(Vec<u8>, usize);
    impl Backup for Fake {
        fn read(&mut self, buf: &mut [u8]) -> usize {
            let n = buf.len().min(self.0.len() - self.1);
            buf[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
            self.1 += n;
            n
        }
        fn seek(&mut self, n: u64) -> u64 {
            let moved = n.min((self.0.len() - self.1) as u64);
            self.1 += moved as usize;
            moved
        }
    }

    fn walked(bytes: Vec<u8>) -> Marked {
        collect(&mut Fake(bytes, 0))
    }

    #[test]
    fn headers_that_lie_end_the_walk_and_what_was_read_before_stands() {
        let own = piece(1, "", 6, b"abcdef");
        let good = [own.clone(), glint("glint.label", b"Red")].concat();
        assert_eq!(walked(good.clone()), marks("Red", "", ""));
        // A name size past the buffer.
        let mut lie = good.clone();
        lie.extend(piece(ALTERNATE_DATA, "", 0, b"")[..16].iter());
        lie.extend(900u32.to_le_bytes());
        assert_eq!(walked(lie), marks("Red", "", ""));
        // A stream size of u64::MAX (as the i64 it is read as, a negative one, and the largest positive one).
        for size in [-1i64, i64::MAX] {
            let lie = [good.clone(), piece(ALTERNATE_DATA, ":x:$DATA", size, b"zz"), glint("glint.tags", b"t")].concat();
            assert_eq!(walked(lie), marks("Red", "", ""), "{size}");
        }
        // A glint stream that claims more than there is: what is there stands.
        let lie = piece(ALTERNATE_DATA, ":glint.comment:$DATA", i64::MAX, b"short");
        assert_eq!(walked([good.clone(), lie].concat()), marks("Red", "", "short"));
        // A header cut short, and a file with nothing.
        assert_eq!(walked([good.clone(), vec![4, 0, 0]].concat()), marks("Red", "", ""));
        assert_eq!(walked(Vec::new()), Marked::default());
    }

    #[test]
    fn the_header_walker_refuses_what_cannot_be_true() {
        let ok = piece(4, ":a:$DATA", 10, b"");
        assert_eq!(header(&ok), Some(Header { id: 4, size: 10, name: 16 }));
        assert_eq!(header(&ok[..19]), None);
        assert_eq!(header(&piece(4, "a", -5, b"")), None);
        let mut long = piece(4, "a", 1, b"");
        long[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(header(&long), None);
    }

    #[test]
    fn a_name_that_is_not_utf16_is_no_mark_and_the_walk_goes_on() {
        let mut odd = piece(ALTERNATE_DATA, "", 2, b"zz");
        odd[16..20].copy_from_slice(&3u32.to_le_bytes());
        odd.splice(20..20, [0x00, 0xd8, 0x41]);
        let bytes = [odd, glint("GLINT.Label", b"Blue")].concat();
        assert_eq!(walked(bytes), marks("Blue", "", ""));
        assert!(!stream_name(&[0x00, 0xd8, 0x41]).is_empty());
        assert_eq!(walk_listing(&[0xff; 7]), [false; 3]);
        assert_eq!(walk_listing(&[0xff; 64]), [false; 3]);
    }

    #[test]
    fn a_link_is_not_followed_when_reading_its_marks() {
        let dir = Dir::new();
        if !dir.streams() {
            return;
        }
        let target = dir.path("target.txt");
        std::fs::write(&target, b"").unwrap();
        three(&target);
        let mut ran = false;
        let link = dir.path("link.txt");
        if std::os::windows::fs::symlink_file(&target, &link).is_ok() {
            assert_eq!(read(&link), Some(Marked::default()));
            ran = true;
        }
        let folder = dir.path("folder");
        std::fs::create_dir(&folder).unwrap();
        three(&folder);
        let junction = dir.path("junction");
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J", &junction, &folder]).output();
        if made.is_ok_and(|o| o.status.success()) {
            assert_eq!(read(&junction), Some(Marked::default()));
            ran = true;
        }
        println!("link test ran: {ran}");
    }

    #[test]
    fn ten_thousand_reads_of_an_unmarked_file_are_timed() {
        let dir = Dir::new();
        let p = dir.path("f.txt");
        std::fs::write(&p, b"data").unwrap();
        let (reference, root) = (reference(&p), root());
        let start = Instant::now();
        for _ in 0..10_000 {
            assert_eq!(read_ref(&root, reference), Some(Marked::default()));
        }
        println!("10,000 reads of an unmarked file by reference: {:?}", start.elapsed());
    }

    #[test]
    fn no_function_of_this_file_takes_the_path_of_an_item() {
        let src = include_str!("far.rs");
        for banned in [["fn ", "read(path"], ["fn ", "open(path"], ["fn ", "stream(path"]] {
            assert!(!src.contains(&banned.concat()), "{banned:?}");
        }
    }

    /// X:\dir\{name}, records 30 and up.
    fn far_tree(count: u32) -> Tree {
        let mut tree = Tree::default();
        tree.set_record(ROOT, &RecordState { seq: 5, dir: true, ..Default::default() });
        tree.set_record(20, &RecordState { seq: 1, dir: true, names: vec![(make_ref(ROOT, 5), "dir".into())], ..Default::default() });
        for n in 30..30 + count {
            tree.set_record(n, &RecordState { seq: 1, far: true, names: vec![(make_ref(20, 1), format!("name{n}"))], ..Default::default() });
        }
        tree
    }

    fn never() -> bool {
        false
    }

    #[test]
    fn two_far_records_get_what_the_reader_says_by_reference_and_stop_being_far() {
        let tree = RwLock::new(far_tree(2));
        let asked = Mutex::new(Vec::new());
        let reader = |reference: u64| {
            asked.lock().unwrap().push(reference);
            Some(marks("L", "", ""))
        };
        assert_eq!(drain_with(&tree, &never, &reader), 2);
        let mut asked = asked.into_inner().unwrap();
        asked.sort();
        assert_eq!(asked, [make_ref(30, 1), make_ref(31, 1)]);
        let tree = tree.read().unwrap();
        assert!(tree.far.is_empty());
        assert_eq!(tree.marked(30), Some(&marks("L", "", "")));
        assert_eq!(tree.marked(31), Some(&marks("L", "", "")));
    }

    #[test]
    fn no_lock_is_held_while_reading_and_a_record_made_far_again_meanwhile_stays_far() {
        let tree = RwLock::new(far_tree(1));
        let reader = |_: u64| {
            let mut tree = tree.write().unwrap();
            tree.set_record(30, &RecordState { seq: 1, far: true, names: vec![(make_ref(20, 1), "name30".into())], ..Default::default() });
            Some(marks("Read", "", ""))
        };
        assert_eq!(drain_with(&tree, &never, &reader), 1);
        let tree = tree.read().unwrap();
        assert_eq!(tree.marked(30).map(|m| &*m.label), Some("Read"));
        assert!(tree.far.contains(&30));
    }

    #[test]
    fn a_reader_that_answers_none_changes_nothing_and_a_removed_record_gets_nothing() {
        let tree = RwLock::new(far_tree(2));
        tree.write().unwrap().set_marks(30, marks("Old", "", ""));
        tree.write().unwrap().far.insert(30);
        let reader = |reference: u64| {
            if reference == make_ref(31, 1) {
                // Gone while it was read.
                tree.write().unwrap().remove_record(31);
                Some(marks("Ghost", "", ""))
            } else {
                None
            }
        };
        assert_eq!(drain_with(&tree, &never, &reader), 2);
        let tree = tree.read().unwrap();
        assert_eq!(tree.marked(30), Some(&marks("Old", "", "")));
        assert!(tree.far.is_empty());
        assert_eq!(tree.marked(31), None);
    }

    #[test]
    fn a_record_that_became_another_file_while_it_was_read_gets_nothing() {
        let tree = RwLock::new(far_tree(1));
        let reader = |_: u64| {
            let other = RecordState { seq: 2, names: vec![(make_ref(20, 1), "other".into())], ..Default::default() };
            tree.write().unwrap().set_record(30, &other);
            Some(marks("Old", "", ""))
        };
        assert_eq!(drain_with(&tree, &never, &reader), 1);
        assert_eq!(tree.read().unwrap().marked(30), None);
    }

    #[test]
    fn stop_after_the_first_read_puts_the_rest_back_in_far() {
        let tree = RwLock::new(far_tree(3));
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let reader = |_: u64| {
            reads.fetch_add(1, Ordering::SeqCst);
            Some(marks("L", "", ""))
        };
        let stop = || reads.load(Ordering::SeqCst) >= 1;
        assert_eq!(drain_with(&tree, &stop, &reader), 3);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(tree.read().unwrap().far.len(), 2);
    }

    #[test]
    fn one_pass_takes_a_batch_the_next_the_rest_and_none_far_asks_nothing() {
        let tree = RwLock::new(far_tree(BATCH as u32 + 10));
        let reader = |_: u64| Some(marks("L", "", ""));
        assert_eq!(drain_with(&tree, &never, &reader), BATCH);
        assert_eq!(drain_with(&tree, &never, &reader), 10);
        let silent = |_: u64| -> Option<Marked> { panic!("nothing is far") };
        assert_eq!(drain_with(&tree, &never, &silent), 0);
    }
}
