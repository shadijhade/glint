//! Where Quick Search's answers come from: glint-core in the app, a fake in tests.
//!
//! Ported from the calls QuickSearch.tsx made (`findFiles`, `cancelFind`, `listDrives`, `openItem`,
//! `startDrag`, `quick_dialog`, `quick_jump`, `zoxide_folders`). The view asks a [`Source`] and never calls
//! core itself, so tests inject a fake and nothing touches the PC.
//!
//! **Searching.** [`Source::find`] starts core's `find_files` and hands back a stream of [`Found`]: who is
//! answering, the best [`LIMIT`](super::search::LIMIT) results so far (ranked), an error, and the end. Core's
//! channel delivers JSON and byte buffers from its own search thread; they are decoded and ranked there
//! ([`CoreSource`]'s channel callback), so the UI thread only ever receives ready results.

use super::search::{Collector, ASKED, LANE};
use crate::core_bridge;
use glint_core::host::ipc::Channel;
use glint_core::host::Manager as _;
use glint_model::entries::FileEntry;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

/// A future that needn't be `Send`: the view awaits it on the UI thread.
pub type Fut<T> = Pin<Box<dyn Future<Output = T>>>;

/// What a search is told besides its words (the settings it follows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Include hidden and system files (Settings → Show hidden).
    pub hidden: bool,
    /// Names or globs to skip (Settings → Find exclusions).
    pub exclude: Vec<String>,
}

/// One thing a running search says.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    /// Who answers: "index", "fd", or "index + fd".
    Started { engine: String },
    /// The best results so far, best first. Each replaces the last.
    Results(Vec<FileEntry>),
    /// The search failed (a bad filter, fd could not start…).
    Error(String),
    /// The search ended. `count` is all it found, not just what is shown; `fuzzy` says these are close matches.
    Finished {
        count: usize,
        elapsed: f64,
        limited: bool,
        fuzzy: bool,
    },
}

/// What Quick Search asks of the rest of Glint.
pub trait Source {
    /// The roots to search: every fixed and removable drive (`searchable`). Empty if Windows can't say.
    fn drives(&self) -> Fut<Vec<String>>;

    /// Starts a search of `roots` for `query`. The stream ends when the search does. An `Err` is a query core
    /// refuses (a bad filter), to show as the status. A newer `find` ends the older one.
    fn find(
        &self,
        roots: Vec<String>,
        query: String,
        options: Options,
    ) -> Fut<Result<async_channel::Receiver<Found>, String>>;

    /// Ends the search in flight (`cancelFind`), if there is one.
    fn cancel(&self);

    /// zoxide's folders with their scores, empty when it isn't installed.
    fn zoxide(&self) -> Fut<Vec<(String, f64)>>;

    /// Opens a file with its app, a program, a folder in File Explorer's way: `Some(folder)` when the item was a
    /// shortcut to a folder, for Glint to open (`openItem`).
    fn open(&self, path: String) -> Fut<Result<Option<String>, String>>;

    /// Whether Quick Search was summoned over an Open or Save As dialog (`quick_dialog`).
    fn over_dialog(&self) -> bool;

    /// Puts that dialog in `folder`; false when there was none or it has gone (`quick_jump`).
    fn jump(&self, folder: String) -> Fut<bool>;

    /// Drags `paths` out into another app (Explorer, a mail…) once the pointer has moved far enough. Resolves
    /// when the drag ends.
    fn drag(&self, cx: &gpui_kit::App, paths: Vec<String>) -> Fut<Result<String, String>>;
}

// ---- glint-core ---------------------------------------------------------------------------------------------

/// What core's search said, as a [`Found`]. `collector` ranks the results so far.
fn found_from(event: glint_core::find::FindEvent, collector: &Mutex<Collector>) -> Found {
    use glint_core::find::FindEvent;
    match event {
        FindEvent::Started { engine } => Found::Started {
            engine: engine.to_string(),
        },
        FindEvent::Found(entries) => {
            let rows: Vec<FileEntry> = entries
                .into_iter()
                .map(|entry| core_bridge::file_entry(entry, "", None))
                .collect();
            let mut collector = collector.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            Found::Results(collector.add(rows))
        }
        FindEvent::Error { message } => Found::Error(message),
        FindEvent::Finished {
            count,
            elapsed,
            limited,
            fuzzy,
        } => Found::Finished {
            count,
            elapsed,
            limited,
            fuzzy,
        },
    }
}

/// glint-core, as a [`Source`].
pub struct CoreSource {
    app: glint_core::host::AppHandle,
}

impl CoreSource {
    /// A source for `app` (core_bridge's `Core::app`).
    pub fn new(app: glint_core::host::AppHandle) -> Self {
        Self { app }
    }
}

impl Source for CoreSource {
    fn drives(&self) -> Fut<Vec<String>> {
        let running = core_bridge::run(glint_core::fsx::list_drives());
        Box::pin(async move {
            let drives = running.await.and_then(|listed| listed).unwrap_or_default();
            drives
                .into_iter()
                .filter(|drive| drive.kind == "fixed" || drive.kind == "removable")
                .map(|drive| drive.path)
                .collect()
        })
    }

    fn find(
        &self,
        roots: Vec<String>,
        query: String,
        options: Options,
    ) -> Fut<Result<async_channel::Receiver<Found>, String>> {
        let (sender, receiver) = async_channel::unbounded::<Found>();
        let collector = Mutex::new(Collector::new(&query));
        // Runs on core's search thread, so making rows and ranking never touch the UI thread.
        let channel = Channel::new(move |event| {
            sender
                .try_send(found_from(event, &collector))
                .map_err(|_| glint_core::host::Error::from("Quick Search has moved on"))
        });
        let app = self.app.clone();
        let running = core_bridge::run(async move {
            let state = app
                .try_state::<glint_core::AppState>()
                .ok_or_else(|| "Glint's search isn't ready yet".to_string())?;
            let options = glint_core::find::FindOptions {
                hidden: options.hidden,
                respect_ignore: false,
                exclude: options.exclude,
                max_results: Some(ASKED),
                fuzzy: true,
                rank: true,
            };
            glint_core::find::find_files(state, roots, query, options, channel, LANE.to_string()).await
        });
        Box::pin(async move { running.await.and_then(|started| started).map(|()| receiver) })
    }

    fn cancel(&self) {
        let app = self.app.clone();
        // Ending a search kills fd's process: not on the UI thread.
        glint_core::host::async_runtime::spawn_blocking(move || {
            if let Some(state) = app.try_state::<glint_core::AppState>() {
                glint_core::find::cancel_find(state, LANE.to_string());
            }
        });
    }

    fn zoxide(&self) -> Fut<Vec<(String, f64)>> {
        let running = core_bridge::run(glint_core::zoxide::zoxide_folders());
        Box::pin(async move {
            running
                .await
                .and_then(|listed| listed)
                .ok()
                .flatten()
                .unwrap_or_default()
        })
    }

    fn open(&self, path: String) -> Fut<Result<Option<String>, String>> {
        let Some(window) = self.app.get_webview_window(super::window::LABEL) else {
            return Box::pin(async { Err("Quick Search has no window to open it from".to_string()) });
        };
        let running = core_bridge::run(glint_core::shell::open_item(window, path));
        Box::pin(async move { running.await.and_then(|opened| opened) })
    }

    fn over_dialog(&self) -> bool {
        glint_core::dialogs::quick_dialog()
    }

    fn jump(&self, folder: String) -> Fut<bool> {
        // It talks to another app's window, which may be slow to answer: a blocking thread.
        let running = core_bridge::run(async move {
            glint_core::fsx::blocking(move || glint_core::dialogs::quick_jump(folder)).await
        });
        Box::pin(async move { running.await.and_then(|jumped| jumped).unwrap_or(false) })
    }

    fn drag(&self, cx: &gpui_kit::App, paths: Vec<String>) -> Fut<Result<String, String>> {
        Box::pin(crate::platform::dragdrop::drag_out(
            cx,
            super::window::LABEL,
            paths,
            false,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_core::find::FindEvent;

    #[test]
    fn core_events_become_found_items() {
        let collector = Mutex::new(Collector::new("a"));
        assert_eq!(
            found_from(FindEvent::Started { engine: "index + fd" }, &collector),
            Found::Started {
                engine: "index + fd".into()
            }
        );
        assert_eq!(
            found_from(
                FindEvent::Error {
                    message: "no such folder".into()
                },
                &collector
            ),
            Found::Error("no such folder".into())
        );
        assert_eq!(
            found_from(
                FindEvent::Finished {
                    count: 7,
                    elapsed: 12.5,
                    limited: true,
                    fuzzy: false
                },
                &collector
            ),
            Found::Finished {
                count: 7,
                elapsed: 12.5,
                limited: true,
                fuzzy: false
            }
        );
    }

    #[test]
    fn results_are_ranked_as_they_arrive() {
        use glint_core::fsx::Entry;
        let entry = |path: &str| Entry {
            name: path.rsplit('\\').next().unwrap_or(path).to_string(),
            path: Some(path.to_string()),
            dir: false,
            size: 10,
            modified: 0,
            attrs: 0,
        };
        let collector = Mutex::new(Collector::new("note"));
        let Found::Results(results) = found_from(FindEvent::Found(vec![entry(r"C:\z\my-note.txt")]), &collector) else {
            panic!("results give results");
        };
        assert_eq!(results.len(), 1);
        // The second batch brings a better name: both are held, the better first.
        let Found::Results(results) = found_from(FindEvent::Found(vec![entry(r"C:\z\note.txt")]), &collector) else {
            panic!("results give results");
        };
        let paths: Vec<_> = results.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, [r"C:\z\note.txt", r"C:\z\my-note.txt"]);
    }
}
