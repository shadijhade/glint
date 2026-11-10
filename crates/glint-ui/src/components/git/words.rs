//! What Git's parts say and how they look: the marks' badges, the chip's and the status bar's wording, the colors
//! of a change, a repository card's dates and languages, and which file is a folder's README.
//! Ported from `GIT_BADGE` and `gitOf` (FileList.tsx), `GitChip.tsx`, `GitWork` (StatusBar.tsx), `readmeOf`
//! (ReadmeCard.tsx) and the helpers at the top of `RepoCard.tsx`.
//!
//! Everything is a plain function of its inputs, so it is tested without a window.

use super::model::{Counts, View};
use crate::kit::prelude::*;
use crate::t;
use glint_model::entries::{is_list_only, is_locked, is_online_only, FileEntry, GitMark};
use glint_model::format::{format_count, format_duration};

// ---- Colors -------------------------------------------------------------------------------------------------

/// The four colors Git's changes come in: red for conflicts, amber for changed, emerald for staged, sky for new.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Red,
    Amber,
    Emerald,
    Sky,
}

/// Tailwind's shade `level` (200 to 900) of a tone, sRGB.
fn shade(tone: Tone, level: u16) -> Hsla {
    let hex: u32 = match (tone, level) {
        (Tone::Red, 200) => 0xfecaca,
        (Tone::Red, 300) => 0xfca5a5,
        (Tone::Red, 400) => 0xf87171,
        (Tone::Red, 500) => 0xef4444,
        (Tone::Red, 600) => 0xdc2626,
        (Tone::Red, 700) => 0xb91c1c,
        (Tone::Red, _) => 0x7f1d1d,
        (Tone::Amber, 200) => 0xfde68a,
        (Tone::Amber, 300) => 0xfcd34d,
        (Tone::Amber, 400) => 0xfbbf24,
        (Tone::Amber, 500) => 0xf59e0b,
        (Tone::Amber, 600) => 0xd97706,
        (Tone::Amber, 700) => 0xb45309,
        (Tone::Amber, _) => 0x78350f,
        (Tone::Emerald, 200) => 0xa7f3d0,
        (Tone::Emerald, 300) => 0x6ee7b7,
        (Tone::Emerald, 400) => 0x34d399,
        (Tone::Emerald, 500) => 0x10b981,
        (Tone::Emerald, 600) => 0x059669,
        (Tone::Emerald, 700) => 0x047857,
        (Tone::Emerald, _) => 0x064e3b,
        (Tone::Sky, 200) => 0xbae6fd,
        (Tone::Sky, 300) => 0x7dd3fc,
        (Tone::Sky, 400) => 0x38bdf8,
        (Tone::Sky, 500) => 0x0ea5e9,
        (Tone::Sky, 600) => 0x0284c7,
        (Tone::Sky, 700) => 0x0369a1,
        (Tone::Sky, _) => 0x0c4a6e,
    };
    gpui_kit::rgb(hex).into()
}

impl Tone {
    /// The tone itself (`bg-red-500`): the bar's segments, tints are made from it.
    pub fn bar(self) -> Hsla {
        shade(self, 500)
    }

    /// A tinted fill of it (`bg-red-500/15`).
    pub fn tint(self, alpha: f32) -> Hsla {
        shade(self, 500).opacity(alpha)
    }

    /// Text on a tint, in the status pills (`text-red-600 dark:text-red-400`).
    pub fn pill_text(self, dark: bool) -> Hsla {
        shade(self, if dark { 400 } else { 600 })
    }

    /// Text on a tint, in a row's badge (`text-red-700 dark:text-red-300`).
    pub fn badge_text(self, dark: bool) -> Hsla {
        shade(self, if dark { 300 } else { 700 })
    }

    /// Text of a changed line (`text-red-900 dark:text-red-200`).
    pub fn line_text(self, dark: bool) -> Hsla {
        shade(self, if dark { 200 } else { 900 })
    }

    /// The count of added or removed lines in the Changes header (`text-emerald-700 dark:text-emerald-400`).
    pub fn count_text(self, dark: bool) -> Hsla {
        shade(self, if dark { 400 } else { 700 })
    }
}

// ---- The marks in rows --------------------------------------------------------------------------------------

/// What a row draws for a mark (`GIT_BADGE`): a small word after the name, in the mark's tone.
#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    /// The word on the pill: "Conflict", "Changed", "Staged" or "New".
    pub label: SharedString,
    /// Its hover text. A folder's says it of what's in it.
    pub title: SharedString,
    pub tone: Tone,
}

/// The badge a mark gets on an item, or none: an ignored item draws no badge (it shows faded instead). A folder's
/// tooltip says it of what's in the folder.
pub fn badge(mark: GitMark, dir: bool) -> Option<Badge> {
    let (label, title, tone) = match (mark, dir) {
        (GitMark::Ignored, _) => return None,
        (GitMark::Conflicted, false) => (t!("Conflict"), t!("Git: a merge left conflicts in it"), Tone::Red),
        (GitMark::Conflicted, true) => (t!("Conflict"), t!("Git: a merge left conflicts in here"), Tone::Red),
        (GitMark::Modified, false) => (t!("Changed"), t!("Git: changed since it was committed"), Tone::Amber),
        (GitMark::Modified, true) => (
            t!("Changed"),
            t!("Git: something in here changed since it was committed"),
            Tone::Amber,
        ),
        (GitMark::Staged, false) => (
            t!("Staged"),
            t!("Git: changed and staged for the next commit"),
            Tone::Emerald,
        ),
        (GitMark::Staged, true) => (
            t!("Staged"),
            t!("Git: changes in here are staged for the next commit"),
            Tone::Emerald,
        ),
        (GitMark::Untracked, false) => (t!("New"), t!("Git: new, not added yet"), Tone::Sky),
        (GitMark::Untracked, true) => (t!("New"), t!("Git: new files in here, not added yet"), Tone::Sky),
    };
    Some(Badge { label, title, tone })
}

/// What Git says of one row: its badge, and whether `.gitignore` leaves it out (the row shows faded and its name
/// tooltip says so) (`gitOf`).
#[derive(Clone, Debug, PartialEq)]
pub struct RowGit {
    pub badge: Option<Badge>,
    pub ignored: bool,
}

/// `gitOf`: what the folder's marks (by lowercase name) say of `entry`.
pub fn row_git(entry: &FileEntry, marks: Option<&glint_model::groups::GitMarks>) -> RowGit {
    let mark = marks.and_then(|marks| marks.get(&entry.lower)).copied();
    RowGit {
        badge: mark.and_then(|mark| badge(mark, entry.dir)),
        ignored: mark == Some(GitMark::Ignored),
    }
}

/// A row's name tooltip: the name, and for an ignored item a second line saying why it is faded.
pub fn name_tooltip(name: &str, ignored: bool) -> SharedString {
    if ignored {
        format!("{name}\n{}", t!("Git: left out by .gitignore")).into()
    } else {
        name.to_string().into()
    }
}

// ---- The branch chip ----------------------------------------------------------------------------------------

/// A count as the chip shows it (grouped in the user's locale).
pub fn count(n: u32) -> String {
    format_count(usize::try_from(n).unwrap_or(usize::MAX))
}

/// The chip's name: the branch, else the commit HEAD is detached at, else "no commits yet".
pub fn name_of(view: &View) -> SharedString {
    match (&view.branch, &view.head) {
        (Some(branch), _) => branch.clone().into(),
        (None, Some(head)) => t!("at {head}", head = head),
        (None, None) => t!("no commits yet"),
    }
}

/// The chip's tooltip: "Git: on main, tracking origin/main".
pub fn chip_title(view: &View) -> SharedString {
    let what = match (&view.branch, &view.head) {
        (Some(branch), _) => t!("on {branch}", branch = branch),
        (None, Some(head)) => t!("HEAD detached at {head}", head = head),
        (None, None) => t!("a repository with no commits yet"),
    };
    let tracking = match &view.upstream {
        Some(upstream) => t!(", tracking {upstream}", upstream = upstream),
        None => SharedString::default(),
    };
    t!("Git: {what}{tracking}", what = what, tracking = tracking)
}

/// "3 commits not on origin/main yet" (the arrow after the name).
pub fn ahead_title(view: &View) -> SharedString {
    t!(
        "{n} commits not on {upstream} yet",
        n = count(view.ahead),
        upstream = view.upstream.clone().unwrap_or_default()
    )
}

/// "1 commits on origin/main not here yet" (the other arrow).
pub fn behind_title(view: &View) -> SharedString {
    t!(
        "{n} commits on {upstream} not here yet",
        n = count(view.behind),
        upstream = view.upstream.clone().unwrap_or_default()
    )
}

/// The phrases of what changed, in the TypeScript's order and wording: "2 with conflicts", "3 changed", "1 staged",
/// "4 new". Nothing for a kind with none.
pub fn changes(counts: &Counts) -> Vec<SharedString> {
    let mut out = Vec::new();
    if counts.conflicted > 0 {
        out.push(t!("{n} with conflicts", n = count(counts.conflicted)));
    }
    if counts.modified > 0 {
        out.push(t!("{n} changed", n = count(counts.modified)));
    }
    if counts.staged > 0 {
        out.push(t!("{n} staged", n = count(counts.staged)));
    }
    if counts.untracked > 0 {
        out.push(t!("{n} new", n = count(counts.untracked)));
    }
    out
}

/// One kind of change in the popover's bar and pills (`statusParts`).
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub key: &'static str,
    pub label: SharedString,
    pub count: u32,
    /// The filter it sets in the search box.
    pub query: &'static str,
    pub tone: Tone,
}

/// The kinds of change there are any of, in the order conflicts, modified, staged, untracked.
pub fn parts(counts: &Counts) -> Vec<Part> {
    [
        Part {
            key: "conflicted",
            label: t!("Conflicts"),
            count: counts.conflicted,
            query: "git:conflicted",
            tone: Tone::Red,
        },
        Part {
            key: "modified",
            label: t!("Modified"),
            count: counts.modified,
            query: "git:modified",
            tone: Tone::Amber,
        },
        Part {
            key: "staged",
            label: t!("Staged"),
            count: counts.staged,
            query: "git:staged",
            tone: Tone::Emerald,
        },
        Part {
            key: "untracked",
            label: t!("Untracked"),
            count: counts.untracked,
            query: "git:untracked",
            tone: Tone::Sky,
        },
    ]
    .into_iter()
    .filter(|part| part.count > 0)
    .collect()
}

/// The width of a part's segment in the bar, as a share of the bar: its share of the changes, at least 5%.
pub fn segment_share(part: &Part, total: u32) -> f32 {
    if total == 0 {
        return 0.;
    }
    (part.count as f32 / total as f32).max(0.05)
}

/// ", stopped at 4 s: what's shown is what it found by then", when status ran out of time.
pub fn partial_note(view: &View) -> SharedString {
    if view.partial {
        t!(", stopped at 4 s: what's shown is what it found by then")
    } else {
        SharedString::default()
    }
}

/// "Tracks origin/main, up to date with it (as last fetched)" and the range of "origin/main" in it (a byte range
/// of the text, to draw it in bold), or "Tracks no upstream branch" (no range).
pub fn tracks_line(view: &View) -> (SharedString, Option<std::ops::Range<usize>>) {
    let Some(upstream) = &view.upstream else {
        return (t!("Tracks no upstream branch"), None);
    };
    let rest = if view.ahead > 0 || view.behind > 0 {
        let mut pieces = Vec::new();
        if view.ahead > 0 {
            pieces.push(t!("{n} ahead", n = count(view.ahead)).to_string());
        }
        if view.behind > 0 {
            pieces.push(t!("{n} behind", n = count(view.behind)).to_string());
        }
        format!(": {}", pieces.join(", "))
    } else {
        t!(", up to date with it (as last fetched)").to_string()
    };
    let lead = t!("Tracks").to_string();
    let start = lead.len() + 1;
    let text = format!("{lead} {upstream}{rest}");
    (text.into(), Some(start..start + upstream.len()))
}

// ---- The status bar -----------------------------------------------------------------------------------------

/// How Git status of the folder went: what the status bar says and what its hover text adds (`GitWork`).
#[derive(Clone, Debug, PartialEq)]
pub struct StatusLine {
    /// "git · 12 ms", "git · waits for trust", "git · F5 for status". The bar draws it after a "· ", from the
    /// `xl` width up.
    pub said: SharedString,
    pub title: SharedString,
}

/// The status bar's Git words for a view.
pub fn status_line(view: &View) -> StatusLine {
    if view.limited {
        return StatusLine {
            said: t!("git · waits for trust"),
            title: t!(
                "This repository's own settings name programs for Git status to run (filter drivers or core.fsmonitor), so Glint doesn't look at its status until you trust it, from the branch in the address bar."
            ),
        };
    }
    if view.on_request {
        return StatusLine {
            said: t!("git · F5 for status"),
            title: t!(
                "This repository has too many files (or is on another computer) to look through on every visit: F5 shows its status."
            ),
        };
    }
    let took = format_duration(view.elapsed);
    let said = if view.partial {
        t!("git · {took} · partial", took = took)
    } else {
        t!("git · {took}", took = took)
    };
    let list = changes(&view.counts);
    let what = if list.is_empty() {
        t!("nothing changed in here")
    } else {
        t!(
            "{changes} in here and below",
            changes = list.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
        )
    };
    let branch = match &view.branch {
        Some(branch) => t!(", on {branch}", branch = branch),
        None => SharedString::default(),
    };
    let title = t!(
        "Git status of this folder in {took}{partial}: {what}{branch}.",
        took = took,
        partial = partial_note(view),
        what = what,
        branch = branch
    );
    StatusLine { said, title }
}

/// The status bar's "Git status" stage of the popover's "How long it took": label, detail (the branch, else
/// "repo") and milliseconds. None while status waits (trust, F5): there is nothing to time.
pub fn stage(view: &View) -> Option<(SharedString, SharedString, f64)> {
    (!view.limited && !view.on_request).then(|| {
        (
            t!("Git status"),
            view.branch.clone().map_or_else(|| t!("repo"), SharedString::from),
            view.elapsed,
        )
    })
}

// ---- README -------------------------------------------------------------------------------------------------

/// The names a folder's README goes by, in the order one is picked (`NAMES`).
pub const README_NAMES: [&str; 5] = [
    "readme.md",
    "readme.markdown",
    "readme.mdown",
    "readme.mkd",
    "readme.mkdn",
];

/// A folder's README, if it has one that can be read without downloading it (an online-only file would be)
/// (`readmeOf`).
pub fn readme_of(entries: &[FileEntry]) -> Option<&FileEntry> {
    let mut best: Option<&FileEntry> = None;
    let mut rank = README_NAMES.len();
    for entry in entries {
        let at = if entry.dir {
            None
        } else {
            README_NAMES.iter().position(|name| *name == entry.lower)
        };
        if let Some(at) = at {
            if at < rank && !is_online_only(entry) && !is_locked(entry) && !is_list_only(entry) {
                best = Some(entry);
                rank = at;
            }
        }
    }
    best
}

// ---- The repository card ------------------------------------------------------------------------------------

/// GitHub's colors for the languages most repositories have (`COLORS`, by lowercase name); others take one of the
/// rest.
fn language_hex(language: &str) -> Option<u32> {
    Some(match language.to_lowercase().as_str() {
        "rust" => 0xdea584,
        "typescript" => 0x3178c6,
        "tsx" => 0x6aa5e8,
        "javascript" | "jsx" => 0xf1e05a,
        "python" => 0x3572a5,
        "go" => 0x00add8,
        "java" => 0xb07219,
        "kotlin" => 0xa97bff,
        "c" => 0x555555,
        "c++" | "cpp" => 0xf34b7d,
        "c#" | "csharp" => 0x178600,
        "html" => 0xe34c26,
        "css" => 0x663399,
        "scss" => 0xc6538c,
        "shell" => 0x89e051,
        "powershell" => 0x012456,
        "ruby" => 0x701516,
        "php" => 0x4f5d95,
        "swift" => 0xf05138,
        "dart" => 0x00b4ab,
        "lua" => 0x000080,
        "vue" => 0x41b883,
        "svelte" => 0xff3e00,
        "zig" => 0xec915c,
        "markdown" => 0x083fa1,
        _ => return None,
    })
}

/// The colors a language not in the table takes, by its place in the list (`OTHERS`).
const OTHERS: [u32; 6] = [0x8b5cf6, 0x14b8a6, 0xf59e0b, 0xec4899, 0x64748b, 0x22c55e];

/// A language's color in the card's bar and legend (`colorOf`): GitHub's, else one of six by its place in the list.
pub fn language_color(language: &str, index: usize) -> Hsla {
    gpui_kit::rgb(language_hex(language).unwrap_or(OTHERS[index % OTHERS.len()])).into()
}

/// A language's share as the legend writes it: whole percent, "<1" under one (`share >= 1 ? Math.round(share) : "<1"`).
pub fn share_text(share: f64) -> String {
    if share >= 1. {
        // JS rounds .5 up.
        format!("{}%", (share + 0.5).floor() as u64)
    } else {
        "<1%".to_string()
    }
}

/// The repository's web address, from its remote URL: `git@host:owner/repo.git` becomes `https://host/owner/repo`.
/// Only an `https://` address comes back (the card opens it in the browser).
pub fn site_of(url: &str) -> Option<String> {
    let mut site = url.to_string();
    if let Some(rest) = url.strip_prefix("git@") {
        // `^git@([^:]+):` -> `https://$1/`
        if let Some((host, path)) = rest.split_once(':') {
            if !host.is_empty() {
                site = format!("https://{host}/{path}");
            }
        }
    }
    if let Some(trimmed) = site.strip_suffix(".git") {
        site = trimmed.to_string();
    }
    site.starts_with("https://").then_some(site)
}

/// Milliseconds since 1970 of an RFC 3339 date like `2026-11-09T12:34:56+02:00` (`Date.parse`), or `None`.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, time) = text.split_once(['T', 't', ' ']).unwrap_or((text, "00:00:00"));
    let mut d = date.split('-');
    let (year, month, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // The zone: Z, or +hh:mm / -hh:mm after the clock.
    let (clock, offset_minutes) = match time.find(['Z', 'z', '+', '-']) {
        Some(at) => {
            let (clock, zone) = (time.get(..at)?, time.get(at..)?);
            let minutes = match zone.chars().next()? {
                'Z' | 'z' => 0,
                sign => {
                    let digits: Vec<&str> = zone.get(1..)?.split(':').collect();
                    let hours: i64 = digits.first()?.parse().ok()?;
                    let minutes: i64 = digits.get(1).unwrap_or(&"0").parse().ok()?;
                    let total = hours * 60 + minutes;
                    if sign == '-' {
                        -total
                    } else {
                        total
                    }
                }
            };
            (clock, minutes)
        }
        None => (time, 0),
    };
    let (clock, fraction) = clock.split_once('.').unwrap_or((clock, "0"));
    let mut c = clock.split(':');
    let (h, m, s): (i64, i64, i64) = (
        c.next()?.parse().ok()?,
        c.next().unwrap_or("0").parse().ok()?,
        c.next().unwrap_or("0").parse().ok()?,
    );
    let millis: i64 = format!("{fraction:0<3}").get(..3)?.parse().ok()?;
    // Howard Hinnant's days-from-civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((((days * 24 + h) * 60 + m - offset_minutes) * 60 + s) * 1000 + millis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::entries::{attr, with_details, RawEntry};

    fn entry(name: &str, dir: bool, attrs: u32) -> FileEntry {
        with_details(
            RawEntry {
                name: name.into(),
                dir,
                attrs,
                ..Default::default()
            },
            "C:\\repo",
        )
    }

    #[test]
    fn a_mark_gets_the_badge_the_typescript_gave_it() {
        let _guard = crate::testing::lock();
        crate::testing::set_rtl(false);
        let b = badge(GitMark::Modified, false).unwrap();
        assert_eq!(b.label, "Changed");
        assert_eq!(b.title, "Git: changed since it was committed");
        assert_eq!(b.tone, Tone::Amber);
        // A folder says it of what is in it.
        assert_eq!(
            badge(GitMark::Modified, true).unwrap().title,
            "Git: something in here changed since it was committed"
        );
        assert_eq!(badge(GitMark::Conflicted, false).unwrap().label, "Conflict");
        assert_eq!(badge(GitMark::Staged, false).unwrap().label, "Staged");
        assert_eq!(
            badge(GitMark::Untracked, true).unwrap().title,
            "Git: new files in here, not added yet"
        );
        // Ignored items draw no badge; they show faded.
        assert!(badge(GitMark::Ignored, false).is_none());
    }

    #[test]
    fn a_row_is_faded_when_gitignore_leaves_it_out() {
        let _guard = crate::testing::lock();
        let mut marks = glint_model::groups::GitMarks::new();
        marks.insert("target".into(), GitMark::Ignored);
        marks.insert("a.rs".into(), GitMark::Modified);
        let ignored = row_git(&entry("target", true, 0), Some(&marks));
        assert!(ignored.ignored && ignored.badge.is_none());
        let changed = row_git(&entry("A.rs", false, 0), Some(&marks));
        assert!(!changed.ignored && changed.badge.is_some());
        let none = row_git(&entry("a.rs", false, 0), None);
        assert!(!none.ignored && none.badge.is_none());
        assert_eq!(name_tooltip("target", true), "target\nGit: left out by .gitignore");
        assert_eq!(name_tooltip("a.rs", false), "a.rs");
    }

    fn view() -> View {
        View {
            root: "C:\\repo".into(),
            branch: Some("main".into()),
            head: Some("abc1234".into()),
            upstream: Some("origin/main".into()),
            ahead: 2,
            behind: 1,
            counts: Counts {
                modified: 3,
                staged: 1,
                untracked: 4,
                conflicted: 0,
            },
            elapsed: 12.4,
            ..Default::default()
        }
    }

    #[test]
    fn the_chip_names_the_branch_the_commit_or_nothing() {
        let _guard = crate::testing::lock();
        crate::testing::set_rtl(false);
        assert_eq!(name_of(&view()), "main");
        let detached = View { branch: None, ..view() };
        assert_eq!(name_of(&detached), "at abc1234");
        assert_eq!(
            chip_title(&detached),
            "Git: HEAD detached at abc1234, tracking origin/main"
        );
        let fresh = View {
            branch: None,
            head: None,
            upstream: None,
            ..view()
        };
        assert_eq!(name_of(&fresh), "no commits yet");
        assert_eq!(chip_title(&fresh), "Git: a repository with no commits yet");
        assert_eq!(chip_title(&view()), "Git: on main, tracking origin/main");
        assert_eq!(ahead_title(&view()), "2 commits not on origin/main yet");
        assert_eq!(behind_title(&view()), "1 commits on origin/main not here yet");
    }

    #[test]
    fn changes_read_in_the_order_conflicts_changed_staged_new() {
        let _guard = crate::testing::lock();
        let all = Counts {
            modified: 3,
            staged: 1,
            untracked: 4,
            conflicted: 2,
        };
        let text: Vec<String> = changes(&all).iter().map(|c| c.to_string()).collect();
        assert_eq!(text, ["2 with conflicts", "3 changed", "1 staged", "4 new"]);
        assert!(changes(&Counts::default()).is_empty());
        let keys: Vec<_> = parts(&all).iter().map(|p| p.key).collect();
        assert_eq!(keys, ["conflicted", "modified", "staged", "untracked"]);
        let some: Vec<_> = parts(&view().counts).iter().map(|p| p.query).collect();
        assert_eq!(some, ["git:modified", "git:staged", "git:untracked"]);
        // A small share still shows: at least 5% of the bar.
        let p = parts(&Counts {
            modified: 1,
            staged: 99,
            ..Default::default()
        });
        assert_eq!(segment_share(&p[0], 100), 0.05);
        assert_eq!(segment_share(&p[1], 100), 0.99);
        assert_eq!(segment_share(&p[0], 0), 0.);
    }

    #[test]
    fn tracks_says_how_far_from_the_upstream() {
        let _guard = crate::testing::lock();
        let (text, bold) = tracks_line(&view());
        assert_eq!(text, "Tracks origin/main: 2 ahead, 1 behind");
        assert_eq!(&text[bold.unwrap()], "origin/main");
        let even = View {
            ahead: 0,
            behind: 0,
            ..view()
        };
        assert_eq!(
            tracks_line(&even).0,
            "Tracks origin/main, up to date with it (as last fetched)"
        );
        let none = View {
            upstream: None,
            ..view()
        };
        assert_eq!(tracks_line(&none), ("Tracks no upstream branch".into(), None));
    }

    #[test]
    fn the_status_bar_words_follow_git_work() {
        let _guard = crate::testing::lock();
        let line = status_line(&view());
        assert_eq!(line.said, "git · 12 ms");
        assert_eq!(
            line.title,
            "Git status of this folder in 12 ms: 3 changed, 1 staged, 4 new in here and below, on main."
        );
        let partial = View {
            partial: true,
            ..view()
        };
        assert_eq!(status_line(&partial).said, "git · 12 ms · partial");
        assert!(status_line(&partial)
            .title
            .contains("stopped at 4 s: what's shown is what it found by then"));
        let clean = View {
            counts: Counts::default(),
            branch: None,
            ..view()
        };
        assert_eq!(
            status_line(&clean).title,
            "Git status of this folder in 12 ms: nothing changed in here."
        );
        let limited = View {
            limited: true,
            ..view()
        };
        assert_eq!(status_line(&limited).said, "git · waits for trust");
        let big = View {
            on_request: true,
            ..view()
        };
        assert_eq!(status_line(&big).said, "git · F5 for status");
        assert!(stage(&limited).is_none() && stage(&big).is_none());
        let (label, detail, ms) = stage(&view()).unwrap();
        assert_eq!((label.as_ref(), detail.as_ref(), ms), ("Git status", "main", 12.4));
        let detached = View { branch: None, ..view() };
        assert_eq!(stage(&detached).unwrap().1, "repo");
    }

    #[test]
    fn the_readme_is_the_first_name_in_the_list_that_can_be_read() {
        let entries = vec![
            entry("README.markdown", false, 0),
            entry("readme.md", true, 0),
            entry("notes.md", false, 0),
            entry("Readme.MD", false, 0),
        ];
        // A folder named like one does not count; .md comes before .markdown.
        assert_eq!(readme_of(&entries).map(|e| e.name.as_str()), Some("Readme.MD"));
        assert!(readme_of(&entries[1..3]).is_none());
        // An online-only one would be downloaded to be read: the next is picked.
        let cloud = vec![entry("README.md", false, attr::OFFLINE), entry("readme.mkd", false, 0)];
        assert_eq!(readme_of(&cloud).map(|e| e.name.as_str()), Some("readme.mkd"));
        assert!(readme_of(&[]).is_none());
    }

    #[test]
    fn languages_and_shares_read_as_the_card_did() {
        assert_eq!(share_text(0.4), "<1%");
        assert_eq!(share_text(1.0), "1%");
        assert_eq!(share_text(62.5), "63%");
        assert_eq!(share_text(62.4), "62%");
        assert_eq!(language_color("Rust", 0), gpui_kit::rgb(0xdea584).into());
        assert_eq!(language_color("Zork", 7), gpui_kit::rgb(OTHERS[1]).into());
    }

    #[test]
    fn a_remote_becomes_a_web_address_only_when_it_is_https() {
        assert_eq!(
            site_of("git@github.com:rafd/glint.git").as_deref(),
            Some("https://github.com/rafd/glint")
        );
        assert_eq!(
            site_of("https://github.com/rafd/glint.git").as_deref(),
            Some("https://github.com/rafd/glint")
        );
        assert_eq!(site_of("http://example.com/x.git"), None);
        assert_eq!(site_of("C:\\repos\\glint"), None);
        assert_eq!(site_of("ssh://host/x"), None);
    }

    #[test]
    fn dates_are_read_with_their_zone() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2000-03-01T00:00:00Z"), Some(951_868_800_000));
        // +02:00 is two hours earlier in UTC.
        assert_eq!(parse_rfc3339("2000-03-01T02:00:00+02:00"), Some(951_868_800_000));
        assert_eq!(parse_rfc3339("2000-02-29T22:30:00-01:30"), Some(951_868_800_000));
        assert_eq!(parse_rfc3339("2000-03-01T00:00:00.5Z"), Some(951_868_800_500));
        assert_eq!(parse_rfc3339("nonsense"), None);
        assert_eq!(parse_rfc3339("2000-13-01T00:00:00Z"), None);
    }
}
