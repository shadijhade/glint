//! The index of one NTFS volume: every file record (size, last write time, attributes) by MFT record
//! number, every name (hard links included) as a link from a record to its parent folder, and the
//! total size below every folder, kept current as changes come in.

use crate::query::{fold, is_hidden, Excludes, NameMatcher};
use rayon::prelude::*;
use std::collections::HashMap;

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
pub const LINK_USED: u16 = 1;
/// The name isn't ASCII: its lowercase form follows it in the arena, after a u16 length.
pub const LINK_FOLDED: u16 = 2;

pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rec {
    /// Files: bytes in the unnamed data stream. Folders: everything below them, every hard link counted
    /// where it is (as Explorer and dust count them).
    pub size: u64,
    /// Last write time as a FILETIME (100 ns ticks since 1601).
    pub modified: i64,
    /// FILE_ATTRIBUTE_* bits; folders always carry FILE_ATTRIBUTE_DIRECTORY.
    pub attrs: u32,
    /// The record's first name, NONE when it has none.
    pub first_link: u32,
    /// Bumped by NTFS whenever the record is reused for another file.
    pub seq: u16,
    pub flags: u16,
}

impl Default for Rec {
    fn default() -> Self {
        Rec { size: 0, modified: 0, attrs: 0, first_link: NONE, seq: 0, flags: 0 }
    }
}

impl Rec {
    pub fn used(&self) -> bool {
        self.flags & REC_USED != 0
    }
    pub fn dir(&self) -> bool {
        self.flags & REC_DIR != 0
    }
}

/// One name of a record: a hard link from it to the folder it's in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub rec: u32,
    pub parent: u32,
    /// Where the name starts in the arena.
    pub name: u32,
    /// Its length in bytes (UTF-8).
    pub len: u16,
    pub flags: u16,
    /// The record's next name, NONE after the last.
    pub next: u32,
}

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

#[derive(Default)]
pub struct Tree {
    pub(crate) recs: Vec<Rec>,
    pub(crate) links: Vec<Link>,
    pub(crate) names: Vec<u8>,
    free: Vec<u32>,
    /// Each folder's child folders (as links), for resolving paths and listing folder sizes.
    dirs: HashMap<u32, Vec<u32>>,
    /// Arena bytes no live link uses any more.
    garbage: usize,
}

/// A name search over one volume.
pub struct Search<'a> {
    pub matcher: &'a NameMatcher,
    /// The folder to search below; ROOT for the whole volume.
    pub scope: u32,
    /// Include hidden files and folders (and everything inside hidden folders).
    pub hidden: bool,
    pub excludes: &'a Excludes,
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
    pub(crate) fn from_parts(recs: Vec<Rec>, links: Vec<Link>, names: Vec<u8>) -> Tree {
        let mut tree = Tree { recs, links, names, ..Default::default() };
        let mut live = 0;
        for (id, link) in tree.links.iter().enumerate() {
            if link.flags & LINK_USED == 0 {
                tree.free.push(id as u32);
                continue;
            }
            live += Tree::stored_len(&tree.names, link);
            if tree.recs[link.rec as usize].dir() && link.parent != NONE {
                tree.dirs.entry(link.parent).or_default().push(id as u32);
            }
        }
        tree.garbage = tree.names.len() - live;
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
                next = self.links[id as usize].next;
                id
            })
        })
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
            Some(r) if r.used() && seq != 0 && r.seq != seq => NONE,
            _ => number as u32,
        }
    }

    fn add_link(&mut self, rec: u32, parent: u32, name: &str) {
        let (at, len, flags) = self.push_name(name);
        let link = Link { rec, parent, name: at, len, flags, next: self.recs[rec as usize].first_link };
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
        let r = self.recs[rec as usize];
        self.recs[rec as usize].first_link = id;
        if r.dir() && parent != NONE {
            self.dirs.entry(parent).or_default().push(id);
        }
        if r.size != 0 {
            self.add_up(parent, r.size as i64);
        }
    }

    fn remove_link(&mut self, id: u32) {
        let link = self.links[id as usize];
        let r = self.recs[link.rec as usize];
        if r.first_link == id {
            self.recs[link.rec as usize].first_link = link.next;
        } else {
            let mut at = r.first_link;
            while at != NONE {
                let next = self.links[at as usize].next;
                if next == id {
                    self.links[at as usize].next = link.next;
                    break;
                }
                at = next;
            }
        }
        if r.dir() {
            if let Some(children) = self.dirs.get_mut(&link.parent) {
                children.retain(|&c| c != id);
                if children.is_empty() {
                    self.dirs.remove(&link.parent);
                }
            }
        }
        if r.size != 0 {
            self.add_up(link.parent, -(r.size as i64));
        }
        self.garbage += Tree::stored_len(&self.names, &link);
        self.links[id as usize] = Link { rec: NONE, parent: NONE, name: 0, len: 0, flags: 0, next: NONE };
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
            rec.size = rec.size.saturating_add_signed(delta);
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
        if old.used() && (old.seq != state.seq || old.dir() != state.dir) {
            self.remove_record(n);
        }
        let used = self.recs[n as usize].used();
        // The size first: names kept get the difference, names added below the new size.
        if !state.dir {
            let delta = state.size as i64 - if used { self.recs[n as usize].size as i64 } else { 0 };
            self.recs[n as usize].size = state.size;
            if used && delta != 0 {
                let parents: Vec<u32> = self.links_of(n).map(|id| self.links[id as usize].parent).collect();
                for parent in parents {
                    self.add_up(parent, delta);
                }
            }
        }
        // (A folder new to the index starts with whatever its contents added up to already.)
        let rec = &mut self.recs[n as usize];
        rec.seq = state.seq;
        rec.modified = state.modified;
        rec.attrs = state.attrs | if state.dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 };
        rec.flags = REC_USED | if state.dir { REC_DIR } else { 0 };
        if n == ROOT {
            return;
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
                rec.size = 0;
            }
        }
        // Each folder first gets the files right in it…
        for i in 0..self.links.len() {
            let link = self.links[i];
            if link.flags & LINK_USED == 0 || link.parent == NONE {
                continue;
            }
            let rec = self.recs[link.rec as usize];
            if rec.dir() || rec.size == 0 {
                continue;
            }
            if let Some(parent) = self.recs.get_mut(link.parent as usize) {
                if parent.dir() {
                    parent.size += rec.size;
                }
            }
        }
        // …then hands its total to its parent, deepest folders first.
        let depths = self.depths();
        let mut order: Vec<u32> = (0..self.recs.len() as u32).filter(|&n| depths[n as usize] != u32::MAX && n != ROOT).collect();
        order.sort_unstable_by_key(|&n| std::cmp::Reverse(depths[n as usize]));
        for n in order {
            let parent = self.parent(n);
            let size = self.recs[n as usize].size;
            if parent != NONE && size != 0 {
                self.recs[parent as usize].size += size;
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
            let children = self.dirs.get(&dir)?;
            dir = self.links[*children.iter().find(|&&id| self.name_is(id, &lower))? as usize].rec;
        }
        Some(dir)
    }

    /// The folders right inside `dir`, with the total size of each.
    pub fn child_dirs(&self, dir: u32) -> Vec<(String, u64)> {
        let Some(children) = self.dirs.get(&dir) else { return Vec::new() };
        children.iter().map(|&id| (self.name(id).to_string(), self.recs[self.links[id as usize].rec as usize].size)).collect()
    }

    /// Every name below `search.scope` that matches, in index order.
    pub fn search(&self, search: &Search) -> Vec<u32> {
        const CHUNK: usize = 1 << 14;
        let found: Vec<Vec<u32>> = self
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
                    if !search.matcher.matches(name, folded) {
                        continue;
                    }
                    let id = (chunk * CHUNK + i) as u32;
                    if !search.hidden && is_hidden(self.recs[link.rec as usize].attrs, name) {
                        continue;
                    }
                    if search.excludes.name(name, folded) || !self.reachable(link.parent, search) {
                        continue;
                    }
                    if search.excludes.has_paths() && self.excluded_path(id, search) {
                        continue;
                    }
                    found.push(id);
                }
                found
            })
            .collect();
        found.concat()
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
            if (!search.hidden && is_hidden(rec.attrs, name)) || search.excludes.name(name, Tree::folded_in(&self.names, link)) {
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

    /// Rewrites the name arena with only the names in use.
    pub fn compact(&mut self) {
        let mut names = Vec::with_capacity(self.names.len().saturating_sub(self.garbage));
        for link in self.links.iter_mut().filter(|l| l.flags & LINK_USED != 0) {
            let start = link.name as usize;
            let len = Tree::stored_len(&self.names, link);
            link.name = names.len() as u32;
            names.extend_from_slice(&self.names[start..start + len]);
        }
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
        let dirs: usize = self.dirs.values().map(|v| v.capacity() * 4 + 48).sum();
        stats.memory = (self.recs.capacity() * size_of::<Rec>()
            + self.links.capacity() * size_of::<Link>()
            + self.names.capacity()
            + self.free.capacity() * 4
            + dirs) as u64;
        stats
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
        let tree = Tree { recs: vec![Rec::default(); records], names: Vec::with_capacity(records * 25), ..Default::default() };
        Builder { tree, pending: Vec::with_capacity(records) }
    }

    fn slot(&mut self, n: u32) -> Option<&mut Rec> {
        if n < FIRST_USER_RECORD && n != ROOT {
            return None;
        }
        self.tree.ensure(n);
        Some(&mut self.tree.recs[n as usize])
    }

    /// A base record in use.
    pub fn record(&mut self, n: u32, seq: u16, dir: bool, attrs: u32, modified: i64) {
        if let Some(rec) = self.slot(n) {
            rec.seq = seq;
            rec.modified = modified;
            rec.attrs = attrs | if dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 };
            rec.flags = REC_USED | if dir { REC_DIR } else { 0 };
        }
    }

    /// The unnamed data stream's size, from the base record or one of its extension records.
    pub fn size(&mut self, n: u32, size: u64) {
        if let Some(rec) = self.slot(n) {
            rec.size = size;
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
                rec.size = 0;
            }
        }
        tree.links.reserve(self.pending.len());
        let mut live = 0;
        for &(n, parent, at, len, flags) in &self.pending {
            let rec = tree.recs[n as usize];
            let parent = tree.parent_number(parent);
            if !rec.used() || parent == n {
                continue;
            }
            let id = tree.links.len() as u32;
            let link = Link { rec: n, parent, name: at, len, flags, next: rec.first_link };
            live += Tree::stored_len(&tree.names, &link);
            tree.links.push(link);
            tree.recs[n as usize].first_link = id;
            if rec.dir() && parent != NONE {
                tree.dirs.entry(parent).or_default().push(id);
            }
        }
        tree.garbage = tree.names.len() - live;
        tree.recompute_totals();
        tree.maybe_compact();
        // Keep a little room for what changes next, and give the rest back.
        let (names, links) = (tree.names.len(), tree.links.len());
        tree.names.shrink_to(names + names / 16);
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
        tree.rec(n).unwrap().size
    }

    /// Folder totals kept incrementally must equal a recount from scratch.
    fn assert_totals(tree: &mut Tree) {
        let before: Vec<u64> = tree.recs.iter().map(|r| r.size).collect();
        tree.recompute_totals();
        let after: Vec<u64> = tree.recs.iter().map(|r| r.size).collect();
        assert_eq!(before, after);
    }

    fn find(tree: &Tree, query: &str, scope: u32, hidden: bool, excludes: &[&str]) -> Vec<String> {
        let matcher = NameMatcher::new(query).unwrap();
        let excludes = Excludes::new(&excludes.iter().map(|e| e.to_string()).collect::<Vec<_>>());
        let mut cache = HashMap::new();
        let mut paths: Vec<String> = tree
            .search(&Search { matcher: &matcher, scope, hidden, excludes: &excludes })
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
    fn builder_links_names_once_all_records_are_known() {
        let mut builder = Builder::new(8);
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
        assert_eq!(tree.rec(20).unwrap().size, 199_979 * 100 + 100);
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
