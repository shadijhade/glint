//! Archives that are one stream: a tar (plain, or compressed as a .tar.gz, .tar.xz, .tar.bz2,
//! .tar.zst or .tar.lz4) and a single compressed file (.gz, .xz, .bz2, .zst, .lz4), shown as a folder
//! holding the file it unpacks to.
//!
//! A compressed tar keeps no list of its entries: listing one means decompressing all of it once,
//! with progress and Stop (`Scan`), after which its tree is in memory like any archive's. Reading an
//! entry decompresses from the start again (the whole stream is one solid block), so a selection is
//! read in one pass, in the order stored. A plain tar is read entry by entry with a seek.

use super::codec::Codec;
use super::tar::{self, Data};
use super::tree::Item;
use super::{Failure, RATIO_FLOOR, MAX_RATIO};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A first read of an archive in progress (its whole stream, for a compressed tar): compressed bytes
/// read of `total`, and Stop.
#[derive(Debug)]
pub struct Scan {
    pub done: AtomicU64,
    pub total: u64,
    pub stop: AtomicBool,
    /// Reading the file itself failed (rather than the decompressor finding damage).
    unreadable: AtomicBool,
}

impl Scan {
    pub fn new(total: u64) -> Scan {
        Scan { done: AtomicU64::new(0), total, stop: AtomicBool::new(false), unreadable: AtomicBool::new(false) }
    }
}

/// Stop, as an error. Not `ErrorKind::Interrupted`: `io::copy` would try again forever.
fn stopped() -> io::Error {
    io::Error::other(CANCELLED)
}

const CANCELLED: &str = "cancelled";

/// The compressed file as read, counted for progress, and stopped when asked.
struct Counted<'a, R> {
    inner: R,
    scan: &'a Scan,
}

/// `inner` (a file) read with progress and Stop through `scan`, for other first reads of a whole
/// file (an ncdu export's), whose failures `scan_failure` then tells apart.
pub fn counted<'a, R: Read + 'a>(inner: R, scan: &'a Scan) -> impl Read + 'a {
    Counted { inner, scan }
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.scan.stop.load(Ordering::Relaxed) {
            return Err(stopped());
        }
        let n = self.inner.read(buf).inspect_err(|_| self.scan.unreadable.store(true, Ordering::Relaxed))?;
        self.scan.done.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// What comes out of the decompressor while a whole stream is read: counted, and refused once it
/// grows past `RATIO_FLOOR` at more than `MAX_RATIO` times the compressed bytes read so far.
struct Guarded<'a, R> {
    inner: R,
    scan: &'a Scan,
    decoded: u64,
}

impl<R: Read> Read for Guarded<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // A decompressor may wrap the stop in an error of its own.
        let n = match self.inner.read(buf) {
            Err(_) if self.scan.stop.load(Ordering::Relaxed) => return Err(stopped()),
            other => other?,
        };
        self.decoded += n as u64;
        let packed = self.scan.done.load(Ordering::Relaxed).max(1);
        if self.decoded > RATIO_FLOOR && self.decoded / packed > MAX_RATIO {
            return Err(io::Error::other(BOMB));
        }
        Ok(n)
    }
}

const BOMB: &str = "it expands more than a thousand times over";

/// What a failed first read of a whole stream was: stopped, a bomb, the file unreadable, or (what
/// the decompressor said) damage.
pub fn scan_failure(e: Failure, scan: &Scan) -> Failure {
    match e {
        _ if scan.stop.load(Ordering::Relaxed) => Failure::Cancelled,
        Failure::Io(text) | Failure::Damaged(text) if text.contains(BOMB) => Failure::Bomb(BOMB.into()),
        Failure::Io(text) if !scan.unreadable.load(Ordering::Relaxed) => Failure::Damaged(format!("it can't be decompressed ({text})")),
        other => other,
    }
}

/// What a failed read through a stream (to reach an entry) was.
fn io_failure(e: io::Error) -> Failure {
    if e.to_string() == CANCELLED {
        Failure::Cancelled
    } else if matches!(e.kind(), io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput | io::ErrorKind::Other) {
        Failure::Damaged(format!("it can't be decompressed ({e})"))
    } else {
        Failure::from(e)
    }
}

/// An archive that's one stream, ready to extract from.
pub struct StreamSource {
    codec: Codec,
    /// One compressed file, not a tar: its one entry is the whole stream.
    single: bool,
    data: Vec<Data>,
    problems: Vec<Option<Failure>>,
    /// What the stream unpacks to, and what it takes on disk.
    total: u64,
    packed: u64,
}

/// What a stream turned out to be, by its first bytes: a tar (compressed or not), or one compressed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Tar(Codec),
    Single(Codec),
}

/// Looks at the first bytes of `path`: its codec, and whether what it unpacks to starts with a tar
/// header. None when it's neither a tar nor compressed with a codec Glint reads.
pub fn kind_of(path: &Path) -> Option<Kind> {
    let mut file = File::open(path).ok()?;
    let mut head = Vec::with_capacity(4096);
    (&mut file).take(4096).read_to_end(&mut head).ok()?;
    let codec = Codec::detect(&head).unwrap_or(Codec::Plain);
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut first = [0u8; 512];
    let tar = codec.decoder(BufReader::new(&mut file)).read_exact(&mut first).is_ok() && tar::is_header(&first);
    match (tar, codec) {
        (true, codec) => Some(Kind::Tar(codec)),
        (false, Codec::Plain) => None,
        (false, codec) => Some(Kind::Single(codec)),
    }
}

impl StreamSource {
    /// Reads the archive's entries: a tar's headers (for a compressed one, the whole stream, with
    /// progress and Stop through `scan`), or a single file's size. `name` is what the single file is
    /// called inside (the archive's name without its codec's extension).
    pub fn open(path: &Path, kind: Kind, scan: &Scan, max_entries: usize) -> Result<(StreamSource, Vec<Item>), Failure> {
        let mut file = File::open(path)?;
        let meta = file.metadata()?;
        let packed = meta.len();
        let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
        match kind {
            Kind::Tar(Codec::Plain) => {
                // Uncompressed: the headers are read with a seek over each entry's data.
                let mut reader = BufReader::with_capacity(64 << 10, file);
                let mut entries = tar::walk(&mut reader, max_entries, &mut |r: &mut BufReader<File>, n| r.seek_relative(n as i64).map(|_| n))?;
                // A seek goes past the end without failing: data that would be there is missing.
                for entry in &mut entries {
                    if matches!(entry.data, Data::At { offset, size } if offset.saturating_add(size) > packed) {
                        entry.problem = Some(Failure::Damaged("it ends before this file's data (is it complete?)".into()));
                    }
                }
                Ok(Self::from_tar(Codec::Plain, entries, packed, packed))
            }
            Kind::Tar(codec) => {
                let counted = Counted { inner: BufReader::with_capacity(256 << 10, file), scan };
                let mut guarded = Guarded { inner: codec.decoder(BufReader::with_capacity(256 << 10, counted)), scan, decoded: 0 };
                let entries = tar::walk(&mut guarded, max_entries, &mut tar::read_through).map_err(|e| scan_failure(e, scan))?;
                // On past the tar's end (padding, as a rule) to the stream's: its checksum is there, and
                // covers every entry, which a tar doesn't check on its own.
                io::copy(&mut guarded, &mut io::sink()).map_err(|e| scan_failure(e.into(), scan))?;
                let total = guarded.decoded;
                Ok(Self::from_tar(codec, entries, total, packed))
            }
            Kind::Single(codec) => {
                let total = match codec.stated_size(&mut file) {
                    Some(size) => size,
                    // Only reading it all tells: gzip keeps its size modulo 4 GB, bzip2 none at all.
                    None => {
                        let counted = Counted { inner: BufReader::with_capacity(256 << 10, file), scan };
                        let mut guarded = Guarded { inner: codec.decoder(BufReader::with_capacity(256 << 10, counted)), scan, decoded: 0 };
                        io::copy(&mut guarded, &mut io::sink()).map_err(|e| scan_failure(e.into(), scan))?;
                        guarded.decoded
                    }
                };
                let name = single_name(path, codec);
                let item = Item { path: name, dir: false, size: total, modified, attrs: 0 };
                Ok((StreamSource { codec, single: true, data: vec![Data::At { offset: 0, size: total }], problems: vec![None], total, packed }, vec![item]))
            }
        }
    }

    fn from_tar(codec: Codec, entries: Vec<tar::Entry>, total: u64, packed: u64) -> (StreamSource, Vec<Item>) {
        let mut data = Vec::with_capacity(entries.len());
        let mut problems = Vec::with_capacity(entries.len());
        let mut items = Vec::with_capacity(entries.len());
        for entry in entries {
            data.push(entry.data);
            problems.push(entry.problem);
            items.push(entry.item);
        }
        (StreamSource { codec, single: false, data, problems, total, packed }, items)
    }

    fn compressed(&self) -> bool {
        self.codec != Codec::Plain
    }

    pub fn cost(&self, item: usize, size: u64) -> u64 {
        match self.data.get(item) {
            Some(Data::At { offset, .. }) if self.compressed() => offset.saturating_add(size),
            _ => size,
        }
    }

    /// A compressed stream is one solid block; a plain tar has none.
    pub fn block_of(&self, item: usize) -> Option<usize> {
        matches!(self.data.get(item), Some(Data::At { .. })).then_some(0).filter(|_| self.compressed())
    }

    pub fn ratio(&self) -> u64 {
        if self.compressed() {
            self.total / self.packed.max(1)
        } else {
            1
        }
    }

    pub fn problem(&self, item: usize) -> Option<Failure> {
        self.problems.get(item).cloned().flatten()
    }

    /// Streams the entries in `items` out: links' targets from memory, the rest in the order they're
    /// stored, in one pass (two when hard links share data with an entry already passed).
    pub fn read_each(&self, file: &Path, items: &[usize], each: &mut dyn FnMut(usize, &mut dyn Read) -> bool, skipped: &mut dyn FnMut(u64) -> bool) -> Vec<(usize, Failure)> {
        let mut failed = Vec::new();
        let mut at_offsets = Vec::new();
        for &item in items {
            match (self.problem(item), self.data.get(item)) {
                (Some(problem), _) => failed.push((item, problem)),
                (None, Some(Data::Inline(bytes))) => {
                    if !each(item, &mut &bytes[..]) {
                        failed.push((item, Failure::Cancelled));
                    }
                }
                (None, Some(&Data::At { offset, size })) => at_offsets.push((offset, size, item)),
                _ => {
                    if !each(item, &mut io::empty()) {
                        failed.push((item, Failure::Cancelled));
                    }
                }
            }
        }
        at_offsets.sort_unstable();
        while !at_offsets.is_empty() {
            let (pass, again) = self.pass(file, &at_offsets, each, skipped, &mut failed);
            if !pass {
                failed.extend(again.into_iter().map(|(_, _, i)| (i, Failure::Cancelled)));
                break;
            }
            at_offsets = again;
        }
        failed
    }

    /// One pass through the stream for `wanted` (sorted by offset). Returns whether to go on, and
    /// the entries it didn't serve: on a stop, every one left; otherwise those whose data is shared
    /// with one already passed (a hard link's), for another pass.
    #[allow(clippy::type_complexity)]
    fn pass(&self, file: &Path, wanted: &[(u64, u64, usize)], each: &mut dyn FnMut(usize, &mut dyn Read) -> bool, skipped: &mut dyn FnMut(u64) -> bool, failed: &mut Vec<(usize, Failure)>) -> (bool, Vec<(u64, u64, usize)>) {
        let source = match File::open(file) {
            Ok(f) => BufReader::with_capacity(256 << 10, f),
            Err(e) => {
                failed.extend(wanted.iter().map(|&(_, _, i)| (i, Failure::from_io(&e))));
                return (false, Vec::new());
            }
        };
        if !self.compressed() {
            let mut source = source;
            for (k, &(offset, size, item)) in wanted.iter().enumerate() {
                if let Err(e) = source.seek(SeekFrom::Start(offset)) {
                    failed.push((item, Failure::from(e)));
                    continue;
                }
                if !each(item, &mut (&mut source).take(size)) {
                    return (false, wanted[k + 1..].to_vec());
                }
            }
            return (true, Vec::new());
        }
        // A single file's entry is the stream to its end, where its checksum is (and where it would
        // turn out bigger or smaller than its header said). A tar's was checked when it was listed.
        serve_in_order(&mut self.codec.decoder(source), wanted, self.single, each, skipped, failed)
    }
}

/// Serves `wanted` (entries' offsets, sizes and indexes, sorted by offset) from `stream`, read front
/// to back: what's between them is read through and counted by `skipped`, which stops it by
/// returning false. With `to_end`, an entry is read on to the stream's end. Returns whether to go
/// on, and the entries not served: on a stop, every one left; otherwise those whose data comes before
/// what was already passed (entries sharing bytes, as a tar's hard links do), for another pass.
#[allow(clippy::type_complexity)]
pub fn serve_in_order(stream: &mut dyn Read, wanted: &[(u64, u64, usize)], to_end: bool, each: &mut dyn FnMut(usize, &mut dyn Read) -> bool, skipped: &mut dyn FnMut(u64) -> bool, failed: &mut Vec<(usize, Failure)>) -> (bool, Vec<(u64, u64, usize)>) {
    let mut again = Vec::new();
    let mut pos = 0u64;
    for (k, &(offset, size, item)) in wanted.iter().enumerate() {
        if offset < pos {
            again.push((offset, size, item));
            continue;
        }
        // Read through what isn't wanted: everything after it depends on it.
        let mut sink = Progress { skipped: &mut *skipped, stop: false };
        match io::copy(&mut Read::take(&mut *stream, offset - pos), &mut sink) {
            Ok(n) if n == offset - pos => {}
            _ if sink.stop => {
                again.extend_from_slice(&wanted[k..]);
                return (false, again);
            }
            Ok(_) => {
                failed.extend(wanted[k..].iter().map(|&(_, _, i)| (i, Failure::Damaged("it ends too early (is it complete?)".into()))));
                return (true, again);
            }
            Err(e) => {
                let failure = io_failure(e);
                failed.extend(wanted[k..].iter().map(|&(_, _, i)| (i, failure.clone())));
                return (true, again);
            }
        }
        let mut entry = CountedTake { inner: Read::take(&mut *stream, if to_end { u64::MAX } else { size }), read: 0 };
        if !each(item, &mut entry) {
            again.extend_from_slice(&wanted[k + 1..]);
            return (false, again);
        }
        // Whatever the reader left of the entry is passed over too.
        let rest = size.saturating_sub(entry.read);
        let mut sink = Progress { skipped: &mut *skipped, stop: false };
        if rest > 0 && io::copy(&mut Read::take(&mut *stream, rest), &mut sink).is_err() && sink.stop {
            again.extend_from_slice(&wanted[k + 1..]);
            return (false, again);
        }
        pos = offset + size;
    }
    (true, again)
}

/// An entry's reader, counting what was taken from it.
struct CountedTake<R> {
    inner: R,
    read: u64,
}

impl<R: Read> Read for CountedTake<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        Ok(n)
    }
}

/// Bytes read through, passed on to the progress callback, which stops the copy by returning false.
struct Progress<'a> {
    skipped: &'a mut dyn FnMut(u64) -> bool,
    stop: bool,
}

impl io::Write for Progress<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !(self.skipped)(buf.len() as u64) {
            self.stop = true;
            return Err(stopped());
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What a single compressed file is called inside: its name without the codec's extension
/// ("notes.txt" for notes.txt.gz), or the name with "~" when it doesn't end in one.
fn single_name(path: &Path, codec: Codec) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = codec.ext();
    match name.rsplit_once('.') {
        Some((stem, e)) if e.eq_ignore_ascii_case(ext) && !stem.is_empty() => stem.to_string(),
        _ => format!("{name}~"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_what_a_compressed_file_holds() {
        assert_eq!(single_name(Path::new("C:\\logs\\app.log.gz"), Codec::Gz), "app.log");
        assert_eq!(single_name(Path::new("data.XZ"), Codec::Xz), "data");
        assert_eq!(single_name(Path::new("renamed.bin"), Codec::Zst), "renamed.bin~");
        assert_eq!(single_name(Path::new(".zst"), Codec::Zst), ".zst~");
    }
}
