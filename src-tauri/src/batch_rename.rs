//! Two-phase atomic batch renamer with cycle/chain resolution and automatic rollback.
//!
//! Standard sequential renames fail when files swap names (`a.txt <-> b.txt`), shift along a
//! numbered chain (`01.txt -> 02.txt`, `02.txt -> 03.txt`), or change only letter case on NTFS
//! (`readme.md -> README.md`). By staging all participating paths through deterministic sibling
//! temporary names on the same volume, the entire batch succeeds or rolls back cleanly.

use crate::fsx::blocking;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static BATCH_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamePair {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchRenameResult {
    pub renamed: usize,
    /** Inverse pairs (`from: new_path, to: old_path`) so the UI can undo the batch in one click. */
    pub undo_ops: Vec<RenamePair>,
    pub elapsed_ms: f64,
}

#[tauri::command]
pub async fn batch_rename(ops: Vec<RenamePair>) -> Result<BatchRenameResult, String> {
    blocking(move || {
        // A preview handler may hold one of them open.
        crate::native_preview::release(&ops.iter().map(|op| op.from.clone()).collect::<Vec<_>>());
        execute_batch_rename(ops)
    })
    .await
}

/// Renames without replacing anything at `to` (std's rename would): if something appeared there after
/// the checks, the batch rolls back instead of overwriting it.
#[cfg(windows)]
fn move_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVE_FILE_FLAGS};
    let wide = |p: &Path| p.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (from, to) = (wide(from), wide(to));
    unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), MOVE_FILE_FLAGS(0)) }.map_err(io::Error::from)
}

#[cfg(not(windows))]
fn move_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "already exists"));
    }
    std::fs::rename(from, to)
}

fn is_reserved_dos_name(stem: &str) -> bool {
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn validate_filename(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." {
        return Err("File name cannot be empty.".into());
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return Err(format!("“{name}” cannot end with a space or period on Windows."));
    }
    if name.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 32) {
        return Err(format!("“{name}” contains characters not allowed in Windows file names."));
    }
    let stem = name.split('.').next().unwrap_or(name);
    if is_reserved_dos_name(stem) {
        return Err(format!("“{name}” is a reserved Windows device name."));
    }
    Ok(())
}

pub fn execute_batch_rename(ops: Vec<RenamePair>) -> Result<BatchRenameResult, String> {
    let started = Instant::now();
    // Filter out no-op pairs where the exact byte string is unchanged.
    let active: Vec<RenamePair> = ops.into_iter().filter(|p| p.from != p.to).collect();
    if active.is_empty() {
        return Ok(BatchRenameResult {
            renamed: 0,
            undo_ops: Vec::new(),
            elapsed_ms: 0.0,
        });
    }

    let mut source_keys = HashSet::with_capacity(active.len());
    let mut target_keys = HashSet::with_capacity(active.len());

    for pair in &active {
        let from_path = Path::new(&pair.from);
        let to_path = Path::new(&pair.to);
        if !from_path.exists() {
            return Err(format!("Source item does not exist: {}", pair.from));
        }
        let to_name = to_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("Invalid destination path: {}", pair.to))?;
        validate_filename(to_name)?;
        // A rename, not a move: every item stays in its folder.
        let folder = |p: &Path| p.parent().map(|f| f.to_string_lossy().to_lowercase());
        if folder(from_path).is_none() || folder(from_path) != folder(to_path) {
            return Err(format!("“{to_name}” would move to another folder."));
        }

        let src_key = pair.from.to_lowercase();
        let dst_key = pair.to.to_lowercase();
        if !source_keys.insert(src_key) {
            return Err(format!("Duplicate source path in batch: {}", pair.from));
        }
        if !target_keys.insert(dst_key) {
            return Err(format!("Two items would be renamed to the same name: {to_name}"));
        }
    }

    // Ensure no destination overwrites an existing file outside the batch's source set.
    for pair in &active {
        let dst_key = pair.to.to_lowercase();
        if !source_keys.contains(&dst_key) && Path::new(&pair.to).exists() {
            let to_name = Path::new(&pair.to)
                .file_name()
                .map_or(pair.to.as_str(), |n| n.to_str().unwrap_or(&pair.to));
            return Err(format!("“{to_name}” already exists in this folder."));
        }
    }

    let batch_id = BATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut staged: Vec<(PathBuf, PathBuf, PathBuf)> = Vec::with_capacity(active.len());

    // Phase 1: Move all sources to temporary sibling names.
    for (idx, pair) in active.iter().enumerate() {
        let from_path = PathBuf::from(&pair.from);
        let to_path = PathBuf::from(&pair.to);
        let parent = from_path
            .parent()
            .ok_or_else(|| format!("Cannot determine parent folder of {}", pair.from))?;
        let mut attempt = 0u32;
        let temp_path = loop {
            let candidate = parent.join(format!(".glint-ren-{pid}-{batch_id}-{idx}-{attempt}.tmp"));
            if !candidate.exists() {
                break candidate;
            }
            attempt += 1;
        };

        if let Err(e) = move_no_replace(&from_path, &temp_path) {
            // Roll back all Phase 1 moves completed so far.
            for (orig, tmp, _) in staged.iter().rev() {
                let _ = move_no_replace(tmp, orig);
            }
            return Err(format!("Couldn't stage “{}”: {e}", pair.from));
        }
        staged.push((from_path, temp_path, to_path));
    }

    // Phase 2: Move all temporary sibling names to their final destinations.
    let mut completed = 0usize;
    for (_, temp_path, to_path) in &staged {
        if let Err(e) = move_no_replace(temp_path, to_path) {
            // Roll back Phase 2 moves that already finished back to temp, then all temps back to original.
            for (_, tmp, dst) in staged[..completed].iter().rev() {
                let _ = move_no_replace(dst, tmp);
            }
            for (orig, tmp, _) in staged.iter().rev() {
                let _ = move_no_replace(tmp, orig);
            }
            return Err(format!("Couldn't rename to “{}”: {e}", to_path.display()));
        }
        completed += 1;
    }

    let undo_ops = staged
        .into_iter()
        .map(|(orig, _, dst)| RenamePair {
            from: dst.to_string_lossy().into_owned(),
            to: orig.to_string_lossy().into_owned(),
        })
        .collect();

    Ok(BatchRenameResult {
        renamed: completed,
        undo_ops,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn keeps_items_in_their_folder() {
        let dir = std::env::temp_dir().join(format!("glint_batch_rename_folder_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("elsewhere")).unwrap();
        let file = dir.join("note.txt");
        fs::write(&file, b"x").unwrap();

        let err = execute_batch_rename(vec![RenamePair {
            from: file.to_string_lossy().into_owned(),
            to: dir.join("elsewhere").join("note.txt").to_string_lossy().into_owned(),
        }])
        .unwrap_err();
        assert!(err.contains("another folder"), "{err}");
        assert!(file.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn swaps_cyclic_and_case_only_names_and_undoes() {
        let dir = std::env::temp_dir().join(format!("glint_batch_rename_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let f1 = dir.join("alpha.txt");
        let f2 = dir.join("beta.txt");
        let f3 = dir.join("case.txt");
        fs::write(&f1, b"one").unwrap();
        fs::write(&f2, b"two").unwrap();
        fs::write(&f3, b"three").unwrap();

        let f3_upper = dir.join("CASE.txt");
        let res = execute_batch_rename(vec![
            RenamePair {
                from: f1.to_string_lossy().into_owned(),
                to: f2.to_string_lossy().into_owned(),
            },
            RenamePair {
                from: f2.to_string_lossy().into_owned(),
                to: f1.to_string_lossy().into_owned(),
            },
            RenamePair {
                from: f3.to_string_lossy().into_owned(),
                to: f3_upper.to_string_lossy().into_owned(),
            },
        ])
        .unwrap();

        assert_eq!(res.renamed, 3);
        assert_eq!(fs::read_to_string(&f1).unwrap(), "two");
        assert_eq!(fs::read_to_string(&f2).unwrap(), "one");
        assert_eq!(fs::read_to_string(&f3_upper).unwrap(), "three");

        // Undo the entire batch using the returned inverse recipe.
        let undo = execute_batch_rename(res.undo_ops).unwrap();
        assert_eq!(undo.renamed, 3);
        assert_eq!(fs::read_to_string(&f1).unwrap(), "one");
        assert_eq!(fs::read_to_string(&f2).unwrap(), "two");

        let _ = fs::remove_dir_all(&dir);
    }
}
