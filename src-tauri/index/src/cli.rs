//! `shadui-index`: the service itself, its installer, and tools to query and measure it.

use crate::client::{self, Event};
use crate::engine::{self, Stop};
use crate::proto::{SearchRequest, UsageRequest};
use crate::query::{Excludes, NameMatcher};
use crate::tree::{Search, ROOT};
use crate::volume::Volume;
use crate::{install, service, snapshot, win};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use windows::Win32::System::Console::SetConsoleCtrlHandler;

const USAGE: &str = "\
shadui-index: instant file name search and folder sizes for ShadUI Ripgrep

  service                     run as the Windows service (started by Windows)
  install [--allow-sid SID]   install and start the service (administrator)
  uninstall                   stop and remove it, with its snapshots (administrator)
  run [--volume C:]...        run in this console instead of as a service (administrator)
  status                      what the running service has indexed
  search QUERY [--in PATH]... [--max N] [--hidden] [--repeat N] [--print | --long]
  sizes PATH                  folder sizes from the running service
  usage PATH [--depth N] [--max N] [--repeat N]
                              a disk-usage map from the running service, timed end to end
  bench [C:]...               read drives directly and time indexing and queries (administrator)
";

fn values(args: &[String], flag: &str) -> Vec<String> {
    args.windows(2).filter(|w| w[0] == flag).map(|w| w[1].clone()).collect()
}

fn letters(items: impl IntoIterator<Item = String>) -> Vec<char> {
    items.into_iter().filter_map(|v| v.chars().next()).filter(char::is_ascii_alphabetic).map(|c| c.to_ascii_uppercase()).collect()
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Runs the command line; returns the process exit code.
pub fn main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("service") => match service::dispatch() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("Not started by Windows: {e}. Use `run` to try it in a console.");
                1
            }
        },
        Some("install") => match install::install(&values(rest, "--allow-sid")) {
            Ok(()) => {
                println!("Installed and started {}.", crate::SERVICE_NAME);
                0
            }
            Err(failure) => {
                eprintln!("{failure}");
                failure.exit_code()
            }
        },
        Some("uninstall") => match install::uninstall() {
            Ok(()) => {
                println!("Removed {}.", crate::SERVICE_NAME);
                0
            }
            Err(failure) => {
                eprintln!("{failure}");
                failure.exit_code()
            }
        },
        Some("run") => run(rest),
        Some("status") => match client::status() {
            Ok(status) => {
                println!("{}", serde_json::to_string_pretty(&status).unwrap_or_default());
                0
            }
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Some("search") if rest.first().is_some_and(|q| !q.starts_with("--")) => search(rest),
        Some("sizes") if !rest.is_empty() => match client::sizes(&rest[0]) {
            Ok(mut sizes) => {
                sizes.sizes.sort_by_key(|s| std::cmp::Reverse(s.1));
                println!("{} bytes in {} ({:.2} ms in the service)", sizes.total, sizes.path, sizes.elapsed);
                for (name, size) in sizes.sizes.iter().take(20) {
                    println!("{size:>16}  {name}");
                }
                0
            }
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Some("usage") if rest.first().is_some_and(|p| !p.starts_with("--")) => usage(rest),
        Some("bench") => bench(rest),
        _ => {
            eprint!("{USAGE}");
            1
        }
    }
}

static CONSOLE_STOP: OnceLock<Arc<Stop>> = OnceLock::new();

unsafe extern "system" fn on_ctrl_c(_kind: u32) -> windows::core::BOOL {
    if let Some(stop) = CONSOLE_STOP.get() {
        stop.set();
    }
    true.into()
}

/// The service, in a console: for trying it out and debugging. Ctrl+C stops it (saving snapshots).
fn run(args: &[String]) -> i32 {
    if !win::is_elevated() {
        eprintln!("Reading drives directly needs an administrator terminal.");
        return 1;
    }
    let mut allowed = values(args, "--allow-sid");
    if let Ok(me) = win::current_user_sid() {
        allowed.push(me);
    }
    let only = Some(letters(values(args, "--volume"))).filter(|v| !v.is_empty());
    let stop = CONSOLE_STOP.get_or_init(|| Arc::new(Stop::default())).clone();
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(on_ctrl_c), true);
    }
    match service::run(&stop, &allowed, only, true, || eprintln!("Listening on {}; Ctrl+C stops.", crate::PIPE_NAME)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn median_p95(mut times: Vec<f64>) -> (f64, f64) {
    times.sort_by(f64::total_cmp);
    (times[times.len() / 2], times[(times.len() * 95).div_ceil(100) - 1])
}

/// A search through the running service, timed end to end (connect, ask, receive every result).
fn search(args: &[String]) -> i32 {
    let mut roots = values(args, "--in");
    if roots.is_empty() {
        roots = crate::volume::fixed_ntfs_drives().iter().map(|l| format!(r"{l}:\")).collect();
    }
    let request = SearchRequest {
        query: args[0].clone(),
        roots,
        hidden: args.iter().any(|a| a == "--hidden"),
        exclude: vec!["node_modules".into(), ".git".into()],
        max_results: Some(values(args, "--max").first().and_then(|m| m.parse().ok()).unwrap_or(50_000)),
    };
    let repeat: usize = values(args, "--repeat").first().and_then(|r| r.parse().ok()).unwrap_or(1).max(1);
    let print = args.iter().any(|a| a == "--print");
    let long = args.iter().any(|a| a == "--long");
    let mut times = Vec::new();
    let mut summary = String::new();
    for round in 0..repeat {
        let started = Instant::now();
        let (mut first, mut received) = (None, 0usize);
        let result = client::search(&request, |event| {
            match event {
                Event::Plan { skipped, .. } if !skipped.is_empty() && round == 0 => eprintln!("Not indexed: {}", skipped.join(", ")),
                Event::Hits(hits) => {
                    first.get_or_insert_with(|| ms(started));
                    received += hits.len();
                    if (print || long) && round == 0 {
                        for hit in &hits {
                            if long {
                                println!("{:>15} {:08x} {:>14} {}{}", hit.size, hit.attrs, hit.modified, hit.path, if hit.dir { "\\" } else { "" });
                            } else {
                                println!("{}", hit.path);
                            }
                        }
                    }
                }
                Event::Done { count, limited, elapsed } => {
                    summary = format!("{count} found{} · {elapsed:.1} ms in the service", if limited { " (limit reached)" } else { "" });
                }
                _ => {}
            }
            true
        });
        if let Err(e) = result {
            eprintln!("{e}");
            return 1;
        }
        times.push(ms(started));
        if round == 0 {
            eprintln!("{summary} · {received} received · first results after {:.1} ms", first.unwrap_or(0.0));
        }
    }
    let (median, p95) = median_p95(times);
    eprintln!("end to end: median {median:.1} ms, p95 {p95:.1} ms over {repeat} run(s)");
    0
}

/// A disk-usage map through the running service, timed end to end; prints what's biggest at the top.
fn usage(args: &[String]) -> i32 {
    let number = |flag: &str, default: u32| values(args, flag).first().and_then(|v| v.parse().ok()).unwrap_or(default);
    let request = UsageRequest { path: args[0].clone(), depth: number("--depth", 4), max_items: number("--max", 20_000) };
    let repeat = number("--repeat", 1).max(1);
    let mut times = Vec::new();
    for round in 0..repeat {
        let started = Instant::now();
        let (nodes, summary) = match client::usage(&request) {
            Ok(map) => map,
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        };
        times.push(ms(started));
        if round == 0 {
            let more = if summary.limited { ", the biggest" } else { "" };
            println!("{} bytes in {}: {} items{more}, {} levels down ({:.2} ms in the service)", summary.total, summary.path, summary.items, request.depth, summary.elapsed);
            for node in nodes.iter().filter(|n| n.parent == 0).take(20) {
                println!("{:>16}  {}{}", node.size, node.name, if node.dir { "\\" } else { "" });
            }
        }
    }
    let (median, p95) = median_p95(times);
    eprintln!("end to end: median {median:.1} ms, p95 {p95:.1} ms over {repeat} run(s)");
    0
}

/// Indexes drives in this process and times everything the service does with them.
fn bench(args: &[String]) -> i32 {
    if !win::is_elevated() {
        eprintln!("Reading drives directly needs an administrator terminal.");
        return 1;
    }
    let mut drives = letters(args.iter().filter(|a| !a.starts_with("--")).cloned());
    if drives.is_empty() {
        drives.push('C');
    }
    let excludes = Excludes::new(&["node_modules".into(), ".git".into()]);
    for letter in drives {
        let volume = match Volume::open(letter) {
            Ok(volume) => volume,
            Err(e) => {
                eprintln!("{letter}: {e}");
                continue;
            }
        };
        let started = Instant::now();
        let mut tree = match volume.scan(&|_, _| true) {
            Ok(tree) => tree,
            Err(e) => {
                eprintln!("{letter}: {e}");
                continue;
            }
        };
        let scan = ms(started);
        let stats = tree.stats();
        println!("{letter}: read the MFT ({} records) in {scan:.0} ms", tree.record_count());
        println!("    {} files, {} folders, {} names, {} MB in memory", stats.files, stats.folders, stats.names, stats.memory >> 20);

        let path = std::env::temp_dir().join(format!("shadui-index-bench-{letter}.idx"));
        let started = Instant::now();
        let meta = snapshot::Meta { volume_serial: volume.serial, ..Default::default() };
        if let Err(e) = snapshot::save(&path, &tree, &meta) {
            eprintln!("    snapshot: {e}");
        } else {
            let saved = ms(started);
            let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
            let started = Instant::now();
            let loaded = snapshot::load(&path).is_ok();
            println!("    snapshot: {} MB, saved in {saved:.0} ms, loaded in {:.0} ms{}", bytes >> 20, ms(started), if loaded { "" } else { " (FAILED)" });
            let _ = std::fs::remove_file(&path);
        }

        let started = Instant::now();
        tree.recompute_totals();
        println!("    recounting every folder total: {:.0} ms", ms(started));
        let started = Instant::now();
        let (total, children) = (tree.rec(ROOT).map_or(0, |r| r.size), tree.child_dirs(ROOT));
        println!("    folder sizes of {letter}:\\ ({} folders, {} GB): {:.3} ms", children.len(), total >> 30, ms(started));
        for depth in [2, 4, 6] {
            let mut times = Vec::new();
            let mut items = 0;
            for _ in 0..11 {
                let started = Instant::now();
                items = tree.usage(ROOT, depth, 20_000).0.len();
                times.push(ms(started));
            }
            let (median, p95) = median_p95(times);
            println!("    disk-usage map of {letter}:\\, {depth} levels ({items} items): median {median:.1} ms, p95 {p95:.1} ms");
        }

        println!("    query (app defaults: no hidden, skip node_modules/.git, max 50k)   matches   median    p95");
        let queries = ["kernel32.dll", "report", ".pdf", "*.dll", "budget 2026", "zzqxjv", "résumé", "e"];
        for query in queries {
            let matcher = NameMatcher::new(query).unwrap();
            let search = Search { matcher: &matcher, scope: ROOT, hidden: false, excludes: &excludes };
            let mut times = Vec::new();
            let mut found = 0;
            for round in 0..24 {
                let started = Instant::now();
                let (frames, matched) = engine::collect_hits(&tree, letter, &search, 50_000);
                std::hint::black_box(frames);
                if round >= 3 {
                    times.push(ms(started));
                }
                found = matched;
            }
            let (median, p95) = median_p95(times);
            println!("    {query:<66} {found:>9} {median:>7.1}ms {p95:>6.1}ms");
        }
    }
    0
}
