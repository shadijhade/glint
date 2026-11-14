//! The status bar under the file list: counts, the selection, how the folder was read and shown, the folder's
//! breakdown by kind, the search run, and instant search's state. Ported from src/components/explorer/StatusBar.tsx.
//!
//! The bar follows the active tab and keeps no copy of the explorer's state: [`summary::Summary`] is worked out
//! from the explorer when it changes, and redrawn with the bar. Instant search is read through [`index`], which asks
//! glint-core in the background (the explorer doesn't hold the index).
//!
//! **What's here and what isn't.** The counts, the selection and its size, Copy paths, the work line (read and
//! shown times, archives, the index, searches), instant search's progress and its turn-on button, the "What's in
//! this folder" popover with its breakdown, filter chips and "How long it took". StatusBar.tsx has no view switch
//! and no zoom (those live in the toolbar), so the bar has neither.
//!
//! The thumbnails' line (`N thumbnails in X`) comes from the file grid's burst for the tab, and shows only while the
//! tab shows the grid, as the TypeScript did.
//!
//! **In the dock.** The bar is the dock at the foot of the sheet: its counts at the start edge, its work line at the
//! end, and the commands' island over its middle. The window says how much room each end has beside the island
//! ([`StatusBar::beside`]); each end then says what fits there (`Wide::beside`) and is cut at its own room, so
//! nothing of it runs under the island. What acted on the selection from here (Rename, Zip, Copy paths) is the
//! island's while the bar is docked. Alone in a window (tests), the bar goes by the window's width as before.
//!
//! **Keyboard.** The breakdown's segments are tab stops: Enter or Space filters to the kind, as a click does.
//!
//! The work line also says how long Git status took in the folder (`git · 12 ms`, from the xl width) and, once
//! "Folder sizes" has measured the folder, `index sized 1.2 GB in 3 ms`; the popover's "How long it took" has a
//! "Folder sizes" and a "Git status" stage for them. The bar redraws when the Git model changes (it must exist,
//! `git::init`, when the bar is made).
//!
//! **Buttons for the selection.** One selected file (in a folder that can be changed) offers File X-Ray; several
//! offer Rename (the batch rename dialog, as F2 opens it) and Zip (a zip beside the items, after looking for
//! secrets, as "Compress to > Zip file" does). X-Ray and Rename show from the medium width, Zip from the large.
//!
//! **Narrow windows.** The TypeScript used container queries (`@lg:` and so on) to drop the least important words
//! first. GPUI has none, so the bar reads the window's width in rems instead: the same breakpoints, measured on the
//! window rather than on the bar (the bar spans the window's full width).

pub mod index;
pub(crate) mod summary;
#[cfg(test)]
mod tests;

use crate::components::file_grid::thumbnail::{self, Thumbnails};
use crate::components::git;
use crate::explorer::{self, Explorer};
use crate::kit::prelude::*;
use crate::{i18n, t, theme};
use index::{Index, IndexModel, Source, Volume};
use std::cell::Cell;
use std::rc::Rc;
use summary::{
    chip_active, chip_title, counts, drives, items, segment_title, stages, thumbs_line, thumbs_stage, thumbs_title,
    Category, Stage, Summary, Thumbs, Work,
};

/// The bar's height: the dock's, at the foot of the sheet (the commands' island sits over its middle).
pub const HEIGHT: Pixels = px(52.);
/// The widths the bar drops words at, in rems (the TypeScript's `@2xs`, `@md`, `@lg` and `@xl` containers).
const TWO_XS: f32 = 18.;
const MEDIUM: f32 = 28.;
const LARGE: f32 = 32.;
const EXTRA_LARGE: f32 = 36.;
/// How many kinds the popover lists as chips (the TypeScript's top six).
const CHIPS: usize = 6;

/// The status bar: a GPUI entity the workspace places at the bottom of the window.
pub struct StatusBar {
    explorer: Entity<Explorer>,
    index: Entity<IndexModel>,
    /// The index's last answer, copied from the model when it changes (so a frame doesn't clone it).
    index_info: Option<Index>,
    summary: Summary,
    /// The selection's counts, kept from one change of the explorer to the next.
    counted: summary::SelectedCount,
    /// The explorer changed since the summary was made.
    dirty: bool,
    /// The "What's in this folder" popover is open.
    details_open: bool,
    /// Where the last press outside the popover landed. The click that follows that press, on the same spot, is
    /// the press closing the popover, so it doesn't open it again.
    dismissed_at: Option<Point<Pixels>>,
    details_focus: FocusHandle,
    /// Turning instant search on is under way; the button waits for it.
    enabling: bool,
    /// The file grid's thumbnails: the bar reads the tab's burst from them.
    thumbs: Entity<Thumbnails>,
    /// One focus handle per segment of the breakdown, so the keyboard can reach them (kept across frames).
    segment_focus: Vec<FocusHandle>,
    /// In the dock: how wide each of the bar's two ends may be, in pixels, beside the commands' island over its
    /// middle (the window says, each frame). Zero while the bar has its whole width to itself.
    beside: Rc<Cell<f32>>,
    _subscriptions: Vec<Subscription>,
}

impl StatusBar {
    /// Where the window says how wide each end of the bar may be, in pixels, while the commands' island sits
    /// over its middle (zero: the whole width is the bar's).
    pub fn beside(&self) -> Rc<Cell<f32>> {
        self.beside.clone()
    }
}

/// Start-up: the status bars read instant search through glint-core. Call after `core_bridge::init`.
pub fn init(cx: &mut App) {
    index::init(cx);
}

/// [`init`] with an index source of your own (tests, tools).
pub fn init_with(cx: &mut App, source: Source) {
    index::init_with(cx, source);
}

impl StatusBar {
    /// A status bar for the active tab, reading instant search from the source [`init`] set. The explorer must
    /// exist (`explorer::init`).
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer::explorer(cx);
        let source = index::shared_source(cx);
        let model = cx.new(move |_| IndexModel::new(source));
        model.update(cx, |model, cx| model.refresh(cx));
        let thumbs = thumbnail::thumbnails(cx);
        let usage = crate::components::disk_usage::shared(cx);
        let mut subscriptions = vec![
            // Any change to the explorer may change what the bar says; the summary is made on the next frame.
            cx.observe(&explorer, |this: &mut Self, _, cx| {
                this.dirty = true;
                cx.notify();
            }),
            // The map's measuring and its time, for the work line while a tab shows it.
            cx.observe(&usage, |_, _, cx| cx.notify()),
            cx.observe(&model, |this: &mut Self, model, cx| {
                this.index_info = model.read(cx).info().cloned();
                cx.notify();
            }),
            // A thumbnail that arrives changes the thumbnails' line.
            cx.observe(&thumbs, |_: &mut Self, _, cx| cx.notify()),
        ];
        // Git's status of the folder arrived: the work line and the popover's stages say how long it took.
        if let Some(model) = git::try_shared(cx) {
            subscriptions.push(cx.observe(&model, |_: &mut Self, _, cx| cx.notify()));
        }
        StatusBar {
            explorer,
            index: model,
            index_info: None,
            summary: Summary::default(),
            counted: summary::SelectedCount::default(),
            dirty: true,
            details_open: false,
            dismissed_at: None,
            details_focus: cx.focus_handle(),
            enabling: false,
            thumbs,
            segment_focus: Vec::new(),
            beside: Rc::default(),
            _subscriptions: subscriptions,
        }
    }

    /// Makes the summary from the explorer's active tab (`Summary::of`). A tab change closes the popover.
    fn refresh_summary(&mut self, cx: &App) {
        let _span = crate::perf::span("status summary");
        let summary = {
            let explorer = self.explorer.read(cx);
            let tab = explorer.active_tab();
            let sizes = tab.and_then(|tab| explorer.size_run(&tab.path));
            Summary::of_counted(tab, explorer.drives().len(), sizes, &mut self.counted)
        };
        if summary.tab != self.summary.tab {
            self.details_open = false;
        }
        self.summary = summary;
        self.dirty = false;
    }

    /// The first click of the press that closes the popover is the press's own, so it does nothing else.
    fn toggle_details(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let closed_by_this_press = self.dismissed_at.take() == Some(at);
        if self.details_open {
            self.details_open = false;
        } else if !closed_by_this_press {
            self.details_open = true;
            window.focus(&self.details_focus, cx);
        }
        cx.notify();
    }

    /// A filter chip: shows only that kind, or everything again if the box already shows it (`toggleCategoryFilter`).
    fn toggle_category(&mut self, query: &str, cx: &mut Context<Self>) {
        if query.is_empty() {
            return;
        }
        let next = if self.summary.filter.trim() == query {
            String::new()
        } else {
            query.to_string()
        };
        self.explorer
            .update(cx, |explorer, cx| explorer.set_filter(&next, None, cx));
        self.details_open = false;
        cx.notify();
    }

    /// "Show everything again": clears the search box's filter.
    fn show_everything(&mut self, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.set_filter("", None, cx));
        self.details_open = false;
        cx.notify();
    }

    /// A small tinted button with a picture and a word (the X-Ray, Rename and Zip buttons): in the accent color when
    /// `primary`, else in the plain tint Copy paths has.
    fn tinted(
        id: &'static str,
        glyph: IconName,
        label: SharedString,
        tip: SharedString,
        primary: bool,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> Button {
        let variant = if primary {
            ButtonCustomVariant::new(cx)
                .color(tint(t.primary, 0.1))
                .foreground(t.primary)
                .hover(tint(t.primary, 0.2))
                .active(tint(t.primary, 0.2))
        } else {
            ButtonCustomVariant::new(cx)
                .color(t.foreground.opacity(0.06))
                .foreground(t.foreground.opacity(0.85))
                .hover(t.accent)
                .active(t.accent)
        };
        Button::new(id)
            .custom(variant)
            .compact()
            .tooltip(tip)
            .accessibility_label(label.clone())
            .child(
                row_start()
                    .items_center()
                    .gap(space(1.))
                    .text_size(theme::size::XXS)
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon(glyph).size_3())
                    .child(label),
            )
    }

    /// The active tab's selected paths, in row order.
    fn selected_paths(&self, cx: &App) -> Vec<String> {
        self.explorer
            .read(cx)
            .active_tab()
            .map(summary::selected_paths)
            .unwrap_or_default()
    }

    /// File X-Ray of the one selected file: what "File X-Ray" in its menu opens (`inspectXRay`).
    fn xray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_paths(cx).first().cloned() {
            crate::dialogs::xray::open(window, cx, path);
        }
    }

    /// The batch rename dialog for the selected items: what F2 opens with several selected (`openBatchRename`).
    fn rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selected_paths(cx);
        crate::dialogs::batch_rename::open(window, cx, paths);
    }

    /// A zip of the selected items beside them, once the secrets check is through: what "Compress to > Zip file"
    /// does (`secretsCheckFirst`, then `compressEntries`).
    fn zip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selected_paths(cx);
        if paths.is_empty() {
            return;
        }
        let explorer = self.explorer.clone();
        crate::dialogs::check::secrets_check_first(window, cx, paths.clone(), t!("Zip anyway"), move |_, cx| {
            let paths = paths.clone();
            explorer.update(cx, |e, cx| {
                e.compress(paths, explorer::CompressFormat::Zip, None, cx).detach()
            });
        });
    }

    /// Copies the selected paths, one to a line (Copy paths).
    fn copy_paths(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selected_paths(cx);
        let count = paths.len();
        let text = paths.join("\r\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        window.push_notification(Notification::success(t!("Copied {count} paths", count = count)), cx);
    }

    /// Turns instant search on (or starts it). Windows asks for administrator approval; declining says nothing.
    fn turn_on(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.enabling {
            return;
        }
        self.enabling = true;
        cx.notify();
        let answer = self.index.update(cx, |model, cx| model.enable(cx));
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            this.update_in(cx, |this, window, cx| {
                this.enabling = false;
                match answer {
                    Ok(()) => window.push_notification(
                        Notification::success(t!("Each drive is searched with the index as soon as it has been read."))
                            .title(t!("Instant search is on")),
                        cx,
                    ),
                    Err(message) if message != "cancelled" => window.push_notification(
                        Notification::error(message).title(t!("Couldn't turn on instant search")),
                        cx,
                    ),
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// The window's width, as the widths the bar drops words at.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Wide {
    two_xs: bool,
    medium: bool,
    large: bool,
    extra_large: bool,
    /// The bar's ends are beside the commands' island: what acts on the selection is there, not here.
    docked: bool,
}

impl Wide {
    /// What fits at one end of the dock, `side` pixels wide: the counts always, X-Ray and the index's progress
    /// from 13 rem, how the folder was read from 15, how its sizes and Git's status were from 28.
    fn beside(side: f32, window: &Window) -> Self {
        let rem = f32::from(window.rem_size());
        let at = |rems: f32| side >= rem * rems;
        Wide {
            two_xs: at(6.),
            medium: at(13.),
            large: at(15.),
            extra_large: at(28.),
            docked: true,
        }
    }

    fn of(window: &Window) -> Self {
        let width = window.viewport_size().width;
        let rem = window.rem_size();
        let at = |rems: f32| width >= rem * rems;
        Wide {
            two_xs: at(TWO_XS),
            medium: at(MEDIUM),
            large: at(LARGE),
            extra_large: at(EXTRA_LARGE),
            docked: false,
        }
    }
}

/// "· git · 12 ms", with what Git found in the folder on hover, while the tab has a Git view of its folder.
fn git_piece(cx: &App, tab: crate::explorer::TabId) -> Option<AnyElement> {
    let line = git::status_line(cx, tab)?;
    let title = line.title.clone();
    Some(
        div()
            .id("status-git")
            .role(Role::Status)
            .aria_label(line.said.clone())
            .aria_description(line.title)
            .test_support()
            .tooltip(move |window, cx| crate::kit::tooltip(title.clone(), window, cx))
            .child(format!("· {}", line.said))
            .into_any_element(),
    )
}

/// "Indexing C: 45%…", or without a percentage while the total isn't known.
fn indexing_text(volume: &Volume) -> SharedString {
    match volume.percent() {
        Some(percent) => t!(
            "Indexing {volume} {percent}%…",
            volume = volume.volume.clone(),
            percent = percent
        ),
        None => t!("Indexing {volume}…", volume = volume.volume.clone()),
    }
}

/// The Tailwind-style "bg-primary/10" tint of the instant search button and the active chip.
fn tint(color: Hsla, alpha: f32) -> Hsla {
    color.opacity(alpha)
}

impl StatusBar {
    /// The thumbnails of the items on screen: the tab's burst, when the tab shows the grid and some came
    /// (`thumbsHere` in StatusBar.tsx). Read every frame, because the service changes while the grid fills.
    fn thumbs_here(&self, cx: &App) -> Option<Thumbs> {
        let s = &self.summary;
        if s.thumbs_scope.is_empty() {
            return None;
        }
        let run = self.thumbs.read(cx).run(&s.lane)?;
        (run.scope == s.thumbs_scope && run.found > 0).then(|| Thumbs::of(run))
    }
}

impl Render for StatusBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _span = crate::perf::span("status bar");
        if self.dirty {
            self.refresh_summary(cx);
        }
        // One focus handle per segment of the breakdown, kept across frames so the keyboard stays where it was.
        let kinds = self.summary.spectrum.len();
        while self.segment_focus.len() < kinds {
            self.segment_focus.push(cx.focus_handle().tab_stop(true));
        }
        let t = theme::tokens(cx).clone();
        // In the dock each end has the room beside the island, and says what fits there; alone, the bar goes by
        // the window's width.
        let side = self.beside.get();
        let wide = if side > 0. {
            Wide::beside(side, window)
        } else {
            Wide::of(window)
        };
        let thumbs = self.thumbs_here(cx);
        let s = &self.summary;

        let left = self.left(s, wide, &t, cx);
        let right = self.right(s, wide, thumbs.as_ref(), &t, cx);
        // An end never runs under the island: it is cut at its own room. The start's words are at the bar's
        // start edge, the end's against its end edge, whatever is between them.
        let left = div()
            .flex_none()
            .min_w_0()
            .overflow_hidden()
            .when(side > 0., |left| left.max_w(px(side)))
            .child(left);
        let right = right.map(|right| {
            let held = div().min_w_0().overflow_hidden().flex().justify_end().child(right);
            let held = if side > 0. {
                held.flex_none().max_w(px(side))
            } else {
                held.flex_1()
            };
            div().flex_1().min_w_0().flex().justify_end().child(held)
        });
        let popover = (s.has_listing && !s.this_pc && s.find.is_none() && self.details_open)
            .then(|| self.details(s, thumbs.as_ref(), &t, cx));

        row_start()
            .id("status-bar")
            .role(Role::Status)
            .aria_label(t!("Status bar"))
            .test_support()
            .relative()
            .flex_none()
            // The dock at the foot of the sheet: the counts at its start, the work at its end, and the commands'
            // island over its middle (the window puts it there).
            .w_full()
            .h(HEIGHT)
            .px(space(4.))
            .gap(space(1.5))
            .items_center()
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .text_start()
            .child(left)
            .children(right)
            .children(popover)
    }
}

impl StatusBar {
    /// The counts on the left (`shown`, hidden, selected and the selection's size), and Copy paths when several
    /// are selected and the window is wide.
    fn left(&self, s: &Summary, wide: Wide, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let text: SharedString = if s.this_pc { drives(s.drives) } else { counts(s) };
        // An archive's items, a remote's, a closed folder's and the Recycle Bin's aren't files to look into or
        // rename here.
        let writable = s.writable && !s.this_pc;
        let xray = (writable && s.selected == 1 && s.selected_files == 1 && wide.medium).then(|| {
            Self::tinted(
                "status-bar-xray",
                IconName::FingerprintPattern,
                t!("X-Ray"),
                t!("File X-Ray: hashes and hidden streams"),
                true,
                t,
                cx,
            )
            .on_click(cx.listener(|this, _, window, cx| this.xray(window, cx)))
        });
        let rename = (writable && s.selected > 1 && wide.medium && !wide.docked).then(|| {
            Self::tinted(
                "status-bar-rename",
                IconName::PenLine,
                t!("Rename"),
                t!("Rename the selected items (F2)"),
                true,
                t,
                cx,
            )
            .on_click(cx.listener(|this, _, window, cx| this.rename(window, cx)))
        });
        let zip = (writable && s.selected > 1 && wide.large && !wide.docked).then(|| {
            Self::tinted(
                "status-bar-zip",
                IconName::FileArchive,
                t!("Zip"),
                t!("Compress the selected items to a .zip archive"),
                false,
                t,
                cx,
            )
            .on_click(cx.listener(|this, _, window, cx| this.zip(window, cx)))
        });
        let copy = (!s.this_pc && s.selected > 1 && wide.extra_large && !wide.docked).then(|| {
            Button::new("status-bar-copy-paths")
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(t.foreground.opacity(0.06))
                        .foreground(t.foreground.opacity(0.85))
                        .hover(t.accent)
                        .active(t.accent),
                )
                .compact()
                .tooltip(t!("Copy selected paths (Ctrl+Shift+C)"))
                .accessibility_label(t!("Copy paths"))
                .on_click(cx.listener(|this, _, window, cx| this.copy_paths(window, cx)))
                .child(
                    row_start()
                        .items_center()
                        .gap(space(1.))
                        .text_size(theme::size::XXS)
                        .font_weight(FontWeight::MEDIUM)
                        .child(icon(IconName::ClipboardCopy).size_3())
                        .child(t!("Copy paths")),
                )
        });
        row_start()
            .flex_none()
            .items_center()
            .gap(space(1.5))
            .whitespace_nowrap()
            .child(
                div()
                    .id("status-counts")
                    .role(Role::Status)
                    .aria_label(text.clone())
                    .test_support()
                    .child(text),
            )
            .children(xray)
            .children(rename)
            .children(zip)
            .children(copy)
    }

    /// The work on the right: instant search's state and progress, and how the folder was read and shown. Gone in
    /// a very narrow window, as the TypeScript's `@2xs` hid it (This PC's drives and index stay).
    fn right(
        &self,
        s: &Summary,
        wide: Wide,
        thumbs: Option<&Thumbs>,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !s.this_pc && !wide.two_xs {
            return None;
        }
        let index = self.index_info.as_ref();
        let offer = index.is_some_and(Index::can_turn_on);
        let scanning = index.and_then(Index::scanning);

        let content: AnyElement = if s.this_pc {
            // This PC says what instant search is doing, or offers to turn it on (the TypeScript's `turnOn ?? indexing`).
            row_start()
                .items_center()
                .gap(space(1.))
                .children(offer.then(|| self.turn_on_button(t, cx)))
                .children(scanning.filter(|_| !offer).map(|v| indexing_line(v, t)))
                .into_any_element()
        } else if let Some(find) = &s.find {
            // A search: its engine and what it found; instant search's button when the search used `fd`.
            let fd_offer = offer && find.engine == "fd";
            row_start()
                .items_center()
                .gap(space(1.))
                .children(wide.medium.then(|| scanning.map(|v| indexing_line(v, t))).flatten())
                .children(find.running.then(|| spinner(t)))
                .child(div().child(summary::search_text(find)))
                .children(thumbs.and_then(|th| thumbs_piece(th, wide, t)))
                .children((fd_offer && wide.large).then(|| self.turn_on_button(t, cx)))
                .into_any_element()
        } else if let Some(line) = self.usage_line(cx) {
            // The tab shows its disk usage map: how it was mapped, instead of how the folder was read.
            line.element(cx)
        } else if s.has_listing {
            // The folder's work, with its details one click away (the popover).
            let work = Work::of(s);
            self.work_button(&work, wide, scanning, thumbs, t, cx)
        } else {
            row_start()
                .items_center()
                .gap(space(1.))
                .children(wide.medium.then(|| scanning.map(|v| indexing_line(v, t))).flatten())
                .into_any_element()
        };

        Some(
            row_start()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .justify_end()
                .items_center()
                .whitespace_nowrap()
                .child(content)
                .into_any_element(),
        )
    }

    /// How the active tab's disk usage map was made ("index · mapped 12 GB in 3.2 ms"), while the tab shows it.
    fn usage_line(&self, cx: &App) -> Option<crate::components::disk_usage::StatusLine> {
        let tab = self.explorer.read(cx).active_tab_id();
        if !crate::components::disk_usage::shows(cx, tab) {
            return None;
        }
        crate::components::disk_usage::status(cx, tab)
    }

    /// The button that opens the folder's details, holding the index's progress and the work line.
    fn work_button(
        &self,
        work: &Work,
        wide: Wide,
        scanning: Option<&Volume>,
        thumbs: Option<&Thumbs>,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, pieces): (Option<SharedString>, AnyElement) = match work {
            Work::Archive { lead, shown, title } => (
                Some(title.clone()),
                row_start()
                    .items_center()
                    .gap(space(1.))
                    .child(icon(IconName::FileArchive).size_3().text_color(t.warning).flex_none())
                    .children(wide.large.then(|| div().child(lead.clone())))
                    .child(div().child(shown.clone()))
                    .into_any_element(),
            ),
            Work::Index { lead, shown, title } => (
                Some(title.clone()),
                row_start()
                    .items_center()
                    .gap(space(1.))
                    .child(icon(IconName::FolderLock).size_3().text_color(t.warning).flex_none())
                    .children(wide.large.then(|| div().child(lead.clone())))
                    .child(div().child(shown.clone()))
                    .into_any_element(),
            ),
            Work::Disk {
                lead,
                shown,
                sized,
                title,
            } => {
                // From the xl width on: how the folder sizes were measured, and how long Git status took.
                let sized = sized
                    .as_ref()
                    .filter(|_| wide.extra_large)
                    .map(|sized| self.sized_piece(sized));
                let git = self
                    .summary
                    .tab
                    .filter(|_| wide.extra_large)
                    .and_then(|tab| git_piece(cx, tab));
                (
                    Some(title.clone()),
                    row_start()
                        .items_center()
                        .gap(space(1.))
                        .child(icon(IconName::Zap).size_3().text_color(t.primary).flex_none())
                        .children(wide.large.then(|| div().child(lead.clone())))
                        .child(div().child(shown.clone()))
                        .children(sized)
                        .children(git)
                        .into_any_element(),
                )
            }
            // Nothing to say about the folder yet: the button still opens the popover, with its breakdown.
            Work::Nothing | Work::Search { .. } => (None, div().into_any_element()),
        };
        let trigger = Button::new("status-bar-details")
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(t.accent.opacity(0.))
                    .foreground(t.muted_foreground)
                    .hover(t.accent)
                    .active(t.accent),
            )
            .compact()
            // The popover is the one named "What's in this folder"; the button that opens it says what it is.
            .accessibility_label(t!("Folder details"))
            .on_click(
                cx.listener(|this, event: &ClickEvent, window, cx| this.toggle_details(event.position(), window, cx)),
            )
            .child(
                row_start()
                    .items_center()
                    .gap(space(1.))
                    .min_w_0()
                    .overflow_hidden()
                    .children(wide.medium.then(|| scanning.map(|v| indexing_line(v, t))).flatten())
                    .child(pieces)
                    .children(thumbs.and_then(|th| thumbs_piece(th, wide, t))),
            );
        let trigger = match title {
            Some(title) => trigger.tooltip(title),
            None => trigger,
        };
        trigger.into_any_element()
    }

    /// The active tab, when Git's view is of the folder the tab is in now (not the one it has just left).
    fn git_tab(&self, cx: &App) -> Option<crate::explorer::TabId> {
        let tab = self.summary.tab?;
        let run = git::try_shared(cx)?.read(cx).run(tab)?;
        let path = &self.explorer.read(cx).tab(tab)?.path;
        glint_model::paths::same_path(&run.folder, path).then_some(tab)
    }

    /// "· index sized 1.2 GB in 3 ms", after the time to screen, once "Folder sizes" has measured the folder.
    fn sized_piece(&self, sized: &SharedString) -> AnyElement {
        div()
            .id("status-sized")
            .role(Role::Status)
            .aria_label(sized.clone())
            .test_support()
            .child(format!("· {sized}"))
            .into_any_element()
    }

    /// Instant search's button: turns it on (the TypeScript's "Turn on instant search").
    fn turn_on_button(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let label = if self.enabling {
            t!("Turning on instant search…")
        } else {
            t!("Turn on instant search")
        };
        Button::new("status-bar-turn-on")
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(tint(t.primary, 0.1))
                    .foreground(t.primary)
                    .hover(tint(t.primary, 0.2))
                    .active(tint(t.primary, 0.2)),
            )
            .compact()
            .disabled(self.enabling)
            .tooltip(t!(
                "Find files on every drive in milliseconds (Windows asks for administrator approval)"
            ))
            .accessibility_label(label.clone())
            .on_click(cx.listener(|this, _, window, cx| this.turn_on(window, cx)))
            .child(
                row_start()
                    .items_center()
                    .gap(space(1.))
                    .text_size(theme::size::XXS)
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon(IconName::Zap).size_3())
                    .child(label),
            )
            .into_any_element()
    }

    /// "What's in this folder": the breakdown by kind, filter chips, and how long showing the folder took. It
    /// opens above the bar, at the end edge (as the TypeScript placed it beside its button).
    fn details(&self, s: &Summary, thumbs: Option<&Thumbs>, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let total: usize = s.spectrum.iter().map(|c| c.count).sum::<usize>().max(1);

        // Built now, not lazily: the segments and the chips both need `cx`.
        let segments: Vec<AnyElement> = s
            .spectrum
            .iter()
            .enumerate()
            .map(|(i, category)| {
                let color = theme::hsl(category.group.hue as f32, category.group.sat as f32, 52.);
                let clickable = !category.query.is_empty();
                let query = category.query.clone();
                let fraction = (category.count as f32 / total as f32).max(0.04);
                let title = segment_title(category);
                let segment = div()
                    .id(("status-segment", i))
                    .h_full()
                    .flex_none()
                    .w(gpui_kit::relative(fraction))
                    .bg(color)
                    .tooltip(move |window, cx| crate::kit::tooltip(title.clone(), window, cx));
                if clickable {
                    // Enter and Space act as the click, so the segment is a tab stop with its own focus.
                    let key_query = query.clone();
                    let mut segment = segment
                        .cursor_pointer()
                        .hover(|style| style.opacity(0.8))
                        .role(Role::Button)
                        .aria_label(segment_title(category))
                        .test_support()
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_category(&query, cx)))
                        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                this.toggle_category(&key_query, cx);
                                cx.stop_propagation();
                            }
                        }));
                    if let Some(focus) = self.segment_focus.get(i) {
                        segment = segment.track_focus(focus).focus(|style| style.bg(t.ring));
                    }
                    segment.into_any_element()
                } else {
                    segment.into_any_element()
                }
            })
            .collect();
        let bar = row_start()
            .h(space(2.))
            .w_full()
            .gap(space(0.5))
            .rounded_full()
            .overflow_hidden()
            .bg(t.muted)
            .children(segments);

        let chips: Vec<AnyElement> = s
            .spectrum
            .iter()
            .take(CHIPS)
            .enumerate()
            .map(|(i, category)| self.chip(i, category, &s.filter, t, cx))
            .collect();

        let show_all = (!s.filter.is_empty()).then(|| {
            Button::new("status-show-everything")
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(tint(t.primary, 0.12))
                        .foreground(t.primary)
                        .hover(tint(t.primary, 0.2))
                        .active(tint(t.primary, 0.2)),
                )
                .compact()
                .w_full()
                .accessibility_label(t!("Show everything again"))
                .on_click(cx.listener(|this, _, _, cx| this.show_everything(cx)))
                .child(
                    row_start()
                        .items_center()
                        .justify_center()
                        .gap(space(1.5))
                        .text_size(theme::size::XXS)
                        .font_weight(FontWeight::MEDIUM)
                        .child(icon(IconName::ListFilter).size_3())
                        .child(t!("Show everything again")),
                )
        });

        // Folder read, on screen and folder sizes; then Git status, then the thumbnails.
        let mut stage_rows = stages(s);
        stage_rows.extend(
            self.git_tab(cx)
                .and_then(|tab| git::stage(cx, tab))
                .map(|(label, detail, ms)| Stage { label, detail, ms }),
        );
        stage_rows.extend(thumbs.and_then(thumbs_stage));
        let max_ms = stage_rows.iter().map(|stage| stage.ms).fold(1.0_f64, f64::max);
        let timing = (!stage_rows.is_empty()).then(|| {
            col()
                .gap(space(1.5))
                .pt(space(2.5))
                .border_t_1()
                .border_color(t.border)
                .child(
                    row_start()
                        .items_center()
                        .gap(space(1.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(icon(IconName::Zap).size_3p5().text_color(t.primary))
                        .child(t!("How long it took")),
                )
                .children(stage_rows.iter().enumerate().map(|(i, stage)| {
                    let line = format!("{} · {}", stage.label, stage.detail);
                    let time = glint_model::format::format_duration(stage.ms);
                    let fraction = (stage.ms / max_ms).clamp(0.06, 1.0) as f32;
                    col()
                        .id(("status-stage", i))
                        .role(Role::Group)
                        .aria_label(format!("{line}: {time}"))
                        .test_support()
                        .gap(space(0.5))
                        .child(
                            row_start()
                                .justify_between()
                                .text_size(theme::size::XXS)
                                .child(div().text_color(t.muted_foreground).child(line))
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(t.popover_foreground)
                                        .child(time),
                                ),
                        )
                        .child(
                            div()
                                .h(space(0.5))
                                .w_full()
                                .rounded_full()
                                .overflow_hidden()
                                .bg(t.muted)
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui_kit::relative(fraction))
                                        .rounded_full()
                                        .bg(tint(t.primary, 0.75)),
                                ),
                        )
                }))
        });

        let header = row_start()
            .justify_between()
            .items_center()
            .gap(space(2.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.popover_foreground)
                    .child(t!("What's in this folder")),
            )
            .child(
                div()
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(items(s.listing_entries)),
            );

        let panel = col()
            .id("status-details")
            .role(Role::Group)
            .aria_label(t!("What's in this folder"))
            .test_support()
            .track_focus(&self.details_focus)
            .absolute()
            .bottom_full()
            .mb(space(2.))
            .w(space(80.))
            .p(space(3.))
            .gap(space(2.))
            .text_size(theme::size::XS)
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.popover)
            .text_color(t.popover_foreground)
            .shadow_lg()
            .occlude()
            .when(i18n::is_rtl(), |this| this.left_0())
            .when(!i18n::is_rtl(), |this| this.right_0())
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                this.dismissed_at = Some(event.position);
                this.details_open = false;
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.details_open = false;
                    cx.notify();
                }
            }))
            .child(header)
            .when(!s.spectrum.is_empty(), |this| {
                this.child(bar)
                    .child(row_start().flex_wrap().gap(space(1.)).children(chips))
            })
            .children(show_all)
            .children(timing);
        panel.into_any_element()
    }

    /// A kind's chip in the popover: its color, name and count; clicking it filters the folder to that kind.
    fn chip(
        &self,
        i: usize,
        category: &Category,
        filter: &str,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = chip_active(category, filter);
        let color = theme::hsl(category.group.hue as f32, category.group.sat as f32, 52.);
        let title = chip_title(category);
        let query = category.query.clone();
        Button::new(("status-chip", i))
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(if active {
                        tint(t.primary, 0.15)
                    } else {
                        t.accent.opacity(0.)
                    })
                    .foreground(if active { t.primary } else { t.muted_foreground })
                    .hover(t.accent)
                    .active(t.accent),
            )
            .compact()
            .disabled(category.query.is_empty())
            .tooltip(title.clone())
            .accessibility_label(title)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_category(&query, cx)))
            .child(
                row_start()
                    .items_center()
                    .gap(space(1.5))
                    .text_size(theme::size::XXS)
                    .child(div().size_2().flex_none().rounded_full().bg(color))
                    .child(t!(category.group.name))
                    .child(
                        div()
                            .opacity(0.75)
                            .child(glint_model::format::format_count(category.count)),
                    ),
            )
            .into_any_element()
    }
}

/// A spinner drawn in the primary color, for work that is running (`Loader2` with `animate-spin`).
fn spinner(t: &theme::Tokens) -> AnyElement {
    Spinner::new().color(t.primary).into_any_element()
}

/// The thumbnails' line in the folder's work button: "· 12 thumbnails in 3.2 ms", with its title on hover, or
/// "· 5 thumbnails…" with a spinner while more are on their way. Only in a wide bar (`hidden @lg:inline`).
fn thumbs_piece(th: &Thumbs, wide: Wide, t: &theme::Tokens) -> Option<AnyElement> {
    if !wide.large {
        return None;
    }
    let line = thumbs_line(th);
    let row = row_start()
        .id("status-thumbnails")
        .role(Role::Status)
        .aria_label(line.clone())
        .test_support()
        .items_center()
        .gap(space(1.))
        .flex_none()
        .child(div().child("·"));
    let row = if th.pending {
        row.child(spinner(t)).child(div().child(line))
    } else {
        let title = thumbs_title(th);
        row.tooltip(move |window, cx| crate::kit::tooltip(title.clone(), window, cx))
            .child(div().child(line))
    };
    Some(row.into_any_element())
}

/// "Indexing C: 45%…" with a spinner, while the service reads a drive.
fn indexing_line(volume: &Volume, t: &theme::Tokens) -> AnyElement {
    let text = indexing_text(volume);
    row_start()
        .id("status-indexing")
        .role(Role::Status)
        .aria_label(text.clone())
        .test_support()
        .items_center()
        .gap(space(1.))
        .flex_none()
        .child(spinner(t))
        .child(div().child(text))
        .into_any_element()
}
