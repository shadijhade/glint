//! The timeline rail's arithmetic and words: where each stop sits along the rail, which stop a pointer is
//! nearest, the change-density sparkline's bars, and what a screen reader says of a stop.
//!
//! All of it is pure (numbers in, numbers out) so it is tested without a window; `mod.rs` draws it.

use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::time_machine::{Moment, MomentSource};

/// The closest two neighbouring stops are drawn, as a fraction of the rail's width, when time alone would put
/// them on top of each other (twenty commits in one afternoon). The rail knows its real width only after it
/// has been drawn, so the first frame uses this and later ones use ten pixels' worth.
pub const MIN_GAP: f32 = 0.012;

/// How many bars the sparkline has.
pub const BARS: usize = 48;

/// Where each stop sits along the rail, from 0 (the start edge) to 1, for `times` (Unix milliseconds, oldest
/// first, as the model keeps them). Stops sit where their time puts them, between the oldest and the newest,
/// pushed apart just enough that no two are closer than `min_gap`. A lone stop, or stops all at one time, are
/// spread evenly, and so are stops too many to fit at `min_gap`.
pub fn positions(times: &[i64], min_gap: f32) -> Vec<f32> {
    let n = times.len();
    match n {
        0 => return Vec::new(),
        1 => return vec![0.5],
        _ => {}
    }
    let even = || (0..n).map(|i| i as f32 / (n - 1) as f32).collect::<Vec<_>>();
    // A stop without a date (0) is drawn with the oldest dated one rather than stretching the rail back to 1970.
    let floor = times.iter().copied().filter(|&t| t > 0).min().unwrap_or(0);
    let ceil = times.iter().copied().max().unwrap_or(0);
    if ceil <= floor || (n - 1) as f32 * min_gap >= 1.0 {
        return even();
    }
    let span = (ceil - floor) as f64;
    let mut at: Vec<f32> = times
        .iter()
        .map(|&t| ((t.max(floor) - floor) as f64 / span) as f32)
        .collect();
    for i in 1..n {
        at[i] = at[i].max(at[i - 1] + min_gap);
    }
    if at[n - 1] > 1.0 {
        at[n - 1] = 1.0;
        for i in (0..n - 1).rev() {
            at[i] = at[i].min(at[i + 1] - min_gap);
        }
    }
    at
}

/// The stop nearest to `at` (a fraction of the rail), by index into `positions`.
pub fn nearest(positions: &[f32], at: f32) -> Option<usize> {
    positions
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - at).abs().total_cmp(&(b.1 - at).abs()))
        .map(|(i, _)| i)
}

/// How many stops sit in each of [`BARS`] equal slices of the time the rail covers, smoothed a little and
/// scaled so the busiest slice is 1. All zero when there are no stops.
pub fn density(times: &[i64]) -> [f32; BARS] {
    let mut bars = [0f32; BARS];
    let floor = times.iter().copied().filter(|&t| t > 0).min().unwrap_or(0);
    let ceil = times.iter().copied().max().unwrap_or(0);
    if times.is_empty() {
        return bars;
    }
    let span = (ceil - floor).max(1) as f64;
    for &t in times {
        let slice = (((t.max(floor) - floor) as f64 / span) * BARS as f64) as usize;
        bars[slice.min(BARS - 1)] += 1.0;
    }
    let mut smooth = [0f32; BARS];
    for i in 0..BARS {
        let before = if i > 0 { bars[i - 1] } else { 0. };
        let after = if i + 1 < BARS { bars[i + 1] } else { 0. };
        smooth[i] = bars[i] + 0.4 * (before + after);
    }
    let top = smooth.iter().copied().fold(0f32, f32::max);
    if top > 0. {
        for bar in &mut smooth {
            *bar /= top;
        }
    }
    smooth
}

/// A stop's color on the rail: Git blue, Windows Restore purple, backups amber, the journal the muted gray.
/// The three hues are the ones Glint's places use (`theme::place_tone`), which read on both themes.
pub fn source_color(source: MomentSource, t: &theme::Tokens) -> Hsla {
    match source {
        MomentSource::GitCommit => rgb(0x3b82f6).into(),
        MomentSource::VssShadow => rgb(0x8b5cf6).into(),
        MomentSource::ResticBackup => rgb(0xf59e0b).into(),
        MomentSource::UsnActivity => t.muted_foreground,
    }
}

/// The order the sources are listed in (chips and the like): the ones with the most to say first.
pub const SOURCES: [MomentSource; 4] = [
    MomentSource::GitCommit,
    MomentSource::VssShadow,
    MomentSource::ResticBackup,
    MomentSource::UsnActivity,
];

/// "just now", "2 hours ago", "3 days ago"…, for something that happened `ms_ago` milliseconds back.
pub fn age_phrase(ms_ago: i64) -> SharedString {
    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    let n = |unit: i64| (ms_ago / unit) as usize;
    if ms_ago < MINUTE {
        t!("just now")
    } else if ms_ago < HOUR {
        match n(MINUTE) {
            1 => t!("1 minute ago"),
            m => t!("{n} minutes ago", n = m),
        }
    } else if ms_ago < DAY {
        match n(HOUR) {
            1 => t!("1 hour ago"),
            h => t!("{n} hours ago", n = h),
        }
    } else if ms_ago < 30 * DAY {
        match n(DAY) {
            1 => t!("1 day ago"),
            d => t!("{n} days ago", n = d),
        }
    } else if ms_ago < 365 * DAY {
        match n(30 * DAY) {
            1 => t!("1 month ago"),
            m => t!("{n} months ago", n = m),
        }
    } else {
        match n(365 * DAY) {
            1 => t!("1 year ago"),
            y => t!("{n} years ago", n = y),
        }
    }
}

/// What a screen reader says of a stop: "Git commit 3f2a1c, 2 hours ago".
pub fn stop_name(m: &Moment, now_ms: i64) -> SharedString {
    let age = if m.timestamp_ms > 0 {
        age_phrase(now_ms.saturating_sub(m.timestamp_ms))
    } else {
        SharedString::default()
    };
    let mut name = match m.source {
        MomentSource::GitCommit => t!("Git commit {id}", id = m.detail.clone()),
        MomentSource::VssShadow => t!(
            "Windows {kind} of {drive}",
            kind = m.label.to_lowercase(),
            drive = m.detail.clone()
        ),
        MomentSource::ResticBackup => t!("{label}", label = m.label.clone()),
        MomentSource::UsnActivity => t!("Journal activity, {changes}", changes = m.detail.clone()),
    }
    .to_string();
    if !age.is_empty() {
        name.push_str(", ");
        name.push_str(&age);
    }
    name.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_sit_where_their_time_puts_them_and_never_closer_than_the_gap() {
        let day = 86_400_000;
        let times = [day, 10 * day, 10 * day + 1, 10 * day + 2, 20 * day];
        let at = positions(&times, 0.05);
        assert_eq!(at.len(), 5);
        assert!((at[0] - 0.0).abs() < 1e-6 && (at[4] - 1.0).abs() < 1e-6);
        // The three commits of one afternoon are pushed apart, in order, around where the day puts them.
        assert!(at.windows(2).all(|w| w[1] - w[0] >= 0.05 - 1e-6), "{at:?}");
        assert!((at[2] - 0.47).abs() < 0.1, "{at:?}");
        // A lone stop is in the middle; stops at one time, or too many for the gap, are even.
        assert_eq!(positions(&[5], 0.05), vec![0.5]);
        assert_eq!(positions(&[7, 7, 7], 0.05), vec![0.0, 0.5, 1.0]);
        assert_eq!(positions(&[1, 2, 3], 0.6), vec![0.0, 0.5, 1.0]);
        assert!(positions(&[], 0.05).is_empty());
    }

    #[test]
    fn crowding_at_the_end_pushes_back_not_off_the_rail() {
        let at = positions(&[0, 1_000, 1_001, 1_002, 1_003], 0.1);
        assert!(at.iter().all(|p| (0.0..=1.0).contains(p)), "{at:?}");
        assert!(at.windows(2).all(|w| w[1] - w[0] >= 0.1 - 1e-6), "{at:?}");
    }

    #[test]
    fn the_pointer_snaps_to_the_nearest_stop() {
        let at = [0.0, 0.2, 0.9, 1.0];
        assert_eq!(nearest(&at, 0.05), Some(0));
        assert_eq!(nearest(&at, 0.15), Some(1));
        assert_eq!(nearest(&at, 0.5), Some(1));
        assert_eq!(nearest(&at, 0.7), Some(2));
        assert_eq!(nearest(&at, 5.0), Some(3));
        assert_eq!(nearest(&[], 0.5), None);
    }

    #[test]
    fn the_sparkline_peaks_where_the_stops_crowd() {
        let times: Vec<i64> = (0..30).map(|i| i * 10).chain([10_000, 20_000]).collect();
        let bars = density(&times);
        assert!(
            (bars[0] - 1.0).abs() < 1e-6,
            "the first slice holds the crowd: {bars:?}"
        );
        assert!(bars[BARS - 1] > 0. && bars[BARS - 1] < 0.2);
        assert!(density(&[]).iter().all(|b| *b == 0.));
    }

    #[test]
    fn a_stop_is_named_by_its_kind_its_id_and_its_age() {
        let _guard = crate::testing::lock();
        let m = Moment {
            id: "git-3f2a1c9".into(),
            timestamp_ms: 1_000_000_000_000,
            label: "fix".into(),
            detail: "3f2a1c9".into(),
            source: MomentSource::GitCommit,
            shadow_path: None,
            commit_oid: None,
            repo_root: None,
            plan_id: None,
            snapshot_id: None,
        };
        assert_eq!(
            stop_name(&m, m.timestamp_ms + 2 * 3_600_000),
            "Git commit 3f2a1c9, 2 hours ago"
        );
        assert_eq!(age_phrase(30_000), "just now");
        assert_eq!(age_phrase(86_400_000), "1 day ago");
        assert_eq!(age_phrase(3 * 86_400_000), "3 days ago");
        assert_eq!(age_phrase(90 * 86_400_000), "3 months ago");
    }
}
