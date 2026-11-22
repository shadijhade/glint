//! Live Folder Time-Machine Scrubber engine.
//!
//! Unifies Windows shadow copies (`versions.rs`), Git commits (`git.rs`), restic backups
//! (`backup.rs`), and the NTFS USN journal (`shadui-index`) into a single timeline per folder.

use crate::fsx::{self, blocking};
use crate::host::Window;
use glint_model::entries::{with_details, FileEntry, RawEntry};
use glint_model::paths::join_path;
use glint_model::time_machine::{diff_entries, merge_moments, Moment, MomentSource, RestoreSource, TimeEntry};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static STAGE_SEQ: AtomicU64 = AtomicU64::new(1);

fn to_model_entries(folder: &str, raw: Vec<fsx::Entry>) -> Vec<FileEntry> {
    raw.into_iter()
        .map(|e| {
            let full = e.path.unwrap_or_else(|| join_path(folder, &e.name));
            let mut item = with_details(
                RawEntry {
                    name: e.name,
                    dir: e.dir,
                    size: e.size,
                    modified: e.modified,
                    attrs: e.attrs,
                    ..RawEntry::default()
                },
                folder,
            );
            item.path = full;
            item
        })
        .collect()
}

fn day_bucket(ts_ms: i64) -> i64 {
    ts_ms.div_euclid(86_400_000) * 86_400_000
}

fn usn_moments(folder: &str) -> Vec<Moment> {
    #[cfg(not(windows))]
    {
        let _ = folder;
        Vec::new()
    }
    #[cfg(windows)]
    {
        let clean = folder.trim_end_matches(['\\', '/']);
        if clean.len() < 3 {
            return Vec::new();
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let since = now_ms.saturating_sub(30 * 86_400_000);
        let Ok(ans) = shadui_index::client::history(&shadui_index::proto::HistoryRequest {
            query: clean.to_string(),
            since,
            max: 500,
        }) else {
            return Vec::new();
        };
        let prefix = format!("{clean}\\").to_lowercase();
        let mut by_day: HashMap<i64, (i64, usize)> = HashMap::new();
        for ev in ans.events {
            let p_lower = ev.path.to_lowercase();
            let Some(rest) = p_lower.strip_prefix(&prefix) else {
                continue;
            };
            if rest.is_empty() || rest.contains('\\') {
                continue;
            }
            if matches!(
                ev.kind,
                shadui_index::proto::Kind::Deleted
                    | shadui_index::proto::Kind::Recycled
                    | shadui_index::proto::Kind::Moved
                    | shadui_index::proto::Kind::Renamed
            ) {
                let day = day_bucket(ev.time);
                let slot = by_day.entry(day).or_insert((ev.time, 0));
                slot.0 = slot.0.max(ev.time);
                slot.1 += 1;
            }
        }
        by_day
            .into_iter()
            .map(|(day, (latest, count))| Moment {
                id: format!("usn-{day}"),
                timestamp_ms: latest,
                label: "Journal activity".to_string(),
                detail: format!("{count} change(s)"),
                source: MomentSource::UsnActivity,
                shadow_path: None,
                commit_oid: None,
                repo_root: None,
                plan_id: None,
                snapshot_id: None,
            })
            .collect()
    }
}

/// Receives a batch of moments as soon as one source has them. Batches arrive in whatever order the sources
/// finish (Git is usually first), possibly from several threads at once; the receiver merges them with
/// [`merge_moments`], whose order depends only on the moments, so the timeline reads the same whichever came first.
pub type MomentSink = Arc<dyn Fn(Vec<Moment>) + Send + Sync>;

/// Streams every historical moment available for `folder` to `sink`: Windows shadow copies, Git commits, the
/// NTFS USN journal and the configured restic backups, all asked at the same time, each reported the moment it
/// is known. Returns when every source has answered.
pub async fn stream_moments(folder: String, sink: MomentSink) -> Result<(), String> {
    // Restic starts first: it is the slowest (a process per plan) and has nothing to wait for.
    let restic = crate::backup::has_plan_for(&folder).then(|| {
        let folder = folder.clone();
        let sink = sink.clone();
        crate::host::async_runtime::spawn(async move {
            if let Ok(backups) = crate::backup::backup_versions(folder).await {
                let moments: Vec<Moment> = backups
                    .into_iter()
                    .map(|b| Moment {
                        id: format!("restic-{}-{}", b.plan, b.snapshot),
                        timestamp_ms: b.time,
                        label: format!("Backup {}", b.snapshot),
                        detail: b.repository,
                        source: MomentSource::ResticBackup,
                        shadow_path: None,
                        commit_oid: None,
                        repo_root: None,
                        plan_id: Some(b.plan),
                        snapshot_id: Some(b.snapshot),
                    })
                    .collect();
                if !moments.is_empty() {
                    sink(moments);
                }
            }
        })
    });

    let local_folder = folder;
    blocking(move || {
        stream_local_moments(&local_folder, &*sink);
        Ok(())
    })
    .await?;
    if let Some(restic) = restic {
        let _ = restic.await;
    }
    Ok(())
}

/// Collects all historical moments available for `folder` across VSS shadow copies, Git commits,
/// configured restic backups, and the NTFS USN journal, newest first, once every source has answered.
pub async fn folder_moments(folder: String) -> Result<Vec<Moment>, String> {
    let all = Arc::new(Mutex::new(Vec::new()));
    let sink: MomentSink = {
        let all = all.clone();
        Arc::new(move |batch| all.lock().unwrap_or_else(|e| e.into_inner()).extend(batch))
    };
    stream_moments(folder, sink).await?;
    let all = std::mem::take(&mut *all.lock().unwrap_or_else(|e| e.into_inner()));
    Ok(merge_moments(all))
}

/// Windows shadow copies of the drive that hold `folder` (non-admin via shadui-index or a device probe).
fn vss_moments(folder: &str) -> Vec<Moment> {
    let mut moments = Vec::new();
    if let Ok(norm) = fsx::normalize(folder) {
        if let Some(drive) = norm.get(..2).filter(|d| d.ends_with(':')) {
            let rest = norm[2..].trim_start_matches('\\');
            for snap in crate::versions::snapshots()
                .into_iter()
                .filter(|s| s.drive.eq_ignore_ascii_case(drive))
            {
                let at = format!("{}{rest}", snap.root);
                if fsx::stat(&at).is_some_and(|s| s.dir) {
                    moments.push(Moment {
                        id: format!("vss-{}", snap.root),
                        timestamp_ms: snap.created,
                        label: if snap.exact {
                            "Restore point".to_string()
                        } else {
                            "Shadow copy".to_string()
                        },
                        detail: snap.drive,
                        source: MomentSource::VssShadow,
                        shadow_path: Some(at),
                        commit_oid: None,
                        repo_root: None,
                        plan_id: None,
                        snapshot_id: None,
                    });
                }
            }
        }
    }
    moments
}

/// Git commits that touched `folder` (pure read-only via gitoxide).
fn git_moments(folder: &str) -> Vec<Moment> {
    crate::git::folder_commits(folder, 25)
        .into_iter()
        .map(|(oid, time_ms, summary, repo_root)| {
            let short: String = oid.chars().take(7).collect();
            Moment {
                id: format!("git-{oid}"),
                timestamp_ms: time_ms,
                label: if summary.is_empty() {
                    format!("Commit {short}")
                } else {
                    summary
                },
                detail: short,
                source: MomentSource::GitCommit,
                shadow_path: None,
                commit_oid: Some(oid),
                repo_root: Some(repo_root),
                plan_id: None,
                snapshot_id: None,
            }
        })
        .collect()
}

/// Asks the three local sources (shadow copies, Git, the USN journal) at once, each on a thread of its own,
/// and hands `sink` each one's moments as it finishes. Returns when all three have.
fn stream_local_moments(folder: &str, sink: &(dyn Fn(Vec<Moment>) + Sync)) {
    let report = |moments: Vec<Moment>| {
        if !moments.is_empty() {
            sink(moments);
        }
    };
    std::thread::scope(|scope| {
        scope.spawn(|| report(vss_moments(folder)));
        scope.spawn(|| report(usn_moments(folder)));
        report(git_moments(folder));
    });
}

/// Inspects `folder` at `moment`, returning entries diffed against the live folder today.
pub async fn inspect_moment(folder: String, moment: Moment) -> Result<Vec<TimeEntry>, String> {
    blocking(move || inspect_moment_sync(&folder, &moment)).await
}

pub(crate) fn inspect_moment_sync(folder: &str, moment: &Moment) -> Result<Vec<TimeEntry>, String> {
    let now_raw = fsx::read_dir(folder).unwrap_or_default();
    let now_entries = to_model_entries(folder, now_raw);

    match moment.source {
        MomentSource::VssShadow => {
            let shadow_path = moment
                .shadow_path
                .as_deref()
                .ok_or_else(|| "Missing shadow copy path".to_string())?;
            let then_raw = fsx::read_dir(shadow_path).map_err(|e| format!("Couldn't read shadow copy: {e}"))?;
            let then_entries = to_model_entries(shadow_path, then_raw);
            Ok(diff_entries(&then_entries, &now_entries, |entry| {
                RestoreSource::ShadowPath(entry.path.clone())
            }))
        }
        MomentSource::GitCommit => {
            let repo_root = moment
                .repo_root
                .as_deref()
                .ok_or_else(|| "Missing Git repository root".to_string())?;
            let oid = moment
                .commit_oid
                .as_deref()
                .ok_or_else(|| "Missing commit hash".to_string())?;
            let then_raw = crate::git::list_tree_at(repo_root, oid, folder)?;
            let then_entries = to_model_entries(folder, then_raw);
            let prefix = crate::git::rel_path_in(repo_root, folder).unwrap_or_default();
            Ok(diff_entries(&then_entries, &now_entries, |entry| {
                let rel_path = if prefix.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{prefix}/{}", entry.name)
                };
                RestoreSource::GitBlob {
                    repo: repo_root.to_string(),
                    oid: oid.to_string(),
                    rel_path,
                }
            }))
        }
        MomentSource::ResticBackup => {
            let plan_id = moment
                .plan_id
                .as_deref()
                .ok_or_else(|| "Missing backup plan".to_string())?;
            let snap = moment
                .snapshot_id
                .as_deref()
                .ok_or_else(|| "Missing backup snapshot".to_string())?;
            let then_raw = crate::backup::list_folder_in_snapshot(plan_id, snap, folder)?;
            let then_entries = to_model_entries(folder, then_raw);
            Ok(diff_entries(&then_entries, &now_entries, |entry| {
                RestoreSource::Restic {
                    plan_id: plan_id.to_string(),
                    snapshot: snap.to_string(),
                    path: join_path(folder, &entry.name),
                    dir: entry.dir,
                }
            }))
        }
        MomentSource::UsnActivity => inspect_usn_moment(folder, moment, &now_entries),
    }
}

fn inspect_usn_moment(folder: &str, moment: &Moment, now_entries: &[FileEntry]) -> Result<Vec<TimeEntry>, String> {
    #[cfg(not(windows))]
    {
        let _ = (folder, moment, now_entries);
        Ok(Vec::new())
    }
    #[cfg(windows)]
    {
        let clean = folder.trim_end_matches(['\\', '/']);
        let day = day_bucket(moment.timestamp_ms);
        let ans = shadui_index::client::history(&shadui_index::proto::HistoryRequest {
            query: clean.to_string(),
            since: day,
            max: 500,
        })
        .map_err(|e| e.to_string())?;
        let prefix = format!("{clean}\\").to_lowercase();
        let mut synthetic_then = now_entries.to_vec();
        let mut usn_targets: HashMap<String, (Option<String>, String)> = HashMap::new();

        for ev in ans.events {
            if day_bucket(ev.time) != day {
                continue;
            }
            let p_lower = ev.path.to_lowercase();
            let Some(rest) = p_lower.strip_prefix(&prefix) else {
                continue;
            };
            if rest.is_empty() || rest.contains('\\') {
                continue;
            }
            if !matches!(
                ev.kind,
                shadui_index::proto::Kind::Deleted
                    | shadui_index::proto::Kind::Recycled
                    | shadui_index::proto::Kind::Moved
                    | shadui_index::proto::Kind::Renamed
            ) {
                continue;
            }
            let name = ev.path.rsplit('\\').next().unwrap_or(&ev.path).to_string();
            let key = name.to_lowercase();
            if usn_targets.contains_key(&key) {
                continue;
            }
            if !now_entries.iter().any(|n| n.name.eq_ignore_ascii_case(&name)) {
                synthetic_then.push(with_details(
                    RawEntry {
                        name,
                        dir: ev.dir,
                        size: 0,
                        modified: ev.time,
                        ..RawEntry::default()
                    },
                    folder,
                ));
            }
            usn_targets.insert(key, (ev.now.or(ev.to), ev.path));
        }

        Ok(diff_entries(&synthetic_then, now_entries, |entry| {
            if let Some((now_path, was_path)) = usn_targets.get(&entry.name.to_lowercase()) {
                RestoreSource::UsnMoved {
                    now_path: now_path.clone(),
                    was_path: was_path.clone(),
                }
            } else {
                RestoreSource::UsnMoved {
                    now_path: Some(entry.path.clone()),
                    was_path: entry.path.clone(),
                }
            }
        }))
    }
}

/// Restores `item` back into `folder` (either beside existing items or replacing via Recycle Bin).
pub async fn restore_item(window: Window, folder: String, item: TimeEntry, replace: bool) -> Result<String, String> {
    let dest = join_path(&folder, &item.name);
    match item.restore {
        RestoreSource::ShadowPath(from) => crate::versions::restore_version(window, from, dest, replace).await,
        RestoreSource::Restic {
            plan_id,
            snapshot,
            path,
            dir,
        } => crate::backup::backup_restore(window, plan_id, snapshot, path, dir, replace, "restored".to_string()).await,
        RestoreSource::GitBlob { repo, oid, rel_path } => {
            let owner = crate::shell::owner(&window);
            blocking(move || {
                let mut target = PathBuf::from(&dest);
                if target.exists() {
                    if replace {
                        crate::shell::recycle(owner, vec![dest.clone()]).map(drop)?;
                    } else {
                        target = crate::queue::free_name(&target);
                    }
                }
                crate::git::extract_blob_at(&repo, &oid, &rel_path, &target)?;
                Ok(target.to_string_lossy().into_owned())
            })
            .await
        }
        RestoreSource::UsnMoved { now_path, was_path } => {
            let owner = crate::shell::owner(&window);
            blocking(move || {
                let Some(cur) = now_path.filter(|p| Path::new(p).exists()) else {
                    return Err(format!(
                        "“{}” is no longer at its moved or recycled path (no shadow copy or backup snapshot held it)",
                        Path::new(&was_path)
                            .file_name()
                            .map(|n| n.to_string_lossy())
                            .unwrap_or_default()
                    ));
                };
                let mut target = PathBuf::from(&dest);
                if target.exists() {
                    if replace {
                        crate::shell::recycle(owner, vec![dest.clone()]).map(drop)?;
                    } else {
                        target = crate::queue::free_name(&target);
                    }
                }
                if Path::new(&cur).is_dir() {
                    return Err("Copy moved folders directly from their current location".into());
                }
                std::fs::copy(&cur, &target).map_err(|e| format!("Couldn't restore it: {e}"))?;
                Ok(target.to_string_lossy().into_owned())
            })
            .await
        }
    }
}

/// Materializes `item` to a read-only temporary file for Compare with today, Quick Look, or Drag-out.
pub async fn stage_item(item: TimeEntry) -> Result<String, String> {
    if item.dir {
        return Err("Only files can be staged for preview or comparison".into());
    }
    match item.restore {
        RestoreSource::ShadowPath(path) => crate::versions::version_copy(path).await,
        RestoreSource::Restic {
            plan_id,
            snapshot,
            path,
            dir,
        } => crate::backup::backup_open(plan_id, snapshot, path, dir).await,
        RestoreSource::GitBlob { repo, oid, rel_path } => {
            let name = item.name.clone();
            blocking(move || {
                let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
                let dir = std::env::temp_dir()
                    .join("Glint")
                    .join("TimeMachine")
                    .join(format!("{}-{seq}", std::process::id()));
                let dest = dir.join(&name);
                crate::git::extract_blob_at(&repo, &oid, &rel_path, &dest)?;
                set_readonly(&dest);
                Ok(dest.to_string_lossy().into_owned())
            })
            .await
        }
        RestoreSource::UsnMoved { now_path, .. } => {
            let Some(cur) = now_path.filter(|p| Path::new(p).is_file()) else {
                return Err("File content is no longer on disk in the USN journal".into());
            };
            Ok(cur)
        }
    }
}

fn set_readonly(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_readonly(true);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// Every local moment for `folder`, with when the first batch and the last one arrived.
    fn collect_local(folder: &str) -> (Vec<Moment>, Duration, Duration) {
        let start = Instant::now();
        let seen = Mutex::new((Vec::new(), None::<Duration>));
        stream_local_moments(folder, &|batch| {
            let mut seen = seen.lock().unwrap();
            seen.1.get_or_insert_with(|| start.elapsed());
            seen.0.extend(batch);
        });
        let all = start.elapsed();
        let (moments, first) = seen.into_inner().unwrap();
        (merge_moments(moments), first.unwrap_or(all), all)
    }

    #[test]
    fn collects_git_moments_inspects_deleted_and_modified_and_restores() {
        let dir = std::env::temp_dir().join(format!("glint-tm-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let git = |args: &[&str]| -> bool {
            Command::new("git")
                .args(args)
                .current_dir(&dir)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q"]) {
            // Skip cleanly if git CLI isn't installed in a minimal test environment.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        git(&["config", "user.email", "test@glint.local"]);
        git(&["config", "user.name", "Glint Test"]);

        std::fs::write(dir.join("notes.txt"), "v1 notes\n").unwrap();
        std::fs::write(dir.join("deleted_later.txt"), "precious data\n").unwrap();
        assert!(git(&["add", "."]));
        assert!(git(&["commit", "-q", "-m", "initial snapshot"]));

        // Mutate working tree: modify notes.txt, delete deleted_later.txt.
        std::fs::write(dir.join("notes.txt"), "v2 edited notes much longer\n").unwrap();
        std::fs::remove_file(dir.join("deleted_later.txt")).unwrap();

        let dir_str = dir.to_string_lossy().into_owned();
        let moments = collect_local(&dir_str).0;
        let git_moment = moments
            .into_iter()
            .find(|m| m.source == MomentSource::GitCommit)
            .expect("should find git commit moment");
        assert_eq!(git_moment.label, "initial snapshot");

        let diff = inspect_moment_sync(&dir_str, &git_moment).expect("should diff commit against workdir");
        let deleted = diff
            .iter()
            .find(|e| e.name == "deleted_later.txt")
            .expect("deleted_later.txt should appear");
        assert_eq!(deleted.delta, glint_model::time_machine::EntryDelta::Deleted);

        let modified = diff
            .iter()
            .find(|e| e.name == "notes.txt")
            .expect("notes.txt should appear");
        assert!(matches!(
            modified.delta,
            glint_model::time_machine::EntryDelta::Modified { .. }
        ));

        // Stage and extract the deleted file directly via git::extract_blob_at.
        if let RestoreSource::GitBlob { repo, oid, rel_path } = &deleted.restore {
            let restored = dir.join("deleted_later.txt");
            crate::git::extract_blob_at(repo, oid, rel_path, &restored).unwrap();
            assert_eq!(std::fs::read_to_string(&restored).unwrap(), "precious data\n");
        } else {
            panic!("expected GitBlob restore source");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moments_stream_git_first_while_the_other_sources_are_still_asked() {
        let dir = std::env::temp_dir().join(format!("glint-tm-stream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| -> bool {
            Command::new("git")
                .args(args)
                .current_dir(&dir)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q"]) {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        git(&["config", "user.email", "test@glint.local"]);
        git(&["config", "user.name", "Glint Test"]);
        for n in 0..20 {
            std::fs::write(
                dir.join(format!("f{n}.txt")),
                format!(
                    "v{n}
"
                ),
            )
            .unwrap();
            assert!(git(&["add", "."]));
            assert!(git(&["commit", "-q", "-m", &format!("commit {n}")]));
        }
        let folder = dir.to_string_lossy().into_owned();

        // One after another, as 0.15 asked.
        let start = Instant::now();
        let mut sequential = vss_moments(&folder);
        let vss_done = start.elapsed();
        sequential.extend(git_moments(&folder));
        let git_done = start.elapsed();
        sequential.extend(usn_moments(&folder));
        let sequential_all = start.elapsed();

        let (streamed, first, all) = collect_local(&folder);
        println!(
            "moments: sequential vss {vss_done:?}, +git {git_done:?} (first Git stops), +usn {sequential_all:?} (all) |              streamed first batch {first:?}, all {all:?}"
        );
        assert_eq!(
            streamed.iter().filter(|m| m.source == MomentSource::GitCommit).count(),
            20
        );
        assert_eq!(
            merge_moments(sequential),
            streamed,
            "the same moments, in the same order"
        );
        assert!(first <= all);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
