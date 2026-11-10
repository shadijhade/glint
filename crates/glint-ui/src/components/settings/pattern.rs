//! An add-a-pattern box with removable chips (`PatternEditor` of SettingsView.tsx), for the folders and patterns
//! searches skip. It edits one of the two lists in the settings and keeps only the text being typed.

use super::widgets::{self, chip};
use crate::kit::prelude::*;
use crate::{state, t, theme};
use glint_model::store::Settings;

/// Which list of the settings an editor edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    /// Folders and patterns name searches skip (`findExclusions`).
    FindExclusions,
    /// Glob patterns text searches skip (`exclusions`).
    TextExclusions,
}

impl Which {
    fn list(self, settings: &Settings) -> &Vec<String> {
        match self {
            Which::FindExclusions => &settings.find_exclusions,
            Which::TextExclusions => &settings.exclusions,
        }
    }

    fn list_mut(self, settings: &mut Settings) -> &mut Vec<String> {
        match self {
            Which::FindExclusions => &mut settings.find_exclusions,
            Which::TextExclusions => &mut settings.exclusions,
        }
    }

    fn slot(self) -> &'static str {
        match self {
            Which::FindExclusions => "find-exclusions",
            Which::TextExclusions => "exclusions",
        }
    }
}

/// The patterns a list holds with `pattern` (trimmed) added, or `None` when there's nothing new to add:
/// blank, or already there.
pub fn with_pattern(list: &[String], pattern: &str) -> Option<Vec<String>> {
    let pattern = pattern.trim();
    if pattern.is_empty() || list.iter().any(|p| p == pattern) {
        return None;
    }
    let mut next = list.to_vec();
    next.push(pattern.to_string());
    Some(next)
}

/// The text box, the Add button and the chips of one list.
pub struct PatternEditor {
    which: Which,
    input: Entity<InputState>,
    _on_input: Subscription,
}

impl PatternEditor {
    /// An editor of `which`, its box showing `placeholder`.
    pub fn new(which: Which, placeholder: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let on_input = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.add(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        Self {
            which,
            input,
            _on_input: on_input,
        }
    }

    /// The text typed so far.
    pub fn typed(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// The text box, for a test to type into.
    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    /// Adds what's typed, if it's new, and empties the box (`add`).
    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.typed(cx);
        let which = self.which;
        if let Some(next) = with_pattern(which.list(state::settings(cx)), &typed) {
            state::update(cx, |settings| *which.list_mut(settings) = next);
        }
        self.input.update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }
}

impl Render for PatternEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let which = self.which;
        let slot = which.slot();
        let list = which.list(state::settings(cx)).clone();
        let can_add = !self.typed(cx).trim().is_empty();
        let mono = theme::mono_font(cx);
        col()
            .gap(space(3.))
            .text_size(theme::size::SM)
            .child(
                row_start()
                    .gap(space(2.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.input).aria_label(match which {
                                Which::FindExclusions => t!("Add a folder or pattern"),
                                Which::TextExclusions => t!("Add a pattern"),
                            })),
                    )
                    .child(
                        widgets::button(&format!("{slot}-add"), t!("Add"), Some(IconName::Plus))
                            .disabled(!can_add)
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    ),
            )
            .child(
                row_start()
                    .id(widgets::id(&["settings-chips", slot]))
                    .role(Role::List)
                    .aria_label(match which {
                        Which::FindExclusions => t!("Skipped by name searches"),
                        Which::TextExclusions => t!("Skipped by text searches"),
                    })
                    .test_support()
                    .flex_wrap()
                    .gap(space(1.5))
                    .max_h(space(44.))
                    .overflow_y_scroll()
                    .when(list.is_empty(), |this| {
                        this.child(
                            div()
                                .id(widgets::id(&["settings-nothing", slot]))
                                .role(Role::Status)
                                .aria_label(t!("Nothing is excluded."))
                                .test_support()
                                .text_color(t.muted_foreground)
                                .child(t!("Nothing is excluded.")),
                        )
                    })
                    .children(list.into_iter().enumerate().map(|(i, pattern)| {
                        let remove_label = t!("Stop excluding {name}", name = pattern);
                        let removed = pattern.clone();
                        chip(
                            &format!("{slot}-{i}"),
                            pattern.into(),
                            remove_label,
                            mono.clone(),
                            move |_, cx| {
                                let removed = removed.clone();
                                state::update(cx, |settings| which.list_mut(settings).retain(|p| *p != removed));
                            },
                            &t,
                        )
                    })),
            )
    }
}
