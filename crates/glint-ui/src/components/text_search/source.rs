//! What text search asks of the PC: glint-core's searches, the files they read and write, and the pickers.
//!
//! The views never call glint-core themselves. They hold a [`Source`], so their tests give them a fake
//! (the way `dialogs::drive_health` takes one) and nothing in a test touches ripgrep, the disk or a dialog.
//! [`CoreSource`] is the real one. Every call returns a future that is driven on core's runtime through
//! `core_bridge::run`, so the UI thread never waits.
//!
//! **How a search streams.** `start` returns once the search is under way, with the folder as it is
//! searched. The matches then arrive as core's `search-event`s (`{ id, type, lines }`), which
//! [`super::model::Search`] hears through `core_bridge::subscribe` and parses off the UI thread.

use crate::components::settings::from_picker;
use crate::components::settings::{Backend as SettingsBackend, CoreBackend as SettingsCoreBackend};
use crate::core_bridge;
use crate::platform::pickers::{self, Filter, Options};
use glint_core::host::{AppHandle, Manager as _, Window};
use glint_model::store::SearchOptions;
use std::future::Future;
use std::pin::Pin;

/// A question to the PC, answered later.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// Which of core's searches answers (`searchBatched` chose among these).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// ripgrep over the files (`run_ripgrep_batched`).
    Ripgrep,
    /// The content index, for plain words in a folder it covers (`run_content_search`).
    Index,
    /// Documents, and Meaning (`run_doc_search`).
    Documents,
    /// A code pattern, by ast-grep (`run_ast_grep`).
    Code,
}

/// A search to run. Core builds the tool's arguments from these; nothing here is passed through as a flag.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// Names the search in its `search-event`s.
    pub id: String,
    pub query: String,
    pub path: String,
    pub options: SearchOptions,
    /// A code search's rewrite, shown on each match.
    pub rewrite: Option<String>,
    pub exclusions: Vec<String>,
    /// Skipping nothing: hidden files, what .gitignore files list and the exclusions too.
    pub everything: bool,
    /// Stop after this many matches (None: all).
    pub max_results: Option<usize>,
}

/// Everything text search asks of the PC. See the module notes.
pub trait Source: 'static {
    /// Whether the content index covers `path` (and is on).
    fn covers(&self, path: &str) -> Fut<bool>;
    /// Starts a search; its matches arrive as `search-event`s. Resolves to the folder as searched
    /// (absolute, with `%VARS%` and `~` expanded). A search that is replaced by a newer one, or stopped
    /// while it starts, resolves to `Err("cancelled")`.
    fn start(&self, engine: Engine, request: Request) -> Fut<String>;
    /// Stops the search that runs (Stop). Never waits on the UI thread.
    fn cancel(&self);
    /// A code search's rewrite made in its files; how many matches it rewrote.
    fn rewrite(&self, request: Request) -> Fut<u64>;
    /// A file's text.
    fn read(&self, path: &str) -> Fut<String>;
    /// Writes a file's text.
    fn write(&self, path: &str, content: String) -> Fut<()>;
    /// Lines `from` to `to` (1-based, both included) of a text file, joined by `\n`.
    fn chunk(&self, path: &str, from: usize, to: usize) -> Fut<String>;
    /// A document's lines `from` to `to` as Documents search read them, each with where it is.
    fn doc_lines(&self, path: &str, from: usize, to: usize) -> Fut<Vec<(String, String)>>;
    /// Opens a file with its app, or (with `editor`) in that program.
    fn open(&self, path: &str, editor: Option<&str>) -> Fut<()>;
    /// Opens File Explorer with the file selected.
    fn show_in_file_explorer(&self, path: &str);
    /// A folder the person picks (`None`: cancelled). `owner` is the window's handle.
    fn pick_folder(&self, title: String, start: Option<String>, owner: isize) -> Fut<Option<String>>;
    /// Where to write the exported results (`None`: cancelled), suggesting `file_name`.
    fn pick_export(&self, file_name: String, owner: isize) -> Fut<Option<String>>;
}

/// glint-core's searches.
pub struct CoreSource {
    app: AppHandle,
    settings: SettingsCoreBackend,
}

impl CoreSource {
    /// The source over the handle `core_bridge::init` made (`cx.global::<Core>().app.clone()`).
    pub fn new(app: AppHandle) -> Self {
        CoreSource {
            settings: SettingsCoreBackend::new(app.clone()),
            app,
        }
    }

    /// The window shell calls and search events belong to: the main window, else any.
    fn window(&self) -> Result<Window, String> {
        self.app
            .get_webview_window(crate::workspace::MAIN)
            .or_else(|| self.app.webview_windows().into_values().next())
            .ok_or_else(|| crate::t!("Glint's window isn't open").to_string())
    }
}

/// What ripgrep's search is asked as: the JSON core's `TextSearch` reads (its fields are private to core).
pub(super) fn ripgrep_json(request: &Request) -> serde_json::Value {
    serde_json::json!({
        "query": request.query,
        "path": request.path,
        "caseSensitive": request.options.case_sensitive,
        "wholeWord": request.options.whole_word,
        "regex": request.options.regex,
        "exclusions": request.exclusions,
        "everything": request.everything,
    })
}

/// Runs `future` on core's runtime and flattens the join error into the call's own.
fn on_core<T: Send + 'static>(future: impl Future<Output = Result<T, String>> + Send + 'static) -> Fut<T> {
    let joined = core_bridge::run(future);
    Box::pin(async move { joined.await.and_then(|inner| inner) })
}

impl Source for CoreSource {
    fn covers(&self, path: &str) -> Fut<bool> {
        let path = path.to_string();
        on_core(async move { Ok(glint_core::content::content_covers(path)) })
    }

    fn start(&self, engine: Engine, request: Request) -> Fut<String> {
        let (app, window) = (self.app.clone(), self.window());
        on_core(async move {
            let window = window?;
            let state = app
                .try_state::<glint_core::AppState>()
                .ok_or_else(|| "Glint's search isn't ready yet".to_string())?;
            match engine {
                Engine::Ripgrep => {
                    // The request's fields are private to core: it builds its own arguments from the JSON.
                    let text = serde_json::from_value(ripgrep_json(&request)).map_err(|e| e.to_string())?;
                    glint_core::run_ripgrep_batched(window, text, request.id, request.max_results, state).await
                }
                Engine::Documents => {
                    let docs = glint_core::doctext::DocSearch {
                        query: request.query,
                        path: request.path,
                        case_sensitive: request.options.case_sensitive,
                        whole_word: request.options.whole_word,
                        regex: request.options.regex,
                        exclusions: request.exclusions,
                        everything: request.everything,
                        ocr: request.options.ocr,
                        meaning: request.options.meaning,
                    };
                    glint_core::run_doc_search(window, docs, request.id, request.max_results, state).await
                }
                Engine::Code => {
                    let code = glint_core::codesearch::CodeSearch {
                        query: request.query,
                        path: request.path,
                        rewrite: request.rewrite.filter(|r| !r.is_empty()),
                        exclusions: request.exclusions,
                        everything: request.everything,
                    };
                    glint_core::run_ast_grep(window, code, request.id, request.max_results, state).await
                }
                Engine::Index => {
                    let words = glint_core::content::ContentSearch {
                        query: request.query,
                        path: request.path,
                    };
                    glint_core::content::run_content_search(window, words, request.id, request.max_results).await
                }
            }
        })
    }

    fn cancel(&self) {
        let app = self.app.clone();
        // Ending a search kills its process: not on the UI thread.
        glint_core::host::async_runtime::spawn_blocking(move || {
            if let Some(state) = app.try_state::<glint_core::AppState>() {
                glint_core::cancel_search(state);
            }
        });
    }

    fn rewrite(&self, request: Request) -> Fut<u64> {
        let code = glint_core::codesearch::CodeSearch {
            query: request.query,
            path: request.path,
            rewrite: request.rewrite,
            exclusions: request.exclusions,
            everything: request.everything,
        };
        on_core(glint_core::ast_grep_apply(code))
    }

    fn read(&self, path: &str) -> Fut<String> {
        on_core(glint_core::read_file_content(path.to_string()))
    }

    fn write(&self, path: &str, content: String) -> Fut<()> {
        let written = glint_core::write_file_content(path.to_string(), content, None);
        on_core(async move { written.await.map(|_| ()) })
    }

    fn chunk(&self, path: &str, from: usize, to: usize) -> Fut<String> {
        on_core(glint_core::read_file_chunk(path.to_string(), from, to))
    }

    fn doc_lines(&self, path: &str, from: usize, to: usize) -> Fut<Vec<(String, String)>> {
        on_core(glint_core::doc_lines(path.to_string(), from, to))
    }

    fn open(&self, path: &str, editor: Option<&str>) -> Fut<()> {
        let path = path.to_string();
        if let Some(editor) = editor.map(str::to_string) {
            // The editor from Settings, run with the file as its argument (`openPath(path, editorPath)`).
            return on_core(async move {
                std::process::Command::new(&editor)
                    .arg(&path)
                    .spawn()
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
        }
        let window = self.window();
        on_core(async move {
            glint_core::shell::open_item(window?, path).await?;
            Ok(())
        })
    }

    fn show_in_file_explorer(&self, path: &str) {
        let path = path.to_string();
        // Explorer is a separate process; starting it waits for nothing, but still not on the UI thread.
        std::thread::spawn(move || {
            let _ = std::process::Command::new("explorer.exe")
                .arg(format!("/select,{path}"))
                .spawn();
        });
    }

    fn pick_folder(&self, title: String, start: Option<String>, owner: isize) -> Fut<Option<String>> {
        self.settings.pick_folder(title, start, owner)
    }

    fn pick_export(&self, file_name: String, owner: isize) -> Fut<Option<String>> {
        export_picker(file_name, owner)
    }
}

/// The save picker for the results: CSV and JSON, the one matching the file name first (it decides the
/// extension a name typed without one gets).
fn export_picker(file_name: String, owner: isize) -> Fut<Option<String>> {
    let csv = Filter::new(crate::t!("CSV"), &["csv"]);
    let json = Filter::new(crate::t!("JSON"), &["json"]);
    let (first, second) = if file_name.to_lowercase().ends_with(".json") {
        (json, csv)
    } else {
        (csv, json)
    };
    let options = Options::new().owner(owner).filter(first).filter(second);
    from_picker(pickers::save_file(options, &file_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::pickers::fake;

    fn block<T>(mut future: Fut<T>) -> Result<T, String> {
        use std::task::{Context, Poll, Waker};
        let mut cx = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("the fake picker answers at once"),
        }
    }

    #[test]
    fn the_export_picker_offers_csv_then_json_and_returns_the_choice() {
        let _guard = crate::testing::lock();
        fake::answer_with(vec![Ok(vec!["D:/r.csv".into()]), Ok(vec![]), Err("no window".into())]);
        assert_eq!(
            block(export_picker("search-results.csv".into(), 7)),
            Ok(Some("D:/r.csv".to_string()))
        );
        assert_eq!(block(export_picker("search-results.csv".into(), 7)), Ok(None));
        assert_eq!(block(export_picker("a.json".into(), 0)), Err("no window".to_string()));
        let asked = fake::asked();
        let names = |ix: usize| -> Vec<String> {
            asked[ix]
                .options
                .filters
                .iter()
                .map(|f| f.extensions[0].clone())
                .collect()
        };
        assert_eq!(names(0), ["csv", "json"]);
        assert_eq!(asked[0].options.owner, 7);
        // A .json file name puts the JSON filter first.
        assert_eq!(names(2), ["json", "csv"]);
    }
}
