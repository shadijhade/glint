//! Clean up: what can go to free space below a folder.
//!
//! - **Developer junk**: build output and package caches that a project's own tools make again
//!   (`node_modules` next to a package.json, `target` next to a Cargo.toml…), told apart from
//!   folders that only share the name by the project file beside them (kondo's rule).
//! - **Empty folders**: nothing in them, down through their subfolders, but more empty folders.
//! - **Temporary files**: what programs leave behind (`*.tmp`, Office's `~$` owner files,
//!   Thumbs.db…), less what a program still has open.
//! - **Big files**: everything over a size.
//!
//! Found from the instant search index where it answers, otherwise with fd (and sized with dust).
//! Windows' own folder, the Recycle Bin and System Volume Information are left out. Nothing is
//! removed here: the UI recycles what's chosen, which Ctrl+Z takes back.

use crate::fsx::{blocking, ms_since};
use crate::proc::Slot;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// What tells a folder of build output or cache from any folder of that name.
#[derive(Debug, Clone, Copy)]
enum Marker {
    /// One of these files is beside it, in the project's folder.
    Beside(&'static [&'static str]),
    /// A file with one of these extensions is beside it.
    BesideExt(&'static [&'static str]),
    /// These folders are all beside it.
    BesideDirs(&'static [&'static str]),
    /// One of these files is in it.
    Inside(&'static [&'static str]),
    /// Its name alone: a cache nothing else is called.
    Anywhere,
}

/// A kind of developer junk: the folders' names, what proves them so, and what to call them.
struct Rule {
    names: &'static [&'static str],
    marker: Marker,
    what: &'static str,
}

const RULES: &[Rule] = &[
    Rule { names: &["node_modules", ".next", ".nuxt", ".svelte-kit", ".turbo", ".parcel-cache", ".angular", ".expo"], marker: Marker::Beside(&["package.json"]), what: "Node.js packages and builds" },
    Rule { names: &["target"], marker: Marker::Beside(&["Cargo.toml"]), what: "Rust builds" },
    Rule { names: &["target"], marker: Marker::Beside(&["pom.xml", "build.sbt"]), what: "Maven and sbt builds" },
    Rule { names: &[".gradle", "build"], marker: Marker::Beside(&["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"]), what: "Gradle builds" },
    Rule { names: &[".cxx", ".externalNativeBuild"], marker: Marker::Beside(&["build.gradle", "build.gradle.kts"]), what: "Android native builds" },
    // Before .NET's: a Unity project has .csproj files beside its Obj too.
    Rule { names: &["Library", "Temp", "Obj", "Logs", "MemoryCaptures"], marker: Marker::BesideDirs(&["Assets", "ProjectSettings"]), what: "Unity caches" },
    Rule { names: &["bin", "obj"], marker: Marker::BesideExt(&["csproj", "fsproj", "vbproj"]), what: ".NET builds" },
    Rule { names: &[".venv", "venv"], marker: Marker::Inside(&["pyvenv.cfg"]), what: "Python virtual environments" },
    Rule { names: &["__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache", ".hypothesis", ".nox", ".ipynb_checkpoints"], marker: Marker::Anywhere, what: "Python and Jupyter caches" },
    Rule { names: &[".tox"], marker: Marker::Beside(&["tox.ini", "pyproject.toml", "setup.cfg"]), what: "Python and Jupyter caches" },
    Rule { names: &["Binaries", "Intermediate", "Saved", "DerivedDataCache"], marker: Marker::BesideExt(&["uproject"]), what: "Unreal Engine caches" },
    Rule { names: &["build", "out", "cmake-build-debug", "cmake-build-release"], marker: Marker::Inside(&["CMakeCache.txt"]), what: "CMake builds" },
    Rule { names: &[".terraform"], marker: Marker::BesideExt(&["tf"]), what: "Terraform providers" },
    Rule { names: &[".dart_tool", "build"], marker: Marker::Beside(&["pubspec.yaml"]), what: "Dart and Flutter builds" },
    Rule { names: &[".build", ".swiftpm"], marker: Marker::Beside(&["Package.swift"]), what: "Swift builds" },
    Rule { names: &["_build", ".elixir_ls"], marker: Marker::Beside(&["mix.exs"]), what: "Elixir builds" },
    Rule { names: &[".stack-work"], marker: Marker::Beside(&["stack.yaml"]), what: "Haskell Stack builds" },
    Rule { names: &[".zig-cache", "zig-cache", "zig-out"], marker: Marker::Beside(&["build.zig"]), what: "Zig caches and builds" },
    Rule { names: &[".godot"], marker: Marker::Beside(&["project.godot"]), what: "Godot caches" },
    Rule { names: &["elm-stuff"], marker: Marker::Beside(&["elm.json"]), what: "Elm packages" },
];

/// Every folder name a rule looks for, each once.
fn names() -> Vec<&'static str> {
    let mut seen = HashSet::new();
    RULES.iter().flat_map(|r| r.names.iter().copied()).filter(|n| seen.insert(n.to_ascii_lowercase())).collect()
}

/// Whether `marker` holds for the folder `dir` (named for a rule): the files beside or in it.
fn holds(marker: Marker, dir: &Path) -> bool {
    let parent = dir.parent();
    match marker {
        Marker::Anywhere => true,
        Marker::Beside(files) => parent.is_some_and(|p| files.iter().any(|f| p.join(f).is_file())),
        Marker::Inside(files) => files.iter().any(|f| dir.join(f).is_file()),
        Marker::BesideDirs(dirs) => parent.is_some_and(|p| dirs.iter().all(|d| p.join(d).is_dir())),
        Marker::BesideExt(exts) => parent.and_then(|p| std::fs::read_dir(p).ok()).is_some_and(|entries| {
            entries.flatten().any(|e| e.path().extension().is_some_and(|x| exts.iter().any(|ext| x.eq_ignore_ascii_case(ext))) && e.file_type().is_ok_and(|t| t.is_file()))
        }),
    }
}

/// The rule a folder at `path` falls under, if any: its name, then what's beside it.
fn rule_for(path: &Path) -> Option<&'static Rule> {
    let name = path.file_name()?.to_string_lossy();
    RULES.iter().find(|r| r.names.iter().any(|n| n.eq_ignore_ascii_case(&name)) && holds(r.marker, path))
}

/// A name programs give what they leave behind.
#[derive(Debug, Clone, Copy)]
enum Name {
    /// Any with this extension: `*.tmp`.
    Ext(&'static str),
    /// Any starting so: `~$*`.
    Prefix(&'static str),
    Exact(&'static str),
    /// Any starting and ending so: `.~lock.*#`.
    Around(&'static str, &'static str),
}

impl Name {
    /// As the index and fd read it: a whole-name glob.
    fn glob(self) -> String {
        match self {
            Name::Ext(ext) => format!("*.{ext}"),
            Name::Prefix(start) => format!("{start}*"),
            Name::Exact(name) => name.to_string(),
            Name::Around(start, end) => format!("{start}*{end}"),
        }
    }

    /// Whether a lowercase name is one.
    fn fits(self, name: &str) -> bool {
        match self {
            Name::Ext(ext) => name.len() > ext.len() + 1 && name.ends_with(ext) && name.as_bytes()[name.len() - ext.len() - 1] == b'.',
            Name::Prefix(start) => name.len() > start.len() && name.starts_with(start),
            Name::Exact(exact) => name == exact,
            Name::Around(start, end) => name.len() > start.len() + end.len() && name.starts_with(start) && name.ends_with(end),
        }
    }
}

/// What programs leave behind, and what to call it.
const LEFTOVERS: &[(Name, &str)] = &[
    (Name::Ext("tmp"), "Temporary files"),
    (Name::Ext("temp"), "Temporary files"),
    // Word, Excel and PowerPoint's ~$report.docx, there while the document is open.
    (Name::Prefix("~$"), "Office owner files"),
    (Name::Around(".~lock.", "#"), "LibreOffice lock files"),
    (Name::Exact("thumbs.db"), "Thumbnail caches"),
    (Name::Exact("ehthumbs.db"), "Thumbnail caches"),
    (Name::Exact("ehthumbs_vista.db"), "Thumbnail caches"),
    (Name::Exact(".ds_store"), "macOS folder settings"),
    // AppleDouble files: a Mac's resource forks, on a drive it wrote to.
    (Name::Prefix("._"), "macOS resource forks"),
];

/// What a file a program left behind is, by its name.
fn leftover(name: &str) -> Option<&'static str> {
    let name = name.to_lowercase();
    LEFTOVERS.iter().find(|(n, _)| n.fits(&name)).map(|(_, what)| *what)
}

/// What Clean up looks for.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Mode {
    /// Build output and package caches beside their projects.
    Junk,
    /// Folders with nothing in them but empty folders.
    Empty,
    /// What programs leave behind: [`LEFTOVERS`].
    Temporary,
    /// Files of `at_least` bytes or more.
    Big { at_least: u64 },
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub path: String,
    /// In bytes; a folder's is everything in it.
    pub size: u64,
    /// When it last changed (developer junk: when its project did, see [`last_changed`]); 0 when
    /// that couldn't be told.
    pub date: i64,
    /// What it is: "Rust builds", "Office owner files"…; empty for empty folders and big files.
    pub kind: &'static str,
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub items: Vec<Found>,
    pub total: u64,
    /// Who found (and sized) them: "index", "fd + dust", "index + fd"…
    pub engine: String,
    pub elapsed: f64,
    /// How many more were found than [`MAX_LISTED`] (the biggest are listed).
    pub more: usize,
    /// Temporary files left out because a program has them open (or this account can't delete them).
    pub held: usize,
}

/// The most a report lists.
const MAX_LISTED: usize = 5_000;

/// Which tools found and sized what's in a report.
#[derive(Default)]
struct Engines {
    index: bool,
    fd: bool,
    dust: bool,
}

impl Engines {
    fn name(&self) -> String {
        [(self.index, "index"), (self.fd, "fd"), (self.dust, "dust")].iter().filter(|(on, _)| *on).map(|(_, name)| *name).collect::<Vec<_>>().join(" + ")
    }
}

/// A report of `items`, biggest first (or by path: empty folders are all 0 bytes), cut to [`MAX_LISTED`].
fn report(mut items: Vec<Found>, by_path: bool, engines: Engines, held: usize, started: Instant) -> Report {
    if by_path {
        items.sort_by_cached_key(|f| f.path.to_lowercase());
    } else {
        items.sort_by_key(|f| Reverse(f.size));
    }
    let more = items.len().saturating_sub(MAX_LISTED);
    items.truncate(MAX_LISTED);
    let total = items.iter().map(|f| f.size).sum();
    Report { items, total, engine: engines.name(), elapsed: ms_since(started), more, held }
}

/// Folders never looked into, wherever they are.
const OFF_LIMITS: &[&str] = &["$Recycle.Bin", "System Volume Information"];

/// Windows' own folder, lowercase: `c:\windows`.
static WINDOWS: LazyLock<String> = LazyLock::new(|| std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).trim_end_matches('\\').to_lowercase());

/// Whether `path` is where Clean up never looks: Windows' own folder, or one of [`OFF_LIMITS`].
fn off_limits(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.strip_prefix(WINDOWS.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with('\\')) || lower.split('\\').any(|part| OFF_LIMITS.iter().any(|o| part.eq_ignore_ascii_case(o)))
}

/// A folder as [`below`] takes it: lowercase, without a trailing `\` unless it's a drive's (`c:\`).
fn folder_key(path: &str) -> String {
    let trimmed = path.trim_end_matches('\\').to_lowercase();
    if trimmed.ends_with(':') {
        trimmed + "\\"
    } else {
        trimmed
    }
}

/// Whether the lowercase `path` is below the lowercase folder `top` (`c:\` or `c:\repos`).
fn below(top: &str, path: &str) -> bool {
    path.len() > top.len() && path.starts_with(top) && (top.ends_with('\\') || path.as_bytes()[top.len()] == b'\\')
}

fn ms_of(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
}

/// A file's size, date and attributes, for what fd found.
fn stat_file(path: &str) -> Option<(u64, i64, u32)> {
    let meta = std::fs::symlink_metadata(path).ok().filter(|m| m.is_file())?;
    #[cfg(windows)]
    let attrs = std::os::windows::fs::MetadataExt::file_attributes(&meta);
    #[cfg(not(windows))]
    let attrs = 0;
    Some((meta.len(), ms_of(&meta), attrs))
}

/// FILE_ATTRIBUTE_SYSTEM: the page file, the hibernation file…
const SYSTEM: u32 = 0x4;

/// Cleanups in flight, one per tab, so another scan there or Stop ends one.
#[derive(Clone, Default)]
struct Run {
    stop: Arc<AtomicBool>,
    child: Slot,
}

impl Run {
    fn stopped(&self) -> Result<(), String> {
        if self.stop.load(Ordering::Relaxed) {
            Err("cancelled".into())
        } else {
            Ok(())
        }
    }
}

static RUNS: LazyLock<Mutex<HashMap<String, Run>>> = LazyLock::new(Default::default);

fn start(lane: &str) -> Run {
    let run = Run::default();
    if let Some(old) = RUNS.lock().unwrap_or_else(|e| e.into_inner()).insert(lane.to_string(), run.clone()) {
        old.stop.store(true, Ordering::SeqCst);
        old.child.kill();
    }
    run
}

#[tauri::command]
pub fn cancel_cleanup(lane: String) {
    if let Some(run) = RUNS.lock().unwrap_or_else(|e| e.into_inner()).remove(&lane) {
        run.stop.store(true, Ordering::SeqCst);
        run.child.kill();
    }
}

/// Runs each of `queries` on the index below `root`, handing it every hit. What below `root` it
/// can't answer for (for fd), or None when the service isn't this version (an older one reads
/// "folder:" as part of a name) or answers for none of it.
#[cfg(windows)]
fn index_search(root: &str, queries: &[String], hidden: bool, skip: &[&str], stop: &AtomicBool, mut each: impl FnMut(shadui_index::proto::Hit)) -> Option<Vec<String>> {
    use shadui_index::client::{self, Event};
    if !crate::find::same_service() {
        return None;
    }
    let mut skipped = None;
    let exclude: Vec<String> = OFF_LIMITS.iter().chain(skip).map(|n| n.to_string()).collect();
    for query in queries {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        let request = shadui_index::proto::SearchRequest { query: query.clone(), roots: vec![root.to_string()], hidden, exclude: exclude.clone(), max_results: None };
        let mut answered = false;
        let ran = client::search(&request, |event| {
            match event {
                Event::Plan { indexed, skipped: rest } => {
                    answered = !indexed.is_empty();
                    skipped.get_or_insert(rest);
                    return answered;
                }
                Event::Hits(hits) => hits.into_iter().for_each(&mut each),
                Event::Done { .. } => {}
            }
            !stop.load(Ordering::Relaxed)
        });
        if ran.is_err() || !answered {
            return None;
        }
    }
    Some(skipped.unwrap_or_default())
}

#[cfg(not(windows))]
fn index_search(_: &str, _: &[String], _: bool, _: &[&str], _: &AtomicBool, _: impl FnMut(shadui_index::proto::Hit)) -> Option<Vec<String>> {
    None
}

/// What fd prints for `flags` (and `pattern`) below `roots`, in one run, not looking into
/// [`OFF_LIMITS`] or folders named as in `skip`.
fn fd_paths(flags: &[&str], pattern: Option<&str>, roots: &[String], skip: &[&str], run: &Run) -> Result<Vec<String>, String> {
    let mut command = crate::proc::tool("fd")?;
    command.args(["--color=never", "--absolute-path", "--no-ignore"]).args(flags);
    for name in OFF_LIMITS.iter().chain(skip) {
        // As NTFS and FAT spell it: $Recycle.Bin, $RECYCLE.BIN.
        command.arg(format!("--exclude={name}")).arg(format!("--exclude={}", name.to_uppercase()));
    }
    match pattern {
        Some(pattern) => command.arg("--").arg(pattern).args(roots),
        None => roots.iter().fold(&mut command, |c, root| c.arg("--search-path").arg(root)),
    };
    command.env_remove("MSYSTEM").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = command.spawn().map_err(|e| format!("Failed to start fd: {e}"))?;
    let stdout = child.stdout.take().ok_or("Failed to open fd's output")?;
    let pid = run.child.replace(child);
    let mut found = Vec::new();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let path = line.trim_end_matches(['\\', '/']);
        if !path.is_empty() {
            found.push(path.to_string());
        }
    }
    run.child.finish(pid);
    run.stopped()?;
    Ok(found)
}

/// Sizes of `paths` (full paths) with dust: a run per hundred, so the command line stays short.
fn sizes(paths: &[String], run: &Run) -> Result<HashMap<String, u64>, String> {
    let mut sizes = HashMap::new();
    for chunk in paths.chunks(100) {
        let mut command = crate::proc::tool("dust")?;
        command.args(["-j", "-d", "0", "-s", "-o", "b", "-P", "--"]).args(chunk).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = command.spawn().map_err(|e| format!("Failed to start dust: {e}"))?;
        let mut stdout = child.stdout.take().ok_or("Failed to open dust's output")?;
        let pid = run.child.replace(child);
        let mut json = String::new();
        let _ = stdout.read_to_string(&mut json);
        run.child.finish(pid);
        run.stopped()?;
        let tree = crate::sizes::parse_tree(&json)?;
        // Several paths are children of a "(total)"; one is the tree itself.
        let nodes = if tree.name == "(total)" { tree.children } else { vec![tree] };
        let asked: Vec<String> = chunk.iter().map(|p| p.to_lowercase()).collect();
        for node in nodes {
            // dust tells same-named folders apart with their parent's name after them:
            // "…\web\node_modules(web)". The path asked for is what it's kept under.
            let name = node.name.to_lowercase();
            let key = asked.iter().find(|p| name == **p || name.strip_prefix(p.as_str()).is_some_and(|rest| rest.starts_with('(') && rest.ends_with(')'))).cloned().unwrap_or(name);
            sizes.insert(key, crate::sizes::bytes(&node.size));
        }
    }
    Ok(sizes)
}

/// A folder found by name, with its size when what found it knows it.
struct Candidate {
    path: String,
    size: Option<u64>,
}

/// Keeps the candidates that are junk, outermost only: sorted, a path comes after every folder it's
/// in, so what's inside a folder already taken is passed over without looking.
fn keep_junk(mut candidates: Vec<Candidate>) -> Vec<(Candidate, &'static str)> {
    candidates.sort_by_cached_key(|c| c.path.to_lowercase());
    candidates.dedup_by(|a, b| a.path.eq_ignore_ascii_case(&b.path));
    let mut taken: HashSet<String> = HashSet::new();
    let mut kept = Vec::new();
    for candidate in candidates {
        let lower = candidate.path.to_lowercase();
        let inside = Path::new(&lower).ancestors().skip(1).any(|a| taken.contains(a.to_string_lossy().as_ref()));
        // A link (a worktree's node_modules, a junction to the main checkout's) frees nothing, and
        // what it points at is found on its own.
        if inside || std::fs::symlink_metadata(&candidate.path).is_ok_and(|m| m.file_type().is_symlink()) {
            continue;
        }
        if let Some(rule) = rule_for(Path::new(&candidate.path)) {
            taken.insert(lower);
            kept.push((candidate, rule.what));
        }
    }
    kept
}

/// How many entries a project is looked through for its date before giving up on it.
const WALK_LIMIT: usize = 50_000;

/// When the project folder `project` last changed: the newest date of what's in it, leaving out its
/// junk (`skip`, lowercase paths) and `.git` (a fetch isn't work on it), and not following links. A
/// change in the last hour ends the look. 0 when it holds more than [`WALK_LIMIT`] entries or can't
/// be read, so a big project is never taken for one left alone.
fn last_changed(project: &Path, skip: &HashSet<String>, stop: &AtomicBool) -> i64 {
    let Ok(top) = std::fs::metadata(project) else { return 0 };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let mut newest = ms_of(&top);
    let mut pending = vec![project.to_path_buf()];
    let mut seen = 0;
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > WALK_LIMIT || stop.load(Ordering::Relaxed) {
                return 0;
            }
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                let path = entry.path();
                if entry.file_name().eq_ignore_ascii_case(".git") || skip.contains(&path.to_string_lossy().to_lowercase()) {
                    continue;
                }
                pending.push(path);
            }
            if let Ok(meta) = entry.metadata() {
                newest = newest.max(ms_of(&meta));
            }
            if newest > now - 3_600_000 {
                return newest;
            }
        }
    }
    newest
}

/// [`last_changed`] for each of `projects`, a few at a time.
fn projects_changed(projects: &[PathBuf], skip: &HashSet<String>, stop: &AtomicBool) -> Vec<i64> {
    let next = AtomicUsize::new(0);
    let dates: Vec<AtomicI64> = projects.iter().map(|_| AtomicI64::new(0)).collect();
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8).min(projects.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let at = next.fetch_add(1, Ordering::Relaxed);
                let Some(project) = projects.get(at) else { break };
                dates[at].store(last_changed(project, skip, stop), Ordering::Relaxed);
            });
        }
    });
    dates.into_iter().map(AtomicI64::into_inner).collect()
}

/// Developer junk below `root`, biggest first.
fn find_junk(root: &str, run: &Run, use_index: bool) -> Result<Report, String> {
    let started = Instant::now();
    let mut engines = Engines::default();
    let names = names();
    let mut candidates = Vec::new();
    let queries: Vec<String> = names.iter().map(|name| format!("folder: \"{name}*\"")).collect();
    // A whole-name glob finds "build*" too: the exact name is kept.
    let named = |hit: &shadui_index::proto::Hit| hit.dir && hit.path.rsplit('\\').next().is_some_and(|n| names.iter().any(|name| n.eq_ignore_ascii_case(name)));
    let indexed = if use_index { index_search(root, &queries, true, &[], &run.stop, |hit| if named(&hit) { candidates.push(Candidate { path: hit.path, size: Some(hit.size) }) }) } else { None };
    run.stopped()?;
    engines.index = indexed.is_some();
    let mut roots = indexed.unwrap_or_else(|| vec![root.to_string()]);
    engines.fd = !roots.is_empty();
    let pattern = format!("{{{}}}", names.join(","));
    while !roots.is_empty() {
        // Not going into what it finds: what's inside junk is junk too.
        let found = fd_paths(&["--hidden", "--ignore-case", "--type=directory", "--prune", "--glob"], Some(&pattern), &roots, &[], run)?;
        // A folder that only has junk's name (a "build" of one's own) is looked into next.
        roots = found.iter().filter(|path| rule_for(Path::new(path)).is_none()).cloned().collect();
        candidates.extend(found.into_iter().map(|path| Candidate { path, size: None }));
    }
    let kept = keep_junk(candidates);
    let to_measure: Vec<String> = kept.iter().filter(|(c, _)| c.size.is_none()).map(|(c, _)| c.path.clone()).collect();
    engines.dust = !to_measure.is_empty();
    let measured = if to_measure.is_empty() { HashMap::new() } else { sizes(&to_measure, run)? };
    // Each project once (its node_modules and .next share one), dated by what's in it.
    let skip: HashSet<String> = kept.iter().map(|(c, _)| c.path.to_lowercase()).collect();
    let project_of = |path: &str| Path::new(path).parent().map(Path::to_path_buf);
    let mut projects: Vec<PathBuf> = kept.iter().filter_map(|(c, _)| project_of(&c.path)).collect();
    projects.sort_by_cached_key(|p| p.to_string_lossy().to_lowercase());
    projects.dedup_by(|a, b| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()));
    let dates = projects_changed(&projects, &skip, &run.stop);
    run.stopped()?;
    let changed: HashMap<String, i64> = projects.iter().map(|p| p.to_string_lossy().to_lowercase()).zip(dates).collect();
    let items = kept
        .into_iter()
        .map(|(c, kind)| {
            let size = c.size.or_else(|| measured.get(&c.path.to_lowercase()).copied()).unwrap_or(0);
            let date = project_of(&c.path).and_then(|p| changed.get(&p.to_string_lossy().to_lowercase()).copied()).unwrap_or(0);
            Found { path: c.path, size, date, kind }
        })
        .collect();
    Ok(report(items, false, engines, 0, started))
}

/// Whether the folder `dir` holds nothing but folders that hold nothing: no files, and no links (a
/// junction in it isn't an empty folder's to take along). Gives up past 10,000 folders.
fn holds_nothing(dir: &Path) -> bool {
    // A link's own metadata isn't a folder's.
    if !std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
        return false;
    }
    let mut pending = vec![dir.to_path_buf()];
    let mut seen = 0;
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { return false };
        for entry in entries {
            let Ok(entry) = entry else { return false };
            seen += 1;
            if seen > 10_000 || !entry.file_type().is_ok_and(|k| k.is_dir()) {
                return false;
            }
            pending.push(entry.path());
        }
    }
    true
}

/// A folder found, with its date (0 when what found it doesn't know it).
type Dated = (String, i64);

/// Empty folders from the index: the folders below `root` that add up to 0 bytes, less those with a
/// 0-byte file somewhere below them. What it can't answer for, for fd.
fn empty_from_index(root: &str, run: &Run) -> Option<(Vec<Dated>, Vec<String>)> {
    let mut zero = Vec::new();
    // Hidden folders (AppData, .git) aren't looked into, as fd doesn't.
    let skipped = index_search(root, &["folder:".into()], false, &[], &run.stop, |hit| {
        if hit.dir && hit.size == 0 {
            zero.push((hit.path, hit.modified));
        }
    })?;
    let mut holding = HashSet::new();
    index_search(root, &["file: size:0".into()], true, &[], &run.stop, |hit| {
        // Every folder it's in holds something; once one is marked, so are those above it.
        for dir in Path::new(&hit.path.to_lowercase()).ancestors().skip(1) {
            if !holding.insert(dir.to_string_lossy().into_owned()) {
                break;
            }
        }
    })?;
    zero.retain(|(path, _)| !holding.contains(&path.to_lowercase()));
    Some((zero, skipped))
}

/// Empty folders from fd: the ones with nothing in them at all, then up from each while a folder
/// holds only empty ones. Hidden folders aren't looked into, nor counted as empty.
fn empty_from_fd(roots: &[String], run: &Run) -> Result<Vec<Dated>, String> {
    let leaves = fd_paths(&["--type=directory", "--type=empty"], None, roots, &[], run)?;
    let tops: Vec<String> = roots.iter().map(|r| folder_key(r)).collect();
    let mut empty: HashSet<String> = leaves.iter().map(|p| p.to_lowercase()).collect();
    let mut found = leaves.clone();
    let mut level = leaves;
    while !level.is_empty() {
        let mut parents: Vec<String> = level.iter().filter_map(|p| Path::new(p).parent().map(|p| p.to_string_lossy().into_owned())).filter(|p| tops.iter().any(|top| below(top, &p.to_lowercase()))).collect();
        parents.sort_by_cached_key(|p| p.to_lowercase());
        parents.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        level = Vec::new();
        for parent in parents {
            run.stopped()?;
            let lower = parent.to_lowercase();
            if empty.contains(&lower) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&parent) else { continue };
            let only_empty = entries.into_iter().all(|e| e.is_ok_and(|e| e.file_type().is_ok_and(|k| k.is_dir() && !k.is_symlink()) && empty.contains(&e.path().to_string_lossy().to_lowercase())));
            if only_empty {
                empty.insert(lower);
                level.push(parent);
            }
        }
        found.extend(level.iter().cloned());
    }
    Ok(found.into_iter().map(|path| (path, 0)).collect())
}

/// Empty folders below `root` (not `root` itself), outermost only, each checked on disk.
fn find_empty(root: &str, run: &Run, use_index: bool) -> Result<Report, String> {
    let started = Instant::now();
    let mut engines = Engines::default();
    let mut found = Vec::new();
    let indexed = if use_index { empty_from_index(root, run) } else { None };
    run.stopped()?;
    engines.index = indexed.is_some();
    let rest = match indexed {
        Some((from_index, rest)) => {
            found = from_index;
            rest
        }
        None => vec![root.to_string()],
    };
    if !rest.is_empty() {
        engines.fd = true;
        found.extend(empty_from_fd(&rest, run)?);
    }
    let top = folder_key(root);
    found.retain(|(path, _)| !off_limits(path) && folder_key(path) != top);
    found.sort_by_cached_key(|(path, _)| path.to_lowercase());
    found.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    // Outermost first (sorted, a folder comes before what's in it): what's in a folder taken goes
    // with it. One the index called empty but that isn't (it changed, or holds a junction) is passed
    // over, and what's in it is looked at on its own.
    let mut taken: HashSet<String> = HashSet::new();
    let mut items = Vec::new();
    for (path, modified) in found {
        let lower = path.to_lowercase();
        if Path::new(&lower).ancestors().skip(1).any(|a| taken.contains(a.to_string_lossy().as_ref())) || !holds_nothing(Path::new(&path)) {
            continue;
        }
        run.stopped()?;
        let date = if modified > 0 { modified } else { std::fs::metadata(&path).map_or(0, |m| ms_of(&m)) };
        taken.insert(lower);
        items.push(Found { path, size: 0, date, kind: "" });
    }
    Ok(report(items, true, engines, 0, started))
}

/// Whether a file can't go now: a program has it open without letting it be deleted (as Office has
/// its ~$ owner files), or this account may not delete it. Asked as deleting asks, for DELETE
/// access, so nothing is read (an online-only OneDrive file isn't downloaded).
#[cfg(windows)]
fn held(path: &str) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    const DELETE: u32 = 0x0001_0000;
    const SHARE_ALL: u32 = 0x7;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    std::fs::OpenOptions::new().access_mode(DELETE).share_mode(SHARE_ALL).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path).is_err()
}

#[cfg(not(windows))]
fn held(_: &str) -> bool {
    false
}

/// What programs left behind below `root`, biggest first, less what a program still has open.
fn find_leftovers(root: &str, run: &Run, use_index: bool) -> Result<Report, String> {
    let started = Instant::now();
    let mut engines = Engines::default();
    let mut found: HashMap<String, Found> = HashMap::new();
    let mut keep = |path: String, size: u64, date: i64| {
        let name = path.rsplit('\\').next().unwrap_or_default();
        if let Some(kind) = leftover(name).filter(|_| !off_limits(&path)) {
            found.insert(path.to_lowercase(), Found { path, size, date, kind });
        }
    };
    let queries: Vec<String> = LEFTOVERS.iter().map(|(name, _)| format!("file: \"{}\"", name.glob())).collect();
    let indexed = if use_index { index_search(root, &queries, true, &[], &run.stop, |hit| if !hit.dir { keep(hit.path, hit.size, hit.modified) }) } else { None };
    run.stopped()?;
    engines.index = indexed.is_some();
    let rest = indexed.unwrap_or_else(|| vec![root.to_string()]);
    if !rest.is_empty() {
        engines.fd = true;
        let pattern = format!("{{{}}}", LEFTOVERS.iter().map(|(name, _)| name.glob()).collect::<Vec<_>>().join(","));
        for path in fd_paths(&["--hidden", "--ignore-case", "--type=file", "--glob"], Some(&pattern), &rest, &[], run)? {
            if let Some((size, date, _)) = stat_file(&path) {
                keep(path, size, date);
            }
        }
    }
    let mut items = Vec::with_capacity(found.len());
    let mut kept_back = 0;
    for item in found.into_values() {
        run.stopped()?;
        if held(&item.path) {
            kept_back += 1;
        } else {
            items.push(item);
        }
    }
    Ok(report(items, false, engines, kept_back, started))
}

/// Folders big files aren't looked for in, besides hidden ones: a profile's AppData holds programs'
/// own (WSL's and Docker's disks are whole systems), even where someone has unhidden it.
const NOT_FOR_BIG: &[&str] = &["AppData"];

/// Files of `at_least` bytes or more below `root`, biggest first, less Windows' own (the page file,
/// the hibernation file: system files). Hidden folders and [`NOT_FOR_BIG`] aren't looked into:
/// what's big there is a program's, not a file to recycle.
fn find_big(root: &str, at_least: u64, run: &Run, use_index: bool) -> Result<Report, String> {
    let started = Instant::now();
    let mut engines = Engines::default();
    let mut items = Vec::new();
    // Below `root` only: looking in a folder that's in AppData itself was asked for.
    let from = root.trim_end_matches('\\').len();
    let mut keep = |path: String, size: u64, date: i64, attrs: u32| {
        let programs = path.get(from..).unwrap_or_default().split('\\').any(|part| NOT_FOR_BIG.iter().any(|n| part.eq_ignore_ascii_case(n)));
        if size >= at_least && attrs & SYSTEM == 0 && !programs && !off_limits(&path) {
            items.push(Found { path, size, date, kind: "" });
        }
    };
    let query = format!("file: size:>={at_least}");
    let indexed = if use_index { index_search(root, &[query], false, NOT_FOR_BIG, &run.stop, |hit| if !hit.dir { keep(hit.path, hit.size, hit.modified, hit.attrs) }) } else { None };
    run.stopped()?;
    engines.index = indexed.is_some();
    let rest = indexed.unwrap_or_else(|| vec![root.to_string()]);
    if !rest.is_empty() {
        engines.fd = true;
        for path in fd_paths(&["--type=file", &format!("--size=+{at_least}b")], None, &rest, NOT_FOR_BIG, run)? {
            if let Some((size, date, attrs)) = stat_file(&path) {
                keep(path, size, date, attrs);
            }
        }
    }
    Ok(report(items, false, engines, 0, started))
}

fn scan(root: &str, mode: Mode, run: &Run, use_index: bool) -> Result<Report, String> {
    match mode {
        Mode::Junk => find_junk(root, run, use_index),
        Mode::Empty => find_empty(root, run, use_index),
        Mode::Temporary => find_leftovers(root, run, use_index),
        Mode::Big { at_least } => find_big(root, at_least.max(1), run, use_index),
    }
}

/// What Clean up's `mode` finds below `root`, for the tab `lane` (a newer scan there, or Stop, ends
/// it). Nothing is removed.
#[tauri::command]
pub async fn cleanup_scan(root: String, mode: Mode, lane: String) -> Result<Report, String> {
    let run = start(&lane);
    blocking(move || {
        let report = scan(&root, mode, &run, true);
        let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
        if runs.get(&lane).is_some_and(|r| Arc::ptr_eq(&r.stop, &run.stop)) {
            runs.remove(&lane);
        }
        report
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A fresh folder for a test, removed with what's in it when it's dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("glint-cleanup-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn mk(&self, path: &str) {
            fs::create_dir_all(self.0.join(path)).unwrap();
        }

        fn put(&self, path: &str, bytes: usize) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, vec![b'x'; bytes]).unwrap();
        }

        fn root(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }

        /// What `report` listed, from this folder, with forward slashes.
        fn listed(&self, report: &Report) -> Vec<String> {
            let root = self.root();
            report.items.iter().map(|f| f.path.strip_prefix(&root).unwrap().trim_start_matches('\\').replace('\\', "/")).collect()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// fd comes with the app; a checkout that hasn't been built (or a CI runner without it) skips.
    fn have_fd() -> bool {
        let found = crate::proc::locate("fd").is_some();
        if !found {
            eprintln!("fd isn't here: skipped");
        }
        found
    }

    #[test]
    fn tells_junk_from_folders_that_only_share_its_name() {
        let dir = Scratch::new("junk");
        // A Node project, with a package inside its node_modules that has its own.
        dir.mk("web/node_modules/left-pad/node_modules");
        dir.put("web/package.json", 1);
        dir.put("web/node_modules/left-pad/package.json", 1);
        // A Rust project; a "target" folder that's just a folder; a .NET project's bin and obj.
        dir.mk("rust/target/debug");
        dir.put("rust/Cargo.toml", 1);
        dir.mk("photos/target");
        dir.mk("app/bin");
        dir.mk("app/obj");
        dir.put("app/App.csproj", 1);
        // A venv (known by what's in it), a cache by name, a CMake build, a Unity project.
        dir.put("py/.venv/pyvenv.cfg", 1);
        dir.mk("py/src/__pycache__");
        dir.put("native/build/CMakeCache.txt", 1);
        dir.mk("native/out");
        dir.mk("game/Library");
        dir.mk("game/Assets");
        dir.mk("game/ProjectSettings");
        // A worktree whose node_modules is a junction to the main checkout's.
        dir.put("worktree/package.json", 1);
        let linked = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(dir.0.join("worktree\\node_modules")).arg(dir.0.join("web\\node_modules")).output().unwrap();
        assert!(linked.status.success(), "mklink /J: {}", String::from_utf8_lossy(&linked.stderr));
        let found = |path: &str| Candidate { path: dir.0.join(path).to_string_lossy().into_owned(), size: None };
        let candidates = ["web/node_modules", "web/node_modules/left-pad/node_modules", "rust/target", "photos/target", "app/bin", "app/obj", "py/.venv", "py/src/__pycache__", "native/build", "native/out", "game/Library", "game/Assets", "worktree/node_modules"].map(found);
        let kept: Vec<(String, &str)> = keep_junk(candidates.into()).into_iter().map(|(c, what)| (c.path.strip_prefix(&*dir.root()).unwrap().trim_start_matches('\\').replace('\\', "/"), what)).collect();
        assert_eq!(
            kept,
            [
                ("app/bin".to_string(), ".NET builds"),
                ("app/obj".to_string(), ".NET builds"),
                ("game/Library".to_string(), "Unity caches"),
                ("native/build".to_string(), "CMake builds"),
                ("py/.venv".to_string(), "Python virtual environments"),
                ("py/src/__pycache__".to_string(), "Python and Jupyter caches"),
                ("rust/target".to_string(), "Rust builds"),
                ("web/node_modules".to_string(), "Node.js packages and builds"),
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn dates_a_project_by_its_own_files() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::time::Duration;
        let dir = Scratch::new("dates");
        dir.mk("target/debug");
        dir.mk(".git");
        for p in ["Cargo.toml", "src/main.rs", "target/debug/app.exe", ".git/FETCH_HEAD"] {
            dir.put(p, 1);
        }
        let days_ago = |days: u64| SystemTime::now() - Duration::from_secs(days * 86_400);
        // Folders too (FILE_WRITE_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS): theirs change as what's in them comes and goes.
        let set = |p: &str, when: SystemTime| fs::OpenOptions::new().access_mode(0x100).custom_flags(0x0200_0000).open(dir.0.join(p)).unwrap().set_modified(when).unwrap();
        set("Cargo.toml", days_ago(90));
        set("src/main.rs", days_ago(60));
        set("src", days_ago(70));
        set("", days_ago(80));
        let skip = HashSet::from([dir.0.join("target").to_string_lossy().to_lowercase()]);
        let ms = |t: SystemTime| t.duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        let stop = AtomicBool::new(false);
        // Its build output and .git are new; nothing of its own is.
        assert!((last_changed(&dir.0, &skip, &stop) - ms(days_ago(60))).abs() < 2_000);
        dir.put("src/lib.rs", 1);
        assert!(last_changed(&dir.0, &skip, &stop) > ms(SystemTime::now()) - 60_000);
    }

    #[test]
    fn looks_for_each_name_once() {
        let all = names();
        assert!(all.contains(&"node_modules") && all.contains(&"target") && all.contains(&"build"));
        assert_eq!(all.iter().filter(|n| **n == "target").count(), 1);
    }

    #[test]
    fn knows_what_programs_leave_behind() {
        assert_eq!(leftover("~$Report.docx"), Some("Office owner files"));
        assert_eq!(leftover("Thumbs.db"), Some("Thumbnail caches"));
        assert_eq!(leftover("ehthumbs_vista.db"), Some("Thumbnail caches"));
        assert_eq!(leftover("setup.TMP"), Some("Temporary files"));
        assert_eq!(leftover(".~lock.budget.ods#"), Some("LibreOffice lock files"));
        assert_eq!(leftover(".DS_Store"), Some("macOS folder settings"));
        assert_eq!(leftover("._IMG_0001.JPG"), Some("macOS resource forks"));
        // Only the name: not a file called just that, nor one that only contains it.
        for name in [".tmp", "tmp", "~$", "._", ".~lock.#", "notes.txt", "thumbs.db.bak", "my.tmp.txt"] {
            assert_eq!(leftover(name), None, "{name}");
        }
        assert_eq!(LEFTOVERS.iter().map(|(n, _)| n.glob()).take(4).collect::<Vec<_>>(), ["*.tmp", "*.temp", "~$*", ".~lock.*#"]);
    }

    #[test]
    fn keeps_out_of_windows_and_the_recycle_bin() {
        let windows = WINDOWS.clone();
        assert!(off_limits(&windows));
        assert!(off_limits(&format!(r"{windows}\Temp\x.tmp")));
        assert!(!off_limits(&format!("{windows}.old")));
        assert!(off_limits(r"D:\$RECYCLE.BIN\S-1-5-21\$R1.tmp"));
        assert!(off_limits(r"E:\System Volume Information\x"));
        assert!(!off_limits(r"C:\Users\me\Recycle Bin notes\a.tmp"));
        assert!(below(r"c:\", r"c:\repos") && below(r"c:\repos", r"c:\repos\a") && !below(r"c:\repos", r"c:\repos2\a") && !below(r"c:\repos", r"c:\repos"));
    }

    #[cfg(windows)]
    #[test]
    fn leaves_out_files_a_program_has_open() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = Scratch::new("held");
        dir.put("~$letter.docx", 162);
        let path = dir.0.join("~$letter.docx").to_string_lossy().into_owned();
        assert!(!held(&path));
        // As Word holds it: open, sharing reads and writes but not deleting.
        let open = fs::OpenOptions::new().read(true).share_mode(0x1 | 0x2).open(&path).unwrap();
        assert!(held(&path));
        drop(open);
        assert!(!held(&path));
    }

    #[test]
    fn finds_empty_folders_and_files_left_behind_with_fd() {
        if !have_fd() {
            return;
        }
        let dir = Scratch::new("fd");
        // a is empty; b holds only empty folders, so it's empty too (and listed alone).
        dir.mk("a");
        dir.mk("b/c");
        dir.mk("b/d/e");
        // f holds a file; its g is empty. h holds a hidden folder, not looked into, so it isn't.
        dir.put("f/notes.txt", 10);
        dir.mk("f/g");
        dir.mk("h/.cache");
        let report = find_empty(&dir.root(), &Run::default(), false).unwrap();
        assert_eq!(dir.listed(&report), ["a", "b", "f/g"]);
        assert_eq!(report.engine, "fd");

        dir.put("f/~$letter.docx", 162);
        dir.put("f/g2/Thumbs.db", 4096);
        dir.put("f/setup.tmp", 1000);
        dir.put("f/keep.tmp.txt", 1000);
        let report = find_leftovers(&dir.root(), &Run::default(), false).unwrap();
        assert_eq!(dir.listed(&report), ["f/g2/Thumbs.db", "f/setup.tmp", "f/~$letter.docx"]);
        assert_eq!(report.items.iter().map(|f| f.kind).collect::<Vec<_>>(), ["Thumbnail caches", "Temporary files", "Office owner files"]);
        assert_eq!(report.total, 4096 + 1000 + 162);

        dir.put("big.bin", 3 << 20);
        dir.put("f/bigger.iso", 5 << 20);
        // A program's, even where AppData isn't hidden: WSL's disk is a whole system.
        dir.put("AppData/Local/wsl/ext4.vhdx", 4 << 20);
        let report = find_big(&dir.root(), 2 << 20, &Run::default(), false).unwrap();
        assert_eq!(dir.listed(&report), ["f/bigger.iso", "big.bin"]);
        assert!(report.items.iter().all(|f| f.date > 0));
    }

    /// `GLINT_CLEAN_ROOT=C:\Repos cargo test --release -- --ignored --nocapture cleans_up_a_real_folder`:
    /// what a real folder holds, and how long finding it takes (nothing is removed).
    /// `GLINT_CLEAN_MODE` is junk (the default), empty, temp or big (1 GB and up); `GLINT_CLEAN_FD=1`
    /// leaves the index out; `GLINT_CLEAN_ALL=1` lists everything found.
    #[test]
    #[ignore]
    fn cleans_up_a_real_folder() {
        let Ok(root) = std::env::var("GLINT_CLEAN_ROOT") else { return };
        let mode = match std::env::var("GLINT_CLEAN_MODE").as_deref() {
            Ok("empty") => Mode::Empty,
            Ok("temp") => Mode::Temporary,
            Ok("big") => Mode::Big { at_least: 1 << 30 },
            _ => Mode::Junk,
        };
        let report = scan(&root, mode, &Run::default(), std::env::var("GLINT_CLEAN_FD").is_err()).unwrap();
        println!("{} items (+{} more, {} held), {:.1} GB, by {} in {:.0} ms", report.items.len(), report.more, report.held, report.total as f64 / (1u64 << 30) as f64, report.engine, report.elapsed);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        for item in report.items.iter().take(if std::env::var("GLINT_CLEAN_ALL").is_ok() { usize::MAX } else { 15 }) {
            let days = if item.date > 0 { format!("{} days", (now - item.date) / 86_400_000) } else { "?".into() };
            println!("{:>10.1} MB  {}  ({}, {days} ago)", item.size as f64 / (1 << 20) as f64, item.path, item.kind);
        }
        println!("{} of {} with no date", report.items.iter().filter(|i| i.date == 0).count(), report.items.len());
    }
}
