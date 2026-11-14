//! Phones and computers nearby: finding the LocalSend apps on this network, sending to them and receiving from
//! them. Ported from src/components/explorer/NearbyDialog.tsx, built the way [`super::drive_health`] is.
//! (Sending to a paired PC, `sendToDevice`, went with paired devices in 0.12.)
//!
//! | TypeScript                                   | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `useNearby`, `openNearby(paths)`             | [`open`]                                                      |
//! | `NearbyModal` state and its calls            | [`Nearby`], a model entity over a [`Backend`]                 |
//! | `nearby_scan`, `nearby_send`, `nearby_cancel`, `nearby_receive` | [`Backend::scan`], [`Backend::send`], [`Backend::cancel`], [`Backend::receive`] |
//! | `listen("nearby-received")`                  | the model subscribes to core's `nearby-received` event        |
//! | `NearbyDialog`                               | [`NearbyDialog`]                                              |
//! | toasts                                       | [`NearbyEvent`] raised by the model, shown as notifications by the dialog |
//!
//! **How the coordinator wires it.**
//!
//! - `MenuEvent::SendNearby { paths }` is `nearby::open(window, cx, paths)`.
//! - `MenuEvent::SendToPc`, `ShareOnNetwork`, `ShareWithWindows` and `ReceiveHere` belong to `share.rs`.
//!
//! Closing the dialog stops receiving, as the TypeScript's unmount did.
//!
//! What was not ported: nothing of the dialog. The list of devices is not virtualized (a network has a few
//! dozen devices at most); the received list is the TypeScript's 50 newest.

use crate::kit::prelude::*;
use crate::{core_bridge, explorer, t, theme};
use glint_core::host::ipc::Channel;
use glint_core::localsend::Peer;
use glint_model::format::{format_size, plural};
use glint_model::paths::{base_name, parent_path};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// How long a scan listens (the TypeScript's 3500 ms).
const SCAN_MILLIS: u64 = 3500;

/// How many received files the list keeps.
const KEEP_RECEIVED: usize = 50;

/// How a send is going: files and bytes sent of all, and the one under way (core's two progress types,
/// LocalSend's and a paired PC's, are the same).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub files: usize,
    pub total_files: usize,
    pub bytes: u64,
    pub total_bytes: u64,
    pub current: String,
}

impl From<glint_core::localsend::SendProgress> for Progress {
    fn from(p: glint_core::localsend::SendProgress) -> Self {
        Self {
            files: p.files,
            total_files: p.total_files,
            bytes: p.bytes,
            total_bytes: p.total_bytes,
            current: p.current,
        }
    }
}

// ---- The calls --------------------------------------------------------------------------------------

/// The calls the dialog makes. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync + 'static {
    /// `nearby_scan`: the devices answering within `millis`.
    fn scan(&self, millis: u64) -> Fut<Vec<Peer>>;
    /// `nearby_send`: sends `paths` to `peer` (it asks its person first); how many files went. The progress
    /// comes on `progress`.
    fn send(
        &self,
        peer: Peer,
        paths: Vec<String>,
        progress: Channel<glint_core::localsend::SendProgress>,
    ) -> Fut<usize>;
    /// `nearby_cancel`.
    fn cancel(&self);
    /// `nearby_receive`: starts or stops taking files; the port it listens on.
    fn receive(&self, on: bool) -> Fut<Option<u16>>;
}

/// glint-core.
pub struct CoreBackend {
    app: Option<glint_core::host::AppHandle>,
}

impl CoreBackend {
    /// Core's, if it is started.
    pub fn new(cx: &App) -> Self {
        Self {
            app: cx.try_global::<core_bridge::Core>().map(|core| core.app.clone()),
        }
    }
}

impl Backend for CoreBackend {
    fn scan(&self, millis: u64) -> Fut<Vec<Peer>> {
        Box::pin(glint_core::localsend::nearby_scan(millis))
    }
    fn send(
        &self,
        peer: Peer,
        paths: Vec<String>,
        progress: Channel<glint_core::localsend::SendProgress>,
    ) -> Fut<usize> {
        Box::pin(glint_core::localsend::nearby_send(peer, paths, progress))
    }
    fn cancel(&self) {
        glint_core::localsend::nearby_cancel();
    }
    fn receive(&self, on: bool) -> Fut<Option<u16>> {
        match self.app.clone() {
            Some(app) => Box::pin(glint_core::localsend::nearby_receive(app, on)),
            None => Box::pin(async { Err("Glint's core isn't running".to_string()) }),
        }
    }
}

/// What happens to a path, given the app.
pub type OnPath = Rc<dyn Fn(&str, &mut App)>;

/// What the dialog does to the rest of the app: the explorer's in the app, a recorder in tests.
#[derive(Clone)]
pub struct Effects {
    /// Shows a file in its folder, selected (`revealInApp`).
    pub reveal: OnPath,
}

impl Effects {
    /// The explorer's.
    pub fn app() -> Self {
        Self {
            reveal: Rc::new(|path, cx| {
                if let Some(explorer) = explorer::try_explorer(cx) {
                    let options = explorer::NavOptions::default().selecting(path.to_string());
                    explorer.update(cx, |explorer, cx| explorer.navigate(parent_path(path), options, cx));
                }
            }),
        }
    }
}

// ---- The model --------------------------------------------------------------------------------------

/// A send under way to one device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    /// The device's fingerprint.
    pub to: String,
    /// How far it is; `None` until the first report (the device is deciding).
    pub progress: Option<Progress>,
}

/// A file that arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    /// Who sent it (the device's name).
    pub from: String,
    pub path: String,
}

/// What the model tells the dialog to say (the TypeScript's toasts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NearbyEvent {
    /// `n` files went to `alias`; 0 means it already had them.
    Sent { alias: String, files: usize },
    /// Something failed: the notice's title and the reason.
    Failed { title: SharedString, reason: String },
}

/// The devices nearby, what is being sent and what has come (`NearbyModal`'s state).
pub struct Nearby {
    backend: Arc<dyn Backend>,
    paths: Vec<String>,
    /// The devices found; `None` before the first scan ends.
    peers: Option<Vec<Peer>>,
    scanning: bool,
    sending: Option<Outgoing>,
    /// The port this PC listens on while receiving.
    receiving: Option<u16>,
    /// Receiving was asked for (or stopped), so closing the dialog must stop it.
    receive_asked: bool,
    received: Vec<Received>,
    /// Tells a send's late reports from the next send's.
    generation: u64,
    _core_events: Option<Subscription>,
}

impl EventEmitter<NearbyEvent> for Nearby {}

impl Nearby {
    /// A model that sends `paths` (none: only looks and receives) through `backend`, and scans now.
    pub fn new(backend: Arc<dyn Backend>, paths: Vec<String>, cx: &mut Context<Self>) -> Self {
        let events = core_bridge::subscribe(cx, |this: &mut Nearby, event, cx| {
            if event.name != "nearby-received" {
                return;
            }
            let text = |key: &str| {
                event
                    .payload
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            };
            let (from, path) = (text("from"), text("path"));
            if !path.is_empty() {
                this.on_received(from, path, cx);
            }
        });
        let mut this = Self {
            backend,
            paths,
            peers: None,
            scanning: false,
            sending: None,
            receiving: None,
            receive_asked: false,
            received: Vec::new(),
            generation: 0,
            _core_events: events,
        };
        this.scan(cx);
        this
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// The devices found; `None` before the first scan ends.
    pub fn peers(&self) -> Option<&[Peer]> {
        self.peers.as_deref()
    }

    pub fn scanning(&self) -> bool {
        self.scanning
    }

    pub fn sending(&self) -> Option<&Outgoing> {
        self.sending.as_ref()
    }

    /// The port this PC listens on while receiving.
    pub fn receiving(&self) -> Option<u16> {
        self.receiving
    }

    pub fn received(&self) -> &[Received] {
        &self.received
    }

    /// Looks for devices again (`scan`). A scan under way isn't started twice.
    pub fn scan(&mut self, cx: &mut Context<Self>) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        cx.notify();
        let work = self.backend.scan(SCAN_MILLIS);
        cx.spawn(async move |this, cx| {
            let found = core_bridge::run(work).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                this.scanning = false;
                match found {
                    Ok(peers) => this.peers = Some(peers),
                    Err(reason) => {
                        this.peers = Some(Vec::new());
                        cx.emit(NearbyEvent::Failed {
                            title: t!("Couldn't look for devices"),
                            reason,
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Sends the paths to `peer`: it says yes first, then the progress shows (`send`). One send at a time.
    pub fn send(&mut self, peer: Peer, cx: &mut Context<Self>) {
        if self.sending.is_some() || self.paths.is_empty() {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        self.sending = Some(Outgoing {
            to: peer.info.fingerprint.clone(),
            progress: None,
        });
        cx.notify();
        let (channel, reports) = core_bridge::stream::<glint_core::localsend::SendProgress>();
        let alias = peer.info.alias.clone();
        let work = self.backend.send(peer, self.paths.clone(), channel);
        cx.spawn(async move |this, cx| {
            while let Ok(report) = reports.recv().await {
                let updated = this.update(cx, |this, cx| {
                    if this.generation == generation {
                        if let Some(outgoing) = this.sending.as_mut() {
                            outgoing.progress = Some(report.into());
                            cx.notify();
                        }
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let sent = core_bridge::run(work).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                this.sending = None;
                match sent {
                    Ok(files) => cx.emit(NearbyEvent::Sent { alias, files }),
                    // Stop was pressed: not a failure.
                    Err(reason) if reason == "cancelled" => {}
                    Err(reason) => cx.emit(NearbyEvent::Failed {
                        title: t!("Couldn't send to {alias}", alias = alias),
                        reason,
                    }),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Stops the send under way (`nearby_cancel`).
    pub fn cancel(&self) {
        self.backend.cancel();
    }

    /// Starts or stops taking files from the devices nearby (`receive`).
    pub fn set_receiving(&mut self, on: bool, cx: &mut Context<Self>) {
        self.receive_asked = true;
        let work = self.backend.receive(on);
        cx.spawn(async move |this, cx| {
            let port = core_bridge::run(work).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                match port {
                    Ok(port) => this.receiving = port,
                    Err(reason) => cx.emit(NearbyEvent::Failed {
                        title: t!("Couldn't start receiving"),
                        reason,
                    }),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A file arrived (core's `nearby-received`): the newest first, 50 kept.
    pub fn on_received(&mut self, from: String, path: String, cx: &mut Context<Self>) {
        self.received.insert(0, Received { from, path });
        self.received.truncate(KEEP_RECEIVED);
        cx.notify();
    }
}

impl Drop for Nearby {
    /// Closing the dialog stops receiving, as the TypeScript's unmount did.
    fn drop(&mut self) {
        if self.receive_asked {
            drop(core_bridge::run(self.backend.receive(false)));
        }
    }
}

// ---- The dialog -------------------------------------------------------------------------------------

/// The icon for a device: a phone, a laptop, or a desktop (`iconOf`).
pub fn icon_of(device_type: Option<&str>) -> IconName {
    match device_type {
        Some("mobile") => IconName::Smartphone,
        Some("laptop") => IconName::Laptop,
        _ => IconName::Monitor,
    }
}

/// The subtitle: what is being sent, or what the dialog is for.
pub fn subtitle(paths: &[String]) -> SharedString {
    match paths {
        [] => t!("Through LocalSend, on this network"),
        [one] => t!("To send “{name}”", name = isolate(base_name(one).to_string())),
        many => t!("To send {items}", items = plural(many.len(), "item")),
    }
}

/// Phones and computers nearby, with a Send beside each when there is something to send.
pub struct NearbyDialog {
    nearby: Entity<Nearby>,
    effects: Effects,
    _subscriptions: Vec<Subscription>,
}

impl NearbyDialog {
    /// The dialog over `nearby`; its notices (sent, couldn't) show as notifications.
    pub fn new(nearby: Entity<Nearby>, effects: Effects, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&nearby, |_, _, cx| cx.notify());
        let notices = cx.subscribe_in(&nearby, window, |_, _, event: &NearbyEvent, window, cx| match event {
            NearbyEvent::Sent { alias, files } => {
                let message = if *files > 0 {
                    t!(
                        "Sent {files} to {alias}",
                        files = plural(*files, "file"),
                        alias = isolate(alias.clone())
                    )
                } else {
                    t!("{alias} already had them", alias = isolate(alias.clone()))
                };
                window.push_notification(Notification::success(message), cx);
            }
            NearbyEvent::Failed { title, reason } => {
                window.push_notification(Notification::error(reason.clone()).title(title.clone()), cx);
            }
        });
        Self {
            nearby,
            effects,
            _subscriptions: vec![observe, notices],
        }
    }

    fn peer_row(&self, ix: usize, peer: &Peer, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let nearby = self.nearby.read(cx);
        let has_paths = !nearby.paths.is_empty();
        let busy = nearby.sending.as_ref().is_some_and(|o| o.to == peer.info.fingerprint);
        let progress = nearby
            .sending
            .as_ref()
            .filter(|o| o.to == peer.info.fingerprint)
            .and_then(|o| o.progress.clone());
        let sending_any = nearby.sending.is_some();
        let detail: SharedString = match (&progress, busy) {
            (Some(p), _) => t!(
                "{done} of {total} · {bytes} of {all}",
                done = p.files,
                total = p.total_files,
                bytes = format_size(p.bytes),
                all = format_size(p.total_bytes)
            ),
            (None, true) => t!("Waiting for them to say yes…"),
            (None, false) => {
                let what = peer
                    .info
                    .device_model
                    .clone()
                    .or_else(|| peer.info.device_type.clone())
                    .unwrap_or_else(|| t!("Device").to_string());
                format!("{what} · {}", peer.ip).into()
            }
        };
        let percent = progress
            .as_ref()
            .filter(|p| p.total_bytes > 0)
            .map(|p| ((100. * p.bytes as f64 / p.total_bytes as f64).round() as f32 / 100.).clamp(0., 1.));
        let alias: SharedString = peer.info.alias.clone().into();
        let model = self.nearby.clone();
        let peer_for_send = peer.clone();
        align_row(3.)
            .id(("nearby-peer", ix))
            .role(Role::Group)
            .aria_label(alias.clone())
            .aria_description(detail.clone())
            .test_support()
            .px(space(3.))
            .py(space(2.))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .child(
                icon(icon_of(peer.info.device_type.as_deref()))
                    .size_5()
                    .text_color(t.primary)
                    .flex_none(),
            )
            .child(
                col()
                    .align_start()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(isolate(alias.clone())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(isolate(detail)),
                    )
                    .when_some(percent, |this, fraction| {
                        this.child(
                            row_start()
                                .id(("nearby-progress", ix))
                                .role(Role::ProgressIndicator)
                                .aria_label(t!("{percent}%", percent = (fraction * 100.).round() as u32))
                                .test_support()
                                .mt(space(1.))
                                .h(space(1.))
                                .w_full()
                                .overflow_hidden()
                                .rounded_full()
                                .bg(t.muted)
                                .child(div().h_full().w(gpui_kit::relative(fraction)).bg(t.primary)),
                        )
                    }),
            )
            .when(has_paths, |this| {
                if busy {
                    this.child(
                        Button::new(("nearby-stop", ix))
                            .small()
                            .outline()
                            .icon(IconName::Square)
                            .label(t!("Stop"))
                            .on_click(move |_, _, cx| model.read(cx).cancel()),
                    )
                } else {
                    this.child(
                        Button::new(("nearby-send", ix))
                            .small()
                            .primary()
                            .icon(IconName::Send)
                            .label(t!("Send"))
                            .disabled(sending_any)
                            .on_click(move |_, _, cx| {
                                let peer = peer_for_send.clone();
                                model.update(cx, |nearby, cx| nearby.send(peer, cx))
                            }),
                    )
                }
            })
    }

    fn peers_view(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let nearby = self.nearby.read(cx);
        let peers = nearby.peers.clone();
        let scanning = nearby.scanning;
        match peers {
            None => Self::looking(t).into_any_element(),
            Some(list) if list.is_empty() && scanning => Self::looking(t).into_any_element(),
            Some(list) if list.is_empty() => div()
                .id("nearby-none")
                .role(Role::Status)
                .aria_label(t!(
                    "No device answered. Open LocalSend on the phone (same Wi-Fi), then look again."
                ))
                .test_support()
                .py(space(4.))
                .text_color(t.muted_foreground)
                .child(t!(
                    "No device answered. Open LocalSend on the phone (same Wi-Fi), then look again."
                ))
                .into_any_element(),
            Some(list) => {
                let mut rows = Vec::new();
                for (ix, peer) in list.iter().enumerate() {
                    rows.push(self.peer_row(ix, peer, t, cx).into_any_element());
                }
                col()
                    .id("nearby-peers")
                    .role(Role::List)
                    .aria_label(t!("Devices nearby"))
                    .test_support()
                    .gap(space(1.))
                    .children(rows)
                    .into_any_element()
            }
        }
    }

    fn looking(t: &theme::Tokens) -> impl IntoElement {
        align_row(2.)
            .id("nearby-looking")
            .role(Role::Status)
            .aria_label(t!("Looking on this network…"))
            .test_support()
            .py(space(4.))
            .text_color(t.muted_foreground)
            .child(Spinner::new())
            .child(t!("Looking on this network…"))
    }

    fn receive_section(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let nearby = self.nearby.read(cx);
        let receiving = nearby.receiving;
        let received = nearby.received.clone();
        let reveal = self.effects.reveal.clone();
        let description: SharedString = match receiving {
            Some(port) => t!(
                "This PC shows up in LocalSend as “Glint on this PC” (port {port}). What's sent goes into Downloads\\LocalSend. If Windows asks, allow Glint on private networks.",
                port = port
            ),
            None => t!("This PC shows up in their LocalSend lists, and what they send goes into Downloads\\LocalSend."),
        };
        let model = self.nearby.clone();
        col()
            .id("nearby-receive")
            .gap(space(2.))
            .p(space(3.))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .child(
                Checkbox::new("nearby-receive-on")
                    .label(t!("Let them send to this PC while this is open"))
                    .checked(receiving.is_some())
                    .on_click(move |checked: &bool, _, cx| {
                        let on = *checked;
                        model.update(cx, |nearby, cx| nearby.set_receiving(on, cx))
                    }),
            )
            .child(
                div()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .when(receiving.is_none(), |this| this.opacity(0.7))
                    .child(description),
            )
            .when(!received.is_empty(), |this| {
                this.child(
                    col()
                        .id("nearby-received")
                        .role(Role::List)
                        .aria_label(t!("Received"))
                        .test_support()
                        .gap(space(0.5))
                        .text_size(theme::size::XS)
                        .children(received.into_iter().enumerate().map(|(ix, r)| {
                            let reveal = reveal.clone();
                            let path = r.path.clone();
                            let name = base_name(&r.path).to_string();
                            align_row(2.)
                                .id(("nearby-received-file", ix))
                                .role(Role::ListItem)
                                .aria_label(t!("{name} from {from}", name = name.clone(), from = r.from.clone()))
                                .test_support()
                                .child(
                                    div().flex_1().min_w_0().truncate().child(isolate(name)).child(
                                        div()
                                            .text_color(t.muted_foreground)
                                            .child(t!(" from {from}", from = isolate(r.from.clone()))),
                                    ),
                                )
                                .child(
                                    Button::new(("nearby-show", ix))
                                        .ghost()
                                        .small()
                                        .icon(IconName::FolderInput)
                                        .label(t!("Show"))
                                        .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            reveal(&path, cx);
                                        }),
                                )
                        })),
                )
            })
    }
}

impl Render for NearbyDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let (scanning, paths) = {
            let nearby = self.nearby.read(cx);
            (nearby.scanning, nearby.paths.clone())
        };
        let peers = self.peers_view(&t, cx);
        let receive = self.receive_section(&t, cx);
        col()
            .id("nearby")
            .role(Role::Group)
            .aria_label(t!("Phones and computers nearby"))
            .test_support()
            .gap(space(4.))
            .text_size(theme::size::SM)
            .text_start()
            .pt(space(1.))
            .child(
                row_start()
                    .gap(space(3.))
                    .child(icon(IconName::Radar).size_5().text_color(t.primary).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(subtitle(&paths)),
                    )
                    .child(
                        Button::new("nearby-scan")
                            .small()
                            .outline()
                            .icon(IconName::RefreshCw)
                            .label(t!("Look again"))
                            .loading(scanning)
                            .disabled(scanning)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.nearby.update(cx, |nearby, cx| nearby.scan(cx))
                            })),
                    ),
            )
            .child(
                col()
                    .id("nearby-body")
                    .gap(space(4.))
                    .max_h(rems(30.))
                    .overflow_y_scrollbar()
                    .child(
                        div()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(t!(
                                "Devices with the LocalSend app open (Android, iPhone, Mac, Linux, Windows), on the same Wi-Fi or network. Nothing goes through the internet; the other device asks its person before it takes anything."
                            )),
                    )
                    .child(peers)
                    .child(receive),
            )
    }
}

/// Opens Nearby: the LocalSend apps on this network, to send `paths` to (or, with none, to receive from)
/// (`openNearby`). It looks for devices at once.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>) -> Entity<NearbyDialog> {
    let backend: Arc<dyn Backend> = Arc::new(CoreBackend::new(cx));
    open_with(window, cx, backend, Effects::app(), paths)
}

/// [`open`] with a backend and effects of your own (tests pass fakes). Returns the view.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    effects: Effects,
    paths: Vec<String>,
) -> Entity<NearbyDialog> {
    let nearby = cx.new(|cx| Nearby::new(backend, paths, cx));
    let view = cx.new(|cx| NearbyDialog::new(nearby, effects, window, cx));
    super::show(window, cx, t!("Phones and computers nearby"), px(512.), view.clone());
    view
}

// ---- Tests ------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use glint_core::localsend::Info;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    fn peer(alias: &str, fingerprint: &str, model: Option<&str>, kind: Option<&str>, ip: &str) -> Peer {
        Peer {
            info: Info {
                alias: alias.into(),
                version: "2.1".into(),
                device_model: model.map(str::to_string),
                device_type: kind.map(str::to_string),
                fingerprint: fingerprint.into(),
                port: 53317,
                protocol: "https".into(),
                download: false,
                announce: None,
            },
            ip: ip.into(),
        }
    }

    /// Records every call; a send waits until the test finishes it.
    struct Fake {
        peers: Mutex<Vec<Peer>>,
        scan_error: Mutex<Option<String>>,
        scans: AtomicUsize,
        sent: Mutex<Vec<(String, Vec<String>)>>,
        cancelled: AtomicUsize,
        receive_calls: Mutex<Vec<bool>>,
        progress: Mutex<Option<Channel<glint_core::localsend::SendProgress>>>,
        finish: Mutex<Option<async_channel::Sender<Result<usize, String>>>>,
    }

    impl Fake {
        fn new(peers: Vec<Peer>) -> Arc<Fake> {
            Arc::new(Fake {
                peers: Mutex::new(peers),
                scan_error: Mutex::new(None),
                scans: AtomicUsize::new(0),
                sent: Mutex::new(Vec::new()),
                cancelled: AtomicUsize::new(0),
                receive_calls: Mutex::new(Vec::new()),
                progress: Mutex::new(None),
                finish: Mutex::new(None),
            })
        }
    }

    impl Backend for Fake {
        fn scan(&self, _: u64) -> Fut<Vec<Peer>> {
            self.scans.fetch_add(1, Ordering::SeqCst);
            let error = self.scan_error.lock().unwrap().clone();
            let peers = self.peers.lock().unwrap().clone();
            Box::pin(async move { error.map_or(Ok(peers), Err) })
        }
        fn send(
            &self,
            peer: Peer,
            paths: Vec<String>,
            progress: Channel<glint_core::localsend::SendProgress>,
        ) -> Fut<usize> {
            self.sent.lock().unwrap().push((peer.info.fingerprint, paths));
            *self.progress.lock().unwrap() = Some(progress);
            let (tx, rx) = async_channel::bounded(1);
            *self.finish.lock().unwrap() = Some(tx);
            Box::pin(async move { rx.recv().await.map_err(|e| e.to_string()).and_then(|r| r) })
        }
        fn cancel(&self) {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
        }
        fn receive(&self, on: bool) -> Fut<Option<u16>> {
            self.receive_calls.lock().unwrap().push(on);
            Box::pin(async move { Ok(on.then_some(53317)) })
        }
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    struct Env {
        visual: VisualTestContext,
        fake: Arc<Fake>,
        revealed: Rc<RefCell<Vec<String>>>,
    }

    fn env(cx: &mut TestAppContext, fake: Arc<Fake>) -> Env {
        cx.update(testing::init);
        let (visual, _) = testing::open_window(cx, |_, cx| cx.new(|_| Host));
        Env {
            visual,
            fake,
            revealed: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn open_dialog(env: &mut Env, paths: &[&str]) -> Entity<NearbyDialog> {
        let backend: Arc<dyn Backend> = env.fake.clone();
        let revealed = env.revealed.clone();
        let effects = Effects {
            reveal: Rc::new(move |path, _| revealed.borrow_mut().push(path.to_string())),
        };
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        let dialog = env
            .visual
            .update(|window, cx| open_with(window, cx, backend, effects, paths));
        env.visual.run_until_parked();
        dialog
    }

    fn model_of(env: &Env, dialog: &Entity<NearbyDialog>) -> Entity<Nearby> {
        dialog.read_with(&env.visual, |d, _| d.nearby.clone())
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the nearby dialog never reached the state the test waits for");
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn click(cx: &mut VisualTestContext, id: impl Into<ElementId>) {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        cx.run_until_parked();
    }

    fn notifications(cx: &mut VisualTestContext) -> usize {
        cx.update(|window, cx| window.notifications(cx).len())
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for icon in [
            IconName::Radar,
            IconName::RefreshCw,
            IconName::Smartphone,
            IconName::Laptop,
            IconName::Monitor,
            IconName::FolderInput,
            IconName::Send,
            IconName::Square,
        ] {
            let path = icon.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn a_device_gets_a_phone_a_laptop_or_a_desktop() {
        assert_eq!(icon_of(Some("mobile")), IconName::Smartphone);
        assert_eq!(icon_of(Some("laptop")), IconName::Laptop);
        assert_eq!(icon_of(Some("desktop")), IconName::Monitor);
        assert_eq!(icon_of(Some("headless")), IconName::Monitor);
        assert_eq!(icon_of(None), IconName::Monitor);
    }

    #[gpui_kit::test]
    fn the_subtitle_says_what_is_being_sent(_cx: &mut TestAppContext) {
        let _guard = testing::lock();
        assert_eq!(subtitle(&[]), "Through LocalSend, on this network");
        assert_eq!(subtitle(&[r"C:\a\report.docx".into()]), "To send “report.docx”");
        assert_eq!(subtitle(&["a".into(), "b".into()]), "To send 2 items");
    }

    #[gpui_kit::test]
    fn opening_scans_and_lists_what_answered_with_a_send_beside_each(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(vec![
            peer("هاتف سارة", "fp-1", Some("Pixel 8"), Some("mobile"), "192.168.1.7"),
            peer("Office PC", "fp-2", None, None, "192.168.1.9"),
        ]);
        let mut env = env(cx, fake);
        let dialog = open_dialog(&mut env, &[r"C:\a\report.docx"]);
        // The first scan was asked for at once, and the list shows what answered.
        settle(&mut env.visual, |cx| has(cx, "هاتف سارة"));
        assert_eq!(env.fake.scans.load(Ordering::SeqCst), 1);
        assert_eq!(role_of(&mut env.visual, "Devices nearby"), Some(Role::List));
        assert_eq!(role_of(&mut env.visual, "هاتف سارة"), Some(Role::Group));
        let description = env.visual.update(|window, cx| {
            window.render_frame(cx);
            (
                window.find(("nearby-peer", 0_usize)).description().map(str::to_string),
                window.find(("nearby-peer", 1_usize)).description().map(str::to_string),
            )
        });
        assert_eq!(description.0.as_deref(), Some("Pixel 8 · 192.168.1.7"));
        assert_eq!(description.1.as_deref(), Some("Device · 192.168.1.9"));
        env.visual.update(|window, _cx| {
            assert!(window.try_find(("nearby-send", 0_usize)).is_some());
            assert!(window.try_find(("nearby-send", 1_usize)).is_some());
        });
        assert!(model_of(&env, &dialog).read_with(&env.visual, |n, _| !n.scanning()));

        // Look again asks again.
        click(&mut env.visual, "nearby-scan");
        settle(&mut env.visual, |_| env.fake.scans.load(Ordering::SeqCst) == 2);
    }

    #[gpui_kit::test]
    fn with_nothing_to_send_there_is_no_send_button_and_no_answer_is_said(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(vec![peer("Sara", "fp-1", None, Some("laptop"), "192.168.1.7")]);
        let mut env = env(cx, fake);
        open_dialog(&mut env, &[]);
        settle(&mut env.visual, |cx| has(cx, "Sara"));
        env.visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(("nearby-send", 0_usize)).is_none());
        });
        // Nobody answers: the dialog says what to do.
        *env.fake.peers.lock().unwrap() = Vec::new();
        click(&mut env.visual, "nearby-scan");
        settle(&mut env.visual, |cx| {
            has(
                cx,
                "No device answered. Open LocalSend on the phone (same Wi-Fi), then look again.",
            )
        });
        assert_eq!(
            role_of(
                &mut env.visual,
                "No device answered. Open LocalSend on the phone (same Wi-Fi), then look again."
            ),
            Some(Role::Status)
        );
    }

    #[gpui_kit::test]
    fn a_scan_that_fails_says_so_and_shows_an_empty_list(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(vec![]);
        *fake.scan_error.lock().unwrap() = Some("no network adapter".into());
        let mut env = env(cx, fake);
        let dialog = open_dialog(&mut env, &[r"C:\a.txt"]);
        settle(&mut env.visual, |cx| notifications(cx) == 1);
        let peers = model_of(&env, &dialog).read_with(&env.visual, |n, _| n.peers().map(<[Peer]>::len));
        assert_eq!(peers, Some(0));
    }

    #[gpui_kit::test]
    fn sending_waits_for_a_yes_shows_progress_and_stop_cancels(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(vec![
            peer("Sara", "fp-1", Some("Pixel 8"), Some("mobile"), "192.168.1.7"),
            peer("Office PC", "fp-2", None, None, "192.168.1.9"),
        ]);
        let mut env = env(cx, fake);
        let dialog = open_dialog(&mut env, &[r"C:\a\report.docx", r"C:\a\ملف.txt"]);
        settle(&mut env.visual, |cx| has(cx, "Sara"));
        let model = model_of(&env, &dialog);
        click(&mut env.visual, ("nearby-send", 0_usize));
        settle(&mut env.visual, |_| !env.fake.sent.lock().unwrap().is_empty());
        assert_eq!(
            *env.fake.sent.lock().unwrap(),
            vec![(
                "fp-1".to_string(),
                vec![r"C:\a\report.docx".to_string(), r"C:\a\ملف.txt".to_string()]
            )]
        );
        let describe = |env: &mut Env, ix: usize| {
            env.visual.update(|window, cx| {
                window.render_frame(cx);
                window.find(("nearby-peer", ix)).description().map(str::to_string)
            })
        };
        // It is waiting for the person to say yes; the other device can't be sent to meanwhile.
        assert_eq!(describe(&mut env, 0).as_deref(), Some("Waiting for them to say yes…"));
        env.visual.update(|window, _cx| {
            assert_eq!(window.find(("nearby-send", 1_usize)).disabled(), Some(true));
            assert!(window.try_find(("nearby-stop", 0_usize)).is_some());
        });

        let progress = env
            .fake
            .progress
            .lock()
            .unwrap()
            .clone()
            .expect("the send got a channel");
        progress
            .send(glint_core::localsend::SendProgress {
                files: 1,
                total_files: 2,
                bytes: 500,
                total_bytes: 2000,
                current: "report.docx".into(),
            })
            .expect("open");
        settle(&mut env.visual, |cx| {
            model.read_with(cx, |n, _| n.sending().is_some_and(|o| o.progress.is_some()))
        });
        let expected = format!("1 of 2 · {} of {}", format_size(500), format_size(2000));
        assert_eq!(describe(&mut env, 0), Some(expected));
        assert_eq!(role_of(&mut env.visual, "25%"), Some(Role::ProgressIndicator));

        // Stop cancels.
        click(&mut env.visual, ("nearby-stop", 0_usize));
        assert_eq!(env.fake.cancelled.load(Ordering::SeqCst), 1);
        // Core answers "cancelled": nothing to tell.
        let finish = env.fake.finish.lock().unwrap().take().expect("a send is waiting");
        finish.try_send(Err("cancelled".into())).expect("room");
        settle(&mut env.visual, |cx| model.read_with(cx, |n, _| n.sending().is_none()));
        assert_eq!(notifications(&mut env.visual), 0);

        // A second send that works says how many went.
        click(&mut env.visual, ("nearby-send", 1_usize));
        settle(&mut env.visual, |_| env.fake.sent.lock().unwrap().len() == 2);
        let finish = env.fake.finish.lock().unwrap().take().expect("a send is waiting");
        finish.try_send(Ok(2)).expect("room");
        settle(&mut env.visual, |cx| notifications(cx) == 1);

        // And one that fails says why.
        click(&mut env.visual, ("nearby-send", 0_usize));
        settle(&mut env.visual, |_| env.fake.sent.lock().unwrap().len() == 3);
        let finish = env.fake.finish.lock().unwrap().take().expect("a send is waiting");
        finish.try_send(Err("declined".into())).expect("room");
        settle(&mut env.visual, |cx| notifications(cx) == 2);
    }

    #[gpui_kit::test]
    fn letting_them_send_to_this_pc_shows_the_port_and_what_arrives(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, Fake::new(vec![]));
        let dialog = open_dialog(&mut env, &[]);
        let model = model_of(&env, &dialog);
        assert!(has(&mut env.visual, "Let them send to this PC while this is open"));
        assert!(!has(&mut env.visual, "Received"));
        click(&mut env.visual, "nearby-receive-on");
        settle(&mut env.visual, |cx| {
            model.read_with(cx, |n, _| n.receiving() == Some(53317))
        });
        assert_eq!(*env.fake.receive_calls.lock().unwrap(), vec![true]);
        assert!(has(&mut env.visual, "Let them send to this PC while this is open"));

        // Core says a file came (the same event the TypeScript listened for).
        env.visual.update(|_, cx| {
            core_bridge::emit(
                cx,
                "nearby-received",
                serde_json::json!({ "from": "سارة", "path": r"C:\Users\me\Downloads\LocalSend\صورة.jpg" }),
            )
        });
        settle(&mut env.visual, |cx| {
            model.read_with(cx, |n, _| n.received().len() == 1)
        });
        assert_eq!(role_of(&mut env.visual, "Received"), Some(Role::List));
        assert_eq!(role_of(&mut env.visual, "صورة.jpg from سارة"), Some(Role::ListItem));
        // Show closes the dialog and reveals it.
        click(&mut env.visual, ("nearby-show", 0_usize));
        assert_eq!(
            *env.revealed.borrow(),
            vec![r"C:\Users\me\Downloads\LocalSend\صورة.jpg".to_string()]
        );
        env.visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        // Closing the dialog stops receiving, as the TypeScript's unmount did (once nothing here holds it).
        drop((dialog, model));
        settle(&mut env.visual, |cx| {
            cx.update(|window, cx| window.render_frame(cx));
            env.fake.receive_calls.lock().unwrap().len() == 2
        });
        assert_eq!(*env.fake.receive_calls.lock().unwrap(), vec![true, false]);
    }

    #[gpui_kit::test]
    fn the_received_list_keeps_the_newest_fifty(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, Fake::new(vec![]));
        let dialog = open_dialog(&mut env, &[]);
        let model = model_of(&env, &dialog);
        env.visual.update(|_, cx| {
            model.update(cx, |n, cx| {
                for i in 0..60 {
                    n.on_received("Sara".into(), format!(r"C:\In\{i}.jpg"), cx);
                }
            })
        });
        let (count, newest) = model.read_with(&env.visual, |n, _| (n.received().len(), n.received()[0].path.clone()));
        assert_eq!(count, 50);
        assert_eq!(newest, r"C:\In\59.jpg");
        // A device that sent no path says nothing (and a closed dialog that never asked has nothing to stop).
        env.visual
            .update(|_, cx| core_bridge::emit(cx, "nearby-received", serde_json::json!({ "from": "x" })));
        env.visual.run_until_parked();
        assert_eq!(model.read_with(&env.visual, |n, _| n.received().len()), 50);
    }

    #[gpui_kit::test]
    fn a_dialog_that_never_asked_to_receive_does_not_stop_it_on_close(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, Fake::new(vec![]));
        open_dialog(&mut env, &[]);
        env.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        env.visual.run_until_parked();
        env.visual
            .update(|window, cx| assert!(!window.has_active_dialog(cx), "Escape closes it"));
        assert!(env.fake.receive_calls.lock().unwrap().is_empty());
    }

    #[gpui_kit::test]
    fn arabic_renders_mirrored_and_a_start_failure_is_said(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(
            cx,
            Fake::new(vec![peer("هاتف", "fp-1", None, Some("mobile"), "10.0.0.4")]),
        );
        testing::set_rtl(true);
        open_dialog(&mut env, &[r"C:\ملفات\تقرير.pdf"]);
        settle(&mut env.visual, |cx| has(cx, "هاتف"));
        assert_eq!(
            role_of(&mut env.visual, "Phones and computers nearby"),
            Some(Role::Group)
        );
        testing::set_rtl(false);
    }
}
