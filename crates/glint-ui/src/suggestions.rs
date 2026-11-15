//! Smart suggestions: what would make Glint better on this PC, worked out from how it's set up and what it sees.
//! Settings' Overview lists every one that applies, each with its button and a cross that puts it away; a while
//! after Glint starts, the most useful one may come up as a notification.
//!
//! | Suggestion              | When                                                          | Its button             |
//! |-------------------------|---------------------------------------------------------------|------------------------|
//! | Index all drives        | instant search is off or stopped                              | Index all drives (UAC) |
//! | A drive is nearly full  | a fixed drive has under 10% and under 15 GB free              | See what takes space   |
//! | Tool updates            | tools Glint downloaded have newer releases it's tested with   | Update them            |
//! | The shortcut needs Glint| a Quick search shortcut is set but Glint quits when closed    | Keep it in the tray    |
//! | Start with Windows      | Glint stays in the tray but doesn't start at sign-in          | Start with Windows     |
//! | Match Windows' mode     | a theme chosen by hand isn't Windows' light or dark mode      | Follow Windows         |
//! | Connect a cloud         | rclone is installed and no cloud is connected                 | Connect a cloud        |
//!
//! **Not a nag.** A notification comes at most once per start, [`OFFER_AFTER`] in, and only for the first three
//! kinds (the rest wait in the Overview); one shown is not shown again for [`QUIET_FOR`] days, and "Don't suggest
//! this again" puts it away for good (on the notification or the Overview's cross). What was shown and put away is
//! kept in the saved state's notices (`suggest-<id>`), as the first-run notices are.

use crate::components::settings::IndexState;
use crate::kit::prelude::*;
use crate::{persistence, state, t};
use glint_model::format::format_size;
use glint_model::store::Theme;
use std::time::Duration;

/// How long after start a suggestion may come up as a notification: once the first folders and the service's
/// own notices have had their turn.
pub const OFFER_AFTER: Duration = Duration::from_secs(40);
/// How many days a suggestion that came up waits before it may come up again.
pub const QUIET_FOR: u64 = 7;

/// What can be suggested, most useful first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    IndexAllDrives,
    FullDrive,
    ToolUpdates,
    ShortcutNeedsTray,
    StartWithWindows,
    MatchWindowsTheme,
    ConnectCloud,
}

impl Kind {
    /// Its name in the saved notices and in element ids.
    pub fn id(self) -> &'static str {
        match self {
            Kind::IndexAllDrives => "index-all-drives",
            Kind::FullDrive => "full-drive",
            Kind::ToolUpdates => "tool-updates",
            Kind::ShortcutNeedsTray => "shortcut-needs-tray",
            Kind::StartWithWindows => "start-with-windows",
            Kind::MatchWindowsTheme => "match-windows-theme",
            Kind::ConnectCloud => "connect-cloud",
        }
    }

    /// Whether it may come up as a notification (the others wait in Settings' Overview).
    pub fn may_notify(self) -> bool {
        matches!(self, Kind::IndexAllDrives | Kind::FullDrive | Kind::ToolUpdates)
    }

    pub fn icon(self) -> IconName {
        match self {
            Kind::IndexAllDrives => IconName::Zap,
            Kind::FullDrive => IconName::HardDrive,
            Kind::ToolUpdates => IconName::Download,
            Kind::ShortcutNeedsTray => IconName::Keyboard,
            Kind::StartWithWindows => IconName::Power,
            Kind::MatchWindowsTheme => IconName::SunMoon,
            Kind::ConnectCloud => IconName::Cloud,
        }
    }
}

/// One suggestion, worded for this PC.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub kind: Kind,
    pub title: SharedString,
    pub body: SharedString,
    pub button: SharedString,
    /// The drive, for a nearly full one ("C:\").
    pub drive: Option<String>,
}

/// A fixed drive's room: its root, free and total bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Room {
    pub root: String,
    pub free: u64,
    pub total: u64,
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
}

impl Facts {
    /// The facts the settings themselves give.
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
            ..Facts::default()
        }
    }
}

/// A drive is nearly full: under a tenth and under 15 GB free (a big drive with 100 GB left is fine).
fn nearly_full(room: &Room) -> bool {
    room.total > 0 && (room.free as f64) < room.total as f64 * 0.10 && room.free < 15 << 30
}

/// Every suggestion that applies, most useful first.
pub fn all(facts: &Facts) -> Vec<Suggestion> {
    let mut out = Vec::new();
    let mut add = |kind: Kind, title: SharedString, body: SharedString, button: SharedString, drive: Option<String>| {
        out.push(Suggestion {
            kind,
            title,
            body,
            button,
            drive,
        })
    };
    if matches!(facts.index, Some(IndexState::Off | IndexState::Stopped)) {
        add(
            Kind::IndexAllDrives,
            t!("Find any file on every drive, instantly"),
            t!("Instant search reads each drive's file table once and keeps up with every change, so any name is found as you type. Windows asks for administrator approval."),
            t!("Index all drives"),
            None,
        );
    }
    if let Some(full) = facts.drives.iter().flatten().filter(|room| nearly_full(room)).min_by_key(|room| room.free) {
        let letter = full.root.trim_end_matches('\\').to_string();
        add(
            Kind::FullDrive,
            t!("{drive} is nearly full", drive = letter),
            t!(
                "{free} free of {total}. See what takes the space, and clear what you don't need.",
                free = format_size(full.free),
                total = format_size(full.total)
            ),
            t!("See what takes space"),
            Some(full.root.clone()),
        );
    }
    if let Some(n) = facts.tool_updates.filter(|n| *n > 0) {
        add(
            Kind::ToolUpdates,
            if n == 1 {
                t!("A tool has an update")
            } else {
                t!("{n} tools have updates", n = n)
            },
            t!("Newer versions Glint is tested with are ready: each is checked before it replaces the old one."),
            if n == 1 { t!("Update it") } else { t!("Update them") },
            None,
        );
    }
    if facts.quick_shortcut && !facts.keep_in_tray {
        add(
            Kind::ShortcutNeedsTray,
            t!("Keep Quick search's shortcut working"),
            t!("The shortcut only works while Glint runs. Kept in the tray, it's there even after you close the window."),
            t!("Keep it in the tray"),
            None,
        );
    }
    if facts.keep_in_tray && !facts.start_with_windows {
        add(
            Kind::StartWithWindows,
            t!("Have Glint ready when you sign in"),
            t!("It starts in the tray, so the first window and Quick search open at once."),
            t!("Start with Windows"),
            None,
        );
    }
    let mismatched = match facts.theme {
        Some(Theme::Light) => facts.windows_dark,
        Some(Theme::Dark) => !facts.windows_dark,
        _ => false,
    };
    if mismatched {
        add(
            Kind::MatchWindowsTheme,
            if facts.windows_dark {
                t!("Windows is in dark mode")
            } else {
                t!("Windows is in light mode")
            },
            t!("Follow Windows' theme, and Glint changes with it, at night too."),
            t!("Follow Windows"),
            None,
        );
    }
    if facts.rclone == Some(true) && facts.clouds == Some(0) {
        add(
            Kind::ConnectCloud,
            t!("Connect a cloud"),
            t!("rclone is installed: Google Drive, OneDrive, Dropbox and 60 more can show here as folders."),
            t!("Connect a cloud"),
            None,
        );
    }
    out
}

// ---- What was shown and put away ------------------------------------------------------------------------------

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
}

/// Came up within [`QUIET_FOR`] days.
fn shown_lately(cx: &App, kind: Kind, today: u64) -> bool {
    noted(cx, kind)
        .and_then(|day| day.parse::<u64>().ok())
        .is_some_and(|day| today.saturating_sub(day) < QUIET_FOR)
}

fn note_shown(cx: &mut App, kind: Kind, today: u64) {
    persistence::update_app(cx, |app| {
        app.notices.insert(key(kind), today.to_string());
    });
}

/// The suggestions that apply and weren't put away, for the Overview.
pub fn current(facts: &Facts, cx: &App) -> Vec<Suggestion> {
    all(facts).into_iter().filter(|s| !dismissed(cx, s.kind)).collect()
}

/// The one to come up as a notification now, if any: the first that may, wasn't put away and didn't come up lately.
pub fn to_offer(facts: &Facts, cx: &App, today: u64) -> Option<Suggestion> {
    all(facts)
        .into_iter()
        .find(|s| s.kind.may_notify() && !dismissed(cx, s.kind) && !shown_lately(cx, s.kind, today))
}

// ---- Doing what they say ---------------------------------------------------------------------------------------

/// Does what `suggestion`'s button says. Settings' own ones (update the tools, connect a cloud) open Settings there.
pub fn act(suggestion: &Suggestion, window: &mut Window, cx: &mut App) {
    match suggestion.kind {
        Kind::IndexAllDrives => crate::notices::turn_on_instant_search(window, cx),
        Kind::FullDrive => {
            if let Some(root) = &suggestion.drive {
                crate::dialogs::cleanup::open(window, cx, root);
            }
        }
        Kind::ToolUpdates => {
            let view = crate::components::settings::open_at(window, cx, Some("tools"));
            let tools = view.read(cx).tools.clone();
            tools.update(cx, |tools, cx| tools.update_all_once_known(window, cx));
        }
        Kind::ShortcutNeedsTray => state::update(cx, |s| s.keep_in_tray = true),
        Kind::StartWithWindows => state::update(cx, |s| s.start_with_windows = true),
        Kind::MatchWindowsTheme => state::update(cx, |s| s.theme = Theme::System),
        Kind::ConnectCloud => {
            crate::components::settings::open_at(window, cx, Some("clouds"));
        }
    }
}

// ---- The notification -----------------------------------------------------------------------------------------

/// The suggestion's notification, keyed by its kind.
struct SuggestionToast;

/// A while after start, the most useful suggestion as a notification (see the module notes). Facts that take a moment
/// (instant search's state, the tools, the drives) are asked off the UI thread. Once per process; the main window's.
pub fn offer(window: &mut Window, cx: &mut App) {
    struct Offered;
    impl Global for Offered {}
    // Debug builds leave it out with GLINT_SUGGESTIONS=off: the e2e run's clicks mustn't land on one.
    let off = cfg!(debug_assertions) && std::env::var("GLINT_SUGGESTIONS").is_ok_and(|v| v == "off");
    if off || crate::platform::secondary::is_secondary() || cx.has_global::<Offered>() || !cx.has_global::<crate::core_bridge::Core>() {
        return;
    }
    cx.set_global(Offered);
    let status = crate::notices::index_status(cx);
    window
        .spawn(cx, async move |cx| {
            cx.background_executor().timer(OFFER_AFTER).await;
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
            let drives = crate::core_bridge::run(glint_core::fsx::list_drives()).await.ok().and_then(Result::ok).map(|drives| {
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
            let _ = cx.update(|window, cx| {
                let facts = Facts {
                    index,
                    tool_updates,
                    drives,
                    ..Facts::from_settings(cx)
                };
                let day = today();
                if let Some(suggestion) = to_offer(&facts, cx, day) {
                    note_shown(cx, suggestion.kind, day);
                    window.push_notification(notification(suggestion), cx);
                }
            });
        })
        .detach();
}

/// The notification for `suggestion`: its words, its button, and "Don't suggest this again".
fn notification(suggestion: Suggestion) -> Notification {
    let kind = suggestion.kind;
    let id = kind.id();
    let shown = suggestion.clone();
    Notification::info(suggestion.body.clone())
        .title(suggestion.title.clone())
        .id1::<SuggestionToast>(id)
        .autohide(false)
        .action(move |_, _, _| {
            let shown = shown.clone();
            Button::new(SharedString::from(format!("suggestion-{id}")))
                .label(shown.button.clone())
                .primary()
                .on_click(move |_, window, cx| {
                    window.remove_notification1::<SuggestionToast>(id, cx);
                    act(&shown, window, cx);
                })
        })
        // Under the words (the message): a quiet way to say no for good. Closing it is "not now".
        .content(move |_, _, cx| {
            let t = crate::theme::tokens(cx);
            row_start()
                .pt(space(1.))
                .child(
                    Button::new(SharedString::from(format!("suggestion-{id}-never")))
                        .ghost()
                        .xsmall()
                        .label(t!("Don't suggest this again"))
                        .text_color(t.muted_foreground)
                        .on_click(move |_, window, cx| {
                            dismiss(cx, kind);
                            window.remove_notification1::<SuggestionToast>(id, cx);
                        }),
                )
                .into_any_element()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(facts: &Facts) -> Vec<Kind> {
        all(facts).into_iter().map(|s| s.kind).collect()
    }

    #[test]
    fn instant_search_off_comes_first() {
        let facts = Facts {
            index: Some(IndexState::Off),
            tool_updates: Some(2),
            ..Facts::default()
        };
        assert_eq!(kinds(&facts), [Kind::IndexAllDrives, Kind::ToolUpdates]);
        let on = Facts {
            index: Some(IndexState::On),
            ..facts.clone()
        };
        assert_eq!(kinds(&on), [Kind::ToolUpdates]);
        // A PC whose drives can't be indexed isn't asked.
        let unsupported = Facts {
            index: Some(IndexState::Unsupported),
            ..Facts::default()
        };
        assert!(kinds(&unsupported).is_empty());
        assert_eq!(kinds(&Facts::default()), Vec::<Kind>::new(), "nothing known, nothing said");
    }

    #[test]
    fn a_drive_is_nearly_full_by_share_and_by_size() {
        const GB: u64 = 1 << 30;
        let room = |root: &str, free: u64, total: u64| Room {
            root: root.into(),
            free,
            total,
        };
        let facts = |drives| Facts {
            drives: Some(drives),
            ..Facts::default()
        };
        // 8 GB of 256: full. 100 GB of 2 TB is 5% but plenty: not.
        let found = all(&facts(vec![room("C:\\", 8 * GB, 256 * GB), room("D:\\", 100 * GB, 2048 * GB)]));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].drive.as_deref(), Some("C:\\"));
        assert_eq!(found[0].title, "C: is nearly full");
        assert!(all(&facts(vec![room("C:\\", 60 * GB, 256 * GB)])).is_empty());
        // The fullest is the one named.
        let two = all(&facts(vec![room("C:\\", 9 * GB, 256 * GB), room("E:\\", 2 * GB, 128 * GB)]));
        assert_eq!(two[0].drive.as_deref(), Some("E:\\"));
    }

    #[test]
    fn settings_suggest_what_goes_with_them() {
        let shortcut_alone = Facts {
            quick_shortcut: true,
            ..Facts::default()
        };
        assert_eq!(kinds(&shortcut_alone), [Kind::ShortcutNeedsTray]);
        let tray = Facts {
            quick_shortcut: true,
            keep_in_tray: true,
            ..Facts::default()
        };
        assert_eq!(kinds(&tray), [Kind::StartWithWindows]);
        let all_set = Facts {
            start_with_windows: true,
            ..tray
        };
        assert!(kinds(&all_set).is_empty());
        let light_on_dark = Facts {
            theme: Some(Theme::Light),
            windows_dark: true,
            ..Facts::default()
        };
        assert_eq!(kinds(&light_on_dark), [Kind::MatchWindowsTheme]);
        let follows = Facts {
            theme: Some(Theme::System),
            windows_dark: true,
            ..Facts::default()
        };
        assert!(kinds(&follows).is_empty());
        let cloudless = Facts {
            rclone: Some(true),
            clouds: Some(0),
            ..Facts::default()
        };
        assert_eq!(kinds(&cloudless), [Kind::ConnectCloud]);
    }

    #[test]
    fn only_the_important_ones_may_come_up_as_notifications() {
        assert!(Kind::IndexAllDrives.may_notify() && Kind::FullDrive.may_notify() && Kind::ToolUpdates.may_notify());
        assert!(!Kind::StartWithWindows.may_notify() && !Kind::MatchWindowsTheme.may_notify());
        assert!(!Kind::ConnectCloud.may_notify() && !Kind::ShortcutNeedsTray.may_notify());
    }
}
