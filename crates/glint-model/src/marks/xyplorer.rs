//! XYplorer's tag database, `tag.dat` (and `XYplorerTag.dat`, the same for one folder), read and written
//! (PLAN.md 7.1). UTF-16LE text in sections found by their names: `Labels:`, `Extra Tags:`, `Storage:`,
//! `Data:`. A data line is `Full filename|LabelID|Tags|Extra1…ExtraN|Comment`, the comment last and the only
//! field that may hold `|`, its line breaks written `¶`. Version 5 has sixteen extra fields, version 3 five.
//!
//! Only read from XYplorer's own files (M7); what is written is an export the person takes there.

use super::{Label, Marks};

/// Version 5 has sixteen extra fields; a file is written with that many.
const XY_EXTRAS: usize = 16;

/// More is never read of a `tag.dat`.
pub const XY_MAX_BYTES: usize = 64 << 20;

/// One of XYplorer's labels: `Red|000000|FC7268` is its name, its text's color and its color.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XyLabel {
    pub name: String,
    /// `0xRRGGBB`.
    pub text: u32,
    /// `0xRRGGBB`: the label's color.
    pub back: u32,
}

/// What an extra field holds (its TypeID).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XyKind {
    Text,
    Number,
    Date,
    /// Empty is off, anything else on.
    Checkbox,
    /// 1 to 5.
    Rating,
    List,
    Location,
    Icon,
    Image,
    Script,
    /// A type this reader doesn't know, by its number.
    Other(u32),
}

/// One extra field's definition (a line of `Extra Tags:`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XyExtra {
    pub caption: String,
    pub kind: XyKind,
}

/// One data line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XyItem {
    /// As written: what it means depends on the file's storage mode.
    pub path: String,
    /// 0 for none, else the label's place in the list, from 1.
    pub label: u32,
    pub tags: Vec<String>,
    /// The extra fields' values, one per definition.
    pub extras: Vec<String>,
    /// With `¶` as written.
    pub comment: String,
}

/// A `tag.dat` as read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XyFile {
    /// From `XYplorer File Tags v5`.
    pub version: u32,
    pub labels: Vec<XyLabel>,
    pub extras: Vec<XyExtra>,
    /// How paths are written (`Storage: 0`): 0 absolute, 1 `?:` for XYplorer's drive, 2 relative to XYplorer's
    /// folder, 3 relative to the file's folder, 4 a volume's serial for the drive.
    pub storage: u32,
    pub items: Vec<XyItem>,
}

/// The text of a file: by its byte-order mark, else UTF-16LE when the second byte is 0, else UTF-8. What isn't a
/// valid unit becomes U+FFFD; an odd last byte is ignored.
fn decode(bytes: &[u8]) -> String {
    let utf16 = |b: &[u8], le: bool| {
        let units: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| if le { u16::from_le_bytes(c) } else { u16::from_be_bytes(c) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ if bytes.get(1) == Some(&0) => utf16(bytes, true),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Six hex digits, else the default.
fn color(s: &str, default: u32) -> u32 {
    let s = s.trim();
    if s.len() == 6 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        u32::from_str_radix(s, 16).unwrap_or(default)
    } else {
        default
    }
}

fn kind_of(s: &str) -> XyKind {
    match s.trim().parse::<u32>() {
        Ok(0) | Err(_) => XyKind::Text,
        Ok(1) => XyKind::Number,
        Ok(2) => XyKind::Date,
        Ok(3) => XyKind::Checkbox,
        Ok(4) => XyKind::Rating,
        Ok(5) => XyKind::List,
        Ok(6) => XyKind::Location,
        Ok(7) => XyKind::Icon,
        Ok(8) => XyKind::Image,
        Ok(9) => XyKind::Script,
        Ok(n) => XyKind::Other(n),
    }
}

fn item_of(line: &str, extras: usize) -> Option<XyItem> {
    let mut parts = line.splitn(extras + 4, '|');
    let path = parts.next().unwrap_or("");
    if path.is_empty() {
        return None;
    }
    let label = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let mut tags: Vec<String> = Vec::new();
    for t in parts.next().unwrap_or("").split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if !tags.iter().any(|k| k.to_lowercase() == t.to_lowercase()) {
            tags.push(t.to_string());
        }
    }
    let extras: Vec<String> = (0..extras).map(|_| parts.next().unwrap_or("").to_string()).collect();
    let comment = parts.next().unwrap_or("").to_string();
    Some(XyItem { path: path.to_string(), label, tags, extras, comment })
}

/// A `tag.dat` from its bytes: with or without a byte-order mark, `\r\n` or `\n`. Err when it isn't one.
pub fn read(bytes: &[u8]) -> Result<XyFile, String> {
    if bytes.len() > XY_MAX_BYTES {
        return Err("This file is too big to be a tag database".into());
    }
    let text = decode(bytes);
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    const NOT: &str = "Not an XYplorer tag database";
    let first = lines.by_ref().find(|l| !l.is_empty()).ok_or(NOT)?;
    let rest = first.strip_prefix("XYplorer File Tags v").ok_or(NOT)?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let version = rest.get(..digits).and_then(|d| d.parse().ok()).ok_or(NOT)?;
    let mut file = XyFile { version, ..XyFile::default() };

    #[derive(PartialEq)]
    enum In {
        Nothing,
        Labels,
        Extras,
        Data,
    }
    let mut at = In::Nothing;
    for line in lines {
        if at == In::Data {
            if !line.is_empty() {
                file.items.extend(item_of(line, file.extras.len()));
            }
            continue;
        }
        if line == "Labels:" {
            at = In::Labels;
        } else if line == "Extra Tags:" {
            at = In::Extras;
        } else if line == "Data:" {
            at = In::Data;
        } else if let Some(n) = line.strip_prefix("Storage:") {
            file.storage = n.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
            at = In::Nothing;
        } else if at == In::Labels && !line.is_empty() {
            for l in line.split(';') {
                let mut f = l.split('|');
                file.labels.push(XyLabel {
                    name: f.next().unwrap_or("").to_string(),
                    text: color(f.next().unwrap_or(""), 0x000000),
                    back: color(f.next().unwrap_or(""), 0xFC7268),
                });
            }
            at = In::Nothing;
        } else if at == In::Extras {
            if line.is_empty() {
                at = In::Nothing;
            } else {
                let mut f = line.split('|');
                let caption = f.next().unwrap_or("").to_string();
                file.extras.push(XyExtra { caption, kind: kind_of(f.next().unwrap_or("")) });
            }
        }
    }
    Ok(file)
}

/// A name or tag as one field: what would split the line becomes a space.
fn field(s: &str, also: char) -> String {
    s.replace(['|', also, '\r', '\n'], " ")
}

/// An `XYplorerTag.dat` (version 5, storage 3: paths relative to its folder) that XYplorer's `tagload` merges.
/// `labels` are Glint's, written as XYplorer's list; an item's `label` is its place in them, from 1.
pub fn write(items: &[XyItem], labels: &[Label]) -> Vec<u8> {
    let mut out = String::from("XYplorer File Tags v5\r\n\r\nLabels:\r\n");
    let names: Vec<String> = labels
        .iter()
        .map(|l| format!("{}|000000|{:06X}", field(&l.name, ';'), l.color & 0xFF_FFFF))
        .collect();
    out.push_str(&names.join(";"));
    out.push_str("\r\n\r\nExtra Tags:\r\n");
    for _ in 0..XY_EXTRAS {
        out.push_str("|0|0|0|||||\r\n");
    }
    out.push_str("\r\nStorage: 3 (Relative to tags database)\r\n\r\nData:\r\n");
    let mut sorted: Vec<&XyItem> = items.iter().filter(|i| !i.path.is_empty()).collect();
    sorted.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    for i in sorted {
        let label = if (i.label as usize) <= labels.len() { i.label } else { 0 };
        let tags: Vec<String> = i.tags.iter().map(|t| field(t, ',')).collect();
        let comment = i.comment.replace("\r\n", "¶").replace(['\n', '\r'], "¶");
        out.push_str(&format!(
            "{}|{}|{}{}{}\r\n",
            field(&i.path, '|'),
            label,
            tags.join(", "),
            "|".repeat(XY_EXTRAS + 1),
            comment
        ));
    }
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(out.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

/// What one item brings (7.3, 7.4): its label by XYplorer's name, its tags, its comment with line breaks, and,
/// when `extras`, its extra fields as tags and comment lines.
pub fn marks_of(file: &XyFile, item: &XyItem, extras: bool) -> Marks {
    let label = (item.label as usize)
        .checked_sub(1)
        .and_then(|i| file.labels.get(i))
        .map(|l| l.name.clone())
        .filter(|n| !n.is_empty());
    let mut tags = item.tags.clone();
    let mut comment = item.comment.replace('¶', "\n").trim_end().to_string();
    if extras {
        let mut lines: Vec<String> = Vec::new();
        for (n, (def, value)) in file.extras.iter().zip(&item.extras).enumerate() {
            let value = value.trim().replace('¶', "\n");
            if value.is_empty() {
                continue;
            }
            let caption = if def.caption.trim().is_empty() { format!("Extra {}", n + 1) } else { def.caption.clone() };
            let tag = match def.kind {
                XyKind::Checkbox => Some(caption.clone()),
                XyKind::Rating | XyKind::List => Some(format!("{caption}: {value}")),
                XyKind::Icon => continue,
                _ => None,
            };
            match tag {
                Some(t) if !tags.iter().any(|k| k.to_lowercase() == t.to_lowercase()) => tags.push(t),
                Some(_) => {}
                None => lines.push(format!("{caption}: {value}")),
            }
        }
        if !lines.is_empty() {
            if !comment.is_empty() {
                comment.push_str("\n\n");
            }
            comment.push_str(&lines.join("\n"));
        }
    }
    Marks { label, tags, comment: (!comment.is_empty()).then_some(comment) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    macro_rules! fixture {
        ($f:literal) => {
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/marks/", $f))
        };
    }

    fn utf16(s: &str, bom: bool) -> Vec<u8> {
        let mut v = if bom { vec![0xFF, 0xFE] } else { Vec::new() };
        v.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
        v
    }

    fn first() -> XyFile {
        read(fixture!("v5-storage0-bom-crlf.dat")).unwrap()
    }

    fn item<'a>(f: &'a XyFile, name: &str) -> &'a XyItem {
        f.items.iter().find(|i| i.path.ends_with(name)).unwrap()
    }

    /// The one fixture XYplorer itself wrote (from issue 8's author; tests/fixtures/marks/README.md): a first line
    /// that goes on after the version, labels named in its own language, a text color left empty.
    #[test]
    fn a_tag_dat_that_xyplorer_wrote_reads_as_it_is() {
        let f = read(fixture!("real-v5-zh.dat")).unwrap();
        assert_eq!((f.version, f.storage, f.labels.len(), f.extras.len(), f.items.len()), (5, 0, 7, 16, 2));
        assert_eq!(f.labels[0], XyLabel { name: "红色".into(), text: 0xFFFF00, back: 0xFC7268 });
        assert_eq!(f.labels[1], XyLabel { name: "橙色".into(), text: 0x000000, back: 0xF6AB46 });
        assert_eq!(f.labels[6], XyLabel { name: "天蓝色".into(), text: 0x0969DA, back: 0xDDF4FF });
        // Sixteen extra fields nobody named: they bring nothing.
        assert!(f.extras.iter().all(|e| e.caption.is_empty() && e.kind == XyKind::Text));
        let (bin, other) = (&f.items[0], &f.items[1]);
        assert_eq!((bin.path.as_str(), bin.label, bin.comment.as_str()), ("C:\\$Recycle.Bin", 1, "comment1¶lala"));
        assert_eq!(
            marks_of(&f, bin, true),
            Marks { label: Some("红色".into()), tags: vec!["tag1".into()], comment: Some("comment1\nlala".into()) }
        );
        // The last line has no line end after it.
        assert_eq!((other.path.as_str(), other.label), ("C:\\Documents and Settings", 0));
        assert_eq!(marks_of(&f, other, true), Marks { label: None, tags: vec!["tag2".into()], comment: None });
    }

    #[test]
    fn the_first_fixture_reads_as_the_readme_says() {
        let f = first();
        assert_eq!((f.version, f.storage, f.labels.len(), f.extras.len(), f.items.len()), (5, 0, 7, 16, 7));
        assert_eq!(f.labels[0], XyLabel { name: "Red".into(), text: 0x000000, back: 0xFC7268 });
        let kinds: Vec<_> = f.extras[..4].iter().map(|e| (e.caption.as_str(), e.kind)).collect();
        assert_eq!(
            kinds,
            [("Status", XyKind::List), ("Stars", XyKind::Rating), ("Reviewed", XyKind::Checkbox), ("Due", XyKind::Date)]
        );
        let bond = item(&f, "bond0.conf");
        assert_eq!((bond.label, bond.tags.as_slice(), bond.comment.as_str()), (1, &["bond".into(), "netplan".into()][..], "mode 4¶LACP rate fast"));
        assert_eq!(bond.extras.len(), 16);
        let pipes = item(&f, "pipes.txt");
        assert_eq!((pipes.label, pipes.tags.clone(), pipes.comment.as_str()), (0, vec!["a".to_string(), "b".into()], "left | right || end"));
        let review = item(&f, "review.docx");
        assert_eq!(review.label, 5);
        assert_eq!(&review.extras[..4], ["In review", "4", "1", "2026-11-30"]);
        let past = item(&f, "past.txt");
        assert_eq!((past.label, past.comment.as_str()), (9, "label 9 of 7"));
        assert_eq!(item(&f, "archive").label, 7);
        assert_eq!(item(&f, "spaces.txt").tags, ["lead", "trail", "dup"]);
    }

    #[test]
    fn marks_of_gives_labels_tags_comments_and_extras() {
        let f = first();
        let m = marks_of(&f, item(&f, "bond0.conf"), true);
        assert_eq!(m.label.as_deref(), Some("Red"));
        assert_eq!(m.tags, ["bond", "netplan"]);
        assert_eq!(m.comment.as_deref(), Some("mode 4\nLACP rate fast"));
        assert_eq!(marks_of(&f, item(&f, "archive"), false).label.as_deref(), Some("Grey"));
        assert_eq!(marks_of(&f, item(&f, "past.txt"), false).label, None);
        let ar = marks_of(&f, item(&f, "تقرير.pdf"), false);
        assert_eq!(ar.label.as_deref(), Some("Orange"));
        assert_eq!(ar.tags, ["مهم", "عمل"]);
        assert_eq!(ar.comment.as_deref(), Some("ملاحظة\nسطر ثان"));
        let with = marks_of(&f, item(&f, "review.docx"), true);
        assert_eq!(with.label.as_deref(), Some("Blue"));
        assert_eq!(with.tags, ["Status: In review", "Stars: 4", "Reviewed"]);
        assert_eq!(with.comment.as_deref(), Some("Due: 2026-11-30"));
        let without = marks_of(&f, item(&f, "review.docx"), false);
        assert_eq!((without.label.as_deref(), without.tags.len(), without.comment), (Some("Blue"), 0, None));
    }

    #[test]
    fn a_file_without_a_byte_order_mark_and_with_lf_reads() {
        let f = read(fixture!("v5-storage1-nobom-lf.dat")).unwrap();
        assert_eq!((f.version, f.storage, f.items.len()), (5, 1, 2));
        assert_eq!(f.items[0].path, "?:\\ops\\bond0.conf");
        assert_eq!(f.items[0].label, 3);
        assert_eq!(f.items[1].path, "?:\\photos\\beach.jpg");
        assert_eq!(f.items[1].tags, ["summer", "beach"]);
        assert_eq!(f.items[1].comment, "sunset¶");
        assert_eq!(marks_of(&f, &f.items[1], false).comment.as_deref(), Some("sunset"));
    }

    #[test]
    fn the_other_storage_modes_keep_their_paths_as_written() {
        let f = read(fixture!("v5-storage2.dat")).unwrap();
        assert_eq!(f.storage, 2);
        assert_eq!((f.items[0].path.as_str(), f.items[0].label), ("..\\..\\ops\\bond0.conf", 1));
        assert_eq!((f.items[1].path.as_str(), f.items[1].tags.clone()), ("..\\data\\notes.md", vec!["notes".to_string()]));
        let f = read(fixture!("XYplorerTag-storage3.dat")).unwrap();
        assert_eq!(f.storage, 3);
        assert_eq!((f.items[0].path.as_str(), f.items[0].label), ("bond0.conf", 4));
        assert_eq!(marks_of(&f, &f.items[1], false).comment.as_deref(), Some("first\nsecond"));
        assert_eq!(f.items[1].path, "sub\\notes.md");
        let f = read(fixture!("v5-storage4.dat")).unwrap();
        assert_eq!(f.storage, 4);
        assert_eq!((f.items[0].path.as_str(), f.items[0].label), ("B2D60DFE:\\ops\\bond0.conf", 6));
        assert_eq!((f.items[1].path.as_str(), f.items[1].tags.clone()), ("0000ABCD:\\x.txt", vec!["elsewhere".to_string()]));
    }

    #[test]
    fn version_3_has_nine_labels_five_extras_and_the_old_items() {
        let f = read(fixture!("v3-storage0.dat")).unwrap();
        assert_eq!((f.version, f.labels.len(), f.extras.len()), (3, 9, 5));
        let old = item(&f, "old.txt");
        assert_eq!((old.label, old.tags.clone(), old.extras[0].as_str()), (8, vec!["legacy".to_string(), "v3".into()], "netplan"));
        let m = marks_of(&f, old, true);
        assert_eq!(m.label.as_deref(), Some("Work"));
        assert_eq!(m.comment.as_deref(), Some("kept\nsince v3\n\nTopic: netplan"));
        assert_eq!(marks_of(&f, item(&f, "plain.txt"), true).comment.as_deref(), Some("a | b"));
    }

    fn some_labels() -> Vec<Label> {
        ["Red", "Orange"]
            .iter()
            .zip([0xEF4444, 0xF97316])
            .map(|(n, c)| Label { name: n.to_string(), color: c, was: Vec::new() })
            .collect()
    }

    fn it(path: &str, label: u32, tags: &[&str], comment: &str) -> XyItem {
        XyItem {
            path: path.into(),
            label,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            extras: Vec::new(),
            comment: comment.into(),
        }
    }

    #[test]
    fn what_is_written_reads_back_sorted_by_path() {
        let items = [it("b.txt", 2, &["x"], "two¶lines"), it("a.txt", 1, &["bond", "netplan"], "mode 4"), it("c.txt", 9, &[], "")];
        let bytes = write(&items, &some_labels());
        assert_eq!(&bytes[..2], [0xFF, 0xFE]);
        let text = decode(&bytes);
        assert!(text.ends_with("\r\n"));
        assert!(!text.replace("\r\n", "").contains(['\r', '\n']));
        let f = read(&bytes).unwrap();
        assert_eq!((f.version, f.storage, f.extras.len()), (5, 3, 16));
        let labels: Vec<_> = f.labels.iter().map(|l| (l.name.as_str(), l.back)).collect();
        assert_eq!(labels, [("Red", 0xEF4444), ("Orange", 0xF97316)]);
        let got: Vec<_> = f.items.iter().map(|i| (i.path.as_str(), i.label, i.tags.join("+"), i.comment.as_str())).collect();
        assert_eq!(
            got,
            [
                ("a.txt", 1, "bond+netplan".to_string(), "mode 4"),
                ("b.txt", 2, "x".to_string(), "two¶lines"),
                ("c.txt", 0, String::new(), "")
            ]
        );
    }

    #[test]
    fn writing_protects_the_format() {
        let labels = [Label { name: "a|b;c".into(), color: 1, was: Vec::new() }];
        let f = read(&write(&[it("p", 1, &["x,y|z"], "one\ntwo | three")], &labels)).unwrap();
        assert_eq!(f.labels[0].name, "a b c");
        assert_eq!(f.items[0].tags, ["x y z"]);
        assert_eq!(f.items[0].comment, "one¶two | three");
    }

    #[test]
    fn what_is_not_a_database_is_an_error() {
        assert!(read(b"").is_err());
        assert!(read(b"hello").is_err());
        assert!(read(b"{\"version\":1}\nkey=value\n").is_err());
        assert!(read(b"XYplorer File Tags v\nData:\na|1\n").is_err());
        assert_eq!(read(&vec![0; XY_MAX_BYTES + 1]).unwrap_err(), "This file is too big to be a tag database");
    }

    #[test]
    fn nothing_panics_on_cut_odd_or_strange_input() {
        let bytes = fixture!("v5-storage0-bom-crlf.dat");
        for n in (0..=bytes.len()).step_by(7) {
            let _ = read(&bytes[..n]);
        }
        let _ = read(&bytes[..bytes.len() - 1]);
        let pipes = format!("XYplorer File Tags v5\nData:\n{}\n", "|".repeat(50));
        assert!(read(pipes.as_bytes()).unwrap().items.is_empty());
        let f = read(b"XYplorer File Tags v5\nLabels:\nA|000000|FFFFFF\nData:\nx|99999999999|t|\n").unwrap();
        assert_eq!(marks_of(&f, &f.items[0], true).label, None);
    }

    #[test]
    fn a_later_version_reads_by_the_same_rules() {
        let bytes = utf16("XYplorer File Tags v6\r\nData:\r\na|1|t|||||||||||||||||c\r\n", true);
        let f = read(&bytes).unwrap();
        assert_eq!((f.version, f.items.len()), (6, 1));
    }

    #[test]
    fn definitions_without_data_have_no_items_and_utf8_copies_read_the_same() {
        let f = read(b"XYplorer File Tags v5\n\nExtra Tags:\nA|0|0|0|||||\n\nStorage: 0\n").unwrap();
        assert_eq!((f.extras.len(), f.items.len()), (1, 0));
        let text: String = decode(fixture!("v5-storage0-bom-crlf.dat"));
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend(text.as_bytes());
        assert_eq!(read(&with_bom).unwrap(), first());
        assert_eq!(read(text.as_bytes()).unwrap(), first());
    }

    #[test]
    fn ten_thousand_lines_read_quickly() {
        let text: String = decode(fixture!("v5-storage0-bom-crlf.dat"));
        let (head, data) = text.split_once("Data:\r\n").unwrap();
        let lines: Vec<&str> = data.lines().collect();
        let mut big = format!("{head}Data:\r\n");
        for n in 0..10_000 {
            big.push_str(&lines[n % lines.len()].replacen("C:\\ops\\", &format!("C:\\ops\\{n}\\"), 1));
            big.push_str("\r\n");
        }
        let bytes = utf16(&big, true);
        let start = Instant::now();
        let f = read(&bytes).unwrap();
        assert_eq!(f.items.len(), 10_000);
        assert!(start.elapsed().as_millis() < 500, "{:?}", start.elapsed());
    }
}
