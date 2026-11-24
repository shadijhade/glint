//! Settings → Overview, the page Settings opens on: how Glint is set up at a glance, and what would make it better.
//!
//! Four tiles, each with what matters most and the one thing to do about it: Instant search (on, with how many
//! files it knows, or Index all drives), Tools (how many are ready, Update all), Clouds (how many are connected) and
//! Theme (System, Light or Dark, chosen right there). A tile's name opens its page. Under them, the suggestions that
//! apply to this PC (`crate::suggestions`), each with its button and a cross that puts it away.

use super::backend::IndexState;
use super::pages::Page;
use super::widgets;
use super::SettingsView;
use crate::kit::prelude::*;
use crate::suggestions::{self, Facts, Room, Suggestion};
use crate::{state, t, theme};
use glint_model::format::format_count;
use glint_model::store::Theme;

/// What a tile shows: its page, a value line, and its button if it has one.
struct Tile {
    page: Page,
    value: SharedString,
    tone: Tone,
    action: Option<AnyElement>,
}

/// How a tile's value reads: fine, wanting something, or plain.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Good,
    Wants,
    Plain,
}

impl SettingsView {
    /// The Overview page's body.
    pub(super) fn overview(&mut self, t: &theme::Tokens, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let facts = self.facts(cx);
        let tiles = vec![
            self.instant_tile(t, cx),
            self.tools_tile(cx),
            self.clouds_tile(cx),
            self.theme_tile(cx),
        ];
        let suggestions = suggestions::current(&facts, cx);
        let _ = window;
        col()
            .id("settings-overview")
            .role(Role::Group)
            .aria_label(t!("Overview"))
            .test_support()
            .w_full()
            .gap(space(5.))
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap(space(3.))
                    .children(tiles.into_iter().map(|tile| self.tile(tile, t, cx))),
            )
            .child(self.suggestions_card(suggestions, t, cx))
            .into_any_element()
    }

    /// The facts the suggestions are worked out from, as Settings knows them now.
    fn facts(&self, cx: &App) -> Facts {
        let tools = self.tools.read(cx);
        Facts {
            index: self.instant.read(cx).info().map(|info| info.state),
            tool_updates: tools.loaded().then(|| tools.counts().updates),
            drives: self.drives.clone(),
            rclone: tools.tool("rclone").map(|tool| tool.origin.is_some()),
            clouds: self.clouds.read(cx).remotes().map(<[_]>::len),
            ..Facts::from_settings(cx)
        }
    }

    fn instant_tile(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> Tile {
        let instant = self.instant.read(cx);
        let working = instant.working();
        let (value, tone, offer) = match instant.info() {
            None => (t!("Looking…"), Tone::Plain, false),
            Some(info) => match info.state {
                IndexState::On => {
                    let files: u64 = info.volumes.iter().map(|v| v.files).sum();
                    let drives = info.volumes.len();
                    let value = if files > 0 {
                        t!(
                            "On · {files} files on {drives}",
                            files = format_count(files as usize),
                            drives = glint_model::format::plural(drives, "drive")
                        )
                    } else {
                        t!("On")
                    };
                    (value, Tone::Good, false)
                }
                IndexState::Off => (t!("Off: names are searched drive by drive"), Tone::Wants, true),
                IndexState::Stopped => (t!("Stopped"), Tone::Wants, true),
                IndexState::NoAccess | IndexState::Untrusted => (t!("Needs your attention"), Tone::Wants, false),
                IndexState::Unsupported => (t!("Not available on this PC"), Tone::Plain, false),
            },
        };
        let _ = t;
        let action = offer.then(|| {
            widgets::primary_button("overview-index", t!("Index all drives"), Some(IconName::Zap))
                .loading(working)
                .disabled(working)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.instant.update(cx, |instant, cx| instant.turn_on(window, cx));
                }))
                .into_any_element()
        });
        Tile {
            page: Page::Instant,
            value,
            tone,
            action,
        }
    }

    fn tools_tile(&self, cx: &mut Context<Self>) -> Tile {
        let tools = self.tools.read(cx);
        let counts = tools.counts();
        let (value, tone) = if !tools.loaded() {
            (t!("Looking…"), Tone::Plain)
        } else if counts.updates > 0 {
            (
                t!("{ready} ready · {updates} with an update", ready = counts.ready, updates = counts.updates),
                Tone::Wants,
            )
        } else {
            (t!("{ready} ready, all up to date", ready = counts.ready), Tone::Good)
        };
        let action = (counts.updates > 0).then(|| {
            widgets::primary_button("overview-update-tools", t!("Update all"), Some(IconName::Download))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.tools.update(cx, |tools, cx| tools.update_all(window, cx));
                }))
                .into_any_element()
        });
        Tile {
            page: Page::Tools,
            value,
            tone,
            action,
        }
    }

    fn clouds_tile(&self, cx: &mut Context<Self>) -> Tile {
        let rclone = self.tools.read(cx).tool("rclone").map(|tool| tool.origin.is_some());
        let (value, tone) = match (rclone, self.clouds.read(cx).remotes().map(<[_]>::len)) {
            (Some(false), _) => (t!("rclone isn't installed yet"), Tone::Plain),
            (_, Some(0)) => (t!("None connected"), Tone::Plain),
            (_, Some(n)) => (t!("{n} connected", n = n), Tone::Good),
            _ => (t!("Looking…"), Tone::Plain),
        };
        let action = Some(
            widgets::button("overview-clouds", t!("Connect"), Some(IconName::Cloud))
                .on_click(cx.listener(|this, _, window, cx| this.show_page(Page::Clouds, window, cx)))
                .into_any_element(),
        );
        Tile {
            page: Page::Clouds,
            value,
            tone,
            action,
        }
    }

    fn theme_tile(&self, cx: &mut Context<Self>) -> Tile {
        let current = state::settings(cx).theme;
        let choice = widgets::segmented(
            "overview-theme",
            t!("Theme"),
            &current,
            vec![
                (Theme::System, t!("System")),
                (Theme::Light, t!("Light")),
                (Theme::Dark, t!("Dark")),
            ],
            |value, _, cx| state::update(cx, |s| s.theme = value),
            cx,
        );
        Tile {
            page: Page::Appearance,
            value: match current {
                Theme::System => t!("Follows Windows"),
                Theme::Light => t!("Light, whatever Windows does"),
                Theme::Dark => t!("Dark, whatever Windows does"),
            },
            tone: Tone::Plain,
            action: Some(choice),
        }
    }

    /// One tile: its page's sign and name (which open the page), the value line, and its button.
    fn tile(&self, tile: Tile, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let page = tile.page;
        let dot = match tile.tone {
            Tone::Good => Some(t.success),
            Tone::Wants => Some(t.warning),
            Tone::Plain => None,
        };
        let (over, ring) = (t.accent, t.ring);
        col()
            .id(widgets::id(&["settings-tile", page.slot()]))
            .role(Role::Group)
            .aria_label(page.name())
            .aria_description(tile.value.clone())
            .test_support()
            .min_w_0()
            .gap(space(2.5))
            .p(space(4.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.card)
            .child(
                row_start()
                    .id(widgets::id(&["settings-tile-open", page.slot()]))
                    .role(Role::Link)
                    .aria_label(t!("Open {page}", page = page.name()))
                    .test_support()
                    .tab_index(0)
                    .gap(space(2.))
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(gpui_kit::transparent_black())
                    .cursor_pointer()
                    .hover(move |style| style.bg(over))
                    .focus_visible(move |style| style.border_color(ring))
                    .on_click(cx.listener(move |this, _, window, cx| this.show_page(page, window, cx)))
                    .child(icon(page.icon()).size_4().flex_none().text_color(t.primary))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(page.name()),
                    )
                    .child(icon(IconName::ChevronRight).size_3p5().flex_none().text_color(t.muted_foreground)),
            )
            .child(
                row_start()
                    .min_w_0()
                    .gap(space(2.))
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .children(dot.map(|color| div().size(space(1.5)).flex_none().rounded_full().bg(color)))
                    .child(div().min_w_0().child(tile.value)),
            )
            .children(tile.action.map(|action| row_start().pt(space(0.5)).child(action)))
            .into_any_element()
    }

    /// The suggestions card: each with its sign, words, button and cross; or a line that all is well.
    fn suggestions_card(&self, list: Vec<Suggestion>, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = if list.is_empty() {
            vec![row_start()
                .id("settings-suggestions-none")
                .role(Role::Status)
                .aria_label(t!("Nothing to suggest: Glint is set up well here."))
                .test_support()
                .gap(space(2.5))
                .px(space(4.))
                .py(space(3.))
                .text_color(t.muted_foreground)
                .child(icon(IconName::CircleCheck).size_4().flex_none().text_color(t.success))
                .child(t!("Nothing to suggest: Glint is set up well here."))
                .into_any_element()]
        } else {
            list.into_iter()
                .map(|suggestion| {
                    let id = suggestion.kind.id();
                    let kind = suggestion.kind;
                    let doing = suggestion.clone();
                    row_start()
                        .id(widgets::id(&["settings-suggestion", id]))
                        .role(Role::Group)
                        .aria_label(suggestion.title.clone())
                        .aria_description(suggestion.body.clone())
                        .test_support()
                        .gap(space(3.))
                        .px(space(4.))
                        .py(space(3.))
                        .child(
                            div()
                                .size(space(8.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(t.primary.opacity(0.12))
                                .child(icon(kind.icon()).size_4().text_color(t.primary)),
                        )
                        .child(
                            col()
                                .flex_1()
                                .min_w_0()
                                .child(div().font_weight(FontWeight::MEDIUM).child(suggestion.title.clone()))
                                .child(
                                    div()
                                        .text_size(theme::size::XS)
                                        .text_color(t.muted_foreground)
                                        .child(suggestion.body.clone()),
                                ),
                        )
                        .child(
                            widgets::primary_button(&format!("suggestion-{id}"), suggestion.button.clone(), None)
                                .on_click(cx.listener(move |this, _, window, cx| this.act_on(&doing, window, cx))),
                        )
                        .child(
                            Button::new(widgets::id(&["settings-suggestion-never", id]))
                                .ghost()
                                .xsmall()
                                .icon(IconName::X)
                                .tooltip(t!("Don't suggest this again"))
                                .accessibility_label(t!("Don't suggest “{what}” again", what = suggestion.title.clone()))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    suggestions::dismiss(cx, kind);
                                    cx.notify();
                                })),
                        )
                        .into_any_element()
                })
                .collect()
        };
        widgets::section("suggestions", t!("Suggestions for you"), None, rows, t)
    }

    /// A suggestion's button, from inside Settings: the ones about Settings' own pages stay in Settings.
    fn act_on(&mut self, suggestion: &Suggestion, window: &mut Window, cx: &mut Context<Self>) {
        match suggestion.kind {
            suggestions::Kind::ToolUpdates => self.tools.update(cx, |tools, cx| tools.update_all(window, cx)),
            suggestions::Kind::ConnectCloud => {
                window.close_dialog(cx);
                cx.emit(super::SettingsEvent::ConnectCloud);
            }
            suggestions::Kind::IndexAllDrives => self.instant.update(cx, |instant, cx| instant.turn_on(window, cx)),
            suggestions::Kind::FullDrive => {
                window.close_dialog(cx);
                suggestions::act(suggestion, window, cx);
            }
            _ => suggestions::act(suggestion, window, cx),
        }
        cx.notify();
    }

    /// Asks core for the fixed drives' room once, for the full-drive suggestion (none without core, as in tests).
    pub(super) fn load_drives(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !cx.has_global::<crate::core_bridge::Core>() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            // F27: XYplorer's tag.dat is asked here too, and kept for `Facts::from_settings`.
            let xyplorer = cx.update(|_, cx| suggestions::ask_xyplorer(cx)).ok();
            let xyplorer = match xyplorer {
                Some(asked) => asked.await,
                None => None,
            };
            let drives = crate::core_bridge::run(glint_core::fsx::list_drives())
                .await
                .ok()
                .and_then(Result::ok)
                .map(|drives| {
                    drives
                        .into_iter()
                        .filter(|d| d.kind == "fixed")
                        .map(|d| Room {
                            root: d.path,
                            free: d.free,
                            total: d.total,
                        })
                        .collect::<Vec<_>>()
                });
            this.update(cx, |this, cx| {
                suggestions::know_xyplorer(cx, xyplorer);
                this.drives = drives;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
