//! Windows' open and save pickers, with file-type filters, for every view (the TypeScript used Tauri's dialog
//! plugin: `open` and `save` with `filters` and `defaultPath`).
//!
//! # Using it
//!
//! ```ignore
//! use crate::platform::pickers::{self, Filter, Options};
//!
//! let owner = pickers::owner_of(window);                  // while you still have the Window
//! let options = Options::new()
//!     .title(t!("Export results"))
//!     .start(folder)                                      // optional: the folder the picker opens in
//!     .filter(Filter::new(t!("CSV"), &["csv"]))           // each filter is a named kind of file
//!     .filter(Filter::new(t!("JSON"), &["json"]))
//!     .owner(owner);
//! cx.spawn(async move |this, cx| {
//!     if let Ok(Some(path)) = pickers::save_file(options, "search-results.csv").await { /* ... */ }
//! }).detach();
//! ```
//!
//! - [`open_file`] and [`open_files`] pick existing files (one, or several); [`open_folder`] and [`open_folders`]
//!   pick folders; [`save_file`] picks where to write, asking before it replaces a file.
//! - Each returns a future of the choice: `Ok(None)` (or an empty list) when the person cancels, `Err` with
//!   Windows' reason when the picker couldn't be shown. Paths are file system paths.
//! - The picker is modal and pumps messages, so it never runs inside a GPUI callback: the work is posted to the UI
//!   thread's message pump ([`core_bridge::post_to_ui`]) and the answer comes back through a channel the future
//!   waits on. The UI thread never waits.
//! - With several filters the first is selected when the picker opens; a save adds the first filter's extension
//!   to a name typed without one. No filter means every file. [`Filter::all_files`] adds "All files" to a list.

use crate::core_bridge;
use std::future::Future;
use std::pin::Pin;
use windows::core::{Interface as _, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog, IShellItem,
    SHCreateItemFromParsingName, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT,
    FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};

/// A future of the choice. It holds only the channel the posted picker answers on, so it is `Send`: callers
/// whose futures must be (Settings' backend) take it as it is.
pub type Fut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

// ---- What to ask for -------------------------------------------------------------------------------------------

/// A named kind of file in the picker's type list: `Filter::new("ncdu export", &["ncdu", "json"])` is "ncdu export
/// (*.ncdu;*.json)". Extensions come without a dot; a leading `.` or `*.` is dropped, and an empty one ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    /// What the list says (translate it with `t!`).
    pub name: String,
    pub extensions: Vec<String>,
}

impl Filter {
    /// A kind of file by its name and extensions.
    pub fn new(name: impl Into<String>, extensions: &[&str]) -> Self {
        Filter {
            name: name.into(),
            extensions: extensions
                .iter()
                .map(|e| e.trim().trim_start_matches("*.").trim_start_matches('.').to_string())
                .filter(|e| !e.is_empty())
                .collect(),
        }
    }

    /// Every file ("All files", `*.*`).
    pub fn all_files() -> Self {
        Filter {
            name: crate::t!("All files").to_string(),
            extensions: vec!["*".to_string()],
        }
    }

    /// The pattern Windows takes: `*.ncdu;*.json`. A filter with no extension matches every file.
    pub fn spec(&self) -> String {
        if self.extensions.is_empty() {
            return "*.*".to_string();
        }
        self.extensions
            .iter()
            .map(|e| format!("*.{e}"))
            .collect::<Vec<_>>()
            .join(";")
    }
}

/// What a picker is asked, besides what it picks. Build it with [`Options::new`] and the methods after it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// The picker's title (Windows' own when none).
    pub title: Option<String>,
    /// The folder it opens in. One that is gone leaves the picker where Windows puts it.
    pub start: Option<String>,
    /// The kinds of file offered, the first selected. None means every file. Folders take none.
    pub filters: Vec<Filter>,
    /// The window it is owned by ([`owner_of`]); 0 for none.
    pub owner: isize,
}

impl Options {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn start(mut self, folder: impl Into<String>) -> Self {
        self.start = Some(folder.into());
        self
    }

    pub fn filter(mut self, filter: Filter) -> Self {
        self.filters.push(filter);
        self
    }

    pub fn owner(mut self, owner: isize) -> Self {
        self.owner = owner;
        self
    }
}

/// The Win32 handle of `window` for [`Options::owner`]; 0 when it has none yet. Take it while you still have the
/// `Window`: the future may run when you don't.
pub fn owner_of(window: &gpui_kit::Window) -> isize {
    core_bridge::hwnd_of(window).map_or(0, |hwnd| hwnd.0 as isize)
}

/// What kind of choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    OpenFiles { multiple: bool },
    Folders { multiple: bool },
    Save { file_name: String },
}

/// A picker as asked: what [`pick`] shows. (Public so a test can see what a view asked for.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub kind: Kind,
    pub options: Options,
}

/// One existing file, or `None` when cancelled.
pub fn open_file(options: Options) -> Fut<Result<Option<String>, String>> {
    first(pick(Request {
        kind: Kind::OpenFiles { multiple: false },
        options,
    }))
}

/// Several existing files (one or more), or none when cancelled.
pub fn open_files(options: Options) -> Fut<Result<Vec<String>, String>> {
    pick(Request {
        kind: Kind::OpenFiles { multiple: true },
        options,
    })
}

/// One folder, or `None` when cancelled.
pub fn open_folder(options: Options) -> Fut<Result<Option<String>, String>> {
    first(pick(Request {
        kind: Kind::Folders { multiple: false },
        options,
    }))
}

/// Several folders, or none when cancelled.
pub fn open_folders(options: Options) -> Fut<Result<Vec<String>, String>> {
    pick(Request {
        kind: Kind::Folders { multiple: true },
        options,
    })
}

/// Where to write a file, suggesting `file_name`; the person is asked before an existing file is replaced.
/// `None` when cancelled.
pub fn save_file(options: Options, file_name: &str) -> Fut<Result<Option<String>, String>> {
    first(pick(Request {
        kind: Kind::Save {
            file_name: file_name.to_string(),
        },
        options,
    }))
}

fn first(choice: Fut<Result<Vec<String>, String>>) -> Fut<Result<Option<String>, String>> {
    Box::pin(async move { Ok(choice.await?.into_iter().next()) })
}

// ---- Showing it -------------------------------------------------------------------------------------------------

/// Shows the picker and gives what was chosen (empty when cancelled). The picker runs on the UI thread's message
/// pump, not here.
pub fn pick(request: Request) -> Fut<Result<Vec<String>, String>> {
    #[cfg(test)]
    {
        if let Some(answer) = fake::answer(&request) {
            return Box::pin(async move { answer });
        }
    }
    let (answer, heard) = async_channel::bounded(1);
    core_bridge::post_to_ui(move || {
        let _ = answer.try_send(show(&request));
    });
    Box::pin(async move {
        heard
            .recv()
            .await
            .map_err(|_| "the picker was closed without an answer".to_string())?
    })
}

/// Shows the picker and waits. UI thread, from the message pump only.
fn show(request: &Request) -> Result<Vec<String>, String> {
    // SAFETY: COM calls on the UI thread, which GPUI has already put in a single-threaded apartment (a second
    // CoInitializeEx with the same mode is harmless; a different mode is reported and ignored). The owner is a
    // window handle that may be stale, which the dialog tolerates by showing unowned. Every string handed to
    // COM outlives the call, and the ones COM gives back are freed once.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        run(request).map_err(|error| error.message())
    }
}

/// # Safety
/// UI thread; see [`show`].
unsafe fn run(request: &Request) -> windows::core::Result<Vec<String>> {
    let Request { kind, options } = request;
    // The open dialog is kept as it is when several results are wanted: only it has `GetResults`.
    let open: Option<IFileOpenDialog> = match kind {
        Kind::Save { .. } => None,
        _ => Some(CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?),
    };
    let dialog: IFileDialog = match &open {
        Some(open) => open.cast()?,
        None => CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?.cast()?,
    };
    if let Some(title) = options.title.as_deref().filter(|t| !t.is_empty()) {
        dialog.SetTitle(&HSTRING::from(title))?;
    }
    let mut flags = dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST;
    match kind {
        Kind::OpenFiles { multiple } => {
            flags |= FOS_FILEMUSTEXIST;
            if *multiple {
                flags |= FOS_ALLOWMULTISELECT;
            }
        }
        Kind::Folders { multiple } => {
            flags |= FOS_PICKFOLDERS;
            if *multiple {
                flags |= FOS_ALLOWMULTISELECT;
            }
        }
        Kind::Save { .. } => flags |= FOS_OVERWRITEPROMPT,
    }
    dialog.SetOptions(flags)?;

    // The strings must outlive the call that reads them.
    let filters: Vec<(HSTRING, HSTRING)> = match kind {
        Kind::Folders { .. } => Vec::new(),
        _ => options
            .filters
            .iter()
            .map(|f| (HSTRING::from(f.name.as_str()), HSTRING::from(f.spec())))
            .collect(),
    };
    if !filters.is_empty() {
        let specs: Vec<COMDLG_FILTERSPEC> = filters
            .iter()
            .map(|(name, spec)| COMDLG_FILTERSPEC {
                pszName: PCWSTR(name.as_ptr()),
                pszSpec: PCWSTR(spec.as_ptr()),
            })
            .collect();
        dialog.SetFileTypes(&specs)?;
        dialog.SetFileTypeIndex(1)?;
        if let (Kind::Save { .. }, Some(extension)) = (kind, default_extension(&options.filters)) {
            dialog.SetDefaultExtension(&HSTRING::from(extension))?;
        }
    }
    if let Kind::Save { file_name } = kind {
        dialog.SetFileName(&HSTRING::from(file_name.as_str()))?;
    }
    if let Some(folder) = options.start.as_deref().filter(|s| !s.trim().is_empty()) {
        // A folder that's gone just leaves the dialog where Windows would put it.
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(folder), None) {
            let _ = dialog.SetFolder(&item);
        }
    }

    match dialog.Show(Some(HWND(options.owner as *mut _))) {
        Ok(()) => {}
        Err(error) if error.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }
    let mut chosen = Vec::new();
    match (&open, kind) {
        (Some(open), Kind::OpenFiles { multiple: true } | Kind::Folders { multiple: true }) => {
            let items = open.GetResults()?;
            for at in 0..items.GetCount()? {
                chosen.extend(path_of(&items.GetItemAt(at)?));
            }
        }
        _ => chosen.extend(path_of(&dialog.GetResult()?)),
    }
    Ok(chosen)
}

/// An item's file system path, freeing what COM allocated for it.
///
/// # Safety
/// UI thread; see [`show`].
unsafe fn path_of(item: &IShellItem) -> Option<String> {
    let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    let path = name.to_string().ok();
    CoTaskMemFree(Some(name.0 as *const _));
    path
}

/// The extension a name typed without one gets: the first filter's first (not "everything").
fn default_extension(filters: &[Filter]) -> Option<&str> {
    filters
        .first()?
        .extensions
        .first()
        .map(String::as_str)
        .filter(|e| *e != "*")
}

// ---- Tests: a fake picker ----------------------------------------------------------------------------------------

/// What views' tests use instead of Windows' dialog: [`answer_with`] queues the answers the next pickers get (in
/// order, on this thread), [`asked`] says what they were asked. A picker with no queued answer is the real one,
/// which a headless test must not reach.
#[cfg(test)]
pub mod fake {
    use super::Request;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    thread_local! {
        static ANSWERS: RefCell<VecDeque<Result<Vec<String>, String>>> = const { RefCell::new(VecDeque::new()) };
        static ASKED: RefCell<Vec<Request>> = const { RefCell::new(Vec::new()) };
    }

    /// The answers of the next pickers: `Ok(vec![])` is a cancel, `Ok(vec![path])` a choice, `Err` a failure.
    pub fn answer_with(answers: Vec<Result<Vec<String>, String>>) {
        ANSWERS.with(|a| *a.borrow_mut() = answers.into());
        ASKED.with(|a| a.borrow_mut().clear());
    }

    /// Every picker asked since [`answer_with`].
    pub fn asked() -> Vec<Request> {
        ASKED.with(|a| a.borrow().clone())
    }

    pub(super) fn answer(request: &Request) -> Option<Result<Vec<String>, String>> {
        let answer = ANSWERS.with(|a| a.borrow_mut().pop_front())?;
        ASKED.with(|a| a.borrow_mut().push(request.clone()));
        Some(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block<T>(future: Fut<T>) -> T {
        // The fake answers at once, so the future is ready at its first poll.
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = future;
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => value,
            std::task::Poll::Pending => panic!("the fake picker answers at once"),
        }
    }

    #[test]
    fn a_filter_is_the_patterns_windows_takes_and_cleans_its_extensions() {
        assert_eq!(Filter::new("ncdu export", &["ncdu", "json"]).spec(), "*.ncdu;*.json");
        assert_eq!(
            Filter::new("Pictures", &[".jpg", "*.png", " gif ", "", "."]).spec(),
            "*.jpg;*.png;*.gif"
        );
        assert_eq!(Filter::new("Everything", &[]).spec(), "*.*");
        assert_eq!(Filter::new("تقارير", &["csv"]).name, "تقارير");
        assert_eq!(Filter::all_files().spec(), "*.*");
    }

    #[test]
    fn a_save_adds_the_first_filters_first_extension_and_never_everything() {
        let csv = Filter::new("CSV", &["csv"]);
        let json = Filter::new("JSON", &["json"]);
        assert_eq!(default_extension(&[csv.clone(), json.clone()]), Some("csv"));
        assert_eq!(default_extension(&[json, csv]), Some("json"));
        assert_eq!(default_extension(&[Filter::all_files()]), None);
        assert_eq!(default_extension(&[]), None);
    }

    #[test]
    fn the_options_build_up_in_the_order_they_are_given() {
        let options = Options::new()
            .title("Export")
            .start("C:\\مجلد")
            .filter(Filter::new("CSV", &["csv"]))
            .filter(Filter::new("JSON", &["json"]))
            .owner(42);
        assert_eq!(options.title.as_deref(), Some("Export"));
        assert_eq!(options.start.as_deref(), Some("C:\\مجلد"));
        assert_eq!(options.filters.len(), 2);
        assert_eq!(options.owner, 42);
        assert_eq!(Options::new(), Options::default());
    }

    #[test]
    fn each_kind_of_picker_asks_what_it_says_and_gives_the_first_choice() {
        fake::answer_with(vec![
            Ok(vec!["C:\\a\\تقرير.pdf".into()]),
            Ok(vec!["C:\\a\\1.txt".into(), "C:\\a\\2.txt".into()]),
            Ok(vec![]),
            Ok(vec!["C:\\out\\ملف.csv".into()]),
            Err("no window".into()),
        ]);
        let options = Options::new().filter(Filter::new("PDF", &["pdf"]));
        assert_eq!(
            block(open_file(options.clone())),
            Ok(Some("C:\\a\\تقرير.pdf".to_string()))
        );
        assert_eq!(
            block(open_files(options.clone())),
            Ok(vec!["C:\\a\\1.txt".to_string(), "C:\\a\\2.txt".to_string()])
        );
        assert_eq!(block(open_folder(Options::new())), Ok(None), "cancelled");
        assert_eq!(
            block(save_file(Options::new().start("C:\\out"), "ملف.csv")),
            Ok(Some("C:\\out\\ملف.csv".to_string()))
        );
        assert_eq!(block(open_folders(Options::new())), Err("no window".to_string()));

        let asked = fake::asked();
        assert_eq!(asked.len(), 5);
        assert_eq!(asked[0].kind, Kind::OpenFiles { multiple: false });
        assert_eq!(asked[0].options, options);
        assert_eq!(asked[1].kind, Kind::OpenFiles { multiple: true });
        assert_eq!(asked[2].kind, Kind::Folders { multiple: false });
        assert_eq!(
            asked[3].kind,
            Kind::Save {
                file_name: "ملف.csv".into()
            }
        );
        assert_eq!(asked[3].options.start.as_deref(), Some("C:\\out"));
        assert_eq!(asked[4].kind, Kind::Folders { multiple: true });
    }
}
