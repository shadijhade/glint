//! File Explorer's "Open in Glint" (on the right-click menu of folders, of the empty space in a
//! folder, and of drives) and, when asked, the app as what a double-click opens folders with:
//! per-user shell verbs under HKCU\Software\Classes, so no administrator rights. They run this
//! copy's exe, so only an installed copy adds them: one the setup program installed (its uninstaller
//! takes them out again: installer/hooks.nsh), or one Scoop installed, run through the app's
//! `current` link (`scoop uninstall` takes them out: the manifest from scripts/package-manifests.mjs).
//! Any copy that starts and finds them running an exe that's gone points them at itself or removes
//! them. File Explorer is never left running a missing program. Turning an option off removes
//! exactly what turning it on added, and puts back what folders opened with before.

use crate::fsx::blocking;
use serde::Serialize;
use tauri::AppHandle;

#[derive(Serialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Integration {
    /// Whether this copy can turn them on: the setup program or Scoop installed it.
    pub available: bool,
    /// "Open in Glint" is on File Explorer's menu.
    pub menu: bool,
    /// Double-clicking a folder opens it here.
    pub by_default: bool,
    /// Pressing Win+E opens Glint instead of File Explorer.
    pub win_e: bool,
    /// This copy is the Microsoft Store's, which Windows keeps out of File Explorer's settings.
    pub store: bool,
}

#[tauri::command]
pub async fn explorer_integration(app: AppHandle) -> Result<Integration, String> {
    blocking(move || {
        // As the check at start does (it may still be running): so this tells how things are after it.
        imp::repair(&app);
        Ok(imp::status(&app))
    })
    .await
}

/// Turns the menu entries, opening folders by default, and Win+E takeover on or off; folders open
/// here by default only with the menu entries on (the default action is the folder's entry).
#[tauri::command]
pub async fn set_explorer_integration(app: AppHandle, menu: bool, by_default: bool, win_e: Option<bool>) -> Result<Integration, String> {
    blocking(move || {
        imp::set(&app, menu, by_default, win_e)?;
        Ok(imp::status(&app))
    })
    .await
}

/// At start: entries that run an exe that's gone (a copy deleted without its uninstaller, an install
/// that moved) are pointed at this copy if the setup program or Scoop installed it, and removed
/// otherwise.
pub fn repair(app: &AppHandle) {
    imp::repair(app);
}

/// The current user's registry and the exe its entries run, for the Run key too (Start with Windows).
#[cfg(windows)]
pub(crate) use imp::{installed_exe, launch_exe, reg};

#[cfg(windows)]
mod imp {
    use super::Integration;
    use std::io;
    use std::path::{Path, PathBuf};
    use tauri::AppHandle;

    /// Folders, the empty space in a folder, and drives.
    const PLACES: [&str; 3] = [r"Directory\shell", r"Directory\Background\shell", r"Drive\shell"];
    /// Whose default verb is what a double-click on a file-system folder runs. Only plain folders:
    /// This PC, the Recycle Bin, libraries and other virtual folders are Folder, not Directory.
    const FOLDERS: &str = r"Directory\shell";
    /// The keys above the verbs, children first, removed once nothing is left in them.
    const PARENTS: [&str; 6] = [r"Directory\Background\shell", r"Directory\Background", r"Directory\shell", "Directory", r"Drive\shell", "Drive"];
    /// Kept in the folder verb while folders open here: the default it replaced, to put back.
    const PREVIOUS: &str = "PreviousDefault";
    const CLASSES: &str = r"Software\Classes";
    /// Windows 10/11's Win+E launcher CLSID (`OpenNewWindow` shell delegate).
    const WIN_E_CLSID: &str = r"CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}";

    /// The app's verbs: where they go and what they run.
    pub(super) struct Verbs {
        /// Under HKEY_CURRENT_USER: Software\Classes, or a scratch key in tests.
        pub classes: String,
        /// The verb's key: the app's name, so a test build's own name keeps its verbs apart.
        pub name: String,
        pub label: String,
        pub exe: PathBuf,
    }

    impl Verbs {
        /// Running the installed exe, or else this one (whose verbs are only ever taken out).
        fn for_app(app: &AppHandle) -> Option<Verbs> {
            let name = app.package_info().name.clone();
            let exe = installed_exe().or_else(|| std::env::current_exe().ok())?;
            Some(Verbs { classes: CLASSES.into(), label: format!("Open in {name}"), name, exe })
        }

        fn key(&self, place: &str) -> String {
            format!(r"{}\{place}\{}", self.classes, self.name)
        }

        fn folders(&self) -> String {
            format!(r"{}\{FOLDERS}", self.classes)
        }

        /// %V: the folder, the folder whose empty space was clicked, or the drive.
        fn command(&self) -> String {
            format!(r#""{}" "%V""#, self.exe.display())
        }

        fn is_default(&self) -> bool {
            reg::get_string(&self.folders(), "").is_some_and(|verb| verb.eq_ignore_ascii_case(&self.name))
        }

        /// (menu, by default)
        pub fn status(&self) -> (bool, bool) {
            let menu = PLACES.iter().any(|place| reg::exists(&self.key(place)));
            (menu, menu && self.is_default())
        }

        pub fn set(&self, menu: bool, by_default: bool) -> io::Result<()> {
            if by_default {
                self.add()?;
                self.take_default()?;
            } else {
                self.give_back_default()?;
                if menu {
                    self.add()?;
                } else {
                    self.remove()?;
                }
            }
            if self.classes == CLASSES {
                notify();
            }
            Ok(())
        }

        fn add(&self) -> io::Result<()> {
            for place in PLACES {
                let key = self.key(place);
                // The command first: an entry is never there without it.
                reg::set_string(&format!(r"{key}\command"), "", &self.command())?;
                reg::set_string(&key, "", &self.label)?;
                reg::set_string(&key, "Icon", &format!("{},0", self.exe.display()))?;
            }
            Ok(())
        }

        fn remove(&self) -> io::Result<()> {
            for place in PLACES {
                reg::delete_tree(&self.key(place))?;
            }
            for parent in PARENTS {
                reg::delete_if_empty(&format!(r"{}\{parent}", self.classes))?;
            }
            Ok(())
        }

        /// Makes the folder entry the default, keeping what was the default (if anything) to put back.
        fn take_default(&self) -> io::Result<()> {
            if self.is_default() {
                return Ok(());
            }
            let verb = self.key(FOLDERS);
            match reg::get(&self.folders(), "") {
                Some(previous) => reg::set(&verb, PREVIOUS, &previous)?,
                None => reg::delete_value(&verb, PREVIOUS)?,
            }
            reg::set_string(&self.folders(), "", &self.name)
        }

        /// Puts back the default from before, unless something else has changed it since.
        fn give_back_default(&self) -> io::Result<()> {
            let verb = self.key(FOLDERS);
            if self.is_default() {
                match reg::get(&verb, PREVIOUS) {
                    Some(previous) => reg::set(&self.folders(), "", &previous)?,
                    None => reg::delete_value(&self.folders(), "")?,
                }
            }
            reg::delete_value(&verb, PREVIOUS)
        }

        /// The exes the entries run, as their commands name them.
        fn exes(&self) -> Vec<String> {
            PLACES
                .iter()
                .filter_map(|place| reg::get_string(&format!(r"{}\command", self.key(place)), ""))
                .filter_map(|command| Some(command.strip_prefix('"')?.split('"').next()?.to_string()))
                .chain(self.win_e_exe())
                .collect()
        }

        fn win_e_command_key(&self) -> String {
            format!(r"{}\{WIN_E_CLSID}\shell\opennewwindow\command", self.classes)
        }

        /// The exe Win+E runs when it's this app's (by its name, so a copy since moved counts too):
        /// another app's (Files…) isn't Glint's to change or remove.
        fn win_e_exe(&self) -> Option<String> {
            let command = reg::get_string(&self.win_e_command_key(), "")?;
            let exe = command.strip_prefix('"')?.split('"').next()?.to_string();
            let name = self.exe.file_name()?.to_string_lossy().into_owned();
            Path::new(&exe).file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&name)).then_some(exe)
        }

        pub fn is_win_e(&self) -> bool {
            self.win_e_exe().is_some_and(|exe| exe.eq_ignore_ascii_case(&self.exe.to_string_lossy()))
        }

        pub fn set_win_e(&self, enabled: bool) -> io::Result<()> {
            if enabled {
                let cmd = self.win_e_command_key();
                reg::set_string(&cmd, "", &format!(r#""{}" "this-pc:""#, self.exe.display()))?;
                reg::set_string(&cmd, "DelegateExecute", "")?;
            } else if self.win_e_exe().is_some() {
                reg::delete_tree(&format!(r"{}\{WIN_E_CLSID}", self.classes))?;
                reg::delete_if_empty(&format!(r"{}\CLSID", self.classes))?;
            } else {
                return Ok(());
            }
            if self.classes == CLASSES {
                notify();
            }
            Ok(())
        }
    }

    pub fn status(app: &AppHandle) -> Integration {
        let Some(verbs) = Verbs::for_app(app) else {
            return Integration::default();
        };
        let (menu, by_default) = verbs.status();
        let win_e = verbs.is_win_e();
        Integration { available: installed_exe().is_some(), menu, by_default, win_e, store: crate::packaged() }
    }

    pub fn set(app: &AppHandle, menu: bool, by_default: bool, win_e: Option<bool>) -> Result<(), String> {
        if installed_exe().is_none() {
            return Err("Only a copy installed with the setup program or Scoop can do this.".into());
        }
        let verbs = Verbs::for_app(app).ok_or("Can't tell where the app is")?;
        verbs.set(menu, by_default).map_err(|e| e.to_string())?;
        if let Some(we) = win_e {
            verbs.set_win_e(we && menu).map_err(|e| e.to_string())?;
        } else if !menu && verbs.is_win_e() {
            verbs.set_win_e(false).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn repair(app: &AppHandle) {
        let Some(verbs) = Verbs::for_app(app) else { return };
        if verbs.exes().iter().all(|exe| Path::new(exe).exists()) {
            return;
        }
        let (menu, by_default) = verbs.status();
        // Win+E running a Glint that's gone runs this one, or goes.
        let win_e = verbs.win_e_exe().is_some();
        let installed = installed_exe().is_some();
        let _ = verbs.set(menu && installed, by_default && installed);
        if win_e {
            let _ = verbs.set_win_e(installed && menu);
        }
    }

    /// The exe the entries can run for as long as this copy is installed: the one the setup program
    /// installed, or a Scoop copy's through its `current` link. A portable copy or a dev build has
    /// none, as it could be moved or deleted with nothing to take the entries out.
    pub(crate) fn installed_exe() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        if crate::installed_by_setup() {
            Some(exe)
        } else {
            scoop_current(&exe)
        }
    }

    /// The exe other programs (AI agents, the Jump List) start Glint with: the installed one, or the
    /// Microsoft Store copy's alias, which stays put as updates come (its own exe's folder is named
    /// after its version), or this one.
    pub(crate) fn launch_exe() -> Option<PathBuf> {
        installed_exe().or_else(store_alias).or_else(|| std::env::current_exe().ok())
    }

    /// The Microsoft Store copy's `glint.exe` alias (its manifest's windows.appExecutionAlias), in the
    /// package's own folder of aliases, where no other app's can take its name.
    fn store_alias() -> Option<PathBuf> {
        let family = crate::package_family()?;
        let alias = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(r"Microsoft\WindowsApps").join(family).join("glint.exe");
        // An alias is a link Windows resolves itself, so it's looked at, not through.
        alias.symlink_metadata().is_ok().then_some(alias)
    }

    /// For an exe Scoop installed (`<root>\apps\<app>\<version>\Glint.exe`), the same exe through the
    /// app's `current` link, which `scoop update` points at each new version and which the manifest's
    /// uninstaller takes the entries out for. It's Scoop's when its folder holds Scoop's record of the
    /// install and is the one `current` leads to (not an older version kept until `scoop cleanup`), in
    /// a root that Scoop itself lives in, which is one user's. Apps installed for all users (`--global`)
    /// don't count, as uninstalling them couldn't reach other accounts' entries, and neither do apps
    /// without the link (Scoop's `no_junction` setting).
    fn scoop_current(exe: &Path) -> Option<PathBuf> {
        let name = exe.file_name()?;
        let version = exe.parent()?;
        let app = version.parent()?;
        let apps = app.parent()?;
        let current = app.join("current");
        let scoops = apps.file_name()?.eq_ignore_ascii_case("apps")
            && apps.join(r"scoop\current").is_dir()
            // scoop-install.json since Scoop 0.6, install.json before.
            && ["scoop-install.json", "install.json"].iter().any(|record| version.join(record).is_file())
            && std::fs::canonicalize(&current).ok()? == std::fs::canonicalize(version).ok()?;
        scoops.then(|| current.join(name))
    }

    /// Tells File Explorer the associations changed.
    fn notify() {
        use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }

    /// The registry under HKEY_CURRENT_USER, by path.
    pub(crate) mod reg {
        use std::io;
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
        use windows::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegQueryInfoKeyW,
            RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
            RRF_NOEXPAND, RRF_RT_ANY,
        };

        /// A value as stored: its type and bytes.
        #[derive(Clone, Debug, PartialEq)]
        pub struct Value {
            pub kind: REG_VALUE_TYPE,
            pub data: Vec<u8>,
        }

        fn check(status: WIN32_ERROR) -> io::Result<()> {
            if status.is_ok() {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(status.0 as i32))
            }
        }

        /// Missing is fine for deleting.
        fn gone(status: WIN32_ERROR) -> io::Result<()> {
            if status == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                check(status)
            }
        }

        pub struct Key(HKEY);

        impl Drop for Key {
            fn drop(&mut self) {
                let _ = unsafe { RegCloseKey(self.0) };
            }
        }

        fn open(path: &str, access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> io::Result<Key> {
            let mut key = HKEY::default();
            check(unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(path), None, access, &mut key) })?;
            Ok(Key(key))
        }

        pub fn exists(path: &str) -> bool {
            open(path, KEY_QUERY_VALUE).is_ok()
        }

        pub fn get(path: &str, name: &str) -> Option<Value> {
            let (path, name) = (HSTRING::from(path), HSTRING::from(name));
            let mut kind = REG_VALUE_TYPE::default();
            let mut size = 0u32;
            let flags = RRF_RT_ANY | RRF_NOEXPAND;
            unsafe { RegGetValueW(HKEY_CURRENT_USER, &path, &name, flags, Some(&mut kind as *mut _), None, Some(&mut size as *mut _)) }.ok().ok()?;
            let mut data = vec![0u8; size as usize];
            unsafe { RegGetValueW(HKEY_CURRENT_USER, &path, &name, flags, Some(&mut kind as *mut _), Some(data.as_mut_ptr().cast()), Some(&mut size as *mut _)) }.ok().ok()?;
            data.truncate(size as usize);
            Some(Value { kind, data })
        }

        pub fn get_string(path: &str, name: &str) -> Option<String> {
            let value = get(path, name)?;
            let units: Vec<u16> = value.data.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
            let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
            Some(String::from_utf16_lossy(&units[..end]))
        }

        /// Opens the key to write values, making it (and any above it) if needed.
        pub fn create(path: &str) -> io::Result<Key> {
            let mut key = HKEY::default();
            check(unsafe {
                RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(path), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None)
            })?;
            Ok(Key(key))
        }

        pub fn set(path: &str, name: &str, value: &Value) -> io::Result<()> {
            let key = create(path)?;
            check(unsafe { RegSetValueExW(key.0, &HSTRING::from(name), None, value.kind, Some(&value.data)) })
        }

        pub fn set_string(path: &str, name: &str, text: &str) -> io::Result<()> {
            let data = text.encode_utf16().chain(Some(0)).flat_map(u16::to_le_bytes).collect();
            set(path, name, &Value { kind: REG_SZ, data })
        }

        pub fn delete_value(path: &str, name: &str) -> io::Result<()> {
            gone(unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(path), &HSTRING::from(name)) })
        }

        pub fn delete_tree(path: &str) -> io::Result<()> {
            gone(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(path)) })
        }

        /// Deletes the key if it has no subkeys and no values (a default value counts).
        pub fn delete_if_empty(path: &str) -> io::Result<()> {
            let key = match open(path, KEY_QUERY_VALUE) {
                Ok(key) => key,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            };
            let (mut subkeys, mut values) = (0u32, 0u32);
            check(unsafe { RegQueryInfoKeyW(key.0, None, None, None, Some(&mut subkeys as *mut _), None, None, Some(&mut values as *mut _), None, None, None, None) })?;
            drop(key);
            if subkeys == 0 && values == 0 {
                gone(unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(path)) })?;
            }
            Ok(())
        }

        #[cfg(test)]
        pub use test_only::*;

        #[cfg(test)]
        mod test_only {
            use super::*;
            use windows::Win32::System::Registry::{RegEnumKeyExW, RegEnumValueW, KEY_READ};

            /// Everything under `path` (keys and values, sorted), to compare before and after.
            pub fn dump(path: &str) -> Vec<String> {
                let mut lines = Vec::new();
                walk(path, path, &mut lines);
                lines.sort();
                lines
            }

            fn walk(root: &str, path: &str, lines: &mut Vec<String>) {
                let Ok(key) = open(path, KEY_READ) else { return };
                let relative = path[root.len()..].to_string();
                lines.push(format!("[{relative}]"));
                for i in 0.. {
                    let mut name = [0u16; 16384];
                    let mut len = name.len() as u32;
                    if unsafe { RegEnumValueW(key.0, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) }.is_err() {
                        break;
                    }
                    let name = String::from_utf16_lossy(&name[..len as usize]);
                    let value = get(path, &name).unwrap();
                    lines.push(format!("[{relative}] {name:?} = {:?} {:?}", value.kind, value.data));
                }
                let mut children = Vec::new();
                for i in 0.. {
                    let mut name = [0u16; 256];
                    let mut len = name.len() as u32;
                    if unsafe { RegEnumKeyExW(key.0, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) }.is_err() {
                        break;
                    }
                    children.push(String::from_utf16_lossy(&name[..len as usize]));
                }
                for child in children {
                    walk(root, &format!(r"{path}\{child}"), lines);
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows::Win32::System::Registry::REG_EXPAND_SZ;

        /// A scratch HKCU\Software\GlintTests\… standing in for Software\Classes, removed afterwards.
        struct Scratch(String);

        impl Scratch {
            fn new(test: &str) -> Scratch {
                let root = format!(r"Software\GlintTests\{}-{test}", std::process::id());
                let _ = reg::delete_tree(&root);
                // Software\Classes is always there.
                reg::create(&format!(r"{root}\Classes")).unwrap();
                Scratch(root)
            }

            fn verbs(&self) -> Verbs {
                Verbs { classes: format!(r"{}\Classes", self.0), name: "GlintTest".into(), label: "Open in GlintTest".into(), exe: PathBuf::from(r"C:\Apps\Glint Test\Glint.exe") }
            }
        }

        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = reg::delete_tree(&self.0);
                let _ = reg::delete_if_empty(r"Software\GlintTests");
            }
        }

        #[test]
        fn menu_entries_come_and_go_exactly() {
            let scratch = Scratch::new("menu");
            let verbs = verbs_with_neighbours(&scratch);
            let before = reg::dump(&scratch.0);

            verbs.set(true, false).unwrap();
            assert_eq!(verbs.status(), (true, false));
            for place in PLACES {
                let key = verbs.key(place);
                assert_eq!(reg::get_string(&key, "").as_deref(), Some("Open in GlintTest"));
                assert_eq!(reg::get_string(&key, "Icon").as_deref(), Some(r"C:\Apps\Glint Test\Glint.exe,0"));
                assert_eq!(reg::get_string(&format!(r"{key}\command"), "").as_deref(), Some(r#""C:\Apps\Glint Test\Glint.exe" "%V""#));
            }
            assert_eq!(verbs.exes(), [r"C:\Apps\Glint Test\Glint.exe"; 3]);
            // On twice changes nothing.
            let on = reg::dump(&scratch.0);
            verbs.set(true, false).unwrap();
            assert_eq!(reg::dump(&scratch.0), on);

            verbs.set(false, false).unwrap();
            assert_eq!(verbs.status(), (false, false));
            assert_eq!(reg::dump(&scratch.0), before);
        }

        /// What Windows and other apps keep there, which must be left alone.
        fn verbs_with_neighbours(scratch: &Scratch) -> Verbs {
            let verbs = scratch.verbs();
            let classes = &verbs.classes;
            reg::set_string(&format!(r"{classes}\Directory\background\ShellEx\ContextMenuHandlers\PowerRenameExt"), "", "{0440049F}").unwrap();
            reg::set_string(&format!(r"{classes}\Drive\ShellEx\ContextMenuHandlers\FileLocksmithExt"), "", "{84D68575}").unwrap();
            reg::set_string(&format!(r"{classes}\Directory"), "AlwaysShowExt", "").unwrap();
            verbs
        }

        #[test]
        fn opening_folders_by_default_puts_back_what_was_there() {
            let scratch = Scratch::new("default");
            let verbs = scratch.verbs();
            let folders = verbs.folders();

            // Nothing there before: nothing there after.
            let before = reg::dump(&scratch.0);
            verbs.set(true, true).unwrap();
            assert_eq!(verbs.status(), (true, true));
            assert_eq!(reg::get_string(&folders, "").as_deref(), Some("GlintTest"));
            verbs.set(true, false).unwrap();
            assert_eq!(verbs.status(), (true, false));
            assert_eq!(reg::get(&folders, ""), None);
            verbs.set(false, false).unwrap();
            assert_eq!(reg::dump(&scratch.0), before);

            // Another default (with another app's verb, and an odd type): back as it was, type and all.
            let other = reg::Value { kind: REG_EXPAND_SZ, data: "OtherApp\0".encode_utf16().flat_map(u16::to_le_bytes).collect() };
            reg::set(&folders, "", &other).unwrap();
            reg::set_string(&format!(r"{folders}\OtherApp\command"), "", r#""C:\Other.exe" "%1""#).unwrap();
            let before = reg::dump(&scratch.0);
            verbs.set(true, true).unwrap();
            assert_eq!(reg::get_string(&folders, "").as_deref(), Some("GlintTest"));
            // On twice keeps the first default it replaced.
            verbs.set(true, true).unwrap();
            // Turning the menu off turns the default off too.
            verbs.set(false, false).unwrap();
            assert_eq!(reg::get(&folders, ""), Some(other.clone()));
            assert_eq!(reg::dump(&scratch.0), before);

            // Something else took the default since: it stays.
            verbs.set(true, true).unwrap();
            reg::set_string(&folders, "", "ThirdApp").unwrap();
            assert_eq!(verbs.status(), (true, false));
            verbs.set(false, false).unwrap();
            assert_eq!(reg::get_string(&folders, "").as_deref(), Some("ThirdApp"));
            assert!(!reg::exists(&verbs.key(FOLDERS)));
        }

        #[test]
        fn status_reads_any_entry() {
            let scratch = Scratch::new("status");
            let verbs = scratch.verbs();
            assert_eq!(verbs.status(), (false, false));
            reg::set_string(&format!(r"{}\command", verbs.key(r"Drive\shell")), "", r#""C:\Gone\Glint.exe" "%V""#).unwrap();
            assert_eq!(verbs.status(), (true, false));
            assert_eq!(verbs.exes(), [r"C:\Gone\Glint.exe"]);
            verbs.set(false, false).unwrap();
            assert_eq!(verbs.status(), (false, false));
            assert_eq!(reg::dump(&scratch.0), [r"[\Classes]", "[]"]);
        }

        #[test]
        fn win_e_override_comes_and_goes_cleanly() {
            let scratch = Scratch::new("win_e");
            let verbs = scratch.verbs();
            let before = reg::dump(&scratch.0);
            assert!(!verbs.is_win_e());
            verbs.set_win_e(true).unwrap();
            assert!(verbs.is_win_e());
            assert_eq!(
                reg::get_string(&verbs.win_e_command_key(), "").as_deref(),
                Some(r#""C:\Apps\Glint Test\Glint.exe" "this-pc:""#)
            );
            assert_eq!(reg::get_string(&verbs.win_e_command_key(), "DelegateExecute").as_deref(), Some(""));
            verbs.set_win_e(false).unwrap();
            assert!(!verbs.is_win_e());
            assert_eq!(reg::dump(&scratch.0), before);
        }

        #[test]
        fn another_apps_win_e_stays() {
            let scratch = Scratch::new("win_e_other");
            let verbs = scratch.verbs();
            let files = r#""C:\Program Files\Files\Files.exe" "%1""#;
            reg::set_string(&verbs.win_e_command_key(), "", files).unwrap();
            assert!(!verbs.is_win_e());
            assert_eq!(verbs.exes(), Vec::<String>::new());
            verbs.set_win_e(false).unwrap();
            assert_eq!(reg::get_string(&verbs.win_e_command_key(), "").as_deref(), Some(files));
            // A moved copy's is Glint's own: turning it off removes it.
            reg::set_string(&verbs.win_e_command_key(), "", r#""D:\Old\Glint.exe" "this-pc:""#).unwrap();
            assert_eq!(verbs.exes(), [r"D:\Old\Glint.exe"]);
            verbs.set_win_e(false).unwrap();
            assert_eq!(reg::get_string(&verbs.win_e_command_key(), ""), None);
        }

        /// A folder in %TEMP%, removed afterwards (the links in it, not what they lead to).
        struct TempDir(PathBuf);

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn scoop_copies_run_through_current() {
            // A Scoop root: Scoop itself, and an app with two versions, `current` leading to the newer.
            let root = TempDir(std::env::temp_dir().join(format!("GlintTests-{}-scoop", std::process::id())));
            let apps = root.0.join("apps");
            let app = apps.join("glint");
            std::fs::create_dir_all(apps.join(r"scoop\current")).unwrap();
            for version in ["1.0.0", "1.1.0"] {
                std::fs::create_dir_all(app.join(version)).unwrap();
                std::fs::write(app.join(version).join("Glint.exe"), "").unwrap();
                std::fs::write(app.join(version).join("scoop-install.json"), "{}").unwrap();
            }
            let link = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(app.join("current")).arg(app.join("1.1.0")).output().unwrap();
            assert!(link.status.success(), "{link:?}");
            let current = app.join(r"current\Glint.exe");

            // Started from Scoop's shortcut or from the version's folder: through `current` either way.
            assert_eq!(scoop_current(&current), Some(current.clone()));
            assert_eq!(scoop_current(&app.join(r"1.1.0\Glint.exe")), Some(current.clone()));
            // A version that an update left until `scoop cleanup`.
            assert_eq!(scoop_current(&app.join(r"1.0.0\Glint.exe")), None);
            // Scoop's record of the install, as Scoop before 0.6 named it, and none at all.
            std::fs::rename(app.join(r"1.1.0\scoop-install.json"), app.join(r"1.1.0\install.json")).unwrap();
            assert_eq!(scoop_current(&current), Some(current.clone()));
            std::fs::remove_file(app.join(r"1.1.0\install.json")).unwrap();
            assert_eq!(scoop_current(&current), None);
            std::fs::write(app.join(r"1.1.0\scoop-install.json"), "{}").unwrap();
            // Apps installed for all users: Scoop itself isn't in their root.
            std::fs::remove_dir_all(apps.join("scoop")).unwrap();
            assert_eq!(scoop_current(&current), None);
            // A portable copy.
            assert_eq!(scoop_current(Path::new(r"C:\Tools\Glint\Glint.exe")), None);
        }

        /// The Scoop manifest's uninstaller (scripts/package-manifests.mjs), run as Scoop runs it.
        #[test]
        fn scoop_uninstall_takes_out_what_turning_them_on_added() {
            let manifests = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), r"\..\scripts\package-manifests.mjs")).unwrap();
            let block = manifests.split("const removeExplorerVerbs = String.raw`").nth(1).unwrap().split('`').next().unwrap();
            let scratch = Scratch::new("scoop");
            let verbs = Verbs { exe: PathBuf::from(r"C:\Users\Someone\scoop\apps\glint\current\Glint.exe"), ..verbs_with_neighbours(&scratch) };
            let block = block.replace(r"'Software\Classes'", &format!("'{}'", verbs.classes)).replace("$verb = 'Glint'", "$verb = 'GlintTest'");
            let scoop = |cmd: &str| {
                // Progress (loading modules) would go to stderr.
                let script = format!(r"$ProgressPreference = 'SilentlyContinue'; $cmd = '{cmd}'; $dir = 'C:\Users\Someone\scoop\apps\glint\1.2.3'; Invoke-Command ([scriptblock]::Create($env:GLINT_UNINSTALLER))");
                let run = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).env("GLINT_UNINSTALLER", &block).output().unwrap();
                assert!(run.status.success() && run.stderr.is_empty(), "{run:?}");
            };

            // With another default, which goes back as it was.
            let folders = verbs.folders();
            reg::set(&folders, "", &reg::Value { kind: REG_EXPAND_SZ, data: "OtherApp\0".encode_utf16().flat_map(u16::to_le_bytes).collect() }).unwrap();
            reg::set_string(&format!(r"{folders}\OtherApp\command"), "", r#""C:\Other.exe" "%1""#).unwrap();
            let before = reg::dump(&scratch.0);
            verbs.set(true, true).unwrap();
            let on = reg::dump(&scratch.0);
            scoop("update");
            assert_eq!(reg::dump(&scratch.0), on);
            scoop("uninstall");
            assert_eq!(reg::dump(&scratch.0), before);

            // Entries that run another copy stay.
            let setup = Verbs { exe: PathBuf::from(r"C:\Users\Someone\AppData\Local\Glint\Glint.exe"), ..scratch.verbs() };
            setup.set(true, true).unwrap();
            let on = reg::dump(&scratch.0);
            scoop("uninstall");
            assert_eq!(reg::dump(&scratch.0), on);
            setup.set(false, false).unwrap();

            // Something else took the default since: it stays.
            verbs.set(true, true).unwrap();
            reg::set_string(&folders, "", "ThirdApp").unwrap();
            scoop("uninstall");
            assert_eq!(verbs.status(), (false, false));
            assert_eq!(reg::get_string(&folders, "").as_deref(), Some("ThirdApp"));
            assert!(!reg::exists(&verbs.key(FOLDERS)));
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Integration;
    use tauri::AppHandle;

    pub fn status(_app: &AppHandle) -> Integration {
        Integration::default()
    }

    pub fn set(_app: &AppHandle, _menu: bool, _by_default: bool, _win_e: Option<bool>) -> Result<(), String> {
        Err("File Explorer is Windows'".into())
    }

    pub fn repair(_app: &AppHandle) {}
}
