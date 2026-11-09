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

use glint_model::entries::{entry_order, is_hidden, matcher, FileEntry, Sort, SortKey};
use glint_model::groups::{group_rows, GroupBy, GroupHeader, Grouped};
use std::cmp::Ordering;
use std::sync::Arc;

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
        }
    }
}

/// The headers of a grouped list and where its lines are (`Grouped` without its copy of the rows).
#[derive(Clone, Debug, PartialEq)]
pub struct GroupLayout {
    pub headers: Vec<GroupHeader>,
    /// Each list line: a row's index, or (as -1 - i) header i.
    pub lines: Vec<i32>,
    /// Each row's line.
    pub line_of: Vec<i32>,
}

/// A tab's rows, in display order. Cloning is cheap (a few `Arc`s).
#[derive(Clone, Debug)]
pub struct Rows {
    entries: Arc<[FileEntry]>,
    /// Display order as indices into `entries`; `None` means `entries` as they are.
    order: Option<Arc<[u32]>>,
    groups: Option<Arc<GroupLayout>>,
    /// How many items the Show hidden items setting left out.
    pub hidden: usize,
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
            hidden: 0,
        }
    }

    /// The listing's own entries, in their order.
    pub fn identity(entries: Arc<[FileEntry]>) -> Self {
        Rows {
            entries,
            order: None,
            groups: None,
            hidden: 0,
        }
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

    /// The index of the row with this path (`findIndex`): the first match, compared exactly.
    pub fn position_of(&self, path: &str) -> Option<usize> {
        self.iter().position(|e| e.path == path)
    }

    /// Group headers and lines when the list is grouped.
    pub fn groups(&self) -> Option<&GroupLayout> {
        self.groups.as_deref()
    }

    /// Whether these rows are the listing's own entries untouched (the fast, default case).
    pub fn is_identity(&self) -> bool {
        self.order.is_none() && self.groups.is_none()
    }
}

/// Makes the rows for `entries` under `spec`. `now` is the time for Group by date (ms since the epoch).
///
/// The same steps as the TypeScript's three memos, in order: sort (skipped for the engine's own order),
/// hide hidden items, narrow by the filter, then group.
pub fn build_rows(entries: &Arc<[FileEntry]>, spec: &RowSpec, now: i64) -> Rows {
    let by_default = spec.presorted && spec.folders_first && spec.sort.key == SortKey::Name && !spec.sort.desc;
    // `matcher` is `None` for text that filters nothing (empty, or asking the models).
    let test = matcher(&spec.filter, None, None);
    let mut hidden = 0usize;
    let narrowing = test.is_some() || !spec.show_hidden;

    let mut order: Option<Vec<u32>> = None;
    if narrowing {
        let mut kept = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            if !spec.show_hidden && is_hidden(entry) {
                hidden += 1;
                continue;
            }
            if test.as_ref().is_some_and(|test| !test(entry)) {
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

    if !by_default {
        if let Some(order) = order.as_mut() {
            let compare = entry_order(spec.sort, spec.folders_first, spec.presorted, None, None, None);
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
        hidden,
    };
    if spec.group == GroupBy::None {
        return sorted;
    }
    let flat: Vec<FileEntry> = sorted.iter().cloned().collect();
    let Grouped {
        rows,
        headers,
        lines,
        line_of,
    } = group_rows(&flat, spec.group, now, None, None);
    Rows {
        entries: Arc::from(rows),
        order: None,
        groups: Some(Arc::new(GroupLayout {
            headers,
            lines,
            line_of,
        })),
        hidden,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::entries::{attr, with_details, RawEntry};

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
}
