//! What Settings asks of the PC: glint-core's services, the file pickers and the folders Windows knows.
//!
//! The views never call glint-core themselves. They hold a [`Backend`], so their tests give them a fake with
//! canned answers (the way `dialogs::drive_health` takes a `Source`), and nothing in a test touches the real
//! service, the registry or a dialog. [`CoreBackend`] is the real one; every call returns a future that is
//! driven on core's runtime through `core_bridge::run`, so the UI thread never waits.
//!
//! Core hands back some answers as types with private fields (`ToolStatus`, `tools::Progress`, `mcp::Setup`,
//! the index's volumes). Those cross here as JSON, the way the old app's `invoke` received them, and become the
//! plain types below.

use crate::core_bridge;
use crate::platform::pickers::{self, Filter, Options};
use glint_core::host::{self, Manager as _};
use glint_core::{agents, content, devices, meaning, media_ai, openers, rclone, term, verbs};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// A question to the PC, answered later.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// A picker's answer as a [`Fut`] (the pickers' futures are `Send` already: they hold only a channel).
pub(crate) fn from_picker(picked: pickers::Fut<Result<Option<String>, String>>) -> Fut<Option<String>> {
    picked
}

/// How an install or download is going: what it's doing, and bytes when it knows them (`ToolProgress`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fetch {
    pub stage: String,
    pub done: u64,
    pub total: u64,
}

/// Where instant search stands (`IndexInfo.state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexState {
    On,
    Off,
    Stopped,
    NoAccess,
    Untrusted,
    Unsupported,
}

impl IndexState {
    fn parse(word: &str) -> IndexState {
        match word {
            "on" => IndexState::On,
            "off" => IndexState::Off,
            "stopped" => IndexState::Stopped,
            "noAccess" => IndexState::NoAccess,
            "untrusted" => IndexState::Untrusted,
            _ => IndexState::Unsupported,
        }
    }
}

/// One drive of the index as the service reports it (`IndexVolume`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Volume {
    /// "C:".
    pub volume: String,
    /// "scanning", "ready" or "error".
    pub state: String,
    pub files: u64,
    pub folders: u64,
    pub memory: u64,
    /// "scan" (the MFT) or "snapshot".
    pub source: String,
    pub build_ms: f64,
    pub progress: u64,
    pub progress_total: u64,
    pub error: Option<String>,
}

/// Instant search's service (`IndexInfo` in fs.ts).
#[derive(Clone, Debug, PartialEq)]
pub struct IndexInfo {
    pub state: IndexState,
    /// The running service's version.
    pub version: Option<String>,
    /// This app's version.
    pub app_version: String,
    pub volumes: Vec<Volume>,
    pub error: Option<String>,
}

impl From<glint_core::index::IndexInfo> for IndexInfo {
    fn from(info: glint_core::index::IndexInfo) -> Self {
        IndexInfo {
            state: IndexState::parse(info.state),
            version: info.version,
            app_version: info.app_version.to_string(),
            volumes: info
                .volumes
                .into_iter()
                .map(|v| Volume {
                    volume: v.volume,
                    state: v.state,
                    files: v.files,
                    folders: v.folders,
                    memory: v.memory,
                    source: v.source,
                    build_ms: v.build_ms,
                    progress: v.progress,
                    progress_total: v.progress_total,
                    error: v.error,
                })
                .collect(),
            error: info.error,
        }
    }
}

/// One program Glint drives (`ToolStatus` in fs.ts).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolInfo {
    pub id: String,
    pub name: String,
    pub used_for: String,
    /// What Install tries first: "winget" or "release".
    pub source: String,
    /// "bundled", "downloaded" or "installed"; `None` when it's missing.
    pub origin: Option<String>,
    pub path: Option<String>,
    pub version: Option<String>,
    /// The version its release download gets.
    pub pinned: Option<String>,
    /// Glint downloaded this copy, so it can remove it.
    pub removable: bool,
}

impl ToolInfo {
    fn from_json(value: &Value) -> Option<ToolInfo> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
        Some(ToolInfo {
            id: text("id")?,
            name: text("name")?,
            used_for: text("usedFor").unwrap_or_default(),
            source: text("source").unwrap_or_default(),
            origin: text("origin"),
            path: text("path"),
            version: text("version"),
            pinned: text("pinned"),
            removable: value.get("removable").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

/// How AI agents that speak MCP are told to start Glint's server (`McpSetup`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpSetup {
    pub claude_code: String,
    pub vscode: String,
    pub json: String,
}

/// Where Windows keeps a folder Settings suggests (`documentDir()` and the others of Tauri's path API).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Known {
    Documents,
    Desktop,
    Pictures,
    Music,
    Videos,
}

/// Everything Settings asks of the PC. See the module notes.
pub trait Backend: Send + Sync + 'static {
    // Instant search (the NTFS index service).
    fn index_status(&self) -> Fut<IndexInfo>;
    fn index_enable(&self) -> Fut<IndexInfo>;
    /// Updates an older service to this app's build without administrator approval (`indexUpdate`); fails with
    /// why not when only the approval route can, for the caller to offer it. A backend with no service says so.
    fn index_update(&self) -> Fut<IndexInfo> {
        Box::pin(async { Err(crate::t!("Instant search isn't available here").to_string()) })
    }
    fn index_disable(&self) -> Fut<IndexInfo>;
    // Search inside files; photos and recordings; devices.
    fn content_status(&self) -> Fut<content::Status>;
    fn content_configure(&self, enabled: bool, folders: Vec<String>, pictures: Option<bool>) -> Fut<content::Status>;
    fn media_status(&self) -> Fut<media_ai::Status>;
    fn media_configure(&self, config: media_ai::Config) -> Fut<media_ai::Status>;
    fn media_install(&self, kind: &'static str, progress: async_channel::Sender<Fetch>) -> Fut<media_ai::Status>;
    fn devices_status(&self) -> Fut<devices::Status>;
    fn devices_configure(&self, sharing: bool, shares: Vec<devices::Share>) -> Fut<devices::Status>;
    fn devices_forget(&self, id: String) -> Fut<devices::Status>;
    fn devices_pair_code(&self) -> Fut<String>;
    /// Enters another PC's code: its key and its name.
    fn devices_pair(&self, code: String) -> Fut<(String, String)>;
    // Tools and the meaning models.
    fn tools_status(&self) -> Fut<Vec<ToolInfo>>;
    fn tool_install(&self, id: String, progress: async_channel::Sender<Fetch>) -> Fut<()>;
    fn tool_remove(&self, id: String) -> Fut<()>;
    /// Stops the install of `id` ("photos", "speech", "whisper", "onnxruntime", "meaning" or a tool).
    fn cancel_install(&self, id: String);
    fn meaning_models(&self) -> Fut<meaning::Models>;
    fn meaning_install(&self, multilingual: bool, progress: async_channel::Sender<Fetch>) -> Fut<()>;
    fn meaning_remove(&self, multilingual: bool) -> Fut<()>;
    // rclone's clouds.
    fn rclone_status(&self) -> Fut<rclone::Status>;
    fn rclone_remotes(&self) -> Fut<Vec<rclone::Remote>>;
    fn rclone_unlock(&self, password: String) -> Fut<()>;
    fn rclone_delete(&self, name: String) -> Fut<()>;
    // Editors, shells, agents, File Explorer.
    fn openers(&self) -> Fut<Vec<openers::Opener>>;
    fn shells(&self) -> Fut<Vec<term::Shell>>;
    fn mcp_setup(&self) -> Fut<McpSetup>;
    fn agents_registry_available(&self) -> Fut<bool>;
    fn agents_register(&self) -> Fut<String>;
    fn agent_actions(&self) -> Fut<Vec<agents::Action>>;
    fn agent_undo(&self, id: u64) -> Fut<()>;
    fn explorer_integration(&self) -> Fut<verbs::Integration>;
    fn set_explorer_integration(&self, menu: bool, by_default: bool, win_e: Option<bool>) -> Fut<verbs::Integration>;
    fn diagnostics(&self) -> Fut<String>;
    // The PC.
    /// A folder the person picks (`None`: cancelled). `owner` is the window's handle.
    fn pick_folder(&self, title: String, start: Option<String>, owner: isize) -> Fut<Option<String>>;
    /// A settings file to read.
    fn pick_settings_file(&self, owner: isize) -> Fut<Option<String>>;
    /// Where to write a settings file, suggesting `file_name`.
    fn save_settings_file(&self, file_name: String, owner: isize) -> Fut<Option<String>>;
    /// A folder Windows knows, without a trailing backslash.
    fn known_folder(&self, which: Known) -> Option<String>;
}

/// The real thing: glint-core through the app's handle.
pub struct CoreBackend {
    app: host::AppHandle,
}

impl CoreBackend {
    /// The backend over core's handle (from `core_bridge::Core`).
    pub fn new(app: host::AppHandle) -> Self {
        CoreBackend { app }
    }

    /// The main window core's calls take as their owner, or why there is none.
    fn window(app: &host::AppHandle) -> Result<host::Window, String> {
        app.get_webview_window(crate::workspace::MAIN)
            .ok_or_else(|| crate::t!("Glint's window isn't open").to_string())
    }
}

/// Runs `future` on core's runtime and flattens the join error into the call's own.
fn on_core<T: Send + 'static>(future: impl Future<Output = Result<T, String>> + Send + 'static) -> Fut<T> {
    let joined = core_bridge::run(future);
    Box::pin(async move { joined.await.and_then(|inner| inner) })
}

/// A channel for core that tells `progress` how an install goes.
fn fetch_channel(progress: async_channel::Sender<Fetch>) -> host::ipc::Channel<glint_core::tools::Progress> {
    host::ipc::Channel::new(move |p: glint_core::tools::Progress| {
        let value = serde_json::to_value(&p).unwrap_or(Value::Null);
        let fetch = Fetch {
            stage: value
                .get("stage")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            done: value.get("done").and_then(Value::as_u64).unwrap_or(0),
            total: value.get("total").and_then(Value::as_u64).unwrap_or(0),
        };
        progress
            .try_send(fetch)
            .map_err(|_| host::Error::from("the window that asked for this has gone"))
    })
}

impl Backend for CoreBackend {
    fn index_status(&self) -> Fut<IndexInfo> {
        on_core(async { glint_core::index::index_status().await.map(IndexInfo::from) })
    }

    fn index_update(&self) -> Fut<IndexInfo> {
        on_core(async { glint_core::index::index_update().await.map(IndexInfo::from) })
    }

    fn index_enable(&self) -> Fut<IndexInfo> {
        let app = self.app.clone();
        on_core(async move {
            let window = Self::window(&app)?;
            glint_core::index::index_enable(window).await.map(IndexInfo::from)
        })
    }

    fn index_disable(&self) -> Fut<IndexInfo> {
        let app = self.app.clone();
        on_core(async move {
            let window = Self::window(&app)?;
            glint_core::index::index_disable(window).await.map(IndexInfo::from)
        })
    }

    fn content_status(&self) -> Fut<content::Status> {
        on_core(content::content_status())
    }

    fn content_configure(&self, enabled: bool, folders: Vec<String>, pictures: Option<bool>) -> Fut<content::Status> {
        on_core(content::content_configure(enabled, folders, pictures))
    }

    fn media_status(&self) -> Fut<media_ai::Status> {
        on_core(async { Ok(media_ai::media_ai_status()) })
    }

    fn media_configure(&self, config: media_ai::Config) -> Fut<media_ai::Status> {
        on_core(async move { media_ai::media_ai_configure(config) })
    }

    fn media_install(&self, kind: &'static str, progress: async_channel::Sender<Fetch>) -> Fut<media_ai::Status> {
        let app = self.app.clone();
        on_core(async move {
            let installs = app.state::<glint_core::tools::Installs>();
            media_ai::media_ai_install(kind.to_string(), fetch_channel(progress), installs).await
        })
    }

    fn devices_status(&self) -> Fut<devices::Status> {
        on_core(async { Ok(devices::devices_status()) })
    }

    fn devices_configure(&self, sharing: bool, shares: Vec<devices::Share>) -> Fut<devices::Status> {
        on_core(devices::devices_configure(sharing, shares))
    }

    fn devices_forget(&self, id: String) -> Fut<devices::Status> {
        on_core(async move { devices::devices_forget(id) })
    }

    fn devices_pair_code(&self) -> Fut<String> {
        on_core(devices::devices_pair_code())
    }

    fn devices_pair(&self, code: String) -> Fut<(String, String)> {
        on_core(async move { devices::devices_pair(code).await.map(|p| (p.id, p.name)) })
    }

    fn tools_status(&self) -> Fut<Vec<ToolInfo>> {
        on_core(async {
            let tools = glint_core::tools::tools_status().await?;
            Ok(tools
                .iter()
                .filter_map(|t| serde_json::to_value(t).ok())
                .filter_map(|v| ToolInfo::from_json(&v))
                .collect())
        })
    }

    fn tool_install(&self, id: String, progress: async_channel::Sender<Fetch>) -> Fut<()> {
        let app = self.app.clone();
        on_core(async move {
            let installs = app.state::<glint_core::tools::Installs>();
            glint_core::tools::tool_install(id, fetch_channel(progress), installs)
                .await
                .map(drop)
        })
    }

    fn tool_remove(&self, id: String) -> Fut<()> {
        on_core(async move { glint_core::tools::tool_remove(id).await.map(drop) })
    }

    fn cancel_install(&self, id: String) {
        let installs = self.app.state::<glint_core::tools::Installs>();
        glint_core::tools::tool_cancel_install(id, installs);
    }

    fn meaning_models(&self) -> Fut<meaning::Models> {
        on_core(async { Ok(meaning::meaning_models()) })
    }

    fn meaning_install(&self, multilingual: bool, progress: async_channel::Sender<Fetch>) -> Fut<()> {
        let app = self.app.clone();
        on_core(async move {
            let installs = app.state::<glint_core::tools::Installs>();
            let variant = multilingual.then(|| "multilingual".to_string());
            meaning::meaning_install(fetch_channel(progress), installs, variant).await
        })
    }

    fn meaning_remove(&self, multilingual: bool) -> Fut<()> {
        on_core(async move { meaning::meaning_remove(multilingual.then(|| "multilingual".to_string())).await })
    }

    fn rclone_status(&self) -> Fut<rclone::Status> {
        on_core(rclone::rclone_status())
    }

    fn rclone_remotes(&self) -> Fut<Vec<rclone::Remote>> {
        on_core(rclone::rclone_remotes())
    }

    fn rclone_unlock(&self, password: String) -> Fut<()> {
        on_core(rclone::rclone_unlock(password))
    }

    fn rclone_delete(&self, name: String) -> Fut<()> {
        on_core(rclone::rclone_delete(name))
    }

    fn openers(&self) -> Fut<Vec<openers::Opener>> {
        on_core(openers::openers())
    }

    fn shells(&self) -> Fut<Vec<term::Shell>> {
        on_core(term::term_shells())
    }

    fn mcp_setup(&self) -> Fut<McpSetup> {
        on_core(async {
            let setup = glint_core::mcp::mcp_setup()?;
            let value = serde_json::to_value(&setup).map_err(|e| e.to_string())?;
            let text = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
            Ok(McpSetup {
                claude_code: text("claudeCode"),
                vscode: text("vscode"),
                json: text("json"),
            })
        })
    }

    fn agents_registry_available(&self) -> Fut<bool> {
        on_core(async { Ok(agents::agents_registry_available()) })
    }

    fn agents_register(&self) -> Fut<String> {
        on_core(agents::agents_register())
    }

    fn agent_actions(&self) -> Fut<Vec<agents::Action>> {
        on_core(async { Ok(agents::agent_actions()) })
    }

    fn agent_undo(&self, id: u64) -> Fut<()> {
        let app = self.app.clone();
        on_core(async move {
            let window = Self::window(&app)?;
            agents::agent_undo(window, id).await
        })
    }

    fn explorer_integration(&self) -> Fut<verbs::Integration> {
        on_core(verbs::explorer_integration(self.app.clone()))
    }

    fn set_explorer_integration(&self, menu: bool, by_default: bool, win_e: Option<bool>) -> Fut<verbs::Integration> {
        on_core(verbs::set_explorer_integration(
            self.app.clone(),
            menu,
            by_default,
            win_e,
        ))
    }

    fn diagnostics(&self) -> Fut<String> {
        on_core(glint_core::diagnostics(self.app.clone()))
    }

    fn pick_folder(&self, title: String, start: Option<String>, owner: isize) -> Fut<Option<String>> {
        let mut options = Options::new().title(title).owner(owner);
        if let Some(start) = start.filter(|s| !s.trim().is_empty()) {
            options = options.start(start);
        }
        from_picker(pickers::open_folder(options))
    }

    fn pick_settings_file(&self, owner: isize) -> Fut<Option<String>> {
        let options = Options::new()
            .title(crate::t!("Import settings"))
            .filter(Filter::new("Glint settings", &["json"]))
            .owner(owner);
        from_picker(pickers::open_file(options))
    }

    fn save_settings_file(&self, file_name: String, owner: isize) -> Fut<Option<String>> {
        let options = Options::new()
            .title(crate::t!("Export settings"))
            .filter(Filter::new("Glint settings", &["json"]))
            .owner(owner);
        from_picker(pickers::save_file(options, &file_name))
    }

    fn known_folder(&self, which: Known) -> Option<String> {
        known_folder(which)
    }
}

/// Windows' folder for `which`, without a trailing backslash.
fn known_folder(which: Known) -> Option<String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Videos, SHGetKnownFolderPath,
        KF_FLAG_DEFAULT,
    };
    let id = match which {
        Known::Documents => &FOLDERID_Documents,
        Known::Desktop => &FOLDERID_Desktop,
        Known::Pictures => &FOLDERID_Pictures,
        Known::Music => &FOLDERID_Music,
        Known::Videos => &FOLDERID_Videos,
    };
    // SAFETY: SHGetKnownFolderPath allocates the returned string with CoTaskMemAlloc; it is read once and freed
    // once, and the id is a static GUID.
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text.map(|p| p.trim_end_matches(['\\', '/']).to_string())
    }
}

/// `1.10.0` against `1.9.5`: numbers compared as numbers (`localeCompare(.., {numeric: true})`).
pub fn numeric_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |s: &str| -> Vec<u64> { s.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x.cmp(&y);
        }
    }
    std::cmp::Ordering::Equal
}

impl IndexInfo {
    /// Whether the service runs an older build than the app's: true after the app updated itself. (A newer
    /// one, from another account's Glint, serves this one as it is.) (`serviceOutdated`)
    pub fn outdated(&self) -> bool {
        let numbers = !self.app_version.is_empty()
            && self
                .app_version
                .split('.')
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        self.state == IndexState::On
            && numbers
            && self
                .version
                .as_deref()
                .is_some_and(|v| !v.is_empty() && numeric_cmp(&self.app_version, v) == std::cmp::Ordering::Greater)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        use std::cmp::Ordering::*;
        assert_eq!(numeric_cmp("0.10.0", "0.9.5"), Greater);
        assert_eq!(numeric_cmp("1.2", "1.2.0"), Equal);
        assert_eq!(numeric_cmp("0.9.5", "0.9.6"), Less);
    }

    #[test]
    fn a_service_is_outdated_only_when_it_runs_and_is_older() {
        let info = |state, version: Option<&str>, app: &str| IndexInfo {
            state,
            version: version.map(str::to_string),
            app_version: app.to_string(),
            volumes: Vec::new(),
            error: None,
        };
        assert!(info(IndexState::On, Some("0.9.5"), "0.10.0").outdated());
        assert!(!info(IndexState::On, Some("0.10.0"), "0.10.0").outdated());
        assert!(!info(IndexState::On, Some("0.11.0"), "0.10.0").outdated());
        assert!(!info(IndexState::Stopped, Some("0.9.5"), "0.10.0").outdated());
        assert!(!info(IndexState::On, None, "0.10.0").outdated());
        assert!(!info(IndexState::On, Some("0.9.5"), "0.10.0-beta").outdated());
    }

    #[test]
    fn tools_read_from_the_json_core_sends() {
        let value = serde_json::json!({
            "id": "rg", "name": "ripgrep", "usedFor": "Text search", "origin": "bundled",
            "path": "C:\\rg.exe", "version": "14.1.0", "pinned": "14.1.0", "removable": false
        });
        let tool = ToolInfo::from_json(&value).expect("a tool");
        assert_eq!((tool.id.as_str(), tool.origin.as_deref()), ("rg", Some("bundled")));
        assert!(ToolInfo::from_json(&serde_json::json!({})).is_none());
    }
}
