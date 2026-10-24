//! Sending to another PC with sendme (iroh): what's sent is offered under a ticket, which the other PC
//! receives it with, end-to-end encrypted, directly or through a relay when the two can't reach each
//! other. A send lasts until it's stopped or Glint ends: its process is held in a job that ends with
//! Glint, however Glint ends. sendme keeps its working files in the folder it's started in, so a send
//! runs in a folder of Glint's own in the temp folder (what's sent is read where it is), and a receive
//! in a hidden one in the folder it's received into, out of which what came is moved, under a free name.
//! A working folder left by a run Glint didn't see end (it crashed, or was ended) goes at the next start.

use crate::fsx::blocking;
use crate::proc::Job;
use serde::Serialize;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;
use tauri::ipc::Channel;

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A send or a receive running: dropping it ends its process (closing its job), after which the thread
/// that waits on it tidies up.
struct Running {
    id: u64,
    _job: Job,
    stopped: Arc<AtomicBool>,
    /// sendme's working folder, removed once it's ended.
    work: PathBuf,
    _lock: File,
}

static RUNNING: Mutex<Vec<Running>> = Mutex::new(Vec::new());

fn running() -> MutexGuard<'static, Vec<Running>> {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Stops a send (its ticket stops working) or a receive (what came so far goes).
#[tauri::command]
pub fn share_stop(id: u64) {
    let mut list = running();
    if let Some(at) = list.iter().position(|r| r.id == id) {
        list[at].stopped.store(true, Ordering::Relaxed);
        list.remove(at);
    }
}

/// Glint's closing: every send and receive ends, and their working files go.
pub fn stop_all() {
    let ended = std::mem::take(&mut *running());
    let works: Vec<PathBuf> = ended.iter().map(|r| r.work.clone()).collect();
    for r in &ended {
        r.stopped.store(true, Ordering::Relaxed);
    }
    drop(ended);
    for work in works {
        tidy(&work);
    }
}

/// Held open, unshared, in a working folder while its run is on, so that a start's sweep (in another
/// Glint) can only delete it once the run has ended: archive/extract.rs's lock, again.
const LOCK: &str = ".glint-lock";

fn lock(work: &Path) -> Result<File, String> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).share_mode(0).open(work.join(LOCK)).map_err(|e| e.to_string())
}

/// Where receives' hidden folders are noted, for a start after a crash to find them.
fn journal() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("receiving.txt"))
}

fn note(work: &Path) {
    if let Some(Ok(mut file)) = journal().map(|j| std::fs::OpenOptions::new().create(true).append(true).open(j)) {
        let _ = writeln!(file, "{}", work.display());
    }
}

/// At start: the working folders of runs that didn't end well go, unless one's still on (in another Glint).
pub fn sweep() {
    let ended = |work: &Path| std::fs::remove_file(work.join(LOCK)).is_ok() || !work.join(LOCK).exists();
    for entry in std::fs::read_dir(std::env::temp_dir()).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with("glint-send-") && ended(&entry.path()) {
            tidy(&entry.path());
        }
    }
    let Some(journal) = journal() else { return };
    let Ok(text) = std::fs::read_to_string(&journal) else { return };
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| {
            let work = Path::new(line);
            let ours = work.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".glint-receiving-")) && work.is_dir();
            if ours && ended(work) {
                tidy(work);
            }
            ours && work.is_dir()
        })
        .collect();
    let _ = if kept.is_empty() { std::fs::remove_file(&journal) } else { std::fs::write(&journal, kept.join("\n") + "\n") };
}

/// Removes a working folder sendme has let go of (its files can stay open a moment after it ends).
fn tidy(work: &Path) {
    for _ in 0..20 {
        if !work.exists() || std::fs::remove_dir_all(work).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Starts `command` held in a job of its own, which ends it with Glint.
fn start(command: &mut Command) -> Result<(Job, Child), String> {
    let job = Job::ending()?;
    let mut child = command.spawn().map_err(|e| format!("Couldn't start sendme: {e}"))?;
    if let Err(e) = job.assign(&child) {
        let _ = child.kill();
        return Err(e);
    }
    Ok((job, child))
}

/// Reads what a process says on `stream`, line by line, to `each`; ends with its last lines, for
/// saying why it failed.
fn listen(stream: Option<impl Read + Send + 'static>, each: impl Fn(&str) + Send + 'static) -> JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        let mut last = Vec::new();
        for line in stream.into_iter().flat_map(|s| BufReader::new(s).lines().map_while(Result::ok)) {
            each(&line);
            if !line.trim().is_empty() {
                last.push(line);
                if last.len() > 8 {
                    last.remove(0);
                }
            }
        }
        last
    })
}

/// Why sendme ended, from its last words: its error, without clap's advice.
fn why(last: &[String], code: Option<i32>) -> String {
    let said = last.iter().rev().map(|l| l.trim()).find(|l| !l.starts_with("For more information") && !l.starts_with("You can remove"));
    match said {
        Some(line) => {
            let line = line.strip_prefix("error: ").unwrap_or(line);
            let mut chars = line.chars();
            chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
        }
        None => match code {
            Some(0) => "sendme ended".into(),
            Some(code) => format!("sendme ended with code {code}"),
            None => "sendme was ended".into(),
        },
    }
}

/// `text` as a QR code: an SVG, dark on light.
pub(crate) fn qr_svg(text: &str) -> String {
    use qrcode::render::svg;
    qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L)
        .map(|code| code.render::<svg::Color>().min_dimensions(232, 232).dark_color(svg::Color("#000000")).light_color(svg::Color("#ffffff")).build())
        .unwrap_or_default()
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SendEvent {
    /// What's sent has been read: the ticket that receives it, and that ticket as a QR code (an SVG).
    Ready { ticket: String, size: String, qr: String },
    /// It ended by itself: why, as sendme said.
    Ended { message: String },
}

/// Offers `path` (a file or a folder) to another PC until it's stopped: its ticket comes to `on_event`
/// once sendme has read it through. Resolves to its id, for `share_stop`.
#[tauri::command]
pub async fn send_start(path: String, on_event: Channel<SendEvent>) -> Result<u64, String> {
    blocking(move || {
        if !Path::new(&path).exists() {
            return Err(format!("“{path}” isn't there any more"));
        }
        let mut command = crate::tools::command("sendme")?;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let work = std::env::temp_dir().join(format!("glint-send-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let held = lock(&work).inspect_err(|_| tidy(&work))?;
        command.args(["send", "--no-progress", "--"]).arg(&path).current_dir(&work).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let (job, mut child) = start(&mut command).inspect_err(|_| tidy(&work))?;
        let stopped = Arc::new(AtomicBool::new(false));
        running().push(Running { id, _job: job, stopped: stopped.clone(), work: work.clone(), _lock: held });
        let said = listen(child.stderr.take(), |_| {});
        let stdout = child.stdout.take();
        std::thread::spawn(move || {
            let mut size = String::new();
            for line in stdout.into_iter().flat_map(|s| BufReader::new(s).lines().map_while(Result::ok)) {
                if let Some(event) = send_said(&line, &mut size) {
                    let _ = on_event.send(event);
                }
            }
            let code = child.wait().ok().and_then(|s| s.code());
            let last = said.join().unwrap_or_default();
            running().retain(|r| r.id != id);
            tidy(&work);
            if !stopped.load(Ordering::Relaxed) {
                let _ = on_event.send(SendEvent::Ended { message: why(&last, code) });
            }
        });
        Ok(id)
    })
    .await
}

/// What a line of `sendme send` says: "imported file C:\a.txt, 24.00 MiB, hash …" (its size, kept),
/// then "sendme receive blob…" (its ticket).
fn send_said(line: &str, size: &mut String) -> Option<SendEvent> {
    if line.starts_with("imported ") {
        *size = line.rsplit(", ").nth(1).unwrap_or_default().to_string();
        return None;
    }
    let ticket = line.strip_prefix("sendme receive ")?.trim().to_string();
    let qr = qr_svg(&ticket);
    Some(SendEvent::Ready { ticket, size: size.clone(), qr })
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReceiveEvent {
    /// What's coming: how many files, and their size (as sendme says it, and in bytes).
    Getting { files: u64, size: String, bytes: u64 },
    /// How much of it is here.
    Progress { bytes: u64 },
    /// It's here, at `path`.
    Done { path: String },
    Failed { message: String },
}

/// The ticket in what's pasted: "blob…" as the sending PC shows it, or its whole "sendme receive …" line.
fn ticket_of(text: &str) -> Result<String, String> {
    let text = text.trim();
    let ticket = ["sendme receive", "sendme recv"].iter().find_map(|run| text.strip_prefix(run)).unwrap_or(text).trim().to_ascii_lowercase();
    // Base32, as sendme writes it; nothing else goes on its command line.
    if (60..=8192).contains(&ticket.len()) && ticket.starts_with("blob") && ticket.bytes().all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b)) {
        Ok(ticket)
    } else {
        Err("That isn't a sendme ticket: it's the long line starting with “blob” that the sending PC shows".into())
    }
}

/// "getting collection <hash> 2 files, 24.00 MiB": how many files, and their size.
fn getting(line: &str) -> Option<(u64, String)> {
    let rest = line.strip_prefix("getting collection ")?;
    let (_hash, rest) = rest.split_once(' ')?;
    let (files, rest) = rest.split_once(' ')?;
    let (_, size) = rest.split_once(", ")?;
    Some((files.parse().ok()?, size.trim().to_string()))
}

/// A size as sendme says it ("24.00 MiB"), in bytes: near enough for a progress bar.
fn bytes_of(size: &str) -> u64 {
    let mut words = size.split_whitespace();
    let Some(n) = words.next().and_then(|n| n.parse::<f64>().ok()) else { return 0 };
    let scale: u64 = match words.next().unwrap_or("B") {
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        "PiB" => 1 << 50,
        _ => 1,
    };
    (n * scale as f64) as u64
}

/// The size of what's in sendme's store in `work`: how much has come.
fn received(work: &Path) -> u64 {
    let mut total = 0;
    let mut stack: Vec<PathBuf> = std::fs::read_dir(work).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(".sendme-recv-")).map(|e| e.path()).collect();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            match entry.metadata() {
                Ok(m) if m.is_dir() => stack.push(entry.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

fn hide(path: &Path) {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};
    unsafe {
        let _ = SetFileAttributesW(&HSTRING::from(path.as_os_str()), FILE_ATTRIBUTE_HIDDEN);
    }
}

/// What came, moved out of `work` into `folder` under free names; the first's path.
fn move_out(work: &Path, folder: &Path) -> Result<String, String> {
    let mut first = None;
    for entry in std::fs::read_dir(work).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".sendme-recv-") || name == LOCK {
            continue;
        }
        let dir = entry.file_type().is_ok_and(|t| t.is_dir());
        let to = crate::fsx::move_numbered(&entry.path(), folder, &name, dir)?;
        first.get_or_insert(to);
    }
    first.map(|p| p.to_string_lossy().into_owned()).ok_or_else(|| "Nothing came".into())
}

/// Receives what `ticket` offers into `folder`, saying how far it is to `on_event`. Resolves to its id,
/// for `share_stop`.
#[tauri::command]
pub async fn receive_start(folder: String, ticket: String, on_event: Channel<ReceiveEvent>) -> Result<u64, String> {
    blocking(move || {
        let ticket = ticket_of(&ticket)?;
        let into = PathBuf::from(&folder);
        if crate::archive::is_read_only(&folder) || !into.is_dir() {
            return Err("What's received goes into a folder on a drive".into());
        }
        let mut command = crate::tools::command("sendme")?;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let work = into.join(format!(".glint-receiving-{}-{id}", std::process::id()));
        std::fs::create_dir(&work).map_err(crate::fsx::file_error)?;
        hide(&work);
        note(&work);
        let held = lock(&work).inspect_err(|_| tidy(&work))?;
        command.args(["receive", "--no-progress", "--"]).arg(&ticket).current_dir(&work).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let (job, mut child) = start(&mut command).inspect_err(|_| tidy(&work))?;
        let stopped = Arc::new(AtomicBool::new(false));
        running().push(Running { id, _job: job, stopped: stopped.clone(), work: work.clone(), _lock: held });
        let events = on_event.clone();
        let said = listen(child.stderr.take(), move |line| {
            if let Some((files, size)) = getting(line) {
                let _ = events.send(ReceiveEvent::Getting { files, bytes: bytes_of(&size), size });
            }
        });
        // Its output only says where it's put ("exporting to …"); read so it never waits on it.
        let output = listen(child.stdout.take(), |_| {});
        let ended = Arc::new(AtomicBool::new(false));
        {
            let (ended, work, events) = (ended.clone(), work.clone(), on_event.clone());
            std::thread::spawn(move || {
                let mut last = 0;
                while !ended.load(Ordering::Relaxed) {
                    let now = received(&work);
                    if now != last {
                        last = now;
                        let _ = events.send(ReceiveEvent::Progress { bytes: now });
                    }
                    std::thread::sleep(Duration::from_millis(400));
                }
            });
        }
        std::thread::spawn(move || {
            let status = child.wait();
            ended.store(true, Ordering::Relaxed);
            let last = said.join().unwrap_or_default();
            let _ = output.join();
            running().retain(|r| r.id != id);
            if stopped.load(Ordering::Relaxed) {
                return tidy(&work);
            }
            let event = match status {
                Ok(status) if status.success() => match move_out(&work, &into) {
                    Ok(path) => ReceiveEvent::Done { path },
                    Err(message) => ReceiveEvent::Failed { message },
                },
                status => ReceiveEvent::Failed { message: why(&last, status.ok().and_then(|s| s.code())) },
            };
            tidy(&work);
            let _ = on_event.send(event);
        });
        Ok(id)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_sendme_says() {
        let mut size = String::new();
        assert_eq!(send_said(r"imported directory C:\a, b\photos, 24.00 MiB, hash 2b81d0690b68", &mut size), None);
        assert_eq!(size, "24.00 MiB");
        let Some(SendEvent::Ready { ticket, size, qr }) = send_said("sendme receive blobadn5sy7eq5z6qoyqqn7ceg7c", &mut size) else { panic!() };
        assert_eq!((ticket.as_str(), size.as_str()), ("blobadn5sy7eq5z6qoyqqn7ceg7c", "24.00 MiB"));
        assert!(qr.starts_with("<?xml") && qr.contains("<svg"));
        assert_eq!(send_said("press c to copy command to clipboard", &mut String::new()), None);

        assert_eq!(getting("getting collection 2b81d0690b68606115 2 files, 24.00 MiB"), Some((2, "24.00 MiB".into())));
        assert_eq!(getting("connected directly"), None);
        assert_eq!(bytes_of("24.00 MiB"), 24 << 20);
        assert_eq!(bytes_of("1.50 GiB"), 3 << 29);
        assert_eq!(bytes_of("812 B"), 812);

        let last = ["getting collection 2b81 2 files, 24.00 MiB".to_string(), "error: target C:\\a\\one.txt already exists".into()];
        assert_eq!(why(&last, Some(1)), "Target C:\\a\\one.txt already exists");
        let clap = ["error: invalid value 'blobx' for '<TICKET>': non-zero trailing bits at 9".to_string(), "For more information, try '--help'.".into()];
        assert_eq!(why(&clap, Some(2)), "Invalid value 'blobx' for '<TICKET>': non-zero trailing bits at 9");
        assert_eq!(why(&[], Some(3)), "sendme ended with code 3");
    }

    #[test]
    fn takes_only_a_ticket() {
        let ticket = format!("blob{}", "adn5sy7eq5z6qoyq".repeat(5));
        assert_eq!(ticket_of(&format!("  {ticket}\r\n")), Ok(ticket.clone()));
        assert_eq!(ticket_of(&format!("sendme receive {ticket}")), Ok(ticket.clone()));
        assert_eq!(ticket_of(&ticket.to_uppercase()), Ok(ticket.clone()));
        // Nothing that could be read as an option, or anything but base32.
        assert!(ticket_of(&format!("--relay=https://example.com {ticket}")).is_err());
        assert!(ticket_of(&format!("{ticket}0")).is_err());
        assert!(ticket_of("blobshort").is_err());
        assert!(ticket_of("").is_err());
    }

    #[test]
    fn a_runs_folder_is_held_while_it_runs() {
        let work = std::env::temp_dir().join(format!("glint-share-lock-test-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let held = lock(&work).unwrap();
        assert!(std::fs::remove_file(work.join(LOCK)).is_err());
        drop(held);
        assert!(std::fs::remove_file(work.join(LOCK)).is_ok());
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn moves_what_came_under_free_names() {
        let dir = std::env::temp_dir().join(format!("glint-share-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let work = dir.join(".glint-receiving-1");
        std::fs::create_dir_all(work.join(".sendme-recv-ab12").join("data")).unwrap();
        std::fs::create_dir_all(work.join("photos")).unwrap();
        std::fs::write(work.join("photos").join("a.jpg"), "a").unwrap();
        std::fs::write(work.join(".sendme-recv-ab12").join("data").join("x"), "12345").unwrap();
        std::fs::write(work.join(LOCK), "").unwrap();
        assert_eq!(received(&work), 5);
        std::fs::create_dir_all(dir.join("photos")).unwrap();
        let moved = move_out(&work, &dir).unwrap();
        assert_eq!(moved, dir.join("photos (2)").to_string_lossy());
        assert!(dir.join("photos (2)").join("a.jpg").is_file() && dir.join("photos").is_dir());
        assert_eq!(move_out(&work, &dir), Err("Nothing came".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
