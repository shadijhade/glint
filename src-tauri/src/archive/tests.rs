use super::*;
use codec::Codec;
use tree::{ENCRYPTED, IN_ARCHIVE, UNREADABLE};
use std::fs;
use std::io::Write;
use zip::write::SimpleFileOptions;

/// A tar of `files` (a name ending in "/" is a folder), as GNU tar writes one: long names too.
fn write_tar(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = ::tar::Builder::new(Vec::new());
    for (name, data) in files {
        let mut header = ::tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_mtime(1_700_000_000);
        if name.ends_with('/') {
            header.set_entry_type(::tar::EntryType::Directory);
            header.set_size(0);
            builder.append_data(&mut header, name, std::io::empty()).unwrap();
        } else {
            header.set_size(data.len() as u64);
            builder.append_data(&mut header, name, *data).unwrap();
        }
    }
    builder.into_inner().unwrap()
}

/// `data` compressed the way each codec's own tool writes it.
fn compressed(data: &[u8], codec: Codec) -> Vec<u8> {
    match codec {
        Codec::Plain => data.to_vec(),
        Codec::Gz => {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            e.write_all(data).unwrap();
            e.finish().unwrap()
        }
        Codec::Xz => {
            let mut w = lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1)).unwrap();
            w.write_all(data).unwrap();
            w.finish().unwrap()
        }
        Codec::Bz2 => {
            let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
            e.write_all(data).unwrap();
            e.finish().unwrap()
        }
        Codec::Zst => ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest),
        Codec::Lz4 => {
            let mut e = lz4_flex::frame::FrameEncoder::new(Vec::new());
            e.write_all(data).unwrap();
            e.finish().unwrap()
        }
    }
}

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
fn reads_tars_with_every_codec() {
    let dir = scratch("tars");
    let folder = "very-long-folder-name".repeat(8);
    let long = format!("{folder}/deep.txt");
    let files: &[(&str, &[u8])] = &[("docs/", b""), ("docs/readme.txt", b"hello tar"), ("photos/a.jpg", &[7; 3000]), (&long, b"deep"), ("top.bin", &[1; 1500])];
    let tar = write_tar(files);
    for codec in Codec::ALL {
        let name = if codec == Codec::Plain { "photos.tar".to_string() } else { format!("photos.tar.{}", codec.ext()) };
        let path = dir.join(&name);
        fs::write(&path, compressed(&tar, codec)).unwrap();
        let p = text(&path);
        let top = listing(&p);
        assert_eq!(names(&top), ["docs", "photos", folder.as_str(), "top.bin"], "{name}");
        assert_eq!(top.items.iter().map(|i| i.2).collect::<Vec<_>>(), [9, 3000, 4, 1500], "{name}: folder totals");
        assert_eq!(names(&listing(&format!("{p}\\{folder}"))), ["deep.txt"], "{name}: a GNU long name");
        assert_eq!(fs::read(copy(&format!("{p}\\docs\\readme.txt"), Purpose::Preview).unwrap()).unwrap(), b"hello tar", "{name}");
        assert_eq!(read_prefix(&format!("{p}\\docs\\readme.txt"), 5).unwrap().unwrap().0, b"hello", "{name}");
        // A selection from across the stream, in one pass.
        let staged = stage_all(&[format!("{p}\\photos"), format!("{p}\\top.bin")]).unwrap();
        assert!(staged.skipped.is_empty(), "{name}: {:?}", staged.skipped);
        assert_eq!(fs::read(Path::new(&staged.paths[1])).unwrap(), [1; 1500], "{name}");
        assert_eq!(fs::read(Path::new(&staged.paths[0]).join("a.jpg")).unwrap(), [7; 3000], "{name}");
        extract::remove_tree(Path::new(&staged.folder));
        let label = spot(&p).unwrap().unwrap().archive.format.label();
        assert_eq!(label, if codec == Codec::Plain { "tar".to_string() } else { format!("tar.{}", codec.ext()) });
    }
    // What a file is comes from its bytes: a tar.gz called .gz is a tar, a plain tar called .tar.gz too.
    let renamed = dir.join("backup.gz");
    fs::write(&renamed, compressed(&tar, Codec::Gz)).unwrap();
    assert_eq!(names(&listing(&text(&renamed))), ["docs", "photos", folder.as_str(), "top.bin"]);
    let plain = dir.join("plain.tar.gz");
    fs::write(&plain, &tar).unwrap();
    assert_eq!(spot(&text(&plain)).unwrap().unwrap().archive.format, Format::Tar(Codec::Plain));
}

#[test]
fn serves_links_in_tars() {
    let dir = scratch("tarlinks");
    let mut builder = ::tar::Builder::new(Vec::new());
    let mut header = ::tar::Header::new_gnu();
    header.set_size(5);
    header.set_mode(0o644);
    builder.append_data(&mut header, "a.txt", &b"hello"[..]).unwrap();
    for (kind, name) in [(::tar::EntryType::Link, "hard.txt"), (::tar::EntryType::Symlink, "soft.txt")] {
        let mut link = ::tar::Header::new_gnu();
        link.set_entry_type(kind);
        link.set_size(0);
        builder.append_link(&mut link, name, "a.txt").unwrap();
    }
    let path = dir.join("links.tar.gz");
    fs::write(&path, compressed(&builder.into_inner().unwrap(), Codec::Gz)).unwrap();
    let p = text(&path);
    // A hard link reads its target's bytes (in a second pass, as they were passed already); a
    // symlink is a small file holding where it points.
    let staged = stage_all(&[format!("{p}\\a.txt"), format!("{p}\\hard.txt"), format!("{p}\\soft.txt")]).unwrap();
    let read = |i: usize| fs::read(Path::new(&staged.paths[i])).unwrap();
    assert_eq!((read(0), read(1), read(2)), (b"hello".to_vec(), b"hello".to_vec(), b"a.txt".to_vec()));
    extract::remove_tree(Path::new(&staged.folder));
}

#[test]
fn opens_single_compressed_files_as_a_folder_of_one() {
    let dir = scratch("singles");
    let log = "a line of the log\n".repeat(5000);
    for codec in [Codec::Gz, Codec::Xz, Codec::Bz2, Codec::Zst, Codec::Lz4] {
        let path = dir.join(format!("app.log.{}", codec.ext()));
        fs::write(&path, compressed(log.as_bytes(), codec)).unwrap();
        let p = text(&path);
        let top = listing(&p);
        assert_eq!(names(&top), ["app.log"], "{codec:?}");
        assert_eq!(top.items[0].2, log.len() as u64, "{codec:?}: its exact size");
        assert_eq!(fs::read_to_string(copy(&format!("{p}\\app.log"), Purpose::Preview).unwrap()).unwrap(), log, "{codec:?}");
    }
    // An xz file says how big it unpacks to in its index: nothing is decompressed to find out.
    let xz = dir.join("app.log.xz");
    assert_eq!(Codec::Xz.stated_size(&mut fs::File::open(&xz).unwrap()), Some(log.len() as u64));
    assert_eq!(Codec::Gz.stated_size(&mut fs::File::open(dir.join("app.log.gz")).unwrap()), None, "gzip's size is modulo 4 GB");
    // Two gzip members one after another (what `cat a.gz b.gz` makes) are one file.
    let mut two = compressed(b"first, ", Codec::Gz);
    two.extend(compressed(b"second", Codec::Gz));
    let path = dir.join("two.txt.gz");
    fs::write(&path, two).unwrap();
    assert_eq!(fs::read_to_string(copy(&format!("{}\\two.txt", text(&path)), Purpose::Preview).unwrap()).unwrap(), "first, second");
}

#[test]
fn extracts_here_next_to_the_archive() {
    let dir = scratch("here");
    let here = |path: &Path| tauri::async_runtime::block_on(archive_extract_here(text(path), "here".into(), Channel::new(|_| Ok(()))));
    // A single compressed file comes out as the file, next to it, numbered when its name is taken.
    let gz = dir.join("notes.txt.gz");
    fs::write(&gz, compressed(b"some notes", Codec::Gz)).unwrap();
    let first = here(&gz).unwrap();
    assert_eq!((Path::new(&first.folder), fs::read(&first.folder).unwrap()), (dir.join("notes.txt").as_path(), b"some notes".to_vec()));
    assert_eq!(Path::new(&here(&gz).unwrap().folder), dir.join("notes (2).txt"));
    // A compressed tar goes into a folder named without its ".tar".
    let tgz = dir.join("photos.tar.gz");
    fs::write(&tgz, compressed(&write_tar(&[("a/b.txt", b"bee")]), Codec::Gz)).unwrap();
    assert_eq!(Path::new(&here(&tgz).unwrap().folder), dir.join("photos"));
    assert_eq!(fs::read(dir.join("photos").join("a").join("b.txt")).unwrap(), b"bee");
    // Damaged past its index (which says how big it is): the extraction fails, and leaves nothing.
    let mut seed = 7u32;
    let noise: Vec<u8> = (0..40_000)
        .map(|_| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) as u8
        })
        .collect();
    let damaged = |data: &[u8]| {
        let mut xz = compressed(data, Codec::Xz);
        let middle = xz.len() / 2;
        xz[middle] ^= 0x5A;
        xz
    };
    let bad = dir.join("noise.bin.xz");
    fs::write(&bad, damaged(&noise)).unwrap();
    let error = here(&bad).unwrap_err();
    assert!(error.contains("damaged"), "{error}");
    assert!(!dir.join("noise.bin").exists());
    // A tar.xz damaged the same way doesn't open: its first read goes on to the checksum at its end.
    let bad = dir.join("noise.tar.xz");
    fs::write(&bad, damaged(&write_tar(&[("noise.bin", &noise)]))).unwrap();
    let error = list(&text(&bad), Instant::now()).unwrap().unwrap_err();
    assert!(error.contains("damaged"), "{error}");
}

/// A tar.zst of a 128 MB file of zeros, in about 4 KB: a frame of the tar header as a raw block, then
/// a thousand RLE blocks of 128 KB each (one stored byte apiece), made by hand rather than by
/// compressing 128 MB.
fn zstd_bomb() -> Vec<u8> {
    let header = tar::testing::header("zeros.bin", b'0', 128 << 20, "");
    let mut frame = vec![0x28, 0xB5, 0x2F, 0xFD];
    // Single segment, with an 8-byte content size: the header, the zeros, and the end of the tar.
    frame.push(0xE0);
    frame.extend((512u64 + (128 << 20) + 1024).to_le_bytes());
    let block = |frame: &mut Vec<u8>, kind: u32, size: u32, last: bool| frame.extend(&((size << 3) | (kind << 1) | last as u32).to_le_bytes()[..3]);
    block(&mut frame, 0, 512, false);
    frame.extend(header);
    for i in 0..1024 {
        block(&mut frame, 1, 128 << 10, false);
        frame.push(0);
        let _ = i;
    }
    block(&mut frame, 1, 1024, true);
    frame.push(0);
    frame
}

#[test]
fn refuses_stream_bombs_and_stops_when_asked() {
    let dir = scratch("streambombs");
    // 128 MB of zeros in a tar.zst: listing reads it all, and stops it as a bomb.
    let path = dir.join("bomb.tar.zst");
    fs::write(&path, zstd_bomb()).unwrap();
    assert!(fs::metadata(&path).unwrap().len() < 8 << 10);
    let error = list(&text(&path), Instant::now()).unwrap().unwrap_err();
    assert!(error.contains("zip bomb"), "{error}");
    // A first read stopped: it says so, and nothing is kept.
    let gz = dir.join("big.tar.gz");
    fs::write(&gz, compressed(&write_tar(&[("a.bin", &vec![9u8; 4 << 20])]), Codec::Gz)).unwrap();
    let scan = stream::Scan::new(0);
    scan.stop.store(true, Ordering::SeqCst);
    let stopped = Source::open(&gz, Format::Tar(Codec::Gz), &scan);
    assert!(matches!(stopped, Err(Failure::Cancelled)), "{:?}", stopped.err());
}

#[test]
fn peeks_at_big_streams_without_reading_them() {
    let dir = scratch("peek");
    let small = dir.join("small.tar.gz");
    fs::write(&small, compressed(&write_tar(&[("a.txt", b"abc"), ("b/c.txt", b"defg")]), Codec::Gz)).unwrap();
    let peek = tauri::async_runtime::block_on(archive_peek(text(&small))).unwrap();
    assert_eq!((peek.format, peek.entries, peek.unpacked), ("tar.gz", Some(3), Some(7)));
    // Past 64 MB, a compressed stream isn't read just because it's selected.
    let big = dir.join("big.tar.gz");
    let file = fs::File::create(&big).unwrap();
    file.set_len(65 << 20).unwrap();
    drop(file);
    let mut bytes = fs::OpenOptions::new().write(true).open(&big).unwrap();
    bytes.write_all(&[0x1F, 0x8B, 0x08]).unwrap();
    drop(bytes);
    let peek = tauri::async_runtime::block_on(archive_peek(text(&big))).unwrap();
    assert_eq!((peek.format, peek.packed, peek.entries), ("tar.gz", 65 << 20, None));
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

/// A 7z of `files` compressed with `methods` (compressor first, then its filters): solid, one block,
/// or with `solid` false a block per file.
fn write_7z(path: &Path, methods: Vec<sevenz_rust2::EncoderConfiguration>, files: &[(&str, &[u8])], solid: bool) {
    let mut writer = sevenz_rust2::ArchiveWriter::create(path).unwrap();
    writer.set_content_methods(methods);
    if solid {
        let entries = files.iter().map(|(name, _)| sevenz_rust2::ArchiveEntry::new_file(name)).collect();
        let readers = files.iter().map(|(_, data)| sevenz_rust2::SourceReader::new(*data)).collect();
        writer.push_archive_entries(entries, readers).unwrap();
    } else {
        for (name, data) in files {
            writer.push_archive_entry(sevenz_rust2::ArchiveEntry::new_file(name), Some(*data)).unwrap();
        }
    }
    writer.finish().unwrap();
}

#[test]
fn reads_7z_blocks_in_zstandard_and_lz4() {
    use sevenz_rust2::encoder_options::DeltaOptions;
    use sevenz_rust2::EncoderMethod;
    let dir = scratch("7zextra");
    // Calls for BCJ to rewrite, text, and 16-bit samples for Delta.
    let code: Vec<u8> = (0..20_000u32).flat_map(|i| [0xE8, i as u8, (i >> 8) as u8, 0, 0, 0x90]).collect();
    let words = "Zstandard in a 7z, as 7-Zip ZS and NanaZip write it. ".repeat(400);
    let samples: Vec<u8> = (0..30_000u32).flat_map(|i| ((i * 7) as u16).to_le_bytes()).collect();
    let files: &[(&str, &[u8])] = &[("bin/app.exe", &code), ("docs/readme.txt", words.as_bytes()), ("data/samples.raw", &samples)];
    let chains: Vec<(&str, Vec<sevenz_rust2::EncoderConfiguration>)> = vec![
        ("zstd", vec![EncoderMethod::ZSTD.into()]),
        ("bcj-zstd", vec![EncoderMethod::ZSTD.into(), EncoderMethod::BCJ_X86_FILTER.into()]),
        ("delta-zstd", vec![EncoderMethod::ZSTD.into(), DeltaOptions::from_distance(2).into()]),
        ("lz4", vec![EncoderMethod::LZ4.into()]),
    ];
    for (name, methods) in chains {
        // sevenz-rust2 0.24 writes a solid Delta + Zstandard block wrongly from its second file on
        // (it can't read it back either), so that one is a block per file.
        let kinds: &[bool] = if name == "delta-zstd" { &[false] } else { &[true, false] };
        for &solid in kinds {
            let path = dir.join(format!("{name}-{solid}.7z"));
            write_7z(&path, methods.clone(), files, solid);
            let p = text(&path);
            let top = listing(&p);
            assert_eq!(names(&top), ["bin", "data", "docs"], "{name}");
            assert!(top.items.iter().all(|i| i.3 & UNREADABLE == 0), "{name} reads");
            // All of it in one pass, then one file at a time, and the start of one.
            let staged = stage_all(&[format!("{p}\\bin"), format!("{p}\\docs"), format!("{p}\\data")]).unwrap();
            assert!(staged.skipped.is_empty(), "{name}: {:?}", staged.skipped);
            assert_eq!(fs::read(Path::new(&staged.paths[0]).join("app.exe")).unwrap(), code, "{name}");
            assert_eq!(fs::read(Path::new(&staged.paths[2]).join("samples.raw")).unwrap(), samples, "{name}");
            extract::remove_tree(Path::new(&staged.folder));
            assert_eq!(fs::read_to_string(copy(&format!("{p}\\docs\\readme.txt"), Purpose::Preview).unwrap()).unwrap(), words, "{name}");
            assert_eq!(read_prefix(&format!("{p}\\data\\samples.raw"), 4).unwrap().unwrap().0, samples[..4], "{name}");
        }
    }
    // Damaged in the middle of its Zstandard data: an error (a checksum, or the decoder's), not wrong bytes.
    let path = dir.join("bad-zstd.7z");
    write_7z(&path, vec![EncoderMethod::ZSTD.into()], &[("docs/readme.txt", words.as_bytes())], true);
    let packed = sevenz_rust2::Archive::read(&mut fs::File::open(&path).unwrap(), &sevenz_rust2::Password::empty()).unwrap().pack_sizes()[0];
    let mut bytes = fs::read(&path).unwrap();
    bytes[32 + packed as usize / 2] ^= 0x5A;
    fs::write(&path, bytes).unwrap();
    let error = copy(&format!("{}\\docs\\readme.txt", text(&path)), Purpose::Preview).unwrap_err();
    assert!(matches!(error, Failure::Damaged(_)), "{error:?}");
}

/// A cabinet folder to write: its compression and its files' names and bytes.
type CabFolder<'a> = (::cab::CompressionType, &'a [(&'a str, &'a [u8])]);

/// A cabinet of `folders`, written by the cab crate.
fn write_cab(path: &Path, folders: &[CabFolder]) {
    let mut builder = ::cab::CabinetBuilder::new();
    for (kind, files) in folders {
        let folder = builder.add_folder(*kind);
        for (name, _) in files.iter() {
            folder.add_file(*name);
        }
    }
    let mut writer = builder.build(fs::File::create(path).unwrap()).unwrap();
    let mut data = folders.iter().flat_map(|(_, files)| files.iter().map(|(_, d)| *d));
    while let Some(mut file) = writer.next_file().unwrap() {
        file.write_all(data.next().unwrap()).unwrap();
    }
    writer.finish().unwrap();
}

#[test]
fn reads_cabinets() {
    let dir = scratch("cabs");
    // 300 files in one MSZIP folder (an MSI's cabinet holds thousands), and a stored folder.
    let files: Vec<(String, Vec<u8>)> = (0..300).map(|i| (format!("dir{}\\file{i:03}.txt", i % 7), format!("file {i} ").repeat(50 + i).into_bytes())).collect();
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
    let stored = vec![9u8; 70_000];
    let path = dir.join("big.cab");
    write_cab(&path, &[(::cab::CompressionType::MsZip, &refs), (::cab::CompressionType::None, &[("stored.bin", &stored)])]);
    let p = text(&path);
    let top = listing(&p);
    assert_eq!(names(&top), ["dir0", "dir1", "dir2", "dir3", "dir4", "dir5", "dir6", "stored.bin"]);
    assert_eq!(top.items.iter().map(|i| i.2).sum::<u64>(), files.iter().map(|(_, d)| d.len() as u64).sum::<u64>() + 70_000);
    // All of it, each folder read once, front to back.
    let staged = stage_all(&top.items.iter().map(|(n, ..)| format!("{p}\\{n}")).collect::<Vec<_>>()).unwrap();
    assert!(staged.skipped.is_empty(), "{:?}", staged.skipped);
    for (name, data) in &files {
        assert_eq!(&fs::read(Path::new(&staged.folder).join(name)).unwrap(), data, "{name}");
    }
    assert_eq!(fs::read(Path::new(&staged.folder).join("stored.bin")).unwrap(), stored);
    extract::remove_tree(Path::new(&staged.folder));
    // One from the middle of the folder (MSZIP's history carried through the blocks before it), and a start.
    assert_eq!(fs::read(copy(&format!("{p}\\dir3\\file150.txt"), Purpose::Preview).unwrap()).unwrap(), files[150].1);
    assert_eq!(read_prefix(&format!("{p}\\dir6\\file286.txt"), 9).unwrap().unwrap().0, b"file 286 ");
    // A data block's checksum gives damage away.
    let mut bytes = fs::read(&path).unwrap();
    let stored_at = u32::from_le_bytes(bytes[44..48].try_into().unwrap()) as usize;
    bytes[stored_at + 8 + 100] ^= 1;
    let damaged = dir.join("damaged.cab");
    fs::write(&damaged, &bytes).unwrap();
    let error = copy(&format!("{}\\stored.bin", text(&damaged)), Purpose::Preview).unwrap_err();
    assert!(matches!(error, Failure::Damaged(_)), "{error:?}");
    // Quantum isn't read, and says so.
    let mut bytes = fs::read(&path).unwrap();
    bytes[42..44].copy_from_slice(&2u16.to_le_bytes());
    let quantum = dir.join("quantum.cab");
    fs::write(&quantum, &bytes).unwrap();
    let q = text(&quantum);
    assert!(listing(&format!("{q}\\dir0")).items.iter().all(|i| i.3 & UNREADABLE != 0));
    let error = copy(&format!("{q}\\dir0\\file000.txt"), Purpose::Preview).unwrap_err().message();
    assert!(error.contains("Quantum"), "{error}");
    assert_eq!(fs::read(copy(&format!("{q}\\stored.bin"), Purpose::Preview).unwrap()).unwrap(), stored, "the other folder reads");
}

#[test]
fn reads_what_makecab_made() {
    let dir = scratch("makecab");
    // LZX with a 2 MB window, as Windows' own cabinets are.
    let lzx = fixture("cab-lzx.cab", &dir);
    assert_eq!(names(&listing(&lzx)), ["docs", "config.ini"]);
    let readme = (1..=60).map(|i| format!("Line {i} of the readme, in an LZX cabinet made by makecab.")).collect::<Vec<_>>().join("\r\n");
    let config = (1..=40).map(|i| format!("setting{i}=value{}", i * 7)).collect::<Vec<_>>().join("\r\n");
    assert_eq!(fs::read_to_string(copy(&format!("{lzx}\\docs\\readme.txt"), Purpose::Preview).unwrap()).unwrap(), readme);
    assert_eq!(fs::read_to_string(copy(&format!("{lzx}\\config.ini"), Purpose::Preview).unwrap()).unwrap(), config);
    // A set split across two cabinets: what's whole in the first reads; what goes on into the second,
    // or comes from the first, says why it can't.
    let first = fixture("cab-split1.cab", &dir);
    let whole = (1..=2000).map(|i| format!("Whole line {i} of a file that fills one block exactly.")).collect::<Vec<_>>().join("\r\n")[..32768].to_string();
    assert_eq!(fs::read_to_string(copy(&format!("{first}\\whole.txt"), Purpose::Preview).unwrap()).unwrap(), whole);
    let second = fixture("cab-split2.cab", &dir);
    for (cabinet, name) in [(&first, "noise.bin"), (&first, "config.ini"), (&second, "noise.bin"), (&second, "config.ini")] {
        let listed = listing(cabinet);
        assert!(listed.items.iter().find(|i| i.0 == name).unwrap().3 & UNREADABLE != 0, "{cabinet} {name}");
        let error = copy(&format!("{cabinet}\\{name}"), Purpose::Preview).unwrap_err().message();
        assert!(error.contains("split across several cabinet files"), "{error}");
    }
}

/// What a test package holds: its folders (key, parent, DefaultDir), files (key, folder, FileName,
/// bytes), where its cabinet is ("#stream" in it, a file name next to it, or none), its kind and its
/// word count (compressed or not, short source names).
struct TestMsi<'a> {
    dirs: &'a [(&'a str, Option<&'a str>, &'a str)],
    files: &'a [(&'a str, &'a str, &'a str, &'a [u8])],
    cabinet: Option<&'a str>,
    kind: ::msi::PackageType,
    word_count: i32,
}

fn write_msi(path: &Path, spec: &TestMsi) {
    use ::msi::{Column, Insert, Value};
    let file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path).unwrap();
    let mut package = ::msi::Package::create(spec.kind, file).unwrap();
    package.summary_info_mut().set_word_count(spec.word_count);
    let create = |package: &mut ::msi::Package<fs::File>, name: &str, columns: Vec<Column>, rows: Vec<Vec<Value>>| {
        package.create_table(name, columns).unwrap();
        if !rows.is_empty() {
            package.insert_rows(rows.into_iter().fold(Insert::into(name), |q, r| q.row(r))).unwrap();
        }
    };
    let id = |name: &str| Column::build(name).id_string(72);
    let dirs = spec.dirs.iter().map(|(k, p, d)| vec![Value::from(*k), p.map_or(Value::Null, Value::from), Value::from(*d)]).collect();
    create(&mut package, "Directory", vec![Column::build("Directory").primary_key().id_string(72), Column::build("Directory_Parent").nullable().id_string(72), Column::build("DefaultDir").text_string(255)], dirs);
    // A component per folder used.
    let mut used: Vec<&str> = spec.files.iter().map(|f| f.1).collect();
    used.sort_unstable();
    used.dedup();
    let components = used.iter().map(|d| vec![Value::from(format!("C_{d}")), Value::from(*d)]).collect();
    create(&mut package, "Component", vec![Column::build("Component").primary_key().id_string(72), id("Directory_")], components);
    let files = spec.files.iter().enumerate().map(|(i, (k, d, n, data))| vec![Value::from(*k), Value::from(format!("C_{d}")), Value::from(*n), Value::Int(data.len() as i32), Value::Int(0), Value::Int(i as i32 + 1)]).collect();
    create(
        &mut package,
        "File",
        vec![Column::build("File").primary_key().id_string(72), id("Component_"), Column::build("FileName").text_string(255), Column::build("FileSize").int32(), Column::build("Attributes").nullable().int16(), Column::build("Sequence").int32()],
        files,
    );
    if spec.cabinet != Some(MERGE_MODULE) {
        let media = vec![vec![Value::Int(1), Value::Int(spec.files.len() as i32), spec.cabinet.map_or(Value::Null, Value::from)]];
        create(&mut package, "Media", vec![Column::build("DiskId").primary_key().int16(), Column::build("LastSequence").int32(), Column::build("Cabinet").nullable().string(255)], media);
    }
    // The cabinet holds each file by its key, as a package's cabinets do.
    let by_key: Vec<(&str, &[u8])> = spec.files.iter().map(|(k, _, _, data)| (*k, *data)).collect();
    let cab_path = path.with_extension("cab.tmp");
    write_cab(&cab_path, &[(::cab::CompressionType::MsZip, &by_key)]);
    let bytes = fs::read(&cab_path).unwrap();
    fs::remove_file(&cab_path).unwrap();
    match spec.cabinet {
        Some(MERGE_MODULE) => package.write_stream("MergeModule.CABinet").unwrap().write_all(&bytes).unwrap(),
        Some(name) if name.starts_with('#') => package.write_stream(&name[1..]).unwrap().write_all(&bytes).unwrap(),
        Some(name) if name != "missing.cab" => fs::write(path.parent().unwrap().join(name), &bytes).unwrap(),
        _ => {}
    }
    package.flush().unwrap();
}

/// A merge module's cabinet, for `TestMsi::cabinet`.
const MERGE_MODULE: &str = "(merge module)";

#[test]
fn opens_installer_packages_as_what_they_install() {
    let dir = scratch("msi");
    let dirs: &[(&str, Option<&str>, &str)] = &[
        ("TARGETDIR", None, "SourceDir"),
        ("ProgramFilesFolder", Some("TARGETDIR"), "PFiles"),
        ("APPDIR", Some("ProgramFilesFolder"), "CONTOSO|Contoso App"),
        ("BIN", Some("APPDIR"), "bin"),
        ("SAME", Some("BIN"), "."),
        ("System64Folder", Some("TARGETDIR"), "."),
        ("LOOP1", Some("LOOP2"), "a"),
        ("LOOP2", Some("LOOP1"), "b"),
    ];
    let exe = vec![0x4Du8; 50_000];
    let files: &[(&str, &str, &str, &[u8])] = &[
        ("readme.txt", "APPDIR", "README.TXT|readme.txt", b"read me"),
        ("app.exe", "BIN", "app.exe", &exe),
        ("tool.dll", "SAME", "TOOL~1.DLL|tool.dll", b"a tool"),
        ("helper.dll", "System64Folder", "helper.dll", b"a helper"),
        ("looped.txt", "LOOP1", "looped.txt", b"in a loop"),
    ];
    let spec = |cabinet| TestMsi { dirs, files, cabinet, kind: ::msi::PackageType::Installer, word_count: 2 };
    // The cabinet in the package, as a stream.
    let msi = dir.join("setup.msi");
    write_msi(&msi, &spec(Some("#app.cab")));
    let m = text(&msi);
    assert_eq!(Format::claimed("SETUP.MSI"), Some((Format::Msi, read::Claim::OnRequest)), "Enter runs the installer");
    assert_eq!(names(&listing(&m)), ["b", "Program Files (x86)", "Windows"]);
    assert_eq!(names(&listing(&format!("{m}\\Program Files (x86)\\Contoso App"))), ["bin", "readme.txt"]);
    assert_eq!(names(&listing(&format!("{m}\\Program Files (x86)\\Contoso App\\bin"))), ["app.exe", "tool.dll"]);
    for (path, data) in [("Program Files (x86)\\Contoso App\\readme.txt", &b"read me"[..]), ("Program Files (x86)\\Contoso App\\bin\\app.exe", &exe), ("Program Files (x86)\\Contoso App\\bin\\tool.dll", b"a tool"), ("Windows\\System32\\helper.dll", b"a helper"), ("b\\a\\looped.txt", b"in a loop")] {
        assert_eq!(fs::read(copy(&format!("{m}\\{path}"), Purpose::Preview).unwrap()).unwrap(), data, "{path}");
    }
    let staged = stage_all(&[format!("{m}\\Program Files (x86)")]).unwrap();
    assert!(staged.skipped.is_empty(), "{:?}", staged.skipped);
    assert_eq!(fs::read(Path::new(&staged.folder).join("Program Files (x86)").join("Contoso App").join("bin").join("app.exe")).unwrap(), exe);
    extract::remove_tree(Path::new(&staged.folder));
    // A cabinet next to it; one that isn't there.
    let external = dir.join("external");
    fs::create_dir_all(&external).unwrap();
    write_msi(&external.join("setup.msi"), &spec(Some("data1.cab")));
    let e = text(&external.join("setup.msi"));
    assert_eq!(fs::read(copy(&format!("{e}\\Windows\\System32\\helper.dll"), Purpose::Preview).unwrap()).unwrap(), b"a helper");
    let missing = dir.join("missing");
    fs::create_dir_all(&missing).unwrap();
    write_msi(&missing.join("setup.msi"), &spec(Some("missing.cab")));
    let x = text(&missing.join("setup.msi"));
    assert!(listing(&format!("{x}\\Windows\\System32")).items.iter().all(|i| i.3 & UNREADABLE != 0));
    let error = copy(&format!("{x}\\Windows\\System32\\helper.dll"), Purpose::Preview).unwrap_err().message();
    assert!(error.contains("missing.cab") && error.contains("isn't next to the package"), "{error}");
    // Uncompressed: the files are next to it, in its source tree (long names, as the word count says).
    let loose = dir.join("loose");
    fs::create_dir_all(loose.join("PFiles").join("Contoso App").join("bin")).unwrap();
    write_msi(&loose.join("setup.msi"), &TestMsi { word_count: 0, ..spec(None) });
    fs::write(loose.join("PFiles").join("Contoso App").join("readme.txt"), b"read me").unwrap();
    fs::write(loose.join("PFiles").join("Contoso App").join("bin").join("app.exe"), b"the wrong size").unwrap();
    let l = text(&loose.join("setup.msi"));
    assert_eq!(fs::read(copy(&format!("{l}\\Program Files (x86)\\Contoso App\\readme.txt"), Purpose::Preview).unwrap()).unwrap(), b"read me");
    let error = copy(&format!("{l}\\Program Files (x86)\\Contoso App\\bin\\app.exe"), Purpose::Preview).unwrap_err().message();
    assert!(error.contains("isn't the size"), "{error}");
    let error = copy(&format!("{l}\\Program Files (x86)\\Contoso App\\bin\\tool.dll"), Purpose::Preview).unwrap_err().message();
    assert!(error.contains("PFiles\\Contoso App\\bin\\tool.dll") && error.contains("isn't there"), "{error}");
    // A merge module keeps its files in one cabinet stream, with no Media table.
    let msm = dir.join("module.msm");
    write_msi(&msm, &spec(Some(MERGE_MODULE)));
    assert_eq!(fs::read(copy(&format!("{}\\Program Files (x86)\\Contoso App\\readme.txt", text(&msm)), Purpose::Preview).unwrap()).unwrap(), b"read me");
    // Found by its bytes under another name; a patch isn't a folder of files.
    let renamed = dir.join("installer.bin");
    fs::copy(&msi, &renamed).unwrap();
    let r = format!("{}\\Windows", text(&renamed));
    assert!(sniff(&r));
    assert_eq!(names(&listing(&r)), ["System32"]);
    let patch = dir.join("fix.msi");
    write_msi(&patch, &TestMsi { kind: ::msi::PackageType::Patch, ..spec(Some("#app.cab")) });
    let error = list(&text(&patch), Instant::now()).unwrap().unwrap_err();
    assert!(error.contains("patch"), "{error}");
    // Nor is a patch, or another compound file, found by its bytes.
    let other = dir.join("fix.bin");
    fs::copy(&patch, &other).unwrap();
    assert!(!sniff(&format!("{}\\x", text(&other))));
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
    let tar = write_tar(&[("docs/a.txt", &[b'a'; 3000]), ("b.bin", &[7; 1000])]);
    for codec in [Codec::Plain, Codec::Gz, Codec::Xz, Codec::Zst, Codec::Bz2, Codec::Lz4] {
        archives.push(compressed(&tar, codec));
    }
    // 7z blocks Glint decodes itself (Zstandard, after BCJ), and LZ4 ones.
    for (n, methods) in [vec![sevenz_rust2::EncoderMethod::ZSTD.into(), sevenz_rust2::EncoderMethod::BCJ_X86_FILTER.into()], vec![sevenz_rust2::EncoderMethod::LZ4.into()]].into_iter().enumerate() {
        let path = dir.join(format!("codec-{n}.7z"));
        write_7z(&path, methods, &[("docs/a.txt", &[b'a'; 3000]), ("b.bin", &[7; 1000])], true);
        archives.push(fs::read(&path).unwrap());
    }
    // Cabinets: MSZIP and stored folders, and LZX.
    let path = dir.join("codec.cab");
    write_cab(&path, &[(::cab::CompressionType::MsZip, &[("docs\\a.txt", &[b'a'; 40_000])]), (::cab::CompressionType::None, &[("b.bin", &[7; 1000])])]);
    archives.push(fs::read(&path).unwrap());
    archives.push(fs::read(fixture("cab-lzx.cab", &dir)).unwrap());
    // An installer package with its cabinet inside.
    let path = dir.join("codec.msi");
    let dirs: &[(&str, Option<&str>, &str)] = &[("TARGETDIR", None, "SourceDir"), ("ProgramFilesFolder", Some("TARGETDIR"), "PFiles"), ("APPDIR", Some("ProgramFilesFolder"), "App")];
    write_msi(&path, &TestMsi { dirs, files: &[("a.txt", "APPDIR", "a.txt", &[b'a'; 3000])], cabinet: Some("#a.cab"), kind: ::msi::PackageType::Installer, word_count: 2 });
    archives.push(fs::read(&path).unwrap());
    for (n, original) in archives.iter().enumerate() {
        // By its first bytes, as Glint itself tells them apart.
        let ext = match Format::detect(original) {
            Some(Format::SevenZ) => "7z".to_string(),
            Some(Format::Cab) => "cab".to_string(),
            Some(Format::Tar(_)) => "tar".to_string(),
            Some(Format::Single(codec)) => format!("tar.{}", codec.ext()),
            _ if original.starts_with(b"\xD0\xCF\x11\xE0") => "msi".to_string(),
            _ => "zip".to_string(),
        };
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
        let (_, items, _) = Source::open(&zip, Format::Zip, &stream::Scan::new(0)).unwrap();
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
