//! What the file actions share (Media studio, PDF tools, Convert data, Remove personal info, Encrypt):
//! each is a list of jobs, a job being some files in (one video, or the PDFs to merge), made by the
//! action's own code (a tool's process, or Rust) into a hidden work folder beside its first file, a few
//! jobs at a time. Then what each job made is placed: next to its files with a suffix after the name,
//! into a folder there, or in their place, the files it replaces recycled together first. The report says
//! what was made and where the originals went in the Recycle Bin, so the UI takes it all back as one step,
//! and what failed and why.
//!
//! Tools run through `run_tool`: no console window, inside a Windows job (so they end with Glint, however
//! it ends), stopped at once when the action is, their output read a line at a time for progress.

use crate::fsx;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Where what's made goes.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Output {
    /// Next to the job's first file, the name made with `suffix` after it.
    Beside { suffix: String },
    /// Into a folder of this name next to the job's first file.
    Folder { name: String },
    /// In the files' place, the files recycled.
    Replace,
}

/// How far an action is: jobs done of all, the one just finished, and how far the job under way is (0 to
/// 1, for long ones such as a video; 0 when unknown).
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub current: String,
    pub part: f32,
}

/// A file taken out of the way, and where it went in the Recycle Bin.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Recycled {
    pub path: String,
    pub bin: String,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// Every file made, where it went.
    pub made: Vec<String>,
    /// The files that were replaced, and where each went in the Recycle Bin.
    pub recycled: Vec<Recycled>,
    /// Each job that failed (by its first file), and why.
    pub failed: Vec<(String, String)>,
    /// The jobs' files' sizes, and what they made, for the jobs that worked.
    pub before: u64,
    pub after: u64,
    pub stopped: bool,
    pub elapsed_ms: f64,
}

/// A job: the files it works on (the first decides where what's made goes).
#[derive(Clone, Debug)]
pub struct Job {
    pub inputs: Vec<PathBuf>,
}

impl Job {
    pub fn one(path: impl Into<PathBuf>) -> Self {
        Job { inputs: vec![path.into()] }
    }

    fn first(&self) -> &Path {
        &self.inputs[0]
    }
}

/// A name Windows takes for a file or a folder (`suffix` may be empty).
pub fn valid_name(name: &str, empty: bool) -> bool {
    (empty || !name.trim().is_empty()) && !name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) && !name.ends_with(['.', ' '])
}

/// Checks `output`'s names before anything starts.
pub fn check(output: &Output) -> Result<(), String> {
    match output {
        Output::Beside { suffix } if !valid_name(suffix, true) => Err(format!("“{suffix}” can't be part of a file name")),
        Output::Folder { name } if !valid_name(name, false) => Err(format!("“{name}” can't be a folder's name")),
        _ => Ok(()),
    }
}

/// The folder and name a file made as `made` goes to (made free with " (2)" and so on when placed).
pub fn destination(first: &Path, made: &Path, output: &Output) -> (PathBuf, String) {
    let folder = first.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = made.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match output {
        Output::Beside { suffix } if !suffix.is_empty() => {
            let (stem, ext) = match name.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
                _ => (name.clone(), String::new()),
            };
            (folder, format!("{stem}{suffix}{ext}"))
        }
        Output::Beside { .. } | Output::Replace => (folder, name),
        Output::Folder { name: sub } => (folder.join(sub), name),
    }
}

/// Puts `made` at `folder\name`, or at "name (2)" and so on when that's taken: where it went.
pub fn place(made: &Path, folder: &Path, name: &str) -> Result<String, String> {
    fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    // A file of the name, made new (so nothing's overwritten), that the rename then replaces.
    let to = fsx::create_numbered(&folder.to_string_lossy(), name, false)?;
    if let Err(e) = fs::rename(made, &to) {
        let _ = fs::remove_file(&to);
        // Another drive than the work folder's (it never is, but a link may say so): copy instead.
        if fs::copy(made, &to).is_ok() {
            let _ = fs::remove_file(made);
            return Ok(to);
        }
        return Err(fsx::file_error(e));
    }
    Ok(to)
}

/// The hidden work folder beside `first`, for this action.
fn work_of(first: &Path, tag: &str) -> PathBuf {
    first.parent().unwrap_or(Path::new(".")).join(format!(".glint-{tag}-{}", std::process::id()))
}

/// What one job made (by its index), or why it didn't.
type Made = (usize, Result<Vec<PathBuf>, String>);

/// Runs `jobs`, `at_once` at a time, each made by `make(job, part, work)` into `work` (a folder of its
/// own; `part` reports how far the job is), then placed as `output` says. `tag` names the work folders.
/// Stops (leaving the files as they were) once `stop` is set.
#[allow(clippy::too_many_arguments)]
pub fn run<F>(owner: isize, tag: &str, jobs: &[Job], output: &Output, at_once: usize, stop: &AtomicBool, progress: &(dyn Fn(Progress) + Sync), make: F) -> Result<Report, String>
where
    F: Fn(&Job, &(dyn Fn(f32) + Sync), &Path) -> Result<Vec<PathBuf>, String> + Sync,
{
    let started = Instant::now();
    check(output)?;
    if jobs.iter().any(|j| j.inputs.is_empty()) {
        return Err("A job has no files".into());
    }
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Mutex<Vec<Made>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..at_once.max(1).min(jobs.len()) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(job) = jobs.get(i) else { break };
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let work = work_of(job.first(), tag);
                if fs::create_dir_all(&work).is_ok() {
                    hide(&work);
                }
                let own = work.join(i.to_string());
                let current = job.first().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let part = |p: f32| progress(Progress { done: done.load(Ordering::SeqCst), total: jobs.len(), current: current.clone(), part: p.clamp(0.0, 1.0) });
                let made = fs::create_dir_all(&own).map_err(|e| e.to_string()).and_then(|()| make(job, &part, &own));
                if made.as_ref().err().is_some_and(|e| e == "cancelled") {
                    break;
                }
                let made = made.and_then(|files| if files.is_empty() { Err("Nothing was made".to_string()) } else { Ok(files) });
                results.lock().unwrap_or_else(|e| e.into_inner()).push((i, made));
                progress(Progress { done: done.fetch_add(1, Ordering::SeqCst) + 1, total: jobs.len(), current: current.clone(), part: 0.0 });
            });
        }
    });
    let stopped = stop.load(Ordering::SeqCst);
    let mut results = results.into_inner().unwrap_or_else(|e| e.into_inner());
    results.sort_by_key(|(i, _)| *i);

    let mut report = Report { stopped, ..Report::default() };
    // In their place: the files of the jobs that worked are recycled together first, so their names are free.
    if *output == Output::Replace {
        let originals: Vec<String> = results.iter().filter(|(_, made)| made.is_ok()).flat_map(|(i, _)| jobs[*i].inputs.iter().map(|p| p.to_string_lossy().into_owned())).collect();
        if !originals.is_empty() {
            crate::native_preview::release(&originals);
            if let Ok(performed) = crate::shell::recycle(owner, originals) {
                report.recycled = performed.items.into_iter().filter_map(|item| Some(Recycled { path: item.from, bin: item.to? })).collect();
            }
        }
    }
    for (i, made) in results {
        let job = &jobs[i];
        let first = job.first().to_string_lossy().into_owned();
        match made {
            Ok(files) => {
                let mut placed = Vec::new();
                let mut failure = None;
                for file in &files {
                    let (folder, name) = destination(job.first(), file, output);
                    match place(file, &folder, &name) {
                        Ok(to) => placed.push(to),
                        Err(why) => {
                            failure = Some(why);
                            break;
                        }
                    }
                }
                report.after += placed.iter().map(|p| fs::metadata(p).map_or(0, |m| m.len())).sum::<u64>();
                report.before += job.inputs.iter().map(|p| fs::metadata(p).map_or(0, |m| m.len())).sum::<u64>();
                report.made.extend(placed);
                if let Some(why) = failure {
                    report.failed.push((first, why));
                }
            }
            Err(why) => report.failed.push((first, why)),
        }
    }
    // Sizes of what was replaced come from the Recycle Bin's copies.
    if *output == Output::Replace {
        report.before = report.recycled.iter().map(|r| fs::metadata(&r.bin).map_or(0, |m| m.len())).sum();
    }
    for work in jobs.iter().map(|j| work_of(j.first(), tag)).collect::<BTreeSet<_>>() {
        let _ = fs::remove_dir_all(work);
    }
    report.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(report)
}

/// What a tool's run said: its standard output (when not handed to `on_line`) and its error output.
pub struct Said {
    pub stdout: String,
    /// Kept for the tools that answer on it.
    #[allow(dead_code)]
    pub stderr: String,
}

/// Runs `command` without a console window, in a job that ends it with Glint, until it ends or `stop`
/// is set ("cancelled"). Each line of its standard output goes to `on_line` (and is kept too); its
/// error output is kept. A tool that fails says so with its last error lines.
pub fn run_tool(mut command: Command, name: &str, stop: &AtomicBool, mut on_line: impl FnMut(&str)) -> Result<Said, String> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::proc::no_window(&mut command);
    let mut child = command.spawn().map_err(|e| format!("Couldn't start {name}: {e}"))?;
    #[cfg(windows)]
    let job = crate::proc::Job::ending().ok();
    #[cfg(windows)]
    if let Some(job) = &job {
        let _ = job.assign(&child);
    }
    let mut stderr = child.stderr.take().ok_or_else(|| format!("{name} has no output"))?;
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let stdout = child.stdout.take().ok_or_else(|| format!("{name} has no output"))?;
    let (lines_tx, lines_rx) = std::sync::mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).split(b'\n').map_while(Result::ok) {
            if lines_tx.send(String::from_utf8_lossy(&line).trim_end_matches('\r').to_string()).is_err() {
                break;
            }
        }
    });
    let mut out = String::new();
    let status = loop {
        while let Ok(line) = lines_rx.try_recv() {
            on_line(&line);
            out.push_str(&line);
            out.push('\n');
        }
        if stop.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if let Ok(line) = lines_rx.recv_timeout(Duration::from_millis(20)) {
                    on_line(&line);
                    out.push_str(&line);
                    out.push('\n');
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let _ = reader.join();
    for line in lines_rx.try_iter() {
        on_line(&line);
        out.push_str(&line);
        out.push('\n');
    }
    let stderr = errors.join().unwrap_or_default();
    if !status.success() {
        let last: Vec<&str> = stderr.lines().chain(out.lines()).map(str::trim).filter(|l| !l.is_empty()).collect();
        let tail = last.iter().rev().take(2).rev().cloned().collect::<Vec<_>>().join(" · ");
        return Err(if tail.is_empty() { format!("{name} stopped with exit code {}", status.code().unwrap_or(-1)) } else { tail });
    }
    Ok(Said { stdout: out, stderr })
}

/// Hides a work folder, which only lasts as long as its action.
#[cfg(windows)]
pub fn hide(folder: &Path) {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};
    let _ = unsafe { SetFileAttributesW(&HSTRING::from(folder.as_os_str()), FILE_ATTRIBUTE_HIDDEN) };
}

#[cfg(not(windows))]
pub fn hide(_: &Path) {}

/// `path`'s name with another extension (`ext` without the dot), in `folder`.
pub fn renamed(folder: &Path, path: &Path, ext: &str) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    folder.join(format!("{stem}.{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_what_it_makes() {
        let first = Path::new(r"C:\Videos\beach.mov");
        let made = Path::new(r"C:\Videos\.glint-media-1\0\beach.mp4");
        assert_eq!(destination(first, made, &Output::Beside { suffix: " (small)".into() }), (PathBuf::from(r"C:\Videos"), "beach (small).mp4".into()));
        assert_eq!(destination(first, made, &Output::Beside { suffix: String::new() }), (PathBuf::from(r"C:\Videos"), "beach.mp4".into()));
        assert_eq!(destination(first, made, &Output::Folder { name: "Converted".into() }), (PathBuf::from(r"C:\Videos\Converted"), "beach.mp4".into()));
        assert_eq!(destination(first, made, &Output::Replace), (PathBuf::from(r"C:\Videos"), "beach.mp4".into()));
        // A name without an extension keeps the suffix at its end.
        assert_eq!(destination(first, Path::new(r"C:\w\README"), &Output::Beside { suffix: "-2".into() }).1, "README-2");
        assert!(check(&Output::Beside { suffix: "a/b".into() }).is_err() && check(&Output::Folder { name: " ".into() }).is_err() && check(&Output::Replace).is_ok());
        assert_eq!(renamed(Path::new(r"C:\w"), Path::new(r"C:\x\clip.final.mkv"), "mp4"), PathBuf::from(r"C:\w\clip.final.mp4"));
    }

    #[test]
    fn runs_jobs_and_places_what_they_make() {
        let dir = std::env::temp_dir().join(format!("glint-actions-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for name in ["a.txt", "b.txt", "a.out"] {
            fs::write(dir.join(name), name).unwrap();
        }
        let jobs = vec![Job::one(dir.join("a.txt")), Job::one(dir.join("b.txt")), Job::one(dir.join("missing.txt"))];
        let stop = AtomicBool::new(false);
        let report = run(0, "test", &jobs, &Output::Beside { suffix: String::new() }, 2, &stop, &|_| {}, |job, part, work| {
            part(0.5);
            let text = fs::read_to_string(&job.inputs[0]).map_err(|e| e.to_string())?;
            let made = renamed(work, &job.inputs[0], "out");
            fs::write(&made, text.to_uppercase()).map_err(|e| e.to_string())?;
            Ok(vec![made])
        })
        .unwrap();
        // a.out was there already: a (2) beside it; b.out is new; the missing file failed.
        assert_eq!(report.made.len(), 2);
        assert!(report.made[0].ends_with("a (2).out"), "{:?}", report.made);
        assert_eq!(fs::read_to_string(dir.join("a (2).out")).unwrap(), "A.TXT");
        assert_eq!(fs::read_to_string(dir.join("a.out")).unwrap(), "a.out");
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].0.ends_with("missing.txt"));
        // The work folder went.
        assert!(!fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(".glint-")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn says_what_a_failing_tool_said() {
        let stop = AtomicBool::new(false);
        let mut command = Command::new("cmd");
        command.args(["/c", "echo first & echo oops 1>&2 & exit 3"]);
        let mut lines = Vec::new();
        let err = run_tool(command, "cmd", &stop, |l| lines.push(l.to_string())).err().unwrap();
        assert!(err.contains("oops"), "{err}");
        assert_eq!(lines, ["first "]);
    }
}
