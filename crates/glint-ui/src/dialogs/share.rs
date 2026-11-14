//! Sharing files: to another PC with sendme, on this network with dufs, and with Windows' own Share.
//! Ported from src/components/explorer/ShareDialog.tsx and src/lib/share.ts, built the way
//! [`super::drive_health`] is.
//!
//! | TypeScript                                   | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `useShare` (zustand) and `lib/share.ts`      | [`Sharing`], one model in a global ([`shared`])               |
//! | `sendToPC`, `stopSend`                       | [`Sharing::send_to_pc`], [`Sharing::stop_send`]               |
//! | `openReceive`, `receive`, `stopReceive`      | [`Sharing::open_receive`], [`Sharing::receive`], [`Sharing::stop_receive`] |
//! | `openNetworkShare`, `serve`, `stopServing`   | [`Sharing::open_network_share`], [`Sharing::serve`], [`Sharing::stop_serving`] |
//! | `checkReach`, `recheckShares`                | [`Sharing::recheck`] (the dialog calls it when the window comes back to the front) |
//! | `useSharing()` and `SharingButton`           | [`Sharing::button`] (the title bar draws the button)          |
//! | `offerToPC`, `offerOnNetwork`                | [`offer_to_pc`], [`offer_on_network`] (the secrets check first) |
//! | `windowsShare` ("Share with Windows…")       | [`share_with_windows`]                                        |
//! | `ShareDialog`, `ShareModal`                  | [`ShareDialog`], opened with [`open`], [`open_view`], [`open_receive`] |
//! | `SendView`, `ReceiveView`, `NetworkView`     | the three views of [`ShareDialog`]                            |
//! | `Qr` (an `<img>` of core's SVG)              | [`Qr`], parsed from core's SVG and drawn with quads           |
//! | `ToolNeeded` (sendme, dufs)                  | `settings::tools::needed`                                     |
//!
//! **How the coordinator wires it.**
//!
//! - `MenuEvent::SendToPc { path }` is `share::offer_to_pc(window, cx, path)`: looks for secrets first, then
//!   opens the dialog on Send with the file's ticket coming.
//! - `MenuEvent::ShareOnNetwork { path }` is `share::offer_on_network(window, cx, path)`: the same look, then
//!   the dialog on "On this network", with the choices (which network, how long, whether they may add files).
//! - `MenuEvent::ShareWithWindows { paths }` is `share::share_with_windows(window, cx, paths)`: Windows'
//!   Share flyout, started by glint-core on the UI thread's message pump (never inside a GPUI callback).
//! - `MenuEvent::ReceiveHere { folder }` is `share::open_receive(window, cx, folder)`.
//! - The title bar's "Sending" / "Sharing" button: `share::shared(cx).read(cx).button()` gives its label and
//!   the view it opens (`None`: nothing is on, no button); its click is `share::open_view(window, cx, view)`.
//!   Observe the model (`cx.observe(&share::shared(cx), ..)`) to redraw the button.
//! - `share::open(window, cx, paths)` opens "On this network" offering `paths[0]` (dufs serves one thing, so
//!   the menus pass one; with no paths it just shows what is shared). It skips the secrets check; the two
//!   `offer_*` functions above do not.
//! - When Glint's window is in front again after Windows' prompt or Settings, [`Sharing::recheck`] reads what
//!   keeps other devices out of each share again; the open dialog does that itself.
//!
//! What was not ported: the `<select>` boxes (which network, how long) are radio buttons here, because the
//! kit has no select yet; the link is plain text with Copy, not text you can select (the ticket is a
//! read-only text box, which can be); "Stops at {time}" says "Today 3:45 PM" for a share ending today, as
//! glint-model has no bare time of day to print.

use crate::components::settings::tools::{self, Tools};
use crate::components::settings::{Backend as SettingsBackend, CoreBackend as SettingsCoreBackend};
use crate::kit::prelude::*;
use crate::kit::{Textarea, TextareaState};
use crate::{core_bridge, explorer, i18n, t, theme};
use glint_core::host::ipc::Channel;
use glint_core::host::Manager as _;
use glint_core::lan::{Address, Firewall, LanEvent, LanOptions, Reach};
use glint_core::share::{ReceiveEvent, SendEvent};
use glint_model::format::{format_date, format_size, plural};
use glint_model::paths::{base_name, parent_path, same_path};
use gpui_kit::{canvas, fill, point, size, Bounds, ClipboardItem, EventEmitter, WeakEntity};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

// ---- The calls --------------------------------------------------------------------------------------

/// The calls sharing makes. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync + 'static {
    /// `send_start`: offers `path` to another PC; the ticket comes on `events`. Resolves to the run's id.
    fn send_start(&self, path: String, events: Channel<SendEvent>) -> Fut<u64>;
    /// `receive_start`: takes what `ticket` offers into `folder`. Resolves to the run's id.
    fn receive_start(&self, folder: String, ticket: String, events: Channel<ReceiveEvent>) -> Fut<u64>;
    /// `lan_share_start`: serves `path` on this network. Resolves to the run's id.
    fn serve_start(&self, path: String, options: LanOptions, events: Channel<LanEvent>) -> Fut<u64>;
    /// `share_stop`: ends a send, a receive or a share.
    fn stop(&self, id: u64);
    /// `lan_addresses`: this PC's addresses on its networks.
    fn addresses(&self) -> Fut<Vec<Address>>;
    /// `lan_reach`: what keeps other devices from a share at `address`.
    fn reach(&self, address: String) -> Fut<Reach>;
    /// `lan_open_settings`: opens "network" or "firewall" in Windows.
    fn open_settings(&self, page: &'static str, address: String) -> Fut<()>;
    /// `windows_share`: Windows' Share flyout for `paths`.
    fn windows_share(&self, paths: Vec<String>) -> Fut<()>;
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
    fn send_start(&self, path: String, events: Channel<SendEvent>) -> Fut<u64> {
        Box::pin(glint_core::share::send_start(path, events))
    }
    fn receive_start(&self, folder: String, ticket: String, events: Channel<ReceiveEvent>) -> Fut<u64> {
        Box::pin(glint_core::share::receive_start(folder, ticket, events))
    }
    fn serve_start(&self, path: String, options: LanOptions, events: Channel<LanEvent>) -> Fut<u64> {
        Box::pin(glint_core::lan::lan_share_start(path, options, events))
    }
    fn stop(&self, id: u64) {
        glint_core::share::share_stop(id);
    }
    fn addresses(&self) -> Fut<Vec<Address>> {
        Box::pin(glint_core::lan::lan_addresses())
    }
    fn reach(&self, address: String) -> Fut<Reach> {
        Box::pin(glint_core::lan::lan_reach(address))
    }
    fn open_settings(&self, page: &'static str, address: String) -> Fut<()> {
        Box::pin(glint_core::lan::lan_open_settings(page.to_string(), address))
    }
    fn windows_share(&self, paths: Vec<String>) -> Fut<()> {
        let window = self.app.as_ref().and_then(|app| {
            app.get_webview_window(crate::workspace::MAIN)
                .or_else(|| app.webview_windows().into_values().next())
        });
        match window {
            // glint-core runs the flyout on the UI thread's message pump (`on_main`), not in a GPUI callback.
            Some(window) => Box::pin(glint_core::shell::windows_share(window, paths)),
            None => Box::pin(async { Err(t!("Windows didn't answer").to_string()) }),
        }
    }
}

/// What happens to a path or a link, given the app.
pub type OnPath = Rc<dyn Fn(&str, &mut App)>;

/// What the dialog does to the rest of the app: the real ones in the app, recorders in tests.
#[derive(Clone)]
pub struct Effects {
    /// Shows a file in its folder, selected (`revealInApp`).
    pub reveal: OnPath,
    /// Opens a link in the browser (`openUrl`).
    pub open_url: OnPath,
    /// Now, in milliseconds since 1970 (a share's countdown).
    pub now: fn() -> i64,
}

impl Effects {
    /// The app's.
    pub fn app() -> Self {
        Self {
            reveal: Rc::new(|path, cx| {
                if let Some(explorer) = explorer::try_explorer(cx) {
                    let options = explorer::NavOptions::default().selecting(path.to_string());
                    explorer.update(cx, |explorer, cx| explorer.navigate(parent_path(path), options, cx));
                }
            }),
            open_url: Rc::new(|url, cx| cx.open_url(url)),
            now: now_ms,
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

// ---- The QR code ------------------------------------------------------------------------------------

/// A QR code, as core draws it (an SVG of dark rectangles), ready to paint with quads.
#[derive(Clone, Debug, PartialEq)]
pub struct Qr {
    /// The SVG's width and height (it is square), the units of the rectangles.
    pub side: f32,
    /// The dark rectangles: left, top, width, height.
    pub rects: Vec<[f32; 4]>,
}

impl Qr {
    /// Reads core's SVG (`qr_svg`): a `viewBox="0 0 W H"` and one path of `M{l} {t}h{w}v{h}H{l}V{t}`
    /// pieces. `None` when it isn't that, or is empty (the draw then shows nothing, as a broken `<img>` did).
    pub fn from_svg(svg: &str) -> Option<Qr> {
        let after = |text: &'static str| svg.split_once(text).map(|(_, rest)| rest);
        let side = after("viewBox=\"0 0 ")?
            .split_whitespace()
            .next()?
            .parse::<f32>()
            .ok()
            .filter(|side| *side > 0.)?;
        let path = after(" d=\"")?.split('"').next()?;
        let mut rects = Vec::new();
        for piece in path.split('M').filter(|piece| !piece.trim().is_empty()) {
            let (origin, rest) = piece.split_once('h')?;
            let (width, rest) = rest.split_once('v')?;
            let height = rest.split('H').next()?;
            let mut origin = origin.split_whitespace();
            let left: f32 = origin.next()?.parse().ok()?;
            let top: f32 = origin.next()?.parse().ok()?;
            rects.push([left, top, width.trim().parse().ok()?, height.trim().parse().ok()?]);
        }
        (!rects.is_empty()).then_some(Qr { side, rects })
    }
}

/// Paints `qr` in the largest square that fits `bounds`, each rectangle snapped to whole pixels so neighbours
/// meet without seams. Black on white whatever the theme: a scanner needs the contrast.
fn paint_qr(qr: &Qr, bounds: Bounds<Pixels>, window: &mut Window) {
    let extent = f32::from(bounds.size.width.min(bounds.size.height));
    let scale = extent / qr.side;
    let (left, top) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    for [x, y, w, h] in &qr.rects {
        let (x0, x1) = ((left + x * scale).round(), (left + (x + w) * scale).round());
        let (y0, y1) = ((top + y * scale).round(), (top + (y + h) * scale).round());
        let quad = Bounds {
            origin: point(px(x0), px(y0)),
            size: size(px(x1 - x0), px(y1 - y0)),
        };
        window.paint_quad(fill(quad, gpui_kit::rgb(0x000000)));
    }
}

/// A QR code on a white card, `extent` wide and tall, named for screen readers.
fn qr_view(id: impl Into<ElementId>, qr: Rc<Qr>, extent: Rems, label: SharedString) -> impl IntoElement {
    div()
        .id(id)
        .role(Role::Image)
        .aria_label(label)
        .test_support()
        .flex_none()
        .size(extent)
        .p(px(4.))
        .rounded(theme::size::RADIUS)
        .bg(gpui_kit::rgb(0xffffff))
        .child(canvas(|_, _, _| (), move |bounds, _, window, _| paint_qr(&qr, bounds, window)).size_full())
}

// ---- What is sent, received and shared -----------------------------------------------------------------

/// Which part of the dialog is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// Send to another PC.
    Send,
    /// Receive from another PC.
    Receive,
    /// Share on this network.
    Network,
}

impl View {
    const ALL: [View; 3] = [View::Send, View::Receive, View::Network];

    fn tab(self) -> SharedString {
        match self {
            View::Send => t!("Send"),
            View::Receive => t!("Receive"),
            View::Network => t!("On this network"),
        }
    }

    fn title(self) -> SharedString {
        match self {
            View::Send => t!("Send to another PC"),
            View::Receive => t!("Receive from another PC"),
            View::Network => t!("Share on this network"),
        }
    }

    fn subtitle(self) -> SharedString {
        match self {
            View::Send => t!("With sendme: PC to PC, encrypted end to end"),
            View::Receive => t!("With sendme: from the ticket it shows"),
            View::Network => t!("With dufs: a link for a phone's browser"),
        }
    }

    fn icon(self) -> IconName {
        match self {
            View::Send => IconName::Send,
            View::Receive => IconName::Download,
            View::Network => IconName::Wifi,
        }
    }
}

/// A file or folder offered to another PC with sendme, from the start to its Stop (`Sending`).
#[derive(Clone, Debug)]
pub struct Sending {
    pub key: u64,
    pub path: String,
    /// Its run's id, once sendme has started.
    pub id: Option<u64>,
    /// What receives it, once sendme has read it through; its size as sendme says it; and as a QR code.
    pub ticket: Option<String>,
    pub size: Option<String>,
    pub qr: Option<Rc<Qr>>,
    /// Why it ended by itself.
    pub ended: Option<String>,
}

/// What is being received, from a ticket into a folder (`Receiving`).
#[derive(Clone, Debug)]
pub struct Receiving {
    pub key: u64,
    pub id: Option<u64>,
    pub folder: String,
    pub files: Option<u64>,
    /// Its size as sendme says it, in bytes, and how much is here.
    pub size: Option<String>,
    pub total: Option<u64>,
    pub bytes: u64,
    /// Where it went.
    pub done: Option<String>,
    pub failed: Option<String>,
}

impl Receiving {
    fn running(&self) -> bool {
        self.done.is_none() && self.failed.is_none()
    }
}

/// The last request a share answered.
#[derive(Clone, Debug, PartialEq)]
pub struct Visit {
    pub from: String,
    pub request: String,
    pub status: u16,
    /// When (ms since 1970).
    pub at: i64,
}

/// A folder or file shared on this network with dufs: its link once dufs is listening (`Serving`).
#[derive(Clone, Debug)]
pub struct Serving {
    pub key: u64,
    pub path: String,
    pub id: Option<u64>,
    pub uploads: bool,
    pub url: Option<String>,
    pub qr: Option<Rc<Qr>>,
    /// The address it is served at, once it is listening.
    pub address: Option<String>,
    /// What may keep other devices from it, as Windows has it.
    pub reach: Option<Reach>,
    /// When it ends by itself (ms since 1970); `None` when it is stopped by hand.
    pub ends: Option<u64>,
    pub visits: usize,
    pub from: Vec<String>,
    pub last: Option<Visit>,
    pub ended: Option<String>,
}

/// This PC's addresses, as the form for sharing needs them.
#[derive(Clone, Debug, PartialEq)]
pub enum Addresses {
    /// Not asked for yet.
    Unknown,
    Loading,
    /// The answer (empty when this PC isn't on a network, or core couldn't say).
    Known(Vec<Address>),
}

/// The choices made last time, for the next (while Glint runs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    /// Whoever has the link may save files into it.
    pub uploads: bool,
    /// Minutes until it stops by itself; 0 when it is stopped by hand.
    pub minutes: u32,
}

/// What the model tells whoever listens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareEvent {
    /// Something arrived at `path`. `shown` is whether the dialog was showing Receive, which says so itself.
    Received { path: String, shown: bool },
}

/// Everything sent, received and shared since Glint started (`useShare`). One lives in a global ([`shared`]),
/// so closing the dialog changes nothing: the ticket and the link stay good until Stop.
pub struct Sharing {
    backend: Arc<dyn Backend>,
    effects: Effects,
    sends: Vec<Sending>,
    receive: Option<Receiving>,
    /// Where Receive puts what comes.
    into: String,
    serving: Vec<Serving>,
    /// What "Share on this network" was opened on, to be shared.
    offer: String,
    addresses: Addresses,
    choice: Choice,
    dialog: Option<WeakEntity<ShareDialog>>,
    next: u64,
}

impl EventEmitter<ShareEvent> for Sharing {}

impl Sharing {
    /// A model calling `backend`.
    pub fn new(backend: Arc<dyn Backend>, effects: Effects) -> Self {
        Self {
            backend,
            effects,
            sends: Vec::new(),
            receive: None,
            into: String::new(),
            serving: Vec::new(),
            offer: String::new(),
            addresses: Addresses::Unknown,
            choice: Choice {
                uploads: false,
                minutes: 60,
            },
            dialog: None,
            next: 0,
        }
    }

    pub fn sends(&self) -> &[Sending] {
        &self.sends
    }

    pub fn receiving(&self) -> Option<&Receiving> {
        self.receive.as_ref()
    }

    /// Where Receive puts what comes (empty before it was opened).
    #[cfg(test)]
    pub fn into_folder(&self) -> &str {
        &self.into
    }

    pub fn serving(&self) -> &[Serving] {
        &self.serving
    }

    /// What "Share on this network" waits to share (empty when nothing).
    pub fn offer(&self) -> &str {
        &self.offer
    }

    pub fn addresses(&self) -> &Addresses {
        &self.addresses
    }

    pub fn choice(&self) -> Choice {
        self.choice
    }

    /// The title bar's button: its label and the view it opens, while something is sent or shared; `None`
    /// when nothing is (no button) (`useSharing` and `SharingButton`).
    pub fn button(&self) -> Option<(SharedString, View)> {
        let sends = self.sends.iter().filter(|s| s.ended.is_none()).count();
        let serving = self.serving.iter().filter(|s| s.ended.is_none()).count();
        let count = sends + serving;
        if count == 0 {
            return None;
        }
        let view = if sends > 0 { View::Send } else { View::Network };
        let label = if count == 1 && view == View::Send {
            t!("Sending")
        } else if count > 1 {
            t!("Sharing {n}", n = count)
        } else {
            t!("Sharing")
        };
        Some((label, view))
    }

    fn next_key(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// The view the dialog is showing, if it is open.
    fn dialog_view(&self, cx: &App) -> Option<View> {
        self.dialog.as_ref()?.upgrade().map(|dialog| dialog.read(cx).view)
    }

    // ---- Send ---------------------------------------------------------------------------------------

    /// Offers `path` to another PC: its ticket shows once sendme has read it (`sendToPC`). Offering what is
    /// offered already changes nothing.
    pub fn send_to_pc(&mut self, path: String, cx: &mut Context<Self>) {
        if self
            .sends
            .iter()
            .any(|s| same_path(&s.path, &path) && s.ended.is_none())
        {
            return;
        }
        let key = self.next_key();
        self.sends.push(Sending {
            key,
            path: path.clone(),
            id: None,
            ticket: None,
            size: None,
            qr: None,
            ended: None,
        });
        cx.notify();
        let (channel, events) = core_bridge::stream::<SendEvent>();
        let start = self.backend.send_start(path, channel);
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this.update(cx, |this, cx| this.on_send_event(key, event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let started = core_bridge::run(start).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| this.send_started(key, started, cx));
        })
        .detach();
    }

    fn on_send_event(&mut self, key: u64, event: SendEvent, cx: &mut Context<Self>) {
        let Some(send) = self.sends.iter_mut().find(|s| s.key == key) else {
            return;
        };
        match event {
            SendEvent::Ready { ticket, size, qr } => {
                send.ticket = Some(ticket);
                send.size = Some(size);
                send.qr = Qr::from_svg(&qr).map(Rc::new);
            }
            SendEvent::Ended { message } => send.ended = Some(message),
        }
        cx.notify();
    }

    fn send_started(&mut self, key: u64, started: Result<u64, String>, cx: &mut Context<Self>) {
        match (self.sends.iter_mut().find(|s| s.key == key), started) {
            (Some(send), Ok(id)) => send.id = Some(id),
            (Some(send), Err(reason)) => send.ended = Some(reason),
            // Stopped while it started.
            (None, Ok(id)) => self.backend.stop(id),
            (None, Err(_)) => {}
        }
        cx.notify();
    }

    /// Stops a send (its ticket stops working), or takes away one that ended (`stopSend`).
    pub fn stop_send(&mut self, key: u64, cx: &mut Context<Self>) {
        if let Some(send) = self.sends.iter().find(|s| s.key == key) {
            if let (Some(id), None) = (send.id, &send.ended) {
                self.backend.stop(id);
            }
        }
        self.sends.retain(|s| s.key != key);
        cx.notify();
    }

    // ---- Receive ------------------------------------------------------------------------------------

    /// Prepares Receive to put what comes into `folder`, unless something is coming already (`openReceive`).
    pub fn open_receive(&mut self, folder: String, cx: &mut Context<Self>) {
        if !self.receive.as_ref().is_some_and(Receiving::running) {
            self.into = folder;
            self.receive = None;
        }
        cx.notify();
    }

    /// Receives what `ticket` offers into the folder Receive was opened on (`receive`).
    pub fn receive(&mut self, ticket: String, cx: &mut Context<Self>) {
        let key = self.next_key();
        let folder = self.into.clone();
        self.receive = Some(Receiving {
            key,
            id: None,
            folder: folder.clone(),
            files: None,
            size: None,
            total: None,
            bytes: 0,
            done: None,
            failed: None,
        });
        cx.notify();
        let (channel, events) = core_bridge::stream::<ReceiveEvent>();
        let start = self.backend.receive_start(folder, ticket, channel);
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| this.on_receive_event(key, event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let started = core_bridge::run(start).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| this.receive_started(key, started, cx));
        })
        .detach();
    }

    fn on_receive_event(&mut self, key: u64, event: ReceiveEvent, cx: &mut Context<Self>) {
        let Some(receiving) = self.receive.as_mut().filter(|r| r.key == key) else {
            return;
        };
        let mut arrived = None;
        match event {
            ReceiveEvent::Getting { files, size, bytes } => {
                receiving.files = Some(files);
                receiving.size = Some(size);
                receiving.total = Some(bytes);
            }
            ReceiveEvent::Progress { bytes } => receiving.bytes = bytes,
            ReceiveEvent::Failed { message } => receiving.failed = Some(message),
            ReceiveEvent::Done { path } => {
                receiving.done = Some(path.clone());
                arrived = Some(path);
            }
        }
        cx.notify();
        if let Some(path) = arrived {
            let shown = self.dialog_view(cx) == Some(View::Receive);
            cx.emit(ShareEvent::Received { path, shown });
        }
    }

    fn receive_started(&mut self, key: u64, started: Result<u64, String>, cx: &mut Context<Self>) {
        match (self.receive.as_mut().filter(|r| r.key == key), started) {
            (Some(receiving), Ok(id)) => receiving.id = Some(id),
            (Some(receiving), Err(reason)) => receiving.failed = Some(reason),
            (None, Ok(id)) => self.backend.stop(id),
            (None, Err(_)) => {}
        }
        cx.notify();
    }

    /// Stops what is coming (what came so far goes), or clears what is done for the next (`stopReceive`).
    pub fn stop_receive(&mut self, cx: &mut Context<Self>) {
        if let Some(receiving) = self.receive.take() {
            if let (Some(id), true) = (receiving.id, receiving.running()) {
                self.backend.stop(id);
            }
        }
        cx.notify();
    }

    // ---- Share on the network -----------------------------------------------------------------------

    /// Prepares "Share on this network" to share `path`: asks for this PC's addresses (`openNetworkShare`).
    pub fn open_network_share(&mut self, path: String, cx: &mut Context<Self>) {
        self.offer = path;
        if self.addresses == Addresses::Loading {
            return;
        }
        self.addresses = Addresses::Loading;
        cx.notify();
        let list = self.backend.addresses();
        cx.spawn(async move |this, cx| {
            let list = core_bridge::run(list).await.and_then(|inner| inner).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.addresses = Addresses::Known(list);
                cx.notify();
            });
        })
        .detach();
    }

    /// Remembers the choices for the next share.
    pub fn set_choice(&mut self, choice: Choice, cx: &mut Context<Self>) {
        self.choice = choice;
        cx.notify();
    }

    /// Shares `path` on this network as `options` say; its link shows once dufs is listening (`serve`).
    pub fn serve(&mut self, path: String, options: LanOptions, cx: &mut Context<Self>) {
        self.offer.clear();
        cx.notify();
        if self
            .serving
            .iter()
            .any(|s| same_path(&s.path, &path) && s.ended.is_none())
        {
            return;
        }
        let key = self.next_key();
        self.serving.push(Serving {
            key,
            path: path.clone(),
            id: None,
            uploads: options.uploads,
            url: None,
            qr: None,
            address: None,
            reach: None,
            ends: None,
            visits: 0,
            from: Vec::new(),
            last: None,
            ended: None,
        });
        let (channel, events) = core_bridge::stream::<LanEvent>();
        let start = self.backend.serve_start(path, options, channel);
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this.update(cx, |this, cx| this.on_lan_event(key, event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let started = core_bridge::run(start).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| this.serve_started(key, started, cx));
        })
        .detach();
    }

    fn on_lan_event(&mut self, key: u64, event: LanEvent, cx: &mut Context<Self>) {
        let now = (self.effects.now)();
        let Some(serving) = self.serving.iter_mut().find(|s| s.key == key) else {
            return;
        };
        let mut ready = false;
        match event {
            LanEvent::Ready { url, qr, ends } => {
                serving.address = Some(host_of(&url));
                serving.url = Some(url);
                serving.qr = Qr::from_svg(&qr).map(Rc::new);
                serving.ends = ends;
                ready = true;
            }
            LanEvent::Visit { from, request, status } => {
                serving.visits += 1;
                if !serving.from.contains(&from) {
                    serving.from.push(from.clone());
                }
                serving.last = Some(Visit {
                    from,
                    request,
                    status,
                    at: now,
                });
            }
            LanEvent::Ended { message } => serving.ended = Some(message),
        }
        cx.notify();
        if ready {
            self.check_reach(key, cx);
        }
    }

    fn serve_started(&mut self, key: u64, started: Result<u64, String>, cx: &mut Context<Self>) {
        match (self.serving.iter_mut().find(|s| s.key == key), started) {
            (Some(serving), Ok(id)) => serving.id = Some(id),
            (Some(serving), Err(reason)) => serving.ended = Some(reason),
            (None, Ok(id)) => self.backend.stop(id),
            (None, Err(_)) => {}
        }
        cx.notify();
    }

    /// Reads what may keep other devices from a share, as Windows has it now (`checkReach`).
    fn check_reach(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(address) = self
            .serving
            .iter()
            .find(|s| s.key == key && s.ended.is_none())
            .and_then(|s| s.address.clone())
        else {
            return;
        };
        let reach = self.backend.reach(address);
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(reach)) = core_bridge::run(reach).await {
                let _ = this.update(cx, |this, cx| {
                    if let Some(serving) = this.serving.iter_mut().find(|s| s.key == key) {
                        serving.reach = Some(reach);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// When Glint is back in front: what may keep other devices from each share, read again (`recheckShares`).
    pub fn recheck(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<u64> = self.serving.iter().map(|s| s.key).collect();
        for key in keys {
            self.check_reach(key, cx);
        }
    }

    /// Stops sharing it on the network (its link stops working), or takes away one that ended (`stopServing`).
    pub fn stop_serving(&mut self, key: u64, cx: &mut Context<Self>) {
        if let Some(serving) = self.serving.iter().find(|s| s.key == key) {
            if let (Some(id), None) = (serving.id, &serving.ended) {
                self.backend.stop(id);
            }
        }
        self.serving.retain(|s| s.key != key);
        cx.notify();
    }
}

/// The host in a share's link: "192.168.1.5" of "http://192.168.1.5:23456/k3j9/".
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', ':']).next().unwrap_or_default().to_string()
}

/// A ticket as the sending PC shows it, or its whole "sendme receive …" line (`TICKET`): core checks it again.
pub fn is_ticket(text: &str) -> bool {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    let body = ["sendme receive", "sendme recv"]
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix))
        // The prefix needs whitespace after it; the ticket can't start with a space.
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .map_or(lower.as_str(), str::trim_start);
    body.strip_prefix("blob").is_some_and(|rest| {
        rest.len() >= 56
            && rest
                .bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
    })
}

/// "5 s ago", "3 min ago" (`ago`).
pub fn ago(ms: i64) -> SharedString {
    if ms < 60_000 {
        let seconds = ((ms as f64 / 1000.).round() as i64).max(1);
        t!("{n} s ago", n = seconds)
    } else {
        t!("{n} min ago", n = (ms as f64 / 60_000.).round() as i64)
    }
}

/// What the countdown says: "Shared till you stop it.", or when it ends and how long is left.
pub fn countdown(ends: Option<u64>, now: i64) -> SharedString {
    let Some(ends) = ends else {
        return t!("Shared till you stop it.");
    };
    let ends = i64::try_from(ends).unwrap_or(i64::MAX);
    let left = (ends - now).max(0);
    let left_text = if left < 60_000 {
        t!("under a minute")
    } else {
        SharedString::from(plural(
            usize::try_from((left + 59_999) / 60_000).unwrap_or(usize::MAX),
            "minute",
        ))
    };
    t!(
        "Stops {when}, in {left}.",
        when = format_date(ends, Some(now), i18n::language()),
        left = left_text
    )
}

// ---- What keeps other devices out --------------------------------------------------------------------------

/// The note about Windows Firewall and the network's kind, when they keep other devices from a share's link:
/// what is wrong, what to do, and whether the network is public (which adds its Settings button). `None`
/// when nothing does (`ReachNote`).
pub fn reach_note(reach: &Reach) -> Option<(SharedString, SharedString, bool)> {
    if reach.firewall != Firewall::Blocked && reach.firewall != Firewall::Unset {
        return None;
    }
    let network = reach.network.unwrap_or("private");
    let kind = match network {
        "public" => t!("public"),
        "domain" => t!("domain"),
        _ => t!("private"),
    };
    let is_public = reach.network == Some("public");
    let public_note = if is_public {
        t!(", and Windows has this network as public")
    } else {
        SharedString::default()
    };
    let blocked = reach.firewall == Firewall::Blocked;
    let said = if blocked {
        t!(
            "Other devices can't open the link: Windows Firewall blocks dufs on {kind} networks{where}.",
            kind = kind,
            where = public_note
        )
    } else {
        t!(
            "Other devices can't open the link yet: Windows Firewall hasn't been told dufs may take connections on {kind} networks{where}.",
            kind = kind,
            where = public_note
        )
    };
    let fix = match (blocked, is_public) {
        (true, true) => t!("If it's your home or work network, set it to private and share again. Or let dufs in on public networks, in the firewall's allowed apps."),
        (true, false) => t!("Let dufs in on {kind} networks, in the firewall's allowed apps.", kind = kind),
        (false, true) => t!("If Windows asks, allow it with Public networks ticked. Or, if it's your home or work network, set it to private and share again."),
        (false, false) => t!("If Windows asks, allow it. Or let dufs in, in the firewall's allowed apps."),
    };
    Some((said, fix, is_public))
}

// ---- The shared model ------------------------------------------------------------------------------------

/// The shared model, in a GPUI `Global`.
struct SharedSharing(Entity<Sharing>);

impl Global for SharedSharing {}

/// The model every dialog and the title bar's button share; made at the first use, on core.
pub fn shared(cx: &mut App) -> Entity<Sharing> {
    if let Some(existing) = cx.try_global::<SharedSharing>() {
        return existing.0.clone();
    }
    let backend: Arc<dyn Backend> = Arc::new(CoreBackend::new(cx));
    let model = cx.new(|_| Sharing::new(backend, Effects::app()));
    install(&model, cx);
    cx.set_global(SharedSharing(model.clone()));
    model
}

/// Installs `model` as the shared one (tests use a model with a fake backend).
#[cfg(test)]
pub fn set_shared(model: Entity<Sharing>, cx: &mut App) {
    install(&model, cx);
    cx.set_global(SharedSharing(model));
}

/// Tells the person when something arrived while the dialog was not showing Receive: a notice with Show.
fn install(model: &Entity<Sharing>, cx: &mut App) {
    cx.subscribe(model, |model, event: &ShareEvent, cx| {
        let ShareEvent::Received { path, shown: false } = event else {
            return;
        };
        let reveal = model.read(cx).effects.reveal.clone();
        let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) else {
            return;
        };
        let name = base_name(path).to_string();
        let path = path.clone();
        let _ = window.update(cx, move |_, window, cx| {
            let reveal = reveal.clone();
            let path = path.clone();
            window.push_notification(
                Notification::success(t!("Received “{name}”", name = isolate(name.clone())))
                    .autohide(false)
                    .action(move |_, _, _| {
                        let reveal = reveal.clone();
                        let path = path.clone();
                        Button::new("share-received-show")
                            .label(t!("Show"))
                            .on_click(move |_, window, cx| {
                                window.clear_notifications(cx);
                                reveal(&path, cx)
                            })
                    }),
                cx,
            );
        });
    })
    .detach();
}

// ---- The dialog -------------------------------------------------------------------------------------------

/// How long a share lasts, as the form offers it: (label, minutes).
fn lengths() -> [(SharedString, u32); 4] {
    [
        (t!("After 15 minutes"), 15),
        (t!("After an hour"), 60),
        (t!("After 4 hours"), 240),
        (t!("When I stop it"), 0),
    ]
}

/// Send, receive and share in one dialog with three tabs (`ShareModal`).
pub struct ShareDialog {
    model: Entity<Sharing>,
    view: View,
    tools: Option<Entity<Tools>>,
    /// The box the ticket is pasted into (Receive).
    ticket: Entity<TextareaState>,
    /// Where focus rests when the box it was in goes away (leaving Receive), so Escape still reaches the dialog.
    focus: FocusHandle,
    /// A read-only box per send, holding its ticket so it can be selected and copied.
    boxes: HashMap<u64, Entity<TextareaState>>,
    /// The network chosen in the form; the first when none.
    address: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ShareDialog {
    /// The dialog on `view`, over `model`. `tools` is the model the install cards read (`None`: no card, and
    /// the buttons that need sendme or dufs are enabled).
    pub fn new(
        model: Entity<Sharing>,
        view: View,
        tools: Option<Entity<Tools>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let ticket = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(4)
                .submit_on_enter(true)
                .placeholder("blob…")
        });
        let mut subscriptions = vec![cx.observe(&model, |_, _, cx| cx.notify())];
        subscriptions.push(
            cx.subscribe_in(&ticket, window, |this, _, event: &InputEvent, _, cx| match event {
                // Enter receives; a ticket has no lines.
                InputEvent::PressEnter { .. } => this.receive(cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            }),
        );
        // After Windows' prompt or Settings, what keeps other devices out of a share may have changed.
        let recheck_model = model.clone();
        subscriptions.push(cx.observe_window_activation(window, move |_, window, cx| {
            if window.is_window_active() {
                recheck_model.update(cx, |model, cx| model.recheck(cx));
            }
        }));
        // A countdown and "how long ago" tick once a second while a share has either.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                break;
            }
        })
        .detach();
        Self {
            model,
            view,
            tools,
            ticket,
            focus: cx.focus_handle(),
            boxes: HashMap::new(),
            address: None,
            _subscriptions: subscriptions,
        }
    }

    /// The view shown.
    pub fn view(&self) -> View {
        self.view
    }

    /// The box the ticket is pasted into.
    #[cfg(test)]
    pub fn ticket_input(&self) -> &Entity<TextareaState> {
        &self.ticket
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let ticking = self
            .model
            .read(cx)
            .serving
            .iter()
            .any(|s| s.ended.is_none() && (s.ends.is_some() || s.last.is_some()));
        if ticking && self.view == View::Network {
            cx.notify();
        }
    }

    /// Shows `view`; Receive first prepares its folder (`openReceive(into || currentPath())`).
    fn show_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        if view == View::Receive {
            let folder = self.receive_folder(cx);
            self.model.update(cx, |model, cx| model.open_receive(folder, cx));
            self.ticket.update(cx, |ticket, cx| ticket.focus(window, cx));
        } else if self.view == View::Receive {
            // The ticket box is about to go; focus it holds would leave Escape with nowhere to go.
            window.focus(&self.focus, cx);
        }
        self.view = view;
        cx.notify();
    }

    fn receive_folder(&self, cx: &App) -> String {
        let into = self.model.read(cx).into.clone();
        if !into.is_empty() {
            return into;
        }
        explorer::try_explorer(cx).map_or_else(String::new, |e| e.read(cx).current_path().to_string())
    }

    fn receive(&mut self, cx: &mut Context<Self>) {
        let text = self.ticket.read(cx).value().to_string();
        if self.view != View::Receive || !is_ticket(&text) || !self.tool_ready("sendme", cx) {
            return;
        }
        self.model
            .update(cx, |model, cx| model.receive(text.trim().to_string(), cx));
    }

    /// Whether `id` is there to use. Without a tools model nothing can be installed, so the answer is yes.
    fn tool_ready(&self, id: &str, cx: &App) -> bool {
        match &self.tools {
            None => true,
            Some(tools) => tools.read(cx).tool(id).is_some_and(|info| info.origin.is_some()),
        }
    }

    fn effects(&self, cx: &App) -> Effects {
        self.model.read(cx).effects.clone()
    }

    // ---- Pieces -------------------------------------------------------------------------------------

    fn tabs(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        align_row(0.5)
            .id("share-tabs")
            .role(Role::TabList)
            .aria_label(t!("Send, receive or share"))
            .test_support()
            .flex_none()
            .self_start()
            .p(px(2.))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .children(View::ALL.into_iter().map(|view| {
                let chosen = self.view == view;
                Button::new(("share-tab", view as usize))
                    .small()
                    .when(chosen, |b| b.primary())
                    .when(!chosen, |b| b.ghost())
                    .role(Role::Tab)
                    .toggled(chosen)
                    .accessibility_label(view.tab())
                    .label(view.tab())
                    .on_click(cx.listener(move |this, _, window, cx| this.show_view(view, window, cx)))
            }))
    }

    /// What is shown and why (the modal's header).
    fn heading(&self, t: &theme::Tokens) -> impl IntoElement {
        let view = self.view;
        align_row(3.)
            .id(("share-heading", view as usize))
            .role(Role::Group)
            .aria_label(view.title())
            .aria_description(view.subtitle())
            .test_support()
            .child(icon(view.icon()).size_5().text_color(t.primary).flex_none())
            .child(
                col()
                    .align_start()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(view.title()))
                    .child(
                        div()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(view.subtitle()),
                    ),
            )
    }

    /// A card's head: what it is, where, and its Stop (or Remove, once it has ended) (`Head`).
    fn card_head(path: &str, detail: Option<SharedString>, stop: Button, t: &theme::Tokens) -> impl IntoElement {
        let name = base_name(path);
        let name: SharedString = if name.is_empty() {
            path.to_string()
        } else {
            name.to_string()
        }
        .into();
        align_row(2.)
            .child(
                col()
                    .align_start()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .child(div().font_weight(FontWeight::MEDIUM).child(isolate(name)))
                            .when_some(detail, |this, detail| {
                                this.child(div().text_color(t.muted_foreground).child(format!(" · {detail}")))
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(isolate(path.to_string())),
                    ),
            )
            .child(stop)
    }

    fn stop_button(
        id: impl Into<ElementId>,
        ended: bool,
        label: SharedString,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Button {
        let (glyph, text) = if ended {
            (IconName::X, t!("Remove"))
        } else {
            (IconName::Square, label)
        };
        Button::new(id)
            .small()
            .outline()
            .icon(glyph)
            .label(text)
            .on_click(move |_, window, cx| on_click(window, cx))
    }

    fn ended_note(message: &str, t: &theme::Tokens) -> impl IntoElement {
        align_row(2.)
            .id("share-ended")
            .role(Role::Alert)
            .aria_label(SharedString::from(message.to_string()))
            .test_support()
            .items_start()
            .text_size(theme::size::XS)
            .text_color(t.destructive)
            .child(icon(IconName::TriangleAlert).size_3p5().mt(space(0.5)).flex_none())
            .child(div().flex_1().min_w_0().child(SharedString::from(message.to_string())))
    }

    fn waiting(id: &'static str, text: SharedString, t: &theme::Tokens) -> impl IntoElement {
        align_row(2.)
            .id(id)
            .role(Role::Status)
            .aria_label(text.clone())
            .test_support()
            .text_color(t.muted_foreground)
            .child(Spinner::new())
            .child(div().flex_1().min_w_0().child(text))
    }

    fn card(id: impl Into<ElementId>, name: SharedString) -> impl IntoElement + ParentElement + Styled {
        col()
            .id(id)
            .role(Role::Group)
            .aria_label(name)
            .test_support()
            .gap(space(2.5))
            .p(space(3.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
    }

    fn note(text: SharedString, t: &theme::Tokens) -> impl IntoElement {
        div()
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(text)
    }

    fn empty(id: &'static str, text: SharedString, t: &theme::Tokens) -> impl IntoElement {
        div()
            .id(id)
            .role(Role::Status)
            .aria_label(text.clone())
            .test_support()
            .py(space(6.))
            .text_center()
            .text_color(t.muted_foreground)
            .child(text)
    }

    fn copy(&self, text: &str, what: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        window.push_notification(Notification::success(what), cx);
    }

    // ---- Send ---------------------------------------------------------------------------------------

    fn send_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let sends = self.model.read(cx).sends.clone();
        self.boxes.retain(|key, _| sends.iter().any(|s| s.key == *key));
        if sends.is_empty() {
            return Self::empty(
                "share-send-empty",
                t!("Right-click a file or a folder and choose Send to another PC, or find it in Ctrl+K."),
                &t,
            )
            .into_any_element();
        }
        let mut cards = Vec::new();
        for send in sends {
            cards.push(self.send_card(&send, &t, window, cx).into_any_element());
        }
        col()
            .gap(space(4.))
            .children(cards)
            .child(Self::note(
                t!("On the other PC, choose Receive from another PC in Glint (in Tools, or Ctrl+K) and paste the ticket, or run sendme receive with it. Anyone with the ticket can download what it offers while it's offered, so give it only to whom it's for. It's offered till you stop it or close Glint; keep it as it is till then."),
                &t,
            ))
            .into_any_element()
    }

    fn send_card(
        &mut self,
        send: &Sending,
        t: &theme::Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = send.key;
        let model = self.model.clone();
        let stop = Self::stop_button(
            ("share-stop-send", key as usize),
            send.ended.is_some(),
            t!("Stop sending"),
            move |_, cx| model.update(cx, |model, cx| model.stop_send(key, cx)),
        );
        let name = base_name(&send.path);
        let body = if let Some(message) = &send.ended {
            Self::ended_note(message, t).into_any_element()
        } else if let Some(ticket) = &send.ticket {
            let boxed = self
                .boxes
                .entry(key)
                .or_insert_with(|| {
                    let state = cx.new(|cx| TextareaState::new(window, cx).rows(5));
                    state.update(cx, |state, cx| {
                        state.set_value(ticket.clone(), window, cx);
                        state.set_readonly(true, cx);
                    });
                    state
                })
                .clone();
            let copied = ticket.clone();
            row_start()
                .gap(space(3.))
                .items_start()
                .when_some(send.qr.clone(), |this, qr| {
                    this.child(qr_view(
                        ("share-qr-send", key as usize),
                        qr,
                        rems(8.),
                        t!("The ticket as a QR code"),
                    ))
                })
                .child(
                    col()
                        .align_start()
                        .flex_1()
                        .min_w_0()
                        .gap(space(2.))
                        .child(
                            Textarea::new(&boxed)
                                .aria_label(t!("Ticket"))
                                .readonly(true)
                                .font_family(theme::mono_font(cx)),
                        )
                        .child(
                            Button::new(("share-copy-ticket", key as usize))
                                .small()
                                .primary()
                                .icon(IconName::Copy)
                                .label(t!("Copy the ticket"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.copy(&copied, t!("Copied the ticket"), window, cx)
                                })),
                        ),
                )
                .into_any_element()
        } else {
            Self::waiting("share-reading", t!("Reading it through for its ticket…"), t).into_any_element()
        };
        Self::card(("share-send", key as usize), name.to_string().into())
            .border_color(t.border)
            .child(Self::card_head(
                &send.path,
                send.size.clone().map(SharedString::from),
                stop,
                t,
            ))
            .child(body)
    }

    // ---- Receive ------------------------------------------------------------------------------------

    fn receive_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let (receiving, into) = {
            let model = self.model.read(cx);
            (model.receive.clone(), model.into.clone())
        };
        if let Some(receiving) = receiving {
            return self.receive_progress(&receiving, &t, cx).into_any_element();
        }
        let text = self.ticket.read(cx).value().to_string();
        let filled = !text.trim().is_empty();
        let valid = is_ticket(&text);
        let ready = valid && self.tool_ready("sendme", cx);
        col()
            .id("share-receive-form")
            .role(Role::Form)
            .aria_label(t!("The ticket the sending PC shows"))
            .test_support()
            .gap(space(3.))
            .child(
                col()
                    .align_start()
                    .gap(space(1.5))
                    .child(
                        div()
                            .text_size(theme::size::XS)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.muted_foreground)
                            .child(t!("The ticket the sending PC shows")),
                    )
                    .child(
                        Textarea::new(&self.ticket)
                            .aria_label(t!("Ticket"))
                            .font_family(theme::mono_font(cx))
                            .when(filled && !valid, |this| this.border_color(t.destructive)),
                    ),
            )
            .when(filled && !valid, |this| {
                this.child(
                    div()
                        .id("share-ticket-invalid")
                        .role(Role::Alert)
                        .aria_label(t!(
                            "That isn't a whole ticket: it's the long line starting with “blob”."
                        ))
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.destructive)
                        .child(t!(
                            "That isn't a whole ticket: it's the long line starting with “blob”."
                        )),
                )
            })
            .child(
                align_row(1.5)
                    .id("share-into")
                    .role(Role::Group)
                    .aria_label(t!("Into {folder}", folder = into.clone()))
                    .test_support()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(icon(IconName::FolderOpen).size_3p5().flex_none())
                    .child(t!("Into"))
                    .child(div().truncate().min_w_0().text_color(t.foreground).child(isolate(into))),
            )
            .child(
                Button::new("share-receive")
                    .primary()
                    .icon(IconName::Download)
                    .label(t!("Receive"))
                    .disabled(!ready)
                    .on_click(cx.listener(|this, _, _, cx| this.receive(cx))),
            )
            .into_any_element()
    }

    fn receive_progress(&self, r: &Receiving, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.clone();
        if let Some(path) = &r.done {
            let (show_path, again_model) = (path.clone(), model);
            let reveal = self.effects(cx).reveal;
            return align_row(3.)
                .id("share-received")
                .role(Role::Status)
                .aria_label(t!("Received “{name}”", name = base_name(path)))
                .test_support()
                .items_start()
                .py(space(2.))
                .child(icon(IconName::CircleCheck).size_5().text_color(t.success).flex_none())
                .child(
                    col()
                        .align_start()
                        .flex_1()
                        .min_w_0()
                        .gap(space(2.))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(t!("Received “{name}”", name = isolate(base_name(path).to_string()))),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(isolate(path.clone())),
                        )
                        .child(
                            align_row(2.)
                                .child(
                                    Button::new("share-received-show")
                                        .small()
                                        .primary()
                                        .icon(IconName::FolderOpen)
                                        .label(t!("Show"))
                                        .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            reveal(&show_path, cx);
                                        }),
                                )
                                .child(
                                    Button::new("share-receive-another")
                                        .small()
                                        .outline()
                                        .label(t!("Receive another"))
                                        .on_click(move |_, _, cx| {
                                            again_model.update(cx, |model, cx| model.stop_receive(cx))
                                        }),
                                ),
                        ),
                )
                .into_any_element();
        }
        if let Some(failed) = &r.failed {
            return align_row(3.)
                .id("share-receive-failed")
                .role(Role::Alert)
                .aria_label(t!("Couldn't receive it"))
                .aria_description(SharedString::from(failed.clone()))
                .test_support()
                .items_start()
                .py(space(2.))
                .child(
                    icon(IconName::TriangleAlert)
                        .size_5()
                        .text_color(t.destructive)
                        .flex_none(),
                )
                .child(
                    col()
                        .align_start()
                        .flex_1()
                        .min_w_0()
                        .gap(space(2.))
                        .child(div().font_weight(FontWeight::MEDIUM).child(t!("Couldn't receive it")))
                        .child(
                            div()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(SharedString::from(failed.clone())),
                        )
                        .child(
                            Button::new("share-receive-retry")
                                .small()
                                .outline()
                                .label(t!("Try again"))
                                .on_click(move |_, _, cx| model.update(cx, |model, cx| model.stop_receive(cx))),
                        ),
                )
                .into_any_element();
        }
        let status: SharedString = match (r.files, &r.size) {
            (Some(files), Some(size)) => t!(
                "Receiving {files}, {size}",
                files = plural(usize::try_from(files).unwrap_or(usize::MAX), "file"),
                size = size
            ),
            _ => t!("Reaching the other PC…"),
        };
        let percent = r.total.filter(|total| *total > 0).map_or(0, |total| {
            ((100. * r.bytes as f64 / total as f64).round() as u32).min(100)
        });
        let detail = r.total.map(|total| {
            t!(
                "{done} of {size}, into {folder}",
                done = format_size(r.bytes.min(total)),
                size = r.size.clone().unwrap_or_else(|| format_size(total)),
                folder = r.folder
            )
        });
        col()
            .id("share-receiving")
            .role(Role::ProgressIndicator)
            .aria_label(t!("{status}, {percent}%", status = status.clone(), percent = percent))
            .test_support()
            .gap(space(2.))
            .py(space(4.))
            .child(
                align_row(2.)
                    .text_color(t.muted_foreground)
                    .child(Spinner::new())
                    .child(div().flex_1().min_w_0().truncate().child(status))
                    .child(
                        Button::new("share-receive-stop")
                            .small()
                            .outline()
                            .icon(IconName::Square)
                            .label(t!("Stop"))
                            .on_click(move |_, _, cx| model.update(cx, |model, cx| model.stop_receive(cx))),
                    ),
            )
            .when(r.total.is_some(), |this| {
                this.child(
                    row_start()
                        .h(space(1.5))
                        .w_full()
                        .overflow_hidden()
                        .rounded_full()
                        .bg(t.muted)
                        .child(
                            div()
                                .h_full()
                                .w(gpui_kit::relative(percent as f32 / 100.))
                                .rounded_full()
                                .bg(t.primary),
                        ),
                )
            })
            .when_some(detail, |this, detail| {
                this.child(
                    div()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(isolate(detail)),
                )
            })
            .into_any_element()
    }

    // ---- On this network ----------------------------------------------------------------------------

    fn network_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let (offer, serving) = {
            let model = self.model.read(cx);
            (model.offer.clone(), model.serving.clone())
        };
        let asked = !offer.is_empty() && !serving.iter().any(|s| same_path(&s.path, &offer) && s.ended.is_none());
        let empty = !asked && serving.is_empty();
        let now = (self.effects(cx).now)();
        let mut cards = Vec::new();
        for s in &serving {
            cards.push(self.serving_card(s, now, &t, cx).into_any_element());
        }
        col()
            .gap(space(4.))
            .when(asked, |this| this.child(self.serve_form(&offer, &t, cx)))
            .children(cards)
            .when(empty, |this| {
                this.child(Self::empty(
                    "share-network-empty",
                    t!("Right-click a folder or a file and choose Share on this network, or find it in Ctrl+K."),
                    &t,
                ))
            })
            .child(Self::note(
                t!("Anyone on this network who has the link can see and download all that's in it, and it isn't encrypted on its way, so share on networks you trust. The first time, Windows asks whether dufs may accept connections: allow it, or other devices can't open the link. A share says so when Windows keeps them out."),
                &t,
            ))
            .into_any_element()
    }

    fn serve_form(&self, path: &str, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let (addresses, choice) = {
            let model = self.model.read(cx);
            (model.addresses.clone(), model.choice)
        };
        let list: Vec<Address> = match &addresses {
            Addresses::Known(list) => list.clone(),
            _ => Vec::new(),
        };
        let known = matches!(addresses, Addresses::Known(_));
        let chosen = self
            .address
            .clone()
            .filter(|a| list.iter().any(|x| &x.address == a))
            .or_else(|| list.first().map(|a| a.address.clone()));
        let can_share = known && !list.is_empty() && self.tool_ready("dufs", cx);
        let path_owned = path.to_string();
        let name = base_name(path);
        let name: SharedString = if name.is_empty() {
            path.to_string()
        } else {
            name.to_string()
        }
        .into();
        let model = self.model.clone();
        let label = |text: SharedString| {
            div()
                .w(space(20.))
                .flex_none()
                .text_size(theme::size::XS)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted_foreground)
                .child(text)
        };
        Self::card("share-serve-form", name.to_string().into())
            .border_color(t.border)
            .gap(space(3.))
            .child(
                col()
                    .align_start()
                    .min_w_0()
                    .child(div().truncate().font_weight(FontWeight::MEDIUM).child(isolate(name)))
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(isolate(path.to_string())),
                    ),
            )
            .when(known && list.is_empty(), |this| {
                this.child(
                    div()
                        .id("share-no-network")
                        .role(Role::Alert)
                        .aria_label(t!("This PC isn't on a network."))
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.destructive)
                        .child(t!("This PC isn't on a network.")),
                )
            })
            .when(!known, |this| {
                this.child(Self::waiting("share-looking", t!("Looking on this network…"), t))
            })
            .when(list.len() > 1, |this| {
                this.child(
                    align_row(3.).items_start().child(label(t!("Network"))).child(
                        col()
                            .id("share-network-choice")
                            .role(Role::RadioGroup)
                            .aria_label(t!("Network"))
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .gap(space(1.))
                            .children(list.iter().enumerate().map(|(ix, a)| {
                                let picked = chosen.as_deref() == Some(a.address.as_str());
                                let text: SharedString = format!("{} · {}", a.name, a.address).into();
                                let address = a.address.clone();
                                Button::new(("share-network", ix))
                                    .small()
                                    .when(picked, |b| b.primary())
                                    .when(!picked, |b| b.outline())
                                    .role(Role::RadioButton)
                                    .toggled(picked)
                                    .accessibility_label(text.clone())
                                    .label(text)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.address = Some(address.clone());
                                        cx.notify();
                                    }))
                            })),
                    ),
                )
            })
            .child(
                align_row(3.).items_start().child(label(t!("Stop it"))).child(
                    row_start()
                        .id("share-length")
                        .role(Role::RadioGroup)
                        .aria_label(t!("Stop it"))
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .flex_wrap()
                        .gap(space(1.))
                        .children(lengths().into_iter().map(|(text, minutes)| {
                            let picked = choice.minutes == minutes;
                            let model = model.clone();
                            Button::new(("share-length", minutes as usize))
                                .small()
                                .when(picked, |b| b.primary())
                                .when(!picked, |b| b.outline())
                                .role(Role::RadioButton)
                                .toggled(picked)
                                .accessibility_label(text.clone())
                                .label(text)
                                .on_click(move |_, _, cx| {
                                    model.update(cx, |model, cx| {
                                        let choice = Choice {
                                            minutes,
                                            ..model.choice
                                        };
                                        model.set_choice(choice, cx)
                                    })
                                })
                        })),
                ),
            )
            .child(
                col()
                    .align_start()
                    .gap(space(0.5))
                    .child(
                        Checkbox::new("share-uploads")
                            .label(t!("Let them add files to it"))
                            .checked(choice.uploads)
                            .on_click({
                                let model = model.clone();
                                move |checked: &bool, _, cx| {
                                    let checked = *checked;
                                    model.update(cx, |model, cx| {
                                        let choice = Choice {
                                            uploads: checked,
                                            ..model.choice
                                        };
                                        model.set_choice(choice, cx)
                                    })
                                }
                            }),
                    )
                    .child(
                        div()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(t!(
                            "Whoever has the link can then save files into it, but not replace or delete what's there."
                        )),
                    ),
            )
            .child(
                Button::new("share-serve")
                    .primary()
                    .icon(IconName::Wifi)
                    .label(t!("Share it"))
                    .disabled(!can_share)
                    .on_click(move |_, _, cx| {
                        let options = LanOptions {
                            uploads: choice.uploads,
                            minutes: choice.minutes,
                            address: chosen.clone(),
                        };
                        let path = path_owned.clone();
                        model.update(cx, |model, cx| model.serve(path, options, cx));
                    }),
            )
    }

    fn serving_card(&self, s: &Serving, now: i64, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let key = s.key;
        let model = self.model.clone();
        let stop = Self::stop_button(
            ("share-stop-serve", key as usize),
            s.ended.is_some(),
            t!("Stop sharing"),
            move |_, cx| model.update(cx, |model, cx| model.stop_serving(key, cx)),
        );
        let detail = if s.uploads {
            t!("they can add files")
        } else {
            t!("read-only")
        };
        let name = base_name(&s.path);
        let body = if let Some(message) = &s.ended {
            Self::ended_note(message, t).into_any_element()
        } else if let Some(url) = &s.url {
            let (link, opened) = (url.clone(), url.clone());
            let open_url = self.effects(cx).open_url;
            let last = s.last.as_ref().map(|last| {
                let mut line = t!(
                    "{requests} from {devices}; the last {ago}: {request}",
                    requests = plural(s.visits, "request"),
                    devices = plural(s.from.len(), "device"),
                    ago = ago(now - last.at),
                    request = last.request
                )
                .to_string();
                if last.status >= 400 {
                    line.push_str(&format!(" ({})", last.status));
                }
                SharedString::from(line)
            });
            col()
                .gap(space(2.5))
                .when_some(s.reach.as_ref().and_then(reach_note), |this, (said, fix, is_public)| {
                    this.child(self.reach_note(s, said, fix, is_public, t, cx))
                })
                .child(
                    row_start()
                        .gap(space(3.))
                        .items_start()
                        .when_some(s.qr.clone(), |this, qr| {
                            this.child(qr_view(
                                ("share-qr-serve", key as usize),
                                qr,
                                rems(9.),
                                t!("The link as a QR code"),
                            ))
                        })
                        .child(
                            col()
                                .align_start()
                                .flex_1()
                                .min_w_0()
                                .gap(space(2.))
                                .child(
                                    div()
                                        .id(("share-link", key as usize))
                                        .role(Role::Link)
                                        .aria_label(t!("Link"))
                                        .aria_description(SharedString::from(url.clone()))
                                        .test_support()
                                        .font_family(theme::mono_font(cx))
                                        .text_size(theme::size::XS)
                                        .child(SharedString::from(url.clone())),
                                )
                                .child(
                                    row_start()
                                        .flex_wrap()
                                        .gap(space(2.))
                                        .child(
                                            Button::new(("share-copy-link", key as usize))
                                                .small()
                                                .primary()
                                                .icon(IconName::Copy)
                                                .label(t!("Copy the link"))
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.copy(&link, t!("Copied the link"), window, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new(("share-open-link", key as usize))
                                                .small()
                                                .outline()
                                                .icon(IconName::ExternalLink)
                                                .label(t!("Open"))
                                                .tooltip(t!("Open it in your browser here, as others see it"))
                                                .on_click(move |_, _, cx| open_url(&opened, cx)),
                                        ),
                                )
                                .child(
                                    div()
                                        .id(("share-countdown", key as usize))
                                        .role(Role::Status)
                                        .aria_label(countdown(s.ends, now))
                                        .test_support()
                                        .text_size(theme::size::XS)
                                        .text_color(t.muted_foreground)
                                        .child(countdown(s.ends, now)),
                                )
                                .child(
                                    div()
                                        .text_size(theme::size::XS)
                                        .text_color(t.muted_foreground)
                                        .child(last.unwrap_or_else(|| t!("Nobody's opened it yet."))),
                                ),
                        ),
                )
                .into_any_element()
        } else {
            Self::waiting("share-starting", t!("Starting…"), t).into_any_element()
        };
        Self::card(("share-serve", key as usize), name.to_string().into())
            .border_color(t.border)
            .child(Self::card_head(&s.path, Some(detail), stop, t))
            .child(body)
    }

    /// What keeps other devices from a share's link, with the buttons that open Windows' pages for it.
    fn reach_note(
        &self,
        s: &Serving,
        said: SharedString,
        fix: SharedString,
        is_public: bool,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = s.key as usize;
        let address = s.address.clone().unwrap_or_default();
        let open = |page: &'static str, address: String| {
            cx.listener(move |this, _, window, cx| this.open_settings(page, address.clone(), window, cx))
        };
        col()
            .id(("share-reach", key))
            .role(Role::Status)
            .aria_label(said.clone())
            .aria_description(fix.clone())
            .test_support()
            .gap(space(2.))
            .p(space(3.))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.warning.opacity(0.4))
            .bg(t.warning.opacity(0.1))
            .text_size(theme::size::XS)
            .child(
                align_row(2.)
                    .items_start()
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        icon(IconName::ShieldAlert)
                            .size_3p5()
                            .mt(space(0.5))
                            .flex_none()
                            .text_color(t.warning),
                    )
                    .child(div().flex_1().min_w_0().child(said)),
            )
            .child(div().text_color(t.muted_foreground).child(fix))
            .child(
                row_start()
                    .flex_wrap()
                    .gap(space(2.))
                    .when(is_public, |this| {
                        this.child(
                            Button::new(("share-network-settings", key))
                                .small()
                                .outline()
                                .icon(IconName::Network)
                                .label(t!("Network settings"))
                                .tooltip(t!("Settings for this network, where it's set as public or private"))
                                .on_click(open("network", address.clone())),
                        )
                    })
                    .child(
                        Button::new(("share-allowed-apps", key))
                            .small()
                            .outline()
                            .icon(IconName::Shield)
                            .label(t!("Allowed apps"))
                            .tooltip(t!(
                                "Windows Firewall's allowed apps (changing them asks for administrator approval)"
                            ))
                            .on_click(open("firewall", address)),
                    ),
            )
    }

    /// Opens a page of Windows' settings for a share; says so if Windows can't.
    fn open_settings(&mut self, page: &'static str, address: String, window: &mut Window, cx: &mut Context<Self>) {
        let work = self.model.read(cx).backend.open_settings(page, address);
        window
            .spawn(cx, async move |cx| {
                if let Err(reason) = core_bridge::run(work).await.and_then(|inner| inner) {
                    let _ = cx.update(|window, cx| window.push_notification(Notification::error(reason), cx));
                }
            })
            .detach();
    }
}

impl Render for ShareDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let (tool, why, glyph) = match self.view {
            View::Network => (
                "dufs",
                t!("dufs serves a folder to browsers on this network: a phone opens its link, or scans its QR code, to see and download what's in it."),
                IconName::Wifi,
            ),
            _ => (
                "sendme",
                t!("sendme sends files and folders straight to another PC, encrypted end to end, with no account and no size limit. The other PC needs it too."),
                IconName::Send,
            ),
        };
        let card = self
            .tools
            .as_ref()
            .and_then(|tools| tools::needed(tools, tool, why, glyph, cx));
        let content = match self.view {
            View::Send => self.send_view(window, cx),
            View::Receive => self.receive_view(cx),
            View::Network => self.network_view(cx),
        };
        let tabs = self.tabs(&t, cx);
        let heading = self.heading(&t);
        // The dialog layer names its own surface poorly, so the content has the name too.
        col()
            .id("share")
            .role(Role::Group)
            .aria_label(t!("Send, receive or share"))
            .test_support()
            .track_focus(&self.focus)
            .gap(space(4.))
            .text_size(theme::size::SM)
            .text_start()
            .pt(space(1.))
            .child(
                row_start()
                    .flex_wrap()
                    .gap(space(3.))
                    .justify_between()
                    .child(heading)
                    .child(tabs),
            )
            .child(
                col()
                    .id(("share-body", self.view as usize))
                    .gap(space(4.))
                    .max_h(rems(32.))
                    .overflow_y_scrollbar()
                    .children(card)
                    .child(content),
            )
    }
}

// ---- Opening it -----------------------------------------------------------------------------------------

/// The tools model the install cards read (none without core).
fn tools_model(window: &mut Window, cx: &mut App) -> Option<Entity<Tools>> {
    let core = cx.try_global::<core_bridge::Core>()?;
    let backend: Arc<dyn SettingsBackend> = Arc::new(SettingsCoreBackend::new(core.app.clone()));
    Some(cx.new(|cx| Tools::new(backend, window, cx)))
}

/// Shows the dialog on `view`, over the shared model.
pub fn open_view(window: &mut Window, cx: &mut App, view: View) -> Entity<ShareDialog> {
    let model = shared(cx);
    let tools = tools_model(window, cx);
    open_with(window, cx, model, view, tools)
}

/// [`open_view`] with a model and tools of your own (tests pass fakes). Returns the view.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    model: Entity<Sharing>,
    view: View,
    tools: Option<Entity<Tools>>,
) -> Entity<ShareDialog> {
    let dialog = cx.new(|cx| ShareDialog::new(model.clone(), view, tools, window, cx));
    model.update(cx, |model, _| model.dialog = Some(dialog.downgrade()));
    super::show(window, cx, t!("Send, receive or share"), px(576.), dialog.clone());
    if view == View::Receive {
        // Opened from the title bar's button rather than a menu: Receive still needs a folder.
        let folder = dialog.read(cx).receive_folder(cx);
        model.update(cx, |model, cx| {
            if model.into.is_empty() {
                model.open_receive(folder, cx);
            }
        });
        let ticket = dialog.read(cx).ticket.clone();
        ticket.update(cx, |ticket, cx| ticket.focus(window, cx));
    }
    dialog
}

/// Opens "On this network", offering `paths[0]` to be shared (dufs serves one thing; the menus pass one).
/// With no paths it shows what is shared already.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    let model = shared(cx);
    let tools = tools_model(window, cx);
    open_network(window, cx, model, tools, paths.into_iter().next());
}

/// [`open`] for a model of your own.
pub fn open_network(
    window: &mut Window,
    cx: &mut App,
    model: Entity<Sharing>,
    tools: Option<Entity<Tools>>,
    path: Option<String>,
) {
    if let Some(path) = path {
        model.update(cx, |model, cx| model.open_network_share(path, cx));
    }
    open_with(window, cx, model, View::Network, tools);
}

/// "Receive from another PC here…": the dialog on Receive, into `folder` unless something is coming already
/// (`openReceive`).
pub fn open_receive(window: &mut Window, cx: &mut App, folder: String) {
    let model = shared(cx);
    model.update(cx, |model, cx| model.open_receive(folder, cx));
    let tools = tools_model(window, cx);
    open_with(window, cx, model, View::Receive, tools);
}

/// Sends `path` to another PC now: the dialog opens on Send, its ticket showing once sendme has read it
/// (`sendToPC`).
pub fn send_to_pc(window: &mut Window, cx: &mut App, path: String) {
    let model = shared(cx);
    let tools = tools_model(window, cx);
    send_to_pc_with(window, cx, model, tools, path);
}

/// [`send_to_pc`] for a model and tools of your own.
pub fn send_to_pc_with(
    window: &mut Window,
    cx: &mut App,
    model: Entity<Sharing>,
    tools: Option<Entity<Tools>>,
    path: String,
) {
    model.update(cx, |model, cx| model.send_to_pc(path, cx));
    open_with(window, cx, model, View::Send, tools);
}

/// "Send to another PC…": looks for passwords and keys in `path` first (when Kingfisher is there), asking
/// before it goes on if it finds any, then sends it (`offerToPC`).
pub fn offer_to_pc(window: &mut Window, cx: &mut App, path: String) {
    let paths = vec![path.clone()];
    super::check::secrets_check_first(window, cx, paths, t!("Send anyway"), move |window, cx| {
        send_to_pc(window, cx, path.clone())
    });
}

/// "Share on this network…": the same look first, then the dialog with the choices (`offerOnNetwork`).
pub fn offer_on_network(window: &mut Window, cx: &mut App, path: String) {
    let paths = vec![path.clone()];
    super::check::secrets_check_first(window, cx, paths, t!("Share anyway"), move |window, cx| {
        open(window, cx, vec![path.clone()])
    });
}

/// "Share with Windows…": Windows' Share flyout for `paths` (`windowsShare`). Says so if Windows can't.
pub fn share_with_windows(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    let backend: Arc<dyn Backend> = Arc::new(CoreBackend::new(cx));
    share_with_windows_using(window, cx, backend, paths);
}

/// [`share_with_windows`] with a backend of your own.
pub fn share_with_windows_using(window: &mut Window, cx: &mut App, backend: Arc<dyn Backend>, paths: Vec<String>) {
    let work = backend.windows_share(paths);
    window
        .spawn(cx, async move |cx| {
            if let Err(reason) = core_bridge::run(work).await.and_then(|inner| inner) {
                let _ = cx.update(|window, cx| {
                    window.push_notification(
                        Notification::error(reason).title(t!("Couldn't open Windows' Share")),
                        cx,
                    )
                });
            }
        })
        .detach();
}

// ---- Tests ------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Mutex;

    const NOW: i64 = 1_800_000_000_000;

    fn fixed_now() -> i64 {
        NOW
    }

    /// Records every call and keeps the channels, so a test can send events as sendme and dufs would.
    struct Fake {
        next_id: AtomicU64,
        sends: Mutex<Vec<(String, Channel<SendEvent>)>>,
        receives: Mutex<Vec<(String, String, Channel<ReceiveEvent>)>>,
        serves: Mutex<Vec<(String, LanOptions, Channel<LanEvent>)>>,
        stopped: Mutex<Vec<u64>>,
        settings: Mutex<Vec<(String, String)>>,
        shared: Mutex<Vec<Vec<String>>>,
        addresses: Vec<Address>,
        reach: Mutex<Reach>,
        windows_fails: AtomicBool,
    }

    impl Fake {
        fn new(addresses: Vec<Address>) -> Arc<Fake> {
            Arc::new(Fake {
                next_id: AtomicU64::new(100),
                sends: Mutex::new(Vec::new()),
                receives: Mutex::new(Vec::new()),
                serves: Mutex::new(Vec::new()),
                stopped: Mutex::new(Vec::new()),
                settings: Mutex::new(Vec::new()),
                shared: Mutex::new(Vec::new()),
                addresses,
                reach: Mutex::new(Reach {
                    network: Some("private"),
                    firewall: Firewall::Allowed,
                }),
                windows_fails: AtomicBool::new(false),
            })
        }

        fn id(&self) -> u64 {
            self.next_id.fetch_add(1, Ordering::SeqCst)
        }
    }

    impl Backend for Fake {
        fn send_start(&self, path: String, events: Channel<SendEvent>) -> Fut<u64> {
            self.sends.lock().unwrap().push((path, events));
            let id = self.id();
            Box::pin(async move { Ok(id) })
        }
        fn receive_start(&self, folder: String, ticket: String, events: Channel<ReceiveEvent>) -> Fut<u64> {
            self.receives.lock().unwrap().push((folder, ticket, events));
            let id = self.id();
            Box::pin(async move { Ok(id) })
        }
        fn serve_start(&self, path: String, options: LanOptions, events: Channel<LanEvent>) -> Fut<u64> {
            self.serves.lock().unwrap().push((path, options, events));
            let id = self.id();
            Box::pin(async move { Ok(id) })
        }
        fn stop(&self, id: u64) {
            self.stopped.lock().unwrap().push(id);
        }
        fn addresses(&self) -> Fut<Vec<Address>> {
            let list = self.addresses.clone();
            Box::pin(async move { Ok(list) })
        }
        fn reach(&self, _: String) -> Fut<Reach> {
            let reach = self.reach.lock().unwrap().clone();
            Box::pin(async move { Ok(reach) })
        }
        fn open_settings(&self, page: &'static str, address: String) -> Fut<()> {
            self.settings.lock().unwrap().push((page.to_string(), address));
            Box::pin(async { Ok(()) })
        }
        fn windows_share(&self, paths: Vec<String>) -> Fut<()> {
            self.shared.lock().unwrap().push(paths);
            let fails = self.windows_fails.load(Ordering::SeqCst);
            Box::pin(async move {
                if fails {
                    Err("Windows said no".to_string())
                } else {
                    Ok(())
                }
            })
        }
    }

    fn wifi() -> Address {
        Address {
            address: "192.168.1.5".into(),
            name: "Wi-Fi".into(),
        }
    }

    fn ethernet() -> Address {
        Address {
            address: "10.0.0.2".into(),
            name: "Ethernet".into(),
        }
    }

    const QR: &str = r##"<?xml version="1.0" standalone="yes"?><svg xmlns="http://www.w3.org/2000/svg" version="1.1" width="232" height="232" viewBox="0 0 232 232" shape-rendering="crispEdges"><rect x="0" y="0" width="232" height="232" fill="#ffffff"/><path fill="#000000" d="M32 32h8v8H32V32M40 32h8v8H40V32M32 40h8v8H32V40"/></svg>"##;

    /// A valid ticket: "blob" and 65 base32 characters.
    fn ticket() -> String {
        format!("blob{}", "abcdefg234567".repeat(5))
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
        model: Entity<Sharing>,
        revealed: Rc<RefCell<Vec<String>>>,
        opened: Rc<RefCell<Vec<String>>>,
    }

    fn env(cx: &mut TestAppContext, addresses: Vec<Address>) -> Env {
        cx.update(testing::init);
        let fake = Fake::new(addresses);
        let backend: Arc<dyn Backend> = fake.clone();
        let revealed = Rc::new(RefCell::new(Vec::new()));
        let opened = Rc::new(RefCell::new(Vec::new()));
        let (r, o) = (revealed.clone(), opened.clone());
        let effects = Effects {
            reveal: Rc::new(move |path, _| r.borrow_mut().push(path.to_string())),
            open_url: Rc::new(move |url, _| o.borrow_mut().push(url.to_string())),
            now: fixed_now,
        };
        let (mut visual, _) = testing::open_window(cx, |_, cx| cx.new(|_| Host));
        let model = visual.update(|_, cx| cx.new(|_| Sharing::new(backend, effects)));
        Env {
            visual,
            fake,
            model,
            revealed,
            opened,
        }
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the share dialog never reached the state the test waits for");
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

    /// Whether the button is disabled (an enabled one doesn't say).
    fn disabled(cx: &mut VisualTestContext, id: &'static str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(id).disabled() == Some(true)
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

    fn open(env: &mut Env, view: View) -> Entity<ShareDialog> {
        let model = env.model.clone();
        let dialog = env.visual.update(|window, cx| open_with(window, cx, model, view, None));
        env.visual.run_until_parked();
        dialog
    }

    fn send_events(env: &Env, ix: usize) -> Channel<SendEvent> {
        env.fake.sends.lock().unwrap()[ix].1.clone()
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for icon in [
            IconName::Wifi,
            IconName::Send,
            IconName::Download,
            IconName::Copy,
            IconName::ExternalLink,
            IconName::FolderOpen,
            IconName::CircleCheck,
            IconName::TriangleAlert,
            IconName::Network,
            IconName::Shield,
            IconName::ShieldAlert,
            IconName::Square,
            IconName::X,
        ] {
            let path = icon.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn a_ticket_is_the_long_line_starting_with_blob_with_or_without_sendmes_words() {
        let t = ticket();
        assert!(is_ticket(&t));
        assert!(is_ticket(&format!("  {t}\n")));
        assert!(is_ticket(&format!("sendme receive {t}")));
        assert!(is_ticket(&format!("SENDME RECV   {t}")));
        assert!(is_ticket(&t.to_uppercase()));
        assert!(!is_ticket(""));
        assert!(!is_ticket("blob"));
        assert!(!is_ticket(&t[..30]));
        // Base32 has no 0, 1, 8 or 9, and a ticket has no spaces inside.
        assert!(!is_ticket(&format!("{t}1")));
        assert!(!is_ticket(&format!("blob {}", &t[4..])));
        assert!(!is_ticket(&format!("sendme receive{t}")));
        assert!(!is_ticket(&format!("sendme send {t}")));
    }

    #[test]
    fn a_links_host_is_what_the_reach_check_asks_about() {
        assert_eq!(host_of("http://192.168.1.5:23456/k3j9/"), "192.168.1.5");
        assert_eq!(host_of("http://10.0.0.2/x"), "10.0.0.2");
        assert_eq!(host_of("192.168.1.5:80"), "192.168.1.5");
    }

    #[test]
    fn cores_qr_svg_is_read_into_rectangles() {
        let qr = Qr::from_svg(QR).expect("the svg has a path");
        assert_eq!(qr.side, 232.);
        assert_eq!(
            qr.rects,
            vec![[32., 32., 8., 8.], [40., 32., 8., 8.], [32., 40., 8., 8.]]
        );
        assert_eq!(Qr::from_svg(""), None);
        assert_eq!(Qr::from_svg("<svg viewBox=\"0 0 10 10\"><path d=\"\"/></svg>"), None);
        assert_eq!(
            Qr::from_svg("<svg viewBox=\"0 0 0 0\"><path d=\"M0 0h1v1H0V0\"/></svg>"),
            None
        );
        // A damaged path draws nothing rather than something wrong.
        assert_eq!(
            Qr::from_svg("<svg viewBox=\"0 0 10 10\"><path d=\"M0 0hxv1H0V0\"/></svg>"),
            None
        );
    }

    #[gpui_kit::test]
    fn only_a_blocking_firewall_gets_a_note(_cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let note = |network, firewall| reach_note(&Reach { network, firewall });
        assert!(note(Some("private"), Firewall::Allowed).is_none());
        assert!(note(Some("private"), Firewall::Off).is_none());
        assert!(note(None, Firewall::Unknown).is_none());
        let (said, fix, is_public) = note(Some("public"), Firewall::Blocked).expect("blocked on public");
        assert_eq!(
            said,
            "Other devices can't open the link: Windows Firewall blocks dufs on public networks, and Windows has this network as public."
        );
        assert!(fix.starts_with("If it's your home or work network, set it to private"));
        assert!(is_public);
        let (said, fix, is_public) = note(Some("private"), Firewall::Unset).expect("unset on private");
        assert!(said.starts_with("Other devices can't open the link yet: Windows Firewall hasn't been told dufs may take connections on private networks."));
        assert_eq!(
            fix,
            "If Windows asks, allow it. Or let dufs in, in the firewall's allowed apps."
        );
        assert!(!is_public);
        // A network Windows can't name is taken for a private one, as the TypeScript did.
        let (said, _, _) = note(None, Firewall::Blocked).expect("blocked");
        assert!(said.contains("on private networks"));
    }

    #[gpui_kit::test]
    fn how_long_ago_and_how_long_left_are_worded_for_a_person(_cx: &mut TestAppContext) {
        let _guard = testing::lock();
        assert_eq!(ago(400), "1 s ago");
        assert_eq!(ago(42_000), "42 s ago");
        assert_eq!(ago(180_000), "3 min ago");
        assert_eq!(countdown(None, NOW), "Shared till you stop it.");
        let ten_minutes = u64::try_from(NOW + 600_000).unwrap();
        assert!(countdown(Some(ten_minutes), NOW).ends_with(", in 10 minutes."));
        assert!(countdown(Some(ten_minutes), NOW + 570_001).ends_with(", in under a minute."));
        assert!(countdown(Some(ten_minutes), NOW + 3_600_000).ends_with(", in under a minute."));
        assert!(countdown(Some(ten_minutes), NOW + 539_999).ends_with(", in 2 minutes."));
    }

    #[gpui_kit::test]
    fn sending_shows_the_ticket_and_its_qr_and_stop_ends_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, vec![wifi()]);
        let dialog = open(&mut env, View::Send);
        env.visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("share").role(), Some(Role::Group));
            assert_eq!(window.find("share-tabs").role(), Some(Role::TabList));
        });
        assert!(has(
            &mut env.visual,
            "Right-click a file or a folder and choose Send to another PC, or find it in Ctrl+K."
        ));
        assert_eq!(env.model.read_with(&env.visual, |m, _| m.button()), None);

        let model = env.model.clone();
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.send_to_pc(r"C:\ملفات\تقرير.pdf".into(), cx)));
        settle(&mut env.visual, |cx| {
            model.read_with(cx, |m, _| m.sends().first().is_some_and(|s| s.id.is_some()))
        });
        assert!(has(&mut env.visual, "Reading it through for its ticket…"));
        assert_eq!(role_of(&mut env.visual, "تقرير.pdf"), Some(Role::Group));
        let (label, view) = env
            .model
            .read_with(&env.visual, |m, _| m.button())
            .expect("a send is on");
        assert_eq!((label.as_ref(), view), ("Sending", View::Send));
        // Offering what is offered already changes nothing.
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.send_to_pc(r"c:\ملفات\تقرير.PDF".into(), cx)));
        assert_eq!(env.fake.sends.lock().unwrap().len(), 1);

        send_events(&env, 0)
            .send(SendEvent::Ready {
                ticket: ticket(),
                size: "24.00 MiB".into(),
                qr: QR.into(),
            })
            .expect("the channel is open");
        settle(&mut env.visual, |cx| has(cx, "The ticket as a QR code"));
        assert_eq!(role_of(&mut env.visual, "The ticket as a QR code"), Some(Role::Image));
        assert!(has(&mut env.visual, "Ticket"));
        assert!(has(&mut env.visual, "Copy the ticket"));
        assert!(!has(&mut env.visual, "Reading it through for its ticket…"));
        let boxed = dialog.read_with(&env.visual, |d, _| {
            d.boxes.values().next().cloned().expect("a ticket box")
        });
        assert_eq!(boxed.read_with(&env.visual, |b, _| b.value().to_string()), ticket());

        // Copy puts the ticket on the clipboard.
        click(&mut env.visual, ("share-copy-ticket", 1_usize));
        let copied = env
            .visual
            .update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
        assert_eq!(copied, Some(ticket()));

        click(&mut env.visual, ("share-stop-send", 1_usize));
        assert_eq!(*env.fake.stopped.lock().unwrap(), vec![100]);
        assert!(env.model.read_with(&env.visual, |m, _| m.sends().is_empty()));
        assert!(has(
            &mut env.visual,
            "Right-click a file or a folder and choose Send to another PC, or find it in Ctrl+K."
        ));
    }

    #[gpui_kit::test]
    fn a_send_that_ends_by_itself_says_why_and_can_be_removed(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, vec![]);
        open(&mut env, View::Send);
        let model = env.model.clone();
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.send_to_pc(r"C:\a.zip".into(), cx)));
        settle(&mut env.visual, |cx| {
            model.read_with(cx, |m, _| m.sends().first().is_some_and(|s| s.id.is_some()))
        });
        send_events(&env, 0)
            .send(SendEvent::Ended {
                message: "sendme stopped".into(),
            })
            .expect("open");
        settle(&mut env.visual, |cx| has(cx, "sendme stopped"));
        assert_eq!(role_of(&mut env.visual, "sendme stopped"), Some(Role::Alert));
        assert_eq!(env.model.read_with(&env.visual, |m, _| m.button()), None);
        // Remove takes it away without telling sendme to stop what has stopped.
        click(&mut env.visual, ("share-stop-send", 1_usize));
        assert!(env.fake.stopped.lock().unwrap().is_empty());
        assert!(env.model.read_with(&env.visual, |m, _| m.sends().is_empty()));
    }

    #[gpui_kit::test]
    fn a_send_stopped_while_it_starts_is_stopped_when_it_has(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, vec![]);
        let model = env.model.clone();
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.send_to_pc(r"C:\a.zip".into(), cx)));
        // Stop before core has answered with an id.
        env.visual.update(|_, cx| {
            model.update(cx, |m, cx| {
                let key = m.sends()[0].key;
                m.stop_send(key, cx)
            })
        });
        let mut visual = env.visual;
        settle(&mut visual, |_| !env.fake.stopped.lock().unwrap().is_empty());
        assert_eq!(*env.fake.stopped.lock().unwrap(), vec![100]);
    }

    #[gpui_kit::test]
    fn receiving_checks_the_ticket_then_shows_progress_and_where_it_went(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, vec![]);
        let model = env.model.clone();
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.open_receive(r"D:\Incoming".into(), cx)));
        let dialog = open(&mut env, View::Receive);
        let arrived = Rc::new(RefCell::new(Vec::new()));
        let sink = arrived.clone();
        let _hear = env.visual.update(|_, cx| {
            cx.subscribe(&model, move |_, event: &ShareEvent, _| {
                sink.borrow_mut().push(event.clone())
            })
        });
        assert!(disabled(&mut env.visual, "share-receive"));
        assert!(has(&mut env.visual, "Into D:\\Incoming"));

        // Half a ticket is said not to be one, and Receive waits.
        let input = dialog.read_with(&env.visual, |d, _| d.ticket_input().clone());
        env.visual.update(|window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
            window.input("blobabc", cx);
        });
        env.visual.run_until_parked();
        assert_eq!(
            role_of(
                &mut env.visual,
                "That isn't a whole ticket: it's the long line starting with “blob”."
            ),
            Some(Role::Alert)
        );
        assert!(disabled(&mut env.visual, "share-receive"));
        // Enter with half a ticket does nothing.
        env.visual.update(|window, cx| window.press("enter", cx));
        env.visual.run_until_parked();
        assert!(env.fake.receives.lock().unwrap().is_empty());

        // The whole line, as the sending PC shows it, is accepted.
        env.visual.update(|window, cx| {
            window.press("ctrl-a", cx);
            window.input(&format!("sendme receive {}", ticket()), cx);
        });
        env.visual.run_until_parked();
        assert!(!has(
            &mut env.visual,
            "That isn't a whole ticket: it's the long line starting with “blob”."
        ));
        assert!(!disabled(&mut env.visual, "share-receive"));
        click(&mut env.visual, "share-receive");
        settle(&mut env.visual, |cx| {
            model.read_with(cx, |m, _| m.receiving().is_some_and(|r| r.id.is_some()))
        });
        {
            let receives = env.fake.receives.lock().unwrap();
            assert_eq!(receives.len(), 1);
            assert_eq!(receives[0].0, r"D:\Incoming");
            assert!(
                receives[0].1.starts_with("sendme receive blob"),
                "core checks the line again"
            );
        }
        assert!(has(&mut env.visual, "Reaching the other PC…, 0%"));

        let events = env.fake.receives.lock().unwrap()[0].2.clone();
        events
            .send(ReceiveEvent::Getting {
                files: 2,
                size: "24.00 MiB".into(),
                bytes: 24 * 1024 * 1024,
            })
            .expect("open");
        events
            .send(ReceiveEvent::Progress {
                bytes: 12 * 1024 * 1024,
            })
            .expect("open");
        settle(&mut env.visual, |cx| has(cx, "Receiving 2 files, 24.00 MiB, 50%"));
        assert_eq!(
            role_of(&mut env.visual, "Receiving 2 files, 24.00 MiB, 50%"),
            Some(Role::ProgressIndicator)
        );

        events
            .send(ReceiveEvent::Done {
                path: r"D:\Incoming\تقرير".into(),
            })
            .expect("open");
        settle(&mut env.visual, |cx| has(cx, "Received “تقرير”"));
        assert_eq!(
            *arrived.borrow(),
            vec![ShareEvent::Received {
                path: r"D:\Incoming\تقرير".into(),
                shown: true
            }]
        );
        // Show closes the dialog and reveals the file.
        click(&mut env.visual, "share-received-show");
        assert_eq!(*env.revealed.borrow(), vec![r"D:\Incoming\تقرير".to_string()]);
        env.visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn receiving_that_fails_says_why_and_try_again_starts_over(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, vec![]);
        let model = env.model.clone();
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.open_receive(r"D:\In".into(), cx)));
        open(&mut env, View::Receive);
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.receive(ticket(), cx)));
        settle(&mut env.visual, |_| !env.fake.receives.lock().unwrap().is_empty());
        let events = env.fake.receives.lock().unwrap()[0].2.clone();
        events
            .send(ReceiveEvent::Failed {
                message: "no route to the other PC".into(),
            })
            .expect("open");
        settle(&mut env.visual, |cx| has(cx, "Couldn't receive it"));
        assert_eq!(role_of(&mut env.visual, "Couldn't receive it"), Some(Role::Alert));
        click(&mut env.visual, "share-receive-retry");
        assert!(env.model.read_with(&env.visual, |m, _| m.receiving().is_none()));
        // Stopping what has failed doesn't tell sendme to stop.
        assert!(env.fake.stopped.lock().unwrap().is_empty());
        assert!(disabled(&mut env.visual, "share-receive"));
    }

    #[gpui_kit::test]
    fn stopping_a_receive_under_way_stops_sendme_and_a_late_event_is_ignored(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, vec![]);
        let model = env.model.clone();
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.open_receive(r"D:\In".into(), cx)));
        open(&mut env, View::Receive);
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.receive(ticket(), cx)));
        settle(&mut env.visual, |cx| {
            model.read_with(cx, |m, _| m.receiving().is_some_and(|r| r.id.is_some()))
        });
        click(&mut env.visual, "share-receive-stop");
        assert_eq!(*env.fake.stopped.lock().unwrap(), vec![100]);
        let events = env.fake.receives.lock().unwrap()[0].2.clone();
        events.send(ReceiveEvent::Progress { bytes: 5 }).expect("open");
        env.visual.run_until_parked();
        assert!(env.model.read_with(&env.visual, |m, _| m.receiving().is_none()));
        // The folder choice stays where it was.
        assert_eq!(
            env.model.read_with(&env.visual, |m, _| m.into_folder().to_string()),
            r"D:\In"
        );
    }

    #[gpui_kit::test]
    fn sharing_on_the_network_asks_which_network_how_long_and_whether_they_may_add_files(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, vec![wifi(), ethernet()]);
        let model = env.model.clone();
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.open_network_share(r"C:\Photos".into(), cx)));
        let dialog = open(&mut env, View::Network);
        settle(&mut env.visual, |cx| has(cx, "Network"));
        assert_eq!(role_of(&mut env.visual, "Network"), Some(Role::RadioGroup));
        assert_eq!(role_of(&mut env.visual, "Wi-Fi · 192.168.1.5"), Some(Role::RadioButton));
        assert_eq!(role_of(&mut env.visual, "Stop it"), Some(Role::RadioGroup));
        assert!(!disabled(&mut env.visual, "share-serve"));

        // The second network, four hours, and let them add files.
        click(&mut env.visual, ("share-network", 1_usize));
        assert_eq!(
            dialog.read_with(&env.visual, |d, _| d.address.clone()),
            Some("10.0.0.2".to_string())
        );
        click(&mut env.visual, ("share-length", 240_usize));
        click(&mut env.visual, "share-uploads");
        assert_eq!(
            env.model.read_with(&env.visual, |m, _| m.choice()),
            Choice {
                uploads: true,
                minutes: 240
            }
        );
        click(&mut env.visual, "share-serve");
        settle(&mut env.visual, |_| !env.fake.serves.lock().unwrap().is_empty());
        {
            let serves = env.fake.serves.lock().unwrap();
            assert_eq!(serves[0].0, r"C:\Photos");
            assert!(serves[0].1.uploads);
            assert_eq!(serves[0].1.minutes, 240);
            assert_eq!(serves[0].1.address.as_deref(), Some("10.0.0.2"));
        }
        assert_eq!(env.model.read_with(&env.visual, |m, _| m.offer().to_string()), "");
        assert!(has(&mut env.visual, "Starting…"));
        let (label, view) = env
            .model
            .read_with(&env.visual, |m, _| m.button())
            .expect("a share is on");
        assert_eq!((label.as_ref(), view), ("Sharing", View::Network));

        // dufs is listening: the link, its QR code and the countdown.
        let events = env.fake.serves.lock().unwrap()[0].2.clone();
        *env.fake.reach.lock().unwrap() = Reach {
            network: Some("public"),
            firewall: Firewall::Blocked,
        };
        events
            .send(LanEvent::Ready {
                url: "http://10.0.0.2:23456/k3j9/".into(),
                qr: QR.into(),
                ends: Some(u64::try_from(NOW + 600_000).unwrap()),
            })
            .expect("open");
        settle(&mut env.visual, |cx| {
            model.read_with(cx, |m, _| m.serving().first().is_some_and(|s| s.reach.is_some()))
        });
        assert_eq!(role_of(&mut env.visual, "The link as a QR code"), Some(Role::Image));
        assert_eq!(role_of(&mut env.visual, "Link"), Some(Role::Link));
        assert_eq!(
            role_of(
                &mut env.visual,
                "Other devices can't open the link: Windows Firewall blocks dufs on public networks, and Windows has this network as public."
            ),
            Some(Role::Status)
        );
        let countdown_label = env.visual.update(|window, cx| {
            window.render_frame(cx);
            window.find(("share-countdown", 1_usize)).label().map(str::to_string)
        });
        assert!(countdown_label.expect("a countdown").ends_with(", in 10 minutes."));

        // Both of Windows' pages open for the address that was shared.
        click(&mut env.visual, ("share-network-settings", 1_usize));
        click(&mut env.visual, ("share-allowed-apps", 1_usize));
        settle(&mut env.visual, |_| env.fake.settings.lock().unwrap().len() == 2);
        assert_eq!(
            *env.fake.settings.lock().unwrap(),
            vec![
                ("network".to_string(), "10.0.0.2".to_string()),
                ("firewall".to_string(), "10.0.0.2".to_string())
            ]
        );

        // Open and Copy.
        click(&mut env.visual, ("share-open-link", 1_usize));
        assert_eq!(*env.opened.borrow(), vec!["http://10.0.0.2:23456/k3j9/".to_string()]);
        click(&mut env.visual, ("share-copy-link", 1_usize));
        let copied = env
            .visual
            .update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
        assert_eq!(copied.as_deref(), Some("http://10.0.0.2:23456/k3j9/"));

        // A request, then a failing one.
        events
            .send(LanEvent::Visit {
                from: "10.0.0.9".into(),
                request: "GET /k3j9/a.jpg".into(),
                status: 200,
            })
            .expect("open");
        events
            .send(LanEvent::Visit {
                from: "10.0.0.9".into(),
                request: "GET /k3j9/b.jpg".into(),
                status: 404,
            })
            .expect("open");
        settle(&mut env.visual, |cx| {
            model.read_with(cx, |m, _| m.serving()[0].visits == 2)
        });
        let visits = env.model.read_with(&env.visual, |m, _| {
            (m.serving()[0].from.clone(), m.serving()[0].last.clone())
        });
        assert_eq!(visits.0, vec!["10.0.0.9".to_string()]);
        assert_eq!(
            visits.1.map(|v| (v.request, v.status, v.at)),
            Some(("GET /k3j9/b.jpg".to_string(), 404, NOW))
        );

        // Stop sharing.
        click(&mut env.visual, ("share-stop-serve", 1_usize));
        assert_eq!(*env.fake.stopped.lock().unwrap(), vec![100]);
        assert!(env.model.read_with(&env.visual, |m, _| m.serving().is_empty()));
    }

    #[gpui_kit::test]
    fn a_pc_on_no_network_cannot_share_and_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, vec![]);
        let model = env.model.clone();
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.open_network_share(r"C:\Photos".into(), cx)));
        open(&mut env, View::Network);
        settle(&mut env.visual, |cx| has(cx, "This PC isn't on a network."));
        assert_eq!(
            role_of(&mut env.visual, "This PC isn't on a network."),
            Some(Role::Alert)
        );
        assert!(disabled(&mut env.visual, "share-serve"));
        // With no network there is none to choose between.
        assert!(!has(&mut env.visual, "Network"));
    }

    #[gpui_kit::test]
    fn a_share_that_ended_says_why_and_sharing_it_twice_starts_one(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, vec![wifi()]);
        let model = env.model.clone();
        open(&mut env, View::Network);
        let options = LanOptions {
            uploads: false,
            minutes: 0,
            address: None,
        };
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.serve(r"C:\Photos".into(), options.clone(), cx)));
        settle(&mut env.visual, |_| !env.fake.serves.lock().unwrap().is_empty());
        // Sharing it again while it is shared changes nothing.
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.serve(r"c:\photos".into(), options.clone(), cx)));
        assert_eq!(env.fake.serves.lock().unwrap().len(), 1);
        let events = env.fake.serves.lock().unwrap()[0].2.clone();
        events
            .send(LanEvent::Ended {
                message: "Its time was up".into(),
            })
            .expect("open");
        settle(&mut env.visual, |cx| has(cx, "Its time was up"));
        assert_eq!(role_of(&mut env.visual, "Its time was up"), Some(Role::Alert));
        assert_eq!(env.model.read_with(&env.visual, |m, _| m.button()), None);
        click(&mut env.visual, ("share-stop-serve", 1_usize));
        assert!(env.fake.stopped.lock().unwrap().is_empty());
    }

    #[gpui_kit::test]
    fn the_tabs_switch_views_and_escape_closes_in_arabic_too(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, vec![wifi()]);
        testing::set_rtl(true);
        let model = env.model.clone();
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.send_to_pc(r"C:\ملفات\أرشيف.7z".into(), cx)));
        let dialog = open(&mut env, View::Send);
        assert!(has(&mut env.visual, "أرشيف.7z"));
        let selected = |visual: &mut VisualTestContext, view: View| {
            visual.update(|window, cx| {
                window.render_frame(cx);
                window.find(("share-tab", view as usize)).checked()
            })
        };
        assert_eq!(selected(&mut env.visual, View::Send), Some(true));
        click(&mut env.visual, ("share-tab", View::Receive as usize));
        assert_eq!(dialog.read_with(&env.visual, |d, _| d.view()), View::Receive);
        assert_eq!(selected(&mut env.visual, View::Send), Some(false));
        assert_eq!(selected(&mut env.visual, View::Receive), Some(true));
        assert!(has(&mut env.visual, "Receive from another PC"));
        click(&mut env.visual, ("share-tab", View::Network as usize));
        assert_eq!(dialog.read_with(&env.visual, |d, _| d.view()), View::Network);
        assert!(has(
            &mut env.visual,
            "Right-click a folder or a file and choose Share on this network, or find it in Ctrl+K."
        ));
        env.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        env.visual.run_until_parked();
        env.visual
            .update(|window, cx| assert!(!window.has_active_dialog(cx), "Escape closes it"));
        // Closing the dialog doesn't stop the send: its ticket stays good.
        assert!(env.fake.stopped.lock().unwrap().is_empty());
        assert_eq!(env.model.read_with(&env.visual, |m, _| m.sends().len()), 1);
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn something_received_while_the_dialog_is_closed_is_announced_with_show(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, vec![]);
        let model = env.model.clone();
        env.visual.update(|_, cx| install(&model, cx));
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.open_receive(r"D:\In".into(), cx)));
        env.visual
            .update(|_, cx| model.update(cx, |m, cx| m.receive(ticket(), cx)));
        settle(&mut env.visual, |_| !env.fake.receives.lock().unwrap().is_empty());
        let events = env.fake.receives.lock().unwrap()[0].2.clone();
        events
            .send(ReceiveEvent::Done {
                path: r"D:\In\photo.jpg".into(),
            })
            .expect("open");
        settle(&mut env.visual, |cx| {
            cx.update(|window, cx| window.notifications(cx).len() == 1)
        });
    }

    #[gpui_kit::test]
    fn share_with_windows_hands_the_paths_over_and_a_refusal_is_reported(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let env = env(cx, vec![]);
        let backend: Arc<dyn Backend> = env.fake.clone();
        let mut visual = env.visual;
        visual.update(|window, cx| {
            share_with_windows_using(
                window,
                cx,
                backend.clone(),
                vec![r"C:\a.txt".into(), r"C:\ب.txt".into()],
            )
        });
        settle(&mut visual, |_| !env.fake.shared.lock().unwrap().is_empty());
        assert_eq!(
            *env.fake.shared.lock().unwrap(),
            vec![vec![r"C:\a.txt".to_string(), r"C:\ب.txt".to_string()]]
        );
        visual.update(|window, cx| assert_eq!(window.notifications(cx).len(), 0));
        env.fake.windows_fails.store(true, Ordering::SeqCst);
        visual.update(|window, cx| share_with_windows_using(window, cx, backend.clone(), vec![r"C:\a.txt".into()]));
        settle(&mut visual, |cx| {
            cx.update(|window, cx| window.notifications(cx).len() == 1)
        });
    }

    #[gpui_kit::test]
    fn the_title_bar_button_counts_sends_and_shares(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let env = env(cx, vec![]);
        let model = env.model.clone();
        let mut visual = env.visual;
        let label =
            |visual: &VisualTestContext| model.read_with(visual, |m, _| m.button().map(|(l, v)| (l.to_string(), v)));
        assert_eq!(label(&visual), None);
        visual.update(|_, cx| model.update(cx, |m, cx| m.send_to_pc(r"C:\a".into(), cx)));
        assert_eq!(label(&visual), Some(("Sending".into(), View::Send)));
        visual.update(|_, cx| model.update(cx, |m, cx| m.send_to_pc(r"C:\b".into(), cx)));
        assert_eq!(label(&visual), Some(("Sharing 2".into(), View::Send)));
        let options = LanOptions {
            uploads: false,
            minutes: 0,
            address: None,
        };
        visual.update(|_, cx| model.update(cx, |m, cx| m.serve(r"C:\c".into(), options, cx)));
        assert_eq!(label(&visual), Some(("Sharing 3".into(), View::Send)));
        settle(&mut visual, |_| env.fake.sends.lock().unwrap().len() == 2);
        // Only a share left: the button opens the network view.
        visual.update(|_, cx| {
            model.update(cx, |m, cx| {
                let keys: Vec<u64> = m.sends().iter().map(|s| s.key).collect();
                for key in keys {
                    m.stop_send(key, cx);
                }
            })
        });
        assert_eq!(label(&visual), Some(("Sharing".into(), View::Network)));
    }
    #[gpui_kit::test]
    fn escape_closes_the_dialog_with_the_ticket_box_focused(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = env(cx, vec![]);
        let dialog = open(&mut env, View::Receive);
        let input = dialog.read_with(&env.visual, |d, _| d.ticket_input().clone());
        let focused = env
            .visual
            .update(|window, cx| input.read(cx).focus_handle(cx).is_focused(window));
        assert!(focused, "Receive opens with the ticket box focused");
        env.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        env.visual.run_until_parked();
        env.visual
            .update(|window, cx| assert!(!window.has_active_dialog(cx), "Escape closes it"));
    }
}
