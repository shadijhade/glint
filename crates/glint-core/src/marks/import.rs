//! Bringing marks in (PLAN.md 7.3, F25, F28–F31): one pipeline for XYplorer's `tag.dat` / `XYplorerTag.dat` and
//! Glint's own export. Read the file, find each item (by the storage mode, then where the index's history says
//! it went), plan what each gets, then apply in batches with the befores in a journal and one Undo step.
//! XYplorer's files are only read (M7).

use super::xy_find;
use crate::host::ipc::Channel;
use glint_model::marks::lines::read_export;
use glint_model::marks::xyplorer::{self, XY_MAX_BYTES};
use glint_model::marks::{default_labels, label_of, tidy, Label, Marks, Meet, XyExtra, MAX_LABELS};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// What the person chose in the preview.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportOptions {
    /// Fill what is empty (the default), or the source's marks replace Glint's.
    pub meet: Meet,
    /// XYplorer's extra columns come in as tags and comment lines (7.4).
    pub extras: bool,
    /// The Glint label each of the source's labels becomes, by the source's name; one not here goes by name,
    /// else becomes a new label (32 at most, then a tag).
    pub labels: Vec<(String, String)>,
    /// XYplorer's own folder, for storage modes 1 and 2 when Glint couldn't tell (asked once in the preview).
    pub base: Option<String>,
    /// Glint's labels as they are now: the settings are the interface's, so it hands them in.
    pub have: Vec<Label>,
}

/// Where a planned item is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum At {
    /// Where the source says.
    Here,
    /// Not there, but the index's history says where it went (`now`).
    Moved { now: String },
    /// On a drive that isn't connected: it waits in the ledger.
    Away,
    /// Not found: it waits, or is left out.
    Missing,
}

/// One item of the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    /// As the source has it, made whole.
    pub path: String,
    pub at: At,
    /// What it gets from the source.
    pub gets: Marks,
    /// What it has in Glint already, when anything.
    pub has: Option<Marks>,
}

/// One of the source's labels and what it becomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelPlan {
    pub name: String,
    /// `0xRRGGBB`.
    pub color: u32,
    /// The Glint label it becomes; empty when it comes in as a tag by its own name (there are 32 labels already,
    /// or the person pointed it at none).
    pub to: String,
    /// `to` is a label Glint doesn't have yet.
    pub new: bool,
    /// It is Glint's label `to` by XYplorer's own color and not by name, and `to` still has the name Glint gave
    /// it: when the import is applied, the interface renames `to` to `name` (7.3, "What an item gets").
    pub takes_name: bool,
}

/// What an import would do, for the preview (F28); applied as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportPlan {
    pub source: String,
    /// The source's format version (0 for a Glint export).
    pub version: u32,
    /// When the source last changed, Unix ms.
    pub changed: i64,
    pub options: ImportOptions,
    pub labels: Vec<LabelPlan>,
    /// XYplorer's extra columns.
    pub extras: Vec<XyExtra>,
    pub items: Vec<Planned>,
    /// Storage 1 or 2 and Glint couldn't tell where XYplorer is: the preview asks for its folder once, and plans
    /// again with `options.base`.
    pub needs_base: bool,
}

/// How far an import is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// What an import did (F29).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub marked: u64,
    /// In the ledger, waiting for their drive or their file.
    pub waiting: u64,
    pub missing: u64,
    /// Items whose own, different comment was kept (fill what is empty).
    pub kept_comments: u64,
    /// Every item's marks before, for the one Undo step.
    pub befores: Vec<(String, Marks)>,
    /// Every item with what happened to it, worded, for "What happened".
    pub items: Vec<(String, String)>,
    /// The journal of befores in Glint's data folder (`imports\<time>.jsonl`), when one was written.
    pub journal: Option<String>,
}

/// Reads `source` and plans the import, finding each item (reading only: nothing is written, anywhere).
pub async fn import_plan(source: String, options: ImportOptions) -> Result<ImportPlan, String> {
    crate::fsx::blocking(move || {
        let history = real_history();
        let env = Env { cap: XY_MAX_BYTES, base_of: &xy_find::base_of, serial_root: &serial_root, history: &history };
        plan(&source, options, &env)
    })
    .await
}

/// What the plan asks of the world, so a test can say where XYplorer is, which drive has a serial and where the
/// index's history says a file went, and can make the size cap small.
struct Env<'a> {
    /// A source bigger than this is refused.
    cap: usize,
    /// XYplorer's own folder, from where its `tag.dat` is (storage 1 and 2, when the person gave none).
    base_of: &'a dyn Fn(&Path) -> Option<PathBuf>,
    /// The root (`D:\`) of the drive with this volume serial (storage 4).
    serial_root: &'a dyn Fn(u32) -> Option<String>,
    /// Where the index's history says the file that was at this path is now.
    history: &'a dyn Fn(&str) -> Option<String>,
}

/// A server that does not answer is given this long, once, however many items are on it.
const SERVER_WAIT: Duration = Duration::from_secs(2);

/// The most the history is asked about one folder after the broad question came back full (see [`real_history`]).
const HISTORY_FOLDERS: usize = 20;

/// XYplorer's own colors for its first seven labels, whatever the language: Red, Orange, Yellow, Green, Blue,
/// Purple, Grey (the places of `default_labels()`).
const XY_COLORS: [u32; 7] = [0xFC7268, 0xF6AB46, 0xEFDC4A, 0xB5D74A, 0x5DA4FE, 0xE29CDC, 0xB5B5B5];

/// The brightness (0 to 255, Rec. 709 weights) above which a color is too pale to tell a row by: a wash of it on
/// the white sheet looks like no label. XYplorer's own palest, Yellow `EFDC4A`, is 214; `DDF4FF` is 240.
const PALE: f64 = 225.0;

/// One item as read, before it is found: its path as written, what it brings (its label is the source's name
/// until the plan's labels are known), and which of the source's labels it has (an index into them).
struct Src {
    path: String,
    marks: Marks,
    label: Option<usize>,
}

/// One of the source's labels: its name, its color, its text's color, and whether XYplorer's own colors may name it.
type SrcLabel = (String, u32, u32, bool);

/// A color's brightness, 0 to 255.
fn luma(color: u32) -> f64 {
    let (r, g, b) = ((color >> 16) & 0xFF, (color >> 8) & 0xFF, color & 0xFF);
    0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)
}

/// The color a new label gets: XYplorer's own (`back`), or its text's color where its own is too pale (see
/// [`PALE`]) and the text's is a color (neither pale nor nearly black: XYplorer's default text is black).
fn tone(back: u32, text: u32) -> u32 {
    if luma(back) > PALE && (40.0..=PALE).contains(&luma(text)) {
        text
    } else {
        back
    }
}

/// A name as compared: trimmed, lower case.
fn fold(name: &str) -> String {
    name.trim().to_lowercase()
}

/// What each of the source's labels (`name`, `back`, `text`, and whether XYplorer's own colors may name it) becomes
/// in Glint (7.3, "What an item gets"), in the source's order.
fn plan_labels(source: &[SrcLabel], options: &ImportOptions) -> Vec<LabelPlan> {
    let have = &options.have;
    let defaults = default_labels();
    let mut made: Vec<String> = Vec::new();
    let mut out: Vec<LabelPlan> = Vec::with_capacity(source.len());
    for (name, back, text, by_color) in source {
        let mut plan = LabelPlan { name: name.clone(), color: *back, to: String::new(), new: false, takes_name: false };
        let pointed = options.labels.iter().find(|(s, _)| !fold(s).is_empty() && fold(s) == fold(name));
        // A label of Glint's by this name, else a new one of this name while there is room, else a tag.
        let mut by_name = |plan: &mut LabelPlan, glint: &str| {
            if let Some((_, l)) = label_of(have, glint) {
                plan.to = l.name.clone();
            } else if made.contains(&fold(glint)) {
                (plan.to, plan.new, plan.color) = (glint.trim().to_string(), true, tone(*back, *text));
            } else if have.len() + made.len() < MAX_LABELS && !fold(glint).is_empty() {
                made.push(fold(glint));
                (plan.to, plan.new, plan.color) = (glint.trim().to_string(), true, tone(*back, *text));
            }
        };
        if let Some((_, glint)) = pointed {
            by_name(&mut plan, glint);
        } else if let Some((_, l)) = label_of(have, name) {
            plan.to = l.name.clone();
        } else if let Some((def, l)) = by_color
            .then(|| XY_COLORS.iter().position(|c| c == back))
            .flatten()
            .and_then(|k| defaults.get(k))
            .and_then(|def| label_of(have, &def.name).map(|(_, l)| (def, l)))
        {
            plan.takes_name = l.name == def.name && !fold(name).is_empty();
            plan.to = l.name.clone();
        } else {
            by_name(&mut plan, name);
        }
        out.push(plan);
    }
    out
}

/// What `raw` could mean in a folder it must not leave: false for a path that names a drive or a server, starts
/// at a root, or goes up with `..` (M10).
fn stays_inside(raw: &str) -> bool {
    !raw.contains(':') && !raw.starts_with(['\\', '/']) && !raw.split(['\\', '/']).any(|part| part == "..")
}

/// A path that names its own drive or server.
fn is_absolute(raw: &str) -> bool {
    let b = raw.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') || raw.starts_with("\\\\")
}

/// `rel` below `base`, with each `..` going up one folder (and stopping at the root). Nothing is read.
fn join_up(base: &Path, rel: &str) -> String {
    let mut out = base.to_path_buf();
    for part in rel.split(['\\', '/']) {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            p => out.push(p),
        }
    }
    out.to_string_lossy().into_owned()
}

/// What the storage line makes of an item's path.
enum Made {
    /// The path, whole.
    Path(String),
    /// The drive it is on is not connected: the path as written.
    Away(String),
    /// Refused or not found, as written.
    Missing(String),
    /// Storage 1 or 2 and nothing says where XYplorer is.
    Unresolved(String),
}

/// What storage 1 to 4 need to make a path whole.
struct Ctx<'a> {
    storage: u32,
    /// The source's folder (storage 3).
    folder: Option<PathBuf>,
    /// XYplorer's own folder (storage 1 and 2).
    base: Option<PathBuf>,
    drives: Vec<char>,
    env: &'a Env<'a>,
}

/// An item's path made whole (7.3, "Finding the items"). Reads only to tell which drive a `?:` path is on.
fn whole(raw: &str, c: &Ctx) -> Made {
    if raw.is_empty() || raw.len() > 32_000 || raw.contains('\0') {
        return Made::Missing(raw.to_string());
    }
    let strip = |s: &str| s.trim_start_matches(['\\', '/']).to_string();
    match c.storage {
        3 => match &c.folder {
            Some(folder) if stays_inside(raw) => Made::Path(folder.join(raw.replace('/', "\\")).to_string_lossy().into_owned()),
            _ => Made::Missing(raw.to_string()),
        },
        4 => match raw.split_once(':') {
            Some((serial, rest)) if !serial.is_empty() && serial.len() <= 8 => {
                match u32::from_str_radix(serial, 16).ok().and_then(|s| (c.env.serial_root)(s)) {
                    Some(root) => Made::Path(format!("{}\\{}", root.trim_end_matches(['\\', '/']), strip(rest))),
                    None if u32::from_str_radix(serial, 16).is_ok() => Made::Away(raw.to_string()),
                    None => Made::Missing(raw.to_string()),
                }
            }
            _ if is_absolute(raw) => Made::Path(raw.to_string()),
            _ => Made::Missing(raw.to_string()),
        },
        1 => match raw.strip_prefix("?:") {
            Some(rest) => {
                let rest = strip(rest);
                let drive = c.base.as_ref().and_then(|b| b.to_string_lossy().chars().next()).filter(char::is_ascii_alphabetic);
                if let Some(d) = drive {
                    return Made::Path(format!("{d}:\\{rest}"));
                }
                let there: Vec<&char> = c.drives.iter().filter(|d| std::fs::symlink_metadata(format!("{d}:\\{rest}")).is_ok()).collect();
                match there.as_slice() {
                    [d] => Made::Path(format!("{d}:\\{rest}")),
                    _ => Made::Unresolved(raw.to_string()),
                }
            }
            None => Made::Path(raw.to_string()),
        },
        2 if is_absolute(raw) => Made::Path(raw.to_string()),
        2 => match &c.base {
            Some(base) => Made::Path(join_up(base, raw)),
            None => Made::Unresolved(raw.to_string()),
        },
        _ => Made::Path(raw.to_string()),
    }
}

/// The local drives' letters (fixed, removable, RAM): where a `?:` path is looked for.
#[cfg(windows)]
fn local_drives() -> Vec<char> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    // SAFETY: reads the bitmask of logical drives.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| (b'A' + i as u8) as char)
        .filter(|letter| {
            let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
            // SAFETY: `root` is NUL-terminated.
            matches!(unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }, 2 | 3 | 6)
        })
        .collect()
}

#[cfg(not(windows))]
fn local_drives() -> Vec<char> {
    Vec::new()
}

#[cfg(windows)]
fn serial_root(serial: u32) -> Option<String> {
    crate::permalinks::win::find_volume_root_by_serial(serial)
}

#[cfg(not(windows))]
fn serial_root(_: u32) -> Option<String> {
    None
}

/// The drive or the server's share a path is on, as a root (`C:\`, `\\server\share\`); None for a path that is on neither.
fn root_of(path: &str) -> Option<String> {
    let p = path.strip_prefix("\\\\?\\").unwrap_or(path);
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return Some(format!("{}:\\", (b[0] as char).to_ascii_uppercase()));
    }
    let mut parts = p.strip_prefix("\\\\")?.split(['\\', '/']).filter(|s| !s.is_empty());
    let (server, share) = (parts.next()?, parts.next()?);
    Some(format!("\\\\{server}\\{share}\\"))
}

/// Which of `roots` answer (lower case): each is asked once, on a thread of its own, and a root that has not
/// answered within `wait` of the start is not there. The threads that are still waiting are left to end by
/// themselves.
fn roots_up(roots: &[String], check: Arc<dyn Fn(&str) -> bool + Send + Sync>, wait: Duration) -> HashSet<String> {
    let (tx, rx) = mpsc::channel();
    let mut asked = 0;
    for root in roots {
        let (tx, root, check) = (tx.clone(), root.clone(), check.clone());
        let started = std::thread::Builder::new().spawn(move || {
            let up = check(&root);
            let _ = tx.send((root, up));
        });
        asked += usize::from(started.is_ok());
    }
    drop(tx);
    let deadline = Instant::now() + wait;
    let mut up = HashSet::new();
    for _ in 0..asked {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else { break };
        match rx.recv_timeout(left) {
            Ok((root, true)) => {
                up.insert(root.to_lowercase());
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    up
}

/// Whether a root is there: no "no disk in the drive" box for an empty card reader.
fn root_answers(root: &str) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::System::Diagnostics::Debug::{SetThreadErrorMode, SEM_FAILCRITICALERRORS};
        // SAFETY: sets this thread's error mode and nothing else.
        let _ = unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, None) };
    }
    std::fs::metadata(root).is_ok()
}

/// Asks the index's history where files went (7.3, "Moved since"): one request for everything of the last 30
/// days (10,000 events at most); only when that came back full, one more for the folder of each path it did not
/// answer, 20 folders at most. Nothing when the service is not this version or does not answer.
fn real_history() -> impl Fn(&str) -> Option<String> {
    #[derive(Default)]
    struct Asked {
        known: Option<HashMap<String, String>>,
        full: bool,
        folders: HashSet<String>,
    }
    let state = std::sync::Mutex::new(Asked::default());
    move |old: &str| {
        #[cfg(windows)]
        {
            use shadui_index::proto::HistoryRequest;
            if !crate::find::same_service() {
                return None;
            }
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .and_then(|d| i64::try_from(d.as_millis()).ok())
                .unwrap_or(0);
            let ask = |query: &str, into: &mut HashMap<String, String>| -> Option<bool> {
                let request = HistoryRequest { query: query.to_string(), since: now_ms - 30 * 86_400_000, max: 10_000 };
                let answer = shadui_index::client::history(&request).ok()?;
                // Newest first: the first event for a path says where it went last.
                for e in answer.events {
                    if let Some(now) = e.now.filter(|n| *n != e.path) {
                        into.entry(e.path.to_lowercase()).or_insert(now);
                    }
                }
                Some(answer.limited)
            };
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            if s.known.is_none() {
                let mut known = HashMap::new();
                s.full = ask("", &mut known).unwrap_or(false);
                s.known = Some(known);
            }
            let key = old.to_lowercase();
            if let Some(now) = s.known.as_ref().and_then(|k| k.get(&key)) {
                return Some(now.clone());
            }
            let folder = key.rsplit_once('\\').map(|(f, _)| f.to_string())?;
            if !s.full || s.folders.len() >= HISTORY_FOLDERS || !s.folders.insert(folder.clone()) {
                return None;
            }
            let mut more = HashMap::new();
            ask(&folder, &mut more);
            let found = more.get(&key).cloned();
            if let Some(known) = s.known.as_mut() {
                known.extend(more);
            }
            found
        }
        #[cfg(not(windows))]
        {
            let _ = (old, &state);
            None
        }
    }
}

/// Whether a file's bytes are a Glint export (its first line is `{"glint-export":…`), with or without a UTF-8 mark.
fn is_glint_export(bytes: &[u8]) -> bool {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes).starts_with(b"{\"glint-export\"")
}

/// Reads `source` and plans what importing it would do. Reads only.
fn plan(source: &str, options: ImportOptions, env: &Env) -> Result<ImportPlan, String> {
    let path = Path::new(source);
    let bytes = xy_find::read_shared(path, env.cap).map_err(|e| format!("Couldn't read this file: {e}"))?;
    if bytes.len() > env.cap {
        return Err("This file is too big to be a tag database (64 MB at most)".into());
    }
    let mut plan = ImportPlan { source: source.to_string(), changed: xy_find::modified_ms(path), ..ImportPlan::default() };

    // 1. Read: the source's labels, its items as written, how its paths are written.
    let (labels, items, storage): (Vec<SrcLabel>, Vec<Src>, u32) = if is_glint_export(&bytes) {
        let text = String::from_utf8_lossy(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes)).into_owned();
        let export = read_export(&text)?;
        let labels = export.labels.iter().map(|l| (l.name.clone(), l.color, l.color, false)).collect();
        let items = export
            .lines
            .iter()
            .map(|line| Src {
                path: line.name.clone(),
                marks: line.marks.clone(),
                label: line.marks.label.as_deref().and_then(|n| label_of(&export.labels, n)).map(|(i, _)| i),
            })
            .collect();
        (labels, items, 3)
    } else {
        let file = xyplorer::read(&bytes)?;
        plan.version = file.version;
        plan.extras = file.extras.clone();
        let labels = file.labels.iter().map(|l| (l.name.clone(), l.back, l.text, true)).collect();
        let items = file
            .items
            .iter()
            .map(|i| Src {
                path: i.path.clone(),
                marks: xyplorer::marks_of(&file, i, options.extras),
                label: (i.label as usize).checked_sub(1).filter(|k| *k < file.labels.len()),
            })
            .collect();
        (labels, items, file.storage)
    };

    // 2. Labels.
    plan.labels = plan_labels(&labels, &options);

    // 3. Each path made whole.
    let base = (storage == 1 || storage == 2)
        .then(|| {
            let given = options.base.as_deref().map(str::trim).filter(|b| !b.is_empty()).map(PathBuf::from);
            given.or_else(|| (env.base_of)(path))
        })
        .flatten();
    let ctx = Ctx { storage, folder: path.parent().map(Path::to_path_buf), base, drives: local_drives(), env };
    let made: Vec<Made> = items.iter().map(|i| whole(&i.path, &ctx)).collect();
    plan.needs_base = made.iter().any(|m| matches!(m, Made::Unresolved(_)));

    // 4. Where each is: the roots asked once, then one call an item.
    let mut roots: Vec<String> = Vec::new();
    for m in &made {
        if let Some(root) = (if let Made::Path(p) = m { root_of(p) } else { None }).filter(|r| !roots.contains(r)) {
            roots.push(root);
        }
    }
    let up = roots_up(&roots, Arc::new(root_answers), SERVER_WAIT);
    let found: Vec<(String, At)> = made
        .into_iter()
        .map(|m| match m {
            Made::Path(p) => match root_of(&p) {
                Some(r) if up.contains(&r.to_lowercase()) => {
                    if std::fs::symlink_metadata(&p).is_ok() {
                        return (p, At::Here);
                    }
                    let moved = (env.history)(&p).filter(|now| std::fs::symlink_metadata(now).is_ok());
                    match moved {
                        Some(now) => (p, At::Moved { now }),
                        None => (p, At::Missing),
                    }
                }
                Some(_) => (p, At::Away),
                None => (p, At::Missing),
            },
            Made::Away(p) => (p, At::Away),
            Made::Missing(p) | Made::Unresolved(p) => (p, At::Missing),
        })
        .collect();

    // 5. What each gets, and what it has now where it is.
    let here: Vec<String> = found
        .iter()
        .filter_map(|(p, at)| match at {
            At::Here => Some(p.clone()),
            At::Moved { now } => Some(now.clone()),
            _ => None,
        })
        .collect();
    let mut has = super::real(|w| super::read_all(&here, w)).into_iter();
    for (src, (path, at)) in items.into_iter().zip(found) {
        let mut marks = src.marks;
        marks.label = match src.label.and_then(|i| plan.labels.get(i)) {
            Some(l) if l.to.is_empty() => {
                if !l.name.trim().is_empty() {
                    marks.tags.push(l.name.clone());
                }
                None
            }
            Some(l) => Some(l.to.clone()),
            // An export's label that its first line does not list: Glint's by that name, else as written.
            None => marks.label.map(|n| label_of(&options.have, &n).map_or(n, |(_, l)| l.name.clone())),
        };
        let now_has = matches!(at, At::Here | At::Moved { .. }).then(|| has.next()).flatten().filter(|m| !m.is_empty());
        plan.items.push(Planned { path, at, gets: tidy(&marks), has: now_has });
    }
    plan.options = options;
    Ok(plan)
}

/// [`import_plan`]'s work for another module's test: no index history, no XYplorer folder, no drive by serial.
#[cfg(test)]
pub(super) fn plan_here(source: &Path, options: ImportOptions) -> Result<ImportPlan, String> {
    let env = Env { cap: XY_MAX_BYTES, base_of: &|_| None, serial_root: &|_| None, history: &|_| None };
    plan(&source.to_string_lossy(), options, &env)
}

/// Applies `plan` in batches on background threads, saying how far it is.
pub async fn import_apply(plan: ImportPlan, progress: Channel<Progress>) -> Result<ImportReport, String> {
    crate::fsx::blocking(move || super::apply::apply(plan, &progress)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::marks::lines::{write_export, Line};
    use glint_model::marks::Export;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/../../tests/fixtures/marks/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap_or_default()
    }

    /// A folder of the test's own, empty.
    fn folder() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("glint-import-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn put(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = name.split('/').fold(dir.to_path_buf(), |p, part| p.join(part));
        let _ = std::fs::create_dir_all(p.parent().unwrap_or(dir));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn text(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn no_history(_: &str) -> Option<String> {
        None
    }

    /// The plan with no XYplorer known, no drive with a serial, and the history this test gives.
    fn plan_with(source: &Path, options: ImportOptions, cap: usize, history: &dyn Fn(&str) -> Option<String>) -> Result<ImportPlan, String> {
        let env = Env { cap, base_of: &|_| None, serial_root: &|_| None, history };
        plan(&text(source), options, &env)
    }

    fn plan_of(source: &Path, options: ImportOptions) -> Result<ImportPlan, String> {
        plan_with(source, options, XY_MAX_BYTES, &no_history)
    }

    fn with_have(have: Vec<Label>) -> ImportOptions {
        ImportOptions { have, ..ImportOptions::default() }
    }

    fn defaults() -> ImportOptions {
        with_have(default_labels())
    }

    /// A version 5 file with these labels (name, color) and items (path, label, tags), storage 0.
    fn tag_file(dir: &Path, labels: &[(&str, u32)], items: &[(&str, u32, &str)]) -> PathBuf {
        let names: Vec<String> = labels.iter().map(|(n, c)| format!("{n}|000000|{c:06X}")).collect();
        let mut out = format!("XYplorer File Tags v5\n\nLabels:\n{}\n\nExtra Tags:\n\nStorage: 0\n\nData:\n", names.join(";"));
        for (path, label, tags) in items {
            out += &format!("{path}|{label}|{tags}|\n");
        }
        put(dir, "tag.dat", out.as_bytes())
    }

    fn by_path<'a>(plan: &'a ImportPlan, end: &str) -> &'a Planned {
        plan.items.iter().find(|i| i.path.ends_with(end)).unwrap()
    }

    #[test]
    fn every_fixture_plans_and_storage_0_is_taken_as_written() {
        let dir = folder();
        for name in ["v5-storage0-bom-crlf.dat", "real-v5-zh.dat", "v3-storage0.dat", "v5-storage1-nobom-lf.dat", "v5-storage2.dat", "v5-storage4.dat", "XYplorerTag-storage3.dat"] {
            let file = put(&dir, name, &fixture(name));
            assert!(plan_of(&file, defaults()).is_ok(), "{name}");
        }
        let p = plan_of(&dir.join("v5-storage0-bom-crlf.dat"), defaults()).unwrap();
        assert_eq!((p.version, p.items.len(), p.needs_base), (5, 7, false));
        assert_eq!(p.items[0].path, "C:\\ops\\bond0.conf");
        assert_eq!(p.extras.len(), 16);
        assert_eq!(plan_of(&dir.join("v3-storage0.dat"), defaults()).unwrap().version, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn storage_3_joins_the_sources_folder_and_finds_both_files() {
        let dir = folder();
        let file = put(&dir, "XYplorerTag.dat", &fixture("XYplorerTag-storage3.dat"));
        put(&dir, "bond0.conf", b"x");
        put(&dir, "sub/notes.md", b"y");
        let p = plan_of(&file, defaults()).unwrap();
        assert_eq!(p.items.len(), 2);
        assert!(p.items.iter().all(|i| i.at == At::Here), "{:?}", p.items);
        assert_eq!(Path::new(&p.items[0].path), dir.join("bond0.conf"));
        assert_eq!(Path::new(&p.items[1].path), dir.join("sub").join("notes.md"));
        assert_eq!(p.changed, xy_find::modified_ms(&file));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn storage_4_with_a_serial_no_drive_has_is_away_and_one_that_is_there_is_whole() {
        let dir = folder();
        let file = put(&dir, "t.dat", &fixture("v5-storage4.dat"));
        let p = plan_of(&file, defaults()).unwrap();
        assert!(p.items.iter().all(|i| i.at == At::Away), "{:?}", p.items);
        assert_eq!(p.items[0].path, "B2D60DFE:\\ops\\bond0.conf");
        let drive = text(&dir).chars().next().unwrap_or('C');
        let root = format!("{drive}:\\");
        let env = Env { cap: XY_MAX_BYTES, base_of: &|_| None, serial_root: &|s| (s == 0xB2D60DFE).then(|| root.clone()), history: &no_history };
        let p = plan(&text(&file), defaults(), &env).unwrap();
        assert_eq!(p.items[0].path, format!("{drive}:\\ops\\bond0.conf"));
        assert_eq!(p.items[1].at, At::Away);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn storage_1_and_2_with_no_base_ask_for_one_and_with_it_find_the_files() {
        let dir = folder();
        for name in ["v5-storage1-nobom-lf.dat", "v5-storage2.dat"] {
            let p = plan_of(&put(&dir, name, &fixture(name)), defaults()).unwrap();
            assert!(p.needs_base, "{name}");
            assert!(p.items.iter().all(|i| i.at == At::Missing), "{name}");
        }
        // Storage 2: relative to XYplorer's folder, \`..\` going up.
        let base = dir.join("a").join("b");
        put(&dir, "ops/bond0.conf", b"x");
        put(&dir, "a/data/notes.md", b"y");
        let options = ImportOptions { base: Some(text(&base)), ..defaults() };
        let p = plan_of(&dir.join("v5-storage2.dat"), options).unwrap();
        assert!(!p.needs_base);
        assert!(p.items.iter().all(|i| i.at == At::Here), "{:?}", p.items);
        assert_eq!(Path::new(&p.items[0].path), dir.join("ops").join("bond0.conf"));
        // Storage 1: XYplorer's drive.
        let drive = text(&dir).chars().next().unwrap_or('C');
        let options = ImportOptions { base: Some(text(&dir)), ..defaults() };
        let p = plan_of(&dir.join("v5-storage1-nobom-lf.dat"), options).unwrap();
        assert!(!p.needs_base);
        assert_eq!(p.items[0].path, format!("{drive}:\\ops\\bond0.conf"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_xyplorer_wrote_in_chinese_meets_the_default_labels_by_its_own_colors() {
        let dir = folder();
        let file = put(&dir, "tag.dat", &fixture("real-v5-zh.dat"));
        let p = plan_of(&file, defaults()).unwrap();
        let first_six: Vec<(&str, bool)> = p.labels[..6].iter().map(|l| (l.to.as_str(), l.takes_name)).collect();
        assert_eq!(first_six, [("Red", true), ("Orange", true), ("Yellow", true), ("Green", true), ("Blue", true), ("Purple", true)]);
        assert_eq!((p.labels[0].name.as_str(), p.labels[0].new, p.labels[0].color), ("红色", false, 0xFC7268));
        let sky = &p.labels[6];
        assert_eq!((sky.name.as_str(), sky.to.as_str(), sky.new, sky.takes_name, sky.color), ("天蓝色", "天蓝色", true, false, 0x0969DA));
        assert_eq!(by_path(&p, "$Recycle.Bin").gets.label.as_deref(), Some("Red"));
        assert_eq!(by_path(&p, "$Recycle.Bin").gets.comment.as_deref(), Some("comment1\nlala"));
        // Red renamed: its old name is in was, and the person's word stays.
        let mut have = default_labels();
        have[0] = Label { name: "Important".into(), color: have[0].color, was: vec!["Red".into()] };
        let p = plan_of(&file, with_have(have)).unwrap();
        assert_eq!((p.labels[0].to.as_str(), p.labels[0].takes_name), ("Important", false));
        assert_eq!(by_path(&p, "$Recycle.Bin").gets.label.as_deref(), Some("Important"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn labels_go_by_name_in_any_case_with_grey_for_gray_and_old_names() {
        let dir = folder();
        let file = tag_file(&dir, &[("Grey", 0x123456), ("rEd", 0x654321), ("Mine", 0x111111)], &[("C:\\a", 1, ""), ("C:\\b", 2, ""), ("C:\\c", 3, "")]);
        let p = plan_of(&file, defaults()).unwrap();
        assert_eq!((p.labels[0].to.as_str(), p.labels[0].new, p.labels[0].takes_name), ("Gray", false, false));
        assert_eq!(p.labels[1].to, "Red");
        assert_eq!((p.labels[2].to.as_str(), p.labels[2].new, p.labels[2].color), ("Mine", true, 0x111111));
        let mut have = default_labels();
        have[0] = Label { name: "Important".into(), color: 0xef4444, was: vec!["Red".into()] };
        let p = plan_of(&file, with_have(have.clone())).unwrap();
        assert_eq!(p.labels[1].to, "Important");
        // A pointing by the person wins over the name.
        let options = ImportOptions { labels: vec![("rEd".into(), "Blue".into()), ("Mine".into(), "Orange".into())], ..with_have(have) };
        let p = plan_of(&file, options).unwrap();
        assert_eq!((p.labels[1].to.as_str(), p.labels[1].new), ("Blue", false));
        assert_eq!((p.labels[2].to.as_str(), p.labels[2].new), ("Orange", false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_label_pointed_at_none_becomes_a_tag_and_so_does_one_past_32() {
        let dir = folder();
        let file = tag_file(&dir, &[("Mine", 0x111111)], &[("C:\\a", 1, "x")]);
        let options = ImportOptions { labels: vec![("Mine".into(), String::new())], ..defaults() };
        let p = plan_of(&file, options).unwrap();
        assert_eq!((p.labels[0].to.as_str(), p.labels[0].new), ("", false));
        assert_eq!((p.items[0].gets.label.clone(), p.items[0].gets.tags.clone()), (None, vec!["x".to_string(), "Mine".to_string()]));
        let full: Vec<Label> = (0..32).map(|i| Label { name: format!("L{i}"), color: 0x808080, was: Vec::new() }).collect();
        let p = plan_of(&file, with_have(full)).unwrap();
        assert_eq!((p.labels[0].to.as_str(), p.labels[0].new), ("", false));
        assert_eq!(p.items[0].gets.tags, ["x", "Mine"]);
        let some: Vec<Label> = (0..31).map(|i| Label { name: format!("L{i}"), color: 0x808080, was: Vec::new() }).collect();
        let two = tag_file(&dir, &[("One", 0x111111), ("Two", 0x222222)], &[("C:\\a", 2, "")]);
        let p = plan_of(&two, with_have(some)).unwrap();
        assert_eq!((p.labels[0].new, p.labels[1].new, p.labels[1].to.as_str()), (true, false, ""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pale_color_is_told_by_its_text_and_yellow_is_not_pale() {
        assert_eq!(tone(0xDDF4FF, 0x0969DA), 0x0969DA);
        assert_eq!(tone(0xDDF4FF, 0x000000), 0xDDF4FF);
        assert_eq!(tone(0xDDF4FF, 0xFFFFFF), 0xDDF4FF);
        assert_eq!(tone(0xEFDC4A, 0x0969DA), 0xEFDC4A);
        assert_eq!(tone(0xFC7268, 0x000000), 0xFC7268);
    }

    #[test]
    fn what_is_there_is_here_with_what_it_has_what_moved_is_found_and_the_rest_is_missing() {
        let dir = folder();
        let a = put(&dir, "a.txt", b"x");
        put(&dir, "new/b.txt", b"y");
        let (old_b, gone) = (text(&dir.join("old").join("b.txt")), text(&dir.join("nowhere.txt")));
        let file = tag_file(&dir, &[("Red", 0xFC7268)], &[(&text(&a), 1, "t"), (&old_b, 0, "m"), (&gone, 0, "n"), ("Z:\\no\\drive.txt", 0, "d")]);
        crate::host::async_runtime::block_on(super::super::marks_set(vec![text(&a)], glint_model::marks::Change::Label(Some("Blue".into())))).unwrap();
        let new_b = text(&dir.join("new").join("b.txt"));
        let history = |p: &str| (p == old_b).then(|| new_b.clone()).or_else(|| (p == gone).then(|| text(&dir.join("still-nowhere"))));
        let p = plan_with(&file, defaults(), XY_MAX_BYTES, &history).unwrap();
        assert_eq!(p.items[0].at, At::Here);
        assert_eq!(p.items[0].has.as_ref().and_then(|m| m.label.as_deref()), Some("Blue"));
        assert_eq!(p.items[1].at, At::Moved { now: new_b });
        assert_eq!(p.items[1].path, old_b);
        assert_eq!(p.items[1].has, None);
        assert_eq!(p.items[2].at, At::Missing);
        let zed = text(&dir).starts_with(['Z', 'z']);
        assert_eq!(p.items[3].at, if zed { At::Missing } else { At::Away });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_that_leaves_the_folder_is_missing_and_nothing_outside_is_looked_at() {
        let dir = folder();
        put(&dir, "x", b"outside");
        put(&dir, "src/inside.txt", b"in");
        let body = "XYplorer File Tags v5\n\nLabels:\nRed|000000|FC7268\n\nExtra Tags:\n\nStorage: 3\n\nData:\n..\\x|1||\nsub\\..\\..\\x|1||\n\\x|1||\nC:\\Windows|1||\ninside.txt|1||\n";
        let file = put(&dir, "src/t.dat", body.as_bytes());
        let looked = std::cell::RefCell::new(Vec::new());
        let history = |p: &str| {
            looked.borrow_mut().push(p.to_string());
            None
        };
        let p = plan_with(&file, defaults(), XY_MAX_BYTES, &history).unwrap();
        let ats: Vec<&At> = p.items.iter().map(|i| &i.at).collect();
        assert_eq!(ats, [&At::Missing, &At::Missing, &At::Missing, &At::Missing, &At::Here]);
        assert_eq!(p.items[0].path, "..\\x");
        assert!(looked.borrow().is_empty(), "{:?}", looked.borrow());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_glint_export_plans_as_version_0_with_its_labels_and_its_paths_under_its_folder() {
        let dir = folder();
        put(&dir, "a.txt", b"x");
        put(&dir, "sub/b.txt", b"y");
        let line = |name: &str, label: &str, tag: &str| Line {
            name: name.into(),
            marks: Marks { label: Some(label.into()), tags: vec![tag.into()], comment: None },
            ..Line::default()
        };
        let export = Export {
            labels: vec![Label { name: "Red".into(), color: 0xff0000, was: Vec::new() }, Label { name: "Mine".into(), color: 0x112233, was: Vec::new() }],
            lines: vec![line("a.txt", "Mine", "one"), line("sub\\b.txt", "red", "two"), line("c.txt", "Unlisted", "three")],
        };
        let file = put(&dir, "all.glint-marks", write_export(&export).as_bytes());
        let p = plan_of(&file, defaults()).unwrap();
        assert_eq!((p.version, p.labels.len()), (0, 2));
        assert_eq!((p.labels[0].to.as_str(), p.labels[0].new), ("Red", false));
        assert_eq!((p.labels[1].to.as_str(), p.labels[1].new, p.labels[1].color), ("Mine", true, 0x112233));
        // An export's lines are in name order.
        assert_eq!(Path::new(&p.items[0].path), dir.join("a.txt"));
        assert_eq!((p.items[0].at.clone(), p.items[0].gets.label.as_deref()), (At::Here, Some("Mine")));
        assert_eq!((p.items[1].at.clone(), p.items[1].gets.label.as_deref()), (At::Missing, Some("Unlisted")));
        assert_eq!((p.items[2].at.clone(), p.items[2].gets.label.as_deref()), (At::Here, Some("Red")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extra_columns_come_in_only_when_asked_for() {
        let dir = folder();
        let file = put(&dir, "t.dat", &fixture("v5-storage0-bom-crlf.dat"));
        let p = plan_of(&file, ImportOptions { extras: true, ..defaults() }).unwrap();
        let review = by_path(&p, "review.docx");
        assert_eq!(review.gets.tags, ["Status: In review", "Stars: 4", "Reviewed"]);
        assert_eq!(review.gets.comment.as_deref(), Some("Due: 2026-11-30"));
        assert_eq!(review.gets.label.as_deref(), Some("Blue"));
        let p = plan_of(&file, defaults()).unwrap();
        let review = by_path(&p, "review.docx");
        assert_eq!((review.gets.tags.len(), review.gets.comment.clone()), (0, None));
        assert_eq!(by_path(&p, "past.txt").gets.label, None);
        assert_eq!(by_path(&p, "archive").gets.label.as_deref(), Some("Gray"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn damaged_files_do_not_panic() {
        let dir = folder();
        let head = "XYplorer File Tags v5\n\nLabels:\nRed|000000|FC7268\n\nExtra Tags:\n\nStorage: 0\n\nData:\n";
        let bars = "|".repeat(1000);
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("one byte", vec![b'X']),
            ("cut line", format!("{head}C:\\a|1|t|c\nC:\\b|9").into_bytes()),
            ("bars", format!("{head}{bars}\nC:\\a|1{bars}\n").into_bytes()),
            ("huge label number", format!("{head}C:\\a|4294967295|t|c\nC:\\b|99999999999999999999|t|c\n").into_bytes()),
            ("nul in a path", format!("{head}C:\\a\0b|1||\n").into_bytes()),
            ("long path", format!("{head}C:\\{}|1||\n", "a".repeat(100_000)).into_bytes()),
            ("garbage", (0..=255u8).cycle().take(5000).collect()),
        ];
        for (what, bytes) in cases {
            let file = put(&dir, "d.dat", &bytes);
            let r = plan_of(&file, defaults());
            match what {
                "empty" | "one byte" | "garbage" => assert!(r.is_err(), "{what}"),
                "cut line" => assert_eq!(r.unwrap().items.len(), 2),
                "huge label number" => assert!(r.unwrap().items.iter().all(|i| i.gets.label.is_none())),
                "nul in a path" | "long path" => assert_eq!(r.unwrap().items[0].at, At::Missing),
                _ => assert!(r.is_ok(), "{what}"),
            }
        }
        // Over the cap: refused with words, and read no further than one byte past it.
        let big = put(&dir, "big.dat", &[b'x'; 5000]);
        let e = plan_with(&big, defaults(), 1000, &no_history).unwrap_err();
        assert!(e.contains("too big") && e.contains("64 MB"), "{e}");
        assert!(plan_of(&dir.join("missing.dat"), defaults()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_root_that_does_not_answer_is_given_the_wait_once_and_the_others_are_not_held_up() {
        let check: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(|root: &str| {
            if root.starts_with("\\\\slow") {
                std::thread::sleep(Duration::from_secs(3));
            }
            !root.starts_with("\\\\down")
        });
        let roots: Vec<String> = ["C:\\", "\\\\slow\\share\\", "\\\\down\\share\\", "D:\\"].map(String::from).to_vec();
        let started = Instant::now();
        let up = roots_up(&roots, check, Duration::from_millis(300));
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        assert_eq!(up, HashSet::from(["c:\\".to_string(), "d:\\".to_string()]));
    }

    #[test]
    fn a_path_names_its_drive_or_its_servers_share() {
        assert_eq!(root_of("c:\\a\\b").as_deref(), Some("C:\\"));
        assert_eq!(root_of("\\\\?\\D:\\x").as_deref(), Some("D:\\"));
        assert_eq!(root_of("\\\\nas\\media\\films\\a.mkv").as_deref(), Some("\\\\nas\\media\\"));
        for no in ["relative\\x", "\\\\nas", "", "C", "..\\x"] {
            assert_eq!(root_of(no), None, "{no:?}");
        }
    }

    /// 10,000 lines over 10,000 small files: what reading and finding cost. Prints the milliseconds.
    #[test]
    #[ignore]
    fn ten_thousand_lines_over_ten_thousand_files_cost() {
        let dir = folder();
        let mut body = String::from("XYplorer File Tags v5\r\n\r\nLabels:\r\nRed|000000|FC7268;Blue|000000|5DA4FE\r\n\r\nExtra Tags:\r\n\r\nStorage: 0\r\n\r\nData:\r\n");
        for i in 0..10_000 {
            let path = dir.join("files").join(format!("f{i}.txt"));
            put(&dir, &format!("files/f{i}.txt"), b"x");
            body += &format!("{}|{}|tag{}, shared|comment {i}\r\n", text(&path), 1 + i % 2, i % 7);
        }
        let bytes: Vec<u8> = [0xFF, 0xFE].into_iter().chain(body.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        let file = put(&dir, "tag.dat", &bytes);
        let started = Instant::now();
        let read = xyplorer::read(&std::fs::read(&file).unwrap()).unwrap();
        let reading = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        let p = plan_of(&file, defaults()).unwrap();
        let planning = started.elapsed().as_secs_f64() * 1000.0;
        println!("{} lines: reading {reading:.1} ms; the whole plan (reading, finding each, what each has) {planning:.1} ms", read.items.len());
        assert_eq!(p.items.iter().filter(|i| i.at == At::Here).count(), 10_000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
