//! One item's preview with its name, actions and details; Quick Look shows it large, with a close button
//! (`EntryPreview` in FilePreview.tsx).
//!
//! The header has the name, the "Show" group (the file as it reads, as written, its bytes: [`choose::views`]),
//! and the actions that work today: Show in File Explorer, Open, Restore (in the Recycle Bin), and in Quick Look
//! Maximize and Close. Under it the previewer [`choose::choose`] picks, and under that the details.
//!
//! The chosen view is shared by the pane and Quick Look and kept from file to file ([`chosen`]; the TypeScript's
//! `usePreviewView`). A password that opens something makes the body be built again (`useUnlocks`).
//!
//! Not here yet (each marked `PORT ME`): Compare, File X-Ray, the Changes, Query and Profile views.

use super::cards::{ArchiveCard, FolderCard, LockedCard, Plain, PlainCard};
use super::choose::{choose, effective_view, views, Body, Fallback, View};
use super::data::DataPreview;
use super::details::{has_details, map_link, rows};
use super::hex::HexPreview;
use super::picture::{release_retired, PictureView};
use super::probe::{As, TextProbe};
use super::shell::{ShellHost, PANE, QUICK_LOOK};
use super::sources::{sources, Media, Sources};
use crate::dialogs::password::{unlocks, Unlocks};
use crate::explorer;
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::entries::{is_archived, is_closed, is_recycled, is_remote, FileEntry};
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
    scroll: ScrollHandle,
    _media: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EntryView {
    /// The preview of `entry`.
    pub fn new(entry: FileEntry, place: Place, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chosen = chosen(cx);
        let unlocks = unlocks(cx);
        let sources = sources(cx);
        let subscriptions = vec![
            cx.observe_in(&chosen, window, |this, _, window, cx| this.build(window, cx)),
            cx.observe_in(&unlocks, window, |this, _, window, cx| this.build(window, cx)),
        ];
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
            scroll: ScrollHandle::new(),
            _media: media,
            _subscriptions: subscriptions,
        };
        view.build(window, cx);
        view
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

    /// Scrolls the body a page (0.85 of its height) when the frame scrolls it (Page Down and Page Up in Quick
    /// Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
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
        let view = effective_view(&self.entry, self.chosen.read(cx).view);
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
        let entry = self.entry.clone();
        let path = entry.path.clone();
        let claimant = self.place.claimant();
        let plain = |plain: Plain, cx: &mut Context<Self>| -> AnyView {
            let entry = entry.clone();
            cx.new(|_| PlainCard::new(entry, plain)).into()
        };
        let body: AnyView = match choice.body {
            Body::Folder => cx.new(|cx| FolderCard::new(entry.clone(), cx)).into(),
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
        release_retired(window);
        let t = theme::tokens(cx).clone();
        let prefix = self.place.prefix();
        let entry = self.entry.clone();
        let archived = is_archived(&entry);
        let recycled = is_recycled(&entry);
        let current = self.key.map(|k| k.view).unwrap_or_default();

        let choices = views(&entry);
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
        // PORT ME(compare): "Compare" for the two files selected (compareFiles).
        // PORT ME(X-Ray): "File X-Ray: hashes and hidden streams" (inspectXRay).
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
