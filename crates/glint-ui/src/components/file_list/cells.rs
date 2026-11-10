//! One line of the list: a row of an item, or the header of a group (`renderRow` and `renderLine` in
//! FileList.tsx). Built only for the lines on screen.

use super::{FileList, Frame, Track};
use crate::components::file_grid::thumbnail::thumbnail_square;
use crate::components::git;
use crate::kit::prelude::*;
use crate::theme::{self, Tokens};
use crate::{i18n, t};
use glint_model::columns::ColumnId;
use glint_model::entries::{
    attr, cloud_state, is_archived, is_closed, is_hidden, is_locked, is_recycled, CloudState, FileEntry,
};
use glint_model::file_types::{kind_of, type_label};
use glint_model::format::{format_date, format_full_date, format_size};
use glint_model::groups::GroupHeader;
use glint_model::paths::parent_path;
use std::collections::HashMap;

/// The height of a line, 1.75 rem (`rowHeight`: 28 px at the usual size).
pub const ROW_HEIGHT: Rems = rems(1.75);
/// An item modified within the last hour gets a dot in its date.
const HOT_FOR_MS: i64 = 3_600_000;
/// A size bar only shows where it says something: 40% of the biggest here or more.
const BAR_FROM: f64 = 40.;
/// How many rows the biggest size is looked for in (`Math.min(rows.length, 4000)`).
pub const BIGGEST_OF: usize = 4000;

/// A folder's size: the index's total for a search result (or the Recycle Bin's, with `located`), else its size
/// measured here (`sizes`: the subfolders of the open folder, by name, from `Explorer::size_run`) (`sizeOf`). A
/// folder nobody has measured has none.
pub fn size_of(entry: &FileEntry, located: bool, sizes: Option<&HashMap<String, u64>>) -> Option<u64> {
    if entry.dir {
        if located {
            (entry.size > 0).then_some(entry.size)
        } else {
            sizes.and_then(|sizes| sizes.get(&entry.name).copied())
        }
    } else {
        Some(entry.size)
    }
}

/// What the Size column says: the size, or "…" for a folder whose size is still being measured, else nothing.
pub fn size_words(entry: &FileEntry, size: Option<u64>, measuring: bool) -> String {
    match size {
        Some(size) => format_size(size),
        None if entry.dir && measuring => "…".to_string(),
        None => String::new(),
    }
}

/// A folder that files dragged in can be dropped on: not one in an archive (read-only), nor one Windows keeps
/// closed, nor one in the Recycle Bin (`dropPath`). The drag part uses it to answer a drop over a row.
pub fn drop_path(entry: &FileEntry) -> Option<&str> {
    (entry.dir && !is_archived(entry) && !is_closed(entry) && !is_recycled(entry)).then_some(entry.path.as_str())
}

/// Why an item in an archive can't be opened, for its lock mark (`lockReason`).
pub fn lock_reason(entry: &FileEntry) -> SharedString {
    if entry.attrs & attr::ENCRYPTED != 0 {
        t!("Password-protected")
    } else {
        t!("Compressed in a way Glint can't read")
    }
}

/// The size bar's share: whole percent of the biggest, or 0 below 40% (`sizePct`).
pub fn size_percent(size: Option<u64>, max: u64) -> u32 {
    let Some(size) = size.filter(|&s| s > 0) else { return 0 };
    if max == 0 {
        return 0;
    }
    let share = size as f64 / max as f64 * 100.;
    if share >= BAR_FROM {
        // Math.round for positive values.
        (share + 0.5).floor() as u32
    } else {
        0
    }
}

/// The biggest size among the first rows, which the bars are shares of.
pub fn biggest(rows: &crate::explorer::Rows, located: bool, sizes: Option<&HashMap<String, u64>>) -> u64 {
    (0..rows.len().min(BIGGEST_OF))
        .filter_map(|i| rows.get(i).and_then(|e| size_of(e, located, sizes)))
        .max()
        .unwrap_or(0)
}

/// File Explorer's words and marks for a cloud file's state.
fn cloud_mark(state: CloudState, t: &Tokens) -> (IconName, SharedString, Hsla) {
    match state {
        CloudState::Online => (IconName::Cloud, t!("Available when online"), sky(t)),
        CloudState::Local => (IconName::CircleCheck, t!("Available on this device"), t.success),
        CloudState::Kept => (IconName::CircleCheck, t!("Always available on this device"), t.success),
        CloudState::Pending => (IconName::RefreshCw, t!("Sync pending"), sky(t)),
    }
}

/// `text-sky-500`, the mark of a file that is not (or not yet) on this device.
fn sky(t: &Tokens) -> Hsla {
    theme::tailwind("sky-500").unwrap_or(t.tk_number)
}

impl FileList {
    /// The line `line` of the list: an item's row, or a group's header.
    pub(super) fn line_element(&self, frame: &Frame, t: &Tokens, line: usize, cx: &mut Context<Self>) -> AnyElement {
        match frame.rows.groups() {
            Some(groups) => match groups.lines.get(line).copied() {
                Some(at) if at >= 0 => self.row_element(frame, t, at as usize, cx),
                Some(at) => match groups.headers.get((-at - 1) as usize) {
                    Some(header) => group_header(header, (-at - 1) as usize, t),
                    None => div().h(ROW_HEIGHT).into_any_element(),
                },
                None => div().h(ROW_HEIGHT).into_any_element(),
            },
            None => self.row_element(frame, t, line, cx),
        }
    }

    /// An item's row: its icon and name, then a cell for every column the list is wide enough for.
    fn row_element(&self, frame: &Frame, t: &Tokens, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(entry) = frame.rows.get(i) else {
            return div().h(ROW_HEIGHT).into_any_element();
        };
        let selected = frame.selected.contains(&entry.path);
        let focused = frame.focus_path.as_deref() == Some(entry.path.as_str());
        let flashed = frame.flashed.contains(&entry.path);
        let size = size_of(entry, frame.located, frame.sizes.as_deref());
        let percent = size_percent(size, frame.max_size);
        let hot = !frame.recycled && entry.modified > 0 && (0..HOT_FOR_MS).contains(&(frame.now - entry.modified));
        // What Git says of the item: a badge after its name, and the ones .gitignore leaves out are dimmed.
        let tracked = git::row_git(entry, frame.git.as_deref());
        // Hidden and cut items are dimmed, and so are the ones .gitignore leaves out; an item being dragged out is
        // dimmer still (45%); a folder files are dragged over (see `drop_path`) is tinted and outlined.
        let dim = is_dim(entry, frame.cut.contains(&entry.path), tracked.ignored);
        let dragged = frame.dragging.contains(&entry.path);
        let dropping = frame.drop_row.as_deref() == Some(entry.path.as_str());
        let kind = kind_of(entry);
        let tone = theme::kind_tone(kind, cx);
        let type_text = type_label(entry);
        let date_text = format_date(entry.modified, None, i18n::language());
        // The date's title: the full date and time ("Deleted …" in the Recycle Bin).
        let full_date: SharedString = if frame.recycled {
            t!("Deleted {date}", date = format_full_date(entry.modified))
        } else {
            format_full_date(entry.modified).into()
        };
        let size_text = size_words(entry, size, frame.measuring);

        // The name: its icon (a picture's or a video's own small image once it is in), then the name, or the box
        // it is renamed in.
        let renaming = frame.renaming.as_deref() == Some(entry.path.as_str());
        let mark = match self.look_of(frame, entry, cx) {
            Some(look) => thumbnail_square(&look, kind, frame.box_px, frame.box_px, false, cx).into_any_element(),
            None => icon(super::kind_icon(kind.icon))
                .size(rems(1.))
                .text_color(tone.color)
                .flex_none()
                .into_any_element(),
        };
        let name_tip: SharedString = if tracked.ignored {
            git::name_tooltip(&entry.name, true)
        } else {
            isolate(entry.name.clone())
        };
        let mut name_cell = row_start()
            .id(("file-list-name", i))
            .gap(space(2.))
            .flex_1()
            .min_w_0()
            .child(mark)
            .tooltip(move |window, cx| tooltip(name_tip.clone(), window, cx));
        name_cell = match self.rename.as_ref().filter(|_| renaming) {
            Some(rename) => name_cell.child(self.rename_input(rename, cx)),
            None => name_cell.child(div().min_w_0().truncate().child(isolate(entry.name.clone()))),
        };
        // PORT ME(compare): the "Only here / Newer / Different / Older" badges (the Git badge goes after them).
        if let Some(badge) = &tracked.badge {
            name_cell = name_cell.child(git::row::badge_element(("file-list-git", i), &entry.name, badge, cx));
        }
        // PORT ME(tags): the tag dots.
        if let Some(state) = cloud_state(entry) {
            let (name, label, color) = cloud_mark(state, t);
            name_cell = name_cell.child(
                div()
                    .id(("file-list-cloud", i))
                    .role(Role::Image)
                    .aria_label(label)
                    .flex_none()
                    .child(icon(name).size(rems(0.875)).text_color(color))
                    .test_support(),
            );
        }
        if is_locked(entry) {
            // A key for a password, a crossed-out file for what Glint can't read.
            let mark = if entry.attrs & attr::ENCRYPTED != 0 {
                IconName::KeyRound
            } else {
                IconName::FileX
            };
            let label = lock_reason(entry);
            name_cell = name_cell.child(
                div()
                    .id(("file-list-lock", i))
                    .role(Role::Image)
                    .aria_label(label)
                    .flex_none()
                    .child(icon(mark).size(rems(0.875)).text_color(t.warning))
                    .test_support(),
            );
        }

        let mut line = row_start()
            .id(("file-row", i))
            .role(Role::ListBoxOption)
            .aria_label(entry.name.clone())
            .aria_description(describe(&type_text, &size_text, &date_text))
            .aria_selected(selected)
            .aria_position_in_set(i + 1)
            .aria_size_of_set(frame.rows.len())
            .w_full()
            .h(ROW_HEIGHT)
            .flex_none()
            .px(space(3.))
            .gap(space(3.))
            .border_1()
            .border_color(hsla(0., 0., 0., 0.))
            .text_size(theme::size::SM)
            .text_color(t.foreground)
            .child(name_cell);
        for track in &frame.tracks {
            line = match *track {
                Track::Name => line,
                Track::Where => {
                    let place = match (&frame.where_of, frame.recycled) {
                        (_, true) => entry.origin.clone().unwrap_or_default(),
                        (Some(relative), false) => relative(&entry.path),
                        (None, false) => String::new(),
                    };
                    // Its title was the folder it is in (where it was deleted from, in the Recycle Bin).
                    let full = isolate(if frame.recycled {
                        entry.origin.clone().unwrap_or_default()
                    } else {
                        parent_path(&entry.path)
                    });
                    line.child(
                        where_track(div().id(("file-list-where-cell", i)))
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(isolate(place))
                            .tooltip(move |window, cx| tooltip(full.clone(), window, cx)),
                    )
                }
                Track::Column { id, width } => line.child(match id {
                    ColumnId::Modified => date_cell(i, width, date_text.clone(), full_date.clone(), hot, t),
                    ColumnId::Type => text_cell(width, type_text.clone(), false, t),
                    // PORT ME(media): what photos, videos and songs say about themselves (media.ts's facts):
                    // the dates they were taken, their dimensions, durations, artists and albums. Empty until
                    // the media part reads them, as the web's cells were before they were read.
                    ColumnId::Taken | ColumnId::Dimensions | ColumnId::Artist | ColumnId::Album => {
                        text_cell(width, String::new(), false, t)
                    }
                    ColumnId::Duration => text_cell(width, String::new(), true, t),
                    ColumnId::Size => size_cell(width, size_text.clone(), percent, t),
                }),
            };
        }

        line = if dropping {
            // The folder the files will go into: `bg-primary/20` and an outline.
            line.bg(t.primary.opacity(0.20)).border_color(t.primary)
        } else if selected {
            let tint = if t.dark { 0.20 } else { 0.15 };
            if frame.inactive {
                // The pane the keyboard isn't in: muted, with a faint ring, as an unfocused window shows it.
                line.bg(t.primary.opacity(if t.dark { 0.12 } else { 0.10 }))
                    .border_color(t.primary.opacity(0.25))
            } else {
                line.bg(t.primary.opacity(tint))
            }
        } else {
            line.hover(|style| style.bg(t.accent.opacity(0.6)))
        };
        if focused && !frame.inactive {
            line = line.border_color(t.primary);
        }
        if flashed {
            // PORT ME(coordinator): the web's `glint-flash` fades its ring out over 1.15 s; this holds it
            // while the explorer says the item is flashed (1.2 s).
            line = line.bg(t.primary.opacity(0.25)).border_color(t.primary.opacity(0.85));
        }
        if let Some(opacity) = dimness(dim, dragged) {
            line = line.opacity(opacity);
        }
        if focused {
            line = line.aria_active_descendant();
        }

        let folder = entry.dir.then(|| entry.path.clone());
        let line = line
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| this.row_down(i, event, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| this.row_down(i, event, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| this.row_down(i, event, window, cx)),
            );
        let line = match folder {
            // The pointer resting on a folder reads it ahead; leaving it stops that (`hoverFolder`).
            Some(path) => line.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.explorer.update(cx, |explorer, cx| {
                    if *hovered {
                        explorer.hover_folder(&path, cx);
                    } else {
                        explorer.leave_folder();
                    }
                });
            })),
            None => line,
        };
        line.test_support().into_any_element()
    }
}

/// Whether a row is dimmed: hidden items, items cut to the clipboard and the ones `.gitignore` leaves out.
pub(super) fn is_dim(entry: &FileEntry, cut: bool, ignored: bool) -> bool {
    is_hidden(entry) || cut || ignored
}

/// How dim a row is drawn: hidden and cut items to 55%, an item being dragged out to 45% (`opacity-55`,
/// `opacity-45`); None for a plain row.
pub(super) fn dimness(dim: bool, dragged: bool) -> Option<f32> {
    if dragged {
        Some(0.45)
    } else if dim {
        Some(0.55)
    } else {
        None
    }
}

/// Where an item is takes a little less of the free width than the name does (`minmax(0, 0.9fr)` against
/// `minmax(0, 1fr)`).
pub(super) fn where_track<E: Styled>(cell: E) -> E {
    let mut cell = cell.flex_1().min_w_0();
    cell.style().flex_grow = Some(0.9);
    cell
}

/// "Document, 12 KB, Today 9:31 PM": what a row says besides its name, for a screen reader.
fn describe(kind: &str, size: &str, date: &str) -> String {
    [kind, size, date]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A group's header line: its name, how many items it has and their size.
fn group_header(header: &GroupHeader, index: usize, t: &Tokens) -> AnyElement {
    let count = if header.count == 1 {
        t!("{n} item", n = header.count)
    } else {
        t!("{n} items", n = header.count)
    };
    let counted = if header.size > 0 {
        format!("{count} · {}", format_size(header.size))
    } else {
        count.to_string()
    };
    // The web hid these from a screen reader (`role="presentation"`); a named group says which kind the rows
    // after it belong to.
    row_start()
        .id(("file-group", index))
        .role(Role::Group)
        .aria_label(header.label.clone())
        .aria_description(counted.clone())
        .w_full()
        .h(ROW_HEIGHT)
        .flex_none()
        .items_end()
        .gap(space(2.))
        .px(space(3.))
        .pb(space(0.5))
        .border_b_1()
        .border_color(t.border.opacity(0.6))
        .text_size(theme::size::XS)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.foreground)
                .child(isolate(header.label.clone())),
        )
        .child(div().text_color(t.muted_foreground).child(counted))
        .test_support()
        .into_any_element()
}

/// A plain column cell: a clipped line of small muted text; numbers that compare by their ends sit at the end.
fn text_cell(width: Pixels, text: String, end: bool, t: &Tokens) -> AnyElement {
    let cell = div()
        .w(width)
        .flex_none()
        .truncate()
        .text_size(theme::size::XS)
        .text_color(t.muted_foreground);
    let cell = if end { cell.text_end() } else { cell.text_start() };
    cell.child(isolate(text)).into_any_element()
}

/// The date: small and muted, with a green dot when it changed in the last hour. Its tooltip is the full date.
fn date_cell(i: usize, width: Pixels, text: String, full: SharedString, hot: bool, t: &Tokens) -> AnyElement {
    row_start()
        .id(("file-list-date", i))
        .w(width)
        .flex_none()
        .gap(space(1.5))
        .overflow_hidden()
        .text_size(theme::size::XS)
        .text_color(t.muted_foreground)
        .when(hot, |this| {
            this.child(
                div()
                    .id("file-list-hot")
                    .role(Role::Image)
                    .aria_label(t!("Modified within the last hour"))
                    .size(rems(0.375))
                    .flex_none()
                    .rounded_full()
                    .bg(t.success),
            )
        })
        .child(div().min_w_0().truncate().child(text))
        .tooltip(move |window, cx| tooltip(full.clone(), window, cx))
        .into_any_element()
}

/// The size: at the end of its cell, over a bar showing its share of the biggest here.
fn size_cell(width: Pixels, text: String, percent: u32, t: &Tokens) -> AnyElement {
    let bar = (percent > 0).then(|| {
        let bar = div()
            .absolute()
            .top(space(0.5))
            .bottom(space(0.5))
            .w(relative(percent as f32 / 100.))
            .rounded(theme::size::RADIUS_SM)
            .bg(t.primary.opacity(if t.dark { 0.18 } else { 0.12 }));
        // The bar grows from the end edge.
        if i18n::is_rtl() {
            bar.left_0()
        } else {
            bar.right_0()
        }
    });
    row_start()
        .relative()
        .w(width)
        .flex_none()
        .h(rems(1.25))
        .justify_end()
        .overflow_hidden()
        .rounded(theme::size::RADIUS_SM)
        .px(space(1.))
        .text_size(theme::size::XS)
        .text_color(t.muted_foreground)
        .children(bar)
        .child(div().relative().truncate().child(text))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, folder};

    #[test]
    fn a_folder_has_a_size_only_where_the_index_gave_one() {
        let dir = folder("C:\\x", "docs");
        assert_eq!(
            size_of(&dir, false, None),
            None,
            "a folder in a plain listing has none until it is measured"
        );
        assert_eq!(size_of(&dir, true, None), None, "fd's results come without a size (0)");
        let mut sized = folder("C:\\x", "docs");
        sized.size = 4096;
        assert_eq!(size_of(&sized, true, None), Some(4096));
        assert_eq!(size_of(&file("C:\\x", "a.txt", 10, 0), false, None), Some(10));
        // A folder measured here has the size measured, and a search result keeps the index's.
        let measured = HashMap::from([("docs".to_string(), 7_000u64)]);
        assert_eq!(size_of(&dir, false, Some(&measured)), Some(7_000));
        assert_eq!(size_of(&dir, true, Some(&measured)), None);
        assert_eq!(size_of(&sized, true, Some(&measured)), Some(4096));
        assert_eq!(size_of(&folder("C:\\x", "other"), false, Some(&measured)), None);
    }

    #[test]
    fn a_folder_being_measured_says_so_until_it_has_a_size() {
        let dir = folder("C:\\x", "docs");
        assert_eq!(size_words(&dir, None, true), "…");
        assert_eq!(size_words(&dir, None, false), "");
        assert_eq!(
            size_words(&dir, Some(2048), true),
            "2.0 KB",
            "a measured folder keeps its size"
        );
        let a_file = file("C:\\x", "a.txt", 10, 0);
        assert_eq!(size_words(&a_file, Some(10), true), "10 B");
    }

    #[test]
    fn only_a_folder_that_can_take_files_is_a_drop_target() {
        let plain = folder("C:\\x", "docs");
        assert_eq!(drop_path(&plain), Some("C:\\x\\docs"));
        let mut closed = folder("C:\\x", "locked");
        closed.attrs |= attr::CLOSED;
        let mut archived = folder("C:\\x", "inside");
        archived.attrs |= attr::IN_ARCHIVE;
        let mut recycled = folder("C:\\x", "gone");
        recycled.attrs |= attr::RECYCLED;
        for no in [closed, archived, recycled, file("C:\\x", "a.txt", 1, 0)] {
            assert_eq!(drop_path(&no), None, "{}", no.name);
        }
    }

    #[test]
    fn a_bar_shows_only_from_forty_percent_of_the_biggest() {
        assert_eq!(size_percent(Some(100), 100), 100);
        assert_eq!(size_percent(Some(40), 100), 40);
        assert_eq!(size_percent(Some(39), 100), 0);
        assert_eq!(size_percent(Some(0), 100), 0);
        assert_eq!(size_percent(None, 100), 0);
        assert_eq!(size_percent(Some(5), 0), 0);
        // Halves round up, as Math.round does.
        assert_eq!(size_percent(Some(405), 1000), 41);
    }

    #[test]
    fn hidden_cut_and_git_ignored_items_are_dimmed() {
        let plain = file("C:\\x", "a.txt", 1, 0);
        assert!(!is_dim(&plain, false, false));
        assert!(is_dim(&plain, true, false), "cut");
        assert!(is_dim(&plain, false, true), ".gitignore leaves it out");
        let mut hidden = file("C:\\x", ".secret", 1, 0);
        hidden.attrs |= attr::HIDDEN;
        assert!(is_dim(&hidden, false, false));
    }

    #[test]
    fn a_row_is_described_by_what_it_has() {
        assert_eq!(
            describe("PDF file", "12 KB", "Today 9:31 PM"),
            "PDF file, 12 KB, Today 9:31 PM"
        );
        assert_eq!(
            describe("File folder", "", "Today 9:31 PM"),
            "File folder, Today 9:31 PM"
        );
        assert_eq!(describe("", "", ""), "");
    }
}
