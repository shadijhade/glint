//! Settings → About: Glint's name and version, updates, the shortcuts sheet, diagnostics, the settings file and
//! Reset (the About section, `UpdateRow` and `copyDiagnostics` of SettingsView.tsx).
//!
//! **Updates** are `crate::updater`'s model (feature `updater` until the coordinator integrates it): the row
//! shows "Check for updates" with how the last check went, or "Version X is available" with What's new and
//! Install (a percent while it downloads), as `UpdateRow` did. It reads the shared model lazily, so a Settings
//! opened before the model exists (tests) draws the check and waits.

use super::backend::Backend;
use super::transfer::Transfer;
use super::widgets::{self, bad, good};
use crate::kit::prelude::*;
use crate::{actions, state, t, theme};
use glint_model::store::Settings;
use std::sync::Arc;

use crate::updater::{self, Install, Policy, Status, Updater};

/// Glint's page, for the GitHub button (`REPO_URL`).
pub const REPO_URL: &str = "https://github.com/shadijhade/glint";

/// The About card.
pub struct About {
    backend: Arc<dyn Backend>,
    transfer: Entity<Transfer>,
    version: SharedString,
    /// The Microsoft Store's copy: the Store keeps it up to date, so there is nothing to check.
    store: bool,
    /// The update model, once the app has made one.
    updater: Option<Entity<Updater>>,
    /// Redraws this card when the update model changes.
    _watching: Option<Subscription>,
}

impl About {
    /// The card; `store` is whether this is the Microsoft Store's copy.
    pub fn new(backend: Arc<dyn Backend>, store: bool, cx: &mut Context<Self>) -> Self {
        let transfer = cx.new(|_| Transfer::new(backend.clone()));
        // Only the updater feature changes it.
        let mut about = Self {
            backend,
            transfer,
            version: env!("CARGO_PKG_VERSION").into(),
            store,
            updater: None,
            _watching: None,
        };
        about.attach_updater(cx);
        about
    }

    /// Starts watching the shared update model, once it exists.
    fn attach_updater(&mut self, cx: &mut Context<Self>) {
        if self.updater.is_some() {
            return;
        }
        if let Some(updater) = updater::try_shared(cx) {
            self._watching = Some(cx.observe(&updater, |_, _, cx| cx.notify()));
            self.updater = Some(updater);
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

impl About {
    /// The updates row (`UpdateRow`): what the update model knows, with its buttons.
    fn update_row(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let hint_only = |hint: SharedString| widgets::setting_row(t!("Updates"), Some(hint), Vec::new(), t);
        let Some(model) = self.updater.clone() else {
            // No model yet (a window made without the app's start-up): drawn, and waiting.
            if self.store {
                return hint_only(t!("The Microsoft Store keeps Glint up to date"));
            }
            return widgets::setting_row(
                t!("Updates"),
                None,
                vec![
                    widgets::button("about-update", t!("Check for updates"), Some(IconName::RefreshCw))
                        .disabled(true)
                        .into_any_element(),
                ],
                t,
            );
        };
        let (policy, status, install, available, error) = {
            let model = model.read(cx);
            (
                model.policy(),
                model.status().clone(),
                model.install_state().clone(),
                model.available().cloned(),
                model.install_error().map(str::to_string),
            )
        };
        if self.store || policy == Policy::Store {
            return hint_only(t!("The Microsoft Store keeps Glint up to date"));
        }
        if policy == Policy::Off {
            return hint_only(t!("Updates are off in a debug build"));
        }
        if let Some(available) = available {
            let installable = available.installer.is_some() && policy == Policy::SelfUpdating;
            let base = if installable {
                t!("Installing it restarts the app")
            } else {
                t!("Download the installer from GitHub")
            };
            // A failed install is said plainly; otherwise the notes, when the release has words for them.
            let hint: SharedString = match (&error, available.notes.as_deref().and_then(updater::notes_text)) {
                (Some(reason), _) => t!("Couldn't install: {reason}", reason = reason.clone()),
                (None, Some(notes)) => format!("{base}. {notes}").into(),
                (None, None) => base,
            };
            let working = install.working();
            let label = match &install {
                Install::Downloading { .. } => match install.percent() {
                    Some(percent) => t!("Downloading {percent}%", percent = percent),
                    None => t!("Downloading…"),
                },
                Install::Verifying => t!("Checking…"),
                Install::Launching => t!("Installing…"),
                Install::Ready | Install::None if installable => t!("Install"),
                Install::Ready | Install::None => t!("Download"),
            };
            let url = available.url.clone();
            let install_model = model.clone();
            return widgets::setting_row(
                t!("Version {version} is available", version = available.version.clone()),
                Some(hint),
                vec![
                    widgets::quiet_button("about-whats-new", t!("What's new"))
                        .on_click(move |_, _, cx| cx.open_url(&url))
                        .into_any_element(),
                    widgets::primary_button("about-install", label, Some(IconName::Sparkles))
                        .loading(working)
                        .disabled(working)
                        .on_click(move |_, _, cx| install_model.update(cx, |updater, cx| updater.install(cx)))
                        .into_any_element(),
                ],
                t,
            );
        }
        // Offline, a request fails with words like "error sending request": say that plainly.
        let hint: Option<SharedString> = match &status {
            Status::Idle | Status::Store | Status::Off => None,
            Status::Checking => Some(t!("Checking…")),
            Status::Latest => Some(t!("You have the latest version")),
            Status::Failed(reason) if updater::looks_offline(reason) => {
                Some(t!("Couldn't reach GitHub to check. Are you online?"))
            }
            Status::Failed(reason) => Some(t!("Couldn't check: {reason}", reason = reason.clone())),
        };
        let checking = status == Status::Checking;
        widgets::setting_row(
            t!("Updates"),
            hint,
            vec![
                widgets::button("about-update", t!("Check for updates"), Some(IconName::RefreshCw))
                    .loading(checking)
                    .disabled(checking)
                    .on_click(move |_, _, cx| model.update(cx, |updater, cx| updater.check(true, cx)))
                    .into_any_element(),
            ],
            t,
        )
    }
}

impl Render for About {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.attach_updater(cx);
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
        rows.push(self.update_row(&t, cx));
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
