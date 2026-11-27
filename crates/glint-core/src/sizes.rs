//! Folder sizes, which Explorer never shows: from the instant search index when it has the folder
//! (instant, and kept current), otherwise measured with pdu's engine (the parallel-disk-usage crate) in a
//! worker copy of Glint: [`worker`].

use crate::fsx::{blocking, ms_since};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;
use crate::host::State;

pub struct FolderSizes {
    pub path: String,
    /// Everything in the folder, files included, in bytes.
    pub total: u64,
    /// Subfolder name → bytes.
    pub sizes: Vec<(String, u64)>,
    pub elapsed: f64,
    /// "index" or "pdu", or the archive's format ("zip", "7z") for a folder in one.
    pub engine: &'static str,
}

/// The sizing worker prints what pdu's `--json-output` does, {"unit":"bytes","tree":{"name":"C:\\dir",
/// "size":123,"children":[…]}}: the tree's top named as it was asked for (several at once: "(total)", with
/// each under it by its path), what's in a folder by its own name.
#[derive(Deserialize, Serialize, Debug, PartialEq)]
pub(crate) struct Node {
    pub size: u64,
    pub name: String,
    #[serde(default)]
    pub children: Vec<Node>,
}

#[derive(Deserialize, Serialize)]
struct Output {
    #[serde(default)]
    unit: String,
    tree: Node,
}

/// The worker's whole tree.
pub(crate) fn parse_tree(json: &str) -> Result<Node, String> {
    serde_json::from_str::<Output>(json.trim()).map(|o| o.tree).map_err(|e| format!("Unexpected sizes from the worker: {e}"))
}

const WORKER_ARG: &str = "--sizes-worker";

/// Glint started as the sizing worker, `Glint.exe --sizes-worker <max depth> <min ratio> -- <path>…`:
/// measures each path with pdu's own engine, apparent sizes (what Explorer's Properties calls "Size"), and
/// prints the tree as pdu's `--json-output --quantity apparent-size --silent-errors` did, before anything of
/// the app starts (main.rs). A process rather than a thread because the crate's walk can't be stopped part
/// way, and a process can: a run no longer wanted (another folder, Stop, a closed tab) ends with it
/// (`proc::Slot`), however big the drive. Its author's suggestion (KSXGitHub/parallel-disk-usage#461).
pub fn worker() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(WORKER_ARG)) {
        return None;
    }
    let Some((max_depth, min_ratio, paths)) = worker_args(args) else {
        eprint!("usage: Glint.exe {WORKER_ARG} <max depth> <min ratio> -- <path>...");
        return Some(2);
    };
    let tree = measure_tree(paths, max_depth, min_ratio);
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let written = serde_json::to_writer(&mut out, &Output { unit: "bytes".into(), tree }).map_err(|e| e.to_string());
    Some(match written.and_then(|_| std::io::Write::flush(&mut out).map_err(|e| e.to_string())) {
        Ok(()) => 0,
        Err(_) => 1,
    })
}

/// The worker's command line after `--sizes-worker`: pdu's `--max-depth` (a whole number, 1 or more) and
/// `--min-ratio` (0 to under 1), then `--` and the paths.
fn worker_args(mut args: impl Iterator<Item = std::ffi::OsString>) -> Option<(u64, f32, Vec<PathBuf>)> {
    let max_depth = args.next()?.to_str()?.parse::<u64>().ok().filter(|&d| d >= 1)?;
    let min_ratio = args.next()?.to_str()?.parse::<f32>().ok().filter(|r| (0.0..1.0).contains(r))?;
    if args.next().as_deref() != Some(OsStr::new("--")) {
        return None;
    }
    let paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
    (!paths.is_empty()).then_some((max_depth, min_ratio, paths))
}

/// What pdu's command line does with these options, as its `app/sub.rs` does it: each path's tree built in
/// parallel, several under one "(total)", what's under `min_ratio` of the whole left out, biggest first.
/// Unreadable entries count as nothing (`--silent-errors`); hard links and other drives count in full, as
/// pdu counts them by default.
fn measure_tree(paths: Vec<PathBuf>, max_depth: u64, min_ratio: f32) -> Node {
    use parallel_disk_usage::{
        data_tree::DataTree,
        device::DeviceBoundary,
        fs_tree_builder::FsTreeBuilder,
        get_size::GetApparentSize,
        hardlink::HardlinkIgnorant,
        os_string_display::OsStringDisplay,
        reporter::{ErrorOnlyReporter, ErrorReport},
        size::Bytes,
    };
    let reporter = ErrorOnlyReporter::new(ErrorReport::SILENT);
    let mut trees: Vec<DataTree<OsStringDisplay, Bytes>> = paths
        .into_iter()
        .map(|root| FsTreeBuilder { reporter: &reporter, root, size_getter: GetApparentSize, hardlinks_recorder: &HardlinkIgnorant, device_boundary: DeviceBoundary::Cross, max_depth }.into())
        .collect();
    let several = trees.len() > 1;
    let mut tree = match trees.pop() {
        Some(only) if !several => only,
        last => {
            trees.extend(last);
            // The "(total)" on top is one of the `max_depth` levels, as in pdu's output.
            DataTree::dir(OsStringDisplay::os_string_from(""), Bytes::default(), trees).into_par_retained(|_, depth| depth + 1 < max_depth)
        }
    };
    // pdu's `par_cull_insignificant_data`, which the crate builds only with its command line.
    if min_ratio > 0.0 {
        let minimal = u64::from(tree.size()) as f32 * min_ratio;
        tree.par_retain(|descendant, _| u64::from(descendant.size()) as f32 >= minimal);
    }
    tree.par_sort_by(|left, right| left.size().cmp(&right.size()).reverse());
    fn node(tree: &DataTree<OsStringDisplay, Bytes>) -> Node {
        Node { size: tree.size().into(), name: tree.name().to_string(), children: tree.children().iter().map(node).collect() }
    }
    let mut top = node(&tree);
    if several {
        top.name = "(total)".into();
    }
    top
}

/// Glint itself as the sizing worker ([`worker`]), measuring `paths` `max_depth` levels down and leaving out
/// what's under `min_ratio` of the whole (pdu's options of the same names); its tree comes on stdout.
pub(crate) fn worker_command<P: AsRef<OsStr>>(max_depth: u32, min_ratio: f32, paths: &[P]) -> Result<Command, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Glint can't find its own program to measure with: {e}"))?;
    let mut command = Command::new(exe);
    crate::proc::no_window(&mut command);
    command
        .arg(WORKER_ARG)
        .arg(max_depth.max(1).to_string())
        .arg(min_ratio.clamp(0.0, 0.99).to_string())
        .arg("--")
        .args(paths)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    Ok(command)
}

fn parse(json: &str) -> Result<(u64, Vec<(String, u64)>), String> {
    let root = parse_tree(json)?;
    let sizes = root.children.iter().map(|c| (c.name.clone(), c.size)).collect();
    Ok((root.size, sizes))
}

/// The index's answer for `path`, if its service runs and has the folder. Junctions and links are
/// resolved first: the index knows each folder by its real path.
#[cfg(windows)]
fn index_sizes(path: &str) -> Option<(u64, Vec<(String, u64)>)> {
    let real = std::fs::canonicalize(path).ok()?;
    let sizes = shadui_index::client::sizes(&real.to_string_lossy()).ok()?;
    Some((sizes.total, sizes.sizes))
}

#[cfg(not(windows))]
fn index_sizes(_: &str) -> Option<(u64, Vec<(String, u64)>)> {
    None
}

/// The worker measuring what's in `path`, one level deep (files come too: the list takes folders' sizes
/// only), in bytes, nothing left out.
fn pdu_sizes(path: &str) -> Result<Command, String> {
    worker_command(2, 0.0, &[path])
}

/// A folder's total, each folder in it with its own, and which engine measured them.
pub(crate) type Measured = (u64, Vec<(String, u64)>, &'static str);

/// The total of `path` and of each folder in it, for a caller that can wait (an agent asking, mcp.rs):
/// the index's answer, or pdu's.
pub(crate) fn measure(path: &str) -> Result<Measured, String> {
    if let Some((total, sizes)) = index_sizes(path) {
        return Ok((total, sizes, "index"));
    }
    let output = pdu_sizes(path)?.output().map_err(|e| format!("Couldn't start measuring: {e}"))?;
    let (total, sizes) = parse(&String::from_utf8_lossy(&output.stdout))?;
    Ok((total, sizes, "pdu"))
}

/// Sizes of every subfolder of `path`. With `index_only`, fails instead of running pdu when the
/// index can't answer (for sizing every folder as it opens, which only makes sense when it's instant).
/// Each tab asks in its own `lane`, so sizing a folder in one pane doesn't stop the other's.
pub async fn folder_sizes(state: State<'_, AppState>, path: String, index_only: Option<bool>, lane: String) -> Result<FolderSizes, String> {
    let started = Instant::now();
    let slot = state.sizes.slot(&lane);
    let asked = path.clone();
    // A folder in an archive: the archive's own list of entries has every size.
    if let Some(found) = blocking(move || Ok(crate::archive::folder_sizes(&asked))).await? {
        slot.kill();
        let (total, sizes, engine) = found?;
        return Ok(FolderSizes { path, total, sizes, elapsed: ms_since(started), engine });
    }
    let asked = path.clone();
    if let Some((total, sizes)) = blocking(move || Ok(index_sizes(&asked))).await? {
        // A pdu run still going in this tab is for a folder no longer wanted.
        slot.kill();
        return Ok(FolderSizes { path, total, sizes, elapsed: ms_since(started), engine: "index" });
    }
    if index_only == Some(true) {
        return Err("not indexed".into());
    }
    // Started on a blocking thread too: Windows takes its time to start a program, and the runtime has few threads.
    blocking(move || {
        let mut child = pdu_sizes(&path)?.spawn().map_err(|e| format!("Couldn't start measuring: {e}"))?;
        let mut stdout = child.stdout.take().ok_or("Couldn't read the sizes")?;
        let pid = slot.replace(child);
        let mut json = String::new();
        let _ = stdout.read_to_string(&mut json);
        if slot.finish(pid).is_none() {
            return Err("cancelled".into());
        }
        let (total, sizes) = parse(&json)?;
        Ok(FolderSizes { path, total, sizes, elapsed: ms_since(started), engine: "pdu" })
    })
    .await
}

/// Stops the tab's sizing (Stop, or the tab closed).
pub fn cancel_folder_sizes(state: State<AppState>, lane: String) {
    state.sizes.close(&lane);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn parses_pdu_json() {
        let json = r#" {"schema-version":"2026-04-02","pdu":"0.24.0","unit":"bytes","tree":{"name":"C:\\repo","size":12076482,"children":[{"name":".git","size":5778575,"children":[]},{"name":"src","size":92090,"children":[]}]}}"#;
        let (total, sizes) = parse(json).unwrap();
        assert_eq!(total, 12_076_482);
        assert_eq!(sizes, [(".git".to_string(), 5_778_575), ("src".to_string(), 92_090)]);
        assert!(parse("").is_err());
    }

    /// A folder of known sizes: big\ (a 3000-byte file and deep\, 500 more), small\ (10), and a 100-byte file.
    fn sample(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-sizes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("big").join("deep")).unwrap();
        std::fs::create_dir_all(dir.join("small")).unwrap();
        std::fs::write(dir.join("big").join("a.bin"), vec![0u8; 3000]).unwrap();
        std::fs::write(dir.join("big").join("deep").join("b.bin"), vec![0u8; 500]).unwrap();
        std::fs::write(dir.join("small").join("c.bin"), vec![0u8; 10]).unwrap();
        std::fs::write(dir.join("top.txt"), vec![0u8; 100]).unwrap();
        dir
    }

    fn sizes_of(node: &Node) -> Vec<(&str, u64)> {
        node.children.iter().map(|c| (c.name.as_str(), c.size)).collect()
    }

    #[test]
    fn the_worker_measures_a_folder_as_pdu_did() {
        let dir = sample("one");
        // Glint's folder sizes: two levels (the folder and what's in it), nothing left out, biggest first.
        let tree = measure_tree(vec![dir.clone()], 2, 0.0);
        assert_eq!(tree.name, dir.to_string_lossy());
        assert_eq!(tree.size, 3610);
        assert_eq!(sizes_of(&tree), [("big", 3500), ("top.txt", 100), ("small", 10)]);
        assert!(tree.children.iter().all(|c| c.children.is_empty()), "deeper levels count but aren't listed");
        // Three levels, and what's under a tenth of the whole left out (small\ and top.txt).
        let tree = measure_tree(vec![dir.clone()], 3, 0.1);
        assert_eq!(sizes_of(&tree), [("big", 3500)]);
        assert_eq!(sizes_of(&tree.children[0]), [("a.bin", 3000), ("deep", 500)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_folders_come_under_a_total_each_by_its_path() {
        let dir = sample("several");
        let (big, small) = (dir.join("big"), dir.join("small"));
        let tree = measure_tree(vec![small.clone(), big.clone()], 2, 0.0);
        assert_eq!(tree.name, "(total)");
        assert_eq!(tree.size, 3510);
        let big_name = big.to_string_lossy();
        let small_name = small.to_string_lossy();
        assert_eq!(sizes_of(&tree), [(big_name.as_ref(), 3500), (small_name.as_ref(), 10)]);
        // The total is one of the two levels (Clean up asks for just each path's size), as pdu had it.
        assert!(tree.children.iter().all(|c| c.children.is_empty()));
        let tree = measure_tree(vec![small, big], 3, 0.0);
        assert_eq!(sizes_of(&tree.children[0]), [("a.bin", 3000), ("deep", 500)]);
        assert!(tree.children[0].children[1].children.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_the_worker_prints_reads_back() {
        let dir = sample("json");
        let tree = measure_tree(vec![dir.clone()], 2, 0.0);
        let json = serde_json::to_string(&Output { unit: "bytes".into(), tree }).unwrap();
        assert!(json.starts_with(r#"{"unit":"bytes","tree":{"size":3610,"name":"#), "{json}");
        let (total, sizes) = parse(&json).unwrap();
        assert_eq!(total, 3610);
        assert_eq!(sizes, [("big".to_string(), 3500), ("top.txt".to_string(), 100), ("small".to_string(), 10)]);
        // A path that isn't there counts as nothing, as pdu's --silent-errors had it.
        let gone = measure_tree(vec![dir.join("gone")], 2, 0.0);
        assert_eq!((gone.size, gone.children.len()), (0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_workers_command_line_is_checked() {
        let args = |list: &[&str]| worker_args(list.iter().map(std::ffi::OsString::from));
        assert_eq!(args(&["2", "0", "--", r"C:\a", r"C:\b c"]), Some((2, 0.0, vec![PathBuf::from(r"C:\a"), PathBuf::from(r"C:\b c")])));
        assert_eq!(args(&["4", "0.0001", "--", r"C:\a"]).map(|(d, r, _)| (d, r)), Some((4, 0.0001)));
        assert_eq!(args(&["0", "0", "--", r"C:\a"]), None, "a depth of 0");
        assert_eq!(args(&["2", "1", "--", r"C:\a"]), None, "a ratio of 1");
        assert_eq!(args(&["2", "0", r"C:\a"]), None, "no --");
        assert_eq!(args(&["2", "0", "--"]), None, "no path");
        assert_eq!(args(&["two", "0", "--", r"C:\a"]), None);
        // What Glint asks for: the depth and ratio it means, then the paths after "--".
        let command = worker_command(2, 0.0, &[r"C:\a b"]).unwrap();
        let given: Vec<String> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(given, [WORKER_ARG, "2", "0", "--", r"C:\a b"]);
    }
}
