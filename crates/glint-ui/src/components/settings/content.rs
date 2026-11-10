//! Settings → Search inside files instantly: an index of what the files below the chosen folders hold, which
//! Text mode answers from in milliseconds (`ContentIndexSection` of SettingsView.tsx).

use super::backend::{Backend, Known};
use super::widgets::{self, bad};
use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::content::Status;
use glint_model::format::{format_count, format_size};
use std::sync::Arc;
use std::time::Duration;

/// How often the index is asked how it is doing (the TypeScript's 1.5 s).
const POLL: Duration = Duration::from_millis(1500);

/// The content index card. Draws nothing until the index has answered once.
pub struct ContentIndex {
    backend: Arc<dyn Backend>,
    status: Option<Status>,
    _poll: Task<()>,
}

impl ContentIndex {
    /// Asks the index how it is, and keeps asking while Settings is open.
    pub fn new(backend: Arc<dyn Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let asking = backend.clone();
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            let answer = asking.content_status().await;
            let alive = this
                .update(cx, |this, cx| {
                    if let Ok(status) = answer {
                        this.status = Some(status);
                        cx.notify();
                    }
                })
                .is_ok();
            if !alive {
                break;
            }
            cx.background_executor().timer(POLL).await;
        });
        Self {
            backend,
            status: None,
            _poll: poll,
        }
    }

    /// The last answer.
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    /// Changes what is indexed (`configure`): the answer replaces the status, a failure is said.
    fn configure(
        &mut self,
        enabled: bool,
        folders: Vec<String>,
        pictures: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let asked = self.backend.content_configure(enabled, folders, pictures);
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| {
                match answer {
                    Ok(status) => this.status = Some(status),
                    Err(reason) => bad(window, cx, t!("Couldn't change content indexing"), reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Turns indexing on: for the folders already chosen, or else Documents and Desktop (`start`).
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(status) = &self.status else { return };
        let folders = if status.folders.is_empty() {
            [Known::Documents, Known::Desktop]
                .into_iter()
                .filter_map(|which| self.backend.known_folder(which))
                .collect()
        } else {
            status.folders.clone()
        };
        self.configure(true, folders, None, window, cx);
    }

    /// Adds a folder the person picks (`add`).
    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let picked = self
            .backend
            .pick_folder(t!("A folder to search inside instantly").to_string(), None, owner);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Some(folder)) = picked.await else { return };
            this.update_in(cx, |this, window, cx| {
                if let Some(status) = &this.status {
                    if !status.folders.contains(&folder) {
                        let mut folders = status.folders.clone();
                        folders.push(folder);
                        this.configure(true, folders, None, window, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn remove(&mut self, folder: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(status) = &self.status else { return };
        let folders = status.folders.iter().filter(|f| *f != folder).cloned().collect();
        self.configure(true, folders, None, window, cx);
    }
}

impl Render for ContentIndex {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let Some(status) = self.status.clone() else {
            return div().into_any_element();
        };
        let hint = if status.enabled {
            if status.indexing {
                t!(
                    "Reading the folders: {seen} files looked at, {indexed} indexed…",
                    seen = format_count(usize::try_from(status.seen).unwrap_or(usize::MAX)),
                    indexed = format_count(usize::try_from(status.indexed).unwrap_or(usize::MAX))
                )
            } else {
                t!(
                    "{files} files in the index ({size}). Words typed in Text mode in these folders are found in milliseconds; regular expressions, case and whole words still read the files.",
                    files = format_count(usize::try_from(status.files).unwrap_or(usize::MAX)),
                    size = format_size(status.bytes)
                )
            }
        } else {
            t!("Text, code and documents (PDF, Word, Excel, PowerPoint, EPUB) below the folders you choose, kept current as they change. The index stays on this PC, in Glint's data folder.")
        };
        let enabled = status.enabled;
        let mut rows: Vec<AnyElement> = vec![widgets::switch_row(
            "content-index",
            t!("Index what's in my files"),
            Some(hint),
            enabled,
            false,
            widgets::toggler(cx, move |this, on, window, cx| {
                if on {
                    this.start(window, cx);
                } else {
                    let folders = this.status.as_ref().map(|s| s.folders.clone()).unwrap_or_default();
                    this.configure(false, folders, None, window, cx);
                }
            }),
            &t,
        )
        .into_any_element()];
        if enabled {
            rows.push(
                widgets::switch_row(
                    "content-pictures",
                    t!("The text in pictures too"),
                    Some(t!("Screenshots, scans and photos of whiteboards are found by the words in them, read by Windows' own OCR in the background (in the languages Windows has for it).")),
                    status.pictures,
                    false,
                    widgets::toggler(cx, |this, on, window, cx| {
                        let folders = this.status.as_ref().map(|s| s.folders.clone()).unwrap_or_default();
                        this.configure(true, folders, Some(on), window, cx);
                    }),
                    &t,
                )
                .into_any_element(),
            );
            rows.push(
                col()
                    .gap(space(1.))
                    .px(space(4.))
                    .pb(space(3.))
                    .children(status.folders.iter().enumerate().map(|(i, folder)| {
                        let remove_folder = folder.clone();
                        widgets::path_line(
                            "content-folder",
                            i,
                            folder.clone(),
                            quiet_remove(&format!("content-folder-{i}"), folder).on_click(
                                cx.listener(move |this, _, window, cx| this.remove(&remove_folder, window, cx)),
                            ),
                        )
                    }))
                    .child(
                        row_start().child(
                            widgets::button("content-add", t!("Add a folder"), Some(IconName::Plus))
                                .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                        ),
                    )
                    .into_any_element(),
            );
        }
        widgets::section(
            "search-inside-files-instantly",
            t!("Search inside files instantly"),
            None,
            rows,
            &t,
        )
        .into_any_element()
    }
}

/// A Remove button named for the folder it takes out.
fn quiet_remove(slot: &str, folder: &str) -> Button {
    widgets::quiet_button(slot, t!("Remove")).accessibility_label(t!("Remove {name}", name = folder))
}
