//! A quick count of the photos in a listing that say where they were taken, for the folder head's Photo map pill:
//! what the cache already knows is counted at once, the rest is read once (and kept, so the map never reads it
//! again) in small steps.

use super::meta::{self, Job};
use super::{keep, lower, with_records, F_GEO};

/// How many files are read in one step.
const STEP: usize = 100;

/// How many of `files` (path, size, modified) have a place. Reads only what the cache doesn't know.
pub fn geo_count(files: &[(String, u64, i64)]) -> usize {
    let mut count = 0;
    let mut unread: Vec<Job> = Vec::new();
    with_records(|records| {
        for (path, size, modified) in files {
            match records.get(&lower(path)) {
                Some(m) if m.is_of(*size, *modified) && m.flags & F_GEO != 0 => {
                    count += usize::from(m.place().is_some())
                }
                _ => unread.push(Job {
                    path: path.clone(),
                    size: *size,
                    modified: *modified,
                }),
            }
        }
    });
    for step in unread.chunks(STEP) {
        let found = std::sync::atomic::AtomicUsize::new(0);
        meta::scan(step, &|rows| {
            keep(step, &rows);
            let n = rows.iter().filter(|(_, m)| m.place().is_some()).count();
            found.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
        });
        count += found.into_inner();
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_the_photos_with_a_place_and_reads_each_once() {
        let dir = std::env::temp_dir().join(format!("glint-probe-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tokyo = meta::tests::test_jpeg("2022:03:04 05:06:07", Some((35.6762, 139.6503)), (800, 600), 3_000);
        let plain = meta::tests::test_jpeg("2022:03:05 05:06:07", None, (800, 600), 3_000);
        std::fs::write(dir.join("tokyo.jpg"), &tokyo).unwrap();
        std::fs::write(dir.join("nowhere.jpg"), &plain).unwrap();
        let files: Vec<(String, u64, i64)> = ["tokyo.jpg", "nowhere.jpg"]
            .iter()
            .map(|name| {
                let path = dir.join(name);
                let info = std::fs::metadata(&path).unwrap();
                let modified = info
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64;
                (path.to_string_lossy().into_owned(), info.len(), modified)
            })
            .collect();
        assert_eq!(geo_count(&files), 1);
        // The second time the cache answers (the files are gone and it still counts them).
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(geo_count(&files), 1);
        assert_eq!(geo_count(&[]), 0);
    }
}
