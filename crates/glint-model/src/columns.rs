//! Ported from src/lib/columns.ts.
//!
//! The details view's columns: which exist, their order, widths and folder kinds.
//!
//! NOTE(port): the web UI drew the row as a CSS grid whose tracks were `var(--col-<width>)`. The GPUI layout
//! has no CSS, so `grid_tracks` gives the same thing as data (a name track, an optional "where" track, then
//! one track per column, for each width of the list) and `grid_templates` still builds the CSS strings from
//! it, so the original assertions stay meaningful.

use crate::entries::{FileEntry, Sort, SortKey};
use crate::file_types::{type_group, TYPE_GROUPS};
use crate::js::{js_round, number_to_string};
use crate::store::{Column, ViewMode};

/// The list's columns besides the name (and, in a list of items from many folders, where each is).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnId {
    Modified,
    Taken,
    Type,
    Size,
    Dimensions,
    Duration,
    Artist,
    Album,
}

impl ColumnId {
    /// The id the TypeScript used (and settings files hold): "modified", "taken"...
    pub fn as_str(self) -> &'static str {
        match self {
            ColumnId::Modified => "modified",
            ColumnId::Taken => "taken",
            ColumnId::Type => "type",
            ColumnId::Size => "size",
            ColumnId::Dimensions => "dimensions",
            ColumnId::Duration => "duration",
            ColumnId::Artist => "artist",
            ColumnId::Album => "album",
        }
    }

    /// The column with this id, if there is one.
    pub fn parse(id: &str) -> Option<ColumnId> {
        COLUMN_ORDER.iter().copied().find(|c| c.as_str() == id)
    }

    /// This column's definition (`COLUMNS[id]`).
    pub fn def(self) -> &'static ColumnDef {
        &COLUMNS[self as usize].1
    }
}

/// Where a column is left out as the list narrows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropAt {
    /// Left out first in a narrow list (under 42rem).
    Md,
    /// Left out only in a narrow pane (under 26rem).
    Sm,
}

/// One column's definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnDef {
    pub label: &'static str,
    /// Its width's name (`--col-<width>` on the web), dragged in the header (settings.columns).
    pub width: Column,
    pub sort: SortKey,
    /// Left out first in a narrow list, or only in a narrow pane.
    pub drop: Option<DropAt>,
    /// Read from what photos, videos and songs say about themselves (media.ts).
    pub facts: bool,
    /// Right-aligned: numbers that compare by their ends.
    pub end: bool,
}

const fn def(
    label: &'static str,
    width: Column,
    sort: SortKey,
    drop: Option<DropAt>,
    facts: bool,
    end: bool,
) -> ColumnDef {
    ColumnDef {
        label,
        width,
        sort,
        drop,
        facts,
        end,
    }
}

/// Every column's definition, in `ColumnId` order (the TypeScript record listed size last). Index with `ColumnId::def`.
///
/// NOTE(port): the record became an array; the position of each row is its `ColumnId` discriminant.
pub static COLUMNS: [(ColumnId, ColumnDef); 8] = [
    (
        ColumnId::Modified,
        def(
            "Date modified",
            Column::Date,
            SortKey::Modified,
            Some(DropAt::Sm),
            false,
            false,
        ),
    ),
    (
        ColumnId::Taken,
        def(
            "Date taken",
            Column::Taken,
            SortKey::Taken,
            Some(DropAt::Md),
            true,
            false,
        ),
    ),
    (
        ColumnId::Type,
        def("Type", Column::Type, SortKey::Type, Some(DropAt::Md), false, false),
    ),
    (
        ColumnId::Size,
        def("Size", Column::Size, SortKey::Size, None, false, true),
    ),
    (
        ColumnId::Dimensions,
        def(
            "Dimensions",
            Column::Dimensions,
            SortKey::Dimensions,
            Some(DropAt::Md),
            true,
            false,
        ),
    ),
    (
        ColumnId::Duration,
        def(
            "Duration",
            Column::Duration,
            SortKey::Duration,
            Some(DropAt::Md),
            true,
            true,
        ),
    ),
    (
        ColumnId::Artist,
        def("Artist", Column::Artist, SortKey::Artist, Some(DropAt::Md), true, false),
    ),
    (
        ColumnId::Album,
        def("Album", Column::Album, SortKey::Album, Some(DropAt::Md), true, false),
    ),
];

/// Sorts by what media files say about themselves (read before they sort).
pub const FACT_SORTS: [SortKey; 5] = [
    SortKey::Taken,
    SortKey::Dimensions,
    SortKey::Duration,
    SortKey::Artist,
    SortKey::Album,
];

/// Sorts whose first click puts the biggest, longest or newest first.
pub const DESCENDING: [SortKey; 5] = [
    SortKey::Modified,
    SortKey::Taken,
    SortKey::Size,
    SortKey::Dimensions,
    SortKey::Duration,
];

/// The order columns show in, whichever are chosen.
pub const COLUMN_ORDER: [ColumnId; 8] = [
    ColumnId::Modified,
    ColumnId::Taken,
    ColumnId::Type,
    ColumnId::Dimensions,
    ColumnId::Duration,
    ColumnId::Artist,
    ColumnId::Album,
    ColumnId::Size,
];

/// Chosen columns in their order (unknown ones left out). Takes the ids as text, as settings hold them.
pub fn in_order<S: AsRef<str>>(columns: &[S]) -> Vec<ColumnId> {
    COLUMN_ORDER
        .iter()
        .copied()
        .filter(|id| columns.iter().any(|c| c.as_ref() == id.as_str()))
        .collect()
}

/// Kinds of folder, each with the columns (and sort) it opens with until it's given its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FolderKind {
    General,
    Pictures,
    Videos,
    Music,
    Downloads,
    Recent,
    Gallery,
}

/// What a kind of folder opens with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KindDef {
    pub label: &'static str,
    pub columns: &'static [ColumnId],
    pub sort: Option<Sort>,
    pub view: Option<ViewMode>,
}

const NEWEST_FIRST: Option<Sort> = Some(Sort {
    key: SortKey::Modified,
    desc: true,
});

/// Every kind's definition, in the TypeScript record's key order. Use `FolderKind::def` to look one up.
pub static KINDS: [(FolderKind, KindDef); 7] = [
    (
        FolderKind::General,
        KindDef {
            label: "Any folder",
            columns: &[ColumnId::Modified, ColumnId::Type, ColumnId::Size],
            sort: None,
            view: None,
        },
    ),
    (
        FolderKind::Pictures,
        KindDef {
            label: "Pictures",
            columns: &[ColumnId::Taken, ColumnId::Dimensions, ColumnId::Type, ColumnId::Size],
            sort: None,
            view: None,
        },
    ),
    (
        FolderKind::Videos,
        KindDef {
            label: "Videos",
            columns: &[
                ColumnId::Modified,
                ColumnId::Duration,
                ColumnId::Dimensions,
                ColumnId::Size,
            ],
            sort: None,
            view: None,
        },
    ),
    (
        FolderKind::Music,
        KindDef {
            label: "Music",
            columns: &[ColumnId::Artist, ColumnId::Album, ColumnId::Duration, ColumnId::Size],
            sort: None,
            view: None,
        },
    ),
    (
        FolderKind::Downloads,
        KindDef {
            label: "Downloads",
            columns: &[ColumnId::Modified, ColumnId::Type, ColumnId::Size],
            sort: NEWEST_FIRST,
            view: None,
        },
    ),
    (
        FolderKind::Recent,
        KindDef {
            label: "Recent",
            columns: &[ColumnId::Modified, ColumnId::Size],
            sort: NEWEST_FIRST,
            view: None,
        },
    ),
    (
        FolderKind::Gallery,
        KindDef {
            label: "Gallery",
            columns: &[ColumnId::Modified, ColumnId::Dimensions, ColumnId::Size],
            sort: NEWEST_FIRST,
            view: Some(ViewMode::Grid),
        },
    ),
];

impl FolderKind {
    /// This kind's definition (`KINDS[kind]`).
    pub fn def(self) -> &'static KindDef {
        // Every kind has a row, in discriminant order.
        &KINDS[self as usize].1
    }
}

/// Places that decide a folder's kind by where they are (the `place` argument of `folderKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderPlace {
    Recent,
    Gallery,
    Downloads,
}

/// What kind of folder a listing is: Recent and Downloads by where they are, else by what's in it: mostly
/// pictures, videos or songs (at least five files, three in five of them of one kind), or any.
pub fn folder_kind(entries: &[FileEntry], place: Option<FolderPlace>) -> FolderKind {
    if let Some(place) = place {
        return match place {
            FolderPlace::Recent => FolderKind::Recent,
            FolderPlace::Gallery => FolderKind::Gallery,
            FolderPlace::Downloads => FolderKind::Downloads,
        };
    }
    let mut files = 0usize;
    let (mut pictures, mut videos, mut music) = (0usize, 0usize, 0usize);
    for e in entries {
        if e.dir {
            continue;
        }
        files += 1;
        let group = type_group(e);
        if *group == TYPE_GROUPS.images {
            pictures += 1;
        } else if *group == TYPE_GROUPS.video {
            videos += 1;
        } else if *group == TYPE_GROUPS.audio {
            music += 1;
        }
    }
    if files < 5 {
        return FolderKind::General;
    }
    // The first of the highest count wins a tie (the TypeScript's reduce kept `best` unless `next` was bigger).
    let mut best = (FolderKind::Pictures, pictures);
    for next in [(FolderKind::Videos, videos), (FolderKind::Music, music)] {
        if next.1 > best.1 {
            best = next;
        }
    }
    let (kind, count) = best;
    let files = files as f64;
    // Photos and videos together (a phone's camera folder): the kind there's more of.
    let media = (pictures + videos) as f64;
    if matches!(kind, FolderKind::Pictures | FolderKind::Videos) && media >= files * 0.6 {
        return if pictures >= videos {
            FolderKind::Pictures
        } else {
            FolderKind::Videos
        };
    }
    if count as f64 >= files * 0.6 {
        kind
    } else {
        FolderKind::General
    }
}

/// Where a column is left out as the list narrows: the date goes sooner among items from many folders.
fn drop_of(id: ColumnId, located: bool) -> Option<DropAt> {
    if located && id == ColumnId::Modified {
        Some(DropAt::Md)
    } else {
        id.def().drop
    }
}

/// The class that leaves a column's cells out as the list narrows (index.css): "col-optional", "col-narrow",
/// or none.
pub fn drop_class(id: ColumnId, located: bool) -> Option<&'static str> {
    match drop_of(id, located) {
        Some(DropAt::Md) => Some("col-optional"),
        Some(DropAt::Sm) => Some("col-narrow"),
        None => None,
    }
}

/// One track of the row's grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridTrack {
    /// The name: takes what's left (`minmax(0, 1fr)`).
    Name,
    /// Where each item is, in a list of items from many folders (`minmax(0, 0.9fr)`).
    Where,
    /// A column, as wide as its draggable width.
    Column(ColumnId),
}

/// The row's grid as data, for each width of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct GridTracks {
    /// As wide as it is.
    pub full: Vec<GridTrack>,
    /// Narrow (under 42rem).
    pub md: Vec<GridTrack>,
    /// In a narrow pane (under 26rem).
    pub sm: Vec<GridTrack>,
}

/// The row's grid: the name, where each item is (in a list of items from many folders), then the columns;
/// as wide as it is, narrow (under 42rem) and in a narrow pane (under 26rem).
pub fn grid_tracks(columns: &[ColumnId], located: bool) -> GridTracks {
    let start = || {
        let mut tracks = vec![GridTrack::Name];
        if located {
            tracks.push(GridTrack::Where);
        }
        tracks
    };
    let with = |keep: &dyn Fn(ColumnId) -> bool| {
        let mut tracks = start();
        tracks.extend(columns.iter().copied().filter(|&id| keep(id)).map(GridTrack::Column));
        tracks
    };
    GridTracks {
        full: with(&|_| true),
        md: with(&|id| drop_of(id, located) != Some(DropAt::Md)),
        sm: with(&|id| drop_of(id, located).is_none()),
    }
}

/// The CSS name of a column's width: `--col-<name>`.
pub fn width_name(width: Column) -> &'static str {
    match width {
        Column::Date => "date",
        Column::Type => "type",
        Column::Size => "size",
        Column::Taken => "taken",
        Column::Dimensions => "dimensions",
        Column::Duration => "duration",
        Column::Artist => "artist",
        Column::Album => "album",
    }
}

fn template(tracks: &[GridTrack]) -> String {
    tracks
        .iter()
        .map(|track| match track {
            GridTrack::Name => "minmax(0, 1fr)".to_string(),
            GridTrack::Where => "minmax(0, 0.9fr)".to_string(),
            GridTrack::Column(id) => format!("var(--col-{})", width_name(id.def().width)),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The row's grid as CSS templates, as the web UI used them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridTemplates {
    pub full: String,
    pub md: String,
    pub sm: String,
}

/// `grid_tracks` written as CSS grid templates (the same strings the TypeScript produced).
pub fn grid_templates(columns: &[ColumnId], located: bool) -> GridTemplates {
    let tracks = grid_tracks(columns, located);
    GridTemplates {
        full: template(&tracks.full),
        md: template(&tracks.md),
        sm: template(&tracks.sm),
    }
}

/// A song's or a video's length: "3:05", "1:02:03".
///
/// NOTE(port): takes seconds as `f64` and does the arithmetic the way JavaScript did (rounding half up,
/// `%` keeping the sign), so odd inputs (negative, NaN) give what the TypeScript gave.
pub fn clock_time(seconds: f64) -> String {
    let s = js_round(seconds);
    let (h, m, rest) = ((s / 3600.0).floor(), ((s % 3600.0) / 60.0).floor(), s % 60.0);
    let two = |n: f64| {
        let text = number_to_string(n);
        let missing = 2usize.saturating_sub(text.chars().count());
        format!("{}{}", "0".repeat(missing), text)
    };
    // JavaScript's `h ?` is false for 0 and NaN.
    if h != 0.0 && !h.is_nan() {
        format!("{}:{}:{}", number_to_string(h), two(m), two(rest))
    } else {
        format!("{}:{}", number_to_string(m), two(rest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entries::{with_details, RawEntry};

    /// `named(...)` of the check file: files of size 1 in "D:/music".
    fn named(names: &[&str]) -> Vec<FileEntry> {
        names
            .iter()
            .map(|name| {
                with_details(
                    RawEntry {
                        name: (*name).into(),
                        dir: false,
                        size: 1,
                        modified: 0,
                        attrs: 0,
                        ..RawEntry::default()
                    },
                    "D:/music",
                )
            })
            .collect()
    }

    // explorer.check.ts: folder kinds, columns, grid and clock
    #[test]
    fn folder_kind_by_what_is_in_it() {
        assert_eq!(
            folder_kind(&named(&["a.jpg", "b.jpg", "c.png", "d.heic", "e.txt"]), None),
            FolderKind::Pictures
        );
        assert_eq!(
            folder_kind(&named(&["a.mp4", "b.mov", "c.mkv", "d.jpg", "e.txt"]), None),
            FolderKind::Videos
        );
        assert_eq!(
            folder_kind(&named(&["a.mp3", "b.flac", "c.m4a", "d.mp3", "cover.jpg"]), None),
            FolderKind::Music
        );
        assert_eq!(
            folder_kind(&named(&["a.jpg", "b.jpg", "c.txt", "d.txt", "e.txt"]), None),
            FolderKind::General
        );
        // "too few to tell"
        assert_eq!(folder_kind(&named(&["a.jpg", "b.jpg"]), None), FolderKind::General);
        assert_eq!(
            folder_kind(&named(&["a.jpg"]), Some(FolderPlace::Recent)),
            FolderKind::Recent
        );
    }

    #[test]
    fn in_order_keeps_chosen_columns_in_their_order() {
        assert_eq!(
            in_order(&["size", "type", "bogus", "modified"]),
            vec![ColumnId::Modified, ColumnId::Type, ColumnId::Size]
        );
    }

    #[test]
    fn grid_templates_drop_columns_as_the_list_narrows() {
        let plain = grid_templates(&[ColumnId::Modified, ColumnId::Type, ColumnId::Size], false);
        assert_eq!(
            plain.full,
            "minmax(0, 1fr) var(--col-date) var(--col-type) var(--col-size)"
        );
        assert_eq!(plain.md, "minmax(0, 1fr) var(--col-date) var(--col-size)");
        assert_eq!(plain.sm, "minmax(0, 1fr) var(--col-size)");
        let found = grid_templates(&[ColumnId::Modified, ColumnId::Size], true);
        // "the date goes sooner among items from many folders"
        assert_eq!(found.md, "minmax(0, 1fr) minmax(0, 0.9fr) var(--col-size)");
    }

    #[test]
    fn grid_tracks_hold_the_same_layout_as_data() {
        let tracks = grid_tracks(&[ColumnId::Modified, ColumnId::Size], true);
        assert_eq!(
            tracks.full,
            vec![
                GridTrack::Name,
                GridTrack::Where,
                GridTrack::Column(ColumnId::Modified),
                GridTrack::Column(ColumnId::Size)
            ]
        );
        assert_eq!(
            tracks.md,
            vec![GridTrack::Name, GridTrack::Where, GridTrack::Column(ColumnId::Size)]
        );
    }

    #[test]
    fn drop_class_names_when_a_column_goes() {
        assert_eq!(drop_class(ColumnId::Modified, true), Some("col-optional"));
        assert_eq!(drop_class(ColumnId::Modified, false), Some("col-narrow"));
        assert_eq!(drop_class(ColumnId::Size, false), None);
    }

    #[test]
    fn clock_time_shows_minutes_or_hours() {
        assert_eq!(clock_time(185.0), "3:05");
        assert_eq!(clock_time(3723.0), "1:02:03");
    }

    #[test]
    fn tables_line_up_with_their_ids() {
        for (i, (id, def)) in COLUMNS.iter().enumerate() {
            assert_eq!(*id as usize, i);
            assert_eq!(id.def(), def);
        }
        for (i, (kind, def)) in KINDS.iter().enumerate() {
            assert_eq!(*kind as usize, i);
            assert_eq!(kind.def(), def);
        }
        assert_eq!(ColumnId::parse("album"), Some(ColumnId::Album));
        assert_eq!(ColumnId::parse("name"), None);
        assert_eq!(FolderKind::Gallery.def().view, Some(ViewMode::Grid));
    }
}
