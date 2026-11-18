//! Paste as link: what is on the clipboard made into links in a folder instead of copies.
//!
//! A folder becomes a directory junction (`mklink /J`, which needs no administrator rights), a file on the same
//! drive a hardlink (one file with two names, no space used twice), and a file from another drive or a network
//! share a `.lnk` shortcut, which is the only link that crosses drives. Names that are taken are numbered like
//! Glint's other pastes ("a (2).txt").
//!
//! Taking a link back ([`remove_links`]) must never touch what it points to. A junction is removed with
//! `remove_dir`, which on a junction removes the link only and on a real folder with anything in it fails; a
//! folder that is not a reparse point is refused outright. Nothing here deletes recursively.

use crate::fsx;
use std::io;
use std::path::{Path, PathBuf};

/// `FILE_ATTRIBUTE_DIRECTORY` and `FILE_ATTRIBUTE_REPARSE_POINT`.
const DIRECTORY: u32 = 0x10;
const REPARSE_POINT: u32 = 0x400;

/// What kind of link was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A directory junction to a folder.
    Junction,
    /// A second name for a file on the same drive.
    Hardlink,
    /// A `.lnk` shortcut.
    Shortcut,
}

/// A link that was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Made {
    /// Where the link is.
    pub path: String,
    /// What it points to.
    pub target: String,
    /// What kind it is.
    pub kind: Kind,
}

/// What a paste as link did: the links made and, for each item that failed, its path and why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The links made, in the order of the sources.
    pub made: Vec<Made>,
    /// The sources that could not be linked, with the reason.
    pub failed: Vec<(String, String)>,
}

/// Links every source into `dest`, on a thread of its own. Nothing is replaced.
pub async fn paste_as_links(sources: Vec<String>, dest: String) -> Result<Report, String> {
    fsx::blocking(move || Ok(make_links(&sources, &dest))).await
}

/// Takes links made by [`paste_as_links`] away again, each by its own kind of removal (see the module's note).
pub async fn undo_links(paths: Vec<String>) -> Result<(), String> {
    fsx::blocking(move || remove_links(&paths)).await
}

/// The synchronous work of [`paste_as_links`].
pub fn make_links(sources: &[String], dest: &str) -> Report {
    let mut report = Report::default();
    for source in sources {
        match make_link(source, dest) {
            Ok(made) => report.made.push(made),
            Err(why) => report.failed.push((source.clone(), why)),
        }
    }
    report
}

/// Whether `path` is on a local drive (a letter and a colon), where a junction or hardlink can point.
fn local(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Whether both paths are on the same local drive letter.
fn same_drive(a: &str, b: &str) -> bool {
    local(a) && local(b) && a.as_bytes()[0].eq_ignore_ascii_case(&b.as_bytes()[0])
}

/// The first name in `dest` for `name` that nothing (not even a broken link) has.
fn free_in(dest: &Path, name: &str, dir: bool) -> Result<PathBuf, String> {
    (1..10_000)
        .map(|n| dest.join(fsx::numbered(name, dir, n)))
        .find(|candidate| std::fs::symlink_metadata(candidate).is_err())
        .ok_or_else(|| "Couldn't find a free name".to_string())
}

fn make_link(source: &str, dest: &str) -> Result<Made, String> {
    let meta = std::fs::metadata(source).map_err(|e| format!("{source}: {}", fsx::file_error(e)))?;
    let name = Path::new(source)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| "A drive can't be linked to".to_string())?;
    let dest_path = Path::new(dest);
    let made = |link: &Path, kind| Made {
        path: link.to_string_lossy().into_owned(),
        target: source.to_string(),
        kind,
    };
    if meta.is_dir() {
        // A junction points at a local folder; a folder on a share can only be reached by a shortcut.
        if !local(source) {
            let link = free_in(dest_path, &format!("{name}.lnk"), false)?;
            shortcut(source, &link)?;
            return Ok(made(&link, Kind::Shortcut));
        }
        let link = free_in(dest_path, &name, true)?;
        junction(source, &link)?;
        return Ok(made(&link, Kind::Junction));
    }
    if same_drive(source, dest) {
        let link = free_in(dest_path, &name, false)?;
        match std::fs::hard_link(source, &link) {
            Ok(()) => return Ok(made(&link, Kind::Hardlink)),
            // A drive that keeps no hardlinks (FAT, exFAT) takes a shortcut instead; any other failure is told.
            Err(e) if unsupported(&e) => {}
            Err(e) => return Err(format!("{source}: {}", fsx::file_error(e))),
        }
    }
    let link = free_in(dest_path, &format!("{name}.lnk"), false)?;
    shortcut(source, &link)?;
    Ok(made(&link, Kind::Shortcut))
}

/// The file system doesn't do hardlinks (`ERROR_INVALID_FUNCTION` or `ERROR_NOT_SUPPORTED`).
fn unsupported(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(1 | 50)) || e.kind() == io::ErrorKind::Unsupported
}

/// `mklink /J link target`. A drive that can't hold a junction (FAT, exFAT, a share) is told in plain words.
fn junction(target: &str, link: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Couldn't start mklink: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let said = if said.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        said
    };
    Err(format!(
        "This drive can't hold a junction (only NTFS and ReFS can), or Windows refused it: {said}"
    ))
}

/// Writes a `.lnk` at `link` that points at `target`.
fn shortcut(target: &str, link: &Path) -> Result<(), String> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    let (target, link) = (target.to_string(), link.to_path_buf());
    crate::shell::in_sta(move || unsafe {
        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("Windows can't make a shortcut: {}", e.message()))?;
        shell_link
            .SetPath(&HSTRING::from(target.as_str()))
            .map_err(|e| format!("{target}: {}", e.message()))?;
        if let Some(folder) = Path::new(&target).parent().filter(|_| Path::new(&target).is_file()) {
            let _ = shell_link.SetWorkingDirectory(&HSTRING::from(folder));
        }
        shell_link
            .cast::<IPersistFile>()
            .and_then(|file| file.Save(&HSTRING::from(link.as_path()), true))
            .map_err(|e| format!("{}: {}", link.display(), e.message()))
    })
}

/// Removes the links at `paths`, and only the links (see the module's note). The first that can't be removed
/// stops the rest and is told.
pub fn remove_links(paths: &[String]) -> Result<(), String> {
    for path in paths {
        remove_link(Path::new(path)).map_err(|e| format!("{path}: {}", fsx::file_error(e)))?;
    }
    Ok(())
}

fn remove_link(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        // Already gone: nothing to take back.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let attrs = meta.file_attributes();
    if attrs & DIRECTORY != 0 {
        // A folder that is not a reparse point is a real folder now (someone replaced the link): never touched.
        if attrs & REPARSE_POINT == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "It is a folder, not a link, any more",
            ));
        }
        // On a junction this removes the link and leaves the folder it points to as it is.
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-links-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn text(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_folder_becomes_a_junction_and_undoing_it_keeps_every_file_in_the_target() {
        let dir = scratch("junction");
        let target = dir.join("target folder");
        std::fs::create_dir_all(target.join("sub")).unwrap();
        std::fs::write(target.join("one.txt"), "1").unwrap();
        std::fs::write(target.join("sub").join("two.txt"), "2").unwrap();
        let dest = dir.join("dest");
        std::fs::create_dir(&dest).unwrap();

        let report = make_links(&[text(&target)], &text(&dest));
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(report.made.len(), 1);
        assert_eq!(report.made[0].kind, Kind::Junction);
        let link = PathBuf::from(&report.made[0].path);
        // Through the junction, the files are there.
        assert_eq!(std::fs::read_to_string(link.join("one.txt")).unwrap(), "1");
        assert_eq!(std::fs::read_to_string(link.join("sub").join("two.txt")).unwrap(), "2");

        remove_links(&[text(&link)]).unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err(), "the link is gone");
        // The folder it pointed to, and everything in it, is untouched.
        assert_eq!(std::fs::read_to_string(target.join("one.txt")).unwrap(), "1");
        assert_eq!(
            std::fs::read_to_string(target.join("sub").join("two.txt")).unwrap(),
            "2"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_folder_is_never_removed_as_if_it_were_a_link() {
        let dir = scratch("real");
        let folder = dir.join("real");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("keep.txt"), "k").unwrap();
        assert!(remove_links(&[text(&folder)]).is_err());
        assert!(folder.join("keep.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_on_the_same_drive_becomes_a_hardlink_with_a_free_name() {
        let dir = scratch("hard");
        let source = dir.join("a.txt");
        std::fs::write(&source, "same").unwrap();
        // Into its own folder: the name is taken, so it is numbered.
        let first = make_links(&[text(&source)], &text(&dir));
        assert_eq!(first.made[0].kind, Kind::Hardlink);
        assert!(first.made[0].path.ends_with("a (2).txt"));
        let second = make_links(&[text(&source)], &text(&dir));
        assert!(second.made[0].path.ends_with("a (3).txt"));
        // One file with several names: a write through one shows in the others.
        std::fs::write(&source, "changed").unwrap();
        assert_eq!(std::fs::read_to_string(&first.made[0].path).unwrap(), "changed");
        // Taking a hardlink back leaves the original.
        remove_links(&[first.made[0].path.clone()]).unwrap();
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "changed");
        assert!(std::fs::symlink_metadata(&first.made[0].path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shortcut_is_a_lnk_that_can_be_taken_back() {
        let dir = scratch("lnk");
        let source = dir.join("b.txt");
        std::fs::write(&source, "b").unwrap();
        let link = dir.join("b.txt.lnk");
        shortcut(&text(&source), &link).unwrap();
        assert!(link.is_file());
        assert_eq!(
            crate::recent::link_target(&link).map(|t| t.to_lowercase()),
            Some(text(&source).to_lowercase())
        );
        remove_links(&[text(&link)]).unwrap();
        assert!(!link.exists());
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "b");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_source_is_reported_and_the_rest_are_still_linked() {
        let dir = scratch("missing");
        let source = dir.join("c.txt");
        std::fs::write(&source, "c").unwrap();
        let dest = dir.join("dest");
        std::fs::create_dir(&dest).unwrap();
        let report = make_links(&[text(&dir.join("gone.txt")), text(&source)], &text(&dest));
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.made.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn taking_back_a_link_that_is_already_gone_is_fine() {
        let dir = scratch("gone");
        assert!(remove_links(&[text(&dir.join("nothing"))]).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
