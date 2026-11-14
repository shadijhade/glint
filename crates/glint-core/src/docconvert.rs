//! Convert document: pandoc turns Word, OpenDocument, Markdown, HTML, EPUB, LaTeX, RTF, reStructuredText,
//! Org, Textile, Typst, FictionBook and Jupyter notebooks into one another, and Markdown into PowerPoint;
//! PDFs come from Typst typesetting what pandoc writes, with no LaTeX or Word. pandoc runs with
//! `--sandbox`, reading only the document it's given, and Typst compiles inside the job's work folder (its
//! root), so neither reads anything else or goes online. Through actions.rs: beside the originals or in
//! their place, and Undo takes it back. A document converted to a text format (Markdown, LaTeX…) gets its
//! pictures in a folder beside it ("<name> files"); HTML keeps them inside.
//!
//! A document that's mostly Arabic, Persian or Hebrew is marked so (`lang`, `dir`), so its PDF, Word and
//! HTML read right to left.
//!

use crate::actions::{self, Job, Output, Progress, Report};
use crate::fsx::blocking;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use crate::host::ipc::Channel;
use crate::host::Window;

static STOP: AtomicBool = AtomicBool::new(false);

pub fn doc_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

/// pandoc's reader for a document, by its extension.
pub fn reader(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    Some(match ext.as_str() {
        "docx" => "docx",
        "odt" => "odt",
        "md" | "markdown" | "mdown" | "mkd" => "markdown",
        "html" | "htm" | "xhtml" => "html",
        "epub" => "epub",
        "tex" | "latex" => "latex",
        "rtf" => "rtf",
        "rst" => "rst",
        "org" => "org",
        "textile" => "textile",
        "ipynb" => "ipynb",
        "typ" => "typst",
        "fb2" => "fb2",
        _ => return None,
    })
}

/// What a document becomes.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Docx,
    Odt,
    Pdf,
    Markdown,
    Html,
    Epub,
    Latex,
    Rtf,
    Rst,
    Org,
    Ipynb,
    Pptx,
    Typst,
    Text,
}

impl Target {
    /// pandoc's writer, and the extension of what's made.
    fn writer(self) -> (&'static str, &'static str) {
        match self {
            Target::Docx => ("docx", "docx"),
            Target::Odt => ("odt", "odt"),
            Target::Pdf => ("typst", "pdf"),
            Target::Markdown => ("gfm", "md"),
            Target::Html => ("html5", "html"),
            Target::Epub => ("epub3", "epub"),
            Target::Latex => ("latex", "tex"),
            Target::Rtf => ("rtf", "rtf"),
            Target::Rst => ("rst", "rst"),
            Target::Org => ("org", "org"),
            Target::Ipynb => ("ipynb", "ipynb"),
            Target::Pptx => ("pptx", "pptx"),
            Target::Typst => ("typst", "typ"),
            Target::Text => ("plain", "txt"),
        }
    }

    /// A text format: its pictures go into a folder beside it.
    fn text(self) -> bool {
        matches!(self, Target::Markdown | Target::Latex | Target::Rst | Target::Org | Target::Typst | Target::Text)
    }

    /// A format that reads right to left when its document does.
    fn directed(self) -> bool {
        matches!(self, Target::Pdf | Target::Docx | Target::Odt | Target::Html | Target::Epub | Target::Pptx)
    }
}

/// Converts `files` into `target`, three at a time.
pub async fn doc_convert(window: Window, files: Vec<String>, target: Target, output: Output, on_progress: Channel<Progress>) -> Result<Report, String> {
    let owner = crate::shell::owner(&window);
    STOP.store(false, Ordering::SeqCst);
    blocking(move || {
        crate::tools::command("pandoc")?;
        if target == Target::Pdf {
            crate::tools::command("typst")?;
        }
        let jobs: Vec<Job> = files.iter().filter(|f| reader(Path::new(f)).is_some()).map(Job::one).collect();
        if jobs.is_empty() {
            return Err("None of them is a document pandoc reads".into());
        }
        actions::run(owner, "convert-doc", &jobs, &output, 3, &STOP, &|p| drop(on_progress.send(p)), |job, part, work| convert(&job.inputs[0], target, part, work))
    })
    .await
}

/// One document into `target`, in `work`: what was made (the document, and its pictures' folder).
fn convert(file: &Path, target: Target, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<Vec<PathBuf>, String> {
    let from = reader(file).ok_or("pandoc doesn't read this kind of file")?;
    let (writer, ext) = target.writer();
    let own_ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if own_ext == ext || (from == "markdown" && target == Target::Markdown) {
        return Err(format!("It's already a .{own_ext} file"));
    }
    // A .typ file is typeset by Typst itself, its folder its root (its pictures and includes are there).
    if from == "typst" && target == Target::Pdf {
        let out = actions::renamed(work, file, "pdf");
        compile_typst(file, file.parent().unwrap_or(Path::new(".")), &out)?;
        return Ok(vec![out]);
    }
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let language = if target.directed() { right_to_left(file, from, work) } else { None };
    part(0.2);
    let mut pandoc = pandoc(file, from, work)?;
    pandoc.args(["-t", writer]);
    if let Some(lang) = language {
        pandoc.arg("-M").arg(format!("lang={lang}")).args(["-M", "dir=rtl"]);
    }
    let made = match target {
        Target::Pdf => {
            let source = work.join("document.typ");
            pandoc.args(["-s", "--extract-media=media", "-o"]).arg(&source);
            actions::run_tool(pandoc, "pandoc", &STOP, |_| {})?;
            part(0.6);
            let out = actions::renamed(work, file, "pdf");
            compile_typst(&source, work, &out)?;
            vec![out]
        }
        _ => {
            let out = work.join(format!("{stem}.{ext}"));
            match target {
                Target::Html => drop(pandoc.args(["-s", "--embed-resources"])),
                Target::Latex | Target::Rtf | Target::Typst => drop(pandoc.arg("-s")),
                _ => {}
            }
            let pictures = format!("{stem} files");
            if target.text() {
                pandoc.arg(format!("--extract-media={pictures}"));
            }
            pandoc.arg("-o").arg(&out);
            actions::run_tool(pandoc, "pandoc", &STOP, |_| {})?;
            let folder = work.join(&pictures);
            let mut made = vec![out];
            if folder.is_dir() && fs::read_dir(&folder).map(|mut d| d.next().is_some()).unwrap_or(false) {
                made.push(folder);
            }
            made
        }
    };
    Ok(made)
}

/// pandoc, sandboxed, reading `file` as `from`, in `work` (where relative names land).
fn pandoc(file: &Path, from: &str, work: &Path) -> Result<std::process::Command, String> {
    let mut pandoc = crate::tools::command("pandoc")?;
    pandoc.current_dir(work).args(["--sandbox", "-f", from]).arg(file);
    Ok(pandoc)
}

/// Typst typesets `source` into `out`, reading only what's under `root`. One that imports Typst
/// packages is refused: they'd download from the internet.
fn compile_typst(source: &Path, root: &Path, out: &Path) -> Result<(), String> {
    let text = fs::read_to_string(source).unwrap_or_default();
    if imports_packages(&text) {
        return Err("It uses Typst packages, which download from the internet: open it with Typst itself".into());
    }
    let mut typst = crate::tools::command("typst")?;
    typst.arg("compile").arg("--root").arg(root).arg(source).arg(out);
    actions::run_tool(typst, "Typst", &STOP, |_| {}).map(drop)
}

fn imports_packages(text: &str) -> bool {
    text.lines().any(|l| l.contains("import") && l.contains("\"@"))
}

/// "ar" or "he" (with Persian and Urdu as Arabic script) when the document is mostly written right to
/// left, from its text as pandoc reads it.
fn right_to_left(file: &Path, from: &str, work: &Path) -> Option<&'static str> {
    let text = work.join("text-for-direction.txt");
    let mut pandoc = pandoc(file, from, work).ok()?;
    pandoc.args(["-t", "plain", "-o"]).arg(&text);
    actions::run_tool(pandoc, "pandoc", &STOP, |_| {}).ok()?;
    let found = direction_of(&fs::read_to_string(&text).unwrap_or_default());
    let _ = fs::remove_file(&text);
    found
}

fn direction_of(text: &str) -> Option<&'static str> {
    let (mut arabic, mut hebrew, mut latin) = (0usize, 0usize, 0usize);
    for c in text.chars() {
        match c {
            '\u{0600}'..='\u{06FF}' | '\u{0750}'..='\u{077F}' | '\u{08A0}'..='\u{08FF}' | '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}' => arabic += 1,
            '\u{0590}'..='\u{05FF}' => hebrew += 1,
            c if c.is_ascii_alphabetic() => latin += 1,
            _ => {}
        }
    }
    if arabic > latin && arabic >= hebrew {
        Some("ar")
    } else if hebrew > latin {
        Some("he")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_pandoc_reads() {
        assert_eq!(reader(Path::new(r"C:\d\Report.DOCX")), Some("docx"));
        assert_eq!(reader(Path::new(r"C:\d\notes.md")), Some("markdown"));
        assert_eq!(reader(Path::new(r"C:\d\paper.tex")), Some("latex"));
        assert_eq!(reader(Path::new(r"C:\d\old.doc")), None);
    }

    #[test]
    fn writes_each_format() {
        assert_eq!(Target::Markdown.writer(), ("gfm", "md"));
        assert_eq!(Target::Pdf.writer(), ("typst", "pdf"));
        assert!(Target::Markdown.text() && !Target::Html.text() && !Target::Pdf.text());
        assert!(Target::Pdf.directed() && !Target::Markdown.directed());
    }

    #[test]
    fn tells_right_to_left_documents() {
        assert_eq!(direction_of("تقرير الرحلة: هذا نص عربي مع English words"), Some("ar"));
        assert_eq!(direction_of("שלום עולם, זה מסמך"), Some("he"));
        assert_eq!(direction_of("A report with one word: مرحبا"), None);
        assert_eq!(direction_of(""), None);
    }

    #[test]
    fn refuses_typst_packages() {
        assert!(imports_packages("#import \"@preview/cetz:0.3.4\": canvas"));
        assert!(!imports_packages("#import \"template.typ\": conf\n= Title"));
    }

    /// Every document in SAMPLES converted to PDF, Word, Markdown and HTML with the pandoc and Typst in
    /// TOOLS_DIR (tools\<id>\<program>).
    #[test]
    #[ignore]
    fn converts_real_documents() {
        crate::tools::set_dir(PathBuf::from(std::env::var("TOOLS_DIR").expect("TOOLS_DIR")));
        let samples = PathBuf::from(std::env::var("SAMPLES").expect("SAMPLES"));
        let mut failed = Vec::new();
        for file in fs::read_dir(&samples).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_file() && reader(p).is_some()) {
            for target in [Target::Pdf, Target::Docx, Target::Markdown, Target::Html, Target::Epub] {
                let work = std::env::temp_dir().join(format!("glint-doc-{}", std::process::id())).join(format!("{target:?}"));
                let _ = fs::remove_dir_all(&work);
                fs::create_dir_all(&work).unwrap();
                match convert(&file, target, &|_| {}, &work) {
                    Ok(made) => println!("{} → {target:?}: {:?}", file.display(), made.iter().map(|m| (m.file_name().unwrap().to_owned(), fs::metadata(m).map_or(0, |x| x.len()))).collect::<Vec<_>>()),
                    Err(why) if why.starts_with("It's already") => {}
                    Err(why) => failed.push(format!("{} → {target:?}: {why}", file.display())),
                }
            }
        }
        assert!(failed.is_empty(), "{failed:#?}");
    }
}
