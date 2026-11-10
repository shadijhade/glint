//! Adding and editing a remote connection, and removing one: a cloud (through rclone), S3-compatible storage,
//! WebDAV or SFTP. Ported from src/components/explorer/RemoteDialog.tsx (and `remotes.ts` for the calls).
//!
//! | TypeScript                                  | Here                                                           |
//! |---------------------------------------------|----------------------------------------------------------------|
//! | `addRemote()`                               | [`open_add`]                                                   |
//! | `editRemote(remote)`                        | [`open_edit`] (by the connection's id)                         |
//! | `removeRemote(remote)`                      | [`remove_remote`] (by id; the form's Remove uses [`remove_remote_then`]) |
//! | `RemoteForm`'s state, `runTest`, `trust`, `save` | [`Form`], a model entity                                  |
//! | `RemoteForm`'s markup, `Field`, `KINDS`     | [`RemoteDialog`], the view                                     |
//! | `<CloudSetup />` in the form for a new cloud | `cloud_setup::CloudSetup`, embedded                                       |
//! | `remoteTest`, `remoteHostKeys`, `remoteTrustHost`, `remoteSecret*`, `rcloneDelete` | [`Backend`] (a fake in the tests), [`CoreBackend`] |
//! | `useStore().saveRemote`, `removeRemote`     | `persistence::update_app` with `AppState::save_remote` / `remove_remote` |
//! | `leaveRemote(id)`                           | `Explorer::leave_remote`                                       |
//! | `initRemotes()` (pushes connections to Rust) | [`push_to_core`], called after every change made here          |
//! | `confirm()` of Tauri's dialog plugin        | `dialogs::confirm`, and a two-button dialog for "Sign out"     |
//!
//! **How the coordinator wires it.**
//! - Sidebar `SidebarEvent::AddConnection` (and Ctrl+K's "Add a remote connection"): `open_add(window, cx)`.
//! - `MenuEvent::EditConnection { id }` (and `SidebarEvent::EditRemote(id)`, no longer emitted): `open_edit(window, cx, id)`.
//! - `MenuEvent::RemoveConnection { id }` (and `SidebarEvent::RemoveRemote(id)`): `remove_remote(window, cx, id)`.
//! - `SettingsEvent::ConnectCloud`: `dialogs::cloud_setup::open(window, cx)` (or `open_add`, which starts on "A cloud").
//! - At start, and whenever `AppState.remotes` changes elsewhere: `remote::push_to_core(cx)`, as `initRemotes` did.
//!   (This file calls it after each change it makes itself.)

use crate::components::settings::clouds;
use crate::kit::prelude::*;
use crate::{core_bridge, explorer, persistence, t, theme};
use glint_model::store::{RemoteConnection, RemoteKind};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// How an SFTP sign-in's failure starts when the server's host key isn't trusted yet (remote/sftp.rs).
// TODO(coordinator): from glint_core::remote::sftp::UNKNOWN_HOST, which that private module doesn't export.
pub const UNKNOWN_HOST: &str = "Its host key isn't known yet";

/// An SFTP server's host keys, and their fingerprints, to see before trusting them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeys {
    /// One fingerprint a line ("256 SHA256:… host (ED25519)").
    pub fingerprints: Vec<String>,
    /// The keys, as `remote_trust_host` takes them.
    pub keys: String,
}

/// What the dialog asks of the PC. [`CoreBackend`] is the real one; tests give a fake.
pub trait Backend: Send + Sync {
    /// Tries a connection as it is typed: `secret` is what was typed, or (`None`) the one kept; what is at its
    /// top, or why it didn't connect (`remote_test`).
    fn test(&self, connection: RemoteConnection, secret: Option<String>) -> Fut<String>;
    /// An SFTP server's host keys (`remote_host_keys`).
    fn host_keys(&self, host: String, port: u16) -> Fut<HostKeys>;
    /// Trusts the keys `host_keys` read (`remote_trust_host`).
    fn trust_host(&self, keys: String) -> Fut<()>;
    /// Whether a connection has a secret kept in Credential Manager (`remote_secret_saved`).
    fn secret_saved(&self, id: String) -> Fut<bool>;
    /// Keeps a connection's secret (`remote_secret_set`).
    fn secret_set(&self, id: String, secret: String) -> Fut<()>;
    /// Forgets a connection's secret (`remote_secret_delete`).
    fn secret_delete(&self, id: String) -> Fut<()>;
    /// Takes a remote out of rclone's configuration (`rclone_delete`).
    fn rclone_delete(&self, name: String) -> Fut<()>;
}

/// Waits for a core call and flattens the two layers of failure (the task panicking, the call failing).
fn flat<T: Send + 'static>(call: impl Future<Output = Result<T, String>> + Send + 'static) -> Fut<T> {
    let joined = core_bridge::run(call);
    Box::pin(async move { joined.await.and_then(|inner| inner) })
}

/// core's connection for a saved one (the same JSON shape; the saved one types its choices). A port beyond
/// 65535 is cut to it, so one wrong digit can't make the whole connection blank.
fn to_core(connection: &RemoteConnection) -> glint_core::remote::Connection {
    let mut connection = connection.clone();
    connection.port = connection.port.map(|p| p.min(65_535));
    serde_json::to_value(&connection)
        .and_then(serde_json::from_value)
        .unwrap_or_default()
}

/// glint-core's remote connections.
pub struct CoreBackend;

impl Backend for CoreBackend {
    fn test(&self, connection: RemoteConnection, secret: Option<String>) -> Fut<String> {
        flat(glint_core::remote::remote_test(to_core(&connection), secret))
    }

    fn host_keys(&self, host: String, port: u16) -> Fut<HostKeys> {
        flat(async move {
            let found = glint_core::remote::remote_host_keys(host, port).await?;
            Ok(HostKeys {
                fingerprints: found.fingerprints,
                keys: found.keys,
            })
        })
    }

    fn trust_host(&self, keys: String) -> Fut<()> {
        flat(async move { glint_core::remote::remote_trust_host(keys) })
    }

    fn secret_saved(&self, id: String) -> Fut<bool> {
        flat(async move { Ok(glint_core::remote::remote_secret_saved(id)) })
    }

    fn secret_set(&self, id: String, secret: String) -> Fut<()> {
        flat(async move { glint_core::remote::remote_secret_set(id, secret) })
    }

    fn secret_delete(&self, id: String) -> Fut<()> {
        flat(async move {
            glint_core::remote::remote_secret_delete(id);
            Ok(())
        })
    }

    fn rclone_delete(&self, name: String) -> Fut<()> {
        flat(glint_core::rclone::rclone_delete(name))
    }
}

struct SharedBackend(Arc<dyn Backend>);

impl Global for SharedBackend {}

/// Sets the backend the dialogs use (tests pass a fake). [`backend`] makes core's if this was never called.
pub fn init_with(cx: &mut App, backend: Arc<dyn Backend>) {
    cx.set_global(SharedBackend(backend));
}

/// The backend in use.
pub fn backend(cx: &mut App) -> Arc<dyn Backend> {
    if let Some(shared) = cx.try_global::<SharedBackend>() {
        return shared.0.clone();
    }
    let backend: Arc<dyn Backend> = Arc::new(CoreBackend);
    init_with(cx, backend.clone());
    backend
}

/// Tells core the connections there are now (`initRemotes` pushed them at start and at every change). The call
/// runs on core's runtime, so the UI thread doesn't wait on its lock.
pub fn push_to_core(cx: &App) {
    let all: Vec<glint_core::remote::Connection> = persistence::app(cx)
        .map(|app| app.remotes.iter().map(to_core).collect())
        .unwrap_or_default();
    // The call is made as it is spawned; the answer (there is none) isn't waited for.
    drop(core_bridge::run(
        async move { glint_core::remote::remote_configure(all) },
    ));
}

/// What `remote_test` says (core's English), in the interface language.
fn said_text(said: &str) -> SharedString {
    match said {
        "Connected: there's nothing there yet" => t!("Connected: there's nothing there yet"),
        "Connected: 1 item at its top" => t!("Connected: 1 item at its top"),
        other => match other
            .strip_prefix("Connected: ")
            .and_then(|rest| rest.strip_suffix(" items at its top"))
        {
            Some(n) => t!("Connected: {n} items at its top", n = n),
            None => other.to_string().into(),
        },
    }
}

/// A service's icon: a server for protocols, a cloud for companies' clouds (`serviceIcon`).
fn service_icon(kind: &str) -> IconName {
    if ["sftp", "ftp", "smb", "webdav", "http", "hdfs", "seafile"].contains(&kind) {
        IconName::Server
    } else {
        IconName::Cloud
    }
}

// ---- Removing ---------------------------------------------------------------------------------------------------

/// What runs once a connection is gone.
type Then = Rc<dyn Fn(&mut Window, &mut App)>;

/// What the sign-out question answers: whether to sign out.
type Answer = Rc<dyn Fn(&mut Window, &mut App, bool)>;

/// Removes a connection (asking first): its secret leaves Credential Manager, its tabs go to This PC. Nothing on
/// the remote changes (`removeRemote`). A cloud can be signed out of as well.
pub fn remove_remote(window: &mut Window, cx: &mut App, id: String) {
    remove_remote_then(window, cx, id, |_, _| {});
}

/// [`remove_remote`], and `then` runs once the connection is gone (the form closes itself).
pub fn remove_remote_then(
    window: &mut Window,
    cx: &mut App,
    id: String,
    then: impl Fn(&mut Window, &mut App) + 'static,
) {
    let backend = backend(cx);
    remove_with(window, cx, backend, id, Rc::new(then));
}

/// [`remove_remote_then`] with a backend of your own.
pub fn remove_with(window: &mut Window, cx: &mut App, backend: Arc<dyn Backend>, id: String, then: Then) {
    let Some(remote) = persistence::app(cx).and_then(|app| app.remotes.iter().find(|r| r.id == id).cloned()) else {
        return;
    };
    let message = t!(
        "Remove the connection “{name}”? Its secret goes from Credential Manager; nothing on it changes.",
        name = remote.name
    );
    super::confirm(
        window,
        cx,
        t!("Remove the connection"),
        message,
        t!("Remove"),
        true,
        move |window, cx| {
            let (backend, remote, then) = (backend.clone(), remote.clone(), then.clone());
            // One of rclone's remotes: signed out of as well, if asked (a terminal's rclone then loses it too).
            match remote.rclone.clone().filter(|_| remote.kind == RemoteKind::Rclone) {
                Some(rclone) => ask_sign_out(window, cx, rclone, move |window, cx, sign_out| {
                    finish_removal(window, cx, backend.clone(), remote.clone(), sign_out, then.clone())
                }),
                None => finish_removal(window, cx, backend, remote, false, then),
            }
        },
    );
}

/// "Sign out of it as well?": Sign out, or Keep it in rclone. Closing the dialog any other way keeps it.
fn ask_sign_out(
    window: &mut Window,
    cx: &mut App,
    rclone: String,
    answer: impl Fn(&mut Window, &mut App, bool) + 'static,
) {
    let answer: Answer = Rc::new(answer);
    let message = t!(
        "Sign out of it as well? “{name}” goes from rclone's configuration, with its sign-in, so rclone in a terminal loses it too.",
        name = rclone
    );
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let (keep, out, escape) = (answer.clone(), answer.clone(), answer.clone());
        dialog
            .title(t!("Sign out"))
            .w(px(440.))
            .on_ok(|_, _, _| false)
            .on_cancel(move |_, window, cx| {
                escape(window, cx, false);
                true
            })
            .child(
                div()
                    .id("sign-out")
                    .role(Role::Group)
                    .aria_label(message.clone())
                    .test_support()
                    .text_start()
                    .child(message.clone()),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("remove-keep")
                            .label(t!("Keep it in rclone"))
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                keep(window, cx, false);
                            }),
                    )
                    .child(Button::new("remove-sign-out").danger().label(t!("Sign out")).on_click(
                        move |_, window, cx| {
                            window.close_dialog(cx);
                            out(window, cx, true);
                        },
                    )),
            )
    });
}

/// Signs out (if asked), forgets the secret, drops the connection and sends its tabs to This PC.
fn finish_removal(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    remote: RemoteConnection,
    sign_out: bool,
    then: Then,
) {
    window
        .spawn(cx, async move |cx| {
            if let (true, Some(rclone)) = (sign_out, remote.rclone.clone()) {
                if let Err(reason) = backend.rclone_delete(rclone).await {
                    let _ = cx.update(|window, cx| {
                        window.push_notification(
                            Notification::error(reason).title(t!("Couldn't take it out of rclone")),
                            cx,
                        )
                    });
                }
            }
            let _ = backend.secret_delete(remote.id.clone()).await;
            let _ = cx.update(|window, cx| {
                let id = remote.id.clone();
                persistence::update_app(cx, |app| app.remove_remote(&id));
                push_to_core(cx);
                if let Some(tabs) = explorer::try_explorer(cx) {
                    tabs.update(cx, |e, cx| e.leave_remote(&id, cx));
                }
                then(window, cx);
            });
        })
        .detach();
}

// ---- The form's model -------------------------------------------------------------------------------------------

/// The form's text boxes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Endpoint,
    Region,
    Bucket,
    User,
    Host,
    Port,
    Key,
    Root,
    Secret,
}

impl Field {
    /// Every box, in the order they are made.
    pub const ALL: [Field; 10] = [
        Field::Name,
        Field::Endpoint,
        Field::Region,
        Field::Bucket,
        Field::User,
        Field::Host,
        Field::Port,
        Field::Key,
        Field::Root,
        Field::Secret,
    ];
}

/// How a test of the connection stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Test {
    /// Connecting…
    Running,
    /// Connected: what is at its top.
    Ok(String),
    /// Why it didn't connect.
    Failed(String),
}

/// What the form tells the view.
#[derive(Clone, Debug, PartialEq)]
pub enum FormEvent {
    /// Saved (its secret kept): store the connection, close, and (for a new one) open it.
    Saved(Box<RemoteConnection>),
    /// The secret couldn't be kept in Credential Manager, so nothing was saved.
    SecretFailed(String),
}

/// A connection being added or edited: what is typed, the test, and saving.
pub struct Form {
    backend: Arc<dyn Backend>,
    editing: Option<RemoteConnection>,
    kind: RemoteKind,
    texts: [String; 10],
    thumbnails: bool,
    /// A secret is kept in Credential Manager for the connection being edited.
    kept: bool,
    test: Option<Test>,
    /// An SFTP server whose host key isn't trusted yet: its fingerprints, to trust or not.
    host_keys: Option<HostKeys>,
    /// Counts edits, so the answer of a test of an earlier state is dropped.
    serial: u64,
    taken_ids: Vec<String>,
}

impl EventEmitter<FormEvent> for Form {}

impl Form {
    /// A form for a new connection (`editing` None, starting on "A cloud") or for editing one.
    pub fn new(
        backend: Arc<dyn Backend>,
        editing: Option<RemoteConnection>,
        taken_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut texts: [String; 10] = Default::default();
        let mut thumbnails = false;
        let mut kind = RemoteKind::Rclone;
        if let Some(r) = &editing {
            kind = r.kind;
            thumbnails = r.thumbnails.unwrap_or(false);
            texts[Field::Name as usize] = r.name.clone();
            texts[Field::Endpoint as usize] = r.endpoint.clone();
            texts[Field::Region as usize] = r.region.clone();
            texts[Field::Bucket as usize] = r.bucket.clone();
            texts[Field::User as usize] = r.user.clone();
            texts[Field::Host as usize] = r.host.clone().unwrap_or_default();
            texts[Field::Port as usize] = r.port.filter(|p| *p != 0).map(|p| p.to_string()).unwrap_or_default();
            texts[Field::Key as usize] = r.key.clone().unwrap_or_default();
            texts[Field::Root as usize] = r.root.clone();
        }
        let this = Self {
            backend: backend.clone(),
            editing: editing.clone(),
            kind,
            texts,
            thumbnails,
            kept: false,
            test: None,
            host_keys: None,
            serial: 0,
            taken_ids,
        };
        if let Some(r) = editing {
            let asking = backend.secret_saved(r.id);
            cx.spawn(async move |this, cx| {
                if let Ok(kept) = asking.await {
                    let _ = this.update(cx, |this, cx| {
                        this.kept = kept;
                        cx.notify();
                    });
                }
            })
            .detach();
        }
        this
    }

    /// The connection being edited.
    pub fn editing(&self) -> Option<&RemoteConnection> {
        self.editing.as_ref()
    }

    /// What kind of connection the form makes.
    pub fn kind(&self) -> RemoteKind {
        self.kind
    }

    /// Whether a secret is kept for the connection being edited.
    pub fn kept(&self) -> bool {
        self.kept
    }

    /// How the test stands.
    pub fn test(&self) -> Option<&Test> {
        self.test.as_ref()
    }

    /// The host keys of an untrusted SFTP server, once read.
    pub fn host_keys(&self) -> Option<&HostKeys> {
        self.host_keys.as_ref()
    }

    /// Thumbnails of pictures in the grid.
    pub fn thumbnails(&self) -> bool {
        self.thumbnails
    }

    /// What is typed in a box.
    pub fn text(&self, field: Field) -> &str {
        &self.texts[field as usize]
    }

    /// The port typed (`Number(digits) || 0`), at most 65535.
    pub fn port(&self) -> u32 {
        let digits: String = self.text(Field::Port).chars().filter(char::is_ascii_digit).collect();
        digits.parse::<u32>().unwrap_or(0).min(65_535)
    }

    fn forget_test(&mut self) {
        self.serial += 1;
        self.test = None;
    }

    /// A box changed. Any edit but the secret's also forgets the host keys shown (`change`).
    pub fn set_text(&mut self, field: Field, text: String, cx: &mut Context<Self>) {
        if self.texts[field as usize] == text {
            return;
        }
        self.texts[field as usize] = text;
        self.forget_test();
        if field != Field::Secret {
            self.host_keys = None;
        }
        cx.notify();
    }

    /// The kind of connection was chosen (`change({ kind })`).
    pub fn set_kind(&mut self, kind: RemoteKind, cx: &mut Context<Self>) {
        if self.kind == kind {
            return;
        }
        self.kind = kind;
        self.forget_test();
        self.host_keys = None;
        cx.notify();
    }

    /// Thumbnails were switched.
    pub fn set_thumbnails(&mut self, on: bool, cx: &mut Context<Self>) {
        self.thumbnails = on;
        self.forget_test();
        self.host_keys = None;
        cx.notify();
    }

    /// Name and the one thing the kind needs are filled in (`ready`).
    pub fn ready(&self) -> bool {
        let has = |field: Field| !self.text(field).trim().is_empty();
        has(Field::Name)
            && match self.kind {
                RemoteKind::Rclone => self.editing.as_ref().is_some_and(|r| r.rclone.is_some()),
                RemoteKind::S3 => has(Field::Bucket),
                RemoteKind::Sftp => has(Field::Host),
                RemoteKind::Webdav | RemoteKind::Device => has(Field::Endpoint),
            }
    }

    /// The connection as typed (`connection`).
    pub fn connection(&self) -> RemoteConnection {
        let trimmed = |field: Field| self.text(field).trim().to_string();
        let sftp = self.kind == RemoteKind::Sftp;
        let name = trimmed(Field::Name);
        let base = self.editing.clone().unwrap_or_default();
        RemoteConnection {
            id: match &self.editing {
                Some(r) => r.id.clone(),
                None => crate::components::settings::devices::remote_id(&name, &self.taken_ids),
            },
            name,
            kind: self.kind,
            endpoint: trimmed(Field::Endpoint),
            region: trimmed(Field::Region),
            bucket: trimmed(Field::Bucket),
            user: trimmed(Field::User),
            root: trimmed(Field::Root),
            host: sftp.then(|| trimmed(Field::Host)),
            port: sftp.then(|| self.port()),
            key: sftp.then(|| trimmed(Field::Key)),
            thumbnails: (self.thumbnails || base.thumbnails.is_some()).then_some(self.thumbnails),
            rclone: base.rclone,
            service: base.service,
        }
    }

    /// What is typed, or (editing, nothing typed) the secret kept: `None` (`typed`).
    pub fn typed_secret(&self) -> Option<String> {
        let secret = self.text(Field::Secret);
        if !secret.is_empty() {
            Some(secret.to_string())
        } else if self.editing.is_some() && self.kept {
            None
        } else {
            Some(String::new())
        }
    }

    /// Tries the connection as typed (`runTest`). An SFTP server that isn't trusted yet shows its host keys.
    pub fn run_test(&mut self, cx: &mut Context<Self>) {
        self.forget_test();
        self.host_keys = None;
        self.test = Some(Test::Running);
        cx.notify();
        let serial = self.serial;
        let asking = self.backend.test(self.connection(), self.typed_secret());
        let backend = self.backend.clone();
        let sftp = self.kind == RemoteKind::Sftp;
        let (host, port) = (self.text(Field::Host).trim().to_string(), self.port());
        cx.spawn(async move |this, cx| {
            let answer = asking.await;
            let mut unknown = false;
            let alive = this
                .update(cx, |this, cx| {
                    if this.serial != serial {
                        return;
                    }
                    match &answer {
                        Ok(said) => this.test = Some(Test::Ok(said.clone())),
                        Err(reason) => {
                            this.test = Some(Test::Failed(reason.clone()));
                            unknown = sftp && reason.starts_with(UNKNOWN_HOST);
                        }
                    }
                    cx.notify();
                })
                .is_ok();
            if !alive || !unknown {
                return;
            }
            // A server not trusted yet: what its host keys are, for the person to decide.
            let keys = backend.host_keys(host, u16::try_from(port).unwrap_or(0)).await;
            let _ = this.update(cx, |this, cx| {
                if this.serial != serial {
                    return;
                }
                match keys {
                    Ok(keys) => this.host_keys = Some(keys),
                    Err(reason) => this.test = Some(Test::Failed(reason)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Trusts the host keys shown, then tests again (`trust`).
    pub fn trust(&mut self, cx: &mut Context<Self>) {
        let Some(keys) = self.host_keys.as_ref().map(|k| k.keys.clone()) else {
            return;
        };
        let trusting = self.backend.trust_host(keys);
        cx.spawn(async move |this, cx| {
            let answer = trusting.await;
            let _ = this.update(cx, |this, cx| match answer {
                Ok(()) => this.run_test(cx),
                Err(reason) => {
                    this.test = Some(Test::Failed(reason));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Keeps the secret typed in Credential Manager, then says it is saved (`save`).
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if !self.ready() {
            return;
        }
        let made = self.connection();
        let secret = self.text(Field::Secret).to_string();
        let keeping = (!secret.is_empty()).then(|| self.backend.secret_set(made.id.clone(), secret));
        cx.spawn(async move |this, cx| {
            if let Some(keeping) = keeping {
                if let Err(reason) = keeping.await {
                    let _ = this.update(cx, |_, cx| cx.emit(FormEvent::SecretFailed(reason)));
                    return;
                }
            }
            let _ = this.update(cx, |_, cx| cx.emit(FormEvent::Saved(Box::new(made))));
        })
        .detach();
    }
}

// ---- The dialog -------------------------------------------------------------------------------------------------

/// The kinds of connection, as the form lists them (`KINDS`): the kind, its label, its hint and its icon.
fn kinds() -> [(RemoteKind, SharedString, SharedString, IconName); 4] {
    [
        (
            RemoteKind::Rclone,
            t!("A cloud"),
            t!("Google Drive, OneDrive, Dropbox and 60 more"),
            IconName::Cloud,
        ),
        (
            RemoteKind::S3,
            t!("S3-compatible storage"),
            t!("Amazon S3, Cloudflare R2, Backblaze B2, MinIO…"),
            IconName::Database,
        ),
        (
            RemoteKind::Webdav,
            t!("WebDAV"),
            t!("Nextcloud, a NAS, dufs…"),
            IconName::Server,
        ),
        (
            RemoteKind::Sftp,
            t!("SFTP"),
            t!("A server you reach with SSH"),
            IconName::SquareTerminal,
        ),
    ]
}

/// A label, its box and its hint (`Field`).
fn field(label: SharedString, hint: Option<SharedString>, content: impl IntoElement, t: &theme::Tokens) -> Div {
    let slot = format!("remote-hint-{label}");
    row_start()
        .items_start()
        .gap(space(3.))
        .child(
            div()
                .w(space(28.))
                .flex_none()
                .pt(space(1.5))
                .text_size(theme::size::XS)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted_foreground)
                .child(label),
        )
        .child(
            col()
                .flex_1()
                .min_w_0()
                .gap(space(1.))
                .child(content)
                .when_some(hint, |this, hint| {
                    // Named, so a screen reader reads the hint with its field.
                    this.child(
                        div()
                            .id(SharedString::from(slot))
                            .role(Role::Group)
                            .aria_label(hint.clone())
                            .test_support()
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(hint),
                    )
                }),
        )
}

type CloudEmbed = Option<Entity<super::cloud_setup::CloudSetup>>;

/// Adding or editing a connection.
pub struct RemoteDialog {
    form: Entity<Form>,
    boxes: Vec<Entity<InputState>>,
    cloud: CloudEmbed,
    cloud_backend: Option<Arc<dyn super::cloud_setup::Backend>>,
    tools: Option<Entity<crate::components::settings::tools::Tools>>,
    _subscriptions: Vec<Subscription>,
}

impl RemoteDialog {
    /// A dialog over `form`. For a new cloud, `cloud_backend` and `tools` make the embedded rclone steps.
    pub fn new(
        form: Entity<Form>,
        cloud_backend: Option<Arc<dyn super::cloud_setup::Backend>>,
        tools: Option<Entity<crate::components::settings::tools::Tools>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build(form, window, cx);
        this.cloud_backend = cloud_backend;
        this.tools = tools;
        this.sync_cloud(window, cx);
        this
    }

    fn build(form: Entity<Form>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.observe_in(&form, window, |this, _, window, cx| {
            this.sync_placeholders(window, cx);
            cx.notify();
        })];
        let mut boxes = Vec::new();
        for field in Field::ALL {
            let secret = field == Field::Secret;
            let placeholder: SharedString = Self::placeholder(field, form.read(cx));
            let initial = form.read(cx).text(field).to_string();
            let input = cx.new(|cx| InputState::new(window, cx).masked(secret).placeholder(placeholder));
            if !initial.is_empty() {
                input.update(cx, |input, cx| input.set_value(initial, window, cx));
            }
            subscriptions.push(
                cx.subscribe_in(
                    &input,
                    window,
                    move |this, input, event: &InputEvent, window, cx| match event {
                        InputEvent::Change => {
                            let mut text = input.read(cx).value().to_string();
                            if field == Field::Port {
                                // Digits only, as the TypeScript's `replace(/\D/g, "")`; 0 is empty.
                                let digits: String = text.chars().filter(char::is_ascii_digit).collect();
                                let number = digits.parse::<u32>().unwrap_or(0).min(65_535);
                                let shown = if number == 0 { String::new() } else { number.to_string() };
                                if shown != text {
                                    input.update(cx, |input, cx| input.set_value(shown.clone(), window, cx));
                                }
                                text = shown;
                            }
                            this.form.update(cx, |form, cx| form.set_text(field, text, cx));
                        }
                        // Enter saves, as the modal's submit did.
                        InputEvent::PressEnter { .. } => this.form.update(cx, |form, cx| form.save(cx)),
                        _ => {}
                    },
                ),
            );
            boxes.push(input);
        }
        // Saved: the connection is stored, the dialog closes, a new one opens.
        subscriptions.push(
            cx.subscribe_in(&form, window, |this, form, event: &FormEvent, window, cx| match event {
                FormEvent::Saved(made) => {
                    let new = form.read(cx).editing().is_none();
                    this.store(window, cx, (**made).clone(), new);
                }
                FormEvent::SecretFailed(reason) => window.push_notification(
                    Notification::error(reason.clone()).title(t!("Couldn't keep the secret in Credential Manager")),
                    cx,
                ),
            }),
        );
        Self {
            form,
            boxes,
            cloud: None,
            cloud_backend: None,
            tools: None,
            _subscriptions: subscriptions,
        }
    }

    /// The boxes' hints follow the kind (the name's example) and whether a secret is kept.
    fn sync_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for field in [Field::Name, Field::Secret] {
            let placeholder = Self::placeholder(field, self.form.read(cx));
            self.boxes[field as usize].update(cx, |input, cx| input.set_placeholder(placeholder, window, cx));
        }
    }

    fn placeholder(field: Field, form: &Form) -> SharedString {
        let s3 = form.kind() == RemoteKind::S3;
        match field {
            Field::Name if s3 => t!("Photos backup"),
            Field::Name => t!("My NAS"),
            Field::Secret if form.kept() => t!("Kept in Credential Manager: type to change it"),
            Field::Endpoint => t!("https://cloud.example.com/remote.php/dav/files/you/"),
            Field::Region => t!("us-east-1"),
            Field::Host => t!("nas.local"),
            Field::Port => t!("22"),
            Field::Key => t!("~/.ssh/id_ed25519"),
            _ => SharedString::default(),
        }
    }

    /// The box of a field, for a test to type into.
    pub fn input(&self, field: Field) -> &Entity<InputState> {
        &self.boxes[field as usize]
    }

    /// The embedded rclone steps, while "A cloud" is chosen for a new connection.
    pub fn cloud(&self) -> Option<&Entity<super::cloud_setup::CloudSetup>> {
        self.cloud.as_ref()
    }

    /// Stores a saved connection, tells core, closes the dialog, and opens a new one's folder.
    fn store(&mut self, window: &mut Window, cx: &mut Context<Self>, made: RemoteConnection, new: bool) {
        let path = glint_model::paths::REMOTE.to_string() + &made.id;
        persistence::update_app(cx, |app| app.save_remote(made));
        push_to_core(cx);
        window.close_dialog(cx);
        if new {
            if let Some(tabs) = explorer::try_explorer(cx) {
                tabs.update(cx, |e, cx| e.navigate(path, explorer::NavOptions::default(), cx));
            }
        }
    }

    /// Makes or drops the embedded rclone steps to match the kind chosen.
    fn sync_cloud(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use super::cloud_setup::{adopt, CloudSetup, Setup};
        let wanted = {
            let form = self.form.read(cx);
            form.editing().is_none() && form.kind() == RemoteKind::Rclone
        };
        match (wanted, self.cloud.is_some(), self.cloud_backend.clone()) {
            (true, false, Some(backend)) => {
                let connections = persistence::app(cx).map(|a| a.remotes.clone()).unwrap_or_default();
                let model = cx.new(|cx| Setup::new(backend, connections, cx));
                let tools = self.tools.clone();
                self.cloud = Some(cx.new(|cx| {
                    CloudSetup::new(
                        model,
                        tools,
                        |made, window, cx| adopt(window, cx, made),
                        |window, cx| window.close_dialog(cx),
                        window,
                        cx,
                    )
                }));
            }
            (false, true, _) => self.cloud = None,
            _ => {}
        }
    }

    fn cloud_body(&self) -> Option<AnyElement> {
        self.cloud.as_ref().map(|c| c.clone().into_any_element())
    }

    fn kind_chosen(&mut self, kind: RemoteKind, window: &mut Window, cx: &mut Context<Self>) {
        self.form.update(cx, |form, cx| form.set_kind(kind, cx));
        self.sync_cloud(window, cx);
    }

    /// The four kinds, as radio buttons in two columns (`kinds`).
    fn kind_picker(&self, current: RemoteKind, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        row_start()
            .id("remote-kind")
            .role(Role::RadioGroup)
            .aria_label(t!("Kind"))
            .test_support()
            .flex_wrap()
            .gap(space(1.5))
            .children(
                kinds()
                    .into_iter()
                    .enumerate()
                    .map(|(i, (kind, label, hint, kind_icon))| {
                        let on = kind == current;
                        Button::new(("remote-kind", i))
                            .outline()
                            .selected(on)
                            .toggled(on)
                            .h_auto()
                            .w(relative(0.49))
                            .py(space(1.5))
                            .justify_start()
                            .accessibility_label(format!("{label}: {hint}"))
                            .on_click(cx.listener(move |this, _, window, cx| this.kind_chosen(kind, window, cx)))
                            .child(
                                align_row(2.)
                                    .items_start()
                                    .child(
                                        icon(kind_icon)
                                            .size_4()
                                            .flex_none()
                                            .mt(space(0.5))
                                            .text_color(t.primary),
                                    )
                                    .child(
                                        col()
                                            .min_w_0()
                                            .align_start()
                                            .text_start()
                                            .child(div().font_weight(FontWeight::MEDIUM).child(label))
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(theme::size::XXS)
                                                    .text_color(t.muted_foreground)
                                                    .child(hint),
                                            ),
                                    ),
                            )
                    }),
            )
    }

    fn text_box(&self, field: Field, label: SharedString) -> Input {
        Input::new(self.input(field)).small().aria_label(label)
    }
}

impl Render for RemoteDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let form = self.form.read(cx);
        let kind = form.kind();
        let editing = form.editing().cloned();
        let ready = form.ready();
        let test = form.test().cloned();
        let host_keys = form.host_keys().cloned();
        let thumbnails = form.thumbnails();
        let title = match &editing {
            Some(r) => t!("Edit “{name}”", name = r.name),
            None => t!("Add a remote connection"),
        };
        let adding_cloud = editing.is_none() && kind == RemoteKind::Rclone;

        // A new cloud: rclone's own steps, in a part of their own.
        if adding_cloud {
            let steps = self.cloud_body().unwrap_or_else(|| {
                div()
                    .p(space(4.))
                    .text_color(t.muted_foreground)
                    .child(t!("Connecting a cloud isn't part of this build."))
                    .into_any_element()
            });
            return col()
                .id("remote")
                .role(Role::Group)
                .aria_label(title)
                .test_support()
                .text_start()
                .text_size(theme::size::SM)
                .child(
                    col()
                        .gap(space(3.))
                        .px(space(4.))
                        .pt(space(1.))
                        .child(
                            div()
                                .id("remote-subtitle")
                                .role(Role::Group)
                                .aria_label(t!("Through rclone: sign in once, then open it like a folder"))
                                .test_support()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(t!("Through rclone: sign in once, then open it like a folder")),
                        )
                        .child(self.kind_picker(kind, &t, cx)),
                )
                .child(steps);
        }

        let s3 = kind == RemoteKind::S3;
        let sftp = kind == RemoteKind::Sftp;
        let rclone = kind == RemoteKind::Rclone;

        let mut fields = col().gap(space(3.));
        if editing.is_none() {
            fields = fields.child(self.kind_picker(kind, &t, cx));
        }
        fields = fields.child(field(t!("Name"), None, self.text_box(Field::Name, t!("Name")), &t));
        if rclone {
            let (remote, service) = editing
                .as_ref()
                .map(|r| {
                    (
                        r.rclone.clone().unwrap_or_default(),
                        r.service.clone().unwrap_or_default(),
                    )
                })
                .unwrap_or_default();
            fields = fields
                .child(field(
                    t!("In rclone"),
                    Some(t!(
                        "The remote it opens, in rclone's configuration: rclone keeps its sign-in."
                    )),
                    align_row(2.)
                        .id("remote-in-rclone")
                        .role(Role::Group)
                        .aria_label(format!("{remote} {}", clouds::service_name(&service)))
                        .test_support()
                        .h(space(8.))
                        .child(icon(service_icon(&service)).size_4().flex_none().text_color(t.primary))
                        .child(div().font_weight(FontWeight::MEDIUM).child(isolate(remote)))
                        .child(
                            div()
                                .text_color(t.muted_foreground)
                                .child(clouds::service_name(&service)),
                        ),
                    &t,
                ))
                .child(field(
                    t!("Folder on it"),
                    Some(t!("Where it opens; empty for its top.")),
                    self.text_box(Field::Root, t!("Folder on it")),
                    &t,
                ));
        } else if s3 {
            fields = fields
                .child(field(
                    t!("Address"),
                    Some(t!("Empty for Amazon S3. R2: https://<account>.r2.cloudflarestorage.com · B2: https://s3.<region>.backblazeb2.com")),
                    self.text_box(Field::Endpoint, t!("Address")),
                    &t,
                ))
                .child(field(
                    t!("Region"),
                    Some(t!("R2 takes “auto”.")),
                    self.text_box(Field::Region, t!("Region")),
                    &t,
                ))
                .child(field(t!("Bucket"), None, self.text_box(Field::Bucket, t!("Bucket")), &t))
                .child(field(
                    t!("Access key ID"),
                    Some(t!("Empty for a public bucket.")),
                    self.text_box(Field::User, t!("Access key ID")),
                    &t,
                ))
                .child(field(
                    t!("Secret access key"),
                    None,
                    self.text_box(Field::Secret, t!("Secret access key")),
                    &t,
                ))
                .child(field(
                    t!("Folder in it"),
                    Some(t!("Where it opens; empty for the bucket's top.")),
                    self.text_box(Field::Root, t!("Folder in it")),
                    &t,
                ));
        } else if sftp {
            fields = fields
                .child(field(
                    t!("Server"),
                    Some(t!("Its name or address; a name ~/.ssh/config knows will do.")),
                    self.text_box(Field::Host, t!("Server")),
                    &t,
                ))
                .child(field(t!("Port"), None, self.text_box(Field::Port, t!("Port")), &t))
                .child(field(t!("User name"), None, self.text_box(Field::User, t!("User name")), &t))
                .child(field(
                    t!("Password"),
                    Some(t!("Or a key's passphrase. Empty to sign in as ssh does: with your SSH agent, or your keys in ~/.ssh.")),
                    self.text_box(Field::Secret, t!("Password")),
                    &t,
                ))
                .child(field(
                    t!("Key file"),
                    Some(t!("Empty for ssh's own choice.")),
                    self.text_box(Field::Key, t!("Key file")),
                    &t,
                ))
                .child(field(
                    t!("Folder on it"),
                    Some(t!("Where it opens: below the folder it signs in to, or from / when it starts with one.")),
                    self.text_box(Field::Root, t!("Folder on it")),
                    &t,
                ));
        } else {
            fields = fields
                .child(field(
                    t!("Address"),
                    None,
                    self.text_box(Field::Endpoint, t!("Address")),
                    &t,
                ))
                .child(field(
                    t!("User name"),
                    Some(t!("Empty for a server that doesn't ask.")),
                    self.text_box(Field::User, t!("User name")),
                    &t,
                ))
                .child(field(
                    t!("Password"),
                    None,
                    self.text_box(Field::Secret, t!("Password")),
                    &t,
                ))
                .child(field(
                    t!("Folder on it"),
                    Some(t!("Where it opens; empty for the address's own.")),
                    self.text_box(Field::Root, t!("Folder on it")),
                    &t,
                ));
        }

        let thumbnail_note = if s3 {
            t!("Downloads each picture shown (up to 16 MB), which a paid bucket charges for.")
        } else {
            t!("Downloads each picture shown (up to 16 MB).")
        };
        fields = fields.child(
            col()
                .gap(space(0.5))
                .ps(31.)
                .child(
                    Checkbox::new("remote-thumbnails")
                        .label(t!("Thumbnails of pictures in the grid"))
                        .checked(thumbnails)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            let on = *checked;
                            this.form.update(cx, |form, cx| form.set_thumbnails(on, cx));
                        })),
                )
                .child(
                    div()
                        .id("remote-thumbnails-note")
                        .role(Role::Group)
                        .aria_label(thumbnail_note.clone())
                        .test_support()
                        .ps(6.)
                        .text_size(theme::size::XXS)
                        .text_color(t.muted_foreground)
                        .child(thumbnail_note),
                ),
        );

        if let Some(keys) = host_keys {
            fields = fields.child(
                col()
                    .id("remote-host-keys")
                    .role(Role::Group)
                    .aria_label(t!("Glint hasn't seen this server before. Its host key:"))
                    .test_support()
                    .gap(space(2.))
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(t.warning.opacity(0.4))
                    .bg(t.warning.opacity(0.1))
                    .p(space(3.))
                    .text_size(theme::size::XS)
                    .child(
                        align_row(2.)
                            .items_start()
                            .font_weight(FontWeight::MEDIUM)
                            .child(icon(IconName::KeyRound).size_3p5().flex_none().mt(space(0.5)).text_color(t.warning))
                            .child(t!("Glint hasn't seen this server before. Its host key:")),
                    )
                    .child(
                        col()
                            .gap(space(0.5))
                            .ps(5.)
                            .font_family("Cascadia Mono")
                            .children(keys.fingerprints.iter().map(|f| div().child(f.clone()))),
                    )
                    .child(
                        div()
                            .text_color(t.muted_foreground)
                            .child(t!("Trust it only if that's the server's: whoever runs it can tell you (on the server, ssh-keygen -lf on its key). Glint keeps it in a list of its own, and won't connect if it ever changes.")),
                    )
                    .child(
                        div().child(
                            Button::new("remote-trust")
                                .outline()
                                .small()
                                .icon(IconName::KeyRound)
                                .label(t!("Trust it and test again"))
                                .on_click(cx.listener(|this, _, _, cx| this.form.update(cx, |form, cx| form.trust(cx)))),
                        ),
                    ),
            );
        }

        if let Some(test) = &test {
            let (line, tone, mark): (SharedString, Hsla, AnyElement) = match test {
                Test::Running => (t!("Connecting…"), t.muted_foreground, Spinner::new().into_any_element()),
                Test::Failed(reason) => (
                    reason.clone().into(),
                    t.destructive,
                    icon(IconName::TriangleAlert).size_3p5().into_any_element(),
                ),
                Test::Ok(said) => (
                    said_text(said),
                    t.muted_foreground,
                    icon(IconName::CircleCheck)
                        .size_3p5()
                        .text_color(t.success)
                        .into_any_element(),
                ),
            };
            fields = fields.child(
                align_row(2.)
                    .id("remote-test")
                    .role(if matches!(test, Test::Failed(_)) {
                        Role::Alert
                    } else {
                        Role::Status
                    })
                    .aria_label(line.clone())
                    .test_support()
                    .items_start()
                    .text_size(theme::size::XS)
                    .text_color(tone)
                    .child(div().flex_none().mt(space(0.5)).child(mark))
                    .child(div().flex_1().min_w_0().child(line)),
            );
        }

        let safe: SharedString = if rclone {
            t!("Glint only reads it: nothing there is changed. Its sign-in stays with rclone, never in Glint's settings.")
        } else if s3 {
            t!("Glint only reads it: nothing there is changed. The secret access key stays in Windows Credential Manager, never in Glint's settings.")
        } else if sftp {
            t!("Glint only reads it: nothing there is changed. The password stays in Windows Credential Manager, never in Glint's settings, and reaches ssh (Windows' own) only when it asks.")
        } else {
            t!("Glint only reads it: nothing there is changed. The password stays in Windows Credential Manager, never in Glint's settings.")
        };
        fields = fields.child(
            div()
                .id("remote-safe")
                .role(Role::Group)
                .aria_label(safe.clone())
                .test_support()
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(safe),
        );

        let running = matches!(test, Some(Test::Running));
        let mut footer = align_row(2.)
            .px(space(4.))
            .py(space(3.))
            .border_t_1()
            .border_color(t.border)
            .child(
                Button::new("remote-test-button")
                    .outline()
                    .small()
                    .icon(IconName::PlugZap)
                    .label(t!("Test"))
                    .disabled(!ready || running)
                    .on_click(cx.listener(|this, _, _, cx| this.form.update(cx, |form, cx| form.run_test(cx)))),
            );
        if let Some(editing) = &editing {
            let id = editing.id.clone();
            footer = footer.child(
                Button::new("remote-remove")
                    .ghost()
                    .small()
                    .danger()
                    .icon(IconName::Trash)
                    .label(t!("Remove"))
                    .on_click(move |_, window, cx| {
                        // Gone: the form closes too.
                        remove_remote_then(window, cx, id.clone(), |window, cx| window.close_dialog(cx));
                    }),
            );
        }
        footer = footer
            .child(div().flex_1())
            .child(
                Button::new("remote-cancel")
                    .ghost()
                    .small()
                    .label(t!("Cancel"))
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
            .child(
                Button::new("remote-save")
                    .primary()
                    .small()
                    .label(if editing.is_some() {
                        t!("Save")
                    } else {
                        t!("Add and open")
                    })
                    .disabled(!ready)
                    .on_click(cx.listener(|this, _, _, cx| this.form.update(cx, |form, cx| form.save(cx)))),
            );

        col()
            .id("remote")
            .role(Role::Group)
            .aria_label(title)
            .test_support()
            .text_start()
            .text_size(theme::size::SM)
            .child(
                div()
                    .id("remote-body")
                    .max_h(rems(34.))
                    .overflow_y_scroll()
                    .px(space(4.))
                    .pt(space(1.))
                    .pb(space(4.))
                    .child(
                        col()
                            .gap(space(3.))
                            .child(
                                div()
                                    .id("remote-subtitle")
                                    .role(Role::Group)
                                    .aria_label(t!("Its folders open read-only; what you open or copy is downloaded"))
                                    .test_support()
                                    .text_size(theme::size::XS)
                                    .text_color(t.muted_foreground)
                                    .child(t!("Its folders open read-only; what you open or copy is downloaded")),
                            )
                            .child(fields),
                    ),
            )
            .child(footer)
    }
}

// ---- Opening ----------------------------------------------------------------------------------------------------

/// Opens the dialog that adds a connection (`addRemote`): it starts on "A cloud".
pub fn open_add(window: &mut Window, cx: &mut App) {
    let backend = backend(cx);
    open_with(window, cx, backend, None);
}

/// Opens the dialog that edits the connection `id` (`editRemote`). Nothing opens for an id that isn't saved, or
/// for a paired PC's folders, which have nothing to edit.
pub fn open_edit(window: &mut Window, cx: &mut App, id: String) {
    let Some(remote) = persistence::app(cx).and_then(|app| app.remotes.iter().find(|r| r.id == id).cloned()) else {
        return;
    };
    if remote.kind == RemoteKind::Device {
        return;
    }
    let backend = backend(cx);
    open_with(window, cx, backend, Some(remote));
}

/// [`open_add`] / [`open_edit`] with a backend of your own (tests pass a fake). Returns the form and the view.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    editing: Option<RemoteConnection>,
) -> (Entity<Form>, Entity<RemoteDialog>) {
    let cloud = cx.try_global::<core_bridge::Core>().map(|core| {
        Arc::new(super::cloud_setup::CoreBackend::new(core.app.clone())) as Arc<dyn super::cloud_setup::Backend>
    });
    let tools = if editing.is_none() {
        cloud_tools(window, cx)
    } else {
        None
    };
    open_full(window, cx, backend, editing, cloud, tools)
}

/// The ids of the connections there are.
fn taken_ids(cx: &App) -> Vec<String> {
    persistence::app(cx)
        .map(|app| app.remotes.iter().map(|r| r.id.clone()).collect())
        .unwrap_or_default()
}

/// The dialog's title.
fn title_of(editing: Option<&RemoteConnection>) -> SharedString {
    match editing {
        Some(r) => t!("Edit “{name}”", name = r.name),
        None => t!("Add a remote connection"),
    }
}

/// The tools model the cloud steps' install card reads (none without core).
fn cloud_tools(window: &mut Window, cx: &mut App) -> Option<Entity<crate::components::settings::tools::Tools>> {
    use crate::components::settings::{Backend as SettingsBackend, CoreBackend as SettingsCoreBackend};
    let core = cx.try_global::<core_bridge::Core>()?;
    let backend: Arc<dyn SettingsBackend> = Arc::new(SettingsCoreBackend::new(core.app.clone()));
    Some(cx.new(|cx| crate::components::settings::tools::Tools::new(backend, window, cx)))
}

/// [`open_with`] with the cloud steps' backend and tools of your own.
pub fn open_full(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    editing: Option<RemoteConnection>,
    cloud: Option<Arc<dyn super::cloud_setup::Backend>>,
    tools: Option<Entity<crate::components::settings::tools::Tools>>,
) -> (Entity<Form>, Entity<RemoteDialog>) {
    let taken = taken_ids(cx);
    let title = title_of(editing.as_ref());
    let form = cx.new(|cx| Form::new(backend, editing, taken, cx));
    let view = cx.new(|cx| RemoteDialog::new(form.clone(), cloud, tools, window, cx));
    super::show(window, cx, title, px(576.), view.clone());
    (form, view)
}

// ---- Tests ------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct Log {
        calls: Vec<String>,
        test: Vec<Result<String, String>>,
        kept: bool,
        keys: Option<Result<HostKeys, String>>,
        secret_fails: bool,
        delete_fails: bool,
    }

    struct Fake(Mutex<Log>);

    impl Fake {
        fn new() -> Arc<Fake> {
            Arc::new(Fake(Mutex::new(Log {
                test: vec![Ok("Connected: 3 items at its top".into())],
                ..Default::default()
            })))
        }

        fn with(&self, change: impl FnOnce(&mut Log)) {
            change(&mut self.0.lock().unwrap());
        }

        fn calls(&self) -> Vec<String> {
            self.0.lock().unwrap().calls.clone()
        }

        fn log(&self, what: String) {
            self.0.lock().unwrap().calls.push(what);
        }
    }

    impl Backend for Fake {
        fn test(&self, connection: RemoteConnection, secret: Option<String>) -> Fut<String> {
            self.log(format!(
                "test {} {:?} {} {}",
                connection.id,
                connection.kind,
                connection.host.clone().unwrap_or_default(),
                match &secret {
                    Some(s) => format!("secret={s}"),
                    None => "kept".to_string(),
                }
            ));
            let mut log = self.0.lock().unwrap();
            let answer = if log.test.len() > 1 {
                log.test.remove(0)
            } else {
                log.test[0].clone()
            };
            Box::pin(async move { answer })
        }

        fn host_keys(&self, host: String, port: u16) -> Fut<HostKeys> {
            self.log(format!("keys {host} {port}"));
            let answer = self.0.lock().unwrap().keys.clone().unwrap_or(Err("no keys".into()));
            Box::pin(async move { answer })
        }

        fn trust_host(&self, keys: String) -> Fut<()> {
            self.log(format!("trust {keys}"));
            Box::pin(async { Ok(()) })
        }

        fn secret_saved(&self, _: String) -> Fut<bool> {
            let kept = self.0.lock().unwrap().kept;
            Box::pin(async move { Ok(kept) })
        }

        fn secret_set(&self, id: String, secret: String) -> Fut<()> {
            self.log(format!("secret-set {id} {secret}"));
            let fails = self.0.lock().unwrap().secret_fails;
            Box::pin(async move {
                if fails {
                    Err("Credential Manager said no".into())
                } else {
                    Ok(())
                }
            })
        }

        fn secret_delete(&self, id: String) -> Fut<()> {
            self.log(format!("secret-delete {id}"));
            Box::pin(async { Ok(()) })
        }

        fn rclone_delete(&self, name: String) -> Fut<()> {
            self.log(format!("rclone-delete {name}"));
            let fails = self.0.lock().unwrap().delete_fails;
            Box::pin(async move {
                if fails {
                    Err("rclone is busy".into())
                } else {
                    Ok(())
                }
            })
        }
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn saved(id: &str, name: &str, kind: RemoteKind) -> RemoteConnection {
        RemoteConnection {
            id: id.into(),
            name: name.into(),
            kind,
            ..Default::default()
        }
    }

    /// An app with a saved state in a temporary folder (holding `remotes`), and a window.
    fn window(cx: &mut TestAppContext, remotes: Vec<RemoteConnection>) -> VisualTestContext {
        cx.update(|cx| {
            testing::init(cx);
            let dir = std::env::temp_dir().join(format!(
                "glint-remote-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join("state.json");
            let _ = std::fs::remove_file(&file);
            persistence::init_at(cx, file);
            persistence::update_app(cx, |app| app.remotes = remotes);
        });
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    fn stored(cx: &mut VisualTestContext) -> Vec<RemoteConnection> {
        cx.update(|_, cx| persistence::app(cx).map(|a| a.remotes.clone()).unwrap_or_default())
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn settle(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("{what} never happened");
    }

    fn type_into(cx: &mut VisualTestContext, input: &Entity<InputState>, text: &str) {
        let input = input.clone();
        let text = text.to_string();
        cx.update(|window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
            window.press("ctrl-a", cx);
            window.input(&text, cx);
        });
        cx.run_until_parked();
    }

    fn click(cx: &mut VisualTestContext, id: &'static str) {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        cx.run_until_parked();
    }

    /// Opens the dialog without core's cloud steps (they would look at this PC's rclone).
    fn open_for_test(
        window: &mut Window,
        cx: &mut App,
        backend: Arc<dyn Backend>,
        editing: Option<RemoteConnection>,
    ) -> (Entity<Form>, Entity<RemoteDialog>) {
        open_full(window, cx, backend, editing, None, None)
    }

    fn open(
        cx: &mut VisualTestContext,
        fake: &Arc<Fake>,
        editing: Option<RemoteConnection>,
    ) -> (Entity<Form>, Entity<RemoteDialog>) {
        let backend: Arc<dyn Backend> = fake.clone();
        let opened = cx.update(|window, cx| {
            init_with(cx, backend.clone());
            open_for_test(window, cx, backend, editing)
        });
        cx.run_until_parked();
        opened
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Cloud,
            IconName::Database,
            IconName::Server,
            IconName::SquareTerminal,
            IconName::PlugZap,
            IconName::Trash,
            IconName::KeyRound,
            IconName::CircleCheck,
            IconName::TriangleAlert,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[gpui_kit::test]
    fn a_new_s3_connection_needs_a_name_and_a_bucket_tests_with_its_secret_and_saves(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx, vec![saved("backups", "Backups", RemoteKind::S3)]);
        let fake = Fake::new();
        let (form, view) = open(&mut visual, &fake, None);
        // It starts on "A cloud"; S3 is the second kind.
        assert_eq!(role_of(&mut visual, "Add a remote connection"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Kind"), Some(Role::RadioGroup));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("remote-kind", 1_usize), cx);
        });
        assert_eq!(form.read_with(&visual, |f, _| f.kind()), RemoteKind::S3);
        assert!(has(&mut visual, "Empty for Amazon S3. R2: https://<account>.r2.cloudflarestorage.com · B2: https://s3.<region>.backblazeb2.com"));
        assert!(!form.read_with(&visual, |f, _| f.ready()));
        let (name, bucket, secret) = view.read_with(&visual, |v, _| {
            (
                v.input(Field::Name).clone(),
                v.input(Field::Bucket).clone(),
                v.input(Field::Secret).clone(),
            )
        });
        type_into(&mut visual, &name, "Backups");
        assert!(!form.read_with(&visual, |f, _| f.ready()), "a bucket is needed too");
        type_into(&mut visual, &bucket, " photos ");
        type_into(&mut visual, &secret, "s3cret");
        assert!(form.read_with(&visual, |f, _| f.ready()));
        // Test: the connection as typed (trimmed, its id not one that is taken), with the secret typed.
        click(&mut visual, "remote-test-button");
        settle(&mut visual, "the test", |cx| {
            form.read_with(cx, |f, _| matches!(f.test(), Some(Test::Ok(_))))
        });
        assert_eq!(fake.calls(), ["test backups-2 S3  secret=s3cret"]);
        assert_eq!(
            role_of(&mut visual, "Connected: 3 items at its top"),
            Some(Role::Status)
        );
        // Editing anything forgets the result.
        type_into(&mut visual, &bucket, "photos2");
        assert!(form.read_with(&visual, |f, _| f.test().is_none()));
        // Add and open: the secret is kept first, then the connection is stored.
        click(&mut visual, "remote-save");
        settle(&mut visual, "the save", |cx| stored(cx).len() == 2);
        let made = stored(&mut visual).remove(1);
        assert_eq!(
            (made.id.as_str(), made.name.as_str(), made.bucket.as_str()),
            ("backups-2", "Backups", "photos2")
        );
        assert_eq!(fake.calls()[1], "secret-set backups-2 s3cret");
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "it closes"));
    }

    #[gpui_kit::test]
    fn editing_keeps_the_secret_unless_a_new_one_is_typed_and_the_id_through_a_rename(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut dav = saved("nas", "My NAS", RemoteKind::Webdav);
        dav.endpoint = "https://nas.local/dav/".into();
        let mut visual = window(cx, vec![dav.clone()]);
        let fake = Fake::new();
        fake.with(|l| l.kept = true);
        let (form, view) = open(&mut visual, &fake, Some(dav));
        settle(&mut visual, "the kept secret", |cx| form.read_with(cx, |f, _| f.kept()));
        assert_eq!(role_of(&mut visual, "Edit “My NAS”"), Some(Role::Group));
        // The secret box says so in its placeholder (set from the form once it knows).
        assert_eq!(
            form.read_with(&visual, |f, _| RemoteDialog::placeholder(Field::Secret, f)),
            "Kept in Credential Manager: type to change it"
        );
        // The kind can't be changed while editing.
        assert!(!has(&mut visual, "Kind"));
        click(&mut visual, "remote-test-button");
        settle(&mut visual, "the test", |cx| {
            form.read_with(cx, |f, _| f.test().is_some_and(|t| *t != Test::Running))
        });
        assert_eq!(fake.calls(), ["test nas Webdav  kept"]);
        let name = view.read_with(&visual, |v, _| v.input(Field::Name).clone());
        type_into(&mut visual, &name, "Home NAS");
        click(&mut visual, "remote-save");
        settle(&mut visual, "the save", |cx| stored(cx)[0].name == "Home NAS");
        // Same id, no secret written (none was typed).
        assert_eq!(stored(&mut visual).len(), 1);
        assert_eq!(stored(&mut visual)[0].id, "nas");
        assert!(!fake.calls().iter().any(|c| c.starts_with("secret-set")));
    }

    #[gpui_kit::test]
    fn a_secret_that_cannot_be_kept_saves_nothing(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx, vec![]);
        let fake = Fake::new();
        fake.with(|l| l.secret_fails = true);
        let (_, view) = open(&mut visual, &fake, None);
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("remote-kind", 2_usize), cx);
        });
        let (name, endpoint, secret) = view.read_with(&visual, |v, _| {
            (
                v.input(Field::Name).clone(),
                v.input(Field::Endpoint).clone(),
                v.input(Field::Secret).clone(),
            )
        });
        type_into(&mut visual, &name, "NAS");
        type_into(&mut visual, &endpoint, "https://nas/dav/");
        type_into(&mut visual, &secret, "pw");
        click(&mut visual, "remote-save");
        settle(&mut visual, "the attempt", |_| fake.calls().len() == 1);
        visual.run_until_parked();
        assert!(stored(&mut visual).is_empty());
        visual.update(|window, cx| assert!(window.has_active_dialog(cx), "the form stays"));
    }

    #[gpui_kit::test]
    fn an_sftp_server_not_trusted_yet_shows_its_fingerprints_and_trusting_tests_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx, vec![]);
        let fake = Fake::new();
        fake.with(|l| {
            l.test = vec![
                Err(format!("{UNKNOWN_HOST}: Edit the connection and Test it")),
                Ok("Connected: 1 item at its top".into()),
            ];
            l.keys = Some(Ok(HostKeys {
                fingerprints: vec!["256 SHA256:abc nas (ED25519)".into()],
                keys: "nas ssh-ed25519 AAAA".into(),
            }));
        });
        let (form, view) = open(&mut visual, &fake, None);
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("remote-kind", 3_usize), cx);
        });
        let (name, host, port) = view.read_with(&visual, |v, _| {
            (
                v.input(Field::Name).clone(),
                v.input(Field::Host).clone(),
                v.input(Field::Port).clone(),
            )
        });
        type_into(&mut visual, &name, "NAS");
        type_into(&mut visual, &host, " nas.local ");
        // The port keeps digits only.
        type_into(&mut visual, &port, "2a2b");
        assert_eq!(port.read_with(&visual, |p, _| p.value().to_string()), "22");
        click(&mut visual, "remote-test-button");
        settle(&mut visual, "the host keys", |cx| {
            form.read_with(cx, |f, _| f.host_keys().is_some())
        });
        assert_eq!(fake.calls()[1], "keys nas.local 22");
        assert!(
            has(&mut visual, "256 SHA256:abc nas (ED25519)")
                || has(&mut visual, "Glint hasn't seen this server before. Its host key:")
        );
        // The button is below the fold of the scrolling form; it does what Trust does.
        form.update_in(&mut visual, |form, _, cx| form.trust(cx));
        settle(&mut visual, "the second test", |cx| {
            form.read_with(cx, |f, _| matches!(f.test(), Some(Test::Ok(_))))
        });
        assert_eq!(fake.calls()[2], "trust nas ssh-ed25519 AAAA");
        assert!(fake.calls()[3].starts_with("test nas Sftp nas.local"));
        assert!(
            form.read_with(&visual, |f, _| f.host_keys().is_none()),
            "the keys go once trusted"
        );
        assert_eq!(form.read_with(&visual, |f, _| f.connection().port), Some(22));
    }

    #[gpui_kit::test]
    fn a_failed_test_is_an_alert(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx, vec![]);
        let fake = Fake::new();
        fake.with(|l| l.test = vec![Err("403 Forbidden".into())]);
        let (_, view) = open(&mut visual, &fake, None);
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("remote-kind", 1_usize), cx);
        });
        let (name, bucket) = view.read_with(&visual, |v, _| {
            (v.input(Field::Name).clone(), v.input(Field::Bucket).clone())
        });
        type_into(&mut visual, &name, "X");
        type_into(&mut visual, &bucket, "b");
        click(&mut visual, "remote-test-button");
        settle(&mut visual, "the failure", |cx| has(cx, "403 Forbidden"));
        assert_eq!(role_of(&mut visual, "403 Forbidden"), Some(Role::Alert));
    }

    #[gpui_kit::test]
    fn removing_asks_first_signs_out_if_asked_and_closes_the_form(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut cloud = saved("drive", "Work drive", RemoteKind::Rclone);
        cloud.rclone = Some("gdrive".into());
        cloud.service = Some("drive".into());
        let mut visual = window(cx, vec![cloud.clone(), saved("other", "Other", RemoteKind::S3)]);
        let fake = Fake::new();
        let (_, _) = open(&mut visual, &fake, Some(cloud));
        assert!(has(&mut visual, "gdrive Google Drive"));
        click(&mut visual, "remote-remove");
        assert_eq!(role_of(&mut visual, "Remove the connection"), Some(Role::Group));
        // Nothing is removed until it is confirmed.
        assert_eq!(stored(&mut visual).len(), 2);
        click(&mut visual, "confirm-ok");
        assert_eq!(
            role_of(
                &mut visual,
                "Sign out of it as well? “gdrive” goes from rclone's configuration, with its sign-in, so rclone in a terminal loses it too."
            ),
            Some(Role::Group)
        );
        click(&mut visual, "remove-sign-out");
        settle(&mut visual, "the removal", |cx| stored(cx).len() == 1);
        assert_eq!(stored(&mut visual)[0].id, "other");
        assert_eq!(fake.calls(), ["rclone-delete gdrive", "secret-delete drive"]);
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "the form closed with it"));
    }

    #[gpui_kit::test]
    fn keeping_it_in_rclone_and_escape_leave_rclones_configuration_alone(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut cloud = saved("drive", "Work drive", RemoteKind::Rclone);
        cloud.rclone = Some("gdrive".into());
        let mut visual = window(cx, vec![cloud, saved("nas", "NAS", RemoteKind::Webdav)]);
        let fake = Fake::new();
        let backend: Arc<dyn Backend> = fake.clone();
        visual.update(|window, cx| remove_with(window, cx, backend.clone(), "drive".into(), Rc::new(|_, _| {})));
        click(&mut visual, "confirm-ok");
        click(&mut visual, "remove-keep");
        settle(&mut visual, "the removal", |cx| stored(cx).len() == 1);
        assert_eq!(fake.calls(), ["secret-delete drive"]);
        // A connection that is not rclone's has no sign-out to ask about, and Escape on the sign-out asks nothing.
        visual.update(|window, cx| remove_with(window, cx, backend, "nas".into(), Rc::new(|_, _| {})));
        click(&mut visual, "confirm-ok");
        settle(&mut visual, "the second removal", |cx| stored(cx).is_empty());
        assert_eq!(fake.calls(), ["secret-delete drive", "secret-delete nas"]);
    }

    #[gpui_kit::test]
    fn cancelling_the_confirmation_removes_nothing(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx, vec![saved("nas", "NAS", RemoteKind::Webdav)]);
        let fake = Fake::new();
        let backend: Arc<dyn Backend> = fake.clone();
        visual.update(|window, cx| remove_with(window, cx, backend, "nas".into(), Rc::new(|_, _| {})));
        click(&mut visual, "confirm-cancel");
        visual.run_until_parked();
        assert_eq!(stored(&mut visual).len(), 1);
        assert!(fake.calls().is_empty());
    }

    #[gpui_kit::test]
    fn escape_on_the_sign_out_question_keeps_the_remote_in_rclone_and_still_removes_the_connection(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let mut cloud = saved("drive", "Work drive", RemoteKind::Rclone);
        cloud.rclone = Some("gdrive".into());
        let mut visual = window(cx, vec![cloud]);
        let fake = Fake::new();
        let backend: Arc<dyn Backend> = fake.clone();
        visual.update(|window, cx| remove_with(window, cx, backend, "drive".into(), Rc::new(|_, _| {})));
        click(&mut visual, "confirm-ok");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        settle(&mut visual, "the removal", |cx| stored(cx).is_empty());
        assert_eq!(fake.calls(), ["secret-delete drive"]);
    }

    #[gpui_kit::test]
    fn escape_closes_the_form_and_a_device_has_nothing_to_edit(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx, vec![saved("pc", "Laptop", RemoteKind::Device)]);
        let fake = Fake::new();
        visual.update(|window, cx| {
            init_with(cx, fake.clone());
            open_edit(window, cx, "pc".into());
            open_edit(window, cx, "missing".into());
            assert!(
                !window.has_active_dialog(cx),
                "nothing opens for a paired PC or an unknown id"
            );
        });
        let (_, _) = open(&mut visual, &fake, None);
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut dav = saved("nas", "جهاز التخزين", RemoteKind::Webdav);
        dav.endpoint = "https://nas.local/".into();
        let mut visual = window(cx, vec![dav.clone()]);
        testing::set_rtl(true);
        let fake = Fake::new();
        let (form, view) = open(&mut visual, &fake, Some(dav));
        assert_eq!(role_of(&mut visual, "Edit “جهاز التخزين”"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Name"), Some(Role::TextInput));
        let name = view.read_with(&visual, |v, _| v.input(Field::Name).clone());
        type_into(&mut visual, &name, "مجلد الصور");
        assert_eq!(form.read_with(&visual, |f, _| f.connection().name), "مجلد الصور");
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn the_cloud_kind_embeds_rclones_steps_for_a_new_connection(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx, vec![]);
        let fake = Fake::new();
        let (form, view) = open(&mut visual, &fake, None);
        assert_eq!(form.read_with(&visual, |f, _| f.kind()), RemoteKind::Rclone);
        // Without core's backend (the test passes none) no steps are made, and the note says so; with it they are.
        assert!(view.read_with(&visual, |v, _| v.cloud().is_none()));
        let _ = view;
        assert!(has(
            &mut visual,
            "Through rclone: sign in once, then open it like a folder"
        ));
    }

    #[test]
    fn what_remote_test_says_is_worded_in_the_interface_language() {
        let _guard = testing::lock();
        assert_eq!(
            said_text("Connected: 1 item at its top"),
            "Connected: 1 item at its top"
        );
        assert_eq!(
            said_text("Connected: 12 items at its top"),
            "Connected: 12 items at its top"
        );
        assert_eq!(said_text("something else"), "something else");
        assert!(matches!(service_icon("sftp"), IconName::Server));
    }

    #[test]
    fn a_port_past_65535_is_cut_so_core_still_reads_the_connection() {
        let mut c = saved("x", "X", RemoteKind::Sftp);
        c.host = Some("h".into());
        c.port = Some(70_000);
        assert_eq!(to_core(&c).port, 65_535);
        assert_eq!(to_core(&c).host, "h");
    }
}
