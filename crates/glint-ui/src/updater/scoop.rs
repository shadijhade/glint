//! Updating a copy Scoop installed, through Scoop: `scoop update glint`, run the way the person would run it, so
//! Scoop's own record of the version (what `scoop status`, `scoop list` and UniGetUI read) stays right. Glint never
//! replaces files in Scoop's folder itself.
//!
//! Scoop refuses to update an app while it runs from its folder, so the update happens once Glint has quit: a
//! PowerShell helper ([`SCRIPT`], written with a job file into Glint's updates folder) waits for this process, has
//! Scoop read its buckets again when the bucket on this PC doesn't know the version yet, runs `scoop update glint`,
//! writes how it went to a result file and starts Glint again when the job says so. The next start reads the result
//! ([`take_outcome`]).
//!
//! A Scoop copy beside the setup program's copy (both installed on one PC) is brought along the same way after the
//! setup's copy updated itself, without waiting for anything or starting anything again ([`SIBLING_JOB`]): otherwise
//! Scoop keeps listing the old version as one to update.

use super::Launcher;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The helper. ASCII only: Windows PowerShell reads a script without a byte order mark in the ANSI code page.
pub const SCRIPT: &str = include_str!("scoop-update.ps1");
/// The helper's file name in the updates folder (written again before each run, so it is always this version's).
pub const SCRIPT_NAME: &str = "scoop-update.ps1";
/// This copy's own update: the job, and how it went (read at the next start).
pub const JOB: &str = "scoop-job.json";
pub const RESULT: &str = "scoop-result.json";
/// A Scoop copy beside this one: nobody reads its result back (it's tried again later while it is still behind).
pub const SIBLING_JOB: &str = "scoop-sibling-job.json";
pub const SIBLING_RESULT: &str = "scoop-sibling-result.json";

/// A copy of Glint that Scoop installed for this user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copy {
    /// The app's name in Scoop, its folder under `apps` ("glint").
    pub name: String,
    /// Scoop's root (`%USERPROFILE%\scoop`, or `%SCOOP%`).
    pub root: PathBuf,
    /// The version installed, as the manifest in `current` says.
    pub version: Option<String>,
    /// The bucket it came from, as Scoop's record of the install says.
    pub bucket: Option<String>,
    /// The program's file name ("Glint.exe").
    pub exe: String,
}

impl Copy {
    /// `<root>\apps\<name>`, which holds a folder per version and the `current` link.
    pub fn app_dir(&self) -> PathBuf {
        self.root.join("apps").join(&self.name)
    }

    /// The program through `current`, which `scoop update` points at each new version.
    pub fn current_exe(&self) -> PathBuf {
        self.app_dir().join("current").join(&self.exe)
    }

    /// Scoop itself: what its `scoop` command runs.
    pub fn scoop(&self) -> PathBuf {
        self.root
            .join("apps")
            .join("scoop")
            .join("current")
            .join("bin")
            .join("scoop.ps1")
    }

    /// The bucket's manifest of the app on this PC, which `scoop update` (without an app) reads again.
    pub fn bucket_manifest(&self) -> Option<PathBuf> {
        let bucket = self.bucket.as_deref()?;
        Some(
            self.root
                .join("buckets")
                .join(bucket)
                .join("bucket")
                .join(format!("{}.json", self.name)),
        )
    }
}

/// This copy, when Scoop installed it for this user: `<root>\apps\<name>\<version>\Glint.exe` (or `current`), the
/// version `current` leads to (not an older one kept until `scoop cleanup`), with Scoop's record of the install
/// beside it, in a root where Scoop itself lives. Apps installed for all users (`--global`) live in a root without
/// Scoop and need an administrator to update, so they aren't one.
pub fn this_copy(exe: &Path) -> Option<Copy> {
    let version_dir = exe.parent()?;
    let app_dir = version_dir.parent()?;
    let apps = app_dir.parent()?;
    if !apps.file_name()?.eq_ignore_ascii_case("apps") {
        return None;
    }
    let current = app_dir.join("current");
    let same = version_dir.file_name()?.eq_ignore_ascii_case("current")
        || matches!(
            (std::fs::canonicalize(version_dir), std::fs::canonicalize(&current)),
            (Ok(a), Ok(b)) if a == b
        );
    if !same {
        return None;
    }
    read(
        apps.parent()?,
        &app_dir.file_name()?.to_string_lossy(),
        &exe.file_name()?.to_string_lossy(),
    )
}

/// Glint installed with Scoop for this user, wherever this copy came from: in `%SCOOP%`, else `%USERPROFILE%\scoop`.
pub fn installed(name: &str, exe: &str) -> Option<Copy> {
    let root = std::env::var_os("SCOOP")
        .map(PathBuf::from)
        .filter(|root| root.is_dir())
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("scoop")))?;
    installed_in(&root, name, exe)
}

/// [`installed`] in a given root.
pub fn installed_in(root: &Path, name: &str, exe: &str) -> Option<Copy> {
    if !root.join("apps").join(name).join("current").join(exe).is_file() {
        return None;
    }
    read(root, name, exe)
}

fn read(root: &Path, name: &str, exe: &str) -> Option<Copy> {
    // Scoop lives in this root (a user's), and Scoop recorded the install.
    if !root.join("apps").join("scoop").join("current").is_dir() {
        return None;
    }
    let current = root.join("apps").join(name).join("current");
    let record = ["scoop-install.json", "install.json"]
        .iter()
        .map(|file| current.join(file))
        .find(|path| path.is_file())?;
    let field = |path: &Path, key: &str| read_json(path).and_then(|value| value.get(key)?.as_str().map(str::to_string));
    let version = ["scoop-manifest.json", "manifest.json"]
        .iter()
        .map(|file| current.join(file))
        .find(|path| path.is_file())
        .and_then(|path| field(&path, "version"));
    Some(Copy {
        name: name.to_string(),
        root: root.to_path_buf(),
        version,
        bucket: field(&record, "bucket"),
        exe: exe.to_string(),
    })
}

/// A JSON file Scoop or the helper wrote (Windows PowerShell writes UTF-8 with a byte order mark).
fn read_json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// What the helper is asked to do (the job file it reads).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub app: String,
    pub app_dir: PathBuf,
    pub scoop: PathBuf,
    pub bucket_manifest: Option<PathBuf>,
    /// The version Scoop should end up with (or past).
    pub target: String,
    /// The process to wait for (this one), 0 for none.
    pub wait_for: u32,
    /// Where the helper says how it went.
    pub result: PathBuf,
    /// What to start once Scoop is done: the program through `current`, none for nothing.
    pub relaunch: Option<PathBuf>,
    pub relaunch_args: Vec<String>,
}

impl Job {
    /// Bring `copy` to `target`, writing the outcome to `result`.
    pub fn new(copy: &Copy, target: &str, result: PathBuf) -> Job {
        Job {
            app: copy.name.clone(),
            app_dir: copy.app_dir(),
            scoop: copy.scoop(),
            bucket_manifest: copy.bucket_manifest(),
            target: target.to_string(),
            wait_for: 0,
            result,
            relaunch: None,
            relaunch_args: Vec::new(),
        }
    }
}

/// How the helper's run went.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Outcome {
    pub target: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub ok: bool,
    /// The last lines Scoop wrote, for the notice when it didn't work.
    pub message: String,
}

/// Windows PowerShell, which Scoop runs on (always there, unlike PowerShell 7).
pub fn powershell() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("powershell.exe"))
}

/// The helper's command line for `script` and `job`.
pub fn arguments(script: &Path, job: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]
        .iter()
        .map(OsString::from)
        .collect();
    args.push(script.into());
    args.push("-Job".into());
    args.push(job.into());
    args
}

/// Writes the helper and `job` (as `job_name`) into `dir`, and starts it with no window, so that it outlives Glint.
pub fn start(dir: &Path, job_name: &str, job: &Job, launcher: &dyn Launcher) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't make Glint's updates folder: {e}"))?;
    let script = dir.join(SCRIPT_NAME);
    std::fs::write(&script, SCRIPT).map_err(|e| format!("Couldn't write the update's helper: {e}"))?;
    let job_path = dir.join(job_name);
    let text = serde_json::to_string_pretty(job).map_err(|e| e.to_string())?;
    std::fs::write(&job_path, text).map_err(|e| format!("Couldn't write the update's helper: {e}"))?;
    let _ = std::fs::remove_file(&job.result);
    launcher.spawn_hidden(&powershell(), &arguments(&script, &job_path), dir)
}

/// How this copy's last update through Scoop went, once: the result is read and removed. A job left without a
/// result (the helper didn't run, or was stopped) reads as a failure.
pub fn take_outcome(dir: &Path) -> Option<Outcome> {
    let result = dir.join(RESULT);
    let job = dir.join(JOB);
    if result.is_file() {
        let outcome = read_json(&result)
            .and_then(|value| serde_json::from_value::<Outcome>(value).ok())
            .unwrap_or_default();
        let _ = std::fs::remove_file(&result);
        let _ = std::fs::remove_file(&job);
        return Some(outcome);
    }
    // The helper removes its job when it is done; one that is still here a while later never finished.
    let stale = std::fs::metadata(&job)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > std::time::Duration::from_secs(10 * 60));
    if stale {
        let _ = std::fs::remove_file(&job);
        return Some(Outcome {
            message: "Scoop's update didn't finish".into(),
            ..Outcome::default()
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Root(PathBuf);

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A Scoop root with Scoop itself and Glint at `version` from bucket `test`, whose manifest on this PC says
    /// `bucket_version`.
    fn root(tag: &str, version: &str, bucket_version: &str) -> Root {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "glint-scoop-test-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let current = root.join(r"apps\glint\current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(root.join(r"apps\scoop\current\bin")).unwrap();
        std::fs::create_dir_all(root.join(r"buckets\test\bucket")).unwrap();
        std::fs::write(current.join("Glint.exe"), b"MZ").unwrap();
        std::fs::write(current.join("scoop-install.json"), "\u{feff}{\"bucket\": \"test\"}").unwrap();
        std::fs::write(
            current.join("scoop-manifest.json"),
            format!("{{\"version\":\"{version}\"}}"),
        )
        .unwrap();
        std::fs::write(
            root.join(r"buckets\test\bucket\glint.json"),
            format!("{{\"version\":\"{bucket_version}\"}}"),
        )
        .unwrap();
        Root(root)
    }

    #[test]
    fn a_copy_is_scoops_through_current_or_the_version_current_leads_to() {
        let root = root("this", "1.2.3", "1.2.3");
        let copy = this_copy(&root.0.join(r"apps\glint\current\Glint.exe")).expect("a Scoop copy");
        assert_eq!(copy.name, "glint");
        assert_eq!(copy.version.as_deref(), Some("1.2.3"));
        assert_eq!(copy.bucket.as_deref(), Some("test"), "read past the byte order mark");
        assert_eq!(copy.exe, "Glint.exe");
        assert_eq!(copy.current_exe(), root.0.join(r"apps\glint\current\Glint.exe"));
        assert_eq!(copy.scoop(), root.0.join(r"apps\scoop\current\bin\scoop.ps1"));
        assert_eq!(
            copy.bucket_manifest(),
            Some(root.0.join(r"buckets\test\bucket\glint.json"))
        );
        // An older version's folder, kept until `scoop cleanup`, isn't the copy Scoop updates.
        let old = root.0.join(r"apps\glint\1.0.0");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("scoop-install.json"), "{}").unwrap();
        assert_eq!(this_copy(&old.join("Glint.exe")), None);
        // Not under `apps`, or a root without Scoop (an install for all users).
        assert_eq!(this_copy(Path::new(r"C:\Tools\Glint\Glint.exe")), None);
        std::fs::remove_dir_all(root.0.join(r"apps\scoop")).unwrap();
        assert_eq!(this_copy(&root.0.join(r"apps\glint\current\Glint.exe")), None);
    }

    #[test]
    fn a_copy_beside_this_one_is_found_in_a_root_and_needs_scoops_record() {
        let root = root("beside", "0.12.0", "0.12.0");
        let copy = installed_in(&root.0, "glint", "Glint.exe").expect("Scoop's copy");
        assert_eq!(copy.version.as_deref(), Some("0.12.0"));
        assert_eq!(installed_in(&root.0, "other", "Glint.exe"), None);
        std::fs::remove_file(root.0.join(r"apps\glint\current\scoop-install.json")).unwrap();
        assert_eq!(
            installed_in(&root.0, "glint", "Glint.exe"),
            None,
            "no record: not Scoop's install"
        );
        // Scoop before 0.6 named its files install.json and manifest.json.
        std::fs::write(root.0.join(r"apps\glint\current\install.json"), "{\"bucket\":\"old\"}").unwrap();
        std::fs::rename(
            root.0.join(r"apps\glint\current\scoop-manifest.json"),
            root.0.join(r"apps\glint\current\manifest.json"),
        )
        .unwrap();
        let copy = installed_in(&root.0, "glint", "Glint.exe").expect("an older Scoop's copy");
        assert_eq!(
            (copy.version.as_deref(), copy.bucket.as_deref()),
            (Some("0.12.0"), Some("old"))
        );
    }

    #[test]
    fn the_helper_is_plain_ascii_and_its_job_names_everything_it_needs() {
        assert!(
            SCRIPT.is_ascii(),
            "Windows PowerShell would read other characters in the ANSI code page"
        );
        let root = root("job", "1.0.0", "1.0.0");
        let copy = installed_in(&root.0, "glint", "Glint.exe").unwrap();
        let job = Job::new(&copy, "2.0.0", root.0.join("result.json"));
        let json = serde_json::to_value(&job).unwrap();
        for key in [
            "app",
            "appDir",
            "scoop",
            "bucketManifest",
            "target",
            "waitFor",
            "result",
            "relaunch",
            "relaunchArgs",
        ] {
            assert!(json.get(key).is_some(), "the helper reads {key}");
        }
        let args = arguments(Path::new(r"C:\x\scoop-update.ps1"), Path::new(r"C:\x\job.json"));
        let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args[..5],
            ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]
        );
        assert_eq!(args[5..], [r"C:\x\scoop-update.ps1", "-Job", r"C:\x\job.json"]);
    }

    /// Scoop for the helper to run: it writes down what it was asked and, for `update <app>`, installs
    /// `installs` (or nothing, when that is empty).
    fn fake_scoop(root: &Root, installs: &str) {
        let script = format!(
            r#"Add-Content -LiteralPath (Join-Path $PSScriptRoot 'calls.txt') -Value ($args -join ' ')
if ($args.Count -ge 2 -and $args[0] -eq 'update') {{
    if ('{installs}') {{
        $manifest = Join-Path $PSScriptRoot '..\..\..\glint\current\scoop-manifest.json'
        Set-Content -LiteralPath $manifest -Value '{{"version":"{installs}"}}'
        Write-Output "Updating 'glint' -> {installs}"
    }} else {{
        Write-Output "ERROR The following instances of `"glint`" are still running."
    }}
}}
exit 0
"#
        );
        std::fs::write(root.0.join(r"apps\scoop\current\bin\scoop.ps1"), script).unwrap();
    }

    /// Runs the real helper on `job`, as Glint starts it, and waits for it.
    fn run_helper(dir: &Path, job: &Job, job_name: &str) -> Outcome {
        std::fs::write(dir.join(SCRIPT_NAME), SCRIPT).unwrap();
        let job_path = dir.join(job_name);
        std::fs::write(&job_path, serde_json::to_string(job).unwrap()).unwrap();
        let status = std::process::Command::new(powershell())
            .args(arguments(&dir.join(SCRIPT_NAME), &job_path))
            .current_dir(dir)
            .status()
            .expect("Windows PowerShell runs");
        assert!(status.success(), "{status:?}");
        assert!(!job_path.exists(), "the helper removes its job when it's done");
        let text = std::fs::read_to_string(&job.result).expect("the helper says how it went");
        serde_json::from_str(text.trim_start_matches('\u{feff}')).expect("its outcome reads")
    }

    fn calls(root: &Root) -> String {
        std::fs::read_to_string(root.0.join(r"apps\scoop\current\bin\calls.txt")).unwrap_or_default()
    }

    #[test]
    fn the_helper_has_scoop_read_its_buckets_then_update_the_app_and_says_so() {
        // The bucket on this PC doesn't know 2.0.0 yet: `scoop update` first, then `scoop update glint`.
        let root = root("helper", "1.0.0", "1.0.0");
        fake_scoop(&root, "2.0.0");
        let copy = installed_in(&root.0, "glint", "Glint.exe").unwrap();
        let job = Job::new(&copy, "2.0.0", root.0.join(RESULT));
        let outcome = run_helper(&root.0, &job, JOB);
        assert!(outcome.ok, "{outcome:?}");
        assert_eq!(
            (outcome.before.as_deref(), outcome.after.as_deref()),
            (Some("1.0.0"), Some("2.0.0"))
        );
        assert_eq!(calls(&root).lines().collect::<Vec<_>>(), ["update", "update glint"]);
        // It went: take_outcome reads it once.
        let taken = take_outcome(&root.0).expect("the outcome");
        assert!(taken.ok);
        assert_eq!(take_outcome(&root.0), None);
    }

    #[test]
    fn the_helper_skips_reading_buckets_that_know_the_version_and_reports_a_refusal() {
        let root = root("refused", "1.0.0", "2.0.0");
        fake_scoop(&root, "");
        let copy = installed_in(&root.0, "glint", "Glint.exe").unwrap();
        let job = Job::new(&copy, "2.0.0", root.0.join(RESULT));
        let outcome = run_helper(&root.0, &job, JOB);
        assert!(!outcome.ok);
        assert_eq!(outcome.after.as_deref(), Some("1.0.0"));
        assert!(outcome.message.contains("still running"), "{}", outcome.message);
        assert_eq!(calls(&root).lines().collect::<Vec<_>>(), ["update glint"]);
    }

    #[test]
    fn the_helper_does_nothing_for_a_copy_already_there() {
        let root = root("there", "2.0.0", "2.0.0");
        fake_scoop(&root, "9.9.9");
        let copy = installed_in(&root.0, "glint", "Glint.exe").unwrap();
        let job = Job::new(&copy, "2.0.0", root.0.join(SIBLING_RESULT));
        let outcome = run_helper(&root.0, &job, SIBLING_JOB);
        assert!(outcome.ok);
        assert_eq!(calls(&root), "", "Scoop isn't run for nothing");
    }

    #[test]
    fn a_job_left_without_a_result_reads_as_a_failure_once_it_is_old() {
        let root = root("stale", "1.0.0", "1.0.0");
        std::fs::write(root.0.join(JOB), "{}").unwrap();
        assert_eq!(take_outcome(&root.0), None, "a fresh job may still be running");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let file = std::fs::File::options().write(true).open(root.0.join(JOB)).unwrap();
        file.set_modified(old).unwrap();
        drop(file);
        let outcome = take_outcome(&root.0).expect("a failure");
        assert!(!outcome.ok);
        assert!(!root.0.join(JOB).exists());
    }
}
