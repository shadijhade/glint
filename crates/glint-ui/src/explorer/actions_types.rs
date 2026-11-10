//! The plain types of the menu actions part: what the menus' remaining events ask of glint-core and answer, the
//! folder sizes the list shows, and what the part tells the person. Ported from `SizeRun` in
//! src/lib/explorer.ts, `FolderSizes`, `CloudPin` and `OnDemand` in src/lib/fs.ts and src/lib/ondemand.ts, and
//! from the toasts of `runInSandbox`, `rotatePictures`, `cloudPin`, `setWallpaper`, `forgetRecent`,
//! `exportAsNcdu`, `runScript`, `openScriptsFolder`, `showOnDemand` and `hideOnDemand`.
//!
//! Nothing here touches GPUI or the disk. The wording each notice has is given in its doc, exactly as the
//! TypeScript toasts had it (English, `{x}` marking what goes in): the workspace says it with `t!`. A name is the
//! last part of a path (`base_name`, which keeps the whole path for a drive); `“…”` are the curly quotes the
//! toasts used. `{what}` for several items is `plural(count, "item")` ("3 items").
//!
//! These are not variants of [`OpNotice`](super::OpNotice), [`Notice`](super::Notice) or
//! [`ArchiveNotice`](super::ArchiveNotice) so that the exhaustive matches over those in notices.rs and
//! startup.rs keep compiling. They travel in their own [`ExplorerEvent`](super::ExplorerEvent) variants
//! ([`Action`](super::ExplorerEvent::Action), [`Export`](super::ExplorerEvent::Export),
//! [`Request`](super::ExplorerEvent::Request)).

use super::types::TabId;
use std::collections::HashMap;
use std::sync::Arc;

pub use glint_core::scripts::Script;

// ---- Asking glint-core -------------------------------------------------------------------------------------

/// Which way cloud files are kept (`CloudPin` in fs.ts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinState {
    /// Always keep on this device.
    Keep,
    /// Free up space (online only).
    Free,
    /// Back to automatic: kept until Windows frees up space.
    Auto,
}

impl PinState {
    /// Core's word for it ("keep", "free", "auto").
    pub fn as_str(self) -> &'static str {
        match self {
            PinState::Keep => "keep",
            PinState::Free => "free",
            PinState::Auto => "auto",
        }
    }
}

/// A remote shown in File Explorer on demand: its connection, and its folder there (`OnDemand`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnDemand {
    pub id: String,
    pub folder: String,
}

/// What measuring a folder came to (`FolderSizes` in fs.ts).
#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    /// Everything in the folder, files included, in bytes.
    pub total: u64,
    /// Subfolder name to bytes.
    pub sizes: Vec<(String, u64)>,
    /// Milliseconds it took.
    pub elapsed: f64,
    /// "index" or "pdu", or the archive's format ("zip", "7z") for a folder in one.
    pub engine: String,
}

// ---- Folder sizes ---------------------------------------------------------------------------------------------

/// Subfolder sizes for one folder, from the index or pdu (`SizeRun`). The list's Size column, the grid, the
/// preview card, sorting and grouping by size, and the status bar read it from
/// [`Explorer::size_run`](super::Explorer::size_run).
#[derive(Clone, Debug)]
pub struct SizeRun {
    /// Subfolder name (as the listing spells it) to bytes. Shared, never copied.
    pub sizes: Arc<HashMap<String, u64>>,
    /// Everything in the folder, files included, in bytes.
    pub total: u64,
    /// Milliseconds the measuring took.
    pub elapsed: f64,
    /// Being measured now (the list shows "…" for a folder with no size yet; the button says Stop).
    pub running: bool,
    /// The index or pdu, or the archive's format for a folder in one (its sizes come with its listing).
    pub engine: String,
    /// The tab measuring it, so Stop stops the right run.
    pub lane: TabId,
    /// Changes whenever `sizes` does (not when only `running` does): rows are rebuilt for a new one.
    pub version: u64,
    /// The newest run that touched this entry, to tell a stale answer from the current one.
    pub(super) run: u64,
}

impl SizeRun {
    /// A subfolder's size by its name, if it has been measured.
    pub fn size_of(&self, name: &str) -> Option<u64> {
        self.sizes.get(name).copied()
    }

    /// What the status bar says once measured (`sizes && !sizes.running`).
    pub fn sized(&self) -> Option<SizedLine<'_>> {
        (!self.running).then_some(SizedLine {
            engine: &self.engine,
            total: self.total,
            elapsed: self.elapsed,
        })
    }
}

/// What a finished measuring says of itself.
///
/// - The status bar's line (shown from the xl width, and in the tooltip as ` The {line}.`):
///   `{engine} sized {format_size(total)} in {format_duration(elapsed)}`, e.g. "index sized 1.2 GB in 3 ms".
/// - The popover's stage (`telemetryStages`): label `Folder sizes ({engine})`, `ms` = `elapsed`, detail
///   `format_size(total)`; it is shown only when measured (`!running`).
///
/// The engine word is never translated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SizedLine<'a> {
    pub engine: &'a str,
    pub total: u64,
    pub elapsed: f64,
}

// ---- Things the workspace is asked to do --------------------------------------------------------------------

/// A run in the terminal pane: what a shell is typed to run a script. The terminal makes the line for the shell
/// of its tab with [`TerminalJob::line`], the way `runInTerminal(cwd, (shell) => runLine(shell, …), title)` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalJob {
    /// The folder the terminal tab starts in.
    pub cwd: String,
    /// The tab's title: the script's name.
    pub title: String,
    /// What runs it (a Windows path).
    pub program: String,
    /// Its arguments: the script among them, then the paths chosen.
    pub args: Vec<String>,
}

impl TerminalJob {
    /// What the shell called `shell` ("pwsh", "powershell", "cmd", "bash", "nu"…) is typed to run it, Enter
    /// included (`glint_model::cd::run_line`).
    pub fn line(&self, shell: &str) -> String {
        let args: Vec<&str> = self.args.iter().map(String::as_str).collect();
        glint_model::cd::run_line(shell, &self.program, &args)
    }
}

/// Work for the workspace: what a menu action needs a window or a part the explorer doesn't own for. Carried by
/// [`ExplorerEvent::Request`](super::ExplorerEvent::Request).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open a window of its own on this folder (`open_window`), as Ctrl+N and "Open in new window" do. The
    /// path is always a folder (a file or This PC's gets the home folder).
    NewWindow { path: String },
    /// Run a script in a new terminal tab (`runInTerminal`).
    Terminal(TerminalJob),
    /// The script needs a tool that isn't installed (`installTool(script.missing)`): the tool's id. Settings'
    /// `Tools::install` shows its progress and says `{name} is installed` or `Couldn't install {name}`.
    InstallTool { tool: String },
}

// ---- Telling the person -------------------------------------------------------------------------------------

/// What failed, for the error notices' titles (`toast.error(title, { description: String(e) })`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionFailure {
    /// `Couldn't open the file` (Open with the editor).
    OpenFile,
    /// `Couldn't open the app picker` (Open with…).
    AppPicker,
    /// `Couldn't run it in Windows Sandbox`.
    Sandbox,
    /// `Couldn't open File Explorer` (Show in File Explorer, Show the archive in File Explorer, and the
    /// notices' Open button).
    FileExplorer,
    /// `Couldn't open Windows' optional features` (new wording: the TypeScript's button did not catch it).
    OptionalFeatures,
    /// `Couldn't take it out of Recent`.
    Recent,
    /// `Couldn't set the desktop background`.
    Wallpaper,
    /// `Couldn't free up space` (Free up space).
    FreeUpSpace,
    /// `Couldn't change what's kept on this device` (Always keep on this device, and taking it back).
    CloudKept,
    /// `Couldn't calculate folder sizes`.
    FolderSizes,
    /// `Couldn't open the Scripts folder`.
    ScriptsFolder,
    /// `Couldn't show it in File Explorer` (on demand).
    ShowOnDemand,
    /// `Couldn't take it out of File Explorer` (on demand).
    HideOnDemand,
}

/// What the menu actions tell the person. Carried by [`ExplorerEvent::Action`](super::ExplorerEvent::Action):
/// the workspace turns each into a notification with `t!`, the wording in the docs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionNotice {
    /// Error with `message` as the description, the title of the failure.
    Failed { what: ActionFailure, message: String },
    /// Success: `Opening it in Windows Sandbox`, description `A copy, in a Windows of its own without the
    /// network: closing the sandbox throws away everything it did.`
    SandboxOpening,
    /// Info, 15 s: `Windows Sandbox isn't on`, description `It comes with Windows 11 Pro, Enterprise and
    /// Education: turn on “Windows Sandbox” in Windows' optional features (Windows restarts once).`, with a
    /// button `Optional features` that calls [`Explorer::open_optional_features`](super::Explorer).
    SandboxNotOn,
    /// Success: `“{name}” is now your desktop background`.
    WallpaperSet { path: String },
    /// Error, description `detail` (up to three `{name}: {error}` lines joined by a line break): `Couldn't
    /// rotate {count} picture(s)` (`plural(count, "picture")`).
    RotateFailed { count: usize, detail: String },
    /// Success, 2.5 s: `Took “{name of the path}” out of Recent` for one path, else `Took {plural(count,
    /// "item")} out of Recent`.
    TookOutOfRecent { paths: Vec<String> },
    /// Success: the first letter in capitals of `Keeping {items} on this device` (Keep), `Freeing up space
    /// from {items}` (Free) or `{items} no longer always kept on this device` (Auto). `items` is `“{name}”`
    /// for one path, else `plural(count, "item")`.
    CloudPinned { state: PinState, paths: Vec<String> },
    /// Success: `“{name}” is in File Explorer`, description `Its files download when an app opens them, while
    /// Glint runs. It's read-only there.`, with a button `Open` that calls
    /// [`Explorer::open_in_file_explorer`](super::Explorer::open_in_file_explorer) with `folder`.
    OnDemandShown { name: String, folder: String },
    /// Error: `Too much for “{name}”'s command line`, description `The {format_count(paths)} paths take
    /// {format_count(length)} characters, and a command line holds about {format_count(longest)}. Choose
    /// fewer, or run it on their folder.` (`longest` is 8000.)
    ScriptTooLong {
        name: String,
        paths: usize,
        length: usize,
        longest: usize,
    },
    /// Error (new wording, the TypeScript always had the script in hand): `Couldn't find the script
    /// “{name of the path}”`, description `It may have been moved or deleted.`
    ScriptGone { path: String },
}

/// What the ncdu export tells the person while it runs and when it is over. Carried by
/// [`ExplorerEvent::Export`](super::ExplorerEvent::Export). It follows the archive notices (`staging`): the
/// workspace shows and updates one loading notice keyed by `id`, and drives the taskbar button.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportNotice {
    /// Still running after 400 ms: show a loading notice (no auto-hide) keyed by `id` with a **Stop** button
    /// that calls [`Explorer::stop_archive_work`](super::Explorer::stop_archive_work) with `id`, and set the
    /// taskbar button to indeterminate. The words follow [`ExportNotice::progress`] of `done` and `total`.
    Shown {
        id: u64,
        name: String,
        done: u64,
        total: u64,
    },
    /// How far it has come. Taskbar: normal with `done` of `total` when `total` is known, else indeterminate.
    /// When `shown`, replace the notice with the same id by the new label.
    Progress {
        id: u64,
        name: String,
        done: u64,
        total: u64,
        shown: bool,
    },
    /// Over: take the notice away when `was_shown`, and the taskbar progress when `idle`.
    Ended { id: u64, was_shown: bool, idle: bool },
    /// Error with the message as description: `Couldn't export “{name}”`. Not sent when it was stopped.
    Failed { name: String, message: String },
    /// Success: `Exported “{name}” as “{name of dest}”` with a button `Show` that calls
    /// [`Explorer::reveal_in_app`](super::Explorer::reveal_in_app) with `dest`.
    Done { name: String, dest: String },
}

impl ExportNotice {
    /// How far an export has come, for the loading notice's words (English; the workspace says them with `t!`):
    /// `Exporting “{name}”…` while nothing is counted, `Exporting “{name}”… {percent}%` once there is a total, and
    /// `Exporting “{name}”… {format_count(done)} items` while the folder is still being walked (no total ahead).
    pub fn progress(done: u64, total: u64) -> ExportProgress {
        if total > 0 {
            let percent = u128::from(done) * 100 / u128::from(total);
            ExportProgress::Percent(u8::try_from(percent.min(100)).unwrap_or(100))
        } else if done > 0 {
            ExportProgress::Items(done)
        } else {
            ExportProgress::Starting
        }
    }
}

/// How far an export has come, for its label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportProgress {
    /// Nothing counted yet: `Exporting “{name}”…`.
    Starting,
    /// `Exporting “{name}”… {percent}%`.
    Percent(u8),
    /// `Exporting “{name}”… {count} items`.
    Items(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_export_counts_items_while_it_walks_and_a_percent_when_it_knows_the_total() {
        assert_eq!(ExportNotice::progress(0, 0), ExportProgress::Starting);
        assert_eq!(ExportNotice::progress(1200, 0), ExportProgress::Items(1200));
        assert_eq!(ExportNotice::progress(1, 3), ExportProgress::Percent(33));
        assert_eq!(ExportNotice::progress(9, 3), ExportProgress::Percent(100));
    }

    #[test]
    fn a_script_is_typed_to_each_shell_with_its_arguments_quoted() {
        let job = TerminalJob {
            cwd: "C:\\x".into(),
            title: "tidy".into(),
            program: "C:\\bin\\pwsh.exe".into(),
            args: vec![
                "-File".into(),
                "C:\\Scripts\\tidy.ps1".into(),
                "C:\\It's here\\تقرير.pdf".into(),
            ],
        };
        assert_eq!(
            job.line("pwsh"),
            "& 'C:\\bin\\pwsh.exe' '-File' 'C:\\Scripts\\tidy.ps1' 'C:\\It''s here\\تقرير.pdf'\r"
        );
        assert_eq!(
            job.line("cmd"),
            "\"C:\\bin\\pwsh.exe\" \"-File\" \"C:\\Scripts\\tidy.ps1\" \"C:\\It's here\\تقرير.pdf\"\r"
        );
    }

    #[test]
    fn a_size_run_tells_its_total_only_once_it_is_over() {
        let mut run = SizeRun {
            sizes: Arc::new(HashMap::from([("src".to_string(), 7)])),
            total: 10,
            elapsed: 2.0,
            running: true,
            engine: "pdu".into(),
            lane: TabId(1),
            version: 1,
            run: 1,
        };
        assert_eq!(run.size_of("src"), Some(7));
        assert_eq!(
            run.size_of("SRC"),
            None,
            "names are compared as the listing spells them"
        );
        assert!(run.sized().is_none());
        run.running = false;
        assert_eq!(
            run.sized(),
            Some(SizedLine {
                engine: "pdu",
                total: 10,
                elapsed: 2.0
            })
        );
    }
}
