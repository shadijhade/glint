//! The details view's columns as pixels: which ones fit, how wide each is, where a search result is.
//!
//! Ported from the grid part of src/lib/columns.ts (`gridTemplates`, `dropClass`) and index.css (`.file-row`'s
//! container queries and `--col-*` widths), and from `relativeTo` in FileList.tsx. The web drew each row as a
//! CSS grid; GPUI has no grid, so a row is a flex row whose cells have these widths (the name takes what's left).
//! Everything here is pure, so it is easy to test.

use glint_model::columns::{grid_tracks, ColumnId, GridTrack};
use glint_model::paths::parent_path;
use glint_model::store::{Column, ColumnWidths};

/// The narrowest and widest a column can be dragged (`ColumnEdge` in FileList.tsx).
pub const MIN_WIDTH: f32 = 40.;
pub const MAX_WIDTH: f32 = 480.;

/// A column's width when nobody dragged it, in rems (`--col-*` in index.css).
pub fn default_rems(width: Column) -> f32 {
    match width {
        Column::Date | Column::Taken => 9.5,
        Column::Type => 8.,
        Column::Size => 5.5,
        Column::Dimensions => 7.,
        Column::Duration => 4.5,
        Column::Artist | Column::Album => 9.,
    }
}

/// The width dragged for a column, in pixels, if there is one (`settings.columns[column]`).
pub fn saved_width(widths: &ColumnWidths, width: Column) -> Option<f64> {
    match width {
        Column::Date => widths.date,
        Column::Type => widths.r#type,
        Column::Size => widths.size,
        Column::Taken => widths.taken,
        Column::Dimensions => widths.dimensions,
        Column::Duration => widths.duration,
        Column::Artist => widths.artist,
        Column::Album => widths.album,
    }
}

/// Stores a dragged width, or forgets it with `None` (a double-click on the edge: "as it was").
pub fn set_saved_width(widths: &mut ColumnWidths, width: Column, value: Option<f64>) {
    let slot = match width {
        Column::Date => &mut widths.date,
        Column::Type => &mut widths.r#type,
        Column::Size => &mut widths.size,
        Column::Taken => &mut widths.taken,
        Column::Dimensions => &mut widths.dimensions,
        Column::Duration => &mut widths.duration,
        Column::Artist => &mut widths.artist,
        Column::Album => &mut widths.album,
    };
    *slot = value;
}

/// A dragged width kept between the limits, in whole pixels (`Math.round(Math.min(480, Math.max(40, w)))`).
pub fn clamp_width(width: f32) -> f32 {
    // Math.round rounds halves up; the limits are whole, so the clamp can come first.
    (width.clamp(MIN_WIDTH, MAX_WIDTH) + 0.5).floor()
}

/// The tracks of a row for a list this wide, in rems: all of them, the narrow list's (under 42rem), a narrow
/// pane's (under 26rem) or just the names (under 15rem), as index.css's container queries chose them. The
/// queries are inclusive (`max-width`).
pub fn tracks_for(columns: &[ColumnId], located: bool, width_in_rems: f32) -> Vec<GridTrack> {
    let tracks = grid_tracks(columns, located);
    if width_in_rems <= 15. {
        vec![GridTrack::Name]
    } else if width_in_rems <= 26. {
        tracks.sm
    } else if width_in_rems <= 42. {
        tracks.md
    } else {
        tracks.full
    }
}

/// Where a search result is, below the folder that was searched ("" when right in it) (`relativeTo`).
pub fn relative_to(scope: &str) -> impl Fn(&str) -> String {
    let scope = scope.to_string();
    let base = scope.trim_end_matches(['\\', '/']).to_lowercase();
    move |path: &str| {
        let folder = parent_path(path);
        if scope.is_empty() {
            return folder;
        }
        let lower = folder.trim_end_matches(['\\', '/']).to_lowercase();
        if lower == base {
            return String::new();
        }
        if lower.starts_with(&format!("{base}\\")) || lower.starts_with(&format!("{base}/")) {
            // `folder.slice(base.length + 1)` counted UTF-16 units; characters are what a folder name has.
            return folder.chars().skip(base.chars().count() + 1).collect();
        }
        folder
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_are_kept_between_the_limits_and_whole() {
        assert_eq!(clamp_width(10.), 40.);
        assert_eq!(clamp_width(900.), 480.);
        assert_eq!(clamp_width(123.4), 123.);
        // Halves round up, as Math.round does.
        assert_eq!(clamp_width(123.5), 124.);
    }

    #[test]
    fn dragged_widths_round_trip_through_the_settings() {
        let mut widths = ColumnWidths::default();
        assert_eq!(saved_width(&widths, Column::Size), None);
        set_saved_width(&mut widths, Column::Size, Some(120.));
        set_saved_width(&mut widths, Column::Type, Some(90.));
        assert_eq!(saved_width(&widths, Column::Size), Some(120.));
        assert_eq!(saved_width(&widths, Column::Type), Some(90.));
        set_saved_width(&mut widths, Column::Size, None);
        assert_eq!(saved_width(&widths, Column::Size), None);
        assert_eq!(saved_width(&widths, Column::Date), None);
    }

    #[test]
    fn columns_drop_out_as_the_list_narrows() {
        let columns = [ColumnId::Modified, ColumnId::Type, ColumnId::Size];
        let ids = |tracks: Vec<GridTrack>| -> Vec<Option<ColumnId>> {
            tracks
                .into_iter()
                .map(|t| match t {
                    GridTrack::Column(id) => Some(id),
                    _ => None,
                })
                .collect()
        };
        // Wide: the name, then every column.
        assert_eq!(
            ids(tracks_for(&columns, false, 60.)),
            [
                None,
                Some(ColumnId::Modified),
                Some(ColumnId::Type),
                Some(ColumnId::Size)
            ]
        );
        // Under 42rem the type goes first; under 26rem the date goes too; under 15rem only names.
        assert_eq!(
            ids(tracks_for(&columns, false, 40.)),
            [None, Some(ColumnId::Modified), Some(ColumnId::Size)]
        );
        assert_eq!(ids(tracks_for(&columns, false, 20.)), [None, Some(ColumnId::Size)]);
        assert_eq!(ids(tracks_for(&columns, false, 10.)), [None]);
        // In a list of items from many folders the date goes sooner, and "where" is a track of its own.
        let located = tracks_for(&columns, true, 40.);
        assert_eq!(located.len(), 3, "name, where, size");
        assert!(matches!(located[1], GridTrack::Where));
    }

    #[test]
    fn a_result_says_where_it_is_below_the_folder_searched() {
        let relative = relative_to("C:\\Users\\me");
        assert_eq!(relative("C:\\Users\\me\\a.txt"), "");
        assert_eq!(relative("C:\\USERS\\me\\Docs\\b\\a.txt"), "Docs\\b");
        assert_eq!(relative("D:\\other\\a.txt"), "D:\\other");
        // A trailing separator on the scope doesn't matter.
        let relative = relative_to("C:\\Users\\me\\");
        assert_eq!(relative("C:\\Users\\me\\Docs\\a.txt"), "Docs");
        // No scope: the whole folder.
        let everywhere = relative_to("");
        assert_eq!(everywhere("C:\\x\\a.txt"), "C:\\x");
        // Arabic folder names are counted in characters, not bytes.
        let arabic = relative_to("C:\\ملفات");
        assert_eq!(arabic("C:\\ملفات\\تقارير\\a.txt"), "تقارير");
    }
}
