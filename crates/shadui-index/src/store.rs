//! The Microsoft Store's Glint, which can turn instant search on too. Uninstalling a Store app runs
//! nothing of the app's, so a service the Store's copy installed (which names its package when it does:
//! `install --package`) looks for that package now and then, and removes itself once it's gone for
//! every account, as the setup program's uninstaller would remove it. Any other copy that installs it,
//! or uses it (`Request::Claim`), keeps it.
//!
//! Gone means Windows has it installed for no account (an app it archived for disuse, its files gone
//! till it's opened again, still counts) and no folder of it is left in WindowsApps, which an uninstall
//! takes away last; anything it can't tell counts as there.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

/// The service's Parameters value naming the Store package that installed it.
pub const VALUE: &str = "StorePackage";
/// How often the service looks for the package, so an uninstall is seen within the hour: a question to
/// Windows and a folder's names, only while a Store package is named.
pub const EVERY: Duration = Duration::from_secs(30 * 60);
/// Gone once isn't enough: it's looked for again this much later, as an update replaces one version's
/// folder with the next one's.
pub const AGAIN: Duration = Duration::from_secs(10 * 60);

/// A package family name (`Name_PublisherId`), split into its two parts; None for anything else.
pub fn split(family: &str) -> Option<(&str, &str)> {
    let (name, publisher) = family.rsplit_once('_')?;
    let named = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    let published = publisher.len() == 13 && publisher.chars().all(|c| c.is_ascii_alphanumeric());
    (named && published).then_some((name, publisher))
}

/// Whether `apps` (Program Files\WindowsApps) holds a folder of the package family's: each version
/// installed, or on its way, has one, `Name_Version_Architecture_ResourceId_PublisherId`. A family it
/// can't read counts as there.
pub fn installed_in(apps: &Path, family: &str) -> io::Result<bool> {
    let Some((name, publisher)) = split(family) else { return Ok(true) };
    let (start, end) = (format!("{name}_").to_ascii_lowercase(), format!("_{publisher}").to_ascii_lowercase());
    for entry in fs::read_dir(apps)? {
        let entry = entry?;
        let folder = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if folder.starts_with(&start) && folder.ends_with(&end) && entry.file_type()?.is_dir() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether the package is still there, from Windows' record of what's installed for any account
/// (`registered`) and its folders in WindowsApps (`folder`); None when it can't tell. The folder alone
/// never says it's gone: an archived app has none.
pub fn present(registered: Option<bool>, folder: Option<bool>) -> Option<bool> {
    match (registered, folder) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), _) => Some(false),
        (None, _) => None,
    }
}

/// Watches for the package to go: `present` says whether it's there (None when it can't tell, which
/// counts as there), `sleep` waits, and says whether the service is stopping meanwhile. True once it's
/// been gone twice in a row, `again` apart: the service then removes itself.
pub fn watch(present: impl Fn() -> Option<bool>, sleep: impl Fn(Duration) -> bool, every: Duration, again: Duration) -> bool {
    loop {
        if present() == Some(false) {
            if sleep(again) {
                return false;
            }
            if present() == Some(false) {
                return true;
            }
        }
        if sleep(every) {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn reads_package_families() {
        assert_eq!(split("Glint.Test_4cqh9yg7jc6m4"), Some(("Glint.Test", "4cqh9yg7jc6m4")));
        // The Store's own.
        assert_eq!(split("shadi.Glint-FileExplorer_hbph6h7hvqyst"), Some(("shadi.Glint-FileExplorer", "hbph6h7hvqyst")));
        for bad in ["", "Glint", "_4cqh9yg7jc6m4", "Glint_short", "Gl int_4cqh9yg7jc6m4", r"..\x_4cqh9yg7jc6m4"] {
            assert_eq!(split(bad), None, "{bad}");
        }
    }

    #[test]
    fn gone_only_when_windows_has_no_record_of_it_and_no_folder() {
        assert_eq!(present(Some(false), Some(false)), Some(false));
        // Windows' record decides when the folders can't be read.
        assert_eq!(present(Some(false), None), Some(false));
        // An uninstall still clearing its files.
        assert_eq!(present(Some(false), Some(true)), Some(true));
        assert_eq!(present(Some(true), Some(false)), Some(true));
        // Without Windows' record, a missing folder may be an archived app.
        assert_eq!(present(None, Some(false)), None);
        assert_eq!(present(None, Some(true)), Some(true));
        assert_eq!(present(None, None), None);
    }

    #[test]
    fn finds_any_version_of_the_package() {
        let apps = std::env::temp_dir().join(format!("shadui-index-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&apps);
        fs::create_dir_all(apps.join("Microsoft.WindowsTerminal_1.21.0.0_x64__8wekyb3d8bbwe")).unwrap();
        fs::create_dir_all(apps.join("Glint.Testing_0.8.0.0_x64__4cqh9yg7jc6m4")).unwrap();
        // A file, not a package's folder.
        fs::write(apps.join("Glint.Test_0.7.0.0_x64__4cqh9yg7jc6m4"), b"").unwrap();
        let family = "Glint.Test_4cqh9yg7jc6m4";
        assert!(!installed_in(&apps, family).unwrap());
        fs::create_dir_all(apps.join("GLINT.TEST_0.8.0.0_x64__4CQH9YG7JC6M4")).unwrap();
        assert!(installed_in(&apps, family).unwrap());
        // Another publisher's package of the same name isn't it.
        assert!(!installed_in(&apps, "Glint.Test_zzzzzzzzzzzzz").unwrap());
        // Nothing to read, or nothing it can read: never taken for gone.
        assert!(installed_in(&apps, "not a family").unwrap());
        assert!(installed_in(&apps.join("missing"), family).is_err());
        fs::remove_dir_all(&apps).unwrap();
    }

    /// Runs `watch` over a script of answers (None: can't tell), with a sleep that stops it when the
    /// script runs out; returns its verdict and the waits it asked for.
    fn run(script: &[Option<bool>]) -> (bool, Vec<u64>) {
        let answers = RefCell::new(script.iter().copied());
        let left = Cell::new(script.len());
        let waits = RefCell::new(Vec::new());
        let verdict = watch(
            || {
                left.set(left.get() - 1);
                answers.borrow_mut().next().unwrap()
            },
            |time| {
                waits.borrow_mut().push(time.as_secs());
                left.get() == 0
            },
            Duration::from_secs(600),
            Duration::from_secs(60),
        );
        (verdict, waits.into_inner())
    }

    #[test]
    fn goes_only_when_gone_twice_in_a_row() {
        assert_eq!(run(&[Some(false), Some(false)]), (true, vec![60]));
        // Back by the second look (an update in progress): it keeps watching, at the long interval.
        assert_eq!(run(&[Some(false), Some(true), Some(true)]), (false, vec![60, 600, 600]));
        assert_eq!(run(&[Some(true), Some(false), Some(false)]), (true, vec![600, 60]));
        // Can't tell counts as there.
        assert_eq!(run(&[None, Some(false), None, None]), (false, vec![600, 60, 600, 600]));
    }
}
