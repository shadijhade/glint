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
use gpui_kit::{Image, ImageFormat};
use std::sync::{Arc, OnceLock};

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

/// Glint's logo (src/assets/logo.svg, drawn at 128 px, which GPUI rasterizes at twice that: it is shown at under 50, and the full 1024 would hold 4 MB).
const LOGO_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 1024 1024">
  <defs>
    <linearGradient id="back" x1="0" y1="168" x2="0" y2="832" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#f4a62a"/>
      <stop offset="1" stop-color="#e07f06"/>
    </linearGradient>
    <linearGradient id="front" x1="0" y1="344" x2="0" y2="856" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#ffd966"/>
      <stop offset="1" stop-color="#ffb526"/>
    </linearGradient>
    <linearGradient id="spark" x1="584" y1="76" x2="944" y2="476" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#ff8fd0"/>
      <stop offset="0.55" stop-color="#ec4899"/>
      <stop offset="1" stop-color="#c026d3"/>
    </linearGradient>
  </defs>
  <!-- Folder: the back with its tab, then the front panel, as in Windows 11's folder icons. -->
  <path d="M176 168 H366 C398 168 420 180 440 202 L486 252 H848 Q928 252 928 332 V776 Q928 856 848 856 H176 Q96 856 96 776 V248 Q96 168 176 168 Z" fill="url(#back)"/>
  <rect x="96" y="344" width="832" height="512" rx="80" fill="url(#front)"/>
  <rect x="96" y="344" width="832" height="10" rx="5" fill="#fff1b8" opacity="0.9"/>
  <!-- The glint: a four-point sparkle on the corner, and a small one beside it. -->
  <path d="M764 76 C790 215 839 264 976 290 C839 316 790 365 764 504 C738 365 689 316 552 290 C689 264 738 215 764 76 Z" fill="url(#spark)"/>
  <path d="M764 214 C773 263 791 281 840 290 C791 299 773 317 764 366 C755 317 737 299 688 290 C737 281 755 263 764 214 Z" fill="#ffffff" opacity="0.35"/>
  <path d="M586 92 C592 124.5 603.5 136 636 142 C603.5 148 592 159.5 586 192 C580 159.5 568.5 148 536 142 C568.5 136 580 124.5 586 92 Z" fill="#ff8fd0"/>
</svg>"##;

/// The logo as a picture GPUI draws; made once.
fn logo() -> Arc<Image> {
    static LOGO: OnceLock<Arc<Image>> = OnceLock::new();
    LOGO.get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Svg, LOGO_SVG.as_bytes().to_vec())))
        .clone()
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
                    div()
                        .id("about-logo")
                        .role(Role::Image)
                        .aria_label(t!("Glint"))
                        .size(space(11.))
                        .flex_none()
                        .child(img(logo()).size_full()),
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

#[cfg(test)]
mod logo_tests {
    use super::*;

    #[gpui_kit::test]
    fn the_logo_decodes_to_a_square_picture(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            let drawn = logo().to_image_data(cx.svg_renderer()).expect("the logo decodes");
            let size = drawn.size(0);
            assert_eq!((size.width.0, size.height.0), (256, 256));
            // The same picture every time: made once.
            assert!(Arc::ptr_eq(&logo(), &logo()));
        });
    }
}
