//! This PC's arithmetic and names, ported from src/components/explorer/ThisPC.tsx: the percentages, the
//! totals over the drives that have a size, the drive's title and the keyboard's steps between tiles.
//!
//! Nothing here draws or touches GPUI, so the rules are easy to test. The view turns the results into
//! elements, and translates the words (`t!`).

use crate::components::status_bar::index::{Index, State};
use crate::explorer::{Drive, DriveKind};
use glint_model::paths::{droppable, in_archive_path, is_remote_path};

/// The folders' tiles are `minmax(10.5rem, 1fr)` with `gap-2` between them.
pub const FOLDER_TILE_REM: f32 = 10.5;
/// See [`FOLDER_TILE_REM`].
pub const FOLDER_GAP_REM: f32 = 0.5;
/// The drives' tiles are `minmax(17.5rem, 1fr)` with `gap-3` between them.
pub const DRIVE_TILE_REM: f32 = 17.5;
/// See [`DRIVE_TILE_REM`].
pub const DRIVE_GAP_REM: f32 = 0.75;

/// `Math.round` for a value that is not negative: halves go up.
fn round_half_up(value: f64) -> u32 {
    // The clamp keeps the cast from a float in range, so it can't wrap.
    (value + 0.5).floor().clamp(0., f64::from(u32::MAX)) as u32
}

/// How much of a drive is used, as a whole percentage (`Math.round(((total - free) / total) * 100)`). A drive
/// with no size reads 0. Free space above the size (which Windows never reports) counts as nothing used.
pub fn used_percent(total: u64, free: u64) -> u32 {
    if total == 0 {
        return 0;
    }
    let used = total.saturating_sub(free);
    round_half_up(used as f64 / total as f64 * 100.)
}

/// A drive nearly full: its tile and its percentage turn red (`usedPct >= 90`).
pub fn nearly_full(percent: u32) -> bool {
    percent >= 90
}

/// The summary over the drives that have a size (`readyDrives` in ThisPC.tsx).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Totals {
    /// All the capacity of the drives that have a size, in bytes.
    pub capacity: u64,
    /// Their free space, in bytes.
    pub free: u64,
    /// Their used space: the capacity less the free space, never negative.
    pub used: u64,
    /// The used share, as a whole percentage.
    pub percent: u32,
}

/// The summary for the drives, or None when none of them has a size (the TypeScript shows nothing then).
pub fn totals(drives: &[Drive]) -> Option<Totals> {
    let ready = drives.iter().filter(|d| d.total > 0);
    let capacity = ready.clone().fold(0u64, |sum, d| sum.saturating_add(d.total));
    let free = ready.fold(0u64, |sum, d| sum.saturating_add(d.free));
    if capacity == 0 {
        return None;
    }
    let used = capacity.saturating_sub(free);
    Some(Totals {
        capacity,
        free,
        used,
        percent: used_percent(capacity, free),
    })
}

/// A drive's path without its trailing separator: "C:" for "C:\" (`driveName`).
pub fn letters(path: &str) -> &str {
    path.strip_suffix('\\').unwrap_or(path)
}

/// Whether a drive's tile offers the quick actions: a drive with a size, or a removable or disc drive (which
/// can be ejected even when it's empty).
pub fn has_actions(kind: DriveKind, total: u64) -> bool {
    total > 0 || is_ejectable(kind)
}

/// Removable drives and discs get the eject button.
pub fn is_ejectable(kind: DriveKind) -> bool {
    matches!(kind, DriveKind::Removable | DriveKind::Cdrom)
}

/// Whether a drop of `paths` on the tile at `dest` is taken (`targetAt` in Explorer.tsx). Not on a CD or DVD drive
/// (`disc`: its disc is read-only), not into a place that is read-only (a remote connection, an archive), and not
/// where every file already is: the TypeScript refused those, so the tile doesn't light for them.
pub fn drop_accepted(paths: &[String], dest: &str, disc: bool) -> bool {
    !disc && !is_remote_path(dest) && !in_archive_path(dest) && !droppable(paths, dest).is_empty()
}

/// Where the focus goes from a tile. The view maps the arrow keys to these, reversing left and right in
/// Arabic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The next tile in reading order.
    Next,
    /// The previous tile in reading order.
    Previous,
    /// The tile above, in the same column of its grid.
    Up,
    /// The tile below. From the last row of folders it lands in the drives, in the same column when there is one.
    Down,
    /// The first tile.
    First,
    /// The last tile.
    Last,
}

/// How many columns a grid of tiles `min` wide (with `gap` between them) has in `width`: what
/// `repeat(auto-fill, minmax(min, 1fr))` gives. Never less than one.
pub fn grid_columns(width: f32, min: f32, gap: f32) -> usize {
    if width.is_nan() || width <= 0. || min + gap <= 0. {
        return 1;
    }
    let columns = ((width + gap) / (min + gap)).floor();
    // The clamp keeps the cast in range; a window can't have more columns than this.
    columns.clamp(1., 10_000.) as usize
}

/// The tile a step from `index` lands on. The folders are tiles `0..known` and the drives follow them; each
/// grid has its own columns. None when the step leaves the tiles (or there are none).
pub fn step(
    index: usize,
    step: Step,
    known: usize,
    drives: usize,
    columns_known: usize,
    columns_drives: usize,
) -> Option<usize> {
    let total = known + drives;
    if index >= total {
        return None;
    }
    match step {
        Step::First => Some(0),
        Step::Last => Some(total - 1),
        Step::Next => (index + 1 < total).then_some(index + 1),
        Step::Previous => index.checked_sub(1),
        Step::Up | Step::Down => {
            let in_folders = index < known;
            let (start, len, columns) = if in_folders {
                (0, known, columns_known.max(1))
            } else {
                (known, drives, columns_drives.max(1))
            };
            let position = index - start;
            let column = position % columns;
            if step == Step::Down {
                if position + columns < len {
                    Some(start + position + columns)
                } else if in_folders && drives > 0 {
                    Some(known + column.min(drives - 1))
                } else {
                    None
                }
            } else if position >= columns {
                Some(start + position - columns)
            } else if !in_folders && known > 0 {
                // Up from the first row of drives: the folder in the same column of the last row of folders.
                let columns = columns_known.max(1);
                let last_row = (known - 1) / columns * columns;
                Some((last_row + column).min(known - 1))
            } else {
                None
            }
        }
    }
}

/// What the instant search card says, before it is translated (`InstantSearchCard`'s text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Words {
    /// The service isn't running: the offer to turn it on.
    Offer,
    /// It runs, but lists no drive yet: "Instant search is starting…".
    Starting,
    /// The drives being read (with how far, in whole percent, when known) then the drives that failed.
    Drives {
        reading: Vec<(String, Option<u64>)>,
        failed: Vec<String>,
    },
}

/// The card shown above the drives while instant search is off, starting, reading a drive or has a drive that
/// failed (`InstantSearchCard`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub words: Words,
    /// A spinner instead of the bolt: the service runs and is starting or reading a drive.
    pub busy: bool,
    /// The button: "Turn on instant search" (the service is off) or "Retry" (a drive failed, none is being read).
    pub button: Option<Button>,
}

/// The card's button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    TurnOn,
    Retry,
}

/// The card for what the service said, or None when there is nothing to offer or say: the service wasn't asked
/// yet, Settings explains the states a click can't fix (another account's service, an impostor, no NTFS), or all
/// is well.
pub fn card(index: Option<&Index>) -> Option<Card> {
    let index = index?;
    if matches!(index.state, State::Unsupported | State::NoAccess | State::Untrusted) {
        return None;
    }
    let reading: Vec<(String, Option<u64>)> = index
        .volumes
        .iter()
        .filter(|v| v.scanning)
        .map(|v| (v.volume.clone(), v.percent()))
        .collect();
    let failed: Vec<String> = index
        .volumes
        .iter()
        .filter(|v| v.error)
        .map(|v| v.volume.clone())
        .collect();
    let on = index.state == State::On;
    if on && reading.is_empty() && failed.is_empty() && !index.volumes.is_empty() {
        return None;
    }
    let words = if !on {
        Words::Offer
    } else if index.volumes.is_empty() {
        Words::Starting
    } else {
        Words::Drives {
            reading: reading.clone(),
            failed: failed.clone(),
        }
    };
    let button = if !on {
        Some(Button::TurnOn)
    } else if !failed.is_empty() && reading.is_empty() {
        Some(Button::Retry)
    } else {
        None
    };
    Some(Card {
        words,
        busy: on && (!reading.is_empty() || index.volumes.is_empty()),
        button,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(path: &str, total: u64, free: u64) -> Drive {
        Drive {
            path: path.into(),
            label: String::new(),
            kind: DriveKind::Fixed,
            fs: "NTFS".into(),
            total,
            free,
        }
    }

    #[test]
    fn used_percent_rounds_halves_up_like_math_round() {
        assert_eq!(used_percent(0, 0), 0);
        assert_eq!(used_percent(100, 100), 0);
        assert_eq!(used_percent(100, 0), 100);
        // 1 of 8 is 12.5%: Math.round gives 13.
        assert_eq!(used_percent(8, 7), 13);
        // 1 of 3 is 33.33%: 33.
        assert_eq!(used_percent(3, 2), 33);
        // Free space above the size counts as nothing used, not a negative share.
        assert_eq!(used_percent(100, 150), 0);
    }

    #[test]
    fn nearly_full_starts_at_ninety_percent() {
        assert!(!nearly_full(89));
        assert!(nearly_full(90));
    }

    #[test]
    fn totals_count_only_drives_with_a_size() {
        let drives = [drive("C:\\", 100, 40), drive("D:\\", 300, 60), drive("E:\\", 0, 0)];
        assert_eq!(
            totals(&drives),
            Some(Totals {
                capacity: 400,
                free: 100,
                used: 300,
                percent: 75,
            })
        );
        assert_eq!(totals(&[drive("E:\\", 0, 0)]), None);
        assert_eq!(totals(&[]), None);
    }

    #[test]
    fn letters_drop_the_separator_and_actions_follow_size_or_removability() {
        assert_eq!(letters("C:\\"), "C:");
        assert_eq!(letters("\\\\wsl.localhost\\Ubuntu"), "\\\\wsl.localhost\\Ubuntu");
        assert!(has_actions(DriveKind::Fixed, 1));
        assert!(!has_actions(DriveKind::Fixed, 0));
        assert!(has_actions(DriveKind::Removable, 0));
        assert!(has_actions(DriveKind::Cdrom, 0));
        assert!(!has_actions(DriveKind::Network, 0));
    }

    #[test]
    fn grid_columns_match_auto_fill() {
        // 952 px of content, 168 px tiles and 8 px gaps: (952 + 8) / (168 + 8) = 5.4, so five columns.
        assert_eq!(grid_columns(952., 168., 8.), 5);
        // Narrower than one tile: still one column.
        assert_eq!(grid_columns(100., 168., 8.), 1);
        // Not measured yet: one column, not a division by zero.
        assert_eq!(grid_columns(0., 168., 8.), 1);
        assert_eq!(grid_columns(f32::NAN, 168., 8.), 1);
    }

    #[test]
    fn steps_walk_the_tiles_and_stay_in_bounds() {
        // Two folders, three drives; folders in 2 columns, drives in 2 columns.
        let s = |index, step| super::step(index, step, 2, 3, 2, 2);
        assert_eq!(s(0, Step::Next), Some(1));
        assert_eq!(s(1, Step::Next), Some(2));
        assert_eq!(s(4, Step::Next), None);
        assert_eq!(s(0, Step::Previous), None);
        assert_eq!(s(2, Step::Previous), Some(1));
        assert_eq!(s(0, Step::Last), Some(4));
        assert_eq!(s(4, Step::First), Some(0));
        // Down from the last folder row goes to the drive under the same column, then down the drives.
        assert_eq!(s(1, Step::Down), Some(3));
        assert_eq!(s(0, Step::Down), Some(2));
        assert_eq!(s(2, Step::Down), Some(4));
        assert_eq!(s(4, Step::Down), None);
        // Up from the first drive row goes back to the folder above it, and up from the top there is nowhere.
        assert_eq!(s(2, Step::Up), Some(0));
        assert_eq!(s(4, Step::Up), Some(2));
        assert_eq!(s(0, Step::Up), None);
        // No tiles: nothing moves, nothing panics.
        assert_eq!(super::step(0, Step::Next, 0, 0, 1, 1), None);
        assert_eq!(super::step(0, Step::Down, 0, 0, 0, 0), None);
    }

    #[test]
    fn a_drop_is_refused_on_a_disc_and_where_the_files_already_are() {
        let files = vec!["C:\\Users\\me\\Downloads\\ملف.txt".to_string()];
        assert!(drop_accepted(&files, "C:\\Users\\me\\Documents", false));
        // The files are in Downloads already: dropping them there does nothing, so the tile doesn't light.
        assert!(!drop_accepted(&files, "C:\\Users\\me\\Downloads", false));
        // A drive's root is refused for files at its top (they're there already) and taken for others.
        assert!(!drop_accepted(&["C:\\readme.txt".to_string()], "C:\\", false));
        assert!(drop_accepted(&files, "C:\\", false));
        // A CD or DVD drive is read-only, whatever is dropped.
        assert!(!drop_accepted(&files, "D:\\", true));
        // Nothing dropped, nothing taken.
        assert!(!drop_accepted(&[], "C:\\", false));
    }

    #[test]
    fn steps_with_only_drives_or_only_folders() {
        assert_eq!(super::step(0, Step::Down, 0, 2, 1, 1), Some(1));
        assert_eq!(super::step(0, Step::Down, 3, 0, 1, 1), Some(1));
        assert_eq!(super::step(2, Step::Up, 3, 0, 1, 1), Some(1));
    }

    fn volume(
        name: &str,
        scanning: bool,
        error: bool,
        progress: u64,
        total: u64,
    ) -> crate::components::status_bar::index::Volume {
        crate::components::status_bar::index::Volume {
            volume: name.into(),
            scanning,
            error,
            progress,
            progress_total: total,
        }
    }

    fn index(state: State, volumes: Vec<crate::components::status_bar::index::Volume>) -> Index {
        Index {
            state,
            version: None,
            volumes,
            error: None,
        }
    }

    #[test]
    fn the_card_offers_turning_on_when_the_service_is_off_or_stopped() {
        for state in [State::Off, State::Stopped] {
            let card = card(Some(&index(state, vec![]))).expect("a card");
            assert_eq!(card.words, Words::Offer);
            assert_eq!(card.button, Some(Button::TurnOn));
            assert!(!card.busy);
        }
    }

    #[test]
    fn the_card_is_absent_when_unasked_unfixable_or_all_is_well() {
        assert_eq!(card(None), None);
        for state in [State::Unsupported, State::NoAccess, State::Untrusted] {
            assert_eq!(card(Some(&index(state, vec![]))), None);
        }
        assert_eq!(
            card(Some(&index(State::On, vec![volume("C:", false, false, 0, 0)]))),
            None
        );
    }

    #[test]
    fn the_card_says_starting_until_a_drive_is_listed() {
        let card = card(Some(&index(State::On, vec![]))).expect("a card");
        assert_eq!(card.words, Words::Starting);
        assert!(card.busy);
        assert_eq!(card.button, None);
    }

    #[test]
    fn the_card_reports_progress_and_failures_and_retries_only_when_nothing_is_being_read() {
        let both = card(Some(&index(
            State::On,
            vec![volume("C:", true, false, 1, 4), volume("D:", false, true, 0, 0)],
        )))
        .expect("a card");
        assert_eq!(
            both.words,
            Words::Drives {
                reading: vec![("C:".into(), Some(25))],
                failed: vec!["D:".into()],
            }
        );
        assert!(both.busy);
        assert_eq!(both.button, None, "a drive is still being read");
        let failed = card(Some(&index(State::On, vec![volume("D:", false, true, 0, 0)]))).expect("a card");
        assert!(!failed.busy);
        assert_eq!(failed.button, Some(Button::Retry));
        let unknown = card(Some(&index(State::On, vec![volume("E:", true, false, 5, 0)]))).expect("a card");
        assert_eq!(
            unknown.words,
            Words::Drives {
                reading: vec![("E:".into(), None)],
                failed: vec![],
            }
        );
    }
}
