//! Waiting marks wake (PLAN.md F31): an item an import could not find has its marks in the ledger, `why: away`;
//! the first time Glint lists the folder it is in, they move into the file. The listing only reads one atomic;
//! the moving is a thread's of its own.

use super::ledger::{self, Ledger};
use super::{caps, read_all, work, World};
use glint_model::marks::{meet, Change, Marks, Meet};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

/// The folders being woken right now, in lower case: one thread a folder.
static BUSY: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Set when the first listing asked the shared ledger to be opened (on a thread that isn't the listing's).
static OPENING: AtomicBool = AtomicBool::new(false);

/// Told of every real folder Glint lists, on the thread that listed it: it must cost nothing while no mark
/// waits (one atomic read), and never read or write a file on this thread.
pub fn listed(folder: &str) {
    match ledger::shared_if_open() {
        Some(shared) => {
            listed_with(folder, shared, ledger::shared_away(), caps::has_streams);
        }
        // Nothing has read the ledger yet, and reading it is not for this thread: a thread of its own opens it, and
        // wakes this folder if it has waiting marks. (Listings meanwhile wake nothing; the next one does.)
        None if !OPENING.swap(true, Ordering::Relaxed) => {
            let folder = folder.to_string();
            let _ = std::thread::Builder::new().name("marks-open".into()).spawn(move || {
                let shared = ledger::shared();
                if ledger::shared_away().load(Ordering::Relaxed) > 0 {
                    wake_in(&folder, shared, caps::has_streams);
                }
            });
        }
        None => {}
    }
}

/// `listed` over a ledger and a drive test of its own; true when a thread was started.
fn listed_with(
    folder: &str,
    ledger: &'static Mutex<Ledger>,
    away: &'static AtomicUsize,
    streams: fn(&str) -> bool,
) -> bool {
    if away.load(Ordering::Relaxed) == 0 {
        return false;
    }
    // Only a folder a line waits in starts a thread. The lock is only tried: while another thread holds the
    // ledger (it may be saving it), the thread starts and looks for itself.
    if ledger.try_lock().is_ok_and(|l| !l.waits_in(folder)) {
        return false;
    }
    let folder = folder.to_string();
    std::thread::Builder::new()
        .name("marks-wake".into())
        .spawn(move || {
            wake_in(&folder, ledger, streams);
        })
        .is_ok()
}

fn busy() -> MutexGuard<'static, Vec<String>> {
    BUSY.lock().unwrap_or_else(|e| e.into_inner())
}

/// Gives the waiting marks of `folder` whose item is there now to the item, and takes their lines out of the
/// ledger; how many woke. A folder that another thread is waking is left to it.
fn wake_in(folder: &str, ledger: &Mutex<Ledger>, streams: fn(&str) -> bool) -> usize {
    let key = folder.trim_end_matches(['\\', '/']).to_lowercase();
    {
        let mut busy = busy();
        if busy.contains(&key) {
            return 0;
        }
        busy.push(key.clone());
    }
    struct Done(String);
    impl Drop for Done {
        fn drop(&mut self) {
            busy().retain(|k| *k != self.0);
        }
    }
    let _done = Done(key);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0);
    let w = World { streams: &streams, ledger, empty: None, now };
    // Taken out first: the homes read the ledger last, and a line left in it would be what "has" says.
    let waiting: Vec<(String, Marks)> = {
        let mut l = w.ledger();
        let there: Vec<String> =
            l.away_in(folder).into_iter().map(|x| x.name).filter(|p| std::fs::symlink_metadata(p).is_ok()).collect();
        there.into_iter().filter_map(|p| l.take(&p).map(|line| (p, line.marks))).collect()
    };
    if waiting.is_empty() {
        return 0;
    }
    let paths: Vec<String> = waiting.iter().map(|(p, _)| p.clone()).collect();
    let has = read_all(&paths, &w);
    let items: Vec<(String, Change)> = waiting
        .iter()
        .zip(has)
        .map(|((p, marks), has)| (p.clone(), Change::Put(meet(&has, marks, Meet::Fill).0)))
        .collect();
    let outcomes = work(items, &w);
    let mut woke = 0;
    {
        let mut l = w.ledger();
        let failed: HashSet<&str> = outcomes.iter().filter(|o| o.after.is_err()).map(|o| o.path.as_str()).collect();
        for (p, marks) in &waiting {
            if failed.contains(p.as_str()) {
                // It couldn't be written: it waits on, as it did.
                l.put(p, marks, glint_model::marks::Why::Away, w.now);
            } else {
                woke += 1;
            }
        }
        let _ = l.save();
    }
    woke
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::marks::Why;
    use std::path::PathBuf;

    fn none(_: &str) -> bool {
        false
    }

    fn folder(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("glint-wake-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn red() -> Marks {
        Marks { label: Some("Red".into()), tags: vec!["x".into()], comment: None }
    }

    fn leaked(dir: &std::path::Path) -> (&'static Mutex<Ledger>, &'static AtomicUsize) {
        let count: &'static AtomicUsize = Box::leak(Box::new(AtomicUsize::new(0)));
        let mut l = Ledger::open(Some(dir.join("marks.jsonl")));
        l.away_flag = Some(count);
        (Box::leak(Box::new(Mutex::new(l))), count)
    }

    #[test]
    fn with_no_waiting_line_listed_returns_at_once_and_starts_nothing() {
        let d = folder("none");
        let (l, count) = leaked(&d);
        assert_eq!(count.load(Ordering::Relaxed), 0);
        assert!(!listed_with(&d.to_string_lossy(), l, count, none));
        // A line that is shut, not waiting, doesn't start one either.
        l.lock().unwrap().put("C:\\a\\b", &red(), Why::Shut, 1);
        assert!(!listed_with("C:\\a", l, count, none));
        // A line waiting in another folder doesn't either: the listing asks the ledger, it doesn't scan it.
        l.lock().unwrap().put("C:\\elsewhere\\b", &red(), Why::Away, 1);
        assert_eq!(count.load(Ordering::Relaxed), 1);
        assert!(!listed_with("C:\\a", l, count, none));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_waiting_line_whose_file_is_there_moves_into_the_file_and_the_others_stay() {
        let d = folder("wake");
        let there = d.join("there.txt");
        std::fs::write(&there, b"x").unwrap();
        let (here, other) = (there.to_string_lossy().into_owned(), d.join("sub").join("o.txt").to_string_lossy().into_owned());
        let missing = d.join("missing.txt").to_string_lossy().into_owned();
        let (l, count) = leaked(&d);
        {
            let mut g = l.lock().unwrap();
            g.put(&here, &red(), Why::Away, 1);
            g.put(&missing, &red(), Why::Away, 1);
            g.put(&other, &red(), Why::Away, 1);
        }
        assert_eq!(count.load(Ordering::Relaxed), 3);
        assert!(listed_with(&d.to_string_lossy().to_uppercase(), l, count, none));
        let mut waited = 0;
        while waited < 400 && (l.lock().unwrap().get(&here).is_some() || !d.join(".glint-marks").exists()) {
            std::thread::sleep(std::time::Duration::from_millis(25));
            waited += 1;
        }
        assert!(l.lock().unwrap().get(&here).is_none(), "the line should be gone");
        let text = std::fs::read_to_string(d.join(".glint-marks")).unwrap();
        assert!(text.contains("there.txt") && text.contains("Red"), "{text}");
        let g = l.lock().unwrap();
        assert!(g.get(&missing).is_some() && g.get(&other).is_some());
        assert_eq!(g.away(), 2);
        drop(g);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_waiting_mark_fills_what_the_item_was_marked_with_since() {
        let d = folder("fill");
        let a = d.join("a.txt");
        std::fs::write(&a, b"x").unwrap();
        let a = a.to_string_lossy().into_owned();
        let (l, _) = leaked(&d);
        let w = World { streams: &none, ledger: l, empty: None, now: 5 };
        work(vec![(a.clone(), Change::Label(Some("Mine".into())))], &w);
        // The marks beside the file are the file's own now; the waiting line adds what it has that the file lacks.
        l.lock().unwrap().put(&a, &red(), Why::Away, 1);
        assert_eq!(wake_in(&d.to_string_lossy(), l, none), 1);
        let read = read_all(std::slice::from_ref(&a), &w);
        assert_eq!(read[0].label.as_deref(), Some("Mine"));
        assert_eq!(read[0].tags, ["x"]);
        assert!(l.lock().unwrap().get(&a).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
