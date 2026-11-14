//! Ported from src/lib/entries.ts.
//!
//! File entries: their attributes, sort orders (natural order), matching and filters, and the decoder for
//! the binary listing format the core writes (`encode_listing` and `encode_items` in fsx.rs).
//!
//! NOTE(port): the TypeScript `ATTR` object is the module `attr` here (a module named `ATTR` would trip the
//! naming lints). Its keys became SCREAMING_CASE constants: `ATTR.recycled` is `attr::RECYCLED`.
//!
//! NOTE(port): the TypeScript comparators returned a number; the Rust ones return `Ordering`.

use std::cmp::Ordering;
use std::collections::HashMap;

use chrono::{Datelike, Days, Local, Months, NaiveDate, TimeZone};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::archives::{is_browsed_ext, is_on_request_ext};
use crate::collate::compare_base_numeric;
use crate::js::{is_js_space, js_round, js_trim};
use crate::paths::join_path;

/// Windows FILE_ATTRIBUTE_* bits the explorer cares about, and some of Glint's own (Windows doesn't use them).
pub mod attr {
    pub const READONLY: u32 = 0x1;
    pub const HIDDEN: u32 = 0x2;
    pub const SYSTEM: u32 = 0x4;
    /// The directory bit the listings set on every folder.
    pub const DIRECTORY: u32 = 0x10;
    pub const REPARSE_POINT: u32 = 0x400;
    pub const OFFLINE: u32 = 0x1000;
    /// In an archive: password-protected.
    pub const ENCRYPTED: u32 = 0x4000;
    /// On a remote connection (with `IN_ARCHIVE`, as its files are read through copies too): see remote/.
    pub const REMOTE: u32 = 0x0400_0000;
    pub const RECALL_ON_OPEN: u32 = 0x40000;
    /// Cloud files (OneDrive...): always kept on this device.
    pub const PINNED: u32 = 0x80000;
    /// Cloud files: only online, their space freed up.
    pub const UNPINNED: u32 = 0x100000;
    pub const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    /// Glint's: a cloud file (OneDrive...) that's synced, or on its way (fsx.rs).
    pub const CLOUD: u32 = 0x0080_0000;
    /// Glint's: a cloud file that isn't in sync yet: changed here, or never uploaded.
    pub const CLOUD_PENDING: u32 = 0x0100_0000;
    /// In the Recycle Bin: its path is its file there, its `origin` the folder it was deleted from (see recycle.rs).
    pub const RECYCLED: u32 = 0x0800_0000;
    /// Compressed with a method Glint can't read.
    pub const UNREADABLE: u32 = 0x1000_0000;
    /// In an ncdu export (with `IN_ARCHIVE`): listed with its size and date, but its contents aren't there.
    pub const LIST_ONLY: u32 = 0x2000_0000;
    /// In a zip or 7z archive: read-only, and extracted to be opened (see archive/tree.rs).
    pub const IN_ARCHIVE: u32 = 0x4000_0000;
    /// Listed from instant search's index, in a folder Windows keeps closed to this account: read-only, and it
    /// can't be opened (see index.rs).
    pub const CLOSED: u32 = 0x8000_0000;
}

/// A folder's Git mark for a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitMark {
    Ignored,
    Untracked,
    Staged,
    Modified,
    Conflicted,
}

/// What the Date taken sort and property filters (width:, taken:, artist:...) read of a photo, video or song.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaFacts {
    /// Milliseconds since the Unix epoch.
    pub taken: Option<i64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Seconds.
    pub duration: Option<f64>,
    /// As the file has them (filters compare them in lower case).
    pub camera: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// One file or folder in a listing.
#[derive(Clone, Debug, PartialEq)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    /// Bytes.
    pub size: u64,
    /// Last write time, ms since the Unix epoch (0 when unknown).
    pub modified: i64,
    pub attrs: u32,
    /// Lowercased name, for sorting and filtering.
    pub lower: String,
    /// Lowercased extension without the dot; "" for folders and names without one.
    pub ext: String,
    /// In the Recycle Bin: the folder it was deleted from (`modified` is when).
    pub origin: Option<String>,
}

/// An entry as the backend sends it; listings leave out the path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawEntry {
    pub name: String,
    pub path: Option<String>,
    pub dir: bool,
    pub size: u64,
    pub modified: i64,
    pub attrs: u32,
    pub origin: Option<String>,
}

/// Fills in path, lower and ext. (The TypeScript did it in place to avoid copies; here the raw entry's
/// strings move into the result, so there are none either.)
pub fn with_details(raw: RawEntry, dir: &str) -> FileEntry {
    let path = raw.path.unwrap_or_else(|| join_path(dir, &raw.name));
    let lower = raw.name.to_lowercase();
    let ext = match lower.rfind('.') {
        // A dot at 0 (".gitignore") starts the name, it doesn't start an extension.
        Some(dot) if !raw.dir && dot > 0 => lower.get(dot + 1..).unwrap_or("").to_string(),
        _ => String::new(),
    };
    FileEntry {
        name: raw.name,
        path,
        dir: raw.dir,
        size: raw.size,
        modified: raw.modified,
        attrs: raw.attrs,
        lower,
        ext,
        origin: raw.origin,
    }
}

/// Where a folder in an archive is: the archive opened like a folder.
#[derive(Clone, Debug, PartialEq)]
pub struct InArchive {
    /// The archive's own path (an archive in an archive included), the start of the folder's.
    pub path: String,
    /// The archive file on disk everything is in (the same, unless archives nest).
    pub outer: String,
    /// The format's label ("zip", "tar.gz", "disk image"…).
    pub format: &'static str,
    /// Files and folders in the whole archive.
    pub entries: u32,
    /// Its list of entries was already in memory; otherwise it took `read_ms` to read.
    pub cached: bool,
    pub read_ms: f64,
}

/// An archive file (on disk or in another archive) Enter opens like a folder: a zip, a 7z... (On a remote it's a
/// file like any: Glint doesn't look inside archives there.)
pub fn is_archive_file(e: &FileEntry) -> bool {
    !e.dir && e.attrs & attr::REMOTE == 0 && is_browsed_ext(&e.ext)
}

/// A file that's an archive by another name (a .docx, a .jar...): Enter opens it in its app, "Open as archive"
/// looks inside.
pub fn is_browsable(e: &FileEntry) -> bool {
    !e.dir && e.attrs & attr::REMOTE == 0 && is_on_request_ext(&e.ext)
}

/// Something inside an archive: read-only, and extracted to be opened or copied.
pub fn is_archived(e: &FileEntry) -> bool {
    e.attrs & attr::IN_ARCHIVE != 0
}

/// On a remote connection: read-only, and downloaded to be opened, previewed or copied (it's `is_archived` too).
pub fn is_remote(e: &FileEntry) -> bool {
    e.attrs & attr::REMOTE != 0
}

/// Listed from the index, in a folder Windows keeps closed to this account: read-only, and it can't be opened.
pub fn is_closed(e: &FileEntry) -> bool {
    e.attrs & attr::CLOSED != 0
}

/// In the Recycle Bin: restored, or deleted for good, but not opened or changed where it is.
pub fn is_recycled(e: &FileEntry) -> bool {
    e.attrs & attr::RECYCLED != 0
}

/// In an archive, but can't be extracted: password-protected, or compressed in a way Glint can't read.
pub fn is_locked(e: &FileEntry) -> bool {
    e.attrs & (attr::ENCRYPTED | attr::UNREADABLE) != 0
}

/// A file in an ncdu export: a name, a size and a date, with nothing to open or preview.
pub fn is_list_only(e: &FileEntry) -> bool {
    e.attrs & (attr::IN_ARCHIVE | attr::LIST_ONLY) == (attr::IN_ARCHIVE | attr::LIST_ONLY)
}

/// Hidden (the Windows attribute).
pub fn is_hidden(e: &FileEntry) -> bool {
    e.attrs & attr::HIDDEN != 0
}

/// OneDrive-style placeholders: reading them would start a download. (TypeScript: `isOnlineOnly`.)
pub fn is_online_only(e: &FileEntry) -> bool {
    e.attrs & (attr::OFFLINE | attr::RECALL_ON_OPEN | attr::RECALL_ON_DATA_ACCESS) != 0
}

/// A cloud file's state (OneDrive's, Dropbox's...), as File Explorer's Status column shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudState {
    Pending,
    Online,
    Kept,
    Local,
}

/// The cloud state of an entry, or `None` when it isn't a cloud file.
pub fn cloud_state(e: &FileEntry) -> Option<CloudState> {
    if e.attrs & attr::CLOUD_PENDING != 0 {
        return Some(CloudState::Pending);
    }
    // Online only, or on its way here when it's to be kept on this device.
    if is_online_only(e) {
        return Some(if e.attrs & attr::PINNED != 0 {
            CloudState::Pending
        } else {
            CloudState::Online
        });
    }
    if e.attrs & attr::PINNED != 0 {
        return Some(CloudState::Kept);
    }
    if e.attrs & (attr::CLOUD | attr::UNPINNED) != 0 {
        Some(CloudState::Local)
    } else {
        None
    }
}

/// In a cloud-synced folder (OneDrive...): synced, on its way, or kept on this device or only online.
pub fn is_cloud_file(e: &FileEntry) -> bool {
    cloud_state(e).is_some()
}

/// What a listing is sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    Name,
    Modified,
    Taken,
    Type,
    Size,
    Dimensions,
    Duration,
    Artist,
    Album,
}

/// A sort: by what, and which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sort {
    pub key: SortKey,
    pub desc: bool,
}

/// Natural order for lowercased names: "file2" before "file10".
///
/// NOTE(port): this orders exactly like the core's `natural_cmp`/`natural_key` (src-tauri/src/fsx.rs), which
/// compares UTF-8 bytes, so characters outside the Basic Multilingual Plane (emoji) sort by code point. The
/// TypeScript compared UTF-16 units, which puts an emoji before U+E000 to U+FFFF; the core never did, and the
/// core's order is the one listings arrive in.
pub fn natural_compare(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    while let (Some(&ca), Some(&cb)) = (a.get(i), b.get(j)) {
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let (si, sj) = (i, j);
            while a.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            while b.get(j).is_some_and(u8::is_ascii_digit) {
                j += 1;
            }
            let (na, nb) = (
                trim_zeros(a.get(si..i).unwrap_or(&[])),
                trim_zeros(b.get(sj..j).unwrap_or(&[])),
            );
            let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
            if ord != Ordering::Equal {
                return ord;
            }
        } else {
            if ca != cb {
                return ca.cmp(&cb);
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn trim_zeros(digits: &[u8]) -> &[u8] {
    let start = digits.iter().position(|&d| d != b'0').unwrap_or(digits.len());
    digits.get(start..).unwrap_or(&[])
}

/// Names in natural order, ties by the name as written (code point order, as the core's sort does).
fn by_name(a: &FileEntry, b: &FileEntry) -> Ordering {
    natural_compare(&a.lower, &b.lower).then_with(|| a.name.cmp(&b.name))
}

/// Size of an entry to sort by (folders sort by their calculated size).
pub type SizeOf<'a> = dyn Fn(&FileEntry) -> u64 + 'a;
/// When a photo or video was taken, in ms (`None` when unknown).
pub type TakenOf<'a> = dyn Fn(&FileEntry) -> Option<i64> + 'a;
/// What a file says of itself (`None` when unread or nothing).
pub type FactsOf<'f> = dyn Fn(&FileEntry) -> Option<&'f MediaFacts> + 'f;

/// Sorts for display. `presorted` entries already come folders-first in name order (listings sort in Rust), so
/// the default view costs nothing (the vector comes back as it is) and ties keep name order. `size_of` lets
/// folders sort by their calculated size.
pub fn sort_entries<'a, 'f: 'a>(
    mut entries: Vec<FileEntry>,
    sort: Sort,
    folders_first: bool,
    presorted: bool,
    size_of: Option<&'a SizeOf<'a>>,
    taken_of: Option<&'a TakenOf<'a>>,
    facts_of: Option<&'a FactsOf<'f>>,
) -> Vec<FileEntry> {
    if presorted && folders_first && sort.key == SortKey::Name && !sort.desc {
        return entries;
    }
    entries.sort_by(entry_order(sort, folders_first, presorted, size_of, taken_of, facts_of));
    entries
}

/// `sorted` with `more` sorted into it, both in `order`: a list that grows (a search's results, batch by batch)
/// sorts only what's new. Ties keep `sorted`'s first, as sorting them all (stably) would.
#[cfg(test)]
pub fn merge_sorted(
    sorted: Vec<FileEntry>,
    mut more: Vec<FileEntry>,
    order: impl Fn(&FileEntry, &FileEntry) -> Ordering,
) -> Vec<FileEntry> {
    more.sort_by(&order);
    let mut out = Vec::with_capacity(sorted.len() + more.len());
    let mut head = sorted.into_iter().peekable();
    let mut tail = more.into_iter().peekable();
    while let (Some(h), Some(t)) = (head.peek(), tail.peek()) {
        let next = if order(t, h) == Ordering::Less {
            tail.next()
        } else {
            head.next()
        };
        out.extend(next);
    }
    out.extend(head);
    out.extend(tail);
    out
}

/// How `sort_entries` orders two entries.
pub fn entry_order<'a, 'f: 'a>(
    sort: Sort,
    folders_first: bool,
    presorted: bool,
    size_of: Option<&'a SizeOf<'a>>,
    taken_of: Option<&'a TakenOf<'a>>,
    facts_of: Option<&'a FactsOf<'f>>,
) -> impl Fn(&FileEntry, &FileEntry) -> Ordering + use<'a, 'f> {
    let group = move |e: &FileEntry| usize::from(!(folders_first && e.dir));
    let size = move |e: &FileEntry| size_of.map_or(e.size, |f| f(e));
    let taken = move |e: &FileEntry| taken_of.and_then(|f| f(e)).unwrap_or(e.modified);
    let facts = move |e: &FileEntry| -> Option<&'f MediaFacts> { facts_of.and_then(|f| f(e)) };
    let pixels = move |e: &FileEntry| {
        let f = facts(e);
        u64::from(f.and_then(|f| f.width).unwrap_or(0)) * u64::from(f.and_then(|f| f.height).unwrap_or(0))
    };
    let text = move |e: &FileEntry, artist: bool| -> &'f str {
        facts(e)
            .and_then(|f| {
                if artist {
                    f.artist.as_deref()
                } else {
                    f.album.as_deref()
                }
            })
            .unwrap_or("")
    };
    let duration = move |e: &FileEntry| facts(e).and_then(|f| f.duration).unwrap_or(-1.0);
    move |a, b| {
        let by = match sort.key {
            SortKey::Name => by_name(a, b),
            SortKey::Modified => a.modified.cmp(&b.modified),
            // When a photo or video was taken, else when the file was modified.
            SortKey::Taken => taken(a).cmp(&taken(b)),
            SortKey::Size => size(a).cmp(&size(b)),
            // Folders first, then by extension. (NOTE(port): extensions compare by code point.)
            SortKey::Type => b.dir.cmp(&a.dir).then_with(|| a.ext.cmp(&b.ext)),
            SortKey::Dimensions => pixels(a).cmp(&pixels(b)),
            // A NaN duration compared as "equal" in the TypeScript too (NaN is falsy there).
            SortKey::Duration => duration(a).partial_cmp(&duration(b)).unwrap_or(Ordering::Equal),
            SortKey::Artist => compare_base_numeric(text(a, true), text(b, true)),
            SortKey::Album => compare_base_numeric(text(a, false), text(b, false)),
        };
        let by = if sort.desc { by.reverse() } else { by };
        group(a)
            .cmp(&group(b))
            .then(by)
            .then_with(|| if presorted { Ordering::Equal } else { by_name(a, b) })
    }
}

/// Splits a query into terms; "double quotes" keep spaces in one term (matches the Rust side).
pub fn terms(query: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in query.chars() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && is_js_space(c) {
            if !current.is_empty() {
                found.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        found.push(current);
    }
    found
}

/// What JavaScript's `.` does not match.
fn is_line_end(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// A glob (`*` any run, `?` any one character) as an anchored regular expression over the whole text.
///
/// NOTE(port): JavaScript's `.` matched one UTF-16 unit, so `?` could not match an emoji (two units). Here `?`
/// matches one character, as a person typing it means.
fn glob_to_regex(glob: &str) -> Option<Regex> {
    const ANY: &str = "[^\\n\\r\\x{2028}\\x{2029}]";
    let mut source = String::from("\\A");
    for c in glob.chars() {
        match c {
            '*' => {
                source.push_str(ANY);
                source.push('*');
            }
            '?' => source.push_str(ANY),
            _ => source.push_str(&regex::escape(c.encode_utf8(&mut [0; 4]))),
        }
    }
    source.push_str("\\z");
    Regex::new(&source).ok()
}

fn size_unit(unit: &str) -> Option<f64> {
    Some(match unit {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1_048_576.0,
        "g" | "gb" => 1_073_741_824.0,
        "t" | "tb" => 1_099_511_627_776.0,
        _ => return None,
    })
}

/// Seconds in each age unit of `dm:` and `taken:` (h, d, w, y).
fn age_unit(unit: &str) -> Option<f64> {
    Some(match unit {
        "h" => 3600.0,
        "d" => 86_400.0,
        "w" => 7.0 * 86_400.0,
        "y" => 365.0 * 86_400.0,
        _ => return None,
    })
}

/// What `git:` takes, and the marks each stands for.
fn git_marks(name: &str) -> Option<&'static [GitMark]> {
    Some(match name {
        "changed" => &[
            GitMark::Modified,
            GitMark::Staged,
            GitMark::Untracked,
            GitMark::Conflicted,
        ],
        "modified" => &[GitMark::Modified],
        "staged" => &[GitMark::Staged],
        "new" | "untracked" => &[GitMark::Untracked],
        "conflict" | "conflicted" => &[GitMark::Conflicted],
        "ignored" => &[GitMark::Ignored],
        _ => return None,
    })
}

/// At least and at most, both included (query.rs's Range).
#[derive(Clone, Copy)]
struct Range {
    min: Option<f64>,
    max: Option<f64>,
}

impl Range {
    fn contains(&self, n: f64) -> bool {
        self.min.is_none_or(|min| n >= min) && self.max.is_none_or(|max| n <= max)
    }
}

/// Splits `>=1080` into its leading run of `<`, `>` and `=` and the rest. `None` for text with a line break
/// (the TypeScript's pattern did not match it either).
fn split_op(value: &str) -> Option<(&str, &str)> {
    if value.chars().any(is_line_end) {
        return None;
    }
    let cut = value.find(|c| !matches!(c, '<' | '>' | '=')).unwrap_or(value.len());
    Some(value.split_at(cut))
}

/// `>1920`, `<=1080` or `1080` (exactly), each number read by `number`.
fn range(value: &str, number: impl Fn(&str) -> Option<f64>) -> Option<Range> {
    let (op, amount) = split_op(value)?;
    let n = number(amount)?;
    match op {
        ">" => Some(Range {
            min: Some(n + 1.0),
            max: None,
        }),
        ">=" => Some(Range {
            min: Some(n),
            max: None,
        }),
        "<" => (n > 0.0).then_some(Range {
            min: None,
            max: Some(n - 1.0),
        }),
        "<=" => Some(Range {
            min: None,
            max: Some(n),
        }),
        "" | "=" => Some(Range {
            min: Some(n),
            max: Some(n),
        }),
        _ => None,
    }
}

fn all_digits(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_digit())
}

fn whole(text: &str) -> Option<f64> {
    if all_digits(text) {
        text.parse().ok()
    } else {
        None
    }
}

/// "90" or "90s", "5m", "1.5h", "1:30" or "1:02:03", in seconds.
///
/// NOTE(port): the TypeScript looked units up with `in`, so the unit "constructor" passed and made a NaN range
/// that matched nothing. Here an unknown unit is simply unreadable (`None`); a search for it matches nothing
/// either way.
fn seconds(text: &str) -> Option<f64> {
    if text.contains(':') {
        let parts: Vec<&str> = text.split(':').collect();
        if parts
            .iter()
            .enumerate()
            .any(|(i, p)| !all_digits(p) || (i > 0 && p.len() > 2))
        {
            return None;
        }
        return Some(
            parts
                .iter()
                .fold(0.0, |total, p| total * 60.0 + p.parse::<f64>().unwrap_or(f64::NAN)),
        );
    }
    let cut = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(cut);
    if number.is_empty() || !unit.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let scale = match unit.to_ascii_lowercase().as_str() {
        "" | "s" | "sec" => 1.0,
        "m" | "min" => 60.0,
        "h" | "hr" => 3600.0,
        _ => return None,
    };
    Some(js_round(number.parse::<f64>().ok()? * scale))
}

/// JavaScript's `new Date(year, ...)` reads years 0 to 99 as 1900 to 1999.
fn js_year(year: i32) -> i32 {
    if (0..=99).contains(&year) {
        1900 + year
    } else {
        year
    }
}

/// Midnight here, in ms since the epoch, of `month0` (0 to 12, rolling into the next year) and `day` (1 or more,
/// rolling into the next month) of `year`: `new Date(year, month0, day).getTime()`.
fn midnight(year: i32, month0: u32, day: u32) -> Option<i64> {
    let date = NaiveDate::from_ymd_opt(js_year(year), 1, 1)?
        .checked_add_months(Months::new(month0))?
        .checked_add_days(Days::new(u64::from(day.checked_sub(1)?)))?;
    let naive = date.and_hms_opt(0, 0, 0)?;
    // A midnight that doesn't exist (a clock jumping over it) is the first moment after the jump, as in JavaScript.
    (0..4)
        .find_map(|h| {
            let shifted = naive.checked_add_signed(chrono::Duration::hours(h))?;
            Local.from_local_datetime(&shifted).earliest()
        })
        .map(|t| t.timestamp_millis())
}

/// `2024`, `2024-06` or `2024-06-15`: midnight here of its first day, and of the first day after.
///
/// NOTE(port): like JavaScript's `Date`, years 0000 to 0099 mean 1900 to 1999.
fn period(text: &str) -> Option<(i64, i64)> {
    let b = text.as_bytes();
    let number = |from: usize, to: usize| -> Option<u32> {
        let digits = b.get(from..to).filter(|d| d.iter().all(u8::is_ascii_digit))?;
        std::str::from_utf8(digits).ok()?.parse().ok()
    };
    let dashed = |at: usize| b.get(at) == Some(&b'-');
    let year = i32::try_from(number(0, 4)?).ok()?;
    match b.len() {
        4 => Some((midnight(year, 0, 1)?, midnight(year + 1, 0, 1)?)),
        7 if dashed(4) => {
            let month = number(5, 7)?;
            if !(1..=12).contains(&month) {
                return None;
            }
            Some((midnight(year, month - 1, 1)?, midnight(year, month, 1)?))
        }
        10 if dashed(4) && dashed(7) => {
            let month = number(5, 7)?;
            if !(1..=12).contains(&month) {
                return None;
            }
            let day = number(8, 10)?;
            let last = NaiveDate::from_ymd_opt(js_year(year), month, 1)?
                .checked_add_months(Months::new(1))?
                .pred_opt()?;
            if day < 1 || day > last.day() {
                return None;
            }
            Some((midnight(year, month - 1, day)?, midnight(year, month - 1, day + 1)?))
        }
        _ => None,
    }
}

/// The keys of what photos, videos and songs say of themselves (query.rs's Prop), and the rest of the term.
fn split_prop(term: &str) -> Option<(&str, &str)> {
    let (key, value) = term.split_once(':')?;
    if !matches!(
        key,
        "width" | "height" | "duration" | "taken" | "camera" | "artist" | "album"
    ) || value.chars().any(is_line_end)
    {
        return None;
    }
    Some((key, value))
}

/// Whether a search holds a property filter (width:, taken:...), which needs files read to apply.
pub fn has_prop_filter(query: &str) -> bool {
    terms(&query.to_lowercase()).iter().any(|t| split_prop(t).is_some())
}

/// Whether a search holds a filter (ext:, size:, dm:, file:, folder:) or a path (src\lib, path:docs), which an
/// instant search service older than the app doesn't read.
pub fn has_filter_or_path(query: &str) -> bool {
    terms(&query.to_lowercase()).iter().any(|t| {
        let keyed = t.split_once(':').is_some_and(|(key, _)| {
            matches!(
                key,
                "ext" | "size" | "dm" | "file" | "files" | "folder" | "folders" | "path"
            )
        });
        keyed || t.contains(['\\', '/'])
    })
}

/// A test of what a file says.
pub type FactsTest = Box<dyn Fn(&MediaFacts) -> bool>;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// `prop_test` at the time of the call (the TypeScript's default for `now`).
#[cfg(test)]
pub fn prop_test_now(key: &str, value: &str) -> Option<FactsTest> {
    prop_test(key, value, now_ms())
}

/// A property filter's test of what a file says (media.rs, as the search applies it), or `None` when it doesn't
/// read right: `width:>1920`, `height:`, `duration:>5m`, `taken:2024` (2024-06, 2024-06-15, after `>` or `<`, or
/// 7d and >1y as dm: goes), `camera:`, `artist:` and `album:` with some of their text. `now` is ms since the epoch.
///
/// NOTE(port): the TypeScript took any other key as a text field and crashed on use; here it is `None`.
pub fn prop_test(key: &str, value: &str, now: i64) -> Option<FactsTest> {
    match key {
        "width" | "height" => {
            let r = range(value, whole)?;
            let is_width = key == "width";
            Some(Box::new(move |f| {
                let n = if is_width { f.width } else { f.height };
                n.is_some_and(|n| r.contains(f64::from(n)))
            }))
        }
        "duration" => {
            let r = range(value, seconds)?;
            Some(Box::new(move |f| f.duration.is_some_and(|d| r.contains(d.floor()))))
        }
        "taken" => {
            let (op, amount) = split_op(value)?;
            let aged = amount
                .char_indices()
                .last()
                .and_then(|(at, unit)| Some((amount.get(..at)?, age_unit(unit.encode_utf8(&mut [0; 4]))?)))
                .filter(|(count, _)| all_digits(count));
            if let Some((count, unit)) = aged {
                let since = now as f64 - count.parse::<f64>().ok()? * unit * 1000.0;
                return Some(if op.starts_with('>') {
                    Box::new(move |f| f.taken.is_some_and(|t| (t as f64) < since))
                } else {
                    Box::new(move |f| f.taken.is_some_and(|t| t as f64 >= since))
                });
            }
            let (start, end) = period(amount)?;
            let (from, to) = match op {
                "" | "=" => (Some(start), Some(end)),
                ">" => (Some(end), None),
                ">=" => (Some(start), None),
                "<" => (None, Some(start)),
                "<=" => (None, Some(end)),
                _ => return None,
            };
            Some(Box::new(move |f| {
                f.taken
                    .is_some_and(|t| from.is_none_or(|from| t >= from) && to.is_none_or(|to| t < to))
            }))
        }
        "camera" | "artist" | "album" => {
            let wanted = js_trim(value).to_lowercase();
            let field = key.to_string();
            Some(Box::new(move |f| {
                let text = match field.as_str() {
                    "camera" => &f.camera,
                    "artist" => &f.artist,
                    _ => &f.album,
                };
                text.as_ref().is_some_and(|t| t.to_lowercase().contains(&wanted))
            }))
        }
        _ => None,
    }
}

/// A test of one entry.
pub type EntryTest<'a> = Box<dyn Fn(&FileEntry) -> bool + 'a>;
/// A folder's Git marks, by lowercase name.
pub type GitMarks = HashMap<String, GitMark>;

/// A filter term (ext:, size:, dm:, file:, folder:) as the index and fd read it (shadui_index::query::parse), for
/// what's on screen; `None` for a name term. One that doesn't read right matches nothing here (the search says
/// what's wrong with it). `git:changed` (modified, staged, new, conflict, ignored) asks the folder's Git marks,
/// so it filters what's on screen only. A property filter (width:, taken:...) asks what the file says (`facts`,
/// `None` until it's read): without them it's left to the search.
fn filter_test<'a, 'f: 'a>(
    term: &str,
    git: Option<&'a GitMarks>,
    facts: Option<&'a FactsOf<'f>>,
    now: i64,
) -> Option<EntryTest<'a>> {
    if let Some((prop, wanted)) = split_prop(term) {
        let Some(facts) = facts else {
            return Some(Box::new(|_| true));
        };
        let test = prop_test(prop, wanted, now);
        return Some(Box::new(move |e| match &test {
            Some(test) if !e.dir => facts(e).is_some_and(test),
            _ => false,
        }));
    }
    let (key, value) = term.split_once(':')?;
    if !matches!(
        key,
        "ext" | "size" | "dm" | "file" | "files" | "folder" | "folders" | "git"
    ) || value.chars().any(is_line_end)
    {
        return None;
    }
    if key == "git" {
        let wanted = git_marks(value);
        return Some(Box::new(move |e| match (wanted, git) {
            (Some(wanted), Some(git)) => git.get(&e.lower).is_some_and(|mark| wanted.contains(mark)),
            _ => false,
        }));
    }
    if key == "ext" {
        let exts: Vec<String> = value
            .split([';', ','])
            .map(|x| js_trim(x).trim_start_matches('.').to_string())
            .filter(|x| !x.is_empty())
            .collect();
        return Some(Box::new(move |e| {
            exts.is_empty()
                || exts
                    .iter()
                    .any(|x| e.lower.strip_suffix(x.as_str()).is_some_and(|rest| rest.ends_with('.')))
        }));
    }
    if key.starts_with("file") {
        return if value.is_empty() {
            Some(Box::new(|e| !e.dir))
        } else {
            None
        };
    }
    if key.starts_with("folder") {
        return if value.is_empty() {
            Some(Box::new(|e| e.dir))
        } else {
            None
        };
    }
    let nothing = || -> Option<EntryTest<'a>> { Some(Box::new(|_| false)) };
    let Some((op, amount)) = split_op(value) else {
        return nothing();
    };
    if key == "size" {
        let cut = amount
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(amount.len());
        let (number, unit) = amount.split_at(cut);
        if number.is_empty() || !unit.chars().all(|c| c.is_ascii_lowercase()) {
            return nothing();
        }
        let (Ok(n), Some(scale)) = (number.parse::<f64>(), size_unit(unit)) else {
            return nothing();
        };
        let bytes = (n * scale).floor();
        if bytes.is_nan() {
            return nothing();
        }
        let op = op.to_string();
        // Only files have a size of their own.
        return Some(Box::new(move |e| {
            let s = e.size as f64;
            !e.dir
                && match op.as_str() {
                    ">" => s > bytes,
                    ">=" => s >= bytes,
                    "<" => s < bytes,
                    "<=" => s <= bytes,
                    _ => s == bytes,
                }
        }));
    }
    // dm:, the date modified, by age.
    let cut = amount.find(|c: char| !c.is_ascii_digit()).unwrap_or(amount.len());
    let (count, unit) = amount.split_at(cut);
    let scale = if count.is_empty() || unit.is_empty() || !unit.chars().all(|c| c.is_ascii_lowercase()) {
        None
    } else {
        age_unit(unit)
    };
    let (Some(scale), Ok(count)) = (scale, count.parse::<f64>()) else {
        return nothing();
    };
    let ms = count * scale * 1000.0;
    let now = now as f64;
    Some(if op.starts_with('>') {
        Box::new(move |e| now - (e.modified as f64) > ms)
    } else {
        Box::new(move |e| now - (e.modified as f64) < ms)
    })
}

/// A path term (`src\lib`, `path:docs`) cleaned up: `\\?\` and `path:` dropped, `/` made `\`, runs of `\`
/// squeezed (a leading pair, for network paths, kept), a trailing `\` cut off (except a drive's).
fn path_pattern(term: &str) -> String {
    let body = term.strip_prefix("path:").unwrap_or(term);
    let body = body.strip_prefix("\\\\?\\").unwrap_or(body);
    let chars: Vec<char> = body.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
    let mut raw = String::with_capacity(body.len());
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if c != '\\' {
            raw.push(c);
            i += 1;
            continue;
        }
        let run = chars
            .get(i..)
            .map_or(0, |rest| rest.iter().take_while(|&&c| c == '\\').count());
        // A run squeezes to one, but a run's first backslash can't be squeezed into the start of the text
        // (or a line break): the TypeScript required a character before it.
        let protected = i == 0 || chars.get(i - 1).is_some_and(|&p| is_line_end(p));
        raw.push('\\');
        if protected && run > 1 {
            raw.push('\\');
        }
        i += run;
    }
    let drive_root = {
        let mut it = raw.chars();
        let first = (it.next(), it.next(), it.next(), it.next());
        matches!(first, (Some(d), Some(':'), Some('\\'), None) if d.is_ascii_lowercase())
    };
    if raw.encode_utf16().count() > 1 && !drive_root {
        raw.trim_end_matches('\\').to_string()
    } else {
        raw
    }
}

/// The same matching fd does for a search, applied to what's on screen: every term must match the name,
/// case-insensitively; terms with * or ? match the whole name, others any part of it. Filters (ext:pdf,
/// size:>10mb, dm:7d, file:, folder:) narrow it as the index does; git:changed by `git`, the folder's Git marks
/// by lowercase name; width:, taken: and the like by `facts`, what files say. `None` when the query filters
/// nothing (empty, or asking the models).
pub fn matcher<'a, 'f: 'a>(
    query: &str,
    git: Option<&'a GitMarks>,
    facts: Option<&'a FactsOf<'f>>,
) -> Option<EntryTest<'a>> {
    matcher_at(query, git, facts, now_ms())
}

/// `matcher` at a given time (ms since the epoch), for the age filters (`dm:7d`).
pub fn matcher_at<'a, 'f: 'a>(
    query: &str,
    git: Option<&'a GitMarks>,
    facts: Option<&'a FactsOf<'f>>,
    now: i64,
) -> Option<EntryTest<'a>> {
    let parts = terms(&query.to_lowercase());
    if parts.is_empty() {
        return None;
    }
    let tests: Vec<EntryTest<'a>> = parts
        .into_iter()
        .map(|term| -> EntryTest<'a> {
            if let Some(filter) = filter_test(&term, git, facts, now) {
                return filter;
            }
            if term.contains(['\\', '/']) || term.starts_with("path:") {
                let clean = path_pattern(&term);
                if clean.is_empty() {
                    return Box::new(|_| true);
                }
                let slashed = |e: &FileEntry| e.path.to_lowercase().replace('/', "\\");
                if !clean.contains(['*', '?']) {
                    return Box::new(move |e| slashed(e).contains(&clean));
                }
                let front = if clean.starts_with('*') { "" } else { "*" };
                let back = if clean.ends_with('*') { "" } else { "*" };
                return match glob_to_regex(&format!("{front}{clean}{back}")) {
                    Some(re) => Box::new(move |e| re.is_match(&slashed(e))),
                    None => Box::new(|_| false),
                };
            }
            if !term.contains(['*', '?']) {
                return Box::new(move |e| e.lower.contains(&term));
            }
            match glob_to_regex(&term) {
                Some(re) => Box::new(move |e| re.is_match(&e.lower)),
                None => Box::new(|_| false),
            }
        })
        .collect();
    Some(Box::new(move |e| tests.iter().all(|test| test(e))))
}

/// How an item in one pane differs from the matching name in the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffStatus {
    Unique,
    Newer,
    Older,
    Differ,
    Same,
}

/// Compares an item in one pane with the matching name in the other pane (`other`). Allows a 2-second timestamp
/// tolerance for FAT/archive rounding when sizes match.
pub fn compare_entries(
    own: &FileEntry,
    other: Option<&FileEntry>,
    own_dir_size: Option<u64>,
    other_dir_size: Option<u64>,
) -> DiffStatus {
    let Some(other) = other.filter(|o| o.dir == own.dir) else {
        return DiffStatus::Unique;
    };
    if own.dir {
        return match (own_dir_size, other_dir_size) {
            (Some(a), Some(b)) if a != b => DiffStatus::Differ,
            _ => DiffStatus::Same,
        };
    }
    if own.modified > 0 && other.modified > 0 && own.modified.abs_diff(other.modified) > 2000 {
        return if own.modified > other.modified {
            DiffStatus::Newer
        } else {
            DiffStatus::Older
        };
    }
    if own.size != other.size {
        return DiffStatus::Differ;
    }
    DiffStatus::Same
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- helpers ----

    fn entry(name: &str) -> FileEntry {
        let (name, dir) = name.strip_suffix('/').map_or((name, false), |n| (n, true));
        with_details(
            RawEntry {
                name: name.into(),
                dir,
                ..RawEntry::default()
            },
            "C:\\x",
        )
    }

    fn entry_with(name: &str, size: u64, modified: i64) -> FileEntry {
        let mut e = entry(name);
        e.size = size;
        e.modified = modified;
        e
    }

    fn names(list: &[FileEntry]) -> String {
        list.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join(",")
    }

    const BY_NAME: Sort = Sort {
        key: SortKey::Name,
        desc: false,
    };

    fn sorted(list: &[FileEntry], sort: Sort, folders_first: bool, presorted: bool) -> Vec<FileEntry> {
        sort_entries(list.to_vec(), sort, folders_first, presorted, None, None, None)
    }

    /// The core's `natural_cmp` and `natural_key` (src-tauri/src/fsx.rs), copied to check the port against them.
    fn core_natural_cmp(a: &str, b: &str) -> Ordering {
        let (a, b) = (a.as_bytes(), b.as_bytes());
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
                let (si, sj) = (i, j);
                while i < a.len() && a[i].is_ascii_digit() {
                    i += 1;
                }
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                let (na, nb) = (trim_zeros(&a[si..i]), trim_zeros(&b[sj..j]));
                let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
                if ord != Ordering::Equal {
                    return ord;
                }
            } else {
                if a[i] != b[j] {
                    return a[i].cmp(&b[j]);
                }
                i += 1;
                j += 1;
            }
        }
        (a.len() - i).cmp(&(b.len() - j))
    }

    fn core_natural_key(lower: &str) -> Vec<u8> {
        let bytes = lower.as_bytes();
        let mut key = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let run = bytes[i..]
                .iter()
                .position(u8::is_ascii_digit)
                .map_or(bytes.len(), |p| i + p);
            key.extend(bytes[i..run].iter().map(u8::to_ascii_lowercase));
            i = run;
            if i < bytes.len() {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let digits = trim_zeros(&bytes[start..i]);
                key.push(b'0');
                key.push(digits.len().min(255) as u8);
                key.extend_from_slice(digits);
            }
        }
        key
    }

    // ---- natural order and sorting ----

    #[test]
    fn natural_order_reads_digit_runs_as_numbers() {
        assert_eq!(natural_compare("img9", "img10"), Ordering::Less);
        assert_eq!(natural_compare("v1.10", "v1.9"), Ordering::Greater);
        assert_eq!(natural_compare("file02", "file2"), Ordering::Equal);
    }

    #[test]
    fn natural_order_agrees_with_the_core_on_tricky_names() {
        let names = [
            "",
            "0",
            "00",
            "000",
            "1",
            "01",
            "001",
            "2",
            "10",
            "a",
            "a0",
            "a00",
            "a1",
            "a01",
            "a2",
            "a10",
            "a1b",
            "a01b",
            "a1c",
            "a10b",
            "a9999999999999999999999",
            "a10000000000000000000000",
            "file2",
            "file02",
            "file10",
            "file2x",
            "file2.txt",
            "file10.txt",
            "file 2",
            "file-2",
            "file_2",
            "v1.9",
            "v1.10",
            "v1.010",
            "é",
            "é1",
            "e1",
            "z",
            "ä",
            "日本1",
            "日本2",
            "日本10",
            "日本",
            "😀",
            "😀1",
            "😀10",
            "\u{ffff}",
            "\u{e000}1",
            "1a",
            "1.5",
            "1.05",
            "0x10",
            "00x9",
            "a/b",
            "a\\b",
            "a b",
            "a  b",
            "عربي1",
            "عربي10",
            "عربي2",
        ];
        for a in names {
            for b in names {
                let want = core_natural_cmp(a, b);
                assert_eq!(natural_compare(a, b), want, "{a:?} against {b:?}");
                assert_eq!(
                    core_natural_key(a).cmp(&core_natural_key(b)),
                    want,
                    "key of {a:?} against {b:?}"
                );
            }
        }
    }

    #[test]
    fn natural_order_puts_astral_characters_by_code_point() {
        // The core compares UTF-8 bytes: U+1F600 comes after U+FFFF, where UTF-16 units put it before.
        assert_eq!(natural_compare("😀", "\u{ffff}"), Ordering::Greater);
        assert_eq!(natural_compare("a😀2", "a😀10"), Ordering::Less);
    }

    #[test]
    fn details_fill_path_lower_and_ext() {
        let listing = [entry("b/"), entry("a.txt"), entry("c.md"), entry("B.txt")];
        let paths: Vec<&str> = listing.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["C:\\x\\b", "C:\\x\\a.txt", "C:\\x\\c.md", "C:\\x\\B.txt"]);
        assert_eq!(listing[1].ext, "txt");
        assert_eq!(listing[0].ext, "");
        assert_eq!(listing[3].lower, "b.txt");
        assert_eq!(entry(".gitignore").ext, "");
        assert_eq!(entry("archive.TAR.GZ").ext, "gz");
        assert_eq!(entry("sub.d/").ext, "", "folders have no extension");
    }

    fn the_listing() -> Vec<FileEntry> {
        vec![
            entry("b/"),
            entry_with("a.txt", 30, 3),
            entry_with("c.md", 10, 1),
            entry_with("B.txt", 20, 2),
        ]
    }

    #[test]
    fn sorting_by_every_key() {
        let presorted = sorted(&the_listing(), BY_NAME, true, false);
        assert_eq!(names(&presorted), "b,a.txt,B.txt,c.md");
        let again = presorted.clone();
        let pointer = presorted.as_ptr();
        let kept = sort_entries(presorted, BY_NAME, true, true, None, None, None);
        assert_eq!(kept.as_ptr(), pointer, "default view must not copy");
        assert_eq!(kept, again);
        let sort = |key, desc, first| names(&sorted(&again, Sort { key, desc }, first, true));
        assert_eq!(sort(SortKey::Name, true, true), "b,c.md,B.txt,a.txt");
        assert_eq!(sort(SortKey::Size, true, true), "b,a.txt,B.txt,c.md");
        assert_eq!(sort(SortKey::Modified, false, false), "b,c.md,B.txt,a.txt");
        assert_eq!(sort(SortKey::Type, false, true), "b,c.md,a.txt,B.txt");
        let folder_size = |e: &FileEntry| if e.dir { 100 } else { e.size };
        let by_size = sort_entries(
            again,
            Sort {
                key: SortKey::Size,
                desc: true,
            },
            true,
            true,
            Some(&folder_size),
            None,
            None,
        );
        assert_eq!(names(&by_size), "b,a.txt,B.txt,c.md");
    }

    #[test]
    fn date_taken_is_when_a_photo_says_else_the_date_modified() {
        let shots = [
            entry_with("a.jpg", 0, 50),
            entry_with("b.jpg", 0, 10),
            entry_with("c.txt", 0, 30),
        ];
        let taken = Sort {
            key: SortKey::Taken,
            desc: false,
        };
        let five = |e: &FileEntry| if e.name == "a.jpg" { Some(5) } else { None };
        let by = sort_entries(shots.to_vec(), taken, true, false, None, Some(&five), None);
        assert_eq!(names(&by), "a.jpg,b.jpg,c.txt");
        let desc = Sort {
            key: SortKey::Taken,
            desc: true,
        };
        let by = sort_entries(shots.to_vec(), desc, true, false, None, Some(&five), None);
        assert_eq!(names(&by), "c.txt,b.jpg,a.jpg");
    }

    #[test]
    fn a_list_that_grows_sorts_what_is_new_into_what_is_sorted() {
        let shots = [
            entry_with("a.jpg", 0, 50),
            entry_with("b.jpg", 0, 10),
            entry_with("c.txt", 0, 30),
        ];
        for sort in [
            Sort {
                key: SortKey::Name,
                desc: false,
            },
            Sort {
                key: SortKey::Size,
                desc: true,
            },
            Sort {
                key: SortKey::Modified,
                desc: false,
            },
        ] {
            let mut all = the_listing();
            all.extend(shots.iter().cloned());
            all.push(entry_with("a.txt", 30, 3));
            all.push(entry("d/"));
            for cut in 0..=all.len() {
                let (first, rest) = all.split_at(cut);
                let order = entry_order(sort, true, false, None, None, None);
                let grown = merge_sorted(sorted(first, sort, true, false), rest.to_vec(), order);
                assert_eq!(grown, sorted(&all, sort, true, false), "{:?} cut at {cut}", sort.key);
            }
        }
    }

    #[test]
    fn sorting_by_what_songs_and_photos_say() {
        let named = |names: &[&str]| -> Vec<FileEntry> {
            names
                .iter()
                .map(|n| {
                    with_details(
                        RawEntry {
                            name: (*n).into(),
                            size: 1,
                            ..RawEntry::default()
                        },
                        "D:/music",
                    )
                })
                .collect()
        };
        let facts = |width, height, duration, artist: &str| MediaFacts {
            width: Some(width),
            height: Some(height),
            duration: Some(duration),
            artist: Some(artist.into()),
            ..MediaFacts::default()
        };
        let songs = named(&["a.mp3", "b.mp3", "c.mp3"]);
        let said: HashMap<String, MediaFacts> = [
            (songs[0].path.clone(), facts(0, 0, 200.0, "Zed")),
            (songs[1].path.clone(), facts(0, 0, 90.0, "abba")),
            (songs[2].path.clone(), facts(0, 0, 300.0, "Muse")),
        ]
        .into_iter()
        .collect();
        let of = |e: &FileEntry| said.get(&e.path);
        let by = |key, desc| {
            let sorted = sort_entries(songs.clone(), Sort { key, desc }, true, false, None, None, Some(&of));
            sorted.iter().map(|e| e.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(by(SortKey::Duration, false), ["b.mp3", "a.mp3", "c.mp3"]);
        assert_eq!(
            by(SortKey::Artist, false),
            ["b.mp3", "c.mp3", "a.mp3"],
            "artists in any case"
        );
        assert_eq!(by(SortKey::Artist, true), ["a.mp3", "c.mp3", "b.mp3"]);
    }

    #[test]
    fn sorting_by_dimensions_and_missing_facts() {
        let pictures = named_files(&["big.png", "small.png", "unread.png"]);
        let said: HashMap<String, MediaFacts> = [
            (
                "big.png".to_string(),
                MediaFacts {
                    width: Some(4000),
                    height: Some(3000),
                    ..MediaFacts::default()
                },
            ),
            (
                "small.png".to_string(),
                MediaFacts {
                    width: Some(800),
                    height: Some(600),
                    ..MediaFacts::default()
                },
            ),
        ]
        .into_iter()
        .collect();
        let of = |e: &FileEntry| said.get(&e.name);
        let by = |key, desc| {
            let sort = Sort { key, desc };
            names(&sort_entries(
                pictures.clone(),
                sort,
                true,
                false,
                None,
                None,
                Some(&of),
            ))
        };
        assert_eq!(by(SortKey::Dimensions, false), "unread.png,small.png,big.png");
        assert_eq!(by(SortKey::Dimensions, true), "big.png,small.png,unread.png");
        // Duration: nothing read sorts before everything (-1), and ties fall back to the name.
        assert_eq!(by(SortKey::Duration, false), "big.png,small.png,unread.png");
    }

    fn named_files(list: &[&str]) -> Vec<FileEntry> {
        list.iter().map(|n| entry_with(n, 1, 0)).collect()
    }

    // ---- dual-pane comparison ----

    #[test]
    fn dual_pane_comparison() {
        let own = |size, modified| entry_with("a.txt", size, modified);
        assert_eq!(compare_entries(&own(10, 5000), None, None, None), DiffStatus::Unique);
        assert_eq!(
            compare_entries(&own(10, 8000), Some(&own(10, 5000)), None, None),
            DiffStatus::Newer
        );
        assert_eq!(
            compare_entries(&own(10, 5000), Some(&own(10, 8000)), None, None),
            DiffStatus::Older
        );
        assert_eq!(
            compare_entries(&own(20, 5000), Some(&own(10, 5000)), None, None),
            DiffStatus::Differ
        );
        assert_eq!(
            compare_entries(&own(10, 5000), Some(&own(10, 5500)), None, None),
            DiffStatus::Same
        );
        assert_eq!(
            compare_entries(&entry("sub/"), Some(&entry("sub/")), Some(100), Some(200)),
            DiffStatus::Differ
        );
        assert_eq!(
            compare_entries(&entry("sub/"), Some(&entry("sub/")), Some(100), Some(100)),
            DiffStatus::Same
        );
        assert_eq!(
            compare_entries(&entry("sub/"), Some(&entry("sub/")), None, Some(1)),
            DiffStatus::Same
        );
        assert_eq!(
            compare_entries(&entry("sub/"), Some(&own(1, 1)), None, None),
            DiffStatus::Unique
        );
    }

    // ---- matching ----

    fn matches_in(list: &[FileEntry], query: &str) -> String {
        let hits: Vec<FileEntry> = match matcher(query, None, None) {
            Some(test) => list.iter().filter(|e| test(e)).cloned().collect(),
            None => list.to_vec(),
        };
        names(&hits)
    }

    fn the_sorted_listing() -> Vec<FileEntry> {
        the_listing()
    }

    #[test]
    fn terms_split_on_spaces_but_not_inside_quotes() {
        assert_eq!(terms("report \"q3 final\"  pdf"), ["report", "q3 final", "pdf"]);
        assert_eq!(terms(""), Vec::<String>::new());
        assert_eq!(terms("  \u{a0}\u{3000} "), Vec::<String>::new());
        assert_eq!(terms("\"unclosed quote"), ["unclosed quote"]);
        assert_eq!(terms("a\"b c\"d"), ["ab cd"]);
    }

    #[test]
    fn matching_mirrors_fd() {
        let listing = the_sorted_listing();
        let find = |q: &str| matches_in(&listing, q);
        assert_eq!(find("TXT"), "a.txt,B.txt");
        assert_eq!(find("*.md"), "c.md");
        assert_eq!(find("b txt"), "B.txt");
        assert_eq!(find("b"), "b,B.txt");
        assert_eq!(find("?.txt"), "a.txt,B.txt");
        assert_eq!(find("*.t"), "");
        assert!(matcher("   ", None, None).is_none());
        assert!(matcher("", None, None).is_none());
    }

    #[test]
    fn path_terms_match_the_whole_path() {
        let listing = the_sorted_listing();
        let find = |q: &str| matches_in(&listing, q);
        assert_eq!(find("x/b"), "b,B.txt");
        assert_eq!(find("path:X\\*.MD"), "c.md");
        assert_eq!(find("c:/x/ txt"), "a.txt,B.txt");
        assert_eq!(find("y\\b"), "");
        // Squeezed and trimmed separators, the extended-length prefix, and path: with nothing after it.
        assert_eq!(find("x\\\\\\b"), "b,B.txt");
        assert_eq!(find("\\\\?\\c:\\x\\a.txt"), "a.txt");
        assert_eq!(find("path:"), "b,a.txt,c.md,B.txt");
        assert_eq!(find("c:\\x\\"), "b,a.txt,c.md,B.txt");
    }

    #[test]
    fn filters_as_the_index_and_fd_read_them() {
        // a.txt is 30 B, B.txt 20 B, c.md 10 B; b is a folder.
        let listing = the_sorted_listing();
        let find = |q: &str| matches_in(&listing, q);
        assert_eq!(find("ext:TXT"), "a.txt,B.txt");
        assert_eq!(find("size:>=20 ext:txt;md"), "a.txt,B.txt");
        assert_eq!(find("size:<20"), "c.md");
        assert_eq!(find("folder:"), "b");
        assert_eq!(find("file: b"), "B.txt");
        assert_eq!(find("dm:>1y"), "b,a.txt,c.md,B.txt");
        assert_eq!(find("dm:7d"), "");
        assert_eq!(find("size:big"), "");
        assert_eq!(find("size:20"), "B.txt");
        assert_eq!(find("size:0.02kb"), "B.txt", "20.48 bytes round down to 20");
        assert_eq!(find("size:1kb"), "");
        assert_eq!(find("ext:"), "b,a.txt,c.md,B.txt", "no extension named: everything");
        assert_eq!(find("ext:\".md, .txt\""), "a.txt,c.md,B.txt");
        assert_eq!(find("files:x"), "", "a name term, and nothing is named that");
    }

    #[test]
    fn age_filters_use_the_given_time() {
        let day = 86_400_000;
        let now = 100 * day;
        let list = [
            entry_with("new.txt", 1, now - day),
            entry_with("old.txt", 1, now - 10 * day),
        ];
        let find = |q: &str| {
            let test = matcher_at(q, None, None, now).unwrap();
            names(&list.iter().filter(|e| test(e)).cloned().collect::<Vec<_>>())
        };
        assert_eq!(find("dm:7d"), "new.txt");
        assert_eq!(find("dm:<7d"), "new.txt");
        assert_eq!(find("dm:>7d"), "old.txt");
        assert_eq!(find("dm:2w"), "new.txt,old.txt");
        assert_eq!(find("dm:7x"), "");
        assert_eq!(find("dm:d"), "");
    }

    #[test]
    fn git_asks_the_folders_marks() {
        let listing = the_sorted_listing();
        let marks: GitMarks = [
            ("a.txt", GitMark::Modified),
            ("b.txt", GitMark::Untracked),
            ("b", GitMark::Staged),
            ("c.md", GitMark::Ignored),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let git = |q: &str| {
            let test = matcher(q, Some(&marks), None).unwrap();
            names(&listing.iter().filter(|e| test(e)).cloned().collect::<Vec<_>>())
        };
        assert_eq!(git("git:changed"), "b,a.txt,B.txt");
        assert_eq!(git("git:new"), "B.txt");
        assert_eq!(git("git:staged"), "b");
        assert_eq!(git("git:ignored"), "c.md");
        assert_eq!(git("git:changed ext:txt"), "a.txt,B.txt");
        assert_eq!(git("git:nonsense"), "");
        // Without marks (not a repository) it matches nothing.
        assert_eq!(matches_in(&listing, "git:changed"), "");
    }

    #[test]
    fn globs_take_a_character_and_escape_the_rest() {
        let list = [entry("a.txt"), entry("a+b(1).txt"), entry("😀.txt"), entry("axtxt")];
        let find = |q: &str| matches_in(&list, q);
        assert_eq!(find("a.txt"), "a.txt");
        assert_eq!(find("a?txt"), "a.txt,axtxt");
        assert_eq!(find("a+b(*"), "a+b(1).txt");
        assert_eq!(find("?.txt"), "a.txt,😀.txt");
        assert_eq!(find("[a]*"), "");
    }

    // ---- what photos, videos and songs say ----

    fn day(y: i32, m: u32, d: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, m, d, 0, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis()
    }

    fn photo() -> MediaFacts {
        MediaFacts {
            taken: Some(day(2024, 6, 15) + 3_600_000),
            width: Some(4032),
            height: Some(3024),
            duration: None,
            camera: Some("canon eos r5".into()),
            artist: None,
            album: None,
        }
    }

    fn song() -> MediaFacts {
        MediaFacts {
            duration: Some(187.4),
            artist: Some("the beatles".into()),
            album: Some("abbey road".into()),
            ..MediaFacts::default()
        }
    }

    fn hold_all(facts: &MediaFacts, query: &str) -> bool {
        let today = day(2026, 10, 23);
        terms(&query.to_lowercase()).iter().all(|t| {
            let Some((key, value)) = t.split_once(':') else {
                return false;
            };
            prop_test(key, value, today).is_some_and(|test| test(facts))
        })
    }

    #[test]
    fn property_filters_read_what_files_say() {
        let (photo, song) = (photo(), song());
        assert!(hold_all(&photo, "width:>4000 height:>=3024 camera:eos"));
        assert!(!hold_all(&photo, "width:>4032"));
        assert!(hold_all(
            &photo,
            "taken:2024 taken:2024-06 taken:2024-06-15 taken:>2023 taken:<2025-01 taken:>1y"
        ));
        assert!(
            !hold_all(&photo, "taken:2024-06-16") && !hold_all(&photo, "taken:>2024") && !hold_all(&photo, "taken:30d")
        );
        assert!(hold_all(
            &song,
            "duration:>3m duration:<=3:07 artist:beatles album:\"abbey road\""
        ));
        assert!(!hold_all(&song, "duration:>3:07") && !hold_all(&song, "width:>1") && !hold_all(&photo, "artist:"));
        assert!(hold_all(&song, "album:"), "a word left out: any album");
    }

    #[test]
    fn property_filters_that_do_not_read_right_are_none() {
        for bad in [
            "width:wide",
            "width:<0",
            "height:>",
            "duration:5x",
            "duration:1:300",
            "taken:2023-02-29",
            "taken:2024-13",
            "taken:24",
            "taken:yesterday",
        ] {
            let (key, value) = bad.split_once(':').unwrap();
            assert!(prop_test(key, value, 0).is_none(), "{bad}");
        }
        assert!(prop_test_now("colour", "red").is_none());
    }

    #[test]
    fn property_filter_edges() {
        let song = song();
        assert!(hold_all(&song, "duration:187"));
        assert!(hold_all(&song, "duration:3:07"));
        assert!(hold_all(&song, "duration:<=187s"));
        assert!(hold_all(&song, "duration:>=2min duration:<1hr"));
        let photo = photo();
        assert!(hold_all(&photo, "width:4032") && hold_all(&photo, "width:=4032") && !hold_all(&photo, "width:<4032"));
        assert!(hold_all(&photo, "width:<=4032 width:>=4032 width:<4033"));
        // A leap day exists in 2024, and an unknown value is not read.
        assert!(prop_test("taken", "2024-02-29", 0).is_some());
        assert!(prop_test("taken", "2024-00", 0).is_none());
        assert!(prop_test("taken", "2024-06-00", 0).is_none());
        assert!(prop_test("taken", "2024-06-31", 0).is_none());
        // Years 0000 to 0099 mean 1900 to 1999, as in JavaScript's Date.
        let old = prop_test("taken", "0024", 0).unwrap();
        assert!(old(&MediaFacts {
            taken: Some(day(1924, 7, 1)),
            ..MediaFacts::default()
        }));
        // A file that says nothing of a field never passes its filter.
        assert!(!hold_all(&MediaFacts::default(), "width:>0") && !hold_all(&MediaFacts::default(), "taken:2024"));
    }

    #[test]
    fn has_prop_filter_and_has_filter_or_path() {
        assert!(has_prop_filter("holiday Width:>10") && !has_prop_filter("holiday ext:jpg"));
        // What an older instant search service doesn't read: filters and paths, not names.
        for query in [
            "report EXT:pdf",
            "size:>1mb",
            "dm:7d",
            "files:",
            "folder:",
            "src\\lib main",
            "src/lib",
            "PATH:docs",
            "D:\\photos",
        ] {
            assert!(has_filter_or_path(query), "{query}");
        }
        for query in ["budget 2026", "*.pdf -draft", "\"q3 final\"", "taken:2024"] {
            assert!(!has_filter_or_path(query), "{query}");
        }
    }

    fn facts_of_map<'a>(
        said: &'a HashMap<String, Option<MediaFacts>>,
    ) -> impl Fn(&FileEntry) -> Option<&'a MediaFacts> + 'a {
        move |e| said.get(&e.name).and_then(|f| f.as_ref())
    }

    #[test]
    fn in_a_folder_a_file_shows_once_what_it_says_is_read() {
        let pictures = named_files(&["big.png", "small.png", "unread.png", "notes.txt"]);
        let mut said: HashMap<String, Option<MediaFacts>> = HashMap::new();
        said.insert(
            "big.png".into(),
            Some(MediaFacts {
                width: Some(3000),
                ..photo()
            }),
        );
        said.insert(
            "small.png".into(),
            Some(MediaFacts {
                width: Some(800),
                ..photo()
            }),
        );
        said.insert("notes.txt".into(), None);
        let of = facts_of_map(&said);
        let test = matcher("width:>1920", None, Some(&of)).unwrap();
        let hits: Vec<FileEntry> = pictures.iter().filter(|e| test(e)).cloned().collect();
        assert_eq!(names(&hits), "big.png");
        // In search results (no facts) the search checked them: left to the search.
        let test = matcher("width:>1920 big", None, None).unwrap();
        let hits: Vec<FileEntry> = pictures.iter().filter(|e| test(e)).cloned().collect();
        assert_eq!(names(&hits), "big.png", "without facts, left to the search");
        // A folder has nothing to read.
        let folder = entry("wide/");
        assert!(!matcher("width:>1", None, Some(&of)).unwrap()(&folder));
    }

    // ---- attributes ----

    #[test]
    fn attribute_predicates() {
        let with = |name: &str, attrs: u32| {
            let mut e = entry(name);
            e.attrs = attrs;
            e
        };
        assert!(!is_closed(&entry("plain.txt")) && is_closed(&with("a", attr::CLOSED | 0x20)));
        assert!(!is_recycled(&entry("plain.txt")) && is_recycled(&with("a", attr::RECYCLED)));
        assert!(!is_archived(&entry("plain.txt")) && is_archived(&with("a", attr::IN_ARCHIVE)));
        assert!(is_hidden(&with("a", attr::HIDDEN)) && !is_hidden(&entry("a")));
        assert!(is_locked(&with("a", attr::IN_ARCHIVE | attr::ENCRYPTED)) && is_locked(&with("a", attr::UNREADABLE)));
        assert!(!is_remote(&entry("a")) && is_remote(&with("a", attr::REMOTE)));
    }

    #[test]
    fn archives_and_files_that_are_archives_by_another_name() {
        let with = |name: &str, attrs: u32| {
            let mut e = entry(name);
            e.attrs = attrs;
            e
        };
        assert!(is_archive_file(&entry("x.Zip")) && is_archive_file(&entry("y.7z")));
        assert!(!is_archive_file(&entry("z.zip/")) && !is_archive_file(&entry("w.docx")));
        assert!(is_archive_file(&entry("backup.RAR")) && is_archive_file(&entry("drivers.cab")));
        assert!(is_archive_file(&entry("disk.ncdu")));
        assert!(is_browsable(&entry("setup.msi")) && !is_archive_file(&entry("setup.msi")));
        assert!(is_browsable(&entry("report.DOCX")) && is_browsable(&entry("lib.jar")));
        assert!(!is_browsable(&entry("x.zip")) && !is_browsable(&entry("dir.docx/")));
        // A file in an ncdu export has a name, a size and a date, and nothing to open; the bit means that
        // only inside an archive.
        let list_only = with("a.jpg", attr::IN_ARCHIVE | attr::LIST_ONLY);
        assert!(is_list_only(&list_only));
        assert!(!is_list_only(&with("b.jpg", attr::LIST_ONLY)));
        assert!(!is_locked(&list_only));
    }

    #[test]
    fn a_remote_file_is_read_only_and_not_looked_into() {
        let remote = with_details(
            RawEntry {
                name: "beach.zip".into(),
                size: 3,
                attrs: attr::IN_ARCHIVE | attr::REMOTE,
                ..RawEntry::default()
            },
            "remote:photos",
        );
        assert_eq!(remote.path, "remote:photos\\beach.zip");
        assert!(is_remote(&remote) && is_archived(&remote));
        // An archive there is a file like any: Glint doesn't look inside archives on a remote.
        assert!(!is_archive_file(&remote));
        let docx = FileEntry {
            name: "a.docx".into(),
            ext: "docx".into(),
            ..remote
        };
        assert!(!is_browsable(&docx));
    }

    #[test]
    fn cloud_states() {
        let with = |attrs: u32| FileEntry {
            attrs,
            ..entry("a.txt")
        };
        assert_eq!(cloud_state(&with(0)), None);
        assert_eq!(cloud_state(&with(attr::CLOUD)), Some(CloudState::Local));
        assert_eq!(cloud_state(&with(attr::UNPINNED)), Some(CloudState::Local));
        assert_eq!(cloud_state(&with(attr::PINNED)), Some(CloudState::Kept));
        assert_eq!(cloud_state(&with(attr::OFFLINE)), Some(CloudState::Online));
        assert_eq!(
            cloud_state(&with(attr::RECALL_ON_DATA_ACCESS)),
            Some(CloudState::Online)
        );
        assert_eq!(
            cloud_state(&with(attr::OFFLINE | attr::PINNED)),
            Some(CloudState::Pending)
        );
        assert_eq!(
            cloud_state(&with(attr::CLOUD | attr::CLOUD_PENDING)),
            Some(CloudState::Pending)
        );
        assert!(is_cloud_file(&with(attr::PINNED)) && !is_cloud_file(&with(attr::HIDDEN)));
        assert!(is_online_only(&with(attr::RECALL_ON_OPEN)));
    }
}
