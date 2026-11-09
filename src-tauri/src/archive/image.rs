//! Disk images and installers, through 7-Zip (a tool Glint installs; 7z.exe with the 7z.dll that reads
//! every format it knows): ISO and IMG discs, VHD, VHDX, VMDK, QCOW2 and VDI disks, DMG, WIM and ESD,
//! ext, APFS, HFS and squashfs file systems, NSIS installers and programs' resources, Debian and RPM
//! packages, CPIO and CHM. Its technical listing (`7z l -slt`) gives the entries; one entry streams out of
//! `7z e -so`, several are extracted together (`7z x` with a list) into a folder of the run's own and read
//! from there. 7-Zip runs without a console window, ended with Glint.

use super::read::ItemIndex;
use super::tree::{Item, ENCRYPTED};
use super::Failure;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Extensions 7-Zip opens that Enter browses (Windows can't mount these).
pub const BROWSED: &[&str] = &["dmg", "wim", "esd", "swm", "vmdk", "qcow2", "qcow", "vdi", "squashfs", "sfs", "deb", "rpm", "cpio", "chm", "xar", "apfs", "hfs", "hfsx", "ext2", "ext3", "ext4", "lzh", "lha", "arj", "cramfs"];
/// Extensions 7-Zip opens that Enter does something else with (Windows mounts discs and disks, runs
/// programs): "Open as archive" looks inside.
pub const ON_REQUEST: &[&str] = &["iso", "img", "vhd", "vhdx", "exe"];

pub struct ImageSource {
    path: PathBuf,
    /// Each entry's path as 7-Zip lists it (what extracting it asks for).
    names: Vec<String>,
}

fn seven_zip() -> Result<Command, Failure> {
    crate::tools::command("7zip").map_err(|_| Failure::Unreadable("7-Zip isn't installed: Settings → Tools installs it with one click, then this opens".into()))
}

/// 7-Zip's local date, "2024-01-02 03:04:05[.fraction]", as Unix ms (taken as UTC: close enough for a listing).
fn parse_time(text: &str) -> i64 {
    let t = text.trim();
    if t.len() < 19 {
        return 0;
    }
    let num = |a: usize, b: usize| t.get(a..b).and_then(|s| s.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) = (num(0, 4), num(5, 7), num(8, 10), num(11, 13), num(14, 16), num(17, 19)) else { return 0 };
    // Days from the civil date (Howard Hinnant's algorithm).
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    ((days * 86_400) + h * 3600 + mi * 60 + s) * 1000
}

/// 7-Zip's technical listing: the entries, and each one's name as 7-Zip calls it.
fn parse_listing(text: &str, max_entries: usize) -> Result<(Vec<Item>, Vec<String>), Failure> {
    let mut items = Vec::new();
    let mut names = Vec::new();
    for block in text.split("\n\n").map(str::trim).filter(|b| !b.is_empty()) {
        let mut item = Item::default();
        let mut name = None;
        for line in block.lines() {
            let Some((key, value)) = line.split_once(" = ").or_else(|| line.strip_suffix(" =").map(|k| (k, ""))) else { continue };
            match key.trim() {
                "Path" => name = Some(value.to_string()),
                "Folder" => item.dir = value.trim() == "+",
                "Size" => item.size = value.trim().parse().unwrap_or(0),
                "Modified" => item.modified = parse_time(value),
                "Encrypted" if value.trim() == "+" => item.attrs |= ENCRYPTED,
                "Attributes" => {
                    // "D" for a folder in 7-Zip's Windows-style attributes ("DA", "RHS").
                    let flags = value.split_whitespace().next().unwrap_or_default();
                    if !flags.starts_with('-') && flags.contains('D') {
                        item.dir = true;
                    }
                    if !flags.starts_with('-') {
                        item.attrs |= u32::from(flags.contains('R')) | (u32::from(flags.contains('H')) << 1) | (u32::from(flags.contains('S')) << 2);
                    }
                }
                _ => {}
            }
        }
        // The block about the archive itself has a Type; entries don't.
        if block.lines().any(|l| l.starts_with("Type = ")) && !block.lines().any(|l| l.starts_with("Folder = ")) {
            continue;
        }
        let Some(name) = name.filter(|n| !n.is_empty()) else { continue };
        item.path = name.replace('\\', "/");
        items.push(item);
        names.push(name);
        if items.len() > max_entries {
            return Err(Failure::TooManyEntries(max_entries));
        }
    }
    Ok((items, names))
}

pub fn open(path: &Path, max_entries: usize) -> Result<(ImageSource, Vec<Item>), Failure> {
    let mut command = seven_zip()?;
    command.args(["l", "-slt", "-ba", "-sccUTF-8", "-p-", "--"]).arg(path).stdin(Stdio::null());
    let done = command.output().map_err(|e| Failure::Io(e.to_string()))?;
    let text = String::from_utf8_lossy(&done.stdout).replace("\r\n", "\n");
    if !done.status.success() && text.trim().is_empty() {
        let said = String::from_utf8_lossy(&done.stderr);
        let why = said.lines().map(str::trim).rfind(|l| !l.is_empty() && !l.starts_with("7-Zip")).unwrap_or("7-Zip can't open it").to_string();
        return Err(if why.contains("Can not open the file as archive") || why.contains("Cannot open the file as archive") {
            Failure::Unreadable("7-Zip doesn't read what's in it".into())
        } else {
            Failure::Damaged(why)
        });
    }
    let (items, names) = parse_listing(&text, max_entries)?;
    Ok((ImageSource { path: path.to_path_buf(), names }, items))
}

impl ImageSource {
    pub fn cost(&self, _item: ItemIndex, size: u64) -> u64 {
        size
    }

    pub fn problem(&self, item: ItemIndex) -> Option<Failure> {
        (item >= self.names.len()).then_some(Failure::NotFound)
    }

    /// One entry streams out of 7-Zip; several are extracted together into a folder of their own, then read.
    pub fn read_each(&self, items: &[ItemIndex], each: &mut dyn FnMut(ItemIndex, &mut dyn Read) -> bool, skipped: &mut dyn FnMut(u64) -> bool) -> Vec<(ItemIndex, Failure)> {
        let _ = skipped;
        let mut failed = Vec::new();
        let wanted: Vec<ItemIndex> = items.iter().copied().filter(|&i| i < self.names.len()).collect();
        failed.extend(items.iter().copied().filter(|&i| i >= self.names.len()).map(|i| (i, Failure::NotFound)));
        if wanted.len() <= 4 {
            for item in wanted {
                match self.stream(item, each) {
                    Ok(true) => {}
                    Ok(false) => failed.push((item, Failure::Cancelled)),
                    Err(e) => failed.push((item, e)),
                }
            }
            return failed;
        }
        let out = std::env::temp_dir().join("Glint").join(format!("7z-{}-{}", std::process::id(), unique()));
        let list = out.with_extension("txt");
        let result = (|| -> Result<(), Failure> {
            std::fs::create_dir_all(&out).map_err(|e| Failure::Io(e.to_string()))?;
            let names: Vec<&str> = wanted.iter().map(|&i| self.names[i].as_str()).collect();
            std::fs::write(&list, names.join("\n")).map_err(|e| Failure::Io(e.to_string()))?;
            let mut command = seven_zip()?;
            command.args(["x", "-y", "-spd", "-scsUTF-8", "-sccUTF-8", "-p-"]).arg(format!("-o{}", out.display())).arg(format!("-i@{}", list.display())).arg("--").arg(&self.path);
            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            command.status().map_err(|e| Failure::Io(e.to_string()))?;
            Ok(())
        })();
        if let Err(e) = result {
            failed.extend(wanted.iter().map(|&i| (i, e.clone())));
        } else {
            for (n, &item) in wanted.iter().enumerate() {
                let relative = self.names[item].replace('/', "\\");
                let file = out.join(&relative);
                // Kept inside the folder it was extracted to, whatever the name says.
                if relative.split('\\').any(|part| part == "..") || !file.starts_with(&out) {
                    failed.push((item, Failure::Unreadable("its name leads outside the archive".into())));
                    continue;
                }
                match std::fs::File::open(&file) {
                    Ok(mut f) => {
                        if !each(item, &mut f) {
                            failed.extend(wanted[n..].iter().map(|&i| (i, Failure::Cancelled)));
                            break;
                        }
                    }
                    Err(_) => failed.push((item, Failure::Unreadable("7-Zip couldn't extract it".into()))),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::remove_file(&list);
        failed
    }

    /// `item` straight out of 7-Zip's standard output.
    fn stream(&self, item: ItemIndex, each: &mut dyn FnMut(ItemIndex, &mut dyn Read) -> bool) -> Result<bool, Failure> {
        let mut command = seven_zip()?;
        command.args(["e", "-so", "-spd", "-sccUTF-8", "-p-", "--"]).arg(&self.path).arg(&self.names[item]);
        command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = command.spawn().map_err(|e| Failure::Io(e.to_string()))?;
        #[cfg(windows)]
        let job = crate::proc::Job::ending().ok();
        #[cfg(windows)]
        if let Some(job) = &job {
            let _ = job.assign(&child);
        }
        let mut stdout = child.stdout.take().ok_or_else(|| Failure::Io("no output".into()))?;
        let went_on = each(item, &mut stdout);
        drop(stdout);
        if !went_on {
            let _ = child.kill();
        }
        let status = child.wait().map_err(|e| Failure::Io(e.to_string()))?;
        if went_on && !status.success() {
            return Err(Failure::Unreadable("7-Zip couldn't extract it".into()));
        }
        Ok(went_on)
    }
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_seven_zips_listing() {
        let text = "Path = disc.iso\nType = Iso\nPhysical Size = 1000\n\nPath = EFI\nFolder = +\nSize = 0\nModified = 2024-06-01 12:00:00\nAttributes = D\n\nPath = EFI\\boot\\bootx64.efi\nFolder = -\nSize = 1234\nModified = 2024-06-01 12:30:15.1234567\nAttributes = RA\nEncrypted = -\n\nPath = secret.txt\nFolder = -\nSize = 5\nEncrypted = +\n";
        let (items, names) = parse_listing(text, 100).unwrap();
        assert_eq!(names, ["EFI", "EFI\\boot\\bootx64.efi", "secret.txt"]);
        assert!(items[0].dir && !items[1].dir);
        assert_eq!(items[1].path, "EFI/boot/bootx64.efi");
        assert_eq!(items[1].size, 1234);
        assert_eq!(items[1].attrs & 1, 1, "read-only kept");
        assert_eq!(items[2].attrs & ENCRYPTED, ENCRYPTED);
        // 2024-06-01 12:30:15 UTC.
        assert_eq!(items[1].modified, 1_717_245_015_000);
        assert!(matches!(parse_listing(text, 1), Err(Failure::TooManyEntries(1))));
    }
}
