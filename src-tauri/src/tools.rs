//! Optional tools: programs Glint drives for a feature or two but doesn't ship, so the download stays
//! small for everyone who never uses those features. Each installs when asked: with winget when it has
//! the package, else from the tool's own release on GitHub, pinned by its SHA-256 and checked before
//! anything is unpacked. Their arguments are built in Rust from typed options, like rg's, fd's and dust's.

use crate::fsx::blocking;
use crate::proc::{self, Lanes, Origin};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::State;

/// A program Glint can use, and where it comes from.
pub struct Tool {
    pub id: &'static str,
    pub name: &'static str,
    /// Its file name, without `.exe`.
    pub exe: &'static str,
    /// Part of its winget package folder's name (`Packages\<id>_<source>`, lowercase), to find a copy
    /// winget installed before this session's PATH knew about it.
    pub package: &'static str,
    pub source: Source,
    /// What it does in Glint, as Settings says it.
    pub used_for: &'static str,
}

// Until a tool that's only on GitHub is listed, nothing downloads a release.
#[allow(dead_code)]
pub enum Source {
    /// Comes with Glint, next to the app; winget puts a copy back when a security tool removed it.
    Bundled { winget: &'static str },
    Winget { id: &'static str },
    /// A release on GitHub: the version it holds, its address, its SHA-256 and the program's path in the
    /// zip ("" when the download is the program itself).
    Release { version: &'static str, url: &'static str, sha256: &'static str, member: &'static str },
}

pub static TOOLS: &[Tool] = &[
    Tool { id: "ripgrep", name: "ripgrep", exe: "rg", package: "ripgrep", source: Source::Bundled { winget: "BurntSushi.ripgrep.MSVC" }, used_for: "Searching inside files" },
    Tool { id: "fd", name: "fd", exe: "fd", package: "sharkdp.fd", source: Source::Bundled { winget: "sharkdp.fd" }, used_for: "Finding files by name where instant search doesn't" },
    Tool { id: "dust", name: "dust", exe: "dust", package: "bootandy.dust", source: Source::Bundled { winget: "bootandy.dust" }, used_for: "Folder sizes and disk-usage maps where instant search doesn't" },
    Tool {
        id: "czkawka",
        name: "Czkawka",
        exe: "windows_czkawka_cli",
        package: "qarmin.czkawka.cli",
        source: Source::Winget { id: "qarmin.czkawka.cli" },
        used_for: "Look-alike pictures, videos and music, broken files and wrong extensions in Clean up",
    },
    Tool {
        id: "difftastic",
        name: "Difftastic",
        exe: "difft",
        package: "wilfred.difftastic",
        source: Source::Winget { id: "Wilfred.difftastic" },
        used_for: "Comparing code by its structure (Compare two files): what changed in it, not its layout",
    },
    // Any FFmpeg found is used; a new install is the smaller build, which reads every common video.
    Tool { id: "ffmpeg", name: "FFmpeg", exe: "ffmpeg", package: "gyan.ffmpeg", source: Source::Winget { id: "Gyan.FFmpeg.Essentials" }, used_for: "Reading videos for Clean up's look-alike videos" },
    Tool {
        id: "magika",
        name: "Magika",
        exe: "magika",
        package: "google.magika",
        source: Source::Winget { id: "Google.Magika" },
        used_for: "A file's real type in the File X-Ray, told by its content (a program named photo.jpg, say)",
    },
    Tool {
        id: "yara-x",
        name: "YARA-X",
        exe: "yr",
        package: "virustotal.yara-x",
        source: Source::Winget { id: "VirusTotal.YARA-X" },
        used_for: "Scanning files with YARA rules, from a rules folder chosen in Settings",
    },
    Tool {
        id: "kingfisher",
        name: "Kingfisher",
        exe: "kingfisher",
        package: "kingfisher",
        source: Source::Release {
            version: "2.14.0",
            url: "https://github.com/mongodb/kingfisher/releases/download/v2.14.0/kingfisher-windows-x64.zip",
            sha256: "9f188b6fa16cec210203cb71439d3d661ab371a03305b3ada865f8defe601ffb",
            member: "kingfisher.exe",
        },
        used_for: "Finding passwords, keys and tokens left in files before they're shared (MongoDB's secret scanner; nothing is sent anywhere)",
    },
    Tool {
        id: "ast-grep",
        name: "ast-grep",
        exe: "ast-grep",
        package: "ast-grep.ast-grep",
        source: Source::Winget { id: "ast-grep.ast-grep" },
        used_for: "Code patterns in Text: code found (and rewritten) by its structure, console.log($MSG) or $X.unwrap()",
    },
    Tool {
        id: "jaq",
        name: "jaq",
        exe: "jaq",
        package: "01mf02.jaq",
        source: Source::Release {
            version: "3.1.1",
            url: "https://github.com/01mf02/jaq/releases/download/v3.1.1/jaq-x86_64-pc-windows-msvc.exe",
            sha256: "7b2f5153d11118c3ccff01859743e09842cabc42ba08f8876489c1be30d1d08d",
            member: "",
        },
        used_for: "jq filters over JSON, YAML and TOML files, in their preview (jq, rewritten in Rust)",
    },
    Tool {
        id: "zoxide",
        name: "zoxide",
        exe: "zoxide",
        package: "ajeetdsouza.zoxide",
        source: Source::Winget { id: "ajeetdsouza.zoxide" },
        used_for: "Folder history shared with your terminals (Settings → History): the folders you cd to rank in Ctrl+K, and z finds the ones you open here",
    },
];

pub fn get(id: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.id == id)
}

pub fn by_exe(exe: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.exe.eq_ignore_ascii_case(exe))
}

/// Glint's own tools folder: `tools` in the app's local data folder, per user. Set at start.
static DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

/// Where Glint keeps a tool it downloaded itself: `tools\<id>`.
pub fn folder_of(id: &str) -> Option<PathBuf> {
    DIR.get().map(|d| d.join(id))
}

/// A tool, ready to run without a console window; an error that says it isn't installed otherwise.
pub fn command(id: &str) -> Result<Command, String> {
    let tool = get(id).ok_or_else(|| format!("No tool is called {id}"))?;
    let (exe, _) = proc::locate(tool.exe).ok_or_else(|| format!("{} isn't installed. Settings → Tools installs it.", tool.name))?;
    let mut command = Command::new(exe);
    proc::no_window(&mut command);
    Ok(command)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    id: &'static str,
    name: &'static str,
    used_for: &'static str,
    /// What Install uses: "bundled" (comes with Glint; winget can put it back), "winget" or "release".
    source: &'static str,
    /// Where the copy that runs came from: "bundled", "downloaded" or "installed"; None when it's missing.
    origin: Option<&'static str>,
    path: Option<String>,
    version: Option<String>,
    /// The version a release download gets.
    pinned: Option<&'static str>,
    /// Glint downloaded this copy, so it can remove it.
    removable: bool,
}

fn status(tool: &'static Tool) -> ToolStatus {
    let found = proc::locate(tool.exe);
    let (source, pinned) = match tool.source {
        Source::Bundled { .. } => ("bundled", None),
        Source::Winget { .. } => ("winget", None),
        Source::Release { version, .. } => ("release", Some(version)),
    };
    let origin = found.as_ref().map(|(_, origin)| match origin {
        Origin::Bundled => "bundled",
        Origin::Downloaded => "downloaded",
        Origin::Installed => "installed",
    });
    let removable = matches!(found, Some((_, Origin::Downloaded)));
    ToolStatus {
        id: tool.id,
        name: tool.name,
        used_for: tool.used_for,
        source,
        origin,
        version: found.as_ref().and_then(|(path, _)| version_of(path)),
        path: found.map(|(path, _)| path.display().to_string()),
        pinned,
        removable,
    }
}

/// Every tool, where it runs from and its version (each asked at once, so a slow one doesn't hold the rest).
#[tauri::command]
pub async fn tools_status() -> Result<Vec<ToolStatus>, String> {
    blocking(|| Ok(std::thread::scope(|scope| TOOLS.iter().map(|tool| scope.spawn(move || status(tool))).collect::<Vec<_>>().into_iter().filter_map(|h| h.join().ok()).collect()))).await
}

/// The version a program says it is, from `--version`; None when it doesn't answer within a few seconds.
pub fn version_of(path: &Path) -> Option<String> {
    let mut command = Command::new(path);
    command.arg("--version").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    proc::no_window(&mut command);
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + Duration::from_secs(4);
    while child.try_wait().ok()?.is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    let output = child.wait_with_output().ok()?;
    version_in(&String::from_utf8_lossy(if output.stdout.is_empty() { &output.stderr } else { &output.stdout }))
}

/// The first version number in a program's `--version` text: "ripgrep 15.2.0 (rev 1a2b)" → "15.2.0".
fn version_in(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.split(|c: char| c.is_whitespace() || "(),:".contains(c)).find_map(|word| {
            let word = word.trim_start_matches(['v', 'V']);
            let parts: Vec<&str> = word.split('.').collect();
            (parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))).then(|| word.to_string())
        })
    })
}

/// How an install is going, for its progress bar: what it's doing, and bytes when it's downloading.
#[derive(Clone, Serialize)]
pub struct Progress {
    stage: String,
    done: u64,
    total: u64,
}

/// Installs running now: winget's process per tool, or a download's stop flag.
#[derive(Default)]
pub struct Installs {
    winget: Lanes,
    downloads: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

/// Installs a tool (or puts a bundled one back): with winget, or by downloading its release.
#[tauri::command]
pub async fn tool_install(id: String, on_progress: Channel<Progress>, installs: State<'_, Installs>) -> Result<ToolStatus, String> {
    let tool = get(&id).ok_or_else(|| format!("No tool is called {id}"))?;
    match tool.source {
        Source::Bundled { winget } | Source::Winget { id: winget } => {
            let lane = installs.winget.clone();
            blocking(move || winget_install(winget, &lane, tool.id, &on_progress)).await?;
        }
        Source::Release { url, sha256, member, .. } => {
            let stop = Arc::new(AtomicBool::new(false));
            installs.downloads.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), stop.clone());
            let done = download(tool, url, sha256, member, &stop, &on_progress).await;
            installs.downloads.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            done?;
        }
    }
    let found = blocking(move || Ok(status(tool))).await?;
    if found.origin.is_none() {
        return Err(format!("{} was installed, but Glint can't find it yet. Restarting Glint may help.", tool.name));
    }
    Ok(found)
}

#[tauri::command]
pub fn tool_cancel_install(id: String, installs: State<Installs>) {
    installs.winget.close(&id);
    if let Some(stop) = installs.downloads.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
        stop.store(true, Ordering::SeqCst);
    }
}

/// winget's "no installer for that scope": the package only installs for every user.
const NO_APPLICABLE_INSTALLER: i32 = 0x8A15_0010_u32 as i32;

fn winget_install(package: &str, lanes: &Lanes, lane: &str, progress: &Channel<Progress>) -> Result<(), String> {
    // For this account where the package allows it (portable ones always do); else as the package installs.
    match run_winget(&["install", "--id", package, "--scope", "user"], lanes, lane, progress) {
        Err(Ended { code: Some(NO_APPLICABLE_INSTALLER), .. }) => run_winget(&["install", "--id", package], lanes, lane, progress),
        other => other,
    }
    .map_err(|ended| ended.said)
}

/// How a winget run that didn't succeed ended: its exit code (None when it didn't run or was stopped),
/// and its last line of text, past its progress bars.
struct Ended {
    code: Option<i32>,
    said: String,
}

/// Runs winget, reporting what it says as progress.
fn run_winget(args: &[&str], lanes: &Lanes, lane: &str, progress: &Channel<Progress>) -> Result<(), Ended> {
    let mut winget = Command::new("winget");
    winget
        .args(args)
        .args(["--exact", "--source", "winget", "--silent", "--disable-interactivity", "--accept-package-agreements", "--accept-source-agreements"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    proc::no_window(&mut winget);
    let mut child = winget.spawn().map_err(|e| Ended { code: None, said: format!("winget isn't available on this PC ({e})") })?;
    let stdout = child.stdout.take();
    let slot = lanes.slot(lane);
    let pid = slot.replace(child);
    let mut said = String::new();
    if let Some(out) = stdout {
        // winget draws its progress bars with carriage returns: each \r or \n ends a line.
        let mut line = Vec::new();
        for byte in std::io::BufReader::new(out).bytes().map_while(Result::ok) {
            if byte != b'\r' && byte != b'\n' {
                line.push(byte);
                continue;
            }
            let text = String::from_utf8_lossy(&line).trim().to_string();
            line.clear();
            if let Some((done, total)) = bytes_of(&text) {
                let _ = progress.send(Progress { stage: "Downloading".into(), done, total });
            } else if text.chars().filter(|c| c.is_alphabetic()).count() > 2 {
                let _ = progress.send(Progress { stage: text.clone(), done: 0, total: 0 });
                said = text;
            }
        }
    }
    match slot.finish(pid) {
        Some(status) if status.success() => Ok(()),
        Some(status) => Err(Ended { code: status.code(), said: if said.is_empty() { format!("winget stopped with {status}") } else { said } }),
        None => Err(Ended { code: None, said: "cancelled".into() }),
    }
}

/// The bytes in a winget progress line such as "██████▒▒▒▒  2.00 MB / 4.51 MB".
fn bytes_of(line: &str) -> Option<(u64, u64)> {
    let (left, right) = line.split_once(" / ")?;
    let amount = |text: &str| -> Option<u64> {
        let mut words = text.split_whitespace();
        let number: f64 = words.next()?.parse().ok()?;
        let unit = match words.next()? {
            "B" => 1.0,
            "KB" => 1024.0,
            "MB" => 1024.0 * 1024.0,
            "GB" => 1024.0 * 1024.0 * 1024.0,
            _ => return None,
        };
        Some((number * unit) as u64)
    };
    let mut tail = left.split_whitespace().rev();
    let unit = tail.next()?;
    let number = tail.next()?;
    Some((amount(&format!("{number} {unit}"))?, amount(right)?))
}

/// The most a tool's download may be: anything bigger isn't what was pinned.
const MAX_DOWNLOAD: u64 = 512 << 20;

async fn download(tool: &'static Tool, url: &str, sha256: &str, member: &str, stop: &AtomicBool, progress: &Channel<Progress>) -> Result<(), String> {
    let dir = DIR.get().ok_or("Glint's data folder isn't available")?.clone();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // The TLS the updater uses; installing it twice is harmless.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .user_agent(concat!("Glint/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client.get(url).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("Couldn't download {}: {e}", tool.name))?;
    let total = response.content_length().unwrap_or(0);
    if total > MAX_DOWNLOAD {
        return Err(format!("{}'s download is bigger than expected, so it wasn't downloaded", tool.name));
    }
    let temp = dir.join(format!(".{}-{}.download", tool.id, std::process::id()));
    let fetched = async {
        let mut file = File::create(&temp).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let (mut done, mut told) = (0u64, Instant::now());
        while let Some(chunk) = response.chunk().await.map_err(|e| format!("The download of {} broke off: {e}", tool.name))? {
            if stop.load(Ordering::SeqCst) {
                return Err("cancelled".to_string());
            }
            done += chunk.len() as u64;
            if done > MAX_DOWNLOAD {
                return Err(format!("{}'s download is bigger than expected, so it was stopped", tool.name));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).map_err(|e| e.to_string())?;
            if told.elapsed() > Duration::from_millis(100) {
                let _ = progress.send(Progress { stage: "Downloading".into(), done, total: total.max(done) });
                told = Instant::now();
            }
        }
        file.sync_all().map_err(|e| e.to_string())?;
        Ok::<_, String>(hasher.finalize())
    }
    .await;
    let unpacked = match fetched {
        Ok(digest) if hex(&digest).eq_ignore_ascii_case(sha256) => {
            let _ = progress.send(Progress { stage: "Unpacking".into(), done: 0, total: 0 });
            let (temp, member, into, exe) = (temp.clone(), member.to_string(), dir.join(tool.id), tool.exe);
            blocking(move || if member.is_empty() { place(&temp, &into, exe) } else { unpack(&temp, &member, &into, exe) }).await
        }
        Ok(_) => Err(format!("The download didn't match {}'s published checksum, so it was thrown away", tool.name)),
        Err(e) => Err(e),
    };
    let _ = fs::remove_file(&temp);
    unpacked
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Takes only the program out of a checked release zip, into a fresh folder that then replaces the old one.
fn unpack(zip: &Path, member: &str, into: &Path, exe: &str) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(File::open(zip).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let wanted = member.replace('\\', "/");
    let index = (0..archive.len())
        .find(|&i| archive.name_for_index(i).and_then(Result::ok).is_some_and(|n| n.eq_ignore_ascii_case(&wanted)))
        .ok_or_else(|| format!("{member} isn't in the download"))?;
    let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
    let fresh = into.with_extension("new");
    let _ = fs::remove_dir_all(&fresh);
    fs::create_dir_all(&fresh).map_err(|e| e.to_string())?;
    let mut out = File::create(fresh.join(format!("{exe}.exe"))).map_err(|e| e.to_string())?;
    std::io::copy(&mut (&mut entry).take(MAX_DOWNLOAD), &mut out).map_err(|e| e.to_string())?;
    drop(out);
    replace(into, &fresh)
}

/// Puts a checked download that's the program itself in place, in a fresh folder that then replaces the old one.
fn place(download: &Path, into: &Path, exe: &str) -> Result<(), String> {
    let fresh = into.with_extension("new");
    let _ = fs::remove_dir_all(&fresh);
    fs::create_dir_all(&fresh).map_err(|e| e.to_string())?;
    fs::copy(download, fresh.join(format!("{exe}.exe"))).map_err(|e| e.to_string())?;
    replace(into, &fresh)
}

fn replace(into: &Path, fresh: &Path) -> Result<(), String> {
    match fs::remove_dir_all(into) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("The copy there now is in use, so it can't be replaced yet ({e})")),
        _ => {}
    }
    fs::rename(fresh, into).map_err(|e| e.to_string())
}

/// Removes a copy Glint downloaded. One installed with winget may be the person's own, used outside
/// Glint too, so it's theirs to uninstall.
#[tauri::command]
pub async fn tool_remove(id: String) -> Result<ToolStatus, String> {
    let tool = get(&id).ok_or_else(|| format!("No tool is called {id}"))?;
    blocking(move || {
        if !matches!(proc::locate(tool.exe), Some((_, Origin::Downloaded))) {
            return Err(format!("Glint didn't download {}, so it leaves it alone", tool.name));
        }
        let folder = folder_of(tool.id).ok_or("Glint's data folder isn't available")?;
        fs::remove_dir_all(&folder).map_err(|e| format!("Couldn't remove {} (is it running?): {e}", tool.name))?;
        Ok(status(tool))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_versions_from_what_tools_say() {
        assert_eq!(version_in("ripgrep 15.2.0 (rev 4519153e5e)\n\nfeatures:+pcre2").as_deref(), Some("15.2.0"));
        assert_eq!(version_in("fd 10.5.0").as_deref(), Some("10.5.0"));
        assert_eq!(version_in("Difftastic 0.71.0 (built with rustc 1.91.0)").as_deref(), Some("0.71.0"));
        assert_eq!(version_in("yr v1.22.0").as_deref(), Some("1.22.0"));
        assert_eq!(version_in("tokei 12.1.2 compiled with serialization support: json").as_deref(), Some("12.1.2"));
        assert_eq!(version_in("usage: tool [options]"), None);
    }

    #[test]
    fn reads_bytes_from_winget_progress_bars() {
        assert_eq!(bytes_of("██████████████▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒  2.00 MB / 4.00 MB"), Some((2 << 20, 4 << 20)));
        assert_eq!(bytes_of("████████  512 KB / 1.00 MB"), Some((512 << 10, 1 << 20)));
        assert_eq!(bytes_of("Successfully verified installer hash"), None);
    }

    #[test]
    fn every_tool_has_its_own_name_and_program() {
        for (i, tool) in TOOLS.iter().enumerate() {
            assert!(TOOLS[i + 1..].iter().all(|other| other.id != tool.id && other.exe != tool.exe), "{} is listed twice", tool.id);
            assert_eq!(tool.package, tool.package.to_ascii_lowercase(), "{}'s package folder is matched in lowercase", tool.id);
        }
    }

    #[test]
    fn unpacks_only_the_program_and_replaces_the_old_copy() {
        let dir = std::env::temp_dir().join(format!("glint-tools-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("difftastic")).unwrap();
        fs::write(dir.join("difftastic").join("old.txt"), "old").unwrap();
        let zip_path = dir.join("release.zip");
        let mut zip = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("difft-1.0/difft.exe", options).unwrap();
        zip.write_all(b"the program").unwrap();
        zip.start_file("difft-1.0/../../evil.exe", options).unwrap();
        zip.write_all(b"not wanted").unwrap();
        zip.finish().unwrap();
        unpack(&zip_path, "difft-1.0\\difft.exe", &dir.join("difftastic"), "difft").unwrap();
        assert_eq!(fs::read(dir.join("difftastic").join("difft.exe")).unwrap(), b"the program");
        assert!(!dir.join("difftastic").join("old.txt").exists(), "the old copy's folder was replaced");
        assert!(!dir.join("evil.exe").exists() && !std::env::temp_dir().join("evil.exe").exists());
        assert!(unpack(&zip_path, "missing.exe", &dir.join("x"), "x").is_err());
        // A release that's the program itself (jaq's) goes in the same way.
        fs::write(dir.join("jaq-x86_64-pc-windows-msvc.exe"), b"jaq").unwrap();
        place(&dir.join("jaq-x86_64-pc-windows-msvc.exe"), &dir.join("difftastic"), "jaq").unwrap();
        assert_eq!(fs::read(dir.join("difftastic").join("jaq.exe")).unwrap(), b"jaq");
        assert!(!dir.join("difftastic").join("difft.exe").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
