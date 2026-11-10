//! One item's preview with its name, actions and details; Quick Look shows it large, with a close button
//! (`EntryPreview` in FilePreview.tsx).
//!
//! The header has the name, the "Show" group (the file as it reads, as written, what Git changed in it, SQL over
//! it, each column of it, its bytes: [`choose::views`]), and the actions: Compare (two files ready to compare),
//! File X-Ray, Show in File Explorer, Open, Restore (in the Recycle Bin), and in Quick Look Maximize and Close.
//! Under it the previewer [`choose::choose`] picks, and under that the details.
//!
//! The chosen view is shared by the pane and Quick Look and kept from file to file ([`chosen`]; the TypeScript's
//! `usePreviewView`). A password that opens something makes the body be built again (`useUnlocks`).
//!
//! **Git's Changes view** is offered while the Git model says the file has a change ([`git::has_changes_view`]);
//! the frame follows the model, so the button comes and goes as the file's mark does.
//!
//! **The Query view.** A database's Rows button asks for it (`SqliteEvent::ShowQuery`, the TypeScript's
//! `showQuery`): the frame keeps the SQL as the draft the Query view starts with and switches to it.
//!
//! **Compare** works on the pair the explorer would compare (Ctrl+M: [`crate::components::command_bar::Facts`]'s
//! `pair`): two files selected, or one against the other pane's. It is read when the selection moves.

use super::book::BookView;
use super::cards::{ArchiveCard, FolderCard, LockedCard, Plain, PlainCard};
use super::cast::CastView;
use super::choose::{choose, effective_view, views, Body, Fallback, View};
use super::data::DataPreview;
use super::details::{has_details, map_link, rows};
use super::docx::{DocxEvent, DocxView};
use super::hex::HexPreview;
use super::model3d::ModelView;
use super::notebook::NotebookView;
use super::pdf::PdfView;
use super::picture::{release_retired, PictureView};
use super::player::PlayerView;
use super::probe::{As, TextProbe};
use super::program::ProgramPreview;
use super::sheet::SheetPreview;
use super::shell::{ShellHost, PANE, QUICK_LOOK};
use super::signature::SignatureChip;
use super::small::SmallPreview;
use super::sources::{sources, Media, Sources};
use super::sqlite::{SqliteEvent, SqlitePreview};
use super::typst_view::TypstView;
use crate::components::command_bar::{Facts, Pair};
use crate::components::git::{self, ChangesView};
use crate::dialogs::action_run::{tools_model, Tooling};
use crate::dialogs::data_tools::{core_backend, ProfileView, QueryView};
use crate::dialogs::password::{unlocks, Unlocks};
use crate::dialogs::{compare_files, xray};
use crate::explorer::{self, ExplorerEvent};
use crate::kit::prelude::*;
use crate::{state, t, theme};
use glint_model::entries::{is_archived, is_closed, is_recycled, is_remote, FileEntry};
use glint_model::paths::same_path;
use gpui_kit::{ScrollHandle, Subscription, Task};

/// Where an item's preview is: the pane beside the files, or Quick Look over them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Pane,
    QuickLook,
}

impl Place {
    /// The preview host's claimant for Windows' previewers (Quick Look's win).
    pub fn claimant(self) -> u32 {
        match self {
            Place::Pane => PANE,
            Place::QuickLook => QUICK_LOOK,
        }
    }

    /// The start of the ids of the header's buttons, so both can be on screen at once.
    pub fn prefix(self) -> &'static str {
        match self {
            Place::Pane => "preview",
            Place::QuickLook => "quick-look",
        }
    }
}

/// What the header asks of its owner (Quick Look).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryEvent {
    /// The close button.
    Close,
    /// The maximize button, or a double-click on the header.
    ToggleMaximize,
}

impl EventEmitter<EntryEvent> for EntryView {}

/// The view chosen for files that have it (`usePreviewView`).
pub struct Chosen {
    pub view: View,
}

struct SharedChosen(Entity<Chosen>);

impl Global for SharedChosen {}

/// The shared choice of view.
pub fn chosen(cx: &mut App) -> Entity<Chosen> {
    if let Some(shared) = cx.try_global::<SharedChosen>() {
        return shared.0.clone();
    }
    let chosen = cx.new(|_| Chosen { view: View::File });
    cx.set_global(SharedChosen(chosen.clone()));
    chosen
}

/// What the body was built for: it is built again when any of it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    body: Body,
    view: View,
    unlocks: usize,
}

/// An item's preview: header, previewer, details.
pub struct EntryView {
    entry: FileEntry,
    place: Place,
    sources: Sources,
    chosen: Entity<Chosen>,
    unlocks: Entity<Unlocks>,
    key: Option<Key>,
    body: Option<AnyView>,
    media: Option<Media>,
    maximized: bool,
    /// Git has a change for the file, so it has a Changes view (`useGitMark`).
    changed: bool,
    /// The two files Compare would open, when there are two (`comparePair`).
    pair: Option<Pair>,
    /// The SQL a database's Rows button asked the Query view to start with: the file's path and the SQL.
    draft: Option<(String, String)>,
    scroll: ScrollHandle,
    _media: Option<Task<()>>,
    /// What the body told the frame (the database's Rows button).
    _body_events: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl EntryView {
    /// The preview of `entry`.
    pub fn new(entry: FileEntry, place: Place, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chosen = chosen(cx);
        let unlocks = unlocks(cx);
        let sources = sources(cx);
        let explorer = explorer::explorer(cx);
        let mut subscriptions = vec![
            cx.observe_in(&chosen, window, |this, _, window, cx| this.build(window, cx)),
            cx.observe_in(&unlocks, window, |this, _, window, cx| this.build(window, cx)),
            // The pair Compare opens follows the selection (and the other pane's).
            cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| {
                if matches!(
                    event,
                    ExplorerEvent::SelectionChanged(_)
                        | ExplorerEvent::TabChanged(_)
                        | ExplorerEvent::TabsChanged
                        | ExplorerEvent::PaneFocused
                ) {
                    this.read_pair(cx);
                }
            }),
        ];
        // The Changes view comes and goes with the file's Git mark.
        if let Some(model) = git::try_shared(cx) {
            subscriptions.push(cx.observe_in(&model, window, |this, _, window, cx| this.read_changed(window, cx)));
        }
        let changed = git::has_changes_view(cx, &entry);
        let pair = Self::pair_of(cx);
        // A photo's, video's or song's details are read in the background.
        let media = (!is_archived(&entry) && has_details(&entry)).then(|| {
            let running = (sources.media)(&entry.path);
            cx.spawn(async move |this, cx| {
                let media = running.await;
                this.update(cx, |this, cx| {
                    this.media = media;
                    cx.notify();
                })
                .ok();
            })
        });
        let mut view = EntryView {
            entry,
            place,
            sources,
            chosen,
            unlocks,
            key: None,
            body: None,
            media: None,
            maximized: false,
            changed,
            pair,
            draft: None,
            scroll: ScrollHandle::new(),
            _media: media,
            _body_events: None,
            _subscriptions: subscriptions,
        };
        view.build(window, cx);
        view
    }

    /// The two files Compare would open now.
    fn pair_of(cx: &App) -> Option<Pair> {
        let explorer = explorer::try_explorer(cx)?;
        Facts::read(explorer.read(cx), false, state::settings(cx)).pair
    }

    fn read_pair(&mut self, cx: &mut Context<Self>) {
        let pair = Self::pair_of(cx);
        if pair != self.pair {
            self.pair = pair;
            cx.notify();
        }
    }

    /// Git said something: the file's Changes view may have come or gone.
    fn read_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let changed = git::has_changes_view(cx, &self.entry);
        if changed != self.changed {
            self.changed = changed;
            self.build(window, cx);
            cx.notify();
        }
    }

    /// Whether the file has a Changes view (Git has a change for it).
    pub fn has_changes(&self) -> bool {
        self.changed
    }

    /// The two files the Compare button opens, if it shows.
    pub fn compare_pair(&self) -> Option<&Pair> {
        self.pair.as_ref()
    }

    /// The SQL the Query view starts with, once a database's Rows button asked for it.
    pub fn draft(&self) -> Option<&str> {
        self.draft.as_ref().map(|(_, sql)| sql.as_str())
    }

    /// The item shown.
    pub fn entry(&self) -> &FileEntry {
        &self.entry
    }

    /// What is drawn for it now.
    pub fn body(&self) -> Option<Body> {
        self.key.map(|key| key.body)
    }

    /// The view shown.
    pub fn view(&self) -> Option<View> {
        self.key.map(|key| key.view)
    }

    /// The previewer's view, once built.
    pub fn body_view(&self) -> Option<&AnyView> {
        self.body.as_ref()
    }

    /// Quick Look's maximize state, for the button.
    pub fn set_maximized(&mut self, maximized: bool, cx: &mut Context<Self>) {
        self.maximized = maximized;
        cx.notify();
    }

    /// Page Down and Page Up in Quick Look: the frame scrolls its body a page when it is taller than the box,
    /// else the previewer that scrolls itself does. Returns whether anything moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        self.page_frame(down, cx) || self.page_body(down, cx)
    }

    /// The previewers that scroll inside themselves take the page keys for Quick Look.
    fn page_body(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some(body) = self.body.clone() else {
            return false;
        };
        macro_rules! page_of {
            ($($view:ty),*) => {$(
                if let Ok(view) = body.clone().downcast::<$view>() {
                    return view.update(cx, |view, cx| view.page(down, cx));
                }
            )*};
        }
        page_of!(
            HexPreview,
            DataPreview,
            SqlitePreview,
            SheetPreview,
            NotebookView,
            ProgramPreview,
            PlayerView,
            PdfView,
            BookView,
            DocxView,
            ModelView,
            TypstView,
            CastView,
            SmallPreview
        );
        false
    }

    /// Scrolls the body a page (0.85 of its height) when the frame scrolls it. Returns whether it moved.
    fn page_frame(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let max = self.scroll.max_offset().y;
        let viewport = self.scroll.bounds().size.height;
        if max <= px(0.) {
            return false;
        }
        let offset = self.scroll.offset();
        let step = viewport * 0.85;
        let top = if down { offset.y - step } else { offset.y + step };
        let top = top.clamp(-max, px(0.));
        if top == offset.y {
            return false;
        }
        self.scroll.set_offset(gpui_kit::point(offset.x, top));
        cx.notify();
        true
    }

    /// Builds the previewer for the file in the view chosen, unless it is the one already built.
    fn build(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = effective_view(&self.entry, self.chosen.read(cx).view, self.changed);
        let choice = choose(&self.entry, view);
        let key = Key {
            body: choice.body,
            view,
            unlocks: self.unlocks.read(cx).count(),
        };
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        self._body_events = None;
        let entry = self.entry.clone();
        let path = entry.path.clone();
        let claimant = self.place.claimant();
        let plain = |plain: Plain, cx: &mut Context<Self>| -> AnyView {
            let entry = entry.clone();
            cx.new(|_| PlainCard::new(entry, plain)).into()
        };
        let body: AnyView = match choice.body {
            Body::Folder => cx.new(|cx| FolderCard::new(entry.clone(), window, cx)).into(),
            Body::OnlineOnly => plain(Plain::OnlineOnly, cx),
            Body::Closed => plain(Plain::Closed, cx),
            Body::ListOnly => plain(Plain::ListOnly, cx),
            Body::Icon => plain(Plain::Icon, cx),
            Body::Locked => cx.new(|cx| LockedCard::new(entry.clone(), window, cx)).into(),
            Body::Archive => cx.new(|cx| ArchiveCard::new(entry.clone(), cx)).into(),
            Body::Picture => cx.new(|cx| PictureView::new(entry.clone(), window, cx)).into(),
            Body::Text => cx
                .new(|cx| TextProbe::new(entry.clone(), As::Text, claimant, window, cx))
                .into(),
            Body::Markdown => cx
                .new(|cx| TextProbe::new(entry.clone(), As::Markdown, claimant, window, cx))
                .into(),
            Body::Data => cx.new(|cx| DataPreview::new(path, window, cx)).into(),
            Body::Hex => cx.new(|cx| HexPreview::new(path, window, cx)).into(),
            Body::Sqlite => {
                // The Rows button asks the frame for its Query view (`showQuery`).
                let view = cx.new(|cx| {
                    let mut preview = SqlitePreview::new(path, window, cx);
                    preview.set_query_in_frame(true);
                    preview
                });
                self._body_events =
                    Some(
                        cx.subscribe_in(&view, window, |this, _, event: &SqliteEvent, _, cx| match event {
                            SqliteEvent::ShowQuery { path, sql } => this.show_query(path.clone(), sql.clone(), cx),
                        }),
                    );
                view.into()
            }
            Body::Sheet => cx.new(|cx| SheetPreview::new(path, window, cx)).into(),
            Body::Notebook => cx.new(|cx| NotebookView::new(path, window, cx)).into(),
            Body::Program => {
                let chip = cx.new(|cx| SignatureChip::new(path.clone(), cx));
                cx.new(|cx| {
                    let mut program = ProgramPreview::new(path, window, cx);
                    program.set_signature(Some(chip.into()), cx);
                    program
                })
                .into()
            }
            Body::Small(kind) => cx.new(|cx| SmallPreview::new(entry.clone(), kind, window, cx)).into(),
            Body::Player(kind) => cx.new(|cx| PlayerView::new(entry.clone(), kind, window, cx)).into(),
            Body::Pdf => cx.new(|cx| PdfView::new(entry.clone(), window, cx)).into(),
            Body::Book => cx.new(|cx| BookView::new(path, window, cx)).into(),
            Body::Model => cx.new(|cx| ModelView::new(entry.clone(), window, cx)).into(),
            Body::Typst => cx.new(|cx| TypstView::new(entry.clone(), window, cx)).into(),
            Body::Cast => cx.new(|cx| CastView::new(path, window, cx)).into(),
            Body::Docx => {
                // A document Glint can't lay out falls back to Windows' previewer over the icon, as in the web app.
                let view = cx.new(|cx| DocxView::new(path, window, cx));
                self._body_events = Some(cx.subscribe_in(
                    &view,
                    window,
                    |this, _, event: &DocxEvent, _, cx| match event {
                        DocxEvent::Failed => {
                            let entry = this.entry.clone();
                            let under: AnyView = cx.new(|_| PlainCard::new(entry.clone(), Plain::Icon)).into();
                            let claimant = this.place.claimant();
                            this.body = Some(cx.new(|cx| ShellHost::new(&entry, claimant, false, under, cx)).into());
                            cx.notify();
                        }
                    },
                ));
                view.into()
            }
            Body::Changes => cx.new(|cx| ChangesView::new(entry.clone(), cx)).into(),
            Body::Query => {
                let draft = self
                    .draft
                    .as_ref()
                    .filter(|(draft_path, _)| same_path(draft_path, &path))
                    .map(|(_, sql)| sql.clone());
                let backend = core_backend(cx);
                let tooling = Tooling::new(tools_model(window, cx));
                cx.new(|cx| QueryView::new(vec![path], draft, backend, tooling, window, cx))
                    .into()
            }
            Body::Profile => {
                let backend = core_backend(cx);
                let tooling = Tooling::new(tools_model(window, cx));
                cx.new(|cx| ProfileView::new(path, backend, tooling, window, cx)).into()
            }
            Body::Shell(fallback) => {
                let under: AnyView = match fallback {
                    Fallback::Icon => plain(Plain::Icon, cx),
                    Fallback::Hex => cx.new(|cx| HexPreview::new(path, window, cx)).into(),
                };
                let host = cx.new(|cx| ShellHost::new(&entry, claimant, false, under, cx));
                host.into()
            }
        };
        self.body = Some(body);
        cx.notify();
    }

    /// A database's Rows button: the Query view with `sql` written (`showQuery`).
    fn show_query(&mut self, path: String, sql: String, cx: &mut Context<Self>) {
        self.draft = Some((path, sql));
        self.chosen.update(cx, |chosen, cx| {
            chosen.view = View::Query;
            cx.notify();
        });
    }

    fn open(&mut self, cx: &mut App) {
        let entry = self.entry.clone();
        explorer::explorer(cx).update(cx, |explorer, cx| {
            if is_archived(&entry) || entry.dir || is_closed(&entry) {
                explorer.open_entry(&entry, cx).detach();
            } else {
                explorer.open_file(&entry.path, None, cx).detach();
            }
        });
    }

    /// What File Explorer is asked to show: the archive, for an item inside one.
    fn reveal_target(&self, cx: &App) -> String {
        let archive = explorer::try_explorer(cx).and_then(|e| e.read(cx).archive_of(None).map(|a| a.outer.clone()));
        archive.unwrap_or_else(|| self.entry.path.clone())
    }
}

impl Render for EntryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Pictures, frames and drawings of views that are gone leave the GPU atlas.
        release_retired(window);
        super::player::release_retired(window);
        super::model3d::release_retired(window);
        let t = theme::tokens(cx).clone();
        let prefix = self.place.prefix();
        let entry = self.entry.clone();
        let archived = is_archived(&entry);
        let recycled = is_recycled(&entry);
        let current = self.key.map(|k| k.view).unwrap_or_default();

        let choices = views(&entry, self.changed);
        let show = (choices.len() > 1).then(|| {
            row_start()
                .id(format!("{prefix}-views"))
                .role(Role::Group)
                .aria_label(t!("Show"))
                .test_support()
                .flex_none()
                .ms(2.)
                .rounded(theme::size::RADIUS)
                .bg(t.foreground.opacity(0.05))
                .p(px(2.))
                .children(choices.into_iter().enumerate().map(|(i, choice)| {
                    let active = current == choice.view;
                    let view = choice.view;
                    Button::new((SharedString::from(format!("{prefix}-view")), i))
                        .custom(
                            ButtonCustomVariant::new(cx)
                                .color(if active {
                                    if t.dark {
                                        t.foreground.opacity(0.12)
                                    } else {
                                        t.background
                                    }
                                } else {
                                    t.background.opacity(0.)
                                })
                                .foreground(if active { t.primary } else { t.muted_foreground })
                                .hover(t.accent)
                                .active(t.accent),
                        )
                        .compact()
                        .h(px(20.))
                        .px(space(1.5))
                        .text_size(theme::size::XXS)
                        .font_weight(FontWeight::MEDIUM)
                        .label(choice.label.clone())
                        .selected(active)
                        .accessibility_label(choice.label.clone())
                        .tooltip(choice.title.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.chosen.update(cx, |chosen, cx| {
                                chosen.view = view;
                                cx.notify();
                            });
                        }))
                }))
        });

        let action = |id: &'static str, glyph: IconName, label: SharedString, hint: SharedString| {
            Button::new(format!("{prefix}-{id}"))
                .ghost()
                .icon(glyph)
                .accessibility_label(label)
                .tooltip(hint)
        };
        // Compare and X-Ray read the file itself: not a folder, nothing in an archive or a closed folder or the bin.
        let reads = !entry.dir && !archived && !is_closed(&entry) && !recycled;
        let compare = self.pair.clone().filter(|_| reads).map(|pair| {
            action(
                "compare",
                IconName::GitCompareArrows,
                // The command bar's own button is named with the shortcut; this one must not be the same name.
                t!(
                    "Compare “{a}” and “{b}”",
                    a = pair.left_name.clone(),
                    b = pair.right_name.clone()
                ),
                t!(
                    "Compare “{a}” and “{b}” (Ctrl+M)",
                    a = pair.left_name.clone(),
                    b = pair.right_name.clone()
                ),
            )
            .text_color(t.primary)
            .on_click(move |_, window, cx| compare_files::open(window, cx, pair.left.clone(), pair.right.clone()))
        });
        let x_ray = reads.then(|| {
            let path = entry.path.clone();
            action(
                "xray",
                IconName::FingerprintPattern,
                t!("File X-Ray"),
                t!("File X-Ray: hashes and hidden streams"),
            )
            .on_click(move |_, window, cx| xray::open(window, cx, path.clone()))
        });
        let restore = recycled.then(|| {
            let hint = t!(
                "Restore: back into {folder}",
                folder = entry
                    .origin
                    .clone()
                    .unwrap_or_else(|| t!("the folder it was deleted from").to_string())
            );
            action("restore", IconName::RotateCcw, t!("Restore"), hint).on_click({
                let entry = entry.clone();
                move |_, _, cx| {
                    let entry = entry.clone();
                    explorer::explorer(cx).update(cx, |explorer, cx| explorer.restore_recycled(&[entry], cx).detach());
                }
            })
        });
        let reveal = (!recycled && !is_remote(&entry)).then(|| {
            let hint = if archived {
                t!("Show the archive in File Explorer")
            } else {
                t!("Show in File Explorer")
            };
            let target = self.reveal_target(cx);
            let reveal = self.sources.reveal.clone();
            action("reveal", IconName::FolderSearch, hint.clone(), hint).on_click(move |_, _, _| reveal(&target))
        });
        let open = (!recycled).then(|| {
            action("open", IconName::ExternalLink, t!("Open"), t!("Open (Enter)"))
                .on_click(cx.listener(|this, _, _, cx| this.open(cx)))
        });
        let maximize = (self.place == Place::QuickLook).then(|| {
            let label = if self.maximized {
                t!("Restore size")
            } else {
                t!("Maximize preview")
            };
            action(
                "maximize",
                if self.maximized {
                    IconName::Minimize2
                } else {
                    IconName::Maximize2
                },
                label.clone(),
                label,
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(EntryEvent::ToggleMaximize)))
        });
        let close = (self.place == Place::QuickLook).then(|| {
            action("close", IconName::X, t!("Close"), t!("Close (Space or Esc)"))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(EntryEvent::Close)))
        });

        let name = entry.name.clone();
        let path: SharedString = entry.path.clone().into();
        let header = row_start()
            .id(format!("{prefix}-header"))
            .flex_none()
            .h(space(10.))
            .gap(space(2.))
            .ps(3.)
            .pe(1.5)
            .border_b_1()
            .border_color(t.border)
            .on_click(cx.listener(|_, event: &gpui_kit::ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    cx.emit(EntryEvent::ToggleMaximize);
                }
            }))
            .child(
                div()
                    .id(format!("{prefix}-name"))
                    .role(Role::Heading)
                    .aria_label(name.clone())
                    .test_support()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::size::SM)
                    .font_weight(FontWeight::MEDIUM)
                    .tooltip(move |window, cx| tooltip(path.clone(), window, cx))
                    .child(isolate(name)),
            )
            .child(div().flex_1())
            .children(show)
            .child(
                row_start()
                    .flex_none()
                    .gap(space(0.5))
                    .children(compare)
                    .children(x_ray)
                    .children(restore)
                    .children(reveal)
                    .children(open)
                    .children(maximize)
                    .children(close),
            );

        let body = self
            .body
            .clone()
            .map(|body| div().size_full().child(body).into_any_element())
            .unwrap_or_else(|| div().into_any_element());

        col()
            .id(format!("{prefix}-entry"))
            .role(Role::Group)
            .aria_label(t!("Preview of {name}", name = entry.name.clone()))
            .test_support()
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(header)
            .child(
                div()
                    .id(format!("{prefix}-body"))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .bg(t.card)
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
            .child(self.details(&t))
    }
}

impl EntryView {
    /// The rows under the preview (type, size, date, place and what a photo says of itself).
    fn details(&self, t: &theme::Tokens) -> impl IntoElement {
        let prefix = self.place.prefix();
        let place = self.media.as_ref().and_then(|media| media.place);
        let list = rows(&self.entry, self.media.as_ref());
        col()
            .id(format!("{prefix}-details"))
            .role(Role::Group)
            .aria_label(t!("Details"))
            .test_support()
            .flex_none()
            .gap(space(1.))
            .border_t_1()
            .border_color(t.border)
            .px(space(4.))
            .py(space(3.))
            .text_size(theme::size::XS)
            .children(list.into_iter().enumerate().map(|(i, row_)| {
                let is_place = row_.label.as_ref() == "Place" && place.is_some();
                let label = row_.label.clone();
                let value = row_.value.clone();
                let shown = if is_place {
                    let link = place.map(map_link).unwrap_or_default();
                    Button::new((SharedString::from(format!("{prefix}-place")), i))
                        .link()
                        .compact()
                        .label(isolate(value.clone()))
                        .accessibility_label(format!("{label}: {value}"))
                        .tooltip(t!("Show it on OpenStreetMap"))
                        .on_click(move |_, _, cx| cx.open_url(&link))
                        .into_any_element()
                } else {
                    div()
                        .min_w_0()
                        .truncate()
                        .child(isolate(value.clone()))
                        .into_any_element()
                };
                let group = row_start()
                    .id((SharedString::from(format!("{prefix}-detail")), i))
                    .role(Role::Group);
                // A place is its own button, named for itself.
                let group = if is_place {
                    group
                } else {
                    group.aria_label(format!("{label}: {value}"))
                };
                group
                    .test_support()
                    .items_start()
                    .gap(space(3.))
                    .child(
                        div()
                            .w(space(24.))
                            .flex_none()
                            .truncate()
                            .text_color(t.muted_foreground)
                            .child(label),
                    )
                    .child(div().flex_1().min_w_0().child(shown))
            }))
    }
}
