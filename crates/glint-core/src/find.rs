//! Find files by name: from the instant search index when its service runs (whole drives in
//! milliseconds), otherwise with the bundled fd, which also covers what the index can't (drives it
//! doesn't index, .gitignore rules). Results stream in either way, each with its size, date and
//! attributes.

use crate::fsx::{entry_for_path, ms_since, Entry};
use crate::proc::Slot;
use crate::AppState;
use glint_model::marks::{tidy, Beside, Line, Marks, Wanted};
use shadui_index::query::{parse, without_marks, without_props, Filters, Kind, NameMatcher};
use std::io::{BufRead, BufReader, Read};
use std::process::Stdio;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use crate::host::ipc::Channel;
use crate::host::State;

#[derive(Clone)]
pub struct FindOptions {
    /// Include hidden and system files.
    pub hidden: bool,
    /// Respect .gitignore and .ignore files.
    pub respect_ignore: bool,
    /// Names or globs to skip, e.g. node_modules.
    pub exclude: Vec<String>,
    pub max_results: Option<usize>,
    /// When nothing matches as typed, the index looks again with typos allowed (a word an edit or two
    /// from the start of a name's word); fd doesn't.
    pub fuzzy: bool,
    /// The index sends the best `max_results` of every drive's matches, best first, instead of the first
    /// ones it comes to (quick search's and Files' order; fd's stays its own).
    pub rank: bool,
}

/// What a search sends as it goes: who answers, what it found (in batches, each with its own path), how it ended.
#[derive(Clone, Debug)]
pub enum FindEvent {
    /// Who answers: "index", "fd", or "index + fd" when the index covers only some of the folders.
    Started { engine: &'static str },
    Found(Vec<Entry>),
    Error { message: String },
    /// `fuzzy`: nothing matched as typed, and the index found these close matches.
    Finished { count: usize, elapsed: f64, limited: bool, fuzzy: bool },
}

fn tell(channel: &Channel<FindEvent>, event: FindEvent) {
    let _ = channel.send(event);
}

/// The searches in flight, one per lane (each tab searches on its own), so a newer search in the
/// same tab, or Stop, can end one.
#[derive(Clone, Default)]
pub struct Finds(Arc<Mutex<HashMap<String, Run>>>);

/// One search: the flag that stops the index's answer, what ends its wait for the service at once, and fd's
/// process.
#[derive(Clone, Default)]
struct Run {
    stop: Arc<AtomicBool>,
    index: Arc<IndexCancel>,
    fd: Slot,
}

#[cfg(windows)]
type IndexCancel = shadui_index::client::Cancel;
#[cfg(not(windows))]
#[derive(Default)]
struct IndexCancel;

impl Run {
    fn end(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // The service's answer isn't waited for: its pipe closes now, and the service stops scanning for it.
        #[cfg(windows)]
        self.index.cancel();
        self.fd.kill();
    }
}

impl Finds {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Run>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts a search in `lane`, ending the one before it there.
    fn start(&self, lane: &str) -> Run {
        let run = Run::default();
        let previous = self.lock().insert(lane.to_string(), run.clone());
        if let Some(previous) = previous {
            previous.end();
        }
        run
    }

    /// Ends the lane's search, if one runs.
    fn stop(&self, lane: &str) {
        let run = self.lock().remove(lane);
        if let Some(run) = run {
            run.end();
        }
    }

    /// Ends every lane's search.
    pub fn stop_all(&self) {
        let runs: Vec<Run> = self.lock().drain().map(|(_, run)| run).collect();
        for run in runs {
            run.end();
        }
    }

    /// Forgets `run` once it's done, unless a newer search in the lane replaced it.
    fn finished(&self, lane: &str, run: &Run) {
        let mut runs = self.lock();
        if runs.get(lane).is_some_and(|current| Arc::ptr_eq(&current.stop, &run.stop)) {
            runs.remove(lane);
        }
    }
}

/// fd arguments for a query: every term must match the file name (like Everything), case-insensitively.
/// Terms with * or ? are globs over the whole name; the rest match anywhere in it.
pub(crate) fn fd_args(query: &str, roots: &[String], options: &FindOptions) -> Result<Vec<String>, String> {
    let (terms, filters) = parse(query)?;
    if terms.is_empty() && filters.is_empty() {
        return Err("Type part of a file name to search for".into());
    }
    let full_path = terms.iter().any(|t| shadui_index::query::is_path_term(t));
    // Filters alone search every name.
    let glob = !full_path && (terms.is_empty() || terms.iter().any(|t| t.contains(['*', '?'])));
    let mut patterns = terms.into_iter().map(|t| {
        if full_path {
            full_path_regex(&t)
        } else if glob && !t.contains(['*', '?']) {
            format!("*{t}*")
        } else {
            t
        }
    });

    let mut args: Vec<String> = ["--color=never", "--absolute-path", "--ignore-case"].map(String::from).into();
    // Over full paths, fd's globs don't reach across `\`: its patterns are regular expressions there.
    if full_path {
        args.push("--full-path".into());
    } else {
        args.push(if glob { "--glob" } else { "--fixed-strings" }.into());
    }
    if options.hidden {
        args.push("--hidden".into());
    }
    if !options.respect_ignore {
        args.push("--no-ignore".into());
    }
    for exclude in options.exclude.iter().filter(|e| !e.trim().is_empty()) {
        args.push(format!("--exclude={}", exclude.trim()));
    }
    if let Some(max) = options.max_results.filter(|&n| n > 0) {
        args.push(format!("--max-results={max}"));
    }
    // The index's filters as fd's flags: its --size counts files only, as the index does.
    args.extend(filters.exts.iter().map(|ext| format!("--extension={ext}")));
    args.extend(filters.min_size.map(|min| format!("--size=+{min}b")));
    args.extend(filters.max_size.map(|max| format!("--size=-{max}b")));
    args.extend(filters.newer_than.map(|secs| format!("--changed-within={secs}s")));
    args.extend(filters.older_than.map(|secs| format!("--changed-before={secs}s")));
    if filters.files_only() {
        args.push("--type=file".into());
    } else if filters.kind == Some(Kind::Folders) {
        args.push("--type=directory".into());
    }
    let first = patterns.next().unwrap_or_else(|| "*".into());
    args.extend(patterns.map(|p| format!("--and={p}")));
    args.push("--".into());
    args.push(first);
    args.extend(roots.iter().cloned());
    Ok(args)
}

/// A term as a regular expression over the full paths fd matches with --full-path, as the index
/// matches it: a path term anywhere in the path, a name term in its last part. `*` is any run of
/// characters (within the name, for a name term), `?` one.
fn full_path_regex(term: &str) -> String {
    let path = shadui_index::query::is_path_term(term);
    let (any, one) = if path { (".*", ".") } else { (r"[^\\/]*", r"[^\\/]") };
    let body: String = shadui_index::query::clean_term(term)
        .chars()
        .map(|c| match c {
            '*' => any.into(),
            '?' => one.into(),
            c => regex::escape(c.encode_utf8(&mut [0; 4])),
        })
        .collect();
    match (path, term.contains(['*', '?'])) {
        (true, _) => body,
        (false, true) => format!(r"(?:^|[\\/]){body}$"),
        (false, false) => format!(r"{body}[^\\/]*$"),
    }
}

/// A path as the searches key it: without its end separator, in lower case.
fn key_of(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

/// What a query asks of fd, and of the marks: the query without its marks terms (fd reads names; "*" when
/// nothing else is left), and the filter its marks terms make (glint-model's, which agrees with the index's).
fn fd_side(query: &str) -> (String, Wanted) {
    let (wanted, _) = Wanted::parse(&glint_model::entries::terms(&query.to_lowercase()));
    if wanted.is_empty() {
        return (query.to_string(), wanted);
    }
    let rest = without_marks(query);
    (if rest.trim().is_empty() { "*".to_string() } else { rest }, wanted)
}

/// What a search with marks terms asks of each item: the filter, every folder's `.glint-marks` read once for
/// the whole search (four threads share them), and the ledger as it was when the search began.
struct MarkedBy {
    wanted: Wanted,
    folders: Mutex<HashMap<String, Arc<OnceLock<Option<Beside>>>>>,
    /// By path as `key_of` makes it.
    ledger: HashMap<String, Line>,
    #[cfg(test)]
    sidecar_reads: AtomicUsize,
}

impl MarkedBy {
    fn new(wanted: Wanted) -> Self {
        let lines = crate::marks::ledger::shared().lock().unwrap_or_else(|e| e.into_inner()).lines().cloned().collect();
        Self::with(wanted, lines)
    }

    fn with(wanted: Wanted, lines: Vec<Line>) -> Self {
        let ledger = lines.into_iter().map(|line| (key_of(&line.name), line)).collect();
        MarkedBy {
            wanted,
            folders: Mutex::new(HashMap::new()),
            ledger,
            #[cfg(test)]
            sidecar_reads: AtomicUsize::new(0),
        }
    }

    /// The folder's sidecar: read by whichever thread asks first, while the others asking for it wait.
    fn sidecar(&self, folder: &Path) -> Arc<OnceLock<Option<Beside>>> {
        let key = folder.to_string_lossy().to_lowercase();
        let cell = self.folders.lock().unwrap_or_else(|e| e.into_inner()).entry(key).or_default().clone();
        cell.get_or_init(|| {
            #[cfg(test)]
            self.sidecar_reads.fetch_add(1, Ordering::SeqCst);
            crate::marks::beside::read(folder)
        });
        cell
    }

    /// An item's marks, from the first home that has any (as `marks::read` does).
    fn marks(&self, path: &str) -> Marks {
        if let Some(marks) = crate::marks::streams::read(path) {
            return tidy(&marks);
        }
        let item = Path::new(path.trim_end_matches(['\\', '/']));
        if let (Some(folder), Some(name)) = (item.parent().filter(|f| !f.as_os_str().is_empty()), item.file_name()) {
            let side = self.sidecar(folder);
            let name = name.to_string_lossy();
            if let Some(line) = side.get().and_then(Option::as_ref).and_then(|b| crate::marks::beside::line(b, &name)).filter(|l| !l.marks.is_empty()) {
                return tidy(&line.marks);
            }
        }
        self.ledger.get(&key_of(path)).filter(|l| !l.marks.is_empty()).map(|l| tidy(&l.marks)).unwrap_or_default()
    }

    fn accepts(&self, path: &str) -> bool {
        self.wanted.accepts(&self.marks(path))
    }

    /// The ledger's items under `roots` that the filter wants and whose names hold `names`, which
    /// `seen` (keyed by `key_of`) doesn't have yet: what fd can't see.
    fn ledger_adds(&self, roots: &[String], names: &[String], seen: &HashSet<String>) -> Vec<Entry> {
        let matcher = NameMatcher::from_terms(names.to_vec());
        let roots: Vec<String> = roots.iter().map(|r| key_of(r)).collect();
        let mut found: Vec<Entry> = Vec::new();
        for (key, line) in &self.ledger {
            let under = roots.iter().any(|r| key.len() > r.len() && key.starts_with(r.as_str()) && key[r.len()..].starts_with(['\\', '/']));
            let marks = tidy(&line.marks);
            if !under || seen.contains(key) || marks.is_empty() || !self.wanted.accepts(&marks) {
                continue;
            }
            let path = line.name.trim_end_matches(['\\', '/']);
            let name = Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
            let lower = name.to_lowercase();
            let folded = (!name.is_ascii()).then_some(lower.as_bytes());
            if matcher.matches_in(name.as_bytes(), folded, key.as_bytes()) {
                found.push(entry_for_path(path, false));
            }
        }
        found.sort_by(|a, b| a.path.cmp(&b.path));
        found
    }
}

/// Sends results in batches: up to 10,000 at a time, at least every 40 ms.
struct Out<'a> {
    channel: &'a Channel<FindEvent>,
    batch: Vec<Entry>,
    sent: Instant,
    count: usize,
    started: bool,
    /// With property filters (width:, taken:…): what's found goes to be read first (`check`), not
    /// straight to the app.
    /// The entry travels with whether its marks are still to be checked (what fd found: the index reads
    /// marks itself).
    check: Option<mpsc::SyncSender<(Entry, bool)>>,
    /// Set while fd's results are sent: they are checked against the marks filter by the same threads.
    marks: bool,
}

impl<'a> Out<'a> {
    fn new(channel: &'a Channel<FindEvent>, check: Option<mpsc::SyncSender<(Entry, bool)>>) -> Self {
        Out { channel, batch: Vec::new(), sent: Instant::now(), count: 0, started: false, check, marks: false }
    }

    fn started(&mut self, engine: &'static str) {
        if !std::mem::replace(&mut self.started, true) {
            tell(self.channel, FindEvent::Started { engine });
        }
    }

    fn push(&mut self, entry: Entry) {
        self.count += 1;
        if let Some(check) = &self.check {
            let _ = check.send((entry, self.marks));
            return;
        }
        self.batch.push(entry);
        if self.batch.len() >= 10_000 || self.sent.elapsed() >= Duration::from_millis(40) {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if !self.batch.is_empty() {
            let _ = self.channel.send(FindEvent::Found(std::mem::take(&mut self.batch)));
            self.sent = Instant::now();
        }
    }
}

/// What the index answered: the folders it couldn't (searched with fd instead), whether it stopped at
/// the result limit, and whether these are close matches.
struct Answered {
    skipped: Vec<String>,
    limited: bool,
    fuzzy: bool,
}

/// Asks the index service. None when it isn't running, isn't allowed, or covers none of `roots`.
#[cfg(windows)]
fn search_index(roots: &[String], query: &str, options: &FindOptions, stop: &AtomicBool, cancel: &IndexCancel, out: &mut Out) -> Option<Answered> {
    use shadui_index::client::{self, Event};
    let request = shadui_index::proto::SearchRequest {
        query: query.to_string(),
        roots: roots.to_vec(),
        hidden: options.hidden,
        exclude: options.exclude.clone(),
        max_results: options.max_results,
        fuzzy: options.fuzzy,
        rank: options.rank,
        marks: false,
    };
    let mut answered = None;
    // A failure halfway keeps what arrived; one before the plan leaves everything to fd.
    let _ = client::search_until(&request, cancel, |event| {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        match event {
            Event::Plan { indexed, skipped } => {
                if indexed.is_empty() {
                    return false;
                }
                out.started(if skipped.is_empty() { "index" } else { "index + fd" });
                answered = Some(Answered { skipped, limited: false, fuzzy: false });
            }
            Event::Hits(hits) => {
                for hit in hits {
                    let name = hit.path.rsplit('\\').next().unwrap_or(&hit.path).to_string();
                    out.push(Entry { name, path: Some(hit.path), dir: hit.dir, size: hit.size, modified: hit.modified, attrs: hit.attrs });
                }
            }
            Event::Done { limited, fuzzy, .. } => {
                if let Some(answered) = answered.as_mut() {
                    (answered.limited, answered.fuzzy) = (limited, fuzzy);
                }
            }
        }
        true
    });
    answered
}

/// Whether the running service is this app's version (so it reads every filter this app knows). Asked before
/// every search with a filter or a path (as each key is typed), and the service counts every record to answer,
/// so the answer is kept a minute; a service that didn't answer is asked again next time.
#[cfg(windows)]
pub(crate) fn same_service() -> bool {
    static KNOWN: std::sync::Mutex<Option<(bool, std::time::Instant)>> = std::sync::Mutex::new(None);
    const KNOWN_FOR: std::time::Duration = std::time::Duration::from_secs(60);
    if let Some((same, at)) = *KNOWN.lock().unwrap_or_else(|e| e.into_inner()) {
        if at.elapsed() < KNOWN_FOR {
            return same;
        }
    }
    let Ok(status) = shadui_index::client::status() else { return false };
    let same = status.version == shadui_index::VERSION;
    *KNOWN.lock().unwrap_or_else(|e| e.into_inner()) = Some((same, std::time::Instant::now()));
    same
}

#[cfg(not(windows))]
pub(crate) fn same_service() -> bool {
    false
}

/// Whether the running service reads all of a query: an older one reads ext: and the like, and paths
/// (src\lib, path:docs), as part of a name and finds nothing, so fd answers those until it's updated.
/// Asking it can wait on its pipe.
pub(crate) fn service_reads(terms: &[String], filters: &Filters) -> bool {
    any_service_reads(terms, filters) || same_service()
}

/// A query of names alone, which every version of the service reads.
fn any_service_reads(terms: &[String], filters: &Filters) -> bool {
    filters.is_empty() && !terms.iter().any(|t| shadui_index::query::is_path_term(t))
}

#[cfg(not(windows))]
fn search_index(_: &[String], _: &str, _: &FindOptions, _: &AtomicBool, _: &IndexCancel, _: &mut Out) -> Option<Answered> {
    None
}

/// Searches `roots` with fd, streaming what it finds. Returns whether it hit `options.max_results`.
fn search_fd(slot: &Slot, roots: &[String], query: &str, options: &FindOptions, out: &mut Out) -> Result<bool, String> {
    let args = fd_args(query, roots, options)?;
    let mut command = crate::proc::tool("fd")?;
    command
        .args(&args)
        // fd prints "/" separators when it thinks it runs under MSYS (e.g. a dev build started from Git Bash).
        .env_remove("MSYSTEM")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = crate::proc::spawn_tool(&mut command).map_err(|e| format!("Failed to start fd: {e}"))?;
    let stdout = child.stdout.take().ok_or("Failed to open fd's output")?;
    let mut stderr = child.stderr.take().ok_or("Failed to open fd's errors")?;
    let pid = slot.replace(child);

    let stderr_reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let before = out.count;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        // fd marks folders with a trailing separator.
        let dir = line.ends_with(['\\', '/']);
        let path = line.trim_end_matches(['\\', '/']);
        if !path.is_empty() {
            out.push(entry_for_path(path, dir));
        }
    }
    let found = out.count - before;
    let errors = stderr_reader.join().unwrap_or_default();
    // A stopped search ends quietly, and so do warnings about folders fd can't open (it still exits
    // 0); only a run that failed (a missing folder, a bad pattern) reports what fd said.
    if slot.finish(pid).is_some_and(|status| !status.success()) && found == 0 && !errors.trim().is_empty() {
        return Err(errors.trim().to_string());
    }
    Ok(options.max_results.is_some_and(|max| max > 0 && found >= max))
}

/// Searches `roots` by name for the tab `lane`, stopping that tab's previous search (another tab's
/// keeps going).
pub async fn find_files(
    state: State<'_, AppState>,
    roots: Vec<String>,
    query: String,
    options: FindOptions,
    on_event: Channel<FindEvent>,
    lane: String,
) -> Result<(), String> {
    let (terms, filters) = parse(&query)?;
    if terms.is_empty() && filters.is_empty() {
        return Err("Type part of a file name to search for".into());
    }
    // What photos, videos and songs say of themselves (width:, taken:, artist:…) is read here: the
    // index and fd get the query without it, narrowed to the files that can say it.
    let props = filters.props.clone();
    // Whether the marks (and the names) are all a query asks: only then can what only this PC knows of be judged.
    let marks_only = Filters { marks: Default::default(), tags: Vec::new(), props: Vec::new(), ..filters.clone() } == Filters::default();
    let (query, filters) = if props.is_empty() {
        (query, filters)
    } else {
        let exts = crate::media::exts_for(&props);
        if exts.is_empty() {
            return Err("No file is all of that: width:, height:, taken: and camera: go with photos and videos, artist: and album: with songs, duration: with videos and songs.".into());
        }
        let mut narrowed = without_props(&query);
        if filters.exts.is_empty() {
            narrowed = format!("{narrowed} ext:{}", exts.join(";")).trim().to_string();
        }
        let filters = parse(&narrowed)?.1;
        (narrowed, filters)
    };
    // What the marks terms ask is read here for what fd finds (and for what only this PC knows of): the index
    // reads them itself, fd is given the query without them.
    let (_, wanted_marks) = fd_side(&query);
    let run = state.finds.start(&lane);
    let finds = state.finds.clone();
    thread::spawn(move || {
        // Until an older service is updated, fd answers queries with filters or paths. Asking it can wait
        // on its pipe, so it's asked here.
        let index_reads = service_reads(&terms, &filters);
        let (stop, slot) = (&run.stop, &run.fd);
        let started = Instant::now();
        // With properties to check, the index and fd find every candidate, which four threads read
        // (each file once a session): the limit is on what holds them.
        let wanted = options.max_results.filter(|&m| m > 0);
        let checking = !props.is_empty();
        // Properties and marks are read by four threads (each file once a session) after the index and fd.
        let marked = (!wanted_marks.is_empty()).then(|| MarkedBy::new(wanted_marks));
        let threaded = checking || marked.is_some();
        let seen: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
        let options = if threaded { FindOptions { max_results: None, ..options } } else { options };
        let (to_check, checks) = mpsc::sync_channel::<(Entry, bool)>(1024);
        let checks = Mutex::new(checks);
        let held = AtomicUsize::new(0);
        let (count, limited, fuzzy) = thread::scope(|scope| {
            for _ in 0..if threaded { 4 } else { 0 } {
                let channel = on_event.clone();
                let (checks, held, props, marked, seen) = (&checks, &held, &props, &marked, &seen);
                scope.spawn(move || {
                    let mut out = Out::new(&channel, None);
                    out.started = true;
                    let next = || checks.lock().unwrap_or_else(|e| e.into_inner()).recv();
                    while let Ok((entry, marks_too)) = next() {
                        // Stopped: what's left is passed over, so what finds it never waits.
                        if stop.load(Ordering::SeqCst) {
                            continue;
                        }
                        let path = entry.path.as_deref();
                        if !props.is_empty() && !path.is_some_and(|path| crate::media::matches(path, entry.size, entry.modified, props)) {
                            continue;
                        }
                        if let Some(marked) = marked {
                            if marks_too && !path.is_some_and(|path| marked.accepts(path)) {
                                continue;
                            }
                            if let Some(path) = path {
                                seen.lock().unwrap_or_else(|e| e.into_inner()).insert(key_of(path));
                            }
                        }
                        if wanted.is_some_and(|w| held.fetch_add(1, Ordering::SeqCst) >= w) {
                            // The limit: the index and fd stop too.
                            stop.store(true, Ordering::SeqCst);
                            slot.kill();
                            continue;
                        }
                        if wanted.is_none() {
                            held.fetch_add(1, Ordering::SeqCst);
                        }
                        out.push(entry);
                    }
                    out.flush();
                });
            }
            let mut out = Out::new(&on_event, threaded.then_some(to_check));
            let mut fd_roots = roots.clone();
            let (mut limited, mut fuzzy) = (false, false);
            // A folder in an archive: searched in the archive's own list of entries.
            if let [root] = roots.as_slice() {
                let searched = crate::archive::search(root, &query, options.hidden, &options.exclude, options.max_results, stop, &mut |entry| {
                    if !checking {
                        out.push(entry)
                    }
                });
                if let Some(searched) = searched {
                    match searched {
                        Ok(_) if checking => tell(&on_event, FindEvent::Error { message: "Photo, video and music filters don't look inside archives: they read files on disk.".into() }),
                        Ok((engine, hit_limit)) => {
                            out.started(engine);
                            limited = hit_limit;
                        }
                        Err(message) => {
                            out.flush();
                            tell(&on_event, FindEvent::Error { message });
                        }
                    }
                    out.flush();
                    return (out.count, limited, false);
                }
            }
            // .gitignore rules are fd's alone; anything else goes to the index first.
            if !options.respect_ignore && index_reads {
                if let Some(answered) = search_index(&roots, &query, &options, stop, &run.index, &mut out) {
                    (fd_roots, limited, fuzzy) = (answered.skipped, answered.limited, answered.fuzzy);
                }
            }
            let left = options.max_results.filter(|&m| m > 0).map(|m| m.saturating_sub(out.count));
            if !fd_roots.is_empty() && !stop.load(Ordering::SeqCst) && left != Some(0) {
                out.started("fd");
                // fd doesn't read marks: it gets the rest of the query, and what it finds is checked.
                let (fd_query, _) = fd_side(&query);
                out.marks = marked.is_some();
                let options = FindOptions { max_results: left, ..options };
                match search_fd(slot, &fd_roots, &fd_query, &options, &mut out) {
                    Ok(hit_limit) => limited |= hit_limit,
                    Err(message) => {
                        out.flush();
                        tell(&on_event, FindEvent::Error { message });
                    }
                }
            } else if !fd_roots.is_empty() {
                limited = true;
            }
            out.flush();
            (out.count, limited, fuzzy)
        });
        // Checked: as many as held, up to the limit.
        let (mut count, mut limited) = if threaded {
            let held = held.into_inner();
            (wanted.map_or(held, |w| held.min(w)), wanted.is_some_and(|w| held > w))
        } else {
            (count, limited)
        };
        // What only this PC knows of (marks in the ledger: inside an archive, on a remote) joins what was found.
        if let Some(marked) = marked.as_ref().filter(|_| marks_only && !run.stop.load(Ordering::SeqCst)) {
            let seen = seen.into_inner().unwrap_or_else(|e| e.into_inner());
            let mut more: Vec<Entry> = marked.ledger_adds(&roots, &terms, &seen);
            more.retain(|e| props.is_empty() || e.path.as_deref().is_some_and(|p| crate::media::matches(p, e.size, e.modified, &props)));
            let room = wanted.map_or(usize::MAX, |w| w.saturating_sub(count));
            limited |= more.len() > room;
            more.truncate(room);
            count += more.len();
            if !more.is_empty() {
                tell(&on_event, FindEvent::Found(more));
            }
        }
        tell(&on_event, FindEvent::Finished { count, elapsed: ms_since(started), limited, fuzzy });
        finds.finished(&lane, &run);
    });
    Ok(())
}

/// Stops the tab's search (Stop, a new folder, or the tab closed).
pub fn cancel_find(state: State<AppState>, lane: String) {
    state.finds.stop(&lane);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> FindOptions {
        FindOptions { hidden: false, respect_ignore: false, exclude: vec!["node_modules".into(), " ".into()], max_results: Some(100), fuzzy: false, rank: false }
    }

    #[test]
    fn builds_fixed_string_and_glob_queries() {
        let roots = vec![r"C:\Users".to_string()];
        let args = fd_args("budget 2026", &roots, &options()).unwrap();
        assert!(args.contains(&"--fixed-strings".into()));
        assert!(args.contains(&"--no-ignore".into()));
        assert!(args.contains(&"--exclude=node_modules".into()));
        assert!(args.contains(&"--max-results=100".into()));
        assert!(args.ends_with(&["--and=2026".into(), "--".into(), "budget".into(), r"C:\Users".into()]));

        let args = fd_args("*.pdf -draft", &roots, &options()).unwrap();
        assert!(args.contains(&"--glob".into()));
        assert!(args.ends_with(&["--and=*-draft*".into(), "--".into(), "*.pdf".into(), r"C:\Users".into()]));
        assert!(fd_args("  ", &roots, &options()).is_err());

        let args = fd_args("ext:pdf size:>1kb dm:7d", &roots, &options()).unwrap();
        for flag in ["--glob", "--extension=pdf", "--size=+1025b", "--changed-within=604800s", "--type=file"] {
            assert!(args.contains(&flag.into()), "{flag}");
        }
        assert!(args.ends_with(&["--".into(), "*".into(), r"C:\Users".into()]));
        assert!(fd_args("report folder: dm:>1y", &roots, &options()).unwrap().contains(&"--type=directory".into()));
        assert!(fd_args("size:huge", &roots, &options()).is_err());

        // A path term matches the full path, and the names in its last part, by regular expressions.
        let args = fd_args("src/lib util *.ts", &roots, &options()).unwrap();
        assert!(args.contains(&"--full-path".into()) && !args.contains(&"--glob".into()) && !args.contains(&"--fixed-strings".into()));
        assert!(args.ends_with(&[r"--and=util[^\\/]*$".into(), r"--and=(?:^|[\\/])[^\\/]*\.ts$".into(), "--".into(), r"src\\lib".into(), r"C:\Users".into()]));
        assert!(fd_args(r"path:docs\*.md", &roots, &options()).unwrap().ends_with(&["--".into(), r"docs\\.*\.md".into(), r"C:\Users".into()]));
    }

    #[test]
    fn an_older_service_is_asked_only_for_names() {
        let reads = |query: &str| {
            let (terms, filters) = parse(query).unwrap();
            any_service_reads(&terms, &filters)
        };
        assert!(reads("budget 2026") && reads("*.pdf -draft"));
        for query in ["ext:pdf", "report folder:", r"src\lib main", "src/lib", "path:docs", r"D:\photos"] {
            assert!(!reads(query), "{query}");
        }
    }

    #[test]
    fn a_new_search_stops_only_the_last_one_in_its_tab() {
        let finds = Finds::default();
        let stopped = |run: &Run| run.stop.load(Ordering::SeqCst);
        let first = finds.start("left");
        let other = finds.start("right");
        let second = finds.start("left");
        assert!(stopped(&first) && !stopped(&second) && !stopped(&other));

        // A search that ended by itself is forgotten, but never one that replaced it.
        finds.finished("left", &first);
        finds.stop("left");
        assert!(stopped(&second) && !stopped(&other));
        finds.finished("right", &other);
        assert!(finds.lock().is_empty());
        finds.stop("right");
        assert!(!stopped(&other));
    }

    fn line(path: &str, label: &str) -> Line {
        Line { name: path.into(), marks: Marks { label: Some(label.into()), ..Marks::default() }, ..Line::default() }
    }

    #[test]
    fn a_querys_marks_terms_are_taken_out_of_what_fd_is_asked_and_kept_as_the_filter() {
        let (fd, wanted) = fd_side("report label:red !tag:old");
        assert_eq!(fd, "report");
        let (expected, _) = Wanted::parse(&glint_model::entries::terms("label:red !tag:old"));
        assert_eq!(wanted, expected);
        assert!(!wanted.is_empty());
        // Marks terms alone leave fd every name; a query without any is as typed.
        assert_eq!(fd_side("has:comment").0, "*");
        let (plain, none) = fd_side("report 2026");
        assert_eq!((plain.as_str(), none.is_empty()), ("report 2026", true));
        assert!(fd_args(&fd, &["C:\\".to_string()], &options()).is_ok());
    }

    #[test]
    fn two_hundred_items_of_one_folder_read_its_sidecar_once() {
        let dir = std::env::temp_dir().join(format!("glint-find-marks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (wanted, _) = Wanted::parse(&["label:red".to_string()]);
        crate::marks::beside::change(&dir, &[("f7.txt".to_string(), Marks { label: Some("Red".into()), ..Marks::default() })], 1).unwrap_or_default();
        let marked = MarkedBy::with(wanted, Vec::new());
        let paths: Vec<String> = (0..200).map(|i| dir.join(format!("f{i}.txt")).to_string_lossy().into_owned()).collect();
        std::thread::scope(|scope| {
            for chunk in paths.chunks(50) {
                let marked = &marked;
                scope.spawn(move || chunk.iter().for_each(|p| {
                    let _ = marked.accepts(p);
                }));
            }
        });
        assert_eq!(marked.sidecar_reads.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ledgers_lines_under_a_searched_folder_that_pass_are_added_once_and_one_outside_is_not() {
        let (wanted, _) = Wanted::parse(&["label:red".to_string()]);
        let lines = vec![
            line(r"Z:\gone\inside.zip\report.txt", "Red"),
            line(r"Z:\gone\inside.zip\other.txt", "Red"),
            line(r"Z:\gone\inside.zip\blue.txt", "Blue"),
            line(r"Z:\gone\seen.txt", "Red"),
            line(r"Z:\elsewhere\report.txt", "Red"),
            line(r"Z:\gone-too\report.txt", "Red"),
        ];
        let marked = MarkedBy::with(wanted, lines);
        let roots = vec![r"Z:\gone".to_string()];
        let seen: HashSet<String> = [key_of(r"Z:\gone\seen.txt")].into();
        let paths = |found: Vec<Entry>| found.into_iter().filter_map(|e| e.path).collect::<Vec<_>>();
        assert_eq!(paths(marked.ledger_adds(&roots, &[], &seen)), [r"Z:\gone\inside.zip\other.txt", r"Z:\gone\inside.zip\report.txt"]);
        // With name terms the name must hold them.
        assert_eq!(paths(marked.ledger_adds(&roots, &["report".to_string()], &seen)), [r"Z:\gone\inside.zip\report.txt"]);
        assert!(marked.ledger_adds(&[r"Z:\nowhere".to_string()], &[], &seen).is_empty());
    }
}
