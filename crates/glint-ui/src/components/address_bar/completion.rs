//! What the address box offers as you type (`AddressInput` in AddressBar.tsx), without any window: split the
//! typed text, decide which folder to list, rank what it holds, and complete a choice.
//!
//! Everything here is plain data in and out, so it is tested on its own; the view (`mod.rs`) only lists the
//! folder in the background and draws the result.

use crate::explorer::{drive_name, Drive, DriveKind, KnownFolder, KnownId};
use glint_model::entries::{is_archive_file, is_hidden, FileEntry};
use glint_model::js::is_js_space;
use glint_model::rank::{edits_allowed, typo_distance};

/// How many suggestions the list shows.
pub(super) const SHOWN: usize = 10;

/// Matches stop being collected at this many (starts-with and contains), as the TypeScript's loop did.
const COLLECT: usize = 48;

/// What the typed text means: the folder whose names to offer, and the start of the name being typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Typed {
    /// The text up to and including the last separator ("" when none has been typed, or for "This PC").
    pub parent_dir: String,
    /// What follows it, in lower case.
    pub prefix: String,
}

/// Splits typed text into the folder to list and the name being typed (the `useMemo` on `text`).
pub(super) fn split_typed(text: &str) -> Typed {
    let raw = text.trim_matches(is_js_space);
    if raw.is_empty() || raw.eq_ignore_ascii_case("this pc") {
        return Typed::default();
    }
    match raw.rfind(['\\', '/']) {
        // The separators are one byte, so the cut falls on a character boundary.
        Some(at) => Typed {
            parent_dir: raw.get(..=at).unwrap_or("").to_string(),
            prefix: raw.get(at + 1..).unwrap_or("").to_lowercase(),
        },
        None => Typed {
            parent_dir: String::new(),
            prefix: raw.to_lowercase(),
        },
    }
}

/// Whether `dir` is a place a listing makes sense for: "C:\…" or "\\server\share…". Anything else (a bare
/// "remote:" id, "shell:…") is not listed while typing.
pub(super) fn lists_folder(dir: &str) -> bool {
    let b = dir.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') {
        return true;
    }
    // \\server\share: a server name, a backslash, then something that is not a backslash.
    let Some(rest) = dir.strip_prefix("\\\\") else {
        return false;
    };
    match rest.split_once('\\') {
        Some((server, share)) => !server.is_empty() && share.chars().next().is_some_and(|c| c != '\\'),
        None => false,
    }
}

/// The key a folder's listing is cached under: the folder with one trailing backslash, lower case.
pub(super) fn cache_key(dir: &str) -> String {
    format!("{}\\", dir.trim_end_matches(['\\', '/'])).to_lowercase()
}

/// How a suggestion (or a menu item) looks: its icon, and in the view its colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Look {
    Drive(DriveKind),
    Known(KnownId),
    Folder,
    Archive,
    File,
    Recent,
    Gallery,
    RecycleBin,
    Remote,
}

/// One line of the suggestion list, or of the menu a chevron opens.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Suggestion {
    pub name: String,
    /// A drive's file system, a folder's path: the small text at the end.
    pub detail: Option<String>,
    pub path: String,
    /// Something to go into (a folder or an archive).
    pub dir: bool,
    /// An archive: opened like a folder, so Tab completes it without a trailing separator.
    pub archive: bool,
    pub look: Look,
}

/// What `suggest` needs to know.
pub(super) struct Ask<'a> {
    pub typed: &'a Typed,
    /// The listing of `typed.parent_dir`, when it has arrived.
    pub entries: Option<&'a [FileEntry]>,
    pub drives: &'a [Drive],
    pub known: &'a [KnownFolder],
    pub show_hidden: bool,
}

/// Dirs before files, otherwise as they came (the sort is stable).
fn dirs_first(a: &Suggestion, b: &Suggestion) -> std::cmp::Ordering {
    // `false` sorts before `true`, so compare the negation.
    (!a.dir).cmp(&!b.dir)
}

/// The suggestions for what is typed (`suggestions` in AddressInput): with no folder typed yet, the drives
/// and known folders that match; with one, its entries whose names start with, contain or nearly match the
/// rest, in that order, folders first in each group. At most ten.
pub(super) fn suggest(ctx: &Ask<'_>) -> Vec<Suggestion> {
    let Typed { parent_dir, prefix } = ctx.typed;
    if parent_dir.is_empty() {
        if prefix.is_empty() {
            return Vec::new();
        }
        let mut items = Vec::new();
        for drive in ctx.drives {
            let label = drive_name(drive);
            if drive.path.to_lowercase().starts_with(prefix.as_str()) || label.to_lowercase().contains(prefix.as_str())
            {
                items.push(Suggestion {
                    name: label,
                    detail: Some(drive.path.clone()),
                    path: drive.path.clone(),
                    dir: true,
                    archive: false,
                    look: Look::Drive(drive.kind),
                });
            }
        }
        for folder in ctx.known {
            if folder.name.to_lowercase().contains(prefix.as_str())
                || folder.path.to_lowercase().contains(prefix.as_str())
            {
                items.push(Suggestion {
                    name: folder.name.to_string(),
                    detail: Some(folder.path.clone()),
                    path: folder.path.clone(),
                    dir: true,
                    archive: false,
                    look: Look::Known(folder.id),
                });
            }
        }
        items.truncate(SHOWN);
        return items;
    }
    let Some(entries) = ctx.entries else {
        return Vec::new();
    };
    let mut starts = Vec::new();
    let mut contains = Vec::new();
    // Typed with a typo ("Dowloads"): names a letter or two off, fewest first, after the rest.
    let mut close: Vec<(Suggestion, usize)> = Vec::new();
    let typos = edits_allowed(prefix) > 0;
    for entry in entries {
        if !ctx.show_hidden && is_hidden(entry) {
            continue;
        }
        let archive = is_archive_file(entry);
        let item = Suggestion {
            name: entry.name.clone(),
            detail: None,
            path: entry.path.clone(),
            dir: entry.dir || archive,
            archive,
            look: if entry.dir {
                Look::Folder
            } else if archive {
                Look::Archive
            } else {
                Look::File
            },
        };
        if prefix.is_empty() || entry.lower.starts_with(prefix.as_str()) {
            starts.push(item);
        } else if entry.lower.contains(prefix.as_str()) {
            contains.push(item);
        } else if typos {
            if let Some(edits) = typo_distance(prefix, &entry.name) {
                close.push((item, edits));
            }
        }
        if starts.len() + contains.len() >= COLLECT {
            break;
        }
    }
    starts.sort_by(dirs_first);
    contains.sort_by(dirs_first);
    close.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| dirs_first(&a.0, &b.0)));
    starts
        .into_iter()
        .chain(contains)
        .chain(close.into_iter().map(|(item, _)| item))
        .take(SHOWN)
        .collect()
}

/// The text Tab (or a click on a suggestion's name) puts in the box: a folder with a separator after it, in
/// the kind the typed text used; an archive or a file as it is (`completeWith`).
pub(super) fn complete_with(text: &str, item: &Suggestion) -> String {
    let sep = if text.contains('/') && !text.contains('\\') {
        '/'
    } else {
        '\\'
    };
    if item.dir && !item.archive {
        if item.path.ends_with(sep) {
            item.path.clone()
        } else {
            format!("{}{sep}", item.path)
        }
    } else {
        item.path.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, folder, hidden};

    fn names(items: &[Suggestion]) -> Vec<&str> {
        items.iter().map(|s| s.name.as_str()).collect()
    }

    fn in_dir(typed: &str, entries: &[FileEntry]) -> Vec<Suggestion> {
        let typed = split_typed(typed);
        suggest(&Ask {
            typed: &typed,
            entries: Some(entries),
            drives: &[],
            known: &[],
            show_hidden: false,
        })
    }

    #[test]
    fn typed_text_splits_into_the_folder_to_list_and_the_start_of_a_name() {
        let t = split_typed("C:\\Users\\Sh");
        assert_eq!((t.parent_dir.as_str(), t.prefix.as_str()), ("C:\\Users\\", "sh"));
        let t = split_typed("  C:/Users/ ");
        assert_eq!((t.parent_dir.as_str(), t.prefix.as_str()), ("C:/Users/", ""));
        // A name with no separator is looked for among the drives and known folders.
        let t = split_typed("Down");
        assert_eq!((t.parent_dir.as_str(), t.prefix.as_str()), ("", "down"));
        // This PC and nothing are not a prefix.
        assert_eq!(split_typed("This PC"), Typed::default());
        assert_eq!(split_typed("   "), Typed::default());
        // Arabic names lower-case as Unicode, and the cut is on the separator.
        let t = split_typed("C:\\ملفات\\التق");
        assert_eq!((t.parent_dir.as_str(), t.prefix.as_str()), ("C:\\ملفات\\", "التق"));
    }

    #[test]
    fn only_drive_and_share_folders_are_listed_while_typing() {
        assert!(lists_folder("C:\\"));
        assert!(lists_folder("d:/stuff/"));
        assert!(lists_folder("\\\\server\\share\\"));
        assert!(!lists_folder("C:"));
        assert!(!lists_folder("\\\\server\\"));
        assert!(!lists_folder("\\\\server\\\\x"));
        assert!(!lists_folder("remote:photos\\"));
        assert!(!lists_folder(""));
    }

    #[test]
    fn folders_are_cached_by_one_lower_case_key_however_they_were_typed() {
        assert_eq!(cache_key("C:\\Users\\"), "c:\\users\\");
        // Only the separators at the end are made one (the TypeScript's `replace(/[\\/]+$/, "\\")`).
        assert_eq!(cache_key("C:/Users//"), "c:/users\\");
        assert_eq!(cache_key("C:\\"), "c:\\");
    }

    #[test]
    fn starts_with_come_first_then_contains_with_folders_ahead_in_each() {
        let entries = [
            file("C:\\", "my_notes.txt", 1, 1),
            file("C:\\", "notes.txt", 1, 1),
            folder("C:\\", "Notes folder"),
            file("C:\\", "zzz", 1, 1),
        ];
        assert_eq!(
            names(&in_dir("C:\\notes", &entries)),
            ["Notes folder", "notes.txt", "my_notes.txt"]
        );
        // Nothing typed after the separator lists the folder, folders first.
        assert_eq!(
            names(&in_dir("C:\\", &entries)),
            ["Notes folder", "my_notes.txt", "notes.txt", "zzz"]
        );
    }

    #[test]
    fn a_name_a_letter_off_is_offered_only_for_a_word_of_four_letters_or_more() {
        let entries = [folder("C:\\", "Downloads"), folder("C:\\", "Documents")];
        assert_eq!(names(&in_dir("C:\\Dowloads", &entries)), ["Downloads"]);
        // "dwn" is too short to forgive anything, and nothing contains it.
        assert!(in_dir("C:\\dwn", &entries).is_empty());
    }

    #[test]
    fn hidden_items_wait_for_the_setting_and_at_most_ten_are_shown() {
        let mut entries: Vec<FileEntry> = (0..30).map(|i| file("C:\\", &format!("a{i:02}.txt"), 1, 1)).collect();
        entries.insert(0, hidden("C:\\", "a-hidden.txt"));
        let typed = split_typed("C:\\a");
        let ctx = |show_hidden| Ask {
            typed: &typed,
            entries: Some(&entries),
            drives: &[],
            known: &[],
            show_hidden,
        };
        let shown = suggest(&ctx(false));
        assert_eq!(shown.len(), SHOWN);
        assert!(!names(&shown).contains(&"a-hidden.txt"));
        assert!(names(&suggest(&ctx(true))).contains(&"a-hidden.txt"));
    }

    #[test]
    fn an_archive_is_something_to_go_into_but_completes_without_a_separator() {
        let entries = [file("C:\\", "photos.zip", 1, 1), folder("C:\\", "photos")];
        let items = in_dir("C:\\pho", &entries);
        // Both are "something to go into", so neither is ahead of the other: they stay as they came.
        assert_eq!(names(&items), ["photos.zip", "photos"]);
        let (zip, dir) = (&items[0], &items[1]);
        assert!(zip.dir && zip.archive && zip.look == Look::Archive);
        assert!(dir.dir && !dir.archive && dir.look == Look::Folder);
        assert_eq!(complete_with("C:\\pho", zip), "C:\\photos.zip");
        assert_eq!(complete_with("C:\\pho", dir), "C:\\photos\\");
        // Typed with forward slashes, folders complete with one.
        assert_eq!(complete_with("C:/pho", dir), "C:\\photos/");
    }

    #[test]
    fn drives_and_known_folders_match_a_name_with_no_folder_typed() {
        let drives = [Drive {
            path: "C:\\".into(),
            label: String::new(),
            kind: DriveKind::Fixed,
            fs: "NTFS".into(),
            total: 0,
            free: 0,
        }];
        let known = [KnownFolder {
            id: KnownId::Downloads,
            name: "Downloads",
            path: "C:\\Users\\me\\Downloads".into(),
        }];
        let typed = split_typed("loc");
        let items = suggest(&Ask {
            typed: &typed,
            entries: None,
            drives: &drives,
            known: &known,
            show_hidden: false,
        });
        assert_eq!(names(&items), ["Local Disk (C:)"]);
        assert_eq!(items[0].detail.as_deref(), Some("C:\\"));
        let typed = split_typed("down");
        let items = suggest(&Ask {
            typed: &typed,
            entries: None,
            drives: &drives,
            known: &known,
            show_hidden: false,
        });
        assert_eq!(names(&items), ["Downloads"]);
        assert_eq!(items[0].look, Look::Known(KnownId::Downloads));
        // Nothing typed offers nothing.
        let typed = split_typed("");
        assert!(suggest(&Ask {
            typed: &typed,
            entries: None,
            drives: &drives,
            known: &known,
            show_hidden: false
        })
        .is_empty());
    }

    #[test]
    fn arabic_names_match_by_prefix() {
        let entries = [folder("C:\\", "التقارير"), file("C:\\", "تقرير.pdf", 1, 1)];
        assert_eq!(names(&in_dir("C:\\التق", &entries)), ["التقارير"]);
    }
}
