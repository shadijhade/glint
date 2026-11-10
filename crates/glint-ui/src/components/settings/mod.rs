//! Settings (ported from src/components/SettingsView.tsx).
//!
//! **How it shows.** The TypeScript opened Settings as a modal `<dialog>` above everything (54 rem wide, at
//! most 85% of the window high), with a column of section names on the left, one scrolling page of sections
//! on the right, and Escape or a click outside closing it. Here it is a dialog of the window's dialog layer,
//! the same: [`open`] shows it and [`open_at`] opens it scrolled to a section (`"settings-about"`, where an
//! update installs). Wire it to `actions::OpenSettings`.
//!
//! **Where the settings live.** Every control reads `state::settings(cx)` and writes through `state::update`,
//! so the app reacts (theme, language, the tray, the explorer) and saves by itself; Settings keeps only what is
//! being typed and what core said last.
//!
//! | TypeScript                                   | Here                                               |
//! |----------------------------------------------|----------------------------------------------------|
//! | `Section`, `Row`, `SwitchRow`, `Segmented`   | [`widgets`]                                        |
//! | `PatternEditor`                              | [`pattern::PatternEditor`]                         |
//! | `InstantSearch`                              | [`instant::InstantSearch`]                         |
//! | `ContentIndexSection`                        | [`content::ContentIndex`]                          |
//! | `MediaSearchSection`                         | [`media::MediaSearch`]                             |
//! | `DevicesSection`                             | [`devices::Devices`]                               |
//! | `AgentsSection`                              | [`agents::Agents`]                                 |
//! | `CloudsSection`                              | [`clouds::Clouds`]                                 |
//! | `ToolsSection`, `Meaning*Row`                | [`tools::Tools`]                                   |
//! | `FileExplorerRows`                           | [`integration::FileExplorerRows`]                  |
//! | About, `UpdateRow`, `SettingsFileRows`       | [`about::About`], [`transfer::Transfer`]           |
//! | the other sections                           | `general.rs`, methods of [`SettingsView`]          |
//! | `invoke(..)` and Tauri's dialogs             | [`backend::Backend`] (a fake in the tests)         |
//!
//! **What waits for other parts** (each has a `PORT ME` where it is): the updater (About), `refreshZoxide`,
//! the logo, and the connect-a-cloud and folder-rules dialogs, which Settings asks for with a
//! [`SettingsEvent`] after closing itself. Which shortcut another app holds is told to the view with
//! [`SettingsView::set_quick_taken`] by whoever registers the hotkey.

pub mod about;
pub mod agents;
mod backend;
pub mod clouds;
pub mod content;
pub mod devices;
mod general;
pub mod instant;
pub mod integration;
pub mod media;
pub mod pattern;
mod shortcut;
pub mod tools;
pub mod transfer;
mod widgets;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_general;
#[cfg(test)]
mod tests_services;

pub(crate) use backend::from_picker;
pub use backend::{Backend, CoreBackend, Fetch, Fut, IndexInfo, IndexState, Known, McpSetup, ToolInfo, Volume};
pub use clouds::Request as CloudsRequest;

use crate::kit::prelude::*;
use crate::{core_bridge, dialogs, persistence, state, t, theme};
use glint_core::{openers::Opener, term::Shell};
use glint_model::store::Settings;
use std::sync::Arc;

/// The section Settings opens at to show About, where an update installs (`openAbout`).
pub const ABOUT: &str = "settings-about";

/// What Settings asks of the rest of the app. It closes itself first, as the TypeScript's did ("Settings sits
/// above everything: it goes first"); the coordinator opens the dialog asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsEvent {
    /// Clouds → Connect a cloud: the dialog that connects one (`addRemote`).
    ConnectCloud,
    /// Clouds → Folder rules: the folder rules dialog (`openRules`).
    FolderRules,
}

/// Every section's id (after `settings-`) in the order the names are listed on the left (`SECTIONS`).
const NAV: [&str; 14] = [
    "appearance",
    "files",
    "instant-search",
    "find-files-by-name",
    "quick-search",
    "ai-agents",
    "search-text-ripgrep",
    "text-search-skips",
    "terminal",
    "check",
    "clouds",
    "tools",
    "history",
    "about",
];

/// A section's short name in the list on the left.
fn nav_name(slot: &str) -> SharedString {
    match slot {
        "appearance" => t!("Appearance"),
        "files" => t!("Files"),
        "instant-search" => t!("Instant search"),
        "find-files-by-name" => t!("Find files"),
        "quick-search" => t!("Quick search"),
        "ai-agents" => t!("AI agents"),
        "search-text-ripgrep" => t!("Search text"),
        "text-search-skips" => t!("Text search skips"),
        "terminal" => t!("Terminal"),
        "check" => t!("Check"),
        "clouds" => t!("Clouds"),
        "tools" => t!("Tools"),
        "history" => t!("History"),
        "about" => t!("About"),
        _ => SharedString::default(),
    }
}

/// The open Settings, so a second request doesn't stack another on it.
struct Open(WeakEntity<SettingsView>);

impl Global for Open {}

/// Settings: the section names, and the page of sections.
pub struct SettingsView {
    pub(crate) backend: Arc<dyn Backend>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// The sections in the order the page drew them last (their positions in the scrolling column).
    order: Vec<&'static str>,
    /// A section asked for as Settings opened, scrolled to once the page knows where the sections are.
    pending_section: Option<String>,
    /// A shortcut another app holds, when the tray could not take it (`quickTaken`).
    // PORT ME(coordinator): `platform::tray::set_hotkey` knows when another app holds the shortcut (it errors) but
    // startup.rs only words it as a notice; tell the open view with `set_quick_taken` when that happens.
    quick_taken: Option<String>,
    // What is typed.
    pub(crate) default_search_path: Entity<InputState>,
    pub(crate) editor_path: Entity<InputState>,
    pub(crate) yara_rules: Entity<InputState>,
    pub(crate) shortcut_box: Entity<InputState>,
    pub(crate) shortcut_error: Option<SharedString>,
    // The parts that talk to the PC.
    pub(crate) find_exclusions: Entity<pattern::PatternEditor>,
    pub(crate) exclusions: Entity<pattern::PatternEditor>,
    pub(crate) instant: Entity<instant::InstantSearch>,
    pub(crate) content: Entity<content::ContentIndex>,
    pub(crate) media: Entity<media::MediaSearch>,
    pub(crate) devices: Entity<devices::Devices>,
    pub(crate) agents: Entity<agents::Agents>,
    pub(crate) clouds: Entity<clouds::Clouds>,
    pub(crate) tools: Entity<tools::Tools>,
    pub(crate) integration: Entity<integration::FileExplorerRows>,
    pub(crate) about: Entity<about::About>,
    pub(crate) shells: Vec<Shell>,
    pub(crate) editors: Vec<Opener>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    /// Settings over `backend`, scrolled to `section` ("settings-about") or the top. `store` is whether this is
    /// the Microsoft Store's copy.
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
        let default_search_path = input(window, cx, &settings.default_search_path, t!("None"));
        let editor_path = input(window, cx, &settings.editor_path, t!("System default"));
        let yara_rules = input(window, cx, &settings.yara_rules, t!("None chosen"));
        let shortcut_box = input(window, cx, "", t!("Another shortcut, e.g. Ctrl+Alt+K"));

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
        let media = {
            let backend = backend.clone();
            cx.new(|cx| media::MediaSearch::new(backend, window, cx))
        };
        let devices = {
            let backend = backend.clone();
            cx.new(|cx| devices::Devices::new(backend, window, cx))
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
            Self::bind(
                &default_search_path,
                |s| &s.default_search_path,
                |s, v| s.default_search_path = v,
                window,
                cx,
            ),
            Self::bind(&editor_path, |s| &s.editor_path, |s, v| s.editor_path = v, window, cx),
            Self::bind(&yara_rules, |s| &s.yara_rules, |s, v| s.yara_rules = v, window, cx),
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
            cx.observe_global_in::<state::UiState>(window, |this, window, cx| {
                this.sync_inputs(window, cx);
                cx.notify();
            }),
            cx.observe_global::<persistence::Persistence>(|_, cx| cx.notify()),
            cx.observe(&tools, |_, _, cx| cx.notify()),
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

        let mut this = Self {
            backend: backend.clone(),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            order: Vec::new(),
            pending_section: section.map(str::to_string),
            quick_taken: None,
            default_search_path,
            editor_path,
            yara_rules,
            shortcut_box,
            shortcut_error: None,
            find_exclusions,
            exclusions,
            instant,
            content,
            media,
            devices,
            agents,
            clouds,
            tools,
            integration,
            about,
            shells: Vec::new(),
            editors: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.load_shells_and_editors(window, cx);
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
        let settings = state::settings(cx).clone();
        for (input, now) in [
            (&self.default_search_path, settings.default_search_path),
            (&self.editor_path, settings.editor_path),
            (&self.yara_rules, settings.yara_rules),
        ] {
            if input.read(cx).value().as_ref() != now.as_str() {
                input.update(cx, |input, cx| input.set_value(now, window, cx));
            }
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

    /// The names of the sections drawn, in the page's order.
    pub fn sections(&self) -> &[&'static str] {
        &self.order
    }

    /// Scrolls so `section` ("settings-about" or "about") is at the top (`scrollIntoView`).
    pub fn scroll_to(&mut self, section: &str, cx: &mut Context<Self>) {
        let slot = section.strip_prefix("settings-").unwrap_or(section);
        if let Some(at) = self.order.iter().position(|s| *s == slot) {
            self.scroll.scroll_to_top_of_item(at);
            cx.notify();
        }
    }

    /// Moves the page by `dy` pixels (negative: up), as far as it goes.
    pub fn scroll_by(&self, dy: Pixels) {
        let max = self.scroll.max_offset().y;
        let mut offset = self.scroll.offset();
        offset.y = (offset.y - dy).max(-max).min(px(0.));
        self.scroll.set_offset(offset);
    }

    /// The section at the top of the page, marked on the left (`onScroll`): the last one whose top has passed
    /// the top of the page, and the last section once scrolled to the end.
    fn current_section(&self) -> usize {
        let bounds = self.scroll.bounds();
        let max = self.scroll.max_offset().y;
        let at_end = max > px(0.) && self.scroll.offset().y <= -max + px(2.);
        let last = |slot: &str| NAV.iter().position(|s| *s == slot);
        if at_end {
            return NAV.len() - 1;
        }
        let top = bounds.origin.y + px(40.);
        let mut current = 0;
        for (at, slot) in self.order.iter().enumerate() {
            // The bounds of an item are where layout put it; the page is scrolled by `offset` from there.
            let offset = self.scroll.offset().y;
            let passed = self
                .scroll
                .bounds_for_item(at)
                .is_some_and(|item| item.origin.y + offset <= top);
            if passed {
                if let Some(nav) = last(slot) {
                    current = nav;
                }
            }
        }
        current
    }

    /// How far the page is scrolled (zero or negative).
    pub fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll.offset()
    }

    /// The section marked on the left, by its id.
    pub fn current_section_name(&self) -> &'static str {
        NAV[self.current_section().min(NAV.len() - 1)]
    }

    /// The names on the left.
    fn nav(&self, current: usize, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let rtl = crate::i18n::is_rtl();
        col()
            .id("settings-sections")
            .role(Role::Navigation)
            .aria_label(t!("Settings sections"))
            .test_support()
            .w(space(40.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .gap(space(0.5))
            .p(space(2.))
            .border_color(t.border)
            .when(rtl, |this| this.border_l_1())
            .when(!rtl, |this| this.border_r_1())
            .children(NAV.iter().enumerate().map(|(i, slot)| {
                let chosen = i == current;
                let name = nav_name(slot);
                let slot = *slot;
                Button::new(widgets::id(&["settings-nav", slot]))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(if chosen { t.accent } else { t.accent.opacity(0.) })
                            .foreground(if chosen { t.foreground } else { t.muted_foreground })
                            .hover(t.accent)
                            .active(t.accent),
                    )
                    .selected(chosen)
                    .label(name)
                    .w_full()
                    .justify_start()
                    .text_start()
                    .when(chosen, |this| this.font_weight(FontWeight::MEDIUM))
                    .on_click(cx.listener(move |this, _, _, cx| this.scroll_to(slot, cx)))
            }))
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

        let page = self.page(&t, cx);
        self.order = page.iter().map(|(slot, _)| *slot).collect();
        // A section asked for as Settings opened: now the page knows where the sections are.
        if let Some(section) = self.pending_section.take() {
            self.scroll_to(&section, cx);
        }
        let current = self.current_section();

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
            .child(
                row_start()
                    .flex_1()
                    .min_h_0()
                    .when(wide, |this| this.child(self.nav(current, &t, cx)))
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
                            .px(space(5.))
                            .py(space(5.))
                            .gap(space(6.))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .children(page.into_iter().map(|(_, section)| section))
                            .vertical_scrollbar(&self.scroll),
                    ),
            )
    }
}

/// The backend the app uses: glint-core through the app's handle.
fn app_backend(cx: &App) -> Arc<dyn Backend> {
    Arc::new(CoreBackend::new(cx.global::<core_bridge::Core>().app.clone()))
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
