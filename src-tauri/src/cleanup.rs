//! Clean up: what can go to free space below a folder. Developer junk first: build output and
//! package caches that a project's own tools make again (`node_modules` next to a package.json,
//! `target` next to a Cargo.toml…). Folders are found by name (from the instant search index, or
//! with fd), told apart from ones that only share the name by the project file beside them (kondo's
//! rule), and sized (by the index, or with dust). Nothing is removed here: the UI recycles what's
//! chosen, which Ctrl+Z takes back.

use crate::fsx::{blocking, ms_since};
use crate::proc::Slot;
use serde::Serialize;
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

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Junk {
    pub path: String,
    /// Everything in it, in bytes.
    pub size: u64,
    /// When its project last changed (see [`last_changed`]); 0 when that couldn't be told.
    pub project_changed: i64,
    /// What it is: "Node.js packages and builds"…
    pub kind: &'static str,
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct JunkReport {
    pub items: Vec<Junk>,
    pub total: u64,
    /// Who found and sized it: "index", "fd + dust", or "index + fd + dust".
    pub engine: String,
    pub elapsed: f64,
}

/// A folder found by name, with its size when what found it knows it.
struct Candidate {
    path: String,
    size: Option<u64>,
}

/// Cleanups in flight, one per tab, so another scan there or Stop ends one.
#[derive(Clone, Default)]
struct Run {
    stop: Arc<AtomicBool>,
    child: Slot,
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

/// Candidates from the index: a search per name (a whole-name glob, then the exact name kept, as
/// the index matches "build*" too). The folders it can't answer for, for fd. None when it isn't
/// running or answers for none of them.
#[cfg(windows)]
fn from_index(root: &str, names: &[&str], stop: &AtomicBool) -> Option<(Vec<Candidate>, Vec<String>)> {
    use shadui_index::client::{self, Event};
    let mut found = Vec::new();
    let mut skipped = None;
    for name in names {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        let request = shadui_index::proto::SearchRequest { query: format!("folder: \"{name}*\""), roots: vec![root.to_string()], hidden: true, exclude: Vec::new(), max_results: None };
        let mut answered = false;
        let ran = client::search(&request, |event| {
            match event {
                Event::Plan { indexed, skipped: rest } => {
                    answered = !indexed.is_empty();
                    skipped.get_or_insert(rest);
                    return answered;
                }
                Event::Hits(hits) => found.extend(hits.into_iter().filter(|h| h.dir && h.path.rsplit('\\').next().is_some_and(|n| n.eq_ignore_ascii_case(name))).map(|h| Candidate { path: h.path, size: Some(h.size) })),
                Event::Done { .. } => {}
            }
            !stop.load(Ordering::Relaxed)
        });
        if ran.is_err() || !answered {
            return None;
        }
    }
    Some((found, skipped.unwrap_or_default()))
}

#[cfg(not(windows))]
fn from_index(_: &str, _: &[&str], _: &AtomicBool) -> Option<(Vec<Candidate>, Vec<String>)> {
    None
}

/// Candidates from fd: one run for every name, not going into what it finds (what's inside junk
/// is junk too).
fn from_fd(roots: &[String], names: &[&str], run: &Run) -> Result<Vec<Candidate>, String> {
    let mut command = crate::proc::tool("fd")?;
    command
        .args(["--color=never", "--absolute-path", "--ignore-case", "--hidden", "--no-ignore", "--type=directory", "--prune", "--glob", "--"])
        .arg(format!("{{{}}}", names.join(",")))
        .args(roots)
        .env_remove("MSYSTEM")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|e| format!("Failed to start fd: {e}"))?;
    let stdout = child.stdout.take().ok_or("Failed to open fd's output")?;
    let pid = run.child.replace(child);
    let mut found = Vec::new();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let path = line.trim_end_matches(['\\', '/']);
        if !path.is_empty() {
            found.push(Candidate { path: path.to_string(), size: None });
        }
    }
    run.child.finish(pid);
    if run.stop.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
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
        if run.stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
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

fn ms_of(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
}

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
fn find_junk(root: &str, run: &Run, use_index: bool) -> Result<JunkReport, String> {
    let started = Instant::now();
    let names = names();
    // An older service would read "folder:" as part of a name and find nothing: then fd looks.
    let indexed = if use_index && crate::find::same_service() { from_index(root, &names, &run.stop) } else { None };
    let (mut candidates, rest, by_index) = match indexed {
        Some((found, skipped)) => (found, skipped, true),
        None => (Vec::new(), vec![root.to_string()], false),
    };
    if run.stop.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let by_fd = !rest.is_empty();
    let mut roots = rest;
    while !roots.is_empty() {
        let found = from_fd(&roots, &names, run)?;
        // fd doesn't go into what it finds, so a folder that only has junk's name (a "build" of
        // one's own) is looked into next.
        roots = found.iter().filter(|c| rule_for(Path::new(&c.path)).is_none()).map(|c| c.path.clone()).collect();
        candidates.extend(found);
    }
    let kept = keep_junk(candidates);
    let to_measure: Vec<String> = kept.iter().filter(|(c, _)| c.size.is_none()).map(|(c, _)| c.path.clone()).collect();
    let measured = if to_measure.is_empty() { HashMap::new() } else { sizes(&to_measure, run)? };
    // Each project once (its node_modules and .next share one), dated by what's in it.
    let skip: HashSet<String> = kept.iter().map(|(c, _)| c.path.to_lowercase()).collect();
    let project_of = |path: &str| Path::new(path).parent().map(Path::to_path_buf);
    let mut projects: Vec<PathBuf> = kept.iter().filter_map(|(c, _)| project_of(&c.path)).collect();
    projects.sort_by_cached_key(|p| p.to_string_lossy().to_lowercase());
    projects.dedup_by(|a, b| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()));
    let dates = projects_changed(&projects, &skip, &run.stop);
    if run.stop.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let changed: HashMap<String, i64> = projects.iter().map(|p| p.to_string_lossy().to_lowercase()).zip(dates).collect();
    let mut items: Vec<Junk> = kept
        .into_iter()
        .map(|(c, kind)| {
            let size = c.size.or_else(|| measured.get(&c.path.to_lowercase()).copied()).unwrap_or(0);
            let project_changed = project_of(&c.path).and_then(|p| changed.get(&p.to_string_lossy().to_lowercase()).copied()).unwrap_or(0);
            Junk { path: c.path, size, project_changed, kind }
        })
        .collect();
    items.sort_by_key(|j| std::cmp::Reverse(j.size));
    let total = items.iter().map(|j| j.size).sum();
    let engine = match (by_index, by_fd) {
        (true, false) => "index",
        (true, true) => "index + fd + dust",
        _ => "fd + dust",
    };
    Ok(JunkReport { items, total, engine: engine.into(), elapsed: ms_since(started) })
}

/// Finds developer junk below `root` for the tab `lane` (a newer scan there, or Stop, ends it).
#[tauri::command]
pub async fn cleanup_junk(root: String, lane: String) -> Result<JunkReport, String> {
    let run = start(&lane);
    blocking(move || {
        let report = find_junk(&root, &run, true);
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

    #[test]
    fn tells_junk_from_folders_that_only_share_its_name() {
        let dir = std::env::temp_dir().join(format!("glint-cleanup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mk = |p: &str| fs::create_dir_all(dir.join(p)).unwrap();
        let touch = |p: &str| fs::write(dir.join(p), b"x").unwrap();
        // A Node project, with a package inside its node_modules that has its own.
        mk("web/node_modules/left-pad/node_modules");
        touch("web/package.json");
        touch("web/node_modules/left-pad/package.json");
        // A Rust project; a "target" folder that's just a folder; a .NET project's bin and obj.
        mk("rust/target/debug");
        touch("rust/Cargo.toml");
        mk("photos/target");
        mk("app/bin");
        mk("app/obj");
        touch("app/App.csproj");
        // A venv (known by what's in it), a cache by name, a CMake build, a Unity project.
        mk("py/.venv/Scripts");
        touch("py/.venv/pyvenv.cfg");
        mk("py/src/__pycache__");
        mk("native/build/CMakeFiles");
        touch("native/build/CMakeCache.txt");
        mk("native/out");
        mk("game/Library");
        mk("game/Assets");
        mk("game/ProjectSettings");
        // A worktree whose node_modules is a junction to the main checkout's.
        mk("worktree");
        touch("worktree/package.json");
        let linked = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(dir.join("worktree\\node_modules")).arg(dir.join("web\\node_modules")).output().unwrap();
        assert!(linked.status.success(), "mklink /J: {}", String::from_utf8_lossy(&linked.stderr));
        let found = |path: &str| Candidate { path: dir.join(path).to_string_lossy().into_owned(), size: None };
        let candidates = ["web/node_modules", "web/node_modules/left-pad/node_modules", "rust/target", "photos/target", "app/bin", "app/obj", "py/.venv", "py/src/__pycache__", "native/build", "native/out", "game/Library", "game/Assets", "worktree/node_modules"].map(found);
        let kept: Vec<(String, &str)> = keep_junk(candidates.into()).into_iter().map(|(c, what)| (c.path.strip_prefix(&*dir.to_string_lossy()).unwrap().trim_start_matches('\\').replace('\\', "/"), what)).collect();
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
        let _ = fs::remove_dir_all(&dir);
    }

    /// `GLINT_JUNK_ROOT=C:\Repos cargo test --release -- --ignored --nocapture finds_junk_below`: what
    /// a real folder holds, and how long finding it takes (nothing is removed). `GLINT_JUNK_FD=1`
    /// leaves the index out; `GLINT_JUNK_ALL=1` lists every folder found.
    #[test]
    #[ignore]
    fn finds_junk_below() {
        let Ok(root) = std::env::var("GLINT_JUNK_ROOT") else { return };
        let report = find_junk(&root, &Run::default(), std::env::var("GLINT_JUNK_FD").is_err()).unwrap();
        println!("{} items, {:.1} GB, by {} in {:.0} ms", report.items.len(), report.total as f64 / (1u64 << 30) as f64, report.engine, report.elapsed);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        for item in report.items.iter().take(if std::env::var("GLINT_JUNK_ALL").is_ok() { usize::MAX } else { 15 }) {
            let days = if item.project_changed > 0 { format!("{} days", (now - item.project_changed) / 86_400_000) } else { "?".into() };
            println!("{:>10.1} MB  {}  ({}, project changed {days} ago)", item.size as f64 / (1 << 20) as f64, item.path, item.kind);
        }
        println!("{} of {} with no date", report.items.iter().filter(|i| i.project_changed == 0).count(), report.items.len());
    }

    #[cfg(windows)]
    #[test]
    fn dates_a_project_by_its_own_files() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!("glint-cleanup-dates-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for p in ["src", "target/debug", ".git"] {
            fs::create_dir_all(dir.join(p)).unwrap();
        }
        for p in ["Cargo.toml", "src/main.rs", "target/debug/app.exe", ".git/FETCH_HEAD"] {
            fs::write(dir.join(p), b"x").unwrap();
        }
        let days_ago = |days: u64| SystemTime::now() - Duration::from_secs(days * 86_400);
        // Folders too (FILE_WRITE_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS): theirs change as what's in them comes and goes.
        let set = |p: &str, when: SystemTime| fs::OpenOptions::new().access_mode(0x100).custom_flags(0x0200_0000).open(dir.join(p)).unwrap().set_modified(when).unwrap();
        set("Cargo.toml", days_ago(90));
        set("src/main.rs", days_ago(60));
        set("src", days_ago(70));
        set("", days_ago(80));
        let skip = HashSet::from([dir.join("target").to_string_lossy().to_lowercase()]);
        let ms = |t: SystemTime| t.duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        let stop = AtomicBool::new(false);
        // Its build output and .git are new; nothing of its own is.
        assert!((last_changed(&dir, &skip, &stop) - ms(days_ago(60))).abs() < 2_000);
        fs::write(dir.join("src/lib.rs"), b"x").unwrap();
        assert!(last_changed(&dir, &skip, &stop) > ms(SystemTime::now()) - 60_000);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn looks_for_each_name_once() {
        let all = names();
        assert!(all.contains(&"node_modules") && all.contains(&"target") && all.contains(&"build"));
        assert_eq!(all.iter().filter(|n| **n == "target").count(), 1);
    }
}
