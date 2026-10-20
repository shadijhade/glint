//! Disk usage: a map of what fills a folder, its biggest files and folders a few levels down, like
//! WizTree or WinDirStat draw. From the instant search index when it has the folder (one request,
//! milliseconds), otherwise measured with the bundled dust.

use crate::fsx::{blocking, ms_since, stat};
use crate::sizes::{self, Node};
use crate::AppState;
use shadui_index::proto::{UsageNode, NO_PARENT};
use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::Instant;
use tauri::ipc::Response;
use tauri::{State, Window};

const ENGINE_INDEX: u32 = 0;
const ENGINE_DUST: u32 = 1;
/// A folder in an archive, mapped from the archive's own list of entries.
const ENGINE_ZIP: u32 = 2;
const ENGINE_7Z: u32 = 3;
/// Flags: smaller items were left out; the index service runs but is too old to map.
const LIMITED: u32 = 1;
const SERVICE_TOO_OLD: u32 = 2;

/// What the index said to a map request.
enum Indexed {
    /// The map, and whether it left smaller items out.
    Map(Vec<UsageNode>, bool),
    /// The service runs, but is a version from before maps: it drops a request it doesn't know.
    TooOld,
    /// It doesn't run, isn't allowed, or doesn't have the folder.
    No,
}

/// Asks the index for its map of `path`. Junctions and links are resolved first: the index knows each
/// folder by its real path.
#[cfg(windows)]
fn index_usage(path: &str, depth: u32, max_items: u32) -> Indexed {
    use shadui_index::client::{usage, Error};
    use shadui_index::proto::UsageRequest;
    let Ok(real) = std::fs::canonicalize(path) else { return Indexed::No };
    match usage(&UsageRequest { path: real.to_string_lossy().into_owned(), depth, max_items }) {
        Ok((nodes, summary)) => Indexed::Map(nodes, summary.limited),
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => Indexed::TooOld,
        Err(_) => Indexed::No,
    }
}

#[cfg(not(windows))]
fn index_usage(_: &str, _: u32, _: u32) -> Indexed {
    Indexed::No
}

/// Maps `path` down to `depth` levels with at most `max_items` items (the biggest), as one binary
/// buffer (see `encode_map`). Each tab maps in its own `lane`, so one pane's dust run doesn't stop the
/// other's. Rejects with "cancelled" when a newer map in the lane, or Stop, ended its dust run.
#[tauri::command]
pub async fn disk_usage(window: Window, state: State<'_, AppState>, path: String, depth: u32, max_items: u32, lane: String) -> Result<Response, String> {
    let started = Instant::now();
    let (depth, max_items) = (depth.clamp(1, 32), max_items.clamp(1, 500_000));
    let slot = state.usage.slot(&lane);
    let asked = path.clone();
    if let Some(found) = blocking(move || Ok(crate::archive::usage(&asked, depth, max_items))).await? {
        slot.kill();
        let (nodes, limited, format) = found?;
        let engine = if format == crate::archive::Format::Zip { ENGINE_ZIP } else { ENGINE_7Z };
        return Ok(Response::new(encode_map(&path, &nodes, ms_since(started), engine, if limited { LIMITED } else { 0 })));
    }
    let asked = path.clone();
    let too_old = match blocking(move || Ok(index_usage(&asked, depth, max_items))).await? {
        Indexed::Map(nodes, limited) => {
            // A dust run still going in this tab is for a map no longer wanted.
            slot.kill();
            return Ok(Response::new(encode_map(&path, &nodes, ms_since(started), ENGINE_INDEX, if limited { LIMITED } else { 0 })));
        }
        Indexed::TooOld => SERVICE_TOO_OLD,
        Indexed::No => 0,
    };
    let mut command = crate::proc::tool(&window, "dust")?;
    // Files and folders, `depth` levels, the biggest `max_items`, apparent sizes in exact bytes.
    command
        .args(["-j", "-s", "-o", "b", "-P", "-d", &depth.to_string(), "-n", &max_items.to_string(), "--"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|e| format!("Failed to start dust: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("Failed to open dust's output")?;
    let pid = slot.replace(child);
    blocking(move || {
        let mut json = String::new();
        let _ = stdout.read_to_string(&mut json);
        if !slot.finish(pid) {
            return Err("cancelled".into());
        }
        let mut nodes = dust_nodes(sizes::parse_tree(&json)?);
        fill_in(&mut nodes);
        let mut nodes: Vec<UsageNode> = nodes.into_iter().map(|(node, _)| node).collect();
        // dust stops at `max_items`; short of it, nothing was left out.
        let limited = nodes.len() > max_items as usize;
        if !limited {
            nodes = without_folder_entries(nodes, depth);
        }
        Ok(Response::new(encode_map(&path, &nodes, ms_since(started), ENGINE_DUST, too_old | if limited { LIMITED } else { 0 })))
    })
    .await
}

/// Stops the tab's dust run (Stop, the map closed, or the tab closed).
#[tauri::command]
pub fn cancel_disk_usage(state: State<AppState>, lane: String) {
    state.usage.close(&lane);
}

/// dust's tree in the index's order: the folder first, then breadth first, each folder's items
/// together and biggest first, empty ones left out; each with its full path. Only items with
/// something below them are known to be folders yet (see `fill_in`).
fn dust_nodes(root: Node) -> Vec<(UsageNode, String)> {
    let mut nodes = Vec::new();
    let mut queue = VecDeque::from([(root, NO_PARENT)]);
    while let Some((mut node, parent)) = queue.pop_front() {
        let mut children = std::mem::take(&mut node.children);
        children.retain(|c| sizes::bytes(&c.size) > 0);
        children.sort_by(|a, b| sizes::bytes(&b.size).cmp(&sizes::bytes(&a.size)).then_with(|| a.name.cmp(&b.name)));
        let name = if parent == NO_PARENT { String::new() } else { Path::new(&node.name).file_name().map_or_else(|| node.name.clone(), |n| n.to_string_lossy().into_owned()) };
        let dir = parent == NO_PARENT || !children.is_empty();
        let at = nodes.len() as u32;
        nodes.push((UsageNode { parent, name, dir, size: sizes::bytes(&node.size), modified: 0, attrs: 0 }, node.name));
        queue.extend(children.into_iter().map(|c| (c, at)));
    }
    nodes
}

/// dust counts each folder's own entry (4 KB on NTFS for any but the smallest) in its size. With
/// nothing left out, a folder above the depth limit becomes the sum of what's in it, as Explorer's
/// Properties and the index count it; otherwise every folder would show a sliver of "smaller items".
/// Folders that then hold nothing go.
fn without_folder_entries(mut nodes: Vec<UsageNode>, depth: u32) -> Vec<UsageNode> {
    let mut level = vec![0u32; nodes.len()];
    for i in 1..nodes.len() {
        level[i] = level[nodes[i].parent as usize] + 1;
    }
    let mut inside = vec![0u64; nodes.len()];
    // Items come after their folder, so going backwards sums every folder after what's in it.
    for i in (0..nodes.len()).rev() {
        if nodes[i].dir && level[i] < depth {
            nodes[i].size = inside[i];
        }
        if i > 0 {
            inside[nodes[i].parent as usize] += nodes[i].size;
        }
    }
    // Folders that held only their own entries have no room on the map.
    let mut place = vec![NO_PARENT; nodes.len()];
    let mut kept = Vec::with_capacity(nodes.len());
    for (i, node) in nodes.into_iter().enumerate() {
        let parent = if i == 0 { NO_PARENT } else { place[node.parent as usize] };
        if i == 0 || (parent != NO_PARENT && node.size > 0) {
            place[i] = kept.len() as u32;
            kept.push(UsageNode { parent, ..node });
        }
    }
    kept
}

/// Reads each item's attributes and date (without opening it, so online-only files stay online),
/// which also tells the folders dust shows without contents (at the depth limit, or with only small
/// items) from files. Several threads, since a map can have thousands of items.
fn fill_in(nodes: &mut [(UsageNode, String)]) {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let chunk = nodes.len().div_ceil(threads).max(256);
    std::thread::scope(|scope| {
        for part in nodes.chunks_mut(chunk) {
            scope.spawn(move || {
                for (node, path) in part {
                    if let Some(found) = stat(path) {
                        node.dir |= found.dir;
                        node.attrs = found.attrs;
                        node.modified = found.modified;
                    }
                }
            });
        }
    });
}

/// A map as bytes for the UI, laid out like a listing (`fsx::encode_listing`) so each array can be
/// viewed in place as a typed array. All little-endian:
///   0: u32 count · 4: u32 path length (UTF-8 bytes) · 8: f64 milliseconds spent
///   16: u32 engine (0 index, 1 dust, 2 a zip's list of entries, 3 a 7z's) · 20: u32 flags (1: smaller items left out; 2: the index service
///   runs but is too old to map)
///   24: f64 size[count] · then f64 modified[count] · u32 attrs[count] (the directory bit set on
///   folders) · u32 parent[count] (u32::MAX for the folder mapped, always first) · u32 name
///   length[count] (UTF-16 units) · the path (UTF-8) · every name back to back (UTF-8).
pub fn encode_map(path: &str, nodes: &[UsageNode], elapsed: f64, engine: u32, flags: u32) -> Vec<u8> {
    let names: usize = nodes.iter().map(|n| n.name.len()).sum();
    let mut out = Vec::with_capacity(24 + 28 * nodes.len() + path.len() + names);
    out.extend((nodes.len() as u32).to_le_bytes());
    out.extend((path.len() as u32).to_le_bytes());
    out.extend(elapsed.to_le_bytes());
    out.extend(engine.to_le_bytes());
    out.extend(flags.to_le_bytes());
    for n in nodes {
        out.extend((n.size as f64).to_le_bytes());
    }
    for n in nodes {
        out.extend((n.modified as f64).to_le_bytes());
    }
    for n in nodes {
        out.extend((n.attrs | if n.dir { 0x10 } else { 0 }).to_le_bytes());
    }
    for n in nodes {
        out.extend(n.parent.to_le_bytes());
    }
    for n in nodes {
        out.extend((n.name.encode_utf16().count() as u32).to_le_bytes());
    }
    out.extend(path.as_bytes());
    for n in nodes {
        out.extend(n.name.as_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_maps_for_the_ui() {
        let nodes = [
            UsageNode { parent: NO_PARENT, name: String::new(), dir: true, size: 3 << 40, modified: 0, attrs: 0x16 },
            UsageNode { parent: 0, name: "é😀.mkv".into(), dir: false, size: 1 << 40, modified: 1_700_000_000_000, attrs: 0x20 },
        ];
        let bytes = encode_map(r"D:\", &nodes, 2.5, ENGINE_DUST, LIMITED | SERVICE_TOO_OLD);
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let f64_at = |at: usize| f64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        assert_eq!((u32_at(0), u32_at(4), f64_at(8), u32_at(16), u32_at(20)), (2, 3, 2.5, ENGINE_DUST, LIMITED | SERVICE_TOO_OLD));
        assert_eq!((f64_at(24), f64_at(32)), ((3u64 << 40) as f64, (1u64 << 40) as f64));
        assert_eq!((f64_at(40), f64_at(48)), (0.0, 1_700_000_000_000.0));
        assert_eq!((u32_at(56), u32_at(60)), (0x16, 0x20));
        assert_eq!((u32_at(64), u32_at(68)), (NO_PARENT, 0));
        // "é😀.mkv" is 7 UTF-16 units: é, a surrogate pair, then ".mkv".
        assert_eq!((u32_at(72), u32_at(76)), (0, 7));
        assert_eq!(std::str::from_utf8(&bytes[80..]).unwrap(), r"D:\é😀.mkv");
    }

    #[test]
    fn orders_dust_trees_like_the_index() {
        let json = r#"{"size":"1000B","name":"C:\\x","children":[
            {"size":"100B","name":"C:\\x\\small","children":[{"size":"100B","name":"C:\\x\\small\\a.txt","children":[]}]},
            {"size":"0B","name":"C:\\x\\empty","children":[]},
            {"size":"800B","name":"C:\\x\\big","children":[{"size":"500B","name":"C:\\x\\big\\deep","children":[]},{"size":"300B","name":"C:\\x\\big\\b.bin","children":[]}]},
            {"size":"100B","name":"C:\\x\\c.log","children":[]}]}"#;
        let nodes = dust_nodes(sizes::parse_tree(json).unwrap());
        let shape: Vec<_> = nodes.iter().map(|(n, path)| (n.parent, n.name.as_str(), n.dir, n.size, path.as_str())).collect();
        assert_eq!(
            shape,
            [
                (NO_PARENT, "", true, 1000, r"C:\x"),
                (0, "big", true, 800, r"C:\x\big"),
                (0, "c.log", false, 100, r"C:\x\c.log"),
                (0, "small", true, 100, r"C:\x\small"),
                (1, "deep", false, 500, r"C:\x\big\deep"),
                (1, "b.bin", false, 300, r"C:\x\big\b.bin"),
                (3, "a.txt", false, 100, r"C:\x\small\a.txt"),
            ]
        );
    }

    #[test]
    fn leaves_out_dusts_folder_entries() {
        let node = |parent, name: &str, dir, size| UsageNode { parent, name: name.into(), dir, size, modified: 0, attrs: 0 };
        // Two levels of C:\x as dust counts them: x's own 4 KB entry, sub (its entry and a.txt), deep (its
        // entry and inner, which is at the depth limit: its entry and 4 bytes below), empty (only its
        // entry) and b.bin.
        let nodes = vec![
            node(NO_PARENT, "", true, 20788),
            node(0, "sub", true, 4196),
            node(0, "deep", true, 8196),
            node(0, "empty", true, 4096),
            node(0, "b.bin", false, 200),
            node(1, "a.txt", false, 100),
            node(2, "inner", true, 4100),
        ];
        let fixed = without_folder_entries(nodes, 2);
        let shape: Vec<_> = fixed.iter().map(|n| (n.parent, n.name.as_str(), n.size)).collect();
        // Folders above the limit are what's in them; inner keeps dust's count, as nothing below it is known.
        assert_eq!(shape, [(NO_PARENT, "", 4400), (0, "sub", 100), (0, "deep", 4100), (0, "b.bin", 200), (1, "a.txt", 100), (2, "inner", 4100)]);
    }

    #[test]
    fn reads_what_dust_leaves_out() {
        let dir = std::env::temp_dir().join(format!("shadui-usage-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let node = |parent, dir| UsageNode { parent, name: String::new(), dir, size: 5, modified: 0, attrs: 0 };
        // dust shows a folder at its depth limit with no children: only reading it says it's a folder.
        let mut nodes = vec![(node(NO_PARENT, true), path("")), (node(0, false), path("sub")), (node(0, false), path("a.txt")), (node(0, false), path("gone"))];
        fill_in(&mut nodes);
        assert_eq!(nodes.iter().map(|(n, _)| n.dir).collect::<Vec<_>>(), [true, true, false, false]);
        assert!(nodes[2].0.modified > 1_600_000_000_000);
        assert_eq!(nodes[3].0.modified, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
