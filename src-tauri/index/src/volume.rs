//! Raw access to an NTFS volume (needs an administrator or SYSTEM): one fast sequential read of the
//! whole $MFT, single records read through the file system (current, unlike the disk, whose copy of
//! the MFT can lag a few seconds behind), and the USN change journal.

use crate::record::{self, Geometry, Parsed, Run};
use crate::tree::{split_ref, Builder, RecordState, Tree};
use crate::win::{os_error, wide, Handle};
use rayon::prelude::*;
use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::io;
use std::ops::{Deref, DerefMut};
use std::sync::mpsc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, MAX_PATH};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW, ReadFile, FILE_FLAG_NO_BUFFERING, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{
    CREATE_USN_JOURNAL_DATA, FSCTL_CREATE_USN_JOURNAL, FSCTL_GET_NTFS_FILE_RECORD, FSCTL_GET_NTFS_VOLUME_DATA, FSCTL_QUERY_USN_JOURNAL,
    FSCTL_READ_USN_JOURNAL, NTFS_FILE_RECORD_INPUT_BUFFER, NTFS_VOLUME_DATA_BUFFER, READ_USN_JOURNAL_DATA_V1, USN_JOURNAL_DATA_V0,
};
use windows::Win32::System::IO::{DeviceIoControl, OVERLAPPED};

/// Bytes per sequential read of the $MFT.
const CHUNK: u64 = 8 << 20;
const DRIVE_FIXED: u32 = 3;
const ERROR_INVALID_PARAMETER: i32 = 87;
const ERROR_JOURNAL_DELETE_IN_PROGRESS: i32 = 1178;
const ERROR_JOURNAL_NOT_ACTIVE: i32 = 1179;
const ERROR_JOURNAL_ENTRY_DELETED: i32 = 1181;

/// Heap memory aligned for unbuffered reads.
pub struct AlignedBuf {
    ptr: *mut u8,
    len: usize,
}

unsafe impl Send for AlignedBuf {}

impl AlignedBuf {
    pub fn new(len: usize) -> Self {
        let len = len.max(4096);
        let ptr = unsafe { alloc_zeroed(Layout::from_size_align(len, 4096).expect("buffer layout")) };
        assert!(!ptr.is_null(), "out of memory for a {len}-byte read buffer");
        AlignedBuf { ptr, len }
    }
}

impl Deref for AlignedBuf {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl DerefMut for AlignedBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr, Layout::from_size_align(self.len, 4096).unwrap()) }
    }
}

/// A stretch of the $MFT: (offset in the $MFT, offset on disk, length), in bytes.
type Extent = (u64, u64, u64);

/// Where the journal stands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Journal {
    pub id: u64,
    pub first_usn: i64,
    pub next_usn: i64,
}

#[derive(Debug)]
pub enum JournalError {
    /// The records we needed are gone: the journal wrapped past them, or was deleted or recreated.
    Lost,
    Io(io::Error),
}

pub struct Volume {
    handle: Handle,
    pub letter: char,
    pub geometry: Geometry,
    pub serial: u64,
    /// How much of the $MFT holds records (the rest was never written).
    mft_valid: u64,
}

/// Local fixed drives formatted NTFS, by letter.
pub fn fixed_ntfs_drives() -> Vec<char> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| (b'A' + i) as char)
        .filter(|&letter| {
            let root = wide(format!(r"{letter}:\"));
            if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != DRIVE_FIXED {
                return false;
            }
            let mut fs = [0u16; MAX_PATH as usize + 1];
            let ok = unsafe { GetVolumeInformationW(PCWSTR(root.as_ptr()), None, None, None, None, Some(&mut fs)) }.is_ok();
            ok && String::from_utf16_lossy(&fs).trim_end_matches('\0') == "NTFS"
        })
        .collect()
}

impl Volume {
    pub fn open(letter: char) -> io::Result<Volume> {
        let path = wide(format!(r"\\.\{letter}:"));
        let handle = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_NO_BUFFERING,
                None,
            )
        }
        .map_err(os_error)?;
        let handle = Handle(handle);
        let mut data = NTFS_VOLUME_DATA_BUFFER::default();
        ioctl(&handle, FSCTL_GET_NTFS_VOLUME_DATA, &[], as_bytes_mut(&mut data))?;
        let geometry = Geometry {
            bytes_per_sector: data.BytesPerSector,
            bytes_per_cluster: data.BytesPerCluster,
            record_size: data.BytesPerFileRecordSegment,
            mft_lcn: data.MftStartLcn as u64,
            total_sectors: data.NumberSectors as u64,
        };
        if geometry.record_size < 1024 || !geometry.record_size.is_power_of_two() || geometry.bytes_per_cluster == 0 {
            return Err(io::Error::other(format!("unexpected NTFS geometry {geometry:?}")));
        }
        Ok(Volume { handle, letter, geometry, serial: data.VolumeSerialNumber as u64, mft_valid: data.MftValidDataLength as u64 })
    }

    /// Reads `buf.len()` bytes at `offset` (both sector-aligned, as unbuffered reads require).
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut overlapped = OVERLAPPED::default();
        overlapped.Anonymous.Anonymous.Offset = offset as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        let mut read = 0;
        unsafe { ReadFile(self.handle.0, Some(buf), Some(&mut read), Some(&mut overlapped)) }.map_err(os_error)?;
        if read as usize != buf.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short read from the volume"));
        }
        Ok(())
    }

    fn cluster(&self) -> u64 {
        self.geometry.bytes_per_cluster as u64
    }

    /// Reads whole clusters: `bytes` rounded up, at a cluster-aligned `offset`.
    fn read_clusters(&self, offset: u64, bytes: u64) -> io::Result<AlignedBuf> {
        let unit = self.cluster().max(4096);
        let len = bytes.div_ceil(unit) * unit;
        let mut buf = AlignedBuf::new(len as usize);
        self.read_at(offset, &mut buf[..len as usize])?;
        Ok(buf)
    }

    /// Where the $MFT's bytes are on disk, and its size.
    fn mft_layout(&self) -> io::Result<(Vec<Extent>, u64)> {
        let record_size = self.geometry.record_size as usize;
        let start = self.geometry.mft_lcn * self.cluster();
        let mut first = self.read_clusters(start, record_size as u64)?[..record_size].to_vec();
        if !record::apply_fixups(&mut first, true) {
            if let Ok(Some(mut fs_first)) = self.file_record(0) {
                if record::apply_fixups(&mut fs_first, false) {
                    first = fs_first;
                } else {
                    return Err(io::Error::other("the $MFT's own record is damaged"));
                }
            } else {
                return Err(io::Error::other("the $MFT's own record is damaged"));
            }
        }
        let mut extents = record::data_extents(&first);
        // A very fragmented $MFT continues its run list in extension records named by its
        // attribute list; those records lie in the part of the $MFT already mapped (or are read
        // directly through the file system driver).
        if let Some(parsed) = record::parse(&mut first.clone(), false) {
            let list = match parsed.attribute_list {
                Some(record::AttributeList::Resident(list)) => Some(list),
                Some(record::AttributeList::NonResident { runs, size }) => Some(self.read_runs(&runs, size)?),
                None => None,
            };
            if let Some(list) = list {
                let mut pending = record::extension_records(&list, 0);
                // First try reading each extension record via the NTFS driver, which works even when
                // the base $DATA extent itself was moved into an extension record (e.g. record 15).
                pending.retain(|&number| {
                    if let Ok(Some(mut ext)) = self.file_record(number) {
                        if record::apply_fixups(&mut ext, false) {
                            extents.extend(record::data_extents(&ext));
                            return false;
                        }
                    }
                    true
                });
                // Fallback for records not returned by the driver: resolve iteratively as extents grow.
                loop {
                    let before = pending.len();
                    let layout = self.byte_extents(&extents);
                    pending.retain(|&number| {
                        let at = number * record_size as u64;
                        let disk_offset = layout
                            .iter()
                            .find(|&&(from, _, len)| at >= from && at < from + len)
                            .map(|&(from, disk, _)| disk + (at - from))
                            .or_else(|| (number < 16).then_some(start + at));
                        let Some(offset) = disk_offset else { return true };
                        let unit = self.cluster().max(4096);
                        let aligned = offset - offset % unit;
                        let Ok(buf) = self.read_clusters(aligned, (offset - aligned) + record_size as u64) else { return true };
                        let mut ext = buf[(offset - aligned) as usize..][..record_size].to_vec();
                        if record::apply_fixups(&mut ext, true) {
                            extents.extend(record::data_extents(&ext));
                        }
                        false
                    });
                    if pending.is_empty() || pending.len() == before {
                        break;
                    }
                }
            }
        }
        extents.sort_by_key(|e| e.0);
        let bytes = self.byte_extents(&extents);
        let size = extents
            .iter()
            .find_map(|e| e.1)
            .or_else(|| (self.mft_valid > 0).then_some(self.mft_valid))
            .or_else(|| bytes.iter().map(|&(at, _, len)| at + len).max())
            .ok_or_else(|| io::Error::other("the $MFT has no size"))?;
        Ok((bytes, size))
    }

    fn byte_extents(&self, extents: &[(u64, Option<u64>, Vec<Run>)]) -> Vec<Extent> {
        let cluster = self.cluster();
        let mut out = Vec::new();
        for (vcn, _, runs) in extents {
            let mut at = vcn * cluster;
            for run in runs {
                let len = run.clusters * cluster;
                if let Some(lcn) = run.lcn {
                    out.push((at, lcn * cluster, len));
                }
                at += len;
            }
        }
        out
    }

    /// Fills `buf` with the $MFT's bytes from `start`, whichever extents they're in.
    fn read_mft(&self, layout: &[Extent], start: u64, buf: &mut [u8]) -> io::Result<()> {
        let end = start + buf.len() as u64;
        let covered: u64 = layout.iter().map(|&(at, _, len)| (at + len).min(end).saturating_sub(at.max(start))).sum();
        if covered < buf.len() as u64 {
            buf.fill(0); // holes must not keep the last chunk's records
        }
        for &(at, disk, len) in layout {
            let (from, to) = (at.max(start), (at + len).min(end));
            if from < to {
                let offset = disk + (from - at);
                let dst = &mut buf[(from - start) as usize..(to - start) as usize];
                // Unbuffered reads on 4Kn drives require 4096-byte buffer, offset, and length alignment.
                if (from - start).is_multiple_of(4096) && dst.len().is_multiple_of(4096) && offset.is_multiple_of(4096) {
                    self.read_at(offset, dst)?;
                } else {
                    let unit = self.cluster().max(4096);
                    let aligned = offset - offset % unit;
                    let scratch = self.read_clusters(aligned, (offset - aligned) + dst.len() as u64)?;
                    dst.copy_from_slice(&scratch[(offset - aligned) as usize..][..dst.len()]);
                }
            }
        }
        Ok(())
    }

    /// Reads every record of the $MFT in one sequential pass (parsing in parallel while the next
    /// chunk is read) and builds the index. `progress` gets (records read, records in all) and
    /// returns false to stop.
    pub fn scan(&self, progress: &(dyn Fn(u64, u64) -> bool + Sync)) -> io::Result<Tree> {
        let (layout, size) = self.mft_layout()?;
        let record_size = self.geometry.record_size as u64;
        let total = size.min(if self.mft_valid > 0 { self.mft_valid } else { size }) / record_size;
        let end = total * record_size;
        let unit = self.cluster().max(record_size).max(4096);
        let piece = (CHUNK / unit).max(1) * unit;
        let pieces: Vec<(u64, u64)> = (0..end.div_ceil(piece)).map(|i| (i * piece, (end - i * piece).min(piece))).collect();
        let mut builder = Builder::new(total as usize);

        std::thread::scope(|scope| {
            let (full_tx, full_rx) = mpsc::sync_channel::<io::Result<(u64, AlignedBuf, usize)>>(2);
            let (free_tx, free_rx) = mpsc::channel::<AlignedBuf>();
            for _ in 0..3 {
                let _ = free_tx.send(AlignedBuf::new(piece as usize));
            }
            let layout = &layout;
            scope.spawn(move || {
                for (start, len) in pieces {
                    let Ok(mut buf) = free_rx.recv() else { return };
                    let read = len.div_ceil(unit) * unit;
                    let result = self.read_mft(layout, start, &mut buf[..read as usize]).map(|()| (start, buf, len as usize));
                    let failed = result.is_err();
                    if full_tx.send(result).is_err() || failed {
                        return;
                    }
                }
            });
            for item in full_rx {
                let (start, mut buf, len) = item?;
                let first = start / record_size;
                let parsed: Vec<Option<Parsed>> = buf[..len].par_chunks_mut(record_size as usize).map(|r| record::parse(r, true)).collect();
                for (i, parsed) in parsed.into_iter().enumerate() {
                    let number = first + i as u64;
                    if let Some(parsed) = parsed {
                        merge(&mut builder, number, parsed);
                    } else if buf.get(i * record_size as usize..i * record_size as usize + 4) == Some(b"FILE") {
                        // A record being written on an active drive (like C:) can have a torn fixup on raw disk;
                        // ask the NTFS driver for its clean copy so folders like C:\Users are never dropped.
                        if let Ok(Some(mut clean)) = self.file_record(number) {
                            if let Some(parsed) = record::parse(&mut clean, false) {
                                merge(&mut builder, number, parsed);
                            }
                        }
                    }
                }
                let _ = free_tx.send(buf);
                if !progress((first + (len as u64 / record_size)).min(total), total) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"));
                }
            }
            Ok::<(), io::Error>(())
        })?;
        Ok(builder.finish())
    }

    /// A record as the file system has it now (not the possibly older copy on disk). None when it
    /// isn't in use.
    fn file_record(&self, number: u64) -> io::Result<Option<Vec<u8>>> {
        let input = NTFS_FILE_RECORD_INPUT_BUFFER { FileReferenceNumber: number as i64 };
        // The output header is 12 bytes; Windows wants room for its padded size plus a record.
        let mut out = vec![0u8; 16 + self.geometry.record_size as usize];
        let len = ioctl(&self.handle, FSCTL_GET_NTFS_FILE_RECORD, as_bytes(&input), &mut out)?;
        // For a record not in use, NTFS answers with the closest one before it that is.
        let returned = u64::from_le_bytes(out[0..8].try_into().unwrap()) & 0xFFFF_FFFF_FFFF;
        let size = u32::from_le_bytes(out[8..12].try_into().unwrap()) as usize;
        if returned != number || len < 12 {
            return Ok(None);
        }
        Ok(Some(out[12..(12 + size).min(len)].to_vec()))
    }

    /// What record `number` holds now: None for a deleted file (or one the index doesn't keep).
    pub fn record_state(&self, number: u32) -> io::Result<Option<RecordState>> {
        let Some(mut bytes) = self.file_record(number as u64)? else { return Ok(None) };
        let Some(parsed) = record::parse(&mut bytes, false).filter(|p| p.in_use && p.base == 0) else { return Ok(None) };
        let mut state = RecordState {
            seq: parsed.seq,
            dir: parsed.dir,
            attrs: parsed.attrs,
            modified: parsed.modified.unwrap_or(0),
            size: parsed.size.unwrap_or(0),
            names: parsed.names.into_iter().map(|n| (n.parent, n.name)).collect(),
        };
        let mut size = parsed.size;
        // Names and the data stream can sit in extension records, listed in the attribute list.
        let list = match parsed.attribute_list {
            Some(record::AttributeList::Resident(list)) => Some(list),
            Some(record::AttributeList::NonResident { runs, size }) => Some(self.read_runs(&runs, size)?),
            None => None,
        };
        for ext in list.map(|l| record::extension_records(&l, number as u64)).unwrap_or_default() {
            let Some(mut bytes) = self.file_record(ext)? else { continue };
            let Some(parsed) = record::parse(&mut bytes, false) else { continue };
            if !parsed.in_use || split_ref(parsed.base).0 != number as u64 {
                continue;
            }
            state.names.extend(parsed.names.into_iter().map(|n| (n.parent, n.name)));
            size = size.or(parsed.size);
        }
        state.size = size.unwrap_or(0);
        Ok(Some(state))
    }

    /// The bytes of a non-resident attribute.
    fn read_runs(&self, runs: &[Run], size: u64) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        for run in runs {
            if out.len() as u64 >= size || run.clusters > (64 << 20) / self.cluster() {
                break;
            }
            match run.lcn {
                Some(lcn) => out.extend_from_slice(&self.read_clusters(lcn * self.cluster(), run.clusters * self.cluster())?),
                None => out.resize(out.len() + (run.clusters * self.cluster()) as usize, 0),
            }
        }
        out.truncate(size as usize);
        Ok(out)
    }

    /// The journal's position, or None when the volume has none.
    pub fn journal(&self) -> io::Result<Option<Journal>> {
        let mut data = USN_JOURNAL_DATA_V0::default();
        match ioctl(&self.handle, FSCTL_QUERY_USN_JOURNAL, &[], as_bytes_mut(&mut data)) {
            Ok(_) => Ok(Some(Journal { id: data.UsnJournalID, first_usn: data.FirstUsn, next_usn: data.NextUsn })),
            Err(e) if matches!(e.raw_os_error(), Some(ERROR_JOURNAL_NOT_ACTIVE | ERROR_JOURNAL_DELETE_IN_PROGRESS)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Turns the journal on, with Windows' own default size (32 MiB). That changes the volume, so it
    /// takes a handle with write access.
    pub fn create_journal(&self) -> io::Result<()> {
        let path = wide(format!(r"\\.\{}:", self.letter));
        let handle = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                Default::default(),
                None,
            )
        }
        .map_err(os_error)?;
        let input = CREATE_USN_JOURNAL_DATA { MaximumSize: 32 << 20, AllocationDelta: 8 << 20 };
        ioctl(&Handle(handle), FSCTL_CREATE_USN_JOURNAL, as_bytes(&input), &mut []).map(|_| ())
    }

    /// Reads journal records from `start` into `buf`; returns the bytes filled (the next USN, then
    /// records, as `usn::parse_page` takes them).
    pub fn read_journal(&self, start: i64, id: u64, buf: &mut [u8]) -> Result<usize, JournalError> {
        let input = READ_USN_JOURNAL_DATA_V1 {
            StartUsn: start,
            ReasonMask: u32::MAX,
            ReturnOnlyOnClose: 0,
            Timeout: 0,
            BytesToWaitFor: 0,
            UsnJournalID: id,
            MinMajorVersion: 2,
            MaxMajorVersion: 3,
        };
        ioctl(&self.handle, FSCTL_READ_USN_JOURNAL, as_bytes(&input), buf).map_err(|e| match e.raw_os_error() {
            Some(ERROR_JOURNAL_ENTRY_DELETED | ERROR_JOURNAL_DELETE_IN_PROGRESS | ERROR_JOURNAL_NOT_ACTIVE | ERROR_INVALID_PARAMETER) => {
                JournalError::Lost
            }
            _ => JournalError::Io(e),
        })
    }
}

/// Adds one scanned record to the index being built: a base record as itself, an extension record's
/// names and data size to its base record.
fn merge(builder: &mut Builder, number: u64, parsed: Parsed) {
    if !parsed.in_use || number >= u32::MAX as u64 {
        return;
    }
    let owner = if parsed.base == 0 {
        let number = number as u32;
        builder.record(number, parsed.seq, parsed.dir, parsed.attrs, parsed.modified.unwrap_or(0));
        number
    } else {
        match split_ref(parsed.base).0 {
            base if base < u32::MAX as u64 => base as u32,
            _ => return,
        }
    };
    if let Some(size) = parsed.size {
        builder.size(owner, size);
    }
    for name in parsed.names {
        builder.name(owner, name.parent, &name.name);
    }
}

fn as_bytes<T>(value: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts(value as *const T as *const u8, size_of::<T>()) }
}

fn as_bytes_mut<T>(value: &mut T) -> &mut [u8] {
    unsafe { std::slice::from_raw_parts_mut(value as *mut T as *mut u8, size_of::<T>()) }
}

/// DeviceIoControl on a synchronous handle; returns the bytes written to `output`.
fn ioctl(handle: &Handle, code: u32, input: &[u8], output: &mut [u8]) -> io::Result<usize> {
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            handle.0,
            code,
            (!input.is_empty()).then_some(input.as_ptr() as *const _),
            input.len() as u32,
            (!output.is_empty()).then_some(output.as_mut_ptr() as *mut _),
            output.len() as u32,
            Some(&mut returned),
            None,
        )
    }
    .map_err(os_error)?;
    Ok(returned as usize)
}
