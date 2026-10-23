//! Decompressing streams: Zstandard frames (zip's method 93), and a CRC32 check on what comes out.
//! Pure Rust, like the archive readers.

use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};
use std::io::{self, BufRead, Read};

/// The most memory one Zstandard frame may ask for as its window. zstd's own levels stay at 8 MB or
/// less; only `--long` goes past 128 MB, and a crafted frame could ask for gigabytes.
pub const MAX_ZSTD_WINDOW: u64 = 256 << 20;

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
