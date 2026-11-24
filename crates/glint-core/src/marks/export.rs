//! Export marks… (PLAN.md F24, F33): everything marked below a folder into one file, Glint's JSON Lines or an
//! `XYplorerTag.dat` that XYplorer's `tagload` merges.

use super::{find, real, Sidecars, World};
use crate::fsx::blocking;
use glint_model::marks::lines::{write_export, Export, Line};
use glint_model::marks::xyplorer::{self, XyItem};
use glint_model::marks::{default_labels, label_of, Label, Marks, BESIDE_NAME, MAX_LABELS};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The format an export is written in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    /// JSON Lines: the labels first, then a line per item, by path under the folder.
    #[default]
    Glint,
    /// An `XYplorerTag.dat`: version 5, storage 3.
    Xyplorer,
}

/// The color of a label XYplorer's file gets for a label this PC's list lacks.
const XY_GRAY: u32 = 0xB5_B5B5;

/// Writes the marks of everything below `root` to `to`; how many items were written.
pub async fn marks_export(root: String, to: String, format: Format) -> Result<u64, String> {
    marks_export_with(root, to, format, default_labels(), Arc::new(AtomicBool::new(false))).await
}

/// [`marks_export`] with the labels of the PC (the interface hands them in: core has no settings) and a flag
/// that ends it with `Err("Stopped")` when set, leaving nothing behind.
pub async fn marks_export_with(
    root: String,
    to: String,
    format: Format,
    labels: Vec<Label>,
    stop: Arc<AtomicBool>,
) -> Result<u64, String> {
    blocking(move || real(|w| export_in(&root, &to, format, &labels, &stop, w))).await
}

/// What `path` is below `folder`, as a marks file writes it (`Sub\Photos`; empty for `folder` itself), compared in
/// any case; None when `path` isn't `folder` or below it. Reads nothing.
pub fn rest_under(folder: &str, path: &str) -> Option<String> {
    let mut rest = Path::new(path).components();
    for part in Path::new(folder).components() {
        let next = rest.next()?;
        if next.as_os_str().to_string_lossy().to_lowercase() != part.as_os_str().to_string_lossy().to_lowercase() {
            return None;
        }
    }
    Some(rest.map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("\\"))
}

/// `rest` written from a folder `lead` above the one exported.
fn with_lead(lead: &str, rest: &str) -> String {
    if lead.is_empty() {
        rest.to_string()
    } else {
        format!("{lead}\\{rest}")
    }
}

/// Whether a directory entry is a link or a junction: never walked into.
fn is_link(md: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        md.file_attributes() & REPARSE_POINT != 0 || md.is_symlink()
    }
    #[cfg(not(windows))]
    {
        md.is_symlink()
    }
}

/// Every item below `root`, files and folders, without following a link; a `.glint-marks` is no item.
fn walk(root: &Path, stop: &AtomicBool) -> Result<Vec<PathBuf>, String> {
    let (mut items, mut todo) = (Vec::new(), vec![root.to_path_buf()]);
    while let Some(dir) = todo.pop() {
        if stop.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        // A folder that can't be read is skipped.
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().eq_ignore_ascii_case(BESIDE_NAME) {
                continue;
            }
            let path = entry.path();
            if entry.metadata().is_ok_and(|md| md.is_dir() && !is_link(&md)) {
                todo.push(path.clone());
            }
            items.push(path);
        }
    }
    Ok(items)
}

fn export_in(
    root: &str,
    to: &str,
    format: Format,
    labels: &[Label],
    stop: &AtomicBool,
    w: &World,
) -> Result<u64, String> {
    let name = Path::new(to).file_name().map(|n| n.to_string_lossy().to_lowercase());
    if name.as_deref() == Some("tag.dat") {
        return Err("Glint never writes XYplorer's own tag.dat: choose another name, and load it there with tagload".into());
    }
    // A drive's root keeps its separator: `C:` alone is the drive's current folder.
    let trimmed = root.trim_end_matches(['\\', '/']);
    let root = if trimmed.ends_with(':') { format!("{trimmed}\\") } else { trimmed.to_string() };
    // The paths are written from the folder the file is in (as XYplorer's storage 3 is, and as Import marks… reads
    // them back): the file goes in the folder exported, or in one above it.
    let lead = Path::new(to)
        .parent()
        .filter(|base| !base.as_os_str().is_empty())
        .and_then(|base| rest_under(&base.to_string_lossy(), &root));
    let Some(lead) = lead else {
        let name = Path::new(&root).file_name().map_or_else(|| root.clone(), |n| n.to_string_lossy().into_owned());
        return Err(format!("Keep the file in “{name}” or a folder above it: the paths in it are written from where it is"));
    };
    let stopped = || stop.load(Ordering::Relaxed);
    if stopped() {
        return Err("Stopped".into());
    }
    let items = walk(Path::new(&root), stop)?;

    // The marks of each item, from the first home that has any; a folder's sidecar is read once.
    let sidecars = Sidecars::default();
    let mut found: Vec<(String, Marks)> = items
        .par_iter()
        .filter_map(|p| {
            if stopped() {
                return None;
            }
            let rel = p.strip_prefix(&root).ok()?.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>();
            let marks = find(&p.to_string_lossy(), w, &sidecars).0;
            (!marks.is_empty()).then(|| (with_lead(&lead, &rel.join("\\")), marks))
        })
        .collect();
    if stopped() {
        return Err("Stopped".into());
    }

    // What waits in the ledger for an item that isn't there.
    if !w.ledger_empty() {
        for line in w.ledger().lines() {
            let path = line.name.replace('/', "\\");
            let Some(rest) = rest_under(&root, &path).filter(|rest| !rest.is_empty()) else { continue };
            if !line.marks.is_empty() && std::fs::symlink_metadata(&path).is_err() {
                found.push((with_lead(&lead, &rest), glint_model::marks::tidy(&line.marks)));
            }
        }
    }
    if found.is_empty() {
        return Ok(0);
    }
    found.sort_by_cached_key(|(rel, _)| (rel.to_lowercase(), rel.clone()));
    let count = found.len() as u64;

    let bytes = match format {
        Format::Glint => {
            let lines = found.into_iter().map(|(name, marks)| Line { name, marks, ..Line::default() }).collect();
            write_export(&Export { labels: labels.to_vec(), lines }).into_bytes()
        }
        Format::Xyplorer => {
            // A label the list lacks is added at its end, gray, up to MAX_LABELS (32): what Glint's own readers
            // take of a label list. A label past that is written without one.
            let mut list = labels.to_vec();
            let items: Vec<XyItem> = found
                .into_iter()
                .map(|(path, marks)| {
                    let label = marks.label.as_deref().map_or(0, |name| match label_of(&list, name) {
                        Some((i, _)) => i + 1,
                        None if list.len() < MAX_LABELS && !name.trim().is_empty() => {
                            list.push(Label { name: name.trim().to_string(), color: XY_GRAY, was: Vec::new() });
                            list.len()
                        }
                        None => 0,
                    });
                    XyItem {
                        path,
                        label: u32::try_from(label).unwrap_or(0),
                        tags: marks.tags,
                        extras: Vec::new(),
                        comment: marks.comment.unwrap_or_default(),
                    }
                })
                .collect();
            xyplorer::write(&items, &list)
        }
    };
    put(to, &bytes, stop)?;
    Ok(count)
}

/// Writes `bytes` whole or not at all: to a temporary file beside `to` that then takes its name.
fn put(to: &str, bytes: &[u8], stop: &AtomicBool) -> Result<(), String> {
    let to = Path::new(to);
    let tmp = to.with_file_name(format!("{}.glint-tmp", to.file_name().unwrap_or_default().to_string_lossy()));
    let done = std::fs::write(&tmp, bytes).map_err(|e| e.to_string()).and_then(|()| {
        if stop.load(Ordering::Relaxed) {
            return Err("Stopped".to_string());
        }
        std::fs::rename(&tmp, to).map_err(|e| e.to_string())
    });
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marks::ledger::Ledger;
    use crate::marks::{caps, streams, work};
    use glint_model::marks::lines::read_export;
    use glint_model::marks::{Change, Why};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    fn folder() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("glint-export-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn none(_: &str) -> bool {
        false
    }

    fn ledger_in(dir: &Path) -> Mutex<Ledger> {
        Mutex::new(Ledger::open(Some(dir.join("ledger.jsonl"))))
    }

    fn world<'a>(ledger: &'a Mutex<Ledger>) -> World<'a> {
        World { streams: &none, ledger, empty: None, now: 1_000 }
    }

    fn file(dir: &Path, rel: &str) -> String {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
        p.to_string_lossy().into_owned()
    }

    fn dir(root: &Path, rel: &str) -> String {
        let p = root.join(rel);
        std::fs::create_dir_all(&p).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn marks(label: &str, tags: &[&str], comment: Option<&str>) -> Marks {
        Marks {
            label: (!label.is_empty()).then(|| label.to_string()),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            comment: comment.map(String::from),
        }
    }

    /// Marks put beside their items (the test's world has no streams).
    fn mark(w: &World, path: &str, m: Marks) {
        work(vec![(path.to_string(), Change::Put(m))], w);
    }

    fn run(root: &Path, to: &Path, format: Format, labels: &[Label], w: &World) -> Result<u64, String> {
        export_in(&root.to_string_lossy(), &to.to_string_lossy(), format, labels, &AtomicBool::new(false), w)
    }

    fn two_labels() -> Vec<Label> {
        default_labels().into_iter().filter(|l| l.name == "Red" || l.name == "Blue").collect()
    }

    #[test]
    fn a_tree_of_three_folders_exports_every_marked_item_sorted_and_reads_back_the_same() {
        let (root, out) = (folder(), folder());
        let l = ledger_in(&out);
        let w = world(&l);
        let items = [
            ("a.txt", marks("Red", &["one", "two"], Some("line one\nline two"))),
            ("sub", marks("Blue", &[], None)),
            ("sub\\b.txt", marks("", &["only"], None)),
            ("sub\\deep\\c.txt", marks("", &[], Some("just words"))),
        ];
        for (rel, m) in &items {
            let path = if *rel == "sub" { dir(&root, rel) } else { file(&root, rel) };
            mark(&w, &path, m.clone());
        }
        file(&root, "unmarked.txt");
        let labels = two_labels();
        let to = root.join("all.glint-marks.jsonl");
        assert_eq!(run(&root, &to, Format::Glint, &labels, &w), Ok(4));
        let read = read_export(&std::fs::read_to_string(&to).unwrap()).unwrap();
        assert_eq!(read.labels, labels);
        let got: Vec<(&str, Marks)> = read.lines.iter().map(|l| (l.name.as_str(), l.marks.clone())).collect();
        let want: Vec<(&str, Marks)> = items.iter().map(|(n, m)| (*n, m.clone())).collect();
        assert_eq!(got, want);
        assert!(!root.join("all.glint-marks.jsonl.glint-tmp").exists());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn items_in_all_three_homes_are_exported_and_a_sidecar_is_no_item() {
        let (root, out) = (folder(), folder());
        let l = ledger_in(&out);
        let w = world(&l);
        mark(&w, &file(&root, "beside.txt"), marks("Red", &[], None));
        let gone = root.join("gone.txt").to_string_lossy().into_owned();
        l.lock().unwrap().put(&gone, &marks("Blue", &[], None), Why::Away, 1);
        let mut want = vec!["beside.txt", "gone.txt"];
        if caps::has_streams(&root.to_string_lossy()) {
            streams::write(&file(&root, "streamed.txt"), &marks("Red", &["s"], None)).unwrap();
            want = vec!["beside.txt", "gone.txt", "streamed.txt"];
        }
        assert!(root.join(BESIDE_NAME).exists());
        let to = root.join("e.jsonl");
        assert_eq!(run(&root, &to, Format::Glint, &two_labels(), &w), Ok(want.len() as u64));
        let read = read_export(&std::fs::read_to_string(&to).unwrap()).unwrap();
        assert_eq!(read.lines.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), want);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn xyplorers_format_reads_back_with_each_label_its_place_and_a_missing_label_added_at_the_end() {
        let (root, out) = (folder(), folder());
        let l = ledger_in(&out);
        let w = world(&l);
        mark(&w, &file(&root, "a.txt"), marks("Blue", &["t"], Some("one\ntwo")));
        mark(&w, &file(&root, "d\\b.txt"), marks("red", &[], None));
        mark(&w, &file(&root, "d\\c.txt"), marks("Purple", &[], None));
        let to = root.join("XYplorerTag.dat");
        assert_eq!(run(&root, &to, Format::Xyplorer, &two_labels(), &w), Ok(3));
        let xy = xyplorer::read(&std::fs::read(&to).unwrap()).unwrap();
        assert_eq!((xy.version, xy.storage), (5, 3));
        let names: Vec<&str> = xy.labels.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Red", "Blue", "Purple"]);
        assert_eq!(xy.labels[2].back, XY_GRAY);
        let got: Vec<(&str, u32)> = xy.items.iter().map(|i| (i.path.as_str(), i.label)).collect();
        assert_eq!(got, [("a.txt", 2), ("d\\b.txt", 1), ("d\\c.txt", 3)]);
        assert_eq!(xy.items[0].comment, "one¶two");
        assert_eq!(xy.items[0].tags, ["t"]);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_or_link_inside_the_root_is_not_walked_into() {
        let (root, other, out) = (folder(), folder(), folder());
        let l = ledger_in(&out);
        let w = world(&l);
        mark(&w, &file(&other, "outside.txt"), marks("Red", &[], None));
        mark(&w, &file(&root, "inside.txt"), marks("Red", &[], None));
        let link = root.join("link");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&other)
            .output()
            .is_ok_and(|o| o.status.success())
            || std::os::windows::fs::symlink_dir(&other, &link).is_ok();
        if !made {
            eprintln!("skipped: no junction or symbolic link could be made here");
        } else {
            let to = root.join("e.jsonl");
            assert_eq!(run(&root, &to, Format::Glint, &two_labels(), &w), Ok(1));
            let read = read_export(&std::fs::read_to_string(&to).unwrap()).unwrap();
            assert_eq!(read.lines.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["inside.txt"]);
            // A junction is removed as a folder, which leaves its target alone.
            let _ = std::fs::remove_dir(&link);
        }
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&other);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn a_stop_set_before_it_starts_answers_stopped_and_leaves_no_file_and_no_temporary_file() {
        let (root, out) = (folder(), folder());
        let l = ledger_in(&out);
        let w = world(&l);
        mark(&w, &file(&root, "a.txt"), marks("Red", &[], None));
        let stop = AtomicBool::new(true);
        let to = root.join("e.jsonl");
        let got = export_in(&root.to_string_lossy(), &to.to_string_lossy(), Format::Glint, &[], &stop, &w);
        assert_eq!(got, Err("Stopped".to_string()));
        assert!(!to.exists() && !root.join("e.jsonl.glint-tmp").exists());
        assert_eq!(std::fs::read_dir(&out).unwrap().flatten().count(), 0);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn a_target_named_tag_dat_in_any_case_is_refused_and_nothing_is_written() {
        let (root, out) = (folder(), folder());
        let l = ledger_in(&out);
        let w = world(&l);
        mark(&w, &file(&root, "a.txt"), marks("Red", &[], None));
        let to = out.join("TAG.DAT");
        let got = run(&root, &to, Format::Xyplorer, &two_labels(), &w);
        assert_eq!(
            got,
            Err("Glint never writes XYplorer's own tag.dat: choose another name, and load it there with tagload".to_string())
        );
        assert_eq!(std::fs::read_dir(&out).unwrap().flatten().count(), 0);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn a_file_above_the_folder_writes_its_paths_from_there_and_one_beside_it_is_refused() {
        let (top, out) = (folder(), folder());
        let l = ledger_in(&out);
        let w = world(&l);
        let root = top.join("Photos");
        mark(&w, &file(&root, "d\\a.txt"), marks("Red", &[], None));
        let gone = root.join("gone.txt").to_string_lossy().into_owned();
        l.lock().unwrap().put(&gone, &marks("Blue", &[], None), Why::Away, 1);
        let to = top.join("e.jsonl");
        assert_eq!(run(&root, &to, Format::Glint, &two_labels(), &w), Ok(2));
        let read = read_export(&std::fs::read_to_string(&to).unwrap()).unwrap();
        assert_eq!(read.lines.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Photos\\d\\a.txt", "Photos\\gone.txt"]);
        // Beside the folder, not above it: refused, in words that name the folder, and nothing written.
        let beside = out.join("e.jsonl");
        let got = run(&root, &beside, Format::Xyplorer, &two_labels(), &w);
        assert!(got.as_ref().is_err_and(|e| e.contains("“Photos”")), "{got:?}");
        assert!(!beside.exists());
        assert_eq!(rest_under("C:\\", "c:\\Docs\\a"), Some("Docs\\a".to_string()));
        assert_eq!(rest_under("C:\\Docs\\", "C:\\docs"), Some(String::new()));
        assert_eq!(rest_under("C:\\Docs", "C:\\Docs2\\a"), None);
        let _ = std::fs::remove_dir_all(&top);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn nothing_marked_is_zero_and_no_file() {
        let (root, out) = (folder(), folder());
        let l = ledger_in(&out);
        let w = world(&l);
        file(&root, "a.txt");
        for format in [Format::Glint, Format::Xyplorer] {
            let to = root.join("e.out");
            assert_eq!(run(&root, &to, format, &two_labels(), &w), Ok(0));
            assert!(!to.exists());
        }
        assert_eq!(std::fs::read_dir(&out).unwrap().flatten().count(), 0);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn an_arabic_name_tag_and_comment_come_through_both_formats() {
        let (root, out) = (folder(), folder());
        let l = ledger_in(&out);
        let w = world(&l);
        let m = marks("Red", &["وسم"], Some("تعليق\nسطر ثان"));
        mark(&w, &file(&root, "ملفات\\تقرير.txt"), m.clone());
        let to = root.join("e.jsonl");
        assert_eq!(run(&root, &to, Format::Glint, &two_labels(), &w), Ok(1));
        let read = read_export(&std::fs::read_to_string(&to).unwrap()).unwrap();
        assert_eq!((read.lines[0].name.as_str(), &read.lines[0].marks), ("ملفات\\تقرير.txt", &m));
        let to = root.join("XYplorerTag.dat");
        assert_eq!(run(&root, &to, Format::Xyplorer, &two_labels(), &w), Ok(1));
        let xy = xyplorer::read(&std::fs::read(&to).unwrap()).unwrap();
        let item = &xy.items[0];
        assert_eq!((item.path.as_str(), item.label, item.tags.clone()), ("ملفات\\تقرير.txt", 1, vec!["وسم".to_string()]));
        assert_eq!(item.comment, "تعليق¶سطر ثان");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    #[ignore = "a measurement: 10,000 marked files in 100 folders"]
    fn ten_thousand_marked_files_in_a_hundred_folders_time() {
        let (root, out) = (folder(), folder());
        let l = ledger_in(&out);
        let w = world(&l);
        for d in 0..100 {
            let items: Vec<(String, Change)> = (0..100)
                .map(|f| (file(&root, &format!("d{d}\\f{f}.txt")), Change::Put(marks("Red", &["t"], Some("c")))))
                .collect();
            work(items, &w);
        }
        for (format, name) in [(Format::Glint, "e.jsonl"), (Format::Xyplorer, "XYplorerTag.dat")] {
            let start = std::time::Instant::now();
            let n = run(&root, &root.join(name), format, &two_labels(), &w).unwrap();
            println!("{format:?}: {n} items in {} ms", start.elapsed().as_millis());
            assert_eq!(n, 10_000);
        }
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out);
    }
}
