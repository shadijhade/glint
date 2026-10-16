//! Folder sizes, which Explorer never shows: from the instant search index when it has the folder
//! (instant, and kept current), otherwise measured with the bundled dust.

use crate::fsx::{blocking, ms_since};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;
use tauri::{State, Window};
use tauri_plugin_shell::ShellExt;

#[derive(Serialize)]
pub struct FolderSizes {
    pub path: String,
    /// Everything in the folder, files included, in bytes.
    pub total: u64,
    /// Subfolder name → bytes.
    pub sizes: Vec<(String, u64)>,
    pub elapsed: f64,
    /// "index" or "dust".
    pub engine: &'static str,
}

/// dust -j prints {"size":"123B","name":"C:\\dir","children":[…]} when sizes are in bytes (-o b).
#[derive(Deserialize)]
struct Node {
    size: String,
    name: String,
    #[serde(default)]
    children: Vec<Node>,
}

fn bytes(size: &str) -> u64 {
    size.trim_end_matches('B').parse().unwrap_or(0)
}

fn parse(json: &str) -> Result<(u64, Vec<(String, u64)>), String> {
    let root: Node = serde_json::from_str(json.trim()).map_err(|e| format!("Unexpected output from dust: {e}"))?;
    let sizes = root
        .children
        .iter()
        .map(|c| {
            let name = Path::new(&c.name).file_name().map_or_else(|| c.name.clone(), |n| n.to_string_lossy().into_owned());
            (name, bytes(&c.size))
        })
        .collect();
    Ok((bytes(&root.size), sizes))
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

/// Sizes of every subfolder of `path`. With `index_only`, fails instead of running dust when the
/// index can't answer (for sizing every folder as it opens, which only makes sense when it's instant).
/// Each tab asks in its own `lane`, so sizing a folder in one pane doesn't stop the other's.
#[tauri::command]
pub async fn folder_sizes(window: Window, state: State<'_, AppState>, path: String, index_only: Option<bool>, lane: String) -> Result<FolderSizes, String> {
    let started = Instant::now();
    let slot = state.sizes.slot(&lane);
    let asked = path.clone();
    if let Some((total, sizes)) = blocking(move || Ok(index_sizes(&asked))).await? {
        // A dust run still going in this tab is for a folder no longer wanted.
        slot.kill();
        return Ok(FolderSizes { path, total, sizes, elapsed: ms_since(started), engine: "index" });
    }
    if index_only == Some(true) {
        return Err("not indexed".into());
    }
    let mut command: Command = window.shell().sidecar("dust").map_err(|e| e.to_string())?.into();
    // One level deep, folders only, apparent sizes (what Explorer's Properties calls "Size") in exact bytes.
    command
        .args(["-j", "-d", "1", "-D", "-s", "-o", "b", "-P", "-n", "1000000", "--"])
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
        let (total, sizes) = parse(&json)?;
        Ok(FolderSizes { path, total, sizes, elapsed: ms_since(started), engine: "dust" })
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
    fn parses_dust_json() {
        let json = r#" {"size":"12076482B","name":"C:\\repo","children":[{"size":"5778575B","name":"C:\\repo\\.git","children":[]},{"size":"92090B","name":"C:\\repo\\src","children":[]}]}"#;
        let (total, sizes) = parse(json).unwrap();
        assert_eq!(total, 12_076_482);
        assert_eq!(sizes, [(".git".to_string(), 5_778_575), ("src".to_string(), 92_090)]);
        assert!(parse("").is_err());
    }
}
