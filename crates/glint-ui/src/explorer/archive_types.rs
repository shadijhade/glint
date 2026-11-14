//! The plain types of the archives part: what staging, compressing and extracting ask of glint-core and answer,
//! and what the part tells the person while it works. Ported from `Staged`, `Progress`, `CompressFormat` and
//! `archiveStage`/`archiveCompress`/`archiveExtractHere` in src/lib/fs.ts and from the toasts of `staging`,
//! `compressEntries`, `extractHere`, `dragFromArchive`, `openFromArchive`, `mountImage` and `openVersionFile`
//! in src/lib/explorer.ts.
//!
//! Nothing here touches GPUI or the disk. The wording each [`ArchiveNotice`] has is given in its doc, exactly as
//! the TypeScript toasts had it (English, `{x}` marking what goes in): the workspace says it with `t!`. `{what}`
//! is a [`Subject`]: `“{name}”` for one item, else `plural(count, "item")` ("3 items"). A name is the last part
//! of a path.

// ---- Asking glint-core -------------------------------------------------------------------------------------

/// What Compress to writes (`CompressFormat` in fs.ts): a zip (Deflate), a 7z (LZMA2) or a tar.zst (Zstandard).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompressFormat {
    #[default]
    Zip,
    SevenZ,
    TarZst,
}

impl CompressFormat {
    /// The extension the archive gets, and core's word for the format: "zip", "7z" or "tar.zst".
    pub fn ext(self) -> &'static str {
        match self {
            CompressFormat::Zip => "zip",
            CompressFormat::SevenZ => "7z",
            CompressFormat::TarZst => "tar.zst",
        }
    }

    /// The format whose extension this is ("zip", "7z", "tar.zst"), as the password dialog and the menus name it.
    #[cfg(test)]
    pub fn from_ext(ext: &str) -> Option<Self> {
        match ext {
            "zip" => Some(CompressFormat::Zip),
            "7z" => Some(CompressFormat::SevenZ),
            "tar.zst" => Some(CompressFormat::TarZst),
            _ => None,
        }
    }

    /// Whether a password can protect the archive (a tar can't).
    #[cfg(test)]
    pub fn can_have_password(self) -> bool {
        self != CompressFormat::TarZst
    }
}

/// Why files are extracted (`purpose` of `archive_stage`): `Copy` for a copy made here (deleted after, with
/// [`Source::unstage`](super::Source::unstage)), `Out` for the clipboard or a drag (Glint deletes those once
/// they are off the clipboard, itself), `Open` for a remote's file downloaded to be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagePurpose {
    Copy,
    Out,
    Open,
}

impl StagePurpose {
    /// Core's word for it.
    pub fn as_str(self) -> &'static str {
        match self {
            StagePurpose::Copy => "copy",
            StagePurpose::Out => "out",
            StagePurpose::Open => "open",
        }
    }
}

/// What a staging, a compression or an extraction made, what it skipped, and how far it has come: core's own.
pub use glint_core::archive::{Skipped, StageProgress, Staged};

/// Where a source reports progress to. Cheap to clone and safe to send from any thread: core's workers send.
pub type ProgressSink = async_channel::Sender<StageProgress>;

/// How a file is opened once it is a copy on disk (`openArchivedWith`'s `open`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenHow {
    /// With its app, as a double-click in File Explorer does (`openItem`).
    Default,
    /// With this program (the editor in Settings) (`openPath(path, editorPath)`).
    Editor(String),
    /// With Windows' "Open with" picker (`openWith`).
    Picker,
}

// ---- Telling the person -------------------------------------------------------------------------------------

/// What archive work is about, for its words: one item by name, or several by count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    /// `“{name}”`.
    One(String),
    /// `plural(count, "item")`: "3 items".
    Many(usize),
}

impl Subject {
    /// One item by its name when there is one path, else all of them by count (`paths.length === 1 ? … : plural`).
    pub fn of(paths: &[String]) -> Subject {
        match paths {
            [one] => Subject::One(glint_model::paths::base_name(one).to_string()),
            _ => Subject::Many(paths.len()),
        }
    }
}

/// Which slow archive work runs: the words of its notice are `{doing}…`, with ` {percent}%` once progress is
/// known (and for staging work, a **Stop** button that calls [`Explorer::stop_archive_work`](super::Explorer)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Doing {
    /// `Compressing {what}` (Compress to).
    Compressing(Subject),
    /// `Extracting {what}` (Extract here).
    Extracting(Subject),
    /// `Extracting {what} from the archive` (copying out of an archive: the other pane, the clipboard, a drag).
    ExtractingFromArchive(Subject),
    /// `Downloading {what}` (the same, from a remote connection).
    Downloading(Subject),
    /// `Extracting “{name}”` (a file in an archive, to open it). No Stop, no progress.
    ExtractingFile(String),
    /// `Mounting “{name}”`. No Stop, no progress.
    Mounting(String),
    /// `Copying out “{name}”` (a previous version of a file, to open it). No Stop, no progress.
    CopyingOut(String),
}

impl Doing {
    /// Whether the notice has a **Stop** button (the work can be cancelled in core).
    pub fn stoppable(&self) -> bool {
        matches!(
            self,
            Doing::Compressing(_) | Doing::Extracting(_) | Doing::ExtractingFromArchive(_) | Doing::Downloading(_)
        )
    }

    /// How long the work runs before its notice shows (the TypeScript's 400 ms for staging, 500 ms for `busy`).
    pub fn shows_after(&self) -> std::time::Duration {
        std::time::Duration::from_millis(if self.stoppable() { 400 } else { 500 })
    }
}

/// What "couldn't be {verb}" says about the files core left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkippedVerb {
    /// `added` (compressing).
    Added,
    /// `extracted`.
    Extracted,
    /// `downloaded`.
    Downloaded,
}

/// What failed, for the error notices' titles (`toast.error(title, { description: String(e) })`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveFailure {
    /// `Couldn't compress {what}`
    Compress(Subject),
    /// `Couldn't extract {what}`
    Extract(Subject),
    /// `Couldn't download {what}`
    Download(Subject),
    /// `Couldn't drag these items`
    Drag,
    /// `Couldn't mount “{name}”`
    Mount(String),
}

/// What the archives part tells the person. Carried by [`ExplorerEvent::Archive`](super::ExplorerEvent): the
/// workspace turns each into a notification with `t!`, the wording in the docs. (They are not variants of
/// [`OpNotice`](super::OpNotice) or [`Working`](super::Working) so that the matches over those in notices.rs
/// keep compiling; fold them in when wiring if you like.) Paths are carried whole; the notice shows their
/// `base_name`. Failures to open a file say `Notice::CouldNotOpenFile` (already worded), a read-only archive
/// `Notice::ReadOnly`, a closed folder `Notice::Closed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveNotice {
    /// Slow work is still running after [`Doing::shows_after`]: show a loading notice (no auto-hide) keyed by `id`
    /// that says `{doing}…` (`{doing}… {percent}%` when `percent` is known), with **Stop** when
    /// `doing.stoppable()`, and set the taskbar button to indeterminate (or normal with the progress, if any).
    Shown { id: u64, doing: Doing, percent: Option<u8> },
    /// How far the work has come (sent only when the percent changes; before the notice shows too, for the
    /// taskbar). Taskbar: normal with `done` of `total` (indeterminate while `total` is 0). When `shown`, replace
    /// the notice with the same id by `{doing}… {percent}%`.
    Progress {
        id: u64,
        doing: Doing,
        percent: u8,
        done: u64,
        total: u64,
        shown: bool,
    },
    /// The work ended: take the notice away when `was_shown`, and the taskbar progress when `idle` (no other
    /// operation, busy or archive work, runs).
    Ended { id: u64, was_shown: bool, idle: bool },
    /// Warning, 10 s, with a description: title `{count} file(s) couldn't be {verb}` (`plural(count, "file")`),
    /// description `{names}{more}: {reason}` where `names` are the first (at most three) `“{name}”` joined by `, `
    /// and `more` is ` and {n} more` when `more > 0`.
    Skipped {
        count: usize,
        names: Vec<String>,
        more: usize,
        reason: String,
        verb: SkippedVerb,
    },
    /// Error with the message as description: the title of the failure.
    Failed { what: ArchiveFailure, message: String },
    /// Success: `Compressed {what} into “{name of zip}”` (the archive isn't in the folder shown, so it can't be
    /// selected and renamed there).
    Compressed { what: Subject, zip: String },
    /// Success: `Extracted “{name of archive}” {as|into} “{name of folder}”` (`as` when `file`, a single compressed
    /// file written as the file itself) with a button `Show` (when `file`) or `Open`, which calls
    /// [`Explorer::show_extracted`](super::Explorer::show_extracted) with `folder` and `file`.
    Extracted {
        archive: String,
        folder: String,
        file: bool,
    },
    /// Message (id "drag-ready") `Ready to drag`, description `They took a moment to extract: drag them again.`
    ReadyToDrag,
    /// Success: `Mounted “{name}” as {drive}` (`drive` is the drive's path as core gave it, "F:\"), or
    /// `Mounted “{name}”` when no new drive was found.
    Mounted { name: String, drive: Option<String> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_percent_is_floored_clamped_and_zero_while_the_total_is_unknown() {
        let at = |done, total| StageProgress { done, total }.percent();
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(50, 0), 0);
        assert_eq!(at(1, 3), 33);
        assert_eq!(at(2, 3), 66);
        assert_eq!(at(3, 3), 100);
        assert_eq!(at(9, 3), 100);
        assert_eq!(at(u64::MAX, u64::MAX), 100);
    }

    #[test]
    fn formats_know_their_extension_and_which_take_a_password() {
        for format in [CompressFormat::Zip, CompressFormat::SevenZ, CompressFormat::TarZst] {
            assert_eq!(CompressFormat::from_ext(format.ext()), Some(format));
        }
        assert_eq!(CompressFormat::from_ext("rar"), None);
        assert!(CompressFormat::SevenZ.can_have_password());
        assert!(!CompressFormat::TarZst.can_have_password());
    }

    #[test]
    fn a_subject_is_a_name_for_one_path_and_a_count_for_more() {
        assert_eq!(
            Subject::of(&["C:\\a\\تقرير.pdf".to_string()]),
            Subject::One("تقرير.pdf".into())
        );
        assert_eq!(
            Subject::of(&["C:\\a".to_string(), "C:\\b".to_string()]),
            Subject::Many(2)
        );
        assert_eq!(Subject::of(&[]), Subject::Many(0));
    }

    #[test]
    fn only_staging_work_can_be_stopped_and_it_shows_sooner() {
        let subject = Subject::Many(2);
        assert!(Doing::Compressing(subject.clone()).stoppable());
        assert!(Doing::Downloading(subject).stoppable());
        assert!(!Doing::Mounting("a.iso".into()).stoppable());
        assert!(
            Doing::Extracting(Subject::One("a.zip".into())).shows_after() < Doing::CopyingOut("a".into()).shows_after()
        );
    }
}
