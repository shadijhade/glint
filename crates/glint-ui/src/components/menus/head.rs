//! What a menu's head says: the item's name, a line of facts, and its badges (docs/gpui/aware/PLAN.md, F1).
//!
//! Pure: everything is read from the entries, the answers and `Extras`, so a head is built without a window.
//! A fact is said only when it is known (R1): unknown is nothing at all.

use super::event::MenuEvent;
use super::spec::{Badge, Head, HeadPicture, Run, Task, Tone};
use crate::kit::prelude::*;
use crate::{i18n, t};
use glint_model::aware::{Probed, Signer};
use glint_model::entries::{attr, cloud_state, is_hidden, CloudState, FileEntry};
use glint_model::file_types::{kind_of, type_label};
use glint_model::format::{format_count, format_date, format_size};
use glint_model::paths::base_name;

/// What the head needs that isn't in the entries or the answers.
#[derive(Clone, Debug, Default)]
pub struct Extras {
    /// Now, in Unix milliseconds (for "Changed …").
    pub now: i64,
    /// Git's word for the one target: "Changed" or "New".
    pub git: Option<SharedString>,
}

/// How many identical copies' paths a badge's tooltip lists.
const TWINS_IN_TIP: usize = 5;

/// The head of the menu for `targets`. `probed` is the one target's answers (used only with exactly one).
pub fn head_of(targets: &[FileEntry], probed: Option<&Probed>, extras: &Extras) -> Head {
    match targets {
        [] => folder_head(SharedString::default(), None, None),
        [only] => one(only, probed, extras),
        several => many(several),
    }
}

/// The head of a folder's own menu: its name, how many items it shows, and its Git branch.
pub fn folder_head(name: SharedString, items: Option<usize>, branch: Option<SharedString>) -> Head {
    folder_head_with(name, items, branch, None)
}

/// [`folder_head`] with what Aware found out about the folder: its whole size, once it is counted, after how many
/// items it shows.
pub fn folder_head_with(
    name: SharedString,
    items: Option<usize>,
    branch: Option<SharedString>,
    probed: Option<&Probed>,
) -> Head {
    let mut line = match items {
        Some(1) => t!("1 item"),
        Some(n) => t!("{n} items", n = format_count(n)),
        None => SharedString::default(),
    };
    if let Some(inside) = probed.and_then(|probed| probed.inside).filter(|inside| inside.done && inside.bytes > 0) {
        let size = format_size(inside.bytes);
        line = if line.is_empty() { size.into() } else { format!("{line} · {size}").into() };
    }
    let badges = branch
        .map(|branch| Badge {
            label: branch,
            icon: IconName::GitBranch,
            tone: Tone::Plain,
            tip: None,
            run: Run::Nothing,
        })
        .into_iter()
        .collect();
    Head {
        title: name,
        picture: HeadPicture::Folder,
        line,
        badges,
        changed: None,
    }
}

/// "Changed yesterday": none when the date isn't known.
fn changed(entry: &FileEntry, extras: &Extras) -> Option<SharedString> {
    if entry.modified == 0 {
        return None;
    }
    let when = format_date(entry.modified, Some(extras.now), i18n::language());
    // "Changed today 9:52 PM": the day's word starts a line of its own in a list, and not here.
    let when = match when.chars().next() {
        Some(first) if when.starts_with("Today") || when.starts_with("Yesterday") => {
            let rest = when.get(first.len_utf8()..).unwrap_or("");
            format!("{}{rest}", first.to_lowercase())
        }
        _ => when,
    };
    (!when.is_empty()).then(|| t!("Changed {when}", when = when))
}

fn files_word(n: u64) -> SharedString {
    match n {
        1 => t!("1 file"),
        n => t!("{n} files", n = format_count(clamp(n))),
    }
}

fn folders_word(n: u64) -> SharedString {
    match n {
        1 => t!("1 folder"),
        n => t!("{n} folders", n = format_count(clamp(n))),
    }
}

/// A count as the formatter takes it (a 64-bit count never overflows a 64-bit `usize`; 32-bit builds clamp).
fn clamp(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

fn one(entry: &FileEntry, probed: Option<&Probed>, extras: &Extras) -> Head {
    let line = if entry.dir {
        folder_line(probed)
    } else {
        file_line(entry, probed)
    };
    Head {
        title: entry.name.clone().into(),
        picture: if entry.dir {
            HeadPicture::Folder
        } else {
            HeadPicture::Kind(kind_of(entry))
        },
        line: line.into(),
        badges: badges(entry, probed, extras),
        changed: changed(entry, extras),
    }
}

fn folder_line(probed: Option<&Probed>) -> String {
    let mut parts: Vec<String> = vec![t!("Folder").to_string()];
    if let Some(inside) = probed.and_then(|p| p.inside).filter(|inside| inside.done) {
        if inside.files > 0 {
            parts.push(files_word(inside.files).to_string());
        }
        if inside.files > 0 || inside.bytes > 0 {
            parts.push(format_size(inside.bytes));
        }
    }
    parts.join(" · ")
}

fn file_line(entry: &FileEntry, probed: Option<&Probed>) -> String {
    let mut parts: Vec<String> = vec![type_label(entry), format_size(entry.size)];
    if let Some(probed) = probed {
        if let Some(media) = probed.media {
            if media.width > 0 && media.height > 0 {
                parts.push(t!("{w} × {h}", w = media.width, h = media.height).to_string());
            }
            if media.seconds > 0.0 {
                // As a person says it ("2 min 31 s"), in the dock's words.
                parts.push(crate::components::status_bar::sentence::length(media.seconds).to_string());
            }
        }
        if let Some(text) = probed.text.filter(|text| !text.binary) {
            parts.push(match text.lines {
                1 => t!("1 line").to_string(),
                n => t!("{n} lines", n = format_count(clamp(n))).to_string(),
            });
        }
        if let Some(pages) = probed.pages {
            parts.push(match pages {
                1 => t!("1 page").to_string(),
                n => t!("{n} pages", n = format_count(clamp(u64::from(n)))).to_string(),
            });
        }
    }
    parts.join(" · ")
}

fn many(targets: &[FileEntry]) -> Head {
    let folders = targets.iter().filter(|entry| entry.dir).count();
    let files = targets.len() - folders;
    let bytes: u64 = targets.iter().filter(|entry| !entry.dir).map(|entry| entry.size).sum();
    let mut counts: Vec<String> = Vec::new();
    if files > 0 {
        counts.push(files_word(files as u64).to_string());
    }
    if folders > 0 {
        counts.push(folders_word(folders as u64).to_string());
    }
    let mut line = counts.join(", ");
    if files > 0 {
        line = format!("{line} · {}", format_size(bytes));
    }
    Head {
        title: t!("{n} items", n = format_count(targets.len())),
        picture: HeadPicture::Several,
        line: line.into(),
        badges: Vec::new(),
        changed: None,
    }
}

fn badge(label: SharedString, icon: IconName, tone: Tone, tip: Option<SharedString>, run: Run) -> Badge {
    Badge {
        label,
        icon,
        tone,
        tip,
        run,
    }
}

/// What is worth pointing out about one target, most important first.
fn badges(entry: &FileEntry, probed: Option<&Probed>, extras: &Extras) -> Vec<Badge> {
    let path = &entry.path;
    let mut all: Vec<Badge> = Vec::new();
    if !entry.dir {
        if let Some(probed) = probed {
            if let Some(first) = probed.locked_by.as_ref().and_then(|names| names.first()) {
                let names = probed.locked_by.iter().flatten().cloned().collect::<Vec<_>>().join(", ");
                all.push(badge(
                    t!("In use by {app}", app = first),
                    IconName::Lock,
                    Tone::Warn,
                    Some(names.into()),
                    Run::Ask(MenuEvent::Locks {
                        paths: vec![path.clone()],
                    }),
                ));
            }
            let x_ray = || Run::Ask(MenuEvent::XRay { path: path.clone() });
            if let Some(origin) = probed.origin.as_ref().filter(|origin| origin.blocked) {
                let label = if origin.host.is_empty() {
                    t!("From the internet")
                } else {
                    t!("From {host}", host = origin.host)
                };
                all.push(badge(
                    label,
                    IconName::Download,
                    Tone::Warn,
                    Some(t!("Windows marked it as downloaded")),
                    x_ray(),
                ));
            }
            match &probed.signer {
                Some(Signer::Unsigned) => {
                    all.push(badge(t!("Not signed"), IconName::ShieldAlert, Tone::Warn, None, x_ray()));
                }
                Some(Signer::Signed(name)) => {
                    all.push(badge(
                        t!("Signed by {name}", name = name),
                        IconName::ShieldCheck,
                        Tone::Good,
                        None,
                        x_ray(),
                    ));
                }
                None => {}
            }
            if let Some(folder) = probed.extracted.as_ref().and_then(Option::as_ref) {
                all.push(badge(
                    t!("Already extracted"),
                    IconName::FolderOpen,
                    Tone::Good,
                    Some(folder.clone().into()),
                    Run::Do(Task::OpenExtracted(folder.clone())),
                ));
            }
            if let Some(first) = probed.twins.as_ref().and_then(|twins| twins.first()) {
                let count = probed.twins.as_ref().map_or(0, Vec::len);
                let tip = probed
                    .twins
                    .iter()
                    .flatten()
                    .take(TWINS_IN_TIP)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                let label = match count {
                    1 => t!("1 identical copy"),
                    n => t!("{n} identical copies", n = format_count(n)),
                };
                all.push(badge(
                    label,
                    IconName::Files,
                    Tone::Plain,
                    Some(tip.into()),
                    Run::Ask(MenuEvent::OpenFileLocation { path: first.clone() }),
                ));
            }
        }
    }
    match cloud_state(entry) {
        Some(CloudState::Online) => {
            all.push(badge(t!("Online only"), IconName::CloudOff, Tone::Plain, None, Run::Nothing));
        }
        Some(CloudState::Kept) => {
            all.push(badge(
                t!("Always on this PC"),
                IconName::CloudDownload,
                Tone::Plain,
                None,
                Run::Nothing,
            ));
        }
        _ => {}
    }
    if !entry.dir && entry.attrs & attr::READONLY != 0 {
        all.push(badge(t!("Read-only"), IconName::PencilOff, Tone::Plain, None, Run::Nothing));
    }
    if is_hidden(entry) {
        all.push(badge(t!("Hidden"), IconName::EyeOff, Tone::Plain, None, Run::Nothing));
    }
    if !entry.dir {
        if let Some(target) = probed.and_then(|probed| probed.target.as_ref()) {
            all.push(badge(
                t!("Points to {name}", name = base_name(target)),
                IconName::ExternalLink,
                Tone::Plain,
                Some(target.clone().into()),
                Run::Ask(MenuEvent::OpenFileLocation { path: target.clone() }),
            ));
        }
    }
    if let Some(word) = &extras.git {
        all.push(badge(word.clone(), IconName::GitBranch, Tone::Plain, None, Run::Nothing));
    }
    all
}
