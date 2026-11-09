//! Reading cabinet files (.cab), Windows' own archive format: drivers, updates, and the files inside
//! MSI packages. A cabinet holds folders, each one compressed stream made of data blocks of up to
//! 32 KB (stored, MSZIP or LZX), and files, each a name and a place in one folder's stream.
//!
//! Glint reads cabinets itself rather than with the cab crate, which starts a folder over for every
//! file it reads: quadratic for the thousands of files in an MSI's cabinet. Here a folder is read
//! once, front to back, for everything wanted from it, like a solid 7z block.

use super::read::{zip_name, LocalTime};
use super::tree::{Item, UNREADABLE};
use super::Failure;
use std::io::{self, Read, Seek, SeekFrom};

/// What a data block holds at most, decompressed.
const MAX_BLOCK: usize = 32 << 10;
/// The longest name read for a file (the format allows 256 bytes) or a cabinet (in a split set).
const MAX_NAME: usize = 1024;
/// MSZIP keeps the last 32 KB as each block's history.
const MSZIP_HISTORY: usize = 32 << 10;

const PREV_CABINET: u16 = 0x0001;
const NEXT_CABINET: u16 = 0x0002;
const RESERVE_PRESENT: u16 = 0x0004;
const NAME_IS_UTF8: u16 = 0x0080;

/// Whether `head` starts like a cabinet: "MSCF" and four zero bytes.
pub fn is_cabinet(head: &[u8]) -> bool {
    head.starts_with(b"MSCF\0\0\0\0")
}

/// How a folder is compressed: the low four bits of its type, and LZX's window in bits 8 to 12.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Stored,
    MsZip,
    Quantum,
    Lzx(u8),
    Unknown(u16),
}

impl Method {
    fn of(kind: u16) -> Method {
        match kind & 0x000F {
            0 => Method::Stored,
            1 => Method::MsZip,
            2 => Method::Quantum,
            3 => Method::Lzx(((kind >> 8) & 0x1F) as u8),
            _ => Method::Unknown(kind),
        }
    }

    /// Why Glint can't decompress it, if it can't.
    fn problem(self) -> Option<Failure> {
        match self {
            Method::Stored | Method::MsZip => None,
            Method::Lzx(15..=21) => None,
            Method::Lzx(bits) => Some(Failure::Unsupported(format!("LZX with a {bits}-bit window"))),
            Method::Quantum => Some(Failure::Unsupported("Quantum".into())),
            Method::Unknown(kind) => Some(Failure::Unsupported(format!("cabinet compression type {kind}"))),
        }
    }
}

pub struct Folder {
    /// Where its first data block is, and how many it has.
    data_at: u64,
    blocks: u16,
    pub method: Method,
    /// What its blocks take in the cabinet, and what its files add up to once decompressed.
    pub packed: u64,
    pub unpacked: u64,
}

/// A file: what the tree shows of it, and where its bytes are in which folder's stream.
pub struct CabFile {
    pub item: Item,
    pub folder: usize,
    pub offset: u64,
    pub problem: Option<Failure>,
}

pub struct Cabinet {
    pub folders: Vec<Folder>,
    pub files: Vec<CabFile>,
    /// Bytes each data block keeps before its data, as the cabinet's header says.
    block_reserve: u8,
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn damaged(what: &str) -> Failure {
    Failure::Damaged(what.into())
}

fn split() -> Failure {
    Failure::Unreadable("it's split across several cabinet files, which Glint doesn't read yet".into())
}

/// A NUL-ended name, read a byte at a time (the reader is buffered).
fn read_name(r: &mut impl Read) -> Result<Vec<u8>, Failure> {
    let mut name = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        r.read_exact(&mut byte).map_err(|_| damaged("it ends in the middle of its list of files (is it complete?)"))?;
        match byte[0] {
            0 => return Ok(name),
            b if name.len() < MAX_NAME => name.push(b),
            _ => return Err(damaged("a name in its list of files is far too long")),
        }
    }
}

/// Reads a cabinet's header, folders and files (nothing is decompressed).
pub fn read(r: &mut (impl Read + Seek), max_entries: usize) -> Result<Cabinet, Failure> {
    let len = r.seek(SeekFrom::End(0))?;
    r.seek(SeekFrom::Start(0))?;
    let mut head = [0u8; 36];
    r.read_exact(&mut head).map_err(|_| damaged("it ends before its header"))?;
    if !is_cabinet(&head) {
        return Err(damaged("it isn't a cabinet file"));
    }
    let (minor, major) = (head[24], head[25]);
    if major != 1 {
        return Err(Failure::Unreadable(format!("it's a cabinet in format version {major}.{minor}")));
    }
    let files_at = le32(&head, 16) as u64;
    let (folder_count, file_count, flags) = (le16(&head, 26) as usize, le16(&head, 28) as usize, le16(&head, 30));
    if file_count > max_entries {
        return Err(Failure::TooManyEntries(file_count));
    }
    let (mut folder_reserve, mut block_reserve) = (0u8, 0u8);
    if flags & RESERVE_PRESENT != 0 {
        let mut sizes = [0u8; 4];
        r.read_exact(&mut sizes).map_err(|_| damaged("it ends in its header"))?;
        folder_reserve = sizes[2];
        block_reserve = sizes[3];
        r.seek(SeekFrom::Current(le16(&sizes, 0) as i64))?;
    }
    // The names of the cabinets and disks before and after it in a split set.
    let names = 2 * ((flags & PREV_CABINET != 0) as usize + (flags & NEXT_CABINET != 0) as usize);
    for _ in 0..names {
        read_name(r)?;
    }
    let mut spans = Vec::with_capacity(folder_count);
    for _ in 0..folder_count {
        let mut f = [0u8; 8];
        r.read_exact(&mut f).map_err(|_| damaged("it ends in its list of folders"))?;
        r.seek(SeekFrom::Current(folder_reserve as i64))?;
        spans.push((le32(&f, 0) as u64, le16(&f, 4), Method::of(le16(&f, 6))));
    }
    if files_at >= len {
        return Err(damaged("its list of files is past its end"));
    }
    r.seek(SeekFrom::Start(files_at))?;
    let mut local = LocalTime::default();
    let mut files = Vec::with_capacity(file_count);
    for _ in 0..file_count {
        let mut f = [0u8; 16];
        r.read_exact(&mut f).map_err(|_| damaged("it ends in its list of files (is it complete?)"))?;
        let (size, offset, folder, date, time, attribs) = (le32(&f, 0) as u64, le32(&f, 4) as u64, le16(&f, 8), le16(&f, 10), le16(&f, 12), le16(&f, 14));
        let name = zip_name(&read_name(r)?, attribs & NAME_IS_UTF8 != 0);
        let modified = zip::DateTime::try_from_msdos(date, time).map_or(0, |t| local.unix_ms(&t));
        // Only what Windows has: read-only, hidden, system and archive.
        let attrs = (attribs & 0x27) as u32;
        // Files that begin in the cabinet before, or go on into the next, have their folder in code.
        let (folder, problem) = match folder {
            0xFFFD | 0xFFFF => (0, Some(split())),
            0xFFFE => (folder_count.saturating_sub(1), Some(split())),
            n if n as usize >= folder_count => (0, Some(damaged("a file's folder isn't in the cabinet"))),
            n => (n as usize, None),
        };
        files.push(CabFile { item: Item { path: name, dir: false, size, modified, attrs }, folder, offset, problem });
    }
    let folders: Vec<Folder> = spans
        .iter()
        .enumerate()
        .map(|(i, &(data_at, blocks, method))| {
            // Up to the next folder's data (they're in order, as a rule), or the end of the cabinet.
            let end = spans.iter().map(|s| s.0).filter(|&at| at > data_at).min().unwrap_or(len);
            let unpacked = files.iter().filter(|f| f.folder == i).map(|f| f.offset.saturating_add(f.item.size)).max().unwrap_or(0);
            Folder { data_at, blocks, method, packed: end.saturating_sub(data_at), unpacked }
        })
        .collect();
    for file in &mut files {
        // A folder that begins in the cabinet before can't be decompressed without it.
        let continued = flags & PREV_CABINET != 0 && file.folder == 0;
        if file.problem.is_none() {
            file.problem = match folders.get(file.folder) {
                None => Some(damaged("it has files but no folders")),
                Some(_) if continued => Some(split()),
                Some(folder) => folder.method.problem(),
            };
        }
        if file.problem.is_some() {
            file.item.attrs |= UNREADABLE;
        }
    }
    Ok(Cabinet { folders, files, block_reserve })
}

impl Cabinet {
    /// Where folder `folder`'s data starts, to read it from with `FolderReader`.
    pub fn data_at(&self, folder: usize) -> u64 {
        self.folders[folder].data_at
    }
}

/// The cabinet checksum: 32-bit words XORed together, and the last one to three bytes put in
/// highest first.
fn checksum(parts: &[&[u8]]) -> u32 {
    let (mut sum, mut word, mut n) = (0u32, 0u32, 0u32);
    for &b in parts.iter().flat_map(|p| p.iter()) {
        word |= (b as u32) << (8 * n);
        n += 1;
        if n == 4 {
            sum ^= word;
            (word, n) = (0, 0);
        }
    }
    sum ^ match n {
        1 => word & 0xFF,
        2 => ((word & 0xFF) << 8) | ((word >> 8) & 0xFF),
        3 => ((word & 0xFF) << 16) | (word & 0xFF00) | ((word >> 16) & 0xFF),
        _ => 0,
    }
}

fn invalid(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

/// MSZIP: each block is a Deflate stream of its own after "CK", with the last 32 KB before it as
/// its history.
struct MsZip {
    inflate: flate2::Decompress,
    history: Vec<u8>,
}

impl MsZip {
    fn new() -> MsZip {
        MsZip { inflate: flate2::Decompress::new(false), history: Vec::with_capacity(MSZIP_HISTORY) }
    }

    fn decode(&mut self, data: &[u8], size: usize, out: &mut Vec<u8>) -> io::Result<()> {
        let deflated = data.strip_prefix(b"CK").ok_or_else(|| invalid("an MSZIP block doesn't start with its signature"))?;
        self.inflate.reset(false);
        if !self.history.is_empty() {
            self.inflate.set_dictionary(&self.history).map_err(|e| invalid(format!("an MSZIP block can't be decompressed ({e})")))?;
        }
        out.clear();
        out.reserve(size + 1);
        self.inflate.decompress_vec(deflated, out, flate2::FlushDecompress::Finish).map_err(|e| invalid(format!("an MSZIP block can't be decompressed ({e})")))?;
        if out.len() != size {
            return Err(invalid("an MSZIP block holds more or less than it says"));
        }
        let keep = (self.history.len() + out.len()).saturating_sub(MSZIP_HISTORY).min(self.history.len());
        self.history.drain(..keep);
        self.history.extend_from_slice(&out[out.len().saturating_sub(MSZIP_HISTORY)..]);
        Ok(())
    }
}

/// A folder's stream, decompressed a block at a time as it's read. The source must start at the
/// folder's first data block (`Cabinet::data_at`).
pub struct FolderReader<R> {
    source: R,
    method: Method,
    blocks_left: u16,
    reserve: Vec<u8>,
    data: Vec<u8>,
    block: Vec<u8>,
    at: usize,
    mszip: Option<MsZip>,
    lzx: Option<Box<lzxd::Lzxd>>,
}

impl<R: Read> FolderReader<R> {
    pub fn new(source: R, cabinet: &Cabinet, folder: usize) -> FolderReader<R> {
        let f = &cabinet.folders[folder];
        FolderReader { source, method: f.method, blocks_left: f.blocks, reserve: vec![0; cabinet.block_reserve as usize], data: Vec::new(), block: Vec::new(), at: 0, mszip: None, lzx: None }
    }

    /// Decompresses the next data block; false after the last.
    fn next_block(&mut self) -> io::Result<bool> {
        if self.blocks_left == 0 {
            return Ok(false);
        }
        self.blocks_left -= 1;
        let mut head = [0u8; 8];
        self.source.read_exact(&mut head)?;
        let (sum, packed, unpacked) = (le32(&head, 0), le16(&head, 4) as usize, le16(&head, 6) as usize);
        if unpacked == 0 {
            return Err(invalid("its data goes on in the next cabinet file"));
        }
        if unpacked > MAX_BLOCK {
            return Err(invalid("a data block is bigger than cabinets allow"));
        }
        self.source.read_exact(&mut self.reserve)?;
        self.data.resize(packed, 0);
        self.source.read_exact(&mut self.data)?;
        if sum != 0 && checksum(&[&self.reserve, &self.data]) ^ (packed as u32 | (unpacked as u32) << 16) != sum {
            return Err(invalid("a data block's checksum doesn't match"));
        }
        self.at = 0;
        match self.method {
            Method::Stored if packed == unpacked => {
                self.block.clear();
                self.block.extend_from_slice(&self.data);
            }
            Method::Stored => return Err(invalid("a stored data block holds more or less than it says")),
            Method::MsZip => self.mszip.get_or_insert_with(MsZip::new).decode(&self.data, unpacked, &mut self.block)?,
            Method::Lzx(bits) => {
                let window = match bits {
                    15 => lzxd::WindowSize::KB32,
                    16 => lzxd::WindowSize::KB64,
                    17 => lzxd::WindowSize::KB128,
                    18 => lzxd::WindowSize::KB256,
                    19 => lzxd::WindowSize::KB512,
                    20 => lzxd::WindowSize::MB1,
                    21 => lzxd::WindowSize::MB2,
                    _ => return Err(invalid(format!("LZX with a {bits}-bit window"))),
                };
                let lzx = self.lzx.get_or_insert_with(|| Box::new(lzxd::Lzxd::new(window)));
                let out = lzx.decompress_next(&self.data, unpacked).map_err(|e| invalid(format!("an LZX block can't be decompressed ({e})")))?;
                if out.len() != unpacked {
                    return Err(invalid("an LZX block holds more or less than it says"));
                }
                self.block.clear();
                self.block.extend_from_slice(out);
            }
            Method::Quantum | Method::Unknown(_) => return Err(invalid("it's compressed in a way Glint can't read")),
        }
        Ok(true)
    }
}

impl<R: Read> Read for FolderReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.at >= self.block.len() {
            if !self.next_block()? {
                return Ok(0);
            }
        }
        let n = buf.len().min(self.block.len() - self.at);
        buf[..n].copy_from_slice(&self.block[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_like_the_cabinet_spec() {
        // The cab crate's vectors: the two size fields, then the data.
        assert_eq!(checksum(&[b"\x0e\0\x0e\0Hello, world!\n"]), 0x7f2e1a4c);
        assert_eq!(checksum(&[b"\x1d\0\x1d\0Hello, world!\n", b"See you later!\n"]), 0x3509541a);
        assert_eq!(checksum(&[b""]), 0);
    }

    #[test]
    fn knows_the_methods() {
        assert_eq!(Method::of(0x1503), Method::Lzx(21));
        assert_eq!(Method::of(1), Method::MsZip);
        assert!(Method::of(2).problem().is_some_and(|p| p.message().contains("Quantum")));
        assert!(Method::of(0x1603).problem().is_some(), "a 22-bit LZX window is past what cabinets use");
        assert!(Method::of(0x0F03).problem().is_none());
    }
}
