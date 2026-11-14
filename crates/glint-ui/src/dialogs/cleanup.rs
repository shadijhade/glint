//! Clean up: what can go to free space below a folder, or be tidied. Ported from
//! src/components/explorer/CleanupDialog.tsx (with glint-core's `cleanup.rs` doing the finding), built the way
//! [`super::drive_health`] is.
//!
//! | TypeScript                                  | Here                                                              |
//! |---------------------------------------------|-------------------------------------------------------------------|
//! | `useCleanupStore` (`root`, `mode`)          | [`open`], [`open_kind`]; the kind last picked is kept in [`LastKind`] |
//! | `ModeKey`, `MODES`, `SECTIONS`              | [`Kind`] and its methods                                          |
//! | `Helper` (`everything`, `unchangedFor`, `keepOne`) | [`Helper`] and [`Helper::pick`]                            |
//! | `Options`, `requestFor`, `SIZES`, `ALIKE`   | [`Options`], [`request_for`], [`SIZES`], [`alike_label`]           |
//! | `engineText`, `withExtension`, `inRoot`     | [`engine_text`], [`with_extension`], [`in_root`]                   |
//! | `cleanupScan`, `cancelCleanup`, `batchRename`, `resolvePath` | [`Source`] (glint-core in the app, a fake in tests) |
//! | `CleanupModal`'s state and functions        | [`Cleanup`], a model entity (`scan`, `drop_items`, `recycle`, `give_extensions`) |
//! | the `groups` memo                           | [`groups_of`], [`Ready`]                                           |
//! | `useTools`, `refreshTools`, `ToolNeeded`    | the model's own tool list and install (see below)                  |
//! | `fileOp({ kind: "delete" })`                | `Explorer::file_op`, so Undo and the Recycle Bin notice are the explorer's |
//! | toasts                                      | notifications, raised by the view from [`CleanupEvent`]            |
//! | `CleanupModal`                              | [`CleanupDialog`]                                                  |
//!
//! **What it does.** Nine kinds of look, in two groups in a list at the start edge. Each kind is looked for
//! the first time it is picked, in glint-core on its runtime (never the UI thread), and its answer is kept
//! while the dialog is open. Nothing is chosen until you look: a project's own build may be wanted. The
//! footer's helpers choose quickly; the big button moves what is chosen to the Recycle Bin (Windows asks
//! first about one too big for it, and Undo offers it back), or, for wrong extensions, gives each file its
//! real extension, all or none.
//!
//! **Differences from the web page, and why.**
//! - Rows are one height, so the list can be virtualized (it can hold 5,000); the page let them differ.
//! - The tools Czkawka and FFmpeg are looked for the first time a kind that needs them is picked, not when
//!   the dialog opens. Installing them is [`Source::install`]; the card is drawn here because Settings'
//!   `ToolNeeded` can't be reached from a dialog (its backend's types aren't exported).
//! - The dialog layer draws the title, so the header holds the folder's path, the badge and Refresh.
//!
//! **Keyboard.** The results are one tab stop: Up and Down move, Page Up/Down and Home/End jump, Space or
//! Enter chooses the row (or a whole group when on its heading).

use crate::components::file_grid::thumbnail::{self, Look, Thumbnails, Want};
use crate::explorer::{self, BatchPair, FileOp, LaunchTarget, UndoAction, UndoId};
use crate::kit::prelude::*;
use crate::{core_bridge, i18n, t, theme};
use glint_core::batch_rename::{BatchRenameResult, RenamePair};
use glint_core::cleanup::{Found, Mode, Report};
use glint_core::host::{self, Manager as _};
use glint_model::entries::{with_details, FileEntry, RawEntry};
use glint_model::file_types::{is_picture_or_video, kind_of};
use glint_model::format::{format_count, format_date, format_duration, format_size, plural};
use glint_model::paths::{base_name, parent_path};
use gpui_kit::UniformListScrollHandle;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// What Clean up finds: glint-core's `Found` (path, size, date, kind, group, note, fix, pixels).
pub type Item = Found;

/// The lane Clean up scans in, and asks for thumbnails in: one at a time, whichever tab opened it.
pub const LANE: &str = "cleanup";

/// A day in milliseconds.
const DAY: i64 = 86_400_000;

// ---- Where the work is done ----------------------------------------------------------------------------

/// A question to the PC, answered later.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// What a scan found (glint-core's `Report`, which isn't `Clone`).
#[derive(Clone, Debug, Default)]
pub struct Scan {
    /// Biggest first (empty folders by path); at most 5,000.
    pub items: Vec<Item>,
    /// Bytes the items take up.
    pub total: u64,
    /// Who found them: "index", "fd + pdu", "index + fd"…
    pub engine: String,
    /// Milliseconds the looking took.
    pub elapsed: f64,
    /// How many more were found than are listed.
    pub more: usize,
    /// Temporary files left out because a program has them open.
    pub held: usize,
}

impl From<Report> for Scan {
    fn from(report: Report) -> Self {
        Scan {
            items: report.items,
            total: report.total,
            engine: report.engine,
            elapsed: report.elapsed,
            more: report.more,
            held: report.held,
        }
    }
}

/// One of the optional programs a kind runs (`ToolStatus` in fs.ts, cut to what Clean up reads).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolState {
    pub id: String,
    pub name: String,
    /// It is on this PC (bundled, downloaded or installed).
    pub installed: bool,
    /// What Install tries first: "winget" or "release".
    pub source: String,
    /// The version its release download gets.
    pub pinned: String,
}

/// How an install is going: what it is doing, and bytes when it knows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub stage: String,
    pub done: u64,
    pub total: u64,
}

/// Where the answers come from: glint-core in the app, a fake in tests.
#[derive(Clone)]
pub struct Source {
    /// What `mode` finds below `root` for `lane` (a newer scan there ends this one with `"cancelled"`).
    pub scan: Arc<dyn Fn(String, Mode, String) -> Fut<Scan> + Send + Sync>,
    /// Stops the scan running in a lane (`cancelCleanup`). Never waits.
    pub cancel: Arc<dyn Fn(String) + Send + Sync>,
    /// Renames every pair or none, answering with the pairs that undo it (`batchRename`).
    pub rename: Arc<dyn Fn(Vec<RenamePair>) -> Fut<BatchRenameResult> + Send + Sync>,
    /// Which of these paths are gone now (`resolvePath` failing for them).
    pub missing: Arc<dyn Fn(Vec<String>) -> Fut<Vec<String>> + Send + Sync>,
    /// Every tool and whether it is there (`refreshTools`).
    pub tools: Arc<dyn Fn() -> Fut<Vec<ToolState>> + Send + Sync>,
    /// Installs a tool, telling the sender how it goes (`installTool`).
    pub install: Arc<dyn Fn(String, async_channel::Sender<Progress>) -> Fut<()> + Send + Sync>,
    /// Stops an install (`cancelInstall`). Never waits.
    pub cancel_install: Arc<dyn Fn(String) + Send + Sync>,
}

/// glint-core, as a [`Source`].
pub fn core_source(app: host::AppHandle) -> Source {
    let install_app = app.clone();
    let cancel_app = app;
    Source {
        scan: Arc::new(|root, mode, lane| {
            Box::pin(async move {
                glint_core::cleanup::cleanup_scan(root, mode, lane)
                    .await
                    .map(Scan::from)
            })
        }),
        cancel: Arc::new(glint_core::cleanup::cancel_cleanup),
        rename: Arc::new(|pairs| Box::pin(glint_core::batch_rename::batch_rename(pairs))),
        missing: Arc::new(|paths| {
            Box::pin(async move {
                let mut gone = Vec::new();
                for path in paths {
                    if glint_core::fsx::resolve_path(path.clone()).await.is_err() {
                        gone.push(path);
                    }
                }
                Ok(gone)
            })
        }),
        tools: Arc::new(|| {
            Box::pin(async {
                let tools = glint_core::tools::tools_status().await?;
                Ok(tools
                    .iter()
                    .filter_map(|tool| serde_json::to_value(tool).ok())
                    .filter_map(|value| tool_state(&value))
                    .collect())
            })
        }),
        install: Arc::new(move |id, progress| {
            let app = install_app.clone();
            Box::pin(async move {
                let installs = app.state::<glint_core::tools::Installs>();
                glint_core::tools::tool_install(id, progress_channel(progress), installs)
                    .await
                    .map(drop)
            })
        }),
        cancel_install: Arc::new(move |id| {
            let installs = cancel_app.state::<glint_core::tools::Installs>();
            glint_core::tools::tool_cancel_install(id, installs);
        }),
    }
}

/// A tool as core's JSON says it (core's `ToolStatus` has private fields).
fn tool_state(value: &serde_json::Value) -> Option<ToolState> {
    let text = |key: &str| value.get(key).and_then(serde_json::Value::as_str).map(str::to_string);
    Some(ToolState {
        id: text("id")?,
        name: text("name")?,
        installed: text("origin").is_some(),
        source: text("source").unwrap_or_default(),
        pinned: text("pinned").unwrap_or_default(),
    })
}

/// A channel for core that tells `progress` how an install goes (`tools::Progress` has private fields too).
fn progress_channel(progress: async_channel::Sender<Progress>) -> host::ipc::Channel<glint_core::tools::Progress> {
    host::ipc::Channel::new(move |p: glint_core::tools::Progress| {
        let value = serde_json::to_value(&p).unwrap_or(serde_json::Value::Null);
        let fetch = Progress {
            stage: value
                .get("stage")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            done: value.get("done").and_then(serde_json::Value::as_u64).unwrap_or(0),
            total: value.get("total").and_then(serde_json::Value::as_u64).unwrap_or(0),
        };
        progress
            .try_send(fetch)
            .map_err(|_| host::Error::from("the window that asked for this has gone"))
    })
}

/// "Downloading 2.1 MB of 7.4 MB…", or what winget is doing (`describeProgress`).
pub fn describe_progress(progress: &Progress) -> SharedString {
    if progress.total == 0 {
        return if progress.stage.ends_with('…') {
            progress.stage.clone().into()
        } else {
            format!("{}…", progress.stage.trim_end_matches('.')).into()
        };
    }
    t!(
        "{stage} {done} of {total}…",
        stage = progress.stage,
        done = format_size(progress.done),
        total = format_size(progress.total)
    )
}

/// What Install will do, for its tooltip (`installHint`).
fn install_hint(tool: &ToolState) -> SharedString {
    if tool.source == "release" {
        t!(
            "Downloads {name} {version} from its GitHub release and checks it",
            name = tool.name,
            version = tool.pinned
        )
        .replace("  ", " ")
        .into()
    } else {
        t!("Installs {name} with winget", name = tool.name)
    }
}

// ---- The kinds of look ------------------------------------------------------------------------------------

/// What Clean up can look for (`ModeKey`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Build output and package caches beside their projects.
    Junk,
    /// Folders with nothing in them.
    Empty,
    /// What programs leave behind: .tmp files, Office's owner files, Thumbs.db.
    Temporary,
    /// The biggest files.
    Big,
    /// Pictures that look the same (Czkawka).
    Images,
    /// Videos with the same scenes (Czkawka and FFmpeg).
    Videos,
    /// Songs that are the same (Czkawka).
    Music,
    /// Files that don't open (Czkawka).
    Broken,
    /// Files whose extension says another type than what is in them (Czkawka).
    Misnamed,
}

/// An optional program a kind runs, and why (`needs`).
#[derive(Clone, Debug)]
pub struct Need {
    pub id: &'static str,
    pub why: SharedString,
}

impl Kind {
    /// Every kind, in the list's order.
    pub const ALL: [Kind; 9] = [
        Kind::Junk,
        Kind::Empty,
        Kind::Temporary,
        Kind::Big,
        Kind::Images,
        Kind::Videos,
        Kind::Music,
        Kind::Broken,
        Kind::Misnamed,
    ];

    /// The list's two sections (`SECTIONS`): "Free up space" and "With Czkawka".
    pub fn section_list() -> Vec<(SharedString, Vec<Kind>)> {
        vec![
            (
                t!("Free up space"),
                vec![Kind::Junk, Kind::Empty, Kind::Temporary, Kind::Big],
            ),
            (
                t!("With Czkawka"),
                vec![Kind::Images, Kind::Videos, Kind::Music, Kind::Broken, Kind::Misnamed],
            ),
        ]
    }

    /// The list's word for it.
    pub fn label(self) -> SharedString {
        match self {
            Kind::Junk => t!("Developer junk"),
            Kind::Empty => t!("Empty folders"),
            Kind::Temporary => t!("Temporary files"),
            Kind::Big => t!("Big files"),
            Kind::Images => t!("Look-alike pictures"),
            Kind::Videos => t!("Look-alike videos"),
            Kind::Music => t!("Same music"),
            Kind::Broken => t!("Broken files"),
            Kind::Misnamed => t!("Wrong extensions"),
        }
    }

    /// Its icon.
    pub fn icon(self) -> IconName {
        match self {
            Kind::Junk => IconName::Package,
            Kind::Empty => IconName::FolderX,
            Kind::Temporary => IconName::Hourglass,
            Kind::Big => IconName::FileBox,
            Kind::Images => IconName::Images,
            Kind::Videos => IconName::Video,
            Kind::Music => IconName::Music,
            Kind::Broken => IconName::FileX,
            Kind::Misnamed => IconName::FileExclamationPoint,
        }
    }

    /// Above the results: what it looks for.
    pub fn about(self) -> SharedString {
        match self {
            Kind::Junk => t!("Build output and package caches that a project's own tools make again, each beside its project's file."),
            Kind::Empty => t!("Folders with nothing in them, down through their subfolders. Hidden folders aren't looked in."),
            Kind::Temporary => t!("What programs leave behind: .tmp files, Office's ~$ owner files, Thumbs.db, .DS_Store. What a program still has open is left out."),
            Kind::Big => t!("The biggest files, outside hidden folders and AppData (where programs keep theirs). Recycle only what you know."),
            Kind::Images => t!("Pictures that look the same though their files differ: resized, converted or re-saved copies. The biggest of each group comes first."),
            Kind::Videos => t!("Videos with the same scenes, whatever their size or format. FFmpeg reads them, so a big folder takes a while."),
            Kind::Music => t!("Songs that are the same: by their tags (title and artist), or by how they sound."),
            Kind::Broken => t!("Pictures, sound, PDFs and archives that don't open, with why. A file with the wrong extension can show here too: see Wrong extensions."),
            Kind::Misnamed => t!("Files whose extension says another type than what's in them, such as a PNG named .jpg. Chosen ones get their real extension."),
        }
    }

    /// One of what it finds, for `plural` (English, as the page's `noun`).
    pub fn noun(self) -> &'static str {
        match self {
            Kind::Junk => "folder",
            Kind::Empty => "empty folder",
            Kind::Temporary | Kind::Big | Kind::Broken | Kind::Misnamed => "file",
            Kind::Images => "picture",
            Kind::Videos => "video",
            Kind::Music => "song",
        }
    }

    /// The date's tooltip.
    pub fn dated(self) -> SharedString {
        match self {
            Kind::Junk => t!("When its project last changed, leaving out its build output and caches"),
            _ => t!("Last changed"),
        }
    }

    /// It finds folders (with no size to show, for empty ones).
    pub fn folders(self) -> bool {
        matches!(self, Kind::Junk | Kind::Empty)
    }

    /// Look-alikes, shown in their groups.
    pub fn grouped(self) -> bool {
        matches!(self, Kind::Images | Kind::Videos | Kind::Music)
    }

    /// What is chosen gets its real extension rather than going to the Recycle Bin.
    pub fn renames(self) -> bool {
        self == Kind::Misnamed
    }

    /// The optional tools it runs (the tools shelf installs them).
    pub fn needs(self) -> Vec<Need> {
        let czkawka = Need {
            id: "czkawka",
            why: t!("Czkawka finds look-alike pictures, videos and music, broken files and wrong extensions. It installs for your account with winget."),
        };
        match self {
            Kind::Images | Kind::Music | Kind::Broken | Kind::Misnamed => vec![czkawka],
            Kind::Videos => vec![
                czkawka,
                Need {
                    id: "ffmpeg",
                    why: t!("FFmpeg reads the videos for Czkawka. It installs for your account with winget."),
                },
            ],
            Kind::Junk | Kind::Empty | Kind::Temporary | Kind::Big => Vec::new(),
        }
    }

    /// The quick ways to choose that it offers (`helpers`).
    pub fn helpers(self) -> &'static [Helper] {
        match self {
            Kind::Junk => &[Helper::All, Helper::ProjectsUntouched30],
            Kind::Empty | Kind::Broken | Kind::Misnamed => &[Helper::All],
            Kind::Temporary => &[Helper::All, Helper::OlderThanWeek],
            Kind::Big => &[Helper::UnchangedForAYear],
            Kind::Images | Kind::Videos | Kind::Music => &[Helper::KeepBiggest, Helper::KeepNewest],
        }
    }

    /// In the header, for what was found (`badge`).
    pub fn badge(self, scan: &Scan) -> SharedString {
        match self {
            Kind::Junk => t!("{size} of developer junk", size = format_size(scan.total)),
            Kind::Empty => plural(scan.items.len(), "empty folder").into(),
            Kind::Temporary => t!("{size} left behind", size = format_size(scan.total)),
            Kind::Big => t!("{size} in big files", size = format_size(scan.total)),
            Kind::Images | Kind::Videos | Kind::Music => look_alikes(scan),
            Kind::Broken => plural(scan.items.len(), "broken file").into(),
            Kind::Misnamed => plural(scan.items.len(), "misnamed file").into(),
        }
    }

    /// What the empty result says (`nothingFound`).
    pub fn nothing_found(self, options: &Options) -> SharedString {
        match self {
            Kind::Junk => t!("No developer junk here"),
            Kind::Empty => t!("No empty folders here"),
            Kind::Temporary => t!("Nothing left behind here"),
            Kind::Big => t!("No files of {size} or more here", size = format_size(options.at_least)),
            Kind::Images => t!("No look-alike pictures here"),
            Kind::Videos => t!("No look-alike videos here"),
            Kind::Music => t!("No songs found twice here"),
            Kind::Broken => t!("Nothing broken here"),
            Kind::Misnamed => t!("Every file here has the right extension"),
        }
    }
}

/// How many groups of look-alikes, and what all but the biggest of each take up (`spareOf`).
pub fn spare_of(scan: &Scan) -> (usize, u64) {
    let mut biggest: HashMap<u32, u64> = HashMap::new();
    for item in &scan.items {
        let slot = biggest.entry(item.group).or_insert(0);
        *slot = (*slot).max(item.size);
    }
    let kept: u64 = biggest.values().sum();
    (biggest.len(), scan.total.saturating_sub(kept))
}

/// "2 groups · 4.0 MB in extra copies" (`lookAlikes`).
fn look_alikes(scan: &Scan) -> SharedString {
    let (groups, spare) = spare_of(scan);
    t!(
        "{groups} · {spare} in extra copies",
        groups = plural(groups, "group"),
        spare = format_size(spare)
    )
}

// ---- Choosing quickly ----------------------------------------------------------------------------------------

/// A quick way to choose: everything, what has been left alone a while, or all but the best of each
/// look-alike group (`Helper`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Helper {
    /// Everything listed.
    All,
    /// Only junk of projects nothing has changed in for 30 days.
    ProjectsUntouched30,
    /// Only what has not changed in a week.
    OlderThanWeek,
    /// Only files that have not changed in a year.
    UnchangedForAYear,
    /// All but the biggest of each group (the most pixels, for pictures).
    KeepBiggest,
    /// All but the newest of each group.
    KeepNewest,
}

impl Helper {
    /// The button's word.
    pub fn label(self) -> SharedString {
        match self {
            Helper::All => t!("All"),
            Helper::ProjectsUntouched30 => t!("Projects untouched for 30 days"),
            Helper::OlderThanWeek => t!("Older than a week"),
            Helper::UnchangedForAYear => t!("Unchanged for a year"),
            Helper::KeepBiggest => t!("Keep the biggest"),
            Helper::KeepNewest => t!("Keep the newest"),
        }
    }

    /// Its tooltip.
    pub fn title(self) -> SharedString {
        match self {
            Helper::All => t!("Choose everything listed"),
            Helper::ProjectsUntouched30 => {
                t!("Only junk of projects nothing has changed in for 30 days (their build output and caches aside)")
            }
            Helper::OlderThanWeek => t!("Only what hasn't changed in a week"),
            Helper::UnchangedForAYear => t!("Only files that haven't changed in a year"),
            Helper::KeepBiggest => {
                t!("Choose all but the biggest of each group of look-alikes (the most pixels, for pictures)")
            }
            Helper::KeepNewest => t!("Choose all but the newest of each group of look-alikes"),
        }
    }

    /// The indices of the items it chooses, `now` being Unix milliseconds (`pick`).
    pub fn pick(self, items: &[Item], now: i64) -> Vec<usize> {
        match self {
            Helper::All => (0..items.len()).collect(),
            Helper::ProjectsUntouched30 => unchanged_for(items, now, 30),
            Helper::OlderThanWeek => unchanged_for(items, now, 7),
            Helper::UnchangedForAYear => unchanged_for(items, now, 365),
            Helper::KeepBiggest => keep_one(items, |a, b| b.pixels.cmp(&a.pixels).then(b.size.cmp(&a.size))),
            Helper::KeepNewest => keep_one(items, |a, b| b.date.cmp(&a.date)),
        }
    }
}

/// Items that have a date and haven't changed in `days` (`unchangedFor`).
fn unchanged_for(items: &[Item], now: i64, days: i64) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.date > 0 && item.date < now - days * DAY)
        .map(|(ix, _)| ix)
        .collect()
}

/// Everything but the one `better` puts first in each group of look-alikes (`keepOne`); the first of equals
/// stays, as the TypeScript's stable sort kept it.
fn keep_one(items: &[Item], better: impl Fn(&Item, &Item) -> Ordering) -> Vec<usize> {
    let mut order: Vec<u32> = Vec::new();
    let mut groups: HashMap<u32, Vec<usize>> = HashMap::new();
    for (ix, item) in items.iter().enumerate() {
        groups
            .entry(item.group)
            .or_insert_with(|| {
                order.push(item.group);
                Vec::new()
            })
            .push(ix);
    }
    let mut picked = Vec::new();
    for group in order {
        let Some(members) = groups.get(&group) else {
            continue;
        };
        // The indices come from enumerating `items` just above.
        let kept = members.iter().copied().min_by(|&a, &b| better(&items[a], &items[b]));
        picked.extend(members.iter().copied().filter(|&ix| Some(ix) != kept));
    }
    picked
}

// ---- Options and requests -----------------------------------------------------------------------------------

/// Big files' sizes to pick from: 100 MB to 10 GB (`SIZES`).
pub const SIZES: [u64; 5] = [
    100 * 1024 * 1024,
    500 * 1024 * 1024,
    1024 * 1024 * 1024,
    4096 * 1024 * 1024,
    10240 * 1024 * 1024,
];

/// How different look-alikes may be: pictures' perceptual hash distance and videos' frame tolerance (`ALIKE`).
const ALIKE: [(u8, u8); 3] = [(2, 4), (6, 10), (12, 16)];

/// The three levels of "Match" (`ALIKE[n].label`).
pub fn alike_label(level: usize) -> SharedString {
    match level {
        0 => t!("Nearly identical"),
        1 => t!("Very similar"),
        _ => t!("Similar"),
    }
}

/// The titles of the "At least" box, in [`SIZES`] order.
fn size_titles() -> Vec<SharedString> {
    SIZES.iter().map(|&size| format_size(size).into()).collect()
}

/// The titles of the "Match" box, in [`ALIKE`] order.
fn alike_titles() -> Vec<SharedString> {
    (0..ALIKE.len()).map(alike_label).collect()
}

/// The titles of the "Compare" box: tags, then sound.
fn compare_titles() -> Vec<SharedString> {
    vec![t!("Tags: title and artist"), t!("How they sound (slower)")]
}

/// Where `size` is in the "At least" box (none for a size the box doesn't offer).
fn size_at(size: u64) -> Option<IndexPath> {
    SIZES.iter().position(|&s| s == size).map(IndexPath::new)
}

/// The choices that shape a scan (`Options`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Big files: the smallest size listed.
    pub at_least: u64,
    /// An index into the three levels of "Match".
    pub alike: usize,
    /// Music: compare by sound rather than by tags.
    pub by_sound: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            at_least: SIZES[2],
            alike: 1,
            by_sound: false,
        }
    }
}

/// What to ask glint-core for `kind` with these options (`requestFor`).
pub fn request_for(kind: Kind, options: &Options) -> Mode {
    let (images, videos) = ALIKE.get(options.alike).copied().unwrap_or(ALIKE[1]);
    match kind {
        Kind::Junk => Mode::Junk,
        Kind::Empty => Mode::Empty,
        Kind::Temporary => Mode::Temporary,
        Kind::Big => Mode::Big {
            at_least: options.at_least,
        },
        Kind::Images => Mode::Images { alike: images },
        Kind::Videos => Mode::Videos { alike: videos },
        Kind::Music => Mode::Music {
            by_sound: options.by_sound,
        },
        Kind::Broken => Mode::Broken,
        Kind::Misnamed => Mode::Misnamed,
    }
}

/// "index + fd + pdu" as "the index, fd and pdu" (`engineText`).
pub fn engine_text(engine: &str) -> String {
    let parts: Vec<&str> = engine
        .split(" + ")
        .map(|part| match part {
            "index" => "the index",
            "fd" => "fd",
            "pdu" => "pdu",
            "czkawka" => "Czkawka",
            other => other,
        })
        .collect();
    match parts.split_last() {
        Some((last, rest)) if !rest.is_empty() => {
            t!("{list} and {last}", list = rest.join(", "), last = last).to_string()
        }
        Some((only, _)) => (*only).to_string(),
        None => String::new(),
    }
}

/// "photo.JPG" with the extension `fix` instead: "photo.png" (`withExtension`).
pub fn with_extension(path: &str, fix: &str) -> String {
    let stem = path
        .rfind('.')
        .filter(|&dot| !path.get(dot..).unwrap_or("").contains(['\\', '/']))
        .and_then(|dot| path.get(..dot))
        .unwrap_or(path);
    format!("{stem}.{fix}")
}

/// A path from the folder scanned (`web\node_modules`), so the project's name isn't what gets cut off
/// (`inRoot`).
pub fn in_root(root: &str, path: &str) -> String {
    let base = root.trim_end_matches(['\\', '/']);
    let inside = path
        .get(..base.len())
        .zip(path.get(base.len()..))
        .filter(|(prefix, _)| prefix.to_lowercase() == base.to_lowercase())
        .and_then(|(_, rest)| rest.strip_prefix('\\'));
    inside.unwrap_or(path).to_string()
}

/// The summary line under the results (`summary`), for the kind's own report.
pub fn summary(kind: Kind, scan: &Scan) -> String {
    let mut parts = vec![plural(scan.items.len() + scan.more, kind.noun())];
    if !kind.folders() || kind == Kind::Junk {
        parts.push(format_size(scan.total));
    }
    parts.push(
        t!(
            "found by {engine} in {time}",
            engine = engine_text(&scan.engine),
            time = format_duration(scan.elapsed)
        )
        .to_string(),
    );
    if scan.more > 0 {
        let listed = format_count(scan.items.len());
        parts.push(if kind == Kind::Empty {
            t!("the first {n} listed", n = listed).to_string()
        } else {
            t!("the {n} biggest listed", n = listed).to_string()
        });
    }
    if scan.held > 0 {
        parts.push(t!("{files} in use left out", files = plural(scan.held, "file")).to_string());
    }
    parts.retain(|part| !part.is_empty());
    parts.join(" · ")
}

// ---- Grouping what was found --------------------------------------------------------------------------------

/// A file entry for an item, to ask what kind of file it is (`withDetails` in the page).
fn entry_of(kind: Kind, item: &Item) -> FileEntry {
    with_details(
        RawEntry {
            name: base_name(&item.path).to_string(),
            path: Some(item.path.clone()),
            dir: kind.folders(),
            size: item.size,
            modified: item.date,
            attrs: 0,
            ..Default::default()
        },
        "",
    )
}

/// A heading and what is under it.
#[derive(Clone, Debug)]
pub struct Group {
    /// "Rust builds", "Pictures", or "“a.jpg” and 2 look-alikes".
    pub title: SharedString,
    /// Indices into the scan's items.
    pub items: Vec<usize>,
    /// What they take up.
    pub size: u64,
}

/// Grouped by what it is (big files by type, look-alikes by group), biggest group first (empty folders in one)
/// (the `groups` memo).
pub fn groups_of(kind: Kind, items: &[Item]) -> Vec<Group> {
    // Groups in the order they first appear, as a JS `Map` kept them. A group's heading is core's own word for
    // it (`kind`), or one of ours (translated), or a look-alike group's first file.
    let mut order: Vec<(String, Option<&'static str>, Vec<usize>)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for (ix, item) in items.iter().enumerate() {
        let (key, ours): (String, Option<&'static str>) = if kind.grouped() {
            (format!("#{}", item.group), None)
        } else if !item.kind.is_empty() {
            (item.kind.clone(), None)
        } else if kind.folders() {
            ("Empty folders".to_string(), Some("Empty folders"))
        } else {
            let name = kind_of(&entry_of(kind, item)).group.name;
            (name.to_string(), Some(name))
        };
        let slot = *at.entry(key.clone()).or_insert_with(|| {
            order.push((key, ours, Vec::new()));
            order.len() - 1
        });
        if let Some((_, _, members)) = order.get_mut(slot) {
            members.push(ix);
        }
    }
    let mut groups: Vec<Group> = order
        .into_iter()
        .map(|(key, ours, members)| {
            let first = members.first().and_then(|&ix| items.get(ix));
            let title = match (first, ours) {
                // A group of look-alikes is called by its first (biggest) one.
                (Some(first), _) if kind.grouped() => t!(
                    "“{name}” and {others}",
                    name = base_name(&first.path),
                    others = plural(members.len().saturating_sub(1), "look-alike")
                ),
                (_, Some(ours)) => i18n::translate(ours),
                _ => SharedString::from(key),
            };
            let size = members
                .iter()
                .filter_map(|&ix| items.get(ix))
                .map(|item| item.size)
                .sum();
            Group {
                title,
                items: members,
                size,
            }
        })
        .collect();
    groups.sort_by_key(|group| std::cmp::Reverse(group.size));
    groups
}

/// A line of the results: a group's heading, or one of its items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Group(usize),
    Row { group: usize, item: usize },
}

/// What a finished scan holds, ready to draw.
#[derive(Debug)]
pub struct Ready {
    pub scan: Scan,
    pub groups: Vec<Group>,
    /// Headings and rows, flattened for a virtualized list.
    pub lines: Vec<Line>,
}

impl Ready {
    fn new(kind: Kind, scan: Scan) -> Self {
        let groups = groups_of(kind, &scan.items);
        let mut lines = Vec::with_capacity(scan.items.len() + groups.len());
        for (g, group) in groups.iter().enumerate() {
            lines.push(Line::Group(g));
            lines.extend(group.items.iter().map(|&item| Line::Row { group: g, item }));
        }
        Ready { scan, groups, lines }
    }
}

// ---- The model ----------------------------------------------------------------------------------------------

/// Where one kind's look stands (`Outcome`).
#[derive(Clone, Debug)]
pub enum Outcome {
    /// Looking.
    Loading,
    /// Found (maybe nothing).
    Ready(Rc<Ready>),
    /// Couldn't look, with why.
    Failed(SharedString),
    /// Stopped before it finished.
    Stopped,
}

/// What the model tells the view to say.
#[derive(Clone, Debug)]
pub enum CleanupEvent {
    /// `Couldn't recycle the files` (or folders), with the reason.
    RecycleFailed { folders: bool, reason: SharedString },
    /// Files got their real extension: `Gave 2 files their real extension`, with an Undo that is the explorer's
    /// step `step` (which renames the pairs back: Ctrl+Z runs it too). `None` when the batch changed nothing or
    /// there is no explorer to keep a step in: nothing to offer.
    GaveExtensions { renamed: usize, step: Option<UndoId> },
    /// `Couldn't rename them`, with the reason.
    RenameFailed { reason: SharedString },
    /// A tool is installed.
    Installed { name: SharedString },
    /// `Couldn't install {name}`, with the reason.
    InstallFailed { name: SharedString, reason: SharedString },
}

/// The kind last picked, while the app runs (`useCleanupStore.mode`).
#[derive(Clone, Copy)]
pub struct LastKind(pub Kind);

impl Global for LastKind {}

/// The time now, Unix milliseconds.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// Clean up's state: the folder, the kind shown, what each kind found and what is chosen of it.
pub struct Cleanup {
    root: String,
    kind: Kind,
    options: Options,
    outcomes: HashMap<Kind, Outcome>,
    chosen: HashMap<Kind, Rc<HashSet<String>>>,
    /// Recycling or renaming is under way.
    busy: bool,
    /// The number of the scan asked for last: one stopped for it (another kind, Refresh) mustn't say it stopped.
    run: u64,
    source: Source,
    tools: Vec<ToolState>,
    tools_known: bool,
    tools_loading: bool,
    tools_error: Option<SharedString>,
    /// Installs under way, with what each says.
    installing: HashMap<String, SharedString>,
    _release: Subscription,
}

impl EventEmitter<CleanupEvent> for Cleanup {}

impl Cleanup {
    /// Cleaning up `root`, starting on `kind`: it looks right away if it can.
    pub fn new(root: String, kind: Kind, source: Source, cx: &mut Context<Self>) -> Self {
        let cancel = source.cancel.clone();
        let release = cx.on_release(move |_, _| cancel(LANE.to_string()));
        let mut this = Cleanup {
            root,
            kind,
            options: Options::default(),
            outcomes: HashMap::new(),
            chosen: HashMap::new(),
            busy: false,
            run: 0,
            source,
            tools: Vec::new(),
            tools_known: false,
            tools_loading: false,
            tools_error: None,
            installing: HashMap::new(),
            _release: release,
        };
        this.ensure_scan(cx);
        this
    }

    /// The folder being cleaned.
    pub fn root(&self) -> &str {
        &self.root
    }

    /// The kind shown.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The options in force.
    pub fn options(&self) -> Options {
        self.options
    }

    /// Recycling or renaming is under way.
    pub fn busy(&self) -> bool {
        self.busy
    }

    /// Where `kind`'s look stands, once it was started.
    pub fn outcome(&self, kind: Kind) -> Option<&Outcome> {
        self.outcomes.get(&kind)
    }

    /// How many `kind` found, once it has (the count in the list).
    pub fn found_count(&self, kind: Kind) -> Option<usize> {
        match self.outcomes.get(&kind) {
            Some(Outcome::Ready(ready)) => Some(ready.scan.items.len()),
            _ => None,
        }
    }

    /// What is chosen of the current kind.
    pub fn chosen(&self) -> Rc<HashSet<String>> {
        self.chosen.get(&self.kind).cloned().unwrap_or_default()
    }

    /// The current kind's found items that are chosen, in the list's order.
    pub fn chosen_items(&self) -> Vec<Item> {
        let (Some(Outcome::Ready(ready)), Some(chosen)) = (self.outcomes.get(&self.kind), self.chosen.get(&self.kind))
        else {
            return Vec::new();
        };
        ready
            .scan
            .items
            .iter()
            .filter(|item| chosen.contains(&item.path))
            .cloned()
            .collect()
    }

    /// The tools the current kind needs that are missing and known to be (`missing`).
    pub fn missing(&self, kind: Kind) -> Vec<Need> {
        kind.needs()
            .into_iter()
            .filter(|need| !self.tools.iter().any(|tool| tool.id == need.id && tool.installed))
            .collect()
    }

    /// Whether `kind` can be looked for now: what it runs is installed (`ready`).
    pub fn ready(&self, kind: Kind) -> bool {
        kind.needs().is_empty() || (self.tools_known && self.missing(kind).is_empty())
    }

    /// The tools are known (`toolsKnown`).
    pub fn tools_known(&self) -> bool {
        self.tools_known
    }

    /// Why the tools couldn't be read, if they couldn't.
    pub fn tools_error(&self) -> Option<&SharedString> {
        self.tools_error.as_ref()
    }

    /// A tool by id.
    pub fn tool(&self, id: &str) -> Option<&ToolState> {
        self.tools.iter().find(|tool| tool.id == id)
    }

    /// What the install of `id` says, while one runs.
    pub fn installing(&self, id: &str) -> Option<&SharedString> {
        self.installing.get(id)
    }

    // ---- Looking ---------------------------------------------------------------------------------------

    /// Looks for the current kind the first time it is shown, once what it runs is installed (the page's effect).
    fn ensure_scan(&mut self, cx: &mut Context<Self>) {
        let kind = self.kind;
        if !kind.needs().is_empty() && !self.tools_known && !self.tools_loading && self.tools_error.is_none() {
            self.refresh_tools(cx);
        }
        if !self.outcomes.contains_key(&kind) && self.ready(kind) {
            let options = self.options;
            self.scan(kind, options, cx);
        }
    }

    /// Looks for `kind` (`scan`). One still looking is stopped by this one: it looks again when it is picked
    /// again. Nothing is chosen until it's looked at.
    pub fn scan(&mut self, kind: Kind, options: Options, cx: &mut Context<Self>) {
        self.run += 1;
        let run = self.run;
        self.outcomes.retain(|_, outcome| !matches!(outcome, Outcome::Loading));
        self.outcomes.insert(kind, Outcome::Loading);
        self.chosen.insert(kind, Rc::default());
        cx.notify();
        let asked = (self.source.scan)(self.root.clone(), request_for(kind, &options), LANE.to_string());
        cx.spawn(async move |this, cx| {
            let answer = core_bridge::run(asked).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                if run != this.run {
                    return;
                }
                let outcome = match answer {
                    Ok(scan) => Outcome::Ready(Rc::new(Ready::new(kind, scan))),
                    Err(reason) if reason == "cancelled" => Outcome::Stopped,
                    Err(reason) => Outcome::Failed(reason.into()),
                };
                this.outcomes.insert(kind, outcome);
                cx.notify();
            });
        })
        .detach();
    }

    /// Looks again at the current kind (Refresh, Look again).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.ready(self.kind) {
            let (kind, options) = (self.kind, self.options);
            self.scan(kind, options, cx);
        } else if self.tools_error.is_some() {
            self.tools_error = None;
            self.ensure_scan(cx);
        }
    }

    /// Stops the scan that is running (`cancelCleanup`).
    pub fn stop(&self) {
        (self.source.cancel)(LANE.to_string());
    }

    /// Shows another kind, looking for it the first time (`useCleanupStore.setState({ mode })`).
    pub fn select_kind(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if kind == self.kind {
            return;
        }
        self.kind = kind;
        cx.set_global(LastKind(kind));
        cx.notify();
        self.ensure_scan(cx);
    }

    /// Changes the options and looks again at the current kind if it can be looked for (`changeOptions`).
    pub fn change_options(&mut self, change: impl FnOnce(&mut Options), cx: &mut Context<Self>) {
        let mut next = self.options;
        change(&mut next);
        self.options = next;
        cx.notify();
        if self.ready(self.kind) {
            let kind = self.kind;
            self.scan(kind, next, cx);
        }
    }

    // ---- Choosing ----------------------------------------------------------------------------------------

    /// Chooses or unchooses these paths of the current kind (`toggle`).
    pub fn toggle(&mut self, paths: &[String], on: bool, cx: &mut Context<Self>) {
        let chosen = Rc::make_mut(self.chosen.entry(self.kind).or_default());
        for path in paths {
            if on {
                chosen.insert(path.clone());
            } else {
                chosen.remove(path);
            }
        }
        cx.notify();
    }

    /// What clicking line `ix` does: a row toggles, a heading chooses all of its group or none if all are.
    pub fn toggle_line(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(Outcome::Ready(ready)) = self.outcomes.get(&self.kind).cloned() else {
            return;
        };
        let chosen = self.chosen();
        match ready.lines.get(ix).copied() {
            Some(Line::Row { item, .. }) => {
                if let Some(found) = ready.scan.items.get(item) {
                    let on = !chosen.contains(&found.path);
                    self.toggle(std::slice::from_ref(&found.path), on, cx);
                }
            }
            Some(Line::Group(g)) => {
                let paths: Vec<String> = ready
                    .groups
                    .get(g)
                    .map(|group| {
                        group
                            .items
                            .iter()
                            .filter_map(|&item| ready.scan.items.get(item))
                            .map(|item| item.path.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let all = !paths.is_empty() && paths.iter().all(|path| chosen.contains(path));
                self.toggle(&paths, !all, cx);
            }
            None => {}
        }
    }

    /// Chooses what a helper picks, instead of what was chosen (`helper.pick`).
    pub fn apply_helper(&mut self, helper: Helper, now: i64, cx: &mut Context<Self>) {
        let Some(Outcome::Ready(ready)) = self.outcomes.get(&self.kind) else {
            return;
        };
        let picked: HashSet<String> = helper
            .pick(&ready.scan.items, now)
            .into_iter()
            .filter_map(|ix| ready.scan.items.get(ix))
            .map(|item| item.path.clone())
            .collect();
        self.chosen.insert(self.kind, Rc::new(picked));
        cx.notify();
    }

    /// Chooses nothing (`None`).
    pub fn choose_none(&mut self, cx: &mut Context<Self>) {
        self.chosen.insert(self.kind, Rc::default());
        cx.notify();
    }

    // ---- Doing -------------------------------------------------------------------------------------------

    /// What's gone (or renamed) leaves the list; the rest stays as it was found, and the folder on screen is
    /// read again (`drop`).
    fn drop_items(&mut self, at: Kind, gone: &HashSet<String>, cx: &mut Context<Self>) {
        if gone.is_empty() {
            return;
        }
        if let Some(Outcome::Ready(was)) = self.outcomes.get(&at) {
            let left: Vec<Item> = was
                .scan
                .items
                .iter()
                .filter(|item| !gone.contains(&item.path))
                .cloned()
                .collect();
            // A look-alike left on its own is no longer one.
            let mut counts: HashMap<u32, usize> = HashMap::new();
            for item in &left {
                *counts.entry(item.group).or_insert(0) += 1;
            }
            let kept: Vec<Item> = left
                .into_iter()
                .filter(|item| item.group == 0 || counts.get(&item.group).copied().unwrap_or(0) > 1)
                .collect();
            let scan = Scan {
                total: kept.iter().map(|item| item.size).sum(),
                items: kept,
                ..was.scan.clone()
            };
            self.outcomes.insert(at, Outcome::Ready(Rc::new(Ready::new(at, scan))));
        }
        if let Some(chosen) = self.chosen.get_mut(&at) {
            Rc::make_mut(chosen).retain(|path| !gone.contains(path));
        }
        if let Some(explorer) = explorer::try_explorer(cx) {
            explorer.update(cx, |explorer, cx| explorer.reload(None, cx).detach());
        }
        cx.notify();
    }

    /// Moves what is chosen to the Recycle Bin, Windows asking first where it does (`recycle`). The explorer
    /// runs the operation, so Undo and the notice are its own. What went leaves the list; what a cancelled
    /// dialog left stays.
    pub fn recycle(&mut self, cx: &mut Context<Self>) {
        let going: Vec<String> = self.chosen_items().into_iter().map(|item| item.path).collect();
        let at = self.kind;
        if going.is_empty() || self.busy {
            return;
        }
        let Some(explorer) = explorer::try_explorer(cx) else {
            return;
        };
        self.busy = true;
        cx.notify();
        let op = FileOp::Delete {
            paths: going.clone(),
            permanent: false,
            confirm: true,
        };
        let task = explorer.update(cx, |explorer, cx| explorer.file_op(op, cx));
        let missing = self.source.missing.clone();
        cx.spawn(async move |this, cx| {
            let done = task.await;
            if let Err(reason) = done {
                let _ = this.update(cx, |_, cx| {
                    cx.emit(CleanupEvent::RecycleFailed {
                        folders: at.folders(),
                        reason: reason.into(),
                    })
                });
            }
            // Cancelled in Windows' dialog partway, some went and some didn't.
            let gone = core_bridge::run(missing(going))
                .await
                .and_then(|inner| inner)
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.drop_items(at, &gone.into_iter().collect(), cx);
                this.busy = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Gives each chosen misnamed file its real extension, all or none, with Undo (`giveExtensions`): the
    /// explorer keeps "extensions of 3 files" as a step, so Ctrl+Z takes it back as the page's did, and the
    /// notification's Undo runs that same step.
    pub fn give_extensions(&mut self, cx: &mut Context<Self>) {
        let going: Vec<Item> = self
            .chosen_items()
            .into_iter()
            .filter(|item| !item.fix.is_empty())
            .collect();
        let at = self.kind;
        if going.is_empty() || self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        let pairs: Vec<RenamePair> = going
            .iter()
            .map(|item| RenamePair {
                from: item.path.clone(),
                to: with_extension(&item.path, &item.fix),
            })
            .collect();
        let asked = (self.source.rename)(pairs);
        cx.spawn(async move |this, cx| {
            let answer = core_bridge::run(asked).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                match answer {
                    Ok(done) => {
                        let step = remember_extensions(done.renamed, &done.undo_ops, cx);
                        cx.emit(CleanupEvent::GaveExtensions {
                            renamed: done.renamed,
                            step,
                        });
                        let gone: HashSet<String> = going.iter().map(|item| item.path.clone()).collect();
                        this.drop_items(at, &gone, cx);
                    }
                    Err(reason) => cx.emit(CleanupEvent::RenameFailed { reason: reason.into() }),
                }
                this.busy = false;
                cx.notify();
            });
        })
        .detach();
    }

    // ---- Tools -------------------------------------------------------------------------------------------

    /// Looks for every tool (`refreshTools`).
    pub fn refresh_tools(&mut self, cx: &mut Context<Self>) {
        if self.tools_loading {
            return;
        }
        self.tools_loading = true;
        self.tools_error = None;
        cx.notify();
        let asked = (self.source.tools)();
        cx.spawn(async move |this, cx| {
            let answer = core_bridge::run(asked).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                this.tools_loading = false;
                match answer {
                    Ok(tools) => {
                        this.tools = tools;
                        this.tools_known = true;
                    }
                    Err(reason) => this.tools_error = Some(reason.into()),
                }
                cx.notify();
                this.ensure_scan(cx);
            });
        })
        .detach();
    }

    /// Installs a tool, showing how far it has got (`installTool`). Once it is there, what waited for it looks.
    pub fn install_tool(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.installing.contains_key(id) {
            return;
        }
        let name: SharedString = self
            .tool(id)
            .map_or_else(|| id.to_string(), |tool| tool.name.clone())
            .into();
        self.installing.insert(
            id.to_string(),
            describe_progress(&Progress {
                stage: t!("Starting").to_string(),
                ..Default::default()
            }),
        );
        cx.notify();
        let (progress, heard) = async_channel::unbounded();
        let asked = (self.source.install)(id.to_string(), progress);
        let watching = id.to_string();
        cx.spawn(async move |this, cx| {
            while let Ok(step) = heard.recv().await {
                let watching = watching.clone();
                let shown = this.update(cx, |this, cx| {
                    this.installing.insert(watching, describe_progress(&step));
                    cx.notify();
                });
                if shown.is_err() {
                    return;
                }
            }
        })
        .detach();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let answer = core_bridge::run(asked).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                this.installing.remove(&id);
                match answer {
                    Ok(()) => {
                        this.tools_loading = false;
                        this.refresh_tools(cx);
                        cx.emit(CleanupEvent::Installed { name });
                    }
                    Err(reason) if reason == "cancelled" => {}
                    Err(reason) => cx.emit(CleanupEvent::InstallFailed {
                        name,
                        reason: reason.into(),
                    }),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Stops the install of `id` (`cancelInstall`).
    pub fn cancel_install(&self, id: &str) {
        (self.source.cancel_install)(id.to_string());
    }
}

// ---- Telling the person ----------------------------------------------------------------------------------

/// The notification that offers Undo for extensions given.
struct GaveExtensionsToast;

/// How long the Undo offer stays, as the web app's toasts.
const UNDO_FOR: Duration = Duration::from_secs(8);

/// Keeps how to put the names back, for Ctrl+Z (`rememberUndo(`extensions of ${what}`, ...)`): the explorer's
/// step holds the pairs of core's `undo_ops` and runs them through the all-or-nothing renamer. `None` when
/// nothing was renamed (no way back) or there is no explorer.
fn remember_extensions(renamed: usize, undo_ops: &[RenamePair], cx: &mut App) -> Option<UndoId> {
    if undo_ops.is_empty() {
        return None;
    }
    let explorer = explorer::try_explorer(cx)?;
    let pairs = undo_ops
        .iter()
        .map(|pair| BatchPair {
            from: pair.from.clone(),
            to: pair.to.clone(),
        })
        .collect();
    let label = t!("extensions of {what}", what = plural(renamed, "file")).to_string();
    Some(explorer.update(cx, |explorer, cx| {
        explorer.remember_undo(label, UndoAction::RenameBatch(pairs), cx)
    }))
}

/// Says extensions were given, with an Undo button that is the explorer's step (it renames the files back,
/// says how it went and reads their folders again).
fn gave_extensions(renamed: usize, step: Option<UndoId>, window: &mut Window, cx: &mut App) {
    let what = plural(renamed, "file");
    let title = if renamed == 1 {
        t!("Gave {what} its real extension", what = what)
    } else {
        t!("Gave {what} their real extension", what = what)
    };
    let mut note = Notification::success(title).id::<GaveExtensionsToast>();
    let Some(step) = step else {
        window.push_notification(note, cx);
        return;
    };
    note = note.action(move |_, _, _| {
        Button::new("cleanup-undo")
            .label(t!("Undo"))
            .on_click(move |_, window, cx| {
                window.remove_notification::<GaveExtensionsToast>(cx);
                if let Some(explorer) = explorer::try_explorer(cx) {
                    explorer.update(cx, |explorer, cx| explorer.undo(Some(step), cx).detach());
                }
            })
    });
    window.push_notification(note, cx);
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(UNDO_FOR).await;
        let _ = handle.update(cx, |_, window, cx| {
            window.remove_notification::<GaveExtensionsToast>(cx)
        });
    })
    .detach();
}

// ---- The dialog ------------------------------------------------------------------------------------------------

/// A line of the results is this tall (a picture's thumbnail fits).
const ROW: Rems = gpui_kit::rems(3.);
/// A thumbnail's square, CSS pixels.
const THUMB: f32 = 40.;
/// Lines past the ones on screen whose thumbnails are asked for early.
const OVERSCAN: usize = 8;
/// The share of the window's height the dialog may take, and what the rest of it needs, in rems.
const OF_WINDOW: f32 = 0.88;
const OTHER_REMS: f32 = 17.;

/// Whether a checkbox's square is empty, dashed or ticked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    Off,
    Some,
    All,
}

/// A checkbox's square, drawn: the line it sits in is the control.
fn box_mark(mark: Mark, t: &theme::Tokens) -> Div {
    let on = mark != Mark::Off;
    div()
        .flex_none()
        .size(rems(0.875))
        .rounded(theme::size::RADIUS_SM)
        .border_1()
        .border_color(if on { t.primary } else { t.input })
        .bg(if on { t.primary } else { t.field })
        .flex()
        .items_center()
        .justify_center()
        .when(mark == Mark::All, |square| {
            square.child(icon(IconName::Check).size(rems(0.625)).text_color(t.primary_foreground))
        })
        .when(mark == Mark::Some, |square| {
            square.child(icon(IconName::Minus).size(rems(0.625)).text_color(t.primary_foreground))
        })
}

/// What the view reads from the model each frame.
struct Snap {
    kind: Kind,
    root: String,
    options: Options,
    busy: bool,
    outcome: Option<Outcome>,
    chosen: Rc<HashSet<String>>,
    ready: bool,
    missing: Vec<Need>,
    tools_known: bool,
    tools_error: Option<SharedString>,
}

/// What picking an item does: its place in the group.
type PickFn = dyn Fn(usize, &mut Window, &mut App);

/// A group where one item is picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    /// A vertical list of tabs (the kinds): Up and Down move and pick.
    Tab,
}

/// One item of a group where one is picked: the item is the control (focusable, with its role, name and state),
/// so a screen reader hears "Big files, tab, selected". Add its content with `.child(..)`. Enter and Space pick
/// it; the arrows move to a neighbour and pick it, as native tabs do. Only the picked item is a tab stop, so Tab
/// passes through the group in one stop.
#[allow(clippy::too_many_arguments)] // Each is a different thing a control is made of; a struct would only rename them.
fn pick_item(
    id: impl Into<ElementId>,
    pick: Pick,
    label: SharedString,
    on: bool,
    at: usize,
    handles: Rc<Vec<FocusHandle>>,
    t: &theme::Tokens,
    on_pick: Rc<PickFn>,
) -> impl ParentElement + Styled + IntoElement {
    // A group has a handle for each of its items; without one the item is simply not focusable.
    let focus = handles.get(at).cloned();
    let click_pick = on_pick.clone();
    let click_focus = focus.clone();
    let ring = t.ring;
    let item = row_start()
        .id(id)
        .when_some(focus, |item, focus| item.track_focus(&focus.tab_stop(on)))
        .role(Role::Tab)
        .aria_label(label)
        .aria_selected(on)
        .test_support()
        .cursor_pointer()
        .gap(space(2.))
        .px(space(2.))
        .rounded(theme::size::RADIUS)
        .border_1()
        .focus_visible(move |style| style.border_color(ring))
        .on_click(move |_, window, cx| {
            if let Some(focus) = &click_focus {
                window.focus(focus, cx);
            }
            if !on {
                click_pick(at, window, cx);
            }
        })
        .on_key_down(move |event, window, cx| {
            let to = match (pick, event.keystroke.key.as_str()) {
                (_, "enter" | "space") => Some(at),
                (Pick::Tab, "down") => at.checked_add(1),
                (Pick::Tab, "up") => at.checked_sub(1),
                _ => None,
            };
            let Some(to) = to.filter(|&to| to < handles.len()) else {
                return;
            };
            if to != at {
                if let Some(next) = handles.get(to) {
                    window.focus(next, cx);
                }
            }
            if to != at || !on {
                on_pick(to, window, cx);
            }
            cx.stop_propagation();
        });
    match pick {
        Pick::Tab => item
            .w_full()
            .h(rems(1.75))
            .border_color(t.background.opacity(0.))
            .text_size(theme::size::XS)
            .font_weight(FontWeight::MEDIUM)
            .when(on, |item| item.bg(t.background).text_color(t.primary))
            .when(!on, |item| {
                item.text_color(t.muted_foreground)
                    .hover(|item| item.bg(t.foreground.opacity(0.05)).text_color(t.foreground))
            }),
    }
}

/// What the lines of one frame are drawn from.
struct Frame {
    kind: Kind,
    root: String,
    chosen: Rc<HashSet<String>>,
    ready: Rc<Ready>,
    /// The results have the keyboard.
    focused: bool,
    /// Unix milliseconds.
    now: i64,
    t: theme::Tokens,
    mono: SharedString,
}

/// The dialog: kinds at the start, the results beside them, the helpers and the big button under them.
pub struct CleanupDialog {
    model: Entity<Cleanup>,
    thumbs: Entity<Thumbnails>,
    viewer: EntityId,
    focus: FocusHandle,
    /// The dialog's content as a whole: where focus goes when the control that had it goes away.
    root_focus: FocusHandle,
    /// One for each kind in the list, in `Kind::ALL` order (the arrow keys move between them).
    tab_focus: Rc<Vec<FocusHandle>>,
    /// The option boxes: "At least" (Big files), "Match" (look-alikes), "Compare" (music).
    at_least: Entity<SelectState<Vec<SharedString>>>,
    alike: Entity<SelectState<Vec<SharedString>>>,
    compare: Entity<SelectState<Vec<SharedString>>>,
    scroll: UniformListScrollHandle,
    /// The line the keyboard is on.
    cursor: usize,
    /// What the thumbnail service was last told is on screen.
    shown: u64,
    _subscriptions: Vec<Subscription>,
}

impl CleanupDialog {
    /// The dialog over `model`.
    pub fn new(model: Entity<Cleanup>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // F5 is the Refresh button's key, as in the web app's dialogs.
        super::refresh_on_f5(cx, |this, window, cx| {
            {
                this.park_focus(window, cx);
                this.model.update(cx, |model, cx| model.refresh(cx));
            };
        });
        let thumbs = thumbnail::thumbnails(cx);
        let viewer = cx.entity_id();
        let service = thumbs.clone();
        let options = model.read(cx).options();
        let at_least = cx.new(|cx| SelectState::new(size_titles(), size_at(options.at_least), window, cx));
        let alike = cx.new(|cx| SelectState::new(alike_titles(), Some(IndexPath::new(options.alike)), window, cx));
        let compare = cx.new(|cx| {
            SelectState::new(
                compare_titles(),
                Some(IndexPath::new(usize::from(options.by_sound))),
                window,
                cx,
            )
        });
        let subscriptions = vec![
            cx.subscribe(&at_least, |this, _, event: &SelectEvent<Vec<SharedString>>, cx| {
                if let SelectEvent::Confirm(Some(title)) = event {
                    this.pick_size(title, cx);
                }
            }),
            cx.subscribe(&alike, |this, _, event: &SelectEvent<Vec<SharedString>>, cx| {
                if let SelectEvent::Confirm(Some(title)) = event {
                    this.pick_alike(title, cx);
                }
            }),
            cx.subscribe(&compare, |this, _, event: &SelectEvent<Vec<SharedString>>, cx| {
                if let SelectEvent::Confirm(Some(title)) = event {
                    this.pick_compare(title, cx);
                }
            }),
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.observe(&thumbs, |_, _, cx| cx.notify()),
            cx.subscribe_in(&model, window, |this, _, event: &CleanupEvent, window, cx| {
                this.say(event, window, cx)
            }),
            // Closing the dialog lets go of its pictures.
            cx.on_release(move |_, cx| {
                service.update(cx, |thumbs, cx| {
                    thumbs.release(viewer, cx);
                    thumbs.forget_lane(LANE, cx);
                })
            }),
        ];
        CleanupDialog {
            model,
            thumbs,
            viewer,
            focus: cx.focus_handle().tab_stop(true),
            root_focus: cx.focus_handle(),
            tab_focus: Rc::new(Kind::ALL.iter().map(|_| cx.focus_handle()).collect()),
            at_least,
            alike,
            compare,
            scroll: UniformListScrollHandle::new(),
            cursor: 0,
            shown: 0,
            _subscriptions: subscriptions,
        }
    }

    /// The model.
    pub fn model(&self) -> &Entity<Cleanup> {
        &self.model
    }

    /// The results' focus handle (the keyboard's one tab stop).
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The line the keyboard is on.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// What the notifications say (the page's toasts).
    fn say(&mut self, event: &CleanupEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            CleanupEvent::RecycleFailed { folders, reason } => window.push_notification(
                Notification::error(reason.clone()).title(if *folders {
                    t!("Couldn't recycle the folders")
                } else {
                    t!("Couldn't recycle the files")
                }),
                cx,
            ),
            CleanupEvent::GaveExtensions { renamed, step } => gave_extensions(*renamed, *step, window, cx),
            CleanupEvent::RenameFailed { reason } => window.push_notification(
                Notification::error(reason.clone()).title(t!("Couldn't rename them")),
                cx,
            ),
            CleanupEvent::Installed { name } => window.push_notification(
                Notification::success("").title(t!("{name} is installed", name = name)),
                cx,
            ),
            CleanupEvent::InstallFailed { name, reason } => window.push_notification(
                Notification::error(reason.clone()).title(t!("Couldn't install {name}", name = name)),
                cx,
            ),
        }
    }

    fn snapshot(&self, cx: &App) -> Snap {
        let model = self.model.read(cx);
        let kind = model.kind();
        Snap {
            kind,
            root: model.root().to_string(),
            options: model.options(),
            busy: model.busy(),
            outcome: model.outcome(kind).cloned(),
            chosen: model.chosen(),
            ready: model.ready(kind),
            missing: model.missing(kind),
            tools_known: model.tools_known(),
            tools_error: model.tools_error().cloned(),
        }
    }

    /// Shows the item at line `ix` in its folder, closing the dialog first (`revealInApp`).
    fn reveal(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let path = {
            let model = self.model.read(cx);
            match model.outcome(model.kind()) {
                Some(Outcome::Ready(ready)) => match ready.lines.get(ix) {
                    Some(Line::Row { item, .. }) => ready.scan.items.get(*item).map(|found| found.path.clone()),
                    _ => None,
                },
                _ => None,
            }
        };
        let Some(path) = path else {
            return;
        };
        window.close_dialog(cx);
        if let Some(explorer) = explorer::try_explorer(cx) {
            explorer.update(cx, |explorer, cx| {
                explorer.open_requested(
                    vec![LaunchTarget {
                        path: parent_path(&path),
                        select: Some(path.clone()),
                    }],
                    cx,
                );
            });
        }
    }

    /// A button that is about to go away or be disabled (Stop, Recycle, None…) hands focus to the dialog's
    /// content first: focus lost to nowhere would leave Escape without a dialog to close.
    fn park_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.root_focus, cx);
    }

    /// Moves the keyboard to line `ix` (clamped), keeping it in view.
    fn move_cursor(&mut self, to: usize, lines: usize, cx: &mut Context<Self>) {
        self.cursor = to.min(lines.saturating_sub(1));
        self.scroll
            .scroll_to_item(self.cursor, gpui_kit::ScrollStrategy::Nearest);
        cx.notify();
    }

    fn line_count(&self, cx: &App) -> usize {
        let model = self.model.read(cx);
        match model.outcome(model.kind()) {
            Some(Outcome::Ready(ready)) => ready.lines.len(),
            _ => 0,
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lines = self.line_count(cx);
        if lines == 0 || event.keystroke.modifiers.modified() {
            return;
        }
        let at = self.cursor.min(lines - 1);
        let to = match event.keystroke.key.as_str() {
            "down" => at + 1,
            "up" => at.saturating_sub(1),
            "pagedown" => at + 8,
            "pageup" => at.saturating_sub(8),
            "home" => 0,
            "end" => lines - 1,
            "space" => {
                self.model.update(cx, |model, cx| model.toggle_line(at, cx));
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        self.move_cursor(to, lines, cx);
        cx.stop_propagation();
    }

    /// Tells the thumbnail service which pictures the lines on screen (and a few past them) need, if that
    /// changed, and frees what it let go of from the GPU's atlas.
    fn show_thumbnails(&mut self, ready: &Ready, kind: Kind, range: &Range<usize>, window: &mut Window, cx: &mut App) {
        // The list measures one line first: telling the service about that would make it drop the real ones.
        if *range == (0..1) && ready.lines.len() > 1 {
            return;
        }
        let from = range.start.saturating_sub(OVERSCAN);
        let to = range.end.saturating_add(OVERSCAN).min(ready.lines.len());
        let scale = window.scale_factor();
        let mut wants: Vec<Want> = Vec::new();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        scale.to_bits().hash(&mut hasher);
        if !kind.folders() {
            for line in ready.lines.get(from..to).unwrap_or_default() {
                let Line::Row { item, .. } = line else {
                    continue;
                };
                let Some(found) = ready.scan.items.get(*item) else {
                    continue;
                };
                let entry = entry_of(kind, found);
                if !is_picture_or_video(&entry) {
                    continue;
                }
                (&entry.path, entry.modified, entry.size).hash(&mut hasher);
                if let Some(want) = Want::for_entry(&entry, THUMB, scale) {
                    wants.push(want);
                }
            }
        }
        let fingerprint = hasher.finish();
        if self.shown != fingerprint {
            self.shown = fingerprint;
            let viewer = self.viewer;
            self.thumbs
                .update(cx, |thumbs, cx| thumbs.show(viewer, LANE, wants, cx));
        }
        for freed in self.thumbs.update(cx, |thumbs, _| thumbs.take_evicted()) {
            window.drop_image(freed).ok();
        }
    }

    // ---- Drawing the parts -----------------------------------------------------------------------------

    /// The path, the badge for what was found, and Refresh (the page's `ModalHeader` minus its title).
    fn header(&self, snap: &Snap, t: &theme::Tokens, mono: &SharedString, cx: &mut Context<Self>) -> Div {
        let badge = match &snap.outcome {
            Some(Outcome::Ready(ready))
                if !ready.scan.items.is_empty()
                    && (snap.kind == Kind::Empty || ready.scan.total > 0 || !snap.kind.needs().is_empty()) =>
            {
                Some(snap.kind.badge(&ready.scan))
            }
            _ => None,
        };
        let refreshing = matches!(snap.outcome, Some(Outcome::Loading)) || snap.busy;
        row_start()
            .gap(space(2.))
            .child(
                icon(IconName::BrushCleaning)
                    .size_4()
                    .flex_none()
                    .text_color(t.muted_foreground),
            )
            .child(
                div()
                    .id("cleanup-root")
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_start()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .font_family(mono.clone())
                    .child(snap.root.clone()),
            )
            .children(badge.map(|badge| {
                div()
                    .id("cleanup-badge")
                    .role(Role::Status)
                    .aria_label(badge.clone())
                    .test_support()
                    .flex_none()
                    .rounded(theme::size::RADIUS)
                    .bg(t.warning.opacity(0.15))
                    .px(space(2.))
                    .py(space(0.5))
                    .text_size(theme::size::XS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.warning)
                    .child(badge)
            }))
            .child(
                Button::new("cleanup-refresh")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .accessibility_label(t!("Refresh"))
                    .tooltip(t!("Refresh (F5)"))
                    .loading(refreshing)
                    .disabled(!snap.ready)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.park_focus(window, cx);
                        this.model.update(cx, |model, cx| model.refresh(cx));
                    })),
            )
    }

    /// The kinds, in their two sections.
    fn nav(&self, snap: &Snap, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let mut nav = col()
            .id("cleanup-nav")
            .role(Role::Navigation)
            .aria_label(t!("What to look for"))
            .test_support()
            .w(rems(12.))
            .flex_none()
            .gap(space(3.))
            .p(space(2.))
            .bg(t.chrome.opacity(0.6))
            .overflow_y_scrollbar();
        nav = if i18n::is_rtl() {
            nav.border_l_1()
        } else {
            nav.border_r_1()
        };
        nav = nav.border_color(t.border);
        for (s, (title, kinds)) in Kind::section_list().into_iter().enumerate() {
            let mut section = col()
                .id(("cleanup-section", s))
                .role(Role::TabList)
                .aria_label(title.clone())
                .aria_orientation(Orientation::Vertical)
                .test_support()
                .gap(space(0.5))
                .child(
                    div()
                        .px(space(2.))
                        .pb(space(0.5))
                        .text_size(theme::size::XXS)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.muted_foreground)
                        .child(title.to_uppercase()),
                );
            for kind in kinds {
                let label = kind.label();
                let count = model.found_count(kind).filter(|&n| n > 0);
                let ix = Kind::ALL.iter().position(|&k| k == kind).unwrap_or(0);
                let model = self.model.clone();
                let on = kind == snap.kind;
                section = section.child(
                    pick_item(
                        ("cleanup-kind", ix),
                        Pick::Tab,
                        label.clone(),
                        on,
                        ix,
                        self.tab_focus.clone(),
                        t,
                        Rc::new(move |to, _, cx| {
                            if let Some(&kind) = Kind::ALL.get(to) {
                                model.update(cx, |model, cx| model.select_kind(kind, cx));
                            }
                        }),
                    )
                    .child(icon(kind.icon()).size_3p5().flex_none())
                    .child(div().flex_1().min_w_0().truncate().text_start().child(label))
                    .children(count.map(|n| {
                        div()
                            .flex_none()
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(format_count(n))
                    })),
                );
            }
            nav = nav.child(section);
        }
        nav
    }

    /// One of the page's `select` boxes: its label, then the drop-down.
    fn select_row(
        id: &'static str,
        label: SharedString,
        state: &Entity<SelectState<Vec<SharedString>>>,
        t: &theme::Tokens,
    ) -> impl IntoElement {
        row_start()
            .id(id)
            .test_support()
            .flex_none()
            .items_center()
            .gap(space(1.5))
            .text_size(theme::size::XS)
            .child(div().text_color(t.muted_foreground).child(label.clone()))
            .child(
                div()
                    .w(space(44.))
                    .child(Select::new(state).id((id, 0_usize)).small().accessibility_label(label)),
            )
    }

    /// "At least" was picked: the scan looks again with the new size.
    fn pick_size(&mut self, title: &SharedString, cx: &mut Context<Self>) {
        let Some(&size) = size_titles().iter().position(|t| t == title).and_then(|i| SIZES.get(i)) else {
            return;
        };
        if self.model.read(cx).options().at_least != size {
            self.model
                .update(cx, |model, cx| model.change_options(|o| o.at_least = size, cx));
        }
    }

    /// "Match" was picked.
    fn pick_alike(&mut self, title: &SharedString, cx: &mut Context<Self>) {
        let Some(level) = alike_titles().iter().position(|t| t == title) else {
            return;
        };
        if self.model.read(cx).options().alike != level {
            self.model
                .update(cx, |model, cx| model.change_options(|o| o.alike = level, cx));
        }
    }

    /// "Compare" was picked: tags, or how the songs sound.
    fn pick_compare(&mut self, title: &SharedString, cx: &mut Context<Self>) {
        let Some(sound) = compare_titles().iter().position(|t| t == title).map(|i| i == 1) else {
            return;
        };
        if self.model.read(cx).options().by_sound != sound {
            self.model
                .update(cx, |model, cx| model.change_options(|o| o.by_sound = sound, cx));
        }
    }

    /// Shows the boxes' choices as the options in force are (the model is the truth; a language change also
    /// re-words the titles).
    fn sync_selects(&mut self, options: &Options, window: &mut Window, cx: &mut Context<Self>) {
        fn show(
            state: &Entity<SelectState<Vec<SharedString>>>,
            titles: Vec<SharedString>,
            wanted: Option<IndexPath>,
            window: &mut Window,
            cx: &mut Context<CleanupDialog>,
        ) {
            let (words_changed, shown) = {
                let read = state.read(cx);
                let same = read
                    .selected_value()
                    .is_none_or(|title| titles.iter().any(|t| t == title));
                (!same, read.selected_index(cx))
            };
            if words_changed {
                state.update(cx, |state, cx| state.set_items(titles, window, cx));
            }
            if words_changed || shown != wanted {
                state.update(cx, |state, cx| state.set_selected_index(wanted, window, cx));
            }
        }
        show(&self.at_least, size_titles(), size_at(options.at_least), window, cx);
        show(
            &self.alike,
            alike_titles(),
            Some(IndexPath::new(options.alike)),
            window,
            cx,
        );
        show(
            &self.compare,
            compare_titles(),
            Some(IndexPath::new(usize::from(options.by_sound))),
            window,
            cx,
        );
    }

    /// What the kind looks for, and the options that shape it.
    fn options_bar(&self, snap: &Snap, t: &theme::Tokens) -> impl IntoElement {
        let picker = match snap.kind {
            Kind::Big => {
                Some(Self::select_row("cleanup-at-least", t!("At least"), &self.at_least, t).into_any_element())
            }
            Kind::Images | Kind::Videos => {
                Some(Self::select_row("cleanup-match", t!("Match"), &self.alike, t).into_any_element())
            }
            Kind::Music => {
                Some(Self::select_row("cleanup-compare", t!("Compare"), &self.compare, t).into_any_element())
            }
            _ => None,
        };
        row_start()
            .gap(space(3.))
            .px(space(3.5))
            .py(space(2.))
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_start()
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(snap.kind.about()),
            )
            .children(picker)
    }

    /// A centred message with an icon, for the states that are not a list.
    fn notice(id: &'static str, role: Role, label: SharedString) -> impl ParentElement + Styled + IntoElement {
        col()
            .id(id)
            .role(role)
            .aria_label(label)
            .test_support()
            .w_full()
            .items_center()
            .justify_center()
            .gap(space(2.))
            .py(space(12.))
            .text_center()
            .text_size(theme::size::SM)
    }

    /// The card that says a kind needs a tool and installs it with one click, with its progress and Stop
    /// (`ToolNeeded`).
    fn tool_card(
        &self,
        n: usize,
        need: &Need,
        kind: Kind,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let model = self.model.read(cx);
        let tool = model.tool(need.id)?;
        if tool.installed {
            return None;
        }
        let id = need.id;
        let controls: AnyElement = match model.installing(id) {
            Some(step) => {
                let step = step.clone();
                row_start()
                    .gap(space(3.))
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(step),
                    )
                    .child(
                        Button::new(("cleanup-install-stop", n))
                            .outline()
                            .small()
                            .label(t!("Stop"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.park_focus(window, cx);
                                this.model.read(cx).cancel_install(id);
                            })),
                    )
                    .into_any_element()
            }
            None => Button::new(("cleanup-install", n))
                .primary()
                .small()
                .icon(IconName::Download)
                .label(t!("Install {name}", name = tool.name))
                .tooltip(install_hint(tool))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.park_focus(window, cx);
                    this.model.update(cx, |model, cx| model.install_tool(id, cx));
                }))
                .into_any_element(),
        };
        Some(
            row_start()
                .id(("cleanup-needed", n))
                .role(Role::Group)
                .aria_label(need.why.clone())
                .test_support()
                .gap(space(3.))
                .px(space(4.))
                .py(space(3.))
                .rounded(theme::size::RADIUS_LG)
                .border_1()
                .border_color(t.border)
                .bg(t.primary.opacity(0.05))
                .child(icon(kind.icon()).size_4().flex_none().text_color(t.primary))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_start()
                        .text_size(theme::size::SM)
                        .child(need.why.clone()),
                )
                .child(controls)
                .into_any_element(),
        )
    }

    /// One line of the results.
    fn line(&self, ix: usize, line: Line, frame: &Frame, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let (t, ready, mono) = (&frame.t, &*frame.ready, &frame.mono);
        let (focused, now) = (frame.focused, frame.now);
        let on_cursor = focused && ix == self.cursor;
        match line {
            Line::Group(g) => {
                let Some(group) = ready.groups.get(g) else {
                    return div().into_any_element();
                };
                let chosen = group
                    .items
                    .iter()
                    .filter_map(|&item| ready.scan.items.get(item))
                    .filter(|item| frame.chosen.contains(&item.path))
                    .count();
                let mark = if chosen == 0 {
                    Mark::Off
                } else if chosen == group.items.len() {
                    Mark::All
                } else {
                    Mark::Some
                };
                let title = group.title.clone();
                row_start()
                    .id(("cleanup-line", ix))
                    .role(Role::CheckBox)
                    .aria_label(title.clone())
                    .aria_toggled(match mark {
                        Mark::Off => Toggled::False,
                        Mark::Some => Toggled::Mixed,
                        Mark::All => Toggled::True,
                    })
                    .test_support()
                    .h(ROW)
                    .px(space(3.))
                    .gap(space(2.))
                    .cursor_pointer()
                    .border_b_1()
                    .border_color(t.border)
                    .bg(if on_cursor { t.accent } else { t.muted.opacity(0.4) })
                    .text_size(theme::size::XS)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus, cx);
                        this.cursor = ix;
                        this.model.update(cx, |model, cx| model.toggle_line(ix, cx));
                    }))
                    .child(box_mark(mark, t))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(isolate(title)),
                    )
                    .when(!frame.kind.grouped(), |line| {
                        line.child(div().flex_none().text_color(t.muted_foreground).child(format!(
                            "· {}",
                            plural(group.items.len(), if frame.kind.folders() { "folder" } else { "file" })
                        )))
                    })
                    .when(group.size > 0, |line| {
                        line.child(
                            div()
                                .flex_1()
                                .text_end()
                                .text_color(t.warning)
                                .font_weight(FontWeight::MEDIUM)
                                .child(format_size(group.size)),
                        )
                    })
                    .into_any_element()
            }
            Line::Row { item, .. } => {
                let Some(found) = ready.scan.items.get(item) else {
                    return div().into_any_element();
                };
                let on = frame.chosen.contains(&found.path);
                let shown_path = in_root(&frame.root, &found.path);
                let entry = entry_of(frame.kind, found);
                let kind = kind_of(&entry);
                let tone = theme::kind_tone(kind, cx);
                let picture = !entry.dir && is_picture_or_video(&entry);
                let mark = if picture {
                    let look = self.thumbs.read(cx).look(&entry, THUMB, window.scale_factor());
                    let look = if matches!(look, Look::Icon) { Look::Icon } else { look };
                    div()
                        .flex_none()
                        .size(px(THUMB))
                        .overflow_hidden()
                        .rounded(theme::size::RADIUS_SM)
                        .child(thumbnail::thumbnail_square(&look, kind, THUMB, 18., false, cx))
                        .into_any_element()
                } else {
                    icon(kind_icon(kind.icon))
                        .size(rems(0.875))
                        .flex_none()
                        .text_color(tone.color)
                        .into_any_element()
                };
                let date = (found.date > 0).then(|| {
                    // Developer junk of a project untouched for a month is quieter: it is the safer to clear.
                    let old = frame.kind == Kind::Junk && found.date < now - 30 * DAY;
                    let tip = frame.kind.dated();
                    div()
                        .id(("cleanup-date", ix))
                        .flex_none()
                        .text_size(theme::size::XXS)
                        .text_color(if old { t.muted_foreground } else { t.foreground })
                        .tooltip(move |window, cx| tooltip(tip.clone(), window, cx))
                        .child(format_date(found.date, None, i18n::language()))
                });
                let full_path = SharedString::from(found.path.clone());
                let note = (!found.note.is_empty()).then(|| {
                    let full_note = SharedString::from(found.note.clone());
                    div()
                        .id(("cleanup-note", ix))
                        .truncate()
                        .text_size(theme::size::XXS)
                        .text_color(t.muted_foreground)
                        .tooltip(move |window, cx| tooltip(full_note.clone(), window, cx))
                        .child(isolate(found.note.clone()))
                });
                row_start()
                    .id(("cleanup-line", ix))
                    .role(Role::CheckBox)
                    .aria_label(shown_path.clone())
                    .aria_toggled(if on { Toggled::True } else { Toggled::False })
                    .test_support()
                    .h(ROW)
                    .px(space(3.))
                    .gap(space(2.))
                    .cursor_pointer()
                    .border_b_1()
                    .border_color(t.border)
                    .bg(if on_cursor {
                        t.accent
                    } else if on {
                        t.primary.opacity(0.04)
                    } else {
                        t.background.opacity(0.)
                    })
                    .hover(|row| row.bg(t.accent.opacity(0.4)))
                    .text_size(theme::size::XS)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus, cx);
                        this.cursor = ix;
                        this.model.update(cx, |model, cx| model.toggle_line(ix, cx));
                    }))
                    .child(box_mark(if on { Mark::All } else { Mark::Off }, t))
                    .child(mark)
                    .child(
                        col()
                            .flex_1()
                            .min_w_0()
                            .align_start()
                            .child(
                                div()
                                    .id(("cleanup-path", ix))
                                    .max_w_full()
                                    .truncate()
                                    .font_family(mono.clone())
                                    .tooltip(move |window, cx| tooltip(full_path.clone(), window, cx))
                                    .child(isolate(shown_path)),
                            )
                            .children(note),
                    )
                    .children(date)
                    .when(found.size > 0 || !frame.kind.folders(), |line| {
                        line.child(div().w(rems(4.)).flex_none().text_end().child(format_size(found.size)))
                    })
                    .child(
                        Button::new(("cleanup-reveal", ix))
                            .ghost()
                            .small()
                            .icon(IconName::FolderInput)
                            .accessibility_label(t!("Show it in its folder"))
                            .tooltip(t!("Show it in its folder"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.reveal(ix, window, cx);
                            })),
                    )
                    .into_any_element()
            }
        }
    }

    /// The results area: a message for each state that isn't a list, else the list.
    fn results(
        &mut self,
        snap: &Snap,
        height: Pixels,
        t: &theme::Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let folder = base_name(&snap.root).to_string();
        let folder = if folder.is_empty() { snap.root.clone() } else { folder };
        let shown_folder = isolate(folder.clone());
        let body = if !snap.ready {
            if let Some(reason) = &snap.tools_error {
                Self::notice(
                    "cleanup-tools-error",
                    Role::Alert,
                    t!("Couldn't look for the tools: {reason}", reason = reason),
                )
                .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                .child(
                    div()
                        .max_w(rems(28.))
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(reason.clone()),
                )
                .into_any_element()
            } else if snap.tools_known {
                col()
                    .id("cleanup-tools")
                    .w_full()
                    .gap(space(2.))
                    .p(space(3.))
                    .children(
                        snap.missing
                            .iter()
                            .enumerate()
                            .filter_map(|(n, need)| self.tool_card(n, need, snap.kind, t, cx)),
                    )
                    .into_any_element()
            } else {
                Self::notice("cleanup-tools-loading", Role::Status, t!("Looking for the tools…"))
                    .child(Spinner::new())
                    .into_any_element()
            }
        } else {
            match &snap.outcome {
                None | Some(Outcome::Loading) => Self::notice(
                    "cleanup-loading",
                    Role::Status,
                    t!("Looking below “{folder}”…", folder = folder),
                )
                .text_color(t.muted_foreground)
                .child(Spinner::new())
                .child(t!("Looking below “{folder}”…", folder = shown_folder))
                .child(
                    Button::new("cleanup-stop")
                        .outline()
                        .small()
                        .icon(IconName::Square)
                        .label(t!("Stop"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.park_focus(window, cx);
                            this.model.read(cx).stop();
                        })),
                )
                .into_any_element(),
                Some(Outcome::Stopped) => {
                    Self::notice("cleanup-stopped", Role::Status, t!("Stopped before it finished"))
                        .text_color(t.muted_foreground)
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.foreground)
                                .child(t!("Stopped before it finished")),
                        )
                        .child(
                            Button::new("cleanup-look-again")
                                .outline()
                                .small()
                                .icon(IconName::RotateCw)
                                .label(t!("Look again"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.park_focus(window, cx);
                                    this.model.update(cx, |model, cx| model.refresh(cx));
                                })),
                        )
                        .into_any_element()
                }
                Some(Outcome::Failed(reason)) => Self::notice(
                    "cleanup-error",
                    Role::Alert,
                    t!(
                        "Couldn't look below “{folder}”: {reason}",
                        folder = folder,
                        reason = reason
                    ),
                )
                .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.foreground)
                        .child(t!("Couldn't look below “{folder}”", folder = shown_folder)),
                )
                .child(
                    div()
                        .max_w(rems(28.))
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(reason.clone()),
                )
                .into_any_element(),
                Some(Outcome::Ready(ready)) if ready.scan.items.is_empty() => {
                    let what = snap.kind.nothing_found(&snap.options);
                    let time = format_duration(ready.scan.elapsed);
                    let held = ready.scan.held;
                    let detail = if held > 0 {
                        t!(
                            "Looked through in {time}; {files} a program has open {was} left out.",
                            time = time,
                            files = plural(held, "file"),
                            was = if held == 1 { t!("was") } else { t!("were") }
                        )
                    } else {
                        t!("Looked through in {time}.", time = time)
                    };
                    Self::notice("cleanup-nothing", Role::Status, what.clone())
                        .text_color(t.muted_foreground)
                        .child(icon(IconName::CircleCheck).size_8().text_color(t.success))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.foreground)
                                .child(what),
                        )
                        .child(div().max_w(rems(24.)).text_size(theme::size::XS).child(detail))
                        .into_any_element()
                }
                Some(Outcome::Ready(ready)) => self.list(ready.clone(), snap, t, window, cx),
            }
        };
        div()
            .id("cleanup-results")
            .relative()
            .w_full()
            .h(height)
            .flex_none()
            .child(body)
            .into_any_element()
    }

    /// The found items, grouped, virtualized.
    fn list(
        &mut self,
        ready: Rc<Ready>,
        snap: &Snap,
        t: &theme::Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lines = ready.lines.len();
        self.cursor = self.cursor.min(lines.saturating_sub(1));
        let focused = self.focus.is_focused(window);
        let mono = theme::mono_font(cx);
        let now = now_ms();
        let frame = Rc::new(Frame {
            kind: snap.kind,
            root: snap.root.clone(),
            chosen: snap.chosen.clone(),
            ready: ready.clone(),
            focused,
            now,
            t: t.clone(),
            mono,
        });
        let list = uniform_list(
            "cleanup-list",
            lines,
            cx.processor(move |this, range: Range<usize>, window, cx| {
                this.show_thumbnails(&frame.ready, frame.kind, &range, window, cx);
                range
                    .filter_map(|ix| {
                        let line = frame.ready.lines.get(ix).copied()?;
                        Some(this.line(ix, line, &frame, window, cx))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();
        div()
            .id("cleanup-found")
            .role(Role::Group)
            .aria_label(t!("Found"))
            .test_support()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .child(list)
            .child(Scrollbar::vertical(&self.scroll).viewport_from_layout())
            .into_any_element()
    }

    /// The summary, the quick ways to choose, None, and the big button.
    fn footer(&self, snap: &Snap, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = match &snap.outcome {
            Some(Outcome::Ready(ready)) if snap.ready && !ready.scan.items.is_empty() => Some(ready.clone()),
            _ => None,
        };
        let summary_text = ready
            .as_ref()
            .map(|ready| summary(snap.kind, &ready.scan))
            .unwrap_or_default();
        let chosen: Vec<&Item> = ready
            .as_ref()
            .map(|ready| {
                ready
                    .scan
                    .items
                    .iter()
                    .filter(|item| snap.chosen.contains(&item.path))
                    .collect()
            })
            .unwrap_or_default();
        let chosen_count = chosen.len();
        let chosen_bytes: u64 = chosen.iter().map(|item| item.size).sum();
        let can_rename = chosen.iter().any(|item| !item.fix.is_empty());
        let actions = ready.is_some().then(|| {
            let helpers = snap.kind.helpers().iter().enumerate().map(|(i, &helper)| {
                Button::new(("cleanup-helper", i))
                    .ghost()
                    .small()
                    .label(helper.label())
                    .tooltip(helper.title())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model
                            .update(cx, |model, cx| model.apply_helper(helper, now_ms(), cx))
                    }))
            });
            let main = if snap.kind.renames() {
                let label = if chosen_count == 0 {
                    t!("Give the real extension")
                } else if chosen_count == 1 {
                    t!(
                        "Give {what} its real extension",
                        what = plural(chosen_count, "file")
                    )
                } else {
                    t!(
                        "Give {what} their real extension",
                        what = plural(chosen_count, "file")
                    )
                };
                Button::new("cleanup-give")
                    .primary()
                    .small()
                    .icon(IconName::PenLine)
                    .label(label)
                    .tooltip(t!(
                        "Rename them to the extension that says what's in them: Ctrl+Z takes it back"
                    ))
                    .loading(snap.busy)
                    .disabled(snap.busy || !can_rename)
                    .on_click(cx.listener(|this, _, window, cx| { this.park_focus(window, cx); this.model.update(cx, |model, cx| model.give_extensions(cx)); }))
            } else {
                let label = if chosen_count == 0 {
                    t!("Recycle")
                } else if chosen_bytes > 0 {
                    t!(
                        "Recycle {count} · {size}",
                        count = format_count(chosen_count),
                        size = format_size(chosen_bytes)
                    )
                } else {
                    t!("Recycle {what}", what = plural(chosen_count, snap.kind.noun()))
                };
                Button::new("cleanup-recycle")
                    .danger()
                    .small()
                    .icon(IconName::Trash)
                    .label(label)
                    .tooltip(t!(
                        "Move them to the Recycle Bin: Ctrl+Z takes it back. Windows asks first about one too big for the Recycle Bin."
                    ))
                    .loading(snap.busy)
                    .disabled(snap.busy || chosen_count == 0)
                    .on_click(cx.listener(|this, _, window, cx| { this.park_focus(window, cx); this.model.update(cx, |model, cx| model.recycle(cx)); }))
            };
            align_row(1.5)
                .flex_none()
                .children(helpers)
                .child(
                    Button::new("cleanup-none")
                        .ghost()
                        .small()
                        .label(t!("None"))
                        .disabled(snap.chosen.is_empty())
                        .on_click(cx.listener(|this, _, window, cx| { this.park_focus(window, cx); this.model.update(cx, |model, cx| model.choose_none(cx)); })),
                )
                .child(main)
        });
        row_start()
            .h(rems(2.5))
            .flex_none()
            .gap(space(2.))
            .px(space(3.5))
            .border_t_1()
            .border_color(t.border)
            .bg(t.chrome)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(
                div()
                    .id("cleanup-summary")
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_start()
                    .when(!summary_text.is_empty(), |line| {
                        let full = SharedString::from(summary_text.clone());
                        line.tooltip(move |window, cx| tooltip(full.clone(), window, cx))
                    })
                    .child(summary_text),
            )
            .children(actions)
    }
}

impl Render for CleanupDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let mono = theme::mono_font(cx);
        let snap = self.snapshot(cx);
        // At most 88% of the window's height, the results filling what the rest of the dialog leaves.
        let rem = window.rem_size();
        let height = (window.viewport_size().height * OF_WINDOW - rem * OTHER_REMS).clamp(rem * 16., rem * 34.);
        let folder = base_name(&snap.root).to_string();
        let folder = if folder.is_empty() { snap.root.clone() } else { folder };

        let header = self.header(&snap, &t, &mono, cx);
        let nav = self.nav(&snap, &t, cx);
        self.sync_selects(&snap.options, window, cx);
        let options = self.options_bar(&snap, &t);
        let results = self.results(&snap, height, &t, window, cx);
        let footer = self.footer(&snap, &t, cx);

        col()
            .id("cleanup")
            .role(Role::Group)
            .aria_label(t!("Clean up {folder}", folder = folder))
            .track_focus(&self.root_focus)
            .test_support()
            // Enter belongs to the dialog's own boxes and buttons: on the results it chooses the line.
            .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                if this.focus.is_focused(window) {
                    let at = this.cursor;
                    this.model.update(cx, |model, cx| model.toggle_line(at, cx));
                } else {
                    cx.propagate();
                }
            }))
            .gap(space(2.))
            .text_size(theme::size::SM)
            // The dialog layer sits outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .child(header)
            .child(
                col()
                    .rounded(theme::size::RADIUS_LG)
                    .border_1()
                    .border_color(t.border)
                    .overflow_hidden()
                    .child(
                        row_start()
                            .items_stretch()
                            .child(nav)
                            .child(col().flex_1().min_w_0().child(options).child(results)),
                    )
                    .child(footer),
            )
    }
}

// ---- Opening it ------------------------------------------------------------------------------------------

/// Opens Clean up for the folder `root` on the kind picked last: what can go to free space below it
/// (`openCleanup`, for `MenuEvent::Cleanup`). Nothing opens for no folder.
pub fn open(window: &mut Window, cx: &mut App, root: &str) {
    open_kind(window, cx, root, None);
}

/// [`open`] on `kind`, or the one picked last.
pub fn open_kind(window: &mut Window, cx: &mut App, root: &str, kind: Option<Kind>) {
    let app = cx.global::<core_bridge::Core>().app.clone();
    open_with(window, cx, root, kind, core_source(app));
}

/// [`open_kind`] with a [`Source`] of your own (tests pass fakes). Returns the dialog's view.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    root: &str,
    kind: Option<Kind>,
    source: Source,
) -> Option<Entity<CleanupDialog>> {
    if root.is_empty() {
        return None;
    }
    let kind = kind.unwrap_or_else(|| cx.try_global::<LastKind>().map_or(Kind::Junk, |last| last.0));
    cx.set_global(LastKind(kind));
    let root = root.to_string();
    let title_folder = base_name(&root).to_string();
    let title_folder = if title_folder.is_empty() {
        root.clone()
    } else {
        title_folder
    };
    let model = cx.new(|cx| Cleanup::new(root, kind, source, cx));
    let view = cx.new(|cx| CleanupDialog::new(model, window, cx));
    super::show(
        window,
        cx,
        t!("Clean up “{folder}”", folder = isolate(title_folder)),
        px(960.),
        view.clone(),
    );
    Some(view)
}

// ---- Tests ------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::file_grid::thumbnail::NoThumbs;
    use crate::explorer::fake::{folder, FakeDisk};
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, MutexGuard};

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(|e| e.into_inner())
    }

    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;

    fn item(path: &str, size: u64, date: i64, kind: &str) -> Item {
        Item {
            path: path.into(),
            size,
            date,
            kind: kind.into(),
            ..Default::default()
        }
    }

    fn scan_of(items: Vec<Item>) -> Scan {
        let total = items.iter().map(|i| i.size).sum();
        Scan {
            items,
            total,
            engine: "index + fd".into(),
            elapsed: 12.0,
            ..Default::default()
        }
    }

    fn junk_scan() -> Scan {
        scan_of(vec![
            item("C:\\Repos\\app\\target", 2 * GB, 1_000, "Rust builds"),
            item("C:\\Repos\\app\\node_modules", 300 * MB, 2_000, "Node packages"),
            item("C:\\Repos\\app2\\node_modules", 100 * MB, 3_000, "Node packages"),
        ])
    }

    /// A look-alike picture of group `group`.
    fn like(path: &str, group: u32, size: u64) -> Item {
        Item {
            path: path.into(),
            group,
            size,
            pixels: size,
            ..Default::default()
        }
    }

    // ---- The fake source ---------------------------------------------------------------------------

    #[derive(Clone)]
    enum Answer {
        Found(Scan),
        Fail(String),
        /// Never answers until the scan is stopped (then `"cancelled"`).
        Hold,
    }

    fn kind_of_mode(mode: Mode) -> Kind {
        match mode {
            Mode::Junk => Kind::Junk,
            Mode::Empty => Kind::Empty,
            Mode::Temporary => Kind::Temporary,
            Mode::Big { .. } => Kind::Big,
            Mode::Images { .. } => Kind::Images,
            Mode::Videos { .. } => Kind::Videos,
            Mode::Music { .. } => Kind::Music,
            Mode::Broken => Kind::Broken,
            Mode::Misnamed => Kind::Misnamed,
        }
    }

    #[derive(Default)]
    struct Fake {
        answers: Mutex<HashMap<Kind, Answer>>,
        scans: Mutex<Vec<(String, Mode, String)>>,
        held: Mutex<Option<async_channel::Sender<Result<Scan, String>>>>,
        cancels: AtomicUsize,
        renames: Mutex<Vec<Vec<RenamePair>>>,
        gone: Mutex<Vec<String>>,
        tools: Mutex<Vec<ToolState>>,
        installs: Mutex<Vec<String>>,
    }

    fn tool(id: &str, name: &str, installed: bool) -> ToolState {
        ToolState {
            id: id.into(),
            name: name.into(),
            installed,
            source: "winget".into(),
            pinned: "1.0".into(),
        }
    }

    impl Fake {
        fn new() -> Arc<Fake> {
            let fake = Fake::default();
            *lock(&fake.tools) = vec![tool("czkawka", "Czkawka", true), tool("ffmpeg", "FFmpeg", true)];
            Arc::new(fake)
        }

        fn answer(&self, kind: Kind, answer: Answer) {
            lock(&self.answers).insert(kind, answer);
        }

        fn scans(&self) -> Vec<Mode> {
            lock(&self.scans).iter().map(|(_, mode, _)| *mode).collect()
        }
    }

    fn source(fake: &Arc<Fake>) -> Source {
        let (scan, cancel, rename, missing, tools, install) = (
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
        );
        Source {
            scan: Arc::new(move |root, mode, lane| {
                lock(&scan.scans).push((root, mode, lane));
                let answer = lock(&scan.answers).get(&kind_of_mode(mode)).cloned();
                match answer.unwrap_or(Answer::Found(Scan::default())) {
                    Answer::Found(found) => Box::pin(async move { Ok(found) }),
                    Answer::Fail(reason) => Box::pin(async move { Err(reason) }),
                    Answer::Hold => {
                        let (sender, receiver) = async_channel::bounded(1);
                        *lock(&scan.held) = Some(sender);
                        Box::pin(async move { receiver.recv().await.map_err(|e| e.to_string()).and_then(|r| r) })
                    }
                }
            }),
            cancel: Arc::new(move |_| {
                cancel.cancels.fetch_add(1, Ordering::SeqCst);
                if let Some(held) = lock(&cancel.held).take() {
                    let _ = held.try_send(Err("cancelled".into()));
                }
            }),
            rename: Arc::new(move |pairs| {
                lock(&rename.renames).push(pairs.clone());
                let undo_ops = pairs
                    .iter()
                    .map(|p| RenamePair {
                        from: p.to.clone(),
                        to: p.from.clone(),
                    })
                    .collect();
                Box::pin(async move {
                    Ok(BatchRenameResult {
                        renamed: pairs.len(),
                        undo_ops,
                        elapsed_ms: 1.0,
                    })
                })
            }),
            missing: Arc::new(move |paths| {
                let gone = lock(&missing.gone).clone();
                Box::pin(async move { Ok(paths.into_iter().filter(|p| gone.contains(p)).collect()) })
            }),
            tools: Arc::new(move || {
                let list = lock(&tools.tools).clone();
                Box::pin(async move { Ok(list) })
            }),
            install: Arc::new(move |id, progress| {
                lock(&install.installs).push(id.clone());
                let install = install.clone();
                Box::pin(async move {
                    let _ = progress
                        .send(Progress {
                            stage: "Downloading".into(),
                            done: 1024,
                            total: 2048,
                        })
                        .await;
                    for tool in lock(&install.tools).iter_mut().filter(|t| t.id == id) {
                        tool.installed = true;
                    }
                    Ok(())
                })
            }),
            cancel_install: Arc::new(|_| {}),
        }
    }

    // ---- Driving the dialog --------------------------------------------------------------------------

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn checked(cx: &mut VisualTestContext, label: &str) -> Option<bool> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.checked())
        })
    }

    fn disabled(cx: &mut VisualTestContext, label: &str) -> Option<bool> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.disabled())
        })
    }

    /// There is such a control and it is not disabled (a control that isn't says nothing about it).
    fn enabled(cx: &mut VisualTestContext, label: &str) -> bool {
        has(cx, label) && disabled(cx, label) != Some(true)
    }

    fn click(cx: &mut VisualTestContext, id: impl Into<ElementId>) {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        cx.run_until_parked();
    }

    /// Picks `title` in one of the option boxes, as its list does when an item is chosen (the headless windows
    /// don't open the list's popup, so its `Confirm` event is sent from the box).
    fn confirm(cx: &mut VisualTestContext, state: &Entity<SelectState<Vec<SharedString>>>, title: &str) {
        let title = SharedString::from(title.to_string());
        cx.update(|_, cx| {
            state.update(cx, |_, cx| cx.emit(SelectEvent::Confirm(Some(title))));
        });
        cx.run_until_parked();
    }

    /// Lets core's runtime answer to the foreground until `done`.
    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("what the dialog waited for never arrived");
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    /// An app with the skeleton's globals and no pictures, and a window with the dialog layer.
    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        cx.update(|cx| {
            thumbnail::init_with(cx, Rc::new(NoThumbs));
        });
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    fn open_dialog(cx: &mut VisualTestContext, root: &str, kind: Kind, fake: &Arc<Fake>) -> Entity<CleanupDialog> {
        let source = source(fake);
        cx.update(|window, cx| open_with(window, cx, root, Some(kind), source))
            .expect("the dialog opens")
    }

    /// A fake explorer over `disk`, so a recycle has somewhere to go.
    fn with_explorer(cx: &mut VisualTestContext, disk: &FakeDisk) {
        let source_of_disk = disk.source();
        cx.update(|_, cx| explorer::init_with(cx, source_of_disk));
    }

    // ---- The pure pieces -------------------------------------------------------------------------------

    #[test]
    fn the_sections_list_every_kind_once() {
        let _guard = testing::lock();
        let listed: Vec<Kind> = Kind::section_list().into_iter().flat_map(|(_, kinds)| kinds).collect();
        assert_eq!(listed, Kind::ALL);
        let titles: Vec<String> = Kind::section_list().into_iter().map(|(t, _)| t.to_string()).collect();
        assert_eq!(titles, ["Free up space", "With Czkawka"]);
        let labels: Vec<String> = Kind::ALL.iter().map(|k| k.label().to_string()).collect();
        assert_eq!(
            labels,
            [
                "Developer junk",
                "Empty folders",
                "Temporary files",
                "Big files",
                "Look-alike pictures",
                "Look-alike videos",
                "Same music",
                "Broken files",
                "Wrong extensions"
            ]
        );
    }

    #[test]
    fn kinds_know_what_they_need_and_what_they_find() {
        let _guard = testing::lock();
        for kind in [Kind::Junk, Kind::Empty, Kind::Temporary, Kind::Big] {
            assert!(kind.needs().is_empty(), "{kind:?}");
        }
        let needs = |kind: Kind| kind.needs().iter().map(|n| n.id).collect::<Vec<_>>();
        assert_eq!(needs(Kind::Images), ["czkawka"]);
        assert_eq!(needs(Kind::Videos), ["czkawka", "ffmpeg"]);
        assert_eq!(needs(Kind::Misnamed), ["czkawka"]);
        assert!(Kind::Junk.folders() && Kind::Empty.folders() && !Kind::Big.folders());
        assert!(Kind::Images.grouped() && Kind::Music.grouped() && !Kind::Broken.grouped());
        assert!(Kind::Misnamed.renames() && !Kind::Broken.renames());
        assert_eq!(Kind::Videos.noun(), "video");
        assert_eq!(Kind::Empty.noun(), "empty folder");
        assert_eq!(
            Kind::Junk.helpers(),
            [Helper::All, Helper::ProjectsUntouched30].as_slice()
        );
        assert_eq!(Kind::Big.helpers(), [Helper::UnchangedForAYear].as_slice());
    }

    #[test]
    fn requests_follow_the_options() {
        let o = Options::default();
        assert_eq!(o.at_least, GB);
        assert_eq!(request_for(Kind::Big, &o), Mode::Big { at_least: GB });
        assert_eq!(request_for(Kind::Images, &o), Mode::Images { alike: 6 });
        assert_eq!(request_for(Kind::Videos, &o), Mode::Videos { alike: 10 });
        assert_eq!(request_for(Kind::Music, &o), Mode::Music { by_sound: false });
        let sharp = Options {
            alike: 0,
            by_sound: true,
            at_least: SIZES[4],
        };
        assert_eq!(request_for(Kind::Images, &sharp), Mode::Images { alike: 2 });
        assert_eq!(request_for(Kind::Videos, &sharp), Mode::Videos { alike: 4 });
        assert_eq!(request_for(Kind::Music, &sharp), Mode::Music { by_sound: true });
        assert_eq!(request_for(Kind::Big, &sharp), Mode::Big { at_least: 10 * GB });
        let loose = Options { alike: 2, ..o };
        assert_eq!(request_for(Kind::Images, &loose), Mode::Images { alike: 12 });
        assert_eq!(request_for(Kind::Videos, &loose), Mode::Videos { alike: 16 });
        // A level out of range falls back to the middle one rather than panicking.
        let odd = Options { alike: 9, ..o };
        assert_eq!(request_for(Kind::Images, &odd), Mode::Images { alike: 6 });
        for (kind, mode) in [
            (Kind::Junk, Mode::Junk),
            (Kind::Empty, Mode::Empty),
            (Kind::Temporary, Mode::Temporary),
            (Kind::Broken, Mode::Broken),
            (Kind::Misnamed, Mode::Misnamed),
        ] {
            assert_eq!(request_for(kind, &o), mode);
        }
    }

    #[test]
    fn engines_are_named_for_a_sentence() {
        let _guard = testing::lock();
        assert_eq!(engine_text("index + fd + pdu"), "the index, fd and pdu");
        assert_eq!(engine_text("index + fd"), "the index and fd");
        assert_eq!(engine_text("fd"), "fd");
        assert_eq!(engine_text("czkawka"), "Czkawka");
        assert_eq!(engine_text("something new"), "something new");
        assert_eq!(engine_text(""), "");
    }

    #[test]
    fn a_real_extension_replaces_only_the_last_one() {
        assert_eq!(with_extension("C:\\p\\photo.JPG", "png"), "C:\\p\\photo.png");
        assert_eq!(with_extension("C:\\p.d\\photo", "png"), "C:\\p.d\\photo.png");
        assert_eq!(with_extension("C:\\p\\a.tar.gz", "zip"), "C:\\p\\a.tar.zip");
        assert_eq!(with_extension("C:\\صور\\صورة.JPG", "png"), "C:\\صور\\صورة.png");
        assert_eq!(with_extension("noext", "png"), "noext.png");
    }

    #[test]
    fn paths_are_shown_from_the_scanned_folder() {
        assert_eq!(
            in_root("C:\\Repos", "C:\\Repos\\web\\node_modules"),
            "web\\node_modules"
        );
        assert_eq!(in_root("c:\\repos\\", "C:\\Repos\\web"), "web");
        assert_eq!(in_root("C:\\", "C:\\a\\b"), "a\\b");
        // Another folder that merely starts the same, or outside, stays whole.
        assert_eq!(in_root("C:\\Repos", "C:\\ReposX\\a"), "C:\\ReposX\\a");
        assert_eq!(in_root("C:\\Repos", "D:\\other"), "D:\\other");
        assert_eq!(in_root("C:\\مشاريع", "C:\\مشاريع\\تطبيق"), "تطبيق");
    }

    #[test]
    fn helpers_choose_as_the_page_did() {
        let now = 1000 * DAY;
        let items = vec![
            item("a", 1, 0, ""),
            item("b", 1, now - 40 * DAY, ""),
            item("c", 1, now - 10 * DAY, ""),
            item("d", 1, now - 2 * DAY, ""),
            item("e", 1, now - 400 * DAY, ""),
        ];
        assert_eq!(Helper::All.pick(&items, now), [0, 1, 2, 3, 4]);
        // No date is not old.
        assert_eq!(Helper::ProjectsUntouched30.pick(&items, now), [1, 4]);
        assert_eq!(Helper::OlderThanWeek.pick(&items, now), [1, 2, 4]);
        assert_eq!(Helper::UnchangedForAYear.pick(&items, now), [4]);

        let pictures = vec![
            Item {
                size: 900,
                pixels: 100,
                date: 5,
                ..like("g1-small", 1, 0)
            },
            Item {
                size: 100,
                pixels: 400,
                date: 1,
                ..like("g1-big", 1, 0)
            },
            Item {
                size: 50,
                pixels: 10,
                date: 7,
                ..like("g2-a", 2, 0)
            },
            Item {
                size: 50,
                pixels: 10,
                date: 9,
                ..like("g2-b", 2, 0)
            },
        ];
        // The most pixels stays, then the biggest file; the first of equals stays.
        assert_eq!(Helper::KeepBiggest.pick(&pictures, now), [0, 3]);
        assert_eq!(Helper::KeepNewest.pick(&pictures, now), [1, 2]);
        assert!(Helper::KeepBiggest.pick(&[], now).is_empty());
    }

    #[test]
    fn extra_copies_are_all_but_the_biggest_of_each_group() {
        let scan = scan_of(vec![
            like("a", 1, 500),
            like("b", 1, 300),
            like("c", 2, 70),
            like("d", 2, 70),
            like("e", 3, 5),
        ]);
        // Groups 1, 2 and 3; all but the biggest of each is 300 + 70 + 0.
        assert_eq!(spare_of(&scan), (3, 370));
    }

    #[test]
    fn groups_are_biggest_first_and_look_alikes_are_called_by_their_first() {
        let _guard = testing::lock();
        let items = vec![
            item("C:\\a\\build", 10, 0, "Rust builds"),
            item("C:\\b\\node_modules", 500, 0, "Node packages"),
            item("C:\\c\\build", 20, 0, "Rust builds"),
        ];
        let groups = groups_of(Kind::Junk, &items);
        let titles: Vec<_> = groups.iter().map(|g| g.title.to_string()).collect();
        assert_eq!(titles, ["Node packages", "Rust builds"]);
        assert_eq!(groups[1].items, [0, 2]);
        assert_eq!(groups[1].size, 30);

        // Empty folders have no kind and no size: one group.
        let empties = vec![item("C:\\a\\x", 0, 0, ""), item("C:\\a\\y", 0, 0, "")];
        let groups = groups_of(Kind::Empty, &empties);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "Empty folders");

        // Big files without a kind are grouped by the type of file they are.
        let big = vec![item("C:\\v\\movie.mp4", 9, 0, ""), item("C:\\v\\song.mp3", 5, 0, "")];
        let groups = groups_of(Kind::Big, &big);
        let expect = |name: &str| kind_of(&entry_of(Kind::Big, &item(name, 0, 0, ""))).group.name;
        assert_eq!(groups[0].title, expect("movie.mp4"));
        assert_eq!(groups[1].title, expect("song.mp3"));

        let pictures = vec![
            like("C:\\p\\small.jpg", 1, 10),
            like("C:\\p\\big.jpg", 2, 900),
            like("C:\\p\\mid.jpg", 2, 800),
            like("C:\\p\\big-copy.jpg", 2, 700),
        ];
        let groups = groups_of(Kind::Images, &pictures);
        assert_eq!(groups[0].title, "“big.jpg” and 2 look-alikes");
        assert_eq!(groups[1].title, "“small.jpg” and 0 look-alikes");
    }

    #[test]
    fn lines_are_a_heading_then_its_rows() {
        let ready = Ready::new(Kind::Junk, junk_scan());
        assert_eq!(
            ready.lines,
            [
                Line::Group(0),
                Line::Row { group: 0, item: 0 },
                Line::Group(1),
                Line::Row { group: 1, item: 1 },
                Line::Row { group: 1, item: 2 },
            ]
        );
    }

    #[test]
    fn the_summary_says_what_was_found_and_what_was_left_out() {
        let _guard = testing::lock();
        let mut scan = scan_of(vec![item("a", GB, 0, "x"), item("b", GB, 0, "x")]);
        assert_eq!(
            summary(Kind::Junk, &scan),
            format!(
                "2 folders · {} · found by the index and fd in 12 ms",
                format_size(2 * GB)
            )
        );
        // Empty folders have no size to show.
        assert_eq!(
            summary(Kind::Empty, &scan),
            "2 empty folders · found by the index and fd in 12 ms"
        );
        scan.more = 4995;
        scan.held = 1;
        scan.engine = "fd".into();
        let text = summary(Kind::Temporary, &scan);
        assert!(text.starts_with("4,997 files · "), "{text}");
        assert!(
            text.ends_with(" · the 2 biggest listed · 1 file in use left out"),
            "{text}"
        );
        assert!(summary(Kind::Empty, &scan).contains("the first 2 listed"));
    }

    #[test]
    fn the_badge_says_it_the_way_each_kind_does() {
        let _guard = testing::lock();
        let scan = scan_of(vec![item("a", 3 * MB, 0, "x")]);
        assert_eq!(
            Kind::Junk.badge(&scan),
            format!("{} of developer junk", format_size(3 * MB))
        );
        assert_eq!(Kind::Empty.badge(&scan), "1 empty folder");
        assert_eq!(
            Kind::Temporary.badge(&scan),
            format!("{} left behind", format_size(3 * MB))
        );
        assert_eq!(Kind::Big.badge(&scan), format!("{} in big files", format_size(3 * MB)));
        assert_eq!(Kind::Broken.badge(&scan), "1 broken file");
        assert_eq!(Kind::Misnamed.badge(&scan), "1 misnamed file");
        // One item is one group with no spare copy.
        assert_eq!(Kind::Images.badge(&scan), "1 group · 0 B in extra copies");
        assert_eq!(
            Kind::Big.nothing_found(&Options::default()),
            format!("No files of {} or more here", format_size(GB))
        );
    }

    #[test]
    fn progress_is_worded_as_settings_words_it() {
        let _guard = testing::lock();
        let step = |stage: &str, done, total| Progress {
            stage: stage.into(),
            done,
            total,
        };
        assert_eq!(describe_progress(&step("Starting", 0, 0)), "Starting…");
        assert_eq!(describe_progress(&step("Working…", 0, 0)), "Working…");
        assert_eq!(
            describe_progress(&step("Downloading", 2 * MB, 4 * MB)),
            format!("Downloading {} of {}…", format_size(2 * MB), format_size(4 * MB))
        );
    }

    // ---- The dialog -------------------------------------------------------------------------------------

    #[gpui_kit::test]
    fn junk_is_found_in_the_background_and_listed_in_labelled_groups(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.answer(Kind::Junk, Answer::Found(junk_scan()));
        let _view = open_dialog(&mut visual, "C:\\Repos", Kind::Junk, &fake);

        // The dialog layer is up and named after the folder.
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            window.render_frame(cx);
            assert!(!window.find_all_by_role(Role::Dialog).is_empty(), "no Dialog role");
        });
        assert_eq!(role_of(&mut visual, "Clean up Repos"), Some(Role::Group));
        settle(&mut visual, |cx| has(cx, "app\\node_modules"));
        // The scan was asked of the lane, for the folder.
        assert_eq!(
            lock(&fake.scans).as_slice(),
            [("C:\\Repos".to_string(), Mode::Junk, "cleanup".to_string())]
        );

        // The kinds are tabs in two lists.
        for label in [
            "Developer junk",
            "Empty folders",
            "Look-alike videos",
            "Wrong extensions",
        ] {
            assert_eq!(role_of(&mut visual, label), Some(Role::Tab), "{label}");
        }
        // The one picked is selected, the others are not.
        visual.update(|window, _| {
            assert_eq!(window.find_by_label("Developer junk").selected(), Some(true));
            assert_eq!(window.find_by_label("Empty folders").selected(), Some(false));
        });
        assert_eq!(role_of(&mut visual, "Free up space"), Some(Role::TabList));
        assert_eq!(role_of(&mut visual, "With Czkawka"), Some(Role::TabList));
        // Each group has a heading and each item a row, all checkboxes, none chosen yet.
        assert_eq!(role_of(&mut visual, "Found"), Some(Role::Group));
        for label in [
            "Rust builds",
            "Node packages",
            "app\\target",
            "app\\node_modules",
            "app2\\node_modules",
        ] {
            assert_eq!(role_of(&mut visual, label), Some(Role::CheckBox), "{label}");
            assert_eq!(checked(&mut visual, label), Some(false), "{label}");
        }
        // What was found is the badge, and the big button waits for a choice.
        let badge = format!("{} of developer junk", format_size(2 * GB + 400 * MB));
        assert_eq!(role_of(&mut visual, &badge), Some(Role::Status));
        assert_eq!(disabled(&mut visual, "Recycle"), Some(true));
    }

    #[gpui_kit::test]
    fn choosing_rows_groups_and_helpers_updates_the_big_button(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.answer(Kind::Junk, Answer::Found(junk_scan()));
        let _view = open_dialog(&mut visual, "C:\\Repos", Kind::Junk, &fake);
        settle(&mut visual, |cx| has(cx, "app\\target"));
        let all = format!("Recycle 3 · {}", format_size(2 * GB + 400 * MB));

        // A row: the button says how many and how much.
        click(&mut visual, ("cleanup-line", 1_usize));
        assert_eq!(checked(&mut visual, "app\\target"), Some(true));
        assert!(enabled(&mut visual, &format!("Recycle 1 · {}", format_size(2 * GB))));
        // A heading is part-way when only some of its rows are chosen, and a click on it chooses the rest.
        click(&mut visual, ("cleanup-line", 3_usize));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find_by_label("Node packages").indeterminate(), Some(true));
        });
        click(&mut visual, ("cleanup-line", 2_usize));
        assert_eq!(checked(&mut visual, "app2\\node_modules"), Some(true));
        assert!(has(&mut visual, &all));
        // A click on a heading with everything chosen unchooses its group.
        click(&mut visual, ("cleanup-line", 2_usize));
        assert_eq!(checked(&mut visual, "app\\node_modules"), Some(false));
        assert_eq!(checked(&mut visual, "app\\target"), Some(true));
        // None clears; All chooses everything; the projects helper takes the old ones (these are from 1970).
        click(&mut visual, "cleanup-none");
        assert_eq!(disabled(&mut visual, "Recycle"), Some(true));
        click(&mut visual, ("cleanup-helper", 0_usize));
        assert!(has(&mut visual, &all));
        click(&mut visual, "cleanup-none");
        click(&mut visual, ("cleanup-helper", 1_usize));
        assert!(has(&mut visual, &all));
        assert!(enabled(&mut visual, "None"));
    }

    #[gpui_kit::test]
    fn recycling_goes_through_the_explorer_and_only_what_went_leaves_the_list(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let disk = FakeDisk::new();
        disk.folder(
            "C:\\Repos\\app",
            vec![
                folder("C:\\Repos\\app", "target"),
                folder("C:\\Repos\\app", "node_modules"),
            ],
        );
        disk.folder("C:\\Repos\\app2", vec![folder("C:\\Repos\\app2", "node_modules")]);
        with_explorer(&mut visual, &disk);
        let fake = Fake::new();
        fake.answer(Kind::Junk, Answer::Found(junk_scan()));
        // Windows' own dialog was cancelled partway: the last one is still there.
        *lock(&fake.gone) = vec!["C:\\Repos\\app\\target".into(), "C:\\Repos\\app\\node_modules".into()];
        let _view = open_dialog(&mut visual, "C:\\Repos", Kind::Junk, &fake);
        settle(&mut visual, |cx| has(cx, "app\\target"));

        click(&mut visual, ("cleanup-helper", 0_usize));
        click(&mut visual, "cleanup-recycle");
        settle(&mut visual, |cx| !has(cx, "app\\target"));
        // Asked of Windows' file engine to recycle, with its own confirmation, all three in the list's order.
        assert_eq!(
            disk.ops(),
            [FileOp::Delete {
                paths: vec![
                    "C:\\Repos\\app\\target".into(),
                    "C:\\Repos\\app\\node_modules".into(),
                    "C:\\Repos\\app2\\node_modules".into()
                ],
                permanent: false,
                confirm: true,
            }]
        );
        assert!(!has(&mut visual, "app\\node_modules"));
        // What stayed is still listed and still chosen; the badge follows the list.
        assert_eq!(checked(&mut visual, "app2\\node_modules"), Some(true));
        assert!(has(
            &mut visual,
            &format!("{} of developer junk", format_size(100 * MB))
        ));
        assert!(has(&mut visual, &format!("Recycle 1 · {}", format_size(100 * MB))));
    }

    #[gpui_kit::test]
    fn a_failed_recycle_says_so_and_keeps_the_list(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let disk = FakeDisk::new();
        disk.fail_next("Access is denied");
        with_explorer(&mut visual, &disk);
        let fake = Fake::new();
        fake.answer(Kind::Junk, Answer::Found(junk_scan()));
        let _view = open_dialog(&mut visual, "C:\\Repos", Kind::Junk, &fake);
        settle(&mut visual, |cx| has(cx, "app\\target"));
        click(&mut visual, ("cleanup-line", 1_usize));
        click(&mut visual, "cleanup-recycle");
        // The person is told (a notification is an alert), and the list stays as it was.
        settle(&mut visual, |cx| {
            cx.update(|window, _| !window.find_all_by_role(Role::Alert).is_empty())
        });
        assert!(has(&mut visual, "app\\target"));
        // The button is usable again.
        let again = format!("Recycle 1 · {}", format_size(2 * GB));
        settle(&mut visual, |cx| enabled(cx, &again));
    }

    #[gpui_kit::test]
    fn a_kind_that_needs_a_tool_offers_to_install_it_and_looks_once_it_is_there(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        *lock(&fake.tools) = vec![tool("czkawka", "Czkawka", false), tool("ffmpeg", "FFmpeg", true)];
        fake.answer(
            Kind::Images,
            Answer::Found(scan_of(vec![
                like("C:\\Pics\\a.jpg", 1, 900),
                like("C:\\Pics\\a-small.jpg", 1, 100),
            ])),
        );
        let _view = open_dialog(&mut visual, "C:\\Pics", Kind::Images, &fake);
        let why = "Czkawka finds look-alike pictures, videos and music, broken files and wrong extensions. It installs for your account with winget.";
        settle(&mut visual, |cx| has(cx, why));
        assert_eq!(role_of(&mut visual, why), Some(Role::Group));
        // Nothing is looked for until it is there, and the kind's own options are shown meanwhile.
        assert!(fake.scans().is_empty());
        assert!(has(&mut visual, "Install Czkawka"));
        assert_eq!(role_of(&mut visual, "Match"), Some(Role::ComboBox));

        click(&mut visual, ("cleanup-install", 0_usize));
        settle(&mut visual, |cx| has(cx, "a-small.jpg"));
        assert_eq!(*lock(&fake.installs), ["czkawka"]);
        assert_eq!(fake.scans(), [Mode::Images { alike: 6 }]);
        assert!(!has(&mut visual, why));
        assert_eq!(role_of(&mut visual, "“a.jpg” and 1 look-alike"), Some(Role::CheckBox));
        // Look-alike kinds offer to keep the best of each.
        assert!(has(&mut visual, "Keep the biggest") && has(&mut visual, "Keep the newest"));
        click(&mut visual, ("cleanup-helper", 0_usize));
        assert_eq!(checked(&mut visual, "a-small.jpg"), Some(true));
        assert_eq!(checked(&mut visual, "a.jpg"), Some(false));
    }

    #[gpui_kit::test]
    fn videos_need_two_tools_and_each_has_its_own_card(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        *lock(&fake.tools) = vec![tool("czkawka", "Czkawka", false), tool("ffmpeg", "FFmpeg", false)];
        let _view = open_dialog(&mut visual, "C:\\Videos", Kind::Videos, &fake);
        let ffmpeg = "FFmpeg reads the videos for Czkawka. It installs for your account with winget.";
        settle(&mut visual, |cx| has(cx, ffmpeg));
        assert!(has(&mut visual, "Install Czkawka") && has(&mut visual, "Install FFmpeg"));
        assert!(fake.scans().is_empty());
    }

    #[gpui_kit::test]
    fn a_scan_can_be_stopped_and_looked_for_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.answer(Kind::Big, Answer::Hold);
        let _view = open_dialog(&mut visual, "C:\\Repos", Kind::Big, &fake);
        visual.run_until_parked();
        // Looking: a status line and a Stop button, and the options of the kind.
        assert_eq!(role_of(&mut visual, "Looking below “Repos”…"), Some(Role::Status));
        assert_eq!(role_of(&mut visual, "At least"), Some(Role::ComboBox));
        let before = fake.cancels.load(Ordering::SeqCst);
        click(&mut visual, "cleanup-stop");
        settle(&mut visual, |cx| has(cx, "Stopped before it finished"));
        assert_eq!(fake.cancels.load(Ordering::SeqCst), before + 1);
        assert_eq!(role_of(&mut visual, "Stopped before it finished"), Some(Role::Status));

        fake.answer(
            Kind::Big,
            Answer::Found(scan_of(vec![item("C:\\Repos\\big.iso", 5 * GB, 7, "")])),
        );
        click(&mut visual, "cleanup-look-again");
        settle(&mut visual, |cx| has(cx, "big.iso"));
        assert_eq!(fake.scans().len(), 2);
    }

    #[gpui_kit::test]
    fn another_kind_stops_the_one_looking_and_it_looks_again_when_picked(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.answer(Kind::Big, Answer::Hold);
        fake.answer(
            Kind::Temporary,
            Answer::Found(scan_of(vec![item("C:\\Repos\\x.tmp", 10, 5, "Temporary files")])),
        );
        let view = open_dialog(&mut visual, "C:\\Repos", Kind::Big, &fake);
        visual.run_until_parked();
        click(&mut visual, ("cleanup-kind", 2_usize));
        settle(&mut visual, |cx| has(cx, "x.tmp"));
        // Big files was cut short by it and must not claim it stopped: it looks again when picked.
        visual.update(|_, cx| {
            let model = view.read(cx).model().clone();
            let model = model.read(cx);
            assert!(model.outcome(Kind::Big).is_none());
            assert_eq!(model.found_count(Kind::Temporary), Some(1));
            assert_eq!(model.kind(), Kind::Temporary);
        });
        assert_eq!(fake.scans(), [Mode::Big { at_least: GB }, Mode::Temporary]);
        fake.answer(Kind::Big, Answer::Found(Scan::default()));
        click(&mut visual, ("cleanup-kind", 3_usize));
        let none = format!("No files of {} or more here", format_size(GB));
        settle(&mut visual, |cx| has(cx, &none));
        assert_eq!(fake.scans().len(), 3);
    }

    #[gpui_kit::test]
    fn failures_and_empty_results_have_their_own_messages(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.answer(Kind::Junk, Answer::Fail("access denied".into()));
        fake.answer(
            Kind::Empty,
            Answer::Found(Scan {
                elapsed: 3.0,
                ..Scan::default()
            }),
        );
        fake.answer(
            Kind::Temporary,
            Answer::Found(Scan {
                elapsed: 3.0,
                held: 2,
                ..Scan::default()
            }),
        );
        let _view = open_dialog(&mut visual, "C:\\Repos", Kind::Junk, &fake);
        settle(&mut visual, |cx| has(cx, "Couldn't look below “Repos”: access denied"));
        assert_eq!(
            role_of(&mut visual, "Couldn't look below “Repos”: access denied"),
            Some(Role::Alert)
        );
        click(&mut visual, ("cleanup-kind", 1_usize));
        settle(&mut visual, |cx| has(cx, "No empty folders here"));
        assert_eq!(role_of(&mut visual, "No empty folders here"), Some(Role::Status));
        click(&mut visual, ("cleanup-kind", 2_usize));
        settle(&mut visual, |cx| has(cx, "Nothing left behind here"));
        // Nothing is listed to choose from, so there is no big button.
        assert!(!has(&mut visual, "Recycle"));
    }

    #[gpui_kit::test]
    fn wrong_extensions_are_renamed_all_or_none_with_an_undo(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let disk = FakeDisk::new();
        disk.folder("C:\\Pics", Vec::new());
        with_explorer(&mut visual, &disk);
        let runner = visual.update(|_, cx| explorer::FakeUndoRunner::install(cx));
        let fake = Fake::new();
        let wrong = |path: &str, fix: &str| Item {
            path: path.into(),
            size: 10,
            date: 5,
            fix: fix.into(),
            note: format!("It is really a {fix}"),
            ..Default::default()
        };
        fake.answer(
            Kind::Misnamed,
            Answer::Found(scan_of(vec![
                wrong("C:\\Pics\\a.JPG", "png"),
                wrong("C:\\Pics\\b.png", "jpg"),
                wrong("C:\\Pics\\c.gif", "webp"),
            ])),
        );
        let _view = open_dialog(&mut visual, "C:\\Pics", Kind::Misnamed, &fake);
        settle(&mut visual, |cx| has(cx, "a.JPG"));
        // Files are renamed here, not recycled.
        assert!(!has(&mut visual, "Recycle"));
        assert_eq!(disabled(&mut visual, "Give the real extension"), Some(true));
        click(&mut visual, ("cleanup-line", 1_usize));
        assert!(enabled(&mut visual, "Give 1 file its real extension"));
        click(&mut visual, ("cleanup-helper", 0_usize));
        assert!(has(&mut visual, "Give 3 files their real extension"));
        click(&mut visual, "cleanup-give");
        settle(&mut visual, |cx| !has(cx, "a.JPG"));
        let pair = |from: &str, to: &str| (from.to_string(), to.to_string());
        let asked = |n: usize| -> Vec<(String, String)> {
            lock(&fake.renames)[n]
                .iter()
                .map(|p| (p.from.clone(), p.to.clone()))
                .collect()
        };
        assert_eq!(lock(&fake.renames).len(), 1);
        assert_eq!(
            asked(0),
            [
                pair("C:\\Pics\\a.JPG", "C:\\Pics\\a.png"),
                pair("C:\\Pics\\b.png", "C:\\Pics\\b.jpg"),
                pair("C:\\Pics\\c.gif", "C:\\Pics\\c.webp"),
            ]
        );
        settle(&mut visual, |cx| has(cx, "Every file here has the right extension"));
        // Ctrl+Z has it too, under the page's label.
        let explorer = visual.update(|_, cx| explorer::explorer(cx));
        assert_eq!(
            explorer.read_with(&visual, |e, _| e.next_undo().map(|(_, label)| label.to_string())),
            Some("extensions of 3 files".to_string())
        );
        // The toast offers Undo, which is that same step: it renames them back.
        settle(&mut visual, |cx| has(cx, "Undo"));
        assert_eq!(role_of(&mut visual, "Undo"), Some(Role::Button));
        // The dialog is modal: the person closes it, and the toast's Undo is under the pointer.
        visual.update(|window, cx| window.press("escape", cx));
        visual.run_until_parked();
        click(&mut visual, "cleanup-undo");
        settle(&mut visual, |_| runner.renamed().len() == 1);
        // The explorer's step (core's renamer), not the dialog's source, took the names back, all in one call.
        assert_eq!(lock(&fake.renames).len(), 1);
        let back = runner.renamed();
        assert_eq!(back[0].len(), 3);
        assert_eq!(
            (back[0][0].from.as_str(), back[0][0].to.as_str()),
            ("C:\\Pics\\a.png", "C:\\Pics\\a.JPG")
        );
        assert!(explorer.read_with(&visual, |e, _| e.next_undo().is_none()));
    }

    #[gpui_kit::test]
    fn ctrl_z_takes_the_extensions_back_under_an_older_step(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let disk = FakeDisk::new();
        disk.folder("C:\\Pics", Vec::new());
        with_explorer(&mut visual, &disk);
        let runner = visual.update(|_, cx| explorer::FakeUndoRunner::install(cx));
        let explorer = visual.update(|_, cx| explorer::explorer(cx));
        // A step from before: the extensions are the newer one.
        explorer.update(&mut visual, |e, cx| {
            e.remember_undo(
                "copy of 1 item",
                explorer::UndoAction::Recycle(vec!["C:\\Pics\\x".into()]),
                cx,
            );
        });
        let fake = Fake::new();
        fake.answer(
            Kind::Misnamed,
            Answer::Found(scan_of(vec![Item {
                path: "C:\\Pics\\تقرير.JPG".into(),
                size: 10,
                date: 5,
                fix: "png".into(),
                note: "It is really a png".into(),
                ..Default::default()
            }])),
        );
        let _view = open_dialog(&mut visual, "C:\\Pics", Kind::Misnamed, &fake);
        settle(&mut visual, |cx| has(cx, "تقرير.JPG"));
        click(&mut visual, ("cleanup-helper", 0_usize));
        click(&mut visual, "cleanup-give");
        settle(&mut visual, |_| lock(&fake.renames).len() == 1);
        let next = |cx: &mut VisualTestContext| {
            explorer.read_with(cx, |e, _| e.next_undo().map(|(_, label)| label.to_string()))
        };
        settle(&mut visual, |cx| next(cx).as_deref() == Some("extensions of 1 file"));
        // Ctrl+Z: the names go back through the explorer's step, the older step stays.
        let task = explorer.update(&mut visual, |e, cx| e.undo(None, cx));
        visual.foreground_executor().block_test(task);
        visual.run_until_parked();
        let back = runner.renamed();
        assert_eq!(back.len(), 1);
        assert_eq!(
            (back[0][0].from.as_str(), back[0][0].to.as_str()),
            ("C:\\Pics\\تقرير.png", "C:\\Pics\\تقرير.JPG")
        );
        assert_eq!(next(&mut visual).as_deref(), Some("copy of 1 item"));
    }

    #[gpui_kit::test]
    fn the_options_look_again_with_the_new_choice(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.answer(
            Kind::Big,
            Answer::Found(scan_of(vec![item("C:\\Repos\\a.iso", 5 * GB, 7, "")])),
        );
        fake.answer(Kind::Music, Answer::Found(Scan::default()));
        let _view = open_dialog(&mut visual, "C:\\Repos", Kind::Big, &fake);
        settle(&mut visual, |cx| has(cx, "a.iso"));
        // The box shows the size in force; picking another from its list looks again.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find_by_label("At least").is_some());
        });
        let at_least = _view.read_with(&visual, |v, _| v.at_least.clone());
        confirm(&mut visual, &at_least, "4.0 GB");
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find_by_label("At least").value(),
                Some("4.0 GB"),
                "the box shows it"
            );
        });
        settle(&mut visual, |_| fake.scans().len() == 2);
        assert_eq!(fake.scans()[1], Mode::Big { at_least: 4 * GB });
        // Another kind's own option.
        click(&mut visual, ("cleanup-kind", 6_usize));
        settle(&mut visual, |_| fake.scans().len() == 3);
        let compare = _view.read_with(&visual, |v, _| v.compare.clone());
        confirm(&mut visual, &compare, "How they sound (slower)");
        settle(&mut visual, |_| fake.scans().len() == 4);
        assert_eq!(fake.scans()[3], Mode::Music { by_sound: true });
    }

    #[gpui_kit::test]
    fn the_keyboard_walks_the_results_and_chooses(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.answer(Kind::Junk, Answer::Found(junk_scan()));
        let view = open_dialog(&mut visual, "C:\\Repos", Kind::Junk, &fake);
        settle(&mut visual, |cx| has(cx, "app\\target"));
        let focus = view.read_with(&visual, |view, _| view.focus_handle().clone());
        visual.update(|window, cx| {
            window.focus(&focus, cx);
            window.render_frame(cx);
        });
        visual.update(|window, cx| {
            window.press("down", cx);
            window.press("space", cx);
        });
        visual.run_until_parked();
        assert_eq!(view.read_with(&visual, |view, _| view.cursor()), 1);
        assert_eq!(checked(&mut visual, "app\\target"), Some(true));
        visual.update(|window, cx| {
            window.press("end", cx);
            window.press("space", cx);
            // Space on a heading chooses its whole group.
            window.press("up", cx);
            window.press("up", cx);
            window.press("up", cx);
            window.press("space", cx);
        });
        visual.run_until_parked();
        assert_eq!(view.read_with(&visual, |view, _| view.cursor()), 1);
        assert_eq!(checked(&mut visual, "app2\\node_modules"), Some(true));
        visual.update(|window, cx| {
            window.press("home", cx);
            window.press("enter", cx);
            // Enter chooses on the results and doesn't close the dialog.
            assert!(window.has_active_dialog(cx));
        });
        visual.run_until_parked();
        assert_eq!(checked(&mut visual, "Rust builds"), Some(true));
    }

    #[gpui_kit::test]
    fn escape_closes_the_dialog_and_focus_starts_inside_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.answer(Kind::Junk, Answer::Hold);
        let _view = open_dialog(&mut visual, "C:\\Repos", Kind::Junk, &fake);
        visual.run_until_parked();
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            assert!(
                window.focused(cx).is_some(),
                "nothing has focus after the dialog opened"
            );
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Escape must close the dialog"));
    }

    #[gpui_kit::test]
    fn letting_go_of_the_model_stops_the_scan_it_started(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.answer(Kind::Junk, Answer::Hold);
        let source = source(&fake);
        let model = visual.update(|_, cx| cx.new(|cx| Cleanup::new("C:\\Repos".into(), Kind::Junk, source, cx)));
        visual.run_until_parked();
        assert_eq!(fake.scans(), [Mode::Junk]);
        assert_eq!(fake.cancels.load(Ordering::SeqCst), 0);
        drop(model);
        // GPUI lets go of dropped entities when it next flushes its effects.
        visual.update(|_, _| ());
        visual.run_until_parked();
        assert_eq!(fake.cancels.load(Ordering::SeqCst), 1, "the scan was left running");
    }

    #[gpui_kit::test]
    fn showing_an_item_closes_the_dialog_and_opens_its_folder(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let disk = FakeDisk::new();
        disk.folder("C:\\Repos\\app", vec![folder("C:\\Repos\\app", "target")]);
        disk.folder("C:\\Repos", vec![folder("C:\\Repos", "app")]);
        with_explorer(&mut visual, &disk);
        let fake = Fake::new();
        fake.answer(Kind::Junk, Answer::Found(junk_scan()));
        let _view = open_dialog(&mut visual, "C:\\Repos", Kind::Junk, &fake);
        settle(&mut visual, |cx| has(cx, "app\\target"));
        click(&mut visual, ("cleanup-reveal", 1_usize));
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        settle(&mut visual, |cx| {
            cx.update(|_, cx| {
                explorer::explorer(cx)
                    .read(cx)
                    .active_tab()
                    .is_some_and(|tab| tab.path == "C:\\Repos\\app")
            })
        });
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render_without_trouble(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let fake = Fake::new();
        fake.answer(
            Kind::Junk,
            Answer::Found(scan_of(vec![
                item("C:\\مشاريع\\تطبيق\\target", 4 * MB, 9, "بناء"),
                item("C:\\مشاريع\\تطبيق\\node_modules", 2 * MB, 9, "حزم"),
            ])),
        );
        let _view = open_dialog(&mut visual, "C:\\مشاريع", Kind::Junk, &fake);
        settle(&mut visual, |cx| has(cx, "تطبيق\\target"));
        // Names are exposed as they are, without the layout-only isolates.
        assert_eq!(role_of(&mut visual, "تطبيق\\target"), Some(Role::CheckBox));
        assert_eq!(role_of(&mut visual, "بناء"), Some(Role::CheckBox));
        assert_eq!(role_of(&mut visual, "Clean up مشاريع"), Some(Role::Group));
        click(&mut visual, ("cleanup-line", 1_usize));
        assert_eq!(checked(&mut visual, "تطبيق\\target"), Some(true));
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn kinds_are_tabs_the_arrows_walk_and_options_are_boxes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.answer(
            Kind::Big,
            Answer::Found(scan_of(vec![item("C:\\Repos\\a.iso", 5 * GB, 7, "")])),
        );
        fake.answer(
            Kind::Images,
            Answer::Found(scan_of(vec![
                like("C:\\Repos\\a.jpg", 1, 9),
                like("C:\\Repos\\b.jpg", 1, 5),
            ])),
        );
        let view = open_dialog(&mut visual, "C:\\Repos", Kind::Big, &fake);
        settle(&mut visual, |cx| has(cx, "a.iso"));
        // The picked kind and the size in force are the selected ones.
        visual.update(|window, _| {
            assert_eq!(window.find_by_label("Big files").selected(), Some(true));
            assert_eq!(window.find_by_label("Temporary files").selected(), Some(false));
            assert_eq!(window.find_by_label("At least").value(), Some("1.0 GB"));
        });
        let tabs = view.read_with(&visual, |view, _| view.tab_focus.clone());
        let focus = |cx: &mut VisualTestContext, handle: &FocusHandle| {
            let handle = handle.clone();
            cx.update(|window, cx| {
                window.focus(&handle, cx);
                window.render_frame(cx);
            });
        };

        // Down on a kind moves to the next one and picks it (looking for it the first time).
        focus(&mut visual, &tabs[3]);
        visual.update(|window, cx| window.press("down", cx));
        settle(&mut visual, |cx| has(cx, "a.jpg"));
        assert_eq!(fake.scans()[1], Mode::Images { alike: 6 });
        visual.update(|window, _| assert_eq!(window.find_by_label("Match").value(), Some("Very similar")));
        visual.update(|window, _| {
            assert_eq!(window.find_by_label("Look-alike pictures").selected(), Some(true));
            assert_eq!(window.find_by_label("Big files").selected(), Some(false));
        });
        // Only the picked tab is a tab stop; Up goes back to the kind before.
        visual.update(|window, cx| window.press("up", cx));
        visual.run_until_parked();
        visual.update(|window, _| {
            assert_eq!(window.find_by_label("Big files").selected(), Some(true));
        });
    }

    /// A thumbnail source that remembers what is asked of it and has no pictures.
    struct Recorder {
        asked: Rc<std::cell::RefCell<Vec<thumbnail::Ask>>>,
    }

    impl thumbnail::ThumbSource for Recorder {
        fn thumbnail(&self, ask: thumbnail::Ask) -> thumbnail::Fut<Result<thumbnail::Thumbnail, String>> {
            self.asked.borrow_mut().push(ask);
            Box::pin(async { Err("no picture".to_string()) })
        }

        fn cancel(&self, _: &str, _: Option<Vec<String>>, _: u32) {}
    }

    #[gpui_kit::test]
    fn pictures_and_videos_ask_for_thumbnails_in_the_dialogs_lane_and_folders_do_not(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        let asked = Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorder = Recorder { asked: asked.clone() };
        cx.update(|cx| {
            thumbnail::init_with(cx, Rc::new(recorder));
        });
        let mut visual = testing::open_window(cx, |_, cx| cx.new(|_| Host)).0;
        let fake = Fake::new();
        fake.answer(
            Kind::Images,
            Answer::Found(scan_of(vec![
                like("C:\\Pics\\a.jpg", 1, 900),
                like("C:\\Pics\\b.png", 1, 100),
                like("C:\\Pics\\c.mp4", 2, 50),
                like("C:\\Pics\\d.mp4", 2, 40),
            ])),
        );
        fake.answer(Kind::Junk, Answer::Found(junk_scan()));
        let _view = open_dialog(&mut visual, "C:\\Pics", Kind::Images, &fake);
        settle(&mut visual, |cx| has(cx, "a.jpg"));
        settle(&mut visual, |_| asked.borrow().len() == 4);
        let mut paths: Vec<String> = asked.borrow().iter().map(|ask| ask.path.clone()).collect();
        paths.sort();
        assert_eq!(
            paths,
            [
                "C:\\Pics\\a.jpg",
                "C:\\Pics\\b.png",
                "C:\\Pics\\c.mp4",
                "C:\\Pics\\d.mp4"
            ]
        );
        assert!(asked.borrow().iter().all(|ask| ask.lane == "cleanup"));
        // Folders (developer junk) have no picture to ask for.
        let before = asked.borrow().len();
        click(&mut visual, ("cleanup-kind", 0_usize));
        // (Outside the folder scanned, a path is shown whole.)
        settle(&mut visual, |cx| has(cx, "C:\\Repos\\app\\target"));
        assert_eq!(asked.borrow().len(), before);
    }

    #[test]
    fn every_icon_the_dialog_draws_is_in_the_embedded_assets() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        // A missing icon draws nothing and says nothing; kit.rs's own list can't be edited from here.
        let mut icons: Vec<IconName> = Kind::ALL.iter().map(|k| k.icon()).collect();
        icons.extend([
            IconName::BrushCleaning,
            IconName::RefreshCw,
            IconName::Square,
            IconName::RotateCw,
            IconName::TriangleAlert,
            IconName::CircleCheck,
            IconName::FolderInput,
            IconName::Trash,
            IconName::PenLine,
            IconName::Download,
            IconName::Check,
            IconName::Minus,
        ]);
        for name in icons {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }
}
