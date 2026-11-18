//! The Microsoft Store copy's data folders, and the one-time copy that fills them.
//!
//! A Store copy named its folders after the setup copy's (`com.shado.shadui-ripgrep`) until it got an identifier
//! of its own, so the two shared one. Windows' file system virtualization makes that worse than sharing: a packaged
//! app sees `%APPDATA%` and `%LOCALAPPDATA%` as the real folders with its own changes laid over them (kept under
//! `%LOCALAPPDATA%\Packages\<family>\LocalCache`, and gone with the package), so what a Store copy saved under the
//! shared name sits in that overlay, visible only to it.
//!
//! The first time a Store copy runs with folders of its own, [`fill`] fills each from the shared folder as the
//! Store copy sees it (the overlay laid over the real folder, so its own earlier settings come along with a setup
//! copy's). The copy only reads the shared folder: the setup copy keeps using it. What it writes is the Store
//! copy's own folder, which Windows keeps in the package's overlay like every other write.

use std::fs;
use std::io;
use std::path::Path;

/// Left in a folder once it has been filled, so the copy happens once however the folder came to be.
const DONE: &str = ".filled-from-shared";

/// What in the local folder isn't carried across: the old app's web view (its settings are in `state.json`
/// already), downloaded updates, and the content index (made again when asked for, and not to be copied while a
/// setup copy may be writing it).
const NOT_COPIED: &[&str] = &["EBWebView", "updates", "content"];

/// A folder bigger than this isn't copied: what is that big here is a cache or a tool that downloads again.
const MAX_FOLDER_BYTES: u64 = 256 << 20;

/// What a copy did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Copied {
    pub files: u32,
    /// Entries left out (too big, listed in [`NOT_COPIED`], links) or that couldn't be copied.
    pub left: u32,
}

/// Fills the Store copy's two folders from the shared ones, once, when `packaged` (anything else does nothing).
/// `roaming` and `local` are `%APPDATA%` and `%LOCALAPPDATA%`. A failure is passed over: the copy runs again at
/// the next start, and the app starts with what it has meanwhile.
pub fn fill(packaged: bool, roaming: &Path, local: &Path) {
    if !packaged {
        return;
    }
    let _ = copy_shared_into(
        &roaming.join(super::IDENTIFIER),
        &roaming.join(super::STORE_IDENTIFIER),
        &[],
    );
    let _ = copy_shared_into(
        &local.join(super::IDENTIFIER),
        &local.join(super::STORE_IDENTIFIER),
        NOT_COPIED,
    );
}

/// Fills `own` from `shared` once: every file `own` doesn't have yet is copied (never over one it has), the entries
/// named in `skip` are left out, and then [`DONE`] is written. `shared` is only read. A file that can't be copied
/// (one the other copy has locked, say) is left out and the folder isn't marked done, so the next start tries
/// again; what was copied stays, which is safe because nothing already there is replaced. A `shared` that isn't
/// there leaves nothing to copy, and `own` is marked done.
pub fn copy_shared_into(shared: &Path, own: &Path, skip: &[&str]) -> io::Result<Copied> {
    let mut copied = Copied::default();
    if own.join(DONE).exists() {
        return Ok(copied);
    }
    fs::create_dir_all(own)?;
    let mut complete = true;
    match fs::read_dir(shared) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Ok(kind) = entry.file_type() else { continue };
                // A link could lead out of the folder, and a skipped name is meant to stay behind.
                if kind.is_symlink() || skip.iter().any(|s| name.eq_ignore_ascii_case(s)) {
                    copied.left += 1;
                    continue;
                }
                let target = own.join(&name);
                let result = if kind.is_dir() {
                    if size_under(&entry.path(), MAX_FOLDER_BYTES).is_none() {
                        copied.left += 1;
                        Ok(())
                    } else {
                        copy_folder(&entry.path(), &target, &mut copied)
                    }
                } else {
                    copy_file(&entry.path(), &target, &mut copied)
                };
                complete &= result.is_ok();
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if complete {
        fs::write(own.join(DONE), b"")?;
    }
    Ok(copied)
}

/// One file, unless `target` is there already.
fn copy_file(from: &Path, target: &Path, copied: &mut Copied) -> io::Result<()> {
    if target.exists() {
        return Ok(());
    }
    match fs::copy(from, target) {
        Ok(_) => {
            copied.files += 1;
            Ok(())
        }
        Err(error) => {
            copied.left += 1;
            Err(error)
        }
    }
}

/// A folder and what's in it, by the same rules. An error on one file doesn't stop the rest; the first is returned.
fn copy_folder(from: &Path, target: &Path, copied: &mut Copied) -> io::Result<()> {
    fs::create_dir_all(target)?;
    let mut first = Ok(());
    for entry in fs::read_dir(from)?.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let to = target.join(entry.file_name());
        let result = if kind.is_symlink() {
            copied.left += 1;
            Ok(())
        } else if kind.is_dir() {
            copy_folder(&entry.path(), &to, copied)
        } else {
            copy_file(&entry.path(), &to, copied)
        };
        if first.is_ok() {
            first = result;
        }
    }
    first
}

/// The size of the files under `folder`, or None once it passes `limit` (it stops reading there) or can't be read.
fn size_under(folder: &Path, limit: u64) -> Option<u64> {
    let mut total = 0u64;
    let mut pending = vec![folder.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).ok()?.flatten() {
            let kind = entry.file_type().ok()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if !kind.is_symlink() {
                total += entry.metadata().map_or(0, |m| m.len());
                if total > limit {
                    return None;
                }
            }
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A scratch folder per test, removed at the end.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("glint-store-data-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn put(&self, relative: &str, text: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn read(&self, relative: &str) -> Option<String> {
            fs::read_to_string(self.0.join(relative)).ok()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Every file under `dir` with its text, to compare a folder before and after.
    fn tree(dir: &Path) -> Vec<(String, String)> {
        let mut all = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(folder) = pending.pop() {
            for entry in fs::read_dir(folder).unwrap().flatten() {
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path());
                } else {
                    let relative = entry.path().strip_prefix(dir).unwrap().to_string_lossy().into_owned();
                    all.push((relative, fs::read_to_string(entry.path()).unwrap()));
                }
            }
        }
        all.sort();
        all
    }

    #[test]
    fn a_first_run_copies_what_the_shared_folder_has_and_leaves_it_as_it_was() {
        let scratch = Scratch::new("first");
        scratch.put("shared/state.json", "{\"settings\":1}");
        scratch.put("shared/deep/er/note.txt", "kept");
        let (shared, own) = (scratch.0.join("shared"), scratch.0.join("own"));
        let before = tree(&shared);
        let copied = copy_shared_into(&shared, &own, &[]).unwrap();
        assert_eq!(copied, Copied { files: 2, left: 0 });
        assert_eq!(scratch.read("own/state.json").as_deref(), Some("{\"settings\":1}"));
        assert_eq!(scratch.read("own/deep/er/note.txt").as_deref(), Some("kept"));
        assert!(own.join(DONE).exists());
        assert_eq!(tree(&shared), before, "the shared folder was changed");
    }

    #[test]
    fn it_happens_once_and_never_replaces_what_the_store_copy_has() {
        let scratch = Scratch::new("once");
        scratch.put("shared/state.json", "shared");
        scratch.put("shared/window.json", "window");
        scratch.put("own/state.json", "the store's");
        let (shared, own) = (scratch.0.join("shared"), scratch.0.join("own"));
        copy_shared_into(&shared, &own, &[]).unwrap();
        assert_eq!(scratch.read("own/state.json").as_deref(), Some("the store's"));
        assert_eq!(scratch.read("own/window.json").as_deref(), Some("window"));
        // A later change in the shared folder (the setup copy keeps running) isn't carried across.
        scratch.put("shared/later.json", "later");
        assert_eq!(copy_shared_into(&shared, &own, &[]).unwrap(), Copied::default());
        assert!(scratch.read("own/later.json").is_none());
    }

    #[test]
    fn a_missing_shared_folder_gives_an_empty_own_one_that_is_marked_done() {
        let scratch = Scratch::new("missing");
        let own = scratch.0.join("own");
        assert_eq!(
            copy_shared_into(&scratch.0.join("nothing"), &own, &[]).unwrap(),
            Copied::default()
        );
        assert!(own.join(DONE).exists());
        assert_eq!(fs::read_dir(&own).unwrap().count(), 1);
    }

    #[test]
    fn what_is_skipped_or_too_big_stays_behind() {
        let scratch = Scratch::new("skip");
        scratch.put("shared/window.json", "w");
        scratch.put("shared/EBWebView/Default/data", "old web view");
        scratch.put("shared/content/index", "index");
        scratch.put("shared/tools/rg.txt", "a tool");
        let own = scratch.0.join("own");
        let copied = copy_shared_into(&scratch.0.join("shared"), &own, NOT_COPIED).unwrap();
        assert_eq!(copied, Copied { files: 2, left: 2 });
        assert!(own.join("window.json").exists() && own.join("tools/rg.txt").exists());
        assert!(!own.join("EBWebView").exists() && !own.join("content").exists());
        // The size check stops at its limit.
        let tools = scratch.0.join("shared/tools");
        assert_eq!(size_under(&tools, 100), Some(6));
        assert_eq!(size_under(&tools, 5), None);
    }

    #[test]
    fn only_a_packaged_copy_fills_anything_and_each_root_from_its_own_shared_folder() {
        let scratch = Scratch::new("packaged");
        let (roaming, local) = (scratch.0.join("Roaming"), scratch.0.join("Local"));
        scratch.put(&format!("Roaming/{}/state.json", super::super::IDENTIFIER), "state");
        scratch.put(&format!("Local/{}/window.json", super::super::IDENTIFIER), "window");
        scratch.put(&format!("Local/{}/updates/setup.exe", super::super::IDENTIFIER), "big");
        let before = (tree(&roaming), tree(&local));
        let shared = (
            roaming.join(super::super::IDENTIFIER),
            local.join(super::super::IDENTIFIER),
        );
        let shared_before = (tree(&shared.0), tree(&shared.1));

        fill(false, &roaming, &local);
        assert_eq!(
            (tree(&roaming), tree(&local)),
            before,
            "an unpackaged copy changed something"
        );

        fill(true, &roaming, &local);
        let store = super::super::STORE_IDENTIFIER;
        assert_eq!(
            scratch.read(&format!("Roaming/{store}/state.json")).as_deref(),
            Some("state")
        );
        assert_eq!(
            scratch.read(&format!("Local/{store}/window.json")).as_deref(),
            Some("window")
        );
        assert!(scratch.read(&format!("Local/{store}/updates/setup.exe")).is_none());
        // The shared folders are as they were.
        assert_eq!((tree(&shared.0), tree(&shared.1)), shared_before);
    }
}
