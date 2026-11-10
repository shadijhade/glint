//! A Snappy decompressor (the raw block format, not the framing one), for LevelDB's compressed blocks.
//!
//! Chromium's LevelDB compresses the data blocks of its table files with Snappy. A block is the length of the
//! text as a varint, then a run of elements, each starting with a tag byte whose low two bits say what it is:
//!
//! | low bits | element                                                                                        |
//! |----------|------------------------------------------------------------------------------------------------|
//! | `00`     | a literal: `len - 1` in the tag's upper 6 bits, or (for 60 to 63) in the next 1 to 4 bytes      |
//! | `01`     | a copy, 4 to 11 bytes, from up to 2047 bytes back (3 bits of length, 3 bits of offset, 1 byte) |
//! | `10`     | a copy, 1 to 64 bytes, from up to 65535 back (6 bits of length, 2 bytes of offset)              |
//! | `11`     | a copy, 1 to 64 bytes, from up to 4 GiB back (6 bits of length, 4 bytes of offset)              |
//!
//! A copy may overlap what it is writing (an offset of 1 repeats one byte), so it goes byte by byte.
//! Nothing here panics on bad input: every failure is an `Err` saying what was wrong.

/// The most a block may say it decompresses to. A damaged length must not make us allocate gigabytes.
const MAX_OUTPUT: usize = 64 << 20;

/// Reads a varint (up to 32 bits) from the start of `input`: the value and how many bytes it took.
fn varint(input: &[u8]) -> Result<(usize, usize), String> {
    let mut value: u64 = 0;
    for (i, byte) in input.iter().take(5).enumerate() {
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            let value = usize::try_from(value).map_err(|_| "snappy: length too big".to_string())?;
            return Ok((value, i + 1));
        }
    }
    Err("snappy: bad length".to_string())
}

/// Reads `n` (1 to 4) bytes of `input` at `at` as a little-endian number.
fn le(input: &[u8], at: usize, n: usize) -> Result<usize, String> {
    let bytes = at
        .checked_add(n)
        .and_then(|end| input.get(at..end))
        .ok_or_else(|| "snappy: truncated".to_string())?;
    let mut value: u64 = 0;
    for (i, byte) in bytes.iter().enumerate() {
        value |= u64::from(*byte) << (8 * i);
    }
    usize::try_from(value).map_err(|_| "snappy: number too big".to_string())
}

/// The text `input` (a raw Snappy block) stands for.
pub fn decompress(input: &[u8]) -> Result<Vec<u8>, String> {
    let (expected, mut at) = varint(input)?;
    if expected > MAX_OUTPUT {
        return Err("snappy: block claims to be too big".to_string());
    }
    let mut out: Vec<u8> = Vec::with_capacity(expected);
    while at < input.len() {
        let tag = input[at];
        at += 1;
        match tag & 3 {
            0 => {
                let mut len = usize::from(tag >> 2);
                if len >= 60 {
                    let extra = len - 59;
                    len = le(input, at, extra)?;
                    at += extra;
                }
                len = len.checked_add(1).ok_or("snappy: literal too long")?;
                let end = at.checked_add(len).ok_or("snappy: literal too long")?;
                let bytes = input.get(at..end).ok_or("snappy: literal runs past the end")?;
                if out.len() + len > expected {
                    return Err("snappy: more text than the length says".to_string());
                }
                out.extend_from_slice(bytes);
                at = end;
            }
            kind => {
                let (len, offset) = match kind {
                    1 => {
                        let low = le(input, at, 1)?;
                        at += 1;
                        (4 + usize::from((tag >> 2) & 7), (usize::from(tag >> 5) << 8) | low)
                    }
                    2 => {
                        let offset = le(input, at, 2)?;
                        at += 2;
                        (1 + usize::from(tag >> 2), offset)
                    }
                    _ => {
                        let offset = le(input, at, 4)?;
                        at += 4;
                        (1 + usize::from(tag >> 2), offset)
                    }
                };
                if offset == 0 || offset > out.len() {
                    return Err("snappy: a copy from before the start".to_string());
                }
                if out.len() + len > expected {
                    return Err("snappy: more text than the length says".to_string());
                }
                let from = out.len() - offset;
                for i in 0..len {
                    // `from + i` is below `out.len()`: the output grows by one byte per step.
                    let byte = out[from + i];
                    out.push(byte);
                }
            }
        }
    }
    if out.len() != expected {
        return Err("snappy: less text than the length says".to_string());
    }
    Ok(out)
}

/// A Snappy block holding `data` as literals only (no copies): what the tests build tables from.
#[cfg(test)]
pub(super) fn compress_literals(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut n = data.len();
    loop {
        let low = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(low);
            break;
        }
        out.push(low | 0x80);
    }
    for chunk in data.chunks(60_000) {
        let len = chunk.len() - 1;
        if len < 60 {
            out.push((len as u8) << 2);
        } else if len < 256 {
            out.push(60 << 2);
            out.push(len as u8);
        } else {
            out.push(61 << 2);
            out.extend_from_slice(&(len as u16).to_le_bytes());
        }
        out.extend_from_slice(chunk);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_block_and_a_short_literal() {
        assert_eq!(decompress(&[0x00]).expect("empty"), b"");
        assert_eq!(decompress(&[0x03, 0x08, b'a', b'b', b'c']).expect("literal"), b"abc");
    }

    #[test]
    fn a_one_byte_offset_copy_may_overlap_what_it_writes() {
        // "abc" then a copy of 6 bytes from 3 back: abcabcabc.
        let block = [0x09, 0x08, b'a', b'b', b'c', 0x09, 0x03];
        assert_eq!(decompress(&block).expect("copy"), b"abcabcabc");
        // An offset of 1 repeats the last byte: "x" then 7 copies of it.
        let block = [0x08, 0x00, b'x', 0x0d, 0x01];
        assert_eq!(decompress(&block).expect("run"), b"xxxxxxxx");
    }

    #[test]
    fn two_and_four_byte_offset_copies() {
        let two = [0x08, 0x0c, b'a', b'b', b'c', b'd', 0x0e, 0x04, 0x00];
        assert_eq!(decompress(&two).expect("two"), b"abcdabcd");
        let four = [0x08, 0x0c, b'a', b'b', b'c', b'd', 0x0f, 0x04, 0x00, 0x00, 0x00];
        assert_eq!(decompress(&four).expect("four"), b"abcdabcd");
    }

    #[test]
    fn a_long_literal_names_its_length_in_extra_bytes() {
        let text = vec![b'q'; 100];
        let mut block = vec![100, 60 << 2, 99];
        block.extend_from_slice(&text);
        assert_eq!(decompress(&block).expect("one extra byte"), text);
        let text: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8).collect();
        let mut block = vec![0xac, 0x02, 61 << 2, 0x2b, 0x01];
        block.extend_from_slice(&text);
        assert_eq!(decompress(&block).expect("two extra bytes"), text);
    }

    #[test]
    fn the_test_compressor_round_trips() {
        for len in [0usize, 1, 59, 60, 61, 255, 256, 257, 5000, 70_000] {
            let text: Vec<u8> = (0..len).map(|i| (i * 7 % 256) as u8).collect();
            assert_eq!(
                decompress(&compress_literals(&text)).expect("round trip"),
                text,
                "{len}"
            );
        }
    }

    #[test]
    fn bad_blocks_are_errors_and_never_panics() {
        // A copy from before the start, a copy of offset 0, a literal past the end, a wrong length.
        assert!(decompress(&[0x04, 0x01, 0x01]).is_err());
        assert!(decompress(&[0x05, 0x08, b'a', b'b', b'c', 0x01, 0x00]).is_err());
        assert!(decompress(&[0x05, 0x10, b'a']).is_err());
        assert!(decompress(&[0x05, 0x08, b'a', b'b', b'c']).is_err());
        assert!(decompress(&[0x02, 0x08, b'a', b'b', b'c']).is_err());
        // No length, an endless length, a huge length, a truncated offset.
        assert!(decompress(&[]).is_err());
        assert!(decompress(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_err());
        assert!(decompress(&[0xff, 0xff, 0xff, 0xff, 0x0f]).is_err());
        assert!(decompress(&[0x08, 0x0c, b'a', b'b', b'c', b'd', 0x0e, 0x04]).is_err());
        // Every prefix of a good block is an error or the right text, never a crash.
        let good = [0x09, 0x08, b'a', b'b', b'c', 0x09, 0x03];
        for cut in 0..good.len() {
            assert!(decompress(&good[..cut]).is_err(), "{cut}");
        }
    }
}
