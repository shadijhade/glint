//! Saves a volume's index to disk and loads it back, so a restart only has to replay the journal
//! from where the snapshot left off instead of reading the whole MFT again.
//!
//! Layout (little-endian): magic, format version, the volume serial number, journal id and next
//! USN the index is current to, the save time, the three array lengths, then the records (28 bytes
//! each), the links (20 bytes each), the name arena, and a checksum of everything before it.

use crate::tree::{Link, Rec, Tree, LINK_FOLDED, LINK_USED, NONE};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"SHUIIDX\0";
/// Bumped whenever what the arrays mean changes (2: NTFS-internal attribute bits dropped).
const FORMAT: u32 = 2;
const HEADER: usize = 8 + 4 + 4 + 8 * 4 + 8 * 3;
const REC: usize = 28;
const LINK: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Meta {
    pub volume_serial: u64,
    pub journal_id: u64,
    /// The journal position the index is current to.
    pub next_usn: i64,
    /// Unix milliseconds.
    pub saved_at: i64,
}

/// A checksum fast enough not to matter next to the disk (words mixed in order).
#[derive(Default)]
struct Checksum {
    hash: u64,
    pending: Vec<u8>,
}

impl Checksum {
    fn update(&mut self, mut bytes: &[u8]) {
        if !self.pending.is_empty() {
            let take = (8 - self.pending.len()).min(bytes.len());
            self.pending.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.pending.len() == 8 {
                let word = u64::from_le_bytes(self.pending[..].try_into().unwrap());
                self.mix(word);
                self.pending.clear();
            }
        }
        let (words, rest) = bytes.as_chunks::<8>();
        for word in words {
            self.mix(u64::from_le_bytes(*word));
        }
        self.pending.extend_from_slice(rest);
    }

    fn mix(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(23) ^ word).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }

    fn finish(mut self) -> u64 {
        let mut last = [0u8; 8];
        last[..self.pending.len()].copy_from_slice(&self.pending);
        self.mix(u64::from_le_bytes(last) ^ self.pending.len() as u64);
        self.hash
    }
}

/// Writes the snapshot to `path` atomically (a temporary file, then a rename).
pub fn save(path: &Path, tree: &Tree, meta: &Meta) -> io::Result<()> {
    let temp = path.with_extension("tmp");
    let mut file = io::BufWriter::with_capacity(1 << 20, fs::File::create(&temp)?);
    let mut sum = Checksum::default();
    let mut put = |file: &mut io::BufWriter<fs::File>, bytes: &[u8]| -> io::Result<()> {
        sum.update(bytes);
        file.write_all(bytes)
    };

    let mut header = Vec::with_capacity(HEADER);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&FORMAT.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    for value in [meta.volume_serial, meta.journal_id, meta.next_usn as u64, meta.saved_at as u64] {
        header.extend_from_slice(&value.to_le_bytes());
    }
    for len in [tree.recs.len(), tree.links.len(), tree.names.len()] {
        header.extend_from_slice(&(len as u64).to_le_bytes());
    }
    put(&mut file, &header)?;

    let mut buf = Vec::with_capacity(REC * 4096);
    for chunk in tree.recs.chunks(4096) {
        buf.clear();
        for r in chunk {
            buf.extend_from_slice(&r.size.to_le_bytes());
            buf.extend_from_slice(&r.modified.to_le_bytes());
            buf.extend_from_slice(&r.attrs.to_le_bytes());
            buf.extend_from_slice(&r.first_link.to_le_bytes());
            buf.extend_from_slice(&r.seq.to_le_bytes());
            buf.extend_from_slice(&r.flags.to_le_bytes());
        }
        put(&mut file, &buf)?;
    }
    for chunk in tree.links.chunks(4096) {
        buf.clear();
        for l in chunk {
            buf.extend_from_slice(&l.rec.to_le_bytes());
            buf.extend_from_slice(&l.parent.to_le_bytes());
            buf.extend_from_slice(&l.name.to_le_bytes());
            buf.extend_from_slice(&l.len.to_le_bytes());
            buf.extend_from_slice(&l.flags.to_le_bytes());
            buf.extend_from_slice(&l.next.to_le_bytes());
        }
        put(&mut file, &buf)?;
    }
    put(&mut file, &tree.names)?;
    file.write_all(&sum.finish().to_le_bytes())?;
    file.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    fs::rename(&temp, path)
}

pub fn load(path: &Path) -> Result<(Tree, Meta), String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    decode(&bytes)
}

fn decode(bytes: &[u8]) -> Result<(Tree, Meta), String> {
    let u16_at = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    if bytes.len() < HEADER + 8 || &bytes[..8] != MAGIC || u32_at(8) != FORMAT {
        return Err("not a snapshot of this format".into());
    }
    let meta = Meta { volume_serial: u64_at(16), journal_id: u64_at(24), next_usn: u64_at(32) as i64, saved_at: u64_at(40) as i64 };
    let (recs, links, names) = (u64_at(48) as usize, u64_at(56) as usize, u64_at(64) as usize);
    let body = recs.checked_mul(REC).and_then(|r| links.checked_mul(LINK)?.checked_add(r)?.checked_add(names));
    if body.and_then(|b| b.checked_add(HEADER + 8)) != Some(bytes.len()) {
        return Err("snapshot size doesn't match its header".into());
    }
    let end = bytes.len() - 8;
    let mut sum = Checksum::default();
    sum.update(&bytes[..end]);
    if sum.finish() != u64_at(end) {
        return Err("snapshot checksum mismatch".into());
    }

    let mut at = HEADER;
    let mut rec_list = Vec::with_capacity(recs);
    for _ in 0..recs {
        rec_list.push(Rec {
            size: u64_at(at),
            modified: u64_at(at + 8) as i64,
            attrs: u32_at(at + 16),
            first_link: u32_at(at + 20),
            seq: u16_at(at + 24),
            flags: u16_at(at + 26),
        });
        at += REC;
    }
    let mut link_list = Vec::with_capacity(links);
    for _ in 0..links {
        link_list.push(Link {
            rec: u32_at(at),
            parent: u32_at(at + 4),
            name: u32_at(at + 8),
            len: u16_at(at + 12),
            flags: u16_at(at + 14),
            next: u32_at(at + 16),
        });
        at += LINK;
    }
    let arena = bytes[at..end].to_vec();

    // Everything the index will follow must stay in bounds, whatever the file says.
    let valid_link = |id: u32| id == NONE || (id as usize) < link_list.len();
    if !rec_list.iter().all(|r| valid_link(r.first_link)) {
        return Err("snapshot record points past the links".into());
    }
    for link in link_list.iter().filter(|l| l.flags & LINK_USED != 0) {
        let name_end = link.name as usize + link.len as usize;
        let folded_ok = link.flags & LINK_FOLDED == 0
            || arena.get(name_end..name_end + 2).is_some_and(|len| name_end + 2 + u16::from_le_bytes([len[0], len[1]]) as usize <= arena.len());
        let ok = (link.rec as usize) < rec_list.len()
            && (link.parent == NONE || (link.parent as usize) < rec_list.len())
            && valid_link(link.next)
            && name_end <= arena.len()
            && std::str::from_utf8(&arena[link.name as usize..name_end]).is_ok()
            && folded_ok;
        if !ok {
            return Err("snapshot link out of bounds".into());
        }
    }
    // Each record's names are a chain of its own links in use, with an end: anything else would
    // send the index off the end of its links, or round in circles.
    for (n, rec) in rec_list.iter().enumerate() {
        let (mut id, mut steps) = (rec.first_link, 0);
        while id != NONE {
            let link = &link_list[id as usize];
            if link.flags & LINK_USED == 0 || link.rec as usize != n || steps == link_list.len() {
                return Err("snapshot names are tangled".into());
            }
            (id, steps) = (link.next, steps + 1);
        }
    }
    Ok((Tree::from_parts(rec_list, link_list, arena), meta))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{Excludes, NameMatcher};
    use crate::tree::{make_ref, RecordState, Search, ROOT};
    use std::collections::HashMap;

    fn sample() -> Tree {
        let mut tree = Tree::default();
        tree.set_record(ROOT, &RecordState { seq: 5, dir: true, ..Default::default() });
        tree.set_record(20, &RecordState { seq: 1, dir: true, names: vec![(make_ref(ROOT, 5), "Üsers".into())], ..Default::default() });
        tree.set_record(21, &RecordState { seq: 1, size: 42, modified: 9, attrs: 0x20, names: vec![(make_ref(20, 1), "a.txt".into())], ..Default::default() });
        tree.set_record(22, &RecordState { seq: 1, size: 8, names: vec![(make_ref(20, 1), "gone.txt".into())], ..Default::default() });
        tree.remove_record(22);
        tree
    }

    #[test]
    fn round_trips_and_rejects_damage() {
        let dir = std::env::temp_dir().join(format!("shadui-index-snapshot-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("C.idx");
        let meta = Meta { volume_serial: 0xABCD, journal_id: 7, next_usn: 123_456, saved_at: 1 };
        let tree = sample();
        save(&path, &tree, &meta).unwrap();
        let (loaded, loaded_meta) = load(&path).unwrap();
        assert_eq!(loaded_meta, meta);
        assert_eq!(loaded.recs, tree.recs);
        assert_eq!(loaded.links, tree.links);
        assert_eq!(loaded.resolve("üsers"), Some(20));
        let matcher = NameMatcher::new("txt").unwrap();
        let excludes = Excludes::default();
        let found = loaded.search(&Search { matcher: &matcher, filters: &Default::default(), now: 0, scope: ROOT, hidden: true, excludes: &excludes });
        assert_eq!(found.iter().map(|&id| loaded.path(id, &mut HashMap::new()).unwrap()).collect::<Vec<_>>(), [r"Üsers\a.txt"]);
        assert_eq!(loaded.rec(20).unwrap().size, 42);

        let mut bytes = fs::read(&path).unwrap();
        bytes[HEADER + 3] ^= 1;
        assert!(decode(&bytes).err().unwrap().contains("checksum"));
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
        assert!(decode(b"nothing").is_err());
        // A snapshot whose checksum is right but whose names go round in circles is refused.
        let mut looped = sample();
        let first = looped.recs[21].first_link;
        looped.links[first as usize].next = first;
        save(&path, &looped, &meta).unwrap();
        assert!(load(&path).err().unwrap().contains("tangled"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn checksum_doesnt_depend_on_how_bytes_arrive() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        let mut whole = Checksum::default();
        whole.update(&data);
        let mut pieces = Checksum::default();
        for piece in data.chunks(13) {
            pieces.update(piece);
        }
        assert_eq!(whole.finish(), pieces.finish());
    }
}
