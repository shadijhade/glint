//! Remove personal info, and when photos were taken: what files say about themselves beyond their content
//! (where a photo was taken, the camera and its serial number, who made a document and with what, its
//! editing history), read and taken out before they're shared. ExifTool (a tool Glint installs) does
//! photos, videos and PDFs, keeping a picture's orientation and color profile; a PDF is then rewritten by
//! pdfcpu when it's there, as ExifTool only adds an update that hides the old details. Word, Excel and
//! PowerPoint files (which ExifTool only reads) are cleaned here: their author, last editor, company and
//! the like blanked, everything else copied as it is. Each runs through actions.rs, so Undo takes it back.

use crate::actions::{self, Job, Output, Progress, Report};
use crate::fsx::blocking;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::host::ipc::Channel;
use crate::host::Window;

/// What ExifTool writes (of what people share).
pub const EXIFTOOL_WRITES: &[&str] = &[
    "jpg", "jpeg", "jpe", "png", "tif", "tiff", "webp", "heic", "heif", "avif", "jxl", "gif", "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf", "psd", "mp4", "m4v", "mov", "3gp", "m4a", "pdf",
];
/// Office's files, cleaned here.
pub const OFFICE: &[&str] = &["docx", "docm", "xlsx", "xlsm", "pptx", "pptm"];

static STOP: AtomicBool = AtomicBool::new(false);

pub fn meta_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

fn ext(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn exiftool() -> Result<Command, String> {
    crate::tools::command("exiftool")
}

/// A detail a file carries, and what it says about someone.
#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    pub group: String,
    pub name: String,
    pub value: String,
    /// "place", "device", "people", "history", "dates" or "other".
    pub kind: &'static str,
}

/// What a tag tells, by its name.
fn kind_of(group: &str, name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    let g = group.to_ascii_lowercase();
    if g.starts_with("gps") || n.contains("gps") || n.contains("location") || n.contains("city") || n.contains("country") || n.contains("sublocation") {
        "place"
    } else if n.contains("serial") || n == "make" || n == "model" || n.contains("lens") || n.contains("camera") || n.contains("hostcomputer") || n.contains("software") || n.contains("creatortool") {
        "device"
    } else if ["artist", "author", "creator", "byline", "owner", "copyright", "lastmodifiedby", "company", "manager", "rights", "credit", "contact"].iter().any(|k| n.contains(k)) {
        "people"
    } else if g.contains("xmpmm") || n.contains("history") || n.contains("documentid") || n.contains("instanceid") || n.contains("revision") || n.contains("derivedfrom") || n.contains("totaledittime") {
        "history"
    } else if n.contains("date") || n.contains("time") {
        "dates"
    } else {
        "other"
    }
}

/// The details a file carries, from ExifTool (or, for Office files, its document properties).
pub async fn meta_read(path: String) -> Result<Vec<Tag>, String> {
    blocking(move || {
        let path = Path::new(&path);
        if OFFICE.contains(&ext(path).as_str()) {
            return office_tags(path);
        }
        let mut command = exiftool()?;
        command.args(["-j", "-G1", "-s", "-a", "-charset", "filename=utf8", "--"]).arg(path);
        let said = actions::run_tool(command, "ExifTool", &AtomicBool::new(false), |_| {})?;
        Ok(read_tags(&said.stdout))
    })
    .await
}

/// ExifTool's JSON (-j -G1 -s), without what isn't stored in the file (its name, size, dates on disk,
/// what ExifTool works out).
fn read_tags(json: &str) -> Vec<Tag> {
    let value: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let Some(object) = value.get(0).and_then(|v| v.as_object()) else { return Vec::new() };
    let skip = ["System", "File", "Composite", "ExifTool"];
    let mut tags: Vec<Tag> = object
        .iter()
        .filter_map(|(key, value)| {
            let (group, name) = key.split_once(':')?;
            if skip.contains(&group) || name.starts_with("SourceFile") {
                return None;
            }
            let value = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if value.starts_with("(Binary data") && !name.contains("Thumbnail") {
                return None;
            }
            Some(Tag { kind: kind_of(group, name), group: group.to_string(), name: name.to_string(), value: value.chars().take(200).collect() })
        })
        .collect();
    let order = |k: &str| ["place", "people", "device", "history", "dates", "other"].iter().position(|x| *x == k).unwrap_or(9);
    tags.sort_by_key(|t| order(t.kind));
    tags
}

#[derive(Clone, Debug, PartialEq)]
pub enum MetaTask {
    /// Everything personal out (a picture keeps its orientation and color profile).
    Scrub,
    /// When it was taken (and made), as "2024:06:01 12:00:00".
    SetDate { when: String },
    /// When it was taken moved by this many minutes (earlier when negative): a camera's clock that was off.
    ShiftDate { minutes: i64 },
}

pub async fn meta_run(window: Window, files: Vec<String>, task: MetaTask, output: Output, on_progress: Channel<Progress>) -> Result<Report, String> {
    let owner = crate::shell::owner(&window);
    STOP.store(false, Ordering::SeqCst);
    blocking(move || {
        if let MetaTask::SetDate { when } = &task {
            if !valid_date(when) {
                return Err(format!("“{when}” isn't a date and time"));
            }
        }
        let jobs: Vec<Job> = files.iter().map(Job::one).collect();
        actions::run(owner, "meta", &jobs, &output, 3, &STOP, &|p| drop(on_progress.send(p)), |job, _, work| make(&task, &job.inputs[0], work).map(|f| vec![f]))
    })
    .await
}

/// "2024:06:01 12:00:00".
fn valid_date(when: &str) -> bool {
    let b = when.as_bytes();
    b.len() == 19 && b.iter().enumerate().all(|(i, c)| match i {
        4 | 7 => *c == b':',
        10 => *c == b' ',
        13 | 16 => *c == b':',
        _ => c.is_ascii_digit(),
    })
}

fn make(task: &MetaTask, file: &Path, work: &Path) -> Result<PathBuf, String> {
    let kind = ext(file);
    let out = work.join(file.file_name().ok_or("no name")?);
    if OFFICE.contains(&kind.as_str()) {
        return match task {
            MetaTask::Scrub => scrub_office(file, &out).map(|()| out),
            _ => Err("Office files have no date taken".into()),
        };
    }
    if !EXIFTOOL_WRITES.contains(&kind.as_str()) {
        return Err(format!("ExifTool can't write .{kind} files"));
    }
    let mut command = exiftool()?;
    command.args(["-q", "-q", "-charset", "filename=utf8"]);
    match task {
        MetaTask::Scrub => {
            command.args(["-all=", "-tagsfromfile", "@", "-icc_profile", "-orientation"]);
        }
        MetaTask::SetDate { when } => {
            command.arg(format!("-AllDates={when}"));
            if matches!(kind.as_str(), "mp4" | "m4v" | "mov" | "3gp") {
                command.args(["-api", "QuickTimeUTC"]);
            }
        }
        MetaTask::ShiftDate { minutes } => {
            let (sign, m) = if *minutes < 0 { ('-', -minutes) } else { ('+', *minutes) };
            command.arg(format!("-AllDates{sign}=0:0:0 {}:{}:0", m / 60, m % 60));
        }
    }
    command.arg("-o").arg(&out).arg("--").arg(file);
    actions::run_tool(command, "ExifTool", &STOP, |_| {})?;
    if !out.is_file() {
        return Err("ExifTool made nothing (the file may have nothing to change)".into());
    }
    // A PDF keeps what ExifTool took out in an earlier revision inside it: pdfcpu rewrites it without.
    if kind == "pdf" && *task == MetaTask::Scrub {
        if let Ok(mut pdfcpu) = crate::tools::command("pdfcpu") {
            let rewritten = work.join("rewritten.pdf");
            pdfcpu.args(["-c", "disable", "--offline", "optimize", "--"]).arg(&out).arg(&rewritten);
            if actions::run_tool(pdfcpu, "pdfcpu", &STOP, |_| {}).is_ok() && rewritten.is_file() {
                fs::rename(&rewritten, &out).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(out)
}

/// The document properties of an Office file: core.xml's and app.xml's.
fn office_tags(path: &Path) -> Result<Vec<Tag>, String> {
    let mut archive = zip::ZipArchive::new(File::open(path).map_err(|e| e.to_string())?).map_err(|_| "It isn't an Office file Glint can read".to_string())?;
    let mut tags = Vec::new();
    for (part, group) in [("docProps/core.xml", "Core"), ("docProps/app.xml", "App"), ("docProps/custom.xml", "Custom")] {
        let Ok(mut entry) = archive.by_name(part) else { continue };
        let mut xml = String::new();
        entry.by_ref().take(4 << 20).read_to_string(&mut xml).map_err(|e| e.to_string())?;
        for (name, value) in elements(&xml) {
            if !value.trim().is_empty() {
                tags.push(Tag { kind: kind_of(group, &name), group: group.into(), name, value: value.chars().take(200).collect() });
            }
        }
    }
    Ok(tags)
}

/// The leaf elements of a properties part: (local name, text).
fn elements(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        if rest.starts_with(['/', '?', '!']) {
            continue;
        }
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        if tag.ends_with('/') {
            rest = &rest[end + 1..];
            continue;
        }
        let name = tag.split_whitespace().next().unwrap_or_default();
        let local = name.rsplit(':').next().unwrap_or(name).to_string();
        let body = &rest[end + 1..];
        if let Some(close) = body.find("</") {
            let text = &body[..close];
            if !text.contains('<') {
                out.push((local, unescape(text)));
            }
        }
        rest = &rest[end + 1..];
    }
    out
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// `xml` with each element named in `names` (qualified, "dc:creator") emptied.
fn blank(xml: &str, names: &[&str]) -> String {
    let mut out = xml.to_string();
    for name in names {
        let open = format!("<{name}");
        let close = format!("</{name}>");
        let mut from = 0;
        while let Some(at) = out[from..].find(&open).map(|i| i + from) {
            let after = &out[at + open.len()..];
            // The name itself, not one that starts with it.
            if !after.starts_with(['>', ' ', '/']) {
                from = at + open.len();
                continue;
            }
            let Some(gt) = out[at..].find('>').map(|i| i + at) else { break };
            if out[..gt].ends_with('/') {
                from = gt;
                continue;
            }
            let Some(end) = out[gt..].find(&close).map(|i| i + gt) else { break };
            out.replace_range(gt + 1..end, "");
            from = gt + 1;
        }
    }
    out
}

/// What's personal in each properties part, blanked (custom properties go altogether).
fn scrub_part(name: &str, xml: &str) -> Option<String> {
    match name {
        "docProps/core.xml" => Some(blank(xml, &["dc:creator", "cp:lastModifiedBy", "cp:lastPrinted", "cp:contentStatus"])),
        "docProps/app.xml" => Some(blank(xml, &["Company", "Manager", "Template", "HyperlinkBase"])),
        "docProps/custom.xml" => Some(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"/>"#.to_string(),
        ),
        _ => None,
    }
}

/// An Office file rewritten into `out`: its properties cleaned, every other part copied as it's stored.
fn scrub_office(file: &Path, out: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(File::open(file).map_err(|e| e.to_string())?).map_err(|_| "It isn't an Office file Glint can read".to_string())?;
    let mut writer = zip::ZipWriter::new(File::create(out).map_err(|e| e.to_string())?);
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i).map_err(|e| e.to_string())?;
        let name = entry.name().map(|n| n.to_string()).map_err(|e| e.to_string())?;
        let replaced = if name.starts_with("docProps/") {
            drop(entry);
            let mut whole = archive.by_index(i).map_err(|e| e.to_string())?;
            let mut xml = String::new();
            whole.by_ref().take(16 << 20).read_to_string(&mut xml).map_err(|e| e.to_string())?;
            drop(whole);
            match scrub_part(&name, &xml) {
                Some(clean) => {
                    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
                    writer.start_file(name.as_str(), options).map_err(|e| e.to_string())?;
                    writer.write_all(clean.as_bytes()).map_err(|e| e.to_string())?;
                    true
                }
                None => false,
            }
        } else {
            writer.raw_copy_file(entry).map_err(|e| e.to_string())?;
            continue;
        };
        if !replaced {
            let entry = archive.by_index_raw(i).map_err(|e| e.to_string())?;
            writer.raw_copy_file(entry).map_err(|e| e.to_string())?;
        }
    }
    writer.finish().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_tags_by_what_they_tell() {
        let json = r#"[{"SourceFile":"a.jpg","System:FileName":"a.jpg","File:ImageWidth":10,"IFD0:Make":"Canon","GPS:GPSLatitude":"51 deg 30' 0.00\" N","IFD0:Artist":"Ada","ExifIFD:SerialNumber":"123456","ExifIFD:DateTimeOriginal":"2024:06:01 12:00:00","XMP-xmpMM:DocumentID":"xmp.did:1","IFD0:Orientation":"Rotate 90 CW"}]"#;
        let tags = read_tags(json);
        let kinds: Vec<(&str, &str)> = tags.iter().map(|t| (t.name.as_str(), t.kind)).collect();
        // By what they tell; within a kind, in ExifTool's order.
        assert_eq!(kinds, [("GPSLatitude", "place"), ("Artist", "people"), ("Make", "device"), ("SerialNumber", "device"), ("DocumentID", "history"), ("DateTimeOriginal", "dates"), ("Orientation", "other")]);
        assert!(valid_date("2024:06:01 12:00:00") && !valid_date("2024-06-01 12:00") && !valid_date("2024:06:01T12:00:00"));
    }

    #[test]
    fn cleans_an_office_file_and_keeps_the_rest() {
        let dir = std::env::temp_dir().join(format!("glint-meta-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("a.docx");
        let mut zip = zip::ZipWriter::new(File::create(&doc).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(b"<w:document>Hello</w:document>").unwrap();
        zip.start_file("docProps/core.xml", options).unwrap();
        zip.write_all(br#"<cp:coreProperties><dc:title>Plan</dc:title><dc:creator>Ada Lovelace</dc:creator><cp:lastModifiedBy>Charles</cp:lastModifiedBy><cp:revision>12</cp:revision></cp:coreProperties>"#).unwrap();
        zip.start_file("docProps/app.xml", options).unwrap();
        zip.write_all(br#"<Properties><Company>Analytical Engines Ltd</Company><Pages>3</Pages><Template/></Properties>"#).unwrap();
        zip.finish().unwrap();
        let tags = office_tags(&doc).unwrap();
        assert!(tags.iter().any(|t| t.name == "creator" && t.value == "Ada Lovelace" && t.kind == "people"));
        let out = dir.join("clean.docx");
        scrub_office(&doc, &out).unwrap();
        let clean = office_tags(&out).unwrap();
        assert!(!clean.iter().any(|t| t.value.contains("Ada") || t.value.contains("Charles") || t.value.contains("Analytical")), "{clean:?}");
        assert!(clean.iter().any(|t| t.name == "title" && t.value == "Plan"));
        let mut archive = zip::ZipArchive::new(File::open(&out).unwrap()).unwrap();
        let mut body = String::new();
        archive.by_name("word/document.xml").unwrap().read_to_string(&mut body).unwrap();
        assert_eq!(body, "<w:document>Hello</w:document>");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn blanks_only_the_elements_named() {
        let xml = "<a><dc:creator>X</dc:creator><dc:creatorTool>Y</dc:creatorTool><dc:creator/></a>";
        assert_eq!(blank(xml, &["dc:creator"]), "<a><dc:creator></dc:creator><dc:creatorTool>Y</dc:creatorTool><dc:creator/></a>");
    }
}
