//! Instant name search, folder sizes and disk-usage maps for whole NTFS drives, the way Everything
//! and WizTree do it: an elevated service reads each volume's Master File Table once, keeps every
//! name, size, date and attribute in memory, follows the USN change journal to stay current, and
//! answers the app over a named pipe that only the users who turned it on (and administrators) can
//! open.
//!
//! The pure parts (record parsing, the index, queries, snapshots, the wire format) build and test on
//! any OS; reading volumes, the service and the pipe are Windows-only.

pub mod proto;
pub mod query;
pub mod record;
pub mod snapshot;
pub mod tree;
pub mod usn;

#[cfg(windows)]
pub mod client;
#[cfg(windows)]
pub mod cli;
#[cfg(windows)]
mod engine;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
mod service;
#[cfg(windows)]
mod volume;
#[cfg(windows)]
mod win;

/// Windows as a bug report names it, e.g. "Windows 11 Home 25H2 (build 26300)".
#[cfg(windows)]
pub fn windows_version() -> String {
    win::windows_version()
}

/// The service's log, in LocalSystem's profile (administrators can open it).
#[cfg(windows)]
pub fn log_path() -> Option<std::path::PathBuf> {
    win::data_dir().ok().map(|dir| dir.join("service.log"))
}

/// The Windows service's name (as `sc query` shows it).
pub const SERVICE_NAME: &str = "ShadUIRipgrepIndex";
/// What Services and Task Manager call it.
pub const SERVICE_DISPLAY_NAME: &str = "Glint instant search";
/// The executable, bundled next to the app and copied to Program Files when installed.
pub const SERVICE_EXE: &str = "shadui-index.exe";
/// The pipe the service answers on, under the prefix where only administrators and SYSTEM can
/// create a pipe, so nothing can take its name first. The version changes with incompatible protocol
/// changes.
pub const PIPE_NAME: &str = r"\\.\pipe\ProtectedPrefix\Administrators\shadui-ripgrep-index-v1";
/// Where services before 0.6 answer (and this one, on a Windows without the protected prefix): the
/// app still reaches a service there, to offer its update.
pub const LEGACY_PIPE_NAME: &str = r"\\.\pipe\shadui-ripgrep-index-v1";
/// Reported in the status, so the app can offer to update an older service.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
