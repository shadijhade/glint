//! Folder history shared with zoxide, when it's installed and Settings → History says so: the folders it
//! knows (`zoxide query --list --score`, its frecency) rank in Ctrl+K and quick search beside the ones
//! opened in Glint, and each folder opened in Glint goes to it (`zoxide add`), as `cd` in a terminal with
//! its hook does, so `z` finds it there.

use std::process::{Command, Stdio};
use std::sync::{mpsc, OnceLock};

fn zoxide() -> Option<Command> {
    crate::tools::command("zoxide").ok()
}

/// zoxide's folders and their scores, best first; None when zoxide isn't installed.
#[tauri::command]
pub async fn zoxide_folders() -> Result<Option<Vec<(String, f64)>>, String> {
    crate::fsx::blocking(|| {
        let Some(mut command) = zoxide() else { return Ok(None) };
        // A database with nothing in it says so on its error output: no folders.
        let output = command.args(["query", "--list", "--score"]).stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| e.to_string())?;
        Ok(Some(parse(&String::from_utf8_lossy(&output.stdout))))
    })
    .await
}

/// `zoxide query --list --score`'s lines ("  12.5 C:\Users\me\proj"): each folder and its score.
fn parse(listing: &str) -> Vec<(String, f64)> {
    listing
        .lines()
        .filter_map(|line| {
            let (score, path) = line.trim_start().split_once(char::is_whitespace)?;
            let path = path.trim();
            (!path.is_empty()).then(|| Some((path.to_string(), score.parse().ok()?))).flatten()
        })
        .collect()
}

/// Where folders to add wait: one thread adds them, a `zoxide add` at a time, and of a burst opened while
/// one runs only the last (the ones passed through on the way aren't the ones used).
static ADD: OnceLock<mpsc::Sender<String>> = OnceLock::new();

/// Adds a folder to zoxide's (its score goes up), as its shell hook does on `cd`.
#[tauri::command]
pub fn zoxide_add(path: String) {
    let to_add = ADD.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            while let Ok(mut path) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    path = newer;
                }
                if let Some(mut command) = zoxide() {
                    let _ = command.args(["add", "--"]).arg(&path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
                }
            }
        });
        tx
    });
    let _ = to_add.send(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_folders_and_their_scores() {
        let listing = "  52.5 C:\\Users\\me\\proj\n   4.0 D:\\Work Files\\a b\n bad\n   1.2 \n";
        assert_eq!(parse(listing), [("C:\\Users\\me\\proj".to_string(), 52.5), ("D:\\Work Files\\a b".to_string(), 4.0)]);
        assert!(parse("").is_empty());
    }
}
