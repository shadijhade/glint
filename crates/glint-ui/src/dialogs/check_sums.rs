//! "Check files in this folder": every file a checksum file (`.sha256`, `.sha512`, `.b3sum`) lists is checked in
//! the background, and what came of it is said in a toast, with the names of what doesn't match or isn't there.
//!
//! Not a dialog with a window of its own: the answer is a few lines, and the person was in the middle of
//! something else. The words are made by [`words`] from the engine's report, so they are tested without a window.

use crate::core_bridge;
use crate::kit::prelude::*;
use crate::t;
use glint_core::checksums::Report;
use glint_model::format::format_count;
use glint_model::paths::base_name;

/// How many names of a kind are listed before "and 3 more".
const NAMES: usize = 4;

/// The toast that says the check is running, replaced by the one that says how it ended.
struct CheckingToast;

/// What the toast says.
#[derive(Clone, Debug, PartialEq)]
pub struct Words {
    /// Everything listed is there and matches.
    pub good: bool,
    pub title: SharedString,
    pub message: SharedString,
}

/// Checks the files the checksum file at `path` lists, off the UI thread, and tells the window how it went.
pub fn open(window: &mut Window, cx: &mut App, path: String) {
    let name = base_name(&path).to_string();
    window.push_notification(
        Notification::info(t!("Checking the files {name} lists…", name = name.clone()))
            .id::<CheckingToast>()
            .autohide(false),
        cx,
    );
    let checking = core_bridge::run(glint_core::checksums::check_sums(path));
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        let result = checking.await.and_then(|inner| inner);
        let _ = handle.update(cx, |_, window, cx| {
            window.remove_notification::<CheckingToast>(cx);
            let note = match result {
                Ok(report) => {
                    let said = words(&report, &name);
                    let note = if said.good {
                        Notification::success(said.message)
                    } else {
                        Notification::warning(said.message).autohide(false)
                    };
                    note.title(said.title)
                }
                Err(reason) => Notification::error(reason).title(t!("Couldn't check the files")),
            };
            window.push_notification(note, cx);
        });
    })
    .detach();
}

/// The first few names, then how many more.
fn names(all: &[String]) -> String {
    let shown: Vec<&str> = all.iter().take(NAMES).map(String::as_str).collect();
    let rest = all.len().saturating_sub(NAMES);
    if rest == 0 {
        shown.join(", ")
    } else {
        format!("{}, {}", shown.join(", "), t!("and {n} more", n = format_count(rest)))
    }
}

/// What to tell the person about `report`, for the checksum file called `sums`.
pub fn words(report: &Report, sums: &str) -> Words {
    if report.listed == 0 {
        return Words {
            good: false,
            title: t!("Nothing to check"),
            message: t!("{name} lists no files Glint could check", name = sums),
        };
    }
    if report.all_good() {
        let mut message = match report.listed {
            1 => t!("The file matches {name}", name = sums),
            n => t!("All {n} files match {name}", n = format_count(n), name = sums),
        }
        .to_string();
        if report.skipped > 0 {
            message = format!(
                "{message}. {}",
                t!("{n} lines weren't checked", n = format_count(report.skipped))
            );
        }
        return Words {
            good: true,
            title: t!("Checksums match"),
            message: message.into(),
        };
    }
    let mut lines: Vec<String> = Vec::new();
    let bad = &report.mismatched;
    if !bad.is_empty() {
        lines.push(
            match bad.len() {
                1 => t!("1 file doesn't match: {names}", names = names(bad)),
                n => t!(
                    "{n} files don't match: {names}",
                    n = format_count(n),
                    names = names(bad)
                ),
            }
            .to_string(),
        );
    }
    let gone = &report.missing;
    if !gone.is_empty() {
        lines.push(
            match gone.len() {
                1 => t!("1 file is missing: {names}", names = names(gone)),
                n => t!(
                    "{n} files are missing: {names}",
                    n = format_count(n),
                    names = names(gone)
                ),
            }
            .to_string(),
        );
    }
    let stuck = &report.unreadable;
    if !stuck.is_empty() {
        lines.push(
            match stuck.len() {
                1 => t!("1 file couldn't be read: {names}", names = names(stuck)),
                n => t!(
                    "{n} files couldn't be read: {names}",
                    n = format_count(n),
                    names = names(stuck)
                ),
            }
            .to_string(),
        );
    }
    lines.push(
        match report.ok {
            0 => t!("No file matches"),
            1 => t!("1 file matches"),
            n => t!("{n} files match", n = format_count(n)),
        }
        .to_string(),
    );
    Words {
        good: false,
        title: t!("Checksums don't all match"),
        message: lines.join("\n").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::aware::Algo;

    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::testing::lock();
        crate::testing::set_rtl(false);
        guard
    }

    fn report(listed: usize, ok: usize) -> Report {
        Report {
            algo: Some(Algo::Sha256),
            listed,
            ok,
            ..Report::default()
        }
    }

    fn some(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn everything_matching_is_said_once_and_with_the_lines_that_were_not_checked() {
        let _guard = english();
        let said = words(&report(3, 3), "SHA256SUMS.sha256");
        assert!(said.good);
        assert_eq!(
            (said.title.as_ref(), said.message.as_ref()),
            ("Checksums match", "All 3 files match SHA256SUMS.sha256")
        );
        let one = words(&report(1, 1), "a.sha256");
        assert_eq!(one.message.as_ref(), "The file matches a.sha256");
        let skipped = Report {
            skipped: 2,
            ..report(3, 3)
        };
        assert_eq!(
            words(&skipped, "a.sha256").message.as_ref(),
            "All 3 files match a.sha256. 2 lines weren't checked"
        );
    }

    #[test]
    fn what_doesnt_match_is_missing_or_unreadable_is_named_and_what_matched_is_counted() {
        let _guard = english();
        let r = Report {
            mismatched: some(&["b.iso", "c.zip"]),
            missing: some(&["gone.txt"]),
            unreadable: some(&["locked.bin"]),
            ..report(6, 2)
        };
        let said = words(&r, "x.sha256");
        assert!(!said.good);
        assert_eq!(said.title.as_ref(), "Checksums don't all match");
        assert_eq!(
            said.message.as_ref(),
            "2 files don't match: b.iso, c.zip\n1 file is missing: gone.txt\n1 file couldn't be read: locked.bin\n2 files match"
        );
    }

    #[test]
    fn a_long_list_of_names_stops_at_four() {
        let _guard = english();
        let many: Vec<String> = (1..=7).map(|n| format!("f{n}")).collect();
        let r = Report {
            mismatched: many,
            ..report(7, 0)
        };
        let said = words(&r, "x.sha256");
        assert!(
            said.message
                .starts_with("7 files don't match: f1, f2, f3, f4, and 3 more\n"),
            "{}",
            said.message
        );
    }

    #[test]
    fn a_checksum_file_that_lists_nothing_has_nothing_to_check() {
        let _guard = english();
        let said = words(&Report::default(), "empty.sha256");
        assert!(!said.good);
        assert_eq!(said.title.as_ref(), "Nothing to check");
    }
}
