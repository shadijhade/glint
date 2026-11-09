//! Ported from src/lib/grid.ts.
//!
//! The grid view's arithmetic, kept free of the app so it can be tested on its own: its tile sizes,
//! where keys move, how many columns fit, and where it scrolls as they change.
//!
//! Pixel values are `f32`; counts and indices are `usize`. Where the TypeScript used `-1` for "none"
//! (a cursor, a focused tile) the Rust takes an `i64`, so the same call shapes keep working.
//!
//! Not ported: `decodeThumb`, `ThumbAnswer` and `ThumbSource`. They decode the byte payload of the
//! Tauri IPC `thumbnail` command, which the GPUI app does not have: it calls the thumbnail service
//! in-process.

use crate::js::js_round_f32;

/// One of the grid's tile sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileSize {
    /// The smallest tiles: a 4 rem thumbnail.
    Small,
    /// A 6 rem thumbnail, the size an unknown setting falls back to.
    Medium,
    /// An 8 rem thumbnail.
    Large,
    /// A 12 rem thumbnail.
    XLarge,
    /// The biggest tiles: a 16 rem thumbnail.
    Huge,
}

impl TileSize {
    /// The name stored in settings (the TypeScript `size` string).
    pub fn as_str(self) -> &'static str {
        match self {
            TileSize::Small => "small",
            TileSize::Medium => "medium",
            TileSize::Large => "large",
            TileSize::XLarge => "xlarge",
            TileSize::Huge => "huge",
        }
    }
}

/// A row of `TILE_SIZES`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileSpec {
    /// Which size this row is.
    pub size: TileSize,
    /// The name shown to the user for this size, in English (the UI translates it).
    pub label: &'static str,
    /// The square each thumbnail fits in, in rem. Multiply by the pixel size of one rem (the `rem`
    /// argument of `tile_layout`) to get pixels; at the usual 16 px text, 1 rem is 16 px.
    pub rem: f32,
}

/// The grid's tile sizes, smallest first: the square each thumbnail fits in, in rem (it scales with the text size).
pub const TILE_SIZES: [TileSpec; 5] = [
    TileSpec {
        size: TileSize::Small,
        label: "Small",
        rem: 4.0,
    },
    TileSpec {
        size: TileSize::Medium,
        label: "Medium",
        rem: 6.0,
    },
    TileSpec {
        size: TileSize::Large,
        label: "Large",
        rem: 8.0,
    },
    TileSpec {
        size: TileSize::XLarge,
        label: "Extra large",
        rem: 12.0,
    },
    TileSpec {
        size: TileSize::Huge,
        label: "Huge",
        rem: 16.0,
    },
];

/// Where a size is in `TILE_SIZES`; medium for one this version doesn't know (a setting saved by a newer one).
fn tile_index(size: &str) -> usize {
    TILE_SIZES.iter().position(|t| t.size.as_str() == size).unwrap_or(1)
}

/// The size `steps` up from `size` (down when negative), as far as the smallest and the biggest.
pub fn zoom_tiles(size: &str, steps: i64) -> TileSize {
    let last = TILE_SIZES.len() as i64 - 1;
    let at = (tile_index(size) as i64).saturating_add(steps).clamp(0, last);
    // The index is clamped to 0..=last just above.
    TILE_SIZES[at as usize].size
}

/// A grid tile's measurements.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridTile {
    /// The narrowest a tile gets (tiles widen to fill the row).
    pub width: f32,
    /// A tile's height.
    pub height: f32,
    /// The square its thumbnail or icon fits in.
    pub thumb: f32,
    /// The grid's side padding.
    pub pad: f32,
}

/// A grid tile at `size`, in pixels for a rem of `rem` pixels: the narrowest it gets (tiles widen to
/// fill the row), its height, the square its thumbnail or icon fits in, and the grid's side padding.
/// Its name, size and margins take the same room at every size.
pub fn tile_layout(rem: f32, size: &str) -> GridTile {
    let thumb = rem * TILE_SIZES[tile_index(size)].rem;
    GridTile {
        width: js_round_f32(thumb + rem * 1.5),
        height: js_round_f32(thumb + rem * 4.25),
        thumb,
        pad: rem * 0.5,
    }
}

/// A point in the list's content pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Pixels from the left edge of the list's content.
    pub x: f32,
    /// Pixels from the top edge of the list's content, scrolled part included.
    pub y: f32,
}

/// The items a marquee covers, its corners `a` and `b` in the list's content pixels (x from its left, y
/// from its top, scrolled part included), in order: in a list, every row it touches; in a grid, the tiles
/// of its `cols` equal columns, `pad` in from each side of `width`. Pass `cols` 1 (and `width`, `pad` 0)
/// for a list.
pub fn marquee_hits(
    a: Point,
    b: Point,
    count: usize,
    row_height: f32,
    cols: usize,
    width: f32,
    pad: f32,
) -> Vec<usize> {
    // `!(row_height > 0.0)` also rejects NaN, where the TypeScript's `<= 0` check would not.
    if count == 0 || row_height.is_nan() || row_height <= 0.0 {
        return Vec::new();
    }
    let cols = cols.max(1);
    let rows = count.div_ceil(cols);
    // Below the last row is the list's empty part: a marquee only there covers nothing.
    let r0 = (a.y.min(b.y) / row_height).floor().max(0.0);
    let r1 = (a.y.max(b.y) / row_height).floor().min((rows - 1) as f32);
    if r1.is_nan() || r0.is_nan() || r1 < r0 {
        return Vec::new();
    }
    let col_width = if cols > 1 {
        (width - 2.0 * pad) / cols as f32
    } else {
        0.0
    };
    let clamp_col = |x: f32| -> usize {
        if cols > 1 && col_width > 0.0 {
            // NOTE(port): the TypeScript produced NaN (and so no hits) for a zero or negative column
            // width; here such a grid selects column 0.
            ((x - pad) / col_width).floor().clamp(0.0, (cols - 1) as f32) as usize
        } else {
            0
        }
    };
    let (c0, c1) = (clamp_col(a.x.min(b.x)), clamp_col(a.x.max(b.x)));
    // Both are within 0..rows after the clamps above, so the casts cannot truncate.
    let (r0, r1) = (r0 as usize, r1 as usize);
    let mut hits = Vec::new();
    for r in r0..=r1 {
        for c in c0..=c1 {
            let i = r * cols + c;
            if i < count {
                hits.push(i);
            }
        }
    }
    hits
}

/// How a grid is laid out: tiles to a row, and each row's height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridLayout {
    /// How many tiles fit across a row (1 for a list).
    pub cols: usize,
    /// The height of one row of tiles in pixels, gaps included.
    pub row_height: f32,
}

/// Where a grid is scrolled to, as a place that outlasts its layout: the first tile of the row at the
/// top, plus how far into that row it's scrolled. A tab keeps this for when it's shown again.
/// A layout with no row height has no place: 0.
pub fn place_of(top: f32, at: GridLayout) -> f32 {
    if at.row_height.is_nan() || at.row_height <= 0.0 {
        return 0.0;
    }
    let row = (top / at.row_height).floor();
    row * at.cols.max(1) as f32 + top / at.row_height - row
}

/// The scroll position of `place` (see `place_of`) in a grid laid out `at`.
pub fn top_of(place: f32, at: GridLayout) -> f32 {
    let first = place.floor();
    ((first / at.cols.max(1) as f32).floor() + place - first) * at.row_height
}

/// Where a grid scrolls to when its layout changes under it (its tiles zoom, its pane narrows), so it
/// keeps its place: tile `keep` (the focused one, -1 for none) stays about where it was if any of it
/// was on screen, whole if it fits; otherwise the tiles at the top stay at the top. `top` and `height`
/// are where the view was scrolled to and how tall it is, `rows` how many rows there are now.
pub fn keep_place(was: GridLayout, now: GridLayout, top: f32, height: f32, rows: usize, keep: i64) -> f32 {
    let row = if keep >= 0 {
        (keep as usize / was.cols.max(1)) as f32
    } else {
        -1.0
    };
    let y = row * was.row_height - top;
    let next = if row >= 0.0 && y < height && y + was.row_height > 0.0 {
        // Its middle stays put, as far as the whole tile then fits.
        let at = (y + (was.row_height - now.row_height) / 2.0)
            .max(0.0)
            .min((height - now.row_height).max(0.0));
        (keep as usize / now.cols.max(1)) as f32 * now.row_height - at
    } else {
        top_of(place_of(top, was), now)
    };
    next.min(rows as f32 * now.row_height - height).max(0.0)
}

/// The thumbnail size to ask for, in device pixels, for a square of `css` pixels at a device pixel ratio
/// of `dpr`: what the screen shows one to one, so it's sharp (rounded up to a multiple of 4, so a few
/// sizes cover every text size and scale).
pub fn thumb_step(css: f32, dpr: f32) -> u32 {
    let px = (css * dpr - 0.01).ceil();
    let stepped = (px / 4.0).ceil() * 4.0;
    // Clamped to 16..=1024 (NaN falls to 16), so the cast cannot truncate.
    if stepped.is_nan() {
        return 16;
    }
    stepped.clamp(16.0, 1024.0) as u32
}

/// How many tiles at least `tile` pixels wide fit across `width`: one at the least.
pub fn grid_columns(width: f32, tile: f32) -> usize {
    if tile.is_nan() || tile <= 0.0 || width.is_nan() {
        return 1;
    }
    // Clamped below by 1 and finite above by f32's range, so the cast is exact enough to be safe.
    (width / tile).floor().max(1.0) as usize
}

/// Where a key moves the cursor in items laid out `cols` to a row (1 for the list), from `at` (-1 when
/// nothing is focused), with `page` rows on screen; `None` for a key that doesn't move it. As in
/// Explorer: Left and Right step through the items (only in a grid), Up and Down go a row, and Down
/// from above a short last row goes to its last item. Keys use the DOM names ("ArrowDown", "PageUp"...).
pub fn grid_move(key: &str, at: i64, count: usize, cols: usize, page: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let last = count as i64 - 1;
    let cols = cols.max(1) as i64;
    let page = page as i64;
    let row = |i: i64| i / cols;
    let to = |i: i64| Some(i.clamp(0, last) as usize);
    match key {
        "ArrowRight" => (cols > 1).then(|| at.saturating_add(1).min(last)).and_then(to),
        "ArrowLeft" => (cols > 1).then(|| at.saturating_sub(1).max(0)).and_then(to),
        "ArrowDown" => {
            if at < 0 {
                return Some(0);
            }
            if at.saturating_add(cols) <= last {
                return to(at + cols);
            }
            if row(at) < row(last) {
                to(last)
            } else {
                to(at)
            }
        }
        "ArrowUp" => {
            if at < 0 {
                return Some(0);
            }
            if at - cols >= 0 {
                to(at - cols)
            } else {
                to(at)
            }
        }
        "PageDown" => to(at.saturating_add(page.saturating_mul(cols)).max(0).min(last)),
        "PageUp" => to(at.saturating_sub(page.saturating_mul(cols)).max(0)),
        "Home" => Some(0),
        "End" => to(last),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: GridLayout = GridLayout {
        cols: 4,
        row_height: 100.0,
    };
    const BIG: GridLayout = GridLayout {
        cols: 2,
        row_height: 200.0,
    };

    fn mv(key: &str, at: i64, cols: usize, count: usize) -> Option<usize> {
        grid_move(key, at, count, cols, 2)
    }
    fn m4(key: &str, at: i64) -> Option<usize> {
        mv(key, at, 4, 10)
    }

    #[test]
    fn grid_move_left_and_right() {
        let got = [
            m4("ArrowRight", 3),
            m4("ArrowLeft", 4),
            m4("ArrowRight", 9),
            m4("ArrowLeft", 0),
        ];
        assert_eq!(got, [Some(4), Some(3), Some(9), Some(0)]);
    }

    #[test]
    fn grid_move_down_into_the_short_last_row() {
        let got = [
            m4("ArrowDown", 1),
            m4("ArrowDown", 5),
            m4("ArrowDown", 6),
            m4("ArrowDown", 9),
        ];
        assert_eq!(got, [Some(5), Some(9), Some(9), Some(9)]);
    }

    #[test]
    fn grid_move_up_and_from_nothing() {
        let got = [
            m4("ArrowUp", 9),
            m4("ArrowUp", 2),
            m4("ArrowDown", -1),
            m4("ArrowRight", -1),
        ];
        assert_eq!(got, [Some(5), Some(2), Some(0), Some(0)]);
    }

    #[test]
    fn grid_move_pages_home_end() {
        let got = [
            m4("PageDown", 1),
            m4("PageDown", 4),
            m4("PageUp", 9),
            m4("Home", 6),
            m4("End", 2),
        ];
        assert_eq!(got, [Some(9), Some(9), Some(1), Some(0), Some(9)]);
    }

    #[test]
    fn grid_move_other_keys_do_not_move() {
        assert_eq!(m4("Enter", 2), None);
    }

    #[test]
    fn grid_move_list_is_one_column() {
        let got = [
            mv("ArrowRight", 2, 1, 10),
            mv("ArrowDown", 2, 1, 10),
            mv("ArrowUp", 0, 1, 10),
            mv("PageDown", -1, 1, 10),
        ];
        assert_eq!(got, [None, Some(3), Some(0), Some(1)]);
    }

    #[test]
    fn grid_move_nothing_to_move_in() {
        assert_eq!(grid_move("ArrowDown", -1, 0, 4, 2), None);
    }

    #[test]
    fn grid_columns_fit_at_least_one() {
        assert_eq!(
            [
                grid_columns(1000.0, 120.0),
                grid_columns(100.0, 120.0),
                grid_columns(0.0, 120.0)
            ],
            [8, 1, 1]
        );
    }

    #[test]
    fn thumb_step_asks_in_device_pixels() {
        let got = [
            thumb_step(96.0, 1.0),
            thumb_step(96.0, 1.25),
            thumb_step(96.0, 1.5),
            thumb_step(16.0, 1.25),
            thumb_step(14.0, 1.75),
            thumb_step(900.0, 2.0),
            thumb_step(4.0, 1.0),
        ];
        assert_eq!(got, [96, 120, 144, 20, 28, 1024, 16]);
    }

    #[test]
    fn zoom_tiles_steps_and_stops_at_the_ends() {
        let got = [
            zoom_tiles("medium", 1),
            zoom_tiles("medium", -1),
            zoom_tiles("small", -1),
            zoom_tiles("huge", 1),
            zoom_tiles("small", 3),
            zoom_tiles("huge", -9),
        ];
        let want = [
            TileSize::Large,
            TileSize::Small,
            TileSize::Small,
            TileSize::Huge,
            TileSize::XLarge,
            TileSize::Small,
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn zoom_tiles_unknown_size_is_medium() {
        assert_eq!(
            [zoom_tiles("enormous", 0), zoom_tiles("enormous", 1)],
            [TileSize::Medium, TileSize::Large]
        );
    }

    #[test]
    fn tile_layout_medium_is_the_original_tile() {
        assert_eq!(
            tile_layout(16.0, "medium"),
            GridTile {
                width: 120.0,
                height: 164.0,
                thumb: 96.0,
                pad: 8.0
            }
        );
    }

    #[test]
    fn tile_layout_rounds_like_js() {
        let (a, b) = (tile_layout(14.0, "medium"), tile_layout(18.0, "medium"));
        assert_eq!([a.width, a.height, b.height], [105.0, 144.0, 185.0]);
    }

    #[test]
    fn tile_layout_other_sizes() {
        assert_eq!(
            tile_layout(16.0, "small"),
            GridTile {
                width: 88.0,
                height: 132.0,
                thumb: 64.0,
                pad: 8.0
            }
        );
        assert_eq!(
            tile_layout(16.0, "huge"),
            GridTile {
                width: 280.0,
                height: 324.0,
                thumb: 256.0,
                pad: 8.0
            }
        );
        assert_eq!(tile_layout(16.0, "?"), tile_layout(16.0, "medium"));
    }

    #[test]
    fn keep_place_focused_tile_stays_put() {
        // The focused tile (5, in the second row: its middle 150 px down) stays where it was.
        assert_eq!(keep_place(SMALL, BIG, 0.0, 300.0, 10, 5), 2.0 * 200.0 - 50.0);
        // Zoomed out again, it stays put too.
        assert_eq!(keep_place(BIG, SMALL, 350.0, 300.0, 3, 5), 0.0);
    }

    #[test]
    fn keep_place_off_screen_keeps_the_top() {
        assert_eq!(keep_place(SMALL, BIG, 250.0, 300.0, 10, 40), (4.0 + 0.5) * 200.0);
        assert_eq!(keep_place(SMALL, BIG, 250.0, 300.0, 10, -1), 900.0);
    }

    #[test]
    fn keep_place_tall_tiles_and_the_ends() {
        let one = GridLayout {
            cols: 1,
            row_height: 500.0,
        };
        assert_eq!(keep_place(SMALL, one, 0.0, 300.0, 40, 2), 2.0 * 500.0);
        assert_eq!(keep_place(SMALL, BIG, 0.0, 300.0, 2, 5), 100.0);
        assert_eq!(keep_place(BIG, SMALL, 900.0, 300.0, 3, -1), 0.0);
    }

    #[test]
    fn place_of_and_top_of_outlast_a_zoom() {
        let five = GridLayout {
            cols: 5,
            row_height: 100.0,
        };
        let got = [
            place_of(250.0, SMALL),
            top_of(place_of(250.0, SMALL), SMALL),
            top_of(8.5, BIG),
            top_of(8.5, five),
            place_of(0.0, BIG),
        ];
        assert_eq!(got, [8.5, 250.0, 900.0, 150.0, 0.0]);
    }

    fn pt(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    #[test]
    fn marquee_hits_in_a_list() {
        let hit = |a, b, count| marquee_hits(a, b, count, 28.0, 1, 0.0, 0.0);
        assert_eq!(hit(pt(10.0, 5.0), pt(300.0, 70.0), 10), [0, 1, 2]);
        // Dragged up and left.
        assert_eq!(hit(pt(300.0, 70.0), pt(10.0, 5.0), 10), [0, 1, 2]);
        // Past the end, the last rows.
        assert_eq!(hit(pt(0.0, 250.0), pt(0.0, 9000.0), 10), [8, 9]);
        // Only in the empty part below them, none.
        assert!(hit(pt(0.0, 300.0), pt(50.0, 400.0), 10).is_empty());
        assert!(hit(pt(0.0, 0.0), pt(0.0, 0.0), 0).is_empty());
    }

    #[test]
    fn marquee_hits_in_a_grid() {
        // 4 columns, 100 px each inside 8 px of padding, 120 px rows, 10 tiles: the second and third
        // columns of the first two rows, and none past the last tile.
        let hit = |a, b| marquee_hits(a, b, 10, 120.0, 4, 416.0, 8.0);
        assert_eq!(hit(pt(120.0, 10.0), pt(290.0, 130.0)), [1, 2, 5, 6]);
        assert_eq!(hit(pt(0.0, 250.0), pt(416.0, 260.0)), [8, 9]);
    }
}
