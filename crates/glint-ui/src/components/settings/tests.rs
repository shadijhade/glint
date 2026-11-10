//! Settings' tests: headless windows, a fake backend (so nothing touches the real service, registry or
//! dialogs), what is drawn, what clicks and keys do to `state::settings`, and the Arabic interface.

use super::backend::{Backend, Fut, IndexInfo, IndexState, Known, McpSetup, ToolInfo, Volume};
use super::*;
use crate::testing;
use glint_core::{agents, content, devices, meaning, media_ai, openers, rclone, term, verbs};
use glint_model::store::{Backdrop, Language as LanguageChoice, Theme};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

pub(super) fn ready<T: Send + 'static>(value: Result<T, String>) -> Fut<T> {
    Box::pin(async move { value })
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// A backend with canned answers that remembers what it was asked.
pub struct Fake {
    pub calls: Mutex<Vec<String>>,
    pub index: Mutex<IndexInfo>,
    pub tools: Mutex<Vec<ToolInfo>>,
    pub rclone_installed: Mutex<bool>,
    pub rclone_locked: Mutex<bool>,
    pub remotes: Mutex<Vec<rclone::Remote>>,
    /// The next answers of the pickers, in order (`None` is "cancelled").
    pub picks: Mutex<VecDeque<Option<String>>>,
    pub content: Mutex<content::Status>,
    pub media: Mutex<media_ai::Config>,
    pub sharing: Mutex<bool>,
    pub shares: Mutex<Vec<devices::Share>>,
    pub integration_available: bool,
    pub fail_next: Mutex<Option<String>>,
}

impl Fake {
    pub fn new() -> Self {
        Fake {
            calls: Mutex::new(Vec::new()),
            index: Mutex::new(IndexInfo {
                state: IndexState::Off,
                version: None,
                app_version: "0.10.0".into(),
                volumes: Vec::new(),
                error: None,
            }),
            tools: Mutex::new(vec![
                tool("rg", "ripgrep", Some("bundled")),
                tool("zoxide", "zoxide", None),
                tool("rclone", "rclone", None),
            ]),
            rclone_installed: Mutex::new(false),
            rclone_locked: Mutex::new(false),
            remotes: Mutex::new(Vec::new()),
            picks: Mutex::new(VecDeque::new()),
            content: Mutex::new(content::Status {
                pictures: true,
                ..Default::default()
            }),
            media: Mutex::new(media_ai::Config::default()),
            sharing: Mutex::new(false),
            shares: Mutex::new(Vec::new()),
            integration_available: true,
            fail_next: Mutex::new(None),
        }
    }

    fn said(&self, what: impl Into<String>) {
        lock(&self.calls).push(what.into());
    }

    pub fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }

    fn pick(&self, what: &str) -> Fut<Option<String>> {
        self.said(what);
        ready(Ok(lock(&self.picks).pop_front().flatten()))
    }

    fn failure(&self) -> Option<String> {
        lock(&self.fail_next).take()
    }
}

pub(super) fn tool(id: &str, name: &str, origin: Option<&str>) -> ToolInfo {
    ToolInfo {
        id: id.into(),
        name: name.into(),
        used_for: format!("{name} is used for things"),
        source: "release".into(),
        origin: origin.map(str::to_string),
        path: origin.map(|_| format!("C:\\tools\\{id}.exe")),
        version: origin.map(|_| "1.0.0".into()),
        pinned: Some("1.0.0".into()),
        removable: origin == Some("downloaded"),
    }
}

impl Backend for Fake {
    fn index_status(&self) -> Fut<IndexInfo> {
        ready(Ok(lock(&self.index).clone()))
    }

    fn index_enable(&self) -> Fut<IndexInfo> {
        self.said("index_enable");
        if let Some(reason) = self.failure() {
            return ready(Err(reason));
        }
        let mut index = lock(&self.index);
        index.state = IndexState::On;
        index.version = Some(index.app_version.clone());
        index.volumes = vec![Volume {
            volume: "C:".into(),
            state: "ready".into(),
            files: 900,
            folders: 100,
            memory: 5 << 20,
            source: "scan".into(),
            build_ms: 1234.0,
            ..Default::default()
        }];
        ready(Ok(index.clone()))
    }

    fn index_disable(&self) -> Fut<IndexInfo> {
        self.said("index_disable");
        let mut index = lock(&self.index);
        index.state = IndexState::Off;
        index.volumes.clear();
        ready(Ok(index.clone()))
    }

    fn content_status(&self) -> Fut<content::Status> {
        ready(Ok(lock(&self.content).clone()))
    }

    fn content_configure(&self, enabled: bool, folders: Vec<String>, pictures: Option<bool>) -> Fut<content::Status> {
        self.said(format!("content_configure {enabled} {folders:?} {pictures:?}"));
        let mut status = lock(&self.content);
        status.enabled = enabled;
        status.folders = folders;
        if let Some(pictures) = pictures {
            status.pictures = pictures;
        }
        ready(Ok(status.clone()))
    }

    fn media_status(&self) -> Fut<media_ai::Status> {
        ready(Ok(media_ai::Status {
            config: lock(&self.media).clone(),
            photos_installed: false,
            speech_installed: false,
            photos: media_ai::Progress::default(),
            speech: media_ai::Progress::default(),
        }))
    }

    fn media_configure(&self, config: media_ai::Config) -> Fut<media_ai::Status> {
        self.said(format!(
            "media_configure photos={} {:?}",
            config.photos, config.photo_folders
        ));
        *lock(&self.media) = config;
        self.media_status()
    }

    fn media_install(
        &self,
        kind: &'static str,
        _progress: async_channel::Sender<backend::Fetch>,
    ) -> Fut<media_ai::Status> {
        self.said(format!("media_install {kind}"));
        self.media_status()
    }

    fn devices_status(&self) -> Fut<devices::Status> {
        ready(Ok(devices::Status {
            name: "TEST-PC".into(),
            sharing: *lock(&self.sharing),
            shares: lock(&self.shares).clone(),
            allowed: Vec::new(),
        }))
    }

    fn devices_configure(&self, sharing: bool, shares: Vec<devices::Share>) -> Fut<devices::Status> {
        self.said(format!("devices_configure {sharing} {}", shares.len()));
        *lock(&self.sharing) = sharing;
        *lock(&self.shares) = shares;
        self.devices_status()
    }

    fn devices_forget(&self, _id: String) -> Fut<devices::Status> {
        self.devices_status()
    }

    fn devices_pair_code(&self) -> Fut<String> {
        ready(Ok("KEY-ABCDE23456".into()))
    }

    fn devices_pair(&self, code: String) -> Fut<(String, String)> {
        self.said(format!("devices_pair {code}"));
        ready(Ok(("device-key".into(), "Office PC".into())))
    }

    fn tools_status(&self) -> Fut<Vec<ToolInfo>> {
        let mut tools = lock(&self.tools).clone();
        if *lock(&self.rclone_installed) {
            for tool in tools.iter_mut().filter(|t| t.id == "rclone") {
                tool.origin = Some("installed".into());
                tool.version = Some("1.68.0".into());
            }
        }
        ready(Ok(tools))
    }

    fn tool_install(&self, id: String, progress: async_channel::Sender<backend::Fetch>) -> Fut<()> {
        self.said(format!("tool_install {id}"));
        let _ = progress.try_send(backend::Fetch {
            stage: "Downloading".into(),
            done: 1,
            total: 2,
        });
        if id == "rclone" {
            *lock(&self.rclone_installed) = true;
        }
        for tool in lock(&self.tools).iter_mut().filter(|t| t.id == id) {
            tool.origin = Some("downloaded".into());
            tool.removable = true;
        }
        ready(Ok(()))
    }

    fn tool_remove(&self, id: String) -> Fut<()> {
        self.said(format!("tool_remove {id}"));
        ready(Ok(()))
    }

    fn cancel_install(&self, id: String) {
        self.said(format!("cancel_install {id}"));
    }

    fn meaning_models(&self) -> Fut<meaning::Models> {
        ready(Ok(meaning::Models {
            english: true,
            multilingual: false,
        }))
    }

    fn meaning_install(&self, multilingual: bool, _progress: async_channel::Sender<backend::Fetch>) -> Fut<()> {
        self.said(format!("meaning_install {multilingual}"));
        ready(Ok(()))
    }

    fn meaning_remove(&self, multilingual: bool) -> Fut<()> {
        self.said(format!("meaning_remove {multilingual}"));
        ready(Ok(()))
    }

    fn rclone_status(&self) -> Fut<rclone::Status> {
        ready(Ok(rclone::Status {
            installed: *lock(&self.rclone_installed),
            version: Some("1.68.0".into()),
            config: Some("C:\\Users\\me\\rclone.conf".into()),
            locked: *lock(&self.rclone_locked),
        }))
    }

    fn rclone_remotes(&self) -> Fut<Vec<rclone::Remote>> {
        ready(Ok(lock(&self.remotes).clone()))
    }

    fn rclone_unlock(&self, password: String) -> Fut<()> {
        self.said(format!("rclone_unlock {password}"));
        if password == "right" {
            *lock(&self.rclone_locked) = false;
            ready(Ok(()))
        } else {
            ready(Err("That isn't its password".into()))
        }
    }

    fn rclone_delete(&self, name: String) -> Fut<()> {
        self.said(format!("rclone_delete {name}"));
        lock(&self.remotes).retain(|r| r.name != name);
        ready(Ok(()))
    }

    fn openers(&self) -> Fut<Vec<openers::Opener>> {
        ready(Ok(vec![
            openers::Opener {
                id: "vscode",
                name: "VS Code",
                terminal: false,
                path: "C:\\Code.exe".into(),
            },
            openers::Opener {
                id: "wt",
                name: "Windows Terminal",
                terminal: true,
                path: "C:\\wt.exe".into(),
            },
        ]))
    }

    fn shells(&self) -> Fut<Vec<term::Shell>> {
        ready(Ok(vec![
            term::Shell {
                id: "pwsh",
                name: "PowerShell 7",
                path: "C:\\pwsh.exe".into(),
            },
            term::Shell {
                id: "cmd",
                name: "Command Prompt",
                path: "C:\\cmd.exe".into(),
            },
        ]))
    }

    fn mcp_setup(&self) -> Fut<McpSetup> {
        ready(Ok(McpSetup {
            claude_code: "claude mcp add glint -- \"C:\\Glint.exe\" mcp".into(),
            vscode: "code --add-mcp '{}'".into(),
            json: "{\n  \"mcpServers\": {}\n}".into(),
        }))
    }

    fn agents_registry_available(&self) -> Fut<bool> {
        ready(Ok(true))
    }

    fn agents_register(&self) -> Fut<String> {
        self.said("agents_register");
        ready(Ok("added".into()))
    }

    fn agent_actions(&self) -> Fut<Vec<agents::Action>> {
        ready(Ok(vec![agents::Action {
            id: 7,
            at: 1_700_000_000_000,
            kind: "move".into(),
            summary: "Moved 2 files to Archive".into(),
            items: vec![("C:\\a.txt".into(), Some("C:\\Archive\\a.txt".into()))],
            undone: false,
        }]))
    }

    fn agent_undo(&self, id: u64) -> Fut<()> {
        self.said(format!("agent_undo {id}"));
        ready(Ok(()))
    }

    fn explorer_integration(&self) -> Fut<verbs::Integration> {
        ready(Ok(verbs::Integration {
            available: self.integration_available,
            ..Default::default()
        }))
    }

    fn set_explorer_integration(&self, menu: bool, by_default: bool, win_e: Option<bool>) -> Fut<verbs::Integration> {
        self.said(format!("set_explorer_integration {menu} {by_default} {win_e:?}"));
        ready(Ok(verbs::Integration {
            available: true,
            menu,
            by_default,
            win_e: win_e.unwrap_or(false),
            store: false,
        }))
    }

    fn diagnostics(&self) -> Fut<String> {
        ready(Ok("Glint 0.10.0\nrg 14".into()))
    }

    fn pick_folder(&self, title: String, _start: Option<String>, _owner: isize) -> Fut<Option<String>> {
        self.pick(&format!("pick_folder {title}"))
    }

    fn pick_settings_file(&self, _owner: isize) -> Fut<Option<String>> {
        self.pick("pick_settings_file")
    }

    fn save_settings_file(&self, file_name: String, _owner: isize) -> Fut<Option<String>> {
        self.pick(&format!("save_settings_file {file_name}"))
    }

    fn known_folder(&self, which: Known) -> Option<String> {
        Some(format!("C:\\Users\\me\\{which:?}"))
    }
}

/// A view to host the dialog layer.
pub(super) struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

/// An app with the skeleton's globals and a saved state in a temporary folder, a window, and Settings open over
/// `fake`.
pub(crate) fn open_settings(
    cx: &mut TestAppContext,
    fake: &std::sync::Arc<Fake>,
    section: Option<&str>,
) -> (VisualTestContext, Entity<SettingsView>) {
    cx.update(|cx| {
        testing::init(cx);
        let dir = std::env::temp_dir().join(format!(
            "glint-settings-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("state.json");
        let _ = std::fs::remove_file(&file);
        crate::persistence::init_at(cx, file);
        // The saved settings come with the language of the PC the tests run on; the tests read English.
        state::update(cx, |s| {
            s.language = LanguageChoice::En;
            s.backdrop = Backdrop::Solid;
        });
    });
    let (mut visual, _) = testing::open_window(cx, |_, cx| cx.new(|_| Host));
    let backend: std::sync::Arc<dyn Backend> = fake.clone();
    let view = visual.update(|window, cx| open_with(window, cx, backend, false, section));
    settle(&mut visual, |_| true);
    (visual, view)
}

/// Lets the backend's answers (futures that are ready at once) come back to the views.
#[track_caller]
pub(crate) fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    for _ in 0..200 {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("Settings never reached the state the test waits for");
}

pub(super) fn frame(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.render_frame(cx));
}

pub(super) fn has(cx: &mut VisualTestContext, label: &str) -> bool {
    cx.update(|window, cx| {
        window.render_frame(cx);
        !window.find_all_by_label(label).is_empty()
    })
}

/// The role of the first element with this accessible name.
pub(super) fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window
            .find_all_by_label(label)
            .first()
            .and_then(|element| element.role())
    })
}

/// Focuses a box and types into it, as a person does (programmatic changes to a box say nothing).
pub(super) fn type_into(cx: &mut VisualTestContext, input: &Entity<InputState>, text: &str) {
    let input = input.clone();
    let text = text.to_string();
    cx.update(|window, cx| {
        input.update(cx, |input, cx| input.focus(window, cx));
        window.press("ctrl-a", cx);
        window.input(&text, cx);
    });
    cx.run_until_parked();
}

/// Scrolls the page down until the element with this id can be clicked.
pub(super) fn reveal(cx: &mut VisualTestContext, view: &Entity<SettingsView>, id: &str) {
    for _ in 0..60 {
        let visible = cx.update(|window, cx| {
            window.render_frame(cx);
            let body = window.find("settings-body").bounds();
            // All of it inside the page, so its middle (where a click lands) is not under the edge.
            window.try_find(SharedString::from(id.to_string())).is_some_and(|e| {
                e.visible() && e.bounds().top() >= body.top() && e.bounds().bottom() <= body.bottom() - px(24.)
            })
        });
        if visible {
            return;
        }
        cx.update(|_, cx| view.read(cx).scroll_by(px(120.)));
        cx.run_until_parked();
    }
    panic!("{id} never came into view");
}

pub(super) fn click(cx: &mut VisualTestContext, id: &str) {
    let id = SharedString::from(id.to_string());
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    });
    cx.run_until_parked();
}

/// Scrolls the page to a section by its name on the left, as a click does.
pub(super) fn go_to(cx: &mut VisualTestContext, slot: &str) {
    click(cx, &format!("settings-nav-{slot}"));
    frame(cx);
    cx.run_until_parked();
    frame(cx);
}

pub(super) fn settings(cx: &mut VisualTestContext) -> glint_model::store::Settings {
    cx.update(|_, cx| state::settings(cx).clone())
}

#[gpui_kit::test]
fn settings_opens_as_a_named_dialog_with_every_section_listed(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = std::sync::Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    visual.update(|window, cx| {
        assert!(window.has_active_dialog(cx));
        window.render_frame(cx);
        assert!(!window.find_all_by_role(Role::Dialog).is_empty(), "no Dialog role");
    });
    assert_eq!(role_of(&mut visual, "Settings"), Some(Role::Group));
    // The names on the left, one button each, in the TypeScript's order and wording.
    for (slot, name) in [
        ("appearance", "Appearance"),
        ("files", "Files"),
        ("instant-search", "Instant search"),
        ("find-files-by-name", "Find files"),
        ("quick-search", "Quick search"),
        ("ai-agents", "AI agents"),
        ("search-text-ripgrep", "Search text"),
        ("text-search-skips", "Text search skips"),
        ("terminal", "Terminal"),
        ("check", "Check"),
        ("clouds", "Clouds"),
        ("tools", "Tools"),
        ("history", "History"),
        ("about", "About"),
    ] {
        let found = visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(format!("settings-nav-{slot}"))
        });
        let found = found.unwrap_or_else(|| panic!("no name for {slot} on the left"));
        assert_eq!(
            (found.role(), found.label()),
            (Some(Role::Button), Some(name)),
            "{slot}"
        );
    }
    assert_eq!(role_of(&mut visual, "Settings sections"), Some(Role::Navigation));
    // The page is the sections in order; Content/Media/Devices wait until their service has answered.
    let order = visual.update(|_, cx| view.read(cx).sections().to_vec());
    assert_eq!(&order[..3], &["appearance", "files", "instant-search"]);
    assert_eq!(order.last().copied(), Some("about"));
    assert!(order.contains(&"search-inside-files-instantly"));
    assert!(order.contains(&"devices"));
}

#[gpui_kit::test]
fn a_switch_and_its_label_change_the_settings(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = std::sync::Arc::new(Fake::new());
    let (mut visual, _) = open_settings(cx, &fake, None);
    go_to(&mut visual, "files");
    assert!(!settings(&mut visual).show_hidden);
    // The switch has a role and the row's name.
    assert_eq!(role_of(&mut visual, "Show hidden items"), Some(Role::Switch));
    click(&mut visual, "settings-switch-show-hidden");
    assert!(settings(&mut visual).show_hidden);
    // The label toggles it too.
    click(&mut visual, "settings-label-show-hidden");
    assert!(!settings(&mut visual).show_hidden);
    // "Folders first" is on by default.
    assert!(settings(&mut visual).folders_first);
    click(&mut visual, "settings-switch-folders-first");
    assert!(!settings(&mut visual).folders_first);
}

#[gpui_kit::test]
fn a_segmented_choice_marks_the_chosen_option_and_writes_the_setting(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = std::sync::Arc::new(Fake::new());
    let (mut visual, _) = open_settings(cx, &fake, None);
    let role = visual.update(|window, cx| {
        window.render_frame(cx);
        window.find("settings-choice-theme").role()
    });
    assert_eq!(role, Some(Role::RadioGroup));
    assert_eq!(settings(&mut visual).theme, Theme::System);
    click(&mut visual, "settings-choice-theme-1");
    assert_eq!(settings(&mut visual).theme, Theme::Dark);
    let checked = |cx: &mut VisualTestContext, i: usize| {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(format!("settings-choice-theme-{i}")).checked()
        })
    };
    assert_eq!(
        (checked(&mut visual, 1), checked(&mut visual, 0)),
        (Some(true), Some(false))
    );
    click(&mut visual, "settings-choice-theme-0");
    assert_eq!(settings(&mut visual).theme, Theme::Light);
    // The text size and the language are choices too.
    click(&mut visual, "settings-choice-font-size-2");
    assert_eq!(settings(&mut visual).font_size, glint_model::store::FontSize::Large);
}
