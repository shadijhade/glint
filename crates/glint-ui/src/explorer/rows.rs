//! The rows a tab shows: its listing sorted, with hidden items and the filter applied, optionally grouped.
//! Ported from the `sorted`, `listRows` and `grouped` memos in `Explorer.tsx` (the part that decides what is
//! on screen, which the file list and grid both need).
//!
//! **Cheap by construction.** A listing arrives presorted (folders first, natural name order), so the default
//! view is the listing itself: [`Rows`] then just shares the listing's `Arc` and holds no order at all. Any
//! other sort, a hidden item left out or a filter makes an *order*: a list of indices into the listing,
//! 4 bytes a row, instead of a second copy of every name and path. Only grouping copies entries (its
//! `group_rows` reorders them), and only while a list is grouped.
//!
//! [`build_rows`] is a pure function of its inputs so a huge folder can be ordered on a background thread.

use super::types::{Hit, Hits};
use glint_model::entries::{
    by_name, entry_order_by, is_hidden, matcher, matcher_marks, FileEntry, GitMarks, MarkReads, MarksOf, Reads,
    SizeOf, Sort, SortKey,
};
use glint_model::groups::{group_order_marked, GroupBy, GroupHeader, GroupedOrder, SizeOf as GroupSizeOf};
use glint_model::natural::NameKeys;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;

/// How many of a file's matching lines are shown under its row; the rest are counted on one more line.
pub const HITS_SHOWN: usize = 3;
/// Where the codes of the lines under rows start in [`GroupLayout::lines`]: line `j` of
/// [`GroupLayout::hit_lines`] is `HIT_BASE - j`. Far below any header's code (-1 - i).
const HIT_BASE: i32 = -1_000_000_000;

/// The folder sizes measured for the folder (`SizeRun.sizes`), when sorting or grouping by size needs them: a
/// folder sorts and groups by its calculated size. Equal when it is the same measuring (`version`), so a spec
/// compare never walks the map.
#[derive(Clone, Debug)]
pub struct RowSizes {
    /// Changes whenever the sizes do ([`SizeRun::version`](super::SizeRun)).
    pub version: u64,
    /// Subfolder name to bytes.
    pub sizes: Arc<HashMap<String, u64>>,
}

impl PartialEq for RowSizes {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

/// The Git marks of the folder (lowercase name to mark) that the `git:` filter and Group by Git read. Equal when they
/// are the very same map or say the same, so a spec compare of a repository's marks stays cheap when nothing changed.
#[derive(Clone, Debug)]
pub struct RowMarks {
    pub marks: Arc<GitMarks>,
}

impl PartialEq for RowMarks {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.marks, &other.marks) || self.marks == other.marks
    }
}

/// The marks of the tab's entries (by the path the listing gave) and this PC's labels, which a sort or a group by
/// a mark and the marks filters read (docs/gpui/marks, 6.1). Equal when they are the very same maps, so a spec
/// compare walks nothing.
#[derive(Clone, Debug)]
pub struct RowFileMarks {
    pub marks: Arc<std::collections::HashMap<String, glint_model::marks::Marks>>,
    pub labels: Arc<[glint_model::marks::Label]>,
}

impl PartialEq for RowFileMarks {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.marks, &other.marks) && Arc::ptr_eq(&self.labels, &other.labels)
    }
}

/// What a search found inside files (`inside.rs`), for the rows: which files were found by their contents alone,
/// and every file's matching lines. Equal when it is the very same findings, so a spec compare walks nothing.
#[derive(Clone, Debug)]
pub struct RowInside {
    pub only: Arc<HashSet<String>>,
    pub hits: Arc<Hits>,
    /// The files whose every line is shown, not the first [`HITS_SHOWN`] and a count of the rest.
    pub expanded: Arc<HashSet<String>>,
    /// The rows show the lines and the two parts' headers (the list does; the grid and the columns view show the
    /// files alone).
    pub lines: bool,
}

impl PartialEq for RowInside {
    fn eq(&self, other: &Self) -> bool {
        self.lines == other.lines
            && Arc::ptr_eq(&self.only, &other.only)
            && Arc::ptr_eq(&self.hits, &other.hits)
            && Arc::ptr_eq(&self.expanded, &other.expanded)
    }
}

/// Active OKLab color filter and extracted image palettes for 120 FPS photo surfing.
#[derive(Clone, Debug)]
pub struct RowColor {
    pub filter: glint_model::oklab::ColorFilter,
    pub palettes: Arc<std::collections::HashMap<String, glint_model::oklab::Palette3>>,
    /// Monotonic revision bumped when a tab's palette map changes (0 falls back to `Arc::ptr_eq`).
    pub rev: u64,
}

impl PartialEq for RowColor {
    fn eq(&self, other: &Self) -> bool {
        self.filter == other.filter
            && self.rev == other.rev
            && (self.rev != 0 || Arc::ptr_eq(&self.palettes, &other.palettes))
    }
}

/// Everything that decides which rows a listing makes, and in what order. Two equal specs make equal rows.
#[derive(Clone, Debug, PartialEq)]
pub struct RowSpec {
    pub sort: Sort,
    /// Settings → Files → Folders first.
    pub folders_first: bool,
    /// Settings → Files → Show hidden items.
    pub show_hidden: bool,
    /// The search box's text, narrowing what's shown (`matcher`: terms, `ext:pdf`, `size:>10mb`…).
    pub filter: String,
    /// Group by headers. Only the list shows them: the grid and columns views pass [`GroupBy::None`].
    pub group: GroupBy,
    /// Whether the entries arrived presorted from the engine (a listing does; a search's results don't).
    pub presorted: bool,
    /// The measured folder sizes, set only while the sort or the grouping is by size.
    pub sizes: Option<RowSizes>,
    /// The folder's Git marks, set only while the filter holds `git:` or the list is grouped by Git (no search
    /// results: they have none), so a change of marks rebuilds only the rows that read them.
    pub git: Option<RowMarks>,
    /// Compare panes' "diff only": the pane's comparison, whose identical items the rows leave out.
    pub diffs: Option<super::compare::RowDiffs>,
    /// A search's findings inside files: the files found by their contents alone stay whatever the box's words say
    /// of their names; in the list their lines go under their rows, and those files under a header of their own.
    pub inside: Option<RowInside>,
    /// Active OKLab color filter and cached palettes for sorting matching photos closest-first.
    pub color: Option<RowColor>,
    /// The entries' marks, set only while the filter holds a marks key or the sort or the group is by a mark, and
    /// once every row's marks are read (`explorer::marked`).
    pub marked: Option<RowFileMarks>,
}

impl Default for RowSpec {
    fn default() -> Self {
        RowSpec {
            sort: Sort {
                key: SortKey::Name,
                desc: false,
            },
            folders_first: true,
            show_hidden: false,
            filter: String::new(),
            group: GroupBy::None,
            presorted: true,
            sizes: None,
            git: None,
            diffs: None,
            inside: None,
            color: None,
            marked: None,
        }
    }
}

/// A line under a file's row: one of the lines in it that matched a search, or how many more there are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HitLine {
    /// The row it is under.
    pub row: u32,
    /// Which of the file's matching lines it is (the first one not shown, for a "more" line).
    pub hit: u32,
    /// Above zero this is not a line of the file but the count of its matching lines that aren't shown.
    pub more: u32,
}

/// The headers of the parts a search's results are in when it found files by their contents or by a comment or a tag:
/// by name (none when no name matched), in comments (none when none was found so), then inside files (`usize::MAX`
/// when no file was found so). Indices into [`GroupLayout::headers`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sections {
    pub names: Option<usize>,
    pub comments: Option<usize>,
    pub inside: usize,
}

/// The headers of a grouped list and where its lines are (`Grouped` without its copy of the rows).
#[derive(Clone, Debug, PartialEq)]
pub struct GroupLayout {
    pub headers: Vec<GroupHeader>,
    /// Each list line: a row's index, or (as -1 - i) header i, or a line under a row (see [`Rows::line`]).
    pub lines: Vec<i32>,
    /// Each row's line.
    pub line_of: Vec<i32>,
    /// The lines under rows: what a search found inside those files.
    pub hit_lines: Vec<HitLine>,
    /// Set when the headers are a search's two parts rather than a grouping.
    pub sections: Option<Sections>,
}

/// What is on one line of the list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Line<'a> {
    /// Row `i`.
    Row(usize),
    /// Header `i`.
    Header(usize, &'a GroupHeader),
    /// A line under a row: a line of the file that matched the search, or the count of the ones not shown.
    Hit(HitLine),
    /// Nothing (past the end).
    Blank,
}

/// A tab's rows, in display order. Cloning is cheap (a few `Arc`s).
#[derive(Clone, Debug)]
pub struct Rows {
    entries: Arc<[FileEntry]>,
    /// Display order as indices into `entries`; `None` means `entries` as they are.
    order: Option<Arc<[u32]>>,
    groups: Option<Arc<GroupLayout>>,
    /// Every file's lines that matched the search, when the rows show them.
    inside: Option<Arc<Hits>>,
    /// How many items the Show hidden items setting left out.
    pub hidden: usize,
    /// The row [`Rows::position_of`] found last.
    found: Hint,
}

/// The row a path was last found at. The focus, the anchor and the row a key moves to are each looked up a few
/// times for every key press, and a scan compares paths over the whole list (in a folder of 100,000, each one is
/// felt); the row found last, or one beside it, is nearly always the one asked for. A copy starts from the same row.
#[derive(Debug, Default)]
struct Hint(AtomicUsize);

impl Clone for Hint {
    fn clone(&self) -> Self {
        Hint(AtomicUsize::new(self.0.load(AtomicOrdering::Relaxed)))
    }
}

impl Default for Rows {
    fn default() -> Self {
        Rows::empty()
    }
}

impl Rows {
    /// No rows.
    pub fn empty() -> Self {
        Rows {
            entries: Arc::from(Vec::new()),
            order: None,
            groups: None,
            inside: None,
            hidden: 0,
            found: Hint::default(),
        }
    }

    /// The listing's own entries, in their order.
    pub fn identity(entries: Arc<[FileEntry]>) -> Self {
        Rows {
            entries,
            order: None,
            groups: None,
            inside: None,
            hidden: 0,
            found: Hint::default(),
        }
    }

    /// How many lines the list has: its rows, its headers and the lines under rows.
    pub fn line_count(&self) -> usize {
        self.groups.as_ref().map_or(self.len(), |groups| groups.lines.len())
    }

    /// What is on line `line` of the list.
    pub fn line(&self, line: usize) -> Line<'_> {
        let Some(groups) = &self.groups else {
            return if line < self.len() {
                Line::Row(line)
            } else {
                Line::Blank
            };
        };
        match groups.lines.get(line).copied() {
            Some(at) if at >= 0 => Line::Row(at as usize),
            Some(at) if at > HIT_BASE => {
                let index = (-at - 1) as usize;
                groups
                    .headers
                    .get(index)
                    .map_or(Line::Blank, |header| Line::Header(index, header))
            }
            Some(at) => groups
                .hit_lines
                .get((HIT_BASE - at) as usize)
                .map_or(Line::Blank, |hit| Line::Hit(*hit)),
            None => Line::Blank,
        }
    }

    /// The lines of the file at `path` that matched the search, when the rows show them.
    pub fn hits_of(&self, path: &str) -> Option<&Arc<[Hit]>> {
        self.inside.as_ref()?.get(path)
    }

    /// The file a line under a row belongs to, and the line of it that matched (the first one not shown, for a
    /// "more" line).
    pub fn hit(&self, line: HitLine) -> Option<(&FileEntry, &Hit)> {
        let entry = self.get(line.row as usize)?;
        let hit = self.hits_of(&entry.path)?.get(line.hit as usize)?;
        Some((entry, hit))
    }

    /// Whether the rows are a search's two parts (by name, inside files), and which headers those are.
    pub fn sections(&self) -> Option<Sections> {
        self.groups.as_ref()?.sections
    }

    /// Number of rows (not counting group headers).
    pub fn len(&self) -> usize {
        self.order.as_ref().map_or(self.entries.len(), |o| o.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Row `i`, in display order.
    pub fn get(&self, i: usize) -> Option<&FileEntry> {
        match &self.order {
            Some(order) => self.entries.get(*order.get(i)? as usize),
            None => self.entries.get(i),
        }
    }

    /// Every row in display order.
    pub fn iter(&self) -> impl Iterator<Item = &FileEntry> + '_ {
        (0..self.len()).filter_map(|i| self.get(i))
    }

    /// The index of the row with this path (`findIndex`), compared exactly. The row found last and the two beside it
    /// are tried first (a tab's rows never hold a path twice), then every row.
    pub fn position_of(&self, path: &str) -> Option<usize> {
        let last = self.found.0.load(AtomicOrdering::Relaxed);
        let near = [last, last.wrapping_add(1), last.wrapping_sub(1)]
            .into_iter()
            .find(|&at| self.get(at).is_some_and(|e| e.path == path));
        let at = near.or_else(|| self.iter().position(|e| e.path == path))?;
        self.found.0.store(at, AtomicOrdering::Relaxed);
        Some(at)
    }

    /// The index of the row at this location: [`Rows::position_of`], and when the path isn't spelled as the row's is
    /// (another case, a closing separator), the row that is the same place. That second look reads every row, so
    /// it is not made in a listing too long for it to go unnoticed.
    pub fn row_of(&self, path: &str) -> Option<usize> {
        /// The most rows read one by one for a path spelled another way.
        const MOST_COMPARED: usize = 5_000;
        self.position_of(path).or_else(|| {
            (self.len() <= MOST_COMPARED)
                .then(|| {
                    self.iter()
                        .position(|entry| glint_model::paths::same_path(&entry.path, path))
                })
                .flatten()
        })
    }

    /// Whether `other` is these very rows: the same entries in the same order (shared, not compared).
    pub fn same_lists(&self, other: &Rows) -> bool {
        let same_order = match (&self.order, &other.order) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        Arc::ptr_eq(&self.entries, &other.entries) && same_order
    }

    /// Group headers and lines when the list is grouped.
    pub fn groups(&self) -> Option<&GroupLayout> {
        self.groups.as_deref()
    }

    /// Whether these rows are the listing's own entries untouched (the fast, default case).
    #[cfg(test)]
    pub fn is_identity(&self) -> bool {
        self.order.is_none() && self.groups.is_none()
    }

    /// The entries the rows are drawn from, shared: the listing's own, or the grouped copy.
    pub(super) fn shared(&self) -> &Arc<[FileEntry]> {
        &self.entries
    }

    /// The rows of a selection, in display order.
    ///
    /// A few selected items are each looked for by their own path (a compare a row, most settled by the lengths
    /// alone); many are found in one pass over the rows that ends at the last of them. Neither hashes every row's
    /// path to ask the selection about it, which cost 2.5 ms for each press of an arrow key in a folder of 100,000.
    pub fn chosen<'a>(&'a self, selection: &super::tab::Selection) -> Vec<&'a FileEntry> {
        const FEW: usize = 4;
        let wanted = selection.len();
        if wanted == 0 {
            return Vec::new();
        }
        if wanted <= FEW {
            let mut found: Vec<(usize, &FileEntry)> = selection
                .iter()
                .filter_map(|path| {
                    let at = self.position_of(path)?;
                    Some((at, self.get(at)?))
                })
                .collect();
            found.sort_by_key(|(at, _)| *at);
            return found.into_iter().map(|(_, entry)| entry).collect();
        }
        let mut found = Vec::with_capacity(wanted.min(self.len()));
        for entry in self.iter() {
            if selection.contains(&entry.path) {
                found.push(entry);
                if found.len() == wanted {
                    break;
                }
            }
        }
        found
    }
}

/// The key a photo's palette is stored under: its path in lower case (`color_index`'s keys). Built in `buf`, which is
/// reused, so a pass over a listing allocates once; non-ASCII paths take `to_lowercase`'s own rules.
pub(crate) fn palette_key<'a>(path: &str, buf: &'a mut String) -> &'a str {
    buf.clear();
    if path.is_ascii() {
        buf.push_str(path);
        buf.make_ascii_lowercase();
    } else {
        buf.push_str(&path.to_lowercase());
    }
    buf
}

/// Whether a filter or a grouping reads the folder's Git marks (`git:` in the box, Group by Git).
pub fn reads_git(filter: &str, group: GroupBy) -> bool {
    group == GroupBy::Git || filter.to_lowercase().contains("git:")
}

/// Whether a filter, a sort or a grouping reads the entries' marks (`tag:`… in the box, a sort by label, tags or
/// comment, Group by Label): only then are the marks read and handed to the rows.
pub fn reads_marks(filter: &str, sort: Sort, group: GroupBy) -> bool {
    group == GroupBy::Label
        || glint_model::columns::MARK_SORTS.contains(&sort.key)
        || glint_model::entries::has_marks_filter(filter)
}

/// An entry's marks out of the map handed in (by its path); one not in it has none known.
fn marks_reader(map: &std::collections::HashMap<String, glint_model::marks::Marks>) -> Box<MarksOf<'_>> {
    Box::new(move |entry| map.get(&entry.path))
}

/// Makes the rows for `entries` under `spec`. `now` is the time for Group by date (ms since the epoch).
///
/// The same steps as the TypeScript's three memos, in order: sort (skipped for the engine's own order),
/// hide hidden items, narrow by the filter, then group.
pub fn build_rows(entries: &Arc<[FileEntry]>, spec: &RowSpec, now: i64) -> Rows {
    build_rows_while(entries, spec, now, &|| true)
}

/// [`build_rows`] for as long as `wanted` says so: a big folder's rows are made on a background thread, and the box
/// has other text before a slow build is done (every key typed starts one). Asked every few thousand entries and
/// before the sort; once it says no, what comes back is the listing as it is, to be thrown away.
pub fn build_rows_while(entries: &Arc<[FileEntry]>, spec: &RowSpec, now: i64, wanted: &dyn Fn() -> bool) -> Rows {
    let by_default = spec.presorted && spec.folders_first && spec.sort.key == SortKey::Name && !spec.sort.desc;
    // `matcher` is `None` for text that filters nothing (empty, or asking the models).
    let marks = spec.git.as_ref().map(|git| &*git.marks);
    let of = spec.marked.as_ref().map(|m| marks_reader(&m.marks));
    let mark_reads = spec.marked.as_ref().zip(of.as_deref()).map(|(m, of)| MarkReads { of, labels: &m.labels });
    let test = match &of {
        Some(of) => matcher_marks(&spec.filter, marks, None, Some(&**of), now),
        None => matcher(&spec.filter, marks, None),
    };
    let mut hidden = 0usize;
    let diffs = spec.diffs.as_ref().map(|d| &d.comparison.diffs);
    let max_color_dist_sq = spec.color.as_ref().map(|rc| rc.filter.max_dist * rc.filter.max_dist);
    let narrowing = test.is_some() || !spec.show_hidden || diffs.is_some() || spec.color.is_some();

    let mut order: Option<Vec<u32>> = None;
    // With a color filter every kept entry's distance to the target, measured once here, so ranking is a sort of
    // plain numbers (no lookups or allocations in the comparator).
    let mut distances: Vec<f32> = Vec::new();
    let mut key_buf = String::new();
    if narrowing {
        let mut kept = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            if i % 8192 == 0 && !wanted() {
                return Rows::identity(entries.clone());
            }
            if !spec.show_hidden && is_hidden(entry) {
                hidden += 1;
                continue;
            }
            if diffs.is_some_and(|d| d.get(&entry.lower) == Some(&glint_model::entries::DiffStatus::Same)) {
                continue;
            }
            let mut distance = 0.0f32;
            if let (Some(rc), Some(max_sq)) = (&spec.color, max_color_dist_sq) {
                if entry.dir {
                    continue;
                }
                let Some(pal) = rc.palettes.get(palette_key(&entry.path, &mut key_buf)) else {
                    continue;
                };
                let dist = rc.filter.score(pal);
                if dist > max_sq {
                    continue;
                }
                distance = dist;
            }
            // The box's words are looked for in names here; a file found by what is in it needn't be named so
            // (the search's filters were applied to it when it was found).
            let by_contents = spec
                .inside
                .as_ref()
                .is_some_and(|inside| inside.only.contains(&entry.path));
            if !by_contents && test.as_ref().is_some_and(|test| !test(entry)) {
                continue;
            }
            kept.push(u32::try_from(i).unwrap_or(u32::MAX));
            if spec.color.is_some() {
                distances.push(distance);
            }
        }
        // Nothing was left out and the order is the engine's: the listing is the rows.
        if kept.len() != entries.len() || !by_default || spec.color.is_some() {
            order = Some(kept);
        }
    } else if !by_default {
        order = Some(
            (0..entries.len())
                .map(|i| u32::try_from(i).unwrap_or(u32::MAX))
                .collect(),
        );
    }

    // A folder sorts by its measured size, one it has none for before every other (the TypeScript's -1): every
    // size is one more than it is, so "none" is 0 and still sorts below an empty folder.
    let measured = spec.sizes.as_ref().map(|s| {
        let sizes = s.sizes.clone();
        move |e: &FileEntry| -> u64 {
            if e.dir {
                sizes.get(&e.name).map_or(0, |size| size.saturating_add(1))
            } else {
                e.size.saturating_add(1)
            }
        }
    });
    if spec.color.is_some() {
        if let Some(order) = order.as_mut() {
            // Closest first; the sort is stable, so equals keep the listing's order.
            let mut ranked: Vec<(u32, f32)> = order.iter().copied().zip(distances.iter().copied()).collect();
            ranked.sort_by(|a, b| a.1.total_cmp(&b.1));
            order.clear();
            order.extend(ranked.into_iter().map(|(i, _)| i));
        }
    } else if !by_default {
        if !wanted() {
            return Rows::identity(entries.clone());
        }
        if let Some(order) = order.as_mut() {
            let size_of = measured.as_ref().map(|f| f as &SizeOf);
            sort_order(entries, order, spec, size_of, mark_reads);
        }
    }

    let sorted = Rows {
        entries: entries.clone(),
        order: order.map(Arc::from),
        groups: None,
        inside: None,
        hidden,
        found: Hint::default(),
    };
    if spec.group == GroupBy::None {
        return match spec.inside.as_ref().filter(|inside| inside.lines) {
            Some(inside) => with_hits(sorted, inside),
            None => sorted,
        };
    }
    // Grouped as an order over the listing's own entries: no row is copied.
    let shown: Vec<&FileEntry> = sorted.iter().collect();
    let group_size = spec.sizes.as_ref().map(|s| {
        let sizes = s.sizes.clone();
        move |e: &FileEntry| -> Option<u64> { sizes.get(&e.name).copied() }
    });
    let GroupedOrder {
        order: picked,
        headers,
        lines,
        line_of,
    } = group_order_marked(
        &shown,
        spec.group,
        now,
        group_size.as_ref().map(|f| f as &GroupSizeOf),
        marks,
        mark_reads,
    );
    let in_entries = |at: usize| -> u32 {
        match &sorted.order {
            Some(order) => order.get(at).copied().unwrap_or(u32::MAX),
            None => u32::try_from(at).unwrap_or(u32::MAX),
        }
    };
    let order: Arc<[u32]> = picked.into_iter().map(in_entries).collect();
    let grouped = Rows {
        entries: entries.clone(),
        order: Some(order),
        groups: Some(Arc::new(GroupLayout {
            headers,
            lines,
            line_of,
            hit_lines: Vec::new(),
            sections: None,
        })),
        inside: None,
        hidden,
        found: Hint::default(),
    };
    match spec.inside.as_ref().filter(|inside| inside.lines) {
        Some(inside) => with_hits(grouped, inside),
        None => grouped,
    }
}

/// Puts `order` (positions in `entries`) in the order of `spec`'s sort. A stable sort, so ties keep the engine's
/// name order, as the TypeScript's did.
///
/// A sort that compares names (by name, or any sort whose ties go by name) makes every name's natural-order key
/// once and compares those: reading each name's digit runs again at every comparison cost most of the time of
/// sorting a big folder.
fn sort_order(
    entries: &[FileEntry],
    order: &mut Vec<u32>,
    spec: &RowSpec,
    size_of: Option<&SizeOf>,
    marks: Option<MarkReads>,
) {
    /// A row on its way into place: its entry, where it is in `entries`, and where it started in the sort.
    struct Slot<'e> {
        entry: &'e FileEntry,
        index: u32,
        start: usize,
    }
    let mut slots: Vec<Slot> = order
        .iter()
        .filter_map(|&index| entries.get(index as usize).map(|entry| (index, entry)))
        .enumerate()
        .map(|(start, (index, entry))| Slot { entry, index, start })
        .collect();
    let by_names = !spec.presorted || spec.sort.key == SortKey::Name;
    let keys = if by_names {
        NameKeys::new(slots.iter().map(|slot| slot.entry.lower.as_str()))
    } else {
        None
    };
    let name_order = |a: &Slot, b: &Slot| match &keys {
        Some(keys) => keys
            .compare(a.start, b.start)
            .then_with(|| a.entry.name.cmp(&b.entry.name)),
        None => by_name(a.entry, b.entry),
    };
    let reads = Reads {
        size_of,
        taken_of: None,
        facts_of: None,
        marks,
    };
    let compare = entry_order_by(
        spec.sort,
        spec.folders_first,
        spec.presorted,
        reads,
        |slot: &Slot| slot.entry,
        name_order,
    );
    slots.sort_by(compare);
    *order = slots.iter().map(|slot| slot.index).collect();
}

/// `rows` with what a search found inside files: each file's matching lines under its row (the first
/// [`HITS_SHOWN`], then a count of the rest). Rows that aren't grouped are put in the search's parts, each under
/// its header: the files found by name (in their order), then the ones found by a comment or a tag (`in_comments.rs`
/// marks them), then the ones found by their contents alone. A grouped list keeps its groups.
fn with_hits(rows: Rows, inside: &RowInside) -> Rows {
    let count = rows.len();
    let index_of = |i: usize| -> u32 {
        match &rows.order {
            Some(order) => order.get(i).copied().unwrap_or(u32::MAX),
            None => u32::try_from(i).unwrap_or(u32::MAX),
        }
    };
    // Where each row goes, and the headers between them: (row or header) in display order.
    enum Slot {
        Header(usize),
        Row(usize),
    }
    let mut headers: Vec<GroupHeader> = Vec::new();
    let mut slots: Vec<Slot> = Vec::new();
    let mut order: Option<Arc<[u32]>> = rows.order.clone();
    let mut sections = None;
    match rows.groups.as_deref() {
        Some(layout) => {
            headers = layout.headers.clone();
            for &at in &layout.lines {
                if at >= 0 {
                    slots.push(Slot::Row(at as usize));
                } else if at > HIT_BASE {
                    slots.push(Slot::Header((-at - 1) as usize));
                }
            }
        }
        None => {
            let (mut named, mut commented, mut found): (Vec<u32>, Vec<u32>, Vec<u32>) =
                (Vec::new(), Vec::new(), Vec::new());
            for i in 0..count {
                let entry = rows.get(i);
                let by_contents = entry.is_some_and(|entry| inside.only.contains(&entry.path));
                let by_comment =
                    by_contents && entry.is_some_and(|entry| inside.only.contains(&super::in_comments::key(&entry.path)));
                if by_comment {
                    commented.push(index_of(i));
                } else if by_contents {
                    found.push(index_of(i));
                } else {
                    named.push(index_of(i));
                }
            }
            if found.is_empty() && commented.is_empty() {
                slots.extend((0..count).map(Slot::Row));
            } else {
                // Each part: its header (when it has rows), then its rows, which follow one another in the new order.
                let mut part = |label: &str, rows: usize, first: usize| -> Option<usize> {
                    if rows == 0 {
                        return None;
                    }
                    headers.push(GroupHeader {
                        label: label.to_string(),
                        count: rows,
                        size: 0,
                        first,
                    });
                    slots.push(Slot::Header(headers.len() - 1));
                    slots.extend((first..first + rows).map(Slot::Row));
                    Some(headers.len() - 1)
                };
                let names = part("Names", named.len(), 0);
                let comments = part("In comments", commented.len(), named.len());
                let inside_files = part("Inside files", found.len(), named.len() + commented.len());
                sections = Some(Sections {
                    names,
                    comments,
                    inside: inside_files.unwrap_or(usize::MAX),
                });
                named.append(&mut commented);
                named.append(&mut found);
                order = Some(Arc::from(named));
            }
        }
    }
    let placed = Rows {
        entries: rows.entries.clone(),
        order,
        groups: None,
        inside: None,
        hidden: rows.hidden,
        found: Hint::default(),
    };
    let mut lines: Vec<i32> = Vec::with_capacity(slots.len());
    let mut line_of: Vec<i32> = vec![0; count];
    let mut hit_lines: Vec<HitLine> = Vec::new();
    for slot in slots {
        match slot {
            Slot::Header(i) => lines.push(-1 - i32::try_from(i).unwrap_or(0)),
            Slot::Row(row) => {
                if let Some(line) = line_of.get_mut(row) {
                    *line = i32::try_from(lines.len()).unwrap_or(i32::MAX);
                }
                lines.push(i32::try_from(row).unwrap_or(i32::MAX));
                let hits = placed
                    .get(row)
                    .and_then(|entry| inside.hits.get(&entry.path))
                    .map_or(0, |hits| hits.len());
                let shown = if placed
                    .get(row)
                    .is_some_and(|entry| inside.expanded.contains(&entry.path))
                {
                    hits
                } else {
                    hits.min(HITS_SHOWN)
                };
                let row_number = u32::try_from(row).unwrap_or(u32::MAX);
                for hit in 0..shown {
                    lines.push(HIT_BASE - i32::try_from(hit_lines.len()).unwrap_or(0));
                    hit_lines.push(HitLine {
                        row: row_number,
                        hit: u32::try_from(hit).unwrap_or(0),
                        more: 0,
                    });
                }
                if hits > shown {
                    lines.push(HIT_BASE - i32::try_from(hit_lines.len()).unwrap_or(0));
                    hit_lines.push(HitLine {
                        row: row_number,
                        hit: u32::try_from(shown).unwrap_or(0),
                        more: u32::try_from(hits - shown).unwrap_or(u32::MAX),
                    });
                }
            }
        }
    }
    // Nothing to add: the rows as they were (a search whose files all went, say).
    if hit_lines.is_empty() && sections.is_none() {
        return rows;
    }
    Rows {
        groups: Some(Arc::new(GroupLayout {
            headers,
            lines,
            line_of,
            hit_lines,
            sections,
        })),
        inside: Some(inside.hits.clone()),
        ..placed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::entries::{attr, with_details, RawEntry};

    #[test]
    fn a_path_is_found_where_it_is_whichever_row_was_found_last() {
        let entries: Arc<[FileEntry]> = (0..50).map(|i| file(&format!("f{i:02}.txt"), 1, 1)).collect();
        let rows = Rows::identity(entries);
        assert_eq!(rows.position_of("C:\\x\\f10.txt"), Some(10));
        // Beside the last one, either way, far from it, and not there at all.
        assert_eq!(rows.position_of("C:\\x\\f11.txt"), Some(11));
        assert_eq!(rows.position_of("C:\\x\\f10.txt"), Some(10));
        assert_eq!(rows.position_of("C:\\x\\f49.txt"), Some(49));
        assert_eq!(rows.position_of("C:\\x\\f00.txt"), Some(0));
        assert_eq!(rows.position_of("C:\\x\\nope.txt"), None);
        assert_eq!(rows.position_of("C:\\x\\f01.txt"), Some(1));
        // A copy, and rows in another order, answer the same.
        let copy = rows.clone();
        assert_eq!(copy.position_of("C:\\x\\f02.txt"), Some(2));
        assert_eq!(Rows::empty().position_of("C:\\x\\f00.txt"), None);
    }

    fn file(name: &str, size: u64, modified: i64) -> FileEntry {
        with_details(
            RawEntry {
                name: name.into(),
                size,
                modified,
                ..Default::default()
            },
            "C:\\x",
        )
    }

    fn folder(name: &str) -> FileEntry {
        with_details(
            RawEntry {
                name: name.into(),
                dir: true,
                attrs: attr::DIRECTORY,
                ..Default::default()
            },
            "C:\\x",
        )
    }

    fn hidden(name: &str) -> FileEntry {
        with_details(
            RawEntry {
                name: name.into(),
                attrs: attr::HIDDEN,
                ..Default::default()
            },
            "C:\\x",
        )
    }

    #[test]
    fn the_rows_of_a_selection_come_in_display_order_few_or_many() {
        use crate::explorer::Selection;
        let entries: Arc<[FileEntry]> =
            Arc::from((0..40).map(|i| file(&format!("f{i:02}.txt"), i, 0)).collect::<Vec<_>>());
        // By size, biggest first: the display order is the listing's backwards.
        let spec = RowSpec {
            sort: Sort {
                key: SortKey::Size,
                desc: true,
            },
            ..RowSpec::default()
        };
        let rows = build_rows(&entries, &spec, 0);
        let names = |selection: &Selection| -> Vec<String> {
            rows.chosen(selection).iter().map(|entry| entry.name.clone()).collect()
        };
        assert!(names(&Selection::of([])).is_empty());
        // A few: each found by its own path, then put in the rows' order.
        let few = Selection::of(["C:\\x\\f03.txt", "C:\\x\\f30.txt", "C:\\x\\gone.txt"]);
        assert_eq!(names(&few), ["f30.txt", "f03.txt"]);
        // Many: one pass over the rows, which ends at the last of them.
        let paths: Vec<String> = (10..20).map(|i| format!("C:\\x\\f{i}.txt")).collect();
        let many = Selection::of(paths.iter().map(String::as_str));
        let found = names(&many);
        assert_eq!(found.len(), 10);
        assert_eq!(found.first().map(String::as_str), Some("f19.txt"));
        assert_eq!(found.last().map(String::as_str), Some("f10.txt"));
    }

    /// A listing as the engine sends it: folders first, natural name order.
    fn listing() -> Arc<[FileEntry]> {
        Arc::from(vec![
            folder("docs"),
            folder("src"),
            file("a2.txt", 30, 300),
            file("a10.txt", 10, 100),
            hidden(".secret"),
            file("zed.md", 20, 200),
        ])
    }

    fn names(rows: &Rows) -> Vec<String> {
        rows.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn the_default_view_is_the_listing_itself_with_no_order_built() {
        let entries = listing();
        let spec = RowSpec {
            show_hidden: true,
            ..Default::default()
        };
        let rows = build_rows(&entries, &spec, 0);
        assert!(rows.is_identity());
        assert_eq!(rows.len(), 6);
        assert_eq!(rows.hidden, 0);
    }

    #[test]
    fn hidden_items_are_left_out_and_counted() {
        let rows = build_rows(&listing(), &RowSpec::default(), 0);
        assert_eq!(names(&rows), ["docs", "src", "a2.txt", "a10.txt", "zed.md"]);
        assert_eq!(rows.hidden, 1);
        assert!(!rows.is_identity());
    }

    #[test]
    fn sorting_matches_sort_entries_which_the_web_list_used() {
        let entries = listing();
        for (key, desc, folders_first) in [
            (SortKey::Size, false, true),
            (SortKey::Size, true, true),
            (SortKey::Modified, true, false),
            (SortKey::Type, false, true),
            (SortKey::Name, true, true),
            (SortKey::Name, false, false),
        ] {
            let sort = Sort { key, desc };
            let spec = RowSpec {
                sort,
                folders_first,
                show_hidden: true,
                ..Default::default()
            };
            let ours = names(&build_rows(&entries, &spec, 0));
            let theirs: Vec<String> =
                glint_model::entries::sort_entries(entries.to_vec(), sort, folders_first, true, None, None, None)
                    .into_iter()
                    .map(|e| e.name)
                    .collect();
            assert_eq!(ours, theirs, "{key:?} desc={desc} folders_first={folders_first}");
        }
    }

    #[test]
    fn sort_by_size_descending_puts_folders_first_then_the_biggest() {
        let spec = RowSpec {
            sort: Sort {
                key: SortKey::Size,
                desc: true,
            },
            show_hidden: true,
            ..Default::default()
        };
        let rows = build_rows(&listing(), &spec, 0);
        assert_eq!(names(&rows), ["docs", "src", "a2.txt", "zed.md", "a10.txt", ".secret"]);
    }

    #[test]
    fn the_filter_narrows_and_keeps_the_engines_order() {
        let spec = RowSpec {
            filter: "a".into(),
            show_hidden: true,
            ..Default::default()
        };
        let rows = build_rows(&listing(), &spec, 0);
        assert_eq!(names(&rows), ["a2.txt", "a10.txt"]);
        assert_eq!(rows.position_of("C:\\x\\a10.txt"), Some(1));
        assert_eq!(rows.get(5), None);
    }

    #[test]
    fn folders_sort_by_their_measured_size_and_one_with_none_comes_before_an_empty_one() {
        let entries: Arc<[FileEntry]> =
            Arc::from(vec![folder("big"), folder("none"), folder("empty"), folder("small")]);
        let sizes = RowSizes {
            version: 1,
            sizes: Arc::new(HashMap::from([
                ("big".to_string(), 9000),
                ("small".to_string(), 5),
                ("empty".to_string(), 0),
            ])),
        };
        let spec = RowSpec {
            sort: Sort {
                key: SortKey::Size,
                desc: false,
            },
            show_hidden: true,
            sizes: Some(sizes.clone()),
            ..Default::default()
        };
        assert_eq!(
            names(&build_rows(&entries, &spec, 0)),
            ["none", "empty", "small", "big"]
        );
        let descending = RowSpec {
            sort: Sort {
                key: SortKey::Size,
                desc: true,
            },
            ..spec.clone()
        };
        assert_eq!(
            names(&build_rows(&entries, &descending, 0)),
            ["big", "small", "empty", "none"]
        );
        // Without the sizes every folder is 0 and the engine's name order stays.
        let unmeasured = RowSpec { sizes: None, ..spec };
        assert_eq!(
            names(&build_rows(&entries, &unmeasured, 0)),
            ["big", "none", "empty", "small"]
        );
    }

    #[test]
    fn grouping_by_size_puts_a_measured_folder_in_its_size_group() {
        let entries: Arc<[FileEntry]> = Arc::from(vec![folder("big"), folder("unmeasured"), file("a.txt", 20, 1)]);
        let spec = RowSpec {
            group: GroupBy::Size,
            show_hidden: true,
            sizes: Some(RowSizes {
                version: 1,
                sizes: Arc::new(HashMap::from([("big".to_string(), 200 << 20)])),
            }),
            ..Default::default()
        };
        let rows = build_rows(&entries, &spec, 0);
        let labels: Vec<String> = rows
            .groups()
            .map(|g| g.headers.iter().map(|h| h.label.clone()).collect())
            .unwrap_or_default();
        assert!(
            labels.iter().any(|l| l == "Folders"),
            "{labels:?}: an unmeasured folder stays a plain folder"
        );
        assert!(labels.iter().any(|l| l.starts_with("Large")), "{labels:?}");
    }

    #[test]
    fn grouping_orders_rows_group_by_group_with_headers() {
        let spec = RowSpec {
            group: GroupBy::Kind,
            show_hidden: true,
            ..Default::default()
        };
        let rows = build_rows(&listing(), &spec, 0);
        let layout = rows.groups().map(|g| g.headers.len());
        assert!(
            layout.is_some_and(|n| n >= 2),
            "folders and documents/other make groups"
        );
        assert_eq!(rows.len(), 6);
        // Every row is on a line, and the lines hold one entry per row plus one per header.
        let groups = rows.groups().map(|g| (g.lines.len(), g.headers.len()));
        assert_eq!(groups.map(|(lines, headers)| lines - headers), Some(6));
    }

    // ---- Git ----------------------------------------------------------------------------------------------

    fn marks() -> RowMarks {
        use glint_model::entries::GitMark;
        RowMarks {
            marks: Arc::new(
                [
                    ("a2.txt".to_string(), GitMark::Modified),
                    ("zed.md".to_string(), GitMark::Untracked),
                    ("src".to_string(), GitMark::Conflicted),
                    ("a10.txt".to_string(), GitMark::Ignored),
                ]
                .into_iter()
                .collect(),
            ),
        }
    }

    #[test]
    fn the_git_filter_keeps_the_items_git_marked_and_matches_nothing_without_marks() {
        let spec = |filter: &str, git: Option<RowMarks>| RowSpec {
            filter: filter.into(),
            show_hidden: true,
            git,
            ..Default::default()
        };
        assert_eq!(
            names(&build_rows(&listing(), &spec("git:modified", Some(marks())), 0)),
            ["a2.txt"]
        );
        assert_eq!(
            names(&build_rows(&listing(), &spec("git:new", Some(marks())), 0)),
            ["zed.md"]
        );
        assert_eq!(
            names(&build_rows(&listing(), &spec("git:conflicted", Some(marks())), 0)),
            ["src"]
        );
        assert_eq!(
            names(&build_rows(&listing(), &spec("git:ignored", Some(marks())), 0)),
            ["a10.txt"]
        );
        assert_eq!(
            names(&build_rows(&listing(), &spec("git:changed", Some(marks())), 0)),
            ["src", "a2.txt", "zed.md"],
            "changed is everything Git has something to say about, but what it leaves out"
        );
        assert!(
            build_rows(&listing(), &spec("git:changed", None), 0).is_empty(),
            "outside a repository a git: filter matches nothing"
        );
    }

    #[test]
    fn grouping_by_git_puts_each_item_under_its_mark() {
        let spec = RowSpec {
            group: GroupBy::Git,
            show_hidden: true,
            git: Some(marks()),
            ..Default::default()
        };
        let rows = build_rows(&listing(), &spec, 0);
        let labels: Vec<String> = rows
            .groups()
            .map(|g| g.headers.iter().map(|h| h.label.to_string()).collect())
            .unwrap_or_default();
        assert_eq!(labels, ["Conflicts", "Changed", "New", "Unchanged", "Ignored"]);
        assert_eq!(
            names(&rows),
            ["src", "a2.txt", "zed.md", "docs", ".secret", "a10.txt"],
            "group by group, each keeping the sort"
        );
        // Without marks everything is unchanged.
        let plain = build_rows(&listing(), &RowSpec { git: None, ..spec }, 0);
        let labels: Vec<String> = plain
            .groups()
            .map(|g| g.headers.iter().map(|h| h.label.to_string()).collect())
            .unwrap_or_default();
        assert_eq!(labels, ["Unchanged"]);
    }

    #[test]
    fn specs_with_the_same_marks_are_equal_and_with_other_marks_are_not() {
        use glint_model::entries::GitMark;
        let one = marks();
        let same_content = RowMarks {
            marks: Arc::new((*one.marks).clone()),
        };
        assert_eq!(one, same_content, "a copy that says the same is the same");
        let mut other = (*one.marks).clone();
        other.insert("docs".into(), GitMark::Staged);
        assert_ne!(one, RowMarks { marks: Arc::new(other) });
        assert!(reads_git("", GroupBy::Git));
        assert!(reads_git("x GIT:new", GroupBy::None));
        assert!(!reads_git("report", GroupBy::Kind));
    }

    // ---- Marks (docs/gpui/marks, 6.1) ----

    fn labels_of(names: &[&str]) -> Arc<[glint_model::marks::Label]> {
        names
            .iter()
            .map(|name| glint_model::marks::Label {
                name: name.to_string(),
                color: 0,
                was: Vec::new(),
            })
            .collect()
    }

    fn marked_with(items: &[(&str, Option<&str>, &[&str])], labels: &[&str]) -> RowFileMarks {
        let map: HashMap<String, glint_model::marks::Marks> = items
            .iter()
            .map(|(name, label, tags)| {
                (
                    format!("C:\\x\\{name}"),
                    glint_model::marks::Marks {
                        label: label.map(String::from),
                        tags: tags.iter().map(|t| t.to_string()).collect(),
                        comment: None,
                    },
                )
            })
            .collect();
        RowFileMarks {
            marks: Arc::new(map),
            labels: labels_of(labels),
        }
    }

    fn marks_listing() -> Arc<[FileEntry]> {
        Arc::from(vec![
            folder("dir"),
            file("a.txt", 1, 1),
            file("b.txt", 1, 1),
            file("c.txt", 1, 1),
            file("d.txt", 1, 1),
            file("report.txt", 1, 1),
        ])
    }

    fn sample_marks() -> RowFileMarks {
        marked_with(
            &[
                ("a.txt", Some("Blue"), &[]),
                ("b.txt", Some("Red"), &[]),
                ("c.txt", Some("Zed"), &["home"]),
                ("d.txt", Some("Alpha"), &[]),
                ("report.txt", None, &["work"]),
            ],
            &["Red", "Blue"],
        )
    }

    #[test]
    fn marks_filters_keep_what_the_marks_say_and_without_marks_they_keep_every_row() {
        let entries = marks_listing();
        let kept = |filter: &str, marked: Option<RowFileMarks>| {
            names(&build_rows(
                &entries,
                &RowSpec {
                    filter: filter.into(),
                    marked,
                    ..Default::default()
                },
                0,
            ))
        };
        let marks = marked_with(
            &[("a.txt", None, &["work"]), ("b.txt", Some("Work"), &[]), ("c.txt", Some("Red"), &["home"])],
            &["Work", "Red"],
        );
        assert_eq!(kept("tag:work", Some(marks.clone())), ["a.txt", "b.txt"]);
        assert_eq!(
            kept("!has:label", Some(marks.clone())),
            ["dir", "a.txt", "d.txt", "report.txt"]
        );
        let with_report = marked_with(&[("a.txt", None, &["work"]), ("report.txt", None, &["work"])], &[]);
        assert_eq!(kept("report tag:work", Some(with_report)), ["report.txt"]);
        assert_eq!(kept("tag:work", None).len(), entries.len(), "left to the search");
    }

    #[test]
    fn sorted_by_label_the_labels_order_comes_first_then_unknown_ones_by_name_then_none_and_descending_reverses() {
        let entries = marks_listing();
        let by = |desc: bool| {
            names(&build_rows(
                &entries,
                &RowSpec {
                    sort: Sort { key: SortKey::Label, desc },
                    marked: Some(sample_marks()),
                    ..Default::default()
                },
                0,
            ))
        };
        assert_eq!(by(false), ["dir", "b.txt", "a.txt", "d.txt", "c.txt", "report.txt"]);
        assert_eq!(by(true), ["dir", "report.txt", "c.txt", "d.txt", "a.txt", "b.txt"]);
    }

    #[test]
    fn grouped_by_label_the_headers_follow_the_labels_with_no_label_last_and_rows_keep_their_order() {
        let entries = marks_listing();
        let rows = build_rows(
            &entries,
            &RowSpec {
                group: GroupBy::Label,
                marked: Some(marked_with(
                    &[("a.txt", Some("Blue"), &[]), ("b.txt", Some("Red"), &[]), ("c.txt", Some("Blue"), &[])],
                    &["Red", "Blue"],
                )),
                ..Default::default()
            },
            0,
        );
        let layout = rows.groups.as_ref().expect("grouped");
        let heads: Vec<(&str, usize)> = layout.headers.iter().map(|h| (h.label.as_str(), h.count)).collect();
        assert_eq!(heads, [("Red", 1), ("Blue", 2), ("No label", 3)]);
        assert_eq!(
            names(&rows),
            ["b.txt", "a.txt", "c.txt", "dir", "d.txt", "report.txt"]
        );
    }

    #[test]
    fn specs_with_the_very_same_marks_are_equal_and_with_other_maps_are_not() {
        let one = sample_marks();
        let a = RowSpec {
            marked: Some(one.clone()),
            ..Default::default()
        };
        assert_eq!(a, RowSpec { marked: Some(one.clone()), ..Default::default() });
        let other = marked_with(&[("a.txt", None, &["x"])], &["Red", "Blue"]);
        assert_ne!(a, RowSpec { marked: Some(other), ..Default::default() });
        let other_labels = RowFileMarks {
            labels: labels_of(&["Blue"]),
            ..one
        };
        assert_ne!(a, RowSpec { marked: Some(other_labels), ..Default::default() });
        assert_ne!(a, RowSpec::default());
    }

    #[test]
    fn a_filter_a_sort_or_a_group_by_a_mark_reads_marks_and_nothing_else_does() {
        let by_name = Sort { key: SortKey::Name, desc: false };
        assert!(reads_marks("x label:red", by_name, GroupBy::None));
        assert!(reads_marks("", Sort { key: SortKey::Tags, desc: false }, GroupBy::None));
        assert!(reads_marks("", Sort { key: SortKey::Label, desc: true }, GroupBy::None));
        assert!(reads_marks("", by_name, GroupBy::Label));
        assert!(!reads_marks("x ext:pdf", by_name, GroupBy::None));
        assert!(!reads_marks("", Sort { key: SortKey::Size, desc: false }, GroupBy::Kind));
    }

    #[test]
    fn rows_no_longer_wanted_are_not_finished_and_wanted_ones_are_the_same_rows() {
        let entries = tricky_listing(20_000);
        let spec = RowSpec {
            filter: "a".into(),
            sort: Sort { key: SortKey::Size, desc: true },
            ..RowSpec::default()
        };
        let whole = build_rows(&entries, &spec, 0);
        assert!(whole.len() < entries.len(), "the filter leaves some out");
        assert_eq!(names(&build_rows_while(&entries, &spec, 0, &|| true)), names(&whole));
        // Told no at the first asking, it stops there: the listing as it is, after one question.
        let asked = std::cell::Cell::new(0);
        let dropped = build_rows_while(&entries, &spec, 0, &|| {
            asked.set(asked.get() + 1);
            false
        });
        assert_eq!((dropped.len(), asked.get()), (entries.len(), 1));
        // Told no only once the entries are gone through, it stops before the sort.
        let unsorted = build_rows_while(&entries, &spec, 0, &|| asked.replace(asked.get() + 1) < 4);
        assert_eq!(unsorted.len(), entries.len());
    }

    /// Sorts and filters of a 10,000-row folder by names and sizes (M5), then by label with every row marked. By hand:
    /// `cargo test --release -p glint-ui --lib explorer::rows::tests::building_ten_thousand_rows -- --ignored --nocapture`
    #[test]
    #[ignore = "timing: run in release"]
    fn building_ten_thousand_rows_costs_the_same_without_marks_and_this_much_by_label() {
        let entries = tricky_listing(10_000);
        let median = |spec: &RowSpec| {
            let mut times: Vec<f64> = (0..11)
                .map(|_| {
                    let started = std::time::Instant::now();
                    std::hint::black_box(build_rows(&entries, spec, 0).len());
                    started.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            times.sort_by(f64::total_cmp);
            times[5]
        };
        let sort = |key, desc| RowSpec {
            sort: Sort { key, desc },
            presorted: false,
            ..RowSpec::default()
        };
        println!("default spec: {:.2} ms", median(&RowSpec::default()));
        println!("name descending: {:.2} ms", median(&sort(SortKey::Name, true)));
        println!("size: {:.2} ms", median(&sort(SortKey::Size, false)));
        let map: HashMap<String, glint_model::marks::Marks> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                (
                    e.path.clone(),
                    glint_model::marks::Marks {
                        label: Some(["Red", "Blue", "Zed"][i % 3].to_string()),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let by_label = RowSpec {
            marked: Some(RowFileMarks {
                marks: Arc::new(map),
                labels: labels_of(&["Red", "Blue"]),
            }),
            ..sort(SortKey::Label, false)
        };
        println!("label, every row marked: {:.2} ms", median(&by_label));
    }

    /// Names that are hard for a key: digit runs and leading zeros, case, accents, scripts, a long shared start
    /// (differing only after 32 bytes of key) and digit runs longer than a length byte holds.
    fn tricky_names(count: usize) -> Vec<String> {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let parts = [
            "amd64_microsoft-windows-",
            "File",
            "IMG_",
            "é",
            "ß",
            "z",
            "0",
            "007",
            "10",
            "9",
            ".",
            " ",
            "-",
            "a",
            "日本",
            "😀",
            "Σ",
            "_copy",
            ".txt",
        ];
        let mut names: Vec<String> = (0..count)
            .map(|_| (0..1 + next(6)).map(|_| parts[next(parts.len())]).collect::<String>())
            .collect();
        let long = "windows-component-store-assembly-".repeat(2);
        for i in 0..30 {
            names.push(format!("{long}{i}"));
            names.push(format!("{long}{i:03}.dll"));
        }
        for digits in [254usize, 255, 256, 300] {
            names.push(format!("n{}1x", "9".repeat(digits)));
            names.push(format!("n{}2x", "9".repeat(digits)));
            names.push(format!("n0{}", "1".repeat(digits)));
        }
        names
    }

    /// The listing as entries: every fifth one a folder, sizes and dates that tie often.
    fn tricky_listing(count: usize) -> Arc<[FileEntry]> {
        tricky_names(count)
            .iter()
            .enumerate()
            .map(|(i, name)| {
                if i % 5 == 0 {
                    folder(name)
                } else {
                    file(name, (i % 7) as u64, (i % 11) as i64)
                }
            })
            .collect()
    }

    /// What the rows were before they used name keys: `entry_order` over the indices.
    fn order_by_comparing_names(entries: &Arc<[FileEntry]>, spec: &RowSpec) -> Vec<usize> {
        let compare =
            glint_model::entries::entry_order(spec.sort, spec.folders_first, spec.presorted, None, None, None);
        let mut order: Vec<usize> = (0..entries.len()).filter(|&i| !is_hidden(&entries[i])).collect();
        order.sort_by(|&a, &b| compare(&entries[a], &entries[b]));
        order
    }

    fn sorted_by_comparing_names(entries: &Arc<[FileEntry]>, spec: &RowSpec) -> Vec<String> {
        let order = order_by_comparing_names(entries, spec);
        order.into_iter().map(|i| entries[i].path.clone()).collect()
    }

    /// Rows sort exactly as comparing the names did, for every key, either direction, with and without the
    /// engine's presorting, folders first or not.
    #[test]
    fn rows_sorted_with_name_keys_are_in_the_order_comparing_names_gives() {
        let entries = tricky_listing(1_500);
        for key in [SortKey::Name, SortKey::Modified, SortKey::Size, SortKey::Type] {
            for desc in [false, true] {
                for presorted in [false, true] {
                    for folders_first in [false, true] {
                        let spec = RowSpec {
                            sort: Sort { key, desc },
                            folders_first,
                            presorted,
                            ..RowSpec::default()
                        };
                        let want = sorted_by_comparing_names(&entries, &spec);
                        let rows = build_rows(&entries, &spec, 0);
                        let got: Vec<String> = rows.iter().map(|entry| entry.path.clone()).collect();
                        assert_eq!(got.len(), want.len());
                        // Without the engine's order the listing here is not in it, so the by-default shortcut
                        // (which keeps the listing as it is) is not what is being compared.
                        if !(presorted && folders_first && key == SortKey::Name && !desc) {
                            assert_eq!(
                                got, want,
                                "{key:?} desc={desc} presorted={presorted} folders={folders_first}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Sorting 20,000 rows by name, by the old way (comparing names) and by name keys. Run in release:
    /// `cargo test --release -p glint-ui --lib explorer::rows::tests::sorting_twenty_thousand_rows -- --ignored --nocapture`
    #[test]
    #[ignore = "timing: run in release"]
    fn sorting_twenty_thousand_rows_costs_less_with_name_keys() {
        let entries = tricky_listing(20_000);
        let spec = RowSpec {
            sort: Sort {
                key: SortKey::Name,
                desc: true,
            },
            presorted: false,
            ..RowSpec::default()
        };
        let best = |work: &dyn Fn() -> usize| {
            (0..9)
                .map(|_| {
                    let started = std::time::Instant::now();
                    std::hint::black_box(work());
                    started.elapsed().as_secs_f64() * 1000.0
                })
                .fold(f64::MAX, f64::min)
        };
        let before = best(&|| order_by_comparing_names(&entries, &spec).len());
        let after = best(&|| build_rows(&entries, &spec, 0).len());
        println!(
            "{} rows, name sort: comparing names {before:.2} ms, name keys {after:.2} ms",
            entries.len()
        );
    }

    #[test]
    fn color_filter_drops_non_matching_and_ranks_closest_first() {
        use glint_model::oklab::{srgb_to_oklab, ColorFilter, Palette3};
        let entries: Arc<[FileEntry]> = Arc::from(vec![
            with_details(
                RawEntry {
                    name: "blue.jpg".into(),
                    ..Default::default()
                },
                "C:\\pics",
            ),
            with_details(
                RawEntry {
                    name: "red.jpg".into(),
                    ..Default::default()
                },
                "C:\\pics",
            ),
            with_details(
                RawEntry {
                    name: "orange.jpg".into(),
                    ..Default::default()
                },
                "C:\\pics",
            ),
        ]);
        let mut palettes = std::collections::HashMap::new();
        palettes.insert(
            r"c:\pics\blue.jpg".to_string(),
            Palette3 {
                colors: [srgb_to_oklab(20, 80, 240); 3],
                weights: [1.0, 0.0, 0.0],
            },
        );
        palettes.insert(
            r"c:\pics\red.jpg".to_string(),
            Palette3 {
                colors: [srgb_to_oklab(240, 30, 30); 3],
                weights: [1.0, 0.0, 0.0],
            },
        );
        palettes.insert(
            r"c:\pics\orange.jpg".to_string(),
            Palette3 {
                colors: [srgb_to_oklab(240, 120, 40); 3],
                weights: [1.0, 0.0, 0.0],
            },
        );
        let spec = RowSpec {
            color: Some(RowColor {
                filter: ColorFilter {
                    max_dist: 0.20,
                    ..ColorFilter::new(srgb_to_oklab(245, 25, 25))
                },
                palettes: Arc::new(palettes),
                rev: 0,
            }),
            ..RowSpec::default()
        };
        let rows = build_rows(&entries, &spec, 0);
        let names: Vec<&str> = rows.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["red.jpg", "orange.jpg"]);
    }

    /// 100,000 photos with palettes, half of them within tolerance: the rows the way 0.15 made them (a lowercase
    /// path per kept entry and two per comparison) and the way they are made now. Prints both; they must agree.
    /// `cargo test -p glint-ui --lib color_rows_cost_little -- --nocapture` (release shows the real gap).
    #[test]
    fn color_rows_cost_little() {
        use glint_model::oklab::{palette_distance_sq, srgb_to_oklab, ColorFilter, Palette3};
        let n = 100_000usize;
        let mut list = Vec::with_capacity(n);
        let mut palettes = std::collections::HashMap::with_capacity(n);
        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 1000) as f32 / 1000.0
        };
        for i in 0..n {
            let name = format!("IMG_{i:06}.JPG");
            let entry = with_details(
                RawEntry {
                    name: name.clone(),
                    ..Default::default()
                },
                "C:\\Pics\\Trip",
            );
            palettes.insert(
                entry.path.to_lowercase(),
                Palette3 {
                    colors: [
                        glint_model::oklab::Oklab {
                            l: 0.3 + next() * 0.5,
                            a: next() * 0.4 - 0.2,
                            b: next() * 0.4 - 0.2,
                        },
                        srgb_to_oklab(0, 0, 0),
                        srgb_to_oklab(255, 255, 255),
                    ],
                    weights: [0.6, 0.3, 0.1],
                },
            );
            list.push(entry);
        }
        let entries: Arc<[FileEntry]> = Arc::from(list);
        let palettes = Arc::new(palettes);
        let filter = ColorFilter::new(srgb_to_oklab(180, 120, 90));
        let spec = RowSpec {
            color: Some(RowColor {
                filter,
                palettes: palettes.clone(),
                rev: 0,
            }),
            ..RowSpec::default()
        };
        let old = || -> Vec<u32> {
            let max_sq = filter.max_dist * filter.max_dist;
            let mut order: Vec<u32> = Vec::new();
            for (i, e) in entries.iter().enumerate() {
                let Some(p) = palettes.get(&e.path.to_lowercase()) else {
                    continue;
                };
                if palette_distance_sq(p, filter.target) <= max_sq {
                    order.push(i as u32);
                }
            }
            order.sort_by(|&a, &b| {
                let da = palettes
                    .get(&entries[a as usize].path.to_lowercase())
                    .map_or(f32::INFINITY, |p| palette_distance_sq(p, filter.target));
                let db = palettes
                    .get(&entries[b as usize].path.to_lowercase())
                    .map_or(f32::INFINITY, |p| palette_distance_sq(p, filter.target));
                da.total_cmp(&db)
            });
            order
        };
        let best = |work: &dyn Fn() -> usize| {
            (0..3)
                .map(|_| {
                    let started = std::time::Instant::now();
                    std::hint::black_box(work());
                    started.elapsed().as_secs_f64() * 1000.0
                })
                .fold(f64::MAX, f64::min)
        };
        let want = old();
        let rows = build_rows(&entries, &spec, 0);
        assert!(!want.is_empty() && want.len() < n);
        assert_eq!(rows.len(), want.len());
        for (row, &i) in rows.iter().zip(&want) {
            assert_eq!(row.path, entries[i as usize].path);
        }
        let before = best(&|| old().len());
        let after = best(&|| build_rows(&entries, &spec, 0).len());
        println!(
            "{n} photos, {} kept: color rows 0.15 way {before:.1} ms, now {after:.1} ms",
            want.len()
        );
    }

    /// A chord keeps the photos that have any of its colors, or all of them, ranked by the best or the worst.
    #[test]
    fn a_color_chord_keeps_photos_with_any_or_all_of_its_colors() {
        use glint_model::oklab::{srgb_to_oklab, ChordMode, ColorFilter, Palette3};
        let photo = |name: &str| {
            with_details(
                RawEntry {
                    name: name.into(),
                    ..Default::default()
                },
                "C:\\pics",
            )
        };
        let entries: Arc<[FileEntry]> = Arc::from(vec![photo("beach.jpg"), photo("sunset.jpg"), photo("forest.jpg")]);
        let (orange, purple) = (srgb_to_oklab(240, 140, 30), srgb_to_oklab(80, 30, 120));
        let palette = |colors: [glint_model::oklab::Oklab; 3]| Palette3 {
            colors,
            weights: [0.6, 0.3, 0.1],
        };
        let mut palettes = std::collections::HashMap::new();
        palettes.insert(
            "c:\\pics\\sunset.jpg".to_string(),
            palette([orange, purple, srgb_to_oklab(10, 10, 20)]),
        );
        palettes.insert(
            "c:\\pics\\beach.jpg".to_string(),
            palette([orange, srgb_to_oklab(20, 150, 150), srgb_to_oklab(240, 230, 210)]),
        );
        palettes.insert(
            "c:\\pics\\forest.jpg".to_string(),
            palette([srgb_to_oklab(20, 90, 30); 3]),
        );
        let palettes = Arc::new(palettes);
        let names = |mode: ChordMode| {
            let spec = RowSpec {
                color: Some(RowColor {
                    filter: ColorFilter {
                        mode,
                        ..ColorFilter::new(orange).with_added(purple)
                    },
                    palettes: palettes.clone(),
                    rev: 0,
                }),
                ..RowSpec::default()
            };
            let rows = build_rows(&entries, &spec, 0);
            rows.iter().map(|e| e.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(names(ChordMode::All), vec!["sunset.jpg"], "only the sunset has both");
        let any = names(ChordMode::Any);
        assert_eq!(any.len(), 2, "the forest has neither: {any:?}");
        assert!(any.contains(&"sunset.jpg".to_string()) && any.contains(&"beach.jpg".to_string()));
    }
}
