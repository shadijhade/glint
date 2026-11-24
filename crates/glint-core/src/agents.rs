//! What AI agents may change through Glint's MCP server (mcp.rs): move, rename, make folders, send to
//! the Recycle Bin and label, tag or comment. Each asks you first, in a Windows dialog naming exactly what it will do, does it
//! through Windows' own file engine (marks are written to the file's own homes), and is written down, so Settings → AI agents lists what agents did
//! with an Undo for each. And Glint's server, added to Windows' own agent registry (odr.exe) where
//! Windows has one, so Windows' agents find it without setup.

use crate::marks::{Change, Marks};
use glint_model::marks::tidy;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One thing an agent did, as it was done: what it was asked, and where each item went.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub id: u64,
    pub at: i64,
    /// "move", "rename", "folder", "recycle" or "marks".
    pub kind: String,
    pub summary: String,
    /// (from, to) for each item; `to` None when nothing came of it.
    pub items: Vec<(String, Option<String>)>,
    #[serde(default)]
    pub undone: bool,
    /// For "marks": the marks each item had before, which Undo puts back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<(String, Marks)>,
    /// For "marks": what each item was given, in `marks`' order. Undo puts back only an item that still has it,
    /// so marks the person set since are kept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks_after: Vec<Marks>,
}

fn log_path() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("agent-actions.json"))
}

pub fn actions() -> Vec<Action> {
    log_path().map(|p| actions_at(&p)).unwrap_or_default()
}

/// A log's actions, for another module's tests.
#[cfg(test)]
pub(crate) fn actions_in_for_test(path: &Path) -> Vec<Action> {
    actions_at(path)
}

fn actions_at(path: &Path) -> Vec<Action> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_at(path: &Path, actions: &[Action]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, serde_json::to_vec_pretty(actions).unwrap_or_default());
}

/// Writes down what an agent did (the last 200 are kept).
pub fn record(kind: &str, summary: String, items: Vec<(String, Option<String>)>) {
    if let Some(log) = log_path() {
        record_at(&log, kind, summary, items, Vec::new(), Vec::new());
    }
}

/// Writes down that an agent changed marks: each item with the marks it had before (Undo puts them back) and
/// the marks it was given.
pub fn record_marks(summary: String, changes: Vec<(String, Marks, Marks)>) {
    if let Some(log) = log_path() {
        record_marks_at(&log, summary, changes);
    }
}

/// [`record_marks`] in a log of the caller's (a test's).
pub fn record_marks_at(log: &Path, summary: String, changes: Vec<(String, Marks, Marks)>) {
    let items = changes.iter().map(|(path, _, _)| (path.clone(), Some(path.clone()))).collect();
    let afters = changes.iter().map(|(_, _, after)| after.clone()).collect();
    let befores = changes.into_iter().map(|(path, before, _)| (path, before)).collect();
    record_at(log, "marks", summary, items, befores, afters);
}

fn record_at(
    log: &Path,
    kind: &str,
    summary: String,
    items: Vec<(String, Option<String>)>,
    marks: Vec<(String, Marks)>,
    marks_after: Vec<Marks>,
) {
    let mut all = actions_at(log);
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    all.push(Action { id: at as u64, at, kind: kind.into(), summary, items, undone: false, marks, marks_after });
    let excess = all.len().saturating_sub(200);
    all.drain(..excess);
    save_at(log, &all);
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
pub fn agent_actions() -> Vec<Action> {
    let mut all = actions();
    all.reverse();
    all
}

/// Takes back what an agent did: moved and renamed items go back, a folder it made goes to the Recycle
/// Bin (if it's empty), what it recycled is restored, the marks it changed are put back.
pub async fn agent_undo(window: crate::host::Window, id: u64) -> Result<(), String> {
    let owner = crate::shell::owner(&window);
    crate::fsx::blocking(move || {
        let log = log_path().ok_or("That's no longer in the list")?;
        undo_at(&log, owner, id)
    })
    .await
}

fn undo_at(log: &Path, owner: isize, id: u64) -> Result<(), String> {
    {
        let mut all = actions_at(log);
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
            "marks" => {
                // Only what still has what the agent gave it: marks the person set since are theirs to keep. A log
                // from before the afters were kept puts everything back.
                let paths: Vec<String> = action.marks.iter().map(|(path, _)| path.clone()).collect();
                let now = crate::marks::read_many(&paths);
                let puts = action
                    .marks
                    .iter()
                    .zip(now)
                    .enumerate()
                    .filter(|(i, (_, now))| action.marks_after.get(*i).is_none_or(|after| tidy(after) == tidy(now)))
                    .map(|(_, ((path, before), _))| (path.clone(), Change::Put(before.clone())))
                    .collect();
                for outcome in crate::marks::set_many(puts) {
                    if let Err(why) = outcome.after {
                        let name = Path::new(&outcome.path).file_name().map_or_else(|| outcome.path.clone(), |n| n.to_string_lossy().into_owned());
                        return Err(format!("“{name}”: {why}"));
                    }
                }
            }
            _ => {}
        }
        action.undone = true;
        save_at(log, &all);
        Ok(())
    }
}

/// Windows' agent registry tool, where Windows has it.
fn odr() -> Option<PathBuf> {
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let exe = system.join(r"System32\odr.exe");
    exe.exists().then_some(exe)
}

/// Whether Windows has an agent registry to add Glint to.
pub fn agents_registry_available() -> bool {
    odr().is_some()
}

/// Adds Glint's MCP server to Windows' agent registry: an MCP bundle manifest naming `Glint.exe mcp`,
/// written to Glint's data folder, given to `odr.exe mcp add`. What odr said.
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
            "description": "Find files by name on every drive in milliseconds, measure folders, list the biggest files, say where a file went, search inside files, read the labels, tags and comments you gave files, and (asking you first) move, rename, recycle, label, tag and comment them.",
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
                { "name": "move_items" }, { "name": "rename_item" }, { "name": "create_folder" }, { "name": "recycle_items" },
                { "name": "get_marks" }, { "name": "set_marks" }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of the test's own under the temp folder, with its log's path.
    fn scratch(name: &str) -> (PathBuf, PathBuf) {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("glint-agents-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("agent-actions.json");
        (dir, log)
    }

    fn marked(label: &str) -> Marks {
        Marks { label: Some(label.into()), tags: vec!["a".into()], comment: Some("hi".into()) }
    }

    #[test]
    fn a_log_from_before_marks_still_reads() {
        let (dir, log) = scratch("old");
        std::fs::write(&log, r#"[{"id":5,"at":5,"kind":"move","summary":"Moved 1 item into D:\\x","items":[["C:\\a",null]],"undone":true}]"#).unwrap();
        let all = actions_at(&log);
        assert_eq!(all.len(), 1);
        assert!(all[0].marks.is_empty() && all[0].undone);
        // And a log with no marks in it is written as 0.16.1 wrote it.
        save_at(&log, &all);
        assert!(!std::fs::read_to_string(&log).unwrap().contains("marks"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn undo_puts_the_marks_back_and_says_what_it_could_not() {
        let (dir, log) = scratch("undo");
        let file = dir.join("f.txt").to_string_lossy().into_owned();
        std::fs::write(&file, "x").unwrap();
        let before = marked("Important");
        assert!(crate::marks::set_many(vec![(file.clone(), Change::Put(before.clone()))])[0].after.is_ok());
        let after = crate::marks::set_many(vec![(file.clone(), Change::Clear)]);
        assert!(after[0].after.is_ok());
        assert!(crate::marks::read_many(std::slice::from_ref(&file))[0].is_empty());

        record_marks_at(&log, "Changed the marks of 1 item".into(), vec![(file.clone(), before.clone(), Marks::default())]);
        let all = actions_at(&log);
        assert_eq!((all[0].kind.as_str(), all[0].items.len(), all[0].marks.len()), ("marks", 1, 1));
        undo_at(&log, 0, all[0].id).unwrap();
        assert_eq!(crate::marks::read_many(std::slice::from_ref(&file))[0], before);
        assert!(actions_at(&log)[0].undone);

        // An item that can't be put back fails the Undo with words, and the action stays to do.
        let drive = r"C:\".to_string();
        let mut stuck = actions_at(&log);
        stuck[0].undone = false;
        stuck[0].marks = vec![(drive, before)];
        save_at(&log, &stuck);
        let why = undo_at(&log, 0, stuck[0].id).unwrap_err();
        assert!(why.starts_with("“C:\\”: ") || why.contains("drive"), "{why}");
        assert!(!actions_at(&log)[0].undone);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn undo_leaves_an_item_the_person_marked_since() {
        let (dir, log) = scratch("since");
        let (a, b) = (dir.join("a.txt").to_string_lossy().into_owned(), dir.join("b.txt").to_string_lossy().into_owned());
        for file in [&a, &b] {
            std::fs::write(file, "x").unwrap();
        }
        let given = marked("Done");
        crate::marks::set_many(vec![(a.clone(), Change::Put(given.clone())), (b.clone(), Change::Put(given.clone()))]);
        let changes = vec![(a.clone(), Marks::default(), given.clone()), (b.clone(), Marks::default(), given.clone())];
        record_marks_at(&log, "Labeled 2 items “Done”".into(), changes);
        // The person comments on b afterwards: Undo takes the agent's marks off a, and leaves b as the person left it.
        let mine = Marks { comment: Some("mine".into()), ..given };
        crate::marks::set_many(vec![(b.clone(), Change::Put(mine.clone()))]);
        undo_at(&log, 0, actions_at(&log)[0].id).unwrap();
        assert_eq!(crate::marks::read_many(&[a, b]), vec![Marks::default(), mine]);
        assert!(actions_at(&log)[0].undone);
        let _ = std::fs::remove_dir_all(dir);
    }
}
