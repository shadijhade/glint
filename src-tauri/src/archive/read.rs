//! Reading zip and 7z archives: their list of entries (the zip central directory, the 7z header),
//! and the data of the entries asked for, streamed out one at a time.

use super::codec::{Crc32Checked, Zstd};
use super::tree::{Item, ENCRYPTED, UNREADABLE};
use super::Failure;
use sevenz_rust2::{Archive as SevenZ, BlockDecoder, EncoderMethod, Password};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use zip::read::ZipArchiveMetadata;
use zip::{CompressionMethod, HasZipMetadata, ZipArchive};

/// How Glint reads an archive. Its `code` names it to the UI: in a listing's header, and as the
/// engine of a disk-usage map (see `entries.ts` and `usage.ts`, which list the same labels in order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    SevenZ,
}

/// What a file's name says about it: Enter opens it like a folder (`Browse`), or opens it in its app
/// while "Open as archive" looks inside (`OnRequest`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    Browse,
    OnRequest,
}

/// Files that are zips by another name: Office and OpenDocument files, Java and Android packages,
/// NuGet packages, VSIX extensions, Python wheels, EPUBs, MSIX and AppX packages, Firefox add-ons,
/// KMZ and 3MF. Enter opens them in their app; "Open as archive" looks inside.
const ZIP_INSIDE: [&str; 36] = [
    "docx", "docm", "dotx", "dotm", "xlsx", "xlsm", "xltx", "xltm", "pptx", "pptm", "potx", "potm", "odt", "ods", "odp", "odg", "ott", "ots", "otp", "jar", "war", "ear", "apk", "aab", "nupkg", "vsix",
    "whl", "epub", "msix", "msixbundle", "appx", "appxbundle", "xpi", "kmz", "3mf", "ipa",
];

impl Format {
    /// What a name claims, by its extension. Every folder listed asks for every part of its path, so
    /// no copy of the name is made.
    pub fn claimed(name: &str) -> Option<(Format, Claim)> {
        let ext = name.rsplit_once('.').map(|(stem, ext)| (!stem.is_empty()).then_some(ext))??;
        if ext.eq_ignore_ascii_case("zip") {
            Some((Format::Zip, Claim::Browse))
        } else if ext.eq_ignore_ascii_case("7z") {
            Some((Format::SevenZ, Claim::Browse))
        } else if ZIP_INSIDE.iter().any(|z| ext.eq_ignore_ascii_case(z)) {
            Some((Format::Zip, Claim::OnRequest))
        } else {
            None
        }
    }

    /// What Enter opens like a folder, by the name.
    pub fn of(name: &str) -> Option<Format> {
        Format::claimed(name).filter(|&(_, claim)| claim == Claim::Browse).map(|(format, _)| format)
    }

    /// What a file is, by its first bytes, whatever it's called.
    pub fn detect(head: &[u8]) -> Option<Format> {
        if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
            Some(Format::Zip)
        } else if head.starts_with(b"7z\xBC\xAF\x27\x1C") {
            Some(Format::SevenZ)
        } else {
            None
        }
    }

    pub fn code(self) -> u8 {
        match self {
            Format::Zip => 0,
            Format::SevenZ => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::SevenZ => "7z",
        }
    }
}

/// Archives with more entries than this aren't opened: each takes memory for as long as it's open.
pub const MAX_ENTRIES: usize = 2_000_000;
/// The most a 7z header (its list of entries) may take once unpacked. A real one for a million
/// entries takes tens of megabytes; a crafted one could ask for more memory than the PC has.
pub const MAX_7Z_HEADER: u64 = 512 << 20;
/// The most of a zip's end (its central directory and what follows) kept in memory while it's read.
const MAX_ZIP_DIRECTORY: u64 = 1 << 30;

/// An archive's data, ready to extract from.
pub enum Source {
    Zip(ZipSource),
    SevenZ(Box<SevenZSource>),
}

pub struct ZipSource {
    meta: Arc<ZipArchiveMetadata>,
    /// Whether two entries share bytes: a trick of zip bombs (many entries pointing at the same
    /// compressed data) that real archives never use.
    pub overlapping: bool,
}

pub struct SevenZSource {
    archive: SevenZ,
    /// Each entry's block and how much of the block comes before it.
    place: Vec<Option<(usize, u64)>>,
    /// Each block's unpacked and packed sizes, and what keeps it from being read (encryption, a method).
    blocks: Vec<BlockInfo>,
}

#[derive(Debug, Clone, Default)]
struct BlockInfo {
    unpacked: u64,
    packed: u64,
    problem: Option<Failure>,
}

/// One entry to read: its index in the archive.
pub type ItemIndex = usize;

impl Source {
    /// Reads the archive's list of entries.
    pub fn open(path: &Path, format: Format) -> Result<(Source, Vec<Item>), Failure> {
        match format {
            Format::Zip => open_zip(path),
            Format::SevenZ => open_7z(path),
        }
    }

    /// What extracting `item` costs, in bytes to decompress: its size, plus, in a solid 7z block,
    /// everything before it in the block (which must be decompressed to reach it).
    pub fn cost(&self, item: ItemIndex, size: u64) -> u64 {
        match self {
            Source::Zip(_) => size,
            Source::SevenZ(s) => s.place.get(item).copied().flatten().map_or(size, |(_, before)| before.saturating_add(size)),
        }
    }

    /// The 7z block `item` is in (None in a zip, or for an empty 7z file).
    pub fn block_of(&self, item: ItemIndex) -> Option<usize> {
        match self {
            Source::Zip(_) => None,
            Source::SevenZ(s) => s.place.get(item).copied().flatten().map(|(block, _)| block),
        }
    }

    /// How many times over `item` expands what's stored of it (a 7z block's for a 7z entry).
    pub fn ratio(&self, item: ItemIndex, size: u64) -> u64 {
        match self {
            Source::Zip(z) => z.meta.entry(item).map_or(1, |e| size / e.compressed_size().max(1)),
            Source::SevenZ(s) => match s.place.get(item).copied().flatten() {
                Some((block, _)) => s.blocks[block].unpacked / s.blocks[block].packed.max(1),
                None => 1,
            },
        }
    }

    /// Why `item` can't be extracted at all, if it can't: encrypted, an unknown method, or (zip) an
    /// archive whose entries overlap.
    pub fn problem(&self, item: ItemIndex) -> Option<Failure> {
        match self {
            Source::Zip(z) => z.problem(item),
            Source::SevenZ(s) => s.place.get(item).copied().flatten().and_then(|(block, _)| s.blocks[block].problem.clone()),
        }
    }

    /// Streams the entries in `items` out of the archive file at `file`, in the order cheapest to
    /// read (by position in a zip, block by block in a 7z, where everything before an entry in its
    /// block has to be read through). `each` gets each one's reader, which yields its bytes and
    /// fails at the end if they don't match the archive's checksum; returning false stops. Returns
    /// a failure for each item `each` didn't get to (or that failed before it could be read).
    pub fn read_each(&self, file: &Path, items: &[ItemIndex], each: &mut dyn FnMut(ItemIndex, &mut dyn Read) -> bool, skipped: &mut dyn FnMut(u64) -> bool) -> Vec<(ItemIndex, Failure)> {
        match self {
            Source::Zip(z) => z.read_each(file, items, each),
            Source::SevenZ(s) => s.read_each(file, items, each, skipped),
        }
    }
}

// Zip

/// Serves reads from the end of a zip file (its central directory and the records after it) out of
/// memory: the zip crate reads the directory a few bytes at a time with a seek between, which makes
/// 50,000 entries take 150 ms straight from the file, and 13 ms this way.
struct ZipTail {
    file: File,
    len: u64,
    start: u64,
    tail: Vec<u8>,
    pos: u64,
}

impl ZipTail {
    fn open(path: &Path) -> io::Result<ZipTail> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        // The end-of-directory record sits in the last 64 KiB (plus its own 22 bytes); with a ZIP64
        // record before it, the directory's size is in that one.
        let probe = len.min((64 << 10) + 22 + 20 + 56 + 4096);
        let mut tail = vec![0; probe as usize];
        file.seek(SeekFrom::Start(len - probe))?;
        file.read_exact(&mut tail)?;
        let directory = directory_size(&tail).unwrap_or(0).min(MAX_ZIP_DIRECTORY);
        let want = (directory + probe).min(len);
        if want > probe {
            let mut more = vec![0; (want - probe) as usize];
            file.seek(SeekFrom::Start(len - want))?;
            file.read_exact(&mut more)?;
            more.extend_from_slice(&tail);
            tail = more;
        }
        Ok(ZipTail { file, len, start: len - want, tail, pos: 0 })
    }

    /// The bytes of the file from `at`, if they're in memory.
    fn bytes_at(&self, at: u64, len: usize) -> Option<&[u8]> {
        let from = usize::try_from(at.checked_sub(self.start)?).ok()?;
        self.tail.get(from..from.checked_add(len)?)
    }
}

/// The central directory's size, from the end-of-directory record (or its ZIP64 one) in `tail`.
fn directory_size(tail: &[u8]) -> Option<u64> {
    let at = tail.windows(4).rposition(|w| w == b"PK\x05\x06")?;
    let eocd = &tail[at..];
    let size = u32::from_le_bytes(eocd.get(12..16)?.try_into().ok()?);
    if size != u32::MAX {
        return Some(size as u64);
    }
    // ZIP64: the locator (20 bytes) just before, the record before that.
    let record = tail[..at].windows(4).rposition(|w| w == b"PK\x06\x06")?;
    Some(u64::from_le_bytes(tail.get(record + 40..record + 48)?.try_into().ok()?))
}

impl Read for ZipTail {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.start {
            let at = (self.pos - self.start) as usize;
            let n = buf.len().min(self.tail.len().saturating_sub(at));
            buf[..n].copy_from_slice(&self.tail[at..at + n]);
            self.pos += n as u64;
            return Ok(n);
        }
        self.file.seek(SeekFrom::Start(self.pos))?;
        // Never past the start of what's in memory, so the next read comes from there.
        let room = (self.start - self.pos).min(buf.len() as u64) as usize;
        let n = self.file.read(&mut buf[..room])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for ZipTail {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek out of range"))?;
        Ok(self.pos)
    }
}

/// Zstandard: method 93, and 20, its number before 2020. The zip crate's own Zstandard support is C,
/// so it's built without it, and Glint decodes these entries itself (`codec::Zstd`).
#[allow(deprecated)]
fn is_zstd(method: CompressionMethod) -> bool {
    matches!(method, CompressionMethod::Unsupported(93 | 20))
}

/// Why an entry with this encryption flag and method can't be read, if it can't. (The zip crate marks
/// methods it can't decompress as `Unsupported`, with the method's number.)
#[allow(deprecated)]
fn zip_problem(encrypted: bool, method: CompressionMethod) -> Option<Failure> {
    if encrypted {
        return Some(Failure::Encrypted);
    }
    match method {
        _ if is_zstd(method) => None,
        CompressionMethod::Unsupported(99) => Some(Failure::Encrypted),
        CompressionMethod::Unsupported(n) => Some(Failure::Unsupported(zip_method_name(n))),
        _ => None,
    }
}

fn zip_method_name(method: u16) -> String {
    match method {
        10 => "PKWARE DCL Implode".into(),
        18 => "IBM TERSE".into(),
        19 => "IBM LZ77".into(),
        94 => "MP3".into(),
        96 => "JPEG (WinZip)".into(),
        97 => "WavPack".into(),
        n => format!("method {n}"),
    }
}

fn open_zip(path: &Path) -> Result<(Source, Vec<Item>), Failure> {
    let tail = ZipTail::open(path)?;
    let archive = ZipArchive::new(tail).map_err(zip_failure)?;
    let meta = archive.metadata();
    let tail = archive.into_inner();
    if meta.len() > MAX_ENTRIES {
        return Err(Failure::TooManyEntries(meta.len()));
    }
    let mut zone = LocalTime::default();
    let mut items = Vec::with_capacity(meta.len());
    let mut spans = Vec::with_capacity(meta.len());
    for i in 0..meta.len() {
        let entry = meta.entry(i).map_err(zip_failure)?;
        let data = entry.get_metadata();
        let name = zip_name(entry.name_raw(), data.flags.as_u16() & 0x800 != 0);
        let extra = tail.bytes_at(data.central_header_start, 46).and_then(|header| {
            let name_len = u16::from_le_bytes([header[28], header[29]]) as usize;
            let extra_len = u16::from_le_bytes([header[30], header[31]]) as usize;
            tail.bytes_at(data.central_header_start + 46 + name_len as u64, extra_len)
        });
        let modified = extra.and_then(precise_time).unwrap_or_else(|| entry.last_modified().map_or(0, |t| zone.unix_ms(&t)));
        let mut attrs = match entry.system() {
            // DOS attributes in the low byte, as Windows and most zip tools write them.
            zip::System::Dos => entry.external_attributes() & 0xFF,
            _ => 0,
        };
        if entry.encrypted() {
            attrs |= ENCRYPTED;
        } else if zip_problem(false, entry.compression()).is_some() {
            attrs |= UNREADABLE;
        }
        let dir = entry.is_dir();
        if !dir && entry.compressed_size() > 0 {
            spans.push((data.header_start, data.header_start.saturating_add(30 + entry.name_raw().len() as u64).saturating_add(entry.compressed_size())));
        }
        items.push(Item { path: name, dir, size: if dir { 0 } else { entry.size() }, modified, attrs });
    }
    spans.sort_unstable();
    let overlapping = spans.windows(2).any(|w| w[1].0 < w[0].1);
    Ok((Source::Zip(ZipSource { meta, overlapping }), items))
}

fn zip_failure(e: zip::result::ZipError) -> Failure {
    use zip::result::ZipError;
    match e {
        ZipError::Io(e) => Failure::from(e),
        ZipError::CompressionMethodNotSupported(n) => Failure::Unsupported(zip_method_name(n)),
        ZipError::UnsupportedArchive(what) if what.contains("multi-disk") => Failure::Unsupported("split into several files (multi-volume)".into()),
        ZipError::UnsupportedArchive(what) if what == ZipError::PASSWORD_REQUIRED => Failure::Encrypted,
        ZipError::UnsupportedArchive(what) => Failure::Unsupported(what.to_string()),
        ZipError::InvalidPassword => Failure::Encrypted,
        // No end-of-directory record: cut short, or not a zip at all.
        ZipError::InvalidArchive(what) if what.contains("EOCD") => Failure::Damaged("its list of files, at the end of the file, is missing (is it complete, and is it a zip file?)".into()),
        other => Failure::Damaged(other.to_string()),
    }
}

/// A name as the zip stores it: UTF-8 when flagged so (or when it simply is valid UTF-8, as many
/// tools write it without the flag); otherwise the PC's OEM code page, which is what File Explorer
/// and 7-Zip use for names from old Windows zip tools.
fn zip_name(raw: &[u8], utf8: bool) -> String {
    if let Ok(name) = std::str::from_utf8(raw) {
        return name.to_string();
    }
    if utf8 {
        return String::from_utf8_lossy(raw).into_owned();
    }
    oem_name(raw)
}

#[cfg(windows)]
fn oem_name(raw: &[u8]) -> String {
    use windows::Win32::Globalization::{MultiByteToWideChar, CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS};
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    let len = unsafe { MultiByteToWideChar(CP_OEMCP, flags, raw, None) };
    if len <= 0 {
        return String::from_utf8_lossy(raw).into_owned();
    }
    let mut wide = vec![0u16; len as usize];
    let written = unsafe { MultiByteToWideChar(CP_OEMCP, flags, raw, Some(&mut wide)) };
    String::from_utf16_lossy(&wide[..written.max(0) as usize])
}

#[cfg(not(windows))]
fn oem_name(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

/// The exact modification time from a zip entry's extra fields, when it has one: NTFS (0x000a,
/// 100 ns ticks in UTC, written by Windows tools) or Info-ZIP's extended timestamp (0x5455, Unix
/// seconds). Without them a zip only has a local date and time to 2 seconds.
fn precise_time(mut extra: &[u8]) -> Option<i64> {
    let mut unix = None;
    while extra.len() >= 4 {
        let id = u16::from_le_bytes([extra[0], extra[1]]);
        let size = u16::from_le_bytes([extra[2], extra[3]]) as usize;
        let Some(field) = extra.get(4..4 + size) else { break };
        match id {
            0x000a => {
                // 4 reserved bytes, then tagged attributes; tag 1 holds mtime, atime, ctime.
                let mut rest = field.get(4..)?;
                while rest.len() >= 4 {
                    let tag = u16::from_le_bytes([rest[0], rest[1]]);
                    let len = u16::from_le_bytes([rest[2], rest[3]]) as usize;
                    let body = rest.get(4..4 + len)?;
                    if tag == 1 && len >= 8 {
                        let ticks = u64::from_le_bytes(body[..8].try_into().ok()?);
                        return filetime_ms(ticks);
                    }
                    rest = &rest[4 + len..];
                }
            }
            0x5455 if field.first().is_some_and(|flags| flags & 1 != 0) && field.len() >= 5 => {
                unix = Some(i32::from_le_bytes(field[1..5].try_into().ok()?) as i64 * 1000);
            }
            _ => {}
        }
        extra = &extra[4 + size..];
    }
    unix
}

/// A Windows FILETIME (100 ns ticks since 1601) as milliseconds since 1970.
pub fn filetime_ms(ticks: u64) -> Option<i64> {
    (ticks > 0).then(|| (ticks / 10_000) as i64 - 11_644_473_600_000)
}

/// Turns a zip's local date and time into a moment, in this PC's time zone (as File Explorer shows
/// them), remembering each hour's offset so a big archive isn't converted entry by entry.
#[derive(Default)]
struct LocalTime {
    offsets: std::collections::HashMap<(u16, u8, u8, u8), i64>,
    last: Option<((u16, u8, u8, u8), i64)>,
}

impl LocalTime {
    fn unix_ms(&mut self, t: &zip::DateTime) -> i64 {
        let days = days_from_civil(t.year() as i64, t.month() as i64, t.day() as i64);
        let wall = ((days * 24 + t.hour() as i64) * 60 + t.minute() as i64) * 60_000 + t.second() as i64 * 1000;
        let hour = (t.year(), t.month(), t.day(), t.hour());
        let offset = match self.last {
            Some((h, offset)) if h == hour => offset,
            _ => *self.offsets.entry(hour).or_insert_with(|| local_offset_ms(t)),
        };
        self.last = Some((hour, offset));
        wall - offset
    }
}

/// Days from 1970-01-01 to a date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// How far ahead of UTC this PC's clock is at that local time, in milliseconds.
#[cfg(windows)]
fn local_offset_ms(t: &zip::DateTime) -> i64 {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};
    let local = SYSTEMTIME { wYear: t.year(), wMonth: t.month() as u16, wDay: t.day() as u16, wHour: t.hour() as u16, ..Default::default() };
    let mut utc = SYSTEMTIME::default();
    let (mut a, mut b) = (FILETIME::default(), FILETIME::default());
    let converted = unsafe { TzSpecificLocalTimeToSystemTime(None, &local, &mut utc).is_ok() && SystemTimeToFileTime(&local, &mut a).is_ok() && SystemTimeToFileTime(&utc, &mut b).is_ok() };
    if !converted {
        return 0;
    }
    let ticks = |f: FILETIME| ((f.dwHighDateTime as i64) << 32) | f.dwLowDateTime as i64;
    (ticks(a) - ticks(b)) / 10_000
}

#[cfg(not(windows))]
fn local_offset_ms(_: &zip::DateTime) -> i64 {
    0
}

impl ZipSource {
    fn problem(&self, item: ItemIndex) -> Option<Failure> {
        if self.overlapping {
            return Some(Failure::Bomb("its entries overlap each other, as in zip bombs".into()));
        }
        let entry = self.meta.entry(item).ok()?;
        zip_problem(entry.encrypted(), entry.compression())
    }

    fn read_each(&self, file: &Path, items: &[ItemIndex], each: &mut dyn FnMut(ItemIndex, &mut dyn Read) -> bool) -> Vec<(ItemIndex, Failure)> {
        let mut failed = Vec::new();
        let reader = match File::open(file) {
            Ok(f) => BufReader::with_capacity(256 << 10, f),
            Err(e) => return items.iter().map(|&i| (i, Failure::from_io(&e))).collect(),
        };
        // Safety: the metadata was read from this same file, which hasn't changed since (callers
        // check its size and date first, and a temp copy is never written to again).
        let mut zip = unsafe { ZipArchive::unsafe_new_with_metadata(reader, self.meta.clone()) };
        // In the order they're stored, so the file is read front to back.
        let mut order: Vec<ItemIndex> = items.to_vec();
        order.sort_by_key(|&i| self.meta.entry(i).map_or(0, |e| e.get_metadata().header_start));
        let mut stopped = false;
        for item in order {
            if stopped {
                failed.push((item, Failure::Cancelled));
                continue;
            }
            if let Some(problem) = self.problem(item) {
                failed.push((item, problem));
                continue;
            }
            let zstd = self.meta.entry(item).is_ok_and(|e| is_zstd(e.compression()));
            let read = if zstd {
                // The stored bytes, decoded here, then checked against the entry's CRC32.
                zip.by_index_raw(item).map(|raw| {
                    let crc = raw.crc32();
                    stopped = !each(item, &mut Crc32Checked::new(Zstd::new(BufReader::new(raw)), crc));
                })
            } else {
                zip.by_index(item).map(|mut entry| stopped = !each(item, &mut entry))
            };
            if let Err(e) = read {
                failed.push((item, zip_failure(e)));
            }
        }
        failed
    }
}

// 7z

/// The 7z methods this build can decompress (and filters it can undo).
fn sevenz_supported(id: &[u8]) -> bool {
    [
        EncoderMethod::ID_COPY,
        EncoderMethod::ID_LZMA,
        EncoderMethod::ID_LZMA2,
        EncoderMethod::ID_PPMD,
        EncoderMethod::ID_BZIP2,
        EncoderMethod::ID_DEFLATE,
        EncoderMethod::ID_BCJ_X86,
        EncoderMethod::ID_BCJ_ARM,
        EncoderMethod::ID_BCJ_ARM64,
        EncoderMethod::ID_BCJ_ARM_THUMB,
        EncoderMethod::ID_BCJ_PPC,
        EncoderMethod::ID_BCJ_IA64,
        EncoderMethod::ID_BCJ_SPARC,
        EncoderMethod::ID_BCJ_RISCV,
        EncoderMethod::ID_BCJ2,
        EncoderMethod::ID_DELTA,
    ]
    .contains(&id)
}

fn sevenz_method_name(id: &[u8]) -> String {
    match EncoderMethod::by_id(id) {
        Some(m) if m.id() == EncoderMethod::ID_ZSTD => "Zstandard".into(),
        Some(m) => m.name().to_string(),
        None => format!("method {}", id.iter().map(|b| format!("{b:02X}")).collect::<String>()),
    }
}

fn open_7z(path: &Path) -> Result<(Source, Vec<Item>), Failure> {
    let mut reader = BufReader::new(File::open(path)?);
    check_7z_header(&mut reader)?;
    reader.seek(SeekFrom::Start(0))?;
    let archive = SevenZ::read(&mut reader, &Password::empty()).map_err(sevenz_failure)?;
    if archive.files.len() > MAX_ENTRIES {
        return Err(Failure::TooManyEntries(archive.files.len()));
    }
    let offsets = archive.stream_map.pack_stream_offsets();
    let firsts = archive.stream_map.block_first_pack_stream_index();
    let packed_total: u64 = archive.pack_sizes().iter().sum();
    let blocks: Vec<BlockInfo> = archive
        .blocks
        .iter()
        .enumerate()
        .map(|(b, block)| {
            let start = firsts.get(b).and_then(|&p| offsets.get(p)).copied().unwrap_or(0);
            let end = firsts.get(b + 1).and_then(|&p| offsets.get(p)).copied().unwrap_or(packed_total);
            let problem = if block.coders.iter().any(|c| c.encoder_method_id() == EncoderMethod::ID_AES256_SHA256) {
                Some(Failure::Encrypted)
            } else {
                block.coders.iter().find(|c| !sevenz_supported(c.encoder_method_id())).map(|c| Failure::Unsupported(sevenz_method_name(c.encoder_method_id())))
            };
            BlockInfo { unpacked: block.get_unpack_size(), packed: end.saturating_sub(start), problem }
        })
        .collect();
    // Where each entry sits in its block: everything before it there is decompressed to reach it.
    let mut place = vec![None; archive.files.len()];
    for (b, &first) in archive.stream_map.block_first_file_index.iter().enumerate() {
        let mut before = 0u64;
        let mut i = first;
        while i < archive.files.len() && archive.stream_map.file_block_index[i] == Some(b) {
            place[i] = Some((b, before));
            before = before.saturating_add(archive.files[i].size);
            i += 1;
        }
    }
    // Every entry keeps its index in the archive's list; anti-items (deletions recorded by an update)
    // get no path, so the tree leaves them out.
    let items = archive
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut attrs = if f.has_windows_attributes { f.windows_attributes & 0xFF } else { 0 };
            if let Some(problem) = place[i].and_then(|(b, _)| blocks[b].problem.clone()) {
                attrs |= if matches!(problem, Failure::Encrypted) { ENCRYPTED } else { UNREADABLE };
            }
            let modified = if f.has_last_modified_date { filetime_ms(u64::from(f.last_modified_date)).unwrap_or(0) } else { 0 };
            let path = if f.is_anti_item { String::new() } else { f.name.clone() };
            Item { path, dir: f.is_directory, size: if f.is_directory { 0 } else { f.size }, modified, attrs }
        })
        .collect();
    Ok((Source::SevenZ(Box::new(SevenZSource { archive, place, blocks })), items))
}

fn sevenz_failure(e: sevenz_rust2::Error) -> Failure {
    use sevenz_rust2::Error;
    match e {
        Error::PasswordRequired | Error::MaybeBadPassword(_) => Failure::Encrypted,
        Error::UnsupportedCompressionMethod(m) if m.contains("AES") => Failure::Encrypted,
        Error::UnsupportedCompressionMethod(m) => Failure::Unsupported(m),
        Error::Io(e, _) | Error::FileOpen(e, _) => Failure::from(e),
        Error::ChecksumVerificationFailed => Failure::Damaged("a checksum doesn't match".into()),
        Error::NextHeaderCrcMismatch => Failure::Damaged("the list of files doesn't match its checksum".into()),
        Error::BadSignature(_) => Failure::Damaged("it isn't a 7z archive".into()),
        Error::UnsupportedVersion { major, minor } => Failure::Unsupported(format!("7z format version {major}.{minor}")),
        Error::MaxMemLimited { .. } => Failure::Bomb("it needs more memory to unpack than allowed".into()),
        other => Failure::Damaged(other.to_string()),
    }
}

/// A 7z's list of entries is usually compressed itself, and the archive says how big it unpacks to.
/// That's checked before the archive is read: a crafted few-kilobyte header could otherwise unpack
/// to more memory than the PC has. Also spots a list of entries that's encrypted.
fn check_7z_header(reader: &mut (impl Read + Seek)) -> Result<(), Failure> {
    let len = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(0))?;
    let mut start = [0u8; 32];
    reader.read_exact(&mut start).map_err(|_| Failure::Damaged("it ends before its header".into()))?;
    if &start[..6] != b"7z\xBC\xAF\x27\x1C" {
        return Err(Failure::Damaged("it isn't a 7z archive".into()));
    }
    let offset = u64::from_le_bytes(start[12..20].try_into().unwrap());
    let size = u64::from_le_bytes(start[20..28].try_into().unwrap());
    if size == 0 {
        // An empty archive, or one still being written: sevenz-rust2 looks for the header itself.
        return Ok(());
    }
    let at = offset.checked_add(32).filter(|&at| at.checked_add(size).is_some_and(|end| end <= len));
    let Some(at) = at else { return Err(Failure::Damaged("it ends before its list of files (is it complete?)".into())) };
    if size > MAX_7Z_HEADER {
        return Err(Failure::Bomb(format!("its list of files is {size} bytes")));
    }
    reader.seek(SeekFrom::Start(at))?;
    // An encoded header's description is short; its first kilobytes are enough.
    let mut header = vec![0u8; size.min(64 << 10) as usize];
    reader.read_exact(&mut header)?;
    if header.first() != Some(&0x17) {
        return Ok(());
    }
    let info = encoded_header_info(&header[1..]).ok_or_else(|| Failure::Damaged("its list of files can't be read".into()))?;
    if info.encrypted {
        return Err(Failure::EncryptedList);
    }
    if info.largest > MAX_7Z_HEADER {
        return Err(Failure::Bomb(format!("its list of files unpacks to {} bytes", info.largest)));
    }
    Ok(())
}

struct EncodedHeader {
    largest: u64,
    encrypted: bool,
}

/// Reads the streams info of a 7z encoded header just far enough to know how big it unpacks to
/// and whether it's encrypted. None if it's malformed.
fn encoded_header_info(bytes: &[u8]) -> Option<EncodedHeader> {
    let mut r = Bytes { bytes, at: 0 };
    let mut info = EncodedHeader { largest: 0, encrypted: false };
    let mut outs_per_folder = Vec::new();
    loop {
        match r.number()? {
            0x00 => return Some(info),
            // Substreams info comes last, after the sizes that matter here.
            0x08 if !outs_per_folder.is_empty() => return Some(info),
            // Pack info: position, count, then sizes and CRCs up to its end.
            0x06 => {
                r.number()?;
                let streams = r.count()?;
                loop {
                    match r.number()? {
                        0x00 => break,
                        0x09 => {
                            for _ in 0..streams {
                                r.number()?;
                            }
                        }
                        0x0A => r.skip_digests(streams)?,
                        _ => return None,
                    }
                }
            }
            // Unpack info: the folders' coders, then every output stream's size.
            0x07 => {
                if r.number()? != 0x0B {
                    return None;
                }
                let folders = r.count()?;
                if r.byte()? != 0 {
                    return None;
                }
                for _ in 0..folders {
                    let coders = r.count()?;
                    let (mut ins, mut outs) = (0usize, 0usize);
                    for _ in 0..coders {
                        let main = r.byte()?;
                        let id = r.take((main & 0x0F) as usize)?;
                        if id == EncoderMethod::ID_AES256_SHA256 {
                            info.encrypted = true;
                        }
                        let (i, o) = if main & 0x10 != 0 { (r.count()?, r.count()?) } else { (1, 1) };
                        ins = ins.checked_add(i)?;
                        outs = outs.checked_add(o)?;
                        if main & 0x20 != 0 {
                            let props = r.count()?;
                            r.take(props)?;
                        }
                    }
                    let pairs = outs.checked_sub(1)?;
                    for _ in 0..pairs {
                        r.number()?;
                        r.number()?;
                    }
                    let packed = ins.checked_sub(pairs)?;
                    if packed > 1 {
                        for _ in 0..packed {
                            r.number()?;
                        }
                    }
                    outs_per_folder.push(outs);
                }
                if r.number()? != 0x0C {
                    return None;
                }
                for &outs in &outs_per_folder {
                    for _ in 0..outs {
                        info.largest = info.largest.max(r.number()?);
                    }
                }
                loop {
                    match r.number()? {
                        0x00 => break,
                        0x0A => r.skip_digests(folders)?,
                        _ => return None,
                    }
                }
            }
            _ => return None,
        }
    }
}

/// A cursor over 7z header bytes.
struct Bytes<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Bytes<'_> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(b)
    }

    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.at.checked_add(n)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    /// 7z's variable-length number: the first byte's leading 1 bits say how many bytes follow.
    fn number(&mut self) -> Option<u64> {
        let first = self.byte()?;
        let mut mask = 0x80u8;
        let mut value = 0u64;
        for i in 0..8 {
            if first & mask == 0 {
                let high = (first & mask.wrapping_sub(1)) as u64;
                return Some(value | (high << (8 * i)));
            }
            value |= (self.byte()? as u64) << (8 * i);
            mask >>= 1;
        }
        Some(value)
    }

    /// A count of things that each take at least a byte: never more than the bytes left.
    fn count(&mut self) -> Option<usize> {
        let n = self.number()?;
        (n <= (self.bytes.len() - self.at) as u64).then_some(n as usize)
    }

    fn skip_digests(&mut self, n: usize) -> Option<()> {
        let all = self.byte()? != 0;
        let defined = if all {
            n
        } else {
            let bits = self.take(n.div_ceil(8))?;
            (0..n).filter(|i| bits[i / 8] & (0x80 >> (i % 8)) != 0).count()
        };
        self.take(defined.checked_mul(4)?)?;
        Some(())
    }
}

impl SevenZSource {
    fn read_each(&self, file: &Path, items: &[ItemIndex], each: &mut dyn FnMut(ItemIndex, &mut dyn Read) -> bool, skipped: &mut dyn FnMut(u64) -> bool) -> Vec<(ItemIndex, Failure)> {
        let mut failed = Vec::new();
        let mut by_block: BTreeMap<usize, Vec<ItemIndex>> = BTreeMap::new();
        for &item in items {
            match self.place.get(item).copied().flatten() {
                Some((block, _)) => match &self.blocks[block].problem {
                    Some(problem) => failed.push((item, problem.clone())),
                    None => by_block.entry(block).or_default().push(item),
                },
                // No data: an empty file.
                None => {
                    if !each(item, &mut io::empty()) {
                        failed.push((item, Failure::Cancelled));
                    }
                }
            }
        }
        let mut source = match File::open(file) {
            Ok(f) => BufReader::with_capacity(256 << 10, f),
            Err(e) => {
                failed.extend(by_block.into_values().flatten().map(|i| (i, Failure::from_io(&e))));
                return failed;
            }
        };
        let password = Password::empty();
        let mut stopped = false;
        for (block, mut wanted) in by_block {
            if stopped {
                failed.extend(wanted.into_iter().map(|i| (i, Failure::Cancelled)));
                continue;
            }
            wanted.sort_unstable();
            let first = self.archive.stream_map.block_first_file_index[block];
            let last = *wanted.last().unwrap_or(&first);
            let mut next = first;
            let mut done = Vec::new();
            let decoded = BlockDecoder::new(1, block, &self.archive, &password, &mut source).for_each_entries(&mut |entry, reader| {
                let index = next;
                next += 1;
                if wanted.binary_search(&index).is_ok() {
                    done.push(index);
                    if !each(index, reader) {
                        stopped = true;
                        return Ok(false);
                    }
                } else {
                    // Read through what's not wanted: what comes after it in the block depends on it.
                    let mut sink = Counted { skipped: &mut *skipped, stop: false };
                    io::copy(&mut reader.take(entry.size), &mut sink)?;
                    if sink.stop {
                        stopped = true;
                        return Ok(false);
                    }
                }
                Ok(index < last)
            });
            if let Err(e) = decoded {
                let failure = sevenz_failure(e);
                failed.extend(wanted.iter().filter(|i| !done.contains(i)).map(|&i| (i, failure.clone())));
            } else {
                failed.extend(wanted.iter().filter(|i| !done.contains(i)).map(|&i| (i, Failure::Cancelled)));
            }
        }
        failed
    }
}

/// Counts bytes read through (they cost time too) and passes them on to the progress callback.
struct Counted<'a> {
    skipped: &'a mut dyn FnMut(u64) -> bool,
    stop: bool,
}

impl io::Write for Counted<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !(self.skipped)(buf.len() as u64) {
            self.stop = true;
            return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"));
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_7z_numbers() {
        let mut r = Bytes { bytes: &[0x05, 0x80, 0xFF, 0xC1, 0x02, 0x03, 0xFF, 1, 2, 3, 4, 5, 6, 7, 8], at: 0 };
        assert_eq!(r.number(), Some(5));
        assert_eq!(r.number(), Some(0xFF));
        assert_eq!(r.number(), Some(0x01_0302));
        assert_eq!(r.number(), Some(0x0807_0605_0403_0201));
        assert_eq!(r.number(), None);
    }

    #[test]
    fn finds_how_big_an_encoded_header_unpacks() {
        // Pack info (pos 0, 1 stream, size 10), one folder with LZMA, unpacked size 300, CRC.
        let mut header = vec![0x06, 0x00, 0x01, 0x09, 0x0A, 0x00];
        header.extend([0x07, 0x0B, 0x01, 0x00, 0x01, 0x23, 0x03, 0x01, 0x01, 0x05, 0x5D, 0x00, 0x10, 0x00, 0x00]);
        header.extend([0x0C, 0x81, 0x2C, 0x0A, 0x01, 0xAA, 0xBB, 0xCC, 0xDD, 0x00, 0x00]);
        let info = encoded_header_info(&header).unwrap();
        assert_eq!((info.largest, info.encrypted), (300, false));
        // The same with AES in front of LZMA (two coders, one bind pair).
        let mut aes = vec![0x06, 0x00, 0x01, 0x09, 0x0A, 0x00];
        aes.extend([0x07, 0x0B, 0x01, 0x00, 0x02, 0x24, 0x06, 0xF1, 0x07, 0x01, 0x01, 0x00, 0x23, 0x03, 0x01, 0x01, 0x05, 0x5D, 0x00, 0x10, 0x00, 0x00, 0x01, 0x00]);
        aes.extend([0x0C, 0x20, 0x81, 0x2C, 0x00, 0x00]);
        let info = encoded_header_info(&aes).unwrap();
        assert_eq!((info.largest, info.encrypted), (300, true));
        // As sevenz-rust2 writes it: substreams info (a CRC) after the sizes.
        let written = [
            0x06, 0x1d, 0x01, 0x09, 0x69, 0x0a, 0x01, 0xe6, 0x3d, 0xc5, 0x79, 0x00, 0x07, 0x0b, 0x01, 0x00, 0x01, 0x23, 0x03, 0x01, 0x01, 0x05, 0x5d, 0x00, 0x00, 0x80, 0x00, 0x0c, 0x80, 0x9c, 0x00, 0x08, 0x0a, 0x01,
            0xda, 0x88, 0xa1, 0x05, 0x00, 0x00,
        ];
        let info = encoded_header_info(&written).unwrap();
        assert_eq!((info.largest, info.encrypted), (156, false));
        // Truncated anywhere: refused, never a panic.
        for cut in 0..header.len() {
            let _ = encoded_header_info(&header[..cut]);
        }
        // A count bigger than what's left can't be.
        assert!(encoded_header_info(&[0x07, 0x0B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]).is_none());
    }

    #[test]
    fn decodes_precise_zip_times() {
        // NTFS: reserved, tag 1 (24 bytes): mtime 2024-01-01T00:00:00Z.
        let ticks: u64 = (1_704_067_200_000 + 11_644_473_600_000) * 10_000;
        let mut ntfs = vec![0x0a, 0x00, 32, 0x00, 0, 0, 0, 0, 0x01, 0x00, 24, 0x00];
        ntfs.extend(ticks.to_le_bytes());
        ntfs.extend([0u8; 16]);
        assert_eq!(precise_time(&ntfs), Some(1_704_067_200_000));
        let ut = [0x55, 0x54, 5, 0, 1, 0x00, 0x2D, 0x92, 0x65];
        assert_eq!(precise_time(&ut), Some(1_704_078_592_000));
        assert_eq!(precise_time(&[0x55, 0x54, 9, 0]), None);
        assert_eq!(precise_time(&[]), None);
    }

    #[test]
    fn counts_days_like_a_calendar() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2024, 1, 1), 19_723);
    }

    #[test]
    #[allow(deprecated)]
    fn names_unreadable_methods() {
        assert!(matches!(zip_problem(true, CompressionMethod::Stored), Some(Failure::Encrypted)));
        assert!(zip_problem(false, CompressionMethod::ZSTD).is_none(), "Glint decodes Zstandard itself");
        assert!(matches!(zip_problem(false, CompressionMethod::Unsupported(94)), Some(Failure::Unsupported(m)) if m == "MP3"));
        assert!(matches!(zip_problem(false, CompressionMethod::AES), Some(Failure::Encrypted)));
        assert!(zip_problem(false, CompressionMethod::LZMA).is_none());
        assert!(zip_problem(false, CompressionMethod::Deflated).is_none());
        assert!(zip_problem(false, CompressionMethod::Bzip2).is_none());
    }
}
