//! The service updating itself, so a new version of Glint brings its service along without asking for
//! administrator approval again.
//!
//! Each release signs the service executable with Glint's update key, the key its app updates are
//! signed with (`tauri signer sign --app-version`, scripts/sign-service.mjs). The signature names the
//! file and the version in its trusted comment, which it covers too. The app hands the running service
//! the executable it ships with, and that signature, over the pipe. The service takes it only when the
//! signature holds with the key built into it (tauri.conf.json's), is for `shadui-index.exe`, and names
//! a version later than its own. Anything less, and anyone allowed on the pipe could have a LocalSystem
//! service run a program of their choosing, or an older one with a flaw fixed since. (The release's
//! installer is signed with the same key, under its own file name.)
//!
//! The bytes checked are the bytes installed: written next to the running executable in Program Files,
//! where only administrators can write, and swapped in for it while it runs (a running executable can be
//! renamed, not replaced). The service then starts the new executable as `finish-update` and stops.
//! That waits for it to stop (it saves its snapshots), starts the service again from the new executable,
//! and puts the old one back if it doesn't come up.

use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};

/// The service executable's signature, next to it in the app's folder: a .sig file as `tauri signer
/// sign` writes it (base64 of minisign's signature).
pub const SIGNATURE_FILE: &str = "shadui-index.exe.sig";

/// The largest executable the service reads from the pipe (a real one is under 2 MB).
pub const MAX_SIZE: u64 = 16 << 20;

/// The app's configuration, for its update key.
const TAURI_CONF: &str = include_str!("../../tauri.conf.json");

/// The public half of the update key, as tauri.conf.json has it (base64 of minisign's public key);
/// None in a build without one, which never updates itself.
pub fn update_key() -> Option<String> {
    let conf: serde_json::Value = serde_json::from_str(TAURI_CONF).ok()?;
    let key = conf.pointer("/plugins/updater/pubkey")?.as_str()?.trim();
    (!key.is_empty()).then(|| key.to_string())
}

fn base64_text(value: &str, what: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(value.trim()).map_err(|_| format!("{what} isn't base64"))?;
    String::from_utf8(bytes).map_err(|_| format!("{what} isn't text"))
}

/// What a signature that holds says in its trusted comment: the file it was made for, and the version.
#[derive(Debug, Clone, PartialEq)]
pub struct Signed {
    pub file: String,
    pub version: Option<String>,
}

/// Checks `data` against `signature` with `key` (as a .sig file and tauri.conf.json hold them); what the
/// signature says, once it holds.
pub fn check(data: &[u8], signature: &str, key: &str) -> Result<Signed, String> {
    let key = PublicKey::decode(&base64_text(key, "the update key")?).map_err(|e| format!("the update key can't be read: {e}"))?;
    let signature = Signature::decode(&base64_text(signature, "the signature")?).map_err(|e| format!("the signature can't be read: {e}"))?;
    key.verify(data, &signature, false).map_err(|e| format!("the signature doesn't hold: {e}"))?;
    let field = |name: &str| signature.trusted_comment().split('\t').find_map(|f| f.strip_prefix(name)?.strip_prefix(':')).map(str::to_string);
    Ok(Signed { file: field("file").unwrap_or_default(), version: field("version") })
}

/// A version's three numbers (`0.10.0`); None for anything else.
fn numbers(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.split('.');
    let mut out = [0; 3];
    for n in &mut out {
        let part = parts.next().filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))?;
        *n = part.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// Whether `version` is later than `than`, both three numbers; false when either isn't.
pub fn newer(version: &str, than: &str) -> bool {
    matches!((numbers(version), numbers(than)), (Some(a), Some(b)) if a > b)
}

/// Checks a service executable sent to replace the running one, whose version is `running`: signed with
/// `key`, for the service's executable, from a later version. That version, if so.
pub fn verify(exe: &[u8], signature: &str, key: &str, running: &str) -> Result<String, String> {
    let signed = check(exe, signature, key)?;
    if signed.file != crate::SERVICE_EXE {
        return Err(format!("its signature is for \"{}\", not {}", signed.file, crate::SERVICE_EXE));
    }
    let version = signed.version.ok_or("its signature doesn't say which version it is")?;
    if !newer(&version, running) {
        return Err(format!("it's version {version}, not later than {running}"));
    }
    Ok(version)
}

#[cfg(windows)]
pub use installing::{finish, Updater};

#[cfg(windows)]
mod installing {
    use crate::engine::{Log, Stop};
    use crate::win::{os_error, wide};
    use std::fs;
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};

    /// What the installed service needs to update itself: the key it trusts, its folder, how it starts
    /// `finish-update`, and how it stops.
    pub struct Updater {
        key: Option<String>,
        dir: PathBuf,
        finisher: fn(&Path) -> io::Result<()>,
        pub stop: Arc<Stop>,
        under_way: AtomicBool,
    }

    impl Updater {
        /// For the service installed in `dir`, trusting the update key it was built with.
        pub fn new(dir: PathBuf, stop: Arc<Stop>) -> Updater {
            Updater { key: super::update_key(), dir, finisher: start_finisher, stop, under_way: AtomicBool::new(false) }
        }

        /// One that trusts `key` and calls `finisher` in place of starting `finish-update`: for tests.
        #[cfg(test)]
        pub(crate) fn for_test(key: &str, dir: PathBuf, finisher: fn(&Path) -> io::Result<()>) -> Updater {
            Updater { key: Some(key.into()), dir, finisher, stop: Arc::default(), under_way: AtomicBool::new(false) }
        }

        /// Checks `exe` and, when it's a later service signed with the update key, puts it in place of
        /// the running one and starts `finish-update`: this service must stop then. Its version if so.
        pub fn take(&self, exe: &[u8], signature: &str) -> Result<String, String> {
            let key = self.key.as_deref().ok_or("this build of the service has no update key")?;
            let version = super::verify(exe, signature, key, crate::VERSION)?;
            if self.under_way.swap(true, Ordering::SeqCst) {
                return Err("an update is already under way".into());
            }
            install(exe, &self.dir, self.finisher).map_err(|e| {
                self.under_way.store(false, Ordering::SeqCst);
                format!("couldn't put it in place: {e}")
            })?;
            Ok(version)
        }
    }

    fn sibling(dir: &Path, suffix: &str) -> PathBuf {
        dir.join(format!("{}{suffix}", crate::SERVICE_EXE))
    }

    fn rename(from: &Path, to: &Path) -> io::Result<()> {
        unsafe { MoveFileExW(PCWSTR(wide(from).as_ptr()), PCWSTR(wide(to).as_ptr()), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) }.map_err(os_error)
    }

    /// Writes `exe` next to the installed executable in `dir` and swaps the two, keeping the running
    /// one aside as .old; then `finisher` starts the new one. Whatever fails, what was there is put back.
    pub(crate) fn install(exe: &[u8], dir: &Path, finisher: impl FnOnce(&Path) -> io::Result<()>) -> io::Result<()> {
        let (installed, new, old) = (dir.join(crate::SERVICE_EXE), sibling(dir, ".new"), sibling(dir, ".old"));
        let written = fs::File::create(&new).and_then(|mut file| {
            file.write_all(exe)?;
            file.sync_all()
        });
        if let Err(e) = written {
            let _ = fs::remove_file(&new);
            return Err(e);
        }
        if let Err(e) = rename(&installed, &old) {
            let _ = fs::remove_file(&new);
            return Err(e);
        }
        if let Err(e) = rename(&new, &installed) {
            let _ = rename(&old, &installed);
            let _ = fs::remove_file(&new);
            return Err(e);
        }
        if let Err(e) = finisher(&installed) {
            let _ = rename(&installed, &new);
            let _ = rename(&old, &installed);
            let _ = fs::remove_file(&new);
            return Err(e);
        }
        Ok(())
    }

    /// Starts `finish-update` from the new executable, told this process's ID.
    fn start_finisher(installed: &Path) -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new(installed)
            .args(["finish-update", &std::process::id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(drop)
    }

    /// Deletes a file whose process may still be letting go of it, or at the next restart.
    fn remove_soon(path: &Path) {
        for _ in 0..50 {
            match fs::remove_file(path) {
                Ok(()) => return,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return,
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        }
        unsafe {
            let _ = MoveFileExW(PCWSTR(wide(path).as_ptr()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT);
        }
    }

    /// Once the old service has stopped: `first` starts the service from the new executable. If that
    /// fails, the old executable goes back in place (the new one kept as .failed, to look at) and `again`
    /// starts it. Err when the new one didn't take.
    pub(crate) fn finish_in(dir: &Path, first: impl FnOnce() -> io::Result<()>, again: impl FnOnce() -> io::Result<()>, note: &dyn Fn(&str)) -> io::Result<()> {
        let (installed, old) = (dir.join(crate::SERVICE_EXE), sibling(dir, ".old"));
        let Err(e) = first() else {
            remove_soon(&old);
            return Ok(());
        };
        note(&format!("the updated service didn't start ({e}): going back to the one before"));
        if old.exists() {
            rename(&installed, &sibling(dir, ".failed"))?;
            rename(&old, &installed)?;
        }
        again()?;
        Err(e)
    }

    /// `finish-update PID`, run from the new executable by the service it replaces (process `pid`):
    /// starts the service again once that has stopped, or the one before if the new one doesn't come up.
    /// 0 when the new one runs.
    pub fn finish(pid: u32) -> i32 {
        let log = crate::win::data_dir().ok().map(|dir| Log::open(&dir.join("service.log"), false));
        let note = |text: &str| {
            if let Some(log) = &log {
                log.line(text);
            }
        };
        let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) else { return 1 };
        match finish_in(&dir, || crate::install::start_after(pid), crate::install::start_again, &note) {
            Ok(()) => {
                note(&format!("updated: {} {} runs", crate::SERVICE_NAME, crate::VERSION));
                0
            }
            Err(e) => {
                note(&format!("the update to {} didn't take: {e}", crate::VERSION));
                1
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::cell::Cell;
        use std::process::{Command, Stdio};

        fn scratch(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("shadui-index-update-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        #[test]
        fn swaps_the_executable_and_puts_the_old_one_back_when_needed() {
            let dir = scratch("swap");
            let (installed, new, old, failed) = (dir.join(crate::SERVICE_EXE), sibling(&dir, ".new"), sibling(&dir, ".old"), sibling(&dir, ".failed"));
            fs::write(&installed, b"0.9").unwrap();

            // finish-update couldn't start: everything as it was.
            assert!(install(b"1.0", &dir, |_| Err(io::Error::other("no"))).is_err());
            assert_eq!(fs::read(&installed).unwrap(), b"0.9");
            assert!(!new.exists() && !old.exists());

            // In place, with the old one aside, and finish-update started from where the service runs.
            let mut started = None;
            install(b"1.0", &dir, |exe| {
                started = Some(exe.to_path_buf());
                Ok(())
            })
            .unwrap();
            assert_eq!((fs::read(&installed).unwrap(), fs::read(&old).unwrap()), (b"1.0".to_vec(), b"0.9".to_vec()));
            assert_eq!(started.as_ref(), Some(&installed));
            assert!(!new.exists());

            // The new one runs: the old one goes.
            finish_in(&dir, || Ok(()), || unreachable!(), &|_| {}).unwrap();
            assert!(!old.exists());

            // A later one that doesn't come up: the one before is back, and started.
            install(b"1.1", &dir, |_| Ok(())).unwrap();
            let (again, notes) = (Cell::new(0), Cell::new(0));
            let start_again = || {
                again.set(again.get() + 1);
                Ok(())
            };
            let result = finish_in(&dir, || Err(io::Error::other("crashed")), start_again, &|_| notes.set(notes.get() + 1));
            assert!(result.is_err());
            assert_eq!((fs::read(&installed).unwrap(), fs::read(&failed).unwrap()), (b"1.0".to_vec(), b"1.1".to_vec()));
            assert_eq!((again.get(), notes.get()), (1, 1));
            assert!(!old.exists());
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn the_running_executable_is_swapped_out_from_under_it() {
            // What the service does to itself: a program that's running is renamed aside, and the new
            // one takes its name.
            let dir = scratch("running");
            let installed = dir.join(crate::SERVICE_EXE);
            let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            fs::copy(Path::new(&system).join(r"System32\PING.EXE"), &installed).unwrap();
            let mut running = Command::new(&installed).args(["-n", "30", "127.0.0.1"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
            let result = install(b"new", &dir, |_| Ok(()));
            let _ = running.kill();
            let _ = running.wait();
            result.unwrap();
            assert_eq!(fs::read(&installed).unwrap(), b"new");
            assert!(sibling(&dir, ".old").exists());
            let _ = fs::remove_dir_all(&dir);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A key made for these tests with `tauri signer generate` (used for nothing else), and what it signed.
    pub(crate) const KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDRENTRERjJGRjdBMzM3NEQKUldSTk42UDNMOTlVVGFaVkV0YkNpaWQ5WEhreStKNTBhRTBTWlZ1b21qUVlSS0E1dWtTbEttRFQK";
    pub(crate) const EXE: &[u8] = b"MZ a test build of the instant search service";
    /// `tauri signer sign --app-version 999.0.0 shadui-index.exe`
    pub(crate) const SIGNED: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSTk42UDNMOTlVVGN6c1k4UFRqbHFyZ3FsWGUzTWxJeDZhQ3IzWW5jMlFyZGMxZGxacU1RNEcrOGRwU1VVc0FZTVl3RThrV1ZRUDNyNlpWM1ZqRkRHN0kxbkJLT1gvZGdnPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0MDgxNzgyCWZpbGU6c2hhZHVpLWluZGV4LmV4ZQl2ZXJzaW9uOjk5OS4wLjAKaEx6eXQyRW9sMjg1eFJNUmNZcWY0cTFieUZkbnRpeGpSSFpqcmxGMVRzZHVHL1lzRHVoSHRhZTlnelc1bE90WDR2QTdaS2R4YmFFbDZhUGFGbERNREE9PQo=";
    /// The same bytes signed as a release installer is: `--app-version 999.0.0 Glint_0.99.0_x64-setup.exe`.
    const INSTALLER: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSTk42UDNMOTlVVFkyL3IyQTVmNWE5SFp1UGlmQjFOUENUVmFoUEF2WGtlblZyTFVUMmxlN0l2cWRhSVZoTGkxVGlrWmI5Nno5TDQrTmljelB1dHMycDRBYVRlcmVwc0FVPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0MDgxNzgyCWZpbGU6R2xpbnRfMC45OS4wX3g2NC1zZXR1cC5leGUJdmVyc2lvbjo5OTkuMC4wCkx1Z2RSUVRRVU5mL2lGNXVQVDhSMW90Wmsxc3lQc0VKUStQOVNWdFJPTzU4N0hlSGRwajVKLzc3OSt0RTd1NWhhMTludmZRb2NSdmJwUlhoMkNyQURRPT0K";
    /// Signed without a version.
    const UNVERSIONED: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSTk42UDNMOTlVVGZLcVZEZVhqd1J2S2Q3VmVDZ2NjUnFZZ0hZNUVSNTlWOHUrTGorcjd6bmRVa2dtOU1mQVpIMjVWTlFIdTVTRVBHc2VRWVFvSm9GWHlkY20yVnZZY1FrPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0MDgxNzgzCWZpbGU6c2hhZHVpLWluZGV4LmV4ZQpna1llMEtZRFVvdEltVi9WL0tGcnVxblB2T250Z3lUd1pkL0hDVlRtL3dhYjlhNGpQT1oybVVTWlBnVXRNbTU5VWE3dmY1UVhOa2ZDQTNVTnlxQUdCdz09Cg==";

    #[test]
    fn takes_a_later_service_signed_with_the_key() {
        assert_eq!(check(EXE, SIGNED, KEY), Ok(Signed { file: "shadui-index.exe".into(), version: Some("999.0.0".into()) }));
        assert_eq!(verify(EXE, SIGNED, KEY, "0.9.0"), Ok("999.0.0".into()));
        assert_eq!(verify(EXE, SIGNED, KEY, "998.99.99"), Ok("999.0.0".into()));
    }

    #[test]
    fn refuses_anything_else() {
        let refused = |exe: &[u8], signature: &str, key: &str, running: &str| verify(exe, signature, key, running).unwrap_err();
        assert!(refused(EXE, SIGNED, KEY, "999.0.0").contains("not later than 999.0.0"));
        assert!(refused(EXE, SIGNED, KEY, "1000.0.0").contains("not later than"));
        let mut changed = EXE.to_vec();
        changed[3] ^= 1;
        assert!(refused(&changed, SIGNED, KEY, "0.9.0").contains("doesn't hold"));
        assert!(refused(&EXE[1..], SIGNED, KEY, "0.9.0").contains("doesn't hold"));
        assert!(refused(EXE, INSTALLER, KEY, "0.9.0").contains("Glint_0.99.0_x64-setup.exe"));
        assert!(refused(EXE, UNVERSIONED, KEY, "0.9.0").contains("which version"));
        // Glint's own key didn't make these.
        assert!(refused(EXE, SIGNED, &update_key().unwrap(), "0.9.0").contains("doesn't hold"));
        assert!(refused(EXE, "not a signature", KEY, "0.9.0").contains("signature"));
        assert!(refused(EXE, "", KEY, "0.9.0").contains("signature"));
        assert!(refused(EXE, SIGNED, "", "0.9.0").contains("key"));
    }

    #[test]
    fn compares_versions_as_numbers() {
        assert!(newer("0.10.0", "0.9.0"));
        assert!(newer("1.0.0", "0.99.99"));
        assert!(newer("0.9.1", "0.9.0"));
        assert!(!newer("0.9.0", "0.9.0"));
        assert!(!newer("0.8.9", "0.9.0"));
        for odd in ["", "1", "1.2", "1.2.3.4", "1.2.x", "1.2.+3", "1..2", "-1.0.0", "0.10.0-beta", " 1.0.0"] {
            assert!(!newer(odd, "0.0.1"), "{odd}");
            assert!(!newer("9.9.9", odd), "{odd}");
        }
    }

    #[test]
    fn trusts_the_key_the_apps_updates_are_signed_with() {
        let key = update_key().expect("tauri.conf.json has plugins.updater.pubkey");
        assert!(PublicKey::decode(&base64_text(&key, "the key").unwrap()).is_ok());
    }
}
