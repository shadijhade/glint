//! Pure data model and diff logic for the Live Folder Time-Machine Scrubber.
//!
//! Unifies snapshots from Windows Shadow Copies (VSS), Git commits, restic backups, and the
//! NTFS USN journal into a single timeline of [`Moment`]s and [`TimeEntry`] diffs.

use crate::entries::{natural_compare, FileEntry};
use std::collections::{HashMap, HashSet};

/// Which history provider produced a [`Moment`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MomentSource {
    VssShadow,
    GitCommit,
    ResticBackup,
    UsnActivity,
}

impl MomentSource {
    /// Short human-readable badge text for the source.
    pub fn label(self) -> &'static str {
        match self {
            MomentSource::VssShadow => "Windows Restore",
            MomentSource::GitCommit => "Git",
            MomentSource::ResticBackup => "Backup",
            MomentSource::UsnActivity => "Journal",
        }
    }
}

/// One historical point in time for a folder across all available history providers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moment {
    pub id: String,
    /// Unix milliseconds when this snapshot or commit was recorded.
    pub timestamp_ms: i64,
    pub label: String,
    pub detail: String,
    pub source: MomentSource,
    pub shadow_path: Option<String>,
    pub commit_oid: Option<String>,
    pub repo_root: Option<String>,
    pub plan_id: Option<String>,
    pub snapshot_id: Option<String>,
}

/// How an entry at a past [`Moment`] compares to the live folder today.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryDelta {
    Unchanged,
    Deleted,
    Modified { now_size: u64, now_modified: i64 },
    AddedSince,
}

/// Where to recover or stage an item from when restoring or comparing against today.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreSource {
    ShadowPath(String),
    GitBlob {
        repo: String,
        oid: String,
        rel_path: String,
    },
    Restic {
        plan_id: String,
        snapshot: String,
        path: String,
        dir: bool,
    },
    UsnMoved {
        now_path: Option<String>,
        was_path: String,
    },
}

/// One file or folder as it appeared at a past [`Moment`], paired with its delta vs today.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeEntry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified: i64,
    pub delta: EntryDelta,
    pub restore: RestoreSource,
}

/// Diffs a historical listing (`then`) against the live folder (`now`), tagging deleted, modified,
/// and unchanged entries using case-insensitive Windows filename matching.
///
/// Deleted and modified entries come before unchanged entries, each group ordered folders-first
/// then naturally by name.
pub fn diff_entries(
    then: &[FileEntry],
    now: &[FileEntry],
    make_restore: impl Fn(&FileEntry) -> RestoreSource,
) -> Vec<TimeEntry> {
    let mut now_by_name: HashMap<String, &FileEntry> = HashMap::with_capacity(now.len());
    for entry in now {
        now_by_name.insert(entry.name.to_lowercase(), entry);
    }

    let mut out = Vec::with_capacity(then.len());
    for old in then {
        let key = old.name.to_lowercase();
        let delta = match now_by_name.get(&key) {
            None => EntryDelta::Deleted,
            Some(cur) if cur.dir != old.dir => EntryDelta::Modified {
                now_size: cur.size,
                now_modified: cur.modified,
            },
            Some(cur) if !old.dir && (cur.size != old.size || (old.modified > 0 && cur.modified != old.modified)) => {
                EntryDelta::Modified {
                    now_size: cur.size,
                    now_modified: cur.modified,
                }
            }
            Some(_) => EntryDelta::Unchanged,
        };
        out.push(TimeEntry {
            name: old.name.clone(),
            dir: old.dir,
            size: old.size,
            modified: old.modified,
            delta,
            restore: make_restore(old),
        });
    }

    out.sort_by(|a, b| {
        delta_rank(&a.delta)
            .cmp(&delta_rank(&b.delta))
            .then_with(|| b.dir.cmp(&a.dir))
            .then_with(|| natural_compare(&a.name, &b.name))
    });
    out
}

fn delta_rank(delta: &EntryDelta) -> u8 {
    match delta {
        EntryDelta::Deleted => 0,
        EntryDelta::Modified { .. } => 1,
        EntryDelta::AddedSince => 2,
        EntryDelta::Unchanged => 3,
    }
}

/// Returns `(deleted, modified, unchanged)` counts for a moment's diffed entries.
pub fn summarize_entries(entries: &[TimeEntry]) -> (usize, usize, usize) {
    let (mut deleted, mut modified, mut unchanged) = (0usize, 0usize, 0usize);
    for entry in entries {
        match entry.delta {
            EntryDelta::Deleted => deleted += 1,
            EntryDelta::Modified { .. } => modified += 1,
            EntryDelta::Unchanged => unchanged += 1,
            EntryDelta::AddedSince => {}
        }
    }
    (deleted, modified, unchanged)
}

/// Sorts [`Moment`]s newest-first and deduplicates identical `(source, id)` entries.
pub fn merge_moments(mut moments: Vec<Moment>) -> Vec<Moment> {
    moments.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms).then_with(|| a.id.cmp(&b.id)));
    let mut seen: HashSet<(MomentSource, String)> = HashSet::with_capacity(moments.len());
    moments.retain(|m| seen.insert((m.source, m.id.clone())));
    moments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entries::{with_details, RawEntry};

    fn entry(name: &str, dir: bool, size: u64, modified: i64) -> FileEntry {
        with_details(
            RawEntry {
                name: name.into(),
                dir,
                size,
                modified,
                ..RawEntry::default()
            },
            r"C:\Proj",
        )
    }

    #[test]
    fn diffs_deleted_modified_and_unchanged_case_insensitively_with_unicode() {
        let then = vec![
            entry("docs", true, 0, 1000),
            entry("Report.pdf", false, 500, 1000),
            entry("تقرير.docx", false, 800, 2000),
            entry("old_notes.txt", false, 120, 1000),
        ];
        let now = vec![
            entry("docs", true, 0, 1000),
            entry("report.pdf", false, 500, 1000),
            entry("تقرير.docx", false, 950, 3000),
            entry("brand_new.md", false, 50, 4000),
        ];
        let diff = diff_entries(&then, &now, |e| RestoreSource::ShadowPath(e.path.clone()));
        assert_eq!(summarize_entries(&diff), (1, 1, 2));
        assert_eq!(diff[0].name, "old_notes.txt");
        assert_eq!(diff[0].delta, EntryDelta::Deleted);
        assert_eq!(diff[1].name, "تقرير.docx");
        assert_eq!(
            diff[1].delta,
            EntryDelta::Modified {
                now_size: 950,
                now_modified: 3000
            }
        );
        assert_eq!(diff[2].name, "docs");
        assert_eq!(diff[2].delta, EntryDelta::Unchanged);
        assert_eq!(diff[3].name, "Report.pdf");
        assert_eq!(diff[3].delta, EntryDelta::Unchanged);
    }

    #[test]
    fn merges_and_deduplicates_moments_newest_first() {
        let m = |id: &str, ts: i64, source: MomentSource| Moment {
            id: id.into(),
            timestamp_ms: ts,
            label: id.into(),
            detail: String::new(),
            source,
            shadow_path: None,
            commit_oid: None,
            repo_root: None,
            plan_id: None,
            snapshot_id: None,
        };
        let merged = merge_moments(vec![
            m("c1", 100, MomentSource::GitCommit),
            m("c2", 300, MomentSource::VssShadow),
            m("c1", 100, MomentSource::GitCommit),
            m("c3", 200, MomentSource::ResticBackup),
        ]);
        let ids: Vec<&str> = merged.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["c2", "c3", "c1"]);
    }
}
