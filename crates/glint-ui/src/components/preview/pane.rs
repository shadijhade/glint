//! The preview pane beside the files (`PreviewPane` in Explorer.tsx and `FilePreview` in FilePreview.tsx).
//!
//! It shows what the active tab has selected: one item as its [`EntryView`], several as a deck and their totals,
//! nothing as a card about the folder (or the Recycle Bin, the Gallery, Recent, a folder Windows keeps closed).
//! A single item is shown once the selection has rested for 70 ms (`useSettled`), so arrowing quickly through a
//! list doesn't load every file on the way; the other states show at once. What a selection is made of is worked
//! out on a background thread, so a folder of a million files with everything selected doesn't stop the UI.

use super::cards::items;
use super::frame::{EntryView, Place};
use super::picture::release_retired;
use super::states::centered;
use crate::components::file_grid::thumbnail::{self, Thumbnails, Want};
use crate::components::git::RepoCard;
use crate::dialogs::action_run::tools_model;
use crate::dialogs::{batch_rename, check, compare_files};
use crate::explorer::{self, CompressFormat, Explorer, ExplorerEvent, Rows, Selection, Tab};
use crate::kit::prelude::*;
use crate::{state, t, theme};
use glint_model::entries::{is_archived, is_closed, is_recycled, is_remote, FileEntry};
use glint_model::file_types::kind_of;
use glint_model::format::{format_count, format_date, format_size, plural};
use glint_model::paths::{base_name, is_gallery, is_recent, is_recycle_bin, THIS_PC};
use gpui_kit::{Subscription, Task};
use std::sync::Arc;
use std::time::Duration;

/// How long a selection rests before one item is previewed (`useSettled(…, 70)`).
const SETTLE: Duration = Duration::from_millis(70);
/// The deck shows this many items.
const DECK: usize = 3;
/// A card in the deck, and its thumbnail (`size-18`, `size-14`), in pixels at the usual text size.
const CARD: f32 = 72.;
const THUMB: f32 = 56.;

/// What the folder shown has to say when nothing is selected (`FolderSummary`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    /// The folder, or where a search looks.
    pub path: String,
    pub items: usize,
    /// Everything inside, once known: the Recycle Bin's, from its listing.
    pub total: Option<u64>,
    /// Windows keeps this account out of it: there is nothing listed to count.
    pub no_access: bool,
    /// A Git repository's own folder.
    pub repo: bool,
}

/// What a listing is scanned for: the Recycle Bin's total, and a `.git`. It takes a pass over every entry, so
/// it is done once per listing, not once per selection.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Scan {
    total: Option<u64>,
    repo: bool,
}

impl Scan {
    fn of(tab: &Tab) -> Scan {
        let find = tab.find.is_some();
        let bin = tab.is_recycle_bin() && !find;
        Scan {
            total: bin.then(|| tab.entries().iter().map(|e| e.size).sum()),
            repo: !find && tab.archive().is_none() && tab.entries().iter().any(|e| e.lower == ".git"),
        }
    }
}

impl Summary {
    /// What `tab` shows, with what its listing was scanned for and the total its folder sizes came to (the bin's
    /// own total comes first: `binTotal ?? sizeRun?.total`).
    fn of(tab: &Tab, scan: Scan, sized: Option<u64>) -> Summary {
        let find = tab.find.is_some();
        Summary {
            path: tab
                .find
                .as_ref()
                .map_or_else(|| tab.path.clone(), |find| find.scope.clone()),
            items: tab.rows.len(),
            total: scan.total.or(sized),
            no_access: tab.is_closed() && !find,
            repo: scan.repo,
        }
    }
}

/// Several items selected: their totals and the first few for the deck (`selected.length > 1`).
#[derive(Clone, Debug, PartialEq)]
pub struct Many {
    pub count: usize,
    pub folders: usize,
    pub files: usize,
    /// The files' sizes added up.
    pub bytes: u64,
    /// The oldest and newest modified dates among those with one (ms; 0 when none has).
    pub oldest: i64,
    pub newest: i64,
    pub deck: Vec<FileEntry>,
    pub paths: Vec<String>,
    /// None is in an archive, the Recycle Bin, on a remote or closed: they can be renamed and zipped.
    pub writable: bool,
}

/// What the selection comes to.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    /// Nothing selected: the folder's card.
    Nothing,
    /// One item.
    One(FileEntry),
    /// Several.
    Many(Many),
}

/// What `selection` is among `rows`: nothing (or nothing found), one item, or several with their totals.
pub fn resolve(rows: &Rows, selection: &Selection) -> Shown {
    match selection.len() {
        0 => Shown::Nothing,
        1 => selection
            .iter()
            .next()
            .and_then(|path| rows.position_of(path))
            .and_then(|i| rows.get(i))
            .map_or(Shown::Nothing, |entry| Shown::One(entry.clone())),
        _ => {
            let mut many = Many {
                count: 0,
                folders: 0,
                files: 0,
                bytes: 0,
                oldest: 0,
                newest: 0,
                deck: Vec::new(),
                paths: Vec::new(),
                writable: true,
            };
            for entry in rows.iter().filter(|e| selection.contains(&e.path)) {
                many.count += 1;
                if entry.dir {
                    many.folders += 1;
                } else {
                    many.files += 1;
                    many.bytes = many.bytes.saturating_add(entry.size);
                }
                if entry.modified > 0 {
                    many.oldest = if many.oldest == 0 {
                        entry.modified
                    } else {
                        many.oldest.min(entry.modified)
                    };
                    many.newest = many.newest.max(entry.modified);
                }
                if many.deck.len() < DECK {
                    many.deck.push(entry.clone());
                }
                many.writable &= !(is_archived(entry) || is_recycled(entry) || is_remote(entry) || is_closed(entry));
                many.paths.push(entry.path.clone());
            }
            // The selection named things the rows no longer have (a filter hides them): what is left decides.
            match many.count {
                0 => Shown::Nothing,
                1 => Shown::One(many.deck.remove(0)),
                _ => Shown::Many(many),
            }
        }
    }
}

/// What makes two refreshes the same, so a notification that changed nothing the pane shows costs nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Fingerprint {
    tab: explorer::TabId,
    entries: usize,
    read_ms: u64,
    rows: usize,
    selected: usize,
    first: Option<String>,
    focus: Option<String>,
    anchor: Option<String>,
    summary_path: String,
    closed: bool,
    /// The folder sizes measured for the folder, once done.
    sized: Option<u64>,
}

impl Fingerprint {
    fn of(tab: &Tab, sized: Option<u64>) -> Fingerprint {
        Fingerprint {
            tab: tab.id,
            entries: tab
                .shared_entries()
                .map_or(0, |e| Arc::as_ptr(&e) as *const u8 as usize),
            read_ms: tab.listing.as_ref().map_or(0, |l| l.elapsed.to_bits()),
            rows: tab.rows.len(),
            selected: tab.selection.len(),
            first: tab.selection.iter().next().map(str::to_string),
            focus: tab.selection.focus().map(str::to_string),
            anchor: tab.selection.anchor().map(str::to_string),
            summary_path: tab.find.as_ref().map_or_else(|| tab.path.clone(), |f| f.scope.clone()),
            closed: tab.is_closed(),
            sized,
        }
    }
}

/// The preview pane: an entity the workspace places at the end edge of the files.
pub struct PreviewPane {
    explorer: Entity<Explorer>,
    thumbs: Entity<Thumbnails>,
    summary: Summary,
    shown: Shown,
    view: Option<Entity<EntryView>>,
    /// The repository's card, while the folder's card shows for a repository's own folder.
    repo_card: Option<Entity<RepoCard>>,
    seen: Option<Fingerprint>,
    /// The scan of the listing shown, with the listing's address and the path it is for.
    scanned: Option<(usize, String, Scan)>,
    generation: u64,
    _settling: Option<Task<()>>,
    deck_watch: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl PreviewPane {
    /// The pane, showing what the explorer's active tab has selected.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer::explorer(cx);
        let thumbs = thumbnail::thumbnails(cx);
        let subscriptions =
            vec![cx.subscribe_in(
                &explorer,
                window,
                |this, _, event: &ExplorerEvent, window, cx| match event {
                    ExplorerEvent::SelectionChanged(_)
                    | ExplorerEvent::RowsChanged(_)
                    | ExplorerEvent::ListingChanged { .. }
                    | ExplorerEvent::TabsChanged
                    | ExplorerEvent::PaneFocused
                    | ExplorerEvent::SizesChanged { .. }
                    | ExplorerEvent::TabChanged(_) => this.refresh(window, cx),
                    _ => {}
                },
            )];
        let mut pane = PreviewPane {
            explorer,
            thumbs,
            summary: Summary::default(),
            shown: Shown::Nothing,
            view: None,
            repo_card: None,
            seen: None,
            scanned: None,
            generation: 0,
            _settling: None,
            deck_watch: None,
            _subscriptions: subscriptions,
        };
        pane.refresh(window, cx);
        pane
    }

    /// What the selection comes to, once settled.
    pub fn shown(&self) -> &Shown {
        &self.shown
    }

    /// The folder's summary.
    pub fn summary(&self) -> &Summary {
        &self.summary
    }

    /// The item's preview, when one item is shown.
    pub fn entry_view(&self) -> Option<&Entity<EntryView>> {
        self.view.as_ref()
    }

    /// Looks at the active tab again.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (fingerprint, scan, rows, selection, summary_of) = {
            let explorer = self.explorer.read(cx);
            let Some(tab) = explorer.active_tab() else {
                return;
            };
            // What the folder came to once its sizes were measured (not for a search, which has no sizes).
            let sized = if tab.find.is_none() {
                explorer
                    .size_run(&tab.path)
                    .and_then(|run| run.sized().map(|line| line.total))
            } else {
                None
            };
            let fingerprint = Fingerprint::of(tab, sized);
            // The scan is kept while the listing and the path are the same.
            let scan = match &self.scanned {
                Some((entries, path, scan)) if *entries == fingerprint.entries && *path == fingerprint.summary_path => {
                    *scan
                }
                _ => Scan::of(tab),
            };
            let summary_of = if self.seen.as_ref() == Some(&fingerprint) {
                None
            } else {
                Some(Summary::of(tab, scan, sized))
            };
            (fingerprint, scan, tab.rows.clone(), tab.selection.clone(), summary_of)
        };
        let Some(summary) = summary_of else {
            return;
        };
        self.scanned = Some((fingerprint.entries, fingerprint.summary_path.clone(), scan));
        self.seen = Some(fingerprint);
        if summary != self.summary {
            self.summary = summary;
            cx.notify();
        }
        let count = selection.len();
        self.generation += 1;
        let generation = self.generation;
        if count == 0 {
            self._settling = None;
            self.set_shown(Shown::Nothing, window, cx);
            return;
        }
        self._settling = Some(cx.spawn_in(window, async move |this, cx| {
            // One item waits for the selection to rest; several are shown at once.
            if count == 1 {
                cx.background_executor().timer(SETTLE).await;
            }
            let shown = cx
                .background_executor()
                .spawn(async move { resolve(&rows, &selection) })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.generation == generation {
                    this.set_shown(shown, window, cx);
                }
            })
            .ok();
        }));
    }

    fn set_shown(&mut self, shown: Shown, window: &mut Window, cx: &mut Context<Self>) {
        // The deck's thumbnails are asked for only while it shows.
        match &shown {
            Shown::Many(many) => {
                let scale = window.scale_factor();
                let wants: Vec<Want> = many
                    .deck
                    .iter()
                    .filter_map(|e| Want::for_entry(e, THUMB, scale))
                    .collect();
                let viewer = cx.entity_id();
                self.thumbs
                    .update(cx, |thumbs, cx| thumbs.show(viewer, "preview-deck", wants, cx));
                if self.deck_watch.is_none() {
                    self.deck_watch = Some(cx.observe(&self.thumbs, |_, _, cx| cx.notify()));
                }
            }
            _ => {
                if self.deck_watch.take().is_some() {
                    let viewer = cx.entity_id();
                    self.thumbs.update(cx, |thumbs, cx| thumbs.release(viewer, cx));
                }
            }
        }
        match &shown {
            Shown::One(entry) => {
                let same = self.view.as_ref().is_some_and(|view| {
                    let shown = view.read(cx).entry();
                    shown.path == entry.path && shown.modified == entry.modified
                });
                if !same {
                    let entry = entry.clone();
                    self.view = Some(cx.new(|cx| EntryView::new(entry, Place::Pane, window, cx)));
                }
            }
            _ => self.view = None,
        }
        if self.shown != shown {
            self.shown = shown;
        }
        self.sync_repo(window, cx);
        cx.notify();
    }

    /// Whether the folder's card is the plain one (not the Recycle Bin's, the Gallery's, Recent's, This PC's or
    /// a closed folder's) and so has the repository's card under it when the folder is one.
    fn plain_folder_card(&self) -> bool {
        let summary = &self.summary;
        !summary.no_access
            && !is_recycle_bin(&summary.path)
            && !is_gallery(&summary.path)
            && !is_recent(&summary.path)
            && summary.path != THIS_PC
    }

    /// Keeps the repository's card (it reads onefetch and tokei) only while it is on screen: nothing selected in a
    /// repository's own folder.
    fn sync_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wanted = matches!(self.shown, Shown::Nothing) && self.summary.repo && self.plain_folder_card();
        match (wanted, &self.repo_card) {
            (true, Some(card)) => {
                let path = self.summary.path.clone();
                card.update(cx, |card, cx| card.set_path(path, cx));
            }
            (true, None) => {
                let path = self.summary.path.clone();
                let tools = tools_model(window, cx);
                self.repo_card = Some(cx.new(|cx| RepoCard::new(path, tools, cx)));
            }
            (false, Some(_)) => self.repo_card = None,
            (false, None) => {}
        }
    }

    /// The repository's card, while it shows.
    pub fn repo_card(&self) -> Option<&Entity<RepoCard>> {
        self.repo_card.as_ref()
    }
}

impl Render for PreviewPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_retired(window);
        for image in self.thumbs.update(cx, |thumbs, _| thumbs.take_evicted()) {
            window.drop_image(image).ok();
        }
        let t = theme::tokens(cx).clone();
        let body = match (&self.shown, &self.view) {
            (Shown::One(_), Some(view)) => view.clone().into_any_element(),
            (Shown::Many(many), _) => self.many(many.clone(), &t, window, cx),
            _ => self.folder(&t, cx),
        };
        col()
            .id("preview-pane")
            .role(Role::Complementary)
            .aria_label(t!("Preview pane"))
            .test_support()
            .size_full()
            .min_w_0()
            .bg(t.background)
            .text_start()
            .child(body)
    }
}

// ---- Several items ----------------------------------------------------------------------------------------

impl PreviewPane {
    fn many(&self, many: Many, t: &theme::Tokens, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let scale = window.scale_factor();
        let thumbs = self.thumbs.read(cx);
        let looks: Vec<_> = many
            .deck
            .iter()
            .map(|e| (e.clone(), thumbs.look(e, THUMB, scale)))
            .collect();
        let offsets: &[f32] = if looks.len() == 2 {
            &[-14., 14.]
        } else {
            &[-20., 0., 20.]
        };
        let lift: &[f32] = if looks.len() == 2 { &[2., 2.] } else { &[6., -2., 6.] };
        let deck = div()
            .relative()
            .flex_none()
            .w(px(144.))
            .h(px(96.))
            .children(looks.into_iter().enumerate().map(|(i, (entry, look))| {
                let kind = kind_of(&entry);
                div()
                    .absolute()
                    .left(px(72. - CARD / 2. + offsets.get(i).copied().unwrap_or(0.)))
                    .top(px(48. - CARD / 2. + lift.get(i).copied().unwrap_or(0.)))
                    .size(px(CARD))
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .rounded(theme::size::RADIUS_LG)
                    .border_1()
                    .border_color(t.border.opacity(0.8))
                    .bg(t.card)
                    .shadow_md()
                    .child(thumbnail::thumbnail_square(&look, kind, THUMB, 36., false, cx))
            }));

        let mut kinds = Vec::new();
        if many.folders > 0 {
            kinds.push(plural(many.folders, "folder"));
        }
        if many.files > 0 {
            kinds.push(plural(many.files, "file"));
        }
        let mut sub = kinds.join(", ");
        if many.files > 0 {
            sub.push_str(&format!(" · {}", format_size(many.bytes)));
        }
        let language = crate::i18n::language();
        let span = (many.oldest > 0 && many.newest > 0).then(|| {
            let (oldest, newest) = (
                format_date(many.oldest, None, language),
                format_date(many.newest, None, language),
            );
            if oldest == newest {
                newest
            } else {
                format!("{oldest} – {newest}")
            }
        });
        let title = t!("{n} items selected", n = format_count(many.count));
        let paths = many.paths.clone();
        // Two files, on disk, can be compared; any items on disk can be renamed and zipped.
        let pair = (many.count == 2 && many.files == 2 && many.paths.len() == 2)
            .then(|| (many.paths[0].clone(), many.paths[1].clone()));
        let buttons = many.writable.then(|| {
            let zip_paths = paths.clone();
            row()
                .flex_wrap()
                .justify_center()
                .gap(space(1.5))
                .mt(space(1.))
                .children(pair.map(|(left, right)| {
                    Button::new("preview-compare")
                        .outline()
                        .small()
                        .icon(IconName::GitCompareArrows)
                        .label(t!("Compare"))
                        .accessibility_label(t!("Compare"))
                        .on_click(move |_, window, cx| compare_files::open(window, cx, left.clone(), right.clone()))
                }))
                .child(
                    Button::new("preview-rename")
                        .outline()
                        .small()
                        .icon(IconName::PenLine)
                        .label(t!("Rename"))
                        .accessibility_label(t!("Rename"))
                        .on_click(move |_, window, cx| batch_rename::open(window, cx, paths.clone())),
                )
                .child(
                    Button::new("preview-zip")
                        .outline()
                        .small()
                        .icon(IconName::FileArchive)
                        .label(t!("Zip"))
                        .accessibility_label(t!("Zip"))
                        .on_click(move |_, window, cx| zip(window, cx, zip_paths.clone())),
                )
        });
        centered(cx)
            .id("preview-many")
            .role(Role::Group)
            .aria_label(t!("Selected items"))
            .test_support()
            .p(space(4.))
            .gap(space(3.))
            .child(deck)
            .child(
                col()
                    .items_center()
                    .gap(space(0.5))
                    .child(
                        div()
                            .id("preview-many-title")
                            .role(Role::Status)
                            .aria_label(title.clone())
                            .test_support()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.foreground)
                            .child(title),
                    )
                    .child(
                        div()
                            .id("preview-many-totals")
                            .role(Role::Status)
                            .aria_label(sub.clone())
                            .test_support()
                            .text_size(theme::size::XS)
                            .child(sub),
                    )
                    .children(span.map(|span| {
                        div()
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground.opacity(0.8))
                            .child(span)
                    })),
            )
            .children(buttons)
            .into_any_element()
    }

    // ---- Nothing selected -------------------------------------------------------------------------------

    /// The card about the folder shown.
    fn folder(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let summary = &self.summary;
        let total = summary.total.filter(|total| *total > 0);
        let named = |name: SharedString| {
            div()
                .id("preview-folder-name")
                .role(Role::Heading)
                .aria_label(name.clone())
                .test_support()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.foreground)
                .child(isolate(name))
        };
        let line = |text: String| {
            div()
                .id("preview-folder-line")
                .role(Role::Status)
                .aria_label(text.clone())
                .test_support()
                .child(text)
        };
        let hint = |text: SharedString| div().text_size(theme::size::XS).child(text);
        let card = |name: &'static str| centered(cx).id(name).role(Role::Group).test_support();
        let muted_icon = |glyph: IconName| {
            icon(glyph)
                .size(px(56.))
                .flex_none()
                .text_color(t.muted_foreground.opacity(0.7))
        };
        if summary.no_access {
            let name = base_name(&summary.path);
            let name = if name.is_empty() { summary.path.as_str() } else { name };
            let items = if summary.items > 0 {
                format!("{} · ", items(summary.items))
            } else {
                String::new()
            };
            return card("preview-no-access")
                .aria_label(t!("No access"))
                .child(
                    icon(IconName::FolderLock)
                        .size(px(56.))
                        .flex_none()
                        .text_color(t.muted_foreground.opacity(0.6)),
                )
                .child(named(name.to_string().into()))
                .child(line(format!("{items}{}", t!("No access"))))
                .into_any_element();
        }
        if is_recycle_bin(&summary.path) {
            return card("preview-bin")
                .aria_label(t!("Recycle Bin"))
                .child(muted_icon(IconName::Trash))
                .child(named(t!("Recycle Bin")))
                .child(line(match total {
                    Some(total) => format!(
                        "{} · {}",
                        items(summary.items),
                        t!("{size} in total", size = format_size(total))
                    ),
                    None => items(summary.items),
                }))
                .child(hint(t!("Select an item to look at it before you restore it")))
                .into_any_element();
        }
        if is_gallery(&summary.path) {
            let what = if summary.items == 1 {
                "photo or video"
            } else {
                "photos and videos"
            };
            return card("preview-gallery")
                .aria_label(t!("Gallery"))
                .child(muted_icon(IconName::Images))
                .child(named(t!("Gallery")))
                .child(line(format!("{} {what}", format_count(summary.items))))
                .child(hint(t!("Select one to look at it")))
                .into_any_element();
        }
        if is_recent(&summary.path) {
            return card("preview-recent")
                .aria_label(t!("Recent"))
                .child(muted_icon(IconName::Clock))
                .child(named(t!("Recent")))
                .child(line(format!("{} opened lately", items(summary.items))))
                .child(hint(t!("Select a file to preview it")))
                .into_any_element();
        }
        let name = if summary.path == THIS_PC || base_name(&summary.path).is_empty() {
            t!("This PC").to_string()
        } else {
            base_name(&summary.path).to_string()
        };
        card("preview-nothing")
            .aria_label(t!("Select a file to preview it"))
            .child(
                icon(IconName::FolderOpen)
                    .size(px(56.))
                    .flex_none()
                    .text_color(theme::tailwind("amber-500").unwrap_or_default()),
            )
            .child(named(name.into()))
            .child(line(match summary.total {
                Some(total) => format!(
                    "{} · {}",
                    items(summary.items),
                    t!("{size} in total", size = format_size(total))
                ),
                None => items(summary.items),
            }))
            .child(hint(t!("Select a file to preview it")))
            .children(self.repo_card.clone())
            .into_any_element()
    }
}

/// Zip: a .zip beside the items, after looking for secrets in them (`compressEntries`).
fn zip(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    check::secrets_check_first(window, cx, paths.clone(), t!("Compress anyway"), move |_, cx| {
        let paths = paths.clone();
        explorer::explorer(cx).update(cx, |explorer, cx| {
            explorer.compress(paths, CompressFormat::Zip, None, cx).detach()
        });
    });
}

// ---- The setting ------------------------------------------------------------------------------------------

/// Whether the pane is on screen: the setting is on, and the tab is a folder (This PC has nothing to preview,
/// unless it shows a search) (`showPreviewPane && (path !== THIS_PC || find)`).
pub fn should_show(cx: &App) -> bool {
    if !state::settings(cx).show_preview_pane {
        return false;
    }
    explorer::try_explorer(cx).is_none_or(|explorer| {
        explorer
            .read(cx)
            .active_tab()
            .is_none_or(|tab| tab.path != THIS_PC || tab.find.is_some())
    })
}

/// Alt+P: shows the pane or hides it (the `TogglePreview` action).
pub fn toggle(cx: &mut App) {
    state::update(cx, |settings| settings.show_preview_pane = !settings.show_preview_pane);
}
