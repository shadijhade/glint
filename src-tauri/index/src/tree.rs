//! The index of one NTFS volume: every file record (size, last write time, attributes) by MFT record
//! number, every name (hard links included) as a link from a record to its parent folder, and the
//! total size below every folder, kept current as changes come in.

use crate::growth::{Growth, RecordHasher, ABSENT};
use crate::query::{fold, is_hidden, Excludes, Filters, NameMatcher};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::hash::BuildHasherDefault;

/// No record or link.
pub const NONE: u32 = u32::MAX;
/// The root folder's MFT record.
pub const ROOT: u32 = 5;
/// Records below this are NTFS's own metadata ($MFT, $LogFile, $Extend…), except the root.
pub const FIRST_USER_RECORD: u32 = 16;
/// Deeper chains than this can only come from corruption (a cycle); NTFS paths stop far sooner.
const MAX_DEPTH: usize = 4096;

pub const REC_USED: u16 = 1;
pub const REC_DIR: u16 = 2;
/// The biggest size a record holds, 256 TiB: a file or a folder's total beyond it stays there.
pub const MAX_SIZE: u64 = (1 << 48) - 1;
/// The FILE_ATTRIBUTE_* bits a record keeps: the two above them (which Windows doesn't use) are its
/// flags.
const ATTRS: u32 = (1 << 30) - 1;
pub const LINK_USED: u16 = 1;
/// The name isn't ASCII: its lowercase form follows it in the arena, after a u16 length.
pub const LINK_FOLDED: u16 = 2;

pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// A file record. There's one for every record in the MFT, used or not (millions on a system drive),
/// so it's packed into 24 bytes: the sequence number shares a word with the size, and the flags take
/// the attributes' top bits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rec {
    /// The size in the low 48 bits, the sequence number in the high 16.
    size_seq: u64,
    /// Last write time as a FILETIME (100 ns ticks since 1601).
    pub modified: i64,
    /// FILE_ATTRIBUTE_* bits, and the flags in the top two.
    attrs_flags: u32,
    /// The record's first name, NONE when it has none.
    pub first_link: u32,
}

impl Default for Rec {
    fn default() -> Self {
        Rec { size_seq: 0, modified: 0, attrs_flags: 0, first_link: NONE }
    }
}

impl Rec {
    pub fn new(size: u64, modified: i64, attrs: u32, first_link: u32, seq: u16, flags: u16) -> Rec {
        let mut rec = Rec { modified, first_link, ..Rec::default() };
        rec.set_size(size);
        rec.set_seq(seq);
        rec.set_attrs(attrs);
        rec.set_flags(flags);
        rec
    }
    pub fn used(&self) -> bool {
        self.flags() & REC_USED != 0
    }
    pub fn dir(&self) -> bool {
        self.flags() & REC_DIR != 0
    }
    /// Files: bytes in the unnamed data stream. Folders: everything below them, every hard link counted
    /// where it is (as Explorer and pdu count them).
    pub fn size(&self) -> u64 {
        self.size_seq & MAX_SIZE
    }
    pub fn set_size(&mut self, size: u64) {
        self.size_seq = self.size_seq & !MAX_SIZE | size.min(MAX_SIZE);
    }
    /// Bumped by NTFS whenever the record is reused for another file.
    pub fn seq(&self) -> u16 {
        (self.size_seq >> 48) as u16
    }
    pub fn set_seq(&mut self, seq: u16) {
        self.size_seq = self.size_seq & MAX_SIZE | (seq as u64) << 48;
    }
    /// FILE_ATTRIBUTE_* bits; folders always carry FILE_ATTRIBUTE_DIRECTORY.
    pub fn attrs(&self) -> u32 {
        self.attrs_flags & ATTRS
    }
    pub fn set_attrs(&mut self, attrs: u32) {
        self.attrs_flags = self.attrs_flags & !ATTRS | attrs & ATTRS;
    }
    /// REC_USED and REC_DIR.
    pub fn flags(&self) -> u16 {
        (self.attrs_flags >> 30) as u16
    }
    pub fn set_flags(&mut self, flags: u16) {
        self.attrs_flags = self.attrs_flags & ATTRS | (flags as u32 & 3) << 30;
    }
}

/// One name of a record: a hard link from it to the folder it's in. Few records have more than one
/// name, so the name after each that isn't its record's last is kept aside (`Tree::next_name`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub rec: u32,
    pub parent: u32,
    /// Where the name starts in the arena.
    pub name: u32,
    /// Its length in bytes (UTF-8).
    pub len: u16,
    pub flags: u16,
}

/// From one record number or link id to another: they spread well enough for a hash of one multiply.
pub(crate) type IdMap = HashMap<u32, u32, BuildHasherDefault<RecordHasher>>;

/// What a file record says: read from the MFT, or again after the journal reported a change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecordState {
    pub seq: u16,
    pub dir: bool,
    pub attrs: u32,
    pub modified: i64,
    /// The unnamed data stream's size; ignored for folders, whose size is what's below them.
    pub size: u64,
    /// Every name as (parent's file reference, name): one per hard link, DOS 8.3 aliases left out.
    pub names: Vec<(u64, String)>,
}

/// Splits a 64-bit file reference into the record number and its sequence number.
pub fn split_ref(reference: u64) -> (u64, u16) {
    (reference & 0xFFFF_FFFF_FFFF, (reference >> 48) as u16)
}

pub fn make_ref(number: u32, seq: u16) -> u64 {
    number as u64 | (seq as u64) << 48
}

/// Puts each record of a batch after the folders it's in now, where they're in the batch too. NTFS
/// gives a deleted file's record to whatever comes next, so a folder made right after a delete may
/// have a record the batch first saw as the file deleted before it; read in first, what was made
/// in the folder would find that file still there and be left in no folder at all.
pub fn parents_first(batch: Vec<(u32, Option<RecordState>)>) -> Vec<(u32, Option<RecordState>)> {
    let at: HashMap<u32, usize> = batch.iter().enumerate().map(|(i, (n, _))| (*n, i)).collect();
    // Depth first, without recursion: a record is placed once the folders it's in are. A cycle
    // (a move between two reads) just ends where it closes.
    const NEW: u8 = 0;
    const OPEN: u8 = 1;
    const PLACED: u8 = 2;
    let mut mark = vec![NEW; batch.len()];
    let mut order = Vec::with_capacity(batch.len());
    let mut stack = Vec::new();
    for start in 0..batch.len() {
        stack.push((start, false));
        while let Some((i, parents_placed)) = stack.pop() {
            if parents_placed {
                mark[i] = PLACED;
                order.push(i);
                continue;
            }
            if mark[i] != NEW {
                continue;
            }
            mark[i] = OPEN;
            stack.push((i, true));
            for (parent, _) in batch[i].1.iter().flat_map(|s| s.names.iter()) {
                if let Some(&j) = u32::try_from(split_ref(*parent).0).ok().and_then(|n| at.get(&n)) {
                    if mark[j] == NEW {
                        stack.push((j, false));
                    }
                }
            }
        }
    }
    let mut batch: Vec<_> = batch.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| batch[i].take()).collect()
}

#[derive(Default)]
pub struct Tree {
    pub(crate) recs: Vec<Rec>,
    pub(crate) links: Vec<Link>,
    pub(crate) names: Vec<u8>,
    free: Vec<u32>,
    /// The record's next name after a link, for the links that aren't their record's last.
    pub(crate) next_names: IdMap,
    /// Each folder's child folders (as links), for resolving paths and listing folder sizes.
    kids: Kids,
    /// Arena bytes no live link uses any more.
    garbage: usize,
    /// Folders' sizes at daily marks, noted as they change.
    pub growth: Growth,
}

/// A name search over one volume.
/// A match's place in a ranked search (`Tree::rank`): lower is better.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rank {
    /// How the name matches what was typed, and where it is.
    pub score: u32,
    /// The path's length: shorter first, among names as good.
    pub len: u32,
    pub id: u32,
}

/// Windows' own folders and caches, by the folders a match is in (from the drive down, case ignored):
/// what a search for your files rarely means.
fn in_system_folder(folders: &[&[u8]]) -> bool {
    const WINDOWS: [&[u8]; 9] = [b"winsxs", b"servicing", b"assembly", b"installer", b"system32", b"syswow64", b"softwaredistribution", b"systemapps", b"immersivecontrolpanel"];
    let is = |folder: Option<&&[u8]>, names: &[&[u8]]| folder.is_some_and(|f| names.iter().any(|n| f.eq_ignore_ascii_case(n)));
    (0..folders.len()).any(|i| {
        is(folders.get(i), &[b"programdata", b"$recycle.bin", b"node_modules", b".git"])
            || (is(folders.get(i), &[b"windows"]) && is(folders.get(i + 1), &WINDOWS))
            || (is(folders.get(i), &[b"appdata"]) && is(folders.get(i + 1), &[b"local"]) && is(folders.get(i + 2), &[b"temp", b"packages", b"microsoft"]))
    })
}

pub struct Search<'a> {
    pub matcher: &'a NameMatcher,
    /// ext:, size:, dm:, file: and folder:, with `now` (a FILETIME) the time ages count from.
    pub filters: &'a Filters,
    pub now: i64,
    /// The folder to search below; ROOT for the whole volume.
    pub scope: u32,
    /// Include hidden files and folders (and everything inside hidden folders).
    pub hidden: bool,
    pub excludes: &'a Excludes,
}

/// One item of a disk-usage map (see `Tree::usage`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsageItem {
    /// The item's name; NONE for the folder the map is of.
    pub link: u32,
    /// Where the folder it's in is in the map; NONE for the folder the map is of.
    pub parent: u32,
    /// Files: their size. Folders: everything below them.
    pub size: u64,
    pub dir: bool,
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeStats {
    pub files: u64,
    pub folders: u64,
    pub names: u64,
    pub memory: u64,
}

impl Tree {
    /// Rebuilds the lookups from saved arrays (see `snapshot`).
    pub(crate) fn from_parts(recs: Vec<Rec>, links: Vec<Link>, names: Vec<u8>, next_names: IdMap) -> Tree {
        let mut tree = Tree { recs, links, names, next_names, ..Default::default() };
        // Links share names (see `compact`): each name counts once, by where it starts.
        let mut counted = vec![0u64; tree.names.len() / 64 + 1];
        let mut live = 0;
        let mut folders = Vec::new();
        for (id, link) in tree.links.iter().enumerate() {
            if link.flags & LINK_USED == 0 {
                tree.free.push(id as u32);
                continue;
            }
            let (word, bit) = (link.name as usize / 64, 1u64 << (link.name % 64));
            if counted[word] & bit == 0 {
                counted[word] |= bit;
                live += Tree::stored_len(&tree.names, link);
            }
            if tree.recs[link.rec as usize].dir() && link.parent != NONE {
                folders.push((link.parent, id as u32));
            }
        }
        tree.garbage = tree.names.len().saturating_sub(live);
        tree.kids = Kids::from_pairs(folders);
        tree
    }

    pub fn rec(&self, n: u32) -> Option<&Rec> {
        self.recs.get(n as usize)
    }

    pub fn link(&self, id: u32) -> &Link {
        &self.links[id as usize]
    }

    pub fn record_count(&self) -> usize {
        self.recs.len()
    }

    pub fn name_bytes(&self, id: u32) -> &[u8] {
        let link = &self.links[id as usize];
        &self.names[link.name as usize..link.name as usize + link.len as usize]
    }

    pub fn name(&self, id: u32) -> &str {
        std::str::from_utf8(self.name_bytes(id)).unwrap_or_default()
    }

    /// The lowercase name, kept only for names that aren't ASCII.
    pub fn folded(&self, id: u32) -> Option<&[u8]> {
        Tree::folded_in(&self.names, &self.links[id as usize])
    }

    fn folded_in<'n>(names: &'n [u8], link: &Link) -> Option<&'n [u8]> {
        if link.flags & LINK_FOLDED == 0 {
            return None;
        }
        let at = link.name as usize + link.len as usize;
        let len = u16::from_le_bytes([names[at], names[at + 1]]) as usize;
        Some(&names[at + 2..at + 2 + len])
    }

    /// Arena bytes a link's name takes, its folded copy included.
    fn stored_len(names: &[u8], link: &Link) -> usize {
        link.len as usize + Tree::folded_in(names, link).map_or(0, |f| 2 + f.len())
    }

    /// The record's names, as links.
    pub fn links_of(&self, n: u32) -> impl Iterator<Item = u32> + '_ {
        let mut next = self.recs.get(n as usize).map_or(NONE, |r| r.first_link);
        std::iter::from_fn(move || {
            let id = next;
            (id != NONE).then(|| {
                next = self.next_name(id);
                id
            })
        })
    }

    /// The record's name after link `id`, NONE after its last.
    pub fn next_name(&self, id: u32) -> u32 {
        self.next_names.get(&id).copied().unwrap_or(NONE)
    }

    fn set_next_name(&mut self, id: u32, next: u32) {
        if next == NONE {
            self.next_names.remove(&id);
        } else {
            self.next_names.insert(id, next);
        }
    }

    /// A folder's parent folder (NONE for the root or a record with no name).
    pub fn parent(&self, n: u32) -> u32 {
        match self.recs.get(n as usize) {
            Some(r) if n != ROOT && r.first_link != NONE => self.links[r.first_link as usize].parent,
            _ => NONE,
        }
    }

    fn ensure(&mut self, n: u32) {
        if self.recs.len() <= n as usize {
            let missing = n as usize + 1 - self.recs.len();
            make_room(&mut self.recs, missing);
            self.recs.resize(n as usize + 1, Rec::default());
        }
    }

    fn push_name(&mut self, name: &str) -> (u32, u16, u16) {
        let at = self.names.len() as u32;
        // NTFS names are at most 255 UTF-16 units, so 765 bytes of UTF-8.
        let name = &name[..floor_char_boundary(name, u16::MAX as usize / 4)];
        make_room(&mut self.names, 2 + 2 * name.len());
        self.names.extend_from_slice(name.as_bytes());
        let mut flags = LINK_USED;
        if !name.is_ascii() {
            let folded = fold(name);
            let folded = &folded[..floor_char_boundary(&folded, u16::MAX as usize / 4)];
            self.names.extend_from_slice(&(folded.len() as u16).to_le_bytes());
            self.names.extend_from_slice(folded.as_bytes());
            flags |= LINK_FOLDED;
        }
        (at, name.len() as u16, flags)
    }

    /// The parent record a name points at, or NONE when that record now holds another file.
    fn parent_number(&self, parent: u64) -> u32 {
        let (number, seq) = split_ref(parent);
        if number >= NONE as u64 {
            return NONE;
        }
        match self.recs.get(number as usize) {
            Some(r) if r.used() && seq != 0 && r.seq() != seq => NONE,
            _ => number as u32,
        }
    }

    fn add_link(&mut self, rec: u32, parent: u32, name: &str) {
        let (at, len, flags) = self.push_name(name);
        let link = Link { rec, parent, name: at, len, flags };
        let id = match self.free.pop() {
            Some(id) => {
                self.links[id as usize] = link;
                id
            }
            None => {
                make_room(&mut self.links, 1);
                self.links.push(link);
                (self.links.len() - 1) as u32
            }
        };
        // The new name goes first.
        let r = self.recs[rec as usize];
        self.set_next_name(id, r.first_link);
        self.recs[rec as usize].first_link = id;
        if r.dir() && parent != NONE {
            self.kids.add(parent, id);
        }
        if r.size() != 0 {
            self.add_up(parent, r.size() as i64);
        }
    }

    fn remove_link(&mut self, id: u32) {
        let link = self.links[id as usize];
        let r = self.recs[link.rec as usize];
        let after = self.next_name(id);
        self.next_names.remove(&id);
        if r.first_link == id {
            self.recs[link.rec as usize].first_link = after;
        } else {
            let mut at = r.first_link;
            while at != NONE {
                let next = self.next_name(at);
                if next == id {
                    self.set_next_name(at, after);
                    break;
                }
                at = next;
            }
        }
        if r.dir() {
            self.kids.remove(link.parent, id);
        }
        if r.size() != 0 {
            self.add_up(link.parent, -(r.size() as i64));
        }
        // Counted as gone even when other links share the name: compacting finds out.
        self.garbage += Tree::stored_len(&self.names, &link);
        self.links[id as usize] = Link { rec: NONE, parent: NONE, name: 0, len: 0, flags: 0 };
        self.free.push(id);
    }

    /// Adds `delta` bytes to `dir` and every folder above it.
    fn add_up(&mut self, mut dir: u32, delta: i64) {
        for _ in 0..MAX_DEPTH {
            let Some(rec) = self.recs.get_mut(dir as usize) else { return };
            // A record not in use yet may be a folder still to come; it keeps what's below it.
            if rec.used() && !rec.dir() {
                return;
            }
            if rec.used() {
                self.growth.note(dir, rec.seq(), rec.size());
            }
            rec.set_size(rec.size().saturating_add_signed(delta));
            if dir == ROOT || rec.first_link == NONE {
                return;
            }
            dir = self.links[rec.first_link as usize].parent;
        }
    }

    /// Makes record `n` what `state` says, keeping folder totals right: the change for a new file,
    /// a deleted or renamed name, a move, or a size change.
    pub fn set_record(&mut self, n: u32, state: &RecordState) {
        if n < FIRST_USER_RECORD && n != ROOT {
            return;
        }
        self.ensure(n);
        let old = self.recs[n as usize];
        if old.used() && (old.seq() != state.seq || old.dir() != state.dir) {
            self.remove_record(n);
        }
        let used = self.recs[n as usize].used();
        // The size first: names kept get the difference, names added below the new size.
        if !state.dir {
            let delta = state.size as i64 - if used { self.recs[n as usize].size() as i64 } else { 0 };
            self.recs[n as usize].set_size(state.size);
            if used && delta != 0 {
                let parents: Vec<u32> = self.links_of(n).map(|id| self.links[id as usize].parent).collect();
                for parent in parents {
                    self.add_up(parent, delta);
                }
            }
        }
        // (A folder new to the index starts with whatever its contents added up to already.)
        let rec = &mut self.recs[n as usize];
        rec.set_seq(state.seq);
        rec.modified = state.modified;
        rec.set_attrs(state.attrs | if state.dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 });
        rec.set_flags(REC_USED | if state.dir { REC_DIR } else { 0 });
        if n == ROOT {
            return;
        }
        // A folder that's new wasn't there at the last mark.
        if state.dir && !used {
            self.growth.note(n, state.seq, ABSENT);
        }
        let mut wanted: Vec<(u32, &str)> =
            state.names.iter().map(|(parent, name)| (self.parent_number(*parent), name.as_str())).filter(|&(p, _)| p != n).collect();
        let current: Vec<u32> = self.links_of(n).collect();
        for id in current {
            let link = self.links[id as usize];
            match wanted.iter().position(|&(p, name)| p == link.parent && name.as_bytes() == self.name_bytes(id)) {
                Some(i) => {
                    wanted.swap_remove(i);
                }
                None => self.remove_link(id),
            }
        }
        for (parent, name) in wanted {
            self.add_link(n, parent, name);
        }
    }

    /// Drops a deleted file or folder.
    pub fn remove_record(&mut self, n: u32) {
        if !self.recs.get(n as usize).is_some_and(Rec::used) {
            return;
        }
        let links: Vec<u32> = self.links_of(n).collect();
        for id in links {
            self.remove_link(id);
        }
        self.recs[n as usize] = Rec::default();
    }

    /// Recounts every folder's total from scratch: after a full scan, and now and then to undo any
    /// drift from changes the journal described incompletely.
    pub fn recompute_totals(&mut self) {
        for rec in &mut self.recs {
            if rec.dir() || !rec.used() {
                rec.set_size(0);
            }
        }
        // Each folder first gets the files right in it…
        for i in 0..self.links.len() {
            let link = self.links[i];
            if link.flags & LINK_USED == 0 || link.parent == NONE {
                continue;
            }
            let rec = self.recs[link.rec as usize];
            if rec.dir() || rec.size() == 0 {
                continue;
            }
            if let Some(parent) = self.recs.get_mut(link.parent as usize) {
                if parent.dir() {
                    parent.set_size(parent.size() + rec.size());
                }
            }
        }
        // …then hands its total to its parent, deepest folders first.
        let depths = self.depths();
        let mut order: Vec<u32> = (0..self.recs.len() as u32).filter(|&n| depths[n as usize] != u32::MAX && n != ROOT).collect();
        order.sort_unstable_by_key(|&n| std::cmp::Reverse(depths[n as usize]));
        for n in order {
            let parent = self.parent(n);
            let size = self.recs[n as usize].size();
            if parent != NONE && size != 0 {
                let up = &mut self.recs[parent as usize];
                up.set_size(up.size() + size);
            }
        }
    }

    /// Depth below the root of every folder that's reachable from it (u32::MAX for the rest).
    fn depths(&self) -> Vec<u32> {
        const UNKNOWN: u32 = u32::MAX - 1;
        let mut depths = vec![u32::MAX; self.recs.len()];
        for (n, rec) in self.recs.iter().enumerate() {
            if rec.dir() {
                depths[n] = UNKNOWN;
            }
        }
        if let Some(root) = depths.get_mut(ROOT as usize) {
            *root = 0;
        }
        let mut chain = Vec::new();
        for n in 0..self.recs.len() as u32 {
            if depths[n as usize] != UNKNOWN {
                continue;
            }
            chain.clear();
            let mut at = n;
            // Climb until a folder whose depth is known; anything else leaves the chain unreachable.
            let base = loop {
                match depths.get(at as usize) {
                    Some(&UNKNOWN) if chain.len() < MAX_DEPTH => {
                        chain.push(at);
                        depths[at as usize] = u32::MAX; // visited: a cycle ends here
                        at = self.parent(at);
                    }
                    Some(&d) if d != u32::MAX && d != UNKNOWN => break Some(d),
                    _ => break None,
                }
            };
            for (i, &dir) in chain.iter().rev().enumerate() {
                depths[dir as usize] = base.map_or(u32::MAX, |d| d + 1 + i as u32);
            }
        }
        depths
    }

    /// Path of folder `dir` below the root ("" for the root), if it hangs off the root.
    pub fn dir_path<'c>(&self, dir: u32, cache: &'c mut HashMap<u32, String>) -> Option<&'c str> {
        if !cache.contains_key(&dir) {
            let mut chain = Vec::new();
            let mut at = dir;
            let mut path = loop {
                if let Some(path) = cache.get(&at) {
                    break path.clone();
                }
                if at == ROOT {
                    break String::new();
                }
                let rec = self.recs.get(at as usize)?;
                if !rec.used() || rec.first_link == NONE || chain.len() >= MAX_DEPTH {
                    return None;
                }
                chain.push(rec.first_link);
                at = self.links[rec.first_link as usize].parent;
            };
            for &id in chain.iter().rev() {
                if !path.is_empty() {
                    path.push('\\');
                }
                path.push_str(self.name(id));
                cache.insert(self.links[id as usize].rec, path.clone());
            }
            cache.entry(dir).or_insert(path);
        }
        cache.get(&dir).map(String::as_str)
    }

    /// Path of a name below the root, e.g. `Users\me\notes.txt`.
    pub fn path(&self, id: u32, cache: &mut HashMap<u32, String>) -> Option<String> {
        let link = self.links[id as usize];
        let dir = self.dir_path(link.parent, cache)?;
        let mut path = String::with_capacity(dir.len() + 1 + link.len as usize);
        path.push_str(dir);
        if !path.is_empty() {
            path.push('\\');
        }
        path.push_str(self.name(id));
        Some(path)
    }

    fn name_is(&self, id: u32, lower: &str) -> bool {
        match self.folded(id) {
            Some(folded) => folded == lower.as_bytes(),
            None => self.name_bytes(id).eq_ignore_ascii_case(lower.as_bytes()),
        }
    }

    /// The folder at `relative` (below the root, any case, `\` or `/`), if the index has it.
    pub fn resolve(&self, relative: &str) -> Option<u32> {
        let mut dir = ROOT;
        for part in relative.split(['\\', '/']).filter(|p| !p.is_empty()) {
            let lower = fold(part);
            dir = self.links[*self.kids.of(dir).iter().find(|&&id| self.name_is(id, &lower))? as usize].rec;
        }
        Some(dir)
    }

    /// The folders right inside `dir`, with the total size of each.
    pub fn child_dirs(&self, dir: u32) -> Vec<(String, u64)> {
        self.kids.of(dir).iter().map(|&id| (self.name(id).to_string(), self.recs[self.links[id as usize].rec as usize].size())).collect()
    }

    /// Every name right inside folder `dir`, files and folders, empty ones too, as a listing of the
    /// folder shows them (NTFS's own records left out): for a folder Windows keeps a user out of.
    pub fn list(&self, dir: u32) -> Vec<u32> {
        let folders = self.kids.of(dir);
        // Files aren't listed by folder: one pass over every name finds them.
        const CHUNK: usize = 1 << 14;
        let files: Vec<Vec<u32>> = self
            .links
            .par_chunks(CHUNK)
            .enumerate()
            .map(|(chunk, links)| {
                let inside = |link: &Link| link.flags & LINK_USED != 0 && link.parent == dir && link.rec >= FIRST_USER_RECORD && !self.recs[link.rec as usize].dir();
                links.iter().enumerate().filter(|(_, link)| inside(link)).map(|(i, _)| (chunk * CHUNK + i) as u32).collect()
            })
            .collect();
        folders.iter().copied().filter(|&id| self.links[id as usize].rec >= FIRST_USER_RECORD).chain(files.concat()).collect()
    }

    /// A disk-usage map of folder `dir`: the files and folders in it and in its folders, down to
    /// `depth` levels, without empty ones. Only the `max` biggest are kept, with the folders they're
    /// in; the rest still count in their folders' sizes, and the second value says whether any were
    /// left out. The first item is `dir` itself; then each folder's items follow together, biggest
    /// first, and every folder comes before what's in it.
    pub fn usage(&self, dir: u32, depth: u32, max: usize) -> (Vec<UsageItem>, bool) {
        let Some(top) = self.rec(dir).filter(|r| r.used() && r.dir()) else { return (Vec::new(), false) };
        let mut items = vec![UsageItem { link: NONE, parent: NONE, size: top.size(), dir: true }];
        if max == 0 {
            return (items, top.size() != 0);
        }
        // Folders biggest first, from the folder lists. Once `max` items are known, anything smaller
        // than the smallest of the biggest `max` so far (`floor`) can't make the map, and neither can
        // anything inside it: neither is looked at, so deep maps cost little more than shallow ones.
        // `open` maps every folder whose files are wanted to its place in `items`.
        let mut biggest: BinaryHeap<Reverse<u64>> = BinaryHeap::with_capacity(max + 1);
        let mut floor = 0;
        let mut left_out = false;
        let mut open: HashMap<u32, u32> = HashMap::new();
        let mut frontier = BinaryHeap::from([(top.size(), dir, 0u32, 0u32)]);
        while let Some((size, folder, at, level)) = frontier.pop() {
            if size < floor {
                left_out = true;
                continue;
            }
            // A folder seen twice is a cycle, which only corruption makes.
            if open.insert(folder, at).is_some() {
                continue;
            }
            for &id in self.kids.of(folder) {
                let rec = self.links[id as usize].rec;
                let size = self.recs[rec as usize].size();
                if size == 0 {
                    continue;
                }
                if size < floor {
                    left_out = true;
                    continue;
                }
                items.push(UsageItem { link: id, parent: at, size, dir: true });
                biggest.push(Reverse(size));
                if biggest.len() > max {
                    biggest.pop();
                }
                if biggest.len() == max {
                    floor = biggest.peek().map_or(0, |s| s.0);
                }
                if level + 1 < depth {
                    frontier.push((size, rec, (items.len() - 1) as u32, level + 1));
                }
            }
        }
        // Files aren't listed by folder: one pass over every name finds those in the open folders.
        let mut wanted = vec![0u64; self.recs.len().div_ceil(64)];
        for &folder in open.keys() {
            wanted[folder as usize / 64] |= 1 << (folder % 64);
        }
        const CHUNK: usize = 1 << 14;
        let files: Vec<(Vec<u32>, bool)> = self
            .links
            .par_chunks(CHUNK)
            .enumerate()
            .map(|(chunk, links)| {
                let (mut found, mut smaller) = (Vec::new(), false);
                for (i, link) in links.iter().enumerate() {
                    let parent = link.parent as usize;
                    if link.flags & LINK_USED == 0 || parent >= self.recs.len() || wanted[parent / 64] & (1 << (parent % 64)) == 0 {
                        continue;
                    }
                    let rec = &self.recs[link.rec as usize];
                    if link.rec < FIRST_USER_RECORD || rec.dir() || rec.size() == 0 {
                        continue;
                    }
                    if rec.size() < floor {
                        smaller = true;
                    } else {
                        found.push((chunk * CHUNK + i) as u32);
                    }
                }
                (found, smaller)
            })
            .collect();
        for (found, smaller) in files {
            left_out |= smaller;
            for id in found {
                let link = &self.links[id as usize];
                items.push(UsageItem { link: id, parent: open[&link.parent], size: self.recs[link.rec as usize].size(), dir: false });
            }
        }

        // The biggest `max`, and the folders they're in (a folder is never smaller than its items,
        // but sizes can tie).
        let mut keep = vec![true; items.len()];
        let limited = left_out || items.len() - 1 > max;
        if items.len() - 1 > max {
            let mut order: Vec<u32> = (1..items.len() as u32).collect();
            order.select_nth_unstable_by(max, |&a, &b| items[b as usize].size.cmp(&items[a as usize].size).then(a.cmp(&b)));
            keep.fill(false);
            keep[0] = true;
            for &i in &order[..max] {
                keep[i as usize] = true;
            }
            for &i in &order[..max] {
                let mut up = items[i as usize].parent;
                while up != NONE && !keep[up as usize] {
                    keep[up as usize] = true;
                    up = items[up as usize].parent;
                }
            }
        }

        // Each folder's items together, biggest first (then by name), breadth first.
        let mut start = vec![0u32; items.len() + 1];
        for (i, item) in items.iter().enumerate().skip(1) {
            if keep[i] {
                start[item.parent as usize + 1] += 1;
            }
        }
        for i in 0..items.len() {
            start[i + 1] += start[i];
        }
        let mut fill = start.clone();
        let mut inside = vec![0u32; start[items.len()] as usize];
        for (i, item) in items.iter().enumerate().skip(1) {
            if keep[i] {
                let slot = &mut fill[item.parent as usize];
                inside[*slot as usize] = i as u32;
                *slot += 1;
            }
        }
        let mut map = Vec::with_capacity(inside.len() + 1);
        map.push(items[0]);
        let mut queue = vec![0u32];
        let mut at = 0;
        while at < queue.len() {
            let folder = queue[at] as usize;
            let range = &mut inside[start[folder] as usize..start[folder + 1] as usize];
            range.sort_unstable_by(|&a, &b| {
                let (a, b) = (&items[a as usize], &items[b as usize]);
                b.size.cmp(&a.size).then_with(|| self.name_bytes(a.link).cmp(self.name_bytes(b.link)))
            });
            for &i in range.iter() {
                map.push(UsageItem { parent: at as u32, ..items[i as usize] });
                queue.push(i);
            }
            at += 1;
        }
        (map, limited)
    }

    /// Every name below `search.scope` that matches, in index order; close matches (a matcher with
    /// typos allowed) the closest first.
    pub fn search(&self, search: &Search) -> Vec<u32> {
        const CHUNK: usize = 1 << 14;
        let found: Vec<Vec<(u32, u32)>> = self
            .links
            .par_chunks(CHUNK)
            .enumerate()
            .map(|(chunk, links)| {
                let mut found = Vec::new();
                for (i, link) in links.iter().enumerate() {
                    if link.flags & LINK_USED == 0 || link.rec < FIRST_USER_RECORD {
                        continue;
                    }
                    let name = &self.names[link.name as usize..link.name as usize + link.len as usize];
                    let folded = Tree::folded_in(&self.names, link);
                    let Some(edits) = search.matcher.distance(name, folded) else { continue };
                    if !search.filters.is_empty() {
                        let rec = &self.recs[link.rec as usize];
                        if !search.filters.accepts(name, folded, rec.dir(), rec.size(), (search.now - rec.modified) / 10_000_000) {
                            continue;
                        }
                    }
                    let id = (chunk * CHUNK + i) as u32;
                    if !search.hidden && is_hidden(self.recs[link.rec as usize].attrs(), name) {
                        continue;
                    }
                    if search.excludes.name(name, folded) || !self.reachable(link.parent, search) {
                        continue;
                    }
                    if search.excludes.has_paths() && self.excluded_path(id, search) {
                        continue;
                    }
                    found.push((edits, id));
                }
                found
            })
            .collect();
        let mut found = found.concat();
        if search.matcher.is_near() {
            // Stable: index order among names as close.
            found.sort_by_key(|&(edits, _)| edits);
        }
        found.into_iter().map(|(_, id)| id).collect()
    }

    /// The best `max` of `found` (what `search` matched), best first, as quick search ranks them: a name
    /// that is the first word typed (`word`, folded) or whose stem is, then one that starts with it, then
    /// one with a word that does, then any other, then close matches by their typos; each a little
    /// behind when it's in Windows' own folders or a cache, and when it's hidden; then the shorter path.
    pub fn rank(&self, found: &[u32], search: &Search, word: &[u8], max: usize) -> Vec<Rank> {
        let mut ranked: Vec<Rank> = found.par_iter().map(|&id| self.rank_of(id, search, word)).collect();
        if ranked.len() > max {
            ranked.select_nth_unstable(max);
            ranked.truncate(max);
        }
        ranked.sort_unstable();
        ranked
    }

    fn rank_of(&self, id: u32, search: &Search, word: &[u8]) -> Rank {
        let link = &self.links[id as usize];
        let name = &self.names[link.name as usize..link.name as usize + link.len as usize];
        // Only names that fold differently have a folded copy: an ASCII name is compared ignoring case.
        let folded = Tree::folded_in(&self.names, link);
        let lower = folded.unwrap_or(name);
        let is = |part: &[u8]| part.eq_ignore_ascii_case(word);
        let class = if word.is_empty() || is(lower) || lower.iter().rposition(|&b| b == b'.').is_some_and(|dot| is(&lower[..dot])) {
            0
        } else if lower.get(..word.len()).is_some_and(is) {
            1
        } else if lower.windows(word.len() + 1).any(|w| matches!(w[0], b' ' | b'.' | b'_' | b'-') && is(&w[1..])) {
            2
        } else if lower.windows(word.len()).any(is) {
            3
        } else {
            // A close match, not containing what was typed: behind every name that does.
            4 + search.matcher.distance(name, folded).unwrap_or(3)
        };
        // Its folders from the drive down, for where it is and how long its path is ("C:\" first).
        let mut folders = Vec::with_capacity(12);
        let mut len = link.len as u32 + 3;
        let mut dir = link.parent;
        for _ in 0..MAX_DEPTH {
            let Some(rec) = self.recs.get(dir as usize).filter(|r| dir != ROOT && r.first_link != NONE) else { break };
            let up = &self.links[rec.first_link as usize];
            let up_name = &self.names[up.name as usize..up.name as usize + up.len as usize];
            folders.push(Tree::folded_in(&self.names, up).unwrap_or(up_name));
            len += up.len as u32 + 1;
            dir = up.parent;
        }
        folders.reverse();
        let hidden = is_hidden(self.recs[link.rec as usize].attrs(), name);
        Rank { score: class * 4 + if in_system_folder(&folders) { 2 } else { 0 } + u32::from(hidden), len, id }
    }

    /// Whether folder `dir` is the scope or below it, with nothing hidden (unless wanted) or excluded
    /// on the way; the scope itself is never filtered out, as fd searches any folder it's given.
    fn reachable(&self, mut dir: u32, search: &Search) -> bool {
        for _ in 0..MAX_DEPTH {
            if dir == search.scope {
                return true;
            }
            let Some(rec) = self.recs.get(dir as usize) else { return false };
            if dir == ROOT || !rec.used() || rec.first_link == NONE {
                return false;
            }
            let link = &self.links[rec.first_link as usize];
            let name = &self.names[link.name as usize..link.name as usize + link.len as usize];
            if (!search.hidden && is_hidden(rec.attrs(), name)) || search.excludes.name(name, Tree::folded_in(&self.names, link)) {
                return false;
            }
            dir = link.parent;
        }
        false
    }

    /// Whether a path exclusion (one with a separator) covers the name or a folder it's in.
    fn excluded_path(&self, id: u32, search: &Search) -> bool {
        let mut parts = vec![id];
        let mut dir = self.links[id as usize].parent;
        while dir != search.scope && dir != NONE && parts.len() < MAX_DEPTH {
            let Some(rec) = self.recs.get(dir as usize).filter(|r| r.first_link != NONE) else { return false };
            parts.push(rec.first_link);
            dir = self.links[rec.first_link as usize].parent;
        }
        let mut relative = String::new();
        parts.iter().rev().any(|&part| {
            if !relative.is_empty() {
                relative.push('\\');
            }
            relative.push_str(self.name(part));
            search.excludes.path(&relative)
        })
    }

    /// Rewrites the name arena when most of it is names that are gone.
    pub fn maybe_compact(&mut self) {
        if self.garbage >= 1 << 20 && self.garbage >= self.names.len() / 2 {
            self.compact();
        }
    }

    /// Rewrites the name arena with only the names in use, and each of them once: links with the same
    /// name share it (a system drive has about three names for every different one).
    pub fn compact(&mut self) {
        let mut names = Vec::with_capacity(self.names.len().saturating_sub(self.garbage));
        // The first link with each different name (its name in `names` already), by the name's hash:
        // open addressing, kept at most three quarters full.
        let mut firsts = vec![NONE; 1 << 12];
        let mut different = 0;
        for id in 0..self.links.len() {
            let link = self.links[id];
            if link.flags & LINK_USED == 0 {
                continue;
            }
            let stored = &self.names[link.name as usize..link.name as usize + Tree::stored_len(&self.names, &link)];
            let mut slot = slot_of(stored, firsts.len());
            loop {
                let first = firsts[slot];
                if first == NONE {
                    firsts[slot] = id as u32;
                    self.links[id].name = names.len() as u32;
                    names.extend_from_slice(stored);
                    different += 1;
                    break;
                }
                // The same name has the same folded copy after it.
                let other = self.links[first as usize];
                let len = link.len as usize;
                if other.len == link.len && other.flags == link.flags && names[other.name as usize..other.name as usize + len] == stored[..len] {
                    self.links[id].name = other.name;
                    break;
                }
                slot = (slot + 1) & (firsts.len() - 1);
            }
            if different * 4 > firsts.len() * 3 {
                let mut bigger = vec![NONE; firsts.len() * 2];
                for &first in firsts.iter().filter(|&&f| f != NONE) {
                    let other = &self.links[first as usize];
                    let mut slot = slot_of(&names[other.name as usize..other.name as usize + Tree::stored_len(&names, other)], bigger.len());
                    while bigger[slot] != NONE {
                        slot = (slot + 1) & (bigger.len() - 1);
                    }
                    bigger[slot] = first;
                }
                firsts = bigger;
            }
        }
        // A little room for what changes next.
        names.shrink_to(names.len() + names.len() / 16);
        self.names = names;
        self.garbage = 0;
    }

    pub fn stats(&self) -> TreeStats {
        let mut stats = TreeStats::default();
        // Records with no name (NTFS's reserved ones) aren't files anyone sees.
        for rec in self.recs.iter().filter(|r| r.used() && r.first_link != NONE) {
            if rec.dir() {
                stats.folders += 1;
            } else {
                stats.files += 1;
            }
        }
        stats.names = (self.links.len() - self.free.len()) as u64;
        stats.memory = (self.recs.capacity() * size_of::<Rec>()
            + self.links.capacity() * size_of::<Link>()
            + self.names.capacity()
            + self.free.capacity() * 4
            + self.next_names.capacity() * 10
            + self.kids.memory()) as u64
            + self.growth.memory();
        stats
    }
}

/// Each folder's child folders, as links: every list in one array (its length, its room, then its
/// links), so a folder costs no allocation of its own. A list that outgrows its room moves to the end
/// with twice the room, and what lists leave behind is given back once it's most of the array.
#[derive(Default)]
struct Kids {
    /// Where each folder's list starts in `lists`.
    at: IdMap,
    lists: Vec<u32>,
    /// Slots of `lists` no list uses any more.
    garbage: usize,
}

impl Kids {
    /// The lists from (folder, child folder's link) pairs; each list in link order.
    fn from_pairs(mut pairs: Vec<(u32, u32)>) -> Kids {
        pairs.sort_unstable();
        let folders = pairs.chunk_by(|a, b| a.0 == b.0).count();
        let mut kids = Kids::default();
        kids.at.reserve(folders);
        kids.lists.reserve_exact(pairs.len() + 2 * folders);
        for list in pairs.chunk_by(|a, b| a.0 == b.0) {
            kids.at.insert(list[0].0, kids.lists.len() as u32);
            kids.lists.extend([list.len() as u32; 2]);
            kids.lists.extend(list.iter().map(|&(_, link)| link));
        }
        kids
    }

    fn of(&self, dir: u32) -> &[u32] {
        self.at.get(&dir).map_or(&[], |&at| {
            let at = at as usize;
            &self.lists[at + 2..at + 2 + self.lists[at] as usize]
        })
    }

    fn add(&mut self, dir: u32, link: u32) {
        let (mut at, len, room) = match self.at.get(&dir) {
            Some(&at) => (at as usize, self.lists[at as usize] as usize, self.lists[at as usize + 1] as usize),
            None => (usize::MAX, 0, 0),
        };
        if len == room {
            let room = (room * 2).max(1);
            make_room(&mut self.lists, 2 + room);
            let moved = self.lists.len();
            if at == usize::MAX {
                self.lists.extend([0, room as u32]);
            } else {
                self.lists.extend_from_within(at..at + 2 + len);
                self.lists[moved + 1] = room as u32;
                self.garbage += 2 + len;
            }
            self.lists.resize(moved + 2 + room, NONE);
            self.at.insert(dir, moved as u32);
            at = moved;
        }
        self.lists[at + 2 + len] = link;
        self.lists[at] += 1;
        self.maybe_compact();
    }

    fn remove(&mut self, dir: u32, link: u32) {
        let Some(&at) = self.at.get(&dir) else { return };
        let at = at as usize;
        let len = self.lists[at] as usize;
        let Some(i) = self.lists[at + 2..at + 2 + len].iter().position(|&l| l == link) else { return };
        // The rest keep their order.
        self.lists.copy_within(at + 3 + i..at + 2 + len, at + 2 + i);
        self.lists[at] -= 1;
        if len == 1 {
            self.garbage += 2 + self.lists[at + 1] as usize;
            self.at.remove(&dir);
        }
        self.maybe_compact();
    }

    fn maybe_compact(&mut self) {
        if self.garbage >= 1 << 16 && self.garbage >= self.lists.len() / 2 {
            let mut lists = Vec::with_capacity(self.lists.len() - self.garbage);
            for at in self.at.values_mut() {
                let (start, len) = (*at as usize, self.lists[*at as usize] as usize);
                *at = lists.len() as u32;
                lists.extend([len as u32; 2]);
                lists.extend_from_slice(&self.lists[start + 2..start + 2 + len]);
            }
            self.lists = lists;
            self.garbage = 0;
        }
    }

    /// Bytes they take: the table's slots and control bytes, and the array.
    fn memory(&self) -> usize {
        self.at.capacity() * 10 + self.lists.capacity() * 4
    }
}

/// Collects a whole volume's records as the MFT scan finds them, in any order, then links names to
/// their folders once every record is known.
pub struct Builder {
    tree: Tree,
    /// (record, parent reference, name start, length, flags) per name, linked in `finish`.
    pending: Vec<(u32, u64, u32, u16, u16)>,
}

impl Builder {
    pub fn new(records: usize) -> Self {
        // About 25 bytes of name per record on a typical system drive.
        let mut recs = vec![Rec::default(); records];
        // The root is there even if its record can't be read, so the drive's top folders keep their
        // parent: record 5, sequence 5 on every NTFS volume.
        if let Some(root) = recs.get_mut(ROOT as usize) {
            root.set_seq(ROOT as u16);
            root.set_attrs(FILE_ATTRIBUTE_DIRECTORY);
            root.set_flags(REC_USED | REC_DIR);
        }
        let tree = Tree { recs, names: Vec::with_capacity(records * 25), ..Default::default() };
        Builder { tree, pending: Vec::with_capacity(records) }
    }

    /// Record `n`, if it's one of the records scanned: an extension record naming a base record
    /// beyond them (a damaged disk) must not make room for billions of records.
    fn slot(&mut self, n: u32) -> Option<&mut Rec> {
        if n < FIRST_USER_RECORD && n != ROOT {
            return None;
        }
        self.tree.recs.get_mut(n as usize)
    }

    /// A base record in use.
    pub fn record(&mut self, n: u32, seq: u16, dir: bool, attrs: u32, modified: i64) {
        if let Some(rec) = self.slot(n) {
            rec.set_seq(seq);
            rec.modified = modified;
            rec.set_attrs(attrs | if dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 });
            rec.set_flags(REC_USED | if dir { REC_DIR } else { 0 });
        }
    }

    /// The unnamed data stream's size, from the base record or one of its extension records.
    pub fn size(&mut self, n: u32, size: u64) {
        if let Some(rec) = self.slot(n) {
            rec.set_size(size);
        }
    }

    /// A name of record `n` (from the base record or an extension record).
    pub fn name(&mut self, n: u32, parent: u64, name: &str) {
        if n == ROOT || self.slot(n).is_none() {
            return;
        }
        let (at, len, flags) = self.tree.push_name(name);
        self.pending.push((n, parent, at, len, flags));
    }

    pub fn finish(mut self) -> Tree {
        let mut tree = std::mem::take(&mut self.tree);
        for (i, rec) in tree.recs.iter_mut().enumerate() {
            if !rec.used() {
                *rec = Rec::default();
            } else if rec.dir() || i as u32 == ROOT {
                rec.set_size(0);
            }
        }
        tree.links.reserve(self.pending.len());
        let mut folders = Vec::new();
        for &(n, parent, at, len, flags) in &self.pending {
            let rec = tree.recs[n as usize];
            let parent = tree.parent_number(parent);
            if !rec.used() || parent == n {
                continue;
            }
            let id = tree.links.len() as u32;
            tree.links.push(Link { rec: n, parent, name: at, len, flags });
            if rec.first_link != NONE {
                tree.next_names.insert(id, rec.first_link);
            }
            tree.recs[n as usize].first_link = id;
            if rec.dir() && parent != NONE {
                folders.push((parent, id));
            }
        }
        tree.kids = Kids::from_pairs(folders);
        tree.recompute_totals();
        // Each name once; and a little room kept for what changes next, the rest given back.
        tree.compact();
        let links = tree.links.len();
        tree.links.shrink_to(links + links / 16);
        tree
    }
}

/// Makes room for `extra` more items, growing by an eighth instead of doubling: these arrays run to
/// hundreds of megabytes and live as long as the service, so doubling would waste most of a copy.
fn make_room<T>(items: &mut Vec<T>, extra: usize) {
    if items.capacity() - items.len() < extra {
        items.reserve_exact(extra.max(items.len() / 8).max(4096));
    }
}

/// Where a stored name goes in a table of `slots` (a power of two): a hash of its bytes, by words.
fn slot_of(stored: &[u8], slots: usize) -> usize {
    const K: u64 = 0x9E37_79B9_7F4A_7C15;
    let (words, rest) = stored.as_chunks::<8>();
    let mut tail = [0u8; 8];
    tail[..rest.len()].copy_from_slice(rest);
    let hash = words.iter().chain([&tail]).fold(stored.len() as u64, |h, w| (h.rotate_left(5) ^ u64::from_le_bytes(*w)).wrapping_mul(K));
    (hash >> (64 - slots.trailing_zeros())) as usize
}

/// The largest index at or below `max` that starts a character.
fn floor_char_boundary(text: &str, max: usize) -> usize {
    if max >= text.len() {
        return text.len();
    }
    (0..=max).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KB: u64 = 1024;

    /// A reference to folder `n`: the root has sequence number 5, the test's other folders 1.
    fn parent_ref(n: u32) -> u64 {
        make_ref(n, if n == ROOT { 5 } else { 1 })
    }

    fn dir(seq: u16, names: &[(u32, &str)]) -> RecordState {
        RecordState { seq, dir: true, attrs: 0, modified: 1, size: 0, names: names.iter().map(|&(p, n)| (parent_ref(p), n.into())).collect() }
    }

    fn file(size: u64, names: &[(u32, &str)]) -> RecordState {
        RecordState { seq: 1, dir: false, attrs: 0x20, modified: 2, size, names: names.iter().map(|&(p, n)| (parent_ref(p), n.into())).collect() }
    }

    /// C:\Users\me\{a.txt 1K, docs\{b.pdf 10K, .hidden\c.pdf 100K}}, C:\Data\big.bin 1M.
    fn sample() -> Tree {
        let mut tree = Tree::default();
        tree.set_record(ROOT, &RecordState { seq: 5, dir: true, names: vec![(make_ref(ROOT, 5), ".".into())], ..Default::default() });
        tree.set_record(20, &dir(1, &[(ROOT, "Users")]));
        tree.set_record(21, &dir(1, &[(20, "me")]));
        tree.set_record(22, &file(KB, &[(21, "a.txt")]));
        tree.set_record(23, &dir(1, &[(21, "docs")]));
        tree.set_record(24, &file(10 * KB, &[(23, "b.pdf")]));
        let mut hidden = dir(1, &[(23, ".hidden")]);
        hidden.attrs = 0x2;
        tree.set_record(25, &hidden);
        tree.set_record(26, &file(100 * KB, &[(25, "c.pdf")]));
        tree.set_record(30, &dir(1, &[(ROOT, "Data")]));
        tree.set_record(31, &file(1024 * KB, &[(30, "big.bin")]));
        tree
    }

    fn size(tree: &Tree, n: u32) -> u64 {
        tree.rec(n).unwrap().size()
    }

    /// Folder totals kept incrementally must equal a recount from scratch.
    fn assert_totals(tree: &mut Tree) {
        let before: Vec<u64> = tree.recs.iter().map(|r| r.size()).collect();
        tree.recompute_totals();
        let after: Vec<u64> = tree.recs.iter().map(|r| r.size()).collect();
        assert_eq!(before, after);
    }

    fn find(tree: &Tree, query: &str, scope: u32, hidden: bool, excludes: &[&str]) -> Vec<String> {
        let (names, filters) = crate::query::parse(query).unwrap();
        let matcher = NameMatcher::from_terms(names);
        // 2026-01-01 as a FILETIME, for dm:.
        let now = 134_116_992_000_000_000;
        let excludes = Excludes::new(&excludes.iter().map(|e| e.to_string()).collect::<Vec<_>>());
        let mut cache = HashMap::new();
        let mut paths: Vec<String> = tree
            .search(&Search { matcher: &matcher, filters: &filters, now, scope, hidden, excludes: &excludes })
            .into_iter()
            .map(|id| tree.path(id, &mut cache).unwrap())
            .collect();
        paths.sort();
        paths
    }

    #[test]
    fn totals_follow_every_change() {
        let mut tree = sample();
        assert_eq!(size(&tree, 23), 110 * KB);
        assert_eq!(size(&tree, 20), 111 * KB);
        assert_eq!(size(&tree, ROOT), 1135 * KB);
        assert_totals(&mut tree);

        // A file grows.
        tree.set_record(24, &file(20 * KB, &[(23, "b.pdf")]));
        assert_eq!((size(&tree, 23), size(&tree, ROOT)), (120 * KB, 1145 * KB));
        // A folder moves to another parent, taking its total along.
        tree.set_record(23, &dir(1, &[(30, "docs")]));
        assert_eq!((size(&tree, 21), size(&tree, 30)), (KB, 1144 * KB));
        assert_eq!(size(&tree, ROOT), 1145 * KB);
        assert_totals(&mut tree);
        // A file is deleted.
        tree.remove_record(31);
        assert_eq!((size(&tree, 30), size(&tree, ROOT)), (120 * KB, 121 * KB));
        // A record is reused for a different file (new sequence number).
        tree.set_record(22, &RecordState { seq: 2, ..file(5 * KB, &[(30, "new.txt")]) });
        assert_eq!((size(&tree, 21), size(&tree, 30)), (0, 125 * KB));
        assert_totals(&mut tree);
    }

    #[test]
    fn notes_what_folders_held_at_the_last_mark() {
        let mut tree = sample();
        tree.growth.mark(0, size(&tree, ROOT));
        // b.pdf grows twice: its folders are noted at their sizes before the first time.
        tree.set_record(24, &file(20 * KB, &[(23, "b.pdf")]));
        tree.set_record(24, &file(30 * KB, &[(23, "b.pdf")]));
        // A new folder, with a file in it.
        tree.set_record(50, &dir(1, &[(30, "new")]));
        tree.set_record(51, &file(5 * KB, &[(50, "n.bin")]));
        // A folder moves: it stays as big, the folders it left and joined change.
        tree.set_record(25, &dir(1, &[(30, ".hidden")]));
        // Recounting to undo drift notes nothing.
        tree.recompute_totals();
        tree.growth.mark(1, size(&tree, ROOT));

        let then = |tree: &Tree, n: u32, epoch: u16| {
            let r = tree.rec(n).unwrap();
            tree.growth.then(n, r.seq(), epoch, r.size())
        };
        assert_eq!((then(&tree, 23, 0), size(&tree, 23)), (Some(110 * KB), 30 * KB));
        assert_eq!(then(&tree, 21, 0), Some(111 * KB));
        assert_eq!((then(&tree, 30, 0), size(&tree, 30)), (Some(1024 * KB), 1129 * KB));
        assert_eq!(then(&tree, ROOT, 0), Some(1135 * KB));
        assert_eq!(then(&tree, 50, 0), None);
        assert_eq!(then(&tree, 25, 0), Some(100 * KB));
        // At the second mark, they were as they are now.
        assert_eq!(then(&tree, 23, 1), Some(30 * KB));
        assert_eq!(then(&tree, 50, 1), Some(5 * KB));

        // The folder is deleted and its record made into another folder: that one is new.
        tree.remove_record(51);
        tree.remove_record(50);
        tree.set_record(50, &dir(2, &[(ROOT, "again")]));
        assert_eq!(then(&tree, 50, 1), None);
        assert_totals(&mut tree);
    }

    /// What noting folders for growth costs a file change, eight folders deep (run with --release
    /// --ignored --nocapture).
    #[test]
    #[ignore]
    fn growth_notes_cost_little() {
        let build = || {
            let mut tree = Tree::default();
            tree.set_record(ROOT, &RecordState { seq: 5, dir: true, ..Default::default() });
            for (i, n) in (100..108u32).enumerate() {
                tree.set_record(n, &dir(1, &[(if i == 0 { ROOT } else { n - 1 }, "d")]));
            }
            for n in 1000..201_000u32 {
                tree.set_record(n, &file(1, &[(100 + n % 8, "f")]));
            }
            tree
        };
        let run = |tree: &mut Tree| {
            let started = std::time::Instant::now();
            for round in 0..3u64 {
                for n in 1000..201_000u32 {
                    tree.set_record(n, &file(2 + round, &[(100 + n % 8, "f")]));
                }
            }
            started.elapsed().as_secs_f64() * 1e9 / 600_000.0
        };
        let mut plain = build();
        let mut noted = build();
        noted.growth.mark(0, 0);
        let (without, with) = (run(&mut plain), run(&mut noted));
        println!("a file change: {without:.0} ns without growth notes, {with:.0} ns with");
    }

    fn listed(tree: &Tree, dir: u32) -> Vec<String> {
        let mut names: Vec<String> = tree.list(dir).into_iter().map(|id| tree.name(id).to_string()).collect();
        names.sort();
        names
    }

    #[test]
    fn lists_what_is_right_inside_a_folder() {
        let mut tree = sample();
        // Empty files and folders are listed too, and hidden ones (the app hides them as it's set to);
        // what's further down isn't.
        tree.set_record(40, &file(0, &[(21, "empty.txt")]));
        tree.set_record(41, &dir(1, &[(21, "New folder")]));
        assert_eq!(listed(&tree, 21), ["New folder", "a.txt", "docs", "empty.txt"]);
        assert_eq!(listed(&tree, 23), [".hidden", "b.pdf"]);
        assert_eq!(listed(&tree, 41), Vec::<String>::new());
        // NTFS's own records in the root aren't items.
        tree.set_record(11, &dir(1, &[(ROOT, "$Extend")]));
        tree.set_record(3, &file(10, &[(ROOT, "$Volume")]));
        assert_eq!(listed(&tree, ROOT), ["Data", "Users"]);
        // A hard link is listed in each folder it's in.
        tree.set_record(42, &file(KB, &[(21, "one.dll"), (30, "two.dll")]));
        assert!(listed(&tree, 21).contains(&"one.dll".to_string()) && listed(&tree, 30).contains(&"two.dll".to_string()));
    }

    #[test]
    fn hard_links_count_where_they_are() {
        let mut tree = sample();
        tree.set_record(40, &file(2 * KB, &[(21, "one.dll"), (30, "two.dll")]));
        assert_eq!((size(&tree, 21), size(&tree, 30)), (113 * KB, 1026 * KB));
        assert_eq!(find(&tree, "dll", ROOT, false, &[]), [r"Data\two.dll", r"Users\me\one.dll"]);
        // One link goes away, the other stays.
        tree.set_record(40, &file(2 * KB, &[(30, "two.dll")]));
        assert_eq!((size(&tree, 21), size(&tree, 30)), (111 * KB, 1026 * KB));
        assert_totals(&mut tree);
        assert_eq!(tree.links_of(40).count(), 1);
    }

    #[test]
    fn searches_below_a_scope_like_fd() {
        let tree = sample();
        assert_eq!(find(&tree, "pdf", ROOT, false, &[]), [r"Users\me\docs\b.pdf"]);
        assert_eq!(find(&tree, "pdf", ROOT, true, &[]), [r"Users\me\docs\.hidden\c.pdf", r"Users\me\docs\b.pdf"]);
        // Searching inside a hidden folder finds what's in it, as fd does when given that folder.
        assert_eq!(find(&tree, "*.pdf", 25, false, &[]), [r"Users\me\docs\.hidden\c.pdf"]);
        assert_eq!(find(&tree, "a b", ROOT, true, &[]), Vec::<String>::new());
        assert_eq!(find(&tree, "e", 30, false, &[]), Vec::<String>::new());
        assert_eq!(find(&tree, "*", 21, true, &[]).len(), 5);
        assert_eq!(find(&tree, "*", 21, true, &["docs"]), [r"Users\me\a.txt"]);
        assert_eq!(find(&tree, "*", 21, true, &[r"docs\.hidden", "*.txt"]), [r"Users\me\docs", r"Users\me\docs\b.pdf"]);
        // The root folder itself and NTFS metadata never show up.
        assert!(find(&tree, ".", ROOT, true, &[]).iter().all(|p| p.contains('.')));
    }

    #[test]
    fn ranks_the_best_matches_first() {
        let mut tree = sample();
        tree.set_record(50, &dir(1, &[(ROOT, "Windows")]));
        tree.set_record(51, &dir(1, &[(50, "System32")]));
        tree.set_record(52, &file(KB, &[(51, "report.dll")]));
        tree.set_record(53, &file(KB, &[(21, "Q3 report.pdf")]));
        tree.set_record(54, &file(KB, &[(23, "reports 2026.xlsx")]));
        tree.set_record(55, &file(KB, &[(23, "Report.pdf")]));
        tree.set_record(56, &file(KB, &[(30, "misreported.txt")]));
        tree.set_record(57, &file(KB, &[(21, "Report.docx")]));
        let matcher = NameMatcher::from_terms(vec!["report".into()]);
        let search = Search { matcher: &matcher, filters: &Default::default(), now: 0, scope: ROOT, hidden: false, excludes: &Excludes::default() };
        let found = tree.search(&search);
        let mut cache = HashMap::new();
        let mut paths = |ranked: Vec<Rank>| ranked.into_iter().map(|r| tree.path(r.id, &mut cache).unwrap()).collect::<Vec<_>>();
        // As typed (the shorter path first), starting with it, a word starting with it, then inside a
        // word; Windows' own copy, though named as typed, behind every other one named as typed.
        assert_eq!(
            paths(tree.rank(&found, &search, b"report", 10)),
            [r"Users\me\Report.docx", r"Users\me\docs\Report.pdf", r"Windows\System32\report.dll", r"Users\me\docs\reports 2026.xlsx", r"Users\me\Q3 report.pdf", r"Data\misreported.txt"]
        );
        // The best two of them all, whatever the index order.
        assert_eq!(paths(tree.rank(&found, &search, b"report", 2)), [r"Users\me\Report.docx", r"Users\me\docs\Report.pdf"]);
    }

    #[test]
    fn close_matches_come_closest_first() {
        let mut tree = sample();
        tree.set_record(40, &file(KB, &[(30, "Phtograhps 2.zip")]));
        tree.set_record(41, &file(KB, &[(30, "Photographs.zip")]));
        tree.set_record(42, &file(KB, &[(30, "graphs.txt")]));
        let matcher = NameMatcher::near(vec!["photgraphs".into()]).unwrap();
        let mut cache = HashMap::new();
        let found: Vec<String> = tree
            .search(&Search { matcher: &matcher, filters: &Default::default(), now: 0, scope: ROOT, hidden: false, excludes: &Excludes::default() })
            .into_iter()
            .map(|id| tree.path(id, &mut cache).unwrap())
            .collect();
        assert_eq!(found, [r"Data\Photographs.zip", r"Data\Phtograhps 2.zip"], "one edit away before two, whatever the index order");
    }

    #[test]
    fn resolves_paths_and_lists_folder_sizes() {
        let tree = sample();
        assert_eq!(tree.resolve(r"users\ME\Docs"), Some(23));
        assert_eq!(tree.resolve(r"\Users/me/"), Some(21));
        assert_eq!(tree.resolve(""), Some(ROOT));
        assert_eq!(tree.resolve(r"Users\nobody"), None);
        assert_eq!(tree.resolve(r"Users\me\a.txt"), None);
        let mut sizes = tree.child_dirs(ROOT);
        sizes.sort();
        assert_eq!(sizes, [("Data".to_string(), 1024 * KB), ("Users".to_string(), 111 * KB)]);
        let mut cache = HashMap::new();
        assert_eq!(tree.dir_path(25, &mut cache), Some(r"Users\me\docs\.hidden"));
        assert_eq!(tree.dir_path(ROOT, &mut cache), Some(""));
    }

    /// A map as (name, parent's place, size, folder), the folder mapped first as "".
    fn map(tree: &Tree, dir: u32, depth: u32, max: usize) -> (Vec<(String, u32, u64, bool)>, bool) {
        let (items, limited) = tree.usage(dir, depth, max);
        let named = items.iter().map(|i| (if i.link == NONE { String::new() } else { tree.name(i.link).to_string() }, i.parent, i.size, i.dir)).collect();
        (named, limited)
    }

    fn item(name: &str, parent: u32, size: u64, dir: bool) -> (String, u32, u64, bool) {
        (name.to_string(), parent, size, dir)
    }

    #[test]
    fn maps_disk_usage_level_by_level() {
        let mut tree = sample();
        // Empty things take no room in a map.
        tree.set_record(32, &dir(1, &[(30, "Empty")]));
        tree.set_record(33, &file(0, &[(30, "zero.txt")]));
        assert_eq!(map(&tree, ROOT, 1, 100), (vec![item("", NONE, 1135 * KB, true), item("Data", 0, 1024 * KB, true), item("Users", 0, 111 * KB, true)], false));
        // Every folder's items follow it together, biggest first; hidden ones too.
        let everything = vec![
            item("", NONE, 1135 * KB, true),
            item("Data", 0, 1024 * KB, true),
            item("Users", 0, 111 * KB, true),
            item("big.bin", 1, 1024 * KB, false),
            item("me", 2, 111 * KB, true),
            item("docs", 4, 110 * KB, true),
            item("a.txt", 4, KB, false),
            item(".hidden", 5, 100 * KB, true),
            item("b.pdf", 5, 10 * KB, false),
            item("c.pdf", 7, 100 * KB, false),
        ];
        assert_eq!(map(&tree, ROOT, 9, 100), (everything.clone(), false));
        assert_eq!(map(&tree, ROOT, 4, 100).0, everything[..9]);
        // A folder below the root, and a file instead of a folder.
        assert_eq!(map(&tree, 21, 1, 100).0, [item("", NONE, 111 * KB, true), item("docs", 0, 110 * KB, true), item("a.txt", 0, KB, false)]);
        assert!(tree.usage(22, 3, 100).0.is_empty());
    }

    #[test]
    fn keeps_the_biggest_items_and_their_folders() {
        let mut tree = sample();
        // Only the biggest stay; ties go to the item found first (folders before files).
        assert_eq!(map(&tree, ROOT, 9, 3), (vec![item("", NONE, 1135 * KB, true), item("Data", 0, 1024 * KB, true), item("Users", 0, 111 * KB, true), item("big.bin", 1, 1024 * KB, false)], true));
        assert_eq!(map(&tree, ROOT, 9, 0), (vec![item("", NONE, 1135 * KB, true)], true));
        // A total that drifted below what's in the folder still brings the folder along.
        tree.recs[25].set_size(1);
        let (kept, limited) = map(&tree, 23, 9, 1);
        assert_eq!(kept, [item("", NONE, 110 * KB, true), item(".hidden", 0, 1, true), item("c.pdf", 1, 100 * KB, false)]);
        assert!(limited);
    }

    /// What `usage` must return, the slow way: every item down to `depth`, cut to the biggest `max`
    /// (ancestors winning ties) plus their folders, then in map order.
    fn usage_by_hand(tree: &Tree, dir: u32, depth: u32, max: usize) -> (Vec<(String, u64, bool, String)>, bool) {
        let mut cache = HashMap::new();
        // (link, size, folder, level): folders level by level, then files, as discovery order.
        let mut found: Vec<(u32, u64, bool)> = Vec::new();
        let mut level = vec![dir];
        let mut open = vec![];
        for _ in 0..depth {
            let mut next = vec![];
            for folder in level {
                open.push(folder);
                for &id in tree.kids.of(folder) {
                    let rec = tree.link(id).rec;
                    if tree.recs[rec as usize].size() > 0 {
                        found.push((id, tree.recs[rec as usize].size(), true));
                        next.push(rec);
                    }
                }
            }
            level = next;
        }
        for (id, link) in tree.links.iter().enumerate() {
            let rec = tree.recs[link.rec as usize];
            if link.flags & LINK_USED != 0 && open.contains(&link.parent) && !rec.dir() && rec.size() > 0 && link.rec >= FIRST_USER_RECORD {
                found.push((id as u32, rec.size(), false));
            }
        }
        let mut order: Vec<usize> = (0..found.len()).collect();
        order.sort_by(|&a, &b| found[b].1.cmp(&found[a].1).then(a.cmp(&b)));
        let mut kept: Vec<u32> = order.iter().take(max).map(|&i| found[i].0).collect();
        // Their folders, up to the folder mapped.
        for &id in kept.clone().iter() {
            let mut up = tree.link(id).parent;
            while up != dir {
                let link = tree.recs[up as usize].first_link;
                if !kept.contains(&link) {
                    kept.push(link);
                }
                up = tree.link(link).parent;
            }
        }
        let mut items: Vec<(String, u64, bool, String)> = kept
            .iter()
            .map(|&id| {
                let rec = tree.recs[tree.link(id).rec as usize];
                (tree.name(id).to_string(), rec.size(), rec.dir(), tree.dir_path(tree.link(id).parent, &mut cache).unwrap().to_string())
            })
            .collect();
        items.sort();
        (items, found.len() > max)
    }

    #[test]
    fn deep_maps_skip_only_what_cant_make_the_map() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for round in 0..30 {
            let mut tree = Tree::default();
            tree.set_record(ROOT, &RecordState { seq: 5, dir: true, ..Default::default() });
            let mut folders = vec![ROOT];
            for n in 20..45u32 {
                let parent = folders[next(folders.len() as u64) as usize];
                tree.set_record(n, &dir(1, &[(parent, &format!("d{n}"))]));
                folders.push(n);
            }
            // Every file a different power of two, so no two different sets of files tie (and all of
            // them together stay below MAX_SIZE).
            let mut bits: Vec<u32> = (0..47).collect();
            for n in 100..100 + 17 + round {
                let bit = bits.swap_remove(next(bits.len() as u64) as usize);
                let parent = folders[next(folders.len() as u64) as usize];
                tree.set_record(n, &file(1 << bit, &[(parent, &format!("f{n}"))]));
            }
            for depth in [1, 2, 3, 5, 9] {
                for max in [0, 1, 3, 7, 15, 40, 100] {
                    let (items, limited) = tree.usage(ROOT, depth, max);
                    let mut cache = HashMap::new();
                    let mut got: Vec<(String, u64, bool, String)> = items[1..]
                        .iter()
                        .map(|i| (tree.name(i.link).to_string(), i.size, i.dir, tree.dir_path(tree.link(i.link).parent, &mut cache).unwrap().to_string()))
                        .collect();
                    got.sort();
                    assert_eq!((got, limited), usage_by_hand(&tree, ROOT, depth, max), "round {round}, depth {depth}, max {max}");
                    // And in map order: each folder's items together after it, biggest first.
                    for (i, item) in items.iter().enumerate().skip(1) {
                        assert!(item.parent < i as u32 && items[item.parent as usize].dir);
                        if i > 1 && items[i - 1].parent == item.parent {
                            assert!(items[i - 1].size >= item.size);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn maps_every_hard_link_where_it_is() {
        let mut tree = sample();
        tree.set_record(40, &file(2 * KB, &[(21, "one.dll"), (30, "two.dll")]));
        let (items, _) = map(&tree, ROOT, 3, 100);
        let dlls: Vec<_> = items.iter().filter(|i| i.0.ends_with(".dll")).collect();
        // Root, Data, Users, then Data's big.bin and two.dll, Users' me, and me's docs, one.dll, a.txt.
        assert_eq!(dlls, [&item("two.dll", 1, 2 * KB, false), &item("one.dll", 5, 2 * KB, false)]);
        assert_eq!(items.len(), 9);
    }

    #[test]
    fn non_ascii_names() {
        let mut tree = sample();
        tree.set_record(50, &file(1, &[(21, "Ünïcödé Résumé.PDF")]));
        assert_eq!(find(&tree, "résumé", ROOT, false, &[]), [r"Users\me\Ünïcödé Résumé.PDF"]);
        assert_eq!(find(&tree, "ÜNÏ*.pdf", ROOT, false, &[]), [r"Users\me\Ünïcödé Résumé.PDF"]);
        tree.set_record(51, &dir(1, &[(21, "Été")]));
        assert_eq!(tree.resolve(r"users\me\ÉTÉ"), Some(51));
    }

    #[test]
    fn orphans_and_stale_parents_are_not_found() {
        let mut tree = sample();
        // A name whose parent record was reused (sequence mismatch) hangs nowhere.
        tree.set_record(60, &RecordState { names: vec![(make_ref(21, 9), "stale.txt".into())], ..file(1, &[]) });
        // A name in NTFS metadata ($Extend is record 11).
        tree.set_record(61, &file(1, &[(11, "$Quota")]));
        assert!(find(&tree, "stale", ROOT, true, &[]).is_empty());
        assert!(find(&tree, "quota", ROOT, true, &[]).is_empty());
    }

    #[test]
    fn a_folder_made_again_keeps_what_is_made_in_it() {
        // docs (23) goes with everything in it and is made again at once, with b.pdf in it: NTFS
        // gives the new docs b.pdf's old record (24), and the new b.pdf docs' old one (23). The
        // journal lists 23 first (b.pdf's delete), so read in that order, b.pdf would find the old
        // b.pdf in 24 and hang nowhere.
        let docs = RecordState { seq: 2, ..dir(2, &[(21, "docs")]) };
        let pdf = RecordState { seq: 2, names: vec![(make_ref(24, 2), "b.pdf".into())], ..file(10 * KB, &[]) };
        let batch = vec![(23, Some(pdf)), (24, Some(docs)), (25, None), (26, None)];
        let order = |batch: &[(u32, Option<RecordState>)]| batch.iter().map(|(n, _)| *n).collect::<Vec<_>>();
        assert_eq!(order(&parents_first(batch.clone())), [24, 23, 25, 26]);

        let apply = |batch: Vec<(u32, Option<RecordState>)>| {
            let mut tree = sample();
            for (n, state) in batch {
                match state {
                    Some(state) => tree.set_record(n, &state),
                    None => tree.remove_record(n),
                }
            }
            tree
        };
        assert!(find(&apply(batch.clone()), "b.pdf", ROOT, true, &[]).is_empty());
        let mut tree = apply(parents_first(batch));
        assert_eq!(find(&tree, "*.pdf", ROOT, true, &[]), [r"Users\me\docs\b.pdf"]);
        assert_eq!(size(&tree, 21), KB + 10 * KB);
        assert_totals(&mut tree);

        // A cycle (a move between two reads) still places every record once.
        let a = RecordState { names: vec![(parent_ref(41), "a".into())], ..dir(1, &[]) };
        let b = RecordState { names: vec![(parent_ref(40), "b".into())], ..dir(1, &[]) };
        assert_eq!(order(&parents_first(vec![(40, Some(a)), (41, Some(b)), (42, None)])), [41, 40, 42]);
    }

    #[test]
    fn builder_links_names_once_all_records_are_known() {
        let mut builder = Builder::new(32);
        // A damaged extension record naming a base record far past the scan is ignored, not made room for.
        builder.name(1 << 30, make_ref(20, 1), "far.txt");
        builder.size(1 << 30, 1);
        // Children before their folder, and a name from an extension record.
        builder.record(30, 1, false, 0x20, 7);
        builder.size(30, 300);
        builder.name(30, make_ref(20, 1), "late.txt");
        builder.record(20, 1, true, 0, 0);
        builder.name(20, make_ref(ROOT, 5), "Folder");
        builder.record(ROOT, 5, true, 0, 0);
        builder.name(ROOT, make_ref(ROOT, 5), ".");
        builder.name(31, make_ref(20, 1), "never-used.txt");
        builder.name(3, make_ref(ROOT, 5), "$LogFile");
        let tree = builder.finish();
        assert_eq!(tree.resolve("folder"), Some(20));
        assert_eq!((size(&tree, 20), size(&tree, ROOT)), (300, 300));
        assert_eq!(find(&tree, "txt", ROOT, true, &[]), [r"Folder\late.txt"]);
        assert_eq!(tree.stats().files, 1);
        // Filters, from the records themselves: late.txt has 300 bytes, from 1601.
        assert_eq!(find(&tree, "ext:TXT size:>100 dm:>1y", ROOT, true, &[]), [r"Folder\late.txt"]);
        assert!(find(&tree, "size:>1kb", ROOT, true, &[]).is_empty());
        assert!(find(&tree, "late dm:7d", ROOT, true, &[]).is_empty());
        assert_eq!(find(&tree, "folder:", ROOT, true, &[]), ["Folder"]);
    }

    /// A drive's worth of names (3 million, made of real-looking words), searched as typed and then
    /// for close matches: what a search with a typo costs the service. Run with --ignored --nocapture,
    /// in release.
    #[test]
    #[ignore]
    fn close_matches_cost_little_more() {
        const WORDS: &[&str] = &["report", "invoice", "budget", "photo", "download", "setup", "backup", "notes", "draft", "final", "project", "meeting", "summary", "scan", "image", "video", "music", "readme", "config", "data", "cache", "temp", "log", "build"];
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let count = 3_000_000u32;
        let mut builder = Builder::new(count as usize + 32);
        builder.record(ROOT, 5, true, 0, 0);
        builder.record(20, 1, true, 0, 0);
        builder.name(20, make_ref(ROOT, 5), "Folder");
        for n in 21..count {
            builder.record(n, 1, false, 0x20, 0);
            let name = format!("{}_{} {}.{}", WORDS[next(24) as usize], WORDS[next(24) as usize], next(10_000), ["txt", "pdf", "jpg", "dll"][next(4) as usize]);
            builder.name(n, make_ref(20, 1), &name);
        }
        let tree = builder.finish();
        let time = |matcher: &NameMatcher| {
            let started = std::time::Instant::now();
            let found = tree.search(&Search { matcher, filters: &Default::default(), now: 0, scope: ROOT, hidden: false, excludes: &Excludes::default() }).len();
            (found, started.elapsed().as_secs_f64() * 1000.0)
        };
        for query in ["reprot", "invoise", "downlaod", "summray meeting", "zzzzzz"] {
            let terms = crate::query::terms(query);
            let (exact, exact_ms) = time(&NameMatcher::from_terms(terms.clone()));
            let (close, close_ms) = time(&NameMatcher::near(terms).unwrap());
            println!("{query:>16}: as typed {exact} in {exact_ms:.0} ms, close {close} in {close_ms:.0} ms");
        }
    }

    #[test]
    fn folder_lists_follow_every_change() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let mut kids = Kids::from_pairs(vec![(7, 3), (5, 1), (7, 2), (5, 4)]);
        let mut model: HashMap<u32, Vec<u32>> = HashMap::from([(5, vec![1, 4]), (7, vec![2, 3])]);
        for round in 0..200_000u32 {
            let dir = next(300) as u32;
            if next(3) == 0 {
                if let Some(list) = model.get_mut(&dir) {
                    let link = list.remove(next(list.len() as u64) as usize);
                    kids.remove(dir, link);
                    if list.is_empty() {
                        model.remove(&dir);
                    }
                }
            } else {
                kids.add(dir, round + 10);
                model.entry(dir).or_default().push(round + 10);
            }
            if round % 997 == 0 {
                for (dir, list) in &model {
                    assert_eq!(kids.of(*dir), list.as_slice());
                }
            }
        }
        for dir in 0..300 {
            assert_eq!(kids.of(dir), model.get(&dir).map_or(&[][..], Vec::as_slice));
        }
        assert_eq!(kids.at.len(), model.len());
        assert!(kids.garbage < 1 << 16 || kids.garbage < kids.lists.len() / 2);
        // A folder's list of one, then none.
        kids.add(1000, 1);
        assert_eq!(kids.of(1000), [1]);
        kids.remove(1000, 1);
        assert!(kids.of(1000).is_empty() && !kids.at.contains_key(&1000));
    }

    #[test]
    fn records_take_24_bytes() {
        assert_eq!(size_of::<Rec>(), 24);
        let mut rec = Rec::new(MAX_SIZE + 5, -1, 0x0040_0016, 7, 0xFFFF, REC_USED | REC_DIR);
        assert_eq!((rec.size(), rec.modified, rec.attrs(), rec.first_link, rec.seq(), rec.flags()), (MAX_SIZE, -1, 0x0040_0016, 7, 0xFFFF, REC_USED | REC_DIR));
        rec.set_size(12);
        rec.set_attrs(u32::MAX);
        assert_eq!((rec.size(), rec.seq(), rec.flags(), rec.attrs()), (12, 0xFFFF, REC_USED | REC_DIR, ATTRS));
        rec.set_flags(0);
        rec.set_seq(3);
        assert!(!rec.used() && !rec.dir());
        assert_eq!((rec.size(), rec.seq(), rec.attrs()), (12, 3, ATTRS));
    }

    #[test]
    fn links_with_the_same_name_share_it() {
        let mut tree = sample();
        tree.set_record(40, &file(KB, &[(23, "a.txt")]));
        tree.set_record(41, &file(KB, &[(30, "a.txt"), (21, "Ünï.txt")]));
        tree.set_record(42, &file(KB, &[(23, "Ünï.txt")]));
        let every = find(&tree, "*", ROOT, true, &[]);
        let arena = tree.names.len();
        tree.compact();
        assert_eq!(arena - tree.names.len(), 2 * "a.txt".len() + "Ünï.txt".len() + 2 + fold("Ünï.txt").len());
        assert_eq!(find(&tree, "*", ROOT, true, &[]), every);
        assert_eq!(find(&tree, "ünï", ROOT, true, &[]), [r"Users\me\docs\Ünï.txt", r"Users\me\Ünï.txt"]);
        // Back from a snapshot, a shared name counts once…
        let mut tree = Tree::from_parts(tree.recs.clone(), tree.links.clone(), tree.names.clone(), tree.next_names.clone());
        assert_eq!(tree.garbage, 0);
        // …and a link going leaves the name to the others.
        tree.remove_record(40);
        tree.compact();
        assert_eq!(find(&tree, "a.txt", ROOT, true, &[]), [r"Data\a.txt", r"Users\me\a.txt"]);
        // Many different names: the table grows.
        let mut builder = Builder::new(20_000);
        builder.record(20, 1, true, 0, 0);
        builder.name(20, make_ref(ROOT, 5), "Folder");
        for n in 21..20_000u32 {
            builder.record(n, 1, false, 0x20, 0);
            builder.name(n, make_ref(20, 1), &format!("{}.txt", n % 7_000));
        }
        let tree = builder.finish();
        assert_eq!(tree.names.len(), "Folder".len() + (0..7_000).map(|n| format!("{n}.txt").len()).sum::<usize>());
        assert_eq!(find(&tree, "6999.txt", ROOT, true, &[]), [r"Folder\6999.txt"; 2]);
    }

    #[test]
    fn records_and_names_take_what_they_need() {
        let mut builder = Builder::new(101_000);
        for n in 20..1_020u32 {
            builder.record(n, 1, true, 0, 0);
            builder.name(n, make_ref(ROOT, 5), &format!("folder {n}"));
        }
        for n in 1_020..101_000u32 {
            builder.record(n, 1, false, 0x20, 0);
            builder.size(n, 100);
            builder.name(n, make_ref(20 + n % 1_000, 1), &format!("file {}.txt", n % 5_000));
        }
        let tree = builder.finish();
        // 24 bytes a record, 16 a name (and a sixteenth more for what comes next), 5,000 different
        // names of about 13 bytes, each stored once, and the lists of folders.
        let memory = tree.stats().memory as usize;
        assert!(memory < 101_000 * 24 + 100_000 * 17 + 5_000 * 15 + 1_000 * 8 + 64 * 1024, "{memory}");
        assert_eq!(find(&tree, "file 4999.txt", ROOT, true, &[]).len(), 20);
    }

    #[test]
    fn small_changes_dont_double_memory() {
        let mut builder = Builder::new(200_000);
        builder.record(ROOT, 5, true, 0, 0);
        builder.record(20, 1, true, 0, 0);
        builder.name(20, make_ref(ROOT, 5), "Folder");
        for n in 21..200_000u32 {
            builder.record(n, 1, false, 0x20, 0);
            builder.size(n, 100);
            builder.name(n, make_ref(20, 1), &format!("file number {n}.txt"));
        }
        let mut tree = builder.finish();
        let before = tree.stats().memory;
        // New records past the end of the MFT, new names: what the journal brings after a scan.
        for n in 200_000..200_100u32 {
            tree.set_record(n, &file(1, &[(20, &format!("new {n}.txt"))]));
        }
        let after = tree.stats().memory;
        assert!(after < before + before / 8, "{before} → {after}");
        assert_eq!(tree.rec(20).unwrap().size(), 199_979 * 100 + 100);
    }

    /// Random churn: incremental totals, names and paths must stay what a rebuild would give.
    #[test]
    fn random_changes_keep_totals_exact() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let mut tree = sample();
        let dirs = [ROOT, 20, 21, 23, 25, 30];
        for round in 0..2000 {
            let n = 100 + next(60) as u32;
            match next(4) {
                0 => tree.remove_record(n),
                1 if n.is_multiple_of(7) => {
                    // Folders only move under the fixed ones, so no cycles.
                    let parent = dirs[next(dirs.len() as u64) as usize];
                    if !tree.rec(n).is_some_and(|r| r.used() && !r.dir()) {
                        tree.set_record(n, &dir(1, &[(parent, &format!("d{n}"))]));
                    }
                }
                _ => {
                    if tree.rec(n).is_some_and(|r| r.dir()) {
                        continue;
                    }
                    let links = 1 + next(2) as usize;
                    let names: Vec<(u32, String)> = (0..links).map(|i| (dirs[next(dirs.len() as u64) as usize], format!("f{n}-{i}"))).collect();
                    let names: Vec<(u32, &str)> = names.iter().map(|(p, s)| (*p, s.as_str())).collect();
                    tree.set_record(n, &file(next(10_000), &names));
                }
            }
            if round % 100 == 0 {
                assert_totals(&mut tree);
            }
        }
        assert_totals(&mut tree);
        let before = find(&tree, "*", ROOT, true, &[]);
        let arena = tree.names.len();
        tree.compact();
        assert!(tree.names.len() < arena);
        assert_eq!(find(&tree, "*", ROOT, true, &[]), before);
    }
}
