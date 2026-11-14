//! PDF tools, with pdfcpu (Apache-2.0, a tool Glint installs): merging PDFs, splitting one, keeping or
//! taking out pages, turning them, making a PDF smaller (what's repeated or unused in it taken out), its
//! pictures taken out, passwords put on and taken off (AES-256), and a text stamp across its pages. Each
//! runs through actions.rs: what's made lands beside the PDFs (or in their place), and Undo takes it back.
//! Nothing is sent anywhere.

use crate::actions::{self, Job, Output, Progress, Report};
use crate::fsx::blocking;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::host::ipc::Channel;
use crate::host::Window;

static STOP: AtomicBool = AtomicBool::new(false);

pub fn pdf_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

#[derive(Clone, Debug, PartialEq)]
pub enum PdfTask {
    /// The PDFs, in the order given, as one named `name` (without .pdf).
    Merge { name: String },
    /// Into files of `every` pages each (1: a file a page).
    Split { every: u32 },
    /// Only these pages (pdfcpu's page list: "1-3,5,even").
    Keep { pages: String },
    /// Without these pages.
    Remove { pages: String },
    /// These pages ("" for all) turned clockwise by `degrees` (90, 180 or 270).
    Rotate { pages: String, degrees: u32 },
    /// Smaller: what's repeated or unused in it taken out.
    Optimize,
    /// The pictures in it, as files.
    Images,
    /// Locked with `password` (AES-256) to open it.
    Encrypt { password: String },
    /// Unlocked with its `password`.
    Decrypt { password: String },
    /// `text` across each page, at `opacity` (0.1 to 1), diagonal or level.
    Stamp { text: String, opacity: f32, diagonal: bool },
}

fn pdfcpu() -> Result<Command, String> {
    let mut command = crate::tools::command("pdfcpu")?;
    // Its own settings folder isn't needed (it makes one in AppData otherwise), and it never goes online.
    command.args(["-c", "disable", "--offline"]);
    Ok(command)
}

/// What a PDF is: its pages and whether it's locked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PdfInfo {
    pub pages: u32,
    pub encrypted: bool,
    pub version: String,
}

pub async fn pdf_info(path: String) -> Result<PdfInfo, String> {
    blocking(move || {
        let mut command = pdfcpu()?;
        command.args(["info", "-j", "--"]).arg(&path);
        match actions::run_tool(command, "pdfcpu", &AtomicBool::new(false), |_| {}) {
            Ok(said) => Ok(read_info(&said.stdout)),
            // A PDF locked to open says only that.
            Err(e) if e.contains("password") => Ok(PdfInfo { encrypted: true, ..PdfInfo::default() }),
            Err(e) => Err(e),
        }
    })
    .await
}

fn read_info(json: &str) -> PdfInfo {
    let value: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let info = value["infos"].get(0).cloned().unwrap_or(value);
    PdfInfo {
        pages: info["pageCount"].as_u64().unwrap_or(0) as u32,
        encrypted: info["encrypted"].as_bool().unwrap_or(false),
        version: info["version"].as_str().unwrap_or_default().to_string(),
    }
}

/// A page list pdfcpu takes: numbers and ranges ("1-3", "7-" for 7 to the end, "-5"), "even" and
/// "odd", "l" for the last page, "!" before one to leave it out, joined by commas.
pub fn valid_pages(pages: &str) -> bool {
    !pages.trim().is_empty()
        && pages.split(',').map(str::trim).all(|p| {
            let p = p.strip_prefix('!').unwrap_or(p);
            matches!(p, "even" | "odd" | "l") || (!p.is_empty() && p.chars().all(|c| c.is_ascii_digit() || c == '-' || c == 'l') && p.matches('-').count() <= 1)
        })
}

/// Runs `task` on `files`: merging makes one job of them all, everything else a job a PDF.
pub async fn pdf_run(window: Window, files: Vec<String>, task: PdfTask, output: Output, on_progress: Channel<Progress>) -> Result<Report, String> {
    let owner = crate::shell::owner(&window);
    STOP.store(false, Ordering::SeqCst);
    blocking(move || {
        check(&task)?;
        pdfcpu()?;
        let jobs: Vec<Job> = match task {
            PdfTask::Merge { .. } => vec![Job { inputs: files.iter().map(PathBuf::from).collect() }],
            _ => files.iter().map(Job::one).collect(),
        };
        actions::run(owner, "pdf", &jobs, &output, 2, &STOP, &|p| drop(on_progress.send(p)), |job, _, work| make(&task, &job.inputs, work))
    })
    .await
}

fn check(task: &PdfTask) -> Result<(), String> {
    match task {
        PdfTask::Merge { name } if !actions::valid_name(name, false) => Err(format!("“{name}” can't be a file's name")),
        PdfTask::Keep { pages } | PdfTask::Remove { pages } if !valid_pages(pages) => Err(format!("“{pages}” isn't a list of pages (say 1-3, 5, 8-)")),
        PdfTask::Rotate { pages, .. } if !pages.trim().is_empty() && !valid_pages(pages) => Err(format!("“{pages}” isn't a list of pages")),
        PdfTask::Rotate { degrees, .. } if ![90, 180, 270].contains(degrees) => Err("A page turns by 90, 180 or 270 degrees".into()),
        PdfTask::Encrypt { password } | PdfTask::Decrypt { password } if password.is_empty() => Err("The password is empty".into()),
        PdfTask::Stamp { text, .. } if text.trim().is_empty() => Err("The stamp has no text".into()),
        PdfTask::Split { every } if *every == 0 => Err("A part has at least one page".into()),
        _ => Ok(()),
    }
}

/// pdfcpu's arguments for `task` on `inputs`, making its result(s) in `work`: the arguments, and the file
/// it makes (None when it makes a folder's worth, which is everything left in `work`).
fn arguments(task: &PdfTask, inputs: &[PathBuf], work: &Path) -> (Vec<OsString>, Option<PathBuf>) {
    let first = &inputs[0];
    let stem = first.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let out_named = |suffix: &str| work.join(format!("{stem}{suffix}.pdf"));
    let mut args: Vec<OsString> = Vec::new();
    let made = match task {
        PdfTask::Merge { name } => {
            let out = work.join(format!("{name}.pdf"));
            args.extend(["merge".into(), "--".into(), out.clone().into()]);
            args.extend(inputs.iter().map(|p| p.into()));
            Some(out)
        }
        PdfTask::Split { every } => {
            args.extend(["split".into(), "--".into(), first.into(), work.into(), every.to_string().into()]);
            None
        }
        PdfTask::Keep { pages } => {
            let out = out_named(&format!(" (pages {})", short_list(pages)));
            args.extend(["trim".into(), "--pages".into(), pages.replace(' ', "").into(), "--".into(), first.into(), out.clone().into()]);
            Some(out)
        }
        PdfTask::Remove { pages } => {
            let out = out_named(" (pages taken out)");
            args.extend(["pages".into(), "remove".into(), "--pages".into(), pages.replace(' ', "").into(), "--".into(), first.into(), out.clone().into()]);
            Some(out)
        }
        PdfTask::Rotate { pages, degrees } => {
            let out = out_named(" (turned)");
            args.push("rotate".into());
            if !pages.trim().is_empty() {
                args.extend(["--pages".into(), pages.replace(' ', "").into()]);
            }
            args.extend(["--".into(), first.into(), degrees.to_string().into(), out.clone().into()]);
            Some(out)
        }
        PdfTask::Optimize => {
            let out = out_named(" (smaller)");
            args.extend(["optimize".into(), "--".into(), first.into(), out.clone().into()]);
            Some(out)
        }
        PdfTask::Images => {
            args.extend(["extract".into(), "--mode".into(), "image".into(), "--".into(), first.into(), work.into()]);
            None
        }
        // The password goes in a file in the work folder (written by `make`), not on the command line,
        // where other programs could read it.
        PdfTask::Encrypt { .. } => {
            let out = out_named(" (locked)");
            let pw = work.join(PASSWORD_FILE);
            args.extend(["encrypt".into(), "--mode".into(), "aes".into(), "--key".into(), "256".into(), "--upw-file".into(), pw.clone().into(), "--opw-file".into(), pw.into(), "--".into(), first.into(), out.clone().into()]);
            Some(out)
        }
        PdfTask::Decrypt { .. } => {
            let out = out_named(" (unlocked)");
            let pw = work.join(PASSWORD_FILE);
            args.extend(["decrypt".into(), "--upw-file".into(), pw.clone().into(), "--opw-file".into(), pw.into(), "--".into(), first.into(), out.clone().into()]);
            Some(out)
        }
        PdfTask::Stamp { text, opacity, diagonal } => {
            let out = out_named(" (stamped)");
            let description = format!("font:Helvetica, points:48, scale:0.8 rel, rot:{}, op:{:.2}, fillc:#808080, pos:c", if *diagonal { 45 } else { 0 }, opacity.clamp(0.05, 1.0));
            args.extend(["stamp".into(), "add".into(), "--mode".into(), "text".into(), "--".into(), text.into(), description.into(), first.into(), out.clone().into()]);
            Some(out)
        }
    };
    (args, made)
}

/// A page list as part of a file's name ("1-3,5").
fn short_list(pages: &str) -> String {
    let compact: String = pages.chars().filter(|c| !c.is_whitespace()).collect();
    let cut: String = compact.chars().take(24).collect();
    if cut.len() < compact.len() {
        format!("{cut}…")
    } else {
        cut
    }
}

/// Where a password is handed to pdfcpu, in the job's work folder; removed once it has run.
const PASSWORD_FILE: &str = ".password";

fn make(task: &PdfTask, inputs: &[PathBuf], work: &Path) -> Result<Vec<PathBuf>, String> {
    let (args, made) = arguments(task, inputs, work);
    let password = match task {
        PdfTask::Encrypt { password } | PdfTask::Decrypt { password } => Some(work.join(PASSWORD_FILE)).filter(|f| std::fs::write(f, password).is_ok()),
        _ => None,
    };
    let mut command = pdfcpu()?;
    command.args(&args);
    let ran = actions::run_tool(command, "pdfcpu", &STOP, |_| {});
    if let Some(file) = password {
        let _ = std::fs::remove_file(file);
    }
    ran.map_err(|e| plain_error(&e))?;
    match made {
        Some(file) if file.is_file() => Ok(vec![file]),
        Some(_) => Err("pdfcpu made nothing".into()),
        None => {
            let mut files: Vec<PathBuf> = std::fs::read_dir(work).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
            files.sort();
            if files.is_empty() {
                return Err(match task {
                    PdfTask::Images => "There are no pictures in it".into(),
                    _ => "pdfcpu made nothing".into(),
                });
            }
            Ok(files)
        }
    }
}

/// pdfcpu's errors, said simply where they're common.
fn plain_error(said: &str) -> String {
    let lower = said.to_lowercase();
    if lower.contains("password") && (lower.contains("invalid") || lower.contains("wrong") || lower.contains("correct")) {
        "The password isn't right".into()
    } else if lower.contains("encrypted") && lower.contains("password") {
        "It's locked with a password: take it off first".into()
    } else {
        said.trim_start_matches("pdfcpu: ").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(task: &PdfTask, inputs: &[&str]) -> String {
        let inputs: Vec<PathBuf> = inputs.iter().map(PathBuf::from).collect();
        let (args, _) = arguments(task, &inputs, Path::new(r"C:\w"));
        args.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn asks_pdfcpu_for_what_was_chosen() {
        assert_eq!(line(&PdfTask::Merge { name: "All".into() }, &[r"C:\d\a.pdf", r"C:\d\b.pdf"]), r"merge -- C:\w\All.pdf C:\d\a.pdf C:\d\b.pdf");
        assert_eq!(line(&PdfTask::Split { every: 1 }, &[r"C:\d\a.pdf"]), r"split -- C:\d\a.pdf C:\w 1");
        assert_eq!(line(&PdfTask::Keep { pages: "1-3, 5".into() }, &[r"C:\d\a.pdf"]), r"trim --pages 1-3,5 -- C:\d\a.pdf C:\w\a (pages 1-3,5).pdf");
        assert_eq!(line(&PdfTask::Rotate { pages: String::new(), degrees: 90 }, &[r"C:\d\-x.pdf"]), r"rotate -- C:\d\-x.pdf 90 C:\w\-x (turned).pdf");
        // The password never shows on the command line.
        assert_eq!(line(&PdfTask::Encrypt { password: "s3cret".into() }, &[r"C:\d\a.pdf"]), r"encrypt --mode aes --key 256 --upw-file C:\w\.password --opw-file C:\w\.password -- C:\d\a.pdf C:\w\a (locked).pdf");
        assert!(line(&PdfTask::Stamp { text: "DRAFT".into(), opacity: 0.3, diagonal: true }, &[r"C:\d\a.pdf"]).starts_with("stamp add --mode text -- DRAFT font:Helvetica, points:48, scale:0.8 rel, rot:45, op:0.30"));
    }

    #[test]
    fn checks_page_lists() {
        assert!(valid_pages("1-3, 5, 8-") && valid_pages("even") && valid_pages("!2") && valid_pages("l") && valid_pages("-5"));
        assert!(!valid_pages("") && !valid_pages("one") && !valid_pages("1--3") && !valid_pages("1;2"));
        assert!(check(&PdfTask::Rotate { pages: String::new(), degrees: 45 }).is_err());
        assert!(check(&PdfTask::Merge { name: "a/b".into() }).is_err());
        assert_eq!(short_list("1-3, 5, 7, 9, 11, 13, 15, 17, 19"), "1-3,5,7,9,11,13,15,17,19");
    }

    #[test]
    fn reads_info() {
        let json = r#"{"header":{"version":"pdfcpu v0.16.4"},"infos":[{"source":"a.pdf","version":"1.7","pageCount":12,"encrypted":false}]}"#;
        assert_eq!(read_info(json), PdfInfo { pages: 12, encrypted: false, version: "1.7".into() });
        assert_eq!(plain_error("pdfcpu: please provide the correct password"), "The password isn't right");
    }
}
