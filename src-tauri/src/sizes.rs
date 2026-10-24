//! Folder sizes, which Explorer never shows: from the instant search index when it has the folder
//! (instant, and kept current), otherwise measured with the bundled pdu (parallel-disk-usage).

use crate::fsx::{blocking, ms_since};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::process::Stdio;
use std::time::Instant;
use tauri::State;

#[derive(Serialize)]
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

/// pdu --json-output prints {"unit":"bytes","tree":{"name":"C:\\dir","size":123,"children":[…]}}: the
/// tree's top named as it was asked for (several at once: "(total)", with each under it by its path),
/// what's in a folder by its own name.
#[derive(Deserialize)]
pub(crate) struct Node {
    pub size: u64,
    pub name: String,
    #[serde(default)]
    pub children: Vec<Node>,
}

#[derive(Deserialize)]
struct Output {
    tree: Node,
}

/// pdu's whole tree.
pub(crate) fn parse_tree(json: &str) -> Result<Node, String> {
    serde_json::from_str::<Output>(json.trim()).map(|o| o.tree).map_err(|e| format!("Unexpected output from pdu: {e}"))
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

/// Sizes of every subfolder of `path`. With `index_only`, fails instead of running pdu when the
/// index can't answer (for sizing every folder as it opens, which only makes sense when it's instant).
/// Each tab asks in its own `lane`, so sizing a folder in one pane doesn't stop the other's.
#[tauri::command]
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
    let mut command = crate::proc::tool("pdu")?;
    // One level deep (files come too: the list takes folders' sizes only), apparent sizes (what Explorer's
    // Properties calls "Size") in bytes, nothing left out.
    command
        .args(["--json-output", "--quantity", "apparent-size", "--max-depth", "2", "--min-ratio", "0", "--silent-errors", "--"])
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
        let (total, sizes) = parse(&json)?;
        Ok(FolderSizes { path, total, sizes, elapsed: ms_since(started), engine: "pdu" })
    })
    .await
}

/// Stops the tab's sizing (Stop, or the tab closed).
#[tauri::command]
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
}
