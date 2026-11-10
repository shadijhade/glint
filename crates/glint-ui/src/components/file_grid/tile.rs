//! One tile and one row of tiles: what the grid draws for the rows on screen (the `renderTile` and `renderRow` of
//! FileGrid.tsx). Everything a tile needs for a frame is gathered once in a [`Frame`] when the grid renders, so the
//! list callback that draws the visible rows only reads it.

use super::thumbnail::{thumbnail_square, Thumbnails, Want};
use crate::components::{file_list::diff, git};
use crate::explorer::Comparison;
use crate::kit::prelude::*;
use crate::{i18n, t, theme};
use glint_model::entries::{
    cloud_state, is_hidden, is_locked, is_recycled, CloudState, DiffStatus, FileEntry, GitMarks,
};
use glint_model::file_types::{kind_of, type_label};
use glint_model::format::{format_full_date, format_size};
use glint_model::grid::GridTile;
use glint_model::paths::parent_path;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// A listener made by the grid for its tiles' elements: they get the tile's index with the event.
pub(super) type Handler<E> = Rc<dyn Fn(&E, &mut Window, &mut App)>;

/// Rows of tiles asked for above and below the visible ones, so their thumbnails are in before they scroll in
/// (`overscan={2}` in the TypeScript).
const OVERSCAN_ROWS: usize = 2;

/// Everything the visible tiles need, gathered when the grid renders.
pub(super) struct Frame {
    pub rows: crate::explorer::Rows,
    pub selection: crate::explorer::Selection,
    /// The item the keyboard is on.
    pub focus: Option<String>,
    /// In the pane the keyboard isn't acting on: the selection shows muted, as in an unfocused window.
    pub inactive: bool,
    pub flashed: Arc<HashSet<String>>,
    /// Cut to the clipboard: dim until pasted.
    pub cut: Arc<HashSet<String>>,
    /// Being dragged out of the window: dim until the drag is over.
    pub dragging: Option<Arc<HashSet<String>>>,
    /// The folder tile that files dragged over the grid would be dropped into (`dropTarget`).
    pub drop_tile: Option<String>,
    pub tile: GridTile,
    pub cols: usize,
    /// How many rows of tiles there are.
    pub lines: usize,
    /// One column's width, pixels.
    pub col_w: f32,
    /// Items from many folders (search results, the Recycle Bin's) say where each is, under its name.
    pub located: bool,
    /// The open folder's subfolders measured by "Folder sizes", by name (none for search results).
    pub sizes: Option<Arc<HashMap<String, u64>>>,
    /// The sizes are being measured now: a folder with none yet says "…".
    pub measuring: bool,
    /// What Git says of the folder's items (lowercase name to mark): a pill on the thumbnail, ignored ones faded.
    pub git: Option<Arc<GitMarks>>,
    /// How the items differ from the other pane's (compare panes): a pill on the thumbnail, the same ones faded.
    pub diffs: Option<Arc<Comparison>>,
    pub recycled: bool,
    /// The Gallery's tiles are its photos alone: where each is shows in the list, and in its preview.
    pub gallery: bool,
    /// The folder that was searched: a result says where it is below it.
    pub scope: String,
    pub lane: String,
    pub scale: f32,
    pub tokens: theme::Tokens,
    pub thumbs: Entity<Thumbnails>,
    pub viewer: EntityId,
    /// A fingerprint of the thumbnails last asked for, so the service is told only when the set changes.
    pub shown: Rc<Cell<u64>>,
    /// The scrub overlay of the video tile the pointer is on, and that tile's path.
    pub scrub: Option<(String, Entity<crate::components::preview::player::Scrub>)>,
    pub on_down: Handler<(usize, MouseDownEvent)>,
    pub on_right_up: Handler<(usize, MouseUpEvent)>,
    pub on_hover: Handler<(usize, bool)>,
    /// The rename box, drawn in place of the name of the item it is for.
    pub rename: Option<super::rename::RenameView>,
}

/// Where a search result is, below the folder that was searched ("" when right in it) (`relativeTo`).
pub(super) fn relative_to(scope: &str, path: &str) -> String {
    let folder = parent_path(path);
    if scope.is_empty() {
        return folder;
    }
    let base = scope.trim_end_matches(['\\', '/']).to_lowercase();
    let lower = folder.trim_end_matches(['\\', '/']).to_lowercase();
    if lower == base {
        return String::new();
    }
    for sep in ['\\', '/'] {
        let prefix = format!("{base}{sep}");
        if lower.starts_with(&prefix) {
            // `base` is lowercase and so is the prefix test, but the slice is from the original: same lengths for
            // every character that lowercases to one of its own length, which is all that a folder name has.
            return folder.chars().skip(base.chars().count() + 1).collect();
        }
    }
    folder
}

/// Where an item is, in a list of items from many folders: a search result's folder (below `scope`), or the folder
/// a recycled one was deleted from (`whereOf`).
pub(super) fn where_of(scope: &str, entry: &FileEntry) -> String {
    if is_recycled(entry) {
        entry.origin.clone().unwrap_or_default()
    } else {
        relative_to(scope, &entry.path)
    }
}

/// The size line under a tile: a file's size, a folder's where the listing has it (search results, the Recycle Bin:
/// `sizeOf` with `located`) or "Folder sizes" measured it, and "…" for a folder still being measured.
fn size_text(entry: &FileEntry, located: bool, sizes: Option<&HashMap<String, u64>>, measuring: bool) -> String {
    if entry.dir {
        let size = if located {
            (entry.size > 0).then_some(entry.size)
        } else {
            sizes.and_then(|sizes| sizes.get(&entry.name).copied())
        };
        return match size {
            Some(size) => format_size(size),
            None if measuring => "…".to_string(),
            None => String::new(),
        };
    }
    format_size(entry.size)
}

/// The text of a tile's tooltip (the `title` of the TypeScript's tile), built when it is asked for.
fn tooltip_text(frame: &Frame, entry: &FileEntry) -> SharedString {
    let mut lines = vec![entry.name.clone()];
    if frame.located && !frame.gallery {
        let place = where_of(&frame.scope, entry);
        if !place.is_empty() {
            lines.push(if frame.recycled {
                t!("deleted from {place}", place = place).to_string()
            } else {
                t!("in {place}", place = place).to_string()
            });
        }
    }
    if !entry.dir {
        lines.push(format!("{} · {}", type_label(entry), format_size(entry.size)));
    }
    let date = format_full_date(entry.modified);
    lines.push(if frame.recycled {
        t!("Deleted {date}", date = date).to_string()
    } else {
        date
    });
    // Git's line: the mark's sentence (a folder's says it of what is in it), or why the tile is faded.
    lines.extend(git_line(entry, frame.git.as_deref()));
    lines.join("\n").into()
}

/// The Git line of a tile's tooltip: the mark's sentence (a folder's says it of what is in it), or why the tile is
/// faded; none for an item Git has nothing to say of.
pub(super) fn git_line(entry: &FileEntry, marks: Option<&GitMarks>) -> Option<String> {
    let tracked = git::row_git(entry, marks);
    match tracked.badge {
        Some(badge) => Some(badge.title.to_string()),
        None if tracked.ignored => Some(t!("Git: left out by .gitignore").to_string()),
        None => None,
    }
}

/// The rows of tiles `range` (row numbers), as the list callback draws them. Also tells the thumbnail service what
/// is on screen, and frees what it let go of from the GPU's atlas.
pub(super) fn rows_in(frame: &Rc<Frame>, range: Range<usize>, window: &mut Window, cx: &mut App) -> Vec<AnyElement> {
    let cols = frame.cols.max(1);
    let len = frame.rows.len();
    let first = range.start.saturating_mul(cols).min(len);
    let end = range.end.saturating_mul(cols).min(len);
    // The list also asks for its first row alone, to measure it: that is no view of the grid, and telling the
    // service about it would make it drop the real ones and ask for them again every frame.
    let measuring = range == (0..1) && frame.lines > 1;
    if !measuring {
        show_thumbnails(
            frame,
            range.start.saturating_sub(OVERSCAN_ROWS).saturating_mul(cols).min(len)
                ..range.end.saturating_add(OVERSCAN_ROWS).saturating_mul(cols).min(len),
            cx,
        );
        for freed in frame.thumbs.update(cx, |thumbs, _| thumbs.take_evicted()) {
            window.drop_image(freed).ok();
        }
    }
    range
        .map(|r| {
            let mut tiles = Vec::with_capacity(cols);
            for i in (r * cols).max(first)..((r + 1) * cols).min(end) {
                if let Some(entry) = frame.rows.get(i) {
                    tiles.push(tile(frame, i, entry, cx));
                }
            }
            row_start()
                .id(("file-grid-row", r))
                .w_full()
                .h(px(frame.tile.height))
                .px(px(frame.tile.pad))
                .items_start()
                .children(tiles)
                .into_any_element()
        })
        .collect()
}

/// Tells the thumbnail service which thumbnails the rows in `items` need, if that changed since the last frame.
fn show_thumbnails(frame: &Frame, items: Range<usize>, cx: &mut App) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    frame.lane.hash(&mut hasher);
    frame.tile.thumb.to_bits().hash(&mut hasher);
    frame.scale.to_bits().hash(&mut hasher);
    for i in items.clone() {
        if let Some(entry) = frame.rows.get(i) {
            (&entry.path, entry.modified, entry.size).hash(&mut hasher);
        }
    }
    let fingerprint = hasher.finish();
    if frame.shown.get() == fingerprint {
        return;
    }
    frame.shown.set(fingerprint);
    let wants: Vec<Want> = items
        .filter_map(|i| frame.rows.get(i))
        .filter_map(|entry| Want::for_entry(entry, frame.tile.thumb, frame.scale))
        .collect();
    let (viewer, lane) = (frame.viewer, frame.lane.clone());
    frame
        .thumbs
        .update(cx, |thumbs, cx| thumbs.show(viewer, &lane, wants, cx));
}

/// One tile (`renderTile`).
fn tile(frame: &Rc<Frame>, i: usize, entry: &FileEntry, cx: &App) -> impl IntoElement {
    let t = &frame.tokens;
    let selected = frame.selection.contains(&entry.path);
    let focused = frame.focus.as_deref() == Some(entry.path.as_str());
    let shown_focus = focused && !frame.inactive;
    let thumb = frame.tile.thumb;

    let dropping = frame.drop_tile.as_deref() == Some(entry.path.as_str());
    let background = if dropping {
        // `bg-primary/20 outline-primary`: the folder the files would go into.
        t.primary.opacity(0.20)
    } else if selected {
        // `bg-primary/15 dark:bg-primary/20`; muted in the pane the keyboard isn't in.
        t.primary.opacity(match (frame.inactive, t.dark) {
            (true, _) => 0.10,
            (false, true) => 0.20,
            (false, false) => 0.15,
        })
    } else if frame.flashed.contains(&entry.path) {
        // `glint-flash`: just arrived (created, pasted, dropped, renamed).
        t.primary.opacity(0.25)
    } else {
        hsla(0., 0., 0., 0.)
    };
    let border = if shown_focus || dropping {
        t.primary
    } else if selected && frame.inactive {
        t.primary.opacity(0.25)
    } else {
        hsla(0., 0., 0., 0.)
    };

    let look = frame.thumbs.read(cx).look(entry, thumb, frame.scale);
    let kind = kind_of(entry);
    let icon_px = thumb * if entry.dir { 2. / 3. } else { 7. / 12. };
    let mut square = thumbnail_square(&look, kind, thumb, icon_px, true, cx);
    // How it differs from the other pane (top end) and Git's mark (top start) over the thumbnail.
    let tracked = git::row_git(entry, frame.git.as_deref());
    let status = frame.diffs.as_ref().and_then(|diffs| diffs.status(entry));
    if let Some(badge) = status.and_then(diff::diff_badge) {
        let pill = diff::badge_element(("file-grid-diff", i), &entry.name, &badge, cx);
        square = square.child(top_badge(true, pill, t));
    }
    if let Some(badge) = &tracked.badge {
        let pill = git::row::badge_element(("file-grid-git", i), &entry.name, badge, cx);
        square = square.child(top_badge(false, pill, t));
    }
    if let Some(state) = cloud_state(entry) {
        square = square.child(corner_mark(i, "cloud", cloud_mark(state, t), cx));
    }
    if is_locked(entry) {
        square = square.child(corner_mark(i, "lock", lock_mark(entry, t), cx));
    }

    if let Some((_, over)) = frame.scrub.as_ref().filter(|(path, _)| *path == entry.path) {
        square = square.child(over.clone());
    }

    let location = if frame.located && !frame.gallery {
        where_of(&frame.scope, entry)
    } else {
        String::new()
    };
    let size = size_text(entry, frame.located, frame.sizes.as_deref(), frame.measuring);
    let line = if location.is_empty() { size } else { location };

    let on_down = frame.on_down.clone();
    let on_down_right = frame.on_down.clone();
    let on_down_middle = frame.on_down.clone();
    let on_up = frame.on_right_up.clone();
    let on_hover = frame.on_hover.clone();
    let tip_frame = frame.clone();
    let faded = faded(entry, &frame.cut, frame.dragging.as_deref(), tracked.ignored);
    // The same in both panes: dimmer (`opacity-60`, which wins over the other dimming), unless selected.
    let faded = if !selected && status == Some(DiffStatus::Same) {
        Some(diff::SAME_OPACITY)
    } else {
        faded
    };
    let name = entry.name.clone();
    let renaming = frame.rename.as_ref().filter(|rename| rename.path == entry.path);
    // The name under the thumbnail, or the box that renames it.
    let label = match renaming {
        Some(rename) => super::rename::box_element(rename, t),
        None => div()
            .mt(space(1.5))
            .w_full()
            .text_size(theme::size::XS)
            .line_height(rems(1.))
            .line_clamp(2)
            .overflow_hidden()
            .child(isolate(name.clone()))
            .into_any_element(),
    };
    let renaming = renaming.is_some();

    div()
        // A cell of the row's grid; the tile inside is inset by 2 px (`m-0.5`).
        .w(px(frame.col_w))
        .h_full()
        .flex_none()
        .p(space(0.5))
        .child(
            div()
                .id(("file-grid-tile", i))
                .role(Role::ListBoxOption)
                .aria_label(name.clone())
                .aria_description(line.clone())
                .aria_selected(selected)
                .aria_position_in_set(i + 1)
                .aria_size_of_set(frame.rows.len())
                .when(shown_focus, |tile| tile.aria_active_descendant())
                .test_support()
                .size_full()
                .min_w_0()
                .flex()
                .flex_col()
                .items_center()
                .px(space(1.5))
                .pt(space(2.))
                .rounded(theme::size::RADIUS)
                .border_1()
                .border_color(border)
                .bg(background)
                .text_center()
                .when(!selected && !dropping, |tile| {
                    tile.hover(|hover| hover.bg(t.accent.opacity(0.6)))
                })
                .when_some(faded, |tile, opacity| tile.opacity(opacity))
                .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    cx.stop_propagation();
                    on_down(&(i, event.clone()), window, cx)
                })
                .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    cx.stop_propagation();
                    on_down_right(&(i, event.clone()), window, cx)
                })
                .on_mouse_down(MouseButton::Middle, move |event, window, cx| {
                    cx.stop_propagation();
                    on_down_middle(&(i, event.clone()), window, cx)
                })
                .on_mouse_up(MouseButton::Right, move |event, window, cx| {
                    cx.stop_propagation();
                    on_up(&(i, event.clone()), window, cx)
                })
                // Hovering a folder reads it ahead (`hoverFolder` and `leaveFolder`).
                .when(entry.dir || super::scrub::scrubbable(entry), |tile| {
                    tile.on_hover(move |hovered, window, cx| on_hover(&(i, *hovered), window, cx))
                })
                // Not over the box being typed in.
                .when(!renaming, |tile| {
                    tile.tooltip(move |window, cx| {
                        let text = tip_frame
                            .rows
                            .get(i)
                            .map(|entry| tooltip_text(&tip_frame, entry))
                            .unwrap_or_default();
                        crate::kit::tooltip(text, window, cx)
                    })
                })
                .child(square)
                .child(label)
                .child(
                    div()
                        .h(space(4.))
                        .w_full()
                        .flex_none()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(theme::size::XXS)
                        .text_color(t.muted_foreground)
                        .child(line),
                ),
        )
}

/// How dim a tile is drawn, if at all: hidden and cut items, and the ones `.gitignore` leaves out, are dim, as Explorer does (`opacity-55`); the ones being
/// dragged out of the window dimmer (`opacity-45`).
pub(super) fn faded(
    entry: &FileEntry,
    cut: &HashSet<String>,
    dragging: Option<&HashSet<String>>,
    ignored: bool,
) -> Option<f32> {
    if dragging.is_some_and(|paths| paths.contains(&entry.path)) {
        Some(0.45)
    } else if is_hidden(entry) || cut.contains(&entry.path) || ignored {
        Some(0.55)
    } else {
        None
    }
}

/// What a corner mark shows: its icon, color and the words a screen reader says.
struct Mark {
    glyph: IconName,
    color: Hsla,
    label: SharedString,
}

/// File Explorer's marks for cloud files' states (`CLOUD_MARKS`).
fn cloud_mark(state: CloudState, t: &theme::Tokens) -> Mark {
    match state {
        CloudState::Online => Mark {
            glyph: IconName::Cloud,
            color: t.primary,
            label: t!("Available when online"),
        },
        CloudState::Local => Mark {
            glyph: IconName::CircleCheck,
            color: t.success,
            label: t!("Available on this device"),
        },
        CloudState::Kept => Mark {
            glyph: IconName::CircleCheck,
            color: t.success,
            label: t!("Always available on this device"),
        },
        CloudState::Pending => Mark {
            glyph: IconName::RefreshCw,
            color: t.primary,
            label: t!("Sync pending"),
        },
    }
}

/// A key for a password, or a crossed-out file for what Glint can't read (`LockMark`, `lockReason`).
fn lock_mark(entry: &FileEntry, t: &theme::Tokens) -> Mark {
    let password = entry.attrs & glint_model::entries::attr::ENCRYPTED != 0;
    Mark {
        glyph: if password { IconName::KeyRound } else { IconName::FileX },
        color: t.warning,
        label: if password {
            t!("Password-protected")
        } else {
            t!("Compressed in a way Glint can't read")
        },
    }
}

/// A pill over the thumbnail's top corner at the reading direction's end (`end_side`: the diff badge,
/// `top-0.5 end-0.5`) or start (Git's mark, `top-0.5 start-0.5`), on the background so it reads over any picture.
fn top_badge(end_side: bool, pill: impl IntoElement, t: &theme::Tokens) -> Div {
    let corner = div()
        .absolute()
        .top(space(0.5))
        .rounded(theme::size::RADIUS_SM)
        .bg(t.background.opacity(0.9))
        .child(pill);
    // End is the right in English and the left in Arabic; start the other way round.
    if end_side != i18n::is_rtl() {
        corner.right(space(0.5))
    } else {
        corner.left(space(0.5))
    }
}

/// A small mark on the thumbnail's bottom corner at the reading direction's end (`absolute bottom-0.5 end-0.5 size-4`).
fn corner_mark(i: usize, what: &'static str, mark: Mark, _cx: &App) -> impl IntoElement {
    let corner = div()
        .id((SharedString::from(format!("file-grid-{what}")), i))
        .role(Role::Image)
        .aria_label(mark.label)
        .test_support()
        .absolute()
        .bottom(space(0.5))
        .size(rems(1.))
        .child(icon(mark.glyph).size(rems(1.)).text_color(mark.color));
    if i18n::is_rtl() {
        corner.left(space(0.5))
    } else {
        corner.right(space(0.5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, folder};

    #[test]
    fn results_say_where_they_are_below_the_folder_searched() {
        assert_eq!(relative_to("C:\\Users\\me", "C:\\Users\\me\\a.txt"), "");
        assert_eq!(relative_to("C:\\Users\\me", "C:\\Users\\me\\docs\\a.txt"), "docs");
        assert_eq!(
            relative_to("C:\\Users\\me\\", "C:\\USERS\\me\\docs\\old\\a.txt"),
            "docs\\old"
        );
        assert_eq!(relative_to("C:\\Users\\me", "D:\\other\\a.txt"), "D:\\other");
        // Searching everything has no scope: the whole folder.
        assert_eq!(relative_to("", "D:\\other\\a.txt"), "D:\\other");
    }

    #[test]
    fn cut_hidden_and_dragged_tiles_are_dimmed_by_how_much_the_web_app_dimmed_them() {
        let plain = file("C:\\x", "a.txt", 1, 1);
        let hidden = crate::explorer::fake::hidden("C:\\x", "h.txt");
        let none = HashSet::new();
        let only = |entry: &FileEntry| HashSet::from([entry.path.clone()]);
        assert_eq!(faded(&plain, &none, None, false), None);
        assert_eq!(faded(&hidden, &none, None, false), Some(0.55));
        assert_eq!(faded(&plain, &only(&plain), None, false), Some(0.55), "cut");
        // Being dragged out is dimmer than hidden or cut, whichever else it is.
        assert_eq!(faded(&plain, &none, Some(&only(&plain)), false), Some(0.45));
        assert_eq!(faded(&hidden, &only(&hidden), Some(&only(&hidden)), false), Some(0.45));
        // Another item being dragged doesn't dim this one.
        assert_eq!(faded(&plain, &none, Some(&only(&hidden)), false), None);
    }

    #[test]
    fn the_tooltip_says_what_git_says_of_a_tile_and_why_an_ignored_one_is_faded() {
        use glint_model::entries::GitMark;
        let marks: GitMarks = [
            ("a.txt".to_string(), GitMark::Modified),
            ("docs".to_string(), GitMark::Modified),
            ("out.log".to_string(), GitMark::Ignored),
            ("ملف.txt".to_string(), GitMark::Untracked),
        ]
        .into();
        let say = |entry: &FileEntry| git_line(entry, Some(&marks));
        assert_eq!(
            say(&file("C:\\x", "a.txt", 1, 1)).as_deref(),
            Some("Git: changed since it was committed")
        );
        assert_eq!(
            say(&folder("C:\\x", "docs")).as_deref(),
            Some("Git: something in here changed since it was committed"),
            "a folder's says it of what is in it"
        );
        assert_eq!(
            say(&file("C:\\x", "out.log", 1, 1)).as_deref(),
            Some("Git: left out by .gitignore")
        );
        assert_eq!(
            say(&file("C:\\x", "ملف.txt", 1, 1)).as_deref(),
            Some("Git: new, not added yet")
        );
        assert_eq!(say(&file("C:\\x", "other.txt", 1, 1)), None);
        assert_eq!(
            git_line(&file("C:\\x", "a.txt", 1, 1), None),
            None,
            "outside a repository"
        );
    }

    #[test]
    fn an_ignored_tile_is_dim_like_a_hidden_one() {
        let plain = file("C:\\x", "a.txt", 1, 1);
        let none = HashSet::new();
        assert_eq!(faded(&plain, &none, None, true), Some(0.55));
        let dragged = HashSet::from([plain.path.clone()]);
        assert_eq!(faded(&plain, &none, Some(&dragged), true), Some(0.45));
    }

    #[test]
    fn a_recycled_item_says_where_it_was_deleted_from() {
        let mut entry = file("C:\\bin", "a.txt", 1, 1);
        entry.attrs |= glint_model::entries::attr::RECYCLED;
        entry.origin = Some("C:\\Users\\me\\docs".into());
        assert_eq!(where_of("", &entry), "C:\\Users\\me\\docs");
    }

    #[test]
    fn a_folders_size_shows_only_where_the_listing_has_it() {
        let mut dir = folder("C:\\x", "d");
        assert_eq!(size_text(&dir, false, None, false), "");
        assert_eq!(size_text(&dir, true, None, false), "");
        dir.size = 2048;
        assert_eq!(size_text(&dir, false, None, false), "");
        assert_eq!(size_text(&dir, true, None, false), "2.0 KB");
        assert_eq!(size_text(&file("C:\\x", "a", 1024, 1), false, None, false), "1.0 KB");
    }

    #[test]
    fn a_folder_measured_here_shows_its_size_and_one_being_measured_shows_an_ellipsis() {
        let dir = folder("C:\\x", "d");
        let measured = HashMap::from([("d".to_string(), 3 * 1024u64)]);
        assert_eq!(size_text(&dir, false, Some(&measured), false), "3.0 KB");
        assert_eq!(
            size_text(&dir, false, Some(&measured), true),
            "3.0 KB",
            "a size it has stays"
        );
        let other = folder("C:\\x", "other");
        assert_eq!(size_text(&other, false, Some(&measured), true), "…");
        assert_eq!(size_text(&other, false, Some(&measured), false), "");
        assert_eq!(size_text(&file("C:\\x", "a", 1024, 1), false, None, true), "1.0 KB");
    }
}
