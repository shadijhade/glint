//! Disk usage: a map of what fills a folder, its biggest files and folders a few levels down, like
//! WizTree or WinDirStat draw. From the instant search index when it has the folder (one request,
//! milliseconds), otherwise measured with the bundled pdu (parallel-disk-usage).

use crate::fsx::{blocking, ms_since, stat_sized};
use crate::sizes::{self, Node};
use crate::AppState;
use shadui_index::proto::{GrowthSummary, UsageNode, NEW, NO_PARENT, WRITTEN};
use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::Instant;
use tauri::ipc::Response;
use tauri::State;

const ENGINE_INDEX: u32 = 0;
const ENGINE_PDU: u32 = 1;
/// A folder in an archive, mapped from the archive's own list of entries: this plus the archive's
/// format code (2 a zip, 3 a 7z…).
const ENGINE_ARCHIVE: u32 = 2;
/// Flags: smaller items were left out; the index service runs but is too old to map; what grew comes
/// along (see `encode_map`); what grew was asked for, but the index service is too old to say.
const LIMITED: u32 = 1;
const SERVICE_TOO_OLD: u32 = 2;
const GROWTH: u32 = 4;
const GROWTH_TOO_OLD: u32 = 8;

/// What the index said to a map request.
enum Indexed {
    /// The map, whether it left smaller items out, and what its items held at a mark (when asked
    /// and known).
    Map(Vec<UsageNode>, bool, Option<(Vec<u64>, GrowthSummary)>),
    /// The service runs, but is a version from before maps: it drops a request it doesn't know.
    TooOld,
    /// It doesn't run, isn't allowed, or doesn't have the folder.
    No,
}

/// Asks the index for its map of `path`. Junctions and links are resolved first: the index knows each
/// folder by its real path.
#[cfg(windows)]
fn index_usage(path: &str, depth: u32, max_items: u32, since: Option<i64>) -> Indexed {
    use shadui_index::client::{usage, Error};
    use shadui_index::proto::UsageRequest;
    let Ok(real) = std::fs::canonicalize(path) else { return Indexed::No };
    match usage(&UsageRequest { path: real.to_string_lossy().into_owned(), depth, max_items, since }) {
        Ok(map) => Indexed::Map(map.nodes, map.summary.limited, map.previous.zip(map.summary.growth)),
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => Indexed::TooOld,
        Err(_) => Indexed::No,
    }
}

#[cfg(not(windows))]
fn index_usage(_: &str, _: u32, _: u32, _: Option<i64>) -> Indexed {
    Indexed::No
}

/// Maps `path` down to `depth` levels with at most `max_items` items (the biggest), as one binary
/// buffer (see `encode_map`); with `since` (Unix ms), what each item held at the index's daily mark
/// closest to then comes along. Each tab maps in its own `lane`, so one pane's pdu run doesn't stop
/// the other's. Rejects with "cancelled" when a newer map in the lane, or Stop, ended its pdu run.
#[tauri::command]
pub async fn disk_usage(state: State<'_, AppState>, path: String, depth: u32, max_items: u32, lane: String, since: Option<f64>) -> Result<Response, String> {
    let started = Instant::now();
    let (depth, max_items) = (depth.clamp(1, 32), max_items.clamp(1, 500_000));
    let since = since.map(|s| s as i64);
    let slot = state.usage.slot(&lane);
    let asked = path.clone();
    if let Some(found) = blocking(move || Ok(crate::archive::usage(&asked, depth, max_items))).await? {
        slot.kill();
        let (nodes, limited, format) = found?;
        let engine = ENGINE_ARCHIVE + format.code() as u32;
        return Ok(Response::new(encode_map(&path, &nodes, ms_since(started), engine, if limited { LIMITED } else { 0 }, None)));
    }
    let asked = path.clone();
    let too_old = match blocking(move || Ok(index_usage(&asked, depth, max_items, since))).await? {
        Indexed::Map(nodes, limited, growth) => {
            // A pdu run still going in this tab is for a map no longer wanted.
            slot.kill();
            let mut flags = if limited { LIMITED } else { 0 };
            if since.is_some() && growth.is_none() {
                flags |= GROWTH_TOO_OLD;
            }
            let growth = growth.as_ref().map(|(previous, summary)| (previous.as_slice(), summary));
            return Ok(Response::new(encode_map(&path, &nodes, ms_since(started), ENGINE_INDEX, flags, growth)));
        }
        Indexed::TooOld => SERVICE_TOO_OLD,
        Indexed::No => 0,
    };
    let mut command = crate::proc::tool("pdu")?;
    // Files and folders, `depth` levels below, apparent sizes in bytes; what's under a fifth of
    // `max_items`' share of the whole is left out by pdu, the rest cut to the biggest `max_items` here.
    let ratio = format!("{:.12}", 1.0 / (f64::from(max_items) * 5.0));
    command
        .args(["--json-output", "--quantity", "apparent-size", "--max-depth", &(depth + 1).to_string(), "--min-ratio", &ratio, "--silent-errors", "--"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|e| format!("Failed to start pdu: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("Failed to open pdu's output")?;
    let pid = slot.replace(child);
    blocking(move || {
        let mut json = String::new();
        let _ = stdout.read_to_string(&mut json);
        if slot.finish(pid).is_none() {
            return Err("cancelled".into());
        }
        let mut nodes = biggest(pdu_nodes(sizes::parse_tree(&json)?, &path), max_items as usize);
        let own = fill_in(&mut nodes);
        let (nodes, limited) = without_folder_entries(nodes.into_iter().map(|(node, _)| node).collect(), &own, depth);
        Ok(Response::new(encode_map(&path, &nodes, ms_since(started), ENGINE_PDU, too_old | if limited { LIMITED } else { 0 }, None)))
    })
    .await
}

/// Stops the tab's pdu run (Stop, the map closed, or the tab closed).
#[tauri::command]
pub fn cancel_disk_usage(state: State<AppState>, lane: String) {
    state.usage.close(&lane);
}

/// pdu's tree in the index's order: the folder first, then breadth first, each folder's items
/// together and biggest first, empty ones left out; each with its full path (pdu names what's in a
/// folder by its own name, the top `top`). Only items with something below them are known to be
/// folders yet (see `fill_in`).
fn pdu_nodes(root: Node, top: &str) -> Vec<(UsageNode, String)> {
    let mut nodes = Vec::new();
    let mut queue = VecDeque::from([(root, NO_PARENT, top.to_string())]);
    while let Some((mut node, parent, path)) = queue.pop_front() {
        let mut children = std::mem::take(&mut node.children);
        children.retain(|c| c.size > 0);
        children.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
        let name = if parent == NO_PARENT { String::new() } else { node.name.clone() };
        let dir = parent == NO_PARENT || !children.is_empty();
        let at = nodes.len() as u32;
        queue.extend(children.into_iter().map(|c| {
            let below = Path::new(&path).join(&c.name).to_string_lossy().into_owned();
            (c, at, below)
        }));
        nodes.push((UsageNode { parent, name, dir, size: node.size, modified: 0, attrs: 0 }, path));
    }
    nodes
}

/// The biggest `most` of `nodes` (a folder is at least as big as what's in it, so each one kept has
/// its folder kept too), in their order. What's cut goes into its folder's smaller items.
fn biggest(nodes: Vec<(UsageNode, String)>, most: usize) -> Vec<(UsageNode, String)> {
    if nodes.len() <= most {
        return nodes;
    }
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by(|&a, &b| nodes[b].0.size.cmp(&nodes[a].0.size).then(a.cmp(&b)));
    let mut wanted = vec![false; nodes.len()];
    for &i in order.iter().take(most) {
        wanted[i] = true;
    }
    let mut place = vec![NO_PARENT; nodes.len()];
    let mut kept = Vec::with_capacity(most);
    for (i, (node, path)) in nodes.into_iter().enumerate() {
        let parent = if i == 0 { NO_PARENT } else { place[node.parent as usize] };
        if i == 0 || (wanted[i] && parent != NO_PARENT) {
            place[i] = kept.len() as u32;
            kept.push((UsageNode { parent, ..node }, path));
        }
    }
    kept
}

/// pdu counts a folder's own entry in its size (`own`: its index of names, none for the smallest
/// folders on NTFS, 4 KB and up for the rest, 18 MB for WinSxS). Taken out, a folder above the depth
/// limit is what's in it plus what was left out of it (by pdu's cut or `biggest`), its "smaller
/// items", as Explorer's Properties and the index count it; left in, folders would show smaller items
/// that aren't there, most of the map for a folder of tiny files. Folders that then hold nothing go.
/// Also says whether anything was left out.
fn without_folder_entries(mut nodes: Vec<UsageNode>, own: &[u64], depth: u32) -> (Vec<UsageNode>, bool) {
    let mut level = vec![0u32; nodes.len()];
    for i in 1..nodes.len() {
        level[i] = level[nodes[i].parent as usize] + 1;
    }
    // What's listed in each folder, as pdu counted it and as it's drawn.
    let mut counted = vec![0u64; nodes.len()];
    let mut inside = vec![0u64; nodes.len()];
    let mut left_out = false;
    // Items come after their folder, so going backwards sees every folder after what's in it.
    for i in (0..nodes.len()).rev() {
        let size = nodes[i].size;
        if nodes[i].dir && level[i] < depth {
            let smaller = size.saturating_sub(own[i]).saturating_sub(counted[i]);
            left_out |= smaller > 0;
            nodes[i].size = inside[i] + smaller;
        }
        if i > 0 {
            counted[nodes[i].parent as usize] += size;
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
    (kept, left_out)
}

/// Reads each item's attributes and date (without opening it, so online-only files stay online),
/// which also tells the folders pdu shows without contents (at the depth limit, or with only small
/// items) from files; and gives each folder's own entry, which pdu counts in its size. Several
/// threads, since a map can have thousands of items.
fn fill_in(nodes: &mut [(UsageNode, String)]) -> Vec<u64> {
    let mut own = vec![0u64; nodes.len()];
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let chunk = nodes.len().div_ceil(threads).max(256);
    std::thread::scope(|scope| {
        for (part, own) in nodes.chunks_mut(chunk).zip(own.chunks_mut(chunk)) {
            scope.spawn(move || {
                for ((node, path), own) in part.iter_mut().zip(own) {
                    if let Some((found, size)) = stat_sized(path) {
                        node.dir |= found.dir;
                        node.attrs = found.attrs;
                        node.modified = found.modified;
                        if found.dir {
                            *own = size;
                        }
                    }
                }
            });
        }
    });
    own
}

/// A map as bytes for the UI, laid out like a listing (`fsx::encode_listing`) so each array can be
/// viewed in place as a typed array. All little-endian:
///   0: u32 count · 4: u32 path length (UTF-8 bytes) · 8: f64 milliseconds spent
///   16: u32 engine (0 index, 1 pdu, 2 plus an archive's format code for its list of entries: 2 a zip's, 3 a 7z's…) · 20: u32 flags (1: smaller items left out; 2: the index service
///   runs but is too old to map; 4: what grew comes along; 8: what grew was asked for, but the index
///   service is too old to say)
///   24, with flag 4: f64 since (the mark's Unix ms) · f64 oldest mark · f64 the drive's total then
///   · f64 now
///   then f64 size[count] · f64 modified[count] · with flag 4, f64 previous[count] (each item's size
///   at the mark; -1: it wasn't there; -2: a file written since) · u32 attrs[count] (the directory
///   bit set on folders) · u32 parent[count] (u32::MAX for the folder mapped, always first) · u32
///   name length[count] (UTF-16 units) · the path (UTF-8) · every name back to back (UTF-8).
pub fn encode_map(path: &str, nodes: &[UsageNode], elapsed: f64, engine: u32, flags: u32, growth: Option<(&[u64], &GrowthSummary)>) -> Vec<u8> {
    let names: usize = nodes.iter().map(|n| n.name.len()).sum();
    let mut out = Vec::with_capacity(56 + 36 * nodes.len() + path.len() + names);
    out.extend((nodes.len() as u32).to_le_bytes());
    out.extend((path.len() as u32).to_le_bytes());
    out.extend(elapsed.to_le_bytes());
    out.extend(engine.to_le_bytes());
    out.extend((flags | if growth.is_some() { GROWTH } else { 0 }).to_le_bytes());
    if let Some((_, summary)) = growth {
        for value in [summary.since as f64, summary.oldest as f64, summary.drive_then as f64, summary.drive_now as f64] {
            out.extend(value.to_le_bytes());
        }
    }
    for n in nodes {
        out.extend((n.size as f64).to_le_bytes());
    }
    for n in nodes {
        out.extend((n.modified as f64).to_le_bytes());
    }
    if let Some((previous, _)) = growth {
        for &then in previous {
            let then = match then {
                NEW => -1.0,
                WRITTEN => -2.0,
                then => then as f64,
            };
            out.extend(then.to_le_bytes());
        }
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
        let bytes = encode_map(r"D:\", &nodes, 2.5, ENGINE_PDU, LIMITED | SERVICE_TOO_OLD, None);
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let f64_at = |at: usize| f64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        assert_eq!((u32_at(0), u32_at(4), f64_at(8), u32_at(16), u32_at(20)), (2, 3, 2.5, ENGINE_PDU, LIMITED | SERVICE_TOO_OLD));
        assert_eq!((f64_at(24), f64_at(32)), ((3u64 << 40) as f64, (1u64 << 40) as f64));
        assert_eq!((f64_at(40), f64_at(48)), (0.0, 1_700_000_000_000.0));
        assert_eq!((u32_at(56), u32_at(60)), (0x16, 0x20));
        assert_eq!((u32_at(64), u32_at(68)), (NO_PARENT, 0));
        // "é😀.mkv" is 7 UTF-16 units: é, a surrogate pair, then ".mkv".
        assert_eq!((u32_at(72), u32_at(76)), (0, 7));
        assert_eq!(std::str::from_utf8(&bytes[80..]).unwrap(), r"D:\é😀.mkv");

        // What grew: the mark's block after the header, and each item's earlier size after the dates.
        let summary = GrowthSummary { since: 1_000, oldest: 500, drive_then: 7, drive_now: 9 };
        let bytes = encode_map(r"D:\", &nodes, 2.5, ENGINE_INDEX, 0, Some((&[2 << 40, WRITTEN], &summary)));
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let f64_at = |at: usize| f64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        assert_eq!(u32_at(20), GROWTH);
        assert_eq!((f64_at(24), f64_at(32), f64_at(40), f64_at(48)), (1_000.0, 500.0, 7.0, 9.0));
        assert_eq!((f64_at(56), f64_at(72), f64_at(88), f64_at(96)), ((3u64 << 40) as f64, 0.0, (2u64 << 40) as f64, -2.0));
        assert_eq!((u32_at(104), u32_at(112)), (0x16, NO_PARENT));
        assert_eq!(std::str::from_utf8(&bytes[128..]).unwrap(), r"D:\é😀.mkv");
    }

    #[test]
    fn orders_pdu_trees_like_the_index() {
        let json = r#"{"schema-version":"2026-04-02","pdu":"0.24.0","unit":"bytes","tree":{"name":"C:\\x","size":1000,"children":[
            {"name":"small","size":100,"children":[{"name":"a.txt","size":100,"children":[]}]},
            {"name":"empty","size":0,"children":[]},
            {"name":"big","size":800,"children":[{"name":"deep","size":500,"children":[]},{"name":"b.bin","size":300,"children":[]}]},
            {"name":"c.log","size":100,"children":[]}]}}"#;
        let nodes = pdu_nodes(sizes::parse_tree(json).unwrap(), r"C:\x");
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
        // The biggest 4, each with its folder; what's cut is the top folder's smaller items.
        let kept = biggest(nodes.clone(), 4);
        let shape: Vec<_> = kept.iter().map(|(n, _)| (n.parent, n.name.as_str())).collect();
        assert_eq!(shape, [(NO_PARENT, ""), (0, "big"), (1, "deep"), (1, "b.bin")]);
        let (kept, left_out) = without_folder_entries(kept.into_iter().map(|(n, _)| n).collect(), &[0; 4], 2);
        assert_eq!((kept[0].size, kept[1].size, left_out), (1000, 800, true));
        let all = biggest(nodes.clone(), 100);
        assert_eq!(all.len(), nodes.len());
        assert!(!without_folder_entries(all.into_iter().map(|(n, _)| n).collect(), &[0; 7], 2).1);
    }

    #[test]
    fn leaves_out_folder_entries() {
        let node = |parent, name: &str, dir, size| UsageNode { parent, name: name.into(), dir, size, modified: 0, attrs: 0 };
        // Two levels of C:\x as pdu counts them, with each folder's own 4 KB entry: sub (its entry and
        // a.txt), deep (its entry and inner, which is at the depth limit: its entry and 4 bytes below),
        // empty (only its entry) and b.bin.
        let nodes = vec![
            node(NO_PARENT, "", true, 20784),
            node(0, "sub", true, 4196),
            node(0, "deep", true, 8196),
            node(0, "empty", true, 4096),
            node(0, "b.bin", false, 200),
            node(1, "a.txt", false, 100),
            node(2, "inner", true, 4100),
        ];
        let own = [4096, 4096, 4096, 4096, 0, 0, 4096];
        let (fixed, left_out) = without_folder_entries(nodes.clone(), &own, 2);
        let shape: Vec<_> = fixed.iter().map(|n| (n.parent, n.name.as_str(), n.size)).collect();
        // Folders above the limit are what's in them; inner keeps its count, as nothing below it is known.
        let expected = vec![(NO_PARENT, "", 4400), (0, "sub", 100), (0, "deep", 4100), (0, "b.bin", 200), (1, "a.txt", 100), (2, "inner", 4100)];
        assert_eq!((shape, left_out), (expected, false));
        // 50 bytes in sub that pdu left out (under its cut) are sub's smaller items, and count above it.
        let mut short = nodes;
        short[0].size += 50;
        short[1].size += 50;
        let (fixed, left_out) = without_folder_entries(short, &own, 2);
        assert_eq!((fixed[0].size, fixed[1].size, fixed[4].name.as_str(), fixed[4].size, left_out), (4450, 150, "a.txt", 100, true));
    }

    #[test]
    fn reads_what_pdu_leaves_out() {
        let dir = std::env::temp_dir().join(format!("shadui-usage-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let node = |parent, dir| UsageNode { parent, name: String::new(), dir, size: 5, modified: 0, attrs: 0 };
        // pdu shows a folder at its depth limit with no children: only reading it says it's a folder.
        let mut nodes = vec![(node(NO_PARENT, true), path("")), (node(0, false), path("sub")), (node(0, false), path("a.txt")), (node(0, false), path("gone"))];
        let own = fill_in(&mut nodes);
        assert_eq!(nodes.iter().map(|(n, _)| n.dir).collect::<Vec<_>>(), [true, true, false, false]);
        assert!(nodes[2].0.modified > 1_600_000_000_000);
        assert_eq!((nodes[3].0.modified, own[2], own[3]), (0, 0, 0));
        // A folder's own entry, as pdu reads it (on NTFS 96 KB for these 300 names, none for a few).
        for i in 0..300 {
            std::fs::write(dir.join("sub").join(format!("a file with a longish name {i}.txt")), b"x").unwrap();
        }
        let own = fill_in(&mut nodes);
        assert_eq!(own[1], std::fs::symlink_metadata(dir.join("sub")).unwrap().len());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
