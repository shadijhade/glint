//! Windows Installer packages (.msi) and merge modules (.msm), shown as the folders they install:
//! the Directory, Component and File tables give each file's installed place, name and size, and
//! the Media table says which cabinet holds it (a stream in the package, or a .cab next to it), or
//! that it's a loose file next to the package. Bytes come out of those cabinets through Glint's own
//! cabinet reader, a folder at a time.

use super::cab::{self, Cabinet, FolderReader};
use super::codec::Fused;
use super::read::ItemIndex;
use super::stream;
use super::tree::{Item, UNREADABLE};
use super::Failure;
use msi::{Package, PackageType, Row, Select};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// A merge module's files are all in this stream, as a cabinet.
const MERGE_MODULE_CABINET: &str = "MergeModule.CABinet";

/// Windows Installer's standard folders, as the paths they are on 64-bit Windows; the top of the
/// listing stands for the system drive (the user's own folders go by their names).
const STANDARD_FOLDERS: [(&str, &str); 25] = [
    ("WindowsVolume", ""),
    ("ProgramFiles64Folder", "Program Files"),
    ("ProgramFilesFolder", "Program Files (x86)"),
    ("CommonFiles64Folder", "Program Files\\Common Files"),
    ("CommonFilesFolder", "Program Files (x86)\\Common Files"),
    ("WindowsFolder", "Windows"),
    ("System64Folder", "Windows\\System32"),
    ("SystemFolder", "Windows\\SysWOW64"),
    ("System16Folder", "Windows\\System"),
    ("FontsFolder", "Windows\\Fonts"),
    ("CommonAppDataFolder", "ProgramData"),
    ("AppDataFolder", "AppData\\Roaming"),
    ("LocalAppDataFolder", "AppData\\Local"),
    ("DesktopFolder", "Desktop"),
    ("PersonalFolder", "Documents"),
    ("MyPicturesFolder", "Pictures"),
    ("FavoritesFolder", "Favorites"),
    ("StartMenuFolder", "Start Menu"),
    ("ProgramMenuFolder", "Start Menu\\Programs"),
    ("StartupFolder", "Start Menu\\Programs\\Startup"),
    ("AdminToolsFolder", "Start Menu\\Programs\\Administrative Tools"),
    ("SendToFolder", "SendTo"),
    ("TemplateFolder", "Templates"),
    ("TempFolder", "Temp"),
    ("RecentFolder", "Recent"),
];

/// Summary information's word count: short names in the source tree, files compressed by default.
const SHORT_SOURCE_NAMES: i32 = 0x1;
const COMPRESSED: i32 = 0x2;
/// File attributes: read-only, hidden, system (kept), and compressed or not (against the default).
const FILE_NONCOMPRESSED: i32 = 0x2000;
const FILE_COMPRESSED: i32 = 0x4000;

/// Where a file's bytes are.
enum Place {
    /// In cabinet `cabinet` of the package's, as its file `file`.
    Cab { cabinet: usize, file: usize },
    /// A file next to the package (an uncompressed installation source).
    Loose(PathBuf),
    /// Nowhere Glint can get at, and why.
    Missing(Failure),
}

/// Where a cabinet is: a stream in the package, or a file next to it.
enum Origin {
    Stream(String),
    File(PathBuf),
}

struct Cab {
    origin: Origin,
    cabinet: Cabinet,
}

/// A cabinet the package names: where it is in `MsiSource::cabinets` and its files by name, or why
/// it can't be read.
type CabinetFound = Result<(usize, HashMap<String, usize>), Failure>;

/// Entries to read from one cabinet folder: their offsets in it, sizes and indexes.
type Wanted = Vec<(u64, u64, ItemIndex)>;

pub struct MsiSource {
    places: Vec<Place>,
    cabinets: Vec<Cab>,
}

/// Whether the file at `path` is a Windows Installer package or merge module (not a patch, a
/// transform, or another compound file such as an old .doc).
pub fn is_package(path: &Path) -> bool {
    msi::open(path).is_ok_and(|p| p.package_type() == PackageType::Installer)
}

fn text<'a>(row: &'a Row, column: &str) -> Option<&'a str> {
    row.has_column(column).then(|| row[column].as_str()).flatten()
}

fn int(row: &Row, column: &str) -> Option<i32> {
    row.has_column(column).then(|| row[column].as_int()).flatten()
}

/// The rows of `table` (none when the package has no such table).
fn rows(package: &mut Package<File>, table: &str) -> Result<Vec<Row>, Failure> {
    if !package.has_table(table) {
        return Ok(Vec::new());
    }
    Ok(package.select_rows(Select::table(table)).map_err(failure)?.collect())
}

fn failure(e: io::Error) -> Failure {
    match e.kind() {
        io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => Failure::Damaged(format!("its tables can't be read ({e})")),
        _ => Failure::from(e),
    }
}

/// A name in its "short|long" form: the long one, or the short one for a source tree that says so.
/// "." (the parent folder itself) and empty names are None.
fn pick(name: &str, short: bool) -> Option<String> {
    let name = match name.split_once('|') {
        Some((s, _)) if short => s,
        Some((_, long)) => long,
        None => name,
    };
    (!name.is_empty() && name != ".").then(|| name.to_string())
}

/// A folder of the Directory table: its parent, and its own name where it's installed and in the
/// source tree ("target[:source]" in DefaultDir).
struct Dir {
    parent: Option<String>,
    target: Option<String>,
    source: Option<String>,
}

/// Walks from folder `key` up to the top, naming each step with `name`, and stops at a standard
/// folder (when `standard`), a folder with no parent, or a loop.
fn path_of(key: &str, dirs: &HashMap<String, Dir>, standard: bool, name: impl Fn(&Dir) -> Option<&String>) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let mut seen = HashSet::new();
    let mut at = Some(key);
    while let Some(k) = at {
        if standard {
            if let Some(&(_, path)) = STANDARD_FOLDERS.iter().find(|(property, _)| *property == k) {
                if !path.is_empty() {
                    parts.push(path);
                }
                break;
            }
        }
        // A loop in the parents: what was walked hangs from the top.
        if !seen.insert(k) {
            break;
        }
        let Some(dir) = dirs.get(k) else { break };
        // The top (TARGETDIR, named "SourceDir"): its name isn't a folder.
        let Some(parent) = dir.parent.as_deref().filter(|&p| p != k) else { break };
        if let Some(own) = name(dir) {
            parts.push(own);
        }
        at = Some(parent);
    }
    parts.reverse();
    parts.join("\\")
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}\\{name}")
    }
}

/// Reads the package's tables and its cabinets' lists of files (nothing is decompressed).
pub fn open(path: &Path, max_entries: usize) -> Result<(MsiSource, Vec<Item>), Failure> {
    let mut package = msi::open(path).map_err(|e| match e.kind() {
        io::ErrorKind::InvalidData => Failure::Unreadable("it isn't a Windows Installer package".into()),
        _ => Failure::from(e),
    })?;
    match package.package_type() {
        PackageType::Installer => {}
        PackageType::Patch => return Err(Failure::Unreadable("it's a Windows Installer patch, which holds changes to a package rather than its files".into())),
        PackageType::Transform => return Err(Failure::Unreadable("it's a Windows Installer transform, which holds changes to a package rather than its files".into())),
    }
    let word_count = package.summary_info().word_count().unwrap_or(0);
    let modified = package.summary_info().creation_time().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
    let short_source = word_count & SHORT_SOURCE_NAMES != 0;
    let dirs: HashMap<String, Dir> = rows(&mut package, "Directory")?
        .iter()
        .filter_map(|row| {
            let key = text(row, "Directory")?.to_string();
            let default = text(row, "DefaultDir").unwrap_or(".");
            let (target, source) = default.split_once(':').unwrap_or((default, default));
            Some((key, Dir { parent: text(row, "Directory_Parent").map(str::to_string), target: pick(target, false), source: pick(source, short_source) }))
        })
        .collect();
    let components: HashMap<String, String> = rows(&mut package, "Component")?.iter().filter_map(|row| Some((text(row, "Component")?.to_string(), text(row, "Directory_")?.to_string()))).collect();
    let files = rows(&mut package, "File")?;
    if files.len() > max_entries {
        return Err(Failure::TooManyEntries(files.len()));
    }
    // Media rows by last sequence number: a file is on the first whose last is at or after its own.
    let mut media: Vec<(i32, Option<String>)> = rows(&mut package, "Media")?.iter().map(|row| (int(row, "LastSequence").unwrap_or(0), text(row, "Cabinet").filter(|c| !c.is_empty()).map(str::to_string))).collect();
    media.sort_by_key(|m| m.0);
    if media.is_empty() && package.has_stream(MERGE_MODULE_CABINET) {
        media.push((i32::MAX, Some(format!("#{MERGE_MODULE_CABINET}"))));
    }
    // Each cabinet named, read once: its list of files, by name (the File table's key).
    let folder = path.parent().unwrap_or(Path::new(""));
    let mut cabinets: Vec<Cab> = Vec::new();
    let mut by_name: HashMap<String, CabinetFound> = HashMap::new();
    for name in media.iter().filter_map(|m| m.1.clone()) {
        if by_name.contains_key(&name) {
            continue;
        }
        let read = match name.strip_prefix('#') {
            Some(stream) => package.read_stream(stream).map_err(|e| Failure::Damaged(format!("its cabinet “{stream}” can't be read ({e})"))).and_then(|mut s| Ok((Origin::Stream(stream.to_string()), cab::read(&mut s, max_entries)?))),
            None => {
                let at = folder.join(&name);
                match File::open(&at) {
                    Ok(f) => cab::read(&mut BufReader::new(f), max_entries).map(|c| (Origin::File(at), c)),
                    Err(_) => Err(Failure::Unreadable(format!("it's in the cabinet “{name}”, which isn't next to the package"))),
                }
            }
        };
        let entry = read.map(|(origin, cabinet)| {
            let names = cabinet.files.iter().enumerate().map(|(i, f)| (f.item.path.clone(), i)).collect();
            cabinets.push(Cab { origin, cabinet });
            (cabinets.len() - 1, names)
        });
        by_name.insert(name, entry);
    }
    let mut places = Vec::with_capacity(files.len());
    let mut items = Vec::with_capacity(files.len());
    for row in &files {
        let key = text(row, "File").unwrap_or_default();
        let file_name = text(row, "FileName").unwrap_or(key);
        let long = pick(file_name, false).unwrap_or_else(|| key.to_string());
        let dir = text(row, "Component_").and_then(|c| components.get(c)).map(String::as_str).unwrap_or("TARGETDIR");
        let attributes = int(row, "Attributes").unwrap_or(0);
        let sequence = int(row, "Sequence").unwrap_or(0);
        let size = int(row, "FileSize").map_or(0, |s| s as u32 as u64);
        let compressed = attributes & FILE_COMPRESSED != 0 || (word_count & COMPRESSED != 0 && attributes & FILE_NONCOMPRESSED == 0);
        let cabinet = media.iter().find(|m| m.0 >= sequence).and_then(|m| m.1.as_ref());
        let place = match cabinet.map(|name| (name, &by_name[name])) {
            Some((_, Ok((cabinet, names)))) if compressed || names.contains_key(key) => match names.get(key).or_else(|| names.iter().find(|(n, _)| n.eq_ignore_ascii_case(key)).map(|(_, i)| i)) {
                Some(&file) => Place::Cab { cabinet: *cabinet, file },
                None => Place::Missing(Failure::Damaged("it isn't in the cabinet the package says holds it".into())),
            },
            Some((_, Err(why))) if compressed => Place::Missing(why.clone()),
            _ => {
                // An uncompressed source: next to the package, where its source tree puts it.
                let source_dir = path_of(dir, &dirs, false, |d| d.source.as_ref());
                let source_name = pick(file_name, short_source).unwrap_or_else(|| long.clone());
                let at = folder.join(join(&source_dir, &source_name));
                let shown = join(&source_dir, &source_name);
                match at.metadata() {
                    Ok(meta) if meta.is_file() && meta.len() == size => Place::Loose(at),
                    Ok(meta) if meta.is_file() => Place::Missing(Failure::Unreadable(format!("the file it's installed from, next to the package (“{shown}”), isn't the size the package says"))),
                    _ => Place::Missing(Failure::Unreadable(format!("it's installed from a file next to the package (“{shown}”), which isn't there"))),
                }
            }
        };
        let mut attrs = (attributes & 0x7) as u32;
        let problem = match &place {
            Place::Missing(why) => Some(why.clone()),
            Place::Cab { cabinet, file } => cabinets[*cabinet].cabinet.files[*file].problem.clone(),
            Place::Loose(_) => None,
        };
        if problem.is_some() {
            attrs |= UNREADABLE;
        }
        items.push(Item { path: join(&path_of(dir, &dirs, true, |d| d.target.as_ref()), &long), dir: false, size, modified, attrs });
        places.push(place);
    }
    Ok((MsiSource { places, cabinets }, items))
}

impl MsiSource {
    fn in_cabinet(&self, item: ItemIndex) -> Option<(&Cab, &cab::CabFile)> {
        match self.places.get(item)? {
            Place::Cab { cabinet, file } => {
                let cab = &self.cabinets[*cabinet];
                Some((cab, &cab.cabinet.files[*file]))
            }
            _ => None,
        }
    }

    pub fn cost(&self, item: ItemIndex, size: u64) -> u64 {
        self.in_cabinet(item).map_or(size, |(_, f)| f.offset.saturating_add(size))
    }

    /// A cabinet's folder is a solid block; a loose file is in none.
    pub fn block_of(&self, item: ItemIndex) -> Option<usize> {
        match self.places.get(item)? {
            Place::Cab { cabinet, file } => Some((cabinet << 16) | self.cabinets[*cabinet].cabinet.files[*file].folder),
            _ => None,
        }
    }

    pub fn ratio(&self, item: ItemIndex) -> u64 {
        self.in_cabinet(item).and_then(|(c, f)| c.cabinet.folders.get(f.folder)).map_or(1, |f| f.unpacked / f.packed.max(1))
    }

    pub fn problem(&self, item: ItemIndex) -> Option<Failure> {
        match self.places.get(item)? {
            Place::Missing(why) => Some(why.clone()),
            Place::Cab { .. } => self.in_cabinet(item).and_then(|(_, f)| f.problem.clone()),
            Place::Loose(_) => None,
        }
    }

    /// Loose files as they are; the rest a cabinet folder at a time, each read once from its start.
    pub fn read_each(&self, package: &Path, items: &[ItemIndex], each: &mut dyn FnMut(ItemIndex, &mut dyn Read) -> bool, skipped: &mut dyn FnMut(u64) -> bool) -> Vec<(ItemIndex, Failure)> {
        let mut failed = Vec::new();
        let mut by_folder: BTreeMap<(usize, usize), Wanted> = BTreeMap::new();
        let mut stopped = false;
        for &item in items {
            if stopped {
                failed.push((item, Failure::Cancelled));
                continue;
            }
            match (self.places.get(item), self.problem(item)) {
                (_, Some(why)) => failed.push((item, why)),
                (Some(Place::Loose(at)), None) => match File::open(at) {
                    Ok(f) => stopped = !each(item, &mut BufReader::new(f)),
                    Err(e) => failed.push((item, Failure::from(e))),
                },
                (Some(&Place::Cab { cabinet, file }), None) => {
                    let f = &self.cabinets[cabinet].cabinet.files[file];
                    if f.item.size == 0 {
                        stopped = !each(item, &mut io::empty());
                    } else {
                        by_folder.entry((cabinet, f.folder)).or_default().push((f.offset, f.item.size, item));
                    }
                }
                _ => failed.push((item, Failure::NotFound)),
            }
        }
        for ((cabinet, folder), mut wanted) in by_folder {
            wanted.sort_unstable();
            while !wanted.is_empty() && !stopped {
                let cab = &self.cabinets[cabinet];
                let at = cab.cabinet.data_at(folder);
                let served = match &cab.origin {
                    Origin::File(path) => File::open(path).and_then(|mut f| f.seek(SeekFrom::Start(at)).map(|_| f)).map(|f| {
                        let mut reader = Fused::new(FolderReader::new(BufReader::with_capacity(256 << 10, f), &cab.cabinet, folder));
                        stream::serve_in_order(&mut reader, &wanted, false, each, skipped, &mut failed)
                    }),
                    Origin::Stream(name) => msi::open(package).and_then(|mut p| {
                        let mut s = p.read_stream(name)?;
                        s.seek(SeekFrom::Start(at))?;
                        let mut reader = Fused::new(FolderReader::new(BufReader::with_capacity(256 << 10, s), &cab.cabinet, folder));
                        Ok(stream::serve_in_order(&mut reader, &wanted, false, each, skipped, &mut failed))
                    }),
                };
                match served {
                    Ok((go, again)) => {
                        stopped = !go;
                        wanted = again;
                    }
                    Err(e) => failed.extend(wanted.drain(..).map(|(_, _, i)| (i, Failure::from_io(&e)))),
                }
            }
            if stopped {
                failed.extend(wanted.into_iter().map(|(_, _, i)| (i, Failure::Cancelled)));
            }
        }
        failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_default_dir_names() {
        assert_eq!(pick("PROGRA~1|Contoso App", false).as_deref(), Some("Contoso App"));
        assert_eq!(pick("PROGRA~1|Contoso App", true).as_deref(), Some("PROGRA~1"));
        assert_eq!(pick("bin", false).as_deref(), Some("bin"));
        assert_eq!(pick(".", false), None);
        assert_eq!(pick("", false), None);
    }

    #[test]
    fn walks_folders_up_to_the_top_or_a_standard_one() {
        let dir = |parent: Option<&str>, target: Option<&str>| Dir { parent: parent.map(str::to_string), target: target.map(str::to_string), source: target.map(str::to_string) };
        let dirs: HashMap<String, Dir> = [
            ("TARGETDIR", dir(None, Some("SourceDir"))),
            ("ProgramFilesFolder", dir(Some("TARGETDIR"), Some("PFiles"))),
            ("APPDIR", dir(Some("ProgramFilesFolder"), Some("Contoso"))),
            ("BIN", dir(Some("APPDIR"), Some("bin"))),
            ("SAME", dir(Some("BIN"), None)),
            ("LOOP1", dir(Some("LOOP2"), Some("a"))),
            ("LOOP2", dir(Some("LOOP1"), Some("b"))),
            ("ORPHAN", dir(Some("NOWHERE"), Some("orphan"))),
        ]
        .into_iter()
        .map(|(k, d)| (k.to_string(), d))
        .collect();
        let target = |k: &str| path_of(k, &dirs, true, |d| d.target.as_ref());
        assert_eq!(target("BIN"), "Program Files (x86)\\Contoso\\bin");
        assert_eq!(target("SAME"), "Program Files (x86)\\Contoso\\bin", "\".\" is its parent");
        assert_eq!(target("TARGETDIR"), "");
        assert_eq!(path_of("BIN", &dirs, false, |d| d.source.as_ref()), "PFiles\\Contoso\\bin", "the source tree keeps the package's own names");
        assert_eq!(target("LOOP1"), "b\\a", "a loop stops where it comes back");
        assert_eq!(target("ORPHAN"), "orphan");
    }
}
