//! Settings → Clouds: rclone itself (installed, its version and configuration, unlocked), its remotes (in the
//! sidebar or not, or signed out of), and where to connect another or keep a folder in sync (`CloudsSection`
//! of SettingsView.tsx, `connectionFor` of CloudSetup.tsx).

use super::backend::Backend;
use super::tools::{self, Tools};
use super::widgets::{self, bad};
use crate::kit::prelude::*;
use crate::{explorer, persistence, t, theme};
use glint_core::rclone::{Remote, Status};
use glint_model::store::{RemoteConnection, RemoteKind};
use std::sync::Arc;

/// What the Clouds card asks of the rest of the app: both close Settings first, as the TypeScript's did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open the dialog that connects a cloud (`addRemote`).
    Connect,
    /// Open the folder rules (`openRules`).
    Rules,
}

/// A service's name: "Google Drive" for "drive" (`serviceName`). Types without a plainer name show as rclone
/// calls them.
/// A new connection's id, made from its name (`remoteId`): lower-case letters and digits with single dashes,
/// at most 32 characters, and a number added when `taken` has it. (The TypeScript decomposed accents first, so
/// "é" became "e"; here an accented letter becomes a dash, which only matters for a name that has one.)
pub fn remote_id(name: &str, taken: &[String]) -> String {
    let mut base = String::new();
    for ch in name.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            base.push(ch);
        } else if !base.ends_with('-') {
            base.push('-');
        }
    }
    let base: String = base.trim_matches('-').chars().take(32).collect();
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() { "remote".to_string() } else { base };
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

pub fn service_name(kind: &str) -> SharedString {
    match kind {
        "drive" => t!("Google Drive"),
        "onedrive" => t!("OneDrive"),
        "dropbox" => t!("Dropbox"),
        "box" => t!("Box"),
        "pcloud" => t!("pCloud"),
        "mega" => t!("MEGA"),
        "protondrive" => t!("Proton Drive"),
        "iclouddrive" => t!("iCloud Drive"),
        "s3" => t!("Amazon S3 and compatible"),
        "b2" => t!("Backblaze B2"),
        "azureblob" => t!("Azure Blob Storage"),
        "azurefiles" => t!("Azure Files"),
        "google cloud storage" => t!("Google Cloud Storage"),
        "google photos" => t!("Google Photos"),
        "sftp" => t!("SFTP"),
        "ftp" => t!("FTP"),
        "smb" => t!("Windows share (SMB)"),
        "webdav" => t!("WebDAV"),
        "http" => t!("A website's files (HTTP)"),
        "local" => t!("A folder on this PC"),
        "yandex" => t!("Yandex Disk"),
        "crypt" => t!("Encrypted folder (rclone crypt)"),
        other => other.to_string().into(),
    }
}

/// A connection to one of rclone's remotes, named `name` in Glint (`connectionFor`).
pub fn connection_for(name: &str, remote: &Remote, taken: &[String]) -> RemoteConnection {
    RemoteConnection {
        id: remote_id(name, taken),
        name: name.to_string(),
        kind: RemoteKind::Rclone,
        rclone: Some(remote.name.clone()),
        service: Some(remote.kind.clone()),
        ..Default::default()
    }
}

/// The Clouds card.
pub struct Clouds {
    backend: Arc<dyn Backend>,
    tools: Entity<Tools>,
    status: Option<Status>,
    remotes: Option<Vec<Remote>>,
    password: Entity<InputState>,
    error: String,
    /// rclone was installed the last time the tools were looked at, so the first sight of it reads it.
    seen_installed: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Request> for Clouds {}

impl Clouds {
    /// A card reading rclone once the tools say it is installed.
    pub fn new(backend: Arc<dyn Backend>, tools: Entity<Tools>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let on_password = cx.subscribe_in(
            &password,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.unlock(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        );
        let on_tools = cx.observe_in(&tools, window, |this, _, window, cx| {
            let installed = this.installed(cx);
            if installed && !this.seen_installed {
                this.read(window, cx);
            }
            this.seen_installed = installed;
            cx.notify();
        });
        let mut this = Self {
            backend,
            tools,
            status: None,
            remotes: None,
            password,
            error: String::new(),
            seen_installed: false,
            _subscriptions: vec![on_password, on_tools],
        };
        if this.installed(cx) {
            this.seen_installed = true;
            this.read(window, cx);
        }
        this
    }

    fn installed(&self, cx: &App) -> bool {
        self.tools
            .read(cx)
            .tool("rclone")
            .is_some_and(|tool| tool.origin.is_some())
    }

    /// rclone's status and, when it can be read, its remotes (`read`).
    pub fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let status = self.backend.rclone_status();
        let backend = self.backend.clone();
        cx.spawn_in(window, async move |this, cx| {
            let found = match status.await {
                Ok(found) => found,
                Err(reason) => {
                    this.update(cx, |this, cx| {
                        this.error = reason;
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let remotes = if found.installed && !found.locked {
                backend.rclone_remotes().await
            } else {
                Ok(Vec::new())
            };
            this.update(cx, |this, cx| {
                this.status = Some(found);
                match remotes {
                    Ok(remotes) => {
                        this.remotes = Some(remotes);
                        this.error.clear();
                    }
                    Err(reason) => this.error = reason,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The box rclone's password is typed in, for a test to type into.
    pub fn password_input(&self) -> &Entity<InputState> {
        &self.password
    }

    /// The remotes found, once read.
    pub fn remotes(&self) -> Option<&[Remote]> {
        self.remotes.as_deref()
    }

    /// The last error shown under the rows.
    pub fn error(&self) -> &str {
        &self.error
    }

    fn unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let password = self.password.read(cx).value().to_string();
        if password.is_empty() {
            return;
        }
        let asked = self.backend.rclone_unlock(password);
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| match answer {
                Ok(()) => {
                    this.password.update(cx, |input, cx| input.set_value("", window, cx));
                    this.read(window, cx);
                }
                Err(reason) => {
                    this.error = reason;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Asks, then takes `remote` out of rclone's configuration and the sidebar (`signOut`).
    fn sign_out(&mut self, remote: Remote, window: &mut Window, cx: &mut Context<Self>) {
        let using: Vec<String> = persistence::app(cx)
            .map(|app| {
                app.remotes
                    .iter()
                    .filter(|c| c.kind == RemoteKind::Rclone && c.rclone.as_deref() == Some(remote.name.as_str()))
                    .map(|c| c.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let service = service_name(&remote.kind);
        let message = match using.len() {
            0 => t!(
                "Sign out of “{name}”? It goes from rclone's configuration with its sign-in, so rclone in a terminal loses it too. Nothing on {service} changes.",
                name = remote.name,
                service = service
            ),
            1 => t!(
                "Sign out of “{name}”? It goes from rclone's configuration with its sign-in, so rclone in a terminal loses it too, and its place in the sidebar goes. Nothing on {service} changes.",
                name = remote.name,
                service = service
            ),
            _ => t!(
                "Sign out of “{name}”? It goes from rclone's configuration with its sign-in, so rclone in a terminal loses it too, and its places in the sidebar go. Nothing on {service} changes.",
                name = remote.name,
                service = service
            ),
        };
        let this = cx.weak_entity();
        widgets::confirm(
            window,
            cx,
            t!("Sign out"),
            message,
            t!("Sign out"),
            move |window, cx| {
                let this = this.clone();
                let remote = remote.clone();
                let using = using.clone();
                this.update(cx, |this, cx| this.do_sign_out(remote, using, window, cx))
                    .ok();
            },
        );
    }

    fn do_sign_out(&mut self, remote: Remote, using: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.backend.rclone_delete(remote.name.clone());
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| {
                match answer {
                    Ok(()) => {
                        persistence::update_app(cx, |app| {
                            for id in &using {
                                app.remove_remote(id);
                            }
                        });
                        if let Some(explorer) = explorer::try_explorer(cx) {
                            for id in &using {
                                explorer.update(cx, |explorer, cx| explorer.leave_remote(id, cx));
                            }
                        }
                        this.read(window, cx);
                    }
                    Err(reason) => bad(
                        window,
                        cx,
                        t!("Couldn't sign out of “{name}”", name = remote.name),
                        reason,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Puts `remote` in the sidebar (`saveRemote(connectionFor(..))`).
    fn show_in_sidebar(&mut self, remote: &Remote, cx: &mut Context<Self>) {
        let remote = remote.clone();
        persistence::update_app(cx, move |app| {
            let taken: Vec<String> = app.remotes.iter().map(|r| r.id.clone()).collect();
            app.save_remote(connection_for(&remote.name, &remote, &taken));
        });
        cx.notify();
    }
}

impl Render for Clouds {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let mut rows: Vec<AnyElement> = Vec::new();
        if !self.installed(cx) {
            rows.extend(tools::needed(
                &self.tools,
                "rclone",
                t!("Google Drive, OneDrive, Dropbox and 60 more clouds open like folders with rclone, and Folder rules can keep folders in sync with them."),
                IconName::Cloud,
                cx,
            ));
            return widgets::section("clouds", t!("Clouds"), None, rows, &t);
        }
        let status = self.status.clone();
        let label = match status.as_ref().and_then(|s| s.version.clone()) {
            Some(version) => format!("rclone {version}").into(),
            None => SharedString::from("rclone"),
        };
        let hint = match status.as_ref().and_then(|s| s.config.clone()) {
            Some(config) => t!(
                "Its configuration, shared with rclone in your terminals: {config}",
                config = config
            ),
            None => t!("Reading rclone…"),
        };
        rows.push(
            widgets::setting_row(
                label,
                Some(hint),
                vec![
                    widgets::primary_button("clouds-connect", t!("Connect a cloud"), Some(IconName::Plus))
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(Request::Connect)))
                        .into_any_element(),
                ],
                &t,
            )
            .into_any_element(),
        );
        let locked = status.as_ref().is_some_and(|s| s.locked);
        if locked {
            let has_password = !self.password.read(cx).value().is_empty();
            rows.push(
                widgets::setting_row(
                    t!("Its configuration is encrypted"),
                    Some(t!("Its password unlocks it until Glint closes")),
                    vec![row_start()
                        .gap(space(1.5))
                        .child(
                            div()
                                .w(space(40.))
                                .child(Input::new(&self.password).aria_label(t!("rclone's configuration password"))),
                        )
                        .child(
                            widgets::button("clouds-unlock", t!("Unlock"), Some(IconName::LockOpen))
                                .disabled(!has_password)
                                .on_click(cx.listener(|this, _, window, cx| this.unlock(window, cx))),
                        )
                        .into_any_element()],
                    &t,
                )
                .into_any_element(),
            );
        }
        let connections: Vec<RemoteConnection> =
            persistence::app(cx).map(|app| app.remotes.clone()).unwrap_or_default();
        let remotes = self.remotes.clone();
        for (i, remote) in remotes.iter().flatten().enumerate() {
            let shown = connections
                .iter()
                .any(|c| c.kind == RemoteKind::Rclone && c.rclone.as_deref() == Some(remote.name.as_str()));
            let sidebar: AnyElement = if shown {
                div()
                    .self_center()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!("In the sidebar"))
                    .into_any_element()
            } else {
                let remote = remote.clone();
                widgets::button(&format!("clouds-show-{i}"), t!("Show in the sidebar"), None)
                    .accessibility_label(t!("Show {name} in the sidebar", name = remote.name))
                    .on_click(cx.listener(move |this, _, _, cx| this.show_in_sidebar(&remote, cx)))
                    .into_any_element()
            };
            let leaving = remote.clone();
            rows.push(
                widgets::setting_row(
                    remote.name.clone().into(),
                    Some(service_name(&remote.kind)),
                    vec![
                        sidebar,
                        widgets::quiet_button(&format!("clouds-sign-out-{i}"), t!("Sign out"))
                            .icon(IconName::LogOut)
                            .accessibility_label(t!("Sign out of “{name}”", name = remote.name))
                            .tooltip(t!("Take it out of rclone's configuration, with its sign-in"))
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.sign_out(leaving.clone(), window, cx)),
                            )
                            .into_any_element(),
                    ],
                    &t,
                )
                .into_any_element(),
            );
        }
        if remotes.as_ref().is_some_and(Vec::is_empty) && !locked {
            rows.push(
                div()
                    .id("settings-no-clouds")
                    .role(Role::Status)
                    .aria_label(t!("No clouds yet: Connect a cloud signs in to one."))
                    .test_support()
                    .px(space(4.))
                    .py(space(3.))
                    .text_size(theme::size::SM)
                    .text_color(t.muted_foreground)
                    .child(t!("No clouds yet: Connect a cloud signs in to one."))
                    .into_any_element(),
            );
        }
        rows.push(
            widgets::setting_row(
                t!("Keep a folder in sync"),
                Some(t!(
                    "A folder rule copies it to a cloud, mirrors it there, or syncs it both ways"
                )),
                vec![
                    widgets::button("clouds-rules", t!("Folder rules"), Some(IconName::WandSparkles))
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(Request::Rules)))
                        .into_any_element(),
                ],
                &t,
            )
            .into_any_element(),
        );
        if !self.error.is_empty() {
            rows.push(
                div()
                    .id("settings-clouds-error")
                    .role(Role::Alert)
                    .aria_label(self.error.clone())
                    .test_support()
                    .px(space(4.))
                    .py(space(2.))
                    .text_size(theme::size::XS)
                    .text_color(t.destructive)
                    .child(self.error.clone())
                    .into_any_element(),
            );
        }
        widgets::section("clouds", t!("Clouds"), None, rows, &t)
    }
}
