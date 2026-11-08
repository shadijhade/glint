//! Previous versions (time travel): a file or folder as it was in each of Windows' shadow copies (System
//! Restore's restore points and backups' snapshots: shadow.rs in the index crate), to open, compare with
//! now and restore. The copies come from the instant search service (with VSS's own times), or VSS
//! itself when Glint runs as an administrator, or else are found by name. A copy is read-only; restoring
//! copies out of it, and a replaced item goes to the Recycle Bin first.

use serde::Serialize;
use shadui_index::shadow::{Snapshot, DEVICES};
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    /// The item's path in the copy.
    pub path: String,
    /// The copy's top, to open the whole drive as it was.
    pub root: String,
    /// When the copy was taken (Unix ms).
    pub created: i64,
    /// `created` is VSS's own time, not the copy's newest entry.
    pub exact: bool,
    pub dir: bool,
    pub size: u64,
    pub modified: i64,
    /// The item is gone now.
    pub gone: bool,
}

/// Whether a path is in a shadow copy (\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopyN\…).
pub fn is_shadow(path: &str) -> bool {
    path.len() > DEVICES.len() && path[..DEVICES.len()].eq_ignore_ascii_case(DEVICES) && path[DEVICES.len()..].to_ascii_lowercase().starts_with("harddiskvolumeshadowcopy")
}

/// A copy's top itself, with or without its closing backslash.
pub fn is_shadow_root(path: &str) -> bool {
    is_shadow(path) && !path[DEVICES.len()..].trim_end_matches('\\').contains('\\')
}

/// Every shadow copy: the service's answer (exact times), VSS's (as an administrator), or found by name.
pub fn snapshots() -> Vec<Snapshot> {
    #[cfg(windows)]
    {
        if let Ok(all) = shadui_index::client::snapshots() {
            return all;
        }
        if let Ok(all) = shadui_index::shadow::list() {
            return all;
        }
        shadui_index::shadow::probe()
    }
    #[cfg(not(windows))]
    Vec::new()
}

/// Each shadow copy of a drive, newest first.
#[tauri::command]
pub async fn shadow_copies() -> Result<Vec<Snapshot>, String> {
    crate::fsx::blocking(|| Ok(snapshots())).await
}

/// A file or folder in each shadow copy that has it, newest first: a file only where it differs from
/// the copy after it (and from now), a folder in every copy.
#[tauri::command]
pub async fn previous_versions(path: String) -> Result<Vec<Version>, String> {
    crate::fsx::blocking(move || {
        let path = crate::fsx::normalize(&path)?;
        let drive = path.get(..2).filter(|d| d.ends_with(':')).ok_or("Previous versions are for what's on this PC's drives")?.to_ascii_uppercase();
        let rest = path[2..].trim_start_matches('\\').to_string();
        let now = crate::fsx::stat(&path);
        let mut out: Vec<Version> = Vec::new();
        for snapshot in snapshots().into_iter().filter(|s| s.drive.eq_ignore_ascii_case(&drive)) {
            let at = format!("{}{rest}", snapshot.root);
            let Some(then) = crate::fsx::stat(&at) else { continue };
            if !then.dir {
                let same_as_now = now.as_ref().is_some_and(|n| !n.dir && n.size == then.size && n.modified == then.modified);
                let same_as_newer = out.last().is_some_and(|v| v.size == then.size && v.modified == then.modified);
                if same_as_now || same_as_newer {
                    continue;
                }
            }
            out.push(Version { path: at.trim_end_matches('\\').to_string(), root: snapshot.root, created: snapshot.created, exact: snapshot.exact, dir: then.dir, size: then.size, modified: then.modified, gone: now.is_none() });
        }
        Ok(out)
    })
    .await
}

/// Copies an item out of a shadow copy to `to`: beside what's there under the next free name, or, with
/// `replace`, in its place (what's there goes to the Recycle Bin first). Where it went.
#[tauri::command]
pub async fn restore_version(window: tauri::Window, from: String, to: String, replace: bool) -> Result<String, String> {
    let owner = crate::shell::owner(&window);
    crate::fsx::blocking(move || {
        if !is_shadow(&from) {
            return Err("That isn't a previous version".into());
        }
        let to = crate::fsx::normalize(&to)?;
        let mut target = PathBuf::from(&to);
        if std::fs::symlink_metadata(&target).is_ok() {
            if replace {
                crate::shell::recycle(owner, vec![to.clone()]).map(drop)?;
            } else {
                target = crate::queue::free_name(&target);
            }
        }
        copy_tree(Path::new(&from), &target).map_err(|e| format!("Couldn't restore it: {e}"))?;
        Ok(target.to_string_lossy().into_owned())
    })
    .await
}

/// A file from a shadow copy, copied to a temp folder (read-only) for its app to open: apps can't open
/// it where it is.
#[tauri::command]
pub async fn version_copy(path: String) -> Result<String, String> {
    crate::fsx::blocking(move || {
        if !is_shadow(&path) {
            return Err("That isn't a previous version".into());
        }
        let source = Path::new(&path);
        let name = source.file_name().ok_or("No file named")?;
        let copy = std::env::temp_dir().join("Glint").join("Previous versions").join(stamp(&path)).join(name);
        if let Some(dir) = copy.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        if std::fs::symlink_metadata(&copy).is_ok() {
            crate::queue::clear_read_only(&copy);
        }
        std::fs::copy(source, &copy).map_err(|e| format!("Couldn't copy it out: {e}"))?;
        if let Ok(meta) = std::fs::metadata(&copy) {
            let mut permissions = meta.permissions();
            permissions.set_readonly(true);
            let _ = std::fs::set_permissions(&copy, permissions);
        }
        Ok(copy.to_string_lossy().into_owned())
    })
    .await
}

/// The copy's own folder name (HarddiskVolumeShadowCopy3), so versions of one file from two copies
/// don't meet.
fn stamp(path: &str) -> String {
    path[DEVICES.len()..].split('\\').next().unwrap_or("copy").to_string()
}

/// Copy (paste, drag, the other pane) out of a shadow copy: each item into `dest`, under the next free
/// name when the name's taken.
pub fn copy_out(sources: &[String], dest: &str) -> Result<crate::shell::Performed, String> {
    let mut items = Vec::new();
    for source in sources {
        let name = Path::new(source).file_name().ok_or_else(|| format!("{source} has no name"))?;
        let mut target = Path::new(dest).join(name);
        if std::fs::symlink_metadata(&target).is_ok() {
            target = crate::queue::free_name(&target);
        }
        copy_tree(Path::new(source), &target).map_err(|e| format!("Couldn't copy {}: {e}", name.to_string_lossy()))?;
        items.push(crate::shell::Done { from: source.clone(), to: Some(target.to_string_lossy().into_owned()) });
    }
    Ok(crate::shell::Performed { completed: true, items })
}

/// Compare panes' Sync out of a shadow copy: each item into its folder as its name, replacing (what's
/// replaced goes to the Recycle Bin first).
pub fn copy_into(items: &[crate::shell::Back]) -> Result<crate::shell::Performed, String> {
    let mut done = Vec::new();
    for item in items {
        let target = Path::new(&item.folder).join(&item.name);
        if std::fs::symlink_metadata(&target).is_ok() {
            crate::shell::recycle(0, vec![target.to_string_lossy().into_owned()]).map(drop)?;
        }
        std::fs::create_dir_all(&item.folder).map_err(|e| e.to_string())?;
        copy_tree(Path::new(&item.path), &target).map_err(|e| format!("Couldn't copy {}: {e}", item.name))?;
        done.push(crate::shell::Done { from: item.path.clone(), to: Some(target.to_string_lossy().into_owned()) });
    }
    Ok(crate::shell::Performed { completed: true, items: done })
}

/// A file, or a folder and all in it, copied (CopyFileEx keeps dates and attributes).
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::metadata(from)?.is_dir() {
        std::fs::create_dir_all(to)?;
        for item in std::fs::read_dir(from)? {
            let item = item?;
            copy_tree(&item.path(), &to.join(item.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_shadow_paths() {
        assert!(is_shadow(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy3\Users\me"));
        assert!(is_shadow(r"\\?\globalroot\device\harddiskvolumeshadowcopy12\"));
        assert!(!is_shadow(r"\\?\GLOBALROOT\Device\HarddiskVolume3\Users"));
        assert!(!is_shadow(r"C:\Users"));
        assert!(is_shadow_root(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy3\"));
        assert!(is_shadow_root(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy3"));
        assert!(!is_shadow_root(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy3\Users"));
        assert_eq!(stamp(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy3\Users\a.txt"), "HarddiskVolumeShadowCopy3");
    }

    #[test]
    fn restores_a_folder_with_all_in_it() {
        let dir = std::env::temp_dir().join(format!("glint-versions-{}", std::process::id()));
        let from = dir.join("from");
        std::fs::create_dir_all(from.join("inner")).unwrap();
        std::fs::write(from.join("a.txt"), "a").unwrap();
        std::fs::write(from.join("inner").join("b.txt"), "b").unwrap();
        copy_tree(&from, &dir.join("to")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("to").join("inner").join("b.txt")).unwrap(), "b");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
