//! Optional tools: programs Glint drives for a feature or two but doesn't ship, so the download stays
//! small for everyone who never uses those features. Each installs when asked, on any PC: with winget
//! where the PC has it and winget has the tool, else (or when winget can't) from the tool's own release
//! on GitHub, pinned by its SHA-256 and checked before anything is unpacked. Their arguments are built in
//! Rust from typed options, like rg's, fd's and pdu's.

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

/// A program Glint can use, and where it comes from: winget, where the PC has it and winget has the tool
/// (so it's on PATH, where terminals find it too), else the tool's own release, so every tool installs on
/// every PC, with or without winget.
pub struct Tool {
    pub id: &'static str,
    pub name: &'static str,
    /// Its file name, without `.exe`.
    pub exe: &'static str,
    /// Part of its winget package folder's name (`Packages\<id>_<source>`, lowercase), to find a copy
    /// winget installed before this session's PATH knew about it.
    pub package: &'static str,
    /// It comes with Glint, next to the app: installing it puts back a copy a security tool removed, from its
    /// release first (the very build Glint ships).
    pub bundled: bool,
    /// Its winget package; "" when winget hasn't got it (or not at the version Glint is tested with).
    pub winget: &'static str,
    /// Its own release, installed where winget isn't on the PC or can't install it.
    pub release: Release,
    /// What it does in Glint, as Settings says it.
    pub used_for: &'static str,
}

/// A release on GitHub: the version it holds, its address and SHA-256, the program's path in the zip or
/// tarball ("" when the download is the program itself), and any other file it needs beside it.
pub struct Release {
    pub version: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub member: &'static str,
    pub with: &'static [&'static str],
}

const fn release(version: &'static str, url: &'static str, sha256: &'static str, member: &'static str) -> Release {
    Release { version, url, sha256, member, with: &[] }
}

pub static TOOLS: &[Tool] = &[
    Tool {
        id: "ripgrep",
        name: "ripgrep",
        exe: "rg",
        package: "ripgrep",
        bundled: true,
        winget: "BurntSushi.ripgrep.MSVC",
        release: release(
            "15.2.0",
            "https://github.com/BurntSushi/ripgrep/releases/download/15.2.0/ripgrep-15.2.0-x86_64-pc-windows-msvc.zip",
            "71b2fef860abe467217a538ff31de02f5258807c0129f771846f87bd029aafc5",
            "ripgrep-15.2.0-x86_64-pc-windows-msvc/rg.exe",
        ),
        used_for: "Searching inside files",
    },
    Tool {
        id: "fd",
        name: "fd",
        exe: "fd",
        package: "sharkdp.fd",
        bundled: true,
        winget: "sharkdp.fd",
        release: release(
            "10.5.0",
            "https://github.com/sharkdp/fd/releases/download/v10.5.0/fd-v10.5.0-x86_64-pc-windows-msvc.zip",
            "a227701b8551c35a9931d9f6da75503cf86d88e182d71fb849a70864c5d57cd7",
            "fd-v10.5.0-x86_64-pc-windows-msvc/fd.exe",
        ),
        used_for: "Finding files by name where instant search doesn't",
    },
    Tool {
        id: "pdu",
        name: "pdu",
        exe: "pdu",
        package: "parallel-disk-usage",
        bundled: true,
        winget: "",
        release: release(
            "0.24.0",
            "https://github.com/KSXGitHub/parallel-disk-usage/releases/download/0.24.0/pdu-x86_64-pc-windows-msvc.exe",
            "1133cde2e4cf3448b5fa6facd2883391d88957057da625a5ab910e7d7c15c36e",
            "",
        ),
        used_for: "Folder sizes and disk-usage maps where instant search doesn't",
    },
    Tool {
        id: "czkawka",
        name: "Czkawka",
        exe: "windows_czkawka_cli",
        package: "qarmin.czkawka.cli",
        bundled: false,
        winget: "qarmin.czkawka.cli",
        release: release(
            "12.0.2",
            "https://github.com/qarmin/czkawka/releases/download/12.0.2/windows_czkawka_cli.exe",
            "eb7c2009d2dd49cf5202acbd65370caece0a3f819b9515e727cedb1ec088ed1a",
            "",
        ),
        used_for: "Look-alike pictures, videos and music, broken files and wrong extensions in Clean up",
    },
    Tool {
        id: "difftastic",
        name: "Difftastic",
        exe: "difft",
        package: "wilfred.difftastic",
        bundled: false,
        winget: "Wilfred.difftastic",
        release: release(
            "0.72.0",
            "https://github.com/Wilfred/difftastic/releases/download/0.72.0/difft-0.72.0-x86_64-pc-windows-msvc.zip",
            "7b6295a9a433ba79978b02ad4b4b27f955423b8eb379927409b9ff4e5156379e",
            "difft.exe",
        ),
        used_for: "Comparing code by its structure (Compare two files): what changed in it, not its layout",
    },
    // Any FFmpeg found is used; a new install is the smaller build, which reads every common video. Czkawka
    // runs ffprobe beside it too.
    Tool {
        id: "ffmpeg",
        name: "FFmpeg",
        exe: "ffmpeg",
        package: "gyan.ffmpeg",
        bundled: false,
        winget: "Gyan.FFmpeg.Essentials",
        release: Release {
            version: "9.0.2",
            url: "https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip",
            sha256: "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba",
            member: "ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe",
            with: &["ffmpeg-9.0.2-essentials_build/bin/ffprobe.exe"],
        },
        used_for: "Reading videos for Clean up's look-alike videos",
    },
    Tool {
        id: "magika",
        name: "Magika",
        exe: "magika",
        package: "google.magika",
        bundled: false,
        winget: "Google.Magika",
        release: release(
            "1.1.0",
            "https://github.com/google/magika/releases/download/cli%2Fv1.1.0/magika-cli-x86_64-pc-windows-msvc.zip",
            "d4de347c53e2d25f9663780c93fdf26d156284c64d6a5ec4bcdb829f9994f6f3",
            "magika.exe",
        ),
        used_for: "A file's real type in the File X-Ray, told by its content (a program named photo.jpg, say)",
    },
    Tool {
        id: "yara-x",
        name: "YARA-X",
        exe: "yr",
        package: "virustotal.yara-x",
        bundled: false,
        winget: "VirusTotal.YARA-X",
        release: release(
            "1.22.1",
            "https://github.com/VirusTotal/yara-x/releases/download/v1.22.1/yara-x-v1.22.1-x86_64-pc-windows-msvc.zip",
            "dbe8c1ec5d59d8a81c777d949c518fb5e75f1026d3419bca7817727c5d47b327",
            "yr.exe",
        ),
        used_for: "Scanning files with YARA rules, from a rules folder chosen in Settings",
    },
    Tool {
        id: "kingfisher",
        name: "Kingfisher",
        exe: "kingfisher",
        package: "kingfisher",
        bundled: false,
        winget: "",
        release: release(
            "2.14.0",
            "https://github.com/mongodb/kingfisher/releases/download/v2.14.0/kingfisher-windows-x64.zip",
            "9f188b6fa16cec210203cb71439d3d661ab371a03305b3ada865f8defe601ffb",
            "kingfisher.exe",
        ),
        used_for: "Finding passwords, keys and tokens left in files before they're shared (MongoDB's secret scanner; nothing is sent anywhere)",
    },
    Tool {
        id: "ast-grep",
        name: "ast-grep",
        exe: "ast-grep",
        package: "ast-grep.ast-grep",
        bundled: false,
        winget: "ast-grep.ast-grep",
        release: release(
            "0.51.0",
            "https://github.com/ast-grep/ast-grep/releases/download/0.51.0/app-x86_64-pc-windows-msvc.zip",
            "1a5b7824c5925ac34257f806488e54860817cefd855fd308d13df7550d068908",
            "ast-grep.exe",
        ),
        used_for: "Code patterns in Text: code found (and rewritten) by its structure, console.log($MSG) or $X.unwrap()",
    },
    Tool {
        id: "jaq",
        name: "jaq",
        exe: "jaq",
        package: "01mf02.jaq",
        bundled: false,
        winget: "",
        release: release(
            "3.1.1",
            "https://github.com/01mf02/jaq/releases/download/v3.1.1/jaq-x86_64-pc-windows-msvc.exe",
            "7b2f5153d11118c3ccff01859743e09842cabc42ba08f8876489c1be30d1d08d",
            "",
        ),
        used_for: "jq filters over JSON, YAML and TOML files, in their preview (jq, rewritten in Rust)",
    },
    Tool {
        id: "zoxide",
        name: "zoxide",
        exe: "zoxide",
        package: "ajeetdsouza.zoxide",
        bundled: false,
        winget: "ajeetdsouza.zoxide",
        release: release(
            "0.10.0",
            "https://github.com/ajeetdsouza/zoxide/releases/download/v0.10.0/zoxide-0.10.0-x86_64-pc-windows-msvc.zip",
            "f465ae548f8754c8e7edbc60b45fbf58c92bfe123db83d790252d6810fa5daf1",
            "zoxide.exe",
        ),
        used_for: "Folder history shared with your terminals (Settings → History): the folders you cd to rank in Ctrl+K, and z finds the ones you open here",
    },
    Tool {
        id: "rimage",
        name: "rimage",
        exe: "rimage",
        package: "rimage",
        bundled: false,
        winget: "",
        release: release(
            "0.14.0",
            "https://github.com/vlad-salone/rimage/releases/download/v0.14.0/rimage-0.14.0-x86_64-pc-windows-msvc.zip",
            "93667bd6140b9845f72abc3849d22dd25b9c33ae0d8b08ffb86835c81afbcf25",
            "rimage-0.14.0-x86_64-pc-windows-msvc/rimage.exe",
        ),
        used_for: "Converting pictures to JPEG, PNG, WebP or AVIF, smaller if asked (Convert pictures…)",
    },
    Tool {
        id: "onefetch",
        name: "onefetch",
        exe: "onefetch",
        package: "o2sh.onefetch",
        bundled: false,
        winget: "",
        release: release(
            "3.1.0",
            "https://github.com/o2sh/onefetch/releases/download/3.1.0/onefetch-win.tar.gz",
            "da8bbfb4566c2d4ef5303b9f126b08d5176060f508f94b75bba5d5320bc9bc76",
            "onefetch.exe",
        ),
        used_for: "A Git repository's card in its preview: its languages, who works on it, its last change and license",
    },
    // winget's tokei is this one too: its later releases carry no programs.
    Tool {
        id: "tokei",
        name: "tokei",
        exe: "tokei",
        package: "xampprocky.tokei",
        bundled: false,
        winget: "XAMPPRocky.Tokei",
        release: release(
            "12.1.2",
            "https://github.com/XAMPPRocky/tokei/releases/download/v12.1.2/tokei-x86_64-pc-windows-msvc.exe",
            "b1d6c4b18f5fa238bd2c6e47caa65a7a3e4a1bd0de6df0b7c19c8083c941f57b",
            "",
        ),
        used_for: "Lines of code by language, on a Git repository's card",
    },
    Tool {
        id: "just",
        name: "just",
        exe: "just",
        package: "casey.just",
        bundled: false,
        winget: "Casey.Just",
        release: release(
            "1.59.0",
            "https://github.com/casey/just/releases/download/1.59.0/just-1.59.0-x86_64-pc-windows-msvc.zip",
            "9c5463e2d0974e31bee71346aa5cbf19167de1585a4abc1bae7b3b04604ea9c0",
            "just.exe",
        ),
        used_for: "A justfile's recipes in the Run menu",
    },
    Tool {
        id: "uv",
        name: "uv",
        exe: "uv",
        package: "astral-sh.uv",
        bundled: false,
        winget: "astral-sh.uv",
        release: release(
            "0.13.6",
            "https://github.com/astral-sh/uv/releases/download/0.13.6/uv-x86_64-pc-windows-msvc.zip",
            "a2a8820d5eea32f08ef467ef3b7f66883a9b076d5737a2fc2fbb5f458d1bb50f",
            "uv.exe",
        ),
        used_for: "Python projects' scripts in the Run menu, and Python scripts in Scripts",
    },
    // The baseline build, which runs on processors without AVX2 too.
    Tool {
        id: "bun",
        name: "Bun",
        exe: "bun",
        package: "oven-sh.bun",
        bundled: false,
        winget: "Oven-sh.Bun",
        release: release(
            "1.4.4",
            "https://github.com/oven-sh/bun/releases/download/bun-v1.4.4/bun-windows-x64-baseline.zip",
            "0f20ce6674d4f2dbcc8c5d59fdf99f7ba45f8c3b367e0f5781cbfa30a1c78d99",
            "bun-windows-x64-baseline/bun.exe",
        ),
        used_for: "TypeScript and JavaScript scripts in Scripts",
    },
    Tool {
        id: "sendme",
        name: "sendme",
        exe: "sendme",
        package: "n0-computer.sendme",
        bundled: false,
        winget: "n0-computer.sendme",
        release: release(
            "0.37.1",
            "https://github.com/n0-computer/sendme/releases/download/v0.37.1/sendme-v0.37.1-windows-x86_64.zip",
            "fcf37ac1ebfff164f0a8e0cd0bd592531a028dac364ef6b05bff277b3d11faf5",
            "sendme.exe",
        ),
        used_for: "Sending files and folders to another PC",
    },
    Tool {
        id: "dufs",
        name: "dufs",
        exe: "dufs",
        package: "sigoden.dufs",
        bundled: false,
        winget: "sigoden.Dufs",
        release: release(
            "0.47.0",
            "https://github.com/sigoden/dufs/releases/download/v0.47.0/dufs-v0.47.0-x86_64-pc-windows-msvc.zip",
            "39a000b70de423b7200f17099fa734cb6e154d8c6ef6a7d4ec9b2961cb66d0f1",
            "dufs.exe",
        ),
        used_for: "Sharing a folder with a phone on this network",
    },
    Tool {
        id: "rclone",
        name: "rclone",
        exe: "rclone",
        package: "rclone.rclone",
        bundled: false,
        winget: "Rclone.Rclone",
        release: release(
            "1.75.2",
            "https://github.com/rclone/rclone/releases/download/v1.75.2/rclone-v1.75.2-windows-amd64.zip",
            "8a9db2bad1dc79a65b3348fd857069598e2aab7cf4d118b77b1df23055aff818",
            "rclone-v1.75.2-windows-amd64/rclone.exe",
        ),
        used_for: "Google Drive, OneDrive, Dropbox and 60 more clouds as folders, signed in to from Glint, and folders kept in sync with them by Folder rules",
    },
    Tool {
        id: "nushell",
        name: "Nushell",
        exe: "nu",
        package: "nushell.nushell",
        bundled: false,
        winget: "Nushell.Nushell",
        release: release(
            "0.116.1",
            "https://github.com/nushell/nushell/releases/download/0.116.1/nu-0.116.1-x86_64-pc-windows-msvc.zip",
            "6658c9e61fe42c88326c1caa99e8dc9ad99bec6c0bc353e6ecb7c5d3f79feed7",
            "nu.exe",
        ),
        used_for: "A shell for the terminal, and Nushell scripts in Scripts",
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
    /// What Install tries first on this PC: "winget" (where it's on the PC and has the tool, for one that
    /// doesn't come with Glint), else "release".
    source: &'static str,
    /// Where the copy that runs came from: "bundled", "downloaded" or "installed"; None when it's missing.
    origin: Option<&'static str>,
    path: Option<String>,
    version: Option<String>,
    /// The version its release download gets.
    pinned: Option<&'static str>,
    /// Glint downloaded this copy, so it can remove it.
    removable: bool,
}

fn status(tool: &'static Tool) -> ToolStatus {
    let found = proc::locate(tool.exe);
    let source = if !tool.bundled && !tool.winget.is_empty() && winget().is_some() { "winget" } else { "release" };
    let pinned = Some(tool.release.version);
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

/// The first version number in a program's `--version` text: "ripgrep 15.2.0 (rev 1a2b)" → "15.2.0",
/// "ffmpeg version 9.0.2-essentials_build-www.gyan.dev" → "9.0.2".
fn version_in(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.split(|c: char| c.is_whitespace() || "(),:".contains(c)).find_map(|word| {
            let word = word.trim_start_matches(['v', 'V']);
            let number = word[..word.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(word.len())].trim_end_matches('.');
            let parts: Vec<&str> = number.split('.').collect();
            (parts.len() >= 2 && parts.iter().all(|p| !p.is_empty())).then(|| number.to_string())
        })
    })
}

/// How an install is going, for its progress bar: what it's doing, and bytes when it's downloading.
#[derive(Clone, Serialize)]
pub struct Progress {
    pub(crate) stage: String,
    pub(crate) done: u64,
    pub(crate) total: u64,
}

/// Installs running now: winget's process per tool, or a download's stop flag.
#[derive(Default)]
pub struct Installs {
    winget: Lanes,
    downloads: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Installs {
    /// A download starting: its stop flag, which `tool_cancel_install` sets for `id`.
    pub(crate) fn begin(&self, id: &str) -> Arc<AtomicBool> {
        let stop = Arc::new(AtomicBool::new(false));
        self.downloads.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), stop.clone());
        stop
    }

    pub(crate) fn end(&self, id: &str) {
        self.downloads.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }

    /// Stops `id`'s install: winget's process, or the download.
    fn cancel(&self, id: &str) {
        self.winget.close(id);
        if let Some(stop) = self.downloads.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            stop.store(true, Ordering::SeqCst);
        }
    }
}

/// Installs a tool (or puts a bundled one back): with winget where the PC has it and winget has the tool,
/// else, or when winget can't, from the tool's own release.
#[tauri::command]
pub async fn tool_install(id: String, on_progress: Channel<Progress>, installs: State<'_, Installs>) -> Result<ToolStatus, String> {
    install(&id, on_progress, &installs).await
}

async fn install(id: &str, on_progress: Channel<Progress>, installs: &Installs) -> Result<ToolStatus, String> {
    let tool = get(id).ok_or_else(|| format!("No tool is called {id}"))?;
    // The ways to get it, in order (None is its release): winget first, as what it installs goes on PATH,
    // where terminals find it too; but a tool that comes with Glint is put back from its release first, the
    // very build Glint ships. When one way can't, the next is tried.
    let mut ways: Vec<Option<PathBuf>> = winget().filter(|_| !tool.winget.is_empty()).into_iter().map(Some).collect();
    ways.insert(if tool.bundled { 0 } else { ways.len() }, None);
    let mut failed = Vec::new();
    for (i, way) in ways.into_iter().enumerate() {
        if i > 0 {
            let _ = on_progress.send(Progress { stage: "That didn't work, so Glint tries another way".into(), done: 0, total: 0 });
        }
        let tried = match way {
            Some(winget) => with_winget(tool, winget, on_progress.clone(), installs).await,
            None => from_release(tool, &on_progress, installs).await,
        };
        match tried {
            Ok(found) => return Ok(found),
            Err(e) if e == "cancelled" => return Err(e),
            Err(e) => failed.push(e),
        }
    }
    Err(failed.join("; then "))
}

/// Installs a tool with winget; its status, once Glint finds it.
async fn with_winget(tool: &'static Tool, winget: PathBuf, progress: Channel<Progress>, installs: &Installs) -> Result<ToolStatus, String> {
    let lane = installs.winget.clone();
    blocking(move || winget_install(&winget, tool.winget, &lane, tool.id, &progress)).await.map_err(|e| if e == "cancelled" { e } else { format!("winget: {e}") })?;
    found(tool).await
}

/// Downloads a tool's release; its status, once Glint finds it.
async fn from_release(tool: &'static Tool, progress: &Channel<Progress>, installs: &Installs) -> Result<ToolStatus, String> {
    let stop = installs.begin(tool.id);
    let done = download(tool, &stop, progress).await;
    installs.end(tool.id);
    done?;
    found(tool).await
}

/// A tool's status once it's installed, or an error when Glint can't find it yet.
async fn found(tool: &'static Tool) -> Result<ToolStatus, String> {
    let found = blocking(move || Ok(status(tool))).await?;
    if found.origin.is_none() {
        return Err(format!("{} was installed, but Glint can't find it yet (restarting Glint may help)", tool.name));
    }
    Ok(found)
}

/// winget, where this PC has it: App Installer's alias, on PATH or in the account's WindowsApps folder.
/// Windows Server, LTSC and some managed PCs have none.
fn winget() -> Option<PathBuf> {
    let path: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let apps = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(r"Microsoft\WindowsApps"));
    // The alias is a link that can't be opened as a file, so it's looked at rather than followed.
    path.into_iter().chain(apps).map(|dir| dir.join("winget.exe")).find(|exe| exe.is_absolute() && fs::symlink_metadata(exe).is_ok())
}

#[tauri::command]
pub fn tool_cancel_install(id: String, installs: State<Installs>) {
    installs.cancel(&id);
}

/// winget's "no installer for that scope": the package only installs for every user.
const NO_APPLICABLE_INSTALLER: i32 = 0x8A15_0010_u32 as i32;

fn winget_install(winget: &Path, package: &str, lanes: &Lanes, lane: &str, progress: &Channel<Progress>) -> Result<(), String> {
    // For this account where the package allows it (portable ones always do); else as the package installs.
    match run_winget(winget, &["install", "--id", package, "--scope", "user"], lanes, lane, progress) {
        Err(Ended { code: Some(NO_APPLICABLE_INSTALLER), .. }) => run_winget(winget, &["install", "--id", package], lanes, lane, progress),
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
fn run_winget(winget: &Path, args: &[&str], lanes: &Lanes, lane: &str, progress: &Channel<Progress>) -> Result<(), Ended> {
    let mut winget = Command::new(winget);
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

/// Downloads a tool's release into its own folder in Glint's tools folder: the program, under the tool's
/// name, and anything it needs beside it.
async fn download(tool: &'static Tool, stop: &AtomicBool, progress: &Channel<Progress>) -> Result<(), String> {
    let release = &tool.release;
    let temp = fetch(release.url, release.sha256, tool.id, tool.name, stop, progress).await?;
    let _ = progress.send(Progress { stage: "Unpacking".into(), done: 0, total: 0 });
    let into = DIR.get().ok_or("Glint's data folder isn't available")?.join(tool.id);
    let file = temp.clone();
    let unpacked = blocking(move || {
        let program = format!("{}.exe", tool.exe);
        let files: Vec<(&str, &str)> = std::iter::once((release.member, program.as_str())).chain(release.with.iter().map(|m| (*m, m.rsplit('/').next().unwrap_or(m)))).collect();
        if release.member.is_empty() {
            place(&file, &into, tool.exe)
        } else if release.url.ends_with(".tar.gz") {
            unpack_tarball(&file, &files, &into)
        } else {
            unpack(&file, &files, &into)
        }
    })
    .await;
    let _ = fs::remove_file(&temp);
    unpacked
}

/// Downloads `url` (what `name` is) into a file of its own in Glint's data folder, with progress, and
/// checks it against its pinned `sha256`: the file, or why not (one that doesn't match is thrown away).
/// A download that can't connect, or breaks off, is tried again over IPv4 alone.
pub(crate) async fn fetch(url: &str, sha256: &str, id: &str, name: &str, stop: &AtomicBool, progress: &Channel<Progress>) -> Result<PathBuf, String> {
    let dir = DIR.get().ok_or("Glint's data folder isn't available")?.clone();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let temp = dir.join(format!(".{id}-{}.download", std::process::id()));
    // As the PC finds the address first, then over IPv4 alone when that couldn't connect or broke off.
    let mut fetched = fetch_into(&temp, url, name, false, stop, progress).await;
    if matches!(fetched, Err(Fetch::Network(_))) {
        fetched = fetch_into(&temp, url, name, true, stop, progress).await;
    }
    match fetched {
        Ok(digest) if hex(&digest).eq_ignore_ascii_case(sha256) => Ok(temp),
        other => {
            let _ = fs::remove_file(&temp);
            Err(match other {
                Err(Fetch::Network(e) | Fetch::Failed(e)) => e,
                Ok(_) => format!("The download didn't match {name}'s published checksum, so it was thrown away"),
            })
        }
    }
}

/// Why a download didn't finish: the network, worth another try over IPv4, or anything else.
enum Fetch {
    Network(String),
    Failed(String),
}

/// Downloads `url` into `temp` with progress: its SHA-256.
async fn fetch_into(temp: &Path, url: &str, name: &str, ipv4: bool, stop: &AtomicBool, progress: &Channel<Progress>) -> Result<sha2::digest::Output<Sha256>, Fetch> {
    let client = client(ipv4).map_err(Fetch::Failed)?;
    let mut response = match client.get(url).send().await {
        Ok(response) => response.error_for_status().map_err(|e| Fetch::Failed(format!("Couldn't download {name}: {e}")))?,
        Err(e) => return Err(Fetch::Network(format!("Couldn't download {name}: {e}"))),
    };
    let total = response.content_length().unwrap_or(0);
    if total > MAX_DOWNLOAD {
        return Err(Fetch::Failed(format!("{name}'s download is bigger than expected, so it wasn't downloaded")));
    }
    let _ = progress.send(Progress { stage: "Downloading".into(), done: 0, total });
    let mut file = File::create(temp).map_err(|e| Fetch::Failed(e.to_string()))?;
    let mut hasher = Sha256::new();
    let (mut done, mut told) = (0u64, Instant::now());
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(e) => return Err(Fetch::Network(format!("The download of {name} broke off: {e}"))),
        };
        if stop.load(Ordering::SeqCst) {
            return Err(Fetch::Failed("cancelled".to_string()));
        }
        done += chunk.len() as u64;
        if done > MAX_DOWNLOAD {
            return Err(Fetch::Failed(format!("{name}'s download is bigger than expected, so it was stopped")));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).map_err(|e| Fetch::Failed(e.to_string()))?;
        if told.elapsed() > Duration::from_millis(100) {
            let _ = progress.send(Progress { stage: "Downloading".into(), done, total: total.max(done) });
            told = Instant::now();
        }
    }
    file.sync_all().map_err(|e| Fetch::Failed(e.to_string()))?;
    Ok(hasher.finalize())
}

/// The HTTP client Glint downloads with: the TLS the updater uses, and the PC's proxy. With `ipv4`, it
/// connects over IPv4 alone, as a network whose IPv6 is broken (connections reset, or never answered)
/// still gets through that way.
pub(crate) fn client(ipv4: bool) -> Result<reqwest::Client, String> {
    // Installing the TLS provider twice is harmless.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let builder = reqwest::Client::builder().user_agent(concat!("Glint/", env!("CARGO_PKG_VERSION"))).connect_timeout(Duration::from_secs(15));
    let builder = if ipv4 { builder.dns_resolver(Arc::new(Ipv4Only)) } else { builder };
    builder.build().map_err(|e| e.to_string())
}

/// Finds a name's IPv4 addresses only.
struct Ipv4Only;

impl reqwest::dns::Resolve for Ipv4Only {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let found = tauri::async_runtime::spawn_blocking(move || {
                use std::net::ToSocketAddrs;
                (host.as_str(), 0).to_socket_addrs().map(|addrs| addrs.filter(std::net::SocketAddr::is_ipv4).collect::<Vec<_>>())
            })
            .await;
            let addrs = found.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Glint's data folder (the tools folder's parent), for what Glint downloads besides tools.
pub(crate) fn data_dir() -> Option<PathBuf> {
    DIR.get().and_then(|d| d.parent()).map(Path::to_path_buf)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Takes only the files named, `(member, name)`, out of a checked release zip, into a fresh folder that
/// then replaces the old one.
fn unpack(zip: &Path, files: &[(&str, &str)], into: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(File::open(zip).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fresh_folder(into, |fresh| {
        for (member, name) in files {
            let wanted = member.replace('\\', "/");
            let index = (0..archive.len())
                .find(|&i| archive.name_for_index(i).and_then(Result::ok).is_some_and(|n| n.eq_ignore_ascii_case(&wanted)))
                .ok_or_else(|| format!("{member} isn't in the download"))?;
            let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
            let mut out = File::create(fresh.join(name)).map_err(|e| e.to_string())?;
            std::io::copy(&mut (&mut entry).take(MAX_DOWNLOAD), &mut out).map_err(|e| e.to_string())?;
        }
        Ok(())
    })
}

/// Takes only the files named, `(member, name)`, out of a checked release tarball (.tar.gz), into a fresh
/// folder that then replaces the old one.
fn unpack_tarball(tarball: &Path, files: &[(&str, &str)], into: &Path) -> Result<(), String> {
    fresh_folder(into, |fresh| {
        for (member, name) in files {
            let bytes = crate::archive::tar_gz_member(tarball, member, MAX_DOWNLOAD)?;
            fs::write(fresh.join(name), bytes).map_err(|e| e.to_string())?;
        }
        Ok(())
    })
}

/// Fills a fresh folder beside `into`, which then replaces it; one that couldn't be filled is removed.
fn fresh_folder(into: &Path, fill: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    let fresh = into.with_extension("new");
    let _ = fs::remove_dir_all(&fresh);
    fs::create_dir_all(&fresh).map_err(|e| e.to_string())?;
    if let Err(e) = fill(&fresh) {
        let _ = fs::remove_dir_all(&fresh);
        return Err(e);
    }
    replace(into, &fresh)
}

/// Puts a checked download that's the program itself in place, in a fresh folder that then replaces the old one.
fn place(download: &Path, into: &Path, exe: &str) -> Result<(), String> {
    fresh_folder(into, |fresh| fs::copy(download, fresh.join(format!("{exe}.exe"))).map(|_| ()).map_err(|e| e.to_string()))
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

/// `Glint.exe tools install <id>…`: installs tools as Settings → Tools does (and `meaning`, the meaning
/// model), for the installer's Extra tools page (installer/tools.nsh) and scripted setups. One that's on
/// this PC already is left as it is. What it's doing goes to stdout a line at a time (the installer's
/// details) and, with `--installer <window>`, onto the installer's status line as it goes. `--data
/// <folder>` is Glint's local data folder, whose tools folder the downloads go in (the installer names
/// its own; else the one Glint uses). The exit code is 0 once every one is installed, else 1.
pub fn cli() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("tools") {
        return None;
    }
    let command = args.next();
    let (mut ids, mut data, mut window) = (Vec::new(), None, 0);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data" => data = args.next().map(PathBuf::from),
            "--installer" => window = args.next().and_then(|w| w.parse().ok()).unwrap_or(0),
            _ => ids.push(arg),
        }
    }
    if command.as_deref() != Some("install") || ids.is_empty() {
        say("Usage: Glint.exe tools install <tool>... [--data <folder>] [--installer <window>]");
        return Some(2);
    }
    let Some(data) = data.or_else(|| Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(identifier()?))) else {
        say("Glint's data folder isn't known: give it with --data");
        return Some(1);
    };
    set_dir(data.join("tools"));
    let installs = Arc::new(Installs::default());
    let status = StatusLine::find(window);
    let failed = ids
        .iter()
        .filter(|id| match tauri::async_runtime::block_on(install_here(id, &installs, status.clone())) {
            Ok(done) => {
                say(&done);
                false
            }
            Err(e) => {
                say(&format!("Couldn't install {id}: {e}"));
                true
            }
        })
        .count();
    Some(i32::from(failed > 0))
}

/// The app's identifier, as its local data folder is named.
fn identifier() -> Option<String> {
    let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).ok()?;
    config["identifier"].as_str().map(str::to_string)
}

/// Installs one for `tools install`, and what to say once it's done. One that stops making progress for
/// a few minutes (a download that stalled, winget waiting on something) is stopped.
async fn install_here(id: &str, installs: &Arc<Installs>, status: Option<StatusLine>) -> Result<String, String> {
    const STALLED: Duration = Duration::from_secs(300);
    let heard = Arc::new(Mutex::new(Instant::now()));
    let done = Arc::new(AtomicBool::new(false));
    {
        let (heard, done, installs, id) = (heard.clone(), done.clone(), installs.clone(), id.to_string());
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(2));
                if heard.lock().unwrap_or_else(|e| e.into_inner()).elapsed() > STALLED {
                    installs.cancel(&id);
                    break;
                }
            }
        });
    }
    let progress = said_as_it_goes(status, heard);
    let result = if id == "meaning" {
        match blocking(|| Ok(crate::meaning::installed())).await? {
            true => Ok("The meaning model is downloaded already".to_string()),
            false => crate::meaning::install(&progress, installs).await.map(|()| "The meaning model is downloaded".to_string()),
        }
    } else {
        let tool = get(id).ok_or_else(|| format!("no tool is called {id}"))?;
        match blocking(move || Ok(proc::locate(tool.exe))).await? {
            Some((path, _)) => Ok(format!("{} is on this PC already ({})", tool.name, path.display())),
            None => install(id, progress, installs).await.map(|found| match found.version {
                Some(version) => format!("{} {version} is installed", tool.name),
                None => format!("{} is installed", tool.name),
            }),
        }
    };
    done.store(true, Ordering::SeqCst);
    result.map_err(|e| if e == "cancelled" { "it stopped making progress, so it was stopped".to_string() } else { e })
}

/// A progress channel for `tools install`: each new stage is said once (a download's size too), and
/// the installer's status line shows each update, bytes and all. `heard` is when the last one came.
fn said_as_it_goes(status: Option<StatusLine>, heard: Arc<Mutex<Instant>>) -> Channel<Progress> {
    let stage = Mutex::new(String::new());
    Channel::new(move |body| {
        *heard.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        let tauri::ipc::InvokeResponseBody::Json(json) = body else { return Ok(()) };
        let Ok(progress) = serde_json::from_str::<serde_json::Value>(&json) else { return Ok(()) };
        let now = progress["stage"].as_str().unwrap_or_default();
        let (done, total) = (progress["done"].as_u64().unwrap_or(0), progress["total"].as_u64().unwrap_or(0));
        if let Some(status) = &status {
            status.show(&if total > 0 { format!("{now} {} of {}", megabytes(done), megabytes(total)) } else { now.to_string() });
        }
        // Each new stage once, and each download as it starts (no bytes yet), with its size.
        let mut said = stage.lock().unwrap_or_else(|e| e.into_inner());
        if *said != now || (now == "Downloading" && done == 0) {
            *said = now.to_string();
            say(&if total > 0 { format!("{now} {}", megabytes(total)) } else { now.to_string() });
        }
        Ok(())
    })
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1 << 20) as f64)
}

/// A line for whoever ran `tools install`. The installer reads it in the PC's ANSI code page, so it's
/// kept to ASCII; and one that stopped reading doesn't stop the install.
fn say(line: &str) {
    let line: String = line.replace('…', "...").replace('→', "->").chars().map(|c| if c.is_ascii() { c } else { '?' }).collect();
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}").and_then(|()| out.flush());
}

/// The status line of the installer that ran `tools install` (the text over its progress bar on its
/// install page, NSIS's item 1006), which an install's progress is shown on after what the installer put
/// there (Installing Difftastic (2 of 5)).
#[derive(Clone)]
struct StatusLine {
    text: isize,
    prefix: String,
}

impl StatusLine {
    fn find(installer: isize) -> Option<Self> {
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetDlgItem, IsWindow, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_GETTEXT};
        // A silent install has no window: 0, which mustn't find some other program's.
        if installer == 0 {
            return None;
        }
        unsafe {
            let window = HWND(installer as _);
            if !IsWindow(Some(window)).as_bool() {
                return None;
            }
            let page = FindWindowExW(Some(window), None, w!("#32770"), PCWSTR::null()).ok()?;
            let text = GetDlgItem(Some(page), 1006).ok()?;
            let mut buffer = [0u16; 512];
            let mut length = 0usize;
            SendMessageTimeoutW(text, WM_GETTEXT, WPARAM(buffer.len()), LPARAM(buffer.as_mut_ptr() as isize), SMTO_ABORTIFHUNG, 1000, Some(&mut length));
            let prefix = String::from_utf16_lossy(&buffer[..length.min(buffer.len())]);
            Some(StatusLine { text: text.0 as isize, prefix: prefix.trim_end_matches(['…', '.', ' ']).to_string() })
        }
    }

    fn show(&self, what: &str) {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_SETTEXT};
        let line: Vec<u16> = format!("{}: {what}", self.prefix).encode_utf16().chain([0]).collect();
        unsafe { SendMessageTimeoutW(HWND(self.text as _), WM_SETTEXT, WPARAM(0), LPARAM(line.as_ptr() as isize), SMTO_ABORTIFHUNG, 1000, None) };
    }
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
        assert_eq!(version_in("ffmpeg version 9.0.2-essentials_build-www.gyan.dev Copyright (c) 2000-2026 the FFmpeg developers").as_deref(), Some("9.0.2"));
        assert_eq!(version_in("uv 0.13.6 (a1b2c3d4e 2026-10-27)").as_deref(), Some("0.13.6"));
        assert_eq!(version_in("usage: tool [options]"), None);
    }

    #[test]
    fn reads_bytes_from_winget_progress_bars() {
        assert_eq!(bytes_of("██████████████▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒  2.00 MB / 4.00 MB"), Some((2 << 20, 4 << 20)));
        assert_eq!(bytes_of("████████  512 KB / 1.00 MB"), Some((512 << 10, 1 << 20)));
        assert_eq!(bytes_of("Successfully verified installer hash"), None);
    }

    /// Every tool installs on a PC without winget too: from a release on GitHub, pinned by its SHA-256,
    /// whose program (and what it needs beside it) Glint knows how to take out.
    #[test]
    fn every_tool_installs_without_winget() {
        for tool in TOOLS {
            let release = &tool.release;
            assert!(release.url.starts_with("https://github.com/") && release.url.contains("/releases/download/"), "{}'s release is on GitHub", tool.id);
            assert!(release.url.contains(release.version), "{}'s address is its version's", tool.id);
            assert!(release.sha256.len() == 64 && release.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{}'s SHA-256", tool.id);
            let program = release.url.ends_with(".exe");
            assert!(program || release.url.ends_with(".zip") || release.url.ends_with(".tar.gz"), "{}'s download is a program, a zip or a tarball", tool.id);
            assert_eq!(release.member.is_empty(), program, "{}: an archive names its program, a program needs no name", tool.id);
            assert!(release.member.is_empty() || release.member.ends_with(".exe"), "{}'s member is a program", tool.id);
            assert!(release.with.iter().all(|m| !program && m.ends_with(".exe")), "{}'s other files come from its archive", tool.id);
        }
    }

    /// The installer's Extra tools page (installer/tools.nsh) lists every tool that doesn't come with
    /// Glint, with its program and package folder as here, plus instant search and the meaning model.
    #[test]
    fn the_installer_offers_every_tool() {
        let rows: Vec<Vec<&str>> = include_str!("../installer/tools.nsh")
            .lines()
            .filter_map(|line| line.trim().strip_prefix("!insertmacro ${ROW} "))
            .map(|row| {
                // index id "Name" group kind exe package \
                let (start, rest) = row.split_once(" \"").expect("a name in quotes");
                let (_, rest) = rest.split_once("\" ").expect("a name in quotes");
                start.split(' ').chain(rest.split(' ').filter(|word| *word != "\\")).collect()
            })
            .collect();
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row[0], i.to_string(), "{}'s index", row[1]);
        }
        let mut offered: Vec<(&str, &str, &str, &str)> = rows.iter().map(|row| (row[1], row[3], row[4], row[5])).collect();
        let mut expected = vec![("instant-search", "service", "\"\"", "\"\""), ("meaning", "model", "\"\"", "\"\"")];
        expected.extend(TOOLS.iter().filter(|tool| !tool.bundled).map(|tool| (tool.id, "tool", tool.exe, tool.package)));
        offered.sort();
        expected.sort();
        assert_eq!(offered, expected, "installer/tools.nsh's rows (id, kind, program, package) differ from TOOLS");
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
        unpack(&zip_path, &[("difft-1.0\\difft.exe", "difft.exe")], &dir.join("difftastic")).unwrap();
        assert_eq!(fs::read(dir.join("difftastic").join("difft.exe")).unwrap(), b"the program");
        assert!(!dir.join("difftastic").join("old.txt").exists(), "the old copy's folder was replaced");
        assert!(!dir.join("evil.exe").exists() && !std::env::temp_dir().join("evil.exe").exists());
        assert!(unpack(&zip_path, &[("missing.exe", "x.exe")], &dir.join("x")).is_err());
        assert!(!dir.join("x.new").exists(), "a folder that couldn't be filled is removed");
        // A program and what it needs beside it (FFmpeg's ffprobe), and nothing else.
        let ffmpeg = dir.join("ffmpeg.zip");
        let mut zip = zip::ZipWriter::new(File::create(&ffmpeg).unwrap());
        for name in ["ffmpeg", "ffprobe", "ffplay"] {
            zip.start_file(format!("ffmpeg-9.0/bin/{name}.exe"), options).unwrap();
            zip.write_all(name.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        unpack(&ffmpeg, &[("ffmpeg-9.0/bin/ffmpeg.exe", "ffmpeg.exe"), ("ffmpeg-9.0/bin/ffprobe.exe", "ffprobe.exe")], &dir.join("ffmpeg")).unwrap();
        assert_eq!(fs::read(dir.join("ffmpeg").join("ffmpeg.exe")).unwrap(), b"ffmpeg");
        assert_eq!(fs::read(dir.join("ffmpeg").join("ffprobe.exe")).unwrap(), b"ffprobe");
        assert!(!dir.join("ffmpeg").join("ffplay.exe").exists());
        // A release that's the program itself (jaq's) goes in the same way.
        fs::write(dir.join("jaq-x86_64-pc-windows-msvc.exe"), b"jaq").unwrap();
        place(&dir.join("jaq-x86_64-pc-windows-msvc.exe"), &dir.join("difftastic"), "jaq").unwrap();
        assert_eq!(fs::read(dir.join("difftastic").join("jaq.exe")).unwrap(), b"jaq");
        assert!(!dir.join("difftastic").join("difft.exe").exists());
        // A tarball (onefetch's): "./onefetch.exe" in it, gzip-compressed.
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(11);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "./onefetch.exe", &b"one fetched"[..]).unwrap();
        let mut gz = flate2::write::GzEncoder::new(File::create(dir.join("onefetch-win.tar.gz")).unwrap(), flate2::Compression::default());
        gz.write_all(&tar.into_inner().unwrap()).unwrap();
        gz.finish().unwrap();
        unpack_tarball(&dir.join("onefetch-win.tar.gz"), &[("onefetch.exe", "onefetch.exe")], &dir.join("onefetch")).unwrap();
        assert_eq!(fs::read(dir.join("onefetch").join("onefetch.exe")).unwrap(), b"one fetched");
        assert!(unpack_tarball(&dir.join("onefetch-win.tar.gz"), &[("other.exe", "y.exe")], &dir.join("y")).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Every pinned release downloads, matches its SHA-256 and unpacks to its program. About 400 MB, so
    /// only when asked, after changing a pin: cargo test --lib -- --ignored pinned_releases
    #[test]
    #[ignore]
    fn pinned_releases_download_and_unpack() {
        let dir = std::env::temp_dir().join(format!("glint-releases-{}", std::process::id()));
        set_dir(dir.join("tools"));
        let quiet = Channel::new(|_| Ok(()));
        let failed: Vec<String> = TOOLS
            .iter()
            .filter_map(|tool| {
                let stop = AtomicBool::new(false);
                let program = folder_of(tool.id).map(|folder| folder.join(format!("{}.exe", tool.exe)));
                match tauri::async_runtime::block_on(download(tool, &stop, &quiet)) {
                    Ok(()) if program.is_some_and(|p| p.is_file()) => None,
                    Ok(()) => Some(format!("{}: no {}.exe after unpacking", tool.id, tool.exe)),
                    Err(e) => Some(format!("{}: {e}", tool.id)),
                }
            })
            .collect();
        let _ = fs::remove_dir_all(&dir);
        assert!(failed.is_empty(), "{failed:#?}");
    }
}
