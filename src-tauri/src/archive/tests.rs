use super::*;
use tree::{ENCRYPTED, IN_ARCHIVE, UNREADABLE};
use std::fs;
use std::io::Write;
use zip::write::SimpleFileOptions;

/// A fresh folder for one test, deleted when it ends (and with the last one, what the tests extracted).
fn scratch(name: &str) -> extract::testing::Scratch {
    extract::testing::Scratch::new(name)
}

fn text(path: &Path) -> String {
    path.to_str().unwrap().to_string()
}

/// Writes a zip of `files` (a name ending in "/" is a folder).
fn write_zip(path: &Path, files: &[(&str, &[u8])], method: zip::CompressionMethod) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let options = SimpleFileOptions::default().compression_method(method).last_modified_time(zip::DateTime::from_date_and_time(2024, 6, 1, 12, 30, 0).unwrap());
    for (name, data) in files {
        if name.ends_with('/') {
            zip.add_directory(*name, options).unwrap();
        } else {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
    }
    zip.finish().unwrap();
}

/// A listing as `fsx::encode_listing` lays it out.
#[derive(Debug)]
struct Decoded {
    path: String,
    archive: u32,
    outer: u32,
    entries: u32,
    cached: bool,
    items: Vec<(String, bool, u64, u32)>,
}

fn decode(bytes: &[u8]) -> Decoded {
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let f64_at = |at: usize| f64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    let count = u32_at(0) as usize;
    let path_len = u32_at(4) as usize;
    let text = 40 + 24 * count;
    let path = std::str::from_utf8(&bytes[text..text + path_len]).unwrap().to_string();
    let names: Vec<u16> = std::str::from_utf8(&bytes[text + path_len..]).unwrap().encode_utf16().collect();
    let mut at = 0;
    let items = (0..count)
        .map(|i| {
            let len = u32_at(40 + 20 * count + 4 * i) as usize;
            let name = String::from_utf16(&names[at..at + len]).unwrap();
            at += len;
            let attrs = u32_at(40 + 16 * count + 4 * i);
            (name, attrs & 0x10 != 0, f64_at(40 + 8 * i) as u64, attrs)
        })
        .collect();
    Decoded { path, archive: u32_at(16), outer: u32_at(20), entries: u32_at(24), cached: u32_at(28) & 1 != 0, items }
}

fn listing(path: &str) -> Decoded {
    decode(&list(path, Instant::now()).expect("an archive path").expect("a listing"))
}

fn names(listed: &Decoded) -> Vec<&str> {
    listed.items.iter().map(|(n, ..)| n.as_str()).collect()
}

fn copy(path: &str, purpose: Purpose) -> Result<PathBuf, Failure> {
    let spot = spot(path).expect("an archive path")?;
    copy_of(&spot.archive, spot.node, purpose)
}

fn stage_all(paths: &[String]) -> Result<Staged, Failure> {
    let channel = Channel::new(|_| Ok(()));
    stage(paths, "copy", &AtomicBool::new(false), &channel)
}

fn fixture(name: &str, dir: &Path) -> String {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("archive").join("fixtures").join(name);
    let dest = dir.join(name);
    fs::copy(source, &dest).unwrap();
    text(&dest)
}

#[test]
fn finds_where_a_path_goes_into_an_archive() {
    let dir = scratch("locate");
    let zip = dir.join("photos.zip");
    write_zip(&zip, &[("a.txt", b"a")], zip::CompressionMethod::Stored);
    // A folder named like an archive is a folder.
    fs::create_dir_all(dir.join("folder.zip").join("inner")).unwrap();
    let z = text(&zip);
    let at = |outer: &str, inner: &str| Some(Located { outer: outer.into(), inner: inner.into(), format: Format::Zip });
    assert_eq!(locate(&z), at(&z, ""));
    assert_eq!(locate(&format!("{z}\\2024\\summer\\")), at(&z, "2024\\summer"));
    assert_eq!(locate(&format!("{z}/2024/x.jpg")), at(&z, "2024\\x.jpg"));
    assert_eq!(locate(&text(&dir.join("folder.zip").join("inner"))), None);
    assert_eq!(locate(&text(&dir.join("missing.zip").join("x"))), None);
    assert_eq!(locate(&text(&dir)), None);
    assert!(is_inside(&format!("{z}\\a.txt")) && !is_inside(&z) && is_read_only(&z));
    assert_eq!(prefix_units("C:\\é😀\\b.zip\\c", 3), "C:\\é😀\\b.zip".encode_utf16().count() as u32);
    assert_eq!(prefix_units("\\\\server\\share\\x.7z\\y", 3), 19);
}

#[test]
fn opens_zips_by_other_names_on_request_and_by_their_bytes() {
    let dir = scratch("claims");
    // A .docx is a zip: Enter opens it in Word, but a path into it goes into it.
    let docx = dir.join("report.docx");
    write_zip(&docx, &[("word/document.xml", b"<w:document/>"), ("[Content_Types].xml", b"<Types/>")], zip::CompressionMethod::Deflated);
    let d = text(&docx);
    assert!(!is_archive_name(&d) && Format::claimed("report.DOCX") == Some((Format::Zip, read::Claim::OnRequest)));
    assert_eq!(names(&listing(&d)), ["word", "[Content_Types].xml"]);
    assert_eq!(names(&listing(&format!("{d}\\word"))), ["document.xml"]);
    // A zip named .bin is found by its first bytes, once a path into it fails as a folder.
    let bin = dir.join("data.bin");
    write_zip(&bin, &[("inside.txt", b"hi")], zip::CompressionMethod::Stored);
    let b = text(&bin);
    assert_eq!(locate(&format!("{b}\\inside.txt")), None, "not before it's sniffed");
    assert!(sniff(&format!("{b}\\inside.txt")));
    assert_eq!(locate(&format!("{b}\\inside.txt")).map(|l| (l.inner, l.format)), Some(("inside.txt".into(), Format::Zip)));
    assert_eq!(names(&listing(&b)), ["inside.txt"]);
    // Nothing else is sniffed into an archive: a text file, a folder, a missing path.
    fs::write(dir.join("notes.txt"), "PK? no").unwrap();
    assert!(!sniff(&text(&dir.join("notes.txt").join("x"))));
    assert!(!sniff(&text(&dir.join("nowhere").join("x"))));
    assert!(!sniff(&text(&dir)));
    // A zip in a zip under another name opens by its bytes too.
    let mut inner = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut inner));
        z.start_file("deep.txt", SimpleFileOptions::default()).unwrap();
        z.write_all(b"deep").unwrap();
        z.finish().unwrap();
    }
    let outer = dir.join("outer.zip");
    write_zip(&outer, &[("blob.dat", &inner)], zip::CompressionMethod::Stored);
    assert_eq!(names(&listing(&format!("{}\\blob.dat", text(&outer)))), ["deep.txt"]);
}

#[test]
fn lists_a_zip_like_a_folder() {
    let dir = scratch("list");
    let zip = dir.join("photos.zip");
    let files: &[(&str, &[u8])] = &[
        ("docs/readme.txt", b"hello"),
        ("photos/2024/summer/beach10.jpg", &[1; 300]),
        ("photos/2024/summer/beach9.jpg", &[2; 200]),
        ("empty/", b""),
        ("é😀/日本.txt", b"konnichiwa"),
        ("top.bin", &[3; 7]),
    ];
    write_zip(&zip, files, zip::CompressionMethod::Deflated);
    let z = text(&zip);
    let top = listing(&z);
    assert_eq!(names(&top), ["docs", "empty", "photos", "é😀", "top.bin"]);
    assert_eq!((top.path.as_str(), top.archive as usize, top.outer as usize, top.entries), (z.as_str(), z.encode_utf16().count(), z.encode_utf16().count(), 11));
    // Folders carry their totals; everything is marked as being in an archive.
    assert_eq!(top.items.iter().map(|i| i.2).collect::<Vec<_>>(), [5, 0, 500, 10, 7]);
    assert!(top.items.iter().all(|i| i.3 & IN_ARCHIVE != 0));
    let summer = listing(&format!("{z}\\PHOTOS\\2024\\summer"));
    assert_eq!(names(&summer), ["beach9.jpg", "beach10.jpg"]);
    assert!(summer.cached, "read once, then from memory");
    assert_eq!(names(&listing(&format!("{z}\\é😀"))), ["日本.txt"]);
    assert!(list(&format!("{z}\\nothing"), Instant::now()).unwrap().is_err());
    assert!(list(&format!("{z}\\top.bin"), Instant::now()).unwrap().is_err(), "a file isn't a folder");
    assert!(resolve(&format!("{z}\\photos")).unwrap().unwrap().dir);
    assert!(!resolve(&format!("{z}\\top.bin")).unwrap().unwrap().dir);
    assert!(resolve(&z).unwrap().unwrap().dir);
    // Sizes of the folders in a folder, as the archive has them.
    let (total, sizes, engine) = folder_sizes(&format!("{z}\\photos")).unwrap().unwrap();
    assert_eq!((total, sizes, engine), (500, vec![("2024".to_string(), 500)], "zip"));
    // A changed archive is read again.
    write_zip(&zip, &[("new.txt", b"new!")], zip::CompressionMethod::Deflated);
    let again = listing(&z);
    assert_eq!((names(&again), again.cached), (vec!["new.txt"], false));
}

#[test]
fn opens_archives_inside_archives() {
    let dir = scratch("nested");
    let inner = dir.join("inner.zip");
    write_zip(&inner, &[("deep/x.txt", b"deep inside")], zip::CompressionMethod::Deflated);
    let outer = dir.join("outer.zip");
    write_zip(&outer, &[("folder/inner.zip", &fs::read(&inner).unwrap())], zip::CompressionMethod::Stored);
    let o = text(&outer);
    let at = format!("{o}\\folder\\inner.zip");
    let root = listing(&at);
    assert_eq!(names(&root), ["deep"]);
    assert_eq!((root.archive as usize, root.outer as usize), (at.encode_utf16().count(), o.encode_utf16().count()));
    let deep = listing(&format!("{at}\\deep"));
    assert_eq!(names(&deep), ["x.txt"]);
    let copy = copy(&format!("{at}\\deep\\x.txt"), Purpose::Preview).unwrap();
    assert_eq!(fs::read_to_string(copy).unwrap(), "deep inside");
    assert!(resolve(&at).unwrap().unwrap().dir);
}

#[test]
fn extracts_one_file_for_previews_once() {
    let dir = scratch("copy");
    let zip = dir.join("a.zip");
    write_zip(&zip, &[("sub/note.txt", b"a note"), ("big.bin", &[0; 1000])], zip::CompressionMethod::Deflated);
    let z = text(&zip);
    let first = copy(&format!("{z}\\sub\\note.txt"), Purpose::Preview).unwrap();
    assert_eq!(fs::read_to_string(&first).unwrap(), "a note");
    assert_eq!(first.file_name().unwrap(), "note.txt");
    assert!(fs::metadata(&first).unwrap().permissions().readonly(), "changes can't go back into the archive");
    let modified = fs::metadata(&first).unwrap().modified().unwrap();
    assert!(modified > std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    assert_eq!(copy(&format!("{z}\\SUB\\note.txt"), Purpose::Thumbnail).unwrap(), first, "made once");
    assert!(matches!(copy(&format!("{z}\\sub"), Purpose::Preview), Err(Failure::NotAFolder)));
    let (start, more) = read_prefix(&format!("{z}\\big.bin"), 10).unwrap().unwrap();
    assert_eq!((start.len(), more), (10, true));
    assert_eq!(read_prefix(&format!("{z}\\sub\\note.txt"), 1024).unwrap().unwrap(), (b"a note".to_vec(), false));
    assert!(read_prefix(&z, 10).is_none(), "the archive itself is a file on disk");
    assert_eq!(thumbnail_source(&format!("{z}\\sub\\note.txt")).unwrap().unwrap(), text(&first));
    assert!(thumbnail_source(&z).is_none());
}

#[test]
fn copies_a_selection_out_with_its_folders() {
    let dir = scratch("stage");
    let zip = dir.join("a.zip");
    write_zip(&zip, &[("docs/a.txt", b"A"), ("docs/sub/b.txt", b"BB"), ("docs/empty/", b""), ("c.txt", b"CCC"), ("other/c.txt", b"other")], zip::CompressionMethod::Deflated);
    let z = text(&zip);
    let staged = stage_all(&[format!("{z}\\docs"), format!("{z}\\c.txt"), format!("{z}\\other\\c.txt")]).unwrap();
    let folder = PathBuf::from(&staged.folder);
    assert!(extract::is_ours(&folder));
    assert_eq!(staged.paths.len(), 3);
    assert_eq!(fs::read_to_string(folder.join("docs").join("a.txt")).unwrap(), "A");
    assert_eq!(fs::read_to_string(folder.join("docs").join("sub").join("b.txt")).unwrap(), "BB");
    assert!(folder.join("docs").join("empty").is_dir());
    assert_eq!(fs::read_to_string(folder.join("c.txt")).unwrap(), "CCC");
    // Two of the same name each keep theirs.
    assert_eq!(fs::read_to_string(&staged.paths[2]).unwrap(), "other");
    assert!(staged.skipped.is_empty());
    assert_eq!(staged.bytes, 11);
    assert!(!fs::metadata(folder.join("c.txt")).unwrap().permissions().readonly(), "copies out are ordinary files");
    extract::remove_tree(&folder);
    assert!(!folder.exists());
}

#[test]
fn keeps_every_entry_inside_where_it_is_extracted() {
    let dir = scratch("slip");
    let zip = dir.join("evil.zip");
    write_zip(&zip, &[("../../evil.txt", b"1"), ("/abs/evil2.txt", b"2"), ("C:\\Windows\\evil3.txt", b"3"), ("a/../../evil4.txt", b"4"), ("con.txt", b"5"), ("what?.txt", b"6")], zip::CompressionMethod::Stored);
    let z = text(&zip);
    let top = listing(&z);
    assert_eq!(names(&top), ["a", "abs", "C_", "_con.txt", "evil.txt", "what_.txt"]);
    let paths: Vec<String> = top.items.iter().map(|(n, ..)| format!("{z}\\{n}")).collect();
    let staged = stage_all(&paths).unwrap();
    let folder = PathBuf::from(&staged.folder);
    let mut found = Vec::new();
    let mut stack = vec![folder.clone()];
    while let Some(at) = stack.pop() {
        for entry in fs::read_dir(&at).unwrap().flatten() {
            if entry.path().is_dir() {
                stack.push(entry.path());
            } else {
                found.push(entry.path());
            }
        }
    }
    assert_eq!(found.len(), 6);
    assert!(found.iter().all(|p| p.starts_with(&folder)));
    assert!(!dir.parent().unwrap().join("evil.txt").exists());
    extract::remove_tree(&folder);
}

/// Sets a zip entry's method, uncompressed size or CRC32 in both its local and central headers.
fn patch_zip(path: &Path, name: &str, method: Option<u16>, size: Option<u32>) {
    patch_zip_crc(path, name, method, size, None)
}

fn patch_zip_crc(path: &Path, name: &str, method: Option<u16>, size: Option<u32>, crc: Option<u32>) {
    let mut bytes = fs::read(path).unwrap();
    let mut patched = 0;
    for at in 0..bytes.len().saturating_sub(4) {
        let (sig, name_at) = match &bytes[at..at + 4] {
            b"PK\x03\x04" => (30, 30),
            b"PK\x01\x02" => (46, 46),
            _ => continue,
        };
        let len_at = if sig == 30 { 26 } else { 28 };
        let len = u16::from_le_bytes([bytes[at + len_at], bytes[at + len_at + 1]]) as usize;
        if bytes.get(at + name_at..at + name_at + len) != Some(name.as_bytes()) {
            continue;
        }
        let (method_at, crc_at, size_at) = if sig == 30 { (8, 14, 22) } else { (10, 16, 24) };
        if let Some(method) = method {
            bytes[at + method_at..at + method_at + 2].copy_from_slice(&method.to_le_bytes());
        }
        if let Some(size) = size {
            bytes[at + size_at..at + size_at + 4].copy_from_slice(&size.to_le_bytes());
        }
        if let Some(crc) = crc {
            bytes[at + crc_at..at + crc_at + 4].copy_from_slice(&crc.to_le_bytes());
        }
        patched += 1;
    }
    assert_eq!(patched, 2, "local and central header of {name}");
    fs::write(path, bytes).unwrap();
}

#[test]
fn refuses_zip_bombs() {
    let dir = scratch("bomb");
    // An entry claiming to unpack a thousand times over, to a gigabyte.
    let ratio = dir.join("ratio.zip");
    write_zip(&ratio, &[("zeros.bin", &[0; 100_000]), ("fine.txt", b"fine")], zip::CompressionMethod::Deflated);
    patch_zip(&ratio, "zeros.bin", None, Some(1 << 30));
    let r = text(&ratio);
    assert!(matches!(copy(&format!("{r}\\zeros.bin"), Purpose::Open), Err(Failure::Bomb(_))));
    let staged = stage_all(&[format!("{r}\\zeros.bin"), format!("{r}\\fine.txt")]).unwrap();
    assert_eq!(staged.skipped.len(), 1);
    assert!(staged.skipped[0].reason.contains("zip bomb"), "{}", staged.skipped[0].reason);
    assert_eq!((count_text(17_543_859), count_text(999), count_text(1000)), ("17,543,859".into(), "999".into(), "1,000".into()));
    assert_eq!(staged.paths.len(), 1);
    // Many entries sharing one compressed stream: the overlapping kind of bomb.
    let overlap = dir.join("overlap.zip");
    {
        let mut zip = zip::ZipWriter::new(fs::File::create(&overlap).unwrap());
        zip.start_file("a.bin", SimpleFileOptions::default()).unwrap();
        zip.write_all(&[7; 5000]).unwrap();
        for i in 0..20 {
            zip.shallow_copy_file("a.bin", &format!("copy{i}.bin")).unwrap();
        }
        zip.finish().unwrap();
    }
    let o = text(&overlap);
    assert_eq!(listing(&o).items.len(), 21, "it still lists");
    assert!(matches!(copy(&format!("{o}\\copy3.bin"), Purpose::Preview), Err(Failure::Bomb(_))));
    assert!(stage_all(&[format!("{o}\\a.bin")]).is_err());
    // An entry that holds more than it says: it's cut off where it said it ends, and fails.
    let lying = dir.join("lying.zip");
    write_zip(&lying, &[("more.txt", &[b'x'; 5000])], zip::CompressionMethod::Deflated);
    patch_zip(&lying, "more.txt", None, Some(100));
    let l = text(&lying);
    assert!(copy(&format!("{l}\\more.txt"), Purpose::Preview).is_err());
}

#[test]
fn names_what_it_cannot_read() {
    let dir = scratch("unreadable");
    let zip = dir.join("mp3.zip");
    write_zip(&zip, &[("song.bin", b"pretend mp3"), ("ok.txt", b"ok")], zip::CompressionMethod::Stored);
    patch_zip(&zip, "song.bin", Some(94), None);
    let z = text(&zip);
    let top = listing(&z);
    assert!(top.items.iter().find(|i| i.0 == "song.bin").unwrap().3 & UNREADABLE != 0);
    let error = copy(&format!("{z}\\song.bin"), Purpose::Preview).unwrap_err().message();
    assert!(error.contains("MP3"), "{error}");
    assert_eq!(fs::read_to_string(copy(&format!("{z}\\ok.txt"), Purpose::Preview).unwrap()).unwrap(), "ok");
}

#[test]
fn reads_zstandard_entries() {
    let dir = scratch("zstd");
    let text_data = "Zstandard in a zip, as WinZip and Python 3.14 write it. ".repeat(500);
    let packed = ruzstd::encoding::compress_to_vec(text_data.as_bytes(), ruzstd::encoding::CompressionLevel::Fastest);
    // Stored entries holding Zstandard data, then marked as method 93 (and 20, its old number), with
    // the size and CRC32 of what they unpack to.
    let zip = dir.join("zstd.zip");
    write_zip(&zip, &[("z.txt", &packed), ("old.txt", &packed), ("bad.txt", &packed)], zip::CompressionMethod::Stored);
    for (name, method, crc) in [("z.txt", 93, crc32fast::hash(text_data.as_bytes())), ("old.txt", 20, crc32fast::hash(text_data.as_bytes())), ("bad.txt", 93, 12345)] {
        patch_zip_crc(&zip, name, Some(method), Some(text_data.len() as u32), Some(crc));
    }
    let z = text(&zip);
    let top = listing(&z);
    assert!(top.items.iter().all(|i| i.3 & UNREADABLE == 0), "Zstandard is readable now");
    assert_eq!(top.items.iter().find(|i| i.0 == "z.txt").unwrap().2, text_data.len() as u64);
    assert_eq!(fs::read_to_string(copy(&format!("{z}\\z.txt"), Purpose::Preview).unwrap()).unwrap(), text_data);
    assert_eq!(fs::read_to_string(copy(&format!("{z}\\old.txt"), Purpose::Preview).unwrap()).unwrap(), text_data);
    assert_eq!(read_prefix(&format!("{z}\\z.txt"), 9).unwrap().unwrap().0, b"Zstandard");
    // A CRC that doesn't match what comes out: damaged, and nothing is left behind.
    let error = copy(&format!("{z}\\bad.txt"), Purpose::Preview).unwrap_err();
    assert!(matches!(error, Failure::Damaged(_)), "{error:?}");
}

#[test]
fn says_when_entries_are_password_protected() {
    let dir = scratch("encrypted");
    for name in ["aes.zip", "zipcrypto.zip", "encrypted.7z"] {
        let archive = fixture(name, &dir);
        let docs = listing(&format!("{archive}\\docs"));
        assert_eq!(names(&docs), ["readme.txt"], "{name}");
        assert!(docs.items[0].3 & ENCRYPTED != 0, "{name} marks it");
        let error = copy(&format!("{archive}\\docs\\readme.txt"), Purpose::Preview).unwrap_err();
        assert!(matches!(error, Failure::Encrypted), "{name}: {error:?}");
        assert!(error.message().contains("password-protected"));
        assert!(stage_all(&[format!("{archive}\\docs")]).unwrap_err().message().contains("password-protected"));
    }
    // A 7z whose list of files is encrypted too can't even be listed.
    let hidden = fixture("encrypted-list.7z", &dir);
    let error = list(&hidden, Instant::now()).unwrap().unwrap_err();
    assert!(error.contains("password-protected"), "{error}");
}

#[test]
fn reads_what_7zip_made() {
    let dir = scratch("real");
    for name in ["real.7z", "real.zip"] {
        let archive = fixture(name, &dir);
        let top = listing(&archive);
        assert_eq!(names(&top), ["docs", "photos", "emoji-😀.txt", "日本語.txt"], "{name}");
        assert_eq!(top.items[1].2, 440, "{name}: a folder's total");
        let ete = format!("{archive}\\photos\\2024\\été");
        assert_eq!(names(&listing(&ete)), ["pic.txt"], "{name}");
        let pic = copy(&format!("{ete}\\pic.txt"), Purpose::Preview).unwrap();
        // (Written on Windows: CRLF line ends.)
        assert_eq!(fs::read_to_string(pic).unwrap(), "not really a picture\r\n".repeat(20));
        assert_eq!(fs::read_to_string(copy(&format!("{archive}\\日本語.txt"), Purpose::Open).unwrap()).unwrap(), "こんにちは\r\n");
        let mut found = Vec::new();
        let (engine, limited) = search(&archive, "PIC", false, &[], None, &AtomicBool::new(false), &mut |e| found.push(e)).unwrap().unwrap();
        assert_eq!((engine, limited), (if name.ends_with("7z") { "7z" } else { "zip" }, false));
        assert_eq!(found.iter().map(|e| e.path.clone().unwrap()).collect::<Vec<_>>(), [format!("{ete}\\pic.txt")]);
        // Search works like fd's: globs over whole names, every word must match, excluded folders skipped.
        let mut globbed = Vec::new();
        search(&archive, "*.txt", false, &["photos".into()], Some(2), &AtomicBool::new(false), &mut |e| globbed.push(e.name)).unwrap().unwrap();
        assert_eq!(globbed.len(), 2);
        assert!(!globbed.contains(&"pic.txt".to_string()));
    }
}

#[test]
fn reads_solid_7z_blocks_once_for_thumbnails() {
    let dir = scratch("solid");
    let path = dir.join("pics.7z");
    {
        let mut writer = sevenz_rust2::ArchiveWriter::create(&path).unwrap();
        let entries: Vec<sevenz_rust2::ArchiveEntry> = ["a.jpg", "b.jpg", "notes.txt", "c.jpg"].iter().map(|n| sevenz_rust2::ArchiveEntry::new_file(&format!("pics/{n}"))).collect();
        let readers: Vec<_> = [&b"aaaa"[..], &b"bbbbbb"[..], &b"some notes"[..], &b"cccccccc"[..]].into_iter().map(sevenz_rust2::SourceReader::new).collect();
        writer.push_archive_entries(entries, readers).unwrap();
        writer.finish().unwrap();
    }
    let p = text(&path);
    let pics = listing(&format!("{p}\\pics"));
    assert_eq!(names(&pics), ["a.jpg", "b.jpg", "c.jpg", "notes.txt"]);
    assert_eq!(pics.items.iter().map(|i| i.2).collect::<Vec<_>>(), [4, 6, 8, 10]);
    let c = copy(&format!("{p}\\pics\\c.jpg"), Purpose::Thumbnail).unwrap();
    assert_eq!(fs::read(c).unwrap(), b"cccccccc");
    // The pictures read through on the way are kept: no second read for them.
    let key = |name: &str| {
        let spot = spot(&format!("{p}\\pics\\{name}")).unwrap().unwrap();
        format!("{}|{}|{}|{}", spot.archive.path.to_lowercase(), spot.archive.tree.path_of(spot.node).to_lowercase(), spot.archive.stamp.0, spot.archive.stamp.1)
    };
    let (a, b, notes) = (key("a.jpg"), key("b.jpg"), key("notes.txt"));
    {
        let state = state();
        assert!(state.copies.contains_key(&a) && state.copies.contains_key(&b));
        assert!(!state.copies.contains_key(&notes));
    }
    assert_eq!(fs::read(copy(&format!("{p}\\pics\\a.jpg"), Purpose::Thumbnail).unwrap()).unwrap(), b"aaaa");
    let staged = stage_all(&[format!("{p}\\pics")]).unwrap();
    assert_eq!(fs::read_to_string(Path::new(&staged.folder).join("pics").join("notes.txt")).unwrap(), "some notes");
}

#[test]
fn never_falls_over_on_damaged_archives() {
    let dir = scratch("damaged");
    let mut archives = Vec::new();
    for name in ["real.7z", "real.zip", "encrypted.7z", "aes.zip"] {
        archives.push(fs::read(fixture(name, &dir)).unwrap());
    }
    let mut zip_bytes = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_bytes));
        zip.start_file("x.txt", SimpleFileOptions::default()).unwrap();
        zip.write_all(&[b'x'; 2000]).unwrap();
        zip.finish().unwrap();
    }
    archives.push(zip_bytes);
    for (n, original) in archives.iter().enumerate() {
        let ext = if original.starts_with(b"7z") { "7z" } else { "zip" };
        // Every byte flipped in turn, and every length cut off: an error at worst, never a panic.
        let step = (original.len() / 120).max(1);
        for at in (0..original.len()).step_by(step) {
            for damaged in [{
                let mut b = original.clone();
                b[at] ^= 0x5A;
                b
            }, original[..at].to_vec()]
            {
                let path = dir.join(format!("d{n}-{at}-{}.{ext}", damaged.len()));
                fs::write(&path, &damaged).unwrap();
                let p = text(&path);
                if let Some(Ok(bytes)) = list(&p, Instant::now()) {
                    for (name, ..) in decode(&bytes).items {
                        if let Ok(staged) = stage_all(&[format!("{p}\\{name}")]) {
                            extract::remove_tree(Path::new(&staged.folder));
                        }
                    }
                }
                let _ = fs::remove_file(&path);
            }
        }
    }
    // Not an archive at all.
    let fake = dir.join("fake.zip");
    fs::write(&fake, b"this is not a zip file at all").unwrap();
    let error = list(&text(&fake), Instant::now()).unwrap().unwrap_err();
    assert!(error.contains("list of files, at the end of the file, is missing"), "{error}");
    let fake = dir.join("fake.7z");
    fs::write(&fake, b"7z\xBC\xAF\x27\x1C but then nothing").unwrap();
    assert!(list(&text(&fake), Instant::now()).unwrap().is_err());
    // A zip whose data was damaged after it was made: its checksum gives it away.
    let zip = dir.join("bitrot.zip");
    write_zip(&zip, &[("data.txt", &[b'a'; 4000])], zip::CompressionMethod::Stored);
    let mut bytes = fs::read(&zip).unwrap();
    bytes[30 + 8 + 100] ^= 1;
    fs::write(&zip, bytes).unwrap();
    let error = copy(&format!("{}\\data.txt", text(&zip)), Purpose::Preview).unwrap_err();
    assert!(matches!(error, Failure::Damaged(_)), "{error:?}");
}

#[test]
fn maps_what_fills_an_archive() {
    let dir = scratch("usage");
    let zip = dir.join("a.zip");
    write_zip(&zip, &[("big/one.bin", &[1; 500]), ("big/deep/two.bin", &[2; 300]), ("small.txt", &[3; 10]), ("empty/", b"")], zip::CompressionMethod::Deflated);
    let z = text(&zip);
    let (nodes, limited, format) = usage(&z, 6, 100).unwrap().unwrap();
    let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
    // The folder, then what's in it biggest first (empty ones left out), then what's in those.
    assert_eq!(names, ["", "big", "small.txt", "one.bin", "deep", "two.bin"]);
    assert_eq!(nodes.iter().map(|n| n.parent).collect::<Vec<_>>(), [NO_PARENT, 0, 0, 1, 1, 4]);
    assert_eq!((nodes[0].size, limited, format), (810, false, Format::Zip));
    let (nodes, limited, _) = usage(&z, 1, 1).unwrap().unwrap();
    assert_eq!((nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), limited), (vec!["", "big"], true));
}

/// Names from old Windows zip tools are in the PC's OEM code page, as File Explorer reads them.
#[cfg(windows)]
#[test]
fn reads_names_in_the_oem_code_page() {
    let oem = unsafe { windows::Win32::Globalization::GetOEMCP() };
    // "été" was written by 7-Zip on code page 437; 850 and 858 have é in the same place.
    if ![437, 850, 858].contains(&oem) {
        return;
    }
    let dir = scratch("oem");
    let archive = fixture("oem.zip", &dir);
    assert_eq!(names(&listing(&format!("{archive}\\photos\\2024"))), ["été"]);
}

/// `cargo test --release -- --ignored --nocapture lists_50000`: how fast a big zip lists.
#[test]
#[ignore]
fn lists_50000_entries_in_milliseconds() {
    let dir = scratch("big");
    let zip = dir.join("big.zip");
    {
        let mut writer = zip::ZipWriter::new(std::io::BufWriter::new(fs::File::create(&zip).unwrap()));
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        // Folder by folder, as zip tools write them.
        let mut names: Vec<String> = (0..50_000).map(|i| format!("data/{:03}/sub{}/file-{i:06}-résumé.txt", i % 500, i % 7)).collect();
        names.sort();
        for (i, name) in names.iter().enumerate() {
            writer.start_file(name, options).unwrap();
            write!(writer, "entry {i}").unwrap();
        }
        writer.finish().unwrap();
    }
    let z = text(&zip);
    for _ in 0..3 {
        let started = Instant::now();
        let (_, items) = Source::open(&zip, Format::Zip).unwrap();
        let read = ms_since(started);
        let started = Instant::now();
        let tree = Tree::build(&items);
        println!("parts: read {read:.1} ms, tree {:.1} ms ({} nodes)", ms_since(started), tree.len());
    }
    for round in 0..5 {
        // Forget it, so each round reads the archive again.
        state().opened.clear();
        let started = Instant::now();
        let top = listing(&z);
        let first = ms_since(started);
        let started = Instant::now();
        let folder = listing(&format!("{z}\\data\\123\\sub4"));
        let next = ms_since(started);
        println!("round {round}: top in {first:.1} ms ({} entries), a folder after that in {next:.2} ms ({} items)", top.entries, folder.items.len());
        assert_eq!(top.entries, 50_000 + 1 + 500 + 3500);
    }
}
