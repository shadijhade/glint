//! `Glint.exe bench`: times the work behind the README's benchmark table on this PC, stage by stage, so
//! a change made for speed can say what it did (and where the time goes before one is made). It runs
//! inside Glint's own binary, so it measures the very code the app runs: listing a folder (Windows'
//! enumeration, the sort, the encoding for the page, each on its own), name search, typo search and
//! folder sizes through the instant search service when it's running, and, with `--copy`, copying many
//! small files one at a time (the copy queue's way) against several at once.
//!
//! `npm run bench` (scripts/bench.mjs) runs it and compares the run with the one before. Each measure
//! is run `--runs` times (after one warm-up run), and its median and best are reported, in ms.

use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Measure {
    name: String,
    median_ms: f64,
    best_ms: f64,
    /// What it worked on: a folder's items, a search's results, files copied.
    items: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    version: &'static str,
    /// Unix ms.
    when: i64,
    cpu: String,
    threads: usize,
    runs: usize,
    measures: Vec<Measure>,
}

struct Options {
    out: Option<PathBuf>,
    runs: usize,
    copy: usize,
    folders: Vec<String>,
}

/// `Glint.exe bench [--runs N] [--out file.json] [--copy N] [folder…]`: None when that isn't what was asked.
pub fn cli() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("bench") {
        return None;
    }
    let mut options = Options { out: None, runs: 7, copy: 0, folders: Vec::new() };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => options.out = args.next().map(PathBuf::from),
            "--runs" => options.runs = args.next().and_then(|n| n.parse().ok()).unwrap_or(7).clamp(1, 1000),
            "--copy" => options.copy = args.next().and_then(|n| n.parse().ok()).unwrap_or(5000).min(1_000_000),
            _ => options.folders.push(arg),
        }
    }
    if options.folders.is_empty() {
        options.folders.push(std::env::var("SystemRoot").map(|r| format!(r"{r}\System32")).unwrap_or_else(|_| r"C:\Windows\System32".into()));
    }
    let report = run(&options);
    for m in &report.measures {
        say(&format!("{:<44} {:>10.2} ms  (best {:>9.2})  {:>9} items", m.name, m.median_ms, m.best_ms, m.items));
    }
    let json = serde_json::to_string_pretty(&report).unwrap_or_default();
    match &options.out {
        Some(out) => {
            if let Some(dir) = out.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(out, json) {
                say(&format!("Couldn't write {}: {e}", out.display()));
                return Some(1);
            }
        }
        None => say(&json),
    }
    Some(0)
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}").and_then(|()| out.flush());
}

/// Runs `work` once to warm up, then `runs` times: its median and best, and what the last run counted.
fn time(runs: usize, mut work: impl FnMut() -> u64) -> (f64, f64, u64) {
    let mut items = work();
    let mut times: Vec<f64> = (0..runs)
        .map(|_| {
            let started = Instant::now();
            items = work();
            started.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(f64::total_cmp);
    (times[times.len() / 2], times[0], items)
}

fn run(options: &Options) -> Report {
    let mut measures = Vec::new();
    let mut add = |name: String, (median_ms, best_ms, items): (f64, f64, u64)| measures.push(Measure { name, median_ms, best_ms, items });
    for folder in &options.folders {
        let label = short(folder);
        let Ok(entries) = crate::fsx::read_dir(folder) else {
            say(&format!("Couldn't list {folder}"));
            continue;
        };
        add(format!("list {label}: enumerate"), time(options.runs, || crate::fsx::read_dir(folder).map_or(0, |e| e.len() as u64)));
        add(
            format!("list {label}: sort"),
            time(options.runs, || {
                let mut copy = entries.clone();
                crate::fsx::sort_entries(&mut copy);
                copy.len() as u64
            }),
        );
        add(
            format!("list {label}: all"),
            time(options.runs, || {
                let Ok(mut entries) = crate::fsx::read_dir(folder) else { return 0 };
                crate::fsx::sort_entries(&mut entries);
                entries.len() as u64
            }),
        );
    }
    index(options.runs, &mut add);
    if options.copy > 0 {
        copies(options.copy, &mut add);
    }
    Report {
        version: env!("CARGO_PKG_VERSION"),
        when: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64),
        cpu: std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default(),
        threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        runs: options.runs,
        measures,
    }
}

/// A folder's name for a measure: its last two parts.
fn short(folder: &str) -> String {
    let parts: Vec<&str> = folder.trim_end_matches('\\').rsplit('\\').take(2).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("\\")
}

/// Name search (as typed and with a typo) over every fixed drive, and a folder's size, through the
/// instant search service, end to end as the app asks (the pipe included). Skipped when it isn't running.
#[cfg(windows)]
fn index(runs: usize, add: &mut impl FnMut(String, (f64, f64, u64))) {
    use shadui_index::client::{self, Event};
    if client::status().is_err() {
        say("The instant search service isn't running: its measures are skipped.");
        return;
    }
    let roots = fixed_drives();
    let search = |query: &str, fuzzy: bool| {
        let request = shadui_index::proto::SearchRequest { query: query.into(), roots: roots.clone(), hidden: false, exclude: Vec::new(), max_results: Some(1000), fuzzy, rank: true };
        let mut count = 0;
        let _ = client::search(&request, |event| {
            if let Event::Hits(hits) = event {
                count += hits.len() as u64;
            }
            true
        });
        count
    };
    add(format!("index search kernel32.dll ({} drives)", roots.len()), time(runs, || search("kernel32.dll", false)));
    add(format!("index typo search kernal32 ({} drives)", roots.len()), time(runs, || search("kernal32", true)));
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    add(format!("index sizes {}", short(&windows)), time(runs, || client::sizes(&windows).map_or(0, |s| s.sizes.len() as u64)));
}

#[cfg(not(windows))]
fn index(_: usize, _: &mut impl FnMut(String, (f64, f64, u64))) {}

#[cfg(windows)]
fn fixed_drives() -> Vec<String> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    const DRIVE_FIXED: u32 = 3;
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", (b'A' + i) as char))
        .filter(|root| unsafe { GetDriveTypeW(&HSTRING::from(root.as_str())) } == DRIVE_FIXED)
        .collect()
}

/// `count` files of 4 KB, made in the temp folder, copied one at a time with CopyFileEx (as the copy
/// queue does) and then by a pool of threads, each into a fresh folder; everything made is removed after.
fn copies(count: usize, add: &mut impl FnMut(String, (f64, f64, u64))) {
    let root = std::env::temp_dir().join(format!("glint-bench-{}", std::process::id()));
    let source = root.join("from");
    if std::fs::create_dir_all(&source).is_err() {
        return;
    }
    let block = vec![0x5Au8; 4096];
    let files: Vec<PathBuf> = (0..count).map(|i| source.join(format!("file {i:06}.bin"))).collect();
    if files.iter().any(|f| std::fs::write(f, &block).is_err()) {
        say("Couldn't make the files to copy");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    let mut round = 0;
    let mut once = |threads: usize| {
        round += 1;
        let to = root.join(format!("to-{round}"));
        let _ = std::fs::create_dir_all(&to);
        let started = Instant::now();
        let copied = copy_all(&files, &to, threads);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let _ = std::fs::remove_dir_all(&to);
        (ms, ms, copied)
    };
    add(format!("copy {count} small files, one at a time"), once(1));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(4, 16);
    add(format!("copy {count} small files, {threads} at a time"), once(threads));
    let _ = std::fs::remove_dir_all(&root);
}

/// Copies `files` into `to` with `threads` workers: how many were copied.
fn copy_all(files: &[PathBuf], to: &Path, threads: usize) -> u64 {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let copied = std::sync::atomic::AtomicU64::new(0);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(file) = files.get(i) else { break };
                let target = to.join(file.file_name().unwrap_or_default());
                if copy_file(file, &target) {
                    copied.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            });
        }
    });
    copied.into_inner()
}

#[cfg(windows)]
fn copy_file(from: &Path, to: &Path) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{CopyFileExW, COPY_FILE_COPY_SYMLINK};
    unsafe { CopyFileExW(&HSTRING::from(from.as_os_str()), &HSTRING::from(to.as_os_str()), None, None, None, COPY_FILE_COPY_SYMLINK) }.is_ok()
}

#[cfg(not(windows))]
fn copy_file(from: &Path, to: &Path) -> bool {
    std::fs::copy(from, to).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_folders_by_their_last_parts() {
        assert_eq!(short(r"C:\Windows\System32"), r"Windows\System32");
        assert_eq!(short(r"D:\test\100k\"), r"test\100k");
        assert_eq!(short(r"C:\"), "C:");
    }

    #[test]
    fn copies_with_a_pool() {
        let dir = std::env::temp_dir().join(format!("glint-bench-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        let files: Vec<PathBuf> = (0..20).map(|i| dir.join("a").join(format!("{i}.txt"))).collect();
        for f in &files {
            std::fs::write(f, "x").unwrap();
        }
        assert_eq!(copy_all(&files, &dir.join("b"), 4), 20);
        assert_eq!(std::fs::read_dir(dir.join("b")).unwrap().count(), 20);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
