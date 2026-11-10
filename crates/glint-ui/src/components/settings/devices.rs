//! Settings → Devices: Glint on the user's other PCs, over iroh (`DevicesSection` of SettingsView.tsx and
//! `pairDevice` of devices.ts). This PC shares the folders chosen here with the PCs paired with it, read-only;
//! a PC paired the other way shows in the sidebar's Remote section, to browse.

use super::backend::{Backend, Known};
use super::widgets::{self, bad, good};
use crate::kit::prelude::*;
use crate::{core_bridge, persistence, t, theme};
use glint_core::devices::{Peer, Share, Status};
use glint_model::format::{format_date, plural};
use glint_model::store::{RemoteConnection, RemoteKind};
use std::sync::Arc;
use std::time::Duration;

/// How often the devices' state is asked for (the TypeScript's 3 s).
const POLL: Duration = Duration::from_secs(3);

/// What core said (its type can't be cloned).
#[derive(Clone, Debug, Default)]
pub struct State {
    pub name: String,
    pub sharing: bool,
    pub shares: Vec<Share>,
    pub allowed: Vec<Peer>,
}

impl From<Status> for State {
    fn from(status: Status) -> Self {
        State {
            name: status.name,
            sharing: status.sharing,
            shares: status.shares,
            allowed: status.allowed,
        }
    }
}

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

/// The Devices card. Draws nothing until core has answered once.
pub struct Devices {
    backend: Arc<dyn Backend>,
    status: Option<State>,
    /// The pairing code shown for the other PC to enter.
    code: Option<String>,
    typed: Entity<InputState>,
    pairing: bool,
    _on_typed: Subscription,
    _poll: Task<()>,
}

impl Devices {
    /// Asks how the devices are, and keeps asking while Settings is open.
    pub fn new(backend: Arc<dyn Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let typed = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Pairing code")));
        let on_typed = cx.subscribe_in(&typed, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.pair(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        let asking = backend.clone();
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            let answer = asking.devices_status().await;
            let alive = this
                .update(cx, |this, cx| {
                    if let Ok(status) = answer {
                        this.status = Some(status.into());
                        cx.notify();
                    }
                })
                .is_ok();
            if !alive {
                break;
            }
            cx.background_executor().timer(POLL).await;
        });
        Self {
            backend,
            status: None,
            code: None,
            typed,
            pairing: false,
            _on_typed: on_typed,
            _poll: poll,
        }
    }

    /// The last answer.
    pub fn status(&self) -> Option<&State> {
        self.status.as_ref()
    }

    /// The pairing code on show.
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    /// The pairing box, for a test to type into.
    pub fn input(&self) -> &Entity<InputState> {
        &self.typed
    }

    fn adopt(&mut self, answer: Result<Status, String>, window: &mut Window, cx: &mut Context<Self>) {
        match answer {
            Ok(status) => self.status = Some(status.into()),
            Err(reason) => bad(window, cx, t!("Couldn't change what's shared"), reason),
        }
        cx.notify();
    }

    fn configure(&mut self, sharing: bool, shares: Vec<Share>, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.backend.devices_configure(sharing, shares);
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| this.adopt(answer, window, cx))
                .ok();
        })
        .detach();
    }

    /// Turns sharing on or off; on with nothing shared yet shares Pictures.
    pub fn set_sharing(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(status) = &self.status else { return };
        let mut shares = status.shares.clone();
        if on && shares.is_empty() {
            if let Some(path) = self.backend.known_folder(Known::Pictures) {
                shares.push(Share {
                    name: String::new(),
                    path,
                });
            }
        }
        self.configure(on, shares, window, cx);
    }

    fn share_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let picked = self
            .backend
            .pick_folder(t!("A folder to share with your PCs").to_string(), None, owner);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Some(folder)) = picked.await else { return };
            this.update_in(cx, |this, window, cx| {
                let Some(status) = &this.status else { return };
                if !status.shares.iter().any(|s| s.path == folder) {
                    let mut shares = status.shares.clone();
                    shares.push(Share {
                        name: String::new(),
                        path: folder,
                    });
                    this.configure(true, shares, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn stop_sharing(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(status) = &self.status else { return };
        let shares = status.shares.iter().filter(|s| s.path != path).cloned().collect();
        self.configure(true, shares, window, cx);
    }

    fn show_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.backend.devices_pair_code();
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| {
                match answer {
                    Ok(code) => this.code = Some(code),
                    Err(reason) => bad(window, cx, t!("Couldn't make a pairing code"), reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn forget(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.backend.devices_forget(id);
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| match answer {
                Ok(status) => {
                    this.status = Some(status.into());
                    cx.notify();
                }
                Err(reason) => bad(window, cx, t!("Couldn't forget it"), reason),
            })
            .ok();
        })
        .detach();
    }

    /// Enters the code typed in the box: that PC is paired and added to the sidebar's Remote section (`pair`).
    pub fn pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.typed.read(cx).value().to_string();
        if code.trim().is_empty() || self.pairing {
            return;
        }
        self.pairing = true;
        cx.notify();
        let asked = self.backend.devices_pair(code);
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| {
                this.pairing = false;
                match answer {
                    Ok((id, name)) => {
                        this.typed.update(cx, |input, cx| input.set_value("", window, cx));
                        save_device(&id, &name, cx);
                        good(
                            window,
                            cx,
                            t!("Paired with “{name}”", name = name),
                            Some(t!(
                                "It's in the sidebar's Remote section: what it shares opens like a folder."
                            )),
                        );
                    }
                    Err(reason) => bad(window, cx, t!("Couldn't pair them"), reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Adds the paired PC to the saved connections, or renames the one already there (`pairDevice`).
fn save_device(id: &str, name: &str, cx: &mut App) {
    let id = id.to_string();
    let name = name.to_string();
    persistence::update_app(cx, move |app| {
        let known = app
            .remotes
            .iter()
            .find(|r| r.kind == RemoteKind::Device && r.host.as_deref() == Some(id.as_str()))
            .cloned();
        let remote = match known {
            Some(known) => RemoteConnection { name, ..known },
            None => {
                let taken: Vec<String> = app.remotes.iter().map(|r| r.id.clone()).collect();
                RemoteConnection {
                    id: remote_id(&name, &taken),
                    name,
                    kind: RemoteKind::Device,
                    host: Some(id),
                    thumbnails: Some(true),
                    ..Default::default()
                }
            }
        };
        app.save_remote(remote);
    });
}

impl Render for Devices {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let Some(status) = self.status.clone() else {
            return div().into_any_element();
        };
        let language = crate::i18n::language();
        let hint = if status.sharing {
            t!(
                "{folders} shared with {pcs}, while Glint runs.",
                folders = plural(status.shares.len(), "folder"),
                pcs = plural(status.allowed.len(), "paired PC")
            )
        } else {
            t!("Your paired PCs browse and copy from the folders you choose, read-only.")
        };
        let mut rows: Vec<AnyElement> = vec![
            widgets::note(
                t!("Glint on your other PCs, connected directly and end-to-end encrypted, through routers and firewalls (iroh's relays pass the encrypted traffic only when there's no direct path). A paired PC sees only the folders shared here, and only reads them."),
                &t,
            )
            .into_any_element(),
            widgets::switch_row(
                "devices-share",
                t!("Share folders from {name}", name = status.name),
                Some(hint),
                status.sharing,
                false,
                widgets::toggler(cx, |this, on, window, cx| this.set_sharing(on, window, cx)),
                &t,
            )
            .into_any_element(),
        ];
        if status.sharing {
            let mut list = col().gap(space(1.)).px(space(4.)).pb(space(3.));
            for (i, share) in status.shares.iter().enumerate() {
                let path = share.path.clone();
                list = list.child(widgets::path_line(
                    "devices-share",
                    i,
                    format!("{} · {}", share.name, share.path),
                    widgets::quiet_button(&format!("devices-share-remove-{i}"), t!("Remove"))
                        .accessibility_label(t!("Stop sharing {name}", name = share.path))
                        .on_click(cx.listener(move |this, _, window, cx| this.stop_sharing(&path, window, cx))),
                ));
            }
            list = list.child(
                row_start()
                    .flex_wrap()
                    .gap(space(2.))
                    .child(
                        widgets::button("devices-share-folder", t!("Share a folder"), Some(IconName::Plus))
                            .on_click(cx.listener(|this, _, window, cx| this.share_folder(window, cx))),
                    )
                    .child(
                        widgets::button("devices-show-code", t!("Show a pairing code"), None)
                            .on_click(cx.listener(|this, _, window, cx| this.show_code(window, cx))),
                    ),
            );
            if let Some(code) = self.code.clone() {
                let copied = code.clone();
                list = list.child(
                    col()
                        .id("settings-devices-code")
                        .role(Role::Group)
                        .aria_label(t!("Pairing code"))
                        .aria_description(code.clone())
                        .test_support()
                        .gap(space(1.))
                        .p(space(2.))
                        .rounded(theme::size::RADIUS)
                        .border_1()
                        .border_color(t.border)
                        .bg(t.muted.opacity(0.3))
                        .child(
                            row_start()
                                .gap(space(2.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(theme::size::XS)
                                        .font_family(theme::mono_font(cx))
                                        .child(code),
                                )
                                .child(
                                    widgets::quiet_button("devices-copy-code", t!("Copy"))
                                        .icon(IconName::Copy)
                                        .on_click(move |_, window, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                                            good(window, cx, t!("Copied"), None);
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(t!("On the other PC, enter it in Settings → Devices → Add a PC, within ten minutes. It pairs one PC, once.")),
                        ),
                );
            }
            for (i, peer) in status.allowed.iter().enumerate() {
                let id = peer.id.clone();
                list = list.child(widgets::path_line(
                    "devices-peer",
                    i,
                    t!(
                        "{name} · paired {date}",
                        name = peer.name,
                        date = format_date(peer.paired, None, language)
                    )
                    .to_string(),
                    widgets::quiet_button(&format!("devices-forget-{i}"), t!("Forget"))
                        .accessibility_label(t!("Forget {name}", name = peer.name))
                        .on_click(cx.listener(move |this, _, window, cx| this.forget(id.clone(), window, cx))),
                ));
            }
            rows.push(list.into_any_element());
        }
        let typed_something = !self.typed.read(cx).value().trim().is_empty();
        let pairing = self.pairing;
        rows.push(
            widgets::setting_row(
                t!("Add a PC"),
                Some(t!("Its pairing code, from Settings → Devices on it: that PC then shows in the sidebar's Remote section.")),
                vec![row_start()
                    .gap(space(2.))
                    .child(
                        div()
                            .w(space(56.))
                            .child(Input::new(&self.typed).aria_label(t!("Pairing code"))),
                    )
                    .child(
                        widgets::primary_button("devices-pair", t!("Pair"), None)
                            .loading(pairing)
                            .disabled(!typed_something || pairing)
                            .on_click(cx.listener(|this, _, window, cx| this.pair(window, cx))),
                    )
                    .into_any_element()],
                &t,
            )
            .into_any_element(),
        );
        widgets::section("devices", t!("Devices"), None, rows, &t).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connections_id_is_lower_case_letters_and_digits_made_unique() {
        let taken = vec!["office-pc".to_string()];
        assert_eq!(remote_id("Home PC", &[]), "home-pc");
        assert_eq!(remote_id("  --Office PC!  ", &taken), "office-pc-2");
        assert_eq!(remote_id("!!!", &[]), "remote");
        assert_eq!(remote_id("مكتب", &[]), "remote");
        assert_eq!(remote_id(&"a".repeat(40), &[]).len(), 32);
    }
}
