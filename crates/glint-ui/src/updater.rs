//! Glint keeping itself up to date: latest.json, the signed installer, the Update button. First a port of
//! src/lib/updater.ts and the updater plugin Tauri ran for it; since 0.14 it updates the way browsers do, so that
//! nobody has to think about it.
//!
//! **Finding a new version**
//!
//! - Looks at start, then every hour by the wall clock ([`CHECK_EVERY`]), sooner after a look that failed
//!   ([`RETRY_AFTER`]), and again when the window comes back after a while away ([`LOOK_AGAIN_AFTER_AWAY`]). The
//!   clock is read every few minutes ([`TICK`]) instead of trusting one long timer: Windows' timers don't count
//!   time asleep, so the 12-hour timer before 0.14 hardly ever fired in a Glint that lived in the tray, and a new
//!   version waited for the next start.
//! - Keeps looking once it has found one: a newer release takes the place of the one offered so far.
//! - Reads the latest release's `latest.json` ([`MANIFEST_URL`]); if that can't be read, GitHub's latest-release API.
//!   Only the setup program's copy takes the installer from it.
//!
//! **Getting it, by itself** (Settings → About → Install updates automatically, on by default)
//!
//! - The setup program's copy downloads the setup program in the background as soon as it finds it, picking up
//!   where an interrupted download stopped (HTTP ranges), and not on a metered connection. Its minisign signature
//!   is checked with the key the releases are signed with ([`PUBLIC_KEY`], the version it was signed for included)
//!   **before anything runs it**, and again right before it runs. It waits, checked, in Glint's own folder
//!   (`ready.json` beside it), so a later start still has it.
//! - Then it says "Update ready" once, with Restart now. Otherwise it installs **when Glint quits** (silently, no
//!   restart), or, after a start in the tray (with Windows), **before anyone opens the window** (silently, back in
//!   the tray). Restart now asks what quitting asks (a copy running, Compare's edits), saves, stops the tools and
//!   runs the setup program with its progress showing, which starts Glint again (`/P /UPDATE /R`, read by the
//!   installer: crates/glint/installer).
//! - Nothing installs by itself while another Glint process is running that this one didn't start (an AI agent's
//!   MCP server), or while Windows signs out or shuts down.
//! - A copy Scoop installed updates through Scoop (`scoop update glint` once Glint has quit, then Glint starts
//!   again: [`scoop`]), so Scoop and what reads it (UniGetUI) know the new version. A Scoop copy beside the setup
//!   program's copy is brought along after the setup's copy updated itself.
//! - The Microsoft Store's copy never updates itself (the Store does), and neither does a debug build ([`Policy`]).
//!   A portable copy says a new version exists and links to it.
//!
//! **Telling the person.** Notices go to the main window. While it is hidden (in the tray, or minimized) they wait
//! and show when it is back, and Windows' own notification says it meanwhile (for the setup program's copy, which
//! has the app identity its shortcuts carry). The header's button shows once there is something to do: Update, or
//! Restart to update once the download is in.
//!
//! **A secondary window** (`platform::secondary`) never looks, greets, installs or writes the marks: the main
//! process updates Glint. The model exists there (Settings → About reads it) and stays idle.
//!
//! **Testing without a network.** The network is a [`Source`], what the updater asks of Windows (starting the
//! installer or the Scoop helper, other Glint processes, a session ending) is a [`Launcher`], and quitting and the
//! questions before it are [`Hooks`]; the tests give the model fakes of all three, and a clock of their own.
//! [`HttpSource`] is the real source (reqwest over rustls, only ever on core's runtime, never the UI thread).
//!
//! **How the coordinator wires it** (startup.rs): [`init`] early, before the first state write; a subscription
//! that hands every [`UpdaterEvent`] to [`show`] in the main window; [`window_active`] from the workspace when a
//! window comes to the front; [`on_quit`] from `on_app_quit`; [`Updater::set_auto`] when the setting changes; and
//! [`update_button`] in the toolbar.

mod scoop;

use crate::kit::prelude::*;
use crate::{core_bridge, persistence, t, theme};
use minisign_verify::{Error as MinisignError, PublicKey, Signature};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::OsString;
use std::future::Future;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---- The feed and the key ---------------------------------------------------------------------------------

/// The update manifest of the latest release: `plugins.updater.endpoints` of tauri.conf.json.
pub const MANIFEST_URL: &str = "https://github.com/shadijhade/glint/releases/latest/download/latest.json";

/// GitHub's latest-release answer, asked when the manifest can't be used (`LATEST_RELEASE_API`).
pub const LATEST_RELEASE_API: &str = "https://api.github.com/repos/shadijhade/glint/releases/latest";

/// The releases page (`RELEASES`): a version's notes are at `…/tag/v<version>`.
pub const RELEASES: &str = "https://github.com/shadijhade/glint/releases";

/// The minisign public key the releases are signed with, base64 as Tauri's config held it (`plugins.updater.pubkey`).
/// The instant search service trusts the same key (crates/shadui-index/update-pubkey.txt); a test compares them.
pub const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IENFMzYxRjUwQTI4MTYxQzcKUldUSFlZR2lVQjgyenRlb0tmR1RMMEprZWxKdTNHZVhON0hNU1haSGNUYzdSY0czYkt0dXJLaXgK";

/// How often Glint looks, by the wall clock (time asleep counts).
pub const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);

/// After a look that failed (offline, or just woken with the network not back yet), the next one comes sooner.
pub const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

/// How often the clock is read. Windows' timers don't count time asleep (since Windows 8), so one long timer would
/// wait for hours of time awake; a short one wakes nothing that matters.
pub const TICK: Duration = Duration::from_secs(5 * 60);

/// Coming back to the window after this long away looks again.
pub const LOOK_AGAIN_AFTER_AWAY: Duration = Duration::from_secs(30 * 60);

/// After a start in the tray, an update waits this long before installing itself (start-up settles first).
const SETTLE: Duration = Duration::from_secs(20);

/// A Scoop copy beside this one that is still behind is tried again after this long (Scoop's bucket follows a
/// release by minutes, and Scoop skips an app that is running).
const SIBLING_AGAIN: Duration = Duration::from_secs(6 * 60 * 60);

/// The updater targets latest.json may list, most specific first (the plugin's `{os}-{arch}-{bundle}` then
/// `{os}-{arch}`).
const TARGETS: [&str; 2] = ["windows-x86_64-nsis", "windows-x86_64"];

/// No Glint setup program is anywhere near this big; a longer download is cut off.
const MAX_INSTALLER: u64 = 1 << 30;

/// What versions before 0.14 called the folders of their downloads in %TEMP%, which [`sweep`] still clears away.
const WORK_PREFIX: &str = "glint-update-";

/// How old such a folder is before a start removes it.
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// A download broken off is picked up again for this long; an older part is removed.
const PART_KEPT: Duration = Duration::from_secs(3 * 24 * 60 * 60);

/// The record of the checked setup program waiting in the updates folder.
const READY_FILE: &str = "ready.json";

// ---- Versions ---------------------------------------------------------------------------------------------

/// A version number as releases write them: dotted numbers, an optional `-pre.release`, an ignored `+build`.
/// Compared the way semver does, so `0.10.0 > 0.9.5` and `1.0.0-beta < 1.0.0` (the plugin's comparison).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    numbers: Vec<u64>,
    pre: Vec<String>,
}

impl Version {
    /// Reads `1.2.3`, `v1.2.3`, `1.2.3-rc.1`, `1.2.3+build`. `None` for anything else, an empty part included.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text
            .strip_prefix('v')
            .or_else(|| text.strip_prefix('V'))
            .unwrap_or(text);
        let text = text.split('+').next().unwrap_or("");
        let (numbers, pre) = match text.split_once('-') {
            Some((numbers, pre)) => (numbers, Some(pre)),
            None => (text, None),
        };
        let numbers = numbers
            .split('.')
            .map(|part| {
                (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| part.parse::<u64>().ok())
                    .flatten()
            })
            .collect::<Option<Vec<u64>>>()?;
        let pre = match pre {
            Some("") => return None,
            Some(pre) => pre.split('.').map(str::to_string).collect(),
            None => Vec::new(),
        };
        if pre.iter().any(|part| part.is_empty()) {
            return None;
        }
        Some(Version { numbers, pre })
    }

    /// Whether this is a release (no `-pre` part), which is all GitHub's fallback accepts.
    pub fn is_release(&self) -> bool {
        self.pre.is_empty()
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let length = self.numbers.len().max(other.numbers.len());
        for i in 0..length {
            let a = self.numbers.get(i).copied().unwrap_or(0);
            let b = other.numbers.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                Ordering::Equal => {}
                different => return different,
            }
        }
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            // A pre-release comes before its release.
            (false, true) => Ordering::Less,
            (true, false) => Ordering::Greater,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(a), Ok(b)) => a.cmp(&b),
                        // Numbers come before words.
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        (Err(_), Err(_)) => a.cmp(b),
                    };
                    if order != Ordering::Equal {
                        return order;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        }
    }
}

// ---- What the feed says -----------------------------------------------------------------------------------

/// A newer version: what the Update button, Settings → About and the notices speak of (`AvailableUpdate`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Available {
    /// "0.10.0", without a leading `v`.
    pub version: String,
    /// The release page: what's new, and the installer for copies that don't update themselves.
    pub url: String,
    /// The manifest's notes when they are words (Glint's own are the release page's address, so usually `None`).
    pub notes: Option<String>,
    /// Set when this copy can install it itself: where the setup program is and its signature.
    pub installer: Option<InstallerRef>,
}

impl Available {
    /// The release page of `version`.
    fn release_page(version: &str) -> String {
        format!("{RELEASES}/tag/v{version}")
    }
}

/// The setup program a manifest names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallerRef {
    /// Where to download it (https only).
    pub url: String,
    /// Its minisign signature as the manifest holds it (base64 of the signature file).
    pub signature: String,
}

/// latest.json (`scripts/release.mjs` writes it): the static format with `platforms`, or the dynamic one with
/// `url` and `signature` at the top, as the plugin reads both.
#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    platforms: Option<HashMap<String, Platform>>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    signature: Option<String>,
}

#[derive(Deserialize)]
struct Platform {
    url: String,
    signature: String,
}

/// A release read from latest.json.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: Option<String>,
    /// None when the manifest has nothing for this PC's target.
    pub installer: Option<InstallerRef>,
}

/// Reads latest.json. Errors say what is wrong with it; [`find`] treats any of them as "ask GitHub instead".
pub fn parse_manifest(text: &str) -> Result<Release, String> {
    let manifest: Manifest = serde_json::from_str(text).map_err(|e| format!("latest.json isn't readable: {e}"))?;
    let version = manifest.version.trim().trim_start_matches('v').to_string();
    if Version::parse(&version).is_none() {
        return Err(format!("latest.json has no usable version ({})", manifest.version));
    }
    let platform = manifest.platforms.as_ref().and_then(|platforms| {
        TARGETS
            .iter()
            .find_map(|target| platforms.get(*target))
            .map(|p| (p.url.clone(), p.signature.clone()))
    });
    let dynamic = manifest.url.clone().zip(manifest.signature.clone());
    let installer = platform
        .or(dynamic)
        .filter(|(url, signature)| url.starts_with("https://") && !signature.trim().is_empty())
        .map(|(url, signature)| InstallerRef {
            url,
            signature: signature.trim().to_string(),
        });
    Ok(Release {
        version,
        notes: manifest.notes.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
        installer,
    })
}

/// What the notes say when they are words: the manifest's `notes` is usually just the release page's address,
/// which "What's new" opens, so an address isn't shown as text.
pub fn notes_text(notes: &str) -> Option<&str> {
    let notes = notes.trim();
    (!notes.is_empty() && !notes.starts_with("http://") && !notes.starts_with("https://")).then_some(notes)
}

/// GitHub's latest-release answer as an [`Available`] when it is newer than `current` (`findUpdate`'s fallback):
/// the tag must be plain dotted numbers, and the update has no installer, only the release page.
fn from_github(text: &str, current: &Version) -> Result<Option<Available>, String> {
    #[derive(Deserialize)]
    struct Latest {
        #[serde(default)]
        tag_name: Option<String>,
        #[serde(default)]
        html_url: Option<String>,
    }
    let latest: Latest = serde_json::from_str(text).map_err(|e| format!("GitHub's answer isn't readable: {e}"))?;
    let tag = latest.tag_name.unwrap_or_default();
    let version = tag.strip_prefix('v').unwrap_or(&tag).to_string();
    let newer = Version::parse(&version).is_some_and(|v| v.is_release() && !version.contains('+') && v > *current);
    Ok(newer.then(|| Available {
        url: latest.html_url.unwrap_or_else(|| Available::release_page(&version)),
        version,
        notes: None,
        installer: None,
    }))
}

// ---- Where the answers come from --------------------------------------------------------------------------

/// A question to the network, answered later.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// A piece of a download, as [`Source::download`] hands them over.
#[derive(Debug)]
pub enum Piece<'a> {
    /// The download starts at byte `from` of the file: 0, or where an earlier try stopped when the server took up
    /// from there. The whole file is `total` bytes when the server said.
    Start { from: u64, total: Option<u64> },
    /// The next bytes.
    Bytes(&'a [u8]),
}

/// Takes a download's pieces as they arrive. An `Err` stops the download and is its failure.
pub type Sink = Box<dyn FnMut(Piece<'_>) -> Result<(), String> + Send>;

/// What the updater asks of the network. [`HttpSource`] is the real one; tests give a fake with canned answers.
pub trait Source: Send + Sync + 'static {
    /// latest.json's text; `None` when the feed says there is nothing newer (HTTP 204).
    fn manifest(&self) -> Fut<Option<String>>;
    /// GitHub's latest-release answer as text.
    fn latest_release(&self) -> Fut<String>;
    /// Streams the file at `url` into `sink`, asking for it from byte `from` (where an earlier try stopped). The
    /// first piece is always a [`Piece::Start`] saying where it really starts.
    fn download(&self, url: String, from: u64, sink: Sink) -> Fut<()>;
    /// Whether the connection costs by the byte (a phone's data, roaming, over a limit): nothing big is downloaded
    /// unasked then.
    fn metered(&self) -> bool {
        false
    }
}

/// Looks for a newer version than `current`: the manifest first, then GitHub's latest release. Only the setup
/// program's copy takes the manifest's installer; the others are sent to the release page or to Scoop. A manifest
/// that can't be fetched or read falls through to GitHub like the TypeScript's `check()` throwing; a manifest that
/// says "nothing newer" does not.
pub async fn find(source: &dyn Source, policy: Policy, current: &Version) -> Result<Option<Available>, String> {
    match source.manifest().await {
        Ok(None) => return Ok(None),
        Ok(Some(text)) => {
            if let Ok(release) = parse_manifest(&text) {
                let newer = Version::parse(&release.version).is_some_and(|v| v > *current);
                return Ok(newer.then(|| Available {
                    url: Available::release_page(&release.version),
                    version: release.version,
                    notes: release.notes,
                    installer: release.installer.filter(|_| policy == Policy::SelfUpdating),
                }));
            }
        }
        Err(_) => {}
    }
    from_github(&source.latest_release().await?, current)
}

/// Whether a failure reads as "no network" (`/fetch|network|sending request|dns|timed? ?out|connect/i`), which
/// is said plainly instead of in the library's words.
pub fn looks_offline(error: &str) -> bool {
    let error = error.to_lowercase();
    [
        "fetch",
        "network",
        "sending request",
        "dns",
        "timeout",
        "timed out",
        "time out",
        "connect",
    ]
    .iter()
    .any(|word| error.contains(word))
}

/// The real source: GitHub over rustls, with the PC's proxy.
pub struct HttpSource {
    tls: fn(),
}

impl HttpSource {
    /// `install_tls` installs rustls' process-wide crypto provider (the `ring` one, as glint-core's downloads do:
    /// `let _ = rustls::crypto::ring::default_provider().install_default();`). reqwest is built with
    /// `rustls-no-provider`, and glint-ui does not depend on rustls itself, so the coordinator passes it in.
    pub fn new(install_tls: fn()) -> Self {
        Self { tls: install_tls }
    }

    fn client(&self, for_download: bool) -> Result<reqwest::Client, String> {
        (self.tls)();
        let mut builder = reqwest::Client::builder()
            .user_agent(concat!("Glint/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15));
        builder = if for_download {
            // A stalled download ends; a slow one does not.
            builder.read_timeout(Duration::from_secs(60))
        } else {
            builder.timeout(Duration::from_secs(30))
        };
        // Without a provider rustls panics while building; that is a setup mistake to say, not to crash on.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| builder.build()))
            .map_err(|_| "Secure connections aren't set up in this build".to_string())?
            .map_err(|e| e.to_string())
    }

    async fn get_text(&self, url: &str, what: &str) -> Result<Option<String>, String> {
        let response = self
            .client(false)?
            .get(url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = response.status();
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(answered(what, status));
        }
        response.text().await.map(Some).map_err(|e| e.to_string())
    }
}

/// "GitHub answered 404 Not Found".
fn answered(what: &str, status: reqwest::StatusCode) -> String {
    format!(
        "{what} answered {} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    )
    .trim()
    .to_string()
}

/// Where a 206 answer starts and the whole file's size, from its `Content-Range: bytes <start>-<end>/<total>`.
fn content_range(response: &reqwest::Response) -> Option<(u64, Option<u64>)> {
    let value = response.headers().get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let start = range.split_once('-')?.0.trim().parse().ok()?;
    Some((start, total.trim().parse().ok()))
}

impl Source for HttpSource {
    fn manifest(&self) -> Fut<Option<String>> {
        let this = Self { tls: self.tls };
        Box::pin(async move { this.get_text(MANIFEST_URL, "GitHub").await })
    }

    fn latest_release(&self) -> Fut<String> {
        let this = Self { tls: self.tls };
        Box::pin(async move {
            this.get_text(LATEST_RELEASE_API, "GitHub")
                .await?
                .ok_or_else(|| "GitHub answered with nothing".to_string())
        })
    }

    fn download(&self, url: String, from: u64, mut sink: Sink) -> Fut<()> {
        let this = Self { tls: self.tls };
        Box::pin(async move {
            if !url.starts_with("https://") {
                return Err("The update isn't offered over a secure connection".into());
            }
            let client = this.client(true)?;
            let mut request = client.get(&url);
            if from > 0 {
                request = request.header(reqwest::header::RANGE, format!("bytes={from}-"));
            }
            let mut response = request.send().await.map_err(|e| e.to_string())?;
            let mut status = response.status();
            // What was kept is not part of this file any more (or all of it): the whole file again.
            if from > 0 && status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                response = client.get(&url).send().await.map_err(|e| e.to_string())?;
                status = response.status();
            }
            if !status.is_success() {
                return Err(answered("The download", status));
            }
            let start = match (status, content_range(&response)) {
                // Taken up exactly where the last try stopped.
                (reqwest::StatusCode::PARTIAL_CONTENT, Some((start, total))) if start == from => Piece::Start {
                    from,
                    total: total.or_else(|| response.content_length().map(|rest| rest + from)),
                },
                (reqwest::StatusCode::PARTIAL_CONTENT, _) => {
                    return Err("The server sent another part of the update than asked".into());
                }
                _ => Piece::Start {
                    from: 0,
                    total: response.content_length(),
                },
            };
            sink(start)?;
            while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
                sink(Piece::Bytes(&chunk))?;
            }
            Ok(())
        })
    }

    fn metered(&self) -> bool {
        metered_connection()
    }
}

/// Windows' word on the internet connection's cost (`NetworkInformation`): a fixed or variable plan, roaming, or
/// over or near its limit counts as metered. Asked on a thread of its own, in the Windows Runtime's
/// multithreaded apartment; anything that can't be read counts as not metered.
fn metered_connection() -> bool {
    std::thread::spawn(|| {
        use windows::Networking::Connectivity::{NetworkCostType, NetworkInformation};
        use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
        // SAFETY: initialises the Windows Runtime for this short-lived thread only.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        let Ok(profile) = NetworkInformation::GetInternetConnectionProfile() else {
            return false;
        };
        let Ok(cost) = profile.GetConnectionCost() else {
            return false;
        };
        let plan = cost.NetworkCostType().unwrap_or(NetworkCostType::Unknown);
        matches!(plan, NetworkCostType::Fixed | NetworkCostType::Variable)
            || cost.Roaming().unwrap_or(false)
            || cost.OverDataLimit().unwrap_or(false)
            || cost.ApproachingDataLimit().unwrap_or(false)
    })
    .join()
    .unwrap_or(false)
}

/// The real source, ready for [`init`].
pub fn http_source(install_tls: fn()) -> Arc<dyn Source> {
    Arc::new(HttpSource::new(install_tls))
}

// ---- Checking the signature -------------------------------------------------------------------------------

/// Why a downloaded setup program was refused. None of these runs anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The public key built into this copy can't be read.
    BadKey,
    /// The manifest's signature can't be read.
    BadSignature,
    /// Signed by a different key than the one the releases use.
    WrongKey,
    /// The signature doesn't belong to these bytes: changed, cut short or corrupted.
    Mismatch,
    /// The signature doesn't say which version it was made for, and a signed version is required.
    NoSignedVersion,
    /// It was signed for another version than the manifest announced (an older release's file under a newer
    /// number).
    WrongVersion { signed: String, announced: String },
    /// The downloaded file can't be read, or isn't a Windows program.
    Unreadable(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::BadKey => write!(f, "This copy of Glint has no usable update key"),
            Refusal::BadSignature => write!(f, "The update's signature can't be read, so it wasn't installed"),
            Refusal::WrongKey => write!(f, "The update wasn't signed by Glint's key, so it wasn't installed"),
            Refusal::Mismatch => write!(f, "The download doesn't match its signature, so it wasn't installed"),
            Refusal::NoSignedVersion => write!(
                f,
                "The update's signature doesn't name its version, so it wasn't installed"
            ),
            Refusal::WrongVersion { signed, announced } => write!(
                f,
                "The update was signed as version {signed} but announced as {announced}, so it wasn't installed"
            ),
            Refusal::Unreadable(why) => write!(f, "{why}"),
        }
    }
}

/// Standard base64 (padding optional, whitespace ignored), which is how Tauri stores minisign keys and signatures.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut bits = 0u32;
    let mut count = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push(((bits >> count) & 0xff) as u8);
        }
    }
    Some(out)
}

/// A key or signature as the manifest holds it: base64 of the minisign file's text.
fn minisign_text(base64: &str) -> Option<String> {
    String::from_utf8(base64_decode(base64)?).ok()
}

/// The signature's trusted comment has to name the version the manifest announced (`requireSignedVersion`):
/// otherwise a changed manifest could pair a new version number with an older release's signed file. Read only
/// after the signature itself verified, because it is the signature's global part that covers the comment.
fn check_signed_version(trusted_comment: &str, announced: &str) -> Result<(), Refusal> {
    let Some(signed) = trusted_comment
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
    else {
        return Err(Refusal::NoSignedVersion);
    };
    let same = match (
        Version::parse(signed.trim_start_matches('v')),
        Version::parse(announced.trim_start_matches('v')),
    ) {
        (Some(signed), Some(announced)) => signed == announced,
        _ => signed == announced,
    };
    if same {
        Ok(())
    } else {
        Err(Refusal::WrongVersion {
            signed: signed.to_string(),
            announced: announced.to_string(),
        })
    }
}

fn refusal(error: MinisignError) -> Refusal {
    match error {
        MinisignError::UnexpectedKeyId => Refusal::WrongKey,
        MinisignError::InvalidSignature => Refusal::Mismatch,
        MinisignError::IoError(e) => Refusal::Unreadable(format!("Couldn't read the download: {e}")),
        _ => Refusal::BadSignature,
    }
}

/// Checks that `data` is what `signature` (base64, as latest.json holds it) was made for with `public_key`
/// (base64, [`PUBLIC_KEY`]'s form), and for version `announced`.
#[cfg(test)]
pub fn verify_bytes(data: &[u8], signature: &str, public_key: &str, announced: &str) -> Result<(), Refusal> {
    let (key, signature) = decode_pair(public_key, signature)?;
    key.verify(data, &signature, true).map_err(refusal)?;
    check_signed_version(signature.trusted_comment(), announced)
}

fn decode_pair(public_key: &str, signature: &str) -> Result<(PublicKey, Signature), Refusal> {
    let key = minisign_text(public_key)
        .and_then(|text| PublicKey::decode(&text).ok())
        .ok_or(Refusal::BadKey)?;
    let signature = minisign_text(signature)
        .and_then(|text| Signature::decode(&text).ok())
        .ok_or(Refusal::BadSignature)?;
    Ok((key, signature))
}

/// [`verify_bytes`] for a file on disk, read in pieces (a setup program is tens of megabytes). A signature made
/// the old, un-hashed way can't be streamed, so such a file is read whole.
pub fn verify_file(path: &Path, signature: &str, public_key: &str, announced: &str) -> Result<(), Refusal> {
    let unreadable = |e: std::io::Error| Refusal::Unreadable(format!("Couldn't read the download: {e}"));
    let (key, signature) = decode_pair(public_key, signature)?;
    let mut file = std::fs::File::open(path).map_err(unreadable)?;
    match key.verify_stream(&signature) {
        Ok(mut verifier) => {
            let mut buffer = vec![0u8; 256 * 1024];
            loop {
                let read = file.read(&mut buffer).map_err(unreadable)?;
                if read == 0 {
                    break;
                }
                verifier.update(&buffer[..read]);
            }
            verifier.finalize().map_err(refusal)?;
        }
        Err(MinisignError::UnsupportedLegacyMode) => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(unreadable)?;
            key.verify(&bytes, &signature, true).map_err(refusal)?;
        }
        Err(other) => return Err(refusal(other)),
    }
    check_signed_version(signature.trusted_comment(), announced)?;
    // A Windows program starts with "MZ" (the plugin's `infer::app::is_exe`): nothing else is ever run.
    let mut head = [0u8; 2];
    let mut file = std::fs::File::open(path).map_err(unreadable)?;
    match file.read_exact(&mut head) {
        Ok(()) if &head == b"MZ" => Ok(()),
        _ => Err(Refusal::Unreadable("The download isn't a Windows program".into())),
    }
}

// ---- Running the installer --------------------------------------------------------------------------------

/// NSIS reads `/x` as its own switch, so an argument with a slash, space or tab is quoted; the rest is the
/// C runtime's quoting (the plugin's `escape_nsis_current_exe_arg`, from the standard library's).
fn escape_nsis_arg(arg: &[u16]) -> Vec<u16> {
    let quote = arg.is_empty()
        || arg
            .iter()
            .any(|&c| c == u16::from(b' ') || c == u16::from(b'\t') || c == u16::from(b'/'));
    let mut out = Vec::with_capacity(arg.len() + 2);
    if quote {
        out.push(u16::from(b'"'));
    }
    let mut backslashes = 0usize;
    for &c in arg {
        if c == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if c == u16::from(b'"') {
                // n+1 backslashes ahead of an inner quote make 2n+1.
                out.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(c);
    }
    if quote {
        // n backslashes ahead of the closing quote make 2n.
        out.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
        out.push(u16::from(b'"'));
    }
    out
}

/// What happens once the setup program (or Scoop) is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relaunch {
    /// Restart now: the setup program shows its progress, then Glint starts with its window. No folders are
    /// passed (a `--tray` start would come back hidden): the tabs come back as Glint keeps them.
    Window,
    /// After a start in the tray: silent, and Glint starts again in the tray, as it was.
    Tray,
    /// Glint was quitting: silent, and nothing starts.
    No,
}

/// The setup program's command line for `relaunch`: passive (`/P`, a progress bar) or silent (`/S`), as an update
/// (`/UPDATE`, which hooks.nsh and the installer template treat as "keep everything"), then `/R /ARGS …` when Glint
/// starts again. The spelling and order are the Tauri plugin's, which the installer template reads.
pub fn installer_parameters(relaunch: Relaunch) -> Vec<u16> {
    match relaunch {
        Relaunch::Window => command_line("/P", Some(&[])),
        Relaunch::Tray => command_line("/S", Some(&[wide("--tray")])),
        Relaunch::No => command_line("/S", None),
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// `<mode> /UPDATE`, then `/R /ARGS <args>` when Glint starts again, each argument quoted the way NSIS wants.
fn command_line(mode: &str, restart: Option<&[Vec<u16>]>) -> Vec<u16> {
    let mut parts = vec![wide(mode), wide("/UPDATE")];
    if let Some(args) = restart {
        parts.push(wide("/R"));
        parts.push(wide("/ARGS"));
        parts.extend(args.iter().map(|arg| escape_nsis_arg(arg)));
    }
    parts.join(&u16::from(b' '))
}

/// What the updater asks of Windows. [`Windows`] is the real one; tests record what it would have done.
pub trait Launcher: Send + Sync + 'static {
    /// Runs `installer` with `parameters` (UTF-16, no terminator) and returns once Windows has started it.
    fn launch(&self, installer: &Path, parameters: &[u16]) -> Result<(), String>;
    /// Starts `program` with `args` in `dir`, with no window and apart from Glint, so that it outlives it (the
    /// Scoop helper).
    fn spawn_hidden(&self, program: &Path, args: &[OsString], dir: &Path) -> Result<(), String>;
    /// Whether another process runs this same Glint.exe that this one didn't start (an AI agent's MCP server):
    /// the setup program would close it, so nothing installs by itself meanwhile.
    fn others_running(&self) -> bool;
    /// Whether Windows is signing out or shutting down: no installer starts then.
    fn session_ending(&self) -> bool;
}

/// What the updater asks of Windows, for real.
pub struct Windows;

impl Launcher for Windows {
    /// Starts the setup program with `ShellExecuteW`, as the plugin did.
    fn launch(&self, installer: &Path, parameters: &[u16]) -> Result<(), String> {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::{w, PCWSTR};
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
        let file: Vec<u16> = installer.as_os_str().encode_wide().chain(Some(0)).collect();
        let parameters: Vec<u16> = parameters.iter().copied().chain(Some(0)).collect();
        // SAFETY: both strings are NUL-terminated and outlive the call; ShellExecuteW copies what it needs.
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(file.as_ptr()),
                PCWSTR(parameters.as_ptr()),
                PCWSTR::null(),
                SW_SHOW,
            )
        };
        // Values up to 32 are errors (it returns an HINSTANCE for old reasons).
        let code = result.0 as isize;
        if code > 32 {
            return Ok(());
        }
        Err(match code {
            2 | 3 => "The downloaded installer is gone".to_string(),
            5 => "Windows didn't let the installer start (was its administrator prompt declined?)".to_string(),
            _ => format!("Windows couldn't start the installer (code {code})"),
        })
    }

    fn spawn_hidden(&self, program: &Path, args: &[OsString], dir: &Path) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let spawn = |flags: u32| {
            Command::new(program)
                .args(args)
                .current_dir(dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(flags)
                .spawn()
        };
        // Out of any job Glint was started in (one that ends its processes with it), when the job allows that.
        spawn(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
            .or_else(|_| spawn(CREATE_NO_WINDOW))
            .map(|_| ())
            .map_err(|e| format!("Couldn't start the update's helper: {e}"))
    }

    fn others_running(&self) -> bool {
        other_glints()
    }

    fn session_ending(&self) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SHUTTINGDOWN};
        // SAFETY: reads a system metric; no pointers involved.
        unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
    }
}

/// Whether a process other than this one and the helpers it started (the sizing and document workers) runs this
/// same Glint.exe. Another install's Glint.exe (a Scoop copy) doesn't count: the setup program leaves it alone.
fn other_glints() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(name) = exe.file_name().map(|name| name.to_string_lossy().to_lowercase()) else {
        return false;
    };
    let me = std::process::id();
    // SAFETY: a snapshot of the processes, read entry by entry into a correctly sized struct and closed here.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return false;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = false;
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more && !found {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let file = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
            if file == name && entry.th32ProcessID != me && entry.th32ParentProcessID != me {
                found = runs(entry.th32ProcessID, &exe);
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

/// Whether process `pid` runs the program at `exe`.
fn runs(pid: u32, exe: &Path) -> bool {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    // SAFETY: the handle is opened and closed here; the buffer and its length outlive the call.
    let read = unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let read =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        read
    };
    if !read {
        return false;
    }
    let theirs = PathBuf::from(String::from_utf16_lossy(&buffer[..len as usize]));
    match (std::fs::canonicalize(&theirs), std::fs::canonicalize(exe)) {
        (Ok(a), Ok(b)) => a == b,
        _ => theirs.to_string_lossy().eq_ignore_ascii_case(&exe.to_string_lossy()),
    }
}

// ---- The model --------------------------------------------------------------------------------------------

/// Whether and how this copy updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// The setup program installed it: it downloads, checks and installs updates itself.
    SelfUpdating,
    /// Scoop installed it for this user: Scoop updates it, started by Glint once it has quit ([`scoop`]), so
    /// Scoop's record of the version stays right.
    Scoop,
    /// A portable copy, or one Scoop installed for all users: it says a newer version exists and links to it.
    LinkOnly,
    /// The Microsoft Store's copy: the Store keeps it up to date, so it never looks.
    Store,
    /// A debug build: it never updates.
    Off,
}

impl Policy {
    /// This copy's policy: the Store's first (`storeCopy`), then debug builds, then by how it was installed.
    pub fn detect() -> Policy {
        if glint_core::store_copy() {
            Policy::Store
        } else if cfg!(debug_assertions) {
            Policy::Off
        } else if glint_core::installed_by_setup() {
            Policy::SelfUpdating
        } else if std::env::current_exe()
            .ok()
            .and_then(|exe| scoop::this_copy(&exe))
            .is_some()
        {
            Policy::Scoop
        } else {
            Policy::LinkOnly
        }
    }

    /// Whether this copy installs an update itself (the setup program, or Scoop for it).
    pub fn installs(self) -> bool {
        matches!(self, Policy::SelfUpdating | Policy::Scoop)
    }
}

/// How the last check went (`UpdaterState.status`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing asked yet, or an update was found (see [`Updater::available`]).
    Idle,
    /// Looking.
    Checking,
    /// Nothing newer exists.
    Latest,
    /// The check failed, with the reason.
    Failed(String),
    /// The Microsoft Store keeps this copy up to date.
    Store,
    /// A debug build, which never updates.
    Off,
}

/// How far installing has got (`UpdaterState.progress`, and what follows the download).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// Not installing.
    None,
    /// Downloading: `received` bytes of `total`, when the server said.
    Downloading { received: u64, total: Option<u64> },
    /// Checking the signature.
    Verifying,
    /// Downloaded and checked: it waits for a restart, for Glint to quit, or for the questions before quitting.
    Ready,
    /// Saving, stopping the tools and starting the setup program (or handing over to Scoop).
    Launching,
}

impl Install {
    /// Whether the Update button spins: work is going on.
    pub fn working(&self) -> bool {
        matches!(
            self,
            Install::Downloading { .. } | Install::Verifying | Install::Launching
        )
    }

    /// The percent downloaded, when the total is known.
    pub fn percent(&self) -> Option<u8> {
        match self {
            Install::Downloading { received, total } => total.map(|total| percent(*received, total)),
            _ => None,
        }
    }
}

/// `floor(100 * received / total)`, at most 100.
fn percent(received: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    u8::try_from((received.saturating_mul(100) / total).min(100)).unwrap_or(100)
}

/// What a notice about a newer version offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    /// Install: it downloads with its progress showing, then Glint restarts into it.
    Install,
    /// Update: Scoop updates Glint once it has quit, then Glint starts again.
    Scoop,
    /// Download: the release page, for a copy that can't install it.
    Download,
}

/// What the model tells the app. [`show`] words them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdaterEvent {
    /// A check nobody asked for found a version it hasn't told about yet, which isn't coming in by itself:
    /// "Update available" (`firstNotice("update-notice")`).
    Found { version: String, offer: Offer },
    /// Downloaded and checked. `waiting`: it came in the background and waits ("Update ready", with Restart now);
    /// otherwise Glint restarts into it right away.
    Ready { version: String, waiting: bool },
    /// The first start after an update: "Updated to Glint X" with What's new (`greetUpdate`).
    Updated { version: String },
    /// The download the person asked for goes on: `percent` when the size is known.
    Downloading { version: String, percent: Option<u8> },
    /// The setup program (or Scoop) is starting and Glint is about to quit.
    Installing { version: String },
    /// Installing didn't work. `url` is the release page, for downloading it by hand.
    Failed { reason: String, url: String },
}

/// The question before quitting: given what to do on yes.
pub type ConfirmQuit = Rc<dyn Fn(&mut App, Box<dyn FnOnce(&mut App)>)>;

/// How the model reaches the rest of the app. All run on the UI thread.
#[derive(Clone)]
pub struct Hooks {
    /// Asks what quitting asks (a copy still running, Compare's edits) and runs the callback only on yes; runs it
    /// at once when nothing is in the way. startup.rs's `ask_before_quitting` is this, with its final step passed
    /// in instead of `quit_now`.
    pub confirm_quit: ConfirmQuit,
    /// Quits Glint (startup.rs's `quit`: the window's place is kept, then GPUI quits).
    pub quit: Rc<dyn Fn(&mut App)>,
    /// Whether something is going on that a restart nobody asked for would cut short (a copy or move, Compare's
    /// edits): an update then waits instead of installing itself.
    pub busy: Rc<dyn Fn(&App) -> bool>,
}

impl Default for Hooks {
    fn default() -> Self {
        Hooks {
            confirm_quit: Rc::new(|cx, go| go(cx)),
            quit: Rc::new(|cx| cx.quit()),
            busy: Rc::new(|_| false),
        }
    }
}

/// Everything about this copy the model needs, so tests can set it.
#[derive(Clone, Debug)]
pub struct Config {
    /// This build's version (`CARGO_PKG_VERSION`).
    pub current: String,
    pub policy: Policy,
    /// Where downloaded setup programs wait, with `ready.json`, and where the Scoop helper and its files go:
    /// `%LOCALAPPDATA%\<id>\updates` (a temporary folder in tests).
    pub work_dir: PathBuf,
    /// `%TEMP%`, where versions before 0.14 left the folders of their downloads.
    pub legacy_dir: Option<PathBuf>,
    /// The small file that remembers the last version run and the last one told about; none, nothing is kept.
    pub marks: Option<PathBuf>,
    /// Whether this person ran Glint before (saved settings exist), so an update from a version that kept no
    /// version of its own is still greeted.
    pub returning: bool,
    /// Started in the tray (`--tray`: with Windows).
    pub started_in_tray: bool,
    /// The minisign public key downloads are checked with: [`PUBLIC_KEY`] in the app, a throwaway key in tests.
    pub public_key: String,
    /// This copy, when Scoop installed it ([`Policy::Scoop`]).
    pub scoop: Option<scoop::Copy>,
    /// A Scoop copy beside this one (the setup program's), brought along after this one updates itself.
    pub sibling: Option<scoop::Copy>,
    /// This process, which the Scoop helper waits for.
    pub pid: u32,
    /// The wall clock, where time asleep counts; tests turn a clock of their own.
    pub clock: fn() -> SystemTime,
}

impl Config {
    /// This copy, for the app. Read it before the first state write: `returning` looks for the saved state.
    pub fn for_app(started_in_tray: bool) -> Config {
        let policy = Policy::detect();
        let exe = std::env::current_exe().ok();
        let exe_name = exe
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Glint.exe".into());
        Config {
            current: env!("CARGO_PKG_VERSION").to_string(),
            policy,
            work_dir: glint_core::host::PathResolver
                .app_local_data_dir()
                .map(|dir| dir.join("updates"))
                .unwrap_or_else(|_| std::env::temp_dir().join("glint-updates")),
            legacy_dir: Some(std::env::temp_dir()),
            marks: glint_core::host::PathResolver
                .app_data_dir()
                .ok()
                .map(|dir| dir.join("updater.json")),
            returning: persistence::state_path().is_some_and(|path| path.exists()),
            started_in_tray,
            public_key: PUBLIC_KEY.to_string(),
            scoop: (policy == Policy::Scoop)
                .then(|| exe.as_deref().and_then(scoop::this_copy))
                .flatten(),
            sibling: (policy == Policy::SelfUpdating)
                .then(|| scoop::installed("glint", &exe_name))
                .flatten(),
            pid: std::process::id(),
            clock: SystemTime::now,
        }
    }
}

/// What is remembered between runs (the TypeScript's `last-version` and `update-notice` in localStorage).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Marks {
    /// The version that ran last.
    last_version: Option<String>,
    /// The version the "Update available" (or "Update ready") notice was last shown for.
    noticed: Option<String>,
    /// The version a Scoop copy beside this one was last asked to come to, and when (seconds since 1970).
    #[serde(skip_serializing_if = "Option::is_none")]
    sibling_tried: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sibling_tried_at: Option<u64>,
    /// The version an installer (or Scoop) was last started for. Still newer than the version running at the next
    /// start, it didn't take: that version isn't installed unattended again.
    #[serde(skip_serializing_if = "Option::is_none")]
    installing: Option<String>,
    /// The version whose failed update through Scoop was last told about (told once, not at every start).
    #[serde(skip_serializing_if = "Option::is_none")]
    scoop_told: Option<String>,
}

/// The checked setup program waiting in the updates folder (`ready.json`), so that a later start still has it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadyRecord {
    version: String,
    /// Its file name in the updates folder (the name its address gives it: [`file_name`]).
    file: String,
    url: String,
    signature: String,
}

/// The update state shared by the header button, Settings → About and the notices.
pub struct Updater {
    source: Arc<dyn Source>,
    launcher: Arc<dyn Launcher>,
    hooks: Hooks,
    config: Config,
    current: Version,
    available: Option<Available>,
    status: Status,
    install: Install,
    /// Why the last install failed, shown in About.
    install_error: Option<String>,
    /// The downloaded, checked setup program.
    ready: Option<PathBuf>,
    marks: Marks,
    /// How many times the marks were saved, and the newest save on disk: writes land in order.
    marks_saved: u64,
    marks_written: Arc<Mutex<u64>>,
    /// Updates install by themselves (Settings → About): downloaded in the background, installed on quitting or
    /// after a start in the tray.
    auto: bool,
    /// The download going on was started by Glint itself: no progress notices, and it waits once it is in.
    quiet: bool,
    /// Glint restarts as soon as the download is in (Install, or Restart now while it was coming).
    restart_when_ready: bool,
    /// A version whose download was refused (its signature): it isn't fetched again by itself.
    refused: Option<String>,
    /// A version whose install was started before and didn't take: it installs again only when asked.
    failed_before: Option<String>,
    /// When Glint last looked (by the wall clock), whether that failed, and when it last downloaded by itself.
    last_check: Option<SystemTime>,
    last_failed: bool,
    last_quiet_try: Option<SystemTime>,
    /// When this run started (by the wall clock).
    started_at: SystemTime,
    /// Started in the tray, and the window not opened since: an update installs as soon as it is ready.
    untouched: bool,
    /// A later try at installing by itself is already on the clock.
    install_later: bool,
}

impl EventEmitter<UpdaterEvent> for Updater {}

impl Updater {
    /// A model that has looked at nothing yet. [`Updater::start`] begins the checking.
    pub fn new(source: Arc<dyn Source>, launcher: Arc<dyn Launcher>, hooks: Hooks, config: Config) -> Self {
        // A build whose own version can't be read counts as version 0, so any release is newer.
        let current = Version::parse(&config.current).unwrap_or(Version {
            numbers: vec![0],
            pre: Vec::new(),
        });
        let status = match config.policy {
            Policy::Store => Status::Store,
            Policy::Off => Status::Off,
            _ => Status::Idle,
        };
        let started_at = (config.clock)();
        let untouched = config.started_in_tray;
        Self {
            source,
            launcher,
            hooks,
            config,
            current,
            available: None,
            status,
            install: Install::None,
            install_error: None,
            ready: None,
            marks: Marks::default(),
            marks_saved: 0,
            marks_written: Arc::new(Mutex::new(0)),
            auto: false,
            quiet: false,
            restart_when_ready: false,
            refused: None,
            failed_before: None,
            last_check: None,
            last_failed: false,
            last_quiet_try: None,
            started_at,
            untouched,
            install_later: false,
        }
    }

    /// The newer version, once one is found.
    pub fn available(&self) -> Option<&Available> {
        self.available.as_ref()
    }

    /// How the last check went.
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// How far installing has got.
    pub fn install_state(&self) -> &Install {
        &self.install
    }

    /// Why the last install failed.
    pub fn install_error(&self) -> Option<&str> {
        self.install_error.as_deref()
    }

    /// How this copy updates.
    pub fn policy(&self) -> Policy {
        self.config.policy
    }

    /// The setup program waiting to be run, once downloaded and checked.
    pub fn downloaded(&self) -> Option<&Path> {
        self.ready.as_deref()
    }

    /// Whether updates install by themselves.
    pub fn auto(&self) -> bool {
        self.auto
    }

    /// When Glint last looked, by the wall clock.
    pub fn last_checked(&self) -> Option<SystemTime> {
        self.last_check
    }

    /// Whether the download going on is one Glint started by itself (nothing to do until it is in).
    pub fn fetching_quietly(&self) -> bool {
        self.quiet && self.install.working()
    }

    fn now(&self) -> SystemTime {
        (self.config.clock)()
    }

    /// How long ago `then` was; a clock turned back makes it long ago.
    fn since(&self, then: Option<SystemTime>) -> Option<Duration> {
        then.map(|then| self.now().duration_since(then).unwrap_or(Duration::MAX))
    }

    /// Updates install by themselves, or not (Settings → About). Turning it on starts getting an update already
    /// known.
    pub fn set_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.auto == on {
            return;
        }
        self.auto = on;
        if on && self.install == Install::None && self.ready.is_none() && self.can_fetch_quietly() {
            self.download(true, cx);
        }
        cx.notify();
    }

    /// At start: reads what was remembered, greets a first start after an update (`greetUpdate`), takes back a
    /// setup program a run before downloaded, hears how an update through Scoop went, brings a Scoop copy beside
    /// this one along, looks, and keeps looking by the clock.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        // A secondary window leaves the updating (and the marks file) to the main process.
        if crate::platform::secondary::is_secondary() {
            return;
        }
        let marks_path = self.config.marks.clone();
        let work_dir = self.config.work_dir.clone();
        let legacy_dir = self.config.legacy_dir.clone();
        let returning = self.config.returning;
        let policy = self.config.policy;
        let (current, key) = (self.current.clone(), self.config.public_key.clone());
        // The first start after the old app: its `last-version` and `update-notice` carry over, so the update from
        // it is greeted with the right version and an update notice already seen isn't shown again.
        let imported = crate::old_settings::imported().map(|old| Marks {
            last_version: old.last_version.clone(),
            noticed: old.notices.get("update-notice").cloned(),
            ..Marks::default()
        });
        cx.spawn(async move |this, cx| {
            let (marks, kept, scoop_said) = cx
                .background_spawn(async move {
                    if let Some(legacy) = &legacy_dir {
                        sweep(legacy, STALE_AFTER);
                    }
                    let saved = marks_path.as_deref().filter(|path| path.exists()).map(read_marks);
                    // The setup program's copy keeps its downloads in the folder; a Scoop copy shares it and leaves
                    // them be.
                    let kept = (policy == Policy::SelfUpdating)
                        .then(|| keep_or_clear(&work_dir, &current, &key))
                        .flatten();
                    let scoop_said = (policy == Policy::Scoop)
                        .then(|| scoop::take_outcome(&work_dir))
                        .flatten();
                    (saved.or(imported).unwrap_or_default(), kept, scoop_said)
                })
                .await;
            let started = this.update(cx, |this, cx| {
                this.marks = marks;
                this.greet(returning, cx);
                if let Some((record, path)) = kept {
                    this.restore(record, path, cx);
                }
                if let Some(said) = scoop_said {
                    this.scoop_said(said, cx);
                }
                this.bring_sibling_along(cx);
                this.check(false, cx);
            });
            if started.is_err() {
                return;
            }
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Every [`TICK`]: looks when an hour has gone by on the clock (ten minutes after a look that failed), tries
    /// again a download by itself that broke off, and installs an update nobody has looked at since a start in the
    /// tray once nothing is in the way.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let wait = if self.last_failed { RETRY_AFTER } else { CHECK_EVERY };
        if self.since(self.last_check).is_none_or(|since| since >= wait) {
            self.check(false, cx);
        }
        if self.install == Install::None
            && self.ready.is_none()
            && self.since(self.last_quiet_try).is_none_or(|since| since >= RETRY_AFTER)
            && self.can_fetch_quietly()
        {
            self.download(true, cx);
        }
        self.install_if_untouched(cx);
    }

    /// The window came to the front: nobody waits unseen any more, and after a long time away Glint looks again.
    pub fn window_shown(&mut self, cx: &mut Context<Self>) {
        self.untouched = false;
        if self
            .since(self.last_check)
            .is_some_and(|away| away >= LOOK_AGAIN_AFTER_AWAY)
        {
            self.check(false, cx);
        }
    }

    /// The first start after an update says so once, with the release's notes a click away. A fresh install has
    /// no earlier settings and says nothing; versions before 0.6 kept no version, so an update from one of them
    /// counts by its settings (`returning`). Going back to an older version (another copy of Glint on the PC) is
    /// not greeted.
    fn greet(&mut self, returning: bool, cx: &mut Context<Self>) {
        let version = self.config.current.clone();
        let before = self.marks.last_version.clone();
        let mut changed = before.as_deref() != Some(&version);
        self.marks.last_version = Some(version.clone());
        // An install started last time that left an older version running didn't take (the setup program stopped,
        // or Scoop did): that version waits for Restart now, which shows what goes wrong.
        if let Some(installing) = self.marks.installing.clone() {
            if Version::parse(&installing).is_some_and(|wanted| wanted > self.current) {
                self.failed_before = Some(installing);
            } else {
                self.marks.installing = None;
                changed = true;
            }
        }
        if changed {
            self.save_marks(cx);
        }
        let went_up = match before.as_deref().map(Version::parse) {
            Some(Some(before)) => before < self.current,
            Some(None) => true,
            None => returning,
        };
        if went_up {
            cx.emit(UpdaterEvent::Updated { version });
        }
    }

    /// At start: the setup program a run before downloaded and checked (checked again just now) is ready at once,
    /// without the network. It was told about then; the header's button and Settings say it waits.
    fn restore(&mut self, record: ReadyRecord, path: PathBuf, cx: &mut Context<Self>) {
        self.available = Some(Available {
            url: Available::release_page(&record.version),
            version: record.version,
            notes: None,
            installer: Some(InstallerRef {
                url: record.url,
                signature: record.signature,
            }),
        });
        self.ready = Some(path);
        self.install = Install::Ready;
        self.install_if_untouched(cx);
        cx.notify();
    }

    /// At start: how this copy's last update through Scoop went. A failure is said; the update is still offered.
    fn scoop_said(&mut self, outcome: scoop::Outcome, cx: &mut Context<Self>) {
        let arrived = Version::parse(&outcome.target).is_some_and(|target| self.current >= target);
        if outcome.ok || arrived || self.marks.scoop_told.as_deref() == Some(outcome.target.as_str()) {
            return;
        }
        self.marks.scoop_told = Some(outcome.target.clone());
        self.save_marks(cx);
        let said = outcome
            .message
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .map(|line| line.chars().take(240).collect::<String>());
        let reason = match said {
            Some(said) => format!("Scoop didn't update Glint: {said}"),
            None => "Scoop didn't update Glint".to_string(),
        };
        self.install_error = Some(reason.clone());
        let url = Version::parse(&outcome.target)
            .map(|_| Available::release_page(&outcome.target))
            .unwrap_or_else(|| RELEASES.to_string());
        cx.emit(UpdaterEvent::Failed { reason, url });
    }

    /// The setup program's copy: a Scoop copy beside it that is behind is brought along by Scoop, so that Scoop
    /// (and UniGetUI, which reads it) stops listing the old version. Nothing waits and nothing starts; once per
    /// version, and again after a while if it is still behind (Scoop's bucket lags the release by minutes, and
    /// Scoop skips a copy that is running).
    fn bring_sibling_along(&mut self, cx: &mut Context<Self>) {
        if !self.auto || self.config.policy != Policy::SelfUpdating {
            return;
        }
        let Some(sibling) = self.config.sibling.clone() else {
            return;
        };
        let behind = sibling
            .version
            .as_deref()
            .and_then(Version::parse)
            .is_some_and(|version| version < self.current);
        if !behind {
            return;
        }
        let now = unix_seconds(self.now());
        let tried_lately = self.marks.sibling_tried.as_deref() == Some(self.config.current.as_str())
            && self
                .marks
                .sibling_tried_at
                .is_some_and(|at| now.saturating_sub(at) < SIBLING_AGAIN.as_secs());
        if tried_lately {
            return;
        }
        self.marks.sibling_tried = Some(self.config.current.clone());
        self.marks.sibling_tried_at = Some(now);
        self.save_marks(cx);
        let job = scoop::Job::new(
            &sibling,
            &self.config.current,
            self.config.work_dir.join(scoop::SIBLING_RESULT),
        );
        let (dir, launcher) = (self.config.work_dir.clone(), self.launcher.clone());
        cx.background_spawn(async move {
            if let Err(error) = scoop::start(&dir, scoop::SIBLING_JOB, &job, &*launcher) {
                eprintln!("glint: couldn't bring Scoop's copy of Glint along: {error}");
            }
        })
        .detach();
    }

    /// Whether `version` is new for the notice, remembering that it was shown (`firstNotice`). Without storage a
    /// notice can repeat, which beats never showing it.
    fn first_notice(&mut self, version: &str, cx: &mut Context<Self>) -> bool {
        if self.marks.noticed.as_deref() == Some(version) {
            return false;
        }
        self.marks.noticed = Some(version.to_string());
        self.save_marks(cx);
        true
    }

    /// Writes the marks now, on this thread: for what must be on disk before Glint quits.
    fn save_marks_now(&mut self) {
        let (Some(path), Ok(text)) = (self.config.marks.clone(), serde_json::to_string_pretty(&self.marks)) else {
            return;
        };
        self.marks_saved += 1;
        let mut newest = locked(&self.marks_written);
        let _ = persistence::write_atomic(&path, text.as_bytes());
        *newest = self.marks_saved;
    }

    /// Whether `version` may install without anyone asking: not one whose install didn't take before.
    fn unattended(&self, version: &str) -> bool {
        self.failed_before.as_deref() != Some(version)
    }

    /// Writes the marks on a background thread. The writes land in order: an earlier snapshot finishing late never
    /// replaces a later one (a greeting's and a notice's, saved a moment apart).
    fn save_marks(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.config.marks.clone() else {
            return;
        };
        let Ok(text) = serde_json::to_string_pretty(&self.marks) else {
            return;
        };
        self.marks_saved += 1;
        let (saved, written) = (self.marks_saved, self.marks_written.clone());
        cx.background_spawn(async move {
            let mut newest = locked(&written);
            if saved > *newest {
                let _ = persistence::write_atomic(&path, text.as_bytes());
                *newest = saved;
            }
        })
        .detach();
    }

    /// Looks for a newer version, also when one is known already (a newer release takes its place). On its own (at
    /// start and by the clock) it says so once per version; asked from Settings (`manual`), the outcome shows
    /// there. Nothing happens while checking or installing, or in a copy that never updates (`checkForUpdates`).
    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if crate::platform::secondary::is_secondary() {
            return;
        }
        if self.status == Status::Checking
            || matches!(self.status, Status::Store | Status::Off)
            || self.install.working()
        {
            return;
        }
        self.status = Status::Checking;
        self.last_check = Some(self.now());
        cx.notify();
        let source = self.source.clone();
        let policy = self.config.policy;
        let current = self.current.clone();
        let finding = core_bridge::run(async move { find(&*source, policy, &current).await });
        cx.spawn(async move |this, cx| {
            let result = finding.await.and_then(|found| found);
            let _ = this.update(cx, |this, cx| this.found(result, manual, cx));
        })
        .detach();
    }

    fn found(&mut self, result: Result<Option<Available>, String>, manual: bool, cx: &mut Context<Self>) {
        match result {
            Ok(Some(found)) => {
                self.status = Status::Idle;
                self.last_failed = false;
                let newer = self
                    .available
                    .as_ref()
                    .is_none_or(|known| is_newer(&found.version, &known.version));
                if newer {
                    self.offer(found, manual, cx);
                }
            }
            Ok(None) => {
                self.status = Status::Latest;
                self.last_failed = false;
            }
            Err(reason) => {
                self.status = Status::Failed(reason);
                self.last_failed = true;
            }
        }
        cx.notify();
    }

    /// A newer version than any offered so far: it takes the place of an older one (download and all), then comes
    /// in by itself when it can (and says so once it's in), installs at once after a start in the tray (Scoop), or
    /// is told about once.
    fn offer(&mut self, found: Available, manual: bool, cx: &mut Context<Self>) {
        if let Some(old) = self.ready.take() {
            remove_download(&old);
            forget_ready(&self.config.work_dir);
        }
        self.install = Install::None;
        self.install_error = None;
        self.restart_when_ready = false;
        let version = found.version.clone();
        self.available = Some(found);
        if self.can_fetch_quietly() {
            self.download(true, cx);
        } else if self.config.policy == Policy::Scoop && self.install_if_untouched(cx) {
            // Handed to Scoop before anyone looks.
        } else if !manual && self.first_notice(&version, cx) {
            cx.emit(UpdaterEvent::Found {
                version,
                offer: self.offer_kind(),
            });
        }
    }

    /// Whether the offered update can come in without asking: updates install by themselves, this is the setup
    /// program's copy, the feed names a setup program (not one refused before), and the connection isn't metered.
    fn can_fetch_quietly(&self) -> bool {
        self.auto
            && self.config.policy == Policy::SelfUpdating
            && self.available.as_ref().is_some_and(|available| {
                available.installer.is_some() && self.refused.as_deref() != Some(available.version.as_str())
            })
            && !self.source.metered()
    }

    fn offer_kind(&self) -> Offer {
        match self.config.policy {
            Policy::SelfUpdating if self.available.as_ref().is_some_and(|a| a.installer.is_some()) => Offer::Install,
            Policy::Scoop => Offer::Scoop,
            _ => Offer::Download,
        }
    }

    /// Install, Restart now, Update: a checked download restarts into it now (after the questions before
    /// quitting); one coming in the background restarts once it's in, with its progress showing; otherwise the
    /// setup program downloads with progress and then restarts. A Scoop copy hands over to Scoop, and a copy that
    /// can't install it opens the release page (`installUpdate`).
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let Some(available) = self.available.clone() else {
            return;
        };
        match self.config.policy {
            Policy::Store | Policy::Off => return,
            Policy::Scoop if self.config.scoop.is_some() => return self.go_ahead(Relaunch::Window, cx),
            Policy::SelfUpdating if available.installer.is_some() => {}
            _ => return cx.open_url(&available.url),
        }
        match self.install {
            Install::Downloading { .. } | Install::Verifying if self.quiet => {
                self.restart_when_ready = true;
                self.quiet = false;
                cx.emit(UpdaterEvent::Downloading {
                    version: available.version,
                    percent: self.install.percent(),
                });
                cx.notify();
            }
            Install::Downloading { .. } | Install::Verifying | Install::Launching => {}
            Install::Ready if self.ready.is_some() => self.go_ahead(Relaunch::Window, cx),
            _ => {
                self.restart_when_ready = true;
                self.download(false, cx);
            }
        }
    }

    /// Downloads the offered setup program into the updates folder (`<name>.part`, picked up where an earlier try
    /// stopped), checks its signature on a background thread before anything else touches it, and keeps it there
    /// with `ready.json`. `quiet`: Glint started it, so no progress notices.
    fn download(&mut self, quiet: bool, cx: &mut Context<Self>) {
        let Some(available) = self.available.clone() else {
            return;
        };
        let Some(installer) = available.installer.clone() else {
            return;
        };
        let version = available.version.clone();
        self.quiet = quiet;
        if quiet {
            self.last_quiet_try = Some(self.now());
        }
        self.install = Install::Downloading {
            received: 0,
            total: None,
        };
        self.install_error = None;
        if !quiet {
            cx.emit(UpdaterEvent::Downloading {
                version: version.clone(),
                percent: Some(0),
            });
        }
        cx.notify();

        let key = self.config.public_key.clone();
        let work_dir = self.config.work_dir.clone();
        let name = file_name(&installer.url);
        let target = Arc::new(Mutex::new(Target::new(&work_dir, &name)));
        let from = locked(&target).kept();
        let (progress_in, progress_out) = async_channel::unbounded::<(u64, Option<u64>)>();
        let sink: Sink = {
            let target = target.clone();
            Box::new(move |piece| locked(&target).take(piece, &progress_in))
        };
        let download = core_bridge::run(self.source.download(installer.url.clone(), from, sink));
        cx.spawn(async move |this, cx| {
            while let Ok((received, total)) = progress_out.recv().await {
                if this.update(cx, |this, cx| this.progress(received, total, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let downloaded = download.await.and_then(|done| done);
            let part = locked(&target).finish();
            if let Err(reason) = downloaded {
                // What came is kept, to be picked up from there; a file over the size limit isn't.
                if reason.contains(TOO_BIG) {
                    if let Some(part) = &part {
                        remove_download(part);
                    }
                }
                let _ = this.update(cx, |this, cx| this.failed(format!("The download failed: {reason}"), cx));
                return;
            }
            let Some(part) = part else {
                let _ = this.update(cx, |this, cx| this.failed("The download was empty".into(), cx));
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.install = Install::Verifying;
                cx.notify();
            });
            let record = ReadyRecord {
                version: version.clone(),
                file: name,
                url: installer.url.clone(),
                signature: installer.signature.clone(),
            };
            let kept = cx
                .background_spawn(async move {
                    if let Err(refused) = verify_file(&part, &record.signature, &key, &record.version) {
                        remove_download(&part);
                        return Err((refused.to_string(), true));
                    }
                    keep_ready(&work_dir, &part, &record).map_err(|why| (why, false))
                })
                .await;
            let _ = this.update(cx, |this, cx| match kept {
                Ok(path) => this.now_ready(path, version, cx),
                Err((reason, refused)) => {
                    if refused {
                        this.refused = Some(version);
                    }
                    this.failed(reason, cx);
                }
            });
        })
        .detach();
    }

    /// Download progress, applied while the download is still the thing going on.
    fn progress(&mut self, received: u64, total: Option<u64>, cx: &mut Context<Self>) {
        if !matches!(self.install, Install::Downloading { .. }) {
            return;
        }
        let before = self.install.percent();
        self.install = Install::Downloading { received, total };
        if let (Some(available), false) = (&self.available, self.quiet) {
            let now = self.install.percent();
            if now != before || total.is_none() {
                cx.emit(UpdaterEvent::Downloading {
                    version: available.version.clone(),
                    percent: now,
                });
            }
        }
        cx.notify();
    }

    /// The download is in and checked: Glint restarts into it if that was asked; after a start in the tray that
    /// nobody has looked at, it installs unseen; otherwise it waits, and says so once.
    fn now_ready(&mut self, path: PathBuf, version: String, cx: &mut Context<Self>) {
        self.ready = Some(path);
        self.install = Install::Ready;
        self.quiet = false;
        if std::mem::take(&mut self.restart_when_ready) {
            cx.emit(UpdaterEvent::Ready {
                version,
                waiting: false,
            });
            cx.notify();
            return self.go_ahead(Relaunch::Window, cx);
        }
        cx.notify();
        if self.install_if_untouched(cx) {
            return;
        }
        if self.first_notice(&version, cx) {
            cx.emit(UpdaterEvent::Ready { version, waiting: true });
        }
    }

    /// Installing didn't work. A download Glint started by itself fails quietly: the update is offered as it would
    /// have been (once), and the clock tries the download again. One the person asked for says why.
    fn failed(&mut self, reason: String, cx: &mut Context<Self>) {
        let quiet = std::mem::take(&mut self.quiet);
        self.install = Install::None;
        self.ready = None;
        self.restart_when_ready = false;
        self.install_error = Some(reason.clone());
        match self.available.clone() {
            Some(available) if quiet => {
                if self.first_notice(&available.version, cx) {
                    cx.emit(UpdaterEvent::Found {
                        version: available.version,
                        offer: self.offer_kind(),
                    });
                }
            }
            available => cx.emit(UpdaterEvent::Failed {
                reason,
                url: available.map_or_else(|| RELEASES.to_string(), |a| a.url),
            }),
        }
        cx.notify();
    }

    /// Asks what quitting asks, and on yes starts the setup program (or Scoop). On no it stays [`Install::Ready`]:
    /// Install asks again without downloading again.
    fn go_ahead(&mut self, relaunch: Relaunch, cx: &mut Context<Self>) {
        let me = cx.weak_entity();
        let confirm = self.hooks.confirm_quit.clone();
        // After this update ends: a hook that has nothing to ask calls straight back into the model.
        cx.defer(move |cx| {
            confirm(
                cx,
                Box::new(move |cx| {
                    let _ = me.update(cx, |this, cx| this.launch(relaunch, cx));
                }),
            )
        });
    }

    /// Saves, stops the tools (the setup program replaces rg, fd and the sizing workers' Glint.exe, which it can't
    /// while they run), checks the file once more and runs it, or hands over to Scoop; then quits. Anything that
    /// goes wrong leaves Glint running.
    fn launch(&mut self, relaunch: Relaunch, cx: &mut Context<Self>) {
        if self.install == Install::Launching {
            return;
        }
        let Some(available) = self.available.clone() else {
            return;
        };
        type Start = Box<dyn FnOnce(&dyn Launcher) -> Result<(), (String, Option<PathBuf>)> + Send>;
        let start: Start = match self.config.policy {
            Policy::SelfUpdating => {
                let (Some(path), Some(installer)) = (self.ready.clone(), available.installer.clone()) else {
                    return;
                };
                let (key, version) = (self.config.public_key.clone(), available.version.clone());
                let parameters = installer_parameters(relaunch);
                Box::new(move |launcher| {
                    // Checked again here, because the file has waited in a folder since it was first checked.
                    verify_file(&path, &installer.signature, &key, &version)
                        .map_err(|refused| (refused.to_string(), Some(path.clone())))?;
                    launcher.launch(&path, &parameters).map_err(|why| (why, None))
                })
            }
            Policy::Scoop => {
                let Some(job) = self.scoop_job(&available.version, relaunch) else {
                    return;
                };
                let dir = self.config.work_dir.clone();
                Box::new(move |launcher| scoop::start(&dir, scoop::JOB, &job, launcher).map_err(|why| (why, None)))
            }
            _ => return,
        };
        self.install = Install::Launching;
        self.marks.installing = Some(available.version.clone());
        self.save_marks_now();
        cx.emit(UpdaterEvent::Installing {
            version: available.version.clone(),
        });
        cx.notify();
        // Settings, the history and the rest are written now, before anything can end the process.
        persistence::save_now(cx);
        let app = cx.try_global::<core_bridge::Core>().map(|core| core.app.clone());
        let stopping = core_bridge::run(async move {
            use glint_core::host::Manager as _;
            if let Some(state) = app.as_ref().and_then(|app| app.try_state::<glint_core::AppState>()) {
                let _ = glint_core::stop_tools(state).await;
            }
        });
        let launcher = self.launcher.clone();
        let quit = self.hooks.quit.clone();
        let work_dir = self.config.work_dir.clone();
        cx.spawn(async move |this, cx| {
            let _ = stopping.await;
            let started = cx.background_spawn(async move { start(&*launcher) }).await;
            match started {
                Ok(()) => {
                    cx.update(|cx| quit(cx));
                }
                Err((reason, delete)) => {
                    if let Some(path) = delete {
                        remove_download(&path);
                        forget_ready(&work_dir);
                    }
                    let _ = this.update(cx, |this, cx| this.failed(reason, cx));
                }
            }
        })
        .detach();
    }

    /// The Scoop helper's job for this copy: to `version`, waiting for this process, then starting Glint again as
    /// `relaunch` says.
    fn scoop_job(&self, version: &str, relaunch: Relaunch) -> Option<scoop::Job> {
        let copy = self.config.scoop.as_ref()?;
        let mut job = scoop::Job::new(copy, version, self.config.work_dir.join(scoop::RESULT));
        job.wait_for = self.config.pid;
        if relaunch != Relaunch::No {
            job.relaunch = Some(copy.current_exe());
        }
        if relaunch == Relaunch::Tray {
            job.relaunch_args = vec!["--tray".to_string()];
        }
        Some(job)
    }

    /// Glint is quitting (from `on_app_quit`, before the process ends): a checked update installs now, silently,
    /// starting nothing after it, and a Scoop copy hands its update to Scoop. Only when updates install by
    /// themselves, never while Windows signs out or shuts down, and never while another Glint process runs this
    /// exe. True when it started.
    pub fn install_on_quit(&mut self) -> bool {
        if crate::platform::secondary::is_secondary()
            || !self.auto
            || self.install == Install::Launching
            || self.launcher.session_ending()
            || self.launcher.others_running()
        {
            return false;
        }
        let Some(available) = self.available.clone().filter(|a| self.unattended(&a.version)) else {
            return false;
        };
        self.marks.installing = Some(available.version.clone());
        let started = match self.config.policy {
            Policy::SelfUpdating => {
                let (Some(path), Some(installer)) = (self.ready.clone(), available.installer.clone()) else {
                    return false;
                };
                if self.install != Install::Ready {
                    return false;
                }
                // Checked once more, right here: the app is going.
                match verify_file(&path, &installer.signature, &self.config.public_key, &available.version) {
                    Ok(()) => self.launcher.launch(&path, &installer_parameters(Relaunch::No)),
                    Err(refused) => {
                        remove_download(&path);
                        forget_ready(&self.config.work_dir);
                        Err(refused.to_string())
                    }
                }
            }
            Policy::Scoop => match self.scoop_job(&available.version, Relaunch::No) {
                Some(job) => scoop::start(&self.config.work_dir, scoop::JOB, &job, &*self.launcher),
                None => return false,
            },
            _ => return false,
        };
        match started {
            Ok(()) => {
                self.install = Install::Launching;
                self.save_marks_now();
                true
            }
            Err(reason) => {
                eprintln!("glint: couldn't install the update on the way out: {reason}");
                false
            }
        }
    }

    /// After a start in the tray, while nobody has opened the window: a ready update installs now, silently, and
    /// Glint starts again in the tray (a Scoop copy's goes to Scoop), unless something is going on or another Glint
    /// process runs. Start-up settles first ([`SETTLE`]). True when it is installing, or will be shortly.
    fn install_if_untouched(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.untouched
            || !self.auto
            || self.install == Install::Launching
            || crate::platform::secondary::is_secondary()
        {
            return false;
        }
        let unattended = self.available.as_ref().is_some_and(|a| self.unattended(&a.version));
        let ready = unattended
            && match self.config.policy {
                Policy::SelfUpdating => self.install == Install::Ready && self.ready.is_some(),
                Policy::Scoop => self.available.is_some() && self.config.scoop.is_some(),
                _ => false,
            };
        if !ready || (self.hooks.busy)(cx) || self.launcher.others_running() {
            return false;
        }
        let since_start = self.now().duration_since(self.started_at).unwrap_or_default();
        if since_start < SETTLE {
            if !self.install_later {
                self.install_later = true;
                let wait = SETTLE - since_start;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update(cx, |this, cx| {
                        this.install_later = false;
                        this.install_if_untouched(cx);
                    });
                })
                .detach();
            }
            return true;
        }
        self.launch(Relaunch::Tray, cx);
        true
    }
}

/// Whether version `a` is newer than `b` (both readable).
fn is_newer(a: &str, b: &str) -> bool {
    matches!((Version::parse(a), Version::parse(b)), (Some(a), Some(b)) if a > b)
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- Files ------------------------------------------------------------------------------------------------

/// The words of a download refused for its size (such a part isn't kept to be picked up).
const TOO_BIG: &str = "The update is bigger than any Glint installer";

/// Where one download goes: `<name>.part` in the updates folder while it comes (kept when it breaks off, to be
/// picked up from there), and how far it has come.
struct Target {
    dir: PathBuf,
    part: PathBuf,
    file: Option<std::fs::File>,
    received: u64,
    total: Option<u64>,
    last_step: Option<u8>,
}

impl Target {
    fn new(dir: &Path, name: &str) -> Self {
        Self {
            dir: dir.to_path_buf(),
            part: dir.join(format!("{name}.part")),
            file: None,
            received: 0,
            total: None,
            last_step: None,
        }
    }

    /// The bytes an earlier try left, where this one asks to start.
    fn kept(&self) -> u64 {
        std::fs::metadata(&self.part)
            .map(|meta| meta.len())
            .ok()
            .filter(|len| *len < MAX_INSTALLER)
            .unwrap_or(0)
    }

    fn take(&mut self, piece: Piece<'_>, progress: &async_channel::Sender<(u64, Option<u64>)>) -> Result<(), String> {
        match piece {
            Piece::Start { from, total } => {
                if total.is_some_and(|total| total > MAX_INSTALLER) {
                    return Err(TOO_BIG.into());
                }
                std::fs::create_dir_all(&self.dir).map_err(|e| format!("couldn't make a folder for it: {e}"))?;
                let file = if from == 0 {
                    std::fs::File::create(&self.part)
                } else {
                    std::fs::OpenOptions::new().append(true).open(&self.part)
                }
                .map_err(|e| format!("couldn't save it: {e}"))?;
                // Taking up from `from` needs exactly that much kept.
                if from > 0 && file.metadata().map(|meta| meta.len()).ok() != Some(from) {
                    return Err("couldn't take up the download where it stopped".into());
                }
                self.file = Some(file);
                self.received = from;
                self.total = total;
                self.report(progress);
                Ok(())
            }
            Piece::Bytes(chunk) => {
                let Some(file) = &mut self.file else {
                    return Err("the download sent bytes before it started".into());
                };
                file.write_all(chunk).map_err(|e| format!("couldn't save it: {e}"))?;
                self.received = self.received.saturating_add(chunk.len() as u64);
                if self.received > MAX_INSTALLER {
                    return Err(TOO_BIG.into());
                }
                self.report(progress);
                Ok(())
            }
        }
    }

    /// Progress goes out when the percent changes (or about every megabyte when the size is unknown): at most a
    /// hundred redraws for the whole download.
    fn report(&mut self, progress: &async_channel::Sender<(u64, Option<u64>)>) {
        let now = self.total.map(|total| percent(self.received, total));
        let step = now.or(Some((self.received >> 20) as u8));
        if step != self.last_step {
            self.last_step = step;
            let _ = progress.try_send((self.received, self.total));
        }
    }

    /// Closes the part, flushed to disk, and says where it is (none when nothing arrived).
    fn finish(&mut self) -> Option<PathBuf> {
        let file = self.file.take()?;
        let _ = file.sync_all();
        drop(file);
        if self.received == 0 {
            let _ = std::fs::remove_file(&self.part);
            return None;
        }
        Some(self.part.clone())
    }
}

/// The setup program's file name from its address: letters, digits and `._-` only, always ending in `.exe`.
fn file_name(url: &str) -> String {
    let last = url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("");
    let clean: String = last
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    // No leading dots: a name is never "." or "..", nor hidden.
    let clean = clean.trim_start_matches('.').to_string();
    match clean {
        name if name.to_lowercase().ends_with(".exe") && name.len() > 4 => name,
        name if name.is_empty() => "Glint-setup.exe".to_string(),
        name => format!("{name}.exe"),
    }
}

/// Removes a downloaded setup program (or a part of one).
fn remove_download(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Puts the checked download in its place (`<name>`, without `.part`) and writes `ready.json` beside it.
fn keep_ready(dir: &Path, part: &Path, record: &ReadyRecord) -> Result<PathBuf, String> {
    let path = dir.join(&record.file);
    let _ = std::fs::remove_file(&path);
    std::fs::rename(part, &path).map_err(|e| format!("Couldn't keep the update: {e}"))?;
    let text = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
    persistence::write_atomic(&dir.join(READY_FILE), text.as_bytes())
        .map_err(|e| format!("Couldn't keep the update: {e}"))?;
    Ok(path)
}

/// Forgets the waiting setup program's record (its file went, or a newer one takes its place).
fn forget_ready(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(READY_FILE));
}

/// The updates folder at start: the checked setup program of a newer version stays (checked again now), a download
/// broken off lately stays to be picked up, the Scoop helper's files stay (it may still run), and everything else
/// goes: the setup program just installed, a refused or abandoned file.
fn keep_or_clear(dir: &Path, current: &Version, key: &str) -> Option<(ReadyRecord, PathBuf)> {
    let record: Option<ReadyRecord> = std::fs::read_to_string(dir.join(READY_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let kept = record.and_then(|record| {
        let newer = Version::parse(&record.version).is_some_and(|version| version > *current);
        // The file is named by its address; a record naming anything else isn't followed.
        let path = dir.join(file_name(&record.url));
        let named = file_name(&record.url) == record.file;
        (newer && named && verify_file(&path, &record.signature, key, &record.version).is_ok())
            .then_some((record, path))
    });
    if kept.is_none() {
        forget_ready(dir);
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return kept;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let fresh = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < PART_KEPT);
        let keep = (name == READY_FILE && kept.is_some())
            || kept.as_ref().is_some_and(|(_, ready)| *ready == path)
            || name.starts_with("scoop-")
            || (name.ends_with(".part") && fresh);
        if !keep && path.is_file() {
            let _ = std::fs::remove_file(&path);
        }
    }
    kept
}

/// Removes the folders of earlier downloads in `%TEMP%` (versions before 0.14 put them there) that are older than
/// `older_than`. Only folders the updater made, by their name.
fn sweep(work_dir: &Path, older_than: Duration) {
    let Ok(entries) = std::fs::read_dir(work_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(WORK_PREFIX) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= older_than);
        if old && entry.path().is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn read_marks(path: &Path) -> Marks {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

// ---- The shared model -------------------------------------------------------------------------------------

/// The shared model, in a GPUI `Global`.
struct Shared(Entity<Updater>);

impl Global for Shared {}

/// Makes the shared model and starts checking (see the module notes for what the coordinator passes). Call it
/// once, early, before the first state write. `started_in_tray`: a `--tray` start; `auto`: the setting.
pub fn init(cx: &mut App, source: Arc<dyn Source>, hooks: Hooks, started_in_tray: bool, auto: bool) -> Entity<Updater> {
    let mut config = Config::for_app(started_in_tray);
    let launcher: Arc<dyn Launcher> = match dry_run() {
        Some(version) => {
            eprintln!("glint: updater dry run as version {version}: nothing is installed");
            config.current = version;
            config.policy = Policy::SelfUpdating;
            config.sibling = None;
            Arc::new(DryRun)
        }
        None => Arc::new(Windows),
    };
    let updater = cx.new(|_| {
        let mut updater = Updater::new(source, launcher, hooks, config);
        updater.auto = auto;
        updater
    });
    cx.set_global(Shared(updater.clone()));
    updater.update(cx, |updater, cx| updater.start(cx));
    updater
}

/// `GLINT_UPDATE_DRY_RUN=<version>`, in a debug build only: the updater behaves as the setup program's copy of
/// that version (the real feed, download, signature check, notices and quitting), but starts nothing, saying on
/// stderr what it would have run. For seeing the whole update in a dev build without touching an installed Glint.
pub fn dry_run() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var("GLINT_UPDATE_DRY_RUN")
        .ok()
        .filter(|version| Version::parse(version).is_some())
}

/// The dry run's [`Launcher`]: it says what it would start, and starts nothing.
struct DryRun;

impl Launcher for DryRun {
    fn launch(&self, installer: &Path, parameters: &[u16]) -> Result<(), String> {
        eprintln!(
            "glint: dry run: would run {} {}",
            installer.display(),
            String::from_utf16_lossy(parameters)
        );
        Ok(())
    }

    fn spawn_hidden(&self, program: &Path, args: &[OsString], _dir: &Path) -> Result<(), String> {
        eprintln!("glint: dry run: would start {} {args:?}", program.display());
        Ok(())
    }

    fn others_running(&self) -> bool {
        Windows.others_running()
    }

    fn session_ending(&self) -> bool {
        Windows.session_ending()
    }
}

/// From `on_app_quit`: a checked update installs on the way out ([`Updater::install_on_quit`]).
pub fn on_quit(cx: &mut App) {
    if let Some(updater) = try_shared(cx) {
        updater.update(cx, |updater, _| {
            updater.install_on_quit();
        });
    }
}

/// The shared model. Panics before [`init`]; use [`try_shared`] where it may not exist (tests).
pub fn shared(cx: &App) -> Entity<Updater> {
    cx.global::<Shared>().0.clone()
}

/// The shared model, if [`init`] (or [`set_shared`]) made one.
pub fn try_shared(cx: &App) -> Option<Entity<Updater>> {
    cx.try_global::<Shared>().map(|shared| shared.0.clone())
}

/// Makes `updater` the shared model, without starting it (tests, and a window of its own).
#[cfg(test)]
pub fn set_shared(cx: &mut App, updater: Entity<Updater>) {
    cx.set_global(Shared(updater));
}

// ---- What the person sees ---------------------------------------------------------------------------------

/// The "Update available" or "Update ready" notice (one at a time: a later one takes its place).
struct OfferToast;
/// The download's progress notice (`toast.loading` with an id).
struct ProgressToast;
/// Windows' own notification, while the window is hidden.
struct SystemToast;

/// Notices that came while the main window was hidden (in the tray, or minimized), shown when it is back.
#[derive(Default)]
struct Held(Vec<UpdaterEvent>);

impl Global for Held {}

#[cfg(test)]
thread_local! {
    /// Tests' windows have no Windows window to ask: this says whether they count as hidden.
    static HIDDEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Makes the tests' windows count as hidden (in the tray), or not.
#[cfg(test)]
pub(crate) fn pretend_hidden(hidden: bool) {
    HIDDEN.with(|cell| cell.set(hidden));
}

/// Whether `window` is where the person can see it: shown, and not minimized.
fn on_screen(window: &Window) -> bool {
    #[cfg(test)]
    if HIDDEN.with(|hidden| hidden.get()) {
        return false;
    }
    core_bridge::hwnd_of(window).is_none_or(|hwnd| {
        use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
        // SAFETY: asks about our own window; a stale handle just answers no.
        unsafe { IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() }
    })
}

/// Words an event as a notice in `window`, the main window. Call it from the subscription to the model. While the
/// window is hidden the notice waits for it ([`window_active`]), and an offer is said by Windows meanwhile.
pub fn show(event: &UpdaterEvent, window: &mut Window, cx: &mut App) {
    if on_screen(window) {
        push(event, window, cx);
    } else {
        hold(event, window, cx);
    }
}

/// From the workspace, when a window comes to the front: the model hears that someone is looking (and looks again
/// after a long time away), and the main window shows the notices that came while it was hidden.
pub fn window_active(window: &mut Window, cx: &mut App) {
    if !window.is_window_active() {
        return;
    }
    if let Some(updater) = try_shared(cx) {
        updater.update(cx, |updater, cx| updater.window_shown(cx));
    }
    if crate::startup::main_window(cx).is_none_or(|main| main == window.window_handle()) {
        show_held(window, cx);
    }
}

/// Shows the notices that waited for the main window, once it is on screen; Windows' own one goes.
fn show_held(window: &mut Window, cx: &mut App) {
    if !on_screen(window) {
        return;
    }
    let held = cx.try_global::<Held>().map(|held| held.0.clone()).unwrap_or_default();
    if held.is_empty() {
        return;
    }
    cx.set_global(Held::default());
    window.remove_notification::<SystemToast>(cx);
    for event in &held {
        push(event, window, cx);
    }
}

/// Keeps a notice for when the window is back: offers (the latest one only), the greeting and a failure. Progress
/// and "Installing" only matter while they happen.
fn hold(event: &UpdaterEvent, window: &mut Window, cx: &mut App) {
    let is_offer = |event: &UpdaterEvent| {
        matches!(
            event,
            UpdaterEvent::Found { .. } | UpdaterEvent::Ready { waiting: true, .. }
        )
    };
    let offer = is_offer(event);
    if !offer && !matches!(event, UpdaterEvent::Updated { .. } | UpdaterEvent::Failed { .. }) {
        return;
    }
    let held = &mut cx.default_global::<Held>().0;
    held.retain(|old| {
        if offer {
            !is_offer(old)
        } else {
            std::mem::discriminant(old) != std::mem::discriminant(event)
        }
    });
    held.push(event.clone());
    if offer {
        tell_windows(event, window, cx);
    }
}

/// An offer said by Windows itself (a toast in its notification center) while the window is hidden; a click brings
/// the window back, where the notice with its button waits. It takes the app identity the setup program's
/// shortcuts carry (startup.rs sets it); without one, Windows shows nothing.
fn tell_windows(event: &UpdaterEvent, window: &mut Window, cx: &mut App) {
    let (title, body) = match event {
        UpdaterEvent::Ready { version, .. } => (
            t!("Glint {version} is ready", version = version.clone()),
            t!("It installs when you quit Glint. Open Glint to restart now."),
        ),
        UpdaterEvent::Found { version, offer } => (
            t!("Glint {version} is available", version = version.clone()),
            match offer {
                Offer::Install => t!("Open Glint to install it."),
                Offer::Scoop => t!("Open Glint to update it through Scoop."),
                Offer::Download => t!("Open Glint to download it."),
            },
        ),
        _ => return,
    };
    window.push_notification(
        Notification::info(body)
            .title(title)
            .id::<SystemToast>()
            .system()
            .on_click(|_, _, _| core_bridge::post_to_ui(crate::startup::reveal_main)),
        cx,
    );
}

/// The in-app notice for an event.
fn push(event: &UpdaterEvent, window: &mut Window, cx: &mut App) {
    match event {
        UpdaterEvent::Found { version, offer } => {
            let (message, label) = match offer {
                Offer::Install => (
                    t!(
                        "Version {version} is ready. Installing it restarts the app.",
                        version = version.clone()
                    ),
                    t!("Install"),
                ),
                Offer::Scoop => (
                    t!(
                        "Version {version} is out. Scoop updates Glint, then it starts again.",
                        version = version.clone()
                    ),
                    t!("Update"),
                ),
                Offer::Download => (
                    t!("Version {version} is out.", version = version.clone()),
                    t!("Download"),
                ),
            };
            window.push_notification(
                Notification::info(message)
                    .title(t!("Update available"))
                    .id::<OfferToast>()
                    .action(move |_, _, _| {
                        Button::new("update-found-install")
                            .label(label.clone())
                            .on_click(|_, window, cx| {
                                window.remove_notification::<OfferToast>(cx);
                                if let Some(updater) = try_shared(cx) {
                                    updater.update(cx, |updater, cx| updater.install(cx));
                                }
                            })
                    }),
                cx,
            );
        }
        UpdaterEvent::Ready { version, waiting: true } => {
            window.push_notification(
                Notification::success(t!("It installs when you quit Glint, or restart now."))
                    .title(t!("Glint {version} is ready", version = version.clone()))
                    .id::<OfferToast>()
                    .action(move |_, _, _| {
                        Button::new("update-restart")
                            .label(t!("Restart now"))
                            .on_click(|_, window, cx| {
                                window.remove_notification::<OfferToast>(cx);
                                if let Some(updater) = try_shared(cx) {
                                    updater.update(cx, |updater, cx| updater.install(cx));
                                }
                            })
                    }),
                cx,
            );
        }
        UpdaterEvent::Ready { waiting: false, .. } => window.remove_notification::<ProgressToast>(cx),
        UpdaterEvent::Updated { version } => {
            let version = version.clone();
            window.push_notification(
                Notification::success(t!("See what's new in this version."))
                    .title(t!("Updated to Glint {version}", version = version.clone()))
                    .action(move |_, _, _| {
                        let url = Available::release_page(&version);
                        Button::new("update-whats-new")
                            .label(t!("What's new"))
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    }),
                cx,
            );
        }
        UpdaterEvent::Downloading { version, percent } => {
            let message = match percent {
                Some(percent) => t!(
                    "Downloading version {version}… {percent}%",
                    version = version.clone(),
                    percent = *percent
                ),
                None => t!("Downloading version {version}…", version = version.clone()),
            };
            window.push_notification(Notification::info(message).id::<ProgressToast>().autohide(false), cx);
        }
        UpdaterEvent::Installing { version } => window.push_notification(
            Notification::info(t!("Installing version {version}…", version = version.clone()))
                .id::<ProgressToast>()
                .autohide(false),
            cx,
        ),
        UpdaterEvent::Failed { reason, url } => {
            let url = url.clone();
            window.remove_notification::<ProgressToast>(cx);
            window.push_notification(
                Notification::error(reason.clone())
                    .title(t!("Couldn't install the update"))
                    .action(move |_, _, _| {
                        let url = url.clone();
                        Button::new("update-failed-download")
                            .label(t!("Download"))
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    }),
                cx,
            );
        }
    }
}

/// The header's Update button: there once there is something to do (not while an update comes in by itself), a
/// spinner while it installs. Update opens Settings at About; Restart to update (the download is in) restarts.
/// The coordinator puts it in the toolbar before the Settings button and observes the model to redraw.
pub fn update_button(cx: &App) -> Option<AnyElement> {
    let updater = try_shared(cx)?;
    let updater = updater.read(cx);
    let available = updater.available()?;
    if updater.fetching_quietly() {
        return None;
    }
    let ready = *updater.install_state() == Install::Ready && updater.downloaded().is_some();
    let working = updater.install_state().working();
    let t = theme::tokens(cx).clone();
    let (label, tip) = if ready {
        (
            t!("Restart to update"),
            t!(
                "Version {version} is ready: Glint restarts into it",
                version = available.version.clone()
            ),
        )
    } else {
        (
            t!("Update"),
            t!("Version {version} is available", version = available.version.clone()),
        )
    };
    Some(
        Button::new("update")
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(t.primary.opacity(0.))
                    .foreground(t.primary)
                    .hover(t.accent)
                    .active(t.accent),
            )
            .small()
            .icon(IconName::Sparkles)
            .label(label)
            .loading(working)
            .tooltip(tip)
            .on_click(move |_, window, cx| {
                if ready {
                    if let Some(updater) = try_shared(cx) {
                        updater.update(cx, |updater, cx| updater.install(cx));
                    }
                } else {
                    crate::components::settings::open_at(window, cx, Some(crate::components::settings::ABOUT));
                }
            })
            .into_any_element(),
    )
}

// ---- Tests ------------------------------------------------------------------------------------------------

/// The tests, and the fakes Settings' tests borrow to drive About.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::TestAppContext;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    // Fixtures made with a throwaway key (node's crypto, minisign's format): PUBKEY is its public key; the
    // signatures are of INSTALLER below, in the forms the tests need. Nothing here is Glint's real key.
    const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDQ4MjMyODlEM0U0QzcxQTMKUldSSUl5aWRQa3h4bytUbHpJb1FOZlBkSkZDMjg5ZDM5RmpzYlk1amJwWVZHQ1UyUnByeUhrbm4K";
    const OTHER_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEVFNTEwNEMxQjE0OUFGMEEKUldUdVVRVEJzVW12Q2doY2V1RjNxSkR0bk4ybDdQY3pIcWF3MENqTjdEckd2WWtKVnZQWnh0eDEK";
    pub(crate) const SIG_GOOD: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRlc3Qgc2VjcmV0IGtleQpSVVJJSXlpZFBreHhvMTNvL1k3dEIxQWEzd0JXLzR3Sk1aeFB3eEE0bUs5QmlLVmRqR2t5a3FLci9BMCt6YlhCUU5pN3p6cjI1MDdEWm9rNUd4VW9XN1E1ZVMwTXp4dFhSd1U9CnRydXN0ZWQgY29tbWVudDogdGltZXN0YW1wOjEJZmlsZTpHbGludF8wLjk5LjBfeDY0LXNldHVwLmV4ZQl2ZXJzaW9uOjAuOTkuMApDNzU5ZW0waXdPN1hNa1Q2WUQ1TDNPeWlKZmlpNWJCM2ZtSDNmbWQ5MXlHc1lDcUpQaU95dFdwZVpZSDVDZXZhUWZTcGhBT0NFZGdhaDVPRGF0eVlBdz09Cg==";
    const SIG_NO_VERSION: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRlc3Qgc2VjcmV0IGtleQpSVVJJSXlpZFBreHhvMTNvL1k3dEIxQWEzd0JXLzR3Sk1aeFB3eEE0bUs5QmlLVmRqR2t5a3FLci9BMCt6YlhCUU5pN3p6cjI1MDdEWm9rNUd4VW9XN1E1ZVMwTXp4dFhSd1U9CnRydXN0ZWQgY29tbWVudDogdGltZXN0YW1wOjEJZmlsZTpHbGludF8wLjk5LjBfeDY0LXNldHVwLmV4ZQpxMWFFZVB3UUYyKzBFMVZHL1BzMzhXcnpyUXgvaGhNNGc0MStMNWIvZmNJN00zNE1STWdFUWJXUUROK3FNNkZpTWRGZmV0UHJWL3RTNU4wVlBJZytDUT09Cg==";
    const SIG_OLD_VERSION: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRlc3Qgc2VjcmV0IGtleQpSVVJJSXlpZFBreHhvMTNvL1k3dEIxQWEzd0JXLzR3Sk1aeFB3eEE0bUs5QmlLVmRqR2t5a3FLci9BMCt6YlhCUU5pN3p6cjI1MDdEWm9rNUd4VW9XN1E1ZVMwTXp4dFhSd1U9CnRydXN0ZWQgY29tbWVudDogdGltZXN0YW1wOjEJZmlsZTpHbGludF8wLjk4LjBfeDY0LXNldHVwLmV4ZQl2ZXJzaW9uOjAuOTguMApOWGY1clpxTEZlM1ZJTlAzaFp2ODNCN0Y5VXI1b01JUHZoa2VkZ2c4QnlYN2pzb3A0RGVPaDBMOVpaalp4WVpBeXBqMnMxY3gxWVRzc3VmbVpnUUdBUT09Cg==";
    const SIG_LEGACY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRlc3Qgc2VjcmV0IGtleQpSV1JJSXlpZFBreHhvemQ4ZVllYXh4WGUzKzgvdDNON29rWWMxSWJYVzFkdksvV1JvRjZsWDJlT0ZaM0tRRWMyUk5NeGwzOWp1WkJGMG5KR09zamZnSUczR24vOVRNQm94d289CnRydXN0ZWQgY29tbWVudDogdGltZXN0YW1wOjEJZmlsZTpHbGludF8wLjk5LjBfeDY0LXNldHVwLmV4ZQl2ZXJzaW9uOjAuOTkuMApXNVl2bmxMSmFZZVhGWmJYdHNUTmtZTmxmTmpVSExjcmFnR0l2TnAzWm5CY281SW5SQ0FrQjJXTU1mL01FMmhiazRKUmlaS3Y3R1hnTWtMV0U5WnZEUT09Cg==";
    const SIG_OTHER_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRlc3Qgc2VjcmV0IGtleQpSVVR1VVFUQnNVbXZDbkFndUwrZ0tmNG5zTHBDNWRsc0pQUFovM0dyVkdkSTNXY3RsRXlZTW9nY3dkaTNNTkxvSWpXbml0QzB4dmx5enNPNG45V0M5ZTRIV2cwSFg4U1BLQWM9CnRydXN0ZWQgY29tbWVudDogdGltZXN0YW1wOjEJZmlsZTpHbGludF8wLjk5LjBfeDY0LXNldHVwLmV4ZQl2ZXJzaW9uOjAuOTkuMApYMEJyNE5MQWFFNlRYOFdCWHJUdHFyNm5lblFwZ1NzdGlpdGdUeDQvalJNdkx5ZUM1SDlZYnhHY0E4T3VQSmM0bWc4T0dxb2owQmkwWjV4RTUvU0tDUT09Cg==";

    const INSTALLER: &[u8] = b"MZ pretend installer for Glint 0.99.0";

    pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs a future that is ready at once (the fakes' are) without a runtime.
    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        for _ in 0..1000 {
            if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
                return value;
            }
        }
        panic!("the fake never answered");
    }

    fn manifest_for(version: &str, signature: &str) -> String {
        format!(
            r#"{{"version":"{version}","notes":"{RELEASES}/tag/v{version}","pub_date":"2026-11-10T00:00:00Z",
            "platforms":{{"windows-x86_64":{{"signature":"{signature}","url":"{RELEASES}/download/v{version}/Glint_{version}_x64-setup.exe"}}}}}}"#
        )
    }

    /// A source with canned answers that remembers what it was asked.
    pub(crate) struct Fake {
        manifest: Mutex<Result<Option<String>, String>>,
        github: Mutex<Result<String, String>>,
        payload: Mutex<Vec<u8>>,
        download_error: Mutex<Option<String>>,
        /// Whether a download asked to start part way takes up from there (a server that knows ranges).
        ranges: Mutex<bool>,
        metered: Mutex<bool>,
        asked: Mutex<Vec<String>>,
        /// Where each download was asked to start.
        froms: Mutex<Vec<u64>>,
    }

    impl Fake {
        pub(crate) fn new() -> Arc<Fake> {
            Arc::new(Fake {
                manifest: Mutex::new(Ok(None)),
                github: Mutex::new(Err("GitHub answered 500 Internal Server Error".into())),
                payload: Mutex::new(INSTALLER.to_vec()),
                download_error: Mutex::new(None),
                ranges: Mutex::new(true),
                metered: Mutex::new(false),
                asked: Mutex::new(Vec::new()),
                froms: Mutex::new(Vec::new()),
            })
        }

        pub(crate) fn asked(&self) -> Vec<String> {
            lock(&self.asked).clone()
        }

        fn downloads(&self) -> usize {
            self.asked().iter().filter(|a| a.starts_with("download")).count()
        }

        /// Both the feed and GitHub fail with `reason`.
        pub(crate) fn fail(&self, reason: &str) {
            *lock(&self.manifest) = Err(reason.to_string());
            *lock(&self.github) = Err(reason.to_string());
        }

        pub(crate) fn serve(&self, version: &str, signature: &str) {
            *lock(&self.manifest) = Ok(Some(manifest_for(version, signature)));
        }
    }

    impl Source for Fake {
        fn manifest(&self) -> Fut<Option<String>> {
            lock(&self.asked).push("manifest".into());
            let answer = lock(&self.manifest).clone();
            Box::pin(async move { answer })
        }

        fn latest_release(&self) -> Fut<String> {
            lock(&self.asked).push("github".into());
            let answer = lock(&self.github).clone();
            Box::pin(async move { answer })
        }

        fn download(&self, url: String, from: u64, mut sink: Sink) -> Fut<()> {
            lock(&self.asked).push(format!("download {url}"));
            lock(&self.froms).push(from);
            let payload = lock(&self.payload).clone();
            let error = lock(&self.download_error).clone();
            let start = if *lock(&self.ranges) && (from as usize) < payload.len() {
                from as usize
            } else {
                0
            };
            Box::pin(async move {
                sink(Piece::Start {
                    from: start as u64,
                    total: Some(payload.len() as u64),
                })?;
                // Four pieces, so progress has steps (and a download that breaks off has kept three of them).
                let rest = &payload[start..];
                let size = rest.len().div_ceil(4).max(1);
                for (i, piece) in rest.chunks(size).enumerate() {
                    if i == 3 {
                        if let Some(error) = error {
                            return Err(error);
                        }
                    }
                    sink(Piece::Bytes(piece))?;
                }
                Ok(())
            })
        }

        fn metered(&self) -> bool {
            *lock(&self.metered)
        }
    }

    /// What the updater asked of Windows, recorded instead of done.
    #[derive(Default)]
    pub(crate) struct Recorded {
        runs: Mutex<Vec<(PathBuf, String)>>,
        spawned: Mutex<Vec<(PathBuf, Vec<String>, PathBuf)>>,
        fail: Mutex<Option<String>>,
        others: Mutex<bool>,
        ending: Mutex<bool>,
    }

    impl Launcher for Recorded {
        fn launch(&self, installer: &Path, parameters: &[u16]) -> Result<(), String> {
            if let Some(why) = lock(&self.fail).clone() {
                return Err(why);
            }
            lock(&self.runs).push((installer.to_path_buf(), String::from_utf16_lossy(parameters)));
            Ok(())
        }

        fn spawn_hidden(&self, program: &Path, args: &[OsString], dir: &Path) -> Result<(), String> {
            let args = args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
            lock(&self.spawned).push((program.to_path_buf(), args, dir.to_path_buf()));
            Ok(())
        }

        fn others_running(&self) -> bool {
            *lock(&self.others)
        }

        fn session_ending(&self) -> bool {
            *lock(&self.ending)
        }
    }

    /// The tests' wall clock (seconds since 1970), turned by hand. The tests that use it hold `testing::lock()`.
    static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1_800_000_000);

    fn clock() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(CLOCK.load(AtomicOrdering::SeqCst))
    }

    fn turn_clock(by: Duration) {
        CLOCK.fetch_add(by.as_secs(), AtomicOrdering::SeqCst);
    }

    fn temp_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "glint-updater-test-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, AtomicOrdering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary folder for the test");
        dir
    }

    fn version(text: &str) -> Version {
        Version::parse(text).expect("a version")
    }

    // ---- Versions and the feed

    #[test]
    fn versions_compare_like_semver() {
        assert!(version("0.10.0") > version("0.9.5"));
        assert!(version("v1.0.0") == version("1.0.0"));
        assert!(version("1.0.0") > version("1.0.0-beta.2"));
        assert!(version("1.0.0-beta.10") > version("1.0.0-beta.2"));
        assert!(version("1.0.0-beta") > version("1.0.0-alpha"));
        assert!(version("1.0.1") > version("1.0"));
        assert_eq!(version("1.0.0+build.5").cmp(&version("1.0.0")), Ordering::Equal);
        assert!(version("2.0.0") > version("1.99.99"));
    }

    #[test]
    fn versions_that_are_not_versions_do_not_parse() {
        for bad in ["", "v", "1..2", "1.2.x", "latest", "1.0.0-", "1.0.0-a..b", "-1"] {
            assert!(Version::parse(bad).is_none(), "{bad:?} parsed");
        }
    }

    #[test]
    fn the_manifest_release_does_writes_is_read() {
        let release = parse_manifest(&manifest_for("0.10.0", "c2ln")).expect("a manifest");
        assert_eq!(release.version, "0.10.0");
        assert_eq!(
            release.notes.as_deref(),
            Some("https://github.com/shadijhade/glint/releases/tag/v0.10.0")
        );
        let installer = release.installer.expect("an installer for windows-x86_64");
        assert_eq!(installer.signature, "c2ln");
        assert_eq!(
            installer.url,
            "https://github.com/shadijhade/glint/releases/download/v0.10.0/Glint_0.10.0_x64-setup.exe"
        );
    }

    #[test]
    fn the_manifest_prefers_the_nsis_target_and_reads_the_dynamic_format() {
        let both = r#"{"version":"1.2.3","platforms":{
            "windows-x86_64":{"signature":"plain","url":"https://x/plain.exe"},
            "windows-x86_64-nsis":{"signature":"nsis","url":"https://x/nsis.exe"}}}"#;
        assert_eq!(
            parse_manifest(both)
                .expect("read")
                .installer
                .expect("installer")
                .signature,
            "nsis"
        );
        let dynamic = r#"{"version":"v1.2.3","url":"https://x/a.exe","signature":"sig"}"#;
        let release = parse_manifest(dynamic).expect("read");
        assert_eq!(release.version, "1.2.3");
        assert_eq!(release.installer.expect("installer").url, "https://x/a.exe");
    }

    #[test]
    fn a_manifest_without_an_installer_for_this_pc_or_over_http_has_none() {
        let other =
            r#"{"version":"1.2.3","platforms":{"darwin-aarch64":{"signature":"s","url":"https://x/a.tar.gz"}}}"#;
        assert!(parse_manifest(other).expect("read").installer.is_none());
        let insecure = r#"{"version":"1.2.3","platforms":{"windows-x86_64":{"signature":"s","url":"http://x/a.exe"}}}"#;
        assert!(parse_manifest(insecure).expect("read").installer.is_none());
        let unsigned =
            r#"{"version":"1.2.3","platforms":{"windows-x86_64":{"signature":" ","url":"https://x/a.exe"}}}"#;
        assert!(parse_manifest(unsigned).expect("read").installer.is_none());
    }

    #[test]
    fn a_manifest_that_is_not_one_is_an_error() {
        assert!(parse_manifest("not json").is_err());
        assert!(parse_manifest(r#"{"notes":"x"}"#).is_err());
        assert!(parse_manifest(r#"{"version":"soon"}"#).is_err());
    }

    #[test]
    fn notes_that_are_an_address_are_not_shown_as_text() {
        assert_eq!(notes_text("https://github.com/shadijhade/glint/releases/tag/v1"), None);
        assert_eq!(notes_text("  "), None);
        assert_eq!(notes_text("Faster search\nBug fixes"), Some("Faster search\nBug fixes"));
    }

    #[test]
    fn offline_failures_are_recognised_by_their_words() {
        for offline in [
            "error sending request for url (https://github.com/…)",
            "dns error: no such host",
            "operation timed out",
            "tcp connect error",
            "Failed to fetch",
            "network is unreachable",
        ] {
            assert!(looks_offline(offline), "{offline}");
        }
        assert!(!looks_offline("GitHub answered 403 Forbidden"));
    }

    // ---- Finding an update

    #[test]
    fn a_newer_version_in_the_manifest_is_offered_with_its_installer() {
        let fake = Fake::new();
        fake.serve("0.99.0", "c2ln");
        let found = block_on(find(&*fake, Policy::SelfUpdating, &version("0.9.5"))).expect("a check");
        let found = found.expect("an update");
        assert_eq!(found.version, "0.99.0");
        assert_eq!(found.url, "https://github.com/shadijhade/glint/releases/tag/v0.99.0");
        assert!(found.installer.is_some());
        assert_eq!(fake.asked(), vec!["manifest"]);
    }

    #[test]
    fn the_same_or_an_older_version_is_not_an_update() {
        for served in ["0.9.5", "0.9.4", "0.8.0"] {
            let fake = Fake::new();
            fake.serve(served, "c2ln");
            let found = block_on(find(&*fake, Policy::SelfUpdating, &version("0.9.5"))).expect("a check");
            assert_eq!(found, None, "{served} is not newer than 0.9.5");
            // The manifest answered, so GitHub is not asked.
            assert_eq!(fake.asked(), vec!["manifest"]);
        }
    }

    #[test]
    fn a_feed_that_says_nothing_newer_is_believed() {
        let fake = Fake::new();
        let found = block_on(find(&*fake, Policy::SelfUpdating, &version("0.9.5"))).expect("a check");
        assert_eq!(found, None);
        assert_eq!(fake.asked(), vec!["manifest"]);
    }

    #[test]
    fn an_unreadable_manifest_falls_back_to_github_without_an_installer() {
        for manifest in [
            Err("GitHub answered 404 Not Found".to_string()),
            Ok(Some("<html>".to_string())),
        ] {
            let fake = Fake::new();
            *lock(&fake.manifest) = manifest;
            *lock(&fake.github) =
                Ok(r#"{"tag_name":"v0.10.0","html_url":"https://github.com/x/releases/tag/v0.10.0"}"#.into());
            let found = block_on(find(&*fake, Policy::SelfUpdating, &version("0.9.5"))).expect("a check");
            let found = found.expect("an update from GitHub");
            assert_eq!(found.version, "0.10.0");
            assert_eq!(found.url, "https://github.com/x/releases/tag/v0.10.0");
            assert!(found.installer.is_none(), "GitHub's answer can only link");
            assert_eq!(fake.asked(), vec!["manifest", "github"]);
        }
    }

    #[test]
    fn copies_that_do_not_install_themselves_read_the_feed_without_its_installer() {
        // The feed is a release file, which GitHub doesn't count against its API's hourly limit.
        for policy in [Policy::LinkOnly, Policy::Scoop] {
            let fake = Fake::new();
            fake.serve("0.99.0", "c2ln");
            let found = block_on(find(&*fake, policy, &version("0.9.5"))).expect("a check");
            let found = found.expect("an update");
            assert!(found.installer.is_none());
            assert_eq!(found.url, "https://github.com/shadijhade/glint/releases/tag/v0.99.0");
            assert_eq!(fake.asked(), vec!["manifest"]);
        }
    }

    #[test]
    fn githubs_fallback_takes_only_plain_newer_releases() {
        let current = version("0.9.5");
        let ask = |json: &str| from_github(json, &current).expect("readable");
        assert!(ask(r#"{"tag_name":"v0.9.5"}"#).is_none());
        assert!(ask(r#"{"tag_name":"v0.9.4"}"#).is_none());
        assert!(
            ask(r#"{"tag_name":"v1.0.0-beta"}"#).is_none(),
            "pre-releases are not offered"
        );
        assert!(ask(r#"{"tag_name":"nightly"}"#).is_none());
        assert!(ask(r#"{}"#).is_none());
        assert_eq!(ask(r#"{"tag_name":"0.10.0"}"#).expect("newer").version, "0.10.0");
        assert!(from_github("nope", &current).is_err());
    }

    #[test]
    fn when_both_fail_the_check_fails_with_githubs_reason() {
        let fake = Fake::new();
        *lock(&fake.manifest) = Err("error sending request".into());
        let failed = block_on(find(&*fake, Policy::SelfUpdating, &version("0.9.5")));
        assert_eq!(failed, Err("GitHub answered 500 Internal Server Error".to_string()));
    }

    // ---- Signatures

    #[test]
    fn the_key_built_in_is_a_key_the_signer_can_use_and_is_the_services() {
        let text = minisign_text(PUBLIC_KEY).expect("base64 of text");
        assert!(PublicKey::decode(&text).is_ok(), "{text}");
        // The service checks its updates against the same key: a release signs both with one key.
        let service = Path::new(env!("CARGO_MANIFEST_DIR")).join("../shadui-index/update-pubkey.txt");
        let service = std::fs::read_to_string(service).expect("crates/shadui-index/update-pubkey.txt");
        assert_eq!(
            service.trim(),
            PUBLIC_KEY,
            "the service's update key changed: change it here too"
        );
    }

    #[test]
    fn a_good_signature_for_the_announced_version_passes() {
        assert_eq!(verify_bytes(INSTALLER, SIG_GOOD, PUBKEY, "0.99.0"), Ok(()));
        assert_eq!(verify_bytes(INSTALLER, SIG_GOOD, PUBKEY, "v0.99.0"), Ok(()));
    }

    #[test]
    fn changed_bytes_are_refused() {
        let mut changed = INSTALLER.to_vec();
        changed[5] ^= 1;
        assert_eq!(
            verify_bytes(&changed, SIG_GOOD, PUBKEY, "0.99.0"),
            Err(Refusal::Mismatch)
        );
        assert_eq!(
            verify_bytes(&INSTALLER[..10], SIG_GOOD, PUBKEY, "0.99.0"),
            Err(Refusal::Mismatch)
        );
    }

    #[test]
    fn another_keys_signature_is_refused() {
        assert_eq!(
            verify_bytes(INSTALLER, SIG_OTHER_KEY, PUBKEY, "0.99.0"),
            Err(Refusal::WrongKey)
        );
        assert_eq!(
            verify_bytes(INSTALLER, SIG_GOOD, OTHER_PUBKEY, "0.99.0"),
            Err(Refusal::WrongKey)
        );
    }

    #[test]
    fn a_signature_for_another_version_is_refused() {
        // An older release's signed file under a newer number (the downgrade the version binding stops).
        assert_eq!(
            verify_bytes(INSTALLER, SIG_OLD_VERSION, PUBKEY, "0.99.0"),
            Err(Refusal::WrongVersion {
                signed: "0.98.0".into(),
                announced: "0.99.0".into()
            })
        );
        assert_eq!(
            verify_bytes(INSTALLER, SIG_GOOD, PUBKEY, "1.0.0"),
            Err(Refusal::WrongVersion {
                signed: "0.99.0".into(),
                announced: "1.0.0".into()
            })
        );
    }

    #[test]
    fn a_signature_that_names_no_version_is_refused() {
        assert_eq!(
            verify_bytes(INSTALLER, SIG_NO_VERSION, PUBKEY, "0.99.0"),
            Err(Refusal::NoSignedVersion)
        );
    }

    #[test]
    fn garbage_keys_and_signatures_are_refused_without_a_panic() {
        assert_eq!(verify_bytes(INSTALLER, SIG_GOOD, "!!!", "0.99.0"), Err(Refusal::BadKey));
        assert_eq!(verify_bytes(INSTALLER, SIG_GOOD, "", "0.99.0"), Err(Refusal::BadKey));
        assert_eq!(
            verify_bytes(INSTALLER, "!!!", PUBKEY, "0.99.0"),
            Err(Refusal::BadSignature)
        );
        assert_eq!(
            verify_bytes(INSTALLER, "", PUBKEY, "0.99.0"),
            Err(Refusal::BadSignature)
        );
        assert_eq!(
            verify_bytes(INSTALLER, "dGV4dA==", PUBKEY, "0.99.0"),
            Err(Refusal::BadSignature)
        );
    }

    #[test]
    fn a_file_is_verified_in_pieces_and_the_old_unhashed_signatures_too() {
        let dir = temp_dir("verify-file");
        let file = dir.join("setup.exe");
        std::fs::write(&file, INSTALLER).expect("write");
        assert_eq!(verify_file(&file, SIG_GOOD, PUBKEY, "0.99.0"), Ok(()));
        // Signed the legacy way (no BLAKE2b prehash), which can't be streamed, so it is read whole.
        assert_eq!(verify_file(&file, SIG_LEGACY, PUBKEY, "0.99.0"), Ok(()));
        assert_eq!(
            verify_file(&file, SIG_OTHER_KEY, PUBKEY, "0.99.0"),
            Err(Refusal::WrongKey)
        );
        assert!(verify_file(&file, SIG_OLD_VERSION, PUBKEY, "0.99.0")
            .unwrap_err()
            .to_string()
            .contains("0.98.0"));
        std::fs::write(&file, b"MZ something else").expect("write");
        assert_eq!(verify_file(&file, SIG_GOOD, PUBKEY, "0.99.0"), Err(Refusal::Mismatch));
        assert!(matches!(
            verify_file(&dir.join("missing.exe"), SIG_GOOD, PUBKEY, "0.99.0"),
            Err(Refusal::Unreadable(_))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn base64_reads_what_the_signer_writes() {
        assert_eq!(base64_decode("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(base64_decode("aGVsbG8"), Some(b"hello".to_vec()));
        assert_eq!(base64_decode("aGVs\nbG8="), Some(b"hello".to_vec()));
        assert_eq!(base64_decode(""), Some(Vec::new()));
        assert_eq!(base64_decode("a*b"), None);
    }

    // ---- The installer's command line

    fn parameters(relaunch: Relaunch) -> String {
        String::from_utf16(&installer_parameters(relaunch)).expect("utf-16")
    }

    /// Restart now's command line with `args` passed on to Glint.
    fn quoted(args: &[&str]) -> String {
        let args: Vec<Vec<u16>> = args.iter().map(|arg| wide(arg)).collect();
        String::from_utf16(&command_line("/P", Some(&args))).expect("utf-16")
    }

    #[test]
    fn the_installer_shows_its_progress_to_restart_now_and_is_silent_otherwise() {
        assert_eq!(parameters(Relaunch::Window), "/P /UPDATE /R /ARGS");
        assert_eq!(parameters(Relaunch::Tray), "/S /UPDATE /R /ARGS --tray");
        assert_eq!(parameters(Relaunch::No), "/S /UPDATE");
    }

    #[test]
    fn restart_arguments_are_quoted_the_way_nsis_wants() {
        assert_eq!(quoted(&["C:\\Photos"]), "/P /UPDATE /R /ARGS C:\\Photos");
        assert_eq!(
            quoted(&["C:\\My Folder\\"]),
            "/P /UPDATE /R /ARGS \"C:\\My Folder\\\\\""
        );
        // A slash would be read as one of the installer's own switches.
        assert_eq!(quoted(&["/x"]), "/P /UPDATE /R /ARGS \"/x\"");
        assert_eq!(quoted(&[""]), "/P /UPDATE /R /ARGS \"\"");
        assert_eq!(quoted(&["a\"b"]), "/P /UPDATE /R /ARGS a\\\"b");
        assert_eq!(quoted(&["one", "two words"]), "/P /UPDATE /R /ARGS one \"two words\"");
        // Arabic and spaces survive.
        assert_eq!(quoted(&["C:\\ملفات و صور"]), "/P /UPDATE /R /ARGS \"C:\\ملفات و صور\"");
    }

    #[test]
    fn download_names_are_safe_exe_names() {
        assert_eq!(
            file_name("https://x/releases/download/v1/Glint_1.0.0_x64-setup.exe"),
            "Glint_1.0.0_x64-setup.exe"
        );
        assert_eq!(file_name("https://x/a/b.exe?token=1"), "b.exe");
        assert_eq!(file_name("https://x/a/..%2f..%2fevil"), "2f..2fevil.exe");
        assert_eq!(file_name("https://x/"), "Glint-setup.exe");
        assert_eq!(file_name("https://x/a/.."), "Glint-setup.exe");
        assert_eq!(file_name("https://x/a/setup"), "setup.exe");
    }

    #[test]
    fn sweep_removes_only_old_glint_update_folders() {
        let dir = temp_dir("sweep");
        let ours = dir.join(format!("{WORK_PREFIX}1-2"));
        let theirs = dir.join("something-else");
        std::fs::create_dir_all(&ours).expect("dir");
        std::fs::create_dir_all(&theirs).expect("dir");
        std::fs::write(ours.join("setup.exe"), b"MZ").expect("write");
        sweep(&dir, Duration::from_secs(3600));
        assert!(ours.exists(), "a fresh folder stays");
        sweep(&dir, Duration::ZERO);
        assert!(!ours.exists());
        assert!(theirs.exists(), "folders that are not ours stay");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_updates_folder_keeps_a_newer_checked_update_a_fresh_part_and_scoops_files() {
        let dir = temp_dir("keep");
        let name = "Glint_0.99.0_x64-setup.exe";
        let url = format!("{RELEASES}/download/v0.99.0/{name}");
        std::fs::write(dir.join(name), INSTALLER).expect("write");
        let record = ReadyRecord {
            version: "0.99.0".into(),
            file: name.into(),
            url: url.clone(),
            signature: SIG_GOOD.into(),
        };
        std::fs::write(dir.join(READY_FILE), serde_json::to_string(&record).unwrap()).unwrap();
        std::fs::write(dir.join("Glint_1.0.0_x64-setup.exe.part"), b"MZ half").unwrap();
        std::fs::write(dir.join(scoop::RESULT), "{}").unwrap();
        std::fs::write(dir.join("Glint_0.98.0_x64-setup.exe"), b"MZ the one installed before").unwrap();
        let kept = keep_or_clear(&dir, &version("0.9.5"), PUBKEY).expect("the checked update stays");
        assert_eq!(kept.1, dir.join(name));
        for stays in [name, READY_FILE, "Glint_1.0.0_x64-setup.exe.part", scoop::RESULT] {
            assert!(dir.join(stays).exists(), "{stays} should stay");
        }
        assert!(
            !dir.join("Glint_0.98.0_x64-setup.exe").exists(),
            "an old setup program goes"
        );
        // Once that version runs, its setup program and record go.
        assert_eq!(keep_or_clear(&dir, &version("0.99.0"), PUBKEY), None);
        assert!(!dir.join(name).exists() && !dir.join(READY_FILE).exists());
        // A file that no longer matches its signature isn't kept either.
        std::fs::write(dir.join(name), b"MZ swapped").unwrap();
        std::fs::write(dir.join(READY_FILE), serde_json::to_string(&record).unwrap()).unwrap();
        assert_eq!(keep_or_clear(&dir, &version("0.9.5"), PUBKEY), None);
        assert!(!dir.join(name).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---- Windows, for real

    #[test]
    fn windows_answers_whether_the_connection_is_metered_without_falling_over() {
        // Either answer is right on some PC; what matters is that the Windows Runtime call returns.
        let _ = metered_connection();
        assert!(
            !Windows.session_ending(),
            "this test isn't running while Windows signs out"
        );
    }

    #[test]
    fn a_process_is_matched_by_the_program_it_runs() {
        let exe = std::env::current_exe().expect("this test's program");
        assert!(super::runs(std::process::id(), &exe));
        assert!(!super::runs(std::process::id(), Path::new(r"C:\Windows\notepad.exe")));
        assert!(!super::runs(u32::MAX - 1, &exe), "no such process");
        // Nothing else runs this test program.
        assert!(!other_glints());
    }

    #[test]
    fn the_helper_starts_with_no_window_in_the_folder_given() {
        let dir = temp_dir("spawn");
        let marker = dir.join("ran.txt");
        let script = format!(
            "Set-Content -LiteralPath '{}' -Value (Get-Location).Path",
            marker.display()
        );
        let args: Vec<OsString> = ["-NoProfile", "-NonInteractive", "-Command", &script]
            .iter()
            .map(OsString::from)
            .collect();
        Windows
            .spawn_hidden(&scoop::powershell(), &args, &dir)
            .expect("it starts");
        for _ in 0..200 {
            if marker.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(200));
        let said = std::fs::read_to_string(&marker).expect("the helper ran");
        assert_eq!(
            Path::new(said.trim()).canonicalize().ok(),
            dir.canonicalize().ok(),
            "in the updates folder, never in Scoop's"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The real feed and the real key, over the network: the latest release reads, its setup program is offered,
    /// a download asked to start near its end gets exactly the rest (what picks up a broken-off download), and
    /// the whole file passes the signature check with the key built in. `cargo test -p glint-ui --lib --
    /// --ignored the_real_release` runs it.
    #[test]
    #[ignore = "uses the network: GitHub's latest release"]
    fn the_real_release_reads_resumes_and_passes_its_signature() {
        let source = HttpSource::new(glint_core::tools::install_tls_provider);
        use glint_core::host::async_runtime::block_on as block;
        let found = block(find(&source, Policy::SelfUpdating, &version("0.0.1")))
            .expect("the feed reads")
            .expect("a release");
        let installer = found.installer.clone().expect("the setup program and its signature");
        eprintln!("latest: {} at {}", found.version, installer.url);

        // The last 1000 bytes only.
        let total = Arc::new(Mutex::new((None, 0usize, 0u64)));
        let seen = total.clone();
        let probe: Sink = Box::new(move |piece| {
            let mut seen = locked(&seen);
            match piece {
                Piece::Start { from, total } => *seen = (total, seen.1, from),
                Piece::Bytes(bytes) => seen.1 += bytes.len(),
            }
            Ok(())
        });
        // A first answer tells the size; ask again from 1000 bytes before the end.
        let sized: Sink = Box::new({
            let total = total.clone();
            move |piece| {
                if let Piece::Start { total: size, .. } = piece {
                    locked(&total).0 = size;
                    return Err("enough".into());
                }
                Ok(())
            }
        });
        let _ = block(source.download(installer.url.clone(), 0, sized));
        let size = locked(&total).0.expect("GitHub says how big it is");
        block(source.download(installer.url.clone(), size - 1000, probe)).expect("the rest comes");
        let (whole, received, from) = *locked(&total);
        assert_eq!(
            (whole, received as u64, from),
            (Some(size), 1000, size - 1000),
            "picked up exactly there"
        );

        // The whole file, checked as an install would check it.
        let dir = temp_dir("real");
        let target = Arc::new(Mutex::new(Target::new(&dir, &file_name(&installer.url))));
        let (progress, _heard) = async_channel::unbounded();
        let sink: Sink = {
            let target = target.clone();
            Box::new(move |piece| locked(&target).take(piece, &progress))
        };
        block(source.download(installer.url.clone(), 0, sink)).expect("it downloads");
        let part = locked(&target).finish().expect("bytes arrived");
        assert_eq!(std::fs::metadata(&part).map(|m| m.len()).ok(), Some(size));
        verify_file(&part, &installer.signature, PUBLIC_KEY, &found.version).expect("signed with Glint's key");
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---- The model

    /// A model over fakes, with what it was asked kept for the tests to read.
    pub(crate) struct Rig {
        pub(crate) fake: Arc<Fake>,
        launched: Arc<Recorded>,
        asked: Rc<std::cell::RefCell<usize>>,
        pub(crate) quit: Rc<std::cell::RefCell<usize>>,
        say_yes: Rc<std::cell::Cell<bool>>,
        busy: Rc<std::cell::Cell<bool>>,
        pub(crate) updater: Entity<Updater>,
        events: Rc<std::cell::RefCell<Vec<UpdaterEvent>>>,
        pub(crate) dir: PathBuf,
    }

    /// How a rig's copy is set up.
    #[derive(Clone)]
    struct Setup {
        policy: Policy,
        current: String,
        returning: bool,
        in_tray: bool,
        auto: bool,
        scoop: Option<scoop::Copy>,
        sibling: Option<scoop::Copy>,
        /// The folder to use (another rig's, to start again over what it left); a fresh one when none.
        dir: Option<PathBuf>,
    }

    impl Setup {
        fn new(policy: Policy, current: &str) -> Setup {
            Setup {
                policy,
                current: current.to_string(),
                returning: false,
                in_tray: false,
                auto: false,
                scoop: None,
                sibling: None,
                dir: None,
            }
        }

        fn auto(mut self) -> Setup {
            self.auto = true;
            self
        }
    }

    fn rig(cx: &mut TestAppContext, policy: Policy, current: &str, returning: bool) -> Rig {
        rig_with(
            cx,
            Setup {
                returning,
                ..Setup::new(policy, current)
            },
        )
    }

    fn rig_with(cx: &mut TestAppContext, setup: Setup) -> Rig {
        cx.executor().allow_parking();
        cx.update(testing::init);
        cx.update(|cx| build_rig_with(cx, setup))
    }

    /// The model of [`rig`] in an app that is already set up.
    pub(crate) fn build_rig(cx: &mut App, policy: Policy, current: &str, returning: bool) -> Rig {
        build_rig_with(
            cx,
            Setup {
                returning,
                ..Setup::new(policy, current)
            },
        )
    }

    /// A rig whose updates install by themselves; a Scoop copy for [`Policy::Scoop`] (for Settings' tests).
    pub(crate) fn build_auto_rig(cx: &mut App, policy: Policy, current: &str) -> Rig {
        let dir = temp_dir("auto");
        let scoop = (policy == Policy::Scoop).then(|| scoop_copy(&dir, current));
        build_rig_with(
            cx,
            Setup {
                scoop,
                dir: Some(dir),
                ..Setup::new(policy, current).auto()
            },
        )
    }

    fn build_rig_with(cx: &mut App, setup: Setup) -> Rig {
        let fake = Fake::new();
        let launched = Arc::new(Recorded::default());
        let asked = Rc::new(std::cell::RefCell::new(0));
        let quit = Rc::new(std::cell::RefCell::new(0));
        let say_yes = Rc::new(std::cell::Cell::new(true));
        let busy = Rc::new(std::cell::Cell::new(false));
        let dir = setup.dir.clone().unwrap_or_else(|| temp_dir("model"));
        let hooks = Hooks {
            confirm_quit: {
                let (asked, say_yes) = (asked.clone(), say_yes.clone());
                Rc::new(move |cx, go| {
                    *asked.borrow_mut() += 1;
                    if say_yes.get() {
                        go(cx);
                    }
                })
            },
            quit: {
                let quit = quit.clone();
                Rc::new(move |_| *quit.borrow_mut() += 1)
            },
            busy: {
                let busy = busy.clone();
                Rc::new(move |_| busy.get())
            },
        };
        let config = Config {
            current: setup.current.clone(),
            policy: setup.policy,
            work_dir: dir.join("work"),
            legacy_dir: None,
            marks: Some(dir.join("updater.json")),
            returning: setup.returning,
            started_in_tray: setup.in_tray,
            public_key: PUBKEY.to_string(),
            scoop: setup.scoop,
            sibling: setup.sibling,
            pid: 4242,
            clock,
        };
        let events = Rc::new(std::cell::RefCell::new(Vec::new()));
        let updater = {
            let source: Arc<dyn Source> = fake.clone();
            let launcher: Arc<dyn Launcher> = launched.clone();
            let events = events.clone();
            let updater = cx.new(|_| {
                let mut updater = Updater::new(source, launcher, hooks, config);
                updater.auto = setup.auto;
                updater
            });
            cx.subscribe(&updater, move |_, event: &UpdaterEvent, _| {
                events.borrow_mut().push(event.clone())
            })
            .detach();
            updater
        };
        Rig {
            fake,
            launched,
            asked,
            quit,
            say_yes,
            busy,
            updater,
            events,
            dir,
        }
    }

    /// Runs the executor until `done`, as the headless windows' tests do for core's work.
    #[track_caller]
    fn settle(cx: &mut TestAppContext, mut done: impl FnMut(&mut TestAppContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the updater never reached the state the test waits for");
    }

    /// Lets whatever is in flight finish, for tests that check that nothing happened.
    fn idle(cx: &mut TestAppContext) {
        for _ in 0..20 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn read<T>(cx: &mut TestAppContext, rig: &Rig, f: impl FnOnce(&Updater) -> T) -> T {
        cx.update(|cx| f(rig.updater.read(cx)))
    }

    fn check(cx: &mut TestAppContext, rig: &Rig, manual: bool) {
        cx.update(|cx| rig.updater.update(cx, |u, cx| u.check(manual, cx)));
        settle(cx, |cx| read(cx, rig, |u| *u.status() != Status::Checking));
    }

    fn install(cx: &mut TestAppContext, rig: &Rig) {
        cx.update(|cx| rig.updater.update(cx, |u, cx| u.install(cx)));
    }

    fn start(cx: &mut TestAppContext, rig: &Rig) {
        cx.update(|cx| rig.updater.update(cx, |u, cx| u.start(cx)));
    }

    fn tick(cx: &mut TestAppContext, rig: &Rig) {
        cx.update(|cx| rig.updater.update(cx, |u, cx| u.tick(cx)));
        settle(cx, |cx| {
            read(cx, rig, |u| {
                *u.status() != Status::Checking
                    && !matches!(u.install_state(), Install::Downloading { .. } | Install::Verifying)
            })
        });
    }

    fn runs(rig: &Rig) -> Vec<(PathBuf, String)> {
        lock(&rig.launched.runs).clone()
    }

    fn manifests(rig: &Rig) -> usize {
        rig.fake.asked().iter().filter(|a| *a == "manifest").count()
    }

    #[gpui_kit::test]
    fn a_newer_version_is_found_once_and_told_once(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, false);
        assert_eq!(
            read(cx, &rig, |u| u.available().map(|a| a.version.clone())).as_deref(),
            Some("0.99.0")
        );
        assert_eq!(
            rig.events.borrow().as_slice(),
            [UpdaterEvent::Found {
                version: "0.99.0".into(),
                offer: Offer::Install
            }]
        );
        // It keeps looking (a newer release could come), and says nothing more about this one.
        check(cx, &rig, false);
        assert_eq!(rig.fake.asked(), vec!["manifest", "manifest"]);
        assert_eq!(rig.events.borrow().len(), 1);
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_newer_release_takes_the_place_of_the_one_offered(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.98.0", SIG_OLD_VERSION);
        check(cx, &rig, false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, false);
        assert_eq!(
            read(cx, &rig, |u| u.available().map(|a| a.version.clone())).as_deref(),
            Some("0.99.0")
        );
        let told: Vec<String> = rig
            .events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                UpdaterEvent::Found { version, .. } => Some(version.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(told, ["0.98.0", "0.99.0"]);
        // An older answer (a cache that lags) doesn't take it back.
        rig.fake.serve("0.98.0", SIG_OLD_VERSION);
        check(cx, &rig, false);
        assert_eq!(
            read(cx, &rig, |u| u.available().map(|a| a.version.clone())).as_deref(),
            Some("0.99.0")
        );
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_secondary_window_never_looks_for_updates_greets_or_writes_the_marks(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let _secondary = crate::platform::secondary::as_secondary(crate::platform::secondary::window_on("C:\\"));
        let rig = rig_with(cx, Setup::new(Policy::SelfUpdating, "0.9.5").auto());
        rig.fake.serve("0.99.0", SIG_GOOD);
        start(cx, &rig);
        cx.update(|cx| rig.updater.update(cx, |u, cx| u.check(true, cx)));
        idle(cx);
        assert!(rig.fake.asked().is_empty(), "nothing asked of the feed or GitHub");
        assert!(read(cx, &rig, |u| u.available().is_none() && *u.status() == Status::Idle));
        assert!(rig.events.borrow().is_empty());
        assert!(!rig.dir.join("updater.json").exists(), "no marks written");
        assert!(!cx.update(|cx| rig.updater.update(cx, |u, _| u.install_on_quit())));
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_version_already_told_about_is_not_told_again_after_a_restart(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        // The run before told about 0.99.0 and wrote it down.
        std::fs::write(
            rig.dir.join("updater.json"),
            r#"{"lastVersion":"0.9.5","noticed":"0.99.0"}"#,
        )
        .expect("write");
        start(cx, &rig);
        settle(cx, |cx| read(cx, &rig, |u| u.available().is_some()));
        assert!(rig.events.borrow().is_empty(), "{:?}", rig.events.borrow());
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_manual_check_says_nothing_but_shows_the_outcome(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        check(cx, &rig, true);
        assert_eq!(read(cx, &rig, |u| u.status().clone()), Status::Latest);
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, true);
        assert!(read(cx, &rig, |u| u.available().is_some()));
        assert!(
            rig.events.borrow().is_empty(),
            "a check the person asked for needs no notice"
        );
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn an_older_or_equal_version_leaves_the_status_latest(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.99.0", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, false);
        assert_eq!(read(cx, &rig, |u| u.status().clone()), Status::Latest);
        assert!(read(cx, &rig, |u| u.available().is_none()));
        assert!(rig.events.borrow().is_empty());
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_failed_check_is_quiet_and_can_be_tried_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        *lock(&rig.fake.manifest) = Err("error sending request".into());
        check(cx, &rig, false);
        assert_eq!(
            read(cx, &rig, |u| u.status().clone()),
            Status::Failed("GitHub answered 500 Internal Server Error".into())
        );
        assert!(rig.events.borrow().is_empty());
        *lock(&rig.fake.manifest) = Ok(None);
        check(cx, &rig, true);
        assert_eq!(read(cx, &rig, |u| u.status().clone()), Status::Latest);
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn glint_looks_again_by_the_wall_clock_and_sooner_after_a_failure(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        check(cx, &rig, false);
        assert_eq!(manifests(&rig), 1);
        // A tick within the hour asks nothing (time asleep counts: it is the wall clock).
        turn_clock(Duration::from_secs(50 * 60));
        tick(cx, &rig);
        assert_eq!(manifests(&rig), 1);
        turn_clock(Duration::from_secs(11 * 60));
        tick(cx, &rig);
        assert_eq!(manifests(&rig), 2, "an hour on, it looks again");
        // Offline: the next look comes ten minutes later, not an hour.
        rig.fake.fail("error sending request");
        turn_clock(CHECK_EVERY);
        tick(cx, &rig);
        assert_eq!(manifests(&rig), 3);
        assert!(matches!(read(cx, &rig, |u| u.status().clone()), Status::Failed(_)));
        turn_clock(RETRY_AFTER);
        tick(cx, &rig);
        assert_eq!(manifests(&rig), 4);
        // A clock turned back counts as long ago.
        CLOCK.fetch_sub(3 * 24 * 3600, AtomicOrdering::SeqCst);
        tick(cx, &rig);
        assert_eq!(manifests(&rig), 5);
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn coming_back_to_the_window_after_a_while_looks_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        let shown = |cx: &mut TestAppContext| {
            cx.update(|cx| rig.updater.update(cx, |u, cx| u.window_shown(cx)));
            settle(cx, |cx| read(cx, &rig, |u| *u.status() != Status::Checking));
        };
        shown(cx);
        assert_eq!(manifests(&rig), 0, "the start looks first");
        check(cx, &rig, false);
        turn_clock(Duration::from_secs(5 * 60));
        shown(cx);
        assert_eq!(manifests(&rig), 1);
        turn_clock(LOOK_AGAIN_AFTER_AWAY);
        shown(cx);
        assert_eq!(manifests(&rig), 2);
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn the_store_copy_never_looks_and_never_installs(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::Store, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        assert_eq!(read(cx, &rig, |u| u.status().clone()), Status::Store);
        check(cx, &rig, true);
        check(cx, &rig, false);
        install(cx, &rig);
        cx.run_until_parked();
        assert_eq!(read(cx, &rig, |u| u.status().clone()), Status::Store);
        assert!(read(cx, &rig, |u| u.available().is_none()));
        assert!(
            rig.fake.asked().is_empty(),
            "the Store's copy asks nobody: {:?}",
            rig.fake.asked()
        );
        assert!(runs(&rig).is_empty());
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_debug_build_never_updates(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::Off, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, true);
        assert_eq!(read(cx, &rig, |u| u.status().clone()), Status::Off);
        assert!(rig.fake.asked().is_empty());
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn installing_downloads_with_progress_checks_the_signature_asks_and_runs_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, true);
        install(cx, &rig);
        settle(cx, |_| *rig.quit.borrow() == 1);
        // It asked the questions before quitting, once, and only after the signature passed.
        assert_eq!(*rig.asked.borrow(), 1);
        let runs = runs(&rig);
        assert_eq!(runs.len(), 1);
        let (path, parameters) = &runs[0];
        assert_eq!(
            parameters, "/P /UPDATE /R /ARGS",
            "restarted with its window, its tabs as kept"
        );
        assert_eq!(std::fs::read(path).expect("the downloaded file"), INSTALLER);
        assert_eq!(path, &rig.dir.join("work").join("Glint_0.99.0_x64-setup.exe"));
        assert!(rig.dir.join("work").join(READY_FILE).exists(), "kept for a later start");
        // The progress went out in steps ending at 100, then the rest of the story in order.
        let events = rig.events.borrow().clone();
        let percents: Vec<Option<u8>> = events
            .iter()
            .filter_map(|e| match e {
                UpdaterEvent::Downloading { percent, .. } => Some(*percent),
                _ => None,
            })
            .collect();
        assert_eq!(percents.first(), Some(&Some(0)));
        assert_eq!(percents.last(), Some(&Some(100)));
        assert!(percents.windows(2).all(|w| w[0] <= w[1]), "{percents:?}");
        assert!(events.contains(&UpdaterEvent::Ready {
            version: "0.99.0".into(),
            waiting: false
        }));
        assert_eq!(
            events.last(),
            Some(&UpdaterEvent::Installing {
                version: "0.99.0".into()
            })
        );
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_bad_signature_is_refused_and_nothing_runs(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        for (signature, words) in [
            (SIG_OTHER_KEY, "wasn't signed by Glint's key"),
            (SIG_OLD_VERSION, "signed as version 0.98.0"),
            (SIG_NO_VERSION, "doesn't name its version"),
        ] {
            let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
            rig.fake.serve("0.99.0", signature);
            check(cx, &rig, true);
            install(cx, &rig);
            settle(cx, |cx| read(cx, &rig, |u| u.install_error().is_some()));
            let error = read(cx, &rig, |u| u.install_error().map(str::to_string)).expect("a reason");
            assert!(error.contains(words), "{error}");
            assert!(runs(&rig).is_empty(), "a refused update must not run");
            assert_eq!(*rig.asked.borrow(), 0, "nothing to ask about");
            assert_eq!(*rig.quit.borrow(), 0);
            assert_eq!(read(cx, &rig, |u| u.install_state().clone()), Install::None);
            // The refused file is gone, and the person can still get it by hand.
            assert!(read(cx, &rig, |u| u.downloaded().is_none()));
            let work = rig.dir.join("work");
            let left = std::fs::read_dir(&work).map(|d| d.flatten().count()).unwrap_or(0);
            assert_eq!(left, 0, "the refused download was left in {}", work.display());
            assert!(matches!(rig.events.borrow().last(), Some(UpdaterEvent::Failed { .. })));
            let _ = std::fs::remove_dir_all(&rig.dir);
        }
    }

    #[gpui_kit::test]
    fn a_tampered_download_is_refused(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        *lock(&rig.fake.payload) = b"MZ pretend installer for Glint 0.99.0 plus malware".to_vec();
        check(cx, &rig, true);
        install(cx, &rig);
        settle(cx, |cx| read(cx, &rig, |u| u.install_error().is_some()));
        let error = read(cx, &rig, |u| u.install_error().map(str::to_string)).expect("a reason");
        assert!(error.contains("doesn't match its signature"), "{error}");
        assert!(runs(&rig).is_empty());
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_download_that_breaks_off_is_picked_up_where_it_stopped(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        *lock(&rig.fake.download_error) = Some("connection reset".into());
        check(cx, &rig, true);
        install(cx, &rig);
        settle(cx, |cx| read(cx, &rig, |u| u.install_error().is_some()));
        assert_eq!(
            read(cx, &rig, |u| u.install_error().map(str::to_string)).as_deref(),
            Some("The download failed: connection reset")
        );
        assert!(runs(&rig).is_empty());
        let part = rig.dir.join("work").join("Glint_0.99.0_x64-setup.exe.part");
        let kept = std::fs::metadata(&part).expect("what came is kept").len();
        assert!(kept > 0 && kept < INSTALLER.len() as u64, "{kept}");
        // Trying again asks for the rest only, and ends with the whole file.
        *lock(&rig.fake.download_error) = None;
        install(cx, &rig);
        settle(cx, |_| *rig.quit.borrow() == 1);
        assert_eq!(lock(&rig.fake.froms).clone(), [0, kept]);
        let (path, _) = runs(&rig).remove(0);
        assert_eq!(std::fs::read(path).expect("the file"), INSTALLER);
        assert!(!part.exists());
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_server_that_sends_it_whole_again_is_taken_from_the_start(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        *lock(&rig.fake.ranges) = false;
        *lock(&rig.fake.download_error) = Some("connection reset".into());
        check(cx, &rig, true);
        install(cx, &rig);
        settle(cx, |cx| read(cx, &rig, |u| u.install_error().is_some()));
        *lock(&rig.fake.download_error) = None;
        install(cx, &rig);
        settle(cx, |_| *rig.quit.borrow() == 1);
        let (path, _) = runs(&rig).remove(0);
        assert_eq!(
            std::fs::read(path).expect("the file"),
            INSTALLER,
            "not the kept part twice"
        );
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn saying_no_to_the_questions_keeps_the_download_and_install_asks_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        rig.say_yes.set(false);
        check(cx, &rig, true);
        install(cx, &rig);
        settle(cx, |cx| read(cx, &rig, |u| *u.install_state() == Install::Ready));
        assert_eq!(*rig.asked.borrow(), 1);
        assert!(runs(&rig).is_empty(), "the copy that is still running said no");
        assert_eq!(*rig.quit.borrow(), 0);
        // The copy finished: Install asks again, and doesn't download again.
        rig.say_yes.set(true);
        install(cx, &rig);
        settle(cx, |_| *rig.quit.borrow() == 1);
        assert_eq!(*rig.asked.borrow(), 2);
        assert_eq!(rig.fake.downloads(), 1);
        assert_eq!(runs(&rig).len(), 1);
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_setup_program_that_will_not_start_leaves_glint_running(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        *lock(&rig.launched.fail) = Some("Windows didn't let the installer start".into());
        check(cx, &rig, true);
        install(cx, &rig);
        settle(cx, |cx| read(cx, &rig, |u| u.install_error().is_some()));
        assert_eq!(
            *rig.quit.borrow(),
            0,
            "Glint keeps running when the installer didn't start"
        );
        assert!(matches!(
            rig.events.borrow().last(),
            Some(UpdaterEvent::Failed { reason, url })
                if reason.contains("didn't let the installer start") && url.ends_with("/tag/v0.99.0")
        ));
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_file_swapped_after_it_was_checked_is_not_run(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        rig.say_yes.set(false);
        check(cx, &rig, true);
        install(cx, &rig);
        settle(cx, |cx| read(cx, &rig, |u| *u.install_state() == Install::Ready));
        // Someone replaces the checked file while the question is open.
        let path = read(cx, &rig, |u| u.downloaded().map(Path::to_path_buf)).expect("the checked file");
        std::fs::write(&path, b"MZ something else entirely").expect("swap");
        rig.say_yes.set(true);
        install(cx, &rig);
        settle(cx, |cx| read(cx, &rig, |u| u.install_error().is_some()));
        assert!(runs(&rig).is_empty());
        assert_eq!(*rig.quit.borrow(), 0);
        assert!(!path.exists(), "the swapped file is removed");
        assert!(!rig.dir.join("work").join(READY_FILE).exists(), "and its record");
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn copies_that_cannot_install_themselves_open_the_release_page(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::LinkOnly, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, true);
        assert!(read(cx, &rig, |u| u.available().is_some_and(|a| a.installer.is_none())));
        install(cx, &rig);
        cx.run_until_parked();
        assert_eq!(
            cx.opened_url().as_deref(),
            Some("https://github.com/shadijhade/glint/releases/tag/v0.99.0")
        );
        assert_eq!(rig.fake.downloads(), 0);
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn the_first_start_after_an_update_greets_once(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        // Ran 0.9.5 before, runs 0.10.0 now.
        let rig = rig(cx, Policy::SelfUpdating, "0.10.0", false);
        std::fs::write(rig.dir.join("updater.json"), r#"{"lastVersion":"0.9.5"}"#).expect("write");
        start(cx, &rig);
        settle(cx, |_| !rig.events.borrow().is_empty());
        assert_eq!(
            rig.events.borrow().as_slice(),
            [UpdaterEvent::Updated {
                version: "0.10.0".into()
            }]
        );
        // It wrote the new version down, so the next start is quiet.
        settle(cx, |_| {
            std::fs::read_to_string(rig.dir.join("updater.json")).is_ok_and(|text| text.contains("0.10.0"))
        });
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_fresh_install_says_nothing_but_a_returning_person_is_greeted(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fresh = rig(cx, Policy::SelfUpdating, "0.10.0", false);
        start(cx, &fresh);
        settle(cx, |cx| read(cx, &fresh, |u| *u.status() == Status::Latest));
        assert!(fresh.events.borrow().is_empty());
        // It wrote the version down, so a later start knows this one ran.
        settle(cx, |_| fresh.dir.join("updater.json").exists());
        let _ = std::fs::remove_dir_all(&fresh.dir);

        // Settings from a version that kept no version of its own: an update, by its settings.
        let returning = rig(cx, Policy::SelfUpdating, "0.10.0", true);
        start(cx, &returning);
        settle(cx, |_| !returning.events.borrow().is_empty());
        assert_eq!(
            returning.events.borrow().as_slice(),
            [UpdaterEvent::Updated {
                version: "0.10.0".into()
            }]
        );
        let _ = std::fs::remove_dir_all(&returning.dir);
    }

    #[gpui_kit::test]
    fn going_back_to_an_older_copy_is_not_greeted(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        // The setup program's 0.13.1 ran last; now Scoop's older 0.12.0 starts.
        let rig = rig(cx, Policy::SelfUpdating, "0.12.0", true);
        std::fs::write(rig.dir.join("updater.json"), r#"{"lastVersion":"0.13.1"}"#).expect("write");
        start(cx, &rig);
        settle(cx, |cx| read(cx, &rig, |u| *u.status() == Status::Latest));
        assert!(rig.events.borrow().is_empty(), "{:?}", rig.events.borrow());
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    // ---- By itself

    #[gpui_kit::test]
    fn an_update_comes_in_by_itself_says_it_is_ready_and_restarts_on_a_click(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig_with(cx, Setup::new(Policy::SelfUpdating, "0.9.5").auto());
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, false);
        settle(cx, |cx| read(cx, &rig, |u| *u.install_state() == Install::Ready));
        // No progress notices for a download nobody asked for, and nothing runs until someone says so.
        assert_eq!(
            rig.events.borrow().as_slice(),
            [UpdaterEvent::Ready {
                version: "0.99.0".into(),
                waiting: true
            }]
        );
        assert!(runs(&rig).is_empty());
        assert_eq!(*rig.quit.borrow(), 0);
        install(cx, &rig);
        settle(cx, |_| *rig.quit.borrow() == 1);
        assert_eq!(runs(&rig)[0].1, "/P /UPDATE /R /ARGS");
        assert_eq!(rig.fake.downloads(), 1, "not downloaded twice");
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn turning_automatic_updates_on_fetches_an_update_already_known(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, false);
        assert_eq!(rig.fake.downloads(), 0, "off: it only says so");
        cx.update(|cx| rig.updater.update(cx, |u, cx| u.set_auto(true, cx)));
        settle(cx, |cx| read(cx, &rig, |u| *u.install_state() == Install::Ready));
        assert_eq!(rig.fake.downloads(), 1);
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_metered_connection_waits_for_a_click(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig_with(cx, Setup::new(Policy::SelfUpdating, "0.9.5").auto());
        rig.fake.serve("0.99.0", SIG_GOOD);
        *lock(&rig.fake.metered) = true;
        check(cx, &rig, false);
        idle(cx);
        assert_eq!(rig.fake.downloads(), 0);
        assert_eq!(
            rig.events.borrow().as_slice(),
            [UpdaterEvent::Found {
                version: "0.99.0".into(),
                offer: Offer::Install
            }]
        );
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_download_by_itself_that_breaks_off_is_offered_and_tried_again_later(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig_with(cx, Setup::new(Policy::SelfUpdating, "0.9.5").auto());
        rig.fake.serve("0.99.0", SIG_GOOD);
        *lock(&rig.fake.download_error) = Some("connection reset".into());
        check(cx, &rig, false);
        settle(cx, |cx| read(cx, &rig, |u| u.install_error().is_some()));
        // No error notice: the update is offered as it would have been.
        assert_eq!(
            rig.events.borrow().as_slice(),
            [UpdaterEvent::Found {
                version: "0.99.0".into(),
                offer: Offer::Install
            }]
        );
        *lock(&rig.fake.download_error) = None;
        tick(cx, &rig);
        assert_eq!(rig.fake.downloads(), 1, "not again within ten minutes");
        turn_clock(RETRY_AFTER);
        tick(cx, &rig);
        settle(cx, |cx| read(cx, &rig, |u| *u.install_state() == Install::Ready));
        assert_eq!(rig.fake.downloads(), 2);
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_ready_update_installs_silently_when_glint_quits(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig_with(cx, Setup::new(Policy::SelfUpdating, "0.9.5").auto());
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, false);
        settle(cx, |cx| read(cx, &rig, |u| *u.install_state() == Install::Ready));
        assert!(cx.update(|cx| rig.updater.update(cx, |u, _| u.install_on_quit())));
        let runs = runs(&rig);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].1, "/S /UPDATE", "silent, and nothing starts after it");
        assert_eq!(*rig.quit.borrow(), 0, "Glint was quitting anyway");
        assert_eq!(*rig.asked.borrow(), 0);
        // Once is enough.
        assert!(!cx.update(|cx| rig.updater.update(cx, |u, _| u.install_on_quit())));
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn nothing_installs_on_the_way_out_when_it_should_not(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let quit_installs =
            |cx: &mut TestAppContext, rig: &Rig| cx.update(|cx| rig.updater.update(cx, |u, _| u.install_on_quit()));
        // Off: it waits for a click.
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        rig.say_yes.set(false);
        check(cx, &rig, true);
        install(cx, &rig);
        settle(cx, |cx| read(cx, &rig, |u| *u.install_state() == Install::Ready));
        assert!(!quit_installs(cx, &rig));
        // On, but Windows is signing out, or an agent's MCP server runs this Glint.exe.
        cx.update(|cx| rig.updater.update(cx, |u, cx| u.set_auto(true, cx)));
        *lock(&rig.launched.ending) = true;
        assert!(!quit_installs(cx, &rig));
        *lock(&rig.launched.ending) = false;
        *lock(&rig.launched.others) = true;
        assert!(!quit_installs(cx, &rig));
        assert!(runs(&rig).is_empty());
        let _ = std::fs::remove_dir_all(&rig.dir);
        // Nothing downloaded yet.
        let rig = rig_with(cx, Setup::new(Policy::SelfUpdating, "0.9.5").auto());
        *lock(&rig.fake.metered) = true;
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, false);
        assert!(!quit_installs(cx, &rig));
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_checked_update_waits_through_a_restart_and_goes_once_installed(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let first = rig_with(cx, Setup::new(Policy::SelfUpdating, "0.9.5").auto());
        first.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &first, false);
        settle(cx, |cx| read(cx, &first, |u| *u.install_state() == Install::Ready));
        settle(cx, |_| {
            std::fs::read_to_string(first.dir.join("updater.json")).is_ok_and(|t| t.contains("0.99.0"))
        });
        // Glint ends without installing it (the PC shut down); the next start has it, without the network.
        let again = rig_with(
            cx,
            Setup {
                dir: Some(first.dir.clone()),
                ..Setup::new(Policy::SelfUpdating, "0.9.5").auto()
            },
        );
        start(cx, &again);
        settle(cx, |cx| {
            read(cx, &again, |u| {
                *u.install_state() == Install::Ready && *u.status() != Status::Checking
            })
        });
        assert_eq!(again.fake.downloads(), 0);
        assert_eq!(
            read(cx, &again, |u| u.available().map(|a| a.version.clone())).as_deref(),
            Some("0.99.0")
        );
        assert!(
            again.events.borrow().is_empty(),
            "told once already: {:?}",
            again.events.borrow()
        );
        // Installed: the start of 0.99.0 clears it away.
        let installed = rig_with(
            cx,
            Setup {
                dir: Some(first.dir.clone()),
                ..Setup::new(Policy::SelfUpdating, "0.99.0").auto()
            },
        );
        start(cx, &installed);
        settle(cx, |cx| read(cx, &installed, |u| *u.status() == Status::Latest));
        let work = first.dir.join("work");
        assert!(!work.join(READY_FILE).exists());
        assert!(!work.join("Glint_0.99.0_x64-setup.exe").exists());
        let _ = std::fs::remove_dir_all(&first.dir);
    }

    #[gpui_kit::test]
    fn after_a_start_in_the_tray_an_update_installs_before_anyone_looks(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig_with(
            cx,
            Setup {
                in_tray: true,
                ..Setup::new(Policy::SelfUpdating, "0.9.5").auto()
            },
        );
        rig.fake.serve("0.99.0", SIG_GOOD);
        // Start-up has settled.
        turn_clock(SETTLE);
        check(cx, &rig, false);
        settle(cx, |_| *rig.quit.borrow() == 1);
        assert_eq!(
            runs(&rig)[0].1,
            "/S /UPDATE /R /ARGS --tray",
            "silently, and back in the tray"
        );
        assert_eq!(*rig.asked.borrow(), 0, "nothing to ask: nobody is there");
        assert!(
            !rig.events
                .borrow()
                .iter()
                .any(|e| matches!(e, UpdaterEvent::Ready { waiting: true, .. })),
            "nobody to tell"
        );
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn an_update_waits_once_the_window_was_opened_or_while_something_goes_on(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let opened = rig_with(
            cx,
            Setup {
                in_tray: true,
                ..Setup::new(Policy::SelfUpdating, "0.9.5").auto()
            },
        );
        opened.fake.serve("0.99.0", SIG_GOOD);
        turn_clock(SETTLE);
        cx.update(|cx| opened.updater.update(cx, |u, cx| u.window_shown(cx)));
        check(cx, &opened, false);
        settle(cx, |cx| read(cx, &opened, |u| *u.install_state() == Install::Ready));
        idle(cx);
        assert!(runs(&opened).is_empty());
        assert!(opened.events.borrow().contains(&UpdaterEvent::Ready {
            version: "0.99.0".into(),
            waiting: true
        }));
        let _ = std::fs::remove_dir_all(&opened.dir);

        let busy = rig_with(
            cx,
            Setup {
                in_tray: true,
                ..Setup::new(Policy::SelfUpdating, "0.9.5").auto()
            },
        );
        busy.fake.serve("0.99.0", SIG_GOOD);
        busy.busy.set(true);
        turn_clock(SETTLE);
        check(cx, &busy, false);
        settle(cx, |cx| read(cx, &busy, |u| *u.install_state() == Install::Ready));
        idle(cx);
        assert!(runs(&busy).is_empty(), "a copy is running");
        // The copy ends; the next tick installs it.
        busy.busy.set(false);
        tick(cx, &busy);
        settle(cx, |_| *busy.quit.borrow() == 1);
        assert_eq!(runs(&busy)[0].1, "/S /UPDATE /R /ARGS --tray");
        let _ = std::fs::remove_dir_all(&busy.dir);
    }

    // ---- Scoop

    fn scoop_copy(dir: &Path, version: &str) -> scoop::Copy {
        scoop::Copy {
            name: "glint".into(),
            root: dir.join("scoop"),
            version: Some(version.into()),
            bucket: Some("shadijhade".into()),
            exe: "Glint.exe".into(),
        }
    }

    fn job_of(rig: &Rig, name: &str) -> serde_json::Value {
        let text = std::fs::read_to_string(rig.dir.join("work").join(name)).expect("the helper's job");
        serde_json::from_str(&text).expect("json")
    }

    #[gpui_kit::test]
    fn a_scoop_copy_hands_its_update_to_scoop_and_starts_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("scoop-copy");
        let rig = rig_with(
            cx,
            Setup {
                scoop: Some(scoop_copy(&dir, "0.9.5")),
                dir: Some(dir),
                ..Setup::new(Policy::Scoop, "0.9.5")
            },
        );
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, false);
        assert_eq!(
            rig.events.borrow().as_slice(),
            [UpdaterEvent::Found {
                version: "0.99.0".into(),
                offer: Offer::Scoop
            }]
        );
        assert!(read(cx, &rig, |u| u.available().is_some_and(|a| a.installer.is_none())));
        install(cx, &rig);
        settle(cx, |_| *rig.quit.borrow() == 1);
        assert_eq!(*rig.asked.borrow(), 1, "the questions before quitting first");
        assert_eq!(rig.fake.downloads(), 0, "Scoop downloads it");
        let spawned = lock(&rig.launched.spawned).clone();
        assert_eq!(spawned.len(), 1);
        let (program, args, cwd) = &spawned[0];
        assert!(
            program.to_string_lossy().to_lowercase().ends_with("powershell.exe"),
            "{}",
            program.display()
        );
        assert_eq!(
            cwd,
            &rig.dir.join("work"),
            "never in Scoop's folder, which Scoop replaces"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some(rig.dir.join("work").join(scoop::JOB).to_str().unwrap())
        );
        let job = job_of(&rig, scoop::JOB);
        assert_eq!(job["target"], "0.99.0");
        assert_eq!(job["waitFor"], 4242);
        assert_eq!(job["app"], "glint");
        assert_eq!(
            job["relaunch"].as_str(),
            Some(rig.dir.join(r"scoop\apps\glint\current\Glint.exe").to_str().unwrap())
        );
        assert_eq!(job["relaunchArgs"], serde_json::json!([]));
        assert!(rig.dir.join("work").join(scoop::SCRIPT_NAME).exists());
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_scoop_copy_updates_through_scoop_on_the_way_out_and_says_when_it_did_not(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("scoop-quit");
        let setup = Setup {
            scoop: Some(scoop_copy(&dir, "0.9.5")),
            dir: Some(dir.clone()),
            ..Setup::new(Policy::Scoop, "0.9.5").auto()
        };
        let rig = rig_with(cx, setup.clone());
        rig.fake.serve("0.99.0", SIG_GOOD);
        check(cx, &rig, false);
        assert!(cx.update(|cx| rig.updater.update(cx, |u, _| u.install_on_quit())));
        let job = job_of(&rig, scoop::JOB);
        assert_eq!((job["waitFor"].as_u64(), job["relaunch"].is_null()), (Some(4242), true));
        // Scoop didn't manage (the helper wrote why); the next start says so, once.
        std::fs::write(
            dir.join("work").join(scoop::RESULT),
            "\u{feff}{\"target\":\"0.99.0\",\"before\":\"0.9.5\",\"after\":\"0.9.5\",\"ok\":false,\
             \"message\":\"Updating 'glint'\\nERROR The following instances of \\\"glint\\\" are still running.\"}",
        )
        .unwrap();
        let again = rig_with(cx, setup);
        start(cx, &again);
        settle(cx, |_| !again.events.borrow().is_empty());
        assert!(
            matches!(
                again.events.borrow().first(),
                Some(UpdaterEvent::Failed { reason, url })
                    if reason.starts_with("Scoop didn't update Glint: ERROR") && url.ends_with("/tag/v0.99.0")
            ),
            "{:?}",
            again.events.borrow()
        );
        assert!(!dir.join("work").join(scoop::RESULT).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui_kit::test]
    fn a_scoop_copy_beside_the_setups_copy_is_brought_along(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("sibling");
        let setup = Setup {
            sibling: Some(scoop_copy(&dir, "0.12.0")),
            dir: Some(dir.clone()),
            ..Setup::new(Policy::SelfUpdating, "0.13.1").auto()
        };
        let spawns = |rig: &Rig| lock(&rig.launched.spawned).len();
        let rig = rig_with(cx, setup.clone());
        start(cx, &rig);
        settle(cx, |_| spawns(&rig) == 1);
        let job = job_of(&rig, scoop::SIBLING_JOB);
        assert_eq!(job["target"], "0.13.1");
        assert_eq!(
            (job["waitFor"].as_u64(), job["relaunch"].is_null()),
            (Some(0), true),
            "nothing waits, nothing starts"
        );
        settle(cx, |_| {
            std::fs::read_to_string(dir.join("updater.json")).is_ok_and(|t| t.contains("siblingTried"))
        });
        // Not at every start: again after a while, if Scoop's copy is still behind.
        let again = rig_with(cx, setup.clone());
        start(cx, &again);
        idle(cx);
        assert_eq!(spawns(&again), 0);
        turn_clock(SIBLING_AGAIN);
        let later = rig_with(cx, setup.clone());
        start(cx, &later);
        settle(cx, |_| spawns(&later) == 1);
        // In step already, or automatic updates off: left alone.
        for setup in [
            Setup {
                sibling: Some(scoop_copy(&dir, "0.13.1")),
                ..setup.clone()
            },
            Setup {
                auto: false,
                ..setup.clone()
            },
        ] {
            turn_clock(SIBLING_AGAIN);
            let rig = rig_with(cx, setup);
            start(cx, &rig);
            idle(cx);
            assert_eq!(spawns(&rig), 0);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui_kit::test]
    fn an_install_that_did_not_take_waits_for_restart_now(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig_with(
            cx,
            Setup {
                in_tray: true,
                ..Setup::new(Policy::SelfUpdating, "0.9.5").auto()
            },
        );
        // The last run started the setup program for 0.99.0, and 0.9.5 is still what runs.
        std::fs::write(
            rig.dir.join("updater.json"),
            r#"{"lastVersion":"0.9.5","installing":"0.99.0"}"#,
        )
        .expect("write");
        rig.fake.serve("0.99.0", SIG_GOOD);
        turn_clock(SETTLE);
        start(cx, &rig);
        settle(cx, |cx| read(cx, &rig, |u| *u.install_state() == Install::Ready));
        idle(cx);
        assert!(runs(&rig).is_empty(), "not again unseen, in the tray");
        assert!(
            !cx.update(|cx| rig.updater.update(cx, |u, _| u.install_on_quit())),
            "nor on the way out"
        );
        assert!(rig.events.borrow().contains(&UpdaterEvent::Ready {
            version: "0.99.0".into(),
            waiting: true
        }));
        // Restart now still installs it, with the setup program's progress (and its errors) showing.
        install(cx, &rig);
        settle(cx, |_| *rig.quit.borrow() == 1);
        assert_eq!(runs(&rig)[0].1, "/P /UPDATE /R /ARGS");
        let marks = std::fs::read_to_string(rig.dir.join("updater.json")).unwrap_or_default();
        assert!(marks.contains("\"installing\": \"0.99.0\""), "{marks}");
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn an_install_that_took_is_forgotten(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::SelfUpdating, "0.99.0", false);
        std::fs::write(
            rig.dir.join("updater.json"),
            r#"{"lastVersion":"0.9.5","installing":"0.99.0"}"#,
        )
        .expect("write");
        start(cx, &rig);
        settle(cx, |_| {
            std::fs::read_to_string(rig.dir.join("updater.json")).is_ok_and(|t| !t.contains("installing"))
        });
        assert!(rig.events.borrow().contains(&UpdaterEvent::Updated {
            version: "0.99.0".into()
        }));
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_failed_update_through_scoop_is_told_once(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("scoop-told");
        let setup = Setup {
            scoop: Some(scoop_copy(&dir, "0.9.5")),
            dir: Some(dir.clone()),
            ..Setup::new(Policy::Scoop, "0.9.5")
        };
        let result = r#"{"target":"0.99.0","before":"0.9.5","after":"0.9.5","ok":false,"message":"glint: 0.9.5 (latest version)"}"#;
        let failures = |rig: &Rig| {
            rig.events
                .borrow()
                .iter()
                .filter(|e| matches!(e, UpdaterEvent::Failed { .. }))
                .count()
        };
        std::fs::create_dir_all(dir.join("work")).unwrap();
        std::fs::write(dir.join("work").join(scoop::RESULT), result).unwrap();
        let first = rig_with(cx, setup.clone());
        start(cx, &first);
        settle(cx, |_| failures(&first) == 1);
        settle(cx, |_| {
            std::fs::read_to_string(dir.join("updater.json")).is_ok_and(|t| t.contains("scoopTold"))
        });
        // The same failure again (the bucket still lags): not said at every start.
        std::fs::write(dir.join("work").join(scoop::RESULT), result).unwrap();
        let again = rig_with(cx, setup);
        start(cx, &again);
        settle(cx, |cx| {
            read(cx, &again, |u| {
                *u.status() != Status::Checking && u.last_checked().is_some()
            })
        });
        idle(cx);
        assert_eq!(failures(&again), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- What the person sees

    /// A bar that draws the header's Update button, as the toolbar does.
    struct Bar;

    impl Render for Bar {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().children(update_button(cx))
        }
    }

    fn button(visual: &mut gpui_kit::VisualTestContext) -> Option<Option<String>> {
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("update").map(|e| e.label().map(str::to_string))
        })
    }

    fn wait_for(visual: &mut gpui_kit::VisualTestContext, rig: &Rig, done: impl Fn(&Updater) -> bool) {
        for _ in 0..400 {
            visual.run_until_parked();
            if visual.update(|_, cx| done(rig.updater.read(cx))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the updater never got there");
    }

    #[gpui_kit::test]
    fn the_header_button_shows_only_while_an_update_waits(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.executor().allow_parking();
        cx.update(testing::init);
        let (mut visual, bar) = testing::open_window(cx, |_, cx| cx.new(|_| Bar));
        let rig = visual.update(|_, cx| {
            let rig = build_rig(cx, Policy::SelfUpdating, "0.9.5", false);
            set_shared(cx, rig.updater.clone());
            rig
        });
        let redraw = |visual: &mut gpui_kit::VisualTestContext| {
            visual.update(|_, cx| bar.update(cx, |_, cx| cx.notify()));
            visual.run_until_parked();
        };
        assert_eq!(button(&mut visual), None, "nothing newer is known: no button");
        rig.fake.serve("0.99.0", SIG_GOOD);
        visual.update(|_, cx| rig.updater.update(cx, |u, cx| u.check(true, cx)));
        wait_for(&mut visual, &rig, |u| u.available().is_some());
        redraw(&mut visual);
        assert_eq!(button(&mut visual), Some(Some("Update".to_string())));
    }

    #[gpui_kit::test]
    fn the_header_button_restarts_once_the_update_is_in(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.executor().allow_parking();
        cx.update(testing::init);
        let (mut visual, bar) = testing::open_window(cx, |_, cx| cx.new(|_| Bar));
        let rig = visual.update(|_, cx| {
            let rig = build_rig_with(cx, Setup::new(Policy::SelfUpdating, "0.9.5").auto());
            set_shared(cx, rig.updater.clone());
            rig
        });
        rig.fake.serve("0.99.0", SIG_GOOD);
        visual.update(|_, cx| rig.updater.update(cx, |u, cx| u.check(false, cx)));
        wait_for(&mut visual, &rig, |u| *u.install_state() == Install::Ready);
        visual.update(|_, cx| bar.update(cx, |_, cx| cx.notify()));
        visual.run_until_parked();
        assert_eq!(button(&mut visual), Some(Some("Restart to update".to_string())));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(SharedString::from("update"), cx);
        });
        for _ in 0..200 {
            visual.run_until_parked();
            if *rig.quit.borrow() == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*rig.quit.borrow(), 1, "a click restarts into it");
    }

    #[gpui_kit::test]
    fn notices_wait_while_the_window_is_hidden_and_windows_says_them(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.executor().allow_parking();
        cx.update(testing::init);
        let (mut visual, _) = testing::open_window(cx, |_, cx| cx.new(|_| Bar));
        let count =
            |visual: &mut gpui_kit::VisualTestContext| visual.update(|window, cx| window.notifications(cx).len());
        // The setup program's copy has the app identity Windows' notifications need (lib.rs sets it).
        cx.update(|cx| cx.set_app_identity(glint_core::host::IDENTIFIER, "Glint"));
        pretend_hidden(true);
        let ready = UpdaterEvent::Ready {
            version: "0.99.0".into(),
            waiting: true,
        };
        for event in [
            UpdaterEvent::Found {
                version: "0.98.0".into(),
                offer: Offer::Install,
            },
            ready.clone(),
            UpdaterEvent::Downloading {
                version: "0.99.0".into(),
                percent: Some(5),
            },
            UpdaterEvent::Updated {
                version: "0.9.5".into(),
            },
        ] {
            visual.update(|window, cx| show(&event, window, cx));
        }
        visual.run_until_parked();
        assert_eq!(count(&mut visual), 0, "nothing in a window nobody sees");
        let held = visual.update(|_, cx| cx.try_global::<Held>().map(|h| h.0.clone()).unwrap_or_default());
        assert_eq!(
            held,
            [
                ready.clone(),
                UpdaterEvent::Updated {
                    version: "0.9.5".into()
                }
            ],
            "the latest offer and the greeting"
        );
        let told = cx.shown_system_notifications();
        assert_eq!(told.len(), 2, "Windows said each offer: {told:?}");
        assert_eq!(
            told.last().map(|n| n.title.to_string()).as_deref(),
            Some("Glint 0.99.0 is ready")
        );
        // Back on screen: what waited shows.
        pretend_hidden(false);
        visual.update(show_held);
        visual.run_until_parked();
        assert_eq!(count(&mut visual), 2);
        assert!(visual.update(|_, cx| cx.try_global::<Held>().is_none_or(|h| h.0.is_empty())));
    }

    #[gpui_kit::test]
    fn every_notice_can_be_shown(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.executor().allow_parking();
        cx.update(testing::init);
        let (mut visual, _) = testing::open_window(cx, |_, cx| cx.new(|_| Bar));
        let mut events = vec![];
        for offer in [Offer::Install, Offer::Scoop, Offer::Download] {
            events.push(UpdaterEvent::Found {
                version: "0.99.0".into(),
                offer,
            });
        }
        events.extend([
            UpdaterEvent::Ready {
                version: "0.99.0".into(),
                waiting: true,
            },
            UpdaterEvent::Updated {
                version: "0.99.0".into(),
            },
            UpdaterEvent::Downloading {
                version: "0.99.0".into(),
                percent: Some(40),
            },
            UpdaterEvent::Downloading {
                version: "0.99.0".into(),
                percent: None,
            },
            UpdaterEvent::Ready {
                version: "0.99.0".into(),
                waiting: false,
            },
            UpdaterEvent::Installing {
                version: "0.99.0".into(),
            },
            UpdaterEvent::Failed {
                reason: "Windows didn't let the installer start".into(),
                url: format!("{RELEASES}/tag/v0.99.0"),
            },
        ]);
        for event in &events {
            visual.update(|window, cx| {
                show(event, window, cx);
                window.render_frame(cx);
            });
            visual.run_until_parked();
        }
    }
}
