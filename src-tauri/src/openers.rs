//! Open in…: the code editors and terminals found on this PC (VS Code, Cursor, Zed, Antigravity, Windows
//! Terminal, WezTerm, and yazi, which runs in a terminal), each where its installer puts it or on the
//! PATH; and opening a folder or a file in one, the way it takes one.

use crate::fsx::blocking;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Opener {
    pub id: &'static str,
    pub name: &'static str,
    /// A terminal (or a program that runs in one): it opens folders, a file's folder for a file.
    pub terminal: bool,
    pub path: String,
}

/// What Glint looks for: an id, a name, whether it's a terminal, where installers put its program
/// (below %LOCALAPPDATA% or %ProgramFiles%), and its file name on the PATH ("" for none).
const KNOWN: &[(&str, &str, bool, &[&str], &str)] = &[
    ("vscode", "VS Code", false, &[r"%LOCALAPPDATA%\Programs\Microsoft VS Code\Code.exe", r"%ProgramFiles%\Microsoft VS Code\Code.exe"], ""),
    ("cursor", "Cursor", false, &[r"%LOCALAPPDATA%\Programs\cursor\Cursor.exe", r"%ProgramFiles%\Cursor\Cursor.exe"], ""),
    ("zed", "Zed", false, &[r"%LOCALAPPDATA%\Programs\Zed\Zed.exe", r"%ProgramFiles%\Zed\Zed.exe"], "zed.exe"),
    ("antigravity", "Antigravity", false, &[r"%LOCALAPPDATA%\Programs\Antigravity IDE\Antigravity IDE.exe", r"%LOCALAPPDATA%\Programs\antigravity\Antigravity.exe"], ""),
    ("wt", "Windows Terminal", true, &[r"%LOCALAPPDATA%\Microsoft\WindowsApps\wt.exe"], "wt.exe"),
    ("wezterm", "WezTerm", true, &[r"%ProgramFiles%\WezTerm\wezterm-gui.exe", r"%LOCALAPPDATA%\Programs\WezTerm\wezterm-gui.exe"], "wezterm-gui.exe"),
    ("yazi", "yazi", true, &[r"%LOCALAPPDATA%\Microsoft\WinGet\Links\yazi.exe"], "yazi.exe"),
];

/// `%NAME%` in `path` as this account has it, or None when it isn't set.
fn expand(path: &str) -> Option<PathBuf> {
    let (name, rest) = path.strip_prefix('%')?.split_once('%')?;
    Some(PathBuf::from(std::env::var_os(name)?).join(rest.trim_start_matches('\\')))
}

/// Where a program is: one of `places`, else `exe` in a folder on the PATH.
fn find(places: &[&str], exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let on_path = std::env::split_paths(&path).filter(|_| !exe.is_empty()).map(|dir| dir.join(exe));
    places.iter().filter_map(|p| expand(p)).chain(on_path).find(|p| p.is_file())
}

/// The editors and terminals on this PC.
#[tauri::command]
pub async fn openers() -> Result<Vec<Opener>, String> {
    blocking(|| {
        Ok(KNOWN
            .iter()
            .filter_map(|&(id, name, terminal, places, exe)| find(places, exe).map(|p| Opener { id, name, terminal, path: p.to_string_lossy().into_owned() }))
            .collect())
    })
    .await
}

/// How `opener` (at `exe`) opens `path` (a folder when `dir`): the program and its arguments. A
/// terminal opens a file's folder; yazi runs in Windows Terminal when there is one.
fn command_for(id: &str, exe: &Path, path: &str, dir: bool, terminal_at: Option<&Path>) -> (PathBuf, Vec<String>) {
    let folder = if dir { path.to_string() } else { Path::new(path).parent().map_or_else(|| path.to_string(), |p| p.to_string_lossy().into_owned()) };
    match id {
        "wt" => (exe.into(), vec!["-d".into(), folder]),
        "wezterm" => (exe.into(), vec!["start".into(), "--cwd".into(), folder]),
        "yazi" => match terminal_at {
            Some(wt) => (wt.into(), vec!["-d".into(), folder, exe.to_string_lossy().into_owned(), path.to_string()]),
            None => (exe.into(), vec![path.to_string()]),
        },
        _ => (exe.into(), vec![path.to_string()]),
    }
}

/// Opens `path` (a folder or a file) in the editor or terminal `id`.
#[tauri::command]
pub async fn open_in(id: String, path: String) -> Result<(), String> {
    blocking(move || {
        let &(_, name, _, places, exe) = KNOWN.iter().find(|k| k.0 == id).ok_or("No such program")?;
        let at = find(places, exe).ok_or_else(|| format!("{name} isn't installed anymore"))?;
        let dir = Path::new(&path).is_dir();
        let wt = KNOWN.iter().find(|k| k.0 == "wt").and_then(|&(_, _, _, places, exe)| find(places, exe));
        let (program, args) = command_for(&id, &at, &path, dir, wt.as_deref());
        let mut command = Command::new(program);
        command.args(args);
        // yazi on its own needs a console of its own.
        #[cfg(windows)]
        if id == "yazi" && wt.is_none() {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x10);
        }
        command.spawn().map(drop).map_err(|e| format!("Couldn't start {name}: {e}"))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_each_the_way_it_takes_it() {
        let (wt, yazi, code) = (Path::new(r"C:\wt.exe"), Path::new(r"C:\yazi.exe"), Path::new(r"C:\Code.exe"));
        let args = |id: &str, exe: &Path, path: &str, dir: bool, terminal: Option<&Path>| command_for(id, exe, path, dir, terminal).1;
        assert_eq!(args("vscode", code, r"C:\Projects\app", true, None), [r"C:\Projects\app"]);
        assert_eq!(args("vscode", code, r"C:\Projects\app\main.rs", false, None), [r"C:\Projects\app\main.rs"]);
        // A terminal opens a file's folder.
        assert_eq!(args("wt", wt, r"C:\Projects\app\main.rs", false, None), ["-d", r"C:\Projects\app"]);
        assert_eq!(args("wezterm", Path::new(r"C:\w.exe"), r"C:\Projects", true, None), ["start", "--cwd", r"C:\Projects"]);
        // yazi in Windows Terminal when there is one, with the item it opens on.
        assert_eq!(command_for("yazi", yazi, r"C:\Projects\a.txt", false, Some(wt)), (PathBuf::from(r"C:\wt.exe"), vec!["-d".to_string(), r"C:\Projects".into(), r"C:\yazi.exe".into(), r"C:\Projects\a.txt".into()]));
        assert_eq!(command_for("yazi", yazi, r"C:\Projects", true, None).0, PathBuf::from(r"C:\yazi.exe"));
    }

    #[test]
    fn finds_programs_where_installers_put_them() {
        assert_eq!(expand(r"%NO_SUCH_VARIABLE_HERE%\x"), None);
        let windir = std::env::var("SystemRoot").unwrap();
        assert_eq!(expand(r"%SystemRoot%\System32"), Some(PathBuf::from(&windir).join("System32")));
        assert!(find(&[r"%SystemRoot%\notepad.exe"], "").is_some());
        assert!(find(&[r"%SystemRoot%\no-such-program-here.exe"], "").is_none());
        // On the PATH, by its file name.
        assert!(find(&[], "cmd.exe").is_some() && find(&[], "").is_none());
    }
}
