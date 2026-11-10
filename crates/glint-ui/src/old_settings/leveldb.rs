//! A minimal, read-only LevelDB reader: just enough to take the key/value pairs out of the folder WebView2
//! keeps `localStorage` in. It never writes, and a damaged file gives what could be read plus a note of what
//! couldn't, never a panic.
//!
//! LevelDB keeps recent writes in a log file (`.log`) and older ones in sorted table files (`.ldb`, or `.sst`
//! in older versions). Every write has a sequence number, and for a key the highest one wins; a delete is a
//! write that says "gone". So reading a database is reading every file and keeping, per key, the newest write
//! ([`Db`]). The MANIFEST (which files are live) is not needed for that: a file that compaction left behind
//! only holds older writes, which lose.
//!
//! **Log files** are 32 KiB blocks of records `crc(4) length(2) type(1) data`, where the type says whether the
//! data is a whole record (FULL) or the first, a middle or the last piece of one that goes on in the next
//! block. A record is a write batch: `sequence(8) count(4)` and then `count` entries, each a put (`1`, key,
//! value) or a delete (`0`, key) with lengths as varints. Entry `i` has sequence `sequence + i`.
//!
//! **Table files** end in a 48-byte footer (two block handles and a magic number). The index block lists the
//! data blocks; a block is `entries... restarts... count` with each key stored as the part it shares with the
//! key before it plus the rest, and is followed by a type byte (0 plain, 1 Snappy) and a CRC. A key in a table
//! is the user's key plus 8 bytes: `sequence << 8 | kind` (1 put, 0 delete).

use super::snappy;
use std::collections::HashMap;

/// A log file's block size.
const LOG_BLOCK: usize = 32 * 1024;
/// A log record header: checksum, length, type.
const LOG_HEADER: usize = 7;
/// A table file's footer.
const FOOTER: usize = 48;
/// The last 8 bytes of a table file: `0xdb4775248b80fb57` little-endian.
const TABLE_MAGIC: [u8; 8] = [0x57, 0xfb, 0x80, 0x8b, 0x24, 0x75, 0x47, 0xdb];
/// The most one log record may put together from its pieces.
const MAX_RECORD: usize = 64 << 20;

/// CRC-32C (Castagnoli), as LevelDB checksums with.
pub fn crc32c(data: &[u8]) -> u32 {
    // The reflected polynomial, bit by bit: slow, but the files are small and read once.
    let mut crc: u32 = !0;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0x82f6_3b78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A checksum as stored on disk (LevelDB rotates and offsets them so a checksum of a checksum differs).
pub fn mask_crc(crc: u32) -> u32 {
    crc.rotate_right(15).wrapping_add(0xa282_ead8)
}

/// Reads a varint of up to 64 bits at `*pos`, moving `*pos` past it.
fn varint(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value: u64 = 0;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// `len` bytes at `*pos`, moving `*pos` past them.
fn take<'a>(bytes: &'a [u8], pos: &mut usize, len: u64) -> Option<&'a [u8]> {
    let len = usize::try_from(len).ok()?;
    let end = pos.checked_add(len)?;
    let slice = bytes.get(*pos..end)?;
    *pos = end;
    Some(slice)
}

fn u32_le(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_le(bytes: &[u8]) -> Option<u64> {
    let b: [u8; 8] = bytes.get(..8)?.try_into().ok()?;
    Some(u64::from_le_bytes(b))
}

// ---- The merged view ---------------------------------------------------------------------------------------------

/// What a database says, after every file is read: for each key (that `keep` accepts) its newest write.
pub struct Db {
    keep: fn(&[u8]) -> bool,
    /// Key, then the newest write's sequence and its value (`None` for a delete).
    newest: HashMap<Vec<u8>, (u64, Option<Vec<u8>>)>,
    /// What couldn't be read, for the log.
    pub problems: Vec<String>,
}

impl Db {
    /// An empty view that keeps only the keys `keep` says yes to (so other origins' data is never held).
    pub fn new(keep: fn(&[u8]) -> bool) -> Self {
        Db {
            keep,
            newest: HashMap::new(),
            problems: Vec::new(),
        }
    }

    /// One write: kept if it is the newest this key has had.
    fn add(&mut self, seq: u64, key: &[u8], value: Option<&[u8]>) {
        if !(self.keep)(key) {
            return;
        }
        match self.newest.get(key) {
            Some((have, _)) if *have >= seq => {}
            _ => {
                self.newest.insert(key.to_vec(), (seq, value.map(<[u8]>::to_vec)));
            }
        }
    }

    /// The keys that are there (not deleted) with their values and the sequence of the write that put them.
    pub fn live(&self) -> impl Iterator<Item = (&[u8], u64, &[u8])> {
        self.newest
            .iter()
            .filter_map(|(key, (seq, value))| value.as_deref().map(|v| (key.as_slice(), *seq, v)))
    }

    /// The value of `key`, if it is there and not deleted.
    #[cfg(test)]
    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.newest.get(key).and_then(|(_, v)| v.as_deref())
    }

    /// Reads a log file's bytes into the view.
    pub fn read_log(&mut self, bytes: &[u8]) {
        let mut batches: Vec<Vec<u8>> = Vec::new();
        let problems = log_batches(bytes, &mut |batch| batches.push(batch));
        self.problems.extend(problems);
        for batch in &batches {
            if let Err(why) = self.add_batch(batch) {
                self.problems.push(why);
            }
        }
    }

    /// One write batch: `sequence(8) count(4)` and the entries. Entries before a bad one are kept.
    fn add_batch(&mut self, batch: &[u8]) -> Result<(), String> {
        let bad = || "a write batch in the log is damaged".to_string();
        let seq = u64_le(batch).ok_or_else(bad)?;
        let count = u32_le(batch, 8).ok_or_else(bad)?;
        let mut pos = 12;
        for i in 0..u64::from(count) {
            let kind = *batch.get(pos).ok_or_else(bad)?;
            pos += 1;
            let key_len = varint(batch, &mut pos).ok_or_else(bad)?;
            let key = take(batch, &mut pos, key_len).ok_or_else(bad)?;
            match kind {
                1 => {
                    let value_len = varint(batch, &mut pos).ok_or_else(bad)?;
                    let value = take(batch, &mut pos, value_len).ok_or_else(bad)?;
                    self.add(seq.wrapping_add(i), key, Some(value));
                }
                0 => self.add(seq.wrapping_add(i), key, None),
                _ => return Err(bad()),
            }
        }
        Ok(())
    }

    /// Reads a table file's bytes into the view. An error when the file isn't a table at all; blocks that
    /// can't be read are noted in `problems` and the rest is still taken.
    pub fn read_table(&mut self, bytes: &[u8]) -> Result<(), String> {
        let footer_at = bytes.len().checked_sub(FOOTER).ok_or("too short to be a table file")?;
        let footer = &bytes[footer_at..];
        if footer[FOOTER - 8..] != TABLE_MAGIC {
            return Err("not a table file (wrong magic number)".to_string());
        }
        let mut pos = 0;
        let bad = || "the table's footer is damaged".to_string();
        // The metaindex handle comes first and isn't needed.
        let _ = (
            varint(footer, &mut pos).ok_or_else(bad)?,
            varint(footer, &mut pos).ok_or_else(bad)?,
        );
        let index_offset = varint(footer, &mut pos).ok_or_else(bad)?;
        let index_size = varint(footer, &mut pos).ok_or_else(bad)?;
        let index = read_block(bytes, index_offset, index_size).map_err(|e| format!("the table's index: {e}"))?;
        let mut handles: Vec<(u64, u64)> = Vec::new();
        walk_block(&index, &mut |_, value| {
            let mut p = 0;
            if let (Some(offset), Some(size)) = (varint(value, &mut p), varint(value, &mut p)) {
                handles.push((offset, size));
            }
        })
        .map_err(|e| format!("the table's index: {e}"))?;
        for (offset, size) in handles {
            let block = match read_block(bytes, offset, size) {
                Ok(block) => block,
                Err(why) => {
                    self.problems.push(format!("a data block at {offset}: {why}"));
                    continue;
                }
            };
            let walked = walk_block(&block, &mut |internal, value| self.add_internal(internal, value));
            if let Err(why) = walked {
                self.problems.push(format!("a data block at {offset}: {why}"));
            }
        }
        Ok(())
    }

    /// One table entry: the user's key with `sequence << 8 | kind` after it.
    fn add_internal(&mut self, internal: &[u8], value: &[u8]) {
        let Some(split) = internal.len().checked_sub(8) else {
            return;
        };
        let (key, tag) = internal.split_at(split);
        let Some(tag) = u64_le(tag) else { return };
        match tag & 0xff {
            1 => self.add(tag >> 8, key, Some(value)),
            0 => self.add(tag >> 8, key, None),
            _ => {}
        }
    }
}

// ---- Log files -----------------------------------------------------------------------------------------------------

/// Hands each complete record of a log file (a write batch) to `each`. Returns what was wrong: a record whose
/// checksum fails is dropped, a torn end (a crash while writing) just ends the file.
fn log_batches(bytes: &[u8], each: &mut dyn FnMut(Vec<u8>)) -> Vec<String> {
    let mut problems = Vec::new();
    let mut pos = 0;
    let mut pieces: Option<Vec<u8>> = None;
    while pos < bytes.len() {
        let left_in_block = LOG_BLOCK - pos % LOG_BLOCK;
        if left_in_block < LOG_HEADER {
            // The block's tail is too short for a header: zeros.
            pos += left_in_block;
            continue;
        }
        let Some(header) = bytes.get(pos..pos + LOG_HEADER) else {
            break;
        };
        let stored = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let len = usize::from(u16::from_le_bytes([header[4], header[5]]));
        let kind = header[6];
        if kind == 0 && len == 0 {
            // Padding (or a preallocated, never written part): the rest of the block.
            pos += left_in_block;
            continue;
        }
        if len > left_in_block - LOG_HEADER {
            problems.push("a log record runs past its block; the rest of the block was skipped".to_string());
            pos += left_in_block;
            pieces = None;
            continue;
        }
        let start = pos + LOG_HEADER;
        let Some(data) = bytes.get(start..start + len) else {
            // The file ends in the middle of a record: the write was cut short.
            break;
        };
        pos = start + len;
        let mut checked = Vec::with_capacity(len + 1);
        checked.push(kind);
        checked.extend_from_slice(data);
        if mask_crc(crc32c(&checked)) != stored {
            problems.push("a log record failed its checksum and was skipped".to_string());
            pieces = None;
            continue;
        }
        match kind {
            1 => {
                pieces = None;
                each(data.to_vec());
            }
            2 => pieces = Some(data.to_vec()),
            3 | 4 => {
                let Some(mut whole) = pieces.take() else {
                    problems.push("a log record continues one that was lost".to_string());
                    continue;
                };
                if whole.len() + data.len() > MAX_RECORD {
                    problems.push("a log record is too big".to_string());
                    continue;
                }
                whole.extend_from_slice(data);
                if kind == 4 {
                    each(whole);
                } else {
                    pieces = Some(whole);
                }
            }
            _ => problems.push("a log record of an unknown kind was skipped".to_string()),
        }
    }
    problems
}

// ---- Table files ---------------------------------------------------------------------------------------------------

/// The block at `offset` of `size` bytes (not counting its 5-byte trailer), checked and decompressed.
fn read_block(file: &[u8], offset: u64, size: u64) -> Result<Vec<u8>, String> {
    let mut pos = usize::try_from(offset).map_err(|_| "offset too big".to_string())?;
    let contents = take(file, &mut pos, size).ok_or("runs past the end of the file")?;
    let trailer = file.get(pos..pos.saturating_add(5)).ok_or("no trailer")?;
    let kind = trailer[0];
    let stored = u32::from_le_bytes([trailer[1], trailer[2], trailer[3], trailer[4]]);
    let mut checked = Vec::with_capacity(contents.len() + 1);
    checked.extend_from_slice(contents);
    checked.push(kind);
    if mask_crc(crc32c(&checked)) != stored {
        return Err("failed its checksum".to_string());
    }
    match kind {
        0 => Ok(contents.to_vec()),
        1 => snappy::decompress(contents),
        2 => Err("compressed with Zstandard, which this reader doesn't know".to_string()),
        other => Err(format!("compressed in an unknown way ({other})")),
    }
}

/// Calls `each(key, value)` for every entry of a block, rebuilding each key from the one before it.
fn walk_block(block: &[u8], each: &mut dyn FnMut(&[u8], &[u8])) -> Result<(), String> {
    let bad = || "a block is damaged".to_string();
    let count_at = block.len().checked_sub(4).ok_or_else(bad)?;
    let restarts = usize::try_from(u32_le(block, count_at).ok_or_else(bad)?).map_err(|_| bad())?;
    let data_end = restarts
        .checked_mul(4)
        .and_then(|n| count_at.checked_sub(n))
        .ok_or_else(bad)?;
    let entries = &block[..data_end];
    let mut pos = 0;
    let mut key: Vec<u8> = Vec::new();
    while pos < entries.len() {
        let shared = usize::try_from(varint(entries, &mut pos).ok_or_else(bad)?).map_err(|_| bad())?;
        let unshared = varint(entries, &mut pos).ok_or_else(bad)?;
        let value_len = varint(entries, &mut pos).ok_or_else(bad)?;
        if shared > key.len() {
            return Err(bad());
        }
        let rest = take(entries, &mut pos, unshared).ok_or_else(bad)?;
        key.truncate(shared);
        key.extend_from_slice(rest);
        let value = take(entries, &mut pos, value_len).ok_or_else(bad)?;
        each(&key, value);
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod build {
    //! Makes log and table files the way LevelDB does, so the tests have something to read.
    use super::*;

    /// A varint.
    pub fn put_varint(out: &mut Vec<u8>, mut n: u64) {
        loop {
            let low = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(low);
                return;
            }
            out.push(low | 0x80);
        }
    }

    /// A table entry to build: user key, sequence, value (`None` for a delete).
    pub type Write<'a> = (&'a [u8], u64, Option<&'a [u8]>);

    /// One write of a batch.
    pub enum Op<'a> {
        Put(&'a [u8], &'a [u8]),
        Delete(&'a [u8]),
    }

    /// A write batch starting at sequence `seq`.
    pub fn batch(seq: u64, ops: &[Op]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&(ops.len() as u32).to_le_bytes());
        for op in ops {
            match op {
                Op::Put(key, value) => {
                    out.push(1);
                    put_varint(&mut out, key.len() as u64);
                    out.extend_from_slice(key);
                    put_varint(&mut out, value.len() as u64);
                    out.extend_from_slice(value);
                }
                Op::Delete(key) => {
                    out.push(0);
                    put_varint(&mut out, key.len() as u64);
                    out.extend_from_slice(key);
                }
            }
        }
        out
    }

    /// A log file holding `records`, split over 32 KiB blocks as LevelDB does.
    pub fn log(records: &[Vec<u8>]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        for record in records {
            let mut rest = record.as_slice();
            let mut first = true;
            loop {
                let left = LOG_BLOCK - out.len() % LOG_BLOCK;
                if left < LOG_HEADER {
                    out.extend(std::iter::repeat_n(0u8, left));
                    continue;
                }
                let fit = rest.len().min(left - LOG_HEADER);
                let last = fit == rest.len();
                let kind = match (first, last) {
                    (true, true) => 1u8,
                    (true, false) => 2,
                    (false, false) => 3,
                    (false, true) => 4,
                };
                let mut checked = vec![kind];
                checked.extend_from_slice(&rest[..fit]);
                out.extend_from_slice(&mask_crc(crc32c(&checked)).to_le_bytes());
                out.extend_from_slice(&(fit as u16).to_le_bytes());
                out.push(kind);
                out.extend_from_slice(&rest[..fit]);
                rest = &rest[fit..];
                first = false;
                if last {
                    break;
                }
            }
        }
        out
    }

    /// A block of `entries` (sorted), a restart every `interval` entries.
    pub fn block(entries: &[(Vec<u8>, Vec<u8>)], interval: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut restarts: Vec<u32> = vec![0];
        let mut last: &[u8] = &[];
        let mut counter = 0;
        for (key, value) in entries {
            let shared = if counter < interval {
                last.iter().zip(key.iter()).take_while(|(a, b)| a == b).count()
            } else {
                restarts.push(out.len() as u32);
                counter = 0;
                0
            };
            put_varint(&mut out, shared as u64);
            put_varint(&mut out, (key.len() - shared) as u64);
            put_varint(&mut out, value.len() as u64);
            out.extend_from_slice(&key[shared..]);
            out.extend_from_slice(value);
            last = key;
            counter += 1;
        }
        for restart in &restarts {
            out.extend_from_slice(&restart.to_le_bytes());
        }
        out.extend_from_slice(&(restarts.len() as u32).to_le_bytes());
        out
    }

    /// `contents` followed by its type byte and checksum, appended to `file`; the handle it can be found by.
    fn write_block(file: &mut Vec<u8>, contents: &[u8], snappy: bool) -> Vec<u8> {
        let (body, kind) = if snappy {
            (super::super::snappy::compress_literals(contents), 1u8)
        } else {
            (contents.to_vec(), 0u8)
        };
        let offset = file.len() as u64;
        file.extend_from_slice(&body);
        file.push(kind);
        let mut checked = body.clone();
        checked.push(kind);
        file.extend_from_slice(&mask_crc(crc32c(&checked)).to_le_bytes());
        let mut handle = Vec::new();
        put_varint(&mut handle, offset);
        put_varint(&mut handle, body.len() as u64);
        handle
    }

    /// A user key with the tag a table stores after it.
    pub fn internal(key: &[u8], seq: u64, put: bool) -> Vec<u8> {
        let mut out = key.to_vec();
        out.extend_from_slice(&((seq << 8) | u64::from(put)).to_le_bytes());
        out
    }

    /// A table file with the given writes (user key, sequence, value or `None` for a delete), `per_block`
    /// entries to a data block, each data block Snappy-compressed when `snappy`.
    pub fn table(writes: &[Write], per_block: usize, snappy: bool) -> Vec<u8> {
        let mut sorted: Vec<(Vec<u8>, Vec<u8>)> = writes
            .iter()
            .map(|(k, seq, v)| (internal(k, *seq, v.is_some()), v.unwrap_or_default().to_vec()))
            .collect();
        sorted.sort();
        let mut file = Vec::new();
        let mut index: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for chunk in sorted.chunks(per_block.max(1)) {
            let handle = write_block(&mut file, &block(chunk, 2), snappy);
            if let Some((last_key, _)) = chunk.last() {
                index.push((last_key.clone(), handle));
            }
        }
        let meta = write_block(&mut file, &block(&[], 16), false);
        let index = write_block(&mut file, &block(&index, 1), false);
        let mut footer = Vec::new();
        footer.extend_from_slice(&meta);
        footer.extend_from_slice(&index);
        footer.resize(FOOTER - 8, 0);
        footer.extend_from_slice(&TABLE_MAGIC);
        file.extend_from_slice(&footer);
        file
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;

    fn everything(_: &[u8]) -> bool {
        true
    }

    #[test]
    fn crc32c_matches_the_standard_check_value() {
        assert_eq!(crc32c(b"123456789"), 0xe306_9283);
        assert_eq!(crc32c(b""), 0);
    }

    #[test]
    fn a_log_gives_its_puts_and_the_newest_write_of_a_key_wins() {
        let log = log(&[
            batch(1, &[Op::Put(b"a", b"one"), Op::Put(b"b", b"first")]),
            batch(3, &[Op::Put(b"a", b"two")]),
        ]);
        let mut db = Db::new(everything);
        db.read_log(&log);
        assert!(db.problems.is_empty(), "{:?}", db.problems);
        assert_eq!(db.get(b"a"), Some(&b"two"[..]));
        assert_eq!(db.get(b"b"), Some(&b"first"[..]));
    }

    #[test]
    fn a_delete_removes_a_key_and_a_later_put_brings_it_back() {
        let mut db = Db::new(everything);
        db.read_log(&log(&[
            batch(1, &[Op::Put(b"gone", b"x"), Op::Put(b"back", b"x")]),
            batch(3, &[Op::Delete(b"gone"), Op::Delete(b"back")]),
            batch(5, &[Op::Put(b"back", b"again")]),
        ]));
        assert_eq!(db.get(b"gone"), None);
        assert_eq!(db.get(b"back"), Some(&b"again"[..]));
        assert_eq!(db.live().count(), 1);
    }

    #[test]
    fn the_sequence_decides_not_the_order_the_files_are_read_in() {
        // The newer write is in the table, the older one in the log.
        let newer = table(&[(b"k", 10, Some(&b"new"[..]))], 4, false);
        let older = log(&[batch(2, &[Op::Put(b"k", b"old")])]);
        for tables_first in [true, false] {
            let mut db = Db::new(everything);
            if tables_first {
                db.read_table(&newer).expect("table");
                db.read_log(&older);
            } else {
                db.read_log(&older);
                db.read_table(&newer).expect("table");
            }
            assert_eq!(db.get(b"k"), Some(&b"new"[..]), "tables first: {tables_first}");
        }
        // And a delete in the table beats an older put in the log.
        let deleted = table(&[(b"k", 10, None)], 4, false);
        let mut db = Db::new(everything);
        db.read_log(&older);
        db.read_table(&deleted).expect("table");
        assert_eq!(db.get(b"k"), None);
    }

    #[test]
    fn a_record_longer_than_a_block_is_put_together_from_its_pieces() {
        let big = vec![b'z'; 100_000];
        let bytes = log(&[
            batch(1, &[Op::Put(b"small", b"s")]),
            batch(2, &[Op::Put(b"big", &big)]),
            batch(3, &[Op::Put(b"after", b"a")]),
        ]);
        assert!(bytes.len() > 3 * LOG_BLOCK);
        let mut db = Db::new(everything);
        db.read_log(&bytes);
        assert!(db.problems.is_empty(), "{:?}", db.problems);
        assert_eq!(db.get(b"big").map(<[u8]>::len), Some(100_000));
        assert_eq!(db.get(b"small"), Some(&b"s"[..]));
        assert_eq!(db.get(b"after"), Some(&b"a"[..]));
    }

    #[test]
    fn a_table_with_snappy_blocks_and_prefix_compressed_keys_reads_back() {
        let keys: Vec<Vec<u8>> = (0..40)
            .map(|i| format!("_http://tauri.localhost\\0\\x01key-{i:03}").into_bytes())
            .collect();
        let values: Vec<Vec<u8>> = (0..40).map(|i| format!("value number {i}").into_bytes()).collect();
        let writes: Vec<Write> = keys
            .iter()
            .zip(&values)
            .enumerate()
            .map(|(i, (k, v))| (k.as_slice(), i as u64 + 1, Some(v.as_slice())))
            .collect();
        for snappy in [false, true] {
            let mut db = Db::new(everything);
            db.read_table(&table(&writes, 7, snappy)).expect("table");
            assert!(db.problems.is_empty(), "{:?}", db.problems);
            assert_eq!(db.live().count(), 40, "snappy: {snappy}");
            assert_eq!(db.get(&keys[13]), Some(values[13].as_slice()));
            assert_eq!(db.get(&keys[39]), Some(values[39].as_slice()));
        }
    }

    #[test]
    fn only_the_keys_asked_for_are_held() {
        fn underscore(key: &[u8]) -> bool {
            key.first() == Some(&b'_')
        }
        let mut db = Db::new(underscore);
        db.read_log(&log(&[batch(1, &[Op::Put(b"_yes", b"1"), Op::Put(b"META:no", b"2")])]));
        assert_eq!(db.live().count(), 1);
        assert_eq!(db.get(b"META:no"), None);
    }

    #[test]
    fn damage_gives_what_could_be_read_and_a_note_and_never_a_panic() {
        // A flipped byte in the second record fails its checksum; the first and third are still read.
        let one = batch(1, &[Op::Put(b"a", b"1")]);
        let two = batch(2, &[Op::Put(b"b", b"2")]);
        let three = batch(3, &[Op::Put(b"c", b"3")]);
        let mut bytes = log(&[one.clone(), two, three.clone()]);
        let second_data = LOG_HEADER + one.len() + LOG_HEADER + 14;
        bytes[second_data] ^= 0xff;
        let mut db = Db::new(everything);
        db.read_log(&bytes);
        assert_eq!(db.get(b"a"), Some(&b"1"[..]));
        assert_eq!(db.get(b"b"), None);
        assert_eq!(db.get(b"c"), Some(&b"3"[..]));
        assert_eq!(db.problems.len(), 1);

        // A file cut anywhere reads without a panic and gives only keys that were written.
        let whole = log(&[one, three]);
        for cut in 0..whole.len() {
            let mut db = Db::new(everything);
            db.read_log(&whole[..cut]);
            assert!(db.live().count() <= 2);
        }
        // Noise is not a log.
        let noise: Vec<u8> = (0..5000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let mut db = Db::new(everything);
        db.read_log(&noise);
        assert_eq!(db.live().count(), 0);
    }

    #[test]
    fn a_damaged_or_foreign_table_is_an_error_or_a_note() {
        let mut db = Db::new(everything);
        assert!(db.read_table(b"short").is_err());
        assert!(db.read_table(&[7u8; 200]).is_err());
        // One flipped byte inside a data block: noted, and the other block is still read.
        let writes: Vec<Write> = vec![
            (b"a-key", 1, Some(&b"1"[..])),
            (b"b-key", 2, Some(&b"2"[..])),
            (b"c-key", 3, Some(&b"3"[..])),
            (b"d-key", 4, Some(&b"4"[..])),
        ];
        let mut bytes = table(&writes, 2, false);
        bytes[3] ^= 0xff;
        let mut db = Db::new(everything);
        db.read_table(&bytes).expect("the index is fine");
        assert_eq!(db.problems.len(), 1);
        assert_eq!(db.live().count(), 2);
        // Every cut of a good table is an error or a partial read, never a panic.
        let whole = table(&writes, 2, true);
        for cut in 0..whole.len() {
            let mut db = Db::new(everything);
            let _ = db.read_table(&whole[..cut]);
        }
    }

    #[test]
    fn a_zstd_block_is_a_note_not_a_crash() {
        let body = b"whatever";
        let mut file = body.to_vec();
        file.push(2);
        let mut checked = body.to_vec();
        checked.push(2);
        file.extend_from_slice(&mask_crc(crc32c(&checked)).to_le_bytes());
        let error = read_block(&file, 0, body.len() as u64).expect_err("zstd");
        assert!(error.contains("Zstandard"), "{error}");
    }
}
