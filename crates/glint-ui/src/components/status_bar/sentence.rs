//! What the selection weighs and holds, said in the dock's words (docs/gpui/aware/PLAN.md, F10).
//!
//! The counts the bar says today ([`counts`]) gain what was found out about the selection: a folder's inside
//! ("3.2 GB · 1,204 files inside"), and what fits the files ("1,230 lines", "12 min 31 s", "1920 × 1080",
//! "48 pages"). Nothing is said that isn't known (R1): an unknown part is left out, never a placeholder.
//! Nothing here touches GPUI's windows, so the words are tested on their own.

use super::summary::{counts, Summary};
use crate::kit::prelude::*;
use crate::t;
use glint_model::aware::{verdict, CopiedHash, Media, Probed, Verdict};
use glint_model::entries::FileEntry;
use glint_model::format::{format_count, format_size};

/// What is known about the selection beyond its count and its files' bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Deep {
    /// Everything selected, folders' insides included; None when no folder is selected or one isn't known yet.
    pub bytes: Option<u64>,
    /// The files inside the selected folders; None when not known.
    pub inside: Option<u64>,
    /// "1,230 lines", "12 min 31 s", "1920 × 1080", "48 pages".
    pub detail: Option<SharedString>,
    /// Every folder's inside is finished (the numbers have stopped growing).
    pub done: bool,
}

/// The most selected items looked into.
pub const MOST: usize = 64;

/// What is known about `selected`, given what was found out about each.
pub fn deep(selected: &[FileEntry], probed: &dyn Fn(&FileEntry) -> Option<Probed>) -> Deep {
    deep_with(selected, probed, None)
}

/// [`deep`], and for one selected file whether it agrees with the checksum `copied` from the clipboard, once that
/// has been checked: "Matches copied SHA-256" or "Checksum mismatch", after what else is known.
pub fn deep_with(
    selected: &[FileEntry],
    probed: &dyn Fn(&FileEntry) -> Option<Probed>,
    copied: Option<&CopiedHash>,
) -> Deep {
    if selected.is_empty() || selected.len() > MOST {
        return Deep::default();
    }
    let answers: Vec<Option<Probed>> = selected.iter().map(probed).collect();
    if selected.iter().any(|entry| entry.dir) {
        return folders_deep(selected, &answers);
    }
    let detail = detail_of(&answers);
    let said = match (selected, answers.as_slice(), copied) {
        ([only], [Some(probed)], Some(copied)) if !only.dir => checksum_words(probed, copied),
        _ => None,
    };
    let detail = match (detail, said) {
        (Some(detail), Some(said)) => Some(format!("{detail} · {said}").into()),
        (detail, said) => detail.or(said),
    };
    Deep {
        detail,
        ..Deep::default()
    }
}

/// What a file's checksums say about the one copied: None until they have been compared.
fn checksum_words(probed: &Probed, copied: &CopiedHash) -> Option<SharedString> {
    match verdict(probed.sums.as_ref()?, copied)? {
        Verdict::Match(algo) => Some(t!("Matches copied {algo}", algo = algo.name())),
        Verdict::Mismatch => Some(t!("Checksum mismatch")),
    }
}

/// What a selection with folders in it weighs: every folder's inside, and the files' own sizes. Unknown (all of
/// it) while any folder has no answer.
fn folders_deep(selected: &[FileEntry], answers: &[Option<Probed>]) -> Deep {
    let (mut bytes, mut files, mut done) = (0u64, 0u64, true);
    for (entry, answer) in selected.iter().zip(answers) {
        if entry.dir {
            let Some(inside) = answer.as_ref().and_then(|probed| probed.inside) else {
                return Deep::default();
            };
            bytes = bytes.saturating_add(inside.bytes);
            files = files.saturating_add(inside.files);
            done &= inside.done;
        } else {
            bytes = bytes.saturating_add(entry.size);
        }
    }
    Deep {
        bytes: Some(bytes),
        inside: (files > 0).then_some(files),
        detail: None,
        done,
    }
}

/// What fits the selected files, from the first of the four rules that holds for every one of them.
fn detail_of(answers: &[Option<Probed>]) -> Option<SharedString> {
    let all: Vec<&Probed> = answers.iter().map(Option::as_ref).collect::<Option<_>>()?;
    if let [only] = all.as_slice() {
        if let Some(media) = only.media.filter(|media| media.width > 0 && media.height > 0) {
            return Some(size_of_picture(&media));
        }
    }
    if all.iter().all(|probed| probed.media.is_some_and(|media| has_length(&media))) {
        let seconds: f64 = all.iter().filter_map(|probed| probed.media).map(|media| media.seconds).sum();
        return Some(length(seconds));
    }
    if all.iter().all(|probed| probed.text.is_some_and(|text| !text.binary)) {
        let lines = all.iter().filter_map(|probed| probed.text).fold(0u64, |sum, text| sum.saturating_add(text.lines));
        return Some(lines_words(lines));
    }
    if all.iter().all(|probed| probed.pages.is_some()) {
        let pages = all.iter().filter_map(|probed| probed.pages).map(u64::from).sum();
        return Some(pages_words(pages));
    }
    None
}

fn has_length(media: &Media) -> bool {
    media.seconds.is_finite() && media.seconds > 0.
}

/// "1920 × 1080", and " · 2 min 31 s" after it when it has a length.
fn size_of_picture(media: &Media) -> SharedString {
    let across = format!("{} × {}", media.width, media.height);
    if has_length(media) {
        format!("{across} · {}", length(media.seconds)).into()
    } else {
        across.into()
    }
}

/// A length of time as a person says it: "45 s", "12 min 31 s", "1 h 5 min".
pub fn length(seconds: f64) -> SharedString {
    // `as` saturates for floats (and makes NaN zero), so nothing here can overflow.
    let total = (seconds.round() as u64).max(1);
    let (hours, minutes, rest) = (total / 3600, total % 3600 / 60, total % 60);
    if hours > 0 {
        if minutes == 0 {
            t!("{h} h", h = hours)
        } else {
            t!("{h} h {m} min", h = hours, m = minutes)
        }
    } else if minutes > 0 {
        if rest == 0 {
            t!("{m} min", m = minutes)
        } else {
            t!("{m} min {s} s", m = minutes, s = rest)
        }
    } else {
        t!("{s} s", s = rest)
    }
}

/// "1,230 lines", "1 line".
fn lines_words(lines: u64) -> SharedString {
    if lines == 1 {
        t!("1 line")
    } else {
        t!("{n} lines", n = format_count(to_count(lines)))
    }
}

/// "48 pages", "1 page".
fn pages_words(pages: u64) -> SharedString {
    if pages == 1 {
        t!("1 page")
    } else {
        t!("{n} pages", n = format_count(to_count(pages)))
    }
}

/// "1,204 files inside", "1 file inside".
pub fn inside_words(files: u64) -> SharedString {
    if files == 1 {
        t!("1 file inside")
    } else {
        t!("{n} files inside", n = format_count(to_count(files)))
    }
}

fn to_count(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// One piece of the dock's start words. The numbers that can still grow are apart, so the bar can roll them.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    /// Words that don't change while the selection stays: "25 items · 1 selected", "1,230 lines".
    Text(SharedString),
    /// The selection's weight in bytes, said as `format_size` says it.
    Size(u64),
    /// The files inside the selected folders, said by [`inside_words`].
    Inside(u64),
}

/// The pieces of [`counts_deep`], in order; the bar puts " · " between them.
pub fn pieces(s: &Summary, deep: &Deep) -> Vec<Piece> {
    let full = counts(s);
    let mut head: &str = &full;
    if s.selected_files > 0 {
        // The counts end with the files' size; the size is said here, with what is known of the folders.
        let tail = format!(" · {}", format_size(s.selected_bytes));
        head = head.strip_suffix(tail.as_str()).unwrap_or(head);
    }
    let mut out = vec![Piece::Text(head.to_string().into())];
    if let Some(bytes) = deep.bytes {
        out.push(Piece::Size(bytes));
    } else if s.selected_files > 0 {
        out.push(Piece::Size(s.selected_bytes));
    }
    if let Some(files) = deep.inside {
        out.push(Piece::Inside(files));
    }
    if let Some(detail) = &deep.detail {
        out.push(Piece::Text(detail.clone()));
    }
    out
}

/// [`pieces`], starting at the selection when `brief` and something is selected: where the dock's end is tight,
/// what is selected is said, and how much the folder holds is not.
pub fn pieces_in(s: &Summary, deep: &Deep, brief: bool) -> Vec<Piece> {
    let mut out = pieces(s, deep);
    if brief && s.selected > 0 {
        if let Some(Piece::Text(head)) = out.first_mut() {
            let selected = t!("{count} selected", count = format_count(s.selected));
            if let Some(from) = head.find(selected.as_ref()).and_then(|at| head.get(at..)) {
                *head = from.to_string().into();
            }
        }
    }
    out
}

/// The pieces as one line.
pub fn words(pieces: &[Piece]) -> SharedString {
    pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text(text) => text.to_string(),
            Piece::Size(bytes) => format_size(*bytes),
            Piece::Inside(files) => inside_words(*files).to_string(),
        })
        .collect::<Vec<_>>()
        .join(" · ")
        .into()
}

/// The dock's start words: [`counts`], with what `deep` knows.
pub fn counts_deep(s: &Summary, deep: &Deep) -> SharedString {
    if *deep == Deep::default() {
        return counts(s);
    }
    pieces(s, deep)
        .into_iter()
        .map(|piece| match piece {
            Piece::Text(text) => text.to_string(),
            Piece::Size(bytes) => format_size(bytes),
            Piece::Inside(files) => inside_words(files).to_string(),
        })
        .collect::<Vec<_>>()
        .join(" · ")
        .into()
}
