//! Smart suggestions: what would make Glint better for the person at this PC, worked out from how it's set up, what
//! it sees, and what they were just doing. Most people never open Settings, so what is worth turning on there comes
//! to them: as a notification, at the moment it would have helped, with one button that does it.
//!
//! **From how the PC is set up** ([`all`], asked at start and by Settings' Overview):
//!
//! | Suggestion              | When                                                          | Its button             |
//! |-------------------------|---------------------------------------------------------------|------------------------|
//! | Index all drives        | instant search is off or stopped                              | Index all drives (UAC) |
//! | A drive is nearly full  | a fixed drive has under 10% and under 15 GB free              | See what takes space   |
//! | Tool updates            | tools Glint downloaded have newer releases it's tested with   | Update them            |
//! | XYplorer's marks        | a `tag.dat` with marks in it that was not brought in yet      | Bring them in          |
//! | Follow XYplorer         | XYplorer is there, and what it gains isn't followed yet       | Follow XYplorer        |
//! | The shortcut needs Glint| a Quick search shortcut is set but Glint quits when closed    | Keep it in the tray    |
//! | Start with Windows      | Glint stays in the tray but doesn't start at sign-in          | Start with Windows     |
//! | Match Windows' mode     | a theme chosen by hand isn't Windows' light or dark mode      | Follow Windows         |
//! | Connect a cloud         | rclone is installed and no cloud is connected                 | Connect a cloud        |
//! | zoxide                  | zoxide is installed and its history isn't shared              | Share history          |
//! | An AI agent             | an agent that speaks MCP is here and doesn't know Glint       | Connect it             |
//!
//! **From what was just done** ([`moments`], heard from the explorer by the workspace):
//!
//! | Suggestion              | When                                                          | Its button             |
//! |-------------------------|---------------------------------------------------------------|------------------------|
//! | A folder for the Gallery| the Gallery is opened and photos lie outside it; or a folder  | Add to the Gallery     |
//! |                         | of photos that isn't in it is opened                          |                        |
//! | The copy queue          | a copy or a move took long (eight seconds and more)           | Use the copy queue     |
//! | Ask before deleting     | a deletion was undone                                         | Ask first              |
//! | Batch rename            | three items of one folder were renamed one after another      | Rename these…          |
//! | Quick Look              | several files of one folder were opened one after another     | Show me                |
//! | Two panes               | files went from one folder to another three times             | Open two panes         |
//! | Your own labels         | labels are used and still have the names they came with       | Name the labels        |
//! | Finding marks           | a tag or a comment was written                                | Find them              |
//! | A folder rule           | one kind of file went from one folder to another three times  | Make it a rule…        |
//!
//! **Not a nag.** One suggestion is on screen at a time, [`GAP`] apart at least and [`MOST_A_RUN`] in a run of
//! Glint; never over a dialog, and one that comes while Glint is in the background waits until it is in front.
//! A suggestion that came up is not shown again for [`QUIET_FOR`] days, and after [`OFFERS`] times with no press not
//! at all (but for what can be true anew: a full drive); one whose button was pressed has taught what it had to, and
//! "Don't suggest this again" (on the notification, or the Overview's cross) puts one away for good.
//! Settings' Overview has the switch for all of it (`settings.tips`): off, they only wait in the Overview's list.
//! What was shown, done and put away is kept in the saved state's notices (`suggest-<id>`), as the first-run
//! notices are. Nothing is sent anywhere and no model is involved: rules, counts and what the PC says.

pub mod moments;
mod toast;

use crate::components::settings::IndexState;
use crate::kit::prelude::*;
use crate::{persistence, state, t};
use glint_model::format::{format_count, format_size, plural};
use glint_model::paths::base_name;
use glint_model::store::{Settings, Theme};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How long after start a suggestion may come up as a notification: once the first folders and the service's
/// own notices have had their turn.
pub const OFFER_AFTER: Duration = Duration::from_secs(40);
/// How many days a suggestion that came up waits before it may come up again.
pub const QUIET_FOR: u64 = 7;
/// How long a suggestion's notification stays when nothing is done with it.
pub const SHOWN_FOR: Duration = Duration::from_secs(30);
/// The least time between two suggestions.
pub const GAP: Duration = Duration::from_secs(4 * 60);
/// The most suggestions that come up in one run of Glint.
pub const MOST_A_RUN: usize = 3;
/// How many times one may come up without its button being pressed: after that it isn't wanted, and only waits in
/// the Overview.
pub const OFFERS: usize = 3;

/// What can be suggested: first what the PC's setup says, most useful first, then what a moment brings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    IndexAllDrives,
    FullDrive,
    ToolUpdates,
    XyplorerMarks,
    XyplorerFollow,
    ShortcutNeedsTray,
    StartWithWindows,
    MatchWindowsTheme,
    ConnectCloud,
    ShareWithZoxide,
    ConnectAgent,
    GalleryFolder,
    CopyQueue,
    AskBeforeDelete,
    BatchRename,
    QuickLook,
    TwoPanes,
    NameLabels,
    FindMarks,
    FolderRule,
}

impl Kind {
    /// Every kind, in the order of the enum.
    pub const ALL: [Kind; 20] = [
        Kind::IndexAllDrives,
        Kind::FullDrive,
        Kind::ToolUpdates,
        Kind::XyplorerMarks,
        Kind::XyplorerFollow,
        Kind::ShortcutNeedsTray,
        Kind::StartWithWindows,
        Kind::MatchWindowsTheme,
        Kind::ConnectCloud,
        Kind::ShareWithZoxide,
        Kind::ConnectAgent,
        Kind::GalleryFolder,
        Kind::CopyQueue,
        Kind::AskBeforeDelete,
        Kind::BatchRename,
        Kind::QuickLook,
        Kind::TwoPanes,
        Kind::NameLabels,
        Kind::FindMarks,
        Kind::FolderRule,
    ];

    /// Its name in the saved notices and in element ids. Never renamed: what was put away stays put away.
    pub fn id(self) -> &'static str {
        match self {
            Kind::IndexAllDrives => "index-all-drives",
            Kind::FullDrive => "full-drive",
            Kind::ToolUpdates => "tool-updates",
            Kind::XyplorerMarks => "xyplorer-marks",
            Kind::XyplorerFollow => "xyplorer-follow",
            Kind::ShortcutNeedsTray => "shortcut-needs-tray",
            Kind::StartWithWindows => "start-with-windows",
            Kind::MatchWindowsTheme => "match-windows-theme",
            Kind::ConnectCloud => "connect-cloud",
            Kind::ShareWithZoxide => "share-with-zoxide",
            Kind::ConnectAgent => "connect-agent",
            Kind::GalleryFolder => "gallery-folder",
            Kind::CopyQueue => "copy-queue",
            Kind::AskBeforeDelete => "ask-before-delete",
            Kind::BatchRename => "batch-rename",
            Kind::QuickLook => "quick-look",
            Kind::TwoPanes => "two-panes",
            Kind::NameLabels => "name-labels",
            Kind::FindMarks => "find-marks",
            Kind::FolderRule => "folder-rule",
        }
    }

    /// Whether it may come up as the notification a while after start. The rest come at their own moment
    /// (the tray when the shortcut is pressed, the theme when Windows changes its own) or wait in the Overview.
    pub fn may_notify(self) -> bool {
        matches!(
            self,
            Kind::IndexAllDrives
                | Kind::FullDrive
                | Kind::ToolUpdates
                | Kind::XyplorerMarks
                | Kind::XyplorerFollow
                | Kind::ConnectAgent
                | Kind::ShareWithZoxide
        )
    }

    /// Whether pressing its button settles it for good: it turned a setting on or showed a way of working, and
    /// asking again would be nagging. The others are about something that can be true again (a full drive, another
    /// folder of photos).
    pub fn teaches(self) -> bool {
        matches!(
            self,
            Kind::ShortcutNeedsTray
                | Kind::StartWithWindows
                | Kind::MatchWindowsTheme
                | Kind::ShareWithZoxide
                | Kind::ConnectAgent
                | Kind::CopyQueue
                | Kind::AskBeforeDelete
                | Kind::BatchRename
                | Kind::QuickLook
                | Kind::TwoPanes
                | Kind::NameLabels
                | Kind::FindMarks
        )
    }

    /// Whether it is about something that can be true anew each time (a drive that filled up again, another
    /// folder of photos): such a one keeps coming, a week apart, however often it was let go by.
    pub fn recurs(self) -> bool {
        matches!(
            self,
            Kind::FullDrive | Kind::ToolUpdates | Kind::GalleryFolder | Kind::FolderRule
        )
    }

    pub fn icon(self) -> IconName {
        match self {
            Kind::IndexAllDrives => IconName::Zap,
            Kind::FullDrive => IconName::HardDrive,
            Kind::ToolUpdates => IconName::Download,
            Kind::XyplorerMarks | Kind::XyplorerFollow | Kind::NameLabels => IconName::Tag,
            Kind::ShortcutNeedsTray => IconName::Keyboard,
            Kind::StartWithWindows => IconName::Power,
            Kind::MatchWindowsTheme => IconName::SunMoon,
            Kind::ConnectCloud => IconName::Cloud,
            Kind::ShareWithZoxide => IconName::ClockArrowLeft,
            Kind::ConnectAgent => IconName::Sparkles,
            Kind::GalleryFolder => IconName::Images,
            Kind::CopyQueue => IconName::ListChecks,
            Kind::AskBeforeDelete => IconName::Trash,
            Kind::BatchRename => IconName::PenLine,
            Kind::QuickLook => IconName::Eye,
            Kind::TwoPanes => IconName::Columns2,
            Kind::FindMarks => IconName::Search,
            Kind::FolderRule => IconName::WandSparkles,
        }
    }
}

/// What a suggestion is about, for its button.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum About {
    #[default]
    Nothing,
    /// A drive's root ("C:\").
    Drive(String),
    /// XYplorer's `tag.dat`, and whether it was brought in before.
    TagDat { path: String, brought_in: bool },
    /// A folder (to show in the Gallery, to open beside the one shown).
    Folder(String),
    /// Files (the ones just renamed).
    Files(Vec<String>),
    /// What to put in the search box.
    Search(String),
    /// A folder rule to make: files named like `names` in `from` go to `to`.
    Rule { from: String, names: String, to: String },
}

/// One suggestion, worded for this PC.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub kind: Kind,
    pub title: SharedString,
    pub body: SharedString,
    pub button: SharedString,
    /// A second, quieter button, where there is another way to say yes ("Look first…").
    pub also: Option<SharedString>,
    pub about: About,
}

impl Suggestion {
    fn new(kind: Kind, title: SharedString, body: SharedString, button: SharedString) -> Self {
        Suggestion {
            kind,
            title,
            body,
            button,
            also: None,
            about: About::Nothing,
        }
    }

    fn about(mut self, about: About) -> Self {
        self.about = about;
        self
    }

    fn also(mut self, also: SharedString) -> Self {
        self.also = Some(also);
        self
    }
}

/// A fixed drive's room: its root, free and total bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Room {
    pub root: String,
    pub free: u64,
    pub total: u64,
}

/// XYplorer's `tag.dat` as found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XyFound {
    pub path: String,
    /// How many items it marks.
    pub items: u64,
    /// It was brought into Glint before (what it gains since is what following is for).
    pub brought_in: bool,
}

/// What suggestions are worked out from. A fact not known yet is None, and what needs it waits.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// Instant search's state.
    pub index: Option<IndexState>,
    /// How many of the tools Glint downloaded have an update.
    pub tool_updates: Option<usize>,
    /// The fixed drives' room.
    pub drives: Option<Vec<Room>>,
    pub keep_in_tray: bool,
    pub start_with_windows: bool,
    /// A Quick search shortcut is set.
    pub quick_shortcut: bool,
    pub theme: Option<Theme>,
    /// Windows is in dark mode.
    pub windows_dark: bool,
    /// rclone is there.
    pub rclone: Option<bool>,
    /// How many clouds are connected.
    pub clouds: Option<usize>,
    /// XYplorer's `tag.dat`: None while unknown and when none is found.
    pub xyplorer: Option<XyFound>,
    /// What XYplorer gains is followed (the setting).
    pub follow_xyplorer: bool,
    /// zoxide is on the PATH: None while unknown.
    pub zoxide: Option<bool>,
    /// Folder history is shared with zoxide (the setting).
    pub zoxide_shared: bool,
    /// An AI agent found here whose settings don't name Glint, by its name.
    pub agent: Option<String>,
}

impl Facts {
    /// The facts the settings themselves give, and what was last learned of XYplorer.
    pub fn from_settings(cx: &App) -> Facts {
        let s = state::settings(cx);
        Facts {
            keep_in_tray: s.keep_in_tray,
            start_with_windows: s.start_with_windows,
            quick_shortcut: s.quick_shortcut.as_deref().is_some_and(|k| !k.trim().is_empty()),
            theme: Some(s.theme),
            windows_dark: matches!(
                cx.window_appearance(),
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
            xyplorer: cx.try_global::<XyplorerKnown>().and_then(|known| known.0.clone()),
            follow_xyplorer: s.follow_xyplorer,
            zoxide_shared: s.zoxide,
            ..Facts::default()
        }
    }
}

/// What was last learned of XYplorer's `tag.dat` (by the Overview or the offer), for [`Facts::from_settings`].
struct XyplorerKnown(Option<XyFound>);
impl Global for XyplorerKnown {}

/// How the `tag.dat` fact is asked: a test replaces it.
struct XyplorerAsk(Rc<dyn Fn() -> XyplorerFuture>);
impl Global for XyplorerAsk {}

type XyplorerFuture = Pin<Box<dyn Future<Output = Option<XyFound>>>>;

/// Replaces how the `tag.dat` fact is asked (tests).
#[cfg(test)]
fn ask_xyplorer_with(cx: &mut App, ask: impl Fn() -> XyplorerFuture + 'static) {
    cx.set_global(XyplorerAsk(Rc::new(ask)));
}

/// Asks for XYplorer's `tag.dat` and whether it was brought in, off the UI thread (reading only). None when
/// unknown or not there.
pub fn ask_xyplorer(cx: &App) -> XyplorerFuture {
    if let Some(ask) = cx.try_global::<XyplorerAsk>() {
        return (ask.0)();
    }
    if !cx.has_global::<crate::core_bridge::Core>() {
        return Box::pin(async { None });
    }
    Box::pin(async {
        let found = crate::core_bridge::run(async {
            glint_core::fsx::blocking(|| {
                Ok(glint_core::marks::xyplorer_found().map(|found| XyFound {
                    brought_in: glint_core::marks::apply::brought_in(&found.path),
                    path: found.path,
                    items: found.items,
                }))
            })
            .await
        })
        .await;
        found.ok().and_then(Result::ok).flatten()
    })
}

/// Keeps what [`ask_xyplorer`] answered, for the Overview's list.
pub fn know_xyplorer(cx: &mut App, found: Option<XyFound>) {
    cx.set_global(XyplorerKnown(found));
}

/// The facts that take a moment and that only the PC can answer: the drives' room, XYplorer, zoxide, an agent.
#[derive(Clone, Debug, Default)]
pub struct Looked {
    pub drives: Option<Vec<Room>>,
    pub xyplorer: Option<XyFound>,
    pub zoxide: Option<bool>,
    pub agent: Option<String>,
}

/// Asks for [`Looked`], off the UI thread. All None without core (tests).
pub fn look(cx: &App) -> Pin<Box<dyn Future<Output = Looked>>> {
    let xyplorer = ask_xyplorer(cx);
    let core = cx.has_global::<crate::core_bridge::Core>();
    Box::pin(async move {
        let xyplorer = xyplorer.await;
        if !core {
            return Looked {
                xyplorer,
                ..Looked::default()
            };
        }
        let drives = crate::core_bridge::run(glint_core::fsx::list_drives())
            .await
            .ok()
            .and_then(Result::ok)
            .map(|drives| {
                drives
                    .into_iter()
                    .filter(|d| d.kind == "fixed")
                    .map(|d| Room {
                        root: d.path,
                        free: d.free,
                        total: d.total,
                    })
                    .collect::<Vec<_>>()
            });
        let zoxide = crate::core_bridge::run(glint_core::zoxide::zoxide_installed())
            .await
            .ok();
        let agent = crate::core_bridge::run(async {
            glint_core::fsx::blocking(|| Ok(glint_core::mcp::agent_without_glint().map(str::to_string))).await
        })
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
        Looked {
            drives,
            xyplorer,
            zoxide,
            agent,
        }
    })
}

/// A drive is nearly full: under a tenth and under 15 GB free (a big drive with 100 GB left is fine).
fn nearly_full(room: &Room) -> bool {
    room.total > 0 && (room.free as f64) < room.total as f64 * 0.10 && room.free < 15 << 30
}

/// "1,284 items", from core's count.
fn items_of(n: u64) -> String {
    plural(usize::try_from(n).unwrap_or(usize::MAX), "item")
}

/// Every suggestion the PC's setup gives, most useful first.
pub fn all(facts: &Facts) -> Vec<Suggestion> {
    let mut out = Vec::new();
    if matches!(facts.index, Some(IndexState::Off | IndexState::Stopped)) {
        out.push(Suggestion::new(
            Kind::IndexAllDrives,
            t!("Find any file on every drive, instantly"),
            t!("Instant search reads each drive's file table once and keeps up with every change, so any name is found as you type. Windows asks for administrator approval."),
            t!("Index all drives"),
        ));
    }
    if let Some(full) = facts
        .drives
        .iter()
        .flatten()
        .filter(|room| nearly_full(room))
        .min_by_key(|room| room.free)
    {
        let letter = full.root.trim_end_matches('\\').to_string();
        out.push(
            Suggestion::new(
                Kind::FullDrive,
                t!("{drive} is nearly full", drive = letter),
                t!(
                    "{free} free of {total}. See what takes the space, and clear what you don't need.",
                    free = format_size(full.free),
                    total = format_size(full.total)
                ),
                t!("See what takes space"),
            )
            .about(About::Drive(full.root.clone())),
        );
    }
    if let Some(n) = facts.tool_updates.filter(|n| *n > 0) {
        out.push(Suggestion::new(
            Kind::ToolUpdates,
            if n == 1 {
                t!("A tool has an update")
            } else {
                t!("{n} tools have updates", n = n)
            },
            t!("Newer versions Glint is tested with are ready: each is checked before it replaces the old one."),
            if n == 1 { t!("Update it") } else { t!("Update them") },
        ));
    }
    if let Some(found) = &facts.xyplorer {
        let about = About::TagDat {
            path: found.path.clone(),
            brought_in: found.brought_in,
        };
        if found.items > 0 && !found.brought_in {
            // What it marks comes along with one press; the preview is there for whoever wants to look first.
            out.push(
                Suggestion::new(
                    Kind::XyplorerMarks,
                    t!("Bring your XYplorer labels, tags and comments"),
                    t!(
                        "{n} marked in XYplorer. One press brings them here, with Undo; XYplorer's own file is only read.",
                        n = items_of(found.items)
                    ),
                    t!("Bring them in"),
                )
                .also(t!("Look first…"))
                .about(about),
            );
        } else if !facts.follow_xyplorer {
            out.push(
                Suggestion::new(
                    Kind::XyplorerFollow,
                    t!("Keep in step with XYplorer"),
                    if found.brought_in {
                        t!("What you label, tag or comment in XYplorer from now on can show in Glint by itself, each time Glint comes to the front.")
                    } else {
                        t!("XYplorer is on this PC. What you label, tag or comment there can show in Glint too, each time Glint comes to the front.")
                    },
                    t!("Follow XYplorer"),
                )
                .about(about),
            );
        }
    }
    if facts.quick_shortcut && !facts.keep_in_tray {
        out.push(Suggestion::new(
            Kind::ShortcutNeedsTray,
            t!("Keep Quick search's shortcut working"),
            t!("The shortcut only works while Glint runs. Kept in the tray, it's there even after you close the window."),
            t!("Keep it in the tray"),
        ));
    }
    if facts.keep_in_tray && !facts.start_with_windows {
        out.push(Suggestion::new(
            Kind::StartWithWindows,
            t!("Have Glint ready when you sign in"),
            t!("It starts in the tray, so the first window and Quick search open at once."),
            t!("Start with Windows"),
        ));
    }
    let mismatched = match facts.theme {
        Some(Theme::Light) => facts.windows_dark,
        Some(Theme::Dark) => !facts.windows_dark,
        _ => false,
    };
    if mismatched {
        out.push(Suggestion::new(
            Kind::MatchWindowsTheme,
            if facts.windows_dark {
                t!("Windows is in dark mode")
            } else {
                t!("Windows is in light mode")
            },
            t!("Follow Windows' theme, and Glint changes with it, at night too."),
            t!("Follow Windows"),
        ));
    }
    if facts.rclone == Some(true) && facts.clouds == Some(0) {
        out.push(Suggestion::new(
            Kind::ConnectCloud,
            t!("Connect a cloud"),
            t!("rclone is installed: Google Drive, OneDrive, Dropbox and 60 more can show here as folders."),
            t!("Connect a cloud"),
        ));
    }
    if facts.zoxide == Some(true) && !facts.zoxide_shared {
        out.push(Suggestion::new(
            Kind::ShareWithZoxide,
            t!("Share folder history with zoxide"),
            t!("zoxide is installed: the folders you cd to in a terminal can rank in Ctrl+K and Quick search, and z can find the folders you open here."),
            t!("Share history"),
        ));
    }
    if let Some(agent) = &facts.agent {
        out.push(Suggestion::new(
            Kind::ConnectAgent,
            t!("Let {agent} find your files", agent = agent.clone()),
            t!(
                "{agent} is on this PC. Through Glint it can search every drive in milliseconds, size folders and tidy up, asking you before it changes anything.",
                agent = agent.clone()
            ),
            t!("Connect it"),
        ));
    }
    out
}

// ---- What was shown, done and put away -------------------------------------------------------------------------

fn key(kind: Kind) -> String {
    format!("suggest-{}", kind.id())
}

/// Days since 1970, for "shown lately".
fn today() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400)
}

fn noted(cx: &App, kind: Kind) -> Option<String> {
    persistence::app(cx).and_then(|app| app.notices.get(&key(kind)).cloned())
}

/// Put away for good.
pub fn dismissed(cx: &App, kind: Kind) -> bool {
    noted(cx, kind).as_deref() == Some("never")
}

/// Puts `kind` away for good ("Don't suggest this again", or the Overview's cross).
pub fn dismiss(cx: &mut App, kind: Kind) {
    persistence::update_app(cx, |app| {
        app.notices.insert(key(kind), "never".to_string());
    });
    forget_raised(cx, kind);
}

/// Its button was pressed, and it is of a kind that is settled by that ([`Kind::teaches`]).
pub fn settled(cx: &App, kind: Kind) -> bool {
    noted(cx, kind).as_deref() == Some("done")
}

/// The day it last came up and how many times it has: the notice is `<day>` (once, as 0.16 wrote it) or
/// `<day>:<times>`.
fn shown(cx: &App, kind: Kind) -> Option<(u64, usize)> {
    let noted = noted(cx, kind)?;
    let (day, times) = noted.split_once(':').unwrap_or((noted.as_str(), "1"));
    Some((day.parse().ok()?, times.parse().unwrap_or(1)))
}

/// Came up within [`QUIET_FOR`] days.
fn shown_lately(cx: &App, kind: Kind, today: u64) -> bool {
    shown(cx, kind).is_some_and(|(day, _)| today.saturating_sub(day) < QUIET_FOR)
}

/// Came up [`OFFERS`] times and its button was never pressed: it isn't wanted. Not said of what [`Kind::recurs`].
fn let_go_by(cx: &App, kind: Kind) -> bool {
    !kind.recurs() && shown(cx, kind).is_some_and(|(_, times)| times >= OFFERS)
}

fn note_shown(cx: &mut App, kind: Kind, today: u64) {
    let times = shown(cx, kind).map_or(0, |(_, times)| times) + 1;
    persistence::update_app(cx, |app| {
        app.notices.insert(key(kind), format!("{today}:{times}"));
    });
}

/// Neither put away nor settled: it may still be listed.
fn open_to(cx: &App, kind: Kind) -> bool {
    !dismissed(cx, kind) && !settled(cx, kind)
}

/// The suggestions for the Overview: what the PC's setup gives, then what this run's moments brought that still
/// holds, without the ones put away or settled.
pub fn current(facts: &Facts, cx: &App) -> Vec<Suggestion> {
    let mut list: Vec<Suggestion> = all(facts).into_iter().filter(|s| open_to(cx, s.kind)).collect();
    if let Some(run) = cx.try_global::<Run>() {
        let settings = state::settings(cx);
        for raised in &run.raised {
            let listed = list.iter().any(|s| s.kind == raised.kind);
            if !listed && open_to(cx, raised.kind) && moments::still_holds(raised, settings) {
                list.push(raised.clone());
            }
        }
    }
    list
}

/// The one to come up a while after start, if any: the first that may, wasn't put away or settled and didn't
/// come up lately.
pub fn to_offer(facts: &Facts, cx: &App, today: u64) -> Option<Suggestion> {
    all(facts).into_iter().find(|s| {
        s.kind.may_notify() && open_to(cx, s.kind) && !shown_lately(cx, s.kind, today) && !let_go_by(cx, s.kind)
    })
}

// ---- This run --------------------------------------------------------------------------------------------------

/// What this run of Glint has suggested so far.
#[derive(Default)]
struct Run {
    /// How many came up.
    shown: usize,
    /// When the last one did.
    last: Option<Instant>,
    /// What moments brought, newest last, for the Overview.
    raised: Vec<Suggestion>,
    /// One that came while Glint was in the background, kept for when it is in front.
    held: Option<Suggestion>,
}

impl Global for Run {}

/// Whether one more may come up now, by this run's pace alone.
fn pace_allows(shown: usize, last: Option<Instant>, now: Instant) -> bool {
    shown < MOST_A_RUN && last.is_none_or(|last| now.saturating_duration_since(last) >= GAP)
}

fn forget_raised(cx: &mut App, kind: Kind) {
    if cx.has_global::<Run>() {
        let run = cx.global_mut::<Run>();
        run.raised.retain(|s| s.kind != kind);
        if run.held.as_ref().is_some_and(|s| s.kind == kind) {
            run.held = None;
        }
    }
}

/// Debug builds leave every notification out with `GLINT_SUGGESTIONS=off`: the e2e run's clicks mustn't land on
/// one.
fn silenced() -> bool {
    cfg!(debug_assertions) && std::env::var("GLINT_SUGGESTIONS").is_ok_and(|v| v == "off")
}

/// A moment's suggestion comes up as a notification, if it may (see the module notes); either way it is kept for
/// the Overview's list. Whether it came up.
pub fn raise(suggestion: Suggestion, window: &mut Window, cx: &mut App) -> bool {
    let kind = suggestion.kind;
    if !open_to(cx, kind) {
        return false;
    }
    {
        let run = cx.default_global::<Run>();
        run.raised.retain(|s| s.kind != kind);
        run.raised.push(suggestion.clone());
    }
    let day = today();
    if silenced() || !state::settings(cx).tips || shown_lately(cx, kind, day) || let_go_by(cx, kind) {
        return false;
    }
    let run = cx.default_global::<Run>();
    if !pace_allows(run.shown, run.last, Instant::now()) {
        return false;
    }
    // Over a dialog it would be in the way of what the person is in the middle of; in the background nobody
    // would see it before it left.
    if window.has_active_dialog(cx) {
        return false;
    }
    if !window.is_window_active() {
        cx.default_global::<Run>().held = Some(suggestion);
        return false;
    }
    show(suggestion, day, window, cx);
    true
}

/// The window came to the front: a suggestion that waited for that comes up now.
pub fn window_active(window: &mut Window, cx: &mut App) {
    let held = cx.try_global::<Run>().and_then(|run| run.held.clone());
    if let Some(held) = held {
        cx.default_global::<Run>().held = None;
        if moments::still_holds(&held, state::settings(cx)) {
            raise(held, window, cx);
        }
    }
}

/// Puts `suggestion` on screen and counts it.
fn show(suggestion: Suggestion, day: u64, window: &mut Window, cx: &mut App) {
    note_shown(cx, suggestion.kind, day);
    {
        let run = cx.default_global::<Run>();
        run.shown += 1;
        run.last = Some(Instant::now());
    }
    toast::push(suggestion, window, cx);
}

// ---- Doing what they say ---------------------------------------------------------------------------------------

/// What pressing a button did, said back with Undo: a setting that went on is one press from off again.
struct DoneToast;

/// Says `words`, with an Undo that runs `undo` on the settings.
fn say_done(window: &mut Window, cx: &mut App, words: SharedString, undo: impl Fn(&mut Settings) + 'static) {
    let undo = Rc::new(undo);
    let note = Notification::success(words).id::<DoneToast>().action(move |_, _, _| {
        let undo = undo.clone();
        Button::new("suggestion-undo")
            .label(t!("Undo"))
            .on_click(move |_, window, cx| {
                window.remove_notification::<DoneToast>(cx);
                state::update(cx, |settings| undo(settings));
            })
    });
    window.push_notification(note, cx);
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_secs(10)).await;
        let _ = handle.update(cx, |_, window, cx| window.remove_notification::<DoneToast>(cx));
    })
    .detach();
}

/// Turns a setting on with `set` and says so, with Undo (`unset`).
fn turn_on(
    window: &mut Window,
    cx: &mut App,
    words: SharedString,
    set: impl FnOnce(&mut Settings),
    unset: impl Fn(&mut Settings) + 'static,
) {
    state::update(cx, set);
    say_done(window, cx, words, unset);
}

/// Does what `suggestion`'s button says, and settles it where that settles it. Settings' own ones (update the
/// tools, connect a cloud or an agent) open Settings there.
pub fn act(suggestion: &Suggestion, window: &mut Window, cx: &mut App) {
    let kind = suggestion.kind;
    match (kind, &suggestion.about) {
        (Kind::IndexAllDrives, _) => crate::notices::turn_on_instant_search(window, cx),
        (Kind::FullDrive, About::Drive(root)) => crate::dialogs::cleanup::open(window, cx, root),
        (Kind::ToolUpdates, _) => {
            let view = crate::components::settings::open_at(window, cx, Some("tools"));
            let tools = view.read(cx).tools.clone();
            tools.update(cx, |tools, cx| tools.update_all_once_known(window, cx));
        }
        (Kind::XyplorerMarks, About::TagDat { path, .. }) => {
            #[cfg(test)]
            tests::OPENED.with(|opened| opened.borrow_mut().push(format!("bring {path}")));
            crate::dialogs::marks_import::bring_in(window, cx, path.clone(), false);
        }
        // Brought in before: following starts with the switch. Never brought in (nothing was marked there yet):
        // it is brought in once, with nothing in it, so there is something to follow.
        (Kind::XyplorerFollow, About::TagDat { path, brought_in }) => {
            if *brought_in {
                turn_on(
                    window,
                    cx,
                    t!("Glint follows XYplorer. Settings → Files has the switch."),
                    |s| s.follow_xyplorer = true,
                    |s| s.follow_xyplorer = false,
                );
            } else {
                #[cfg(test)]
                tests::OPENED.with(|opened| opened.borrow_mut().push(format!("follow {path}")));
                crate::dialogs::marks_import::bring_in(window, cx, path.clone(), true);
            }
        }
        (Kind::ShortcutNeedsTray, _) => turn_on(
            window,
            cx,
            t!("Glint stays in the tray when its window closes. Settings → Quick search changes it."),
            |s| s.keep_in_tray = true,
            |s| s.keep_in_tray = false,
        ),
        (Kind::StartWithWindows, _) => turn_on(
            window,
            cx,
            t!("Glint starts with Windows, in the tray. Settings → Quick search changes it."),
            |s| s.start_with_windows = true,
            |s| s.start_with_windows = false,
        ),
        (Kind::MatchWindowsTheme, _) => {
            let was = state::settings(cx).theme;
            turn_on(
                window,
                cx,
                t!("Glint follows Windows' light and dark mode"),
                |s| s.theme = Theme::System,
                move |s| s.theme = was,
            );
        }
        (Kind::ConnectCloud, _) => {
            crate::components::settings::open_at(window, cx, Some("clouds"));
        }
        (Kind::ShareWithZoxide, _) => turn_on(
            window,
            cx,
            t!("Folder history is shared with zoxide. Settings → History has the switch."),
            |s| s.zoxide = true,
            |s| s.zoxide = false,
        ),
        (Kind::ConnectAgent, _) => {
            crate::components::settings::open_at(window, cx, Some("ai-agents"));
        }
        (Kind::GalleryFolder, About::Folder(folder)) => moments::add_to_gallery(folder, window, cx),
        (Kind::CopyQueue, _) => turn_on(
            window,
            cx,
            t!("Copies and moves go through Glint's queue from now on. Settings → Files changes it."),
            |s| s.copy_queue = true,
            |s| s.copy_queue = false,
        ),
        (Kind::AskBeforeDelete, _) => turn_on(
            window,
            cx,
            t!("Glint asks before moving anything to the Recycle Bin. Settings → Files changes it."),
            |s| s.confirm_delete = true,
            |s| s.confirm_delete = false,
        ),
        (Kind::BatchRename, About::Files(paths)) => crate::dialogs::batch_rename::open(window, cx, paths.clone()),
        (Kind::QuickLook, _) => crate::components::preview::quick_look::open(window, cx),
        (Kind::TwoPanes, About::Folder(folder)) => moments::open_beside(folder, cx),
        (Kind::NameLabels, _) => {
            crate::components::settings::open_at(window, cx, Some("labels"));
        }
        (Kind::FindMarks, About::Search(query)) => moments::search_for(query, cx),
        (Kind::FolderRule, About::Rule { from, names, to }) => moments::make_rule(from, names, to, window, cx),
        _ => {}
    }
    settle(cx, kind);
}

/// `kind`'s button was pressed: one that taught what it had to ([`Kind::teaches`]) is not suggested again, and
/// the Overview stops listing what a moment brought.
pub fn settle(cx: &mut App, kind: Kind) {
    if kind.teaches() {
        persistence::update_app(cx, |app| {
            app.notices.insert(key(kind), "done".to_string());
        });
    }
    forget_raised(cx, kind);
}

/// The second button's way ([`Suggestion::also`]): XYplorer's marks are looked at first, in the preview.
pub fn act_also(suggestion: &Suggestion, window: &mut Window, cx: &mut App) {
    if let (Kind::XyplorerMarks, About::TagDat { path, .. }) = (suggestion.kind, &suggestion.about) {
        #[cfg(test)]
        tests::OPENED.with(|opened| opened.borrow_mut().push(format!("look {path}")));
        crate::dialogs::marks_import::open(window, cx, Some(path.clone()));
    }
    forget_raised(cx, suggestion.kind);
}

// ---- A while after start ---------------------------------------------------------------------------------------

/// How long after start the first one may come: [`OFFER_AFTER`], or what a debug build is told
/// (`GLINT_SUGGEST_AFTER`, in milliseconds, for a look at one).
fn offer_after() -> Duration {
    let told = cfg!(debug_assertions)
        .then(|| std::env::var("GLINT_SUGGEST_AFTER").ok())
        .flatten()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis);
    told.unwrap_or(OFFER_AFTER)
}

/// A while after start, the most useful suggestion as a notification (see the module notes). Facts that take a moment
/// (instant search's state, the tools, the drives, XYplorer) are asked off the UI thread. Once per process; the main
/// window's.
pub fn offer(window: &mut Window, cx: &mut App) {
    struct Offered;
    impl Global for Offered {}
    if silenced()
        || crate::platform::secondary::is_secondary()
        || cx.has_global::<Offered>()
        || !cx.has_global::<crate::core_bridge::Core>()
    {
        return;
    }
    cx.set_global(Offered);
    let status = crate::notices::index_status(cx);
    let looking = look(cx);
    window
        .spawn(cx, async move |cx| {
            cx.background_executor().timer(offer_after()).await;
            let index = match status {
                Some(asked) => asked.await.ok().map(|info| info.state),
                None => None,
            };
            let tool_updates = crate::core_bridge::run(async {
                glint_core::fsx::blocking(|| Ok(glint_core::tools::updates_ready().len())).await
            })
            .await
            .ok()
            .and_then(Result::ok);
            let looked = looking.await;
            let _ = cx.update(|window, cx| {
                know_xyplorer(cx, looked.xyplorer.clone());
                let facts = Facts {
                    index,
                    tool_updates,
                    drives: looked.drives,
                    xyplorer: looked.xyplorer,
                    zoxide: looked.zoxide,
                    agent: looked.agent,
                    ..Facts::from_settings(cx)
                };
                if !state::settings(cx).tips {
                    return;
                }
                let day = today();
                if let Some(suggestion) = to_offer(&facts, cx, day) {
                    // In the background, it waits for the window to come to the front.
                    if window.is_window_active() && !window.has_active_dialog(cx) {
                        show(suggestion, day, window, cx);
                    } else {
                        cx.default_global::<Run>().held = Some(suggestion);
                    }
                }
            });
        })
        .detach();
}

/// "3 photos and videos", for the Gallery's suggestions.
pub(crate) fn photos_of(n: usize) -> String {
    if n == 1 {
        t!("1 photo or video").to_string()
    } else {
        t!("{n} photos and videos", n = format_count(n)).to_string()
    }
}

/// A folder's name for a sentence: its last part, or the path itself for a drive.
pub(crate) fn folder_name(path: &str) -> String {
    match base_name(path.trim_end_matches('\\')) {
        "" => path.to_string(),
        name => name.to_string(),
    }
}

#[cfg(test)]
mod tests;
