//! Ported from src/lib/groups.ts.
//!
//! Group by: the groups a folder's items fall into, and their headers, as File Explorer's Group by shows them:
//! headers over the rows.
//!
//! NOTE(port): the group labels are plain English in the TypeScript (the header text never went through `t()`),
//! so they are plain English here too. The UI layer translates them when it draws the header.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;

use chrono::{Datelike, Local, TimeZone};

use crate::entries::{FileEntry, GitMark};
use crate::file_types::{kind_of, type_label, TYPE_GROUPS};

/// What a folder's (or a search's) items are grouped by, as File Explorer's Group by: headers over the rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupBy {
    None,
    Kind,
    Date,
    Size,
    Type,
    Git,
}

/// One choice of the Group by menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupByOption {
    pub by: GroupBy,
    pub label: &'static str,
}

/// The Group by menu's choices, in order.
pub const GROUP_BYS: [GroupByOption; 6] = [
    GroupByOption {
        by: GroupBy::None,
        label: "Nothing",
    },
    GroupByOption {
        by: GroupBy::Kind,
        label: "Kind",
    },
    GroupByOption {
        by: GroupBy::Date,
        label: "Date modified",
    },
    GroupByOption {
        by: GroupBy::Size,
        label: "Size",
    },
    GroupByOption {
        by: GroupBy::Type,
        label: "Type",
    },
    GroupByOption {
        by: GroupBy::Git,
        label: "Git status",
    },
];

/// A group's place among the others and its header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub rank: i64,
    pub label: Cow<'static, str>,
}

fn group(rank: i64, label: &'static str) -> Group {
    Group {
        rank,
        label: Cow::Borrowed(label),
    }
}

const DAY: i64 = 86_400_000;

/// The Kind groups in the order the disk-usage map lists them.
fn kind_order() -> [&'static str; 12] {
    let g = &TYPE_GROUPS;
    [
        g.images.name,
        g.video.name,
        g.audio.name,
        g.archives.name,
        g.documents.name,
        g.code.name,
        g.programs.name,
        g.system.name,
        g.disks.name,
        g.data.name,
        g.fonts.name,
        g.other.name,
    ]
}

/// A folder's size for Size grouping: `None` when it isn't known (yet).
pub type SizeOf<'a> = dyn Fn(&FileEntry) -> Option<u64> + 'a;

/// Git marks by lowercased name.
pub type GitMarks = HashMap<String, GitMark>;

/// Local midnight of a day, in ms (a midnight that doesn't exist that day, as in a DST gap, becomes 1 am, as
/// JavaScript's Date does).
fn local_midnight(year: i32, month: u32, day: u32) -> Option<i64> {
    let at = |hour| Local.with_ymd_and_hms(year, month, day, hour, 0, 0);
    let time = at(0).earliest().or_else(|| at(1).earliest())?;
    Some(time.timestamp_millis())
}

/// The edges of the date groups for a "now": made once per grouping, not once per row.
struct DateBounds {
    start: i64,
    week_start: i64,
    month_start: i64,
    last_month: i64,
    year_start: i64,
    year: i32,
}

impl DateBounds {
    fn new(now: i64) -> Option<DateBounds> {
        let today = Local.timestamp_millis_opt(now).earliest()?;
        let (year, month, day) = (today.year(), today.month(), today.day());
        let start = local_midnight(year, month, day)?;
        // Monday starts the week; `num_days_from_sunday` is JavaScript's getDay().
        let week_start = start - ((i64::from(today.weekday().num_days_from_sunday()) + 6) % 7) * DAY;
        let (last_year, last) = if month == 1 { (year - 1, 12) } else { (year, month - 1) };
        Some(DateBounds {
            start,
            week_start,
            month_start: local_midnight(year, month, 1)?,
            last_month: local_midnight(last_year, last, 1)?,
            year_start: local_midnight(year, 1, 1)?,
            year,
        })
    }

    fn group(&self, t: i64) -> Group {
        if t >= self.start + DAY {
            return group(0, "Later");
        }
        if t >= self.start {
            return group(1, "Today");
        }
        if t >= self.start - DAY {
            return group(2, "Yesterday");
        }
        if t >= self.week_start {
            return group(3, "Earlier this week");
        }
        if t >= self.week_start - 7 * DAY {
            return group(4, "Last week");
        }
        if t >= self.month_start {
            return group(5, "Earlier this month");
        }
        if t >= self.last_month {
            return group(6, "Last month");
        }
        if t >= self.year_start {
            return group(7, "Earlier this year");
        }
        // NOTE(port): a time chrono can't place (JavaScript's Invalid Date would give "NaN") is "Unknown".
        let Some(year) = Local.timestamp_millis_opt(t).earliest().map(|d| d.year()) else {
            return group(99, "Unknown");
        };
        Group {
            rank: 8 + i64::from(self.year) - i64::from(year),
            label: Cow::Owned(year.to_string()),
        }
    }
}

fn size_group(size: u64) -> Group {
    if size == 0 {
        group(0, "Empty")
    } else if size < 16 << 10 {
        group(1, "Tiny (under 16 KB)")
    } else if size < 1 << 20 {
        group(2, "Small (16 KB to 1 MB)")
    } else if size < 128 << 20 {
        group(3, "Medium (1 to 128 MB)")
    } else if size < 1 << 30 {
        group(4, "Large (128 MB to 1 GB)")
    } else {
        group(5, "Huge (over 1 GB)")
    }
}

fn group_in(
    entry: &FileEntry,
    by: GroupBy,
    bounds: Option<&DateBounds>,
    size_of: Option<&SizeOf>,
    git: Option<&GitMarks>,
) -> Group {
    match by {
        GroupBy::Kind => {
            if entry.dir {
                return group(-1, "Folders");
            }
            let name = kind_of(entry).group.name;
            let rank = kind_order().iter().position(|n| *n == name).map_or(-1, |i| i as i64);
            group(rank, name)
        }
        GroupBy::Type => {
            if entry.dir {
                group(-1, "Folders")
            } else {
                Group {
                    rank: 0,
                    label: Cow::Owned(type_label(entry)),
                }
            }
        }
        GroupBy::Date => {
            if entry.modified == 0 {
                return group(99, "Unknown");
            }
            match bounds {
                Some(b) => b.group(entry.modified),
                None => group(99, "Unknown"),
            }
        }
        GroupBy::Size => {
            // A folder's size is unknown (`None`) until it has been measured.
            let size = if entry.dir {
                match size_of.and_then(|f| f(entry)) {
                    Some(size) => size,
                    None => return group(-1, "Folders"),
                }
            } else {
                entry.size
            };
            size_group(size)
        }
        GroupBy::Git => match git.and_then(|g| g.get(&entry.lower)) {
            Some(GitMark::Conflicted) => group(0, "Conflicts"),
            Some(GitMark::Modified) => group(1, "Changed"),
            Some(GitMark::Staged) => group(2, "Staged"),
            Some(GitMark::Untracked) => group(3, "New"),
            Some(GitMark::Ignored) => group(5, "Ignored"),
            None => group(4, "Unchanged"),
        },
        GroupBy::None => group(0, ""),
    }
}

/// The group `entry` falls in. `now` is when the grouping is made (Unix ms, for dates), `size_of` a folder's size.
#[cfg(test)]
pub fn group_of(entry: &FileEntry, by: GroupBy, now: i64, size_of: Option<&SizeOf>, git: Option<&GitMarks>) -> Group {
    let bounds = if by == GroupBy::Date {
        DateBounds::new(now)
    } else {
        None
    };
    group_in(entry, by, bounds.as_ref(), size_of, git)
}

/// A header: its label, how many items it has and their size, and where its first row is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupHeader {
    pub label: String,
    pub count: usize,
    pub size: u64,
    /// Index into `Grouped::rows` of the group's first row.
    pub first: usize,
}

/// Rows grouped: the rows, ordered group by group (each group keeping its order), and the headers.
#[derive(Clone, Debug, PartialEq)]
pub struct Grouped {
    pub rows: Vec<FileEntry>,
    pub headers: Vec<GroupHeader>,
    /// Each list line: a row's index, or (as -1 - i) header i.
    pub lines: Vec<i32>,
    /// Each row's line.
    pub line_of: Vec<i32>,
}

/// JavaScript's `<` on strings compares UTF-16 code units, which differs from Rust's order for characters
/// beyond the Basic Multilingual Plane.
fn js_label_order(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn line_number(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// `rows` (sorted) grouped `by`: group order first, the sort within each.
///
/// NOTE(port): the TypeScript's `rows` hold the caller's own entry objects; here they are copies.
pub fn group_rows(
    rows: &[FileEntry],
    by: GroupBy,
    now: i64,
    size_of: Option<&SizeOf>,
    git: Option<&GitMarks>,
) -> Grouped {
    let bounds = if by == GroupBy::Date {
        DateBounds::new(now)
    } else {
        None
    };
    let mut keyed: Vec<(usize, Group)> = rows
        .iter()
        .enumerate()
        .map(|(i, entry)| (i, group_in(entry, by, bounds.as_ref(), size_of, git)))
        .collect();
    keyed.sort_by(|(i, a), (j, b)| {
        a.rank
            .cmp(&b.rank)
            .then_with(|| js_label_order(&a.label, &b.label))
            .then_with(|| i.cmp(j))
    });
    let mut ordered: Vec<FileEntry> = Vec::with_capacity(rows.len());
    let mut headers: Vec<GroupHeader> = Vec::new();
    let mut lines: Vec<i32> = Vec::with_capacity(rows.len());
    let mut line_of: Vec<i32> = vec![0; rows.len()];
    let mut current: Option<&str> = None;
    for (i, group) in &keyed {
        let Some(entry) = rows.get(*i) else { continue };
        if current != Some(group.label.as_ref()) {
            current = Some(group.label.as_ref());
            headers.push(GroupHeader {
                label: group.label.to_string(),
                count: 0,
                size: 0,
                first: ordered.len(),
            });
            lines.push(-line_number(headers.len()));
        }
        if let Some(header) = headers.last_mut() {
            header.count += 1;
            // A folder not measured yet counts as 0.
            header.size += if entry.dir {
                size_of.and_then(|f| f(entry)).unwrap_or(0)
            } else {
                entry.size
            };
        }
        if let Some(slot) = line_of.get_mut(ordered.len()) {
            *slot = line_number(lines.len());
        }
        lines.push(line_number(ordered.len()));
        ordered.push(entry.clone());
    }
    Grouped {
        rows: ordered,
        headers,
        lines,
        line_of,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entries::{with_details, RawEntry};

    const HOUR: i64 = 3_600_000;

    /// A file (or a folder, with a trailing `/`) with a modified time.
    fn entry(name: &str, modified: i64) -> FileEntry {
        let (name, dir) = name.strip_suffix('/').map_or((name, false), |n| (n, true));
        with_details(
            RawEntry {
                name: name.into(),
                dir,
                modified,
                ..RawEntry::default()
            },
            "C:\\x",
        )
    }

    fn sized(name: &str, size: u64) -> FileEntry {
        let mut e = entry(name, 1);
        e.size = size;
        e
    }

    /// Local time, like `new Date(y, m - 1, d, h, min)`.
    fn local_ms(y: i32, m: u32, d: u32, h: u32, min: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .earliest()
            .map(|t| t.timestamp_millis())
            .unwrap_or(0)
    }

    /// A Wednesday.
    fn now() -> i64 {
        local_ms(2026, 11, 11, 15, 0)
    }

    fn label(e: &FileEntry, by: GroupBy) -> String {
        group_of(e, by, now(), None, None).label.into_owned()
    }

    #[test]
    fn group_by_rows_ordered_group_by_group_with_a_header_line_over_each() {
        let day = DAY;
        let now = now();
        let rows = vec![
            entry("old.txt", now - 400 * day),
            entry("today.png", now - HOUR),
            entry("docs/", now - day),
            entry("b.png", now - 2 * HOUR),
        ];
        let by_date = group_rows(&rows, GroupBy::Date, now, None, None);
        let headers: Vec<(&str, usize)> = by_date.headers.iter().map(|h| (h.label.as_str(), h.count)).collect();
        assert_eq!(headers, vec![("Today", 2), ("Yesterday", 1), ("2025", 1)]);
        let names: Vec<&str> = by_date.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["today.png", "b.png", "docs", "old.txt"]);
        // Lines: header, row, row, header, row, header, row; each row knows its line.
        assert_eq!(by_date.lines, vec![-1, 0, 1, -2, 2, -3, 3]);
        assert_eq!(by_date.line_of, vec![1, 2, 4, 6]);
        let by_kind = group_rows(&rows, GroupBy::Kind, now, None, None);
        assert_eq!(by_kind.headers[0].label, "Folders");
        assert_eq!(label(&sized("big.iso", 1 << 31), GroupBy::Size), "Huge (over 1 GB)");
    }

    #[test]
    fn headers_know_their_first_row_count_and_size() {
        let rows = vec![sized("a.png", 100), sized("b.txt", 50), sized("c.png", 7)];
        let g = group_rows(&rows, GroupBy::Kind, now(), None, None);
        let summary: Vec<(&str, usize, u64, usize)> = g
            .headers
            .iter()
            .map(|h| (h.label.as_str(), h.count, h.size, h.first))
            .collect();
        assert_eq!(summary, vec![("Images", 2, 107, 0), ("Code and text", 1, 50, 2)]);
        assert_eq!(g.lines, vec![-1, 0, 1, -2, 2]);
    }

    #[test]
    fn group_by_none_keeps_one_nameless_group_in_order() {
        let rows = vec![entry("b", 1), entry("a", 1)];
        let g = group_rows(&rows, GroupBy::None, now(), None, None);
        assert_eq!(g.headers.len(), 1);
        assert_eq!(g.headers[0].label, "");
        assert_eq!(g.rows[0].name, "b");
        assert!(group_rows(&[], GroupBy::Date, now(), None, None).lines.is_empty());
    }

    #[test]
    fn the_menu_lists_every_choice() {
        let labels: Vec<&str> = GROUP_BYS.iter().map(|g| g.label).collect();
        assert_eq!(
            labels,
            vec!["Nothing", "Kind", "Date modified", "Size", "Type", "Git status"]
        );
        assert_eq!(GROUP_BYS[0].by, GroupBy::None);
        assert_eq!(GROUP_BYS[5].by, GroupBy::Git);
    }

    #[test]
    fn kind_puts_folders_first_then_the_disk_map_order() {
        let g = group_of(&entry("docs/", 1), GroupBy::Kind, now(), None, None);
        assert_eq!((g.rank, g.label.as_ref()), (-1, "Folders"));
        let g = group_of(&entry("a.png", 1), GroupBy::Kind, now(), None, None);
        assert_eq!((g.rank, g.label.as_ref()), (0, "Images"));
        let g = group_of(&entry("a.mp3", 1), GroupBy::Kind, now(), None, None);
        assert_eq!((g.rank, g.label.as_ref()), (2, "Audio"));
        let g = group_of(&entry("a.zzzz", 1), GroupBy::Kind, now(), None, None);
        assert_eq!((g.rank, g.label.as_ref()), (11, "Other files"));
    }

    #[test]
    fn type_groups_by_the_type_column_text() {
        let g = group_of(&entry("docs/", 1), GroupBy::Type, now(), None, None);
        assert_eq!((g.rank, g.label.as_ref()), (-1, "Folders"));
        let e = entry("a.png", 1);
        let g = group_of(&e, GroupBy::Type, now(), None, None);
        assert_eq!((g.rank, g.label.into_owned()), (0, type_label(&e)));
        // Different types sort by their labels.
        let rows = vec![entry("z.zzzz", 1), entry("a.png", 1), entry("d/", 1)];
        let sorted = group_rows(&rows, GroupBy::Type, now(), None, None);
        assert_eq!(sorted.headers[0].label, "Folders");
        assert_eq!(sorted.rows[0].name, "d");
        assert_eq!(sorted.headers.len(), 3);
    }

    #[test]
    fn date_groups_are_judged_against_today() {
        let at = |y, m, d, h| entry("f", local_ms(y, m, d, h, 0));
        let check = |e: &FileEntry, rank: i64, name: &str| {
            let g = group_of(e, GroupBy::Date, now(), None, None);
            assert_eq!((g.rank, g.label.as_ref()), (rank, name));
        };
        // Wednesday 11 November 2026; the week starts on Monday the 9th.
        check(&at(2026, 11, 12, 9), 0, "Later");
        check(&at(2026, 11, 11, 0), 1, "Today");
        check(&at(2026, 11, 10, 12), 2, "Yesterday");
        check(&at(2026, 11, 9, 10), 3, "Earlier this week");
        check(&at(2026, 11, 5, 10), 4, "Last week");
        check(&at(2026, 11, 2, 0), 4, "Last week");
        check(&at(2026, 11, 1, 10), 5, "Earlier this month");
        check(&at(2026, 10, 20, 10), 6, "Last month");
        check(&at(2026, 3, 3, 10), 7, "Earlier this year");
        check(&at(2025, 12, 31, 10), 9, "2025");
        check(&at(2024, 6, 1, 10), 10, "2024");
        check(&entry("nodate", 0), 99, "Unknown");
    }

    #[test]
    fn date_groups_in_january_look_back_to_december_as_last_month() {
        let now = local_ms(2026, 1, 14, 12, 0); // a Wednesday
        let g = group_of(
            &entry("f", local_ms(2025, 12, 20, 9, 0)),
            GroupBy::Date,
            now,
            None,
            None,
        );
        assert_eq!((g.rank, g.label.as_ref()), (6, "Last month"));
        let g = group_of(
            &entry("f", local_ms(2025, 11, 20, 9, 0)),
            GroupBy::Date,
            now,
            None,
            None,
        );
        assert_eq!((g.rank, g.label.as_ref()), (9, "2025"));
    }

    #[test]
    fn date_groups_order_newest_first() {
        let rows = vec![
            entry("y2024", local_ms(2024, 5, 5, 9, 0)),
            entry("later", local_ms(2026, 12, 1, 9, 0)),
            entry("month", local_ms(2026, 11, 1, 9, 0)),
            entry("today", local_ms(2026, 11, 11, 9, 0)),
        ];
        let g = group_rows(&rows, GroupBy::Date, now(), None, None);
        let labels: Vec<&str> = g.headers.iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, vec!["Later", "Today", "Earlier this month", "2024"]);
    }

    #[test]
    fn size_groups_split_at_the_documented_edges() {
        let size = |n: u64| label(&sized("f", n), GroupBy::Size);
        assert_eq!(size(0), "Empty");
        assert_eq!(size(1), "Tiny (under 16 KB)");
        assert_eq!(size((16 << 10) - 1), "Tiny (under 16 KB)");
        assert_eq!(size(16 << 10), "Small (16 KB to 1 MB)");
        assert_eq!(size((1 << 20) - 1), "Small (16 KB to 1 MB)");
        assert_eq!(size(1 << 20), "Medium (1 to 128 MB)");
        assert_eq!(size((128 << 20) - 1), "Medium (1 to 128 MB)");
        assert_eq!(size(128 << 20), "Large (128 MB to 1 GB)");
        assert_eq!(size((1 << 30) - 1), "Large (128 MB to 1 GB)");
        assert_eq!(size(1 << 30), "Huge (over 1 GB)");
        let g = group_of(&sized("f", 5 << 20), GroupBy::Size, now(), None, None);
        assert_eq!(g.rank, 3);
    }

    #[test]
    fn size_groups_use_the_measured_size_of_folders() {
        let folder = entry("docs/", 1);
        // Not measured (no sizes, or none for this folder): the Folders group.
        let g = group_of(&folder, GroupBy::Size, now(), None, None);
        assert_eq!((g.rank, g.label.as_ref()), (-1, "Folders"));
        let unknown: &SizeOf = &|_| None;
        let g = group_of(&folder, GroupBy::Size, now(), Some(unknown), None);
        assert_eq!((g.rank, g.label.as_ref()), (-1, "Folders"));
        // Measured: grouped like a file, an empty one included.
        let big: &SizeOf = &|_| Some(200 << 20);
        let g = group_of(&folder, GroupBy::Size, now(), Some(big), None);
        assert_eq!((g.rank, g.label.as_ref()), (4, "Large (128 MB to 1 GB)"));
        let zero: &SizeOf = &|_| Some(0);
        assert_eq!(group_of(&folder, GroupBy::Size, now(), Some(zero), None).label, "Empty");
    }

    #[test]
    fn headers_add_up_folder_sizes_and_count_unmeasured_ones_as_nothing() {
        let rows = vec![entry("a/", 1), entry("b/", 1), sized("f.txt", 10)];
        let sizes: &SizeOf = &|e| if e.name == "a" { Some(1000) } else { None };
        let g = group_rows(&rows, GroupBy::Size, now(), Some(sizes), None);
        let summary: Vec<(&str, usize, u64)> = g.headers.iter().map(|h| (h.label.as_str(), h.count, h.size)).collect();
        // "b" has no size yet, so it stays under Folders and adds 0; "a" (1000 bytes) is Tiny, like the file.
        assert_eq!(summary, vec![("Folders", 1, 0), ("Tiny (under 16 KB)", 2, 1010)]);
    }

    #[test]
    fn git_groups_follow_the_marks_by_lowercased_name() {
        let mut marks = GitMarks::new();
        marks.insert("a.rs".into(), GitMark::Conflicted);
        marks.insert("b.rs".into(), GitMark::Modified);
        marks.insert("c.rs".into(), GitMark::Staged);
        marks.insert("d.rs".into(), GitMark::Untracked);
        marks.insert("e.rs".into(), GitMark::Ignored);
        let of = |name: &str| {
            let g = group_of(&entry(name, 1), GroupBy::Git, now(), None, Some(&marks));
            (g.rank, g.label.into_owned())
        };
        assert_eq!(of("A.rs"), (0, "Conflicts".to_string()));
        assert_eq!(of("b.rs"), (1, "Changed".to_string()));
        assert_eq!(of("c.rs"), (2, "Staged".to_string()));
        assert_eq!(of("d.rs"), (3, "New".to_string()));
        assert_eq!(of("e.rs"), (5, "Ignored".to_string()));
        assert_eq!(of("f.rs"), (4, "Unchanged".to_string()));
        // Without marks everything is unchanged.
        assert_eq!(label(&entry("a.rs", 1), GroupBy::Git), "Unchanged");
        let rows = vec![entry("f.rs", 1), entry("e.rs", 1), entry("a.rs", 1)];
        let g = group_rows(&rows, GroupBy::Git, now(), None, Some(&marks));
        let labels: Vec<&str> = g.headers.iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, vec!["Conflicts", "Unchanged", "Ignored"]);
    }

    #[test]
    fn rows_keep_their_order_inside_a_group() {
        let rows = vec![
            entry("c.png", 1),
            entry("a.png", 1),
            entry("b.txt", 1),
            entry("b.png", 1),
        ];
        let g = group_rows(&rows, GroupBy::Kind, now(), None, None);
        let names: Vec<&str> = g.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["c.png", "a.png", "b.png", "b.txt"]);
    }

    #[test]
    fn labels_compare_like_javascript_strings() {
        // U+FF5E (a BMP character) is above U+D83D (an emoji's first UTF-16 unit); Rust's own order disagrees.
        assert_eq!(js_label_order("\u{FF5E}", "\u{1F600}"), Ordering::Greater);
        assert_eq!("\u{FF5E}".cmp("\u{1F600}"), Ordering::Less);
    }
}
