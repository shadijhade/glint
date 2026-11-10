//! Renaming several items at once. Ported from src/components/explorer/BatchRenameDialog.tsx, with
//! glint-core's `batch_rename.rs` doing the renaming.
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                              | Here                                                              |
//! |-----------------------------------------|-------------------------------------------------------------------|
//! | `useBatchRenameStore` (items, names)    | [`BatchRename`], a model entity made by [`open`]                  |
//! | the dialog's `useState` rules           | [`Rules`], held by the model                                      |
//! | `simulation` (`useMemo`)                | [`simulate`], pure; the model keeps its latest answer             |
//! | `splitStemExt`, `transformCase`, ...    | [`split_stem_ext`], [`transform_case`], [`validate_target_name`]  |
//! | `mediaDetailsMany`                      | [`Source::details`], asked once when the pattern wants a detail   |
//! | `batchRename`                           | [`Source::rename`] (core's two-phase, all-or-nothing renamer)     |
//! | `BatchRenameDialog`                     | [`BatchRenameDialog`], opened with [`open`]                       |
//! | the "Renamed..." toast and its Undo     | [`renamed`] (a notification with an Undo button; Ctrl+Z too)     |
//! | `lastUndo` and "Undo last batch"        | [`LastBatch`], a global the dialog's header button reads          |
//!
//! **The preview is computed here, not by core.** The TypeScript worked out every new name in the page and
//! asked core only to apply the pairs; core's `batch_rename` re-checks them (a name Windows refuses, two items
//! landing on one name, something already there) and renames all of them or none. So [`simulate`] is a port of
//! the page's `simulation`, and the pairs it produces are what [`Source::rename`] gets.
//!
//! **Differences from the TypeScript, on purpose:**
//! - Find and replace uses the regex crate (through `glint_model::text`), not JS's engine: the same syntax
//!   for ordinary patterns and `$1`, `$&`, `$<name>` in the replacement, but lookaround and back-references
//!   are refused (the error is shown, nothing is renamed), and `\d`, `\w`, `\b` are Unicode-aware.
//! - Start # and Step are whole numbers (`Number("2.5")` was 2.5 there; here it is 2).
//! - Ctrl+Z takes a batch back too, as the TypeScript's `rememberUndo` did: [`renamed`] keeps an
//!   `UndoAction::RenameBatch` (the pairs of `undo_ops`) as the step "rename of 3 items", the notification's
//!   Undo button and the dialog's "Undo last batch" run that same step, and the explorer runs it through core's
//!   all-or-nothing renamer, says how it went and reads the folders again.

use crate::explorer::{try_explorer, BatchPair, UndoAction, UndoId};
use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::batch_rename::{BatchRenameResult, RenamePair};
use glint_model::entries::FileEntry;
use glint_model::format::{format_count, plural};
use glint_model::js::{is_js_space, js_trim, to_fixed};
use glint_model::paths::{base_name, join_path, parent_path};
use glint_model::store::SearchOptions;
use glint_model::text::{escape_regex, replace_text};
use gpui_kit::{Task, UniformListScrollHandle};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

// ---- The rules ------------------------------------------------------------------------------------------

/// What the case option does to the new name (`CaseMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CaseMode {
    /// "Same case".
    #[default]
    None,
    Lower,
    Upper,
    Title,
    Snake,
    Kebab,
}

impl CaseMode {
    /// Every mode, in the order the TypeScript's list had them.
    pub const ALL: [CaseMode; 6] = [
        CaseMode::None,
        CaseMode::Lower,
        CaseMode::Upper,
        CaseMode::Title,
        CaseMode::Snake,
        CaseMode::Kebab,
    ];

    /// The option's label as the TypeScript's `<select>` showed it.
    fn label(self) -> SharedString {
        match self {
            CaseMode::None => t!("Same case"),
            CaseMode::Lower => t!("lowercase"),
            CaseMode::Upper => t!("UPPERCASE"),
            CaseMode::Title => t!("Title Case"),
            CaseMode::Snake => t!("snake_case"),
            CaseMode::Kebab => t!("kebab-case"),
        }
    }
}

/// Everything the person has set in the dialog (the TypeScript's `useState`s).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Step 1: text (or a pattern) to find in each name's stem, and what replaces it.
    pub find: String,
    pub replace: String,
    pub use_regex: bool,
    pub case_sensitive: bool,
    /// Step 2: the new name, with `{name}`, `{n}`... in it.
    pub pattern: String,
    pub case_mode: CaseMode,
    /// What `{n}` starts at, and how much it grows by each item.
    pub start: i64,
    pub step: i64,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            find: String::new(),
            replace: String::new(),
            use_regex: false,
            case_sensitive: false,
            pattern: "{name}".to_string(),
            case_mode: CaseMode::None,
            start: 1,
            step: 1,
        }
    }
}

/// An item to rename: what the preview needs of a file entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    pub name: String,
    pub dir: bool,
    /// Unix milliseconds; 0 when unknown.
    pub modified: i64,
}

impl Item {
    /// An item from a listing's entry.
    pub fn from_entry(entry: &FileEntry) -> Self {
        Self {
            path: entry.path.clone(),
            name: entry.name.clone(),
            dir: entry.dir,
            modified: entry.modified,
        }
    }

    /// An item known only by its path (not in the folder on screen): a file, with no date.
    pub fn from_path(path: &str) -> Self {
        Self {
            path: path.to_string(),
            name: base_name(path).to_string(),
            dir: false,
            modified: 0,
        }
    }
}

/// A photo's or song's details, as the pattern's `{taken}`, `{camera}`... read them (glint-core's
/// `MediaDetails`, less what a name doesn't use).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    /// When it was taken, Unix milliseconds.
    pub taken: Option<f64>,
    pub camera: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    pub track: Option<u32>,
}

impl From<glint_core::media::MediaDetails> for Media {
    fn from(details: glint_core::media::MediaDetails) -> Self {
        Self {
            taken: details.taken,
            camera: details.camera,
            artist: details.artist,
            album: details.album,
            title: details.title,
            track: details.track,
        }
    }
}

/// The tokens a pattern can hold, in the order the dialog offers them (`TOKENS`).
pub const TOKENS: [&str; 13] = [
    "{name}", "{ext}", "{n}", "{nn}", "{nnn}", "{date}", "{parent}", "{taken}", "{camera}", "{artist}", "{album}",
    "{title}", "{track}",
];

/// The tokens that are read from inside the files (EXIF, songs' tags): asking for them reads the files once.
const DETAIL_TOKENS: [&str; 6] = ["{taken}", "{camera}", "{artist}", "{album}", "{title}", "{track}"];

/// Whether a pattern asks for something read from inside the files (`DETAILS.test(pattern)`).
pub fn wants_details(pattern: &str) -> bool {
    DETAIL_TOKENS.iter().any(|token| pattern.contains(token))
}

/// What a token stands for, as its tooltip says.
fn token_help(token: &str) -> SharedString {
    match token {
        "{name}" => t!("Original stem"),
        "{ext}" => t!("Extension"),
        "{n}" => t!("1, 2, 3…"),
        "{nn}" => t!("01, 02, 03…"),
        "{nnn}" => t!("001, 002…"),
        "{date}" => t!("YYYY-MM-DD"),
        "{parent}" => t!("Folder name"),
        "{taken}" => t!("Date a photo or video was taken (YYYY-MM-DD), else its date"),
        "{camera}" => t!("The camera a photo was taken with"),
        "{artist}" => t!("A song's artist"),
        "{album}" => t!("A song's album"),
        "{title}" => t!("A song's title"),
        "{track}" => t!("A song's track number: 01, 02…"),
        _ => SharedString::default(),
    }
}

// ---- Working out the new names --------------------------------------------------------------------------

/// Why a new name can't be used (`validateTargetName` and the two collisions). The words are the
/// TypeScript's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// `Name cannot be empty`
    Empty,
    /// `Cannot end with space or period`
    EndsWithSpaceOrPeriod,
    /// `Contains invalid Windows characters`
    InvalidCharacters,
    /// `Reserved Windows device name`
    ReservedName,
    /// `Duplicate target name in batch`
    Duplicate,
    /// `Already exists in folder`
    Exists,
}

impl Conflict {
    /// What the badge says.
    pub fn message(self) -> SharedString {
        match self {
            Conflict::Empty => t!("Name cannot be empty"),
            Conflict::EndsWithSpaceOrPeriod => t!("Cannot end with space or period"),
            Conflict::InvalidCharacters => t!("Contains invalid Windows characters"),
            Conflict::ReservedName => t!("Reserved Windows device name"),
            Conflict::Duplicate => t!("Duplicate target name in batch"),
            Conflict::Exists => t!("Already exists in folder"),
        }
    }
}

/// One item's line in the preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Where it is now, and where the rules would put it.
    pub from: String,
    pub to: String,
    pub name: String,
    pub new_name: String,
    pub changed: bool,
    pub conflict: Option<Conflict>,
}

/// The preview of every new name (`simulation`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Simulation {
    pub rows: Vec<Row>,
    /// Why the find pattern isn't a pattern, when it isn't. Nothing is replaced then.
    pub regex_error: Option<String>,
    pub changed_count: usize,
    pub conflict_count: usize,
}

impl Simulation {
    /// The renames to ask core for: the items whose name changes.
    pub fn pairs(&self) -> Vec<RenamePair> {
        self.rows
            .iter()
            .filter(|row| row.changed)
            .map(|row| RenamePair {
                from: row.from.clone(),
                to: row.to.clone(),
            })
            .collect()
    }
}

/// A file's name split into the part the rules work on and its extension (`splitStemExt`): folders have no
/// extension, and a dot at the very start (".gitignore") starts the name rather than an extension.
pub fn split_stem_ext(name: &str, dir: bool) -> (&str, &str) {
    if dir {
        return (name, "");
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot + 1..]),
        _ => (name, ""),
    }
}

/// The words of `text` for the case options: a lower-case letter or digit followed by a capital splits
/// there ("myFile" is "my File"), then spaces, `_`, `-` and `.` separate.
fn words(text: &str) -> Vec<String> {
    let mut spaced = String::with_capacity(text.len() + 4);
    let mut previous: Option<char> = None;
    for c in text.chars() {
        if previous.is_some_and(|p| (p.is_ascii_lowercase() || p.is_ascii_digit()) && c.is_ascii_uppercase()) {
            spaced.push(' ');
        }
        spaced.push(c);
        previous = Some(c);
    }
    spaced
        .split(|c: char| is_js_space(c) || matches!(c, '_' | '-' | '.'))
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// `text` in the chosen case (`transformCase`).
pub fn transform_case(text: &str, mode: CaseMode) -> String {
    if mode == CaseMode::None || text.is_empty() {
        return text.to_string();
    }
    match mode {
        CaseMode::Lower => return text.to_lowercase(),
        CaseMode::Upper => return text.to_uppercase(),
        _ => {}
    }
    let words = words(text);
    if words.is_empty() {
        return text.to_string();
    }
    match mode {
        CaseMode::Title => words
            .iter()
            .map(|word| {
                let mut chars = word.chars();
                let first: String = chars.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                format!("{first}{}", chars.as_str().to_lowercase())
            })
            .collect::<Vec<_>>()
            .join(" "),
        CaseMode::Snake => words.iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join("_"),
        CaseMode::Kebab => words.iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join("-"),
        _ => text.to_string(),
    }
}

/// Days since 1970-01-01 as a calendar date (Howard Hinnant's algorithm): year, month 1-12, day 1-31.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    // `day` is 1..=31 and `month` 1..=12 by construction.
    (year, u32::try_from(month).unwrap_or(1), u32::try_from(day).unwrap_or(1))
}

/// The local calendar date of a moment, from Windows' own time zone rules (what JS's `getFullYear()`,
/// `getMonth()` and `getDate()` read). `None` when Windows can't convert it.
fn local_date(ms: i64) -> Option<(i64, u32, u32)> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // FILETIME counts 100 ns from 1601.
    let ticks = u64::try_from(ms.checked_add(11_644_473_600_000)?)
        .ok()?
        .checked_mul(10_000)?;
    let file_time = FILETIME {
        dwLowDateTime: u32::try_from(ticks & 0xFFFF_FFFF).ok()?,
        dwHighDateTime: u32::try_from(ticks >> 32).ok()?,
    };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: every pointer is to a live local; the calls only read the first and write the second.
    unsafe {
        FileTimeToSystemTime(&file_time, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some((i64::from(local.wYear), u32::from(local.wMonth), u32::from(local.wDay)))
}

/// `YYYY-MM-DD` of a moment in local time (`formatIsoDate`); 0 ("unknown") is 1970-01-01.
pub fn iso_date(ms: i64) -> String {
    if ms == 0 {
        return "1970-01-01".to_string();
    }
    let (year, month, day) = local_date(ms).unwrap_or_else(|| civil_from_days(ms.div_euclid(86_400_000)));
    format!("{year}-{month:02}-{day:02}")
}

/// A float of milliseconds as whole ones, or `None` for what no date can be made of.
fn whole_ms(ms: f64) -> Option<i64> {
    // 9e15 ms is beyond any date a file has; inside it the cast cannot saturate.
    (ms.is_finite() && ms.abs() < 9.0e15).then_some(ms as i64)
}

/// The device names Windows keeps (`RESERVED_DOS`).
const RESERVED_DOS: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Whether `c` can't be in a Windows file name.
fn is_invalid_name_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || u32::from(c) < 0x20
}

/// Why `name` can't be a Windows file name, if it can't (`validateTargetName`).
pub fn validate_target_name(name: &str) -> Option<Conflict> {
    if name.is_empty() || name == "." || name == ".." {
        return Some(Conflict::Empty);
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return Some(Conflict::EndsWithSpaceOrPeriod);
    }
    if name.chars().any(is_invalid_name_char) {
        return Some(Conflict::InvalidCharacters);
    }
    let stem = name.split('.').next().unwrap_or("").to_uppercase();
    RESERVED_DOS.contains(&stem.as_str()).then_some(Conflict::ReservedName)
}

/// A detail as part of a name: the characters Windows refuses in one become `_` (`safe`).
fn safe(text: Option<&str>) -> String {
    let replaced: String = text
        .unwrap_or("")
        .chars()
        .map(|c| if is_invalid_name_char(c) { '_' } else { c })
        .collect();
    js_trim(&replaced).to_string()
}

/// `String(n).padStart(width, "0")`: a sign is padded like any other character ("-5" is "0-5").
fn pad_start(number: i128, width: usize) -> String {
    let text = number.to_string();
    let len = text.chars().count();
    if len >= width {
        text
    } else {
        format!("{}{text}", "0".repeat(width - len))
    }
}

/// The text of a regex failure without the crate's drawing of the pattern: its last "error: ..." line.
fn regex_message(error: &str) -> String {
    error
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("error: "))
        .unwrap_or_else(|| error.trim())
        .to_string()
}

/// Step 1 on every stem: the find text (or pattern) replaced by the replacement.
///
/// Ignoring case is `(?i)` in front of the pattern, because the search code's own smart case (a capital
/// means exact) isn't the TypeScript's "gi". Plain text is escaped and its replacement's `$` doubled, so a
/// `$` in a name (price$&tax.txt) stays a `$`. The stems go through one compile of the pattern, one per
/// line, which is how `replace_text` reads a text without a newline in its query.
fn find_and_replace(stems: &[&str], rules: &Rules) -> Result<Vec<String>, String> {
    if rules.find.is_empty() {
        return Ok(stems.iter().map(|s| s.to_string()).collect());
    }
    let case = if rules.case_sensitive { "" } else { "(?i)" };
    let (query, replacement) = if rules.use_regex {
        (format!("{case}{}", rules.find), rules.replace.clone())
    } else {
        (
            format!("{case}{}", escape_regex(&rules.find)),
            rules.replace.replace('$', "$$"),
        )
    };
    let options = SearchOptions {
        case_sensitive: rules.case_sensitive,
        regex: true,
        ..Default::default()
    };
    let one =
        |text: &str| replace_text(text, &query, &options, &replacement).map_err(|e| regex_message(&e.to_string()));
    // A stem has no newline, so none of the lines run into each other. A pattern with a newline in it
    // would read the lines as one text: those go stem by stem.
    if !query.contains('\n') {
        let joined = stems.join("\n");
        let replaced = one(&joined)?;
        let lines: Vec<String> = replaced.split('\n').map(str::to_string).collect();
        if lines.len() == stems.len() {
            return Ok(lines);
        }
    }
    stems.iter().map(|stem| one(stem)).collect()
}

/// Works out every item's new name, and what is wrong with it (`simulation`).
///
/// `existing` holds the lower-cased names already in the folder that aren't among the items. `media` holds
/// the items' details by path, once read (`None` while they are being read or not wanted).
pub fn simulate(
    items: &[Item],
    existing: &HashSet<String>,
    rules: &Rules,
    media: Option<&HashMap<String, Option<Media>>>,
) -> Simulation {
    let parts: Vec<(&str, &str)> = items.iter().map(|i| split_stem_ext(&i.name, i.dir)).collect();
    let stems: Vec<&str> = parts.iter().map(|(stem, _)| *stem).collect();
    let (replaced, regex_error) = match find_and_replace(&stems, rules) {
        Ok(replaced) => (replaced, None),
        Err(message) => (stems.iter().map(|s| s.to_string()).collect(), Some(message)),
    };

    let pattern_has_ext = rules.pattern.contains("{ext}");
    let mut drafts = Vec::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        let ext = parts.get(idx).map_or("", |(_, ext)| *ext);
        let stem = replaced.get(idx).map_or("", String::as_str);
        let seq = i128::from(rules.start) + i128::try_from(idx).unwrap_or(0) * i128::from(rules.step);
        let folder = parent_path(&item.path);
        let parent = base_name(&folder).to_string();
        let date = iso_date(item.modified);
        let details = media.and_then(|m| m.get(&item.path)).and_then(Option::as_ref);

        // The tokens one after the other, as the TypeScript's chain of `replace`s did: a token that a
        // stem or an earlier token brought in is replaced too. NOTE(port): a stem with "{n}" in it gets
        // numbered; the TypeScript did, so this does.
        let taken = details
            .and_then(|d| d.taken)
            .filter(|&ms| ms != 0.0)
            .and_then(whole_ms)
            .map_or_else(|| date.clone(), iso_date);
        let track = details
            .and_then(|d| d.track)
            .filter(|&n| n != 0)
            .map_or_else(String::new, |n| pad_start(i128::from(n), 2));
        let mut built = rules
            .pattern
            .replace("{name}", stem)
            .replace("{ext}", ext)
            .replace("{nnn}", &pad_start(seq, 3))
            .replace("{nn}", &pad_start(seq, 2))
            .replace("{n}", &seq.to_string())
            .replace("{date}", &date)
            .replace("{parent}", &parent)
            .replace("{taken}", &taken)
            .replace("{camera}", &safe(details.and_then(|d| d.camera.as_deref())))
            .replace("{artist}", &safe(details.and_then(|d| d.artist.as_deref())))
            .replace("{album}", &safe(details.and_then(|d| d.album.as_deref())))
            .replace("{title}", &safe(details.and_then(|d| d.title.as_deref())))
            .replace("{track}", &track);
        built = transform_case(&built, rules.case_mode);
        let new_name = if pattern_has_ext || ext.is_empty() {
            built
        } else {
            format!("{built}.{ext}")
        };
        let to = join_path(&folder, &new_name);
        drafts.push((item, new_name, to));
    }

    // How many items land on each path (case-insensitively), to spot two becoming one.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, _, to) in &drafts {
        *counts.entry(to.to_lowercase()).or_insert(0) += 1;
    }

    let mut rows = Vec::with_capacity(drafts.len());
    for (item, new_name, to) in drafts {
        let changed = new_name != item.name;
        let conflict = validate_target_name(&new_name).or_else(|| {
            if counts.get(&to.to_lowercase()).copied().unwrap_or(0) > 1 {
                Some(Conflict::Duplicate)
            } else if changed && existing.contains(&new_name.to_lowercase()) {
                Some(Conflict::Exists)
            } else {
                None
            }
        });
        rows.push(Row {
            from: item.path.clone(),
            to,
            name: item.name.clone(),
            new_name,
            changed,
            conflict,
        });
    }
    let changed_count = rows.iter().filter(|r| r.changed).count();
    let conflict_count = rows.iter().filter(|r| r.conflict.is_some()).count();
    Simulation {
        rows,
        regex_error,
        changed_count,
        conflict_count,
    }
}

// ---- Where the work is done -----------------------------------------------------------------------------

/// A boxed future, as core's functions are called.
pub type Fut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Renames the pairs, all or none.
pub type RenameFn = dyn Fn(Vec<RenamePair>) -> Fut<Result<BatchRenameResult, String>> + Send + Sync;
/// Reads the details of the files at these paths.
pub type DetailsFn = dyn Fn(Vec<String>) -> Fut<Result<Vec<Option<Media>>, String>> + Send + Sync;

/// Where renaming and the files' details come from: glint-core in the app, a fake in tests.
#[derive(Clone)]
pub struct Source {
    /// Renames every pair, or none (core's two-phase renamer). Its answer holds the pairs that undo it.
    pub rename: Arc<RenameFn>,
    /// The details of these files, in order (`None` for a file that says nothing).
    pub details: Arc<DetailsFn>,
}

/// glint-core's `batch_rename` and `media_details_many`, as a [`Source`].
pub fn core_source() -> Source {
    Source {
        rename: Arc::new(|pairs| Box::pin(glint_core::batch_rename::batch_rename(pairs))),
        details: Arc::new(|paths| {
            Box::pin(async move {
                let found = glint_core::media::media_details_many(paths).await?;
                Ok(found.into_iter().map(|d| d.map(Media::from)).collect())
            })
        }),
    }
}

/// What a finished batch left: enough to take it back.
#[derive(Clone, Debug)]
pub struct Applied {
    pub renamed: usize,
    pub elapsed_ms: f64,
    /// The pairs that put every name back (`undoOps`).
    pub undo_ops: Vec<RenamePair>,
    /// The folders the items were in, to read again.
    pub folders: Vec<String>,
    /// The explorer's Undo step that takes it back (Ctrl+Z), once [`renamed`] has kept it.
    pub step: Option<UndoId>,
}

impl Applied {
    fn of(done: BatchRenameResult, asked: &[RenamePair]) -> Self {
        let mut seen = HashSet::new();
        let folders = asked
            .iter()
            .map(|pair| parent_path(&pair.from))
            .filter(|folder| seen.insert(folder.to_lowercase()))
            .collect();
        Self {
            renamed: done.renamed,
            elapsed_ms: done.elapsed_ms,
            undo_ops: done.undo_ops,
            folders,
            step: None,
        }
    }

    /// "3 items".
    fn what(&self) -> String {
        plural(self.renamed, "item")
    }

    /// What the success notification says: "Renamed 3 items in 1.2 ms".
    pub fn message(&self) -> SharedString {
        t!(
            "Renamed {what} in {ms} ms",
            what = self.what(),
            ms = to_fixed(self.elapsed_ms, 1)
        )
    }

    /// What Undo calls it: "rename of 3 items", as in "Undid the rename of 3 items".
    pub fn label(&self) -> String {
        t!("rename of {what}", what = self.what()).to_string()
    }
}

/// The last batch, while Undo can still take it back: the dialog's "Undo last batch" (`lastUndo`). It has its
/// `step` set; the button is only drawn while the explorer still holds that step (Ctrl+Z may have used it).
#[derive(Default)]
pub struct LastBatch(pub Option<Applied>);

impl Global for LastBatch {}

// ---- The model ------------------------------------------------------------------------------------------

/// Where the files' details stand.
#[derive(Clone)]
enum Details {
    /// Not needed yet: no token in the pattern asks for one.
    Idle,
    /// Being read.
    Reading,
    /// Read (or failed, which leaves the map empty), by path.
    Ready(Arc<HashMap<String, Option<Media>>>),
}

/// The dialog's data: the items, the rules, and the preview they make.
pub struct BatchRename {
    items: Arc<[Item]>,
    existing: Arc<HashSet<String>>,
    rules: Rules,
    details: Details,
    simulation: Arc<Simulation>,
    source: Source,
    running: bool,
    _reading: Option<Task<()>>,
}

impl BatchRename {
    /// A model over `items`; `existing` holds the lower-cased names in their folder that aren't among them.
    pub fn new(items: Vec<Item>, existing: HashSet<String>, source: Source) -> Self {
        let items: Arc<[Item]> = items.into();
        let existing = Arc::new(existing);
        let rules = Rules::default();
        let simulation = Arc::new(simulate(&items, &existing, &rules, None));
        Self {
            items,
            existing,
            rules,
            details: Details::Idle,
            simulation,
            source,
            running: false,
            _reading: None,
        }
    }

    /// The items being renamed.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The rules as set.
    pub fn rules(&self) -> &Rules {
        &self.rules
    }

    /// The preview.
    pub fn simulation(&self) -> &Arc<Simulation> {
        &self.simulation
    }

    /// The files' details are being read and the pattern needs them (`reading`).
    pub fn reading(&self) -> bool {
        wants_details(&self.rules.pattern) && !matches!(self.details, Details::Ready(_))
    }

    /// Renaming is under way.
    pub fn running(&self) -> bool {
        self.running
    }

    /// Whether Rename may be pressed: something changes, nothing is wrong, nothing is under way.
    pub fn can_apply(&self) -> bool {
        !self.running
            && !self.reading()
            && self.simulation.regex_error.is_none()
            && self.simulation.conflict_count == 0
            && self.simulation.changed_count > 0
    }

    fn resimulate(&mut self) {
        let media = match &self.details {
            Details::Ready(found) => Some(found.clone()),
            _ => None,
        };
        self.simulation = Arc::new(simulate(&self.items, &self.existing, &self.rules, media.as_deref()));
    }

    /// Changes the rules and works out the preview again; reads the files' details the first time the
    /// pattern asks for one.
    pub fn update_rules(&mut self, change: impl FnOnce(&mut Rules), cx: &mut Context<Self>) {
        let before = self.rules.clone();
        change(&mut self.rules);
        if self.rules == before {
            return;
        }
        self.resimulate();
        if wants_details(&self.rules.pattern) && matches!(self.details, Details::Idle) {
            self.read_details(cx);
        }
        cx.notify();
    }

    /// "Reset": every rule back to its start (`resetRules`). The regex and match case switches stay, as they
    /// did.
    pub fn reset_rules(&mut self, cx: &mut Context<Self>) {
        let (use_regex, case_sensitive) = (self.rules.use_regex, self.rules.case_sensitive);
        self.update_rules(
            |rules| {
                *rules = Rules {
                    use_regex,
                    case_sensitive,
                    ..Rules::default()
                }
            },
            cx,
        );
    }

    /// Reads the items' details on core's runtime, once. A failure leaves the map empty, so the preview
    /// falls back to the items' dates and empty text, as the TypeScript's did.
    fn read_details(&mut self, cx: &mut Context<Self>) {
        self.details = Details::Reading;
        let paths: Vec<String> = self.items.iter().map(|i| i.path.clone()).collect();
        let reading = (self.source.details)(paths.clone());
        self._reading = Some(cx.spawn(async move |this, cx| {
            let found = core_bridge::run(reading)
                .await
                .and_then(|inner| inner)
                .unwrap_or_default();
            let map: HashMap<String, Option<Media>> = paths
                .into_iter()
                .enumerate()
                .map(|(i, path)| (path, found.get(i).cloned().flatten()))
                .collect();
            let _ = this.update(cx, |this, cx| {
                this.details = Details::Ready(Arc::new(map));
                this.resimulate();
                cx.notify();
            });
        }));
    }

    /// Starts renaming: the pairs to ask core for, or `None` when Rename isn't allowed now.
    fn begin(&mut self, cx: &mut Context<Self>) -> Option<Vec<RenamePair>> {
        if !self.can_apply() {
            return None;
        }
        self.running = true;
        cx.notify();
        Some(self.simulation.pairs())
    }

    fn end(&mut self, cx: &mut Context<Self>) {
        self.running = false;
        cx.notify();
    }
}

/// The items and the folder's other names for `paths`, from the folder on screen: the entries of the
/// tab if it lists them (or the search's results), else items known by their path alone. The other names
/// are those of the listing outside the selection; a search's results have none (the items may be in
/// different folders, and core still refuses a name that is taken).
///
/// One pass over the listing, matching its lower-cased names against the selection's first, so a folder of
/// 100,000 entries costs no more than reading it (the entries are shared, not copied).
pub fn gather(paths: &[String], cx: &App) -> (Vec<Item>, HashSet<String>) {
    let shown = try_explorer(cx).and_then(|explorer| {
        explorer
            .read(cx)
            .active_tab()
            .map(|tab| (tab.find.as_ref().map(|find| find.entries.clone()), tab.shared_entries()))
    });
    let Some((found, folder)) = shown else {
        return (paths.iter().map(|p| Item::from_path(p)).collect(), HashSet::new());
    };
    let searching = found.is_some();
    let lowered: Vec<String> = paths.iter().map(|p| p.to_lowercase()).collect();
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (ix, path) in paths.iter().enumerate() {
        by_name.entry(base_name(path).to_lowercase()).or_default().push(ix);
    }
    let mut items: Vec<Option<Item>> = vec![None; paths.len()];
    // Whether `entry` is one of the selection (filling in its item the first time it's seen).
    let mut pick = |entry: &FileEntry| -> bool {
        let Some(candidates) = by_name.get(entry.lower.as_str()) else {
            return false;
        };
        let path = entry.path.to_lowercase();
        let mut hit = false;
        for &ix in candidates {
            if lowered.get(ix).is_some_and(|l| *l == path) {
                hit = true;
                if let Some(slot) = items.get_mut(ix).filter(|slot| slot.is_none()) {
                    *slot = Some(Item::from_entry(entry));
                }
            }
        }
        hit
    };
    for entry in found.iter().flat_map(|found| found.iter()) {
        pick(entry);
    }
    let mut existing = HashSet::new();
    for entry in folder.iter().flat_map(|folder| folder.iter()) {
        if !pick(entry) && !searching {
            existing.insert(entry.lower.clone());
        }
    }
    let items = items
        .into_iter()
        .zip(paths)
        .map(|(item, path)| item.unwrap_or_else(|| Item::from_path(path)))
        .collect();
    (items, existing)
}

// ---- Telling the person ---------------------------------------------------------------------------------

/// The notification that offers Undo (the TypeScript's success toast with an action).
struct RenamedToast;

/// How long the Undo offer stays, as the web app's toasts.
const UNDO_FOR: Duration = Duration::from_secs(8);

/// Reads the folders of a batch again, so the list shows the new names.
fn reload_folders(folders: &[String], cx: &mut App) {
    if let Some(explorer) = try_explorer(cx) {
        explorer.update(cx, |explorer, cx| {
            for folder in folders {
                explorer.reload_folder(folder, cx);
            }
        });
    }
}

/// Keeps how to take the batch back, so Ctrl+Z does (`rememberUndo(`rename of ${what}`, ...)`): the explorer's
/// step holds the pairs that put every name back, and runs them through core's all-or-nothing renamer. `None`
/// when the batch changed nothing (no way back) or there is no explorer.
fn remember(applied: &Applied, cx: &mut App) -> Option<UndoId> {
    if applied.undo_ops.is_empty() {
        return None;
    }
    let explorer = try_explorer(cx)?;
    let pairs = applied
        .undo_ops
        .iter()
        .map(|pair| BatchPair {
            from: pair.from.clone(),
            to: pair.to.clone(),
        })
        .collect();
    let label = applied.label();
    Some(explorer.update(cx, |explorer, cx| {
        explorer.remember_undo(label, UndoAction::RenameBatch(pairs), cx)
    }))
}

/// Says a batch was done (`Renamed 3 items in 1.2 ms`) with an Undo button, keeps it for Ctrl+Z and for "Undo
/// last batch", and reads the folders again.
pub fn renamed(mut applied: Applied, window: &mut Window, cx: &mut App) {
    reload_folders(&applied.folders, cx);
    applied.step = remember(&applied, cx);
    cx.set_global(LastBatch(applied.step.map(|_| applied.clone())));
    let mut note = Notification::success(applied.message()).id::<RenamedToast>();
    if let Some(step) = applied.step {
        note = note.action(move |_, _, _| {
            Button::new("batch-rename-undo")
                .label(t!("Undo"))
                .on_click(move |_, window, cx| {
                    window.remove_notification::<RenamedToast>(cx);
                    undo_batch(step, cx);
                })
        });
    }
    window.push_notification(note, cx);
    if applied.step.is_some() {
        let handle = window.window_handle();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(UNDO_FOR).await;
            let _ = handle.update(cx, |_, window, cx| window.remove_notification::<RenamedToast>(cx));
        })
        .detach();
    }
}

/// Puts the names of a batch back (`undo`), as Ctrl+Z would: the explorer runs the step, says how it went and
/// reads the folders again. The batch is used up whatever comes of it, as the TypeScript's `finally` did.
pub fn undo_batch(step: UndoId, cx: &mut App) {
    cx.set_global(LastBatch(None));
    if let Some(explorer) = try_explorer(cx) {
        explorer.update(cx, |explorer, cx| explorer.undo(Some(step), cx).detach());
    }
}

// ---- The dialog -----------------------------------------------------------------------------------------

/// A preview row's height in rems; the list is uniform so it scrolls cheaply.
const ROW_REMS: f32 = 8. * 0.25;
const ROW: Rems = gpui_kit::rems(ROW_REMS);
/// The share of the window's height the dialog may take, and what the rest of it needs (title, header, the
/// two cards, the list's heading, the footer and the gaps), in rems.
const LIST_OF_WINDOW: f32 = 0.88;
const OTHER_REMS: f32 = 30.;

/// The dialog: the rules at the top, the preview of every new name under them, and the buttons.
pub struct BatchRenameDialog {
    model: Entity<BatchRename>,
    find: Entity<InputState>,
    replace: Entity<InputState>,
    pattern: Entity<InputState>,
    start: Entity<InputState>,
    step: Entity<InputState>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl BatchRenameDialog {
    /// A dialog over `model`, its Find box focused.
    pub fn new(model: Entity<BatchRename>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rules = model.read(cx).rules().clone();
        let regex = rules.use_regex;
        let find = cx.new(|cx| InputState::new(window, cx).placeholder(find_placeholder(regex)));
        let replace = cx.new(|cx| InputState::new(window, cx).placeholder(replace_placeholder(regex)));
        // Not `t!`: it would read {name} and {nn} as blanks to fill.
        let pattern = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("{name}_{nn}")
                .default_value(rules.pattern.clone())
        });
        let start = cx.new(|cx| InputState::new(window, cx).default_value(rules.start.to_string()));
        let step = cx.new(|cx| InputState::new(window, cx).default_value(rules.step.to_string()));

        let mut subscriptions = vec![cx.observe(&model, |_, _, cx| cx.notify())];
        // Each box feeds its rule; Enter in any of them renames, as the TypeScript's form did.
        for (input, apply) in [
            (&find, (|rules, text| rules.find = text) as fn(&mut Rules, String)),
            (&replace, |rules, text| rules.replace = text),
            (&pattern, |rules, text| rules.pattern = text),
            (&start, |rules, text| rules.start = whole_number(&text, 0)),
            (&step, |rules, text| {
                rules.step = match whole_number(&text, 1) {
                    0 => 1,
                    other => other,
                }
            }),
        ] {
            subscriptions.push(
                cx.subscribe_in(
                    input,
                    window,
                    move |this, input, event: &InputEvent, window, cx| match event {
                        InputEvent::Change => {
                            let text = input.read(cx).value().to_string();
                            this.model
                                .update(cx, |model, cx| model.update_rules(|r| apply(r, text), cx));
                        }
                        InputEvent::PressEnter { .. } => this.apply(window, cx),
                        _ => {}
                    },
                ),
            );
        }

        let first = find.clone();
        window.defer(cx, move |window, cx| {
            first.update(cx, |input, cx| input.focus(window, cx));
        });
        Self {
            model,
            find,
            replace,
            pattern,
            start,
            step,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Whether the caret is in one of the five boxes.
    fn typing_in_a_box(&self, window: &Window, cx: &App) -> bool {
        [&self.find, &self.replace, &self.pattern, &self.start, &self.step]
            .into_iter()
            .any(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Turns the regex switch, and words the boxes for it.
    fn set_regex(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.model
            .update(cx, |model, cx| model.update_rules(|r| r.use_regex = on, cx));
        self.find
            .update(cx, |input, cx| input.set_placeholder(find_placeholder(on), window, cx));
        self.replace.update(cx, |input, cx| {
            input.set_placeholder(replace_placeholder(on), window, cx)
        });
    }

    /// A token added to the end of the pattern.
    fn add_token(&mut self, token: &str, window: &mut Window, cx: &mut Context<Self>) {
        let next = format!("{}{token}", self.model.read(cx).rules().pattern);
        self.pattern
            .update(cx, |input, cx| input.set_value(next.clone(), window, cx));
        self.model
            .update(cx, |model, cx| model.update_rules(|r| r.pattern = next, cx));
    }

    /// "Reset": every rule and every box back to its start.
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| model.reset_rules(cx));
        let rules = self.model.read(cx).rules().clone();
        for (input, text) in [
            (&self.find, rules.find),
            (&self.replace, rules.replace),
            (&self.pattern, rules.pattern),
            (&self.start, rules.start.to_string()),
            (&self.step, rules.step.to_string()),
        ] {
            input.update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }

    /// Renames the items (Enter, or the button): all of them or none. On success the dialog closes, the
    /// folders are read again and a notification offers Undo; a failure says why and keeps the dialog.
    ///
    /// The work is spawned on the window, not on this view, so closing the dialog meanwhile (Escape) still
    /// reads the folders and offers Undo.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(asked) = self.model.update(cx, |model, cx| model.begin(cx)) else {
            return;
        };
        let source = self.model.read(cx).source.clone();
        let model = self.model.clone();
        let view = cx.weak_entity();
        let renaming = (source.rename)(asked.clone());
        window
            .spawn(cx, async move |cx| {
                let result = core_bridge::run(renaming).await.and_then(|inner| inner);
                let _ = cx.update(|window, cx| {
                    model.update(cx, |model, cx| model.end(cx));
                    match result {
                        Ok(done) => {
                            // Only close what is still this dialog.
                            if view.upgrade().is_some() {
                                window.close_dialog(cx);
                            }
                            renamed(Applied::of(done, &asked), window, cx);
                        }
                        Err(message) => window
                            .push_notification(Notification::error(message).title(t!("Couldn't rename the items")), cx),
                    }
                });
            })
            .detach();
    }

    /// A small switch button in the rules' cards.
    fn toggle(id: &'static str, on: bool, label: SharedString, tooltip: SharedString) -> Button {
        Button::new(id)
            .ghost()
            .xsmall()
            .selected(on)
            .toggled(on)
            .label(label)
            .tooltip(tooltip)
    }

    /// Card 1: find and replace.
    fn find_card(
        &self,
        rules: &Rules,
        regex_error: Option<&String>,
        t: &theme::Tokens,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let title = t!("1. Find and replace");
        card(t)
            .id("batch-rename-find")
            .role(Role::Group)
            .aria_label(title.clone())
            .test_support()
            .child(
                row_start()
                    .justify_between()
                    .gap(space(2.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(
                        align_row(1.)
                            .child(
                                Self::toggle(
                                    "batch-rename-regex",
                                    rules.use_regex,
                                    t!("Regex"),
                                    t!("Use regular expressions (supports $1, $2 capture groups)"),
                                )
                                .icon(IconName::Regex)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let on = !this.model.read(cx).rules().use_regex;
                                    this.set_regex(on, window, cx);
                                })),
                            )
                            .child(
                                Self::toggle("batch-rename-case", rules.case_sensitive, "Aa".into(), t!("Match case"))
                                    .accessibility_label(t!("Match case"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.model.update(cx, |model, cx| {
                                            model.update_rules(|r| r.case_sensitive = !r.case_sensitive, cx)
                                        });
                                    })),
                            ),
                    ),
            )
            .child(
                row_start()
                    .gap(space(2.))
                    .font_family(mono.clone())
                    .text_size(theme::size::XS)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.find).small().aria_label(t!("Find text"))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.replace).small().aria_label(t!("Replace with"))),
                    ),
            )
            .children(regex_error.map(|message| {
                div()
                    .id("batch-rename-regex-error")
                    .role(Role::Alert)
                    .aria_label(SharedString::from(message.clone()))
                    .test_support()
                    .text_size(theme::size::XXS)
                    .text_color(t.destructive)
                    .child(SharedString::from(message.clone()))
            }))
    }

    /// Card 2: the pattern, the numbering, the case.
    fn pattern_card(
        &self,
        rules: &Rules,
        reading: bool,
        t: &theme::Tokens,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let title = t!("2. Pattern and case");
        let numbers = |label: SharedString, input: &Entity<InputState>| {
            align_row(1.5)
                .text_size(theme::size::XXS)
                .child(div().text_color(t.muted_foreground).child(label.clone()))
                .child(
                    div()
                        .w(space(18.))
                        .font_family(mono.clone())
                        .child(Input::new(input).small().aria_label(label)),
                )
        };
        card(t)
            .id("batch-rename-pattern")
            .role(Role::Group)
            .aria_label(title.clone())
            .test_support()
            .child(
                row_start()
                    .justify_between()
                    .gap(space(2.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(
                        align_row(2.)
                            .child(numbers(t!("Start #"), &self.start))
                            .child(numbers(t!("Step"), &self.step)),
                    ),
            )
            .child(
                div()
                    .font_family(mono.clone())
                    .text_size(theme::size::XS)
                    .child(Input::new(&self.pattern).small().aria_label(t!("Pattern"))),
            )
            .child(
                row_start()
                    .id("batch-rename-case-mode")
                    .role(Role::RadioGroup)
                    .aria_label(t!("Letter case"))
                    .test_support()
                    .flex_wrap()
                    .gap(space(1.))
                    .children(CaseMode::ALL.into_iter().map(|mode| {
                        Button::new(("batch-rename-case-mode", mode as usize))
                            .ghost()
                            .xsmall()
                            .selected(rules.case_mode == mode)
                            .toggled(rules.case_mode == mode)
                            .label(mode.label())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.model
                                    .update(cx, |model, cx| model.update_rules(|r| r.case_mode = mode, cx));
                            }))
                            .into_any_element()
                    })),
            )
            .child(
                row_start()
                    .flex_wrap()
                    .items_center()
                    .gap(space(1.))
                    .when(reading, |this| {
                        this.child(
                            align_row(1.)
                                .id("batch-rename-reading")
                                .role(Role::Status)
                                .aria_label(t!("Reading the files' details…"))
                                .test_support()
                                .me(1.)
                                .text_size(theme::size::XXS)
                                .text_color(t.muted_foreground)
                                .child(Spinner::new().xsmall())
                                .child(t!("Reading the files' details…")),
                        )
                    })
                    .children(TOKENS.into_iter().enumerate().map(|(i, token)| {
                        Button::new(("batch-rename-token", i))
                            .outline()
                            .xsmall()
                            .label(token)
                            .tooltip(token_help(token))
                            .accessibility_label(format!("{token}: {}", token_help(token)))
                            .on_click(cx.listener(move |this, _, window, cx| this.add_token(token, window, cx)))
                    })),
            )
    }

    /// The preview's header.
    fn list_header(t: &theme::Tokens) -> impl IntoElement {
        row_start()
            .h(ROW)
            .flex_none()
            .bg(t.muted.opacity(0.9))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .font_weight(FontWeight::SEMIBOLD)
            .child(
                div()
                    .w(space(10.))
                    .flex_none()
                    .px(space(3.))
                    .text_end()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(t.muted_foreground)
                    .child("#"),
            )
            .child(
                div()
                    .w(gpui_kit::relative(0.44))
                    .flex_none()
                    .px(space(3.))
                    .child(t!("Current name")),
            )
            .child(div().w(space(6.)).flex_none())
            .child(div().flex_1().min_w_0().px(space(3.)).child(t!("New name")))
    }

    /// One line of the preview.
    fn preview_row(ix: usize, row: &Row, t: &theme::Tokens, mono: &SharedString) -> impl IntoElement {
        let fill = if row.conflict.is_some() {
            t.destructive.opacity(0.1)
        } else if row.changed {
            t.success.opacity(0.05)
        } else {
            t.background.opacity(0.)
        };
        let new_color = if row.conflict.is_some() {
            t.destructive
        } else if row.changed {
            t.success
        } else {
            t.muted_foreground
        };
        let mut label = format!("{} → {}", row.name, row.new_name);
        if let Some(conflict) = row.conflict {
            label = format!("{label}: {}", conflict.message());
        }
        row_start()
            .id(("batch-rename-row", ix))
            .role(Role::ListItem)
            .aria_label(SharedString::from(label))
            .test_support()
            .h(ROW)
            .w_full()
            .bg(fill)
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .child(
                div()
                    .w(space(10.))
                    .flex_none()
                    .px(space(3.))
                    .text_end()
                    .font_family(mono.clone())
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(format_count(ix + 1)),
            )
            .child(
                div()
                    .w(gpui_kit::relative(0.44))
                    .flex_none()
                    .px(space(3.))
                    .truncate()
                    .font_family(mono.clone())
                    .text_color(t.muted_foreground)
                    .child(isolate(row.name.clone())),
            )
            .child(
                div()
                    .w(space(6.))
                    .flex_none()
                    .child(icon(forward_icon()).size_3p5().text_color(if row.changed {
                        t.primary
                    } else {
                        t.muted_foreground.opacity(0.3)
                    })),
            )
            .child(
                row_start()
                    .flex_1()
                    .min_w_0()
                    .gap(space(2.))
                    .px(space(3.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(mono.clone())
                            .when(row.changed || row.conflict.is_some(), |this| {
                                this.font_weight(FontWeight::SEMIBOLD)
                            })
                            .text_color(new_color)
                            .child(isolate(row.new_name.clone())),
                    )
                    .children(row.conflict.map(|conflict| {
                        align_row(1.)
                            .flex_none()
                            .ms(2.)
                            .px(space(1.5))
                            .py(space(0.5))
                            .rounded(theme::size::RADIUS_SM)
                            .bg(t.destructive.opacity(0.15))
                            .text_size(theme::size::XXS)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.destructive)
                            .child(icon(IconName::TriangleAlert).size_3())
                            .child(conflict.message())
                    })),
            )
    }

    /// The line at the bottom left: what is wrong, or what will happen.
    fn status_line(&self, sim: &Simulation, total: usize, t: &theme::Tokens) -> impl IntoElement {
        let line = align_row(1.5)
            .id("batch-rename-status")
            .role(Role::Status)
            .test_support();
        if sim.conflict_count > 0 {
            let text = t!(
                "{what} detected — resolve before renaming",
                what = plural(sim.conflict_count, "conflict")
            );
            line.aria_label(text.clone())
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.destructive)
                .child(icon(IconName::TriangleAlert).size_3p5().flex_none())
                .child(div().min_w_0().child(text))
        } else if sim.changed_count > 0 {
            let text = t!(
                "Ready to rename {changed} of {total} items: all of them, or none if one can't be",
                changed = format_count(sim.changed_count),
                total = format_count(total)
            );
            line.aria_label(text.clone())
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.success)
                .child(icon(IconName::CircleCheck).size_3p5().flex_none())
                .child(div().min_w_0().child(text))
        } else {
            let text = t!("Modify Find/Replace, Pattern, or Case above to preview changes");
            line.aria_label(text.clone())
                .text_color(t.muted_foreground)
                .child(div().min_w_0().child(text))
        }
    }
}

/// A rules card: bordered, on the card color.
fn card(t: &theme::Tokens) -> Div {
    col()
        .flex_1()
        .min_w_0()
        .gap(space(2.))
        .p(space(3.))
        .rounded(theme::size::RADIUS)
        .border_1()
        .border_color(t.border)
        .bg(t.card)
}

fn find_placeholder(regex: bool) -> SharedString {
    if regex {
        t!("Regex (e.g. ^IMG_(\\d+))")
    } else {
        t!("Find text…")
    }
}

fn replace_placeholder(regex: bool) -> SharedString {
    if regex {
        t!("Replace (e.g. Photo_$1)")
    } else {
        t!("Replace with…")
    }
}

/// A whole number as typed (`Number(text) || fallback`): nothing, or nothing readable, is `fallback`; a
/// fraction loses its fraction.
fn whole_number(text: &str, fallback: i64) -> i64 {
    let text = js_trim(text);
    if let Ok(n) = text.parse::<i64>() {
        return n;
    }
    match text.parse::<f64>() {
        Ok(f) if f.is_finite() && f.abs() < 9.0e15 => f.trunc() as i64,
        _ => fallback,
    }
}

impl Render for BatchRenameDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let mono = theme::mono_font(cx);
        // The TypeScript's dialog was at most 88vh tall, with the list filling what the rules left: here the
        // list is as tall as 88% of the window allows after the rest of the dialog, between 4 and 18 rows.
        let rem = window.rem_size();
        let list_height = (window.viewport_size().height * LIST_OF_WINDOW - rem * OTHER_REMS)
            .clamp(ROW_REMS * 4. * rem, ROW_REMS * 18. * rem);
        let (rules, sim, reading, running, count) = {
            let model = self.model.read(cx);
            (
                model.rules().clone(),
                model.simulation().clone(),
                model.reading(),
                model.running(),
                model.items().len(),
            )
        };
        let can_apply = self.model.read(cx).can_apply();
        // The last batch, if the explorer still has the step that takes it back.
        let last = cx
            .try_global::<LastBatch>()
            .and_then(|last| last.0.clone())
            .and_then(|applied| {
                let step = applied.step?;
                try_explorer(cx)
                    .is_some_and(|explorer| explorer.read(cx).has_undo(step))
                    .then_some((applied, step))
            });
        let changed = sim.changed_count;

        let header = row_start()
            .gap(space(3.))
            .items_start()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!("Every item is renamed, or none is, and Undo puts the names back")),
            )
            .children(last.map(|(applied, step)| {
                let what = applied.what();
                Button::new("batch-rename-undo-last")
                    .outline()
                    .small()
                    .icon(IconName::RotateCcw)
                    .label(t!("Undo last batch"))
                    .tooltip(t!("Undo the {label}", label = applied.label()))
                    .accessibility_label(t!("Undo last batch: {what}", what = what))
                    .disabled(running)
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        undo_batch(step, cx);
                    })
            }))
            .child(
                Button::new("batch-rename-reset")
                    .ghost()
                    .small()
                    .label(t!("Reset"))
                    .disabled(running)
                    .on_click(cx.listener(|this, _, window, cx| this.reset(window, cx))),
            );

        let rules_row = row_start()
            .items_start()
            .gap(space(3.))
            .p(space(4.))
            .rounded(theme::size::RADIUS)
            .bg(t.muted.opacity(0.2))
            .text_size(theme::size::XS)
            .child(self.find_card(&rules, sim.regex_error.as_ref(), &t, &mono, cx))
            .child(self.pattern_card(&rules, reading, &t, &mono, cx));

        let preview = {
            let sim = sim.clone();
            let mono = mono.clone();
            let colors = t.clone();
            uniform_list(
                "batch-rename-list",
                sim.rows.len(),
                cx.processor(move |_, range: Range<usize>, _, _| {
                    range
                        .filter_map(|ix| {
                            sim.rows
                                .get(ix)
                                .map(|row| Self::preview_row(ix, row, &colors, &mono).into_any_element())
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full()
        };

        let footer = row_start()
            .gap(space(3.))
            .items_center()
            .justify_between()
            .pt(space(2.5))
            .border_t_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .child(div().flex_1().min_w_0().child(self.status_line(&sim, count, &t)))
            .child(
                align_row(2.)
                    .child(
                        Button::new("batch-rename-cancel")
                            .outline()
                            .small()
                            .label(t!("Cancel"))
                            .disabled(running)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("batch-rename-apply")
                            .primary()
                            .small()
                            .icon(IconName::Sparkles)
                            .label(t!("Rename {what}", what = plural(changed, "item")))
                            .loading(running)
                            .disabled(!can_apply)
                            .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
                    ),
            );

        col()
            .id("batch-rename")
            .role(Role::Group)
            .aria_label(t!("Rename items"))
            .test_support()
            // Enter asks the dialog to confirm, which would close it with nothing renamed (a conflict, a
            // broken pattern): here it renames if it can, and the dialog stays otherwise, as the form did.
            .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                if this.typing_in_a_box(window, cx) {
                    this.apply(window, cx);
                } else {
                    // On a button, Enter is the button's own.
                    cx.propagate();
                }
            }))
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer sits outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .child(header)
            .child(rules_row)
            .child(
                col()
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(t.border)
                    .overflow_hidden()
                    .child(Self::list_header(&t))
                    .child(
                        div()
                            .relative()
                            .h(list_height)
                            .child(
                                div()
                                    .id("batch-rename-preview")
                                    .role(Role::List)
                                    .aria_label(t!("New names"))
                                    .test_support()
                                    .size_full()
                                    .child(preview),
                            )
                            .child(Scrollbar::vertical(&self.scroll).viewport_from_layout()),
                    ),
            )
            .child(footer)
    }
}

/// Opens the dialog for `paths`, the items to rename (the selection: the BatchRename action and
/// `ExplorerEvent::BatchRename`). Nothing opens for none. The items' dates and the folder's other names come
/// from the folder on screen.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    let (items, existing) = gather(&paths, cx);
    open_with(window, cx, items, existing, core_source());
}

/// [`open`] for given items and a given [`Source`] (tests pass fakes). Returns the dialog's view.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    items: Vec<Item>,
    existing: HashSet<String>,
    source: Source,
) -> Option<Entity<BatchRenameDialog>> {
    if items.is_empty() {
        return None;
    }
    let title = t!("Rename {what}", what = plural(items.len(), "item"));
    let model = cx.new(|_| BatchRename::new(items, existing, source));
    let view = cx.new(|cx| BatchRenameDialog::new(model, window, cx));
    super::show(window, cx, title, px(896.), view.clone());
    Some(view)
}

// ---- Tests ----------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer;
    use crate::explorer::fake::{file, folder, FakeDisk};
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;

    fn item(dir: &str, name: &str) -> Item {
        Item {
            path: format!("{dir}\\{name}"),
            name: name.to_string(),
            dir: false,
            modified: 0,
        }
    }

    fn folder_item(dir: &str, name: &str) -> Item {
        Item {
            dir: true,
            ..item(dir, name)
        }
    }

    fn rules(pattern: &str) -> Rules {
        Rules {
            pattern: pattern.to_string(),
            ..Rules::default()
        }
    }

    fn names(sim: &Simulation) -> Vec<&str> {
        sim.rows.iter().map(|r| r.new_name.as_str()).collect()
    }

    fn run(items: &[Item], rules: &Rules) -> Simulation {
        simulate(items, &HashSet::new(), rules, None)
    }

    // ---- The pure parts ----

    #[test]
    fn stems_and_extensions_split_as_the_typescript_did() {
        assert_eq!(split_stem_ext("report.final.pdf", false), ("report.final", "pdf"));
        assert_eq!(split_stem_ext("Makefile", false), ("Makefile", ""));
        // A leading dot starts the name; a folder has no extension.
        assert_eq!(split_stem_ext(".gitignore", false), (".gitignore", ""));
        assert_eq!(split_stem_ext("v1.2", true), ("v1.2", ""));
        assert_eq!(split_stem_ext("تقرير.pdf", false), ("تقرير", "pdf"));
        assert_eq!(split_stem_ext("trailing.", false), ("trailing", ""));
    }

    #[test]
    fn case_modes_follow_the_typescript() {
        assert_eq!(transform_case("My File", CaseMode::Lower), "my file");
        assert_eq!(transform_case("My File", CaseMode::Upper), "MY FILE");
        assert_eq!(transform_case("My File", CaseMode::None), "My File");
        assert_eq!(transform_case("my_file-name.v2", CaseMode::Title), "My File Name V2");
        assert_eq!(transform_case("myFileName", CaseMode::Snake), "my_file_name");
        assert_eq!(transform_case("My File  Name", CaseMode::Kebab), "my-file-name");
        // Digits followed by a capital split too; capitals in a row don't.
        assert_eq!(transform_case("IMG2Final", CaseMode::Snake), "img2_final");
        assert_eq!(transform_case("HTTPServer", CaseMode::Snake), "httpserver");
        // Nothing but separators: left as it was.
        assert_eq!(transform_case("___", CaseMode::Snake), "___");
        assert_eq!(transform_case("", CaseMode::Title), "");
        assert_eq!(transform_case("تقرير", CaseMode::Title), "تقرير");
    }

    #[test]
    fn target_names_are_checked_as_windows_does() {
        assert_eq!(validate_target_name(""), Some(Conflict::Empty));
        assert_eq!(validate_target_name(".."), Some(Conflict::Empty));
        assert_eq!(validate_target_name("a "), Some(Conflict::EndsWithSpaceOrPeriod));
        assert_eq!(validate_target_name("a."), Some(Conflict::EndsWithSpaceOrPeriod));
        assert_eq!(validate_target_name("a:b.txt"), Some(Conflict::InvalidCharacters));
        assert_eq!(validate_target_name("a\u{1}b"), Some(Conflict::InvalidCharacters));
        assert_eq!(validate_target_name("con.txt"), Some(Conflict::ReservedName));
        assert_eq!(validate_target_name("LPT3"), Some(Conflict::ReservedName));
        assert_eq!(validate_target_name("console.txt"), None);
        assert_eq!(validate_target_name("تقرير 1.pdf"), None);
        assert_eq!(Conflict::Empty.message(), "Name cannot be empty");
        assert_eq!(Conflict::Exists.message(), "Already exists in folder");
    }

    #[test]
    fn dates_are_iso_in_local_time_and_zero_is_the_epoch() {
        assert_eq!(iso_date(0), "1970-01-01");
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_797), (2024, 3, 15));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        // Local time is within a day of UTC's.
        let noon = 19_797 * 86_400_000 + 43_200_000;
        let shown = iso_date(noon);
        assert!(
            ["2024-03-14", "2024-03-15", "2024-03-16"].contains(&shown.as_str()),
            "{shown}"
        );
        assert_eq!(shown.len(), 10);
    }

    #[test]
    fn numbers_count_from_start_by_step_and_pad() {
        let items = [item("C:\\a", "x.txt"), item("C:\\a", "y.txt"), item("C:\\a", "z.txt")];
        let r = Rules {
            start: 8,
            step: 2,
            ..rules("{nnn}-{nn}-{n}")
        };
        assert_eq!(
            names(&run(&items, &r)),
            ["008-08-8.txt", "010-10-10.txt", "012-12-12.txt"]
        );
        // padStart keeps a sign where it is, like JS: "-5" padded to 3 is "0-5".
        assert_eq!(pad_start(-5, 3), "0-5");
        assert_eq!(pad_start(1234, 3), "1234");
        let r = Rules {
            start: 0,
            step: -1,
            ..rules("{n}")
        };
        assert_eq!(names(&run(&items, &r)), ["0.txt", "-1.txt", "-2.txt"]);
    }

    #[test]
    fn a_pattern_without_ext_keeps_the_extension_and_with_ext_decides_it() {
        let items = [
            item("C:\\a", "one.TXT"),
            folder_item("C:\\a", "dir.d"),
            item("C:\\a", "bare"),
        ];
        let sim = run(&items, &rules("{name}_new"));
        assert_eq!(names(&sim), ["one_new.TXT", "dir.d_new", "bare_new"]);
        let sim = run(&items, &rules("{name}.{ext}"));
        assert_eq!(names(&sim), ["one.TXT", "dir.d.", "bare."]);
        // "bare." and "dir.d." end in a period: flagged, as Windows would drop it.
        assert_eq!(sim.rows[0].conflict, None);
        assert_eq!(sim.rows[1].conflict, Some(Conflict::EndsWithSpaceOrPeriod));
        assert!(!sim.rows[0].changed);
    }

    #[test]
    fn the_other_tokens_read_the_folder_and_the_date() {
        let mut one = item("C:\\Photos\\Trip", "a.jpg");
        one.modified = 0;
        let sim = run(&[one], &rules("{parent}-{date}"));
        assert_eq!(names(&sim), ["Trip-1970-01-01.jpg"]);
        // A drive's root is "C:" (`baseName(parentPath(..))`), which Windows refuses in a name.
        let sim = run(&[item("C:", "a.jpg")], &rules("{parent}"));
        assert_eq!(sim.rows[0].new_name, "C:.jpg");
        assert_eq!(sim.rows[0].conflict, Some(Conflict::InvalidCharacters));
    }

    #[test]
    fn tokens_are_replaced_in_order_so_a_stem_with_one_is_numbered_as_the_typescript_did() {
        let sim = run(&[item("C:\\a", "x{n}y.txt")], &rules("{name}"));
        assert_eq!(names(&sim), ["x1y.txt"]);
    }

    #[test]
    fn media_tokens_use_the_details_and_fall_back_to_the_date() {
        let items = [item("C:\\m", "a.mp3"), item("C:\\m", "b.mp3"), item("C:\\m", "c.mp3")];
        let mut media = HashMap::new();
        media.insert(
            items[0].path.clone(),
            Some(Media {
                artist: Some("Fairuz: Live".into()),
                title: Some("  Habaytak  ".into()),
                album: Some("A/B".into()),
                track: Some(3),
                camera: Some("Canon".into()),
                taken: Some(1_710_504_000_000.0),
            }),
        );
        media.insert(items[1].path.clone(), None);
        let r = rules("{track} {artist} - {title} [{album}] {camera}");
        let sim = simulate(&items, &HashSet::new(), &r, Some(&media));
        assert_eq!(
            names(&sim),
            [
                "03 Fairuz_ Live - Habaytak [A_B] Canon.mp3",
                "  -  [] .mp3",
                "  -  [] .mp3"
            ]
        );
        // Unknown details are empty text; `{taken}` is the item's own date.
        let sim = simulate(&items, &HashSet::new(), &rules("{taken}"), Some(&media));
        assert_eq!(sim.rows[1].new_name, "1970-01-01.mp3");
        assert!(
            sim.rows[0].new_name.starts_with("2024-03-1"),
            "{}",
            sim.rows[0].new_name
        );
        // Without any details yet the same fallbacks hold.
        assert_eq!(run(&items, &rules("{artist}")).rows[0].new_name, ".mp3");
        assert!(wants_details("a{camera}") && !wants_details("{name}{n}"));
    }

    #[test]
    fn find_and_replace_is_plain_text_unless_regex_and_ignores_case_unless_asked() {
        let items = [item("C:\\a", "Price$&tax IMG_001.txt"), item("C:\\a", "img_002.txt")];
        // Plain: the text is taken as typed, and so is the replacement: $ stays $.
        let r = Rules {
            find: "img_".into(),
            replace: "$&Photo$1_".into(),
            ..rules("{name}")
        };
        assert_eq!(
            names(&run(&items, &r)),
            ["Price$&tax $&Photo$1_001.txt", "$&Photo$1_002.txt"]
        );
        // Match case leaves the capitals alone.
        let r = Rules {
            case_sensitive: true,
            ..r
        };
        assert_eq!(names(&run(&items, &r)), ["Price$&tax IMG_001.txt", "$&Photo$1_002.txt"]);
        // Dots and brackets in plain text are plain.
        let dotted = [item("C:\\a", "a.b (1).txt"), item("C:\\a", "axb (1).txt")];
        let r = Rules {
            find: ".b (1)".into(),
            replace: "_".into(),
            ..rules("{name}")
        };
        assert_eq!(names(&run(&dotted, &r)), ["a_.txt", "axb (1).txt"]);
    }

    #[test]
    fn regex_replacements_read_capture_groups_the_js_way() {
        let items = [item("C:\\a", "IMG_0042.jpg"), item("C:\\a", "other.jpg")];
        let r = Rules {
            find: r"^IMG_(\d+)".into(),
            replace: "Photo_$1".into(),
            use_regex: true,
            ..rules("{name}")
        };
        let sim = run(&items, &r);
        assert_eq!(names(&sim), ["Photo_0042.jpg", "other.jpg"]);
        assert!(sim.regex_error.is_none());
        assert_eq!((sim.changed_count, sim.conflict_count), (1, 0));
        // Anchors work on each stem, not on the whole list.
        let r = Rules {
            find: "^(.)".into(),
            replace: "<$1>".into(),
            use_regex: true,
            ..rules("{name}")
        };
        assert_eq!(names(&run(&items, &r)), ["<I>MG_0042.jpg", "<o>ther.jpg"]);
        // Case is ignored by default for a regex too, even when it has a capital.
        let r = Rules {
            find: "OTHER".into(),
            replace: "x".into(),
            use_regex: true,
            ..rules("{name}")
        };
        assert_eq!(names(&run(&items, &r)), ["IMG_0042.jpg", "x.jpg"]);
    }

    #[test]
    fn a_broken_pattern_is_reported_and_renames_nothing() {
        let items = [item("C:\\a", "a.txt")];
        let r = Rules {
            find: "(".into(),
            replace: "x".into(),
            use_regex: true,
            ..rules("{name}")
        };
        let sim = run(&items, &r);
        let error = sim.regex_error.clone().expect("an error");
        assert!(!error.contains('\n') && !error.is_empty(), "{error}");
        assert_eq!(names(&sim), ["a.txt"]);
        assert_eq!(sim.changed_count, 0);
        // The same text, as plain text, is fine.
        let plain = Rules { use_regex: false, ..r };
        assert!(run(&items, &plain).regex_error.is_none());
    }

    #[test]
    fn collisions_in_the_batch_and_on_disk_are_flagged() {
        let items = [item("C:\\a", "a.txt"), item("C:\\a", "b.txt"), item("C:\\a", "c.txt")];
        // Two items to one name: both flagged, whatever the letters' case.
        let sim = run(&items, &rules("same"));
        assert_eq!(names(&sim), ["same.txt"; 3]);
        assert!(sim.rows.iter().all(|r| r.conflict == Some(Conflict::Duplicate)));
        assert_eq!(sim.conflict_count, 3);
        let r = Rules {
            case_mode: CaseMode::Upper,
            ..rules("x")
        };
        assert_eq!(run(&items, &r).conflict_count, 3);
        // On disk: only when the name changes (an item that keeps its name isn't "already there").
        let existing: HashSet<String> = ["new.txt".to_string(), "a.txt".to_string()].into();
        let sim = simulate(&items[..1], &existing, &rules("new"), None);
        assert_eq!(sim.rows[0].conflict, Some(Conflict::Exists));
        let sim = simulate(&items[..1], &existing, &rules("{name}"), None);
        assert_eq!(sim.rows[0].conflict, None);
        assert_eq!(sim.changed_count, 0);
        // A swap inside the batch is no collision: core's two phases handle it.
        let swap = [item("C:\\a", "1.txt"), item("C:\\a", "2.txt")];
        let r = Rules {
            find: "1".into(),
            replace: "2".into(),
            ..rules("{name}")
        };
        let sim = run(&swap, &r);
        assert_eq!(names(&sim), ["2.txt", "2.txt"]);
        assert_eq!(sim.conflict_count, 2);
    }

    #[test]
    fn pairs_are_the_changed_items_in_their_own_folders() {
        let items = [item("C:\\a", "a.txt"), item("C:\\b", "b.txt")];
        let sim = run(&items, &rules("{name}2"));
        let pairs = sim.pairs();
        assert_eq!(pairs.len(), 2);
        assert_eq!(
            (pairs[0].from.as_str(), pairs[0].to.as_str()),
            ("C:\\a\\a.txt", "C:\\a\\a2.txt")
        );
        assert_eq!(pairs[1].to, "C:\\b\\b2.txt");
        assert!(run(&items, &rules("{name}")).pairs().is_empty());
    }

    #[test]
    fn the_success_message_counts_the_items_and_the_milliseconds() {
        let _guard = testing::lock();
        let applied = |renamed, elapsed_ms| Applied {
            renamed,
            elapsed_ms,
            undo_ops: Vec::new(),
            folders: Vec::new(),
            step: None,
        };
        assert_eq!(applied(2, 1.5).message(), "Renamed 2 items in 1.5 ms");
        assert_eq!(applied(1, 0.04).message(), "Renamed 1 item in 0.0 ms");
        assert_eq!(applied(1234, 12.34).message(), "Renamed 1,234 items in 12.3 ms");
        assert_eq!(applied(3, 1.0).label(), "rename of 3 items");
    }

    #[test]
    fn numbers_typed_into_the_boxes_are_read_like_the_typescript_read_them() {
        assert_eq!(whole_number("12", 0), 12);
        assert_eq!(whole_number(" -3 ", 0), -3);
        assert_eq!(whole_number("", 1), 1);
        assert_eq!(whole_number("-", 1), 1);
        assert_eq!(whole_number("2.9", 0), 2);
        assert_eq!(whole_number("abc", 7), 7);
        assert_eq!(whole_number("1e3", 0), 1000);
    }

    #[test]
    fn every_icon_the_dialog_draws_is_in_the_embedded_assets() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Regex,
            IconName::RotateCcw,
            IconName::Sparkles,
            IconName::CircleCheck,
            IconName::TriangleAlert,
            IconName::ArrowLeft,
            IconName::ArrowRight,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    // ---- The dialog ----

    /// A fake [`Source`]: remembers each batch it is asked to rename, answers like core would (the inverse
    /// pairs), or fails with `fail`. `details` answers per path, after `gate` opens when there is one.
    #[derive(Clone, Default)]
    struct Fake {
        renames: Arc<Mutex<Vec<Vec<RenamePair>>>>,
        fail: Arc<Mutex<Option<String>>>,
        details: Arc<Mutex<HashMap<String, Media>>>,
        details_asked: Arc<Mutex<usize>>,
        gate: Arc<Mutex<Option<async_channel::Receiver<()>>>>,
    }

    impl Fake {
        fn source(&self) -> Source {
            let (renames, fail) = (self.renames.clone(), self.fail.clone());
            let (details, asked, gate) = (self.details.clone(), self.details_asked.clone(), self.gate.clone());
            Source {
                rename: Arc::new(move |pairs| {
                    let answer = match fail.lock().unwrap().clone() {
                        Some(message) => Err(message),
                        None => Ok(BatchRenameResult {
                            renamed: pairs.len(),
                            undo_ops: pairs
                                .iter()
                                .map(|p| RenamePair {
                                    from: p.to.clone(),
                                    to: p.from.clone(),
                                })
                                .collect(),
                            elapsed_ms: 1.5,
                        }),
                    };
                    renames.lock().unwrap().push(pairs);
                    Box::pin(async move { answer })
                }),
                details: Arc::new(move |paths| {
                    *asked.lock().unwrap() += 1;
                    let known = details.lock().unwrap().clone();
                    let wait = gate.lock().unwrap().clone();
                    Box::pin(async move {
                        if let Some(wait) = wait {
                            let _ = wait.recv().await;
                        }
                        Ok(paths.iter().map(|p| known.get(p).cloned()).collect())
                    })
                }),
            }
        }

        fn batches(&self) -> Vec<Vec<RenamePair>> {
            self.renames.lock().unwrap().clone()
        }
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    /// The explorer Ctrl+Z reaches, on a fake disk, with a fake for the steps core would do.
    fn with_explorer(cx: &mut VisualTestContext) -> (Entity<explorer::Explorer>, Arc<explorer::FakeUndoRunner>) {
        let disk = FakeDisk::new();
        disk.folder("C:\\", Vec::new());
        disk.folder("C:\\Photos", vec![file("C:\\Photos", "Trip 0001.jpg", 1, 1)]);
        let source = disk.source();
        cx.update(|_, cx| {
            explorer::init_with(cx, source);
            let runner = explorer::FakeUndoRunner::install(cx);
            (explorer::explorer(cx), runner)
        })
    }

    /// What Ctrl+Z would take back now.
    fn next_undo(cx: &mut VisualTestContext, explorer: &Entity<explorer::Explorer>) -> Option<String> {
        explorer.read_with(cx, |e, _| e.next_undo().map(|(_, label)| label.to_string()))
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        role_of(cx, label).is_some()
    }

    fn settle(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("{what} never happened");
    }

    fn is_open(cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| window.has_active_dialog(cx))
    }

    fn disabled(cx: &mut VisualTestContext, id: &'static str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(id).disabled().unwrap_or(false)
        })
    }

    fn open_items(
        cx: &mut VisualTestContext,
        fake: &Fake,
        items: Vec<Item>,
        existing: &[&str],
    ) -> Entity<BatchRenameDialog> {
        let source = fake.source();
        let existing: HashSet<String> = existing.iter().map(|s| s.to_string()).collect();
        let view = cx
            .update(|window, cx| open_with(window, cx, items, existing, source))
            .expect("the dialog opens");
        cx.run_until_parked();
        view
    }

    fn model_of(cx: &mut VisualTestContext, view: &Entity<BatchRenameDialog>) -> Entity<BatchRename> {
        view.read_with(cx, |v, _| v.model.clone())
    }

    fn new_names(cx: &mut VisualTestContext, model: &Entity<BatchRename>) -> Vec<String> {
        model.read_with(cx, |m, _| {
            m.simulation().rows.iter().map(|r| r.new_name.clone()).collect()
        })
    }

    fn value_of(cx: &mut VisualTestContext, input: &Entity<InputState>) -> String {
        input.read_with(cx, |i, _| i.value().to_string())
    }

    /// Types into a rules box, as keys would: focus it, then type.
    fn type_into(cx: &mut VisualTestContext, input: &Entity<InputState>, text: &str) {
        cx.update(|window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
            window.render_frame(cx);
            // A box with something in it starts with the caret before it.
            window.press("end", cx);
            window.input(text, cx);
        });
        cx.run_until_parked();
    }

    fn photos() -> Vec<Item> {
        vec![
            item("C:\\Photos", "IMG_0001.jpg"),
            item("C:\\Photos", "IMG_0002.jpg"),
            item("C:\\Photos", "تقرير.pdf"),
        ]
    }

    #[gpui_kit::test]
    fn opens_with_every_current_name_and_the_cards_named(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let _view = open_items(&mut visual, &Fake::default(), photos(), &[]);
        assert!(is_open(&mut visual));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(!window.find_all_by_role(Role::Dialog).is_empty());
        });
        assert_eq!(role_of(&mut visual, "Rename items"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "1. Find and replace"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "2. Pattern and case"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "New names"), Some(Role::List));
        assert_eq!(role_of(&mut visual, "Find text"), Some(Role::TextInput));
        assert_eq!(role_of(&mut visual, "Replace with"), Some(Role::TextInput));
        assert_eq!(role_of(&mut visual, "Pattern"), Some(Role::TextInput));
        assert_eq!(role_of(&mut visual, "Letter case"), Some(Role::RadioGroup));
        // Nothing changes yet: every row is there, the hint says what to do, and Rename is off.
        for name in ["IMG_0001.jpg", "IMG_0002.jpg", "تقرير.pdf"] {
            assert_eq!(
                role_of(&mut visual, &format!("{name} → {name}")),
                Some(Role::ListItem),
                "{name}"
            );
        }
        assert_eq!(
            role_of(
                &mut visual,
                "Modify Find/Replace, Pattern, or Case above to preview changes"
            ),
            Some(Role::Status)
        );
        assert_eq!(role_of(&mut visual, "Rename 0 items"), Some(Role::Button));
        assert!(disabled(&mut visual, "batch-rename-apply"));
        assert_eq!(role_of(&mut visual, "Reset"), Some(Role::Button));
    }

    #[gpui_kit::test]
    fn typing_in_the_boxes_changes_the_preview_live(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let view = open_items(&mut visual, &Fake::default(), photos(), &[]);
        let model = model_of(&mut visual, &view);
        let (find, replace, pattern) =
            view.read_with(&visual, |v, _| (v.find.clone(), v.replace.clone(), v.pattern.clone()));

        type_into(&mut visual, &find, "IMG");
        type_into(&mut visual, &replace, "Trip");
        // Plain text, ignoring case: two rows change.
        assert_eq!(
            role_of(&mut visual, "IMG_0001.jpg → Trip_0001.jpg"),
            Some(Role::ListItem)
        );
        assert_eq!(
            role_of(&mut visual, "IMG_0002.jpg → Trip_0002.jpg"),
            Some(Role::ListItem)
        );
        assert_eq!(
            role_of(
                &mut visual,
                "Ready to rename 2 of 3 items: all of them, or none if one can't be"
            ),
            Some(Role::Status)
        );
        assert_eq!(role_of(&mut visual, "Rename 2 items"), Some(Role::Button));
        assert!(!disabled(&mut visual, "batch-rename-apply"));

        // The pattern box, typed into after its start value: numbering and the Arabic name.
        type_into(&mut visual, &pattern, "_{nn}");
        assert_eq!(
            model.read_with(&visual, |m, _| m.rules().pattern.clone()),
            "{name}_{nn}"
        );
        assert_eq!(
            new_names(&mut visual, &model),
            ["Trip_0001_01.jpg", "Trip_0002_02.jpg", "تقرير_03.pdf"]
        );
        assert_eq!(role_of(&mut visual, "تقرير.pdf → تقرير_03.pdf"), Some(Role::ListItem));
    }

    #[gpui_kit::test]
    fn chips_case_buttons_switches_and_reset_work_the_rules(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let view = open_items(&mut visual, &Fake::default(), photos(), &[]);
        let model = model_of(&mut visual, &view);
        let (pattern, start) = view.read_with(&visual, |v, _| (v.pattern.clone(), v.start.clone()));

        // A token chip adds itself to the end of the pattern and shows in the box.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("batch-rename-token", 4_usize), cx); // {nnn}
        });
        visual.run_until_parked();
        assert_eq!(
            model.read_with(&visual, |m, _| m.rules().pattern.clone()),
            "{name}{nnn}"
        );
        assert_eq!(value_of(&mut visual, &pattern), "{name}{nnn}");
        assert_eq!(
            new_names(&mut visual, &model),
            ["IMG_0001001.jpg", "IMG_0002002.jpg", "تقرير003.pdf"]
        );
        assert_eq!(role_of(&mut visual, "{nnn}: 001, 002…"), Some(Role::Button));

        // The case buttons: snake_case.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("batch-rename-case-mode", CaseMode::Snake as usize), cx);
        });
        visual.run_until_parked();
        assert_eq!(model.read_with(&visual, |m, _| m.rules().case_mode), CaseMode::Snake);
        assert_eq!(new_names(&mut visual, &model)[0], "img_0001001.jpg");

        // Start # is a whole number typed in its box (after the 1 that is there).
        type_into(&mut visual, &start, "9");
        assert_eq!(model.read_with(&visual, |m, _| m.rules().start), 19);

        // The switches: regex words the boxes differently, and match case is a toggle.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("batch-rename-regex", cx);
            window.click("batch-rename-case", cx);
        });
        visual.run_until_parked();
        let rules = model.read_with(&visual, |m, _| m.rules().clone());
        assert!(rules.use_regex && rules.case_sensitive);
        assert_eq!(
            visual.update(|window, cx| {
                window.render_frame(cx);
                window
                    .find_by_label("Find text")
                    .placeholder()
                    .map(str::to_string)
                    .unwrap_or_default()
            }),
            "Regex (e.g. ^IMG_(\\d+))"
        );

        // Reset puts the rules and the boxes back, and keeps the switches.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("batch-rename-reset", cx);
        });
        visual.run_until_parked();
        let rules = model.read_with(&visual, |m, _| m.rules().clone());
        assert!(rules.use_regex && rules.case_sensitive);
        assert_eq!(
            Rules {
                use_regex: false,
                case_sensitive: false,
                ..rules
            },
            Rules::default()
        );
        assert_eq!(value_of(&mut visual, &pattern), "{name}");
        assert_eq!(value_of(&mut visual, &start), "1");
    }

    #[gpui_kit::test]
    fn conflicts_are_flagged_on_their_rows_and_stop_the_rename(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::default();
        let items = vec![item("C:\\a", "a.txt"), item("C:\\a", "b.txt")];
        let view = open_items(&mut visual, &fake, items, &["taken.txt"]);
        let pattern = view.read_with(&visual, |v, _| v.pattern.clone());
        let model = model_of(&mut visual, &view);
        visual.update(|window, cx| pattern.update(cx, |p, cx| p.set_value("", window, cx)));
        type_into(&mut visual, &pattern, "taken");
        assert_eq!(new_names(&mut visual, &model), ["taken.txt", "taken.txt"]);
        assert_eq!(
            role_of(&mut visual, "a.txt → taken.txt: Duplicate target name in batch"),
            Some(Role::ListItem)
        );
        assert_eq!(
            role_of(&mut visual, "2 conflicts detected — resolve before renaming"),
            Some(Role::Status)
        );
        assert!(disabled(&mut visual, "batch-rename-apply"));
        // Enter in the box renames nothing and leaves the dialog for the person to fix it.
        visual.update(|window, cx| window.press("enter", cx));
        visual.run_until_parked();
        assert!(fake.batches().is_empty());
        assert!(is_open(&mut visual));
    }

    #[gpui_kit::test]
    fn a_name_on_disk_and_a_broken_pattern_each_say_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::default();
        // One item to a name that's already in the folder: that conflict, with its own words.
        let items = vec![item("C:\\a", "c.txt")];
        let view = open_items(&mut visual, &fake, items, &["taken.txt"]);
        let (find, pattern, model) =
            view.read_with(&visual, |v, _| (v.find.clone(), v.pattern.clone(), v.model.clone()));
        visual.update(|window, cx| pattern.update(cx, |p, cx| p.set_value("", window, cx)));
        type_into(&mut visual, &pattern, "taken");
        assert_eq!(
            role_of(&mut visual, "c.txt → taken.txt: Already exists in folder"),
            Some(Role::ListItem)
        );
        assert_eq!(
            role_of(&mut visual, "1 conflict detected — resolve before renaming"),
            Some(Role::Status)
        );
        // A broken regex says so and stops the rename too.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("batch-rename-regex", cx);
        });
        type_into(&mut visual, &find, "(");
        let error = model.read_with(&visual, |m, _| m.simulation().regex_error.clone());
        let error = error.expect("the pattern is broken");
        assert_eq!(role_of(&mut visual, &error), Some(Role::Alert));
        assert!(disabled(&mut visual, "batch-rename-apply"));
    }

    #[gpui_kit::test]
    fn renaming_asks_core_for_the_changed_pairs_closes_and_offers_undo(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        visual.update(|_, cx| cx.set_global(LastBatch(None)));
        let (explorer, runner) = with_explorer(&mut visual);
        let fake = Fake::default();
        let view = open_items(&mut visual, &fake, photos(), &[]);
        let (find, replace) = view.read_with(&visual, |v, _| (v.find.clone(), v.replace.clone()));
        type_into(&mut visual, &find, "IMG_");
        type_into(&mut visual, &replace, "Trip ");

        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("batch-rename-apply", cx);
        });
        settle(&mut visual, "the rename", |_| !fake.batches().is_empty());
        let batches = fake.batches();
        assert_eq!(batches.len(), 1);
        let pairs: Vec<(&str, &str)> = batches[0].iter().map(|p| (p.from.as_str(), p.to.as_str())).collect();
        assert_eq!(
            pairs,
            [
                ("C:\\Photos\\IMG_0001.jpg", "C:\\Photos\\Trip 0001.jpg"),
                ("C:\\Photos\\IMG_0002.jpg", "C:\\Photos\\Trip 0002.jpg"),
            ]
        );
        settle(&mut visual, "the dialog closing", |cx| !is_open(cx));
        // The batch is kept for "Undo last batch", and for Ctrl+Z under the TypeScript's label.
        let kept = visual.update(|_, cx| cx.global::<LastBatch>().0.as_ref().map(|a| (a.renamed, a.step)));
        let step = kept.and_then(|(_, step)| step).expect("the batch has a step");
        assert_eq!(kept.map(|(renamed, _)| renamed), Some(2));
        assert_eq!(
            explorer.read_with(&visual, |e, _| e.next_undo().map(|(id, l)| (id, l.to_string()))),
            Some((step, "rename of 2 items".to_string()))
        );
        // The person is told, with an Undo button.
        settle(&mut visual, "the notification", |cx| has(cx, "Undo"));
        assert_eq!(role_of(&mut visual, "Undo"), Some(Role::Button));
        assert!(visual.update(|window, _| !window.find_all_by_role(Role::Alert).is_empty()));

        // Undo asks core for the way back, once, and uses the batch up.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("batch-rename-undo", cx);
        });
        settle(&mut visual, "the undo", |_| runner.renamed().len() == 1);
        let back = &runner.renamed()[0];
        assert_eq!(back[0].from, "C:\\Photos\\Trip 0001.jpg");
        assert_eq!(back[0].to, "C:\\Photos\\IMG_0001.jpg");
        assert_eq!(back.len(), 2);
        // The names went back through the explorer's step, not through the dialog's own source.
        assert_eq!(fake.batches().len(), 1);
        assert_eq!(next_undo(&mut visual, &explorer), None);
        assert!(visual.update(|_, cx| cx.global::<LastBatch>().0.is_none()));
    }

    #[gpui_kit::test]
    fn ctrl_z_takes_the_batch_back_and_withdraws_the_dialogs_undo_last_batch(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        visual.update(|_, cx| cx.set_global(LastBatch(None)));
        let (explorer, runner) = with_explorer(&mut visual);
        // Something else was done first: the batch is the newer step.
        explorer.update(&mut visual, |e, cx| {
            e.remember_undo(
                "copy of 1 item",
                explorer::UndoAction::Recycle(vec!["C:\\Photos\\x".into()]),
                cx,
            );
        });
        let fake = Fake::default();
        let view = open_items(&mut visual, &fake, photos(), &[]);
        let (find, replace) = view.read_with(&visual, |v, _| (v.find.clone(), v.replace.clone()));
        type_into(&mut visual, &find, "IMG_");
        type_into(&mut visual, &replace, "Trip ");
        visual.update(|window, cx| window.click("batch-rename-apply", cx));
        settle(&mut visual, "the dialog closing", |cx| {
            !is_open(cx) && !fake.batches().is_empty()
        });
        assert_eq!(next_undo(&mut visual, &explorer).as_deref(), Some("rename of 2 items"));

        // Ctrl+Z: the batch goes back first, the copy is still there to undo after it.
        let task = explorer.update(&mut visual, |e, cx| e.undo(None, cx));
        visual.foreground_executor().block_test(task);
        visual.run_until_parked();
        assert_eq!(runner.renamed().len(), 1);
        assert_eq!(runner.renamed()[0][0].to, "C:\\Photos\\IMG_0001.jpg");
        assert_eq!(next_undo(&mut visual, &explorer).as_deref(), Some("copy of 1 item"));

        // The dialog no longer offers to undo what Ctrl+Z already took back.
        let _again = open_items(&mut visual, &fake, photos(), &[]);
        assert!(!has(&mut visual, "Undo last batch: 2 items"));
    }

    #[gpui_kit::test]
    fn a_batch_that_changed_nothing_has_no_undo_to_offer(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        visual.update(|_, cx| cx.set_global(LastBatch(None)));
        let (explorer, _runner) = with_explorer(&mut visual);
        let applied = Applied {
            renamed: 0,
            elapsed_ms: 0.0,
            undo_ops: Vec::new(),
            folders: Vec::new(),
            step: None,
        };
        visual.update(|window, cx| renamed(applied, window, cx));
        assert_eq!(next_undo(&mut visual, &explorer), None);
        assert!(visual.update(|_, cx| cx.global::<LastBatch>().0.is_none()));
        assert!(!has(&mut visual, "Undo"));
    }

    #[gpui_kit::test]
    fn enter_in_a_box_renames_like_the_button(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::default();
        let view = open_items(&mut visual, &fake, photos(), &[]);
        let (find, replace) = view.read_with(&visual, |v, _| (v.find.clone(), v.replace.clone()));
        type_into(&mut visual, &find, "IMG");
        type_into(&mut visual, &replace, "Pic");
        visual.update(|window, cx| window.press("enter", cx));
        settle(&mut visual, "the rename", |_| fake.batches().len() == 1);
        assert_eq!(fake.batches()[0].len(), 2);
    }

    #[gpui_kit::test]
    fn a_failure_is_told_and_the_dialog_stays_for_another_try(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        visual.update(|_, cx| cx.set_global(LastBatch(None)));
        let fake = Fake::default();
        *fake.fail.lock().unwrap() = Some("“a.txt” is open in another program".into());
        let items = vec![item("C:\\a", "a.txt")];
        let view = open_items(&mut visual, &fake, items, &[]);
        let (find, replace, model) =
            view.read_with(&visual, |v, _| (v.find.clone(), v.replace.clone(), v.model.clone()));
        type_into(&mut visual, &find, "a");
        type_into(&mut visual, &replace, "b");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("batch-rename-apply", cx);
        });
        settle(&mut visual, "the failure", |cx| {
            cx.update(|window, cx| {
                window.render_frame(cx);
                !window.find_all_by_role(Role::Alert).is_empty()
            })
        });
        // The failure has no Undo to offer.
        assert!(!has(&mut visual, "Undo"));
        assert!(is_open(&mut visual));
        assert!(!model.read_with(&visual, |m, _| m.running()));
        assert!(!disabled(&mut visual, "batch-rename-apply"));
        assert!(visual.update(|_, cx| cx.global::<LastBatch>().0.is_none()));
        // Once it works, the next try goes through.
        *fake.fail.lock().unwrap() = None;
        visual.update(|window, cx| window.click("batch-rename-apply", cx));
        settle(&mut visual, "the second try", |_| fake.batches().len() == 2);
        settle(&mut visual, "the dialog closing", |cx| !is_open(cx));
    }

    #[gpui_kit::test]
    fn the_last_batch_can_be_undone_from_the_dialogs_header(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (explorer, runner) = with_explorer(&mut visual);
        let fake = Fake::default();
        let mut applied = Applied {
            renamed: 3,
            elapsed_ms: 2.0,
            undo_ops: vec![RenamePair {
                from: "C:\\a\\new.txt".into(),
                to: "C:\\a\\old.txt".into(),
            }],
            folders: vec!["C:\\a".into()],
            step: None,
        };
        applied.step = visual.update(|_, cx| remember(&applied, cx));
        visual.update(|_, cx| cx.set_global(LastBatch(Some(applied))));
        let _view = open_items(&mut visual, &fake, photos(), &[]);
        assert_eq!(role_of(&mut visual, "Undo last batch: 3 items"), Some(Role::Button));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("batch-rename-undo-last", cx);
        });
        settle(&mut visual, "the undo", |_| runner.renamed().len() == 1);
        assert_eq!(runner.renamed()[0][0].to.as_str(), "C:\\a\\old.txt");
        assert!(fake.batches().is_empty());
        settle(&mut visual, "the dialog closing", |cx| !is_open(cx));
        assert!(visual.update(|_, cx| cx.global::<LastBatch>().0.is_none()));
        assert_eq!(next_undo(&mut visual, &explorer), None);
    }

    #[gpui_kit::test]
    fn details_are_read_once_when_the_pattern_asks_and_renaming_waits_for_them(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::default();
        let songs = vec![item("C:\\Music", "01.mp3"), item("C:\\Music", "02.mp3")];
        fake.details.lock().unwrap().insert(
            songs[0].path.clone(),
            Media {
                artist: Some("فيروز".into()),
                title: Some("Habaytak".into()),
                track: Some(1),
                ..Media::default()
            },
        );
        let (open_gate, gate) = async_channel::bounded::<()>(1);
        *fake.gate.lock().unwrap() = Some(gate);
        let view = open_items(&mut visual, &fake, songs, &[]);
        let (pattern, model) = view.read_with(&visual, |v, _| (v.pattern.clone(), v.model.clone()));
        // Nothing asks for details yet: nothing is read.
        assert_eq!(*fake.details_asked.lock().unwrap(), 0);
        visual.update(|window, cx| pattern.update(cx, |p, cx| p.set_value("", window, cx)));
        type_into(&mut visual, &pattern, "{track} {artist} - {title}");
        // Held up: the status says so, and Rename waits.
        assert_eq!(*fake.details_asked.lock().unwrap(), 1);
        assert_eq!(role_of(&mut visual, "Reading the files' details…"), Some(Role::Status));
        assert!(model.read_with(&visual, |m, _| m.reading()));
        assert!(disabled(&mut visual, "batch-rename-apply"));
        // Released: the names have the details, and typing more doesn't read again.
        open_gate.try_send(()).unwrap();
        settle(&mut visual, "the details", |cx| {
            !model.read_with(cx, |m, _| m.reading())
        });
        assert_eq!(new_names(&mut visual, &model), ["01 فيروز - Habaytak.mp3", "  - .mp3"]);
        type_into(&mut visual, &pattern, " x");
        assert_eq!(*fake.details_asked.lock().unwrap(), 1);
        assert!(!has(&mut visual, "Reading the files' details…"));
    }

    #[gpui_kit::test]
    fn escape_closes_the_dialog_without_renaming(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::default();
        let _view = open_items(&mut visual, &fake, photos(), &[]);
        assert!(is_open(&mut visual));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        assert!(!is_open(&mut visual));
        assert!(fake.batches().is_empty());
        // Cancel closes it too.
        let _view = open_items(&mut visual, &fake, photos(), &[]);
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("batch-rename-cancel", cx);
        });
        visual.run_until_parked();
        assert!(!is_open(&mut visual));
    }

    #[gpui_kit::test]
    fn nothing_opens_for_no_items(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        visual.update(|window, cx| open(window, cx, Vec::new()));
        visual.run_until_parked();
        assert!(!is_open(&mut visual));
    }

    #[gpui_kit::test]
    fn open_reads_the_items_and_the_other_names_from_the_folder_on_screen(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let disk = FakeDisk::new();
        disk.folder(
            "C:\\a",
            vec![
                folder("C:\\a", "sub"),
                file("C:\\a", "one.txt", 100, 0),
                file("C:\\a", "two.txt", 200, 0),
                file("C:\\a", "تقرير.pdf", 300, 0),
            ],
        );
        visual.update(|_, cx| explorer::init_with(cx, disk.source()));
        let task = visual.update(|_, cx| {
            explorer::explorer(cx).update(cx, |e, cx| {
                e.navigate_task("C:\\a", explorer::NavOptions::default(), cx)
            })
        });
        assert!(visual.foreground_executor().block_test(task));
        visual.run_until_parked();

        // `two.txt` is outside the selection, so its name is taken; the folder is a folder (no extension).
        let paths = vec![
            "C:\\a\\one.txt".to_string(),
            "C:\\a\\sub".to_string(),
            "C:\\a\\تقرير.pdf".to_string(),
        ];
        let (items, existing) = visual.update(|_, cx| gather(&paths, cx));
        assert_eq!(items.len(), 3);
        assert!(items[1].dir && !items[0].dir);
        assert_eq!(items[0].name, "one.txt");
        assert_eq!(existing, HashSet::from(["two.txt".to_string()]));

        visual.update(|window, cx| open(window, cx, paths));
        visual.run_until_parked();
        assert!(is_open(&mut visual));
        assert_eq!(role_of(&mut visual, "sub → sub"), Some(Role::ListItem));
        assert_eq!(role_of(&mut visual, "one.txt → one.txt"), Some(Role::ListItem));
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render_and_keep_clean_labels(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let fake = Fake::default();
        let items = vec![item("C:\\مستندات", "تقرير الربع.pdf"), item("C:\\مستندات", "صورة.png")];
        let view = open_items(&mut visual, &fake, items, &[]);
        let (find, replace, model) =
            view.read_with(&visual, |v, _| (v.find.clone(), v.replace.clone(), v.model.clone()));
        type_into(&mut visual, &find, "تقرير");
        type_into(&mut visual, &replace, "ملخص");
        // Names are exposed as they are, without the layout-only isolates.
        assert_eq!(
            role_of(&mut visual, "تقرير الربع.pdf → ملخص الربع.pdf"),
            Some(Role::ListItem)
        );
        assert_eq!(new_names(&mut visual, &model), ["ملخص الربع.pdf", "صورة.png"]);
        assert_eq!(model.read_with(&visual, |m, _| m.simulation().changed_count), 1);
        assert_eq!(role_of(&mut visual, "Rename 1 item"), Some(Role::Button));
        testing::set_rtl(false);
    }
}
