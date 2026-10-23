//! Reading a tar archive's entries as its headers go by: ustar, GNU long names and links, and PAX
//! headers (path, link, size and time), with every header's checksum checked. Glint reads tar itself
//! rather than with the tar crate, which reads a GNU long name or a PAX header into memory however
//! big it says it is.

use super::tree::{Item, UNREADABLE};
use super::Failure;
use std::io::{self, Read};

/// The longest name a GNU long-name entry may give (Windows' paths stop well before).
const MAX_LONG_NAME: u64 = 64 << 10;
/// The biggest PAX header read.
const MAX_PAX: u64 = 1 << 20;
/// Sizes past this are taken for damage (no disk holds an entry that big).
const MAX_SIZE: u64 = 1 << 62;

/// Where an entry's bytes are in the (decompressed) tar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data {
    /// `size` bytes at `offset`.
    At { offset: u64, size: u64 },
    /// A link's target, given as a small file holding it (as zips' links are extracted).
    Inline(Box<[u8]>),
    /// A hard link to an earlier entry: its bytes (resolved once all are read).
    LinkTo(String),
    /// Nothing to read: a folder, or what can't be extracted (`problem`).
    None,
}

/// One entry: what the tree shows of it, and where its bytes are.
#[derive(Debug, Clone)]
pub struct Entry {
    pub item: Item,
    pub data: Data,
    pub problem: Option<Failure>,
}

/// Whether a 512-byte block looks like a tar header: "ustar" at 257, or (old tars) a checksum that
/// adds up.
pub fn is_header(block: &[u8]) -> bool {
    block.len() >= 512 && (block[257..262] == *b"ustar" || checksum_ok(block)) && block.iter().any(|&b| b != 0)
}

fn checksum_ok(block: &[u8]) -> bool {
    let Some(stored) = number(&block[148..156]) else { return false };
    let unsigned: u64 = block.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { b' ' as u64 } else { b as u64 }).sum();
    let signed: i64 = block.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { b' ' as i64 } else { b as i8 as i64 }).sum();
    stored == unsigned || stored as i64 == signed
}

/// A header's number: octal digits (spaces and NULs around them), or base-256 when the high bit of
/// the first byte is set (GNU's way for sizes past 8 GB).
fn number(field: &[u8]) -> Option<u64> {
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        let mut value = (field[0] & 0x7F) as u64;
        for &b in &field[1..] {
            value = value.checked_mul(256)?.checked_add(b as u64)?;
        }
        return Some(value);
    }
    let digits = field.iter().copied().skip_while(|&b| b == b' ' || b == 0).take_while(|&b| (b'0'..=b'7').contains(&b));
    let mut value = 0u64;
    let mut any = false;
    for d in digits {
        value = value.checked_mul(8)?.checked_add((d - b'0') as u64)?;
        any = true;
    }
    // An all-blank field is 0 (some writers leave the mtime of a link empty).
    Some(if any { value } else { 0 })
}

/// A NUL-ended text field.
fn text(field: &[u8]) -> &[u8] {
    &field[..field.iter().position(|&b| b == 0).unwrap_or(field.len())]
}

fn name(bytes: &[u8]) -> String {
    super::read::zip_name(bytes, true)
}

/// Reads exactly `n` bytes into a buffer, failing at the end of the tar.
fn read_vec(reader: &mut impl Read, n: u64) -> Result<Vec<u8>, Failure> {
    let mut buf = Vec::with_capacity(n as usize);
    reader.take(n).read_to_end(&mut buf)?;
    if buf.len() as u64 != n {
        return Err(Failure::Damaged("it ends too early (is it complete?)".into()));
    }
    Ok(buf)
}

/// How `walk` passes over data it doesn't need: by reading it (a compressed stream), or with a
/// seek (a plain tar on disk). Returns how many bytes it passed.
pub type Skip<'a, R> = &'a mut dyn FnMut(&mut R, u64) -> io::Result<u64>;

/// Passes over `n` bytes by reading them.
pub fn read_through<R: Read>(reader: &mut R, n: u64) -> io::Result<u64> {
    io::copy(&mut reader.take(n), &mut io::sink())
}

fn skip<R: Read>(reader: &mut R, n: u64, skip_by: Skip<'_, R>) -> Result<(), Failure> {
    if n > 0 && skip_by(reader, n)? != n {
        return Err(Failure::Damaged("it ends too early (is it complete?)".into()));
    }
    Ok(())
}

/// The bytes an entry of `size` takes: whole 512-byte blocks.
fn padded(size: u64) -> u64 {
    size.div_ceil(512) * 512
}

/// PAX records ("length key=value\n"): path, linkpath, size and mtime are what Glint uses.
#[derive(Default)]
struct Pax {
    path: Option<String>,
    link: Option<String>,
    size: Option<u64>,
    mtime: Option<i64>,
}

fn parse_pax(bytes: &[u8], into: &mut Pax) {
    let mut rest = bytes;
    while let Some(space) = rest.iter().position(|&b| b == b' ') {
        let Some(len) = std::str::from_utf8(&rest[..space]).ok().and_then(|l| l.parse::<usize>().ok()) else { return };
        if len <= space || len > rest.len() {
            return;
        }
        let record = &rest[space + 1..len];
        let record = record.strip_suffix(b"\n").unwrap_or(record);
        if let Some(eq) = record.iter().position(|&b| b == b'=') {
            let (key, value) = (&record[..eq], &record[eq + 1..]);
            match key {
                b"path" => into.path = Some(name(value)),
                b"linkpath" => into.link = Some(name(value)),
                b"size" => into.size = std::str::from_utf8(value).ok().and_then(|v| v.parse().ok()),
                b"mtime" => into.mtime = std::str::from_utf8(value).ok().and_then(|v| v.parse::<f64>().ok()).map(|s| (s * 1000.0) as i64),
                _ => {}
            }
        }
        rest = &rest[len..];
    }
}

/// Walks a tar's headers from the start of `reader` (decompressed), passing over its data with
/// `skip_by`, and returns every entry in the order stored. `max_entries` keeps a crafted tar from
/// filling memory.
pub fn walk<R: Read>(reader: &mut R, max_entries: usize, skip_by: Skip<'_, R>) -> Result<Vec<Entry>, Failure> {
    let mut entries = Vec::new();
    let mut at = 0u64;
    let mut long_name: Option<String> = None;
    let mut long_link: Option<String> = None;
    let mut pax = Pax::default();
    let mut global_mtime: Option<i64> = None;
    let mut block = [0u8; 512];
    loop {
        // The end: a block of zeros (two, by the book; one, or none, from tools that stop early).
        let got = read_block(reader, &mut block)?;
        if !got || block.iter().all(|&b| b == 0) {
            break;
        }
        at += 512;
        if !checksum_ok(&block) {
            return Err(Failure::Damaged("a tar header's checksum doesn't match".into()));
        }
        let kind = block[156];
        let size = number(&block[124..136]).filter(|&s| s <= MAX_SIZE).ok_or_else(|| Failure::Damaged("a tar header has no size".into()))?;
        match kind {
            // GNU long name and link name: the next header's, in this entry's data.
            b'L' | b'K' => {
                if size > MAX_LONG_NAME {
                    return Err(Failure::Bomb(format!("a tar entry says its name is {size} bytes long")));
                }
                let bytes = read_vec(reader, size)?;
                skip(reader, padded(size) - size, skip_by)?;
                at += padded(size);
                let value = name(text(&bytes));
                if kind == b'L' {
                    long_name = Some(value);
                } else {
                    long_link = Some(value);
                }
                continue;
            }
            // PAX: for the next entry ('x'), or for every one after ('g', of which only the time counts).
            b'x' | b'g' => {
                if size > MAX_PAX {
                    return Err(Failure::Bomb(format!("a tar entry's extended header is {size} bytes")));
                }
                let bytes = read_vec(reader, size)?;
                skip(reader, padded(size) - size, skip_by)?;
                at += padded(size);
                if kind == b'x' {
                    parse_pax(&bytes, &mut pax);
                } else {
                    let mut global = Pax::default();
                    parse_pax(&bytes, &mut global);
                    global_mtime = global.mtime.or(global_mtime);
                }
                continue;
            }
            _ => {}
        }
        let ustar = block[257..263] == *b"ustar\0";
        let path = pax.path.take().or(long_name.take()).unwrap_or_else(|| {
            let own = text(&block[..100]);
            // ustar's prefix holds the folders of a path too long for the name field.
            let prefix = if ustar { text(&block[345..500]) } else { &[] };
            if prefix.is_empty() {
                name(own)
            } else {
                format!("{}/{}", name(prefix), name(own))
            }
        });
        let link = pax.link.take().or(long_link.take()).unwrap_or_else(|| name(text(&block[157..257])));
        let size = pax.size.take().unwrap_or(size);
        let modified = pax.mtime.take().or(global_mtime).unwrap_or_else(|| number(&block[136..148]).unwrap_or(0) as i64 * 1000);
        // Only what Windows has: read-only when nobody may write it.
        let mode = number(&block[100..108]).unwrap_or(0o644);
        let attrs = if mode & 0o222 == 0 { 0x1 } else { 0 };
        let data_at = at;
        // What the entry's own header says it stores (a hard link's data is its target's).
        let stored = match kind {
            b'1' | b'2' | b'3' | b'4' | b'5' | b'6' => 0,
            _ => size,
        };
        skip(reader, padded(stored), skip_by)?;
        at += padded(stored);
        let item = |dir: bool, size: u64, attrs: u32| Item { path: path.clone(), dir, size, modified, attrs };
        let entry = match kind {
            b'5' | b'D' => Entry { item: item(true, 0, attrs), data: Data::None, problem: None },
            // A symlink: a small file holding where it points.
            b'2' => Entry { item: item(false, link.len() as u64, attrs), data: Data::Inline(link.clone().into_bytes().into_boxed_slice()), problem: None },
            b'1' => Entry { item: item(false, 0, attrs), data: Data::LinkTo(link), problem: None },
            // Devices and FIFOs have no data worth a file; a volume label isn't an entry.
            b'3' | b'4' | b'6' | b'V' => continue,
            b'S' | b'M' => {
                let what = if kind == b'S' { "a sparse file (GNU tar's --sparse)" } else { "split across several tar volumes" };
                Entry { item: item(false, size, attrs | UNREADABLE), data: Data::None, problem: Some(Failure::Unsupported(what.into())) }
            }
            // Regular files, contiguous files, and (as POSIX says to) any type not known.
            _ => Entry { item: item(false, size, attrs), data: Data::At { offset: data_at, size }, problem: None },
        };
        if entries.len() >= max_entries {
            return Err(Failure::TooManyEntries(entries.len() + 1));
        }
        entries.push(entry);
    }
    resolve_links(&mut entries);
    Ok(entries)
}

/// Reads one block; false at a clean end of the stream.
fn read_block(reader: &mut impl Read, block: &mut [u8; 512]) -> Result<bool, Failure> {
    let mut got = 0;
    while got < 512 {
        match reader.read(&mut block[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Failure::from(e)),
        }
    }
    match got {
        0 => Ok(false),
        512 => Ok(true),
        _ => Err(Failure::Damaged("it ends in the middle of a tar header (is it complete?)".into())),
    }
}

/// A hard link reads its target's bytes: the last entry before it with that path. One whose target
/// isn't there can't be extracted.
fn resolve_links(entries: &mut [Entry]) {
    let clean = |p: &str| p.trim_start_matches("./").trim_matches('/').to_string();
    let mut by_path: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for i in 0..entries.len() {
        if let Data::LinkTo(target) = entries[i].data.clone() {
            match by_path.get(&clean(&target)).map(|&t| (entries[t].data.clone(), entries[t].item.size)) {
                Some((Data::At { offset, size }, _)) => {
                    entries[i].data = Data::At { offset, size };
                    entries[i].item.size = size;
                }
                Some((Data::Inline(bytes), size)) => {
                    entries[i].data = Data::Inline(bytes);
                    entries[i].item.size = size;
                }
                _ => {
                    entries[i].data = Data::None;
                    entries[i].item.attrs |= UNREADABLE;
                    entries[i].problem = Some(Failure::Damaged(format!("it's a link to “{target}”, which isn't in the archive")));
                }
            }
        }
        if !entries[i].item.dir {
            by_path.insert(clean(&entries[i].item.path), i);
        }
    }
}

#[cfg(test)]
pub mod testing {
    /// A tar header block for `path`, of `kind`, `size` bytes, with a correct checksum.
    pub fn header(path: &str, kind: u8, size: u64, link: &str) -> [u8; 512] {
        let mut block = [0u8; 512];
        block[..path.len().min(100)].copy_from_slice(&path.as_bytes()[..path.len().min(100)]);
        block[100..107].copy_from_slice(b"0000644");
        block[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
        block[136..147].copy_from_slice(b"14727123000");
        block[156] = kind;
        block[157..157 + link.len()].copy_from_slice(link.as_bytes());
        block[257..263].copy_from_slice(b"ustar\0");
        block[263..265].copy_from_slice(b"00");
        block[148..156].copy_from_slice(b"        ");
        let sum: u64 = block.iter().map(|&b| b as u64).sum();
        block[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        block
    }
}

#[cfg(test)]
mod tests {
    use super::testing::header;
    use super::*;

    fn tar_of(parts: &[(&[u8; 512], &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (head, data) in parts {
            out.extend_from_slice(*head);
            out.extend_from_slice(data);
            out.resize(out.len().div_ceil(512) * 512, 0);
        }
        out.extend([0u8; 1024]);
        out
    }

    #[test]
    fn reads_numbers_in_octal_and_base_256() {
        assert_eq!(number(b"00000001750\0"), Some(1000));
        assert_eq!(number(b"   1750 \0"), Some(1000));
        assert_eq!(number(b"\0\0\0\0"), Some(0));
        assert_eq!(number(&[0x80, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0]), Some(2 << 32));
    }

    #[test]
    fn walks_files_folders_links_and_long_names() {
        let long = format!("{}/deep.txt", "folder".repeat(30));
        // A PAX record counts its own length's digits and the space: 2 + 1 + 19 = 22.
        let pax = format!("{} path=pax-named.txt\n", "path=pax-named.txt\n".len() + 3);
        let tar = tar_of(&[
            (&header("docs/", b'5', 0, ""), b""),
            (&header("docs/a.txt", b'0', 5, ""), b"hello"),
            (&header("././@LongLink", b'L', long.len() as u64 + 1, ""), format!("{long}\0").as_bytes()),
            (&header("short", b'0', 3, ""), b"abc"),
            (&header("pax", b'x', pax.len() as u64, ""), pax.as_bytes()),
            (&header("ignored.txt", b'0', 2, ""), b"hi"),
            (&header("docs/link", b'2', 0, "a.txt"), b""),
            (&header("docs/hard.txt", b'1', 0, "docs/a.txt"), b""),
            (&header("dev", b'3', 0, ""), b""),
        ]);
        let entries = walk(&mut tar.as_slice(), 100, &mut read_through).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.item.path.as_str()).collect();
        assert_eq!(paths, ["docs/", "docs/a.txt", long.as_str(), "pax-named.txt", "docs/link", "docs/hard.txt"]);
        assert!(entries[0].item.dir);
        assert_eq!(entries[1].data, Data::At { offset: 1024, size: 5 });
        assert_eq!(entries[4].data, Data::Inline(b"a.txt".to_vec().into_boxed_slice()));
        assert_eq!((entries[5].data.clone(), entries[5].item.size), (Data::At { offset: 1024, size: 5 }, 5), "a hard link reads its target");
        assert_eq!(entries[1].item.modified, 0o14727123000 * 1000);
    }

    #[test]
    fn refuses_what_a_crafted_tar_could_do() {
        // A long name claiming gigabytes, a PAX header too, a bad checksum, data cut short.
        let huge = tar_of(&[(&header("././@LongLink", b'L', 4 << 30, ""), b"")]);
        assert!(matches!(walk(&mut huge.as_slice(), 100, &mut read_through), Err(Failure::Bomb(_))));
        let pax = tar_of(&[(&header("pax", b'x', 2 << 20, ""), b"")]);
        assert!(matches!(walk(&mut pax.as_slice(), 100, &mut read_through), Err(Failure::Bomb(_))));
        let mut bad = header("a.txt", b'0', 1, "");
        bad[0] = b'b';
        assert!(matches!(walk(&mut tar_of(&[(&bad, b"x")]).as_slice(), 100, &mut read_through), Err(Failure::Damaged(_))));
        let cut = &tar_of(&[(&header("a.txt", b'0', 5000, ""), &[1; 5000])])[..2000];
        assert!(walk(&mut &cut[..], 100, &mut read_through).is_err());
        let many: Vec<_> = (0..5).map(|i| header(&format!("{i}.txt"), b'0', 0, "")).collect();
        let parts: Vec<(&[u8; 512], &[u8])> = many.iter().map(|h| (h, &b""[..])).collect();
        assert!(matches!(walk(&mut tar_of(&parts).as_slice(), 3, &mut read_through), Err(Failure::TooManyEntries(_))));
        // A hard link to nothing can't be extracted; sparse files say so.
        let tar = tar_of(&[(&header("lost", b'1', 0, "nowhere"), b""), (&header("sparse", b'S', 10, ""), &[0; 10])]);
        let entries = walk(&mut tar.as_slice(), 100, &mut read_through).unwrap();
        assert!(entries.iter().all(|e| e.problem.is_some() && e.item.attrs & UNREADABLE != 0));
        assert!(is_header(&header("x", b'0', 0, "")) && !is_header(&[0u8; 512]) && !is_header(b"hello"));
    }
}
