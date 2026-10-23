//! Decompressing streams: the codecs of .gz, .xz, .bz2, .zst and .lz4 files (and of the tar
//! archives they wrap), Zstandard frames for zip's method 93 too, and a CRC32 check on what comes
//! out. All pure Rust, like the archive readers.

use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};

/// The most memory one Zstandard frame may ask for as its window. zstd's own levels stay at 8 MB or
/// less; only `--long` goes past 128 MB, and a crafted frame could ask for gigabytes.
pub const MAX_ZSTD_WINDOW: u64 = 256 << 20;
/// The same for an xz stream's dictionary, in KiB (xz -9 uses 64 MiB).
const MAX_XZ_MEMORY_KB: u32 = 256 << 10;

/// How a stream is compressed. Their order is part of the format codes the UI knows (`Format::code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    Plain,
    Gz,
    Xz,
    Bz2,
    Zst,
    Lz4,
}

impl Codec {
    pub const ALL: [Codec; 6] = [Codec::Plain, Codec::Gz, Codec::Xz, Codec::Bz2, Codec::Zst, Codec::Lz4];

    pub fn index(self) -> u8 {
        Codec::ALL.iter().position(|&c| c == self).unwrap_or(0) as u8
    }

    /// The extension a file compressed this way ends in ("" for none).
    pub fn ext(self) -> &'static str {
        match self {
            Codec::Plain => "",
            Codec::Gz => "gz",
            Codec::Xz => "xz",
            Codec::Bz2 => "bz2",
            Codec::Zst => "zst",
            Codec::Lz4 => "lz4",
        }
    }

    /// By a stream's first bytes; None when it's none of these (it may still be plain data).
    pub fn detect(head: &[u8]) -> Option<Codec> {
        let skippable = head.len() >= 4 && head[0] & 0xF0 == 0x50 && head[1..4] == [0x2A, 0x4D, 0x18];
        if head.starts_with(&[0x1F, 0x8B, 0x08]) {
            Some(Codec::Gz)
        } else if head.starts_with(b"\xFD7zXZ\x00") {
            Some(Codec::Xz)
        } else if head.len() >= 4 && head.starts_with(b"BZh") && (b'1'..=b'9').contains(&head[3]) {
            Some(Codec::Bz2)
        } else if head.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) || (skippable && head.windows(4).any(|w| w == [0x28, 0xB5, 0x2F, 0xFD])) {
            Some(Codec::Zst)
        } else if head.starts_with(&[0x04, 0x22, 0x4D, 0x18]) {
            Some(Codec::Lz4)
        } else {
            None
        }
    }

    /// A reader that decompresses `source`, every member or frame one after another. It's fused:
    /// once it reports damage, it says so again rather than decompressing on.
    pub fn decoder<'a>(self, source: impl BufRead + Send + 'a) -> Box<dyn Read + Send + 'a> {
        match self {
            Codec::Plain => Box::new(source),
            Codec::Gz => Box::new(Fused::new(flate2::bufread::MultiGzDecoder::new(source))),
            Codec::Xz => Box::new(Fused::new(lzma_rust2::XzReader::new_mem_limit(source, true, MAX_XZ_MEMORY_KB))),
            Codec::Bz2 => Box::new(Fused::new(bzip2::bufread::MultiBzDecoder::new(source))),
            Codec::Zst => Box::new(Fused::new(Zstd::new(source))),
            Codec::Lz4 => Box::new(Fused::new(lz4_flex::frame::FrameDecoder::new(source))),
        }
    }

    /// How big the file unpacks to, when the file says so without decompressing it: an xz file's
    /// index, or the content sizes every Zstandard or LZ4 frame states. None otherwise (gzip keeps its
    /// size modulo 4 GB, bzip2 none at all), and then only reading it all tells.
    pub fn stated_size(self, file: &mut File) -> Option<u64> {
        let len = file.seek(SeekFrom::End(0)).ok()?;
        let size = match self {
            Codec::Xz => xz_size(file, len),
            Codec::Zst => frames_size(file, len, zstd_frame),
            Codec::Lz4 => frames_size(file, len, lz4_frame),
            _ => None,
        };
        file.seek(SeekFrom::Start(0)).ok()?;
        size
    }
}

/// An xz file's uncompressed size from the index of each of its streams (read from the end back).
fn xz_size(file: &mut File, len: u64) -> Option<u64> {
    let mut end = len;
    let mut total = 0u64;
    while end > 0 {
        // Stream padding: zeros in fours between and after streams.
        let mut word = [0u8; 4];
        file.seek(SeekFrom::Start(end.checked_sub(4)?)).ok()?;
        file.read_exact(&mut word).ok()?;
        if word == [0; 4] {
            end -= 4;
            continue;
        }
        let mut footer = [0u8; 12];
        file.seek(SeekFrom::Start(end.checked_sub(12)?)).ok()?;
        file.read_exact(&mut footer).ok()?;
        if &footer[10..12] != b"YZ" {
            return None;
        }
        let index_size = (u32::from_le_bytes(footer[4..8].try_into().ok()?) as u64 + 1) * 4;
        let index_at = end.checked_sub(12 + index_size)?;
        if index_size > 64 << 20 {
            return None;
        }
        let mut index = vec![0u8; index_size as usize];
        file.seek(SeekFrom::Start(index_at)).ok()?;
        file.read_exact(&mut index).ok()?;
        let mut at = 1usize;
        if index.first() != Some(&0) {
            return None;
        }
        let records = multibyte(&index, &mut at)?;
        let mut blocks = 0u64;
        for _ in 0..records {
            let unpadded = multibyte(&index, &mut at)?;
            blocks = blocks.checked_add(unpadded.checked_add(3)? & !3)?;
            total = total.checked_add(multibyte(&index, &mut at)?)?;
        }
        // The stream's header (12 bytes), its blocks, then the index and footer just read.
        end = index_at.checked_sub(blocks)?.checked_sub(12)?;
        let mut magic = [0u8; 6];
        file.seek(SeekFrom::Start(end)).ok()?;
        file.read_exact(&mut magic).ok()?;
        if &magic != b"\xFD7zXZ\x00" {
            return None;
        }
    }
    Some(total)
}

/// xz's variable-length integer: 7 bits a byte, at most 9 bytes.
fn multibyte(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for i in 0..9 {
        let b = *bytes.get(*at)?;
        *at += 1;
        value |= ((b & 0x7F) as u64) << (7 * i);
        if b & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// One frame's stated size and how many bytes it takes in the file, or None when it doesn't state
/// one. Skippable frames state 0.
type FrameWalk = fn(&mut BufReader<&mut File>) -> Option<(u64, u64)>;

/// Adds up every frame's stated size, frame header by frame header (nothing is decompressed).
fn frames_size(file: &mut File, len: u64, frame: FrameWalk) -> Option<u64> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut reader = BufReader::with_capacity(64 << 10, file);
    let (mut at, mut total) = (0u64, 0u64);
    while at < len {
        let (size, took) = frame(&mut reader)?;
        total = total.checked_add(size)?;
        at = at.checked_add(took)?;
    }
    Some(total)
}

fn read_u32(r: &mut impl Read) -> Option<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).ok()?;
    Some(u32::from_le_bytes(b))
}

fn skip(r: &mut BufReader<&mut File>, n: u64) -> Option<()> {
    r.seek_relative(i64::try_from(n).ok()?).ok()
}

/// A skippable frame (Zstandard's and LZ4's alike): magic 0x184D2A5?, then its length.
fn skippable(r: &mut BufReader<&mut File>, magic: u32) -> Option<Option<(u64, u64)>> {
    if magic & 0xFFFF_FFF0 != 0x184D_2A50 {
        return Some(None);
    }
    let length = read_u32(r)? as u64;
    skip(r, length)?;
    Some(Some((0, 8 + length)))
}

fn zstd_frame(r: &mut BufReader<&mut File>) -> Option<(u64, u64)> {
    let magic = read_u32(r)?;
    if let Some(frame) = skippable(r, magic)? {
        return Some(frame);
    }
    if magic != 0xFD2F_B528 {
        return None;
    }
    let mut descriptor = [0u8; 1];
    r.read_exact(&mut descriptor).ok()?;
    let d = descriptor[0];
    let single_segment = d & 0x20 != 0;
    let checksum = d & 0x04 != 0;
    let dict_bytes = [0u64, 1, 2, 4][(d & 0x03) as usize];
    let size_bytes = match d >> 6 {
        0 if single_segment => 1,
        0 => return None,
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let mut took = 4 + 1;
    if !single_segment {
        skip(r, 1)?;
        took += 1;
    }
    skip(r, dict_bytes)?;
    let mut field = [0u8; 8];
    r.read_exact(&mut field[..size_bytes]).ok()?;
    let size = u64::from_le_bytes(field) + if size_bytes == 2 { 256 } else { 0 };
    took += dict_bytes + size_bytes as u64;
    // Blocks: a 3-byte header each (last flag, type, size); an RLE block stores one byte.
    loop {
        let mut header = [0u8; 3];
        r.read_exact(&mut header).ok()?;
        let h = u32::from_le_bytes([header[0], header[1], header[2], 0]);
        let stored = match (h >> 1) & 3 {
            1 => 1,
            3 => return None,
            _ => (h >> 3) as u64,
        };
        skip(r, stored)?;
        took += 3 + stored;
        if h & 1 != 0 {
            break;
        }
    }
    if checksum {
        skip(r, 4)?;
        took += 4;
    }
    Some((size, took))
}

fn lz4_frame(r: &mut BufReader<&mut File>) -> Option<(u64, u64)> {
    let magic = read_u32(r)?;
    if let Some(frame) = skippable(r, magic)? {
        return Some(frame);
    }
    if magic != 0x184D_2204 {
        return None;
    }
    let mut flags = [0u8; 2];
    r.read_exact(&mut flags).ok()?;
    let (flg, block_checksums, content_checksum) = (flags[0], flags[0] & 0x10 != 0, flags[0] & 0x04 != 0);
    if flg & 0x08 == 0 {
        return None;
    }
    let mut size = [0u8; 8];
    r.read_exact(&mut size).ok()?;
    let dict = if flg & 0x01 != 0 { 4 } else { 0 };
    skip(r, dict + 1)?;
    let mut took = 4 + 2 + 8 + dict + 1;
    loop {
        let block = read_u32(r)?;
        took += 4;
        if block == 0 {
            break;
        }
        let stored = (block & 0x7FFF_FFFF) as u64 + if block_checksums { 4 } else { 0 };
        skip(r, stored)?;
        took += stored;
    }
    if content_checksum {
        skip(r, 4)?;
        took += 4;
    }
    Some((u64::from_le_bytes(size), took))
}

/// Zstandard frames one after another, as zstd writes a file in parts, skipping skippable frames.
/// A frame that carries a checksum has it checked.
pub struct Zstd<R: BufRead> {
    source: R,
    decoder: FrameDecoder,
    in_frame: bool,
}

impl<R: BufRead> Zstd<R> {
    pub fn new(source: R) -> Zstd<R> {
        let mut decoder = FrameDecoder::new();
        decoder.set_max_window_size(MAX_ZSTD_WINDOW);
        Zstd { source, decoder, in_frame: false }
    }
}

fn damaged(what: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("Zstandard data can't be decompressed ({what})"))
}

impl<R: BufRead> Read for Zstd<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.in_frame {
                while self.decoder.can_collect() == 0 && !self.decoder.is_finished() {
                    self.decoder.decode_blocks(&mut self.source, BlockDecodingStrategy::UptoBlocks(1)).map_err(damaged)?;
                }
                let n = self.decoder.read(buf)?;
                if n > 0 {
                    return Ok(n);
                }
                // The frame is done: its checksum, when it has one, must match what was decoded.
                if let (Some(stored), Some(computed)) = (self.decoder.get_checksum_from_data(), self.decoder.get_calculated_checksum()) {
                    if stored != computed {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "the Zstandard checksum doesn't match"));
                    }
                }
                self.in_frame = false;
            }
            // Another frame, or the end.
            if self.source.fill_buf()?.is_empty() {
                return Ok(0);
            }
            match self.decoder.reset(&mut self.source) {
                Ok(()) => self.in_frame = true,
                Err(FrameDecoderError::ReadFrameHeaderError(ReadFrameHeaderError::SkipFrame { length, .. })) => {
                    let skipped = io::copy(&mut (&mut self.source).take(length as u64), &mut io::sink())?;
                    if skipped < length as u64 {
                        return Err(damaged("a skippable frame ends early"));
                    }
                }
                Err(FrameDecoderError::WindowSizeTooBig { requested, .. }) => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, format!("it was compressed with a {} MB window, more than Glint allows ({} MB)", requested >> 20, MAX_ZSTD_WINDOW >> 20)))
                }
                Err(e) => return Err(damaged(e)),
            }
        }
    }
}

/// A decompressor that, once it has reported damage, reports the same from then on without being
/// asked to decompress again: lzma-rust2 can loop forever when read again after an error (an xz file
/// with one damaged byte did), and nothing should rely on a decoder recovering.
pub struct Fused<R> {
    inner: R,
    failed: Option<(io::ErrorKind, String)>,
}

impl<R: Read> Fused<R> {
    pub fn new(inner: R) -> Fused<R> {
        Fused { inner, failed: None }
    }

    /// The error it stopped at, if it did.
    pub fn error(&self) -> Option<io::Error> {
        self.failed.as_ref().map(|(kind, text)| io::Error::new(*kind, text.clone()))
    }
}

impl<R: Read> Read for Fused<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(e) = self.error() {
            return Err(e);
        }
        match self.inner.read(buf) {
            Err(e) if e.kind() != io::ErrorKind::Interrupted => {
                self.failed = Some((e.kind(), e.to_string()));
                Err(e)
            }
            other => other,
        }
    }
}

/// Passes `inner`'s bytes through and fails at the end if their CRC32 isn't `expected`.
pub struct Crc32Checked<R: Read> {
    inner: R,
    expected: u32,
    hasher: crc32fast::Hasher,
}

impl<R: Read> Crc32Checked<R> {
    pub fn new(inner: R, expected: u32) -> Crc32Checked<R> {
        Crc32Checked { inner, expected, hasher: crc32fast::Hasher::new() }
    }
}

impl<R: Read> Read for Crc32Checked<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.hasher.update(&buf[..n]);
        } else if !buf.is_empty() && self.hasher.clone().finalize() != self.expected {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "its CRC32 checksum doesn't match"));
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn zstd(data: &[u8]) -> Vec<u8> {
        ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
    }

    #[test]
    fn decodes_frames_one_after_another_and_skips_skippable_ones() {
        let mut stream = zstd(b"hello, ");
        // A skippable frame (magic 0x184D2A5x, then a length) between two real ones.
        stream.extend([0x50, 0x2A, 0x4D, 0x18, 3, 0, 0, 0, 1, 2, 3]);
        stream.extend(zstd(b"world"));
        let mut out = String::new();
        Zstd::new(Cursor::new(&stream)).read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello, world");
        // Cut short anywhere: an error, never a panic.
        for cut in 1..stream.len() {
            let mut sink = Vec::new();
            let _ = Zstd::new(Cursor::new(&stream[..cut])).read_to_end(&mut sink);
        }
        let mut garbage = Vec::new();
        assert!(Zstd::new(Cursor::new(b"not zstd at all".as_slice())).read_to_end(&mut garbage).is_err());
    }

    #[test]
    fn says_the_same_error_again_without_decoding_on() {
        struct OnceThenPanic(bool);
        impl Read for OnceThenPanic {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                assert!(!std::mem::replace(&mut self.0, true), "read again after an error");
                Err(io::Error::new(io::ErrorKind::InvalidData, "corrupted input data"))
            }
        }
        let mut fused = Fused::new(OnceThenPanic(false));
        let mut buf = [0u8; 8];
        assert_eq!(fused.read(&mut buf).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(fused.read(&mut buf).unwrap_err().to_string(), "corrupted input data");
    }

    #[test]
    fn checks_crc32_at_the_end() {
        let data = b"some data";
        let mut ok = Vec::new();
        Crc32Checked::new(Cursor::new(data), crc32fast::hash(data)).read_to_end(&mut ok).unwrap();
        assert_eq!(ok, data);
        let mut bad = Vec::new();
        let e = Crc32Checked::new(Cursor::new(data), 1).read_to_end(&mut bad).unwrap_err();
        assert!(e.to_string().contains("checksum"));
    }
}
