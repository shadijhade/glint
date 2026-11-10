//! Saves a volume's index to disk and loads it back, so a restart only has to replay the journal
//! from where the snapshot left off instead of reading the whole MFT again.
//!
//! Layout (little-endian): magic, format version, the growth horizon, the volume serial number,
//! journal id and next USN the index is current to, the save time, the five array lengths, then the
//! records (28 bytes each), the links (20 bytes each), the name arena, the growth marks (24 bytes
//! each) and notes (16 bytes each), and a checksum of everything before it.

use crate::growth::{Growth, Mark, Note};
use crate::tree::{IdMap, Link, Rec, Tree, LINK_FOLDED, LINK_USED, NONE};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"SHUIIDX\0";
/// Bumped whenever what the arrays mean changes (2: NTFS-internal attribute bits dropped; 3: what
/// was made in a folder that took a deleted file's record is no longer lost, and one fresh scan
/// finds what an older service lost that way; 4: folders' sizes at daily marks; 5: links with the
/// same name share it in the arena).
const FORMAT: u32 = 5;
/// The format before names were shared, still read: they're shared once it's loaded.
const UNSHARED: u32 = 4;
const HEADER: usize = 8 + 4 + 4 + 8 * 4 + 8 * 5;
const REC: usize = 28;
const LINK: usize = 20;
const MARK: usize = 24;
const NOTE: usize = 16;

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
pub(crate) struct Checksum {
    hash: u64,
    pending: Vec<u8>,
}

impl Checksum {
    pub(crate) fn update(&mut self, mut bytes: &[u8]) {
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

    pub(crate) fn finish(mut self) -> u64 {
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

    let (horizon, marks, notes) = tree.growth.parts();
    let mut header = Vec::with_capacity(HEADER);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&FORMAT.to_le_bytes());
    header.extend_from_slice(&(horizon as u32).to_le_bytes());
    for value in [meta.volume_serial, meta.journal_id, meta.next_usn as u64, meta.saved_at as u64] {
        header.extend_from_slice(&value.to_le_bytes());
    }
    for len in [tree.recs.len(), tree.links.len(), tree.names.len(), marks.len(), notes.len()] {
        header.extend_from_slice(&(len as u64).to_le_bytes());
    }
    put(&mut file, &header)?;

    let mut buf = Vec::with_capacity(REC * 4096);
    for chunk in tree.recs.chunks(4096) {
        buf.clear();
        for r in chunk {
            buf.extend_from_slice(&r.size().to_le_bytes());
            buf.extend_from_slice(&r.modified.to_le_bytes());
            buf.extend_from_slice(&r.attrs().to_le_bytes());
            buf.extend_from_slice(&r.first_link.to_le_bytes());
            buf.extend_from_slice(&r.seq().to_le_bytes());
            buf.extend_from_slice(&r.flags().to_le_bytes());
        }
        put(&mut file, &buf)?;
    }
    for (i, chunk) in tree.links.chunks(4096).enumerate() {
        buf.clear();
        for (j, l) in chunk.iter().enumerate() {
            buf.extend_from_slice(&l.rec.to_le_bytes());
            buf.extend_from_slice(&l.parent.to_le_bytes());
            buf.extend_from_slice(&l.name.to_le_bytes());
            buf.extend_from_slice(&l.len.to_le_bytes());
            buf.extend_from_slice(&l.flags.to_le_bytes());
            buf.extend_from_slice(&tree.next_name((i * 4096 + j) as u32).to_le_bytes());
        }
        put(&mut file, &buf)?;
    }
    put(&mut file, &tree.names)?;
    buf.clear();
    for m in marks {
        buf.extend_from_slice(&(m.epoch as u64).to_le_bytes());
        buf.extend_from_slice(&m.at.to_le_bytes());
        buf.extend_from_slice(&m.total.to_le_bytes());
    }
    put(&mut file, &buf)?;
    for chunk in notes.chunks(4096) {
        buf.clear();
        for n in chunk {
            buf.extend_from_slice(&n.rec.to_le_bytes());
            buf.extend_from_slice(&n.seq.to_le_bytes());
            buf.extend_from_slice(&n.epoch.to_le_bytes());
            buf.extend_from_slice(&n.before.to_le_bytes());
        }
        put(&mut file, &buf)?;
    }
    file.write_all(&sum.finish().to_le_bytes())?;
    file.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    fs::rename(&temp, path)
}

pub fn load(path: &Path) -> Result<(Tree, Meta), String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    // Decoded as it's read: the file and the index made from it never take memory at once.
    decode_from(io::BufReader::with_capacity(1 << 20, file), len)
}

#[cfg(test)]
fn decode(bytes: &[u8]) -> Result<(Tree, Meta), String> {
    decode_from(bytes, bytes.len() as u64)
}

/// A snapshot's sections, read in turn and summed as they go by.
struct Input<R> {
    from: R,
    sum: Checksum,
}

impl<R: io::Read> Input<R> {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), String> {
        self.from.read_exact(buf).map_err(|e| format!("snapshot unreadable: {e}"))?;
        self.sum.update(buf);
        Ok(())
    }

    /// `count` items of `size` bytes, each made by `item` from its index and bytes.
    fn items<T>(&mut self, count: usize, size: usize, mut item: impl FnMut(usize, &[u8]) -> T) -> Result<Vec<T>, String> {
        let mut items = Vec::with_capacity(count);
        let mut chunk = vec![0u8; size * 4096];
        while items.len() < count {
            let bytes = &mut chunk[..(count - items.len()).min(4096) * size];
            self.fill(bytes)?;
            for one in bytes.chunks_exact(size) {
                items.push(item(items.len(), one));
            }
        }
        Ok(items)
    }
}

fn u16_in(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn u32_in(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_in(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

/// A snapshot of `len` bytes from `from`.
fn decode_from(from: impl io::Read, len: u64) -> Result<(Tree, Meta), String> {
    let mut input = Input { from, sum: Checksum::default() };
    if len < (HEADER + 8) as u64 {
        return Err("not a snapshot of this format".into());
    }
    let mut header = [0u8; HEADER];
    input.fill(&mut header)?;
    let h = &header[..];
    let format = u32_in(h, 8);
    if &h[..8] != MAGIC || ![FORMAT, UNSHARED].contains(&format) {
        return Err("not a snapshot of this format".into());
    }
    let meta = Meta { volume_serial: u64_in(h, 16), journal_id: u64_in(h, 24), next_usn: u64_in(h, 32) as i64, saved_at: u64_in(h, 40) as i64 };
    let (recs, links, names) = (u64_in(h, 48) as usize, u64_in(h, 56) as usize, u64_in(h, 64) as usize);
    let (marks, notes) = (u64_in(h, 72) as usize, u64_in(h, 80) as usize);
    let body = recs.checked_mul(REC).and_then(|r| {
        let growth = marks.checked_mul(MARK)?.checked_add(notes.checked_mul(NOTE)?)?;
        links.checked_mul(LINK)?.checked_add(r)?.checked_add(names)?.checked_add(growth)
    });
    // (So nothing below makes room for more than the file holds.)
    if body.and_then(|b| b.checked_add(HEADER + 8)).map(|n| n as u64) != Some(len) {
        return Err("snapshot size doesn't match its header".into());
    }

    let rec_list = input.items(recs, REC, |_, b| Rec::new(u64_in(b, 0), u64_in(b, 8) as i64, u32_in(b, 16), u32_in(b, 20), u16_in(b, 24), u16_in(b, 26)))?;
    let mut next_names = IdMap::default();
    let link_list = input.items(links, LINK, |id, b| {
        let link = Link { rec: u32_in(b, 0), parent: u32_in(b, 4), name: u32_in(b, 8), len: u16_in(b, 12), flags: u16_in(b, 14) };
        let next = u32_in(b, 16);
        if next != NONE && link.flags & LINK_USED != 0 {
            next_names.insert(id as u32, next);
        }
        link
    })?;
    let mut arena = vec![0u8; names];
    input.fill(&mut arena)?;
    let mark_list = input.items(marks, MARK, |_, b| Mark { epoch: u64_in(b, 0) as u16, at: u64_in(b, 8) as i64, total: u64_in(b, 16) })?;
    let note_list = input.items(notes, NOTE, |_, b| Note { rec: u32_in(b, 0), seq: u16_in(b, 4), epoch: u16_in(b, 6), before: u64_in(b, 8) })?;
    let mut stored = [0u8; 8];
    input.from.read_exact(&mut stored).map_err(|e| format!("snapshot unreadable: {e}"))?;
    if input.sum.finish() != u64::from_le_bytes(stored) {
        return Err("snapshot checksum mismatch".into());
    }
    // Notes that don't hang together only cost what grew: the index itself is fine.
    let growth = Growth::from_parts(u32_in(h, 12) as u16, mark_list, note_list).unwrap_or_default();

    // Everything the index will follow must stay in bounds, whatever the file says.
    let valid_link = |id: u32| id == NONE || (id as usize) < link_list.len();
    if !rec_list.iter().all(|r| valid_link(r.first_link)) {
        return Err("snapshot record points past the links".into());
    }
    if !next_names.values().all(|&next| valid_link(next)) {
        return Err("snapshot link out of bounds".into());
    }
    for link in link_list.iter().filter(|l| l.flags & LINK_USED != 0) {
        let name_end = link.name as usize + link.len as usize;
        let folded_ok = link.flags & LINK_FOLDED == 0
            || arena.get(name_end..name_end + 2).is_some_and(|len| name_end + 2 + u16::from_le_bytes([len[0], len[1]]) as usize <= arena.len());
        let ok = (link.rec as usize) < rec_list.len()
            && (link.parent == NONE || (link.parent as usize) < rec_list.len())
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
            (id, steps) = (next_names.get(&id).copied().unwrap_or(NONE), steps + 1);
        }
    }
    let mut tree = Tree::from_parts(rec_list, link_list, arena, next_names);
    if format == UNSHARED {
        tree.compact();
    }
    tree.growth = growth;
    Ok((tree, meta))
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
        let mut tree = sample();
        // A mark, then a.txt grows: its folder's size before is noted.
        tree.growth.mark(1, 42);
        tree.set_record(21, &RecordState { seq: 1, size: 50, modified: 9, attrs: 0x20, names: vec![(make_ref(20, 1), "a.txt".into())], ..Default::default() });
        // And a file with two names, the second kept aside.
        tree.set_record(24, &RecordState { seq: 1, size: 5, names: vec![(make_ref(ROOT, 5), "one.lnk".into()), (make_ref(ROOT, 5), "two.lnk".into())], ..Default::default() });
        save(&path, &tree, &meta).unwrap();
        let (loaded, loaded_meta) = load(&path).unwrap();
        assert_eq!(loaded_meta, meta);
        assert_eq!(loaded.recs, tree.recs);
        assert_eq!(loaded.links, tree.links);
        assert_eq!(loaded.next_names, tree.next_names);
        assert_eq!(loaded.links_of(24).map(|id| loaded.name(id)).collect::<Vec<_>>(), ["two.lnk", "one.lnk"]);
        assert_eq!(loaded.resolve("üsers"), Some(20));
        let matcher = NameMatcher::new("txt").unwrap();
        let excludes = Excludes::default();
        let found = loaded.search(&Search { matcher: &matcher, filters: &Default::default(), now: 0, scope: ROOT, hidden: true, excludes: &excludes });
        assert_eq!(found.iter().map(|&id| loaded.path(id, &mut HashMap::new()).unwrap()).collect::<Vec<_>>(), [r"Üsers\a.txt"]);
        assert_eq!(loaded.rec(20).unwrap().size(), 50);
        assert_eq!(loaded.growth.last().map(|m| (m.at, m.total)), Some((1, 42)));
        assert_eq!(loaded.growth.then(20, 1, 0, 50), Some(42));

        let mut bytes = fs::read(&path).unwrap();
        bytes[HEADER + 3] ^= 1;
        assert!(decode(&bytes).err().unwrap().contains("checksum"));
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
        assert!(decode(b"nothing").is_err());
        // A snapshot from before names were shared still loads, and shares them.
        let mut twice = sample();
        twice.set_record(23, &RecordState { seq: 1, size: 3, names: vec![(make_ref(ROOT, 5), "a.txt".into())], ..Default::default() });
        save(&path, &twice, &meta).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        bytes[8..12].copy_from_slice(&UNSHARED.to_le_bytes());
        let end = bytes.len() - 8;
        let mut sum = Checksum::default();
        sum.update(&bytes[..end]);
        let sum = sum.finish();
        bytes[end..].copy_from_slice(&sum.to_le_bytes());
        let (old, _) = decode(&bytes).unwrap();
        // "gone.txt" (gone) and the second "a.txt" aren't kept.
        assert_eq!(old.names.len(), twice.names.len() - "gone.txt".len() - "a.txt".len());
        let found = old.search(&Search { matcher: &NameMatcher::new("a.txt").unwrap(), filters: &Default::default(), now: 0, scope: ROOT, hidden: true, excludes: &excludes });
        let mut paths: Vec<String> = found.iter().map(|&id| old.path(id, &mut HashMap::new()).unwrap()).collect();
        paths.sort();
        assert_eq!(paths, ["a.txt", r"Üsers\a.txt"]);
        // A snapshot whose checksum is right but whose names go round in circles is refused.
        let mut looped = sample();
        let first = looped.recs[21].first_link;
        looped.next_names.insert(first, first);
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
