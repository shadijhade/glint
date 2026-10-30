//! Recent, a place Glint opens like a folder (`shell:Recent`, File Explorer's name for Windows' Recent
//! Items): the files and folders opened lately, in any app, as Windows keeps them (a shortcut each in the
//! Recent folder), newest first. Each item is the file itself, at its own path, dated when it was last
//! opened; one that's gone, or on a drive that isn't there, is left out. When Windows is set not to keep
//! recently opened items, there are none. Forgetting one deletes its shortcut, as File Explorer's Remove
//! from Recent does.

use crate::fsx::{self, Entry};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Recent's path in Glint, as File Explorer's address bar takes it.
pub const PLACE: &str = "shell:Recent";

/// The most it lists: the latest.
const MOST: usize = 300;

pub fn is_place(path: &str) -> bool {
    path.eq_ignore_ascii_case(PLACE)
}

/// Windows' "Show recently opened items in Start, Jump Lists and File Explorer".
fn kept() -> bool {
    let value = crate::verbs::reg::get(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced", "Start_TrackDocs");
    !value.is_some_and(|v| v.data.get(..4) == Some(&[0, 0, 0, 0][..]))
}

/// The Recent Items folder, where Windows keeps a shortcut to each.
fn folder() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Recent"))
}

/// The shortcuts there, newest first: each with when it was last written (when its file was last opened).
fn shortcuts() -> Vec<(PathBuf, i64)> {
    let Some(folder) = folder() else { return Vec::new() };
    let mut found: Vec<(PathBuf, i64)> = fsx::read_dir(&folder.to_string_lossy())
        .unwrap_or_default()
        .into_iter()
        .filter(|e| !e.dir && e.name.to_ascii_lowercase().ends_with(".lnk"))
        .map(|e| (folder.join(&e.name), e.modified))
        .collect();
    found.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
    found
}

/// Whether Windows keeps recently opened items (Recent is empty when it doesn't).
#[tauri::command]
pub fn recent_kept() -> bool {
    kept()
}

/// Recent as a listing: `fsx::encode_items` with each item at its own path, newest first.
pub fn listing(started: Instant) -> Result<Vec<u8>, String> {
    let entries = if kept() { crate::shell::in_sta(|| Ok(imp::items(&shortcuts())))? } else { Vec::new() };
    Ok(fsx::encode_items(PLACE, &entries, &[], fsx::ms_since(started), false))
}

/// Takes items out of Recent (their shortcuts deleted, as File Explorer's Remove from Recent does): the
/// files themselves stay where they are.
#[tauri::command]
pub async fn recent_forget(paths: Vec<String>) -> Result<(), String> {
    fsx::blocking(move || {
        crate::shell::in_sta(move || {
            let wanted: Vec<String> = paths.iter().map(|p| p.to_lowercase()).collect();
            for (shortcut, _) in shortcuts() {
                if imp::target(&shortcut).is_some_and(|t| wanted.contains(&t.to_lowercase())) {
                    let _ = std::fs::remove_file(&shortcut);
                }
            }
            Ok(())
        })
    })
    .await
}

#[cfg(windows)]
mod imp {
    use super::{Entry, MOST};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SLGP_RAWPATH};

    /// Where a shortcut points: a file or folder's path (None for one to something else, such as a
    /// control panel or a web page).
    pub fn target(shortcut: &Path) -> Option<String> {
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            link.cast::<IPersistFile>().ok()?.Load(&HSTRING::from(shortcut), STGM_READ).ok()?;
            let mut path = [0u16; 1024];
            link.GetPath(&mut path, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32).ok()?;
            let len = path.iter().position(|&c| c == 0).unwrap_or(0);
            let text = String::from_utf16_lossy(&path[..len]);
            let expanded = super::expand(&text);
            (expanded.len() > 3 && (expanded.as_bytes().get(1) == Some(&b':') || expanded.starts_with(r"\\"))).then_some(expanded)
        }
    }

    /// The items the shortcuts point to, newest first, each once: the ones still there.
    pub fn items(shortcuts: &[(PathBuf, i64)]) -> Vec<Entry> {
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for (shortcut, opened) in shortcuts {
            if items.len() >= MOST {
                break;
            }
            let Some(path) = target(shortcut) else { continue };
            if !seen.insert(path.to_lowercase()) {
                continue;
            }
            // A share or a drive that isn't there now isn't waited for: its items are left out.
            if !super::reachable(&path) {
                continue;
            }
            let Some(found) = crate::fsx::stat(&path) else { continue };
            let name = Path::new(&path).file_name().map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
            items.push(Entry { name, path: Some(path), dir: found.dir, size: found.size, modified: *opened, attrs: found.attrs });
        }
        items
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Entry;
    use std::path::{Path, PathBuf};

    pub fn target(_shortcut: &Path) -> Option<String> {
        None
    }

    pub fn items(_shortcuts: &[(PathBuf, i64)]) -> Vec<Entry> {
        Vec::new()
    }
}

/// `%NAME%` in a shortcut's path, as this account has it.
fn expand(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else { break };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => out.push_str(&value),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out + rest
}

/// Whether a path's drive is one that answers now: a local drive that's there; a share is taken as
/// reachable only while its computer answers File Explorer's own check (fsx::on_other_computer's
/// shares are left out, as asking one that's gone can take half a minute).
fn reachable(path: &str) -> bool {
    if path.starts_with(r"\\") || fsx::on_other_computer(path) {
        return false;
    }
    path.get(..3).is_some_and(|root| Path::new(root).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_shortcuts_paths() {
        assert!(is_place("shell:recent") && !is_place(r"C:\Recent"));
        std::env::set_var("GLINT_RECENT_TEST", r"D:\Data");
        assert_eq!(expand(r"%GLINT_RECENT_TEST%\a.txt"), r"D:\Data\a.txt");
        assert!(!reachable(r"\\server\share\a.txt"));
        assert!(reachable(r"C:\Windows"));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "reads this account's Recent Items"]
    fn lists_this_accounts_recent_items() {
        let items = crate::shell::in_sta(|| Ok(imp::items(&shortcuts()))).unwrap();
        for item in items.iter().take(10) {
            println!("{} {} {}", item.modified, item.dir, item.path.as_deref().unwrap_or(""));
        }
        assert!(items.iter().all(|i| i.path.as_ref().is_some_and(|p| Path::new(p).exists())));
    }
}
