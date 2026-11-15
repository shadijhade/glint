//! Settings (ported from src/components/SettingsView.tsx, and reworked in 0.13).
//!
//! **How it shows.** A dialog of the window's dialog layer (54 rem wide, at most 85% of the window high): a side
//! list on the left, with a search box over its pages in groups (Look, Browse, Search, Connect, Glint), and one page
//! at a time on the right, under a header that says what the page is for. It opens on the Overview ([`overview`]):
//! how Glint is set up (instant search, tools, clouds, updates, theme) and suggestions of what would make it better
//! (`crate::suggestions`). [`open`] shows it and [`open_at`] opens the page with a section (`"settings-about"`,
//! where an update installs). Escape or a click outside closes it. Wire it to `actions::OpenSettings`.
//!
//! **Search.** Typing in the box lists the settings whose names or other words match (`pages::find`); choosing one
//! opens its page with the row marked for a moment and scrolled into view. Ctrl+F goes to the box.
//!
//! **Where the settings live.** Every control reads `state::settings(cx)` and writes through `state::update`,
//! so the app reacts (theme, the tray, the explorer) and saves by itself; Settings keeps only what is being typed
//! and what core said last.
//!
//! | TypeScript                                   | Here                                               |
//! |----------------------------------------------|----------------------------------------------------|
//! | `Section`, `Row`, `SwitchRow`, `Segmented`   | [`widgets`]                                        |
//! | `PatternEditor`                              | [`pattern::PatternEditor`]                         |
//! | `InstantSearch`                              | [`instant::InstantSearch`]                         |
//! | `ContentIndexSection`                        | [`content::ContentIndex`]                          |
//! | `AgentsSection`                              | [`agents::Agents`]                                 |
//! | `CloudsSection`                              | [`clouds::Clouds`]                                 |
//! | `ToolsSection`                               | [`tools::Tools`]                                   |
//! | `FileExplorerRows`                           | [`integration::FileExplorerRows`]                  |
//! | About, `UpdateRow`, `SettingsFileRows`       | [`about::About`], [`transfer::Transfer`]           |
//! | the other sections                           | `general.rs`, methods of [`SettingsView`]          |
//! | `invoke(..)` and Tauri's dialogs             | [`backend::Backend`] (a fake in the tests)         |
//!
//! The connect-a-cloud and folder-rules dialogs are asked for with a [`SettingsEvent`] after Settings closes
//! itself. Which shortcut another app holds is told to the view with [`SettingsView::set_quick_taken`] by whoever
//! registers the hotkey.

pub mod about;
pub mod agents;
mod backend;
pub mod clouds;
pub mod content;
mod general;
pub mod instant;
pub mod integration;
mod overview;
pub mod pages;
pub mod pattern;
mod shortcut;
pub mod tools;
pub mod transfer;
mod widgets;

/// Public to the crate so the accessibility sweep (a11y_tests) can open Settings over the same fake backend.
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod tests_general;
#[cfg(test)]
mod tests_services;

pub(crate) use backend::fetch_channel;
pub use backend::{Backend, CoreBackend, Fetch, Fut, IndexInfo, IndexState, Known, McpSetup, ToolInfo, Volume};
pub use clouds::Request as CloudsRequest;
pub use pages::Page;

use crate::kit::prelude::*;
use crate::{core_bridge, dialogs, persistence, state, t, theme};
use glint_core::{openers::Opener, term::Shell};
use glint_model::store::Settings;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The section Settings opens at to show About, where an update installs (`openAbout`).
pub const ABOUT: &str = "settings-about";

/// How long a row search opened stays marked.
const MARKED_FOR: Duration = Duration::from_millis(2600);

/// What Settings asks of the rest of the app. It closes itself first, as the TypeScript's did ("Settings sits
/// above everything: it goes first"); the coordinator opens the dialog asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsEvent {
    /// Clouds → Connect a cloud: the dialog that connects one (`addRemote`).
    ConnectCloud,
    /// Clouds → Folder rules: the folder rules dialog (`openRules`).
    FolderRules,
}

/// The open Settings, so a second request doesn't stack another on it.
struct Open(WeakEntity<SettingsView>);

impl Global for Open {}

/// The shortcut another app was last said to hold ([`tell_quick_taken`]), for a Settings opened afterwards.
#[derive(Default)]
struct QuickTaken(Option<String>);

impl Global for QuickTaken {}

/// Says that another app holds the quick search shortcut `shortcut` (`None`: it is free now, or none is asked
/// for): the open Settings view's Shortcut row warns about it, and so does one opened later. Call it where
/// `platform::tray::set_hotkey` fails or succeeds.
pub fn tell_quick_taken(shortcut: Option<String>, cx: &mut App) {
    cx.set_global(QuickTaken(shortcut.clone()));
    if let Some(open) = cx.try_global::<Open>().and_then(|open| open.0.upgrade()) {
        open.update(cx, |view, cx| view.set_quick_taken(shortcut, cx));
    }
}

/// Settings: the side list, and the page shown.
pub struct SettingsView {
    pub(crate) backend: Arc<dyn Backend>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// The page shown.
    page: Page,
    /// The sections the page drew last, in order (their positions in the scrolling column).
    order: Vec<&'static str>,
    /// A section asked for, scrolled to once its page has drawn it.
    pending_section: Option<String>,
    /// The search box; while it holds words, what they find shows instead of the page.
    pub(crate) search: Entity<InputState>,
    /// Until when the row search opened stays marked; it's scrolled into view once drawn.
    marked_until: Option<Instant>,
    reveal_mark: bool,
    /// A shortcut another app holds, when the tray could not take it (`quickTaken`).
    // Told by whoever registers the hotkey, through [`tell_quick_taken`] (the open view hears it at once; a Settings
    // opened later starts with what was last said).
    quick_taken: Option<String>,
    // What is typed.
    pub(crate) editor_path: Entity<InputState>,
    pub(crate) shortcut_box: Entity<InputState>,
    pub(crate) shortcut_error: Option<SharedString>,
    // The parts that talk to the PC.
    pub(crate) find_exclusions: Entity<pattern::PatternEditor>,
    pub(crate) exclusions: Entity<pattern::PatternEditor>,
    pub(crate) instant: Entity<instant::InstantSearch>,
    pub(crate) content: Entity<content::ContentIndex>,
    pub(crate) agents: Entity<agents::Agents>,
    pub(crate) clouds: Entity<clouds::Clouds>,
    pub(crate) tools: Entity<tools::Tools>,
    pub(crate) integration: Entity<integration::FileExplorerRows>,
    pub(crate) about: Entity<about::About>,
    pub(crate) shells: Vec<Shell>,
    pub(crate) editors: Vec<Opener>,
    /// The fixed drives' room, for the full-drive suggestion; None until core has said.
    pub(crate) drives: Option<Vec<crate::suggestions::Room>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    /// Settings over `backend`, on the page with `section` ("settings-about") or the Overview. `store` is whether this
    /// is the Microsoft Store's copy.
    pub fn new(
        backend: Arc<dyn Backend>,
        store: bool,
        section: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = state::settings(cx).clone();
        let input = |window: &mut Window, cx: &mut Context<Self>, text: &str, placeholder: SharedString| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(text.to_string())
                    .placeholder(placeholder)
            })
        };
        let editor_path = input(window, cx, &settings.editor_path, t!("System default"));
        let shortcut_box = input(window, cx, "", t!("Another shortcut, e.g. Ctrl+Alt+K"));
        let search = input(window, cx, "", t!("Search settings"));

        let find_exclusions = cx.new(|cx| {
            pattern::PatternEditor::new(
                pattern::Which::FindExclusions,
                t!("Add a folder or pattern, e.g. node_modules or *.tmp"),
                window,
                cx,
            )
        });
        let exclusions = cx.new(|cx| {
            pattern::PatternEditor::new(
                pattern::Which::TextExclusions,
                t!("Add a pattern: folder, *.log, or file name"),
                window,
                cx,
            )
        });
        let instant = {
            let backend = backend.clone();
            cx.new(|cx| instant::InstantSearch::new(backend, store, window, cx))
        };
        let content = {
            let backend = backend.clone();
            cx.new(|cx| content::ContentIndex::new(backend, window, cx))
        };
        let agents = {
            let backend = backend.clone();
            cx.new(|cx| agents::Agents::new(backend, window, cx))
        };
        let tools = {
            let backend = backend.clone();
            cx.new(|cx| tools::Tools::new(backend, window, cx))
        };
        let clouds = {
            let backend = backend.clone();
            let tools = tools.clone();
            cx.new(|cx| clouds::Clouds::new(backend, tools, window, cx))
        };
        let integration = {
            let backend = backend.clone();
            cx.new(|cx| integration::FileExplorerRows::new(backend, window, cx))
        };
        let about = {
            let backend = backend.clone();
            cx.new(|cx| about::About::new(backend, store, cx))
        };

        let mut subscriptions = vec![
            // What is typed goes to the settings as it is typed; a change from elsewhere (Reset, an import)
            // comes back into the boxes.
            Self::bind(&editor_path, |s| &s.editor_path, |s, v| s.editor_path = v, window, cx),
            cx.subscribe_in(
                &shortcut_box,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.set_custom_shortcut(window, cx),
                    InputEvent::Change => {
                        this.shortcut_error = None;
                        cx.notify();
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(&search, window, |this, _, event: &InputEvent, window, cx| match event {
                // Enter opens what comes first.
                InputEvent::PressEnter { .. } => {
                    let query = this.search.read(cx).value().to_string();
                    if let Some(first) = pages::find(&query).first() {
                        this.open_found(first, window, cx);
                    }
                }
                InputEvent::Change => {
                    this.scroll.set_offset(point(px(0.), px(0.)));
                    cx.notify();
                }
                _ => {}
            }),
            cx.observe_global_in::<state::UiState>(window, |this, window, cx| {
                this.sync_inputs(window, cx);
                cx.notify();
            }),
            cx.observe_global::<persistence::Persistence>(|_, cx| cx.notify()),
            cx.observe(&tools, |_, _, cx| cx.notify()),
            cx.observe(&instant, |_, _, cx| cx.notify()),
            cx.observe(&clouds, |_, _, cx| cx.notify()),
            cx.observe(&about, |_, _, cx| cx.notify()),
            cx.subscribe_in(&clouds, window, |_, _, request: &clouds::Request, window, cx| {
                // Settings goes first; the dialog asked for opens in its place.
                window.close_dialog(cx);
                cx.emit(match request {
                    clouds::Request::Connect => SettingsEvent::ConnectCloud,
                    clouds::Request::Rules => SettingsEvent::FolderRules,
                });
            }),
        ];
        subscriptions.shrink_to_fit();
        widgets::mark(None);

        let page = section.and_then(Page::of_section).unwrap_or(Page::Overview);
        let mut this = Self {
            backend: backend.clone(),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            page,
            order: Vec::new(),
            pending_section: section.map(str::to_string),
            search,
            marked_until: None,
            reveal_mark: false,
            quick_taken: cx.try_global::<QuickTaken>().and_then(|taken| taken.0.clone()),
            editor_path,
            shortcut_box,
            shortcut_error: None,
            find_exclusions,
            exclusions,
            instant,
            content,
            agents,
            clouds,
            tools,
            integration,
            about,
            shells: Vec::new(),
            editors: Vec::new(),
            drives: None,
            _subscriptions: subscriptions,
        };
        this.load_shells_and_editors(window, cx);
        this.load_drives(window, cx);
        this
    }

    /// Ties an input to a text setting: typing writes it. Returns the subscription.
    fn bind(
        input: &Entity<InputState>,
        get: fn(&Settings) -> &String,
        set: fn(&mut Settings, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(input, window, move |_, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                let typed = input.read(cx).value().to_string();
                if *get(state::settings(cx)) != typed {
                    state::update(cx, |settings| set(settings, typed));
                }
            }
        })
    }

    /// Puts the settings' text back in the boxes when the settings changed from elsewhere.
    fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = state::settings(cx).editor_path.clone();
        if self.editor_path.read(cx).value().as_ref() != now.as_str() {
            self.editor_path.update(cx, |input, cx| input.set_value(now, window, cx));
        }
    }

    /// Looks for the shells the terminal can start and the editors to open files with.
    fn load_shells_and_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shells = self.backend.shells();
        let editors = self.backend.openers();
        cx.spawn_in(window, async move |this, cx| {
            let (shells, editors) = (shells.await, editors.await);
            this.update(cx, |this, cx| {
                this.shells = shells.unwrap_or_default();
                this.editors = editors
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|o| !o.terminal)
                    .collect();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Says which shortcut another app holds, so the Shortcut row warns about it (`quickTaken`).
    pub fn set_quick_taken(&mut self, shortcut: Option<String>, cx: &mut Context<Self>) {
        self.quick_taken = shortcut;
        cx.notify();
    }

    /// The shortcut another app holds, when told.
    pub fn quick_taken(&self) -> Option<&str> {
        self.quick_taken.as_deref()
    }

    /// The shortcut typed in the custom box, if it is one the tray can register; otherwise why not.
    pub(crate) fn set_custom_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.shortcut_box.read(cx).value().to_string();
        if typed.trim().is_empty() {
            return;
        }
        match shortcut::check(&typed) {
            Ok(shortcut) => {
                self.shortcut_error = None;
                state::update(cx, |settings| settings.quick_shortcut = Some(shortcut));
                self.shortcut_box
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }
            Err(error) => self.shortcut_error = Some(error),
        }
        cx.notify();
    }

    /// The page shown.
    pub fn page(&self) -> Page {
        self.page
    }

    /// The sections the page shows now, in order.
    pub fn sections(&self) -> &[&'static str] {
        &self.order
    }

    /// Every section of every page, in the side list's order (the content index's whether it shows or not).
    pub fn all_sections() -> Vec<&'static str> {
        Page::ALL.iter().flat_map(|page| page.sections().iter().copied()).collect()
    }

    /// Shows `page` from its top; a search under way is put away.
    pub fn show_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search.read(cx).value().is_empty() {
            self.search.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.go(page, cx);
    }

    fn go(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page != page {
            self.page = page;
            self.order.clear();
            widgets::mark(None);
            self.marked_until = None;
        }
        self.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    /// Opens the page with `section` ("settings-about" or "about") and scrolls it to the top.
    pub fn scroll_to(&mut self, section: &str, cx: &mut Context<Self>) {
        let Some(page) = Page::of_section(section) else {
            return;
        };
        if page != self.page {
            self.go(page, cx);
            self.pending_section = Some(section.to_string());
            return;
        }
        let slot = section.strip_prefix("settings-").unwrap_or(section);
        match self.order.iter().position(|s| *s == slot) {
            Some(0) | None => self.scroll.set_offset(point(px(0.), px(0.))),
            Some(at) => self.scroll.scroll_to_top_of_item(at),
        }
        cx.notify();
    }

    /// Opens what search found: its page, the search put away, its row marked and scrolled into view.
    fn open_found(&mut self, found: &pages::Entry, window: &mut Window, cx: &mut Context<Self>) {
        self.show_page(found.page, window, cx);
        widgets::mark(Some(found.label.into()));
        self.marked_until = Some(Instant::now() + MARKED_FOR);
        self.reveal_mark = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MARKED_FOR).await;
            this.update(cx, |this, cx| {
                if this.marked_until.is_some_and(|until| until <= Instant::now()) {
                    this.marked_until = None;
                    widgets::mark(None);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Moves the page by `dy` pixels (negative: up), as far as it goes.
    pub fn scroll_by(&self, dy: Pixels) {
        let max = self.scroll.max_offset().y;
        let mut offset = self.scroll.offset();
        offset.y = (offset.y - dy).max(-max).min(px(0.));
        self.scroll.set_offset(offset);
    }

    /// How far the page is scrolled (zero or negative).
    pub fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll.offset()
    }

    /// The page marked on the left, by its id.
    #[cfg(test)]
    pub fn current_section_name(&self) -> &'static str {
        self.page.slot()
    }

    /// Scrolls the row search marked into view once it has been drawn: a third of the way down the page.
    fn reveal_marked(&mut self, cx: &mut Context<Self>) {
        if !self.reveal_mark {
            return;
        }
        let Some(row) = widgets::marked_at() else {
            // Not drawn yet: next frame.
            cx.notify();
            return;
        };
        self.reveal_mark = false;
        let body = self.scroll.bounds();
        if row.top() >= body.top() && row.bottom() <= body.bottom() {
            return;
        }
        let max = self.scroll.max_offset().y;
        let mut offset = self.scroll.offset();
        offset.y = (offset.y - (row.top() - body.top() - body.size.height / 3.)).max(-max).min(px(0.));
        self.scroll.set_offset(offset);
        cx.notify();
    }

    /// The side list: the search box over the pages, in groups.
    fn side(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let rtl = crate::i18n::is_rtl();
        let searching = !self.search.read(cx).value().is_empty();
        let updates = self.tools.read(cx).counts().updates;
        let mut items: Vec<AnyElement> = vec![div()
            .id("settings-search")
            .flex_none()
            .pb(space(2.))
            .child(
                Input::new(&self.search)
                    .small()
                    .prefix(icon(IconName::Search).size_3p5().text_color(t.muted_foreground))
                    .cleanable(true)
                    .aria_label(t!("Search settings")),
            )
            .into_any_element()];
        let mut last_group = None;
        for page in Page::ALL {
            let group = page.group();
            if last_group != Some(group) {
                last_group = Some(group);
                if let Some(heading) = group.heading() {
                    items.push(
                        div()
                            .flex_none()
                            .pt(space(2.5))
                            .pb(space(0.5))
                            .px(space(2.5))
                            .text_size(theme::size::XXS)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.muted_foreground)
                            .child(heading.to_uppercase())
                            .into_any_element(),
                    );
                }
            }
            let chosen = page == self.page && !searching;
            let name = page.name();
            let (over, foreground, pressed, ring) = (t.accent, t.foreground, t.pressed, t.ring);
            let badge = (page == Page::Tools && updates > 0).then(|| {
                div()
                    .flex_none()
                    .px(space(1.5))
                    .rounded_full()
                    .bg(t.primary)
                    .text_size(theme::size::XXS)
                    .text_color(t.primary_foreground)
                    .child(updates.to_string())
            });
            items.push(
                // A row of its own rather than the kit's button, which centres its label: a list of names reads
                // from the start edge, each behind its sign.
                row_start()
                    .id(widgets::id(&["settings-nav", page.slot()]))
                    .role(Role::Button)
                    .aria_label(name.clone())
                    .aria_selected(chosen)
                    .test_support()
                    .tab_index(0)
                    .w_full()
                    .flex_none()
                    .h(space(7.))
                    .gap(space(2.5))
                    .px(space(2.5))
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(gpui_kit::transparent_black())
                    .cursor_pointer()
                    .text_color(if chosen { t.foreground } else { t.muted_foreground })
                    .when(chosen, |row| row.bg(over).font_weight(FontWeight::MEDIUM))
                    .hover(move |style| style.bg(over).text_color(foreground))
                    .active(move |style| style.bg(pressed))
                    .focus_visible(move |style| style.border_color(ring))
                    .on_click(cx.listener(move |this, _, window, cx| this.show_page(page, window, cx)))
                    .child(icon(page.icon()).size_4().flex_none().text_color(if chosen {
                        t.primary
                    } else {
                        t.muted_foreground
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .children(badge)
                    .into_any_element(),
            );
        }
        col()
            .id("settings-sections")
            .role(Role::Navigation)
            .aria_label(t!("Settings sections"))
            .test_support()
            .w(space(52.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .gap(space(0.25))
            .p(space(2.))
            .border_color(t.border)
            .when(rtl, |this| this.border_l_1())
            .when(!rtl, |this| this.border_r_1())
            .children(items)
    }

    /// The top of a page: its sign in a tile of the glint's colour, its name and what it is for.
    fn page_header(&self, t: &theme::Tokens, cx: &App) -> impl IntoElement {
        let page = self.page;
        row_start()
            .id("settings-page-header")
            .role(Role::Heading)
            .aria_label(page.name())
            .aria_description(page.about())
            .test_support()
            .flex_none()
            .gap(space(3.))
            .pb(space(1.))
            .child(
                div()
                    .size(space(10.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::size::RADIUS_LG)
                    .bg(theme::glint_fill(cx))
                    .child(icon(page.icon()).size_5().text_color(t.primary_foreground)),
            )
            .child(
                col()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(rems(1.125))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(page.name()),
                    )
                    .child(
                        div()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(page.about()),
                    ),
            )
    }

    /// What the search box found, or that it found nothing.
    fn results(&self, query: &str, t: &theme::Tokens, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let found = pages::find(query);
        if found.is_empty() {
            return vec![div()
                .id("settings-nothing-found")
                .role(Role::Status)
                .aria_label(t!("No setting matches “{words}”", words = query))
                .test_support()
                .py(space(10.))
                .text_center()
                .text_color(t.muted_foreground)
                .child(t!("No setting matches “{words}”", words = query))
                .into_any_element()];
        }
        let rows: Vec<AnyElement> = found
            .into_iter()
            .enumerate()
            .map(|(i, entry)| {
                let (over, ring) = (t.accent, t.ring);
                row_start()
                    .id(widgets::id(&["settings-found", &i.to_string()]))
                    .role(Role::Button)
                    .aria_label(SharedString::from(format!("{} › {}", entry.page.name(), crate::i18n::translate(entry.label))))
                    .test_support()
                    .tab_index(0)
                    .w_full()
                    .gap(space(3.))
                    .px(space(4.))
                    .py(space(2.5))
                    .border_1()
                    .border_color(gpui_kit::transparent_black())
                    .cursor_pointer()
                    .hover(move |style| style.bg(over))
                    .focus_visible(move |style| style.border_color(ring))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_found(entry, window, cx)))
                    .child(icon(entry.page.icon()).size_4().flex_none().text_color(t.primary))
                    .child(
                        col()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(theme::size::SM).child(crate::i18n::translate(entry.label)))
                            .child(
                                div()
                                    .text_size(theme::size::XS)
                                    .text_color(t.muted_foreground)
                                    .child(entry.page.name()),
                            ),
                    )
                    .child(icon(IconName::ChevronRight).size_4().flex_none().text_color(t.muted_foreground))
                    .into_any_element()
            })
            .collect();
        vec![widgets::section("search", t!("Found"), None, rows, t)]
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let viewport = window.viewport_size();
        let rem = theme::rem_size(cx);
        // min(85vh, 46rem), less the dialog's own title bar.
        let height = (viewport.height * 0.85).min(rem * 46.) - px(56.);
        let wide = viewport.width >= px(768.);
        let query = self.search.read(cx).value().trim().to_string();

        let mut body: Vec<AnyElement> = Vec::new();
        let mut order: Vec<&'static str> = Vec::new();
        if query.is_empty() {
            body.push(self.page_header(&t, cx).into_any_element());
            if self.page == Page::Overview {
                order.push("overview");
                body.push(self.overview(&t, window, cx));
            } else {
                for slot in self.page.sections() {
                    if let Some(section) = self.section_of(slot, &t, cx) {
                        order.push(slot);
                        body.push(section);
                    }
                }
            }
        } else {
            body.extend(self.results(&query, &t, cx));
        }
        self.order = order;
        // A section asked for: now the page knows where its sections are.
        if let Some(section) = self.pending_section.take() {
            self.scroll_to(&section, cx);
        }
        self.reveal_marked(cx);

        col()
            .id("settings")
            .role(Role::Group)
            .aria_label(t!("Settings"))
            .test_support()
            .h(height)
            .w_full()
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .key_context("Settings")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Ctrl+F goes to the search box.
                if event.keystroke.key == "f" && event.keystroke.modifiers.control {
                    let focus = this.search.read(cx).focus_handle(cx);
                    window.focus(&focus, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                row_start()
                    .flex_1()
                    .min_h_0()
                    .when(wide, |this| this.child(self.side(&t, cx)))
                    .child(
                        col()
                            .id("settings-body")
                            .role(Role::Group)
                            .aria_label(t!("Settings page"))
                            .test_support()
                            .track_focus(&self.focus)
                            .key_context("SettingsBody")
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                let page = this.scroll.bounds().size.height - px(40.);
                                let step = match event.keystroke.key.as_str() {
                                    "pagedown" => page,
                                    "pageup" => -page,
                                    "down" => px(48.),
                                    "up" => px(-48.),
                                    "home" => px(-1.0e6),
                                    "end" => px(1.0e6),
                                    _ => return,
                                };
                                this.scroll_by(step);
                                cx.notify();
                            }))
                            .min_w_0()
                            .flex_1()
                            .h_full()
                            .px(space(6.))
                            .py(space(5.))
                            .gap(space(5.))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .children(body)
                            .vertical_scrollbar(&self.scroll),
                    ),
            )
    }
}

/// The backend the app uses: glint-core through the app's handle.
fn app_backend(cx: &App) -> Arc<dyn Backend> {
    Arc::new(CoreBackend::new(cx.global::<core_bridge::Core>().app.clone()))
}

/// Settings are the main window's to keep (`open_main_settings`). In a secondary window (`platform::secondary`)
/// this asks the main window to open them, at `section` when given, and answers true: the caller opens nothing.
/// In the main window it answers false and does nothing.
pub fn handed_to_main(window: &mut Window, cx: &mut App, section: Option<&str>) -> bool {
    if !crate::platform::secondary::is_secondary() {
        return false;
    }
    crate::platform::secondary::open_main_settings(window, cx, section);
    true
}

/// Opens Settings at the top (the `OpenSettings` action, Ctrl+, and the toolbar's gear). If it is already open,
/// that one is returned.
pub fn open(window: &mut Window, cx: &mut App) -> Entity<SettingsView> {
    open_at(window, cx, None)
}

/// Opens Settings scrolled to `section` ("settings-about"; [`ABOUT`]). The returned view emits
/// [`SettingsEvent`]s the app answers, and takes [`SettingsView::set_quick_taken`].
pub fn open_at(window: &mut Window, cx: &mut App, section: Option<&str>) -> Entity<SettingsView> {
    let backend = app_backend(cx);
    open_with(window, cx, backend, glint_core::store_copy(), section)
}

/// [`open_at`] over a given backend (tests pass a fake).
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    store: bool,
    section: Option<&str>,
) -> Entity<SettingsView> {
    if let Some(open) = cx.try_global::<Open>().and_then(|open| open.0.upgrade()) {
        if window.has_active_dialog(cx) {
            if let Some(section) = section {
                open.update(cx, |view, cx| view.scroll_to(section, cx));
            }
            return open;
        }
    }
    let view = cx.new(|cx| SettingsView::new(backend, store, section, window, cx));
    cx.set_global(Open(view.downgrade()));
    // 54 rem wide, but never wider than 92% of the window (`w-[min(92vw,54rem)]`).
    let width = (window.viewport_size().width * 0.92).min(theme::rem_size(cx) * 54.);
    dialogs::show(window, cx, t!("Settings"), width, view.clone());
    let focus = view.read(cx).focus.clone();
    window.defer(cx, move |window, cx| window.focus(&focus, cx));
    view
}

#[cfg(test)]
mod quick_taken_tests {
    use super::tests::{open_settings, Fake};
    use super::*;
    use crate::testing;
    use gpui_kit::TestAppContext;

    #[gpui_kit::test]
    fn the_open_view_hears_a_shortcut_another_app_holds_and_a_later_one_starts_with_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Arc::new(Fake::new());
        let (mut visual, view) = open_settings(cx, &fake, None);
        assert_eq!(
            view.read_with(&visual, |v, _| v.quick_taken().map(str::to_string)),
            None
        );
        visual.update(|_, cx| tell_quick_taken(Some("Ctrl+Alt+Space".to_string()), cx));
        assert_eq!(
            view.read_with(&visual, |v, _| v.quick_taken().map(str::to_string))
                .as_deref(),
            Some("Ctrl+Alt+Space")
        );
        // Settings opened afterwards starts with what was last said.
        let (visual, later) = open_settings(cx, &fake, None);
        assert_eq!(
            later
                .read_with(&visual, |v, _| v.quick_taken().map(str::to_string))
                .as_deref(),
            Some("Ctrl+Alt+Space")
        );
        // Free again.
        let mut visual = visual;
        visual.update(|_, cx| tell_quick_taken(None, cx));
        assert_eq!(
            later.read_with(&visual, |v, _| v.quick_taken().map(str::to_string)),
            None
        );
    }
}
