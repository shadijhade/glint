//! Applying an import's plan (PLAN.md 7.3, F29 to F31): in batches through the homes, the befores in a journal
//! (`imports\<time>.jsonl` in Glint's data folder) and in the report for one Undo step; what was applied
//! remembered item by item (`xyplorer.json`), which is what running an import twice and following XYplorer
//! compare with.
//!
//! Everything that touches Glint's data folder takes the folder as an argument in a private function, so a test
//! uses a folder of its own; the public functions pass `tools::data_dir()`.
//!
//! `xyplorer.json`: `{"sources":{"<source path, lower case>":{"path","modified"(ms),"size","at"(ms applied),"xy",
//! "options":{meet,extras,labels,base,have},"items":{"<item path, lower case>":{"hash","at"}}}}}`.
//! A journal: `{"glint-import":1,"source":…,"at":…}`, then one ledger-format line an item that changed, holding its
//! marks before (a line with `"none":true` where it had none).

use super::import::{At, ImportOptions, ImportPlan, ImportReport, LabelPlan, Planned, Progress};
use super::{read_all, real, work, World};
use crate::fsx::blocking;
use crate::host::ipc::Channel;
use glint_model::marks::{
    label_of, meet, read_ledger, tidy, write_ledger, Change, Label, Line, Marks, Meet, Outcome, Why, MAX_LABELS,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Items a batch holds: one write a folder, one ledger save and one progress message each.
const BATCH: usize = 200;
/// A journal is kept this long.
const KEEP_DAYS: u64 = 30;
/// More of a journal or of `xyplorer.json` is never read.
const MAX_READ: u64 = 64 << 20;

/// The last import that can still be taken back (F29: Settings → Files → Marks).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LastImport {
    /// Its journal of befores.
    pub journal: String,
    /// The file it read.
    pub source: String,
    /// When it ran, Unix ms.
    pub at: i64,
    /// How many items it changed.
    pub items: u64,
    /// Glint's labels as they were before it (None for a journal written before the labels were kept).
    pub labels: Option<Vec<Label>>,
    /// The labels as the import left them: Undo and take back put `labels` back only while Settings still have these.
    pub made: Option<Vec<Label>>,
}

/// What following XYplorer brought in (F30).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Followed {
    /// The `tag.dat` followed.
    pub source: String,
    /// Items whose marks came in, new or changed.
    pub came: u64,
    /// Items whose line is gone from XYplorer and whose marks were taken off.
    pub gone: u64,
    /// Every item it changed, for the rows that show them.
    pub paths: Vec<String>,
    /// The plan's labels: the interface adds a new one to Settings (`apply_labels`).
    pub labels: Vec<LabelPlan>,
}

/// What `xyplorer.json` remembers: each source that was brought in, and what each item was given.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Memory {
    sources: BTreeMap<String, Source>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Source {
    path: String,
    /// The source's modified time (Unix ms) and size when it was read.
    modified: i64,
    size: u64,
    /// When it was applied, Unix ms.
    at: i64,
    /// An XYplorer file (a Glint export isn't followed).
    xy: bool,
    options: Opts,
    items: BTreeMap<String, Item>,
}

/// The options an import ran with (what following plans again with).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Opts {
    meet: Meet,
    extras: bool,
    labels: Vec<(String, String)>,
    base: Option<String>,
    have: Vec<Label>,
}

/// One item as it was given: a hash of the marks, and the path they were given at.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Item {
    hash: String,
    at: String,
}

fn lower(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

/// Blake3 of the marks (tidied), cut to 128 bits: enough to tell two sets of marks apart.
fn hash_of(marks: &Marks) -> String {
    let m = tidy(marks);
    let mut h = blake3::Hasher::new();
    h.update(m.label.as_deref().unwrap_or("").as_bytes());
    h.update(&[0]);
    for t in &m.tags {
        h.update(t.as_bytes());
        h.update(&[0]);
    }
    h.update(&[1]);
    h.update(m.comment.as_deref().unwrap_or("").as_bytes());
    h.finalize().to_hex().as_str().chars().take(32).collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// A file's modified time in Unix ms.
fn stamp(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

fn read_capped(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(MAX_READ).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// `xyplorer.json`; a damaged one reads as empty.
fn load(data: &Path) -> Memory {
    read_capped(&data.join("xyplorer.json")).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(data: &Path, memory: &Memory) {
    let _ = std::fs::create_dir_all(data);
    if let Ok(text) = serde_json::to_string(memory) {
        let _ = crate::replace_contents(&data.join("xyplorer.json"), text.as_bytes());
    }
}

/// What the plan ran as, written into the memory: the source's state now, its options, and the items applied.
fn remember(memory: &mut Memory, plan: &ImportPlan, applied: BTreeMap<String, Item>, forget: &[String]) {
    let meta = std::fs::metadata(&plan.source).ok();
    let s = memory.sources.entry(lower(&plan.source)).or_default();
    s.path = plan.source.clone();
    s.modified = meta.as_ref().map_or(plan.changed, stamp);
    s.size = meta.as_ref().map_or(0, |m| m.len());
    s.at = now_ms();
    s.xy = plan.version > 0;
    let o = &plan.options;
    s.options = Opts { meet: o.meet, extras: o.extras, labels: o.labels.clone(), base: o.base.clone(), have: o.have.clone() };
    for k in forget {
        s.items.remove(k);
    }
    s.items.extend(applied);
}

/// The thread runs at background priority while this is held (as `aware::pool`'s quiet lane does), and not after.
struct Quiet(bool);

impl Quiet {
    fn begin() -> Quiet {
        #[cfg(windows)]
        {
            use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
            // SAFETY: the handle is this thread's own pseudo-handle.
            Quiet(unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) }.is_ok())
        }
        #[cfg(not(windows))]
        Quiet(false)
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.0 {
            use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_END};
            // SAFETY: the same thread that began background mode ends it.
            let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_END) };
        }
        #[cfg(not(windows))]
        let _ = self.0;
    }
}

/// The journal of one import, created when the first item changes: the befores are written as the batches go.
struct Journal {
    dir: Option<PathBuf>,
    source: String,
    started: i64,
    file: Option<(std::fs::File, PathBuf)>,
    /// The labels before the import and as it left them, for the header.
    labels: (Vec<Label>, Vec<Label>),
}

impl Journal {
    fn new(data: Option<&Path>, plan: &ImportPlan) -> Journal {
        // The interface hands in the labels as they were when the plan was made (`options.have`): the same list
        // the import then changes, as the dialog is open until it runs.
        let before = plan.options.have.clone();
        let mut made = before.clone();
        apply_labels(&mut made, &plan.labels);
        Journal {
            dir: data.map(|d| d.join("imports")),
            source: plan.source.clone(),
            started: now_ms(),
            file: None,
            labels: (before, made),
        }
    }

    fn append(&mut self, lines: &[Line]) {
        if lines.is_empty() {
            return;
        }
        if self.file.is_none() {
            let Some(dir) = &self.dir else { return };
            let _ = std::fs::create_dir_all(dir);
            let mut ms = self.started;
            let created = loop {
                let path = dir.join(format!("{ms}.jsonl"));
                match std::fs::File::create_new(&path) {
                    Ok(f) => break Some((f, path)),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => ms += 1,
                    Err(_) => break None,
                }
            };
            let Some((mut f, path)) = created else {
                self.dir = None;
                return;
            };
            let header = serde_json::json!({
                "glint-import": 1, "source": self.source, "at": self.started,
                "labels": self.labels.0, "made": self.labels.1,
            });
            let _ = writeln!(f, "{header}");
            self.file = Some((f, path));
        }
        if let Some((f, _)) = &mut self.file {
            // The ledger's own text, without its first line (its version).
            let text = write_ledger(lines);
            let body = text.split_once('\n').map_or("", |x| x.1);
            let _ = f.write_all(body.as_bytes()).and_then(|()| f.flush());
        }
    }

    fn path(&self) -> Option<String> {
        self.file.as_ref().map(|(_, p)| p.to_string_lossy().into_owned())
    }
}

/// A journal line: the item and its marks before.
fn journal_line(path: &str, before: &Marks, at: i64) -> Line {
    let mut rest = serde_json::Map::new();
    if before.is_empty() {
        // A line with no marks and nothing else would not be read back; this says "had none".
        rest.insert("none".into(), serde_json::Value::Bool(true));
    }
    Line { name: path.to_string(), marks: before.clone(), at, why: None, rest }
}

/// The settings' labels after an import: a new label of the plan is added (never past 32), and Glint's label that
/// took XYplorer's name is renamed to it, the old name kept in `was` (PLAN 5.5).
pub fn apply_labels(labels: &mut Vec<Label>, plan: &[LabelPlan]) {
    for label in plan {
        if label.to.is_empty() {
            continue;
        }
        if label.new {
            if labels.len() < MAX_LABELS && label_of(labels, &label.name).is_none() {
                labels.push(Label { name: label.name.clone(), color: label.color, was: Vec::new() });
            }
        } else if label.takes_name {
            let Some((at, _)) = label_of(labels, &label.to) else {
                continue;
            };
            // The name is taken by another label: leave both as they are.
            if label_of(labels, &label.name).is_some_and(|(other, _)| other != at) {
                continue;
            }
            if let Some(own) = labels.get_mut(at) {
                if own.name != label.name {
                    let old = std::mem::replace(&mut own.name, label.name.clone());
                    if !own.was.contains(&old) {
                        own.was.push(old);
                    }
                }
            }
        }
    }
}

/// Journals older than 30 days are removed.
fn clean_journals(data: &Path) {
    let Ok(read) = std::fs::read_dir(data.join("imports")) else { return };
    let old = std::time::Duration::from_secs(KEEP_DAYS * 24 * 3600);
    for e in read.flatten() {
        let p = e.path();
        let aged = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|a| a > old);
        if aged && p.extension().is_some_and(|x| x == "jsonl") {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(test)]
thread_local! {
    /// How many times a batch went to the homes (a test counts the writes it cost).
    static WORKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn put_all(items: Vec<(String, Change)>, w: &World) -> Vec<Outcome> {
    #[cfg(test)]
    WORKS.with(|n| n.set(n.get() + 1));
    work(items, w)
}

/// What one run did.
struct Done {
    report: ImportReport,
    /// What each item was given, to remember.
    applied: BTreeMap<String, Item>,
    /// Items whose marks were taken off (following).
    gone: u64,
}

/// Applies the plan in batches of 200: the items, then `gone` (remembered items whose line is gone: their marks
/// come off only where they are still what the source gave).
fn apply_in(
    plan: &ImportPlan,
    progress: Option<&Channel<Progress>>,
    data: Option<&Path>,
    w: &World,
    known: &BTreeMap<String, Item>,
    gone: &[(String, Item)],
) -> Result<Done, String> {
    let _quiet = Quiet::begin();
    if let Some(d) = data {
        clean_journals(d);
    }
    let mut journal = Journal::new(data, plan);
    let mut done = Done { report: ImportReport::default(), applied: BTreeMap::new(), gone: 0 };
    let total = (plan.items.len() + gone.len()) as u64;
    let mut count = 0u64;
    let say = |count: u64| {
        if let Some(p) = progress {
            let _ = p.send(Progress { done: count, total });
        }
    };

    for items in plan.items.chunks(BATCH) {
        let lines = batch(plan, items, known, w, &mut done);
        journal.append(&lines);
        count += items.len() as u64;
        say(count);
    }
    for chunk in gone.chunks(BATCH) {
        let paths: Vec<String> = chunk.iter().map(|(_, it)| it.at.clone()).collect();
        let has = read_all(&paths, w);
        let puts: Vec<(String, Change)> = chunk
            .iter()
            .zip(has)
            .filter(|((_, it), has)| hash_of(has) == it.hash)
            .map(|((_, it), _)| (it.at.clone(), Change::Put(Marks::default())))
            .collect();
        let mut lines = Vec::new();
        for o in put_all(puts, w) {
            if o.after.as_ref().is_ok_and(|(after, _)| *after != o.before) {
                done.gone += 1;
                lines.push(journal_line(&o.path, &o.before, w.now));
                done.report.befores.push((o.path, o.before));
            }
        }
        journal.append(&lines);
        count += chunk.len() as u64;
        say(count);
    }
    done.report.journal = journal.path();
    Ok(done)
}

/// One batch: what each item gets, written once a folder (the homes') and once in the ledger; the journal lines
/// of what changed.
fn batch(plan: &ImportPlan, items: &[Planned], known: &BTreeMap<String, Item>, w: &World, done: &mut Done) -> Vec<Line> {
    const ALREADY: &str = "Already had these marks";
    const KEPT: &str = "Kept its own comment";
    let mut words: Vec<String> = vec![String::new(); items.len()];
    let mut kept = HashSet::new();
    let mut lines = Vec::new();
    let (mut puts, mut put_of, mut waits) = (Vec::new(), Vec::new(), Vec::new());

    // Where each is marked: here, or where it moved to; the others wait.
    let mut targets: Vec<(usize, &str)> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        match &it.at {
            At::Here => targets.push((i, it.path.as_str())),
            At::Moved { now } => targets.push((i, now.as_str())),
            At::Away | At::Missing => waits.push(i),
        }
    }
    let paths: Vec<String> = targets.iter().map(|(_, p)| (*p).to_string()).collect();
    let has = read_all(&paths, w);
    for (&(i, path), has) in targets.iter().zip(has) {
        let it = &items[i];
        let hash = hash_of(&it.gets);
        let same = known.get(&lower(&it.path)).is_some_and(|k| k.hash == hash);
        done.applied.insert(lower(&it.path), Item { hash, at: path.to_string() });
        if same && has == tidy(&it.gets) {
            words[i] = ALREADY.into();
            continue;
        }
        let (after, was_kept) = meet(&has, &it.gets, plan.options.meet);
        if was_kept {
            done.report.kept_comments += 1;
            kept.insert(i);
        }
        if after == has {
            words[i] = if was_kept { KEPT } else { ALREADY }.into();
            continue;
        }
        puts.push((path.to_string(), Change::Put(after)));
        put_of.push(i);
    }
    for (o, i) in put_all(puts, w).into_iter().zip(put_of) {
        let it = &items[i];
        words[i] = match o.after {
            Ok((after, _)) if after != o.before => {
                done.report.marked += 1;
                lines.push(journal_line(&o.path, &o.before, w.now));
                done.report.befores.push((o.path.clone(), o.before));
                match &it.at {
                    _ if kept.contains(&i) => KEPT.to_string(),
                    At::Moved { now } => format!("Marked where it is now: {now}"),
                    _ => "Marked".to_string(),
                }
            }
            Ok(_) => ALREADY.into(),
            Err(e) => {
                done.applied.remove(&lower(&it.path));
                format!("Couldn't mark: {e}")
            }
        };
    }

    if !waits.is_empty() {
        let mut ledger = w.ledger();
        let mut changed = Vec::new();
        for &i in &waits {
            let it = &items[i];
            let has = ledger.get(&it.path).map(|l| tidy(&l.marks)).unwrap_or_default();
            let hash = hash_of(&it.gets);
            let same = known.get(&lower(&it.path)).is_some_and(|k| k.hash == hash);
            done.applied.insert(lower(&it.path), Item { hash, at: it.path.clone() });
            let (after, was_kept) = meet(&has, &it.gets, plan.options.meet);
            if was_kept && !(same && has == tidy(&it.gets)) {
                done.report.kept_comments += 1;
            }
            if matches!(it.at, At::Away) {
                done.report.waiting += 1;
                words[i] = "Waiting for its drive".into();
            } else {
                done.report.missing += 1;
                words[i] = "Not found: its marks wait".into();
            }
            if after != has {
                ledger.put(&it.path, &after, Why::Away, w.now);
                lines.push(journal_line(&it.path, &has, w.now));
                done.report.befores.push((it.path.clone(), has));
                changed.push(i);
            }
        }
        if let Err(e) = ledger.save() {
            for i in changed {
                words[i] = format!("Couldn't mark: Couldn't keep the marks on this PC: {e}");
            }
        }
    }
    done.report.items.extend(items.iter().map(|it| it.path.clone()).zip(words));
    lines
}

/// Applies `plan`, saying how far it is (the blocking part of `import_apply`).
pub fn apply(plan: ImportPlan, progress: &Channel<Progress>) -> Result<ImportReport, String> {
    let data = crate::tools::data_dir();
    real(|w| import_in(&plan, Some(progress), data.as_deref(), w))
}

fn import_in(
    plan: &ImportPlan,
    progress: Option<&Channel<Progress>>,
    data: Option<&Path>,
    w: &World,
) -> Result<ImportReport, String> {
    let mut memory = data.map(load).unwrap_or_default();
    let known = memory.sources.get(&lower(&plan.source)).map(|s| s.items.clone()).unwrap_or_default();
    let done = apply_in(plan, progress, data, w, &known, &[])?;
    if let Some(d) = data {
        remember(&mut memory, plan, done.applied, &[]);
        save(d, &memory);
    }
    Ok(done.report)
}

/// The newest journal in `data`'s imports folder, as `last_import` answers.
fn last_import_in(data: &Path) -> Option<LastImport> {
    let newest = std::fs::read_dir(data.join("imports"))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|p| Some((p.file_stem()?.to_str()?.parse::<i64>().ok()?, p)))
        .max_by_key(|(ms, _)| *ms)?;
    let text = read_capped(&newest.1).unwrap_or_default();
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let head = lines.next().and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok());
    let source = head.as_ref().and_then(|h| h["source"].as_str()).unwrap_or("").to_string();
    let at = head.as_ref().and_then(|h| h["at"].as_i64()).unwrap_or(newest.0);
    let labels_of = |key: &str| head.as_ref().and_then(|h| serde_json::from_value::<Vec<Label>>(h[key].clone()).ok());
    let (labels, made) = (labels_of("labels"), labels_of("made"));
    Some(LastImport { journal: newest.1.to_string_lossy().into_owned(), source, at, items: lines.count() as u64, labels, made })
}

/// The last import whose journal is still kept (30 days), if any.
pub fn last_import() -> Option<LastImport> {
    last_import_in(&crate::tools::data_dir()?)
}

/// Puts back what the import with this journal changed: each item's marks as they were before it.
pub async fn import_take_back(journal: String) -> Result<Vec<Outcome>, String> {
    let data = crate::tools::data_dir();
    blocking(move || real(|w| take_back_in(&journal, data.as_deref(), w))).await
}

fn take_back_in(journal: &str, data: Option<&Path>, w: &World) -> Result<Vec<Outcome>, String> {
    let data = data.ok_or("Glint's data folder isn't available")?;
    // M10: a journal is only read from where Glint keeps them.
    let path = Path::new(journal);
    let inside = match (path.canonicalize(), data.join("imports").canonicalize()) {
        (Ok(p), Ok(imports)) => p.parent() == Some(imports.as_path()) && p.extension().is_some_and(|x| x == "jsonl"),
        _ => false,
    };
    if !inside {
        return Err("That is not one of Glint's import journals".into());
    }
    let text = read_capped(path).ok_or("The journal can't be read")?;
    // The first line is the journal's own header, which a line reader leaves out; a line cut short is left out too.
    let lines = read_ledger(&text);
    let outcomes = put_all(lines.iter().map(|l| (l.name.clone(), Change::Put(l.marks.clone()))).collect(), w);

    let back: HashSet<String> = lines.iter().map(|l| lower(&l.name)).collect();
    let mut memory = load(data);
    for s in memory.sources.values_mut() {
        let before = s.items.len();
        s.items.retain(|k, it| !back.contains(k) && !back.contains(&lower(&it.at)));
        if s.items.len() < before && s.items.is_empty() {
            s.path.clear();
        }
    }
    memory.sources.retain(|_, s| !s.path.is_empty());
    save(data, &memory);
    let _ = std::fs::remove_file(path);
    Ok(outcomes)
}

/// Whether `source` (a `tag.dat`) was brought in before, as it is now or as it was.
pub fn brought_in(source: &str) -> bool {
    crate::tools::data_dir().is_some_and(|d| load(&d).sources.contains_key(&lower(source)))
}

/// The XYplorer file to follow if it changed since it was read (one `stat`), and the options to plan it again
/// with: the newest source that was brought in, when its date or size isn't what is remembered.
fn changed_source(data: &Path) -> Option<(String, ImportOptions)> {
    let memory = load(data);
    let s = memory.sources.values().filter(|s| s.xy).max_by_key(|s| s.at)?;
    let meta = std::fs::metadata(&s.path).ok()?;
    if stamp(&meta) == s.modified && meta.len() == s.size {
        return None;
    }
    let o = &s.options;
    let options =
        ImportOptions { meet: o.meet, extras: o.extras, labels: o.labels.clone(), base: o.base.clone(), have: o.have.clone() };
    Some((s.path.clone(), options))
}

/// Following XYplorer (F30): one `stat` of the source that was brought in; when it changed since, what is new
/// or changed in it comes in. None when nothing is followed or nothing changed.
pub async fn follow() -> Result<Option<Followed>, String> {
    follow_in(None).await
}

/// [`follow`], planned with Glint's labels as they are now (`have`, from Settings) and not as they were at the
/// import: a label renamed in Glint since is matched by its new name and `was`, and one XYplorer gained since is
/// named new in `Followed::labels`.
pub async fn follow_with(have: Vec<Label>) -> Result<Option<Followed>, String> {
    follow_in(Some(have)).await
}

async fn follow_in(have: Option<Vec<Label>>) -> Result<Option<Followed>, String> {
    let Some(data) = crate::tools::data_dir() else { return Ok(None) };
    let d = data.clone();
    let Some((source, mut options)) = blocking(move || Ok(changed_source(&d))).await? else { return Ok(None) };
    if let Some(have) = have {
        options.have = have;
    }
    let plan = super::import::import_plan(source, options).await?;
    blocking(move || real(|w| follow_plan(&plan, &data, w))).await
}

/// What following does with a plan that was just made of the source: the items new or changed since are applied,
/// and what is remembered whose line is gone loses its marks where they are still what the source gave.
fn follow_plan(plan: &ImportPlan, data: &Path, w: &World) -> Result<Option<Followed>, String> {
    let mut memory = load(data);
    let known = memory.sources.get(&lower(&plan.source)).map(|s| s.items.clone()).unwrap_or_default();
    let present: HashSet<String> = plan.items.iter().map(|it| lower(&it.path)).collect();
    let fresh = ImportPlan {
        items: plan
            .items
            .iter()
            .filter(|it| known.get(&lower(&it.path)).is_none_or(|k| k.hash != hash_of(&it.gets)))
            .cloned()
            .collect(),
        ..plan.clone()
    };
    let gone: Vec<(String, Item)> = known.iter().filter(|(k, _)| !present.contains(*k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    let done = apply_in(&fresh, None, Some(data), w, &known, &gone)?;
    let forget: Vec<String> = gone.into_iter().map(|(k, _)| k).collect();
    remember(&mut memory, plan, done.applied, &forget);
    save(data, &memory);
    let (came, gone) = (done.report.marked, done.gone);
    if came + gone == 0 {
        return Ok(None);
    }
    let paths = done.report.befores.into_iter().map(|(p, _)| p).collect();
    Ok(Some(Followed { source: plan.source.clone(), came, gone, paths, labels: plan.labels.clone() }))
}

#[cfg(test)]
mod tests {
    use super::super::ledger::Ledger;
    use super::super::{caps, streams};
    use super::*;
    use std::sync::{Arc, Mutex};

    fn none(_: &str) -> bool {
        false
    }

    /// A folder of the test's own: files, Glint's data folder, and a ledger of its own.
    struct Rig {
        root: PathBuf,
        data: PathBuf,
        files: PathBuf,
        ledger: Mutex<Ledger>,
    }

    impl Rig {
        fn new(name: &str) -> Rig {
            let root = std::env::temp_dir().join(format!("glint-apply-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let (data, files) = (root.join("data"), root.join("files"));
            std::fs::create_dir_all(&files).unwrap();
            std::fs::create_dir_all(&data).unwrap();
            let ledger = Mutex::new(Ledger::open(Some(root.join("ledger").join("marks.jsonl"))));
            Rig { root, data, files, ledger }
        }

        fn w(&self) -> World<'_> {
            World { streams: &none, ledger: &self.ledger, empty: None, now: 1_000 }
        }

        fn file(&self, name: &str) -> String {
            let p = self.files.join(name);
            std::fs::write(&p, b"hello").unwrap();
            p.to_string_lossy().into_owned()
        }

        fn ghost(&self, name: &str) -> String {
            self.files.join(name).to_string_lossy().into_owned()
        }

        fn read(&self, paths: &[&String]) -> Vec<Marks> {
            read_all(&paths.iter().map(|p| (*p).clone()).collect::<Vec<_>>(), &self.w())
        }

        fn import(&self, plan: &ImportPlan) -> ImportReport {
            import_in(plan, None, Some(&self.data), &self.w()).unwrap()
        }

        fn journals(&self) -> Vec<PathBuf> {
            std::fs::read_dir(self.data.join("imports"))
                .map(|r| r.flatten().map(|e| e.path()).collect())
                .unwrap_or_default()
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn label(name: &str) -> Marks {
        Marks { label: Some(name.into()), ..Marks::default() }
    }

    fn full(label: &str, tag: &str, comment: &str) -> Marks {
        Marks { label: Some(label.into()), tags: vec![tag.into()], comment: Some(comment.into()) }
    }

    fn at(path: &str, at: At, gets: Marks) -> Planned {
        Planned { path: path.into(), at, gets, has: None }
    }

    fn here(path: &str, gets: Marks) -> Planned {
        at(path, At::Here, gets)
    }

    fn plan(source: &str, items: Vec<Planned>) -> ImportPlan {
        ImportPlan { source: source.into(), version: 5, items, ..ImportPlan::default() }
    }

    fn lines_of(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path).unwrap().lines().map(String::from).collect()
    }

    /// The whole of an import, as the dialog runs it: planned from a file XYplorer wrote (issue 8's, in Chinese,
    /// its two paths made relative to where it is), applied, applied again, taken back.
    #[test]
    fn a_tag_dat_xyplorer_wrote_is_planned_applied_once_changes_nothing_the_second_time_and_is_taken_back() {
        let r = Rig::new("whole");
        std::fs::create_dir_all(r.files.join("sub")).unwrap();
        let (a, b) = (r.file("a.txt"), r.file("sub\\b.txt"));
        let real = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/marks/real-v5-zh.dat")).unwrap();
        let units: Vec<u16> = real[2..].as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
        let text = String::from_utf16(&units)
            .unwrap()
            .replace("Storage: 0 (Absolute paths)", "Storage: 3 (Relative to tags database)")
            .replace("C:\\$Recycle.Bin", "a.txt")
            .replace("C:\\Documents and Settings", "sub\\b.txt");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let source = r.files.join("tag.dat");
        std::fs::write(&source, bytes).unwrap();
        let options = ImportOptions { have: glint_model::marks::default_labels(), ..ImportOptions::default() };

        let plan = super::super::import::plan_here(&source, options.clone()).unwrap();
        assert_eq!((plan.version, plan.items.len()), (5, 2));
        assert!(plan.items.iter().all(|i| i.at == At::Here), "{:?}", plan.items);
        let red = &plan.labels[0];
        assert_eq!((red.name.as_str(), red.to.as_str(), red.new, red.takes_name), ("红色", "Red", false, true));

        let report = r.import(&plan);
        assert_eq!((report.marked, report.waiting, report.missing), (2, 0, 0), "{:?}", report.items);
        let want_a = Marks { label: Some("Red".into()), tags: vec!["tag1".into()], comment: Some("comment1\nlala".into()) };
        let want_b = Marks { tags: vec!["tag2".into()], ..Marks::default() };
        assert_eq!(r.read(&[&a, &b]), vec![want_a, want_b]);

        let again = r.import(&super::super::import::plan_here(&source, options).unwrap());
        assert_eq!((again.marked, again.journal.as_deref()), (0, None), "{:?}", again.items);

        take_back_in(&report.journal.unwrap(), Some(&r.data), &r.w()).unwrap();
        assert_eq!(r.read(&[&a, &b]), vec![Marks::default(), Marks::default()]);
    }

    #[test]
    fn a_plan_of_three_items_marks_them_keeps_their_befores_a_journal_and_reports_three_of_three() {
        let r = Rig::new("three");
        let paths: Vec<String> = ["a.txt", "b.txt", "c.txt"].iter().map(|n| r.file(n)).collect();
        let p = plan("C:\\xy\\tag.dat", paths.iter().map(|p| here(p, label("Red"))).collect());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let channel = Channel::new(move |m: Progress| {
            sink.lock().unwrap().push(m);
            Ok(())
        });
        let report = import_in(&p, Some(&channel), Some(&r.data), &r.w()).unwrap();
        assert_eq!((report.marked, report.waiting, report.missing, report.kept_comments), (3, 0, 0, 0));
        assert_eq!(report.befores.len(), 3);
        assert!(report.befores.iter().all(|(_, m)| m.is_empty()));
        assert_eq!(report.items.len(), 3);
        assert!(report.items.iter().all(|(_, words)| words == "Marked"));
        assert_eq!(r.read(&[&paths[0], &paths[1], &paths[2]]), vec![label("Red"); 3]);
        let journal = report.journal.expect("a journal");
        assert!(journal.contains("imports"), "{journal}");
        let lines = lines_of(Path::new(&journal));
        assert_eq!(lines.len(), 4, "{lines:?}");
        let head: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!((head["glint-import"].as_i64(), head["source"].as_str()), (Some(1), Some("C:\\xy\\tag.dat")));
        assert!(lines[1..].iter().all(|l| l.contains("\"none\":true")), "{lines:?}");
        assert_eq!(seen.lock().unwrap().last().copied(), Some(Progress { done: 3, total: 3 }));
    }

    #[test]
    fn fill_keeps_a_different_comment_and_counts_it_and_replace_replaces_all_three_marks() {
        let mine = full("Old", "a", "mine");
        let theirs = full("New", "b", "theirs");
        for replace in [false, true] {
            let r = Rig::new(if replace { "replace" } else { "fill" });
            let paths: Vec<String> = ["a.txt", "b.txt", "c.txt"].iter().map(|n| r.file(n)).collect();
            work(paths.iter().map(|p| (p.clone(), Change::Put(mine.clone()))).collect(), &r.w());
            let mut p = plan("C:\\xy\\tag.dat", paths.iter().map(|p| here(p, theirs.clone())).collect());
            p.options.meet = if replace { Meet::Replace } else { Meet::Fill };
            let report = r.import(&p);
            let now = r.read(&[&paths[0], &paths[1], &paths[2]]);
            if replace {
                assert_eq!((report.marked, report.kept_comments), (3, 0));
                assert_eq!(now, vec![theirs.clone(); 3]);
            } else {
                assert_eq!((report.marked, report.kept_comments), (3, 3));
                assert_eq!(now, vec![full("Old", "a", "mine").tap_tags(&["a", "b"]); 3]);
                assert!(report.items.iter().all(|(_, w)| w == "Kept its own comment"), "{:?}", report.items);
            }
        }
    }

    trait TapTags {
        fn tap_tags(self, tags: &[&str]) -> Marks;
    }

    impl TapTags for Marks {
        fn tap_tags(mut self, tags: &[&str]) -> Marks {
            self.tags = tags.iter().map(|t| t.to_string()).collect();
            self
        }
    }

    #[test]
    fn moved_marks_the_file_where_it_is_now_and_away_and_missing_wait_in_the_ledger() {
        let r = Rig::new("waits");
        let now = r.file("now.txt");
        let (old, missing) = (r.ghost("old.txt"), r.ghost("lost.txt"));
        let away = "Z:\\Nowhere\\far.txt".to_string();
        let p = plan(
            "C:\\xy\\tag.dat",
            vec![
                at(&old, At::Moved { now: now.clone() }, label("Blue")),
                at(&away, At::Away, label("Red")),
                at(&missing, At::Missing, label("Green")),
            ],
        );
        let report = r.import(&p);
        assert_eq!((report.marked, report.waiting, report.missing), (1, 1, 1));
        assert_eq!(r.read(&[&now]), vec![label("Blue")]);
        assert_eq!(report.items[0].1, format!("Marked where it is now: {now}"));
        assert_eq!(report.items[1].1, "Waiting for its drive");
        assert_eq!(report.items[2].1, "Not found: its marks wait");
        let ledger = std::fs::read_to_string(r.root.join("ledger").join("marks.jsonl")).unwrap();
        assert_eq!(ledger.matches("\"why\":\"away\"").count(), 2, "{ledger}");
        assert_eq!(r.ledger.lock().unwrap().away(), 2);
    }

    #[test]
    fn the_same_plan_applied_twice_changes_nothing_the_second_time() {
        let r = Rig::new("twice");
        let paths: Vec<String> = ["a.txt", "b.txt", "c.txt"].iter().map(|n| r.file(n)).collect();
        let away = "Z:\\Nowhere\\far.txt".to_string();
        let mut items: Vec<Planned> = paths.iter().map(|p| here(p, full("Red", "t", "c"))).collect();
        items.push(at(&away, At::Away, label("Blue")));
        let p = plan("C:\\xy\\tag.dat", items);
        assert_eq!(r.import(&p).marked, 3);
        let side = r.files.join(".glint-marks");
        let (m_side, m_file) = (std::fs::metadata(&side).unwrap().modified().unwrap(), std::fs::metadata(&paths[0]).unwrap().modified().unwrap());
        let journals = r.journals();
        assert_eq!(journals.len(), 1);
        let before = lines_of(&journals[0]);
        std::thread::sleep(std::time::Duration::from_millis(30));
        let again = r.import(&p);
        assert_eq!((again.marked, again.waiting), (0, 1));
        assert!(again.journal.is_none() && again.befores.is_empty());
        assert_eq!(std::fs::metadata(&side).unwrap().modified().unwrap(), m_side);
        assert_eq!(std::fs::metadata(&paths[0]).unwrap().modified().unwrap(), m_file);
        assert_eq!(r.journals(), journals);
        assert_eq!(lines_of(&journals[0]), before);
        assert!(again.items.iter().take(3).all(|(_, w)| w == "Already had these marks"), "{:?}", again.items);
    }

    #[test]
    fn four_hundred_and_fifty_items_in_one_folder_write_the_sidecar_three_times() {
        let r = Rig::new("batches");
        let items: Vec<Planned> = (0..450).map(|i| here(&r.file(&format!("f{i:03}.txt")), label("Red"))).collect();
        let p = plan("C:\\xy\\tag.dat", items);
        WORKS.with(|n| n.set(0));
        let report = r.import(&p);
        assert_eq!(report.marked, 450);
        assert_eq!(WORKS.with(|n| n.get()), 3);
        assert_eq!(lines_of(&r.files.join(".glint-marks")).len(), 451);
    }

    #[test]
    fn taking_an_import_back_restores_every_item_forgets_it_and_removes_the_journal() {
        let r = Rig::new("back");
        let (a, b) = (r.file("a.txt"), r.file("b.txt"));
        let away = "Z:\\Nowhere\\far.txt".to_string();
        work(vec![(a.clone(), Change::Put(label("Had")))], &r.w());
        let mut p = plan(
            "C:\\xy\\tag.dat",
            vec![here(&a, label("Imp")), here(&b, label("Imp")), at(&away, At::Away, label("Imp"))],
        );
        p.options.meet = Meet::Replace;
        let report = r.import(&p);
        assert_eq!(report.marked, 2);
        assert_eq!(r.read(&[&a, &b]), vec![label("Imp"), label("Imp")]);
        let last = last_import_in(&r.data).expect("a last import");
        assert_eq!((last.source.as_str(), last.items), ("C:\\xy\\tag.dat", 3));
        assert_eq!(Some(last.journal.clone()), report.journal);
        assert!(load(&r.data).sources.contains_key(&lower("C:\\xy\\tag.dat")));

        // A journal from anywhere else is refused.
        let other = r.root.join("other.jsonl");
        std::fs::write(&other, std::fs::read(&last.journal).unwrap()).unwrap();
        assert!(take_back_in(&other.to_string_lossy(), Some(&r.data), &r.w()).is_err());
        assert_eq!(r.read(&[&a]), vec![label("Imp")]);

        let outcomes = take_back_in(&last.journal, Some(&r.data), &r.w()).unwrap();
        assert_eq!(outcomes.len(), 3);
        assert_eq!(r.read(&[&a, &b]), vec![label("Had"), Marks::default()]);
        assert!(r.ledger.lock().unwrap().get(&away).is_none());
        assert!(!Path::new(&last.journal).exists());
        assert_eq!(last_import_in(&r.data), None);
        assert!(!load(&r.data).sources.contains_key(&lower("C:\\xy\\tag.dat")));
    }

    #[test]
    fn a_journal_cut_mid_line_gives_back_the_lines_before_the_cut() {
        let r = Rig::new("cut");
        let paths: Vec<String> = ["x.txt", "y.txt", "z.txt"].iter().map(|n| r.file(n)).collect();
        let report = r.import(&plan("C:\\xy\\tag.dat", paths.iter().map(|p| here(p, label("Imp"))).collect()));
        let text = std::fs::read_to_string(report.journal.unwrap()).unwrap();
        let ends: Vec<usize> = text.match_indices('\n').map(|(i, _)| i + 1).collect();
        // The header, the first item whole, and half of the second.
        let cut = &text[..ends[1] + (ends[2] - ends[1]) / 2];
        let path = r.data.join("imports").join("1.jsonl");
        std::fs::write(&path, cut).unwrap();
        let outcomes = take_back_in(&path.to_string_lossy(), Some(&r.data), &r.w()).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(r.read(&[&paths[0], &paths[1], &paths[2]]), vec![Marks::default(), label("Imp"), label("Imp")]);
    }

    #[test]
    fn marking_leaves_a_files_dates_and_contents() {
        let r = Rig::new("dates");
        let a = r.file("a.txt");
        let modified = std::fs::metadata(&a).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        r.import(&plan("C:\\xy\\tag.dat", vec![here(&a, full("Red", "t", "c"))]));
        assert_eq!(std::fs::metadata(&a).unwrap().modified().unwrap(), modified);
        assert_eq!(std::fs::read(&a).unwrap(), b"hello");
        // And where the drive keeps streams, which is where the marks go then.
        if caps::has_streams(&r.files.to_string_lossy()) {
            let b = r.file("b.txt");
            let modified = std::fs::metadata(&b).unwrap().modified().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(30));
            let w = World { streams: &caps::has_streams, ledger: &r.ledger, empty: None, now: 1_000 };
            let report = import_in(&plan("C:\\xy\\other.dat", vec![here(&b, label("Blue"))]), None, Some(&r.data), &w).unwrap();
            assert_eq!(report.marked, 1);
            assert_eq!(streams::read(&b), Some(label("Blue")));
            assert_eq!(std::fs::metadata(&b).unwrap().modified().unwrap(), modified);
            assert_eq!(std::fs::read(&b).unwrap(), b"hello");
        }
    }

    #[test]
    fn following_brings_in_the_new_and_the_changed_and_takes_off_what_is_gone_where_it_is_still_xyplorers() {
        let r = Rig::new("follow");
        let source = r.root.join("tag.dat");
        std::fs::write(&source, b"version one").unwrap();
        let src = source.to_string_lossy().into_owned();
        let [a, b, c, d, e] = ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"].map(|n| r.file(n));
        let mut first = plan(&src, [&a, &b, &c, &d].iter().map(|p| here(p, label("One"))).collect());
        first.options.meet = Meet::Replace;
        r.import(&first);

        // Nothing changed in the source: nothing to read, one stat.
        assert!(changed_source(&r.data).is_none());
        assert_eq!(follow_plan(&ImportPlan { items: first.items.clone(), ..first.clone() }, &r.data, &r.w()).unwrap(), None);

        // The person marked d themselves since.
        work(vec![(d.clone(), Change::Put(label("Mine")))], &r.w());
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(&source, b"version two, longer").unwrap();
        let (found, options) = changed_source(&r.data).expect("the source changed");
        assert_eq!(found, src);
        assert_eq!(options.meet, Meet::Replace);

        let mut second = plan(&src, vec![here(&a, label("One")), here(&b, label("Two")), here(&e, label("New"))]);
        second.options.meet = Meet::Replace;
        let followed = follow_plan(&second, &r.data, &r.w()).unwrap().expect("something came");
        assert_eq!((followed.came, followed.gone), (2, 1));
        let mut paths = followed.paths.clone();
        paths.sort();
        assert_eq!(paths, [b.clone(), c.clone(), e.clone()]);
        assert_eq!(
            r.read(&[&a, &b, &c, &d, &e]),
            vec![label("One"), label("Two"), Marks::default(), label("Mine"), label("New")]
        );
        // What was followed is the new state: the source is as it is now, and a second look finds nothing.
        assert!(changed_source(&r.data).is_none());
        assert_eq!(follow_plan(&second, &r.data, &r.w()).unwrap(), None);
        assert!(!load(&r.data).sources.values().any(|s| s.items.contains_key(&lower(&c)) || s.items.contains_key(&lower(&d))));
        // It wrote a journal of its own, so the last import can be taken back.
        assert!(last_import_in(&r.data).is_some());
    }

    /// The fixture of issue 8 (Chinese label names) as a tag.dat in the rig's folder, its two paths relative.
    fn xyplorer_file(r: &Rig) -> PathBuf {
        r.file("a.txt");
        std::fs::create_dir_all(r.files.join("sub")).unwrap();
        r.file("sub\\b.txt");
        let real = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/marks/real-v5-zh.dat")).unwrap();
        let units: Vec<u16> = real[2..].as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
        let text = String::from_utf16(&units)
            .unwrap()
            .replace("Storage: 0 (Absolute paths)", "Storage: 3 (Relative to tags database)")
            .replace("C:\\$Recycle.Bin", "a.txt")
            .replace("C:\\Documents and Settings", "sub\\b.txt");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let source = r.files.join("tag.dat");
        std::fs::write(&source, bytes).unwrap();
        source
    }

    fn with_have(have: Vec<Label>) -> ImportOptions {
        ImportOptions { have, meet: Meet::Replace, ..ImportOptions::default() }
    }

    #[test]
    fn the_journals_first_line_keeps_the_labels_before_and_after_and_a_journal_from_before_reads_without() {
        let r = Rig::new("journal-labels");
        let a = r.file("a.txt");
        let before = glint_model::marks::default_labels();
        let new = LabelPlan { name: "天蓝色".into(), color: 0x0ea5e9, to: "天蓝色".into(), new: true, takes_name: false };
        let mut p = plan("C:\\xy\\tag.dat", vec![here(&a, label("天蓝色"))]);
        p.options = with_have(before.clone());
        p.labels = vec![new];
        let report = r.import(&p);
        let last = last_import_in(&r.data).expect("a last import");
        assert_eq!(Some(last.journal.clone()), report.journal);
        assert_eq!(last.labels.as_ref(), Some(&before));
        let made = last.made.clone().expect("the labels it made");
        assert_eq!((made.len(), made.last().map(|l| l.name.as_str())), (before.len() + 1, Some("天蓝色")));

        // A journal written before the labels were kept: the same lines with a bare first line.
        let text = std::fs::read_to_string(&last.journal).unwrap();
        let body = text.split_once('\n').unwrap().1;
        let old = format!("{{\"glint-import\":1,\"source\":\"C:\\\\xy\\\\tag.dat\",\"at\":{}}}\n{body}", last.at);
        std::fs::write(&last.journal, old).unwrap();
        let again = last_import_in(&r.data).expect("still a last import");
        assert_eq!((again.labels, again.made, again.items), (None, None, 1));
    }

    #[test]
    fn following_with_a_label_xyplorer_gained_since_names_it_new_and_one_renamed_in_glint_is_matched_by_its_new_name() {
        let r = Rig::new("follow-labels");
        let source = xyplorer_file(&r);
        let mut have = glint_model::marks::default_labels();
        let first = super::super::import::plan_here(&source, with_have(have.clone())).unwrap();
        assert!(first.labels.iter().any(|l| l.name == "天蓝色" && l.new), "{:?}", first.labels);
        r.import(&first);

        // Since then Red was renamed in Glint (keeping its old name), and the file's marks changed.
        let old = std::mem::replace(&mut have[0].name, "红色".into());
        have[0].was.push(old);
        let mut second = super::super::import::plan_here(&source, with_have(have.clone())).unwrap();
        second.items[0].gets = label("天蓝色");
        let red = &second.labels[0];
        assert_eq!((red.name.as_str(), red.to.as_str(), red.new, red.takes_name), ("红色", "红色", false, false));
        let followed = follow_plan(&second, &r.data, &r.w()).unwrap().expect("something came");
        assert_eq!(followed.labels, second.labels);
        let sky = followed.labels.iter().find(|l| l.name == "天蓝色").expect("the label XYplorer gained");
        assert!(sky.new && sky.to == "天蓝色");
        // Its own journal keeps the labels it was planned with.
        assert_eq!(last_import_in(&r.data).and_then(|l| l.labels), Some(have));
    }

    #[test]
    fn a_damaged_memory_reads_as_empty_and_an_unknown_source_was_not_brought_in() {
        let r = Rig::new("damaged");
        std::fs::write(r.data.join("xyplorer.json"), b"{{ not json").unwrap();
        assert!(load(&r.data).sources.is_empty());
        r.import(&plan("C:\\xy\\tag.dat", vec![]));
        assert!(load(&r.data).sources.contains_key(&lower("c:\\XY\\tag.dat")));
        assert!(!load(&r.data).sources.contains_key(&lower("C:\\xy\\other.dat")));
    }

    #[test]
    #[ignore = "a measurement: 10,000 items over 10,000 small files in 100 folders"]
    fn ten_thousand_items_in_a_hundred_folders_time() {
        for streams in [false, true] {
            let r = Rig::new(if streams { "perf-streams" } else { "perf" });
            let mut items = Vec::new();
            for f in 0..100 {
                let dir = r.files.join(format!("d{f:03}"));
                std::fs::create_dir_all(&dir).unwrap();
                for i in 0..100 {
                    let p = dir.join(format!("f{i:03}.txt"));
                    std::fs::write(&p, b"x").unwrap();
                    items.push(here(&p.to_string_lossy(), full("Red", "t", "a comment")));
                }
            }
            let p = plan("C:\\xy\\tag.dat", items);
            let w = World {
                streams: if streams { &caps::has_streams } else { &none },
                ledger: &r.ledger,
                empty: None,
                now: 1_000,
            };
            let started = std::time::Instant::now();
            let report = import_in(&p, None, Some(&r.data), &w).unwrap();
            println!("10,000 items, streams {streams}: {} ms, marked {}", started.elapsed().as_millis(), report.marked);
            assert_eq!(report.marked, 10_000);
        }
    }
}
