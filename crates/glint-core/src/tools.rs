//! Optional tools: programs Glint drives for a feature or two but doesn't ship, so the download stays
//! small for everyone who never uses those features. Each installs when asked, on any PC: with winget
//! where the PC has it and winget has the tool, else (or when winget can't) from the tool's own release
//! on GitHub, pinned by its SHA-256 and checked before anything is unpacked. Their arguments are built in
//! Rust from typed options, like rg's and fd's. (pdu's engine is built in: sizes.rs.)

use crate::fsx::blocking;
use crate::proc::{self, Lanes, Origin};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use crate::host::ipc::Channel;
use crate::host::State;

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
            "10.5.2",
            "https://github.com/sharkdp/fd/releases/download/v10.5.2/fd-v10.5.2-x86_64-pc-windows-msvc.zip",
            "7edac75ef96b7e9a01ebb251a21ee86ea365a09adcfb865f1ec3346a906153b6",
            "fd-v10.5.2-x86_64-pc-windows-msvc/fd.exe",
        ),
        used_for: "Finding files by name where instant search doesn't",
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
    // Its CPU build: ggml picks the build of its CPU code that this PC's processor runs fastest.
    Tool {
        id: "whisper",
        name: "whisper.cpp",
        exe: "whisper-cli",
        package: "whisper",
        bundled: false,
        winget: "",
        release: Release {
            version: "b5454",
            url: "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip",
            sha256: "6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb",
            member: "Release/whisper-cli.exe",
            with: &[
                "Release/whisper.dll",
                "Release/ggml.dll",
                "Release/ggml-base.dll",
                "Release/ggml-cpu-x64.dll",
                "Release/ggml-cpu-sse42.dll",
                "Release/ggml-cpu-sandybridge.dll",
                "Release/ggml-cpu-haswell.dll",
                "Release/ggml-cpu-skylakex.dll",
                "Release/ggml-cpu-cannonlake.dll",
                "Release/ggml-cpu-cascadelake.dll",
                "Release/ggml-cpu-icelake.dll",
                "Release/ggml-cpu-alderlake.dll",
            ],
        },
        used_for: "Subtitles for videos and recordings (Media), heard on this PC",
    },
    Tool {
        id: "jaq",
        name: "jaq",
        exe: "jaq",
        package: "01mf02.jaq",
        bundled: false,
        winget: "",
        release: release(
            "3.1.2",
            "https://github.com/01mf02/jaq/releases/download/v3.1.2/jaq-x86_64-pc-windows-msvc.exe",
            "8159fcd68008e0dbaeca5c48afcf89acbbf15ccec36d1de9b1bd8658d07143ed",
            "",
        ),
        used_for: "jq filters over JSON, YAML and TOML files, in their preview (jq, rewritten in Rust)",
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
        id: "rclone",
        name: "rclone",
        exe: "rclone",
        package: "rclone.rclone",
        bundled: false,
        winget: "Rclone.Rclone",
        release: release(
            "1.76.0",
            "https://github.com/rclone/rclone/releases/download/v1.76.0/rclone-v1.76.0-windows-amd64.zip",
            "8b4fd2737ade4e70c057ef4a49deb7cffe94b0fb9d7587c189d149823f21a1dc",
            "rclone-v1.76.0-windows-amd64/rclone.exe",
        ),
        used_for: "Google Drive, OneDrive, Dropbox and 60 more clouds as folders, signed in to from Glint, and folders kept in sync with them by Folder rules",
    },
    Tool {
        id: "pdfcpu",
        name: "pdfcpu",
        exe: "pdfcpu",
        package: "pdfcpu",
        bundled: false,
        winget: "",
        release: release(
            "0.16.4",
            "https://github.com/pdfcpu/pdfcpu/releases/download/v0.16.4/pdfcpu_0.16.4_Windows_x86_64.zip",
            "faf6c51d6f54d96a8c1ad5beee02143460b8ae97bd9630cbab1a253adaa43558",
            "pdfcpu_0.16.4_Windows_x86_64/pdfcpu.exe",
        ),
        used_for: "PDF tools: merging, splitting, turning and taking out pages, making PDFs smaller, their pictures, passwords and stamps",
    },
    Tool {
        id: "duckdb",
        name: "DuckDB",
        exe: "duckdb",
        package: "duckdb.cli",
        bundled: false,
        winget: "",
        release: release(
            "2.0.0",
            "https://github.com/duckdb/duckdb/releases/download/v2.0.0/duckdb-cli-windows-amd64.tar.gz",
            "91663c2f36c2a334b0e5e639006d8c29c29e013af0c477d80bb931df68870312",
            "duckdb.exe",
        ),
        used_for: "SQL over CSV, JSON and Parquet files of any size, read where they are, and over SQLite databases and workbooks; data files queried together, their columns' profile, and converting them",
    },
    // The Windows build is Perl with ExifTool beside it: the program and its exiftool_files folder.
    // exiftool.org keeps only the newest version, so the pin points at SourceForge, which keeps them all.
    Tool {
        id: "exiftool",
        name: "ExifTool",
        exe: "exiftool",
        package: "oliverbetz.exiftool",
        bundled: false,
        winget: "",
        release: Release {
            version: "13.61",
            url: "https://downloads.sourceforge.net/project/exiftool/exiftool-13.61_64.zip",
            sha256: "ab75b37515385a753aa60a58f68ffffe1ff81e43b319df9540978fb19e0580e5",
            member: "exiftool-13.61_64/exiftool(-k).exe",
            with: &["exiftool-13.61_64/exiftool_files/"],
        },
        used_for: "Taking the place, the camera, names and history out of photos, videos, PDFs and Office files before they're shared, and editing when a photo was taken",
    },
    // winget's installs it for every user (UAC); the release's installer is a 7z self-extractor whose
    // payload Glint unpacks itself: 7z.exe and the 7z.dll that reads every format.
    Tool {
        id: "7zip",
        name: "7-Zip",
        exe: "7z",
        package: "7zip.7zip",
        bundled: false,
        winget: "",
        release: Release {
            version: "26.06",
            url: "https://github.com/ip7z/7zip/releases/download/26.06/7z2606-x64.exe",
            sha256: "9b697f65ecc2d9b0152f5ee96d72dde522a7827690349d9307935dacf40f42ec",
            member: "7z.exe",
            with: &["7z.dll"],
        },
        used_for: "Disk images and installers opened like folders: ISO, VHD and VHDX, VMDK, DMG, WIM, ext4 and APFS images, installers, Debian and RPM packages",
    },
    Tool {
        id: "pandoc",
        name: "pandoc",
        exe: "pandoc",
        package: "johnmacfarlane.pandoc",
        bundled: false,
        winget: "JohnMacFarlane.Pandoc",
        release: release(
            "3.13",
            "https://github.com/jgm/pandoc/releases/download/3.13/pandoc-3.13-windows-x86_64.zip",
            "23e1f2e978a3df80db8bf65c9c59534f41a23140480cfb4f783a882ab7dc6a4e",
            "pandoc-3.13/pandoc.exe",
        ),
        used_for: "Convert document: Word, OpenDocument, Markdown, HTML, EPUB, LaTeX, RTF and more into one another, run sandboxed",
    },
    Tool {
        id: "typst",
        name: "Typst",
        exe: "typst",
        package: "typst.typst",
        bundled: false,
        winget: "Typst.Typst",
        release: release(
            "0.15.1",
            "https://github.com/typst/typst/releases/download/v0.15.1/typst-x86_64-pc-windows-msvc.zip",
            "19ce3551153c2fe7ee9fa2f95208310c8f4d3209fedb699e0333faf8913f6736",
            "typst-x86_64-pc-windows-msvc/typst.exe",
        ),
        used_for: "Convert document's PDFs: what pandoc writes, typeset without LaTeX or Word",
    },
    Tool {
        id: "restic",
        name: "restic",
        exe: "restic",
        package: "restic.restic",
        bundled: false,
        winget: "restic.restic",
        release: release(
            "0.19.1",
            "https://github.com/restic/restic/releases/download/v0.19.1/restic_0.19.1_windows_amd64.zip",
            "da948ad707ed690426473aaba2046cd61f8f90f6f0e7dab6be0d5796531de67d",
            "restic_0.19.1_windows_amd64.exe",
        ),
        used_for: "Back up: folders copied to a drive, a network folder or a cloud, encrypted, each backup adding only what changed, browsed in Previous versions",
    },
];

pub fn get(id: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.id == id)
}

pub fn by_exe(exe: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.exe.eq_ignore_ascii_case(exe))
}

/// A shell or runtime Glint uses when it is on this PC's PATH, and never downloads, updates or removes: the
/// ones people install for themselves. Without it the feature that wants it says how to get it and carries on.
pub struct Runtime {
    pub id: &'static str,
    pub name: &'static str,
    /// Its file name, without `.exe`.
    pub exe: &'static str,
    /// Its winget package, for the hint that says how to install it.
    pub winget: &'static str,
}

pub static RUNTIMES: &[Runtime] = &[
    // Folder history shared with the terminals' `z` (Settings → History).
    Runtime { id: "zoxide", name: "zoxide", exe: "zoxide", winget: "ajeetdsouza.zoxide" },
    // A justfile's recipes in the Run menu.
    Runtime { id: "just", name: "just", exe: "just", winget: "Casey.Just" },
    // Python scripts and projects.
    Runtime { id: "uv", name: "uv", exe: "uv", winget: "astral-sh.uv" },
    // TypeScript and JavaScript scripts.
    Runtime { id: "bun", name: "Bun", exe: "bun", winget: "Oven-sh.Bun" },
    // Nushell scripts, and a shell for the terminal.
    Runtime { id: "nushell", name: "Nushell", exe: "nu", winget: "Nushell.Nushell" },
];

pub fn runtime(id: &str) -> Option<&'static Runtime> {
    RUNTIMES.iter().find(|r| r.id == id)
}

/// A runtime found on this PC's PATH (or where winget and Scoop put programs), ready to run without a console
/// window; an error that says how to install it otherwise.
pub fn runtime_command(id: &str) -> Result<Command, String> {
    let runtime = runtime(id).ok_or_else(|| format!("No runtime is called {id}"))?;
    let (exe, _) = proc::locate(runtime.exe).ok_or_else(|| not_installed(runtime))?;
    let mut command = Command::new(exe);
    proc::no_window(&mut command);
    Ok(command)
}

/// What to say of a runtime that isn't on this PC: Glint doesn't download it, so how to get it.
fn not_installed(runtime: &Runtime) -> String {
    format!("{} isn't installed. Glint uses the one on your PATH: winget install {}", runtime.name, runtime.winget)
}

/// Glint's own tools folder: `tools` in the app's local data folder, per user. Set at start.
static DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

/// Glint's data folder for a process of its own that isn't the app (the MCP server), as the app's is:
/// so its downloaded tools, models and indexes are found there too.
pub fn init_outside_app() {
    if DIR.get().is_none() {
        if let Some(data) = std::env::var_os("LOCALAPPDATA").and_then(|d| Some(PathBuf::from(d).join(identifier()?))) {
            set_dir(data.join("tools"));
        }
    }
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

#[derive(Clone, Debug)]
pub struct ToolStatus {
    pub id: &'static str,
    pub name: &'static str,
    pub used_for: &'static str,
    /// What Install tries first: always "release", the tool's own pinned download (winget only when that
    /// can't be had; see [`install`]). Kept for the window's hint.
    pub source: &'static str,
    /// Where the copy that runs came from: "bundled", "downloaded" or "installed"; None when it's missing.
    pub origin: Option<&'static str>,
    pub path: Option<String>,
    pub version: Option<String>,
    /// The version its release download gets.
    pub pinned: Option<&'static str>,
    /// The version an update would bring: set only for a copy Glint downloaded whose version is known and
    /// older than the pinned one. A version Glint can't read never offers an update (it used to, forever).
    pub update: Option<&'static str>,
    /// Glint downloaded this copy, so it can remove it.
    pub removable: bool,
}

/// The file in a downloaded tool's folder that says which release Glint put there, so its version is known
/// without running it (some don't say theirs to `--version`, and the first run of a new download can be slow
/// while Windows' antivirus looks at it).
const VERSION_FILE: &str = "glint-version.txt";

fn status(tool: &'static Tool) -> ToolStatus {
    let found = proc::locate(tool.exe);
    let pinned = tool.release.version;
    let origin = found.as_ref().map(|(_, origin)| match origin {
        Origin::Bundled => "bundled",
        Origin::Downloaded => "downloaded",
        Origin::Installed => "installed",
    });
    let downloaded = matches!(found, Some((_, Origin::Downloaded)));
    let version = found.as_ref().and_then(|(path, origin)| {
        let recorded = matches!(origin, Origin::Downloaded).then(|| recorded_version(path)).flatten();
        recorded.or_else(|| version_of(path))
    });
    let update = (downloaded && version.as_deref().is_some_and(|v| older(v, pinned))).then_some(pinned);
    ToolStatus {
        id: tool.id,
        name: tool.name,
        used_for: tool.used_for,
        source: "release",
        origin,
        version,
        path: found.map(|(path, _)| path.display().to_string()),
        pinned: Some(pinned),
        update,
        removable: downloaded,
    }
}

/// The version [`VERSION_FILE`] beside a downloaded program says, if Glint wrote one.
fn recorded_version(program: &Path) -> Option<String> {
    let text = fs::read_to_string(program.with_file_name(VERSION_FILE)).ok()?;
    let version = text.trim();
    (!version.is_empty() && version.len() < 40).then(|| version.to_string())
}

/// Whether version `have` is older than `pinned`, comparing their numbers in order ("0.9.10" < "0.10.0",
/// whisper.cpp's "b5000" < "b5454"). Versions without numbers are only told apart by being different.
fn older(have: &str, pinned: &str) -> bool {
    let numbers = |v: &str| -> Vec<u64> { v.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect() };
    let (a, b) = (numbers(have), numbers(pinned));
    if a.is_empty() || b.is_empty() {
        return have != pinned;
    }
    a < b
}

/// The tools Glint downloaded whose pinned release is newer, by the [`VERSION_FILE`] beside each: nothing is run,
/// so it's cheap enough for a check a while after start (the suggestion to update them). Copies downloaded before
/// 0.13 wrote that file are left to Settings → Tools, which asks them.
pub fn updates_ready() -> Vec<&'static str> {
    TOOLS
        .iter()
        .filter(|tool| {
            let Some(folder) = folder_of(tool.id) else { return false };
            let program = folder.join(format!("{}.exe", tool.exe));
            program.is_file() && recorded_version(&program).is_some_and(|v| older(&v, tool.release.version))
        })
        .map(|tool| tool.name)
        .collect()
}

/// Every tool, where it runs from and its version (each asked at once, so a slow one doesn't hold the rest).
pub async fn tools_status() -> Result<Vec<ToolStatus>, String> {
    blocking(|| Ok(std::thread::scope(|scope| TOOLS.iter().map(|tool| scope.spawn(move || status(tool))).collect::<Vec<_>>().into_iter().filter_map(|h| h.join().ok()).collect()))).await
}

/// What a program is asked to say its version: `--version` for most. restic and pdfcpu have a `version`
/// command (both answer `--version` with "unknown flag"), ExifTool's is `-ver` (`--version` prints its manual),
/// and 7-Zip shows its banner when run with nothing (`--version` is an "Unknown switch" with no banner).
fn version_args(exe: &str) -> &'static [&'static str] {
    match exe.to_ascii_lowercase().as_str() {
        "restic" | "pdfcpu" => &["version"],
        "exiftool" => &["-ver"],
        "7z" => &[],
        _ => &["--version"],
    }
}

/// The version a program says it is (asked as [`version_args`] says for it); None when it doesn't answer within
/// a few seconds. An answer is kept for as long as the program file stays the same, so opening Settings again
/// runs nothing.
pub fn version_of(path: &Path) -> Option<String> {
    type Seen = HashMap<PathBuf, (u64, Option<std::time::SystemTime>, String)>;
    static SEEN: std::sync::LazyLock<Mutex<Seen>> = std::sync::LazyLock::new(Mutex::default);
    let meta = fs::metadata(path).ok()?;
    let stamp = (meta.len(), meta.modified().ok());
    if let Some((len, modified, version)) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        if (*len, *modified) == stamp {
            return Some(version.clone());
        }
    }
    let exe = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let version = ask_version(path, version_args(&exe))?;
    SEEN.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf(), (stamp.0, stamp.1, version.clone()));
    Some(version)
}

/// Runs `path` with `args` and reads the first version number it prints; None after 8 seconds without an answer
/// (the first run of a program just downloaded can wait on the antivirus).
fn ask_version(path: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new(path);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    proc::no_window(&mut command);
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + Duration::from_secs(8);
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
#[derive(Clone, Debug)]
pub struct Progress {
    pub stage: String,
    pub done: u64,
    pub total: u64,
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

/// Installs or updates a tool (or puts a bundled one back): from the tool's own release, else, when that can't be
/// had, with winget.
pub async fn tool_install(id: String, on_progress: Channel<Progress>, installs: State<'_, Installs>) -> Result<ToolStatus, String> {
    install(&id, on_progress, &installs).await
}

pub(crate) async fn install(id: &str, on_progress: Channel<Progress>, installs: &Installs) -> Result<ToolStatus, String> {
    let tool = get(id).ok_or_else(|| format!("No tool is called {id}"))?;
    // The ways to get it, in order (None is its release). The release first: the version Glint is tested with,
    // checked against its SHA-256, for this account alone (no administrator prompt, no winget source to refresh),
    // and Glint knows what it put there. winget went first until 0.12 and made most of the trouble people wrote
    // about: slow, now and then a prompt, a newer version than Glint knows, and a copy on PATH that the one Glint
    // downloaded earlier stood in front of, so Update never ended. winget is tried only when the release can't be
    // had (a network that blocks GitHub), and never over a copy Glint downloaded, which would still stand in front.
    let downloaded = blocking(move || Ok(matches!(proc::locate(tool.exe), Some((_, Origin::Downloaded))))).await?;
    let mut ways: Vec<Option<PathBuf>> = vec![None];
    if !tool.bundled && !downloaded && !tool.winget.is_empty() {
        ways.extend(winget().map(Some));
    }
    let mut failed = Vec::new();
    for (i, way) in ways.into_iter().enumerate() {
        if i > 0 {
            let _ = on_progress.send(Progress { stage: "The download didn't work, so Glint tries winget".into(), done: 0, total: 0 });
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
    proc::forget_locations();
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
                // winget's own lines name addresses and package ids ("Downloading https://github.com/…"): the
                // window says what is happening in words, and the line is kept for an error.
                let stage = if text.starts_with("Downloading") { "Downloading with winget" } else { "Installing with winget" };
                let _ = progress.send(Progress { stage: stage.into(), done: 0, total: 0 });
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
    let folder = into.clone();
    let file = temp.clone();
    let unpacked = blocking(move || {
        let program = format!("{}.exe", tool.exe);
        let files: Vec<(&str, &str)> = std::iter::once((release.member, program.as_str())).chain(release.with.iter().map(|m| (*m, m.rsplit('/').next().unwrap_or(m)))).collect();
        if release.member.is_empty() {
            place(&file, &into, tool.exe)
        } else if release.url.ends_with(".tar.gz") {
            unpack_tarball(&file, &files, &into)
        } else if release.url.ends_with(".exe") {
            unpack_sfx(&file, &files, &into)
        } else {
            unpack(&file, &files, &into)
        }
    })
    .await;
    let _ = fs::remove_file(&temp);
    unpacked?;
    // Which release is there, so its version is known without running it ([`VERSION_FILE`]).
    let _ = fs::write(folder.join(VERSION_FILE), release.version);
    Ok(())
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

/// The TLS provider Glint's HTTP clients use (ring), installed for the process; installing it twice is harmless.
/// The app's updater calls it before its own client.
pub fn install_tls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// The HTTP client Glint downloads with: the TLS the updater uses, and the PC's proxy. With `ipv4`, it
/// connects over IPv4 alone, as a network whose IPv6 is broken (connections reset, or never answered)
/// still gets through that way.
pub(crate) fn client(ipv4: bool) -> Result<reqwest::Client, String> {
    install_tls_provider();
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
            let found = crate::host::async_runtime::spawn_blocking(move || {
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
pub(crate) fn unpack(zip: &Path, files: &[(&str, &str)], into: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(File::open(zip).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fresh_folder(into, |fresh| {
        for (member, name) in files {
            // A folder (named with a "/" at its end): all of it, as it is below its name.
            if let Some(folder) = member.strip_suffix('/') {
                unpack_folder(&mut archive, folder, fresh)?;
                continue;
            }
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

/// Everything below `folder` in a zip, into `fresh\<folder's name>`, kept inside it (names that would
/// leave it are refused).
fn unpack_folder<R: Read + std::io::Seek>(archive: &mut zip::ZipArchive<R>, folder: &str, fresh: &Path) -> Result<(), String> {
    let prefix = format!("{}/", folder.replace('\\', "/").trim_end_matches('/'));
    let leaf = prefix.trim_end_matches('/').rsplit('/').next().unwrap_or("files").to_string();
    let depth = prefix.matches('/').count();
    let mut found = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Ok(name) = entry.name().map(|n| n.replace('\\', "/")) else { continue };
        let Some(rest) = name.strip_prefix(&prefix) else { continue };
        if rest.is_empty() {
            continue;
        }
        let Some(inside) = entry.enclosed_name() else { continue };
        let relative: PathBuf = inside.components().skip(depth).collect();
        let target = fresh.join(&leaf).join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = File::create(&target).map_err(|e| e.to_string())?;
        std::io::copy(&mut (&mut entry).take(MAX_DOWNLOAD), &mut out).map_err(|e| e.to_string())?;
        found += 1;
    }
    if found == 0 {
        return Err(format!("{folder} isn't in the download"));
    }
    Ok(())
}

/// The files named, `(member, name)`, out of a checked 7z self-extractor (7-Zip's own installer): the 7z
/// archive after its program, read with Glint's own 7z reader. Nothing in it runs.
fn unpack_sfx(exe: &Path, files: &[(&str, &str)], into: &Path) -> Result<(), String> {
    let bytes = fs::read(exe).map_err(|e| e.to_string())?;
    // The signature's bytes also appear in the self-extractor's own code (it looks for them): the
    // archive is at the first place they start one that opens.
    let mut reader = bytes
        .windows(6)
        .enumerate()
        .filter(|(_, w)| *w == b"7z\xBC\xAF\x27\x1C")
        .find_map(|(start, _)| sevenz_rust2::ArchiveReader::new(std::io::Cursor::new(bytes[start..].to_vec()), sevenz_rust2::Password::empty()).ok())
        .ok_or("The download holds no 7z archive Glint can open")?;
    fresh_folder(into, |fresh| {
        for (member, name) in files {
            let data = reader.read_file(member).map_err(|_| format!("{member} isn't in the download"))?;
            if data.len() as u64 > MAX_DOWNLOAD {
                return Err(format!("{member} is too big"));
            }
            fs::write(fresh.join(name), data).map_err(|e| e.to_string())?;
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

/// Puts the filled `fresh` folder where `into` is. The old copy goes aside first and is deleted after, so an update
/// never leaves half a tool: deleting it in place stopped at the first file in use (an rclone mount, a backup
/// running), with the files before it already gone. When the folder itself can't be moved, because a program in it
/// is running, its files are moved aside one by one (Windows lets a running program's file be renamed, not
/// deleted) and the fresh ones take their place. What can't be deleted yet is swept at the next start ([`sweep`]).
fn replace(into: &Path, fresh: &Path) -> Result<(), String> {
    if fs::symlink_metadata(into).is_err() {
        return fs::rename(fresh, into).map_err(|e| e.to_string());
    }
    let aside = aside_of(into);
    let _ = fs::remove_dir_all(&aside);
    if fs::rename(into, &aside).is_ok() {
        if let Err(e) = fs::rename(fresh, into) {
            let _ = fs::rename(&aside, into);
            return Err(e.to_string());
        }
        let _ = fs::remove_dir_all(&aside);
        return Ok(());
    }
    let in_use = |e: std::io::Error| format!("The copy there now is in use, so it can't be replaced yet ({e})");
    fs::create_dir_all(&aside).map_err(|e| e.to_string())?;
    let mut moved: Vec<std::ffi::OsString> = Vec::new();
    let mut step = || -> std::io::Result<()> {
        for entry in fs::read_dir(into)? {
            let name = entry?.file_name();
            fs::rename(into.join(&name), aside.join(&name))?;
            moved.push(name);
        }
        for entry in fs::read_dir(fresh)? {
            let name = entry?.file_name();
            fs::rename(fresh.join(&name), into.join(&name))?;
        }
        Ok(())
    };
    if let Err(e) = step() {
        // Back as it was: what came in goes, what went aside comes back.
        for name in &moved {
            let _ = fs::remove_dir_all(into.join(name)).or_else(|_| fs::remove_file(into.join(name)));
            let _ = fs::rename(aside.join(name), into.join(name));
        }
        let _ = fs::remove_dir_all(fresh);
        return Err(in_use(e));
    }
    let _ = fs::remove_dir(fresh);
    let _ = fs::remove_dir_all(&aside);
    Ok(())
}

/// Where [`replace`] puts a tool's old copy while the new one goes in: `tools\<id>.old`.
fn aside_of(into: &Path) -> PathBuf {
    into.with_extension("old")
}

/// Deletes what installs left in the tools folder: old copies [`replace`] couldn't delete while they ran
/// (`<id>.old`), fresh folders an install stopped filling (`<id>.new`), and downloads cut off (`.<id>-<pid>.download`).
/// Once per start, on a background thread (with [`crate::retired::sweep`]). A fresh folder or a download is only
/// taken once it's an hour old: another Glint (the setup's `tools install`) may be filling it now.
pub fn sweep() {
    if let Some(dir) = DIR.get() {
        sweep_in(dir, Duration::from_secs(3600));
    }
}

fn sweep_in(dir: &Path, settled: Duration) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        let old_enough = || entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age >= settled);
        if name.ends_with(".old") || (name.ends_with(".new") && old_enough()) {
            let _ = fs::remove_dir_all(&path);
        } else if name.starts_with('.') && name.ends_with(".download") && old_enough() {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Removes a copy Glint downloaded. One installed with winget may be the person's own, used outside
/// Glint too, so it's theirs to uninstall.
pub async fn tool_remove(id: String) -> Result<ToolStatus, String> {
    let tool = get(&id).ok_or_else(|| format!("No tool is called {id}"))?;
    blocking(move || {
        if !matches!(proc::locate(tool.exe), Some((_, Origin::Downloaded))) {
            return Err(format!("Glint didn't download {}, so it leaves it alone", tool.name));
        }
        let folder = folder_of(tool.id).ok_or("Glint's data folder isn't available")?;
        fs::remove_dir_all(&folder).map_err(|e| format!("Couldn't remove {} (is it running?): {e}", tool.name))?;
        proc::forget_locations();
        Ok(status(tool))
    })
    .await
}

/// `Glint.exe tools install <id>…`: installs tools as Settings → Tools does, for the installer's Extra
/// tools page (installer/tools.nsh) and scripted setups. One that's on this PC already is left as it is. What it's doing goes to stdout a line at a time (the installer's
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
        .filter(|id| match crate::host::async_runtime::block_on(install_here(id, &installs, status.clone())) {
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

/// Glint's local data folder (LOCALAPPDATA\<identifier>), known before the app is (startup.rs).
pub(crate) fn local_data_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(identifier()?))
}

/// The app's identifier, as its local data folder is named.
fn identifier() -> Option<String> {
    // A build with a config overlay (`tauri build --config`, a test copy's own identifier) has it here.
    if let Some(id) = option_env!("TAURI_CONFIG").and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok()).and_then(|c| c["identifier"].as_str().map(str::to_string)) {
        return Some(id);
    }
    Some(crate::host::identifier().to_string())
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
    let result = {
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
    Channel::new(move |progress: Progress| {
        *heard.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        let now = progress.stage.as_str();
        let (done, total) = (progress.done, progress.total);
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
    fn an_update_is_offered_only_for_a_known_older_version() {
        assert!(older("0.16.3", "0.16.4"));
        assert!(older("0.9.10", "0.10.0"), "by number, not by text");
        assert!(!older("0.10.0", "0.9.10"));
        assert!(!older("0.19.1", "0.19.1"));
        assert!(!older("3.14", "3.13"), "a newer copy isn't offered the older pin");
        assert!(older("b5000", "b5454"), "whisper.cpp's build numbers");
        assert!(!older("b5454", "b5454"));
        assert!(older("13.6", "13.61"), "ExifTool's minor numbers are numbers too");
    }

    #[test]
    fn asks_each_program_for_its_version_the_way_it_answers() {
        // restic and pdfcpu answer --version with "unknown flag", ExifTool prints its manual, 7-Zip says nothing.
        assert_eq!(version_args("restic"), &["version"]);
        assert_eq!(version_args("pdfcpu"), &["version"]);
        assert_eq!(version_args("exiftool"), &["-ver"]);
        assert!(version_args("7z").is_empty());
        assert_eq!(version_args("rclone"), &["--version"]);
        assert_eq!(version_in("restic 0.19.1 compiled with go1.26.4 on windows/amd64").as_deref(), Some("0.19.1"));
        assert_eq!(version_in("version: 0.16.4\n commit: 0f5ac76d").as_deref(), Some("0.16.4"));
        assert_eq!(version_in("13.61").as_deref(), Some("13.61"));
        assert_eq!(version_in("\n7-Zip 26.06 (x64) : Copyright (c) 1999-2026 Igor Pavlov : 2026-11-06").as_deref(), Some("26.06"));
    }

    #[test]
    fn a_downloaded_copy_says_its_version_from_the_file_glint_wrote() {
        let dir = std::env::temp_dir().join(format!("glint-tools-version-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let program = dir.join("restic.exe");
        fs::write(&program, b"not a program").unwrap();
        assert_eq!(recorded_version(&program), None);
        fs::write(dir.join(VERSION_FILE), "0.19.1\r\n").unwrap();
        assert_eq!(recorded_version(&program).as_deref(), Some("0.19.1"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_running_tool_is_updated_and_its_old_copy_swept_later() {
        let dir = std::env::temp_dir().join(format!("glint-tools-running-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let into = dir.join("pinger");
        fs::create_dir_all(&into).unwrap();
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        fs::copy(Path::new(&system).join(r"System32\PING.EXE"), into.join("pinger.exe")).unwrap();
        fs::write(into.join("old.txt"), "old").unwrap();
        // The old copy runs while the new one goes in, as an rclone mount or a backup would.
        let mut running = Command::new(into.join("pinger.exe")).args(["-n", "6", "127.0.0.1"]).stdout(Stdio::null()).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let fresh = dir.join("pinger.new");
        fs::create_dir_all(&fresh).unwrap();
        fs::write(fresh.join("pinger.exe"), b"the new program").unwrap();
        fs::write(fresh.join(VERSION_FILE), "2.0").unwrap();
        replace(&into, &fresh).unwrap();
        assert_eq!(fs::read(into.join("pinger.exe")).unwrap(), b"the new program");
        assert!(!into.join("old.txt").exists(), "the old copy's other files went aside too");
        assert!(!fresh.exists());
        assert!(running.try_wait().unwrap().is_none(), "the old copy kept running");
        running.kill().unwrap();
        let _ = running.wait();
        // What couldn't be deleted while it ran goes at the next start.
        sweep_in(&dir, Duration::ZERO);
        assert!(!aside_of(&into).exists());
        assert!(into.join("pinger.exe").exists(), "the new copy stays");
        let _ = fs::remove_dir_all(&dir);
    }

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
            // GitHub releases; ExifTool's own site keeps only its newest version, SourceForge keeps them all.
            let github = release.url.starts_with("https://github.com/") && release.url.contains("/releases/download/");
            let sourceforge = tool.id == "exiftool" && release.url.starts_with("https://downloads.sourceforge.net/project/exiftool/");
            assert!(github || sourceforge, "{}'s release is on GitHub", tool.id);
            assert!(release.url.contains(release.version), "{}'s address is its version's", tool.id);
            assert!(release.sha256.len() == 64 && release.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{}'s SHA-256", tool.id);
            let exe = release.url.ends_with(".exe");
            // An .exe that names a member is a 7z self-extractor (7-Zip's installer), unpacked by Glint.
            let self_extractor = exe && !release.member.is_empty();
            let program = exe && !self_extractor;
            assert!(exe || release.url.ends_with(".zip") || release.url.ends_with(".tar.gz"), "{}'s download is a program, a zip or a tarball", tool.id);
            assert_eq!(release.member.is_empty(), program, "{}: an archive names its program, a program needs no name", tool.id);
            assert!(release.member.is_empty() || release.member.ends_with(".exe"), "{}'s member is a program", tool.id);
            assert!(release.with.iter().all(|m| !program && (m.ends_with(".exe") || m.ends_with(".dll") || m.ends_with('/'))), "{}'s other files (programs, the DLLs it loads, a folder it needs) come from its archive", tool.id);
        }
    }

    /// The installer's Extra tools page (crates/glint/installer/tools.nsh) lists every tool that doesn't come with
    /// Glint, with its program and package folder as here, plus instant search.
    #[test]
    fn the_installer_offers_every_tool() {
        let rows: Vec<Vec<&str>> = include_str!("../../glint/installer/tools.nsh")
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
        let mut expected = vec![("instant-search", "service", "\"\"", "\"\"")];
        expected.extend(TOOLS.iter().filter(|tool| !tool.bundled).map(|tool| (tool.id, "tool", tool.exe, tool.package)));
        offered.sort();
        expected.sort();
        assert_eq!(offered, expected, "installer/tools.nsh's rows (id, kind, program, package) differ from TOOLS");
    }

    /// The runtimes are found on PATH only: none is also a tool Glint downloads, each says how to install it, and
    /// one that isn't there is reported with that hint rather than a download.
    #[test]
    fn the_runtimes_are_looked_for_on_path_and_never_downloaded() {
        for (i, runtime) in RUNTIMES.iter().enumerate() {
            assert!(RUNTIMES[i + 1..].iter().all(|other| other.id != runtime.id && other.exe != runtime.exe), "{} is listed twice", runtime.id);
            assert!(get(runtime.id).is_none() && by_exe(runtime.exe).is_none(), "{} is a tool too", runtime.id);
            assert!(!runtime.winget.is_empty(), "{} has no package to name", runtime.id);
        }
        assert_eq!(runtime("nushell").map(|r| r.exe), Some("nu"));
        assert!(runtime("sendme").is_none());
        // A runtime that isn't on this PC says how to get it.
        assert_eq!(
            not_installed(runtime("uv").unwrap()),
            "uv isn't installed. Glint uses the one on your PATH: winget install astral-sh.uv"
        );
        assert!(runtime_command("nothing").unwrap_err().contains("No runtime"));
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
        // A tarball (DuckDB's): "./duckdb.exe" in it, gzip-compressed.
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "./duckdb.exe", &b"duck"[..]).unwrap();
        let mut gz = flate2::write::GzEncoder::new(File::create(dir.join("duckdb-win.tar.gz")).unwrap(), flate2::Compression::default());
        gz.write_all(&tar.into_inner().unwrap()).unwrap();
        gz.finish().unwrap();
        unpack_tarball(&dir.join("duckdb-win.tar.gz"), &[("duckdb.exe", "duckdb.exe")], &dir.join("duckdb")).unwrap();
        assert_eq!(fs::read(dir.join("duckdb").join("duckdb.exe")).unwrap(), b"duck");
        assert!(unpack_tarball(&dir.join("duckdb-win.tar.gz"), &[("other.exe", "y.exe")], &dir.join("y")).is_err());
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
        let only = std::env::var("TOOLS").ok();
        let failed: Vec<String> = TOOLS
            .iter()
            .filter(|tool| only.as_deref().is_none_or(|ids| ids.split(',').any(|id| id == tool.id)))
            .filter_map(|tool| {
                let stop = AtomicBool::new(false);
                let program = folder_of(tool.id).map(|folder| folder.join(format!("{}.exe", tool.exe)));
                match crate::host::async_runtime::block_on(download(tool, &stop, &quiet)) {
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

#[cfg(test)]
mod sfx_probe {
    /// 7-Zip's installer (SFX names it) unpacked as Settings → Tools does: 7z.exe and 7z.dll out of it.
    #[test]
    #[ignore]
    fn unpacks_seven_zips_installer() {
        let path = std::path::PathBuf::from(std::env::var("SFX").unwrap());
        let into = std::env::temp_dir().join(format!("glint-sfx-{}", std::process::id()));
        super::unpack_sfx(&path, &[("7z.exe", "7z.exe"), ("7z.dll", "7z.dll")], &into).unwrap();
        let dll = std::fs::metadata(into.join("7z.dll")).unwrap().len();
        let exe = std::fs::metadata(into.join("7z.exe")).unwrap().len();
        println!("7z.exe {exe} bytes, 7z.dll {dll} bytes");
        assert!(exe > 100_000 && dll > 1_000_000);
        let _ = std::fs::remove_dir_all(&into);
    }
}
