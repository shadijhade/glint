//! One line of the list: a row of an item, or the header of a group (`renderRow` and `renderLine` in
//! FileList.tsx). Built only for the lines on screen.

use super::{FileList, Frame, InsideInfo, Track};
use crate::components::file_grid::thumbnail::thumbnail_square;
use crate::components::git;
use crate::components::row_facts::{media_cells, Media, MediaCells};
use crate::dialogs::tags::tag_dots;
use crate::explorer::{HitLine, Line};
use crate::kit::prelude::*;
use crate::theme::{self, Tokens};
use crate::{i18n, t};
use glint_model::columns::ColumnId;
use glint_model::entries::{
    attr, cloud_state, is_archived, is_closed, is_hidden, is_locked, is_recycled, CloudState, FileEntry,
};
use glint_model::file_types::{kind_of, type_label};
use glint_model::format::{format_count, format_date, format_full_date, format_size};
use glint_model::groups::GroupHeader;
use glint_model::paths::parent_path;
use gpui_kit::{HighlightStyle, StyledText};
use std::collections::HashMap;

/// The height of a line, 1.875 rem (30 px at the usual size): a 28 px highlight with a pixel of air above and
/// below, so rows selected together read as rows, not as one slab.
pub const ROW_HEIGHT: Rems = rems(2.);
/// The air between a line's edge and its highlight, at the sides.
const INSET: Pixels = px(6.);
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

/// The entries on the lines `lines` of the list (a group's header line has none), for asking what the rows say.
pub(super) fn entries_on(
    rows: &crate::explorer::Rows,
    lines: std::ops::Range<usize>,
) -> impl Iterator<Item = &FileEntry> + '_ {
    lines.filter_map(move |line| match rows.groups() {
        Some(groups) => groups
            .lines
            .get(line)
            .copied()
            .and_then(|at| usize::try_from(at).ok())
            .and_then(|at| rows.get(at)),
        None => rows.get(line),
    })
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
        match frame.rows.line(line) {
            Line::Row(i) => self.row_element(frame, t, i, cx),
            Line::Header(index, header) => match frame.rows.sections() {
                // A search's two parts: what it found by name, and inside files.
                Some(sections) => self.section_header(frame, header, index, index == sections.inside, t, cx),
                None => group_header(header, index, t),
            },
            Line::Hit(at) => self.hit_element(frame, t, line, at, cx),
            Line::Blank => div().h(ROW_HEIGHT).into_any_element(),
        }
    }

    /// A line under a file's row: a line of the file that matched the search (its number, or where it is in a
    /// document, then the line with the matches marked), or the count of the matching lines that aren't shown.
    /// Pressing it is pressing its file's row.
    fn hit_element(&self, frame: &Frame, t: &Tokens, line: usize, at: HitLine, cx: &mut Context<Self>) -> AnyElement {
        let Some((entry, hit)) = frame.rows.hit(at) else {
            return div().h(ROW_HEIGHT).into_any_element();
        };
        let row = at.row as usize;
        let selected = frame.selected.contains(&entry.path);
        let rtl = i18n::is_rtl();
        let mono = theme::mono_font(cx);
        // Pressed, the line is what the preview of its file shows (a "more" line shows the first one not listed).
        let spot = hit.line.map(|line| (entry.path.clone(), line));
        // Under the name: past the row's inset, its padding, the icon and the gap after it.
        let indent = INSET + px(6.) + rems(1.).to_pixels(px(frame.box_px)) + px(8.);
        let (place, body, said): (SharedString, AnyElement, SharedString) = if at.more > 0 {
            let words = if at.more == 1 {
                t!("1 more matching line")
            } else {
                t!("{n} more matching lines", n = at.more)
            };
            (
                SharedString::default(),
                div()
                    .text_color(t.muted_foreground.opacity(0.8))
                    .child(words.clone())
                    .into_any_element(),
                words,
            )
        } else {
            let place: SharedString = hit
                .label
                .clone()
                .or_else(|| hit.line.map(|n| n.to_string()))
                .unwrap_or_default()
                .into();
            let plain: String = hit.parts.iter().map(|part| part.text.as_str()).collect();
            let said = match (&hit.label, hit.line) {
                (Some(label), _) => format!("{label}: {plain}"),
                (None, Some(n)) => t!("Line {n}: {text}", n = n, text = plain).to_string(),
                (None, None) => plain,
            };
            // A document's passage is prose, in the list's own font; a file's line is code.
            let prose = hit.label.is_some();
            (
                place,
                div()
                    .min_w_0()
                    .truncate()
                    .when(!prose, |d| d.font_family(mono.clone()))
                    .text_color(t.muted_foreground)
                    .child(hit_text(hit, t))
                    .into_any_element(),
                said.into(),
            )
        };
        let fill = row_start()
            .id(("file-hit-fill", line))
            .size_full()
            .min_w_0()
            .gap(space(2.5))
            .pe(1.5)
            .rounded(theme::size::RADIUS_ROW)
            .text_size(theme::size::XS)
            .map(|fill| {
                if selected {
                    fill.bg(t.selected.opacity(0.5))
                } else {
                    let over = t.accent;
                    fill.hover(move |style| style.bg(over))
                }
            })
            .child(
                div()
                    .w(rems(2.75))
                    .flex_none()
                    .truncate()
                    .text_end()
                    .when(hit.label.is_none(), |d| d.font_family(mono))
                    .text_color(t.muted_foreground.opacity(0.6))
                    .child(place),
            )
            .child(body);
        // The line that ties a file's lines to its row, under the middle of its icon.
        let guide = div().absolute().top_0().bottom_0().w(px(1.)).bg(t.stroke).map(|guide| {
            let at = INSET + px(6.) + rems(0.5).to_pixels(px(frame.box_px));
            if rtl {
                guide.right(at)
            } else {
                guide.left(at)
            }
        });
        div()
            .id(("file-hit", line))
            .role(Role::Button)
            .aria_label(said)
            .aria_description(entry.name.clone())
            .relative()
            .w_full()
            .h(ROW_HEIGHT)
            .flex_none()
            .py(px(1.))
            .pe(1.5)
            .map(|line| if rtl { line.pr(indent) } else { line.pl(indent) })
            .child(guide)
            .child(fill)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.row_down(row, event, window, cx);
                    if let Some((path, line)) = &spot {
                        crate::components::preview::spot(cx, path, *line);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| this.row_down(row, event, window, cx)),
            )
            .test_support()
            .into_any_element()
    }

    /// The header of one of a search's two parts: "Names" with how many, or "Inside files" with how many lines
    /// matched in how many files, a spinner and Stop while that is still being searched.
    fn section_header(
        &self,
        frame: &Frame,
        header: &GroupHeader,
        index: usize,
        inside: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let info: &InsideInfo = &frame.inside;
        let (glyph, name, counted): (IconName, SharedString, SharedString) = if inside {
            let lines = if info.matches == 1 {
                t!("1 line")
            } else {
                t!("{n} lines", n = format_count(info.matches))
            };
            let files = if info.files == 1 {
                t!("1 file")
            } else {
                t!("{n} files", n = format_count(info.files))
            };
            (
                IconName::TextSearch,
                t!("Inside files"),
                t!("{lines} in {files}", lines = lines, files = files),
            )
        } else {
            let items = if header.count == 1 {
                t!("1 item")
            } else {
                t!("{n} items", n = format_count(header.count))
            };
            (IconName::Search, t!("Names"), items)
        };
        let tab = frame.tab;
        let running = inside && info.running;
        row_start()
            .id(("file-section", index))
            .role(Role::Group)
            .aria_label(name.clone())
            .aria_description(counted.clone())
            .w_full()
            .h(ROW_HEIGHT)
            .flex_none()
            .items_center()
            .gap(space(2.))
            .px(space(3.))
            .text_size(theme::size::XS)
            .child(icon(glyph).size(rems(0.875)).text_color(t.primary))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.foreground)
                    .child(name),
            )
            .child(div().text_color(t.muted_foreground).child(counted))
            .children((inside && info.limited).then(|| div().text_color(t.warning).child(t!("stopped at the limit"))))
            .children(running.then(|| {
                row_start()
                    .gap(space(1.5))
                    .child(
                        div()
                            .id("file-section-searching")
                            .role(Role::ProgressIndicator)
                            .aria_label(t!("Searching inside files"))
                            .child(Spinner::new().xsmall())
                            .test_support(),
                    )
                    .child(
                        Button::new("file-section-stop")
                            .ghost()
                            .xsmall()
                            .label(t!("Stop"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.explorer.update(cx, |explorer, cx| explorer.stop_inside(tab, cx))
                            })),
                    )
            }))
            // A hairline to the end edge, so the header reads as the start of a part.
            .child(div().flex_1().h(px(1.)).bg(t.stroke))
            .test_support()
            .into_any_element()
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
        let type_text = type_label(entry);
        let date_text = format_date(entry.modified, None, i18n::language());
        // The date's title: the full date and time ("Deleted …" in the Recycle Bin).
        let full_date: SharedString = if frame.recycled {
            t!("Deleted {date}", date = format_full_date(entry.modified))
        } else {
            format_full_date(entry.modified).into()
        };
        let size_text = size_words(entry, size, frame.measuring);
        // What was read of the item: its tags (the dots), and what it says of itself while a column of that is
        // shown. Both are empty until read, as the web's were.
        let (tags, media) = {
            let facts = self.facts.read(cx);
            let tags = facts.tags_of(&frame.facts_key, &entry.path).unwrap_or(&[]).to_vec();
            let media = if frame.media == Media::None {
                MediaCells::default()
            } else {
                media_cells(entry, facts.media_of(&frame.facts_key, entry))
            };
            (tags, media)
        };

        // The name: its icon (a picture's or a video's own small image once it is in), then the name, or the box
        // it is renamed in.
        let renaming = frame.renaming.as_deref() == Some(entry.path.as_str());
        let mark = match self.look_of(frame, entry, cx) {
            Some(look) => thumbnail_square(&look, kind, frame.box_px, frame.box_px, false, cx).into_any_element(),
            None => match entry
                .dir
                .then(|| self.explorer.read(cx).known_id_of(&entry.path))
                .flatten()
            {
                Some(known) => crate::art::known_folder(known, px(frame.box_px), cx).into_any_element(),
                None => crate::art::kind(kind, px(frame.box_px), cx).into_any_element(),
            },
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
        // How the item differs from its namesake in the other pane (compare panes), then Git's pill.
        let diff = frame.diffs.as_ref().and_then(|diffs| diffs.status(entry));
        if let Some(badge) = diff.and_then(super::diff::diff_badge) {
            name_cell = name_cell.child(super::diff::badge_element(
                ("file-list-diff", i),
                &entry.name,
                &badge,
                cx,
            ));
        }
        if let Some(badge) = &tracked.badge {
            name_cell = name_cell.child(git::row::badge_element(("file-list-git", i), &entry.name, badge, cx));
        }
        if let Some(dots) = tag_dots(("file-list-tags", i), &tags, cx) {
            name_cell = name_cell.child(dots);
        }
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

        // The row's highlight: a rounded fill inside the line (hover, selection, the folder files will drop into),
        // holding the cells. The line around it is what is clicked, named and measured.
        let mut fill = row_start()
            .id(("file-row-fill", i))
            .relative()
            .size_full()
            .min_w_0()
            // With the line's inset and the border, text starts 12 px in: under the header's.
            .px(px(5.))
            .gap(space(3.))
            .rounded(theme::size::RADIUS_ROW)
            .border_1()
            .border_color(hsla(0., 0., 0., 0.))
            .text_size(theme::size::SM)
            .text_color(t.foreground)
            .child(name_cell);
        for track in &frame.tracks {
            fill = match *track {
                Track::Name => fill,
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
                    fill.child(
                        where_track(div().id(("file-list-where-cell", i)))
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(isolate(place))
                            .tooltip(move |window, cx| tooltip(full.clone(), window, cx)),
                    )
                }
                Track::Column { id, width } => fill.child(match id {
                    ColumnId::Modified => date_cell(i, width, date_text.clone(), full_date.clone(), hot, t),
                    ColumnId::Type => text_cell(width, type_text.clone(), false, t),
                    // What photos, videos and songs say about themselves (media.ts's facts), empty until read.
                    ColumnId::Taken => taken_cell(i, width, &media, t),
                    ColumnId::Dimensions => text_cell(width, media.dimensions.clone(), false, t),
                    ColumnId::Duration => text_cell(width, media.duration.clone(), true, t),
                    ColumnId::Artist => said_cell(("file-list-artist", i), width, media.artist.clone(), t),
                    ColumnId::Album => said_cell(("file-list-album", i), width, media.album.clone(), t),
                    ColumnId::Size => size_cell(width, size_text.clone(), percent, t),
                }),
            };
        }

        // The item the keyboard is on, in the pane the keyboard is in.
        let current = focused && !frame.inactive;
        fill = if dropping {
            // The folder the files will go into: tinted and outlined.
            fill.bg(t.primary.opacity(0.20)).border_color(t.primary)
        } else if selected {
            if frame.inactive {
                // The pane the keyboard isn't in: a quiet gray, as an unfocused window shows it.
                fill.bg(t.selected_inactive)
            } else {
                let over = theme::selected_fill(cx, true);
                fill.bg(theme::selected_fill(cx, false))
                    .border_color(t.primary.opacity(0.22))
                    .hover(move |style| style.bg(over))
            }
        } else {
            let over = t.accent;
            fill.hover(move |style| style.bg(over))
        };
        if current && selected {
            // The current item: Windows 11's accent bar at the start edge.
            let bar = div()
                .absolute()
                .top(rems(0.4375))
                .bottom(rems(0.4375))
                .w(px(3.))
                .rounded_full()
                .bg(theme::glint_fill(cx));
            fill = fill.child(if i18n::is_rtl() {
                bar.right(px(-1.))
            } else {
                bar.left(px(-1.))
            });
        }
        // The keyboard's place when it isn't plain from the selection: on an item that isn't selected, or on
        // one of several that are.
        if current && (!selected || frame.selected.len() > 1) {
            fill = fill.border_color(t.primary.opacity(0.7));
        }
        let fill =
            if flashed {
                // The web's `glint-flash`: the ring holds for the first fifth of 1.15 s, then fades out.
                let primary = t.primary;
                fill.with_animation(("file-row-flash", i), Animation::new(FLASH), move |fill, delta| {
                    match flash_strength(delta) {
                        strength if strength > 0. => fill
                            .bg(primary.opacity(0.25 * strength))
                            .border_color(primary.opacity(0.85 * strength)),
                        _ => fill,
                    }
                })
                .into_any_element()
            } else {
                fill.into_any_element()
            };

        let mut line = div()
            .id(("file-row", i))
            .role(Role::ListBoxOption)
            .aria_label(entry.name.clone())
            .aria_description(with_media(describe(&type_text, &size_text, &date_text), &media))
            .aria_selected(selected)
            .aria_position_in_set(i + 1)
            .aria_size_of_set(frame.rows.len())
            .w_full()
            .h(ROW_HEIGHT)
            .flex_none()
            .px(INSET)
            .py(px(1.))
            .child(fill);
        if let Some(opacity) = dimness(dim, dragged) {
            line = line.opacity(opacity);
        }
        // Identical in both panes: dimmed (`opacity-60`, which wins over the other dimming) unless selected.
        if !selected && frame.same_as_other(entry) {
            line = line.opacity(super::diff::SAME_OPACITY);
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

/// How long a flash lasts (`glint-flash`'s 1.15 s).
pub(super) const FLASH: std::time::Duration = std::time::Duration::from_millis(1150);

/// How strong the flash's ring is `delta` (0 to 1) of the way through [`FLASH`]: full until a fifth of the way,
/// then easing out to nothing (the keyframes' `cubic-bezier(0.22, 1, 0.36, 1)` between them, close enough as a quint).
pub(super) fn flash_strength(delta: f32) -> f32 {
    const HOLD: f32 = 0.2;
    if delta <= HOLD {
        return 1.;
    }
    let along = ((delta - HOLD) / (1. - HOLD)).clamp(0., 1.);
    (1. - along).powi(5)
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

/// What the row's description says besides its kind, size and date: what its file says of itself, in the columns
/// of that (a screen reader hears them as the cells are drawn: the web read them out of the cells).
fn with_media(described: String, media: &MediaCells) -> String {
    [
        described.as_str(),
        media.taken.as_str(),
        media.dimensions.as_str(),
        media.duration.as_str(),
        media.artist.as_str(),
        media.album.as_str(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(", ")
}

/// A line that matched a search, with the matches marked. The matches' places are byte offsets into the text
/// that is drawn; a line that starts right to left is isolated first, which moves them past the mark.
fn hit_text(hit: &crate::explorer::Hit, t: &Tokens) -> StyledText {
    let mut joined = String::new();
    let mut ranges: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
    for part in &hit.parts {
        let start = joined.len();
        joined.push_str(&part.text);
        if part.hit {
            ranges.push((
                start..joined.len(),
                HighlightStyle {
                    color: Some(t.foreground),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    background_color: Some(t.primary.opacity(0.28)),
                    ..Default::default()
                },
            ));
        }
    }
    let shown = isolate(joined.clone());
    let shift = if shown.len() == joined.len() {
        0
    } else {
        '\u{2067}'.len_utf8()
    };
    let ranges = ranges
        .into_iter()
        .map(|(range, style)| (range.start + shift..range.end + shift, style))
        .collect::<Vec<_>>();
    StyledText::new(shown).with_highlights(ranges)
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
        .pb(space(1.))
        .border_b_1()
        .border_color(t.border.opacity(0.7))
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

/// When it was taken: the date the file says in text close to the foreground's, else (for a picture or video that
/// doesn't say) its modified date, dimmer. The tooltip says which.
fn taken_cell(i: usize, width: Pixels, media: &MediaCells, t: &Tokens) -> AnyElement {
    let color = if media.taken_says {
        t.foreground.opacity(0.8)
    } else {
        t.muted_foreground.opacity(0.7)
    };
    let cell = div()
        .id(("file-list-taken", i))
        .w(width)
        .flex_none()
        .truncate()
        .text_start()
        .text_size(theme::size::XS)
        .text_color(color)
        .child(isolate(media.taken.clone()));
    match media.taken_title.clone().filter(|_| !media.taken.is_empty()) {
        Some(title) => {
            let title: SharedString = title.into();
            cell.tooltip(move |window, cx| tooltip(title.clone(), window, cx))
                .test_support()
                .into_any_element()
        }
        None => cell.test_support().into_any_element(),
    }
}

/// A plain text a file says (its artist, its album), with the text itself as its tooltip for when it is cut.
fn said_cell(id: (&'static str, usize), width: Pixels, text: String, t: &Tokens) -> AnyElement {
    let cell = div()
        .id(id)
        .w(width)
        .flex_none()
        .truncate()
        .text_start()
        .text_size(theme::size::XS)
        .text_color(t.muted_foreground);
    if text.is_empty() {
        return cell.test_support().into_any_element();
    }
    let title: SharedString = text.clone().into();
    cell.child(isolate(text))
        .tooltip(move |window, cx| tooltip(title.clone(), window, cx))
        .test_support()
        .into_any_element()
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
            .bg(t.primary.opacity(if t.dark { 0.16 } else { 0.11 }));
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
    fn the_flash_holds_then_fades_to_nothing() {
        assert_eq!(flash_strength(0.), 1.);
        assert_eq!(flash_strength(0.2), 1.);
        let (a, b, c) = (flash_strength(0.3), flash_strength(0.5), flash_strength(0.9));
        assert!(1. > a && a > b && b > c && c > 0., "{a} {b} {c}");
        assert_eq!(flash_strength(1.), 0.);
        assert_eq!(flash_strength(2.), 0.);
    }

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
    fn what_a_file_says_of_itself_is_added_to_its_rows_description() {
        let plain = MediaCells::default();
        assert_eq!(with_media("TXT file, 1 B".to_string(), &plain), "TXT file, 1 B");
        let said = MediaCells {
            taken: "Mar 3".into(),
            dimensions: "10 × 20".into(),
            duration: "3:05".into(),
            artist: "Nour".into(),
            ..MediaCells::default()
        };
        assert_eq!(
            with_media("MP3 file, 3 MB".to_string(), &said),
            "MP3 file, 3 MB, Mar 3, 10 × 20, 3:05, Nour"
        );
    }

    #[test]
    fn the_entries_on_some_lines_are_the_rows_there() {
        use crate::explorer::Rows;
        let rows = Rows::identity(vec![file("C:\\x", "a", 1, 0), file("C:\\x", "b", 1, 0)].into());
        let names = |lines: std::ops::Range<usize>| -> Vec<String> {
            entries_on(&rows, lines).map(|e| e.name.clone()).collect()
        };
        assert_eq!(names(0..2), ["a", "b"]);
        assert_eq!(names(1..9), ["b"], "past the end is left out");
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
