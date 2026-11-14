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
use glint_model::entries::{entry_order, is_hidden, matcher, FileEntry, GitMarks, SizeOf, Sort, SortKey};
use glint_model::groups::{group_rows, GroupBy, GroupHeader, Grouped, SizeOf as GroupSizeOf};
use std::cmp::Ordering;
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

/// What a search found inside files (`inside.rs`), for the rows: which files were found by their contents alone,
/// and every file's matching lines. Equal when it is the very same findings, so a spec compare walks nothing.
#[derive(Clone, Debug)]
pub struct RowInside {
    pub only: Arc<HashSet<String>>,
    pub hits: Arc<Hits>,
    /// The rows show the lines and the two parts' headers (the list does; the grid and the columns view show the
    /// files alone).
    pub lines: bool,
}

impl PartialEq for RowInside {
    fn eq(&self, other: &Self) -> bool {
        self.lines == other.lines && Arc::ptr_eq(&self.only, &other.only) && Arc::ptr_eq(&self.hits, &other.hits)
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

/// The headers of the two parts a search's results are in when it found files by their contents: by name (none
/// when no name matched), then inside files. Indices into [`GroupLayout::headers`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sections {
    pub names: Option<usize>,
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

/// Whether a filter or a grouping reads the folder's Git marks (`git:` in the box, Group by Git).
pub fn reads_git(filter: &str, group: GroupBy) -> bool {
    group == GroupBy::Git || filter.to_lowercase().contains("git:")
}

/// Makes the rows for `entries` under `spec`. `now` is the time for Group by date (ms since the epoch).
///
/// The same steps as the TypeScript's three memos, in order: sort (skipped for the engine's own order),
/// hide hidden items, narrow by the filter, then group.
pub fn build_rows(entries: &Arc<[FileEntry]>, spec: &RowSpec, now: i64) -> Rows {
    let by_default = spec.presorted && spec.folders_first && spec.sort.key == SortKey::Name && !spec.sort.desc;
    // `matcher` is `None` for text that filters nothing (empty, or asking the models).
    let marks = spec.git.as_ref().map(|git| &*git.marks);
    let test = matcher(&spec.filter, marks, None);
    let mut hidden = 0usize;
    let diffs = spec.diffs.as_ref().map(|d| &d.comparison.diffs);
    let narrowing = test.is_some() || !spec.show_hidden || diffs.is_some();

    let mut order: Option<Vec<u32>> = None;
    if narrowing {
        let mut kept = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            if !spec.show_hidden && is_hidden(entry) {
                hidden += 1;
                continue;
            }
            if diffs.is_some_and(|d| d.get(&entry.lower) == Some(&glint_model::entries::DiffStatus::Same)) {
                continue;
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
        }
        // Nothing was left out and the order is the engine's: the listing is the rows.
        if kept.len() != entries.len() || !by_default {
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
    if !by_default {
        if let Some(order) = order.as_mut() {
            let size_of = measured.as_ref().map(|f| f as &SizeOf);
            let compare = entry_order(spec.sort, spec.folders_first, spec.presorted, size_of, None, None);
            // A stable sort, so ties keep the engine's name order, as the TypeScript's did.
            order.sort_by(|&a, &b| match (entries.get(a as usize), entries.get(b as usize)) {
                (Some(a), Some(b)) => compare(a, b),
                _ => Ordering::Equal,
            });
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
    let flat: Vec<FileEntry> = sorted.iter().cloned().collect();
    let group_size = spec.sizes.as_ref().map(|s| {
        let sizes = s.sizes.clone();
        move |e: &FileEntry| -> Option<u64> { sizes.get(&e.name).copied() }
    });
    let Grouped {
        rows,
        headers,
        lines,
        line_of,
    } = group_rows(
        &flat,
        spec.group,
        now,
        group_size.as_ref().map(|f| f as &GroupSizeOf),
        marks,
    );
    let grouped = Rows {
        entries: Arc::from(rows),
        order: None,
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

/// `rows` with what a search found inside files: each file's matching lines under its row (the first
/// [`HITS_SHOWN`], then a count of the rest). Rows that aren't grouped are put in the search's two parts, each
/// under its header: the files found by name (in their order), then the ones found by their contents alone. A
/// grouped list keeps its groups.
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
            let (mut named, mut found): (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
            for i in 0..count {
                let by_contents = rows.get(i).is_some_and(|entry| inside.only.contains(&entry.path));
                if by_contents {
                    found.push(index_of(i));
                } else {
                    named.push(index_of(i));
                }
            }
            if found.is_empty() {
                slots.extend((0..count).map(Slot::Row));
            } else {
                let names = (!named.is_empty()).then(|| {
                    headers.push(GroupHeader {
                        label: "Names".to_string(),
                        count: named.len(),
                        size: 0,
                        first: 0,
                    });
                    slots.push(Slot::Header(0));
                    0
                });
                slots.extend((0..named.len()).map(Slot::Row));
                headers.push(GroupHeader {
                    label: "Inside files".to_string(),
                    count: found.len(),
                    size: 0,
                    first: named.len(),
                });
                slots.push(Slot::Header(headers.len() - 1));
                slots.extend((named.len()..count).map(Slot::Row));
                sections = Some(Sections {
                    names,
                    inside: headers.len() - 1,
                });
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
                let shown = hits.min(HITS_SHOWN);
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
}
