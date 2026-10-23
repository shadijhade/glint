//! Script actions: files in Glint's Scripts folder become commands (Ctrl+K, and a Scripts menu on
//! items and folders), run with the selection's paths as their arguments, visibly, in the terminal
//! pane, as the person (never elevated). A PowerShell script runs with PowerShell, the execution
//! policy bypassed for that run only (one set by group policy still applies); Python with uv (outside
//! any project the folder shown is in) or Python itself; TypeScript with Bun, JavaScript with Bun or
//! Node; Nushell's with nu; and a .cmd or .bat as it is.

use crate::fsx::blocking;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Script {
    pub name: String,
    pub path: String,
    /// What runs it, and its arguments before the selection's (the script among them).
    pub program: String,
    pub args: Vec<String>,
    /// The tool it needs that isn't installed (its id in Settings → Tools), if any.
    pub missing: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Scripts {
    pub folder: String,
    pub scripts: Vec<Script>,
}

fn folder() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("scripts"))
}

/// A program on this PC, by the name Glint knows it.
fn program(exe: &str) -> Option<String> {
    crate::proc::locate(exe).map(|(p, _)| p.to_string_lossy().into_owned())
}

/// How `script` runs: its program and the arguments before the selection's; None for a file that
/// isn't a script.
fn invocation(script: &Path, find: &dyn Fn(&str) -> Option<String>) -> Option<(String, Vec<String>, Option<String>)> {
    let path = script.to_string_lossy().into_owned();
    let ext = script.extension()?.to_string_lossy().to_ascii_lowercase();
    let needs = |id: &str, exe: &str| (exe.to_string(), Vec::<String>::new(), Some(id.to_string()));
    Some(match ext.as_str() {
        "ps1" => {
            let host = find("pwsh").or_else(|| find("powershell")).unwrap_or_else(|| "powershell".into());
            (host, vec!["-NoProfile".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-File".into(), path], None)
        }
        // uv keeps a script's `# /// script` dependencies apart; Windows' python.exe placeholder (an
        // alias that opens the Store) isn't Python.
        "py" => match (find("uv"), find("python").filter(|p| !p.to_ascii_lowercase().contains(r"\microsoft\windowsapps\"))) {
            (Some(uv), _) => (uv, vec!["run".into(), "--no-project".into(), path], None),
            (None, Some(python)) => (python, vec![path], None),
            (None, None) => needs("uv", "uv"),
        },
        "ts" | "mts" => match find("bun") {
            Some(bun) => (bun, vec![path], None),
            None => needs("bun", "bun"),
        },
        "js" | "mjs" => match find("bun").or_else(|| find("node")) {
            Some(runner) => (runner, vec![path], None),
            None => needs("bun", "bun"),
        },
        "nu" => match find("nu") {
            Some(nu) => (nu, vec![path], None),
            None => needs("nushell", "nu"),
        },
        "cmd" | "bat" => (path, Vec::new(), None),
        _ => return None,
    })
}

/// The scripts in Glint's Scripts folder, by name.
#[tauri::command]
pub async fn scripts_list() -> Result<Scripts, String> {
    blocking(|| {
        let dir = folder().ok_or("Glint's data folder isn't available")?;
        let mut scripts: Vec<Script> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| {
                let path = e.path();
                let (program, args, missing) = invocation(&path, &|exe| program(exe))?;
                Some(Script { name: path.file_stem()?.to_string_lossy().into_owned(), path: path.to_string_lossy().into_owned(), program, args, missing })
            })
            .collect();
        scripts.sort_by_key(|s| s.name.to_lowercase());
        Ok(Scripts { folder: dir.to_string_lossy().into_owned(), scripts })
    })
    .await
}

/// The Scripts folder, made if it isn't there yet, with a script to start from and a README (which
/// Glint shows above the folder's files).
#[tauri::command]
pub async fn scripts_folder() -> Result<String, String> {
    blocking(|| {
        let dir = folder().ok_or("Glint's data folder isn't available")?;
        if !dir.is_dir() {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let _ = std::fs::write(dir.join("Sizes and dates.ps1"), EXAMPLE);
            let _ = std::fs::write(dir.join("README.md"), README);
        }
        Ok(dir.to_string_lossy().into_owned())
    })
    .await
}

/// The first script, to show how they're given what's chosen.
const EXAMPLE: &str = "\
# A Glint script: it runs in the terminal under the files, in the folder shown, given the paths of\r
# what's chosen (the folder itself when nothing is). Change it, copy it, or delete it.\r
foreach ($path in $args) {\r
    $item = Get-Item -LiteralPath $path\r
    if ($item.PSIsContainer) {\r
        \"{0}  folder, {1:N0} items\" -f $item.Name, @(Get-ChildItem -LiteralPath $path -Force).Count\r
    } else {\r
        \"{0}  {1:N0} bytes, changed {2:g}\" -f $item.Name, $item.Length, $item.LastWriteTime\r
    }\r
}\r
";

const README: &str = "\
# Glint's scripts\r
\r
Each script here is a command in Glint: in the **Scripts** menu when you right-click files or a folder, and in `Ctrl+K` as \"Script: its name\". \
It runs in the terminal under the files, in the folder shown, given the paths of what's chosen (or the folder, when nothing is) as its arguments. \
It runs as you, never elevated.\r
\r
| Kind | Runs with |\r
| :--- | :--- |\r
| `.ps1` | PowerShell 7, or Windows PowerShell, with the execution policy bypassed for that run |\r
| `.py` | `uv run --no-project` (a `# /// script` block's dependencies are installed for it), or Python |\r
| `.ts`, `.mts` | Bun |\r
| `.js`, `.mjs` | Bun, or Node.js |\r
| `.nu` | Nushell |\r
| `.cmd`, `.bat` | itself |\r
\r
A script whose program isn't installed (uv, Bun, Nushell) offers to install it from its menu entry.\r
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_each_kind_with_its_program() {
        let found = |exe: &str| ["pwsh", "uv", "node"].contains(&exe).then(|| format!(r"C:\bin\{exe}.exe"));
        let run = |name: &str| invocation(Path::new(&format!(r"C:\Scripts\{name}")), &found);
        assert_eq!(run("tidy.ps1"), Some((r"C:\bin\pwsh.exe".into(), vec!["-NoProfile".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-File".into(), r"C:\Scripts\tidy.ps1".into()], None)));
        assert_eq!(run("rename.py"), Some((r"C:\bin\uv.exe".into(), vec!["run".into(), "--no-project".into(), r"C:\Scripts\rename.py".into()], None)));
        // JavaScript with Node when there's no Bun; TypeScript waits for Bun.
        assert_eq!(run("list.js").map(|r| r.0), Some(r"C:\bin\node.exe".into()));
        assert_eq!(run("list.ts").and_then(|r| r.2), Some("bun".into()));
        assert_eq!(run("go.cmd"), Some((r"C:\Scripts\go.cmd".into(), vec![], None)));
        assert_eq!(run("notes.txt"), None);
        assert_eq!(run("README"), None);
    }

    #[test]
    fn python_without_uv_but_not_the_stores_placeholder() {
        let run = |python: &'static str| invocation(Path::new(r"C:\Scripts\a.py"), &move |exe: &str| (exe == "python").then(|| python.to_string()));
        assert_eq!(run(r"C:\Python313\python.exe"), Some((r"C:\Python313\python.exe".into(), vec![r"C:\Scripts\a.py".into()], None)));
        assert_eq!(run(r"C:\Users\a\AppData\Local\Microsoft\WindowsApps\python.exe").and_then(|r| r.2), Some("uv".into()));
    }
}
