//! Settings → About: Glint's name and version, updates, the shortcuts sheet, diagnostics, the settings file and
//! Reset (the About section, `UpdateRow` and `copyDiagnostics` of SettingsView.tsx).

use super::backend::Backend;
use super::transfer::Transfer;
use super::widgets::{self, bad, good};
use crate::kit::prelude::*;
use crate::{actions, state, t, theme};
use glint_model::store::Settings;
use std::sync::Arc;

/// Glint's page, for the GitHub button (`REPO_URL`).
pub const REPO_URL: &str = "https://github.com/shadijhade/glint";

/// The About card.
pub struct About {
    backend: Arc<dyn Backend>,
    transfer: Entity<Transfer>,
    version: SharedString,
    /// The Microsoft Store's copy: the Store keeps it up to date, so there is nothing to check.
    store: bool,
}

impl About {
    /// The card; `store` is whether this is the Microsoft Store's copy.
    pub fn new(backend: Arc<dyn Backend>, store: bool, cx: &mut Context<Self>) -> Self {
        let transfer = cx.new(|_| Transfer::new(backend.clone()));
        Self {
            backend,
            transfer,
            version: env!("CARGO_PKG_VERSION").into(),
            store,
        }
    }

    /// The settings file's row, for tests to drive.
    pub fn transfer(&self) -> &Entity<Transfer> {
        &self.transfer
    }

    /// Copies versions, tools and instant search's state for a bug report (`copyDiagnostics`).
    fn copy_diagnostics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.backend.diagnostics();
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |_, window, cx| match answer {
                Ok(text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    good(
                        window,
                        cx,
                        t!("Diagnostics copied"),
                        Some(t!("Paste them into your bug report.")),
                    );
                }
                Err(reason) => bad(window, cx, t!("Couldn't copy diagnostics"), reason),
            })
            .ok();
        })
        .detach();
    }

    /// Closes Settings, which "sits above everything", and shows the shortcuts sheet.
    fn show_shortcuts(window: &mut Window, cx: &mut App) {
        window.close_dialog(cx);
        window.defer(cx, |window, cx| {
            window.dispatch_action(Box::new(actions::ShowShortcuts), cx);
        });
    }
}

impl Render for About {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let pink = theme::tailwind("pink-500").unwrap_or(t.primary);
        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            row_start()
                .gap(space(3.))
                .px(space(4.))
                .py(space(4.))
                .child(
                    // PORT ME(coordinator): src/assets/logo.svg as an embedded asset; until then Glint's pink tile.
                    div()
                        .size(space(11.))
                        .flex_none()
                        .rounded(theme::size::RADIUS_LG)
                        .bg(t.primary)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(t.primary_foreground)
                        .child(icon(IconName::Sparkles).size_5()),
                )
                .child(
                    col()
                        .flex_1()
                        .min_w_0()
                        .align_start()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child("Glint"))
                        .child(
                            row_start()
                                .gap(space(1.))
                                .flex_wrap()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(t!("Version {version} ·", version = self.version))
                                .child(t!("Made with"))
                                .child(icon(IconName::Heart).size_3().text_color(pink))
                                .child(t!("by Shadi Saadi")),
                        )
                        .child(
                            div()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(t!("Powered by ripgrep, fd, pdu and an NTFS index")),
                        ),
                )
                .child(
                    widgets::button("about-github", t!("GitHub"), Some(IconName::ExternalLink))
                        .on_click(|_, _, cx| cx.open_url(REPO_URL)),
                )
                .into_any_element(),
        );
        // PORT ME(updater): the updater (lib/updater.ts: check, "Version X is available" with What's new and
        // Install, and the progress) isn't built yet, so the row is drawn as the TypeScript draws it and the
        // button waits. The Store's copy has no button: the Store updates it.
        rows.push(if self.store {
            widgets::setting_row(
                t!("Updates"),
                Some(t!("The Microsoft Store keeps Glint up to date")),
                Vec::new(),
                &t,
            )
            .into_any_element()
        } else {
            widgets::setting_row(
                t!("Updates"),
                None,
                vec![
                    widgets::button("about-update", t!("Check for updates"), Some(IconName::RefreshCw))
                        .disabled(true)
                        .tooltip(t!("Updating comes with its part of the port."))
                        .into_any_element(),
                ],
                &t,
            )
            .into_any_element()
        });
        rows.push(
            widgets::setting_row(
                t!("Keyboard shortcuts"),
                Some(t!("Every one on a sheet: F1 or Ctrl+/ anywhere")),
                vec![widgets::button("about-shortcuts", t!("Show"), Some(IconName::Keyboard))
                    .on_click(|_, window, cx| Self::show_shortcuts(window, cx))
                    .into_any_element()],
                &t,
            )
            .into_any_element(),
        );
        rows.push(
            widgets::setting_row(
                t!("Diagnostics"),
                Some(t!(
                    "Versions, tools and instant search's state, to paste into a bug report"
                )),
                vec![
                    widgets::button("about-diagnostics", t!("Copy"), Some(IconName::ClipboardCopy))
                        .accessibility_label(t!("Copy diagnostics"))
                        .on_click(cx.listener(|this, _, window, cx| this.copy_diagnostics(window, cx)))
                        .into_any_element(),
                ],
                &t,
            )
            .into_any_element(),
        );
        rows.push(self.transfer.clone().into_any_element());
        rows.push(
            widgets::setting_row(
                t!("Reset settings"),
                Some(t!("Restore every option above to its default")),
                vec![widgets::button("about-reset", t!("Reset"), Some(IconName::RotateCcw))
                    .on_click(|_, window, cx| {
                        widgets::confirm(
                            window,
                            cx,
                            t!("Reset settings"),
                            t!("Reset all settings to their defaults? Your search history is kept."),
                            t!("Reset"),
                            |_, cx| state::update(cx, |settings| *settings = Settings::default()),
                        )
                    })
                    .into_any_element()],
                &t,
            )
            .into_any_element(),
        );
        widgets::section("about", t!("About"), None, rows, &t)
    }
}
