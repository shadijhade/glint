//! What AI agents may change through Glint's MCP server (mcp.rs): move, rename, make folders and send to
//! the Recycle Bin. Each asks you first, in a Windows dialog naming exactly what it will do, does it
//! through Windows' own file engine, and is written down, so Settings → AI agents lists what agents did
//! with an Undo for each. And Glint's server, added to Windows' own agent registry (odr.exe) where
//! Windows has one, so Windows' agents find it without setup.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One thing an agent did, as it was done: what it was asked, and where each item went.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub id: u64,
    pub at: i64,
    /// "move", "rename", "folder" or "recycle".
    pub kind: String,
    pub summary: String,
    /// (from, to) for each item; `to` None when nothing came of it.
    pub items: Vec<(String, Option<String>)>,
    #[serde(default)]
    pub undone: bool,
}

fn log_path() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("agent-actions.json"))
}

pub fn actions() -> Vec<Action> {
    log_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(actions: &[Action]) {
    if let Some(path) = log_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_vec_pretty(actions).unwrap_or_default());
    }
}

/// Writes down what an agent did (the last 200 are kept).
pub fn record(kind: &str, summary: String, items: Vec<(String, Option<String>)>) {
    let mut all = actions();
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    all.push(Action { id: at as u64, at, kind: kind.into(), summary, items, undone: false });
    let excess = all.len().saturating_sub(200);
    all.drain(..excess);
    save(&all);
}

/// Asks you, in a Windows dialog in front of everything, whether an agent may do `what`.
pub fn confirm(what: &str) -> bool {
    #[cfg(windows)]
    unsafe {
        use windows::core::PCWSTR;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO};
        let text: Vec<u16> = format!("An AI agent asks Glint to {what}\n\nAllow it?").encode_utf16().chain([0]).collect();
        let title: Vec<u16> = "Glint: an AI agent asks".encode_utf16().chain([0]).collect();
        MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(title.as_ptr()), MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2 | MB_TOPMOST | MB_SETFOREGROUND) == IDYES
    }
    #[cfg(not(windows))]
    {
        let _ = what;
        false
    }
}

/// What agents did, newest first.
#[tauri::command]
pub fn agent_actions() -> Vec<Action> {
    let mut all = actions();
    all.reverse();
    all
}

/// Takes back what an agent did: moved and renamed items go back, a folder it made goes to the Recycle
/// Bin (if it's empty), what it recycled is restored.
#[tauri::command]
pub async fn agent_undo(window: tauri::Window, id: u64) -> Result<(), String> {
    let owner = crate::shell::owner(&window);
    crate::fsx::blocking(move || {
        let mut all = actions();
        let action = all.iter_mut().find(|a| a.id == id).ok_or("That's no longer in the list")?;
        if action.undone {
            return Ok(());
        }
        let back = |from: &str, to: &str| crate::shell::Back {
            path: to.to_string(),
            folder: std::path::Path::new(from).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            name: std::path::Path::new(from).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        };
        let items: Vec<crate::shell::Back> = action.items.iter().filter_map(|(from, to)| to.as_deref().map(|to| back(from, to))).collect();
        match action.kind.as_str() {
            "move" | "rename" => crate::shell::agent_op(owner, crate::shell::FileOp::MoveBack { items }).map(drop)?,
            "recycle" => crate::shell::restore(items).map(drop)?,
            "folder" => {
                let folders: Vec<String> = action.items.iter().map(|(from, _)| from.clone()).filter(|f| std::fs::read_dir(f).is_ok_and(|mut d| d.next().is_none())).collect();
                if !folders.is_empty() {
                    crate::shell::recycle(owner, folders).map(drop)?;
                }
            }
            _ => {}
        }
        action.undone = true;
        save(&all);
        Ok(())
    })
    .await
}

/// Windows' agent registry tool, where Windows has it.
fn odr() -> Option<PathBuf> {
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let exe = system.join(r"System32\odr.exe");
    exe.exists().then_some(exe)
}

/// Whether Windows has an agent registry to add Glint to.
#[tauri::command]
pub fn agents_registry_available() -> bool {
    odr().is_some()
}

/// Adds Glint's MCP server to Windows' agent registry: an MCP bundle manifest naming `Glint.exe mcp`,
/// written to Glint's data folder, given to `odr.exe mcp add`. What odr said.
#[tauri::command]
pub async fn agents_register() -> Result<String, String> {
    crate::fsx::blocking(|| {
        let odr = odr().ok_or("This Windows has no agent registry yet (odr.exe comes with newer builds)")?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = crate::tools::data_dir().ok_or("Glint's data folder isn't available")?.join("mcp");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let manifest = serde_json::json!({
            "manifest_version": "0.3",
            "name": "glint",
            "display_name": "Glint",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Find files by name on every drive in milliseconds, measure folders, list the biggest files, say where a file went, search inside files, and (asking you first) move, rename and recycle them.",
            "author": { "name": "Shadi Saadi", "url": "https://github.com/shadijhade/glint" },
            "homepage": "https://glint.rafd.tech/",
            "license": "MIT",
            "server": {
                "type": "binary",
                "entry_point": exe.to_string_lossy(),
                "mcp_config": { "command": exe.to_string_lossy(), "args": ["mcp"] }
            },
            "tools": [
                { "name": "search_files" }, { "name": "folder_sizes" }, { "name": "largest_files" }, { "name": "recent_changes" }, { "name": "search_text" },
                { "name": "move_items" }, { "name": "rename_item" }, { "name": "create_folder" }, { "name": "recycle_items" }
            ]
        });
        let path = dir.join("manifest.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let mut command = std::process::Command::new(odr);
        command.args(["mcp", "add"]).arg(&path);
        crate::proc::no_window(&mut command);
        let out = command.output().map_err(|e| format!("Couldn't run odr.exe: {e}"))?;
        let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string();
        if out.status.success() {
            Ok(said)
        } else {
            Err(if said.is_empty() { format!("odr.exe failed ({})", out.status) } else { said })
        }
    })
    .await
}
