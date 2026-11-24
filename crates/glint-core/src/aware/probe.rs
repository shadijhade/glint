//! Asking about many paths at once.
//!
//! [`probe`] makes one job on the lane for every part of every ask (pictures' sizes are read together through
//! the I/O ring, as one job), and each job sends its answer the moment it has it. A part that can't be known
//! sends nothing: the window draws nothing for it (rule R1 of docs/gpui/aware/PLAN.md).

use super::{file, inside, pool, twins, Answer, Ask, Lane, Media, Part};
use crate::host::ipc::Channel;
use crate::ioring;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The pictures whose size is read from their first 64 KB.
const PICTURES: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "bmp", "webp", "tif", "tiff", "ico", "avif", "heic", "heif", "psd", "qoi",
];

/// How much of a picture's start tells its size.
const HEADER: u32 = 65_536;

/// Asks about every path at once and returns at once: each answer is sent as it is known, in no order. Setting
/// `stop` ends what hasn't started and makes walks and hashes give up. A part that can't be known sends nothing.
pub fn probe(asks: Vec<Ask>, lane: Lane, stop: Arc<AtomicBool>, out: Channel<Answer>) {
    let mut pictures: Vec<String> = Vec::new();
    for ask in asks {
        let Ask { path, size, want, .. } = ask;
        if want.media {
            if is_picture(&path) {
                pictures.push(path.clone());
            } else {
                job(lane, &stop, &out, &path, file::media, Part::Media);
            }
        }
        if want.locked_by {
            job(lane, &stop, &out, &path, file::locked_by, Part::LockedBy);
        }
        if want.origin {
            job(lane, &stop, &out, &path, file::origin, Part::Origin);
        }
        if want.signer {
            job(lane, &stop, &out, &path, file::signer, Part::Signer);
        }
        if want.text {
            let stopped = stop.clone();
            job(lane, &stop, &out, &path, move |p| file::text(p, &stopped), Part::Text);
        }
        if want.pages {
            job(lane, &stop, &out, &path, file::pages, Part::Pages);
        }
        if want.target {
            job(lane, &stop, &out, &path, file::target, Part::Target);
        }
        if want.opens_with {
            job(lane, &stop, &out, &path, file::opens_with, Part::OpensWith);
        }
        if !want.sums.is_empty() {
            // Quiet, and only up to the cap: a file is read for this because somebody pointed at it.
            let (stopped, sums) = (stop.clone(), want.sums);
            job(
                lane,
                &stop,
                &out,
                &path,
                move |p| file::sums(p, sums, glint_model::aware::HASH_CAP, &stopped),
                Part::Sums,
            );
        }
        if want.extracted {
            job(lane, &stop, &out, &path, file::extracted, Part::Extracted);
        }
        if want.inside {
            let (stop, out, path) = (stop.clone(), out.clone(), path.clone());
            pool::spawn(lane, move || {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let mut send = |found| send(&out, &path, Part::Inside(found));
                inside::inside(&path, lane, &stop, &mut send);
            });
        }
        if want.twins {
            let stopped = stop.clone();
            job(lane, &stop, &out, &path, move |p| twins::twins(p, size, &stopped), Part::Twins);
        }
    }
    if !pictures.is_empty() {
        let (stop, out) = (stop.clone(), out.clone());
        pool::spawn(lane, move || {
            if !stop.load(Ordering::Relaxed) {
                picture_sizes(pictures, &stop, &out);
            }
        });
    }
}

/// Whether the file's extension is a picture's.
fn is_picture(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| PICTURES.contains(&e.as_str()))
}

/// Sends one answer; a send that fails means the window is gone, and nobody is told.
fn send(out: &Channel<Answer>, path: &str, part: Part) {
    let _ = out.send(Answer { path: path.to_string(), part });
}

/// One part of one path as a job: nothing when `stop` is set by the time it starts, nothing when `find` has no
/// answer.
fn job<T: Send + 'static>(
    lane: Lane,
    stop: &Arc<AtomicBool>,
    out: &Channel<Answer>,
    path: &str,
    find: impl FnOnce(&str) -> Option<T> + Send + 'static,
    part: impl FnOnce(T) -> Part + Send + 'static,
) {
    let (stop, out, path) = (stop.clone(), out.clone(), path.to_string());
    pool::spawn(lane, move || {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if let Some(found) = find(&path) {
            send(&out, &path, part(found));
        }
    });
}

/// The pictures' sizes: all their headers read together through the I/O ring, the size told from each, and the
/// slower way for any that the ring or the header didn't give.
fn picture_sizes(paths: Vec<String>, stop: &AtomicBool, out: &Channel<Answer>) {
    let asks: Vec<ioring::Ask> = paths.iter().map(|path| ioring::Ask { path, slices: vec![(0, HEADER)] }).collect();
    let got = ioring::read_many(&asks);
    drop(asks);
    for (n, path) in paths.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let bytes = got
            .as_ref()
            .and_then(|all| all.get(n))
            .and_then(|one| one.as_ref())
            .and_then(|one| one.slices.first());
        let size = bytes.and_then(|bytes| imagesize::blob_size(bytes).ok()).or_else(|| imagesize::size(path).ok());
        if let Some(size) = size {
            if let (Ok(width), Ok(height)) = (u32::try_from(size.width), u32::try_from(size.height)) {
                send(out, path, Part::Media(Media { width, height, seconds: 0.0 }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::aware::Want;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn collector() -> (Channel<Answer>, Arc<Mutex<Vec<Answer>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let into = seen.clone();
        let channel = Channel::new(move |answer| {
            into.lock().unwrap_or_else(|e| e.into_inner()).push(answer);
            Ok(())
        });
        (channel, seen)
    }

    fn wait_for(seen: &Mutex<Vec<Answer>>, count: usize) -> Vec<Answer> {
        let start = Instant::now();
        // Long enough for the quiet lane on a busy machine: its threads run at background priority, and beside
        // every other test of the crate an answer has taken more than five seconds. It returns as soon as the
        // answers are in, so a healthy run waits no longer for it.
        while start.elapsed() < Duration::from_secs(30) {
            let now = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if now.len() >= count {
                return now;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-aware-probe-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn ask(path: &std::path::Path, want: Want) -> Ask {
        Ask { path: path.to_string_lossy().into_owned(), dir: false, size: 0, want }
    }

    #[test]
    fn three_text_files_asked_for_their_lines_send_three_answers_and_none_when_stopped() {
        let dir = scratch("text");
        let paths: Vec<_> = (0..3).map(|n| dir.join(format!("{n}.txt"))).collect();
        for (n, path) in paths.iter().enumerate() {
            let _ = std::fs::write(path, "line\n".repeat(n + 1));
        }
        let want = Want { text: true, ..Want::default() };
        let asks = || paths.iter().map(|p| ask(p, want)).collect::<Vec<_>>();

        let (out, seen) = collector();
        probe(asks(), Lane::Now, Arc::new(AtomicBool::new(false)), out);
        let got = wait_for(&seen, 3);
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|a| matches!(a.part, Part::Text(_))));
        for (n, path) in paths.iter().enumerate() {
            let answer = got.iter().find(|a| a.path == path.to_string_lossy());
            let lines = answer.map(|a| match a.part {
                Part::Text(t) => t.lines,
                _ => 0,
            });
            assert_eq!(lines, Some(n as u64 + 1));
        }

        let (out, seen) = collector();
        probe(asks(), Lane::Now, Arc::new(AtomicBool::new(true)), out);
        std::thread::sleep(Duration::from_millis(200));
        assert!(seen.lock().unwrap_or_else(|e| e.into_inner()).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_picture_asked_for_its_media_says_its_size() {
        let dir = scratch("png");
        let path = dir.join("one.png");
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let Ok(mut writer) = encoder.write_header() else { return };
            assert!(writer.write_image_data(&[255, 0, 0, 255]).is_ok());
        }
        let _ = std::fs::write(&path, &bytes);
        let (out, seen) = collector();
        let want = Want { media: true, ..Want::default() };
        probe(vec![ask(&path, want)], Lane::Quiet, Arc::new(AtomicBool::new(false)), out);
        let got = wait_for(&seen, 1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].part, Part::Media(Media { width: 1, height: 1, seconds: 0.0 }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_archive_asked_for_its_folder_says_whether_it_was_extracted() {
        let dir = scratch("extracted");
        let zip = dir.join("pack.zip");
        let _ = std::fs::write(&zip, b"PK");
        let want = Want { extracted: true, ..Want::default() };
        let (out, seen) = collector();
        probe(vec![ask(&zip, want)], Lane::Quiet, Arc::new(AtomicBool::new(false)), out);
        assert_eq!(wait_for(&seen, 1)[0].part, Part::Extracted(None));
        let _ = std::fs::create_dir_all(dir.join("pack"));
        let (out, seen) = collector();
        probe(vec![ask(&zip, want)], Lane::Quiet, Arc::new(AtomicBool::new(false)), out);
        let folder = dir.join("pack").to_string_lossy().into_owned();
        assert_eq!(wait_for(&seen, 1)[0].part, Part::Extracted(Some(folder)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_asked_for_its_checksums_sends_them_and_a_stopped_or_missing_one_sends_none() {
        use glint_model::aware::{Sums, SumsWant};
        let dir = scratch("sums");
        let file = dir.join("abc.txt");
        let _ = std::fs::write(&file, b"abc");
        let want = Want { sums: SumsWant { sha256: true, blake3: true, ..SumsWant::default() }, ..Want::default() };
        let (out, seen) = collector();
        probe(vec![ask(&file, want)], Lane::Quiet, Arc::new(AtomicBool::new(false)), out);
        let got = wait_for(&seen, 1);
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].part,
            Part::Sums(Sums {
                sha256: Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into()),
                blake3: Some("6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85".into()),
                ..Sums::default()
            })
        );
        let (out, seen) = collector();
        probe(vec![ask(&file, want)], Lane::Quiet, Arc::new(AtomicBool::new(true)), out);
        probe(vec![ask(&dir.join("gone.txt"), want)], Lane::Quiet, Arc::new(AtomicBool::new(false)), out_none());
        std::thread::sleep(Duration::from_millis(200));
        assert!(seen.lock().unwrap_or_else(|e| e.into_inner()).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn out_none() -> Channel<Answer> {
        collector().0
    }

    #[test]
    fn a_part_that_cannot_be_known_sends_nothing() {
        let dir = scratch("none");
        let path = dir.join("missing.png");
        let (out, seen) = collector();
        let want = Want { media: true, text: true, origin: true, ..Want::default() };
        probe(vec![ask(&path, want)], Lane::Now, Arc::new(AtomicBool::new(false)), out);
        std::thread::sleep(Duration::from_millis(300));
        assert!(seen.lock().unwrap_or_else(|e| e.into_inner()).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
