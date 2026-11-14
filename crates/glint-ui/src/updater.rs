//! Glint keeping itself up to date: latest.json, the signed installer, the Update button.
//! Ported from src/lib/updater.ts (the store and its three functions), the updater plugin Tauri ran for it
//! (`tauri.conf.json`'s `plugins.updater` and tauri-plugin-updater's Windows install), and the Update button of
//! App.tsx's header.
//!
//! **What it does**, as the TypeScript did:
//!
//! - Looks for a newer version at start and every 12 hours ([`CHECK_EVERY`], `UPDATE_CHECK_MS`), quietly: a failed
//!   check says nothing unless Settings → About asked for it.
//! - Reads the latest release's `latest.json` (the same feed, [`MANIFEST_URL`]). If that can't be read, or this copy
//!   can't install anything itself (Scoop, a portable copy), it asks GitHub's latest-release API instead, and the
//!   person is sent to the release page rather than offered an Install button.
//! - Install downloads the setup program with progress, checks its minisign signature with the key the releases are
//!   signed with ([`PUBLIC_KEY`], `requireSignedVersion` included: a signature for another version is refused)
//!   **before anything runs it**, asks first when a copy is running or Compare has edits, saves, stops the tools
//!   (the installer replaces rg, fd and pdu) and runs the setup passively with the arguments the Tauri updater used
//!   (`/P /UPDATE /R /ARGS …`, which hooks.nsh's update mode and the relaunch rely on), then quits.
//! - The Microsoft Store's copy never updates itself (the Store does), and neither does a debug build ([`Policy`]).
//! - A secondary window (`platform::secondary`) never looks, greets or writes the marks: the main process updates
//!   Glint, and two copies would both download the setup program. The model exists there (Settings → About reads it)
//!   and stays idle.
//!
//! **Where each piece is**
//!
//! | TypeScript                          | Here                                                              |
//! |-------------------------------------|-------------------------------------------------------------------|
//! | `useUpdater` (available, status…)   | [`Updater`], a model entity in a global ([`init`], [`shared`])    |
//! | `checkForUpdates(manual)`           | [`Updater::check`]                                                |
//! | `installUpdate`                     | [`Updater::install`]                                              |
//! | `greetUpdate`, `firstNotice`        | [`Updater::start`] and [`UpdaterEvent::Updated`] / `Found`        |
//! | `findUpdate`, the plugin's `check`  | [`find`], [`parse_manifest`]                                      |
//! | the plugin's `download`, `verify`   | [`Source::download`], [`verify_file`]                             |
//! | the plugin's Windows `install`      | [`Launcher`], [`installer_parameters`]                            |
//! | the header's Update button          | [`update_button`]                                                 |
//! | the toasts                          | [`show`] (words an [`UpdaterEvent`])                              |
//! | `UpdateRow` in SettingsView.tsx     | `components::settings::about` (it reads [`shared`])               |
//!
//! **Testing without a network.** The network is a [`Source`], starting the installer is a [`Launcher`], and
//! quitting and the questions before it are [`Hooks`]; the tests give the model fakes of all three. [`HttpSource`]
//! is the real source (reqwest over rustls, only ever on core's runtime, never the UI thread).
//!
//! **How the coordinator wires it** (startup.rs):
//!
//! 1. Early, before the first state write: `updater::init(cx, updater::http_source(install_tls), hooks)`, where
//!    `install_tls` installs rustls' ring provider (see [`HttpSource::new`]) and `hooks` is [`Hooks`] with
//!    `confirm_quit` (startup.rs's `ask_before_quitting` for a copy still running and Compare's edits, with the
//!    final step passed in) and `quit` (startup.rs's `quit`).
//! 2. Subscribe to the model and show what it says in the main window:
//!    `cx.subscribe(&updater::shared(cx), |_, event, cx| main_window.update(cx, |_, w, cx| updater::show(event, w, cx)))`.
//! 3. In the toolbar, before the Settings button, `.children(updater::update_button(cx))`, and observe the model
//!    so the button appears and spins: `cx.observe(&updater::shared(cx), |_, _, cx| cx.notify())`.

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
use std::time::Duration;

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

/// How often Glint looks again (`UPDATE_CHECK_MS`).
pub const CHECK_EVERY: Duration = Duration::from_secs(12 * 60 * 60);

/// The updater targets latest.json may list, most specific first (the plugin's `{os}-{arch}-{bundle}` then
/// `{os}-{arch}`).
const TARGETS: [&str; 2] = ["windows-x86_64-nsis", "windows-x86_64"];

/// No Glint setup program is anywhere near this big; a longer download is cut off.
const MAX_INSTALLER: u64 = 1 << 30;

/// What the folders of downloaded setup programs are called, so [`sweep`] knows which are Glint's.
const WORK_PREFIX: &str = "glint-update-";

/// How old a downloaded setup program's folder is before the next start removes it.
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

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

/// Takes the setup program's bytes as they arrive, with the total when the server said it. An `Err` stops the
/// download and is its failure.
pub type Sink = Box<dyn FnMut(&[u8], Option<u64>) -> Result<(), String> + Send>;

/// What the updater asks of the network. [`HttpSource`] is the real one; tests give a fake with canned answers.
pub trait Source: Send + Sync + 'static {
    /// latest.json's text; `None` when the feed says there is nothing newer (HTTP 204).
    fn manifest(&self) -> Fut<Option<String>>;
    /// GitHub's latest-release answer as text.
    fn latest_release(&self) -> Fut<String>;
    /// Streams the file at `url` into `sink`, chunk by chunk.
    fn download(&self, url: String, sink: Sink) -> Fut<()>;
}

/// Looks for a newer version than `current`: the manifest first (copies the setup program installed), then
/// GitHub's latest release. A manifest that can't be fetched or read falls through to GitHub like the
/// TypeScript's `check()` throwing; a manifest that says "nothing newer" does not.
pub async fn find(source: &dyn Source, policy: Policy, current: &Version) -> Result<Option<Available>, String> {
    if policy == Policy::SelfUpdating {
        match source.manifest().await {
            Ok(None) => return Ok(None),
            Ok(Some(text)) => {
                if let Ok(release) = parse_manifest(&text) {
                    let newer = Version::parse(&release.version).is_some_and(|v| v > *current);
                    return Ok(newer.then(|| Available {
                        url: Available::release_page(&release.version),
                        version: release.version,
                        notes: release.notes,
                        installer: release.installer,
                    }));
                }
            }
            Err(_) => {}
        }
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
            return Err(format!(
                "{what} answered {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )
            .trim()
            .to_string());
        }
        response.text().await.map(Some).map_err(|e| e.to_string())
    }
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

    fn download(&self, url: String, mut sink: Sink) -> Fut<()> {
        let this = Self { tls: self.tls };
        Box::pin(async move {
            if !url.starts_with("https://") {
                return Err("The update isn't offered over a secure connection".into());
            }
            let mut response = this.client(true)?.get(&url).send().await.map_err(|e| e.to_string())?;
            let status = response.status();
            if !status.is_success() {
                return Err(format!(
                    "The download answered {} {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                )
                .trim()
                .to_string());
            }
            let total = response.content_length();
            while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
                sink(&chunk, total)?;
            }
            Ok(())
        })
    }
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

/// The setup program's command line: passive (`/P`), as an update (`/UPDATE`, which hooks.nsh and the installer
/// template treat as "keep everything"), then start the new version again with the arguments this one had
/// (`/R /ARGS …`). The order and spelling are the plugin's, which the installer template reads.
pub fn installer_parameters(restart_args: &[Vec<u16>]) -> Vec<u16> {
    let mut parts: Vec<Vec<u16>> = ["/P", "/UPDATE", "/R", "/ARGS"]
        .iter()
        .map(|s| s.encode_utf16().collect())
        .collect();
    parts.extend(restart_args.iter().map(|arg| escape_nsis_arg(arg)));
    parts.join(&u16::from(b' '))
}

/// Starts the setup program. [`ShellLauncher`] is the real one; tests record what they would have run.
pub trait Launcher: Send + Sync + 'static {
    /// Runs `installer` with `parameters` (UTF-16, no terminator) and returns once Windows has started it.
    fn launch(&self, installer: &Path, parameters: &[u16]) -> Result<(), String>;
}

/// Starts the setup program with `ShellExecuteW`, as the plugin did.
pub struct ShellLauncher;

impl Launcher for ShellLauncher {
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
}

// ---- The model --------------------------------------------------------------------------------------------

/// Whether and how this copy updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// The setup program installed it: it downloads and installs updates itself.
    SelfUpdating,
    /// Scoop, a portable copy: it can say a newer version exists, and links to the release page.
    LinkOnly,
    /// The Microsoft Store's copy: the Store keeps it up to date, so it never looks.
    Store,
    /// A debug build: it never updates.
    Off,
}

impl Policy {
    /// This copy's policy: the Store's first (`storeCopy`), then debug builds, then by how it was installed
    /// (`installed_by_setup`: the plugin was registered only for those).
    pub fn detect() -> Policy {
        if glint_core::store_copy() {
            Policy::Store
        } else if cfg!(debug_assertions) {
            Policy::Off
        } else if glint_core::installed_by_setup() {
            Policy::SelfUpdating
        } else {
            Policy::LinkOnly
        }
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
    /// Downloaded and verified, waiting for the go-ahead (the questions before quitting said no, or are open).
    Ready,
    /// Saving, stopping the tools and starting the setup program.
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

/// What the model tells the app. [`show`] words them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdaterEvent {
    /// A check nobody asked for found a version it hasn't told about yet: "Update available" with Install
    /// (`firstNotice("update-notice")`).
    Found { version: String, installable: bool },
    /// The first start after an update: "Updated to Glint X" with What's new (`greetUpdate`).
    Updated { version: String },
    /// The download goes on: `percent` when the size is known.
    Downloading { version: String, percent: Option<u8> },
    /// Downloaded and verified; the questions before quitting come next.
    Ready { version: String },
    /// The setup program is starting and Glint is about to quit.
    Installing { version: String },
    /// Installing didn't work. `url` is the release page, for downloading it by hand.
    Failed { reason: String, url: String },
}

/// The question before quitting: given what to do on yes.
pub type ConfirmQuit = Rc<dyn Fn(&mut App, Box<dyn FnOnce(&mut App)>)>;

/// How the model reaches the rest of the app. Both run on the UI thread.
#[derive(Clone)]
pub struct Hooks {
    /// Asks what quitting asks (a copy still running, Compare's edits) and runs the callback only on yes; runs it
    /// at once when nothing is in the way. startup.rs's `ask_before_quitting` is this, with its final step passed
    /// in instead of `quit_now`.
    pub confirm_quit: ConfirmQuit,
    /// Quits Glint (startup.rs's `quit`: the window's place is kept, then GPUI quits).
    pub quit: Rc<dyn Fn(&mut App)>,
}

impl Default for Hooks {
    fn default() -> Self {
        Hooks {
            confirm_quit: Rc::new(|cx, go| go(cx)),
            quit: Rc::new(|cx| cx.quit()),
        }
    }
}

/// Everything about this copy the model needs, so tests can set it.
#[derive(Clone, Debug)]
pub struct Config {
    /// This build's version (`CARGO_PKG_VERSION`).
    pub current: String,
    pub policy: Policy,
    /// Where downloaded setup programs go (each in a folder of its own).
    pub work_dir: PathBuf,
    /// The small file that remembers the last version run and the last one told about; none, nothing is kept.
    pub marks: Option<PathBuf>,
    /// Whether this person ran Glint before (saved settings exist), so an update from a version that kept no
    /// version of its own is still greeted.
    pub returning: bool,
    /// The arguments this run was started with, which the new version is started with again.
    pub restart_args: Vec<OsString>,
    /// The minisign public key downloads are checked with: [`PUBLIC_KEY`] in the app, a throwaway key in tests.
    pub public_key: String,
}

impl Config {
    /// This copy, for the app. Read it before the first state write: `returning` looks for the saved state.
    pub fn for_app() -> Config {
        Config {
            current: env!("CARGO_PKG_VERSION").to_string(),
            policy: Policy::detect(),
            work_dir: std::env::temp_dir(),
            marks: glint_core::host::PathResolver
                .app_data_dir()
                .ok()
                .map(|dir| dir.join("updater.json")),
            returning: persistence::state_path().is_some_and(|path| path.exists()),
            restart_args: std::env::args_os().skip(1).collect(),
            public_key: PUBLIC_KEY.to_string(),
        }
    }
}

/// What is remembered between runs (the TypeScript's `last-version` and `update-notice` in localStorage).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Marks {
    /// The version that ran last.
    last_version: Option<String>,
    /// The version the "Update available" notice was last shown for.
    noticed: Option<String>,
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
    /// The downloaded, verified setup program.
    ready: Option<PathBuf>,
    marks: Marks,
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

    /// The setup program waiting to be run, once downloaded and verified.
    pub fn downloaded(&self) -> Option<&Path> {
        self.ready.as_deref()
    }

    /// At start: reads what was remembered, greets a first start after an update (`greetUpdate`), clears away
    /// old downloads, checks, and keeps checking every [`CHECK_EVERY`].
    pub fn start(&mut self, cx: &mut Context<Self>) {
        // A secondary window leaves the updating (and the marks file) to the main process.
        if crate::platform::secondary::is_secondary() {
            return;
        }
        let marks_path = self.config.marks.clone();
        let work_dir = self.config.work_dir.clone();
        let returning = self.config.returning;
        // The first start after the old app: its `last-version` and `update-notice` carry over, so the update from
        // it is greeted with the right version and an update notice already seen isn't shown again.
        let imported = crate::old_settings::imported().map(|old| Marks {
            last_version: old.last_version.clone(),
            noticed: old.notices.get("update-notice").cloned(),
        });
        cx.spawn(async move |this, cx| {
            let (marks, ()) = cx
                .background_spawn(async move {
                    sweep(&work_dir, STALE_AFTER);
                    let saved = marks_path.as_deref().filter(|path| path.exists()).map(read_marks);
                    (saved.or(imported).unwrap_or_default(), ())
                })
                .await;
            let started = this.update(cx, |this, cx| {
                this.marks = marks;
                this.greet(returning, cx);
                this.check(false, cx);
            });
            if started.is_err() {
                return;
            }
            loop {
                cx.background_executor().timer(CHECK_EVERY).await;
                if this.update(cx, |this, cx| this.check(false, cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// The first start after an update says so once, with the release's notes a click away. A fresh install has
    /// no earlier settings and says nothing; versions before 0.6 kept no version, so an update from one of them
    /// counts by its settings (`returning`).
    fn greet(&mut self, returning: bool, cx: &mut Context<Self>) {
        let version = self.config.current.clone();
        let before = self
            .marks
            .last_version
            .clone()
            .or_else(|| returning.then(|| "earlier".to_string()));
        if self.marks.last_version.as_deref() != Some(&version) {
            self.marks.last_version = Some(version.clone());
            self.save_marks(cx);
        }
        if before.is_some_and(|before| before != version) {
            cx.emit(UpdaterEvent::Updated { version });
        }
    }

    /// Whether `version` is new for the "Update available" notice, remembering that it was shown
    /// (`firstNotice`). Without storage a notice can repeat, which beats never showing it.
    fn first_notice(&mut self, version: &str, cx: &mut Context<Self>) -> bool {
        if self.marks.noticed.as_deref() == Some(version) {
            return false;
        }
        self.marks.noticed = Some(version.to_string());
        self.save_marks(cx);
        true
    }

    fn save_marks(&self, cx: &mut Context<Self>) {
        let Some(path) = self.config.marks.clone() else {
            return;
        };
        let Ok(text) = serde_json::to_string_pretty(&self.marks) else {
            return;
        };
        cx.background_spawn(async move {
            let _ = persistence::write_atomic(&path, text.as_bytes());
        })
        .detach();
    }

    /// Looks for a newer version. On its own (at start and twice a day) it says so once per version; asked from
    /// Settings (`manual`), the outcome shows there. Nothing happens once one is found, while checking, or in a
    /// copy that never updates (`checkForUpdates`).
    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if crate::platform::secondary::is_secondary() {
            return;
        }
        if self.available.is_some()
            || self.status == Status::Checking
            || matches!(self.status, Status::Store | Status::Off)
        {
            return;
        }
        self.status = Status::Checking;
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
            Ok(Some(available)) => {
                self.status = Status::Idle;
                let announce = !manual && self.first_notice(&available.version, cx);
                if announce {
                    cx.emit(UpdaterEvent::Found {
                        version: available.version.clone(),
                        installable: available.installer.is_some() && self.config.policy == Policy::SelfUpdating,
                    });
                }
                self.available = Some(available);
            }
            Ok(None) => self.status = Status::Latest,
            Err(reason) => self.status = Status::Failed(reason),
        }
        cx.notify();
    }

    /// Install (or Download, for copies that don't update themselves): the setup program downloads with
    /// progress and its signature is checked, then the questions before quitting are asked and it runs. Copies
    /// that can't install it themselves open the release page instead (`installUpdate`).
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let Some(available) = self.available.clone() else {
            return;
        };
        match self.install {
            Install::Downloading { .. } | Install::Verifying | Install::Launching => return,
            Install::Ready if self.ready.is_some() => return self.go_ahead(cx),
            _ => {}
        }
        match (&available.installer, self.config.policy) {
            (Some(installer), Policy::SelfUpdating) => self.download(available.version.clone(), installer.clone(), cx),
            (_, Policy::Store | Policy::Off) => {}
            _ => cx.open_url(&available.url),
        }
    }

    fn download(&mut self, version: String, installer: InstallerRef, cx: &mut Context<Self>) {
        self.install = Install::Downloading {
            received: 0,
            total: None,
        };
        self.install_error = None;
        cx.emit(UpdaterEvent::Downloading {
            version: version.clone(),
            percent: Some(0),
        });
        cx.notify();

        let key = self.config.public_key.clone();
        let target = Arc::new(Mutex::new(Target::new(&self.config.work_dir, &installer.url)));
        let (progress_in, progress_out) = async_channel::unbounded::<(u64, Option<u64>)>();
        let sink: Sink = {
            let target = target.clone();
            Box::new(move |chunk, total| {
                let mut target = target.lock().unwrap_or_else(|e| e.into_inner());
                target.write(chunk, total, &progress_in)
            })
        };
        let download = core_bridge::run(self.source.download(installer.url.clone(), sink));
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
            let path = target.lock().unwrap_or_else(|e| e.into_inner()).finish();
            let Ok(()) = downloaded else {
                if let Some(path) = &path {
                    remove_download(path);
                }
                let reason = downloaded.err().unwrap_or_default();
                let _ = this.update(cx, |this, cx| this.failed(format!("The download failed: {reason}"), cx));
                return;
            };
            let Some(path) = path else {
                let _ = this.update(cx, |this, cx| this.failed("The download was empty".into(), cx));
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.install = Install::Verifying;
                cx.notify();
            });
            // The signature is checked on a background thread, and before anything else touches the file.
            let checked = {
                let (path, signature, version) = (path.clone(), installer.signature.clone(), version.clone());
                cx.background_spawn(async move { verify_file(&path, &signature, &key, &version) })
                    .await
            };
            let _ = this.update(cx, |this, cx| match checked {
                Ok(()) => {
                    this.ready = Some(path);
                    this.install = Install::Ready;
                    cx.emit(UpdaterEvent::Ready { version });
                    cx.notify();
                    this.go_ahead(cx);
                }
                Err(refused) => {
                    remove_download(&path);
                    this.failed(refused.to_string(), cx);
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
        if let Some(available) = &self.available {
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

    fn failed(&mut self, reason: String, cx: &mut Context<Self>) {
        self.install = Install::None;
        self.ready = None;
        let url = self
            .available
            .as_ref()
            .map(|a| a.url.clone())
            .unwrap_or_else(|| RELEASES.to_string());
        self.install_error = Some(reason.clone());
        cx.emit(UpdaterEvent::Failed { reason, url });
        cx.notify();
    }

    /// Asks what quitting asks, and on yes runs the setup program. On no it stays [`Install::Ready`]: Install asks
    /// again without downloading again.
    fn go_ahead(&mut self, cx: &mut Context<Self>) {
        let me = cx.weak_entity();
        let confirm = self.hooks.confirm_quit.clone();
        // After this update ends: a hook that has nothing to ask calls straight back into the model.
        cx.defer(move |cx| {
            confirm(
                cx,
                Box::new(move |cx| {
                    let _ = me.update(cx, |this, cx| this.launch(cx));
                }),
            )
        });
    }

    /// Saves, stops the tools (the setup program replaces rg, fd and pdu, which it can't while they run), checks
    /// the file once more and runs it, then quits. Anything that goes wrong leaves Glint running.
    fn launch(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(available)) = (self.ready.clone(), self.available.clone()) else {
            return;
        };
        let Some(installer) = available.installer.clone() else {
            return;
        };
        if self.install == Install::Launching {
            return;
        }
        self.install = Install::Launching;
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
        let parameters = installer_parameters(&restart_args(&self.config.restart_args));
        let quit = self.hooks.quit.clone();
        let key = self.config.public_key.clone();
        cx.spawn(async move |this, cx| {
            let _ = stopping.await;
            let version = available.version.clone();
            // Checked again here, because the file has sat in a temporary folder since it was first checked.
            let started = cx
                .background_spawn(async move {
                    verify_file(&path, &installer.signature, &key, &version)
                        .map_err(|refused| (refused.to_string(), Some(path.clone())))?;
                    launcher.launch(&path, &parameters).map_err(|why| (why, None))
                })
                .await;
            match started {
                Ok(()) => {
                    cx.update(|cx| quit(cx));
                }
                Err((reason, delete)) => {
                    if let Some(path) = delete {
                        remove_download(&path);
                    }
                    let _ = this.update(cx, |this, cx| this.failed(reason, cx));
                }
            }
        })
        .detach();
    }
}

/// The arguments this run started with, as UTF-16, for [`installer_parameters`].
fn restart_args(args: &[OsString]) -> Vec<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    args.iter().map(|arg| arg.encode_wide().collect()).collect()
}

// ---- Files ------------------------------------------------------------------------------------------------

/// Where one download goes and how far it has come: a folder of its own under the work folder, made when the
/// first bytes arrive, so a download that never starts leaves nothing.
struct Target {
    dir: PathBuf,
    path: PathBuf,
    file: Option<std::fs::File>,
    received: u64,
    last_percent: Option<u8>,
}

impl Target {
    fn new(work_dir: &Path, url: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = work_dir.join(format!("{WORK_PREFIX}{}-{nanos}", std::process::id()));
        let path = dir.join(file_name(url));
        Self {
            dir,
            path,
            file: None,
            received: 0,
            last_percent: None,
        }
    }

    fn write(
        &mut self,
        chunk: &[u8],
        total: Option<u64>,
        progress: &async_channel::Sender<(u64, Option<u64>)>,
    ) -> Result<(), String> {
        if total.is_some_and(|total| total > MAX_INSTALLER) {
            return Err("The update is bigger than any Glint installer".into());
        }
        if self.file.is_none() {
            std::fs::create_dir_all(&self.dir).map_err(|e| format!("couldn't make a folder for it: {e}"))?;
            // `create_new`: a file someone else put here first is not written to.
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.path)
                .map_err(|e| format!("couldn't save it: {e}"))?;
            self.file = Some(file);
        }
        if let Some(file) = &mut self.file {
            file.write_all(chunk).map_err(|e| format!("couldn't save it: {e}"))?;
        }
        self.received = self.received.saturating_add(chunk.len() as u64);
        if self.received > MAX_INSTALLER {
            return Err("The update is bigger than any Glint installer".into());
        }
        // Progress goes out when the percent changes (or about every megabyte when the size is unknown): at
        // most a hundred redraws for the whole download.
        let now = total.map(|total| percent(self.received, total));
        let step = (self.received >> 20) as u8;
        let key = now.or(Some(step));
        if key != self.last_percent {
            self.last_percent = key;
            let _ = progress.try_send((self.received, total));
        }
        Ok(())
    }

    /// Closes the file, flushed to disk, and says where it is (none when nothing arrived).
    fn finish(&mut self) -> Option<PathBuf> {
        let file = self.file.take()?;
        let _ = file.sync_all();
        drop(file);
        Some(self.path.clone())
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

/// Removes a downloaded setup program and its folder.
fn remove_download(path: &Path) {
    let _ = std::fs::remove_file(path);
    if let Some(dir) = path.parent() {
        if dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(WORK_PREFIX))
        {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// Removes the folders of earlier downloads that are older than `older_than` (a copy that was cancelled, or
/// quit before it could run). Only folders this module made, by their name.
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
/// once, early, before the first state write.
pub fn init(cx: &mut App, source: Arc<dyn Source>, hooks: Hooks) -> Entity<Updater> {
    let updater = cx.new(|_| Updater::new(source, Arc::new(ShellLauncher), hooks, Config::for_app()));
    cx.set_global(Shared(updater.clone()));
    updater.update(cx, |updater, cx| updater.start(cx));
    updater
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

/// The "Update available" notice (id `update-available`).
struct FoundToast;
/// The download's progress notice (`toast.loading` with an id).
struct ProgressToast;

/// Words an event as the TypeScript's toasts did, in `window`. Call it from the subscription to the model.
pub fn show(event: &UpdaterEvent, window: &mut Window, cx: &mut App) {
    match event {
        UpdaterEvent::Found { version, installable } => {
            let message = if *installable {
                t!(
                    "Version {version} is ready. Installing it restarts the app.",
                    version = version.clone()
                )
            } else {
                t!("Version {version} is out.", version = version.clone())
            };
            let label = if *installable { t!("Install") } else { t!("Download") };
            window.push_notification(
                Notification::info(message)
                    .title(t!("Update available"))
                    .id::<FoundToast>()
                    .action(move |_, _, _| {
                        Button::new("update-found-install")
                            .label(label.clone())
                            .on_click(|_, window, cx| {
                                window.remove_notification::<FoundToast>(cx);
                                if let Some(updater) = try_shared(cx) {
                                    updater.update(cx, |updater, cx| updater.install(cx));
                                }
                            })
                    }),
                cx,
            );
        }
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
        UpdaterEvent::Ready { .. } => window.remove_notification::<ProgressToast>(cx),
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

/// The header's Update button: shown only while a newer version is known, a spinner while it installs, and a
/// click opens Settings at About, where the update installs. The coordinator puts it in the toolbar before the
/// Settings button; it observes the model to redraw (`.children(updater::update_button(cx))`).
pub fn update_button(cx: &App) -> Option<AnyElement> {
    let updater = try_shared(cx)?;
    let updater = updater.read(cx);
    let available = updater.available()?;
    let working = updater.install_state().working();
    let t = theme::tokens(cx).clone();
    let tip = t!("Version {version} is available", version = available.version.clone());
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
            .label(t!("Update"))
            .loading(working)
            .tooltip(tip)
            .on_click(|_, window, cx| {
                crate::components::settings::open_at(window, cx, Some(crate::components::settings::ABOUT));
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
        asked: Mutex<Vec<String>>,
    }

    impl Fake {
        pub(crate) fn new() -> Arc<Fake> {
            Arc::new(Fake {
                manifest: Mutex::new(Ok(None)),
                github: Mutex::new(Err("GitHub answered 500 Internal Server Error".into())),
                payload: Mutex::new(INSTALLER.to_vec()),
                download_error: Mutex::new(None),
                asked: Mutex::new(Vec::new()),
            })
        }

        pub(crate) fn asked(&self) -> Vec<String> {
            lock(&self.asked).clone()
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

        fn download(&self, url: String, mut sink: Sink) -> Fut<()> {
            lock(&self.asked).push(format!("download {url}"));
            let payload = lock(&self.payload).clone();
            let error = lock(&self.download_error).clone();
            Box::pin(async move {
                let total = Some(payload.len() as u64);
                // Four pieces, so progress has steps.
                let size = payload.len().div_ceil(4);
                for (i, piece) in payload.chunks(size.max(1)).enumerate() {
                    if i == 3 {
                        if let Some(error) = error {
                            return Err(error);
                        }
                    }
                    sink(piece, total)?;
                }
                Ok(())
            })
        }
    }

    /// A launcher that records the run instead of making it.
    #[derive(Default)]
    struct Recorded {
        runs: Mutex<Vec<(PathBuf, String)>>,
        fail: Mutex<Option<String>>,
    }

    impl Launcher for Recorded {
        fn launch(&self, installer: &Path, parameters: &[u16]) -> Result<(), String> {
            if let Some(why) = lock(&self.fail).clone() {
                return Err(why);
            }
            lock(&self.runs).push((installer.to_path_buf(), String::from_utf16_lossy(parameters)));
            Ok(())
        }
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
    fn copies_that_do_not_install_themselves_only_ask_github() {
        let fake = Fake::new();
        fake.serve("0.99.0", "c2ln");
        *lock(&fake.github) = Ok(r#"{"tag_name":"v0.99.0"}"#.into());
        let found = block_on(find(&*fake, Policy::LinkOnly, &version("0.9.5"))).expect("a check");
        let found = found.expect("an update");
        assert!(found.installer.is_none());
        assert_eq!(found.url, "https://github.com/shadijhade/glint/releases/tag/v0.99.0");
        assert_eq!(fake.asked(), vec!["github"]);
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

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn parameters(args: &[&str]) -> String {
        let args: Vec<Vec<u16>> = args.iter().map(|a| wide(a)).collect();
        String::from_utf16(&installer_parameters(&args)).expect("utf-16")
    }

    #[test]
    fn the_installer_runs_passively_as_an_update_and_restarts_glint() {
        assert_eq!(parameters(&[]), "/P /UPDATE /R /ARGS");
        assert_eq!(parameters(&["C:\\Photos"]), "/P /UPDATE /R /ARGS C:\\Photos");
    }

    #[test]
    fn restart_arguments_are_quoted_the_way_nsis_wants() {
        assert_eq!(
            parameters(&["C:\\My Folder\\"]),
            "/P /UPDATE /R /ARGS \"C:\\My Folder\\\\\""
        );
        // A slash would be read as one of the installer's own switches.
        assert_eq!(parameters(&["/x"]), "/P /UPDATE /R /ARGS \"/x\"");
        assert_eq!(parameters(&[""]), "/P /UPDATE /R /ARGS \"\"");
        assert_eq!(parameters(&["a\"b"]), "/P /UPDATE /R /ARGS a\\\"b");
        assert_eq!(
            parameters(&["one", "two words"]),
            "/P /UPDATE /R /ARGS one \"two words\""
        );
        // Arabic and spaces survive.
        assert_eq!(
            parameters(&["C:\\ملفات و صور"]),
            "/P /UPDATE /R /ARGS \"C:\\ملفات و صور\""
        );
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

    // ---- The model

    /// A model over fakes, with what it was asked kept for the tests to read.
    pub(crate) struct Rig {
        pub(crate) fake: Arc<Fake>,
        launched: Arc<Recorded>,
        asked: Rc<std::cell::RefCell<usize>>,
        pub(crate) quit: Rc<std::cell::RefCell<usize>>,
        say_yes: Rc<std::cell::Cell<bool>>,
        pub(crate) updater: Entity<Updater>,
        events: Rc<std::cell::RefCell<Vec<UpdaterEvent>>>,
        pub(crate) dir: PathBuf,
    }

    fn rig(cx: &mut TestAppContext, policy: Policy, current: &str, returning: bool) -> Rig {
        cx.executor().allow_parking();
        cx.update(testing::init);
        cx.update(|cx| build_rig(cx, policy, current, returning))
    }

    /// The model of [`rig`] in an app that is already set up.
    pub(crate) fn build_rig(cx: &mut App, policy: Policy, current: &str, returning: bool) -> Rig {
        let fake = Fake::new();
        let launched = Arc::new(Recorded::default());
        let asked = Rc::new(std::cell::RefCell::new(0));
        let quit = Rc::new(std::cell::RefCell::new(0));
        let say_yes = Rc::new(std::cell::Cell::new(true));
        let dir = temp_dir("model");
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
        };
        let config = Config {
            current: current.into(),
            policy,
            work_dir: dir.join("work"),
            marks: Some(dir.join("updater.json")),
            returning,
            restart_args: vec![OsString::from("C:\\Photos")],
            public_key: PUBKEY.to_string(),
        };
        let events = Rc::new(std::cell::RefCell::new(Vec::new()));
        let updater = {
            let source: Arc<dyn Source> = fake.clone();
            let launcher: Arc<dyn Launcher> = launched.clone();
            let events = events.clone();
            let updater = cx.new(|_| Updater::new(source, launcher, hooks, config));
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
                installable: true
            }]
        );
        // Found: no more looking.
        check(cx, &rig, false);
        assert_eq!(rig.fake.asked(), vec!["manifest"]);
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_secondary_window_never_looks_for_updates_greets_or_writes_the_marks(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let _secondary = crate::platform::secondary::as_secondary(crate::platform::secondary::window_on("C:\\"));
        let rig = rig(cx, Policy::SelfUpdating, "0.9.5", false);
        rig.fake.serve("0.99.0", SIG_GOOD);
        cx.update(|cx| rig.updater.update(cx, |u, cx| u.start(cx)));
        cx.update(|cx| rig.updater.update(cx, |u, cx| u.check(true, cx)));
        for _ in 0..20 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(rig.fake.asked().is_empty(), "nothing asked of the feed or GitHub");
        assert!(read(cx, &rig, |u| u.available().is_none() && *u.status() == Status::Idle));
        assert!(rig.events.borrow().is_empty());
        assert!(!rig.dir.join("updater.json").exists(), "no marks written");
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
        cx.update(|cx| rig.updater.update(cx, |u, cx| u.start(cx)));
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
        assert!(rig.launched.runs.lock().expect("lock").is_empty());
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
        let runs = lock(&rig.launched.runs).clone();
        assert_eq!(runs.len(), 1);
        let (path, parameters) = &runs[0];
        assert_eq!(parameters, "/P /UPDATE /R /ARGS C:\\Photos");
        assert_eq!(std::fs::read(path).expect("the downloaded file"), INSTALLER);
        assert!(path.starts_with(rig.dir.join("work")), "{}", path.display());
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
            version: "0.99.0".into()
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
            assert!(lock(&rig.launched.runs).is_empty(), "a refused update must not run");
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
        assert!(lock(&rig.launched.runs).is_empty());
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn a_download_that_breaks_off_says_so_and_leaves_nothing(cx: &mut TestAppContext) {
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
        assert!(lock(&rig.launched.runs).is_empty());
        let work = rig.dir.join("work");
        assert_eq!(std::fs::read_dir(&work).map(|d| d.flatten().count()).unwrap_or(0), 0);
        // And trying again works.
        *lock(&rig.fake.download_error) = None;
        install(cx, &rig);
        settle(cx, |_| *rig.quit.borrow() == 1);
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
        assert!(
            lock(&rig.launched.runs).is_empty(),
            "the copy that is still running said no"
        );
        assert_eq!(*rig.quit.borrow(), 0);
        // The copy finished: Install asks again, and doesn't download again.
        rig.say_yes.set(true);
        install(cx, &rig);
        settle(cx, |_| *rig.quit.borrow() == 1);
        assert_eq!(*rig.asked.borrow(), 2);
        let downloads = rig.fake.asked().iter().filter(|a| a.starts_with("download")).count();
        assert_eq!(downloads, 1);
        assert_eq!(lock(&rig.launched.runs).len(), 1);
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
        assert!(lock(&rig.launched.runs).is_empty());
        assert_eq!(*rig.quit.borrow(), 0);
        assert!(!path.exists(), "the swapped file is removed");
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn copies_that_cannot_install_themselves_open_the_release_page(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx, Policy::LinkOnly, "0.9.5", false);
        *lock(&rig.fake.github) = Ok(r#"{"tag_name":"v0.99.0"}"#.into());
        check(cx, &rig, true);
        assert!(read(cx, &rig, |u| u.available().is_some_and(|a| a.installer.is_none())));
        install(cx, &rig);
        cx.run_until_parked();
        assert_eq!(
            cx.opened_url().as_deref(),
            Some("https://github.com/shadijhade/glint/releases/tag/v0.99.0")
        );
        assert!(rig.fake.asked().iter().all(|a| !a.starts_with("download")));
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[gpui_kit::test]
    fn the_first_start_after_an_update_greets_once(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        // Ran 0.9.5 before, runs 0.10.0 now.
        let rig = rig(cx, Policy::SelfUpdating, "0.10.0", false);
        std::fs::write(rig.dir.join("updater.json"), r#"{"lastVersion":"0.9.5"}"#).expect("write");
        cx.update(|cx| rig.updater.update(cx, |u, cx| u.start(cx)));
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
        cx.update(|cx| fresh.updater.update(cx, |u, cx| u.start(cx)));
        settle(cx, |cx| read(cx, &fresh, |u| *u.status() == Status::Latest));
        assert!(fresh.events.borrow().is_empty());
        // It wrote the version down, so a later start knows this one ran.
        settle(cx, |_| fresh.dir.join("updater.json").exists());
        let _ = std::fs::remove_dir_all(&fresh.dir);

        // Settings from a version that kept no version of its own: an update, by its settings.
        let returning = rig(cx, Policy::SelfUpdating, "0.10.0", true);
        cx.update(|cx| returning.updater.update(cx, |u, cx| u.start(cx)));
        settle(cx, |_| !returning.events.borrow().is_empty());
        assert_eq!(
            returning.events.borrow().as_slice(),
            [UpdaterEvent::Updated {
                version: "0.10.0".into()
            }]
        );
        let _ = std::fs::remove_dir_all(&returning.dir);
    }

    // ---- What the person sees

    /// A bar that draws the header's Update button, as the toolbar does.
    struct Bar;

    impl Render for Bar {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().children(update_button(cx))
        }
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
        let button = |visual: &mut gpui_kit::VisualTestContext| {
            visual.update(|window, cx| {
                window.render_frame(cx);
                window.try_find("update").map(|e| e.label().map(str::to_string))
            })
        };
        let redraw = |visual: &mut gpui_kit::VisualTestContext| {
            visual.update(|_, cx| bar.update(cx, |_, cx| cx.notify()));
            visual.run_until_parked();
        };
        assert_eq!(button(&mut visual), None, "nothing newer is known: no button");
        rig.fake.serve("0.99.0", SIG_GOOD);
        visual.update(|_, cx| rig.updater.update(cx, |u, cx| u.check(true, cx)));
        for _ in 0..200 {
            visual.run_until_parked();
            if visual.update(|_, cx| rig.updater.read(cx).available().is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        redraw(&mut visual);
        assert_eq!(button(&mut visual), Some(Some("Update".to_string())));
    }

    #[gpui_kit::test]
    fn every_notice_can_be_shown(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.executor().allow_parking();
        cx.update(testing::init);
        let (mut visual, _) = testing::open_window(cx, |_, cx| cx.new(|_| Bar));
        let events = [
            UpdaterEvent::Found {
                version: "0.99.0".into(),
                installable: true,
            },
            UpdaterEvent::Found {
                version: "0.99.0".into(),
                installable: false,
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
            },
            UpdaterEvent::Installing {
                version: "0.99.0".into(),
            },
            UpdaterEvent::Failed {
                reason: "Windows didn't let the installer start".into(),
                url: format!("{RELEASES}/tag/v0.99.0"),
            },
        ];
        for event in &events {
            visual.update(|window, cx| {
                show(event, window, cx);
                window.render_frame(cx);
            });
            visual.run_until_parked();
        }
    }
}
