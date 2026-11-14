//! Back up: folders backed up by restic (a tool Glint installs) to a USB drive, a network folder or any
//! cloud rclone reaches (restic's `rclone:` backend, run through Glint's rclone), encrypted (restic's own
//! AES-256), deduplicated, each backup adding only what changed. A backup's password is kept in Windows
//! Credential Manager; without it the backup can't be read, which the dialog says plainly. Backups run
//! when asked, or daily or weekly while Glint runs, keeping 7 daily, 4 weekly and 12 monthly ones, with a
//! check of the repository's structure once a month; their snapshots show in Previous versions beside
//! Windows' shadow copies, to open, compare or restore.
//!
//! Files an app holds open (Outlook's PST) can't be read without a shadow copy, which needs administrator
//! rights: a backup goes on without them and lists them as skipped.

use crate::fsx::blocking;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use crate::host::ipc::Channel;
use crate::host::{AppHandle, Event, Window};

/// What backs a folder up, and how it went last time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub id: String,
    /// The folder backed up.
    pub source: String,
    /// Where: a folder ("E:\Backups") or a cloud ("rclone:drive:Backups").
    pub repository: String,
    pub schedule: Schedule,
    #[serde(default)]
    pub last: Option<LastRun>,
    /// When the repository's structure was last checked (ms since 1970).
    #[serde(default)]
    pub checked: i64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Schedule {
    Manual,
    Daily,
    Weekly,
}

/// A scheduled backup that ran: its folder, and how it went or why it couldn't.
#[derive(Clone, Debug)]
pub struct Finished {
    pub source: String,
    pub result: Result<LastRun, String>,
}

/// How a backup went.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    /// When it finished (ms since 1970).
    pub at: i64,
    pub ok: bool,
    pub snapshot: String,
    pub files_new: u64,
    pub files_changed: u64,
    pub bytes_added: u64,
    pub total_files: u64,
    pub total_bytes: u64,
    /// Files it couldn't read (an app holds them open), the first few.
    pub skipped: Vec<String>,
    pub skipped_count: u64,
    pub error: Option<String>,
    pub elapsed_ms: f64,
}

/// How far a backup is.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackupProgress {
    pub stage: String,
    pub fraction: f64,
    pub files_done: u64,
    pub total_files: u64,
    pub bytes_done: u64,
    pub total_bytes: u64,
    pub current: String,
}

static PLANS: Mutex<Option<Vec<Plan>>> = Mutex::new(None);
static RUNNING: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static STOP: AtomicBool = AtomicBool::new(false);

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn plans_file() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("backups.json"))
}

fn plans() -> Vec<Plan> {
    let mut known = PLANS.lock().unwrap_or_else(|e| e.into_inner());
    if known.is_none() {
        *known = Some(plans_file().and_then(|f| fs::read_to_string(f).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    }
    known.clone().unwrap_or_default()
}

fn save(list: Vec<Plan>) -> Result<(), String> {
    let file = plans_file().ok_or("Glint's data folder isn't available")?;
    let text = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    let temp = file.with_extension("json.new");
    fs::write(&temp, text).map_err(|e| e.to_string())?;
    fs::rename(&temp, &file).map_err(|e| e.to_string())?;
    *PLANS.lock().unwrap_or_else(|e| e.into_inner()) = Some(list);
    Ok(())
}

fn update(id: &str, change: impl FnOnce(&mut Plan)) -> Result<Plan, String> {
    let mut list = plans();
    let plan = list.iter_mut().find(|p| p.id == id).ok_or("That backup isn't set up any more")?;
    change(plan);
    let changed = plan.clone();
    save(list)?;
    Ok(changed)
}

/// The credential a repository's password is kept under.
fn credential(repository: &str) -> String {
    format!("Glint/backup/{}", repository.to_lowercase())
}

/// A folder on one of this PC's drives as restic names it inside a backup: C:\Users\me → /C/Users/me.
fn restic_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }
    let rest = path[2..].replace('\\', "/");
    let rest = rest.trim_end_matches('/');
    Some(format!("/{}{}", path[..1].to_ascii_uppercase(), if rest.starts_with('/') || rest.is_empty() { rest.to_string() } else { format!("/{rest}") }))
}

/// Whether `path` is `folder` or inside it.
fn within(path: &str, folder: &str) -> bool {
    let (path, folder) = (path.trim_end_matches('\\').to_lowercase(), folder.trim_end_matches('\\').to_lowercase());
    path == folder || path.starts_with(&format!("{folder}\\"))
}

/// restic, for `repository`, with its password from Credential Manager (or `password`), Glint's own cache
/// folder, and rclone (with its configuration's password) for a cloud.
fn restic(repository: &str, password: Option<&str>) -> Result<Command, String> {
    let mut command = crate::tools::command("restic")?;
    let password = match password {
        Some(p) => p.to_string(),
        None => crate::remote::secrets::read_target(&credential(repository)).ok_or("This backup's password isn't kept on this PC: set the backup up again with it")?,
    };
    for variable in ["RESTIC_REPOSITORY", "RESTIC_REPOSITORY_FILE", "RESTIC_PASSWORD_FILE", "RESTIC_PASSWORD_COMMAND"] {
        command.env_remove(variable);
    }
    command.env("RESTIC_PASSWORD", password).env("RESTIC_PROGRESS_FPS", "4");
    command.arg("-r").arg(repository);
    if let Some(data) = crate::tools::data_dir() {
        command.arg("--cache-dir").arg(data.join("restic-cache"));
    }
    if repository.starts_with("rclone:") {
        if let Some((rclone, _)) = crate::proc::locate("rclone") {
            command.arg("-o").arg(format!("rclone.program={}", rclone.display()));
        }
        if let Some(pass) = crate::rclone::config_pass() {
            command.env("RCLONE_CONFIG_PASS", pass);
        }
    }
    Ok(command)
}

/// Runs restic to its end: its exit code, standard output and error output.
fn finish(mut command: Command, stop: &AtomicBool, mut on_line: impl FnMut(&str)) -> Result<(i32, String, String), String> {
    use std::io::{BufRead, BufReader, Read};
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    crate::proc::no_window(&mut command);
    let mut child = command.spawn().map_err(|e| format!("Couldn't start restic: {e}"))?;
    let job = crate::proc::Job::ending().ok();
    if let Some(job) = &job {
        let _ = job.assign(&child);
    }
    let mut stderr = child.stderr.take().ok_or("restic has no output")?;
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let stdout = child.stdout.take().ok_or("restic has no output")?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut out = String::new();
    let status = loop {
        while let Ok(line) = rx.try_recv() {
            on_line(&line);
            out.push_str(&line);
            out.push('\n');
        }
        if stop.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if let Ok(line) = rx.recv_timeout(Duration::from_millis(50)) {
                    on_line(&line);
                    out.push_str(&line);
                    out.push('\n');
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let _ = reader.join();
    for line in rx.try_iter() {
        on_line(&line);
        out.push_str(&line);
        out.push('\n');
    }
    Ok((status.code().unwrap_or(-1), out, errors.join().unwrap_or_default()))
}

/// restic's error in plain words: its last line, without its "Fatal:" prefix.
fn said(stderr: &str) -> String {
    let line = stderr.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("restic stopped");
    line.trim_start_matches("Fatal: ").to_string()
}

pub async fn backup_plans() -> Result<Vec<Plan>, String> {
    blocking(|| Ok(plans())).await
}

/// Sets a folder's backup up (or changes it): the password kept, the repository made if there isn't one
/// there yet, or opened with the password if there is.
pub async fn backup_setup(source: String, repository: String, password: String, schedule: Schedule) -> Result<Plan, String> {
    blocking(move || {
        let source = source.trim_end_matches('\\').to_string();
        let repository = repository.trim().trim_end_matches('\\').to_string();
        if restic_path(&source).is_none() || !Path::new(&source).is_dir() {
            return Err("Back up works on folders on this PC's drives".into());
        }
        if !repository.starts_with("rclone:") && within(&repository, &source) {
            return Err("The backup can't go inside the folder it backs up".into());
        }
        if password.chars().count() < 8 {
            return Err("Use a password of at least 8 characters".into());
        }
        let stop = AtomicBool::new(false);
        let mut probe = restic(&repository, Some(&password))?;
        probe.args(["cat", "config", "--json"]);
        let (code, _, stderr) = finish(probe, &stop, |_| {})?;
        match code {
            0 => {}
            // 10: no repository there yet.
            10 => {
                if !repository.starts_with("rclone:") {
                    fs::create_dir_all(&repository).map_err(|e| format!("Couldn't make the backup's folder: {e}"))?;
                }
                let mut init = restic(&repository, Some(&password))?;
                init.arg("init");
                let (code, _, stderr) = finish(init, &stop, |_| {})?;
                if code != 0 {
                    return Err(said(&stderr));
                }
            }
            12 => return Err("There's a backup there already, and this password doesn't open it".into()),
            _ => return Err(said(&stderr)),
        }
        crate::remote::secrets::write_target(&credential(&repository), &password)?;
        let mut list = plans();
        let existing = list.iter().position(|p| within(&p.source, &source) && within(&source, &p.source));
        let plan = Plan {
            id: existing.map(|i| list[i].id.clone()).unwrap_or_else(|| format!("{:x}", now_ms())),
            source,
            repository,
            schedule,
            last: existing.and_then(|i| list[i].last.clone()),
            checked: existing.map_or(0, |i| list[i].checked),
        };
        match existing {
            Some(i) => list[i] = plan.clone(),
            None => list.push(plan.clone()),
        }
        save(list)?;
        ensure_scheduler();
        Ok(plan)
    })
    .await
}

/// Stops backing a folder up: its plan goes; the backups already made stay where they are.
pub async fn backup_remove(id: String) -> Result<(), String> {
    blocking(move || {
        let mut list = plans();
        let Some(at) = list.iter().position(|p| p.id == id) else { return Ok(()) };
        let gone = list.remove(at);
        // The password goes too unless another folder backs up to the same place.
        if !list.iter().any(|p| p.repository.eq_ignore_ascii_case(&gone.repository)) {
            let _ = crate::remote::secrets::delete_target(&credential(&gone.repository));
        }
        save(list)
    })
    .await
}

/// Changes when a folder is backed up.
pub async fn backup_schedule(id: String, schedule: Schedule) -> Result<Plan, String> {
    blocking(move || {
        let plan = update(&id, |p| p.schedule = schedule)?;
        ensure_scheduler();
        Ok(plan)
    })
    .await
}

pub fn backup_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

/// Backs a plan's folder up now, with progress.
pub async fn backup_run(id: String, on_progress: Channel<BackupProgress>) -> Result<LastRun, String> {
    STOP.store(false, Ordering::SeqCst);
    blocking(move || run(&id, &|p| drop(on_progress.send(p)))).await
}

fn run(id: &str, progress: &dyn Fn(BackupProgress)) -> Result<LastRun, String> {
    let plan = plans().into_iter().find(|p| p.id == id).ok_or("That backup isn't set up any more")?;
    {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if !running.get_or_insert_with(HashSet::new).insert(plan.id.clone()) {
            return Err("It's backing up already".into());
        }
    }
    let result = run_plan(&plan, progress);
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).remove(&plan.id);
    let last = match result {
        Ok(last) => last,
        Err(e) if e == "cancelled" => return Err(e),
        Err(e) => LastRun { at: now_ms(), ok: false, error: Some(e), ..LastRun::default() },
    };
    update(&plan.id, |p| p.last = Some(last.clone()))?;
    Ok(last)
}

fn run_plan(plan: &Plan, progress: &dyn Fn(BackupProgress)) -> Result<LastRun, String> {
    let started = Instant::now();
    progress(BackupProgress { stage: "Starting".into(), ..BackupProgress::default() });
    let mut backup = restic(&plan.repository, None)?;
    backup.args(["backup", "--json", "--exclude-caches", "--tag", "glint", "--"]).arg(&plan.source);
    let mut last = LastRun::default();
    let (code, _, stderr) = finish(backup, &STOP, |line| {
        let Ok(said) = serde_json::from_str::<serde_json::Value>(line) else { return };
        let number = |key: &str| said[key].as_u64().unwrap_or(0);
        match said["message_type"].as_str() {
            Some("status") => progress(BackupProgress {
                stage: "Backing up".into(),
                fraction: said["percent_done"].as_f64().unwrap_or(0.0),
                files_done: number("files_done"),
                total_files: number("total_files"),
                bytes_done: number("bytes_done"),
                total_bytes: number("total_bytes"),
                current: said["current_files"][0].as_str().unwrap_or_default().to_string(),
            }),
            Some("error") => {
                last.skipped_count += 1;
                if last.skipped.len() < 20 {
                    last.skipped.push(said["item"].as_str().unwrap_or_default().to_string());
                }
            }
            Some("summary") => {
                last.files_new = number("files_new");
                last.files_changed = number("files_changed");
                last.bytes_added = number("data_added");
                last.total_files = number("total_files_processed");
                last.total_bytes = number("total_bytes_processed");
                last.snapshot = said["snapshot_id"].as_str().unwrap_or_default().chars().take(8).collect();
            }
            _ => {}
        }
    })?;
    // 3: made, but some files couldn't be read.
    if code != 0 && code != 3 {
        return Err(said(&stderr));
    }
    last.ok = true;
    // Old backups thinned out: 7 daily, 4 weekly and 12 monthly kept.
    progress(BackupProgress { stage: "Tidying older backups".into(), fraction: 1.0, ..BackupProgress::default() });
    let mut forget = restic(&plan.repository, None)?;
    forget.args(["forget", "--prune", "--tag", "glint", "--keep-daily", "7", "--keep-weekly", "4", "--keep-monthly", "12", "--path"]).arg(&plan.source);
    if let Ok((code, _, stderr)) = finish(forget, &STOP, |_| {}) {
        if code != 0 {
            last.error = Some(format!("Older backups weren't thinned out: {}", said(&stderr)));
        }
    }
    // The repository's structure checked once a month.
    if now_ms() - plan.checked > 30 * 24 * 3600 * 1000 {
        progress(BackupProgress { stage: "Checking the backups".into(), fraction: 1.0, ..BackupProgress::default() });
        let mut check = restic(&plan.repository, None)?;
        check.arg("check");
        match finish(check, &STOP, |_| {}) {
            Ok((0, _, _)) => drop(update(&plan.id, |p| p.checked = now_ms())),
            Ok((_, _, stderr)) => last.error = Some(format!("The check of the backups found a problem: {}", said(&stderr))),
            Err(_) => {}
        }
    }
    last.at = now_ms();
    last.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(last)
}

/// A backup that holds a path: its plan, the snapshot, and when it was made.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BackupVersion {
    pub plan: String,
    pub repository: String,
    pub snapshot: String,
    /// When it was made (ms since 1970).
    pub time: i64,
    /// The path is a folder (as it is now).
    pub dir: bool,
}

/// The backups made of the folders `path` is in, newest first (restic's snapshots of each plan).
pub async fn backup_versions(path: String) -> Result<Vec<BackupVersion>, String> {
    blocking(move || {
        let mut found = Vec::new();
        let stop = AtomicBool::new(false);
        let dir = Path::new(&path).is_dir();
        for plan in plans().into_iter().filter(|p| within(&path, &p.source)) {
            let mut list = restic(&plan.repository, None)?;
            list.args(["snapshots", "--json", "--tag", "glint", "--path"]).arg(&plan.source);
            let (code, out, stderr) = finish(list, &stop, |_| {})?;
            if code != 0 {
                return Err(format!("The backups at {} couldn't be read: {}", plan.repository, said(&stderr)));
            }
            found.extend(snapshots_of(&out).into_iter().map(|(snapshot, time)| BackupVersion { plan: plan.id.clone(), repository: plan.repository.clone(), snapshot, time, dir }));
        }
        found.sort_by_key(|v| std::cmp::Reverse(v.time));
        Ok(found)
    })
    .await
}

/// `restic snapshots --json` as (short id, ms since 1970).
fn snapshots_of(json: &str) -> Vec<(String, i64)> {
    let list: Vec<serde_json::Value> = serde_json::from_str(json.trim()).unwrap_or_default();
    list.iter()
        .filter_map(|s| {
            let id = s["short_id"].as_str().or_else(|| s["id"].as_str())?.chars().take(8).collect::<String>();
            let time = chrono_ms(s["time"].as_str()?)?;
            Some((id, time))
        })
        .collect()
}

/// RFC 3339 ("2026-11-09T20:15:31.123456789+02:00") as ms since 1970.
fn chrono_ms(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once('T')?;
    let mut d = date.split('-').map(|n| n.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let zone_at = rest.find(['Z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(zone_at);
    let mut c = clock.split(':');
    let (h, min) = (c.next()?.parse::<i64>().ok()?, c.next()?.parse::<i64>().ok()?);
    let secs: f64 = c.next().unwrap_or("0").parse().ok()?;
    let offset = if zone.is_empty() || zone == "Z" {
        0
    } else {
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let (zh, zm) = zone[1..].split_once(':').unwrap_or((&zone[1..], "0"));
        sign * (zh.parse::<i64>().ok()? * 60 + zm.parse::<i64>().ok()?)
    };
    // Days from 1970 by the civil calendar.
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400 + h * 3600 + min * 60 - offset * 60) as f64 * 1000.0 + secs * 1000.0) as i64)
}

/// Where a path from a backup is made, for Open: a folder of its own in the temp folder.
fn temp_for(snapshot: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("Glint backups").join(format!("{snapshot}-{:x}", now_ms()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Takes `path` (a file, or with `dir` a folder) as it was in a backup out into `into`: where it is there.
fn take_out(plan: &Plan, snapshot: &str, path: &str, dir: bool, into: &Path) -> Result<PathBuf, String> {
    let inside = restic_path(path).ok_or("Only folders on this PC's drives are backed up")?;
    let name = Path::new(path).file_name().ok_or("It has no name")?.to_string_lossy().into_owned();
    let stop = AtomicBool::new(false);
    let mut restore = restic(&plan.repository, None)?;
    let target = if dir { into.join(&name) } else { into.to_path_buf() };
    if dir {
        restore.args(["restore", &format!("{snapshot}:{inside}"), "--target"]).arg(&target);
    } else {
        let parent = inside.rsplit_once('/').map(|(p, _)| if p.is_empty() { "/" } else { p }).unwrap_or("/").to_string();
        restore.args(["restore", &format!("{snapshot}:{parent}"), "--target"]).arg(&target).args(["--include", &format!("/{name}")]);
    }
    let (code, _, stderr) = finish(restore, &stop, |_| {})?;
    if code != 0 {
        return Err(said(&stderr));
    }
    let made = into.join(&name);
    if !made.exists() {
        return Err("It isn't in that backup".into());
    }
    Ok(made)
}

/// A file or folder as it was in a backup, made in the temp folder to open or compare: where.
pub async fn backup_open(plan: String, snapshot: String, path: String, dir: bool) -> Result<String, String> {
    blocking(move || {
        let plan = plans().into_iter().find(|p| p.id == plan).ok_or("That backup isn't set up any more")?;
        let made = take_out(&plan, &snapshot, &path, dir, &temp_for(&snapshot)?)?;
        Ok(made.to_string_lossy().into_owned())
    })
    .await
}

/// Restores a file or folder from a backup: in its place (what's there goes to the Recycle Bin), or as a
/// copy beside it, named for the backup's date. Where it went.
pub async fn backup_restore(window: Window, plan: String, snapshot: String, path: String, dir: bool, replace: bool, label: String) -> Result<String, String> {
    let owner = crate::shell::owner(&window);
    blocking(move || {
        let plan = plans().into_iter().find(|p| p.id == plan).ok_or("That backup isn't set up any more")?;
        let target = Path::new(&path);
        let folder = target.parent().ok_or("It has no folder")?;
        let work = folder.join(format!(".glint-restore-{}", std::process::id()));
        fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        crate::actions::hide(&work);
        let result = (|| {
            let made = take_out(&plan, &snapshot, &path, dir, &work)?;
            if replace && target.exists() {
                crate::shell::recycle(owner, vec![path.clone()])?;
            }
            let name = if replace {
                target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            } else {
                // "report (backup 2026-11-09).docx"
                let stem = target.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                match (dir, target.extension()) {
                    (false, Some(ext)) => format!("{stem} ({label}).{}", ext.to_string_lossy()),
                    _ => format!("{} ({label})", target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                }
            };
            let to = crate::fsx::create_numbered(&folder.to_string_lossy(), &name, dir)?;
            if dir {
                let _ = fs::remove_dir(&to);
            }
            fs::rename(&made, &to).map_err(|e| e.to_string())?;
            Ok(to)
        })();
        let _ = fs::remove_dir_all(&work);
        result
    })
    .await
}

/// The app the scheduler tells of the backups it ran (set by [`start_scheduler`]).
static SCHEDULER_APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();
/// Whether the scheduler's thread is running.
static SCHEDULER: AtomicBool = AtomicBool::new(false);

/// Backs up, while Glint runs, the folders whose daily or weekly backup is due, a minute after Glint
/// starts (or a folder gets a schedule) and every half hour after; a folder whose backup drive isn't
/// there is tried next time. No thread runs while no folder has a schedule.
pub fn start_scheduler(app: AppHandle) {
    let _ = SCHEDULER_APP.set(app);
    ensure_scheduler();
}

/// Starts the scheduler's thread when a folder has a schedule, the app has started the scheduler, and the
/// thread isn't running already; it ends once no folder has a schedule.
fn ensure_scheduler() {
    let Some(app) = SCHEDULER_APP.get().cloned() else { return };
    if !scheduled(&plans()) || SCHEDULER.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(60));
        loop {
            for plan in plans() {
                if !due(&plan, now_ms()) || (!plan.repository.starts_with("rclone:") && !Path::new(&plan.repository).exists()) {
                    continue;
                }
                let result = run(&plan.id, &|_| {});
                app.emit(Event::BackupFinished(Finished { source: plan.source.clone(), result }));
            }
            if !scheduled(&plans()) {
                SCHEDULER.store(false, Ordering::SeqCst);
                // A schedule set between the check and now starts the loop again rather than a second thread.
                if !scheduled(&plans()) || SCHEDULER.swap(true, Ordering::SeqCst) {
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(30 * 60));
        }
    });
}

/// Whether any plan is backed up on a schedule (not only by hand).
fn scheduled(plans: &[Plan]) -> bool {
    plans.iter().any(|plan| plan.schedule != Schedule::Manual)
}

/// Whether a plan's next backup is due at `now`: a day (20 hours) or a week (6½ days) after the last
/// that worked; a failed one is tried again an hour later.
fn due(plan: &Plan, now: i64) -> bool {
    let every = match plan.schedule {
        Schedule::Manual => return false,
        Schedule::Daily => 20 * 3600 * 1000,
        Schedule::Weekly => 156 * 3600 * 1000,
    };
    match &plan.last {
        None => true,
        Some(last) if last.ok => now - last.at >= every,
        Some(last) => now - last.at >= 3600 * 1000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_paths_as_restic_does() {
        assert_eq!(restic_path(r"C:\Users\me\Documents").as_deref(), Some("/C/Users/me/Documents"));
        assert_eq!(restic_path(r"d:\Photos\").as_deref(), Some("/D/Photos"));
        assert_eq!(restic_path(r"E:\").as_deref(), Some("/E"));
        assert_eq!(restic_path(r"\\server\share\x"), None);
    }

    #[test]
    fn knows_whats_inside() {
        assert!(within(r"C:\Users\me\Documents\a.txt", r"C:\Users\me\Documents"));
        assert!(within(r"c:\users\me\documents", r"C:\Users\me\Documents\"));
        assert!(!within(r"C:\Users\me\Documents2\a.txt", r"C:\Users\me\Documents"));
    }

    #[test]
    fn reads_snapshot_times() {
        assert_eq!(chrono_ms("1970-01-01T00:00:01Z"), Some(1000));
        assert_eq!(chrono_ms("2026-11-09T20:15:31.5+02:00"), Some(1_794_248_131_500));
        let json = r#"[{"time":"2026-11-09T18:15:31.5Z","short_id":"4f2a9c1b","id":"4f2a9c1b77"},{"time":"bad","short_id":"x"}]"#;
        assert_eq!(snapshots_of(json), vec![("4f2a9c1b".to_string(), 1_794_248_131_500)]);
    }

    #[test]
    fn the_scheduler_runs_only_while_a_folder_has_a_schedule() {
        let plan = |schedule| Plan { id: "1".into(), source: r"C:\x".into(), repository: r"E:\b".into(), schedule, last: None, checked: 0 };
        assert!(!scheduled(&[]));
        assert!(!scheduled(&[plan(Schedule::Manual), plan(Schedule::Manual)]));
        assert!(scheduled(&[plan(Schedule::Manual), plan(Schedule::Weekly)]));
        assert!(scheduled(&[plan(Schedule::Daily)]));
        // Not started by the app (tests, the MCP server): setting a schedule starts no thread.
        ensure_scheduler();
        assert!(!SCHEDULER.load(Ordering::SeqCst));
    }

    #[test]
    fn knows_when_a_backup_is_due() {
        let hour = 3600 * 1000;
        let mut plan = Plan { id: "1".into(), source: r"C:\x".into(), repository: r"E:\b".into(), schedule: Schedule::Daily, last: None, checked: 0 };
        assert!(due(&plan, 0));
        plan.last = Some(LastRun { at: 0, ok: true, ..LastRun::default() });
        assert!(!due(&plan, 19 * hour) && due(&plan, 21 * hour));
        plan.last = Some(LastRun { at: 0, ok: false, ..LastRun::default() });
        assert!(due(&plan, 2 * hour));
        plan.schedule = Schedule::Manual;
        assert!(!due(&plan, 1000 * hour));
    }

    /// A real repository in the temp folder with the restic in TOOLS_DIR (tools\restic\restic.exe): set
    /// up, back up, a second backup that adds only what changed, the versions, Open and a restored copy.
    #[test]
    #[ignore]
    fn backs_up_and_restores_for_real() {
        let tools = PathBuf::from(std::env::var("TOOLS_DIR").expect("TOOLS_DIR"));
        crate::tools::set_dir(tools.clone());
        let base = std::env::temp_dir().join(format!("glint-backup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (source, repository) = (base.join("Documents"), base.join("Backups"));
        fs::create_dir_all(source.join("sub")).unwrap();
        fs::write(source.join("notes.txt"), "first").unwrap();
        fs::write(source.join("sub").join("رحلة.txt"), "trip").unwrap();
        let plan = crate::host::async_runtime::block_on(backup_setup(source.to_string_lossy().into(), repository.to_string_lossy().into(), "correct horse battery".into(), Schedule::Manual)).unwrap();
        let first = run(&plan.id, &|_| {}).unwrap();
        assert!(first.ok && first.files_new == 2, "{first:?}");
        fs::write(source.join("notes.txt"), "second").unwrap();
        std::thread::sleep(Duration::from_millis(1100));
        let second = run(&plan.id, &|_| {}).unwrap();
        assert!(second.ok && second.files_changed == 1 && second.files_new == 0, "{second:?}");
        let versions = crate::host::async_runtime::block_on(backup_versions(source.join("notes.txt").to_string_lossy().into())).unwrap();
        assert_eq!(versions.len(), 2);
        let oldest = &versions[1];
        let opened = crate::host::async_runtime::block_on(backup_open(plan.id.clone(), oldest.snapshot.clone(), source.join("notes.txt").to_string_lossy().into(), false)).unwrap();
        assert_eq!(fs::read_to_string(&opened).unwrap(), "first");
        let folder = crate::host::async_runtime::block_on(backup_open(plan.id.clone(), oldest.snapshot.clone(), source.join("sub").to_string_lossy().into(), true)).unwrap();
        assert_eq!(fs::read_to_string(Path::new(&folder).join("رحلة.txt")).unwrap(), "trip");
        // A wrong password for the same place is refused.
        let wrong = crate::host::async_runtime::block_on(backup_setup(source.to_string_lossy().into(), repository.to_string_lossy().into(), "not the password".into(), Schedule::Manual));
        assert!(wrong.unwrap_err().contains("doesn't open"));
        crate::host::async_runtime::block_on(backup_remove(plan.id.clone())).unwrap();
        let _ = fs::remove_dir_all(&base);
    }
}
