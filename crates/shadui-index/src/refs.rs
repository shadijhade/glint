//! ReFS volumes (Dev Drives among them), which have no MFT to read. The first read is a walk of every
//! folder, each listed in one call with what the index keeps (names, sizes, dates, attributes and
//! 128-bit file ids: `FileIdExtdDirectoryInfo`), on several threads at once. From then on the change
//! journal keeps it current, as on NTFS: its records (USN_RECORD_V3, with 128-bit ids) are translated
//! into the form the index reads, each id given a record number of the index's own, which is kept next
//! to the snapshot so the two always agree.
//!
//! A file read since a change gets its size and date from the file itself (opened by its id), and its
//! name and folder from the journal's latest record of it.

use crate::tree::{make_ref, Builder, RecordState, Tree, FIRST_USER_RECORD, ROOT};
use crate::win::{os_error, wide, Handle};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Condvar, Mutex};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_NO_MORE_FILES, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FileBasicInfo, FileIdExtdDirectoryInfo, FileIdExtdDirectoryRestartInfo, FileIdInfo, FileNameInfo, FileStandardInfo, GetFileInformationByHandleEx,
    OpenFileById, ExtendedFileIdType, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_128, FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_ID_INFO,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, OPEN_EXISTING, SYNCHRONIZE,
};

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
/// The root's sequence number in the index, as on NTFS (record 5, sequence 5); every other ReFS record
/// has sequence 1.
const ROOT_SEQ: u16 = 5;

/// Each 128-bit file id's record number in the index, and back.
#[derive(Default)]
pub struct Ids {
    numbers: HashMap<u128, u32>,
    /// By record number (minus FIRST_USER_RECORD); the root's is kept apart.
    ids: Vec<u128>,
    root: u128,
    /// The latest name and folder (record number) the journal gave each record, for rereading it.
    names: HashMap<u32, (u32, String)>,
}

impl Ids {
    fn with_root(root: u128) -> Ids {
        Ids { root, ..Default::default() }
    }

    /// The record number for `id`, given one if it has none yet.
    pub fn number(&mut self, id: u128) -> u32 {
        if id == self.root {
            return ROOT;
        }
        if let Some(&n) = self.numbers.get(&id) {
            return n;
        }
        let n = FIRST_USER_RECORD + self.ids.len() as u32;
        self.ids.push(id);
        self.numbers.insert(id, n);
        n
    }

    fn id(&self, number: u32) -> Option<u128> {
        if number == ROOT {
            return Some(self.root);
        }
        self.ids.get(number.checked_sub(FIRST_USER_RECORD)? as usize).copied()
    }

    fn reference(&mut self, id: u128) -> u64 {
        let n = self.number(id);
        make_ref(n, if n == ROOT { ROOT_SEQ } else { 1 })
    }

    /// Saved next to the snapshot: the root's id, then every record's, by number.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let tmp = path.with_extension("ids.tmp");
        let mut out = io::BufWriter::new(fs::File::create(&tmp)?);
        out.write_all(b"GLINTIDS")?;
        out.write_all(&(self.ids.len() as u64).to_le_bytes())?;
        out.write_all(&self.root.to_le_bytes())?;
        for id in &self.ids {
            out.write_all(&id.to_le_bytes())?;
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        fs::rename(tmp, path)
    }

    pub fn load(path: &Path) -> io::Result<Ids> {
        let mut bytes = Vec::new();
        fs::File::open(path)?.read_to_end(&mut bytes)?;
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "not a file of record ids");
        if bytes.get(..8) != Some(b"GLINTIDS") {
            return Err(bad());
        }
        let count = u64::from_le_bytes(bytes.get(8..16).ok_or_else(bad)?.try_into().unwrap()) as usize;
        if bytes.len() != 32 + count * 16 {
            return Err(bad());
        }
        let root = u128::from_le_bytes(bytes[16..32].try_into().unwrap());
        let mut ids = Ids::with_root(root);
        for chunk in bytes[32..].as_chunks::<16>().0 {
            let id = u128::from_le_bytes(*chunk);
            ids.number(id);
        }
        Ok(ids)
    }
}

fn file_id(id: &FILE_ID_128) -> u128 {
    u128::from_le_bytes(id.Identifier)
}

/// SeBackupPrivilege, which the service holds, switched on: folders open whoever they belong to.
fn enable_backup_privilege() {
    use windows::Win32::Security::{AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token).is_err() {
            return;
        }
        let token = Handle(token);
        let mut privileges = TOKEN_PRIVILEGES { PrivilegeCount: 1, ..Default::default() };
        let name = wide("SeBackupPrivilege");
        if LookupPrivilegeValueW(PCWSTR::null(), PCWSTR(name.as_ptr()), &mut privileges.Privileges[0].Luid).is_ok() {
            privileges.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
            let _ = AdjustTokenPrivileges(token.0, false, Some(&privileges), 0, None, None);
        }
    }
}

fn open_dir(path: &str) -> io::Result<Handle> {
    let path = wide(path);
    unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            (FILE_LIST_DIRECTORY | SYNCHRONIZE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
    .map(Handle)
    .map_err(os_error)
}

/// One entry of a folder, as the walk finds it.
struct Found {
    parent: u128,
    id: u128,
    name: String,
    attrs: u32,
    size: u64,
    modified: i64,
}

/// Lists one folder; its subfolders (path, id) go to `subfolders`.
fn list(path: &str, parent: u128, out: &mut Vec<Found>, subfolders: &mut Vec<(String, u128)>) -> io::Result<()> {
    let handle = open_dir(path)?;
    let mut buf = vec![0u64; 8192]; // 64 KB, 8-byte aligned
    let mut class = FileIdExtdDirectoryRestartInfo;
    loop {
        let bytes = buf.len() * 8;
        if let Err(e) = unsafe { GetFileInformationByHandleEx(handle.0, class, buf.as_mut_ptr().cast(), bytes as u32) } {
            if e.code() == ERROR_NO_MORE_FILES.to_hresult() {
                return Ok(());
            }
            return Err(os_error(e));
        }
        class = FileIdExtdDirectoryInfo;
        let raw = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), bytes) };
        let mut at = 0usize;
        loop {
            let entry = &raw[at..];
            let next = u32::from_le_bytes(entry[0..4].try_into().unwrap()) as usize;
            let name_len = u32::from_le_bytes(entry[60..64].try_into().unwrap()) as usize;
            let units: Vec<u16> = entry[88..88 + name_len].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            let name = String::from_utf16_lossy(&units);
            if name != "." && name != ".." {
                let attrs = u32::from_le_bytes(entry[56..60].try_into().unwrap());
                let id = u128::from_le_bytes(entry[72..88].try_into().unwrap());
                let dir = attrs & FILE_ATTRIBUTE_DIRECTORY != 0;
                if dir && attrs & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
                    subfolders.push((format!("{}\\{name}", path.trim_end_matches('\\')), id));
                }
                out.push(Found {
                    parent,
                    id,
                    attrs,
                    size: if dir { 0 } else { i64::from_le_bytes(entry[40..48].try_into().unwrap()).max(0) as u64 },
                    modified: i64::from_le_bytes(entry[24..32].try_into().unwrap()),
                    name,
                });
            }
            if next == 0 {
                break;
            }
            at += next;
        }
    }
}

/// Reads the whole volume `letter:` into an index, with the record numbers it gave each id.
/// `progress` gets (items found, 0) and returns false to stop.
pub fn scan(letter: char, progress: &(dyn Fn(u64, u64) -> bool + Sync)) -> io::Result<(Tree, Ids)> {
    enable_backup_privilege();
    let root_path = format!(r"\\?\{letter}:\");
    let root = {
        let handle = open_dir(&root_path)?;
        let mut info = FILE_ID_INFO::default();
        unsafe { GetFileInformationByHandleEx(handle.0, FileIdInfo, (&mut info as *mut FILE_ID_INFO).cast(), size_of::<FILE_ID_INFO>() as u32) }.map_err(os_error)?;
        file_id(&info.FileId)
    };
    // Folders wait in one queue for the threads; each sends what it found to the collector here.
    struct Queue {
        folders: VecDeque<(String, u128)>,
        busy: usize,
        stopped: bool,
    }
    let queue = Mutex::new(Queue { folders: VecDeque::from([(root_path, root)]), busy: 0, stopped: false });
    let wake = Condvar::new();
    let found = AtomicU64::new(0);
    let interrupted = std::sync::atomic::AtomicBool::new(false);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().clamp(2, 8));
    let mut all: Vec<Found> = Vec::new();
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel::<Vec<Found>>();
        for _ in 0..threads {
            let (tx, queue, wake, found) = (tx.clone(), &queue, &wake, &found);
            scope.spawn(move || loop {
                let (path, id) = {
                    let mut q = queue.lock().unwrap();
                    loop {
                        if q.stopped {
                            return;
                        }
                        if let Some(next) = q.folders.pop_front() {
                            q.busy += 1;
                            break next;
                        }
                        if q.busy == 0 {
                            q.stopped = true;
                            wake.notify_all();
                            return;
                        }
                        q = wake.wait(q).unwrap();
                    }
                };
                let (mut out, mut subfolders) = (Vec::new(), Vec::new());
                // A folder that can't be listed (in use, gone since) is left out, as fd leaves it.
                let _ = list(&path, id, &mut out, &mut subfolders);
                found.fetch_add(out.len() as u64, Ordering::Relaxed);
                let _ = tx.send(out);
                let mut q = queue.lock().unwrap();
                q.folders.extend(subfolders);
                q.busy -= 1;
                wake.notify_all();
            });
        }
        drop(tx);
        for batch in rx {
            all.extend(batch);
            if !interrupted.load(Ordering::Relaxed) && !progress(found.load(Ordering::Relaxed), 0) {
                interrupted.store(true, Ordering::Relaxed);
                let mut q = queue.lock().unwrap();
                q.stopped = true;
                wake.notify_all();
            }
        }
    });
    if interrupted.load(Ordering::Relaxed) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"));
    }
    let mut ids = Ids::with_root(root);
    for f in &all {
        ids.number(f.id);
    }
    let mut builder = Builder::new(FIRST_USER_RECORD as usize + ids.ids.len());
    for f in &all {
        let n = ids.number(f.id);
        let dir = f.attrs & FILE_ATTRIBUTE_DIRECTORY != 0;
        builder.record(n, 1, dir, f.attrs, f.modified);
        if !dir {
            builder.size(n, f.size);
        }
        let parent = ids.reference(f.parent);
        builder.name(n, parent, &f.name);
    }
    Ok((builder.finish(), ids))
}

/// Rewrites a page of ReFS journal records (USN_RECORD_V3) in place as the NTFS form the index reads
/// (USN_RECORD_V2: 64-bit references, made of the record numbers `ids` gives), noting each record's
/// latest name and folder. Returns the page's new length.
pub fn translate_page(page: &mut [u8], len: usize, ids: &mut Ids) -> usize {
    let (mut read, mut write) = (8usize, 8usize);
    while read + 76 <= len {
        let rec_len = u32::from_le_bytes(page[read..read + 4].try_into().unwrap()) as usize;
        if rec_len < 8 || read + rec_len > len {
            break;
        }
        let major = u16::from_le_bytes(page[read + 4..read + 6].try_into().unwrap());
        if major != 3 {
            // Already the NTFS form (or one the index doesn't read): moved as it is.
            page.copy_within(read..read + rec_len, write);
            write += rec_len;
            read += rec_len;
            continue;
        }
        let r = page[read..read + rec_len].to_vec();
        let u32_at = |at: usize| u32::from_le_bytes(r[at..at + 4].try_into().unwrap());
        let file = u128::from_le_bytes(r[8..24].try_into().unwrap());
        let parent = u128::from_le_bytes(r[24..40].try_into().unwrap());
        let reason = u32_at(56);
        let (name_len, name_off) = (u16::from_le_bytes(r[72..74].try_into().unwrap()) as usize, u16::from_le_bytes(r[74..76].try_into().unwrap()) as usize);
        let Some(name_bytes) = r.get(name_off..name_off + name_len) else {
            read += rec_len;
            continue;
        };
        let (file_ref, parent_ref) = (ids.reference(file), ids.reference(parent));
        let (n, parent_n) = (crate::tree::split_ref(file_ref).0 as u32, crate::tree::split_ref(parent_ref).0 as u32);
        let units: Vec<u16> = name_bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        if reason & crate::usn::REASON_FILE_DELETE != 0 {
            ids.names.remove(&n);
        } else if reason & crate::usn::REASON_RENAME_OLD_NAME == 0 {
            ids.names.insert(n, (parent_n, String::from_utf16_lossy(&units)));
        }
        let out_len = (60 + name_len).div_ceil(8) * 8;
        let mut v2 = vec![0u8; out_len];
        v2[0..4].copy_from_slice(&(out_len as u32).to_le_bytes());
        v2[4..6].copy_from_slice(&2u16.to_le_bytes());
        v2[8..16].copy_from_slice(&file_ref.to_le_bytes());
        v2[16..24].copy_from_slice(&parent_ref.to_le_bytes());
        v2[24..56].copy_from_slice(&r[40..72]); // USN, time, reason, source, security id, attributes
        v2[56..58].copy_from_slice(&(name_len as u16).to_le_bytes());
        v2[58..60].copy_from_slice(&60u16.to_le_bytes());
        v2[60..60 + name_len].copy_from_slice(name_bytes);
        page[write..write + out_len].copy_from_slice(&v2);
        write += out_len;
        read += rec_len;
    }
    write
}

/// What record `number` holds now: its size, date and attributes from the file (opened by its id on
/// `volume`), its name and folder from the journal (or, not told there, from the file's own path).
/// None once the file is gone.
pub fn record_state(volume: &Handle, letter: char, ids: &mut Ids, number: u32) -> io::Result<Option<RecordState>> {
    let Some(id) = ids.id(number) else { return Ok(None) };
    let descriptor = FILE_ID_DESCRIPTOR {
        dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: ExtendedFileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 { ExtendedFileId: FILE_ID_128 { Identifier: id.to_le_bytes() } },
    };
    let file = match unsafe { OpenFileById(volume.0, &descriptor, FILE_READ_ATTRIBUTES.0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, FILE_FLAG_BACKUP_SEMANTICS) } {
        Ok(handle) => Handle(handle),
        // Deleted (or never there): out of the index.
        Err(e) if matches!(e.code().0 as u32 & 0xFFFF, 2 | 3 | 87 | 1168) => return Ok(None),
        Err(e) => return Err(os_error(e)),
    };
    let mut basic = FILE_BASIC_INFO::default();
    let mut standard = FILE_STANDARD_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(file.0, FileBasicInfo, (&mut basic as *mut FILE_BASIC_INFO).cast(), size_of::<FILE_BASIC_INFO>() as u32).map_err(os_error)?;
        GetFileInformationByHandleEx(file.0, FileStandardInfo, (&mut standard as *mut FILE_STANDARD_INFO).cast(), size_of::<FILE_STANDARD_INFO>() as u32).map_err(os_error)?;
    }
    let dir = standard.Directory;
    let named = match ids.names.get(&number).cloned() {
        Some(named) => Some(named),
        None => name_from_path(&file, letter, ids),
    };
    let names = named.map(|(parent, name)| vec![(make_ref(parent, if parent == ROOT { ROOT_SEQ } else { 1 }), name)]).unwrap_or_default();
    Ok(Some(RecordState {
        seq: if number == ROOT { ROOT_SEQ } else { 1 },
        dir,
        attrs: basic.FileAttributes,
        modified: basic.LastWriteTime,
        size: if dir { 0 } else { standard.EndOfFile.max(0) as u64 },
        names,
        tags: None,
    }))
}

/// A file's name and folder from its own path, when the journal didn't say.
fn name_from_path(file: &Handle, letter: char, ids: &mut Ids) -> Option<(u32, String)> {
    let mut buf = vec![0u32; 2048];
    unsafe { GetFileInformationByHandleEx(file.0, FileNameInfo, buf.as_mut_ptr().cast(), (buf.len() * 4) as u32) }.ok()?;
    let len = buf[0] as usize / 2;
    let units = unsafe { std::slice::from_raw_parts(buf.as_ptr().add(1).cast::<u16>(), len.min(4094)) };
    let path = String::from_utf16_lossy(units);
    let (folder, name) = path.rsplit_once('\\')?;
    let folder = open_dir(&format!(r"\\?\{letter}:{}\", folder)).ok()?;
    let mut info = FILE_ID_INFO::default();
    unsafe { GetFileInformationByHandleEx(folder.0, FileIdInfo, (&mut info as *mut FILE_ID_INFO).cast(), size_of::<FILE_ID_INFO>() as u32) }.ok()?;
    Some((ids.number(file_id(&info.FileId)), name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v3(usn: i64, file: u128, parent: u128, reason: u32, name: &str) -> Vec<u8> {
        let units: Vec<u8> = name.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let len = (76 + units.len()).div_ceil(8) * 8;
        let mut r = vec![0u8; len];
        r[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        r[4..6].copy_from_slice(&3u16.to_le_bytes());
        r[8..24].copy_from_slice(&file.to_le_bytes());
        r[24..40].copy_from_slice(&parent.to_le_bytes());
        r[40..48].copy_from_slice(&usn.to_le_bytes());
        r[56..60].copy_from_slice(&reason.to_le_bytes());
        r[68..72].copy_from_slice(&0x20u32.to_le_bytes());
        r[72..74].copy_from_slice(&(units.len() as u16).to_le_bytes());
        r[74..76].copy_from_slice(&76u16.to_le_bytes());
        r[76..76 + units.len()].copy_from_slice(&units);
        r
    }

    #[test]
    fn translates_refs_journal_pages_for_the_index() {
        let root = 0xAAAA_0000_0000_0000_0000_0000_0000_0001u128;
        let mut ids = Ids::with_root(root);
        let folder = 0x1234_5678_9ABC_DEF0_1111_2222_3333_4444u128;
        let file = 0x9999_0000_0000_0000_0000_0000_0000_0002u128;
        let mut page = 777i64.to_le_bytes().to_vec();
        page.extend(v3(10, folder, root, crate::usn::REASON_FILE_CREATE, "src"));
        page.extend(v3(11, file, folder, crate::usn::REASON_FILE_CREATE | crate::usn::REASON_CLOSE, "main.rs"));
        page.extend(v3(12, file, folder, crate::usn::REASON_RENAME_OLD_NAME, "main.rs"));
        page.extend(v3(13, file, folder, crate::usn::REASON_RENAME_NEW_NAME, "lib.rs"));
        let len = page.len();
        let new_len = translate_page(&mut page, len, &mut ids);
        let (next, records) = crate::usn::parse_page(&page[..new_len]).unwrap();
        assert_eq!(next, 777);
        let refs: Vec<_> = records.iter().map(|r| (crate::tree::split_ref(r.file), crate::tree::split_ref(r.parent), r.name.as_str(), r.usn)).collect();
        assert_eq!(refs, [((16, 1), (ROOT as u64, ROOT_SEQ), "src", 10), ((17, 1), (16, 1), "main.rs", 11), ((17, 1), (16, 1), "main.rs", 12), ((17, 1), (16, 1), "lib.rs", 13)]);
        assert_eq!(ids.names.get(&17), Some(&(16, "lib.rs".to_string())), "the latest name, not the old one");
        // Saved and read back, the numbers stay.
        let path = std::env::temp_dir().join(format!("glint-refs-ids-{}.ids", std::process::id()));
        ids.save(&path).unwrap();
        let mut back = Ids::load(&path).unwrap();
        assert_eq!((back.number(folder), back.number(file), back.number(root)), (16, 17, ROOT));
        let _ = fs::remove_file(&path);
    }
}
