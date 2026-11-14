//! Connecting a cloud through rclone: rclone first (installed, unlocked), then a service (or a remote rclone
//! has already), its first settings, rclone's own questions one at a time, and a browser sign-in where the
//! service has one, until Glint can open it.
//! Ported from src/components/explorer/CloudSetup.tsx and the helpers of src/lib/rclone.ts it uses.
//!
//! | TypeScript                                        | Here                                                     |
//! |---------------------------------------------------|----------------------------------------------------------|
//! | `CloudSetup`'s state and steps                    | [`Setup`], a model entity with [`Phase`]                 |
//! | `CloudSetup`'s markup, `QuestionField`, `Working` | [`CloudSetup`], the view (embeddable, or in [`open`])    |
//! | `rclone*` invokes, `remoteTest`, `onSignInPage`   | [`Backend`] (a fake in the tests), [`CoreBackend`]       |
//! | `serviceIcon`, `connectionFor`                    | [`service_icon`], [`connection_for`]                     |
//! | `serviceName`, `POPULAR`, `offered`               | [`service_name`], [`POPULAR`], [`offered`]               |
//! | `remoteNameFor`, `forProvider`, `helpOf`          | [`remote_name_for`], [`for_provider`], [`help_of`]       |
//! | `Linked` (web addresses in rclone's help)         | [`split_links`]; the addresses are buttons under the text |
//! | `<ToolNeeded id="rclone" />`                      | `components::settings::tools::needed`                    |
//! | `<select>` and `<datalist>` of a question         | a list of radio buttons, and chips that fill the box     |
//!
//! **How the coordinator wires it.** Settings → Clouds → Connect (`SettingsEvent::ConnectCloud`) calls
//! `dialogs::cloud_setup::open(window, cx)`: the dialog saves the connection, opens it and closes itself. The
//! connection dialog (`remote.rs`) embeds the same view for "A cloud".
//!
//! **Cleaning up.** A remote that was made in rclone's configuration and not finished is taken out again when
//! the view goes (Cancel, Escape, "Other clouds"), as the TypeScript's effect cleanup did.
//!
//! Not as in the TypeScript: the web addresses in rclone's help are not inline links but buttons listed under
//! the text. The list of services (about 70) is virtualized: rows of two, a `uniform_list`.

use crate::components::settings::clouds;
use crate::components::settings::tools::{self, Tools};
use crate::components::settings::{Backend as SettingsBackend, CoreBackend as SettingsCoreBackend};
use crate::kit::prelude::*;
use crate::{core_bridge, explorer, persistence, t, theme};
use glint_core::host::Manager as _;
use glint_core::rclone::{Example, Provider, Question, Remote, Status, Step, LOCKED};
use glint_model::collate::compare_base_numeric;
use glint_model::store::RemoteConnection;
use gpui_kit::UniformListScrollHandle;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

/// A service's row in the list, in rems (a name and the line under it, fixed so the list can be virtualized).
const SERVICE_ROW: f32 = 3.;
/// How many rows of the list show before it scrolls.
const SERVICE_ROWS_SHOWN: usize = 6;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

// ---- What the TypeScript's lib/rclone.ts gave ---------------------------------------------------------------

/// The services most people look for first, in the picker's order.
pub const POPULAR: [&str; 16] = [
    "drive",
    "onedrive",
    "dropbox",
    "box",
    "pcloud",
    "mega",
    "protondrive",
    "iclouddrive",
    "s3",
    "b2",
    "azureblob",
    "google cloud storage",
    "sftp",
    "ftp",
    "smb",
    "webdav",
];

/// rclone's types that aren't a place of their own (they wrap or join others, or are for tests): not offered.
const WRAPPERS: [&str; 11] = [
    "alias",
    "archive",
    "cache",
    "chunker",
    "combine",
    "compress",
    "hasher",
    "local",
    "memory",
    "tardigrade",
    "union",
];

/// Whether a type is offered in the list of services (`offered`).
pub fn offered(provider: &Provider) -> bool {
    !WRAPPERS.contains(&provider.name.as_str())
}

/// A service's name: "Google Drive" for "drive", else rclone's description, else the type (`serviceName`).
pub fn service_name(kind: &str, providers: &[Provider]) -> SharedString {
    let named = clouds::service_name(kind);
    if named.as_ref() != kind {
        return named;
    }
    providers
        .iter()
        .find(|p| p.name == kind)
        .map_or_else(|| kind.to_string().into(), |p| p.description.clone().into())
}

/// A name for a new remote in rclone's configuration, from the one it's given: letters, digits, "-" and "_"
/// (`remoteNameFor`). `taken` are the names there already, compared in lower case.
///
/// NOTE(port): the TypeScript decomposed accents first (NFKD), so "é" became "e"; here an accented letter
/// becomes a dash, which only matters for a name that has one (the same as `clouds::remote_id`).
pub fn remote_name_for(name: &str, taken: &[String]) -> String {
    let mut base = String::new();
    for ch in name.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            base.push(ch);
        } else if ch == '-' || !base.ends_with('-') {
            base.push('-');
        }
    }
    let base: String = base.trim_matches('-').chars().take(32).collect();
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() { "cloud".to_string() } else { base };
    let used: Vec<String> = taken.iter().map(|t| t.to_lowercase()).collect();
    let mut candidate = base.clone();
    let mut n = 2;
    while used.contains(&candidate) {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    candidate
}

/// Whether a setting applies to an S3 service: `filter` is as rclone writes it ("AWS,Minio", "!AWS,Other", ""
/// for all), `provider` the service chosen (`forProvider`).
pub fn for_provider(filter: &str, provider: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let (negated, list) = match filter.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, filter),
    };
    let listed = list.split(',').any(|n| n.trim() == provider);
    listed != negated
}

/// A question's text: its first paragraph as the label, the rest as more (`helpOf`).
pub fn help_of(question: &Question) -> (String, String) {
    let text = question.help.trim();
    let cut = text.find("\n\n");
    let first = &text[..cut.unwrap_or(text.len())];
    // Lines joined with a space, the whitespace around each break gone.
    let label = first.split('\n').map(str::trim).collect::<Vec<_>>().join(" ");
    let label = if label.is_empty() { question.name.clone() } else { label };
    let more = cut.map_or_else(String::new, |at| text[at + 2..].trim().to_string());
    (label, more)
}

/// Text cut into plain parts and web addresses (`Linked`): `(is_link, text)`. An address ends at a space or ")".
pub fn split_links(text: &str) -> Vec<(bool, String)> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(at) = ["https://", "http://"].iter().filter_map(|p| rest.find(p)).min() {
        if at > 0 {
            parts.push((false, rest[..at].to_string()));
        }
        let tail = &rest[at..];
        let end = tail.find(|c: char| c.is_whitespace() || c == ')').unwrap_or(tail.len());
        parts.push((true, tail[..end].to_string()));
        rest = &tail[end..];
    }
    if !rest.is_empty() {
        parts.push((false, rest.to_string()));
    }
    parts
}

/// Services reached over a network protocol rather than a company's cloud: a server's icon.
const SERVERS: [&str; 7] = ["sftp", "ftp", "smb", "webdav", "http", "hdfs", "seafile"];

/// A service's icon (`serviceIcon`): a server for protocols, a cloud for companies' clouds.
pub fn service_icon(kind: &str) -> IconName {
    if SERVERS.contains(&kind) {
        IconName::Server
    } else {
        IconName::Cloud
    }
}

/// A connection to one of rclone's remotes, named `name` in Glint (`connectionFor`). `taken_ids` are the ids of
/// the connections there are.
pub fn connection_for(name: &str, remote: &str, kind: &str, taken_ids: &[String]) -> RemoteConnection {
    RemoteConnection {
        id: crate::components::settings::clouds::remote_id(name, taken_ids),
        name: name.to_string(),
        kind: glint_model::store::RemoteKind::Rclone,
        rclone: Some(remote.to_string()),
        service: Some(kind.to_string()),
        ..Default::default()
    }
}

/// core's connection for a saved one (the same JSON shape; the saved one types its choices).
fn to_core(connection: &RemoteConnection) -> glint_core::remote::Connection {
    serde_json::to_value(connection)
        .and_then(serde_json::from_value)
        .unwrap_or_default()
}

/// What `remote_test` says (core's English), in the interface language.
fn said_text(said: &str) -> SharedString {
    if said == "Connected: there's nothing there yet" {
        return t!("Connected: there's nothing there yet");
    }
    if said == "Connected: 1 item at its top" {
        return t!("Connected: 1 item at its top");
    }
    if let Some(n) = said
        .strip_prefix("Connected: ")
        .and_then(|rest| rest.strip_suffix(" items at its top"))
    {
        return t!("Connected: {n} items at its top", n = n);
    }
    said.to_string().into()
}

// ---- What the dialog asks of the PC -----------------------------------------------------------------------------

/// What setting a cloud up asks of the PC. [`CoreBackend`] is the real one; tests give a fake.
pub trait Backend: Send + Sync {
    /// rclone: installed, its version, its configuration, locked (`rclone_status`).
    fn status(&self) -> Fut<Status>;
    /// The kinds of storage rclone reaches (`rclone_providers`).
    fn providers(&self) -> Fut<Vec<Provider>>;
    /// The remotes in rclone's configuration (`rclone_remotes`).
    fn remotes(&self) -> Fut<Vec<Remote>>;
    /// Unlocks an encrypted configuration for this run (`rclone_unlock`).
    fn unlock(&self, password: String) -> Fut<()>;
    /// Starts making the remote (`rclone_config_start`).
    fn config_start(&self, name: String, kind: String, parameters: Map<String, Value>) -> Fut<Step>;
    /// Answers rclone's question at `state` (`rclone_config_answer`).
    fn config_answer(&self, name: String, state: String, answer: String) -> Fut<Step>;
    /// A browser sign-in as rclone's question (its `help`) says: the answer to give. The page's address comes
    /// as core's `RcloneSignIn` event.
    fn authorize(&self, help: String) -> Fut<String>;
    /// Stops a browser sign-in that waits (`rclone_authorize_cancel`).
    fn authorize_cancel(&self);
    /// Takes a remote out of rclone's configuration (`rclone_delete`).
    fn delete(&self, name: String) -> Fut<()>;
    /// Tries the connection as Glint will open it: what is at its top (`remote_test`).
    fn test(&self, connection: RemoteConnection) -> Fut<String>;
}

/// Waits for a core call and flattens the two layers of failure (the task panicking, the call failing).
fn flat<T: Send + 'static>(call: impl Future<Output = Result<T, String>> + Send + 'static) -> Fut<T> {
    let joined = core_bridge::run(call);
    Box::pin(async move { joined.await.and_then(|inner| inner) })
}

/// glint-core's rclone through the app's handle.
pub struct CoreBackend {
    app: glint_core::host::AppHandle,
}

impl CoreBackend {
    /// The backend over core's handle (from `core_bridge::Core`).
    pub fn new(app: glint_core::host::AppHandle) -> Self {
        Self { app }
    }
}

impl Backend for CoreBackend {
    fn status(&self) -> Fut<Status> {
        flat(glint_core::rclone::rclone_status())
    }

    fn providers(&self) -> Fut<Vec<Provider>> {
        flat(glint_core::rclone::rclone_providers())
    }

    fn remotes(&self) -> Fut<Vec<Remote>> {
        flat(glint_core::rclone::rclone_remotes())
    }

    fn unlock(&self, password: String) -> Fut<()> {
        flat(glint_core::rclone::rclone_unlock(password))
    }

    fn config_start(&self, name: String, kind: String, parameters: Map<String, Value>) -> Fut<Step> {
        flat(glint_core::rclone::rclone_config_start(name, kind, parameters))
    }

    fn config_answer(&self, name: String, state: String, answer: String) -> Fut<Step> {
        flat(glint_core::rclone::rclone_config_answer(name, state, answer))
    }

    fn authorize(&self, help: String) -> Fut<String> {
        let Some(window) = self.app.get_webview_window(crate::workspace::MAIN) else {
            return Box::pin(async { Err(t!("Glint's window isn't open").to_string()) });
        };
        flat(glint_core::rclone::rclone_authorize(window, help))
    }

    fn authorize_cancel(&self) {
        glint_core::rclone::rclone_authorize_cancel();
    }

    fn delete(&self, name: String) -> Fut<()> {
        flat(glint_core::rclone::rclone_delete(name))
    }

    fn test(&self, connection: RemoteConnection) -> Fut<String> {
        flat(glint_core::remote::remote_test(to_core(&connection), None))
    }
}

// ---- The model ----------------------------------------------------------------------------------------------------

/// Where setting a cloud up stands.
#[derive(Clone, Debug)]
pub enum Phase {
    /// Looking for rclone.
    Checking,
    /// rclone isn't on this PC: the install card.
    Install,
    /// Its configuration is encrypted. `error`: why the last password was not taken.
    Unlock { error: Option<SharedString> },
    /// The list of services, and the remotes rclone has already.
    Choose,
    /// A service's name in Glint and its first settings.
    Settings { provider: Provider },
    /// One of rclone's questions.
    Asking {
        provider: Provider,
        state: String,
        question: Question,
        error: String,
    },
    /// Waiting for the sign-in in the browser; `url` once rclone has its page ready.
    Browser { provider: Provider, url: Option<String> },
    /// Something rclone or the PC does.
    Working { what: SharedString },
    /// It didn't work.
    Failed { error: String },
    /// It works: what was at its top, and the name to add it under.
    Ready { connection: RemoteConnection, said: String },
}

/// What the model tells the view.
#[derive(Clone, Debug)]
pub enum SetupEvent {
    /// "Add and open" was pressed: the connection to save and open.
    Done(RemoteConnection),
}

/// Where the values of the form come from: the first settings, or the answer to rclone's question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// `values`.
    Settings,
    /// `answer`.
    Answer,
}

/// The state machine of connecting a cloud.
pub struct Setup {
    backend: Arc<dyn Backend>,
    phase: Phase,
    /// Counts phase changes: the view starts its boxes afresh when it moves.
    serial: u64,
    /// Counts abandoned attempts: answers of an earlier attempt are dropped.
    generation: u64,
    status: Option<Status>,
    providers: Vec<Provider>,
    remotes: Vec<Remote>,
    query: String,
    name: String,
    values: HashMap<String, String>,
    answer: String,
    own: bool,
    /// The connections Glint has already (for names and ids that don't repeat).
    connections: Vec<RemoteConnection>,
    /// The remote being made in rclone's configuration: taken out again if this stops before it works.
    making: Option<String>,
    done: bool,
}

impl EventEmitter<SetupEvent> for Setup {}

impl Setup {
    /// A model that looks for rclone at once. `connections` are the ones Glint has.
    pub fn new(backend: Arc<dyn Backend>, connections: Vec<RemoteConnection>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            backend,
            phase: Phase::Checking,
            serial: 0,
            generation: 0,
            status: None,
            providers: Vec::new(),
            remotes: Vec::new(),
            query: String::new(),
            name: String::new(),
            values: HashMap::new(),
            answer: String::new(),
            own: false,
            connections,
            making: None,
            done: false,
        };
        this.check(cx);
        this
    }

    /// Where it stands.
    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// Changes at each phase change.
    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// rclone as last seen.
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    /// The kinds of storage rclone reaches.
    pub fn providers(&self) -> &[Provider] {
        &self.providers
    }

    /// The name typed for the new connection.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// A first setting as typed.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// "My own app (client ID)" is on.
    pub fn own(&self) -> bool {
        self.own
    }

    /// The answer typed to rclone's question.
    pub fn answer(&self) -> &str {
        &self.answer
    }

    /// Whether the connection was handed over.
    #[cfg(test)]
    pub fn is_done(&self) -> bool {
        self.done
    }

    fn set_phase(&mut self, phase: Phase, cx: &mut Context<Self>) {
        self.phase = phase;
        self.serial += 1;
        cx.notify();
    }

    fn taken_ids(&self) -> Vec<String> {
        self.connections.iter().map(|c| c.id.clone()).collect()
    }

    fn service(&self, provider: &Provider) -> SharedString {
        service_name(&provider.name, &self.providers)
    }

    /// Looks for rclone, then for its services and remotes (`check`).
    pub fn check(&mut self, cx: &mut Context<Self>) {
        self.set_phase(Phase::Checking, cx);
        let generation = self.generation;
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let found = backend.status().await;
            let next = match found {
                Err(e) if e.starts_with(LOCKED) => Err(Phase::Unlock { error: None }),
                Err(e) => Err(Phase::Failed { error: e }),
                Ok(found) if !found.installed => Ok((found, None)),
                Ok(found) if found.locked => Ok((found, None)),
                Ok(found) => {
                    // Both are asked for at once.
                    let (providers, remotes) = (backend.providers(), backend.remotes());
                    let (providers, remotes) = (providers.await, remotes.await);
                    match (providers, remotes) {
                        (Ok(p), Ok(r)) => Ok((found, Some((p, r)))),
                        (Err(e), _) | (_, Err(e)) if e.starts_with(LOCKED) => Err(Phase::Unlock { error: None }),
                        (Err(e), _) | (_, Err(e)) => Err(Phase::Failed { error: e }),
                    }
                }
            };
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match next {
                    Err(phase) => this.set_phase(phase, cx),
                    Ok((found, lists)) => {
                        let phase = if !found.installed {
                            Phase::Install
                        } else if found.locked {
                            Phase::Unlock { error: None }
                        } else {
                            Phase::Choose
                        };
                        this.status = Some(found);
                        if let Some((providers, remotes)) = lists {
                            this.providers = providers;
                            this.remotes = remotes;
                        }
                        this.set_phase(phase, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// Unlocks rclone's configuration with `password` (`unlock`).
    pub fn unlock(&mut self, password: String, cx: &mut Context<Self>) {
        self.set_phase(
            Phase::Working {
                what: t!("Unlocking rclone's configuration…"),
            },
            cx,
        );
        let asking = self.backend.unlock(password);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let answer = asking.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match answer {
                    Ok(()) => this.check(cx),
                    Err(e) => this.set_phase(Phase::Unlock { error: Some(e.into()) }, cx),
                }
            });
        })
        .detach();
    }

    /// Back to the unlock box (the failure's "Unlock it").
    pub fn ask_for_password(&mut self, cx: &mut Context<Self>) {
        self.set_phase(Phase::Unlock { error: None }, cx);
    }

    /// The text of the search box.
    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        cx.notify();
    }

    /// The name typed for the new connection.
    pub fn set_name(&mut self, name: String, cx: &mut Context<Self>) {
        self.name = name;
        cx.notify();
    }

    /// A first setting.
    pub fn set_value(&mut self, name: &str, value: String, cx: &mut Context<Self>) {
        self.values.insert(name.to_string(), value);
        cx.notify();
    }

    /// The answer to rclone's question.
    pub fn set_answer(&mut self, answer: String, cx: &mut Context<Self>) {
        self.answer = answer;
        cx.notify();
    }

    /// "My own app (client ID)".
    pub fn set_own(&mut self, own: bool, cx: &mut Context<Self>) {
        self.own = own;
        cx.notify();
    }

    /// The name to add the working connection under (the box of the last step).
    pub fn set_ready_name(&mut self, name: String, cx: &mut Context<Self>) {
        if let Phase::Ready { connection, .. } = &mut self.phase {
            connection.name = name;
            cx.notify();
        }
    }

    /// The services shown for the search, the popular first (`listed`).
    pub fn listed(&self) -> Vec<Provider> {
        let words = self.query.trim().to_lowercase();
        let mut all: Vec<Provider> = self
            .providers
            .iter()
            .filter(|p| offered(p))
            .filter(|p| {
                words.is_empty()
                    || format!("{} {} {}", self.service(p), p.description, p.name)
                        .to_lowercase()
                        .contains(&words)
            })
            .cloned()
            .collect();
        let rank = |p: &Provider| POPULAR.iter().position(|n| *n == p.name).unwrap_or(POPULAR.len());
        all.sort_by(|a, b| {
            rank(a)
                .cmp(&rank(b))
                .then_with(|| compare_base_numeric(&self.service(a), &self.service(b)))
        });
        all
    }

    /// The remotes rclone has that Glint doesn't show yet (`unused`).
    pub fn unused(&self) -> Vec<Remote> {
        let known: Vec<String> = self
            .connections
            .iter()
            .filter(|c| c.kind == glint_model::store::RemoteKind::Rclone)
            .filter_map(|c| c.rclone.as_ref().map(|r| r.to_lowercase()))
            .collect();
        self.remotes
            .iter()
            .filter(|r| !known.contains(&r.name.to_lowercase()))
            .cloned()
            .collect()
    }

    /// A service was picked (`choose`): its name in Glint, not taken, and its first settings.
    pub fn choose(&mut self, provider: Provider, cx: &mut Context<Self>) {
        let service = self.service(&provider).to_string();
        let taken: Vec<String> = self.connections.iter().map(|c| c.name.to_lowercase()).collect();
        let mut shown = service.clone();
        let mut n = 2;
        while taken.contains(&shown.to_lowercase()) {
            shown = format!("{service} {n}");
            n += 1;
        }
        self.name = shown;
        self.values = provider
            .options
            .iter()
            .filter(|o| !o.default.is_empty() && o.kind != "bool")
            .map(|o| (o.name.clone(), String::new()))
            .collect();
        self.own = false;
        self.set_phase(Phase::Settings { provider }, cx);
    }

    /// Its settings to give rclone first: those filled in, for the service chosen (S3's provider)
    /// (`shownOptions`).
    pub fn shown_options(&self, provider: &Provider) -> Vec<Question> {
        let s3 = self.values.get("provider").map_or("", String::as_str);
        provider
            .options
            .iter()
            .filter(|o| {
                o.name != "token"
                    && !o.name.starts_with("config_")
                    && (!provider.oauth || self.own || (o.name != "client_id" && o.name != "client_secret"))
                    && (o.name == "provider" || o.provider.is_empty() || for_provider(&o.provider, s3))
            })
            .cloned()
            .collect()
    }

    /// A needed setting is empty (`missing`).
    pub fn missing(&self, provider: &Provider) -> bool {
        self.shown_options(provider)
            .iter()
            .any(|o| o.required && self.values.get(&o.name).map_or("", |v| v.trim()).is_empty())
    }

    /// Whether "Sign in" / "Connect" can be pressed.
    pub fn can_start(&self) -> bool {
        match &self.phase {
            Phase::Settings { provider } => !self.name.trim().is_empty() && !self.missing(provider),
            _ => false,
        }
    }

    /// Makes the remote in rclone's configuration and follows its steps (`start`).
    pub fn start(&mut self, cx: &mut Context<Self>) {
        let Phase::Settings { provider } = self.phase.clone() else {
            return;
        };
        if !self.can_start() {
            return;
        }
        let taken: Vec<String> = self.remotes.iter().map(|r| r.name.clone()).collect();
        let remote = remote_name_for(&self.name, &taken);
        self.making = Some(remote.clone());
        let parameters: Map<String, Value> = self
            .shown_options(&provider)
            .iter()
            .filter_map(|o| {
                let value = self.values.get(&o.name).map_or("", |v| v.trim());
                (!value.is_empty()).then(|| (o.name.clone(), Value::String(value.to_string())))
            })
            .collect();
        let service = self.service(&provider);
        self.set_phase(
            Phase::Working {
                what: t!("Setting up {service}…", service = service),
            },
            cx,
        );
        let first = self
            .backend
            .config_start(remote.clone(), provider.name.clone(), parameters);
        self.follow(provider, remote, first, cx);
    }

    /// Gives rclone the answer typed (`reply`).
    pub fn reply(&mut self, cx: &mut Context<Self>) {
        let Phase::Asking { provider, state, .. } = self.phase.clone() else {
            return;
        };
        let Some(remote) = self.making.clone() else { return };
        self.set_phase(
            Phase::Working {
                what: t!("Asking rclone…"),
            },
            cx,
        );
        let next = self.backend.config_answer(remote.clone(), state, self.answer.clone());
        self.follow(provider, remote, next, cx);
    }

    /// Where rclone's steps lead: its next question, a browser sign-in, or done (then a test) (`follow`).
    fn follow(&mut self, provider: Provider, remote: String, first: Fut<Step>, cx: &mut Context<Self>) {
        let generation = self.generation;
        let backend = self.backend.clone();
        let service = self.service(&provider);
        let connections = self.connections.clone();
        let name = self.name.clone();
        cx.spawn(async move |this, cx| {
            let mut step = first.await;
            loop {
                let current = match step {
                    Ok(current) => current,
                    Err(error) => {
                        Self::apply(&this, cx, generation, |s, cx| s.set_phase(Phase::Failed { error }, cx));
                        return;
                    }
                };
                let Some(question) = current.question.clone() else {
                    if !current.state.is_empty() {
                        Self::apply(&this, cx, generation, |s, cx| {
                            s.set_phase(
                                Phase::Failed {
                                    error: t!("rclone stopped before it was set up").to_string(),
                                },
                                cx,
                            )
                        });
                        return;
                    }
                    Self::test(
                        &this,
                        cx,
                        generation,
                        &backend,
                        &connections,
                        &name,
                        Some(&provider),
                        &remote,
                    )
                    .await;
                    return;
                };
                match question.name.as_str() {
                    // A browser on this PC: Glint opens the page (rclone authorize), and the sign-in is the answer.
                    "config_is_local" => {
                        step = backend
                            .config_answer(remote.clone(), current.state.clone(), "false".into())
                            .await;
                    }
                    "config_token" => {
                        let shown = provider.clone();
                        if !Self::apply(&this, cx, generation, |s, cx| {
                            s.set_phase(
                                Phase::Browser {
                                    provider: shown,
                                    url: None,
                                },
                                cx,
                            )
                        }) {
                            return;
                        }
                        let token = match backend.authorize(question.help.clone()).await {
                            Ok(token) => token,
                            Err(e) if e == "cancelled" => return,
                            Err(error) => {
                                Self::apply(&this, cx, generation, |s, cx| s.set_phase(Phase::Failed { error }, cx));
                                return;
                            }
                        };
                        let finishing = t!("Signed in. Finishing {service}…", service = service);
                        if !Self::apply(&this, cx, generation, |s, cx| {
                            s.set_phase(Phase::Working { what: finishing }, cx)
                        }) {
                            return;
                        }
                        step = backend
                            .config_answer(remote.clone(), current.state.clone(), token)
                            .await;
                    }
                    _ => {
                        let (shown, state) = (provider.clone(), current.state.clone());
                        Self::apply(&this, cx, generation, |s, cx| {
                            s.answer = question.default.clone();
                            s.set_phase(
                                Phase::Asking {
                                    provider: shown,
                                    state,
                                    question,
                                    error: current.error.clone(),
                                },
                                cx,
                            )
                        });
                        return;
                    }
                }
            }
        })
        .detach();
    }

    /// Runs `change` on the model if it is alive and the attempt is still the current one.
    fn apply(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        generation: u64,
        change: impl FnOnce(&mut Self, &mut Context<Self>),
    ) -> bool {
        this.update(cx, |s, cx| {
            if s.generation != generation {
                return false;
            }
            change(s, cx);
            true
        })
        .unwrap_or(false)
    }

    /// Tries the remote as Glint will open it: what is at its top (`test`).
    #[allow(clippy::too_many_arguments)] // The attempt's parts, carried into a task.
    async fn test(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        generation: u64,
        backend: &Arc<dyn Backend>,
        connections: &[RemoteConnection],
        shown: &str,
        provider: Option<&Provider>,
        remote: &str,
    ) {
        let kind = provider.map(|p| p.name.clone()).or_else(|| {
            this.read_with(cx, |s, _| {
                s.remotes.iter().find(|r| r.name == remote).map(|r| r.kind.clone())
            })
            .ok()
            .flatten()
        });
        let ids: Vec<String> = connections.iter().map(|c| c.id.clone()).collect();
        let shown = if shown.trim().is_empty() { remote } else { shown.trim() };
        let connection = connection_for(shown, remote, &kind.unwrap_or_default(), &ids);
        if !Self::apply(this, cx, generation, |s, cx| {
            s.set_phase(
                Phase::Working {
                    what: t!("Looking inside…"),
                },
                cx,
            )
        }) {
            return;
        }
        let said = backend.test(connection.clone()).await;
        Self::apply(this, cx, generation, |s, cx| match said {
            Ok(said) => s.set_phase(Phase::Ready { connection, said }, cx),
            Err(error) => s.set_phase(Phase::Failed { error }, cx),
        });
    }

    /// One of the remotes rclone has already was picked: test it as it is.
    pub fn use_existing(&mut self, remote: Remote, cx: &mut Context<Self>) {
        self.name = remote.name.clone();
        let generation = self.generation;
        let backend = self.backend.clone();
        let connections = self.connections.clone();
        let shown = remote.name.clone();
        cx.spawn(async move |this, cx| {
            Self::test(
                &this,
                cx,
                generation,
                &backend,
                &connections,
                &shown,
                None,
                &remote.name,
            )
            .await;
        })
        .detach();
    }

    /// The sign-in page's address arrived: remembered for "Open the sign-in page again". Returns it, to be opened.
    pub fn sign_in_page(&mut self, url: String, cx: &mut Context<Self>) -> Option<String> {
        if let Phase::Browser { url: slot, .. } = &mut self.phase {
            *slot = Some(url.clone());
            cx.notify();
            return Some(url);
        }
        None
    }

    /// Back to the services, the half-made remote taken out of rclone's configuration (`startOver`).
    pub fn start_over(&mut self, cx: &mut Context<Self>) {
        self.backend.authorize_cancel();
        self.generation += 1;
        let Some(making) = self.making.take() else {
            self.check(cx);
            return;
        };
        // The services are listed again once rclone has taken the remote out of its configuration (the
        // TypeScript awaited `rclone_delete`), so the list can't still show the half-made remote. A failed
        // delete doesn't hold the list back.
        let deleting = self.backend.delete(making);
        self.set_phase(Phase::Checking, cx);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let _ = deleting.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.check(cx);
                }
            });
        })
        .detach();
    }

    /// "Add and open": hands the connection over, named as typed (`finish`).
    pub fn finish(&mut self, cx: &mut Context<Self>) {
        let Phase::Ready { connection, .. } = &self.phase else {
            return;
        };
        let name = connection.name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let taken: Vec<String> = self.taken_ids();
        let mut made = connection.clone();
        made.id = crate::components::settings::clouds::remote_id(&name, &taken);
        made.name = name;
        self.done = true;
        cx.emit(SetupEvent::Done(made));
    }
}

impl Drop for Setup {
    /// The attempt stops with the view: a sign-in waiting ends, and a remote that was started and not handed
    /// over goes from rclone's configuration.
    fn drop(&mut self) {
        self.backend.authorize_cancel();
        if let (Some(making), false) = (self.making.take(), self.done) {
            drop(self.backend.delete(making));
        }
    }
}

// ---- The view -----------------------------------------------------------------------------------------------------

type OnDone = Rc<dyn Fn(RemoteConnection, &mut Window, &mut App)>;
type OnCancel = Rc<dyn Fn(&mut Window, &mut App)>;

/// The tools model the install card reads (none without core, as in the tests).
fn tools_for(window: &mut Window, cx: &mut App) -> Option<Entity<Tools>> {
    let core = cx.try_global::<core_bridge::Core>()?;
    let backend: Arc<dyn SettingsBackend> = Arc::new(SettingsCoreBackend::new(core.app.clone()));
    Some(cx.new(|cx| Tools::new(backend, window, cx)))
}

/// The steps of connecting a cloud, with their buttons: embeddable in the connection dialog, or shown on its own
/// by [`open`].
pub struct CloudSetup {
    model: Entity<Setup>,
    tools: Option<Entity<Tools>>,
    on_done: OnDone,
    on_cancel: OnCancel,
    /// The boxes of the phase on show, by what they hold ("name", "query", "answer", "password", "q:<setting>").
    inputs: HashMap<String, Entity<InputState>>,
    inputs_for: u64,
    /// The list of services' scroll.
    services_scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl CloudSetup {
    /// A view of `model`. `on_done` gets the connection when "Add and open" is pressed; `on_cancel` runs for
    /// Cancel. `tools` is the model the install card reads (None: no card).
    pub fn new(
        model: Entity<Setup>,
        tools: Option<Entity<Tools>>,
        on_done: impl Fn(RemoteConnection, &mut Window, &mut App) + 'static,
        on_cancel: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = vec![cx.observe(&model, |_, _, cx| cx.notify())];
        subscriptions.push(
            cx.subscribe_in(&model, window, |this, _, event: &SetupEvent, window, cx| {
                let SetupEvent::Done(made) = event;
                (this.on_done.clone())(made.clone(), window, cx);
            }),
        );
        // The sign-in page rclone has ready is opened in the browser.
        if let Some(subscription) = core_bridge::subscribe(cx, |this, event, cx| {
            let core_bridge::CoreEvent::RcloneSignIn(url) = event else {
                return;
            };
            if let Some(url) = this.model.update(cx, |m, cx| m.sign_in_page(url.clone(), cx)) {
                cx.open_url(&url);
            }
        }) {
            subscriptions.push(subscription);
        }
        // rclone installed from the card: on to the services.
        if let Some(tools) = &tools {
            subscriptions.push(cx.observe(tools, |this, tools, cx| {
                let installed = tools.read(cx).tool("rclone").is_some_and(|t| t.origin.is_some());
                let waiting = matches!(this.model.read(cx).phase(), Phase::Install);
                if installed && waiting {
                    this.model.update(cx, |m, cx| m.check(cx));
                }
            }));
        }
        Self {
            model,
            tools,
            on_done: Rc::new(on_done),
            on_cancel: Rc::new(on_cancel),
            inputs: HashMap::new(),
            inputs_for: u64::MAX,
            services_scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    /// The box called `key` ("name", "query", "answer", "password", "q:<setting>", "ready"), if it exists now.
    pub fn input(&self, key: &str) -> Option<&Entity<InputState>> {
        self.inputs.get(key)
    }

    /// Sends what is typed in the box `key` to the model.
    fn typed(&mut self, key: &str, text: String, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| match key {
            "name" => m.set_name(text, cx),
            "query" => m.set_query(text, cx),
            "answer" => m.set_answer(text, cx),
            "ready" => m.set_ready_name(text, cx),
            "password" => cx.notify(),
            other => {
                if let Some(setting) = other.strip_prefix("q:") {
                    m.set_value(setting, text, cx);
                }
            }
        });
    }

    /// Enter in the box `key`.
    fn entered(&mut self, key: &str, cx: &mut Context<Self>) {
        let password = self.inputs.get("password").map(|i| i.read(cx).value().to_string());
        self.model.update(cx, |m, cx| match (key, m.phase.clone()) {
            ("password", Phase::Unlock { .. }) => {
                if let Some(password) = password.filter(|p| !p.is_empty()) {
                    m.unlock(password, cx);
                }
            }
            ("answer", Phase::Asking { .. }) => m.reply(cx),
            ("ready", Phase::Ready { .. }) => m.finish(cx),
            (_, Phase::Settings { .. }) if key == "name" || key.starts_with("q:") => m.start(cx),
            _ => {}
        });
    }

    /// The box `key`, made when first asked for, starting with `initial`. `focus` puts the cursor in it.
    #[allow(clippy::too_many_arguments)] // What a box is made of.
    fn text_box(
        &mut self,
        key: &str,
        initial: &str,
        masked: bool,
        placeholder: SharedString,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(existing) = self.inputs.get(key) {
            return existing.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).masked(masked).placeholder(placeholder));
        if !initial.is_empty() {
            input.update(cx, |input, cx| input.set_value(initial.to_string(), window, cx));
        }
        let name = key.to_string();
        self._subscriptions.push(cx.subscribe_in(
            &input,
            window,
            move |this, input, event: &InputEvent, _window, cx| match event {
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    this.typed(&name, text, cx);
                }
                InputEvent::PressEnter { .. } => this.entered(&name, cx),
                _ => {}
            },
        ));
        if focus {
            let focused = input.clone();
            window.defer(cx, move |window, cx| {
                focused.update(cx, |input, cx| input.focus(window, cx))
            });
        }
        self.inputs.insert(key.to_string(), input.clone());
        input
    }

    /// A line of `text` where its web addresses are buttons under it (`Linked`).
    fn linked(slot: String, text: &str, size: Rems, color: Hsla, cx: &mut Context<Self>) -> Div {
        let parts = split_links(text);
        let links: Vec<String> = parts.iter().filter(|(link, _)| *link).map(|(_, s)| s.clone()).collect();
        let _ = cx;
        col()
            .gap(space(1.))
            .child(div().text_size(size).text_color(color).child(text.to_string()))
            .children(links.into_iter().enumerate().map(|(i, url)| {
                let shown = url.clone();
                Button::new(SharedString::from(format!("{slot}-link-{i}")))
                    .link()
                    .xsmall()
                    .label(shown)
                    .on_click(move |_, _, cx| cx.open_url(&url))
            }))
    }

    /// One of rclone's questions (or a service's setting) as the field that answers it (`QuestionField`).
    fn question_field(
        &mut self,
        question: &Question,
        scope: Scope,
        provider: Option<String>,
        autofocus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let model = self.model.clone();
        let (label, more) = help_of(question);
        let provider = provider.filter(|p| !p.is_empty());
        let examples: Vec<Example> = question
            .examples
            .iter()
            .filter(|e| provider.as_deref().is_none_or(|p| for_provider(&e.provider, p)))
            .cloned()
            .collect();
        let value = {
            let m = model.read(cx);
            match scope {
                Scope::Answer => m.answer.clone(),
                Scope::Settings => m.values.get(&question.name).cloned().unwrap_or_else(|| {
                    if question.kind == "bool" {
                        question.default.clone()
                    } else {
                        String::new()
                    }
                }),
            }
        };
        let name = question.name.clone();
        let setter = move |model: &Entity<Setup>, value: String, cx: &mut App| {
            let name = name.clone();
            model.update(cx, |m, cx| match scope {
                Scope::Answer => m.set_answer(value, cx),
                Scope::Settings => m.set_value(&name, value, cx),
            });
        };
        let slot = format!("cloud-{}", question.name);
        let field: AnyElement = if question.kind == "bool" {
            let yes = value == "true";
            let first_line = |text: &str| text.split('\n').next().unwrap_or("").to_string();
            let word = |wanted: &str, fallback: SharedString| -> SharedString {
                examples
                    .iter()
                    .find(|e| e.value == wanted)
                    .map(|e| e.help.clone())
                    .filter(|h| !h.is_empty())
                    .map_or(fallback, |h| first_line(&h).into())
            };
            row_start()
                .id(SharedString::from(slot.clone()))
                .role(Role::RadioGroup)
                .aria_label(SharedString::from(label.clone()))
                .test_support()
                .gap(space(1.5))
                .children(
                    [("true", word("true", t!("Yes"))), ("false", word("false", t!("No")))]
                        .into_iter()
                        .map(|(answer, text)| {
                            let on = (answer == "true") == yes;
                            let (model, setter) = (model.clone(), setter.clone());
                            Button::new(SharedString::from(format!("{slot}-{answer}")))
                                .outline()
                                .small()
                                .selected(on)
                                .toggled(on)
                                .label(text)
                                .on_click(move |_, _, cx| setter(&model, answer.to_string(), cx))
                        }),
                )
                .into_any_element()
        } else if !examples.is_empty() && question.exclusive {
            let known = examples.iter().any(|e| e.value == value);
            let mut choices: Vec<(String, String)> = Vec::new();
            if !known {
                let shown = if value.is_empty() {
                    t!("Choose…").to_string()
                } else {
                    value.clone()
                };
                choices.push((value.clone(), shown));
            }
            for e in &examples {
                let first = e.help.split('\n').next().unwrap_or("").to_string();
                let shown = if first.is_empty() {
                    e.value.clone()
                } else if first == e.value {
                    first
                } else {
                    format!("{first} ({})", e.value)
                };
                choices.push((e.value.clone(), shown));
            }
            col()
                .id(SharedString::from(slot.clone()))
                .role(Role::RadioGroup)
                .aria_label(SharedString::from(label.clone()))
                .test_support()
                .max_h(rems(10.))
                .overflow_y_scroll()
                .rounded(theme::size::RADIUS)
                .border_1()
                .border_color(t.border)
                .children(choices.into_iter().enumerate().map(|(i, (choice, shown))| {
                    let on = choice == value;
                    let (model, setter) = (model.clone(), setter.clone());
                    Button::new(SharedString::from(format!("{slot}-choice-{i}")))
                        .ghost()
                        .small()
                        .justify_start()
                        .selected(on)
                        .toggled(on)
                        .label(shown)
                        .on_click(move |_, _, cx| setter(&model, choice.clone(), cx))
                }))
                .into_any_element()
        } else {
            let placeholder: SharedString = if !question.default.is_empty() && !question.password {
                question.default.clone().into()
            } else if question.required {
                t!("Needed")
            } else {
                t!("Optional")
            };
            let key = match scope {
                Scope::Answer => "answer".to_string(),
                Scope::Settings => format!("q:{}", question.name),
            };
            let input = self.text_box(&key, &value, question.password, placeholder, autofocus, window, cx);
            let chips = (!examples.is_empty()).then(|| {
                row_start()
                    .flex_wrap()
                    .gap(space(1.))
                    .children(examples.iter().take(8).enumerate().map(|(i, e)| {
                        let first = e.help.split('\n').next().unwrap_or("").to_string();
                        let shown = if first.is_empty() { e.value.clone() } else { first };
                        let (input, setter, model, picked) =
                            (input.clone(), setter.clone(), model.clone(), e.value.clone());
                        Button::new(SharedString::from(format!("{slot}-example-{i}")))
                            .outline()
                            .xsmall()
                            .label(shown)
                            .on_click(move |_, window, cx| {
                                input.update(cx, |input, cx| input.set_value(picked.clone(), window, cx));
                                setter(&model, picked.clone(), cx);
                            })
                    }))
            });
            col()
                .gap(space(1.))
                .child(Input::new(&input).small().aria_label(SharedString::from(label.clone())))
                .children(chips)
                .into_any_element()
        };
        col()
            .gap(space(1.))
            .child(
                align_row(1.)
                    .items_start()
                    .child(Self::linked(
                        format!("{slot}-label"),
                        &label,
                        theme::size::XS,
                        t.muted_foreground,
                        cx,
                    ))
                    .when(question.required, |this| {
                        this.child(div().text_size(theme::size::XS).text_color(t.destructive).child("*"))
                    }),
            )
            .child(field)
            .when(!more.is_empty(), |this| {
                this.child(
                    div()
                        .id(SharedString::from(format!("{slot}-more")))
                        .max_h(rems(7.))
                        .overflow_y_scroll()
                        .child(Self::linked(
                            format!("{slot}-more"),
                            &more,
                            theme::size::XXS,
                            t.muted_foreground,
                            cx,
                        )),
                )
            })
            .into_any_element()
    }

    fn working(what: SharedString, t: &theme::Tokens) -> AnyElement {
        align_row(2.)
            .id("cloud-working")
            .role(Role::Status)
            .aria_label(what.clone())
            .test_support()
            .text_color(t.muted_foreground)
            .child(Spinner::new())
            .child(div().flex_1().min_w_0().child(what))
            .into_any_element()
    }

    fn failure(error: String, t: &theme::Tokens) -> AnyElement {
        align_row(2.)
            .id("cloud-failure")
            .role(Role::Alert)
            .aria_label(SharedString::from(error.clone()))
            .test_support()
            .items_start()
            .text_size(theme::size::XS)
            .text_color(t.destructive)
            .child(icon(IconName::TriangleAlert).size_3p5().flex_none().mt(space(0.5)))
            .child(div().flex_1().min_w_0().child(error))
            .into_any_element()
    }

    /// A box with a heading over it.
    fn labelled(heading: SharedString, content: impl IntoElement, t: &theme::Tokens) -> Div {
        col()
            .gap(space(1.))
            .child(
                div()
                    .text_size(theme::size::XS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.muted_foreground)
                    .child(heading),
            )
            .child(content)
    }

    /// The step on show (`body`).
    fn body(&mut self, phase: Phase, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let model = self.model.clone();
        match phase {
            Phase::Checking => Self::working(t!("Looking for rclone…"), &t),
            Phase::Install => {
                let card = self.tools.as_ref().and_then(|tools| {
                    tools::needed(
                        tools,
                        "rclone",
                        t!("rclone signs in to your clouds and reads them for Glint."),
                        IconName::Cloud,
                        cx,
                    )
                });
                col()
                    .gap(space(3.))
                    .child(t!(
                        "Glint reaches Google Drive, OneDrive, Dropbox, Box, MEGA, S3 and 60 more clouds through rclone, a free program that's been doing it for years. It's not on this PC yet."
                    ))
                    .children(card)
                    .child(
                        Button::new("cloud-installed")
                            .outline()
                            .small()
                            .label(t!("I've installed it"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(tools) = &this.tools {
                                    tools.update(cx, |tools, cx| tools.refresh(window, cx));
                                }
                                this.model.update(cx, |m, cx| m.check(cx));
                            })),
                    )
                    .into_any_element()
            }
            Phase::Unlock { error } => {
                let config = model.read(cx).status().and_then(|s| s.config.clone());
                let explain = match config {
                    Some(config) => t!(
                        "Your rclone configuration is encrypted. Its password unlocks it until Glint closes ({config}).",
                        config = config
                    ),
                    None => t!("Your rclone configuration is encrypted. Its password unlocks it until Glint closes."),
                };
                let input = self.text_box("password", "", true, SharedString::default(), true, window, cx);
                let empty = input.read(cx).value().is_empty();
                col()
                    .gap(space(3.))
                    .child(
                        div()
                            .id("cloud-unlock-explain")
                            .role(Role::Group)
                            .aria_label(explain.clone())
                            .test_support()
                            .child(explain),
                    )
                    .child(
                        Input::new(&input)
                            .small()
                            .aria_label(t!("rclone's configuration password")),
                    )
                    .children(error.map(|e| Self::failure(e.to_string(), &t)))
                    .child(
                        Button::new("cloud-unlock")
                            .primary()
                            .small()
                            .icon(IconName::LockOpen)
                            .label(t!("Unlock"))
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, _, cx| this.entered("password", cx))),
                    )
                    .into_any_element()
            }
            Phase::Choose => self.choose_body(window, cx),
            Phase::Settings { provider } => self.settings_body(provider, window, cx),
            Phase::Asking { question, error, .. } => {
                let field = self.question_field(&question, Scope::Answer, None, true, window, cx);
                col()
                    .gap(space(3.))
                    .child(
                        div()
                            .id("cloud-asks")
                            .role(Role::Group)
                            .aria_label(t!("rclone asks:"))
                            .test_support()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(t!("rclone asks:")),
                    )
                    .child(field)
                    .when(!error.is_empty(), |this| this.child(Self::failure(error.clone(), &t)))
                    .into_any_element()
            }
            Phase::Browser { provider, url } => {
                let service = service_name(&provider.name, model.read(cx).providers());
                col()
                    .gap(space(3.))
                    .child(
                        align_row(2.)
                            .id("cloud-browser")
                            .role(Role::Status)
                            .aria_label(t!(
                                "Sign in to {service} in your browser, and allow rclone there. This carries on by itself once you have.",
                                service = service
                            ))
                            .test_support()
                            .items_start()
                            .child(div().mt(space(0.5)).child(Spinner::new()))
                            .child(div().flex_1().min_w_0().child(t!(
                                "Sign in to {service} in your browser, and allow rclone there. This carries on by itself once you have.",
                                service = service
                            ))),
                    )
                    .when_some(url, |this, url| {
                        this.child(
                            Button::new("cloud-open-sign-in")
                                .outline()
                                .small()
                                .icon(IconName::ExternalLink)
                                .label(t!("Open the sign-in page again"))
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    })
                    .into_any_element()
            }
            Phase::Working { what } => Self::working(what, &t),
            Phase::Failed { error } => {
                let locked = error.starts_with(LOCKED);
                col()
                    .gap(space(3.))
                    .child(Self::failure(error, &t))
                    .when(locked, |this| {
                        this.child(
                            Button::new("cloud-unlock-it")
                                .outline()
                                .small()
                                .icon(IconName::KeyRound)
                                .label(t!("Unlock it"))
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.model.update(cx, |m, cx| m.ask_for_password(cx))),
                                ),
                        )
                    })
                    .into_any_element()
            }
            Phase::Ready { connection, said } => {
                let input = self.text_box(
                    "ready",
                    &connection.name,
                    false,
                    SharedString::default(),
                    false,
                    window,
                    cx,
                );
                col()
                    .gap(space(3.))
                    .child(
                        align_row(2.)
                            .id("cloud-said")
                            .role(Role::Status)
                            .aria_label(said_text(&said))
                            .test_support()
                            .items_start()
                            .child(icon(IconName::CircleCheck).size_4().flex_none().mt(space(0.5)).text_color(t.success))
                            .child(div().flex_1().min_w_0().child(said_text(&said))),
                    )
                    .child(Self::labelled(
                        t!("Its name in Glint"),
                        Input::new(&input).small().aria_label(t!("Its name in Glint")),
                        &t,
                    ))
                    .child(div().text_size(theme::size::XS).text_color(t.muted_foreground).child(t!(
                        "It opens read-only, like Glint's other remotes: what you open or copy out is downloaded. Folder rules can keep a folder of yours in sync with it."
                    )))
                    .into_any_element()
            }
        }
    }

    /// One service's button in the list: its icon, name and what it is (`p`'s description when the name
    /// doesn't already say it).
    fn service_button(
        i: usize,
        p: &Provider,
        providers: &[Provider],
        model: &Entity<Setup>,
        t: &theme::Tokens,
    ) -> AnyElement {
        let shown = service_name(&p.name, providers);
        let describe = !p.description.to_lowercase().contains(&shown.to_lowercase());
        let (model, picked) = (model.clone(), p.clone());
        Button::new(("cloud-service", i))
            .ghost()
            .h(rems(SERVICE_ROW))
            .w(relative(0.49))
            .justify_start()
            .tooltip(p.description.clone())
            .accessibility_label(if describe {
                format!("{shown}: {}", p.description)
            } else {
                shown.to_string()
            })
            .on_click(move |_, _, cx| {
                let picked = picked.clone();
                model.update(cx, |m, cx| m.choose(picked, cx));
            })
            .child(
                align_row(2.)
                    .w_full()
                    .child(
                        icon(service_icon(&p.name))
                            .size_4()
                            .flex_none()
                            .text_color(t.muted_foreground),
                    )
                    .child(
                        col()
                            .flex_1()
                            .min_w_0()
                            .align_start()
                            .text_start()
                            .child(div().truncate().child(isolate(shown.clone())))
                            .when(describe, |this| {
                                this.child(
                                    div()
                                        .truncate()
                                        .text_size(theme::size::XXS)
                                        .text_color(t.muted_foreground)
                                        .child(p.description.clone()),
                                )
                            }),
                    )
                    .when(p.oauth, |this| {
                        // The globe says it signs in in the browser: to the eye, and to a screen reader.
                        this.child(
                            div()
                                .id(("cloud-service-browser", i))
                                .role(Role::Image)
                                .aria_label(t!("Signs in in your browser"))
                                .test_support()
                                .flex_none()
                                .child(icon(IconName::Globe).size_3p5().text_color(t.muted_foreground)),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The services, and the remotes rclone has already.
    fn choose_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let model = self.model.clone();
        let (unused, listed, version, providers) = {
            let m = model.read(cx);
            (
                m.unused(),
                m.listed(),
                m.status().and_then(|s| s.version.clone()),
                m.providers().to_vec(),
            )
        };
        let query = self.text_box(
            "query",
            "",
            false,
            t!("Search {n} services", n = listed.len()),
            true,
            window,
            cx,
        );
        let existing = (!unused.is_empty()).then(|| {
            let heading = match &version {
                Some(version) => t!("Already in rclone {version}", version = version),
                None => t!("Already in rclone"),
            };
            Self::labelled(
                heading.clone(),
                row_start()
                    .id("cloud-existing-list")
                    .role(Role::Group)
                    .aria_label(heading)
                    .test_support()
                    .flex_wrap()
                    .gap(space(1.5))
                    .children(unused.iter().enumerate().map(|(i, r)| {
                        let model = model.clone();
                        let remote = r.clone();
                        let kind = service_name(&r.kind, &providers);
                        Button::new(("cloud-existing", i))
                            .outline()
                            .h_auto()
                            .w(relative(0.49))
                            .py(space(1.5))
                            .justify_start()
                            .accessibility_label(format!("{}: {kind}", r.name))
                            .on_click(move |_, _, cx| {
                                let remote = remote.clone();
                                model.update(cx, |m, cx| m.use_existing(remote, cx));
                            })
                            .child(
                                align_row(2.)
                                    .child(icon(service_icon(&r.kind)).size_4().flex_none().text_color(t.primary))
                                    .child(
                                        col()
                                            .min_w_0()
                                            .align_start()
                                            .text_start()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(isolate(r.name.clone())),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(theme::size::XXS)
                                                    .text_color(t.muted_foreground)
                                                    .child(kind),
                                            ),
                                    ),
                            )
                    })),
                &t,
            )
        });
        let heading = if unused.is_empty() {
            t!("Which cloud?")
        } else {
            t!("Or connect another")
        };
        let services = Self::labelled(
            heading,
            col()
                .gap(space(1.5))
                .child(
                    Input::new(&query)
                        .small()
                        .prefix(icon(IconName::Search).size_3p5().text_color(t.muted_foreground))
                        .aria_label(t!("Search services")),
                )
                .child({
                    // Rows of two, drawn as they scroll into view (about 70 services).
                    let rows = listed.len().div_ceil(2);
                    let height = rows.min(SERVICE_ROWS_SHOWN) as f32 * SERVICE_ROW;
                    let listed = Rc::new(listed);
                    let colors = t.clone();
                    let list = uniform_list(
                        "cloud-service-rows",
                        rows,
                        cx.processor(move |_, range: Range<usize>, _, _| {
                            range
                                .map(|row| {
                                    row_start()
                                        .id(("cloud-service-row", row))
                                        .h(rems(SERVICE_ROW))
                                        .items_start()
                                        .gap(space(1.))
                                        .children((row * 2..row * 2 + 2).map(|i| match listed.get(i) {
                                            Some(p) => Self::service_button(i, p, &providers, &model, &colors),
                                            None => div().w(relative(0.49)).into_any_element(),
                                        }))
                                        .into_any_element()
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.services_scroll)
                    .size_full();
                    div()
                        .id("cloud-services")
                        .role(Role::List)
                        .aria_label(t!("Services"))
                        .test_support()
                        .h(rems(height))
                        .child(list)
                }),
            &t,
        );
        col()
            .gap(space(4.))
            .children(existing)
            .child(services)
            .into_any_element()
    }

    /// A service's name in Glint and its first settings.
    fn settings_body(&mut self, provider: Provider, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let model = self.model.clone();
        let (options, name, own, s3, service) = {
            let m = model.read(cx);
            (
                m.shown_options(&provider),
                m.name().to_string(),
                m.own(),
                m.value("provider").unwrap_or("").to_string(),
                service_name(&provider.name, m.providers()),
            )
        };
        let has_own = provider.oauth && provider.options.iter().any(|o| o.name == "client_id");
        let name_box = self.text_box("name", &name, false, SharedString::default(), true, window, cx);
        let fields: Vec<AnyElement> = options
            .iter()
            .map(|o| {
                let filter = if o.name == "provider" { None } else { Some(s3.clone()) };
                self.question_field(o, Scope::Settings, filter, false, window, cx)
            })
            .collect();
        col()
            .gap(space(3.))
            .child(Self::labelled(
                t!("Its name in Glint"),
                Input::new(&name_box).small().aria_label(t!("Its name in Glint")),
                &t,
            ))
            .children(fields)
            .when(has_own, |this| {
                let drive = provider.name == "drive";
                let hint = if drive {
                    t!("Optional: rclone signs in as its own app otherwise. Google is retiring rclone's shared one for Drive, so yours keeps working.")
                } else {
                    t!("Optional: rclone signs in as its own app otherwise.")
                };
                this.child(
                    col()
                        .gap(space(0.5))
                        .child(
                            Checkbox::new("cloud-own")
                                .label(t!("My own app (client ID)"))
                                .checked(own)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    let own = *checked;
                                    this.model.update(cx, |m, cx| m.set_own(own, cx));
                                })),
                        )
                        .child(
                            div()
                                .id("cloud-own-hint")
                                .role(Role::Group)
                                .aria_label(hint.clone())
                                .test_support()
                                .ps(6.)
                                .text_size(theme::size::XXS)
                                .text_color(t.muted_foreground)
                                .child(hint),
                        ),
                )
            })
            .when(provider.oauth, |this| {
                this.child(
                    align_row(2.)
                        .items_start()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(icon(IconName::Globe).size_3p5().flex_none().mt(space(0.5)))
                        .child(div().flex_1().min_w_0().child(t!(
                            "You sign in on {service}'s own page, in your browser: Glint never sees your password. rclone keeps the sign-in in its configuration.",
                            service = service
                        ))),
                )
            })
            .into_any_element()
    }
}

impl Render for CloudSetup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The boxes of the step that is gone are dropped; the new step makes its own.
        let serial = self.model.read(cx).serial();
        if self.inputs_for != serial {
            self.inputs_for = serial;
            self.inputs.clear();
        }
        let t = theme::tokens(cx).clone();
        let phase = self.model.read(cx).phase().clone();
        let body = self.body(phase.clone(), window, cx);
        let back = matches!(
            phase,
            Phase::Settings { .. }
                | Phase::Failed { .. }
                | Phase::Asking { .. }
                | Phase::Browser { .. }
                | Phase::Ready { .. }
        );
        let can_start = self.model.read(cx).can_start();
        let on_cancel = self.on_cancel.clone();
        let mut footer = align_row(2.)
            .px(space(4.))
            .py(space(3.))
            .border_t_1()
            .border_color(t.border);
        if back {
            footer = footer.child(
                Button::new("cloud-other")
                    .ghost()
                    .small()
                    .icon(back_icon())
                    .label(t!("Other clouds"))
                    .on_click(cx.listener(|this, _, _, cx| this.model.update(cx, |m, cx| m.start_over(cx)))),
            );
        }
        footer = footer.child(div().flex_1()).child(
            Button::new("cloud-cancel")
                .ghost()
                .small()
                .label(t!("Cancel"))
                .on_click(move |_, window, cx| on_cancel(window, cx)),
        );
        match &phase {
            Phase::Settings { provider } => {
                let oauth = provider.oauth;
                let button = Button::new("cloud-start")
                    .primary()
                    .small()
                    .disabled(!can_start)
                    .on_click(cx.listener(|this, _, _, cx| this.model.update(cx, |m, cx| m.start(cx))));
                footer = footer.child(if oauth {
                    button.icon(IconName::Globe).label(t!("Sign in"))
                } else {
                    button.label(t!("Connect")).icon(forward_icon())
                });
            }
            Phase::Asking { .. } => {
                footer = footer.child(
                    Button::new("cloud-continue")
                        .primary()
                        .small()
                        .label(t!("Continue"))
                        .icon(forward_icon())
                        .on_click(cx.listener(|this, _, _, cx| this.model.update(cx, |m, cx| m.reply(cx)))),
                );
            }
            Phase::Ready { connection, .. } => {
                footer = footer.child(
                    Button::new("cloud-add")
                        .primary()
                        .small()
                        .label(t!("Add and open"))
                        .disabled(connection.name.trim().is_empty())
                        .on_click(cx.listener(|this, _, _, cx| this.model.update(cx, |m, cx| m.finish(cx)))),
                );
            }
            _ => {}
        }
        col()
            .id("cloud-setup")
            .role(Role::Group)
            .aria_label(t!("Connect a cloud"))
            .test_support()
            .text_start()
            .text_size(theme::size::SM)
            .child(
                div()
                    .id("cloud-setup-body")
                    .max_h(rems(32.))
                    .overflow_y_scroll()
                    .p(space(4.))
                    .child(body),
            )
            .child(footer)
    }
}

// ---- Opening ------------------------------------------------------------------------------------------------------

/// Saves the connection (`saveRemote`), tells core, opens its folder, and closes the dialog.
pub fn adopt(window: &mut Window, cx: &mut App, made: RemoteConnection) {
    let path = glint_model::paths::REMOTE.to_string() + &made.id;
    persistence::update_app(cx, |app| app.save_remote(made));
    push_connections_to_core(cx);
    window.close_dialog(cx);
    if let Some(tabs) = explorer::try_explorer(cx) {
        tabs.update(cx, |e, cx| e.navigate(path, explorer::NavOptions::default(), cx));
    }
}

/// Tells core the connections there are now (`initRemotes` pushed them at every change).
fn push_connections_to_core(cx: &App) {
    let all: Vec<glint_core::remote::Connection> = persistence::app(cx)
        .map(|app| app.remotes.iter().map(to_core).collect())
        .unwrap_or_default();
    // Run on core's runtime, so the UI thread doesn't wait on its lock; the call is made as it is spawned.
    drop(core_bridge::run(
        async move { glint_core::remote::remote_configure(all) },
    ));
}

/// Opens the dialog that connects a cloud (`addRemote`, for a cloud) in `window`. The connection is saved and
/// opened when "Add and open" is pressed.
pub fn open(window: &mut Window, cx: &mut App) {
    let backend: Arc<dyn Backend> = match cx.try_global::<core_bridge::Core>() {
        Some(core) => Arc::new(CoreBackend::new(core.app.clone())),
        None => return,
    };
    let tools = tools_for(window, cx);
    open_with(window, cx, backend, tools);
}

/// [`open`] with a backend and tools of your own (tests pass fakes).
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    tools: Option<Entity<Tools>>,
) -> (Entity<Setup>, Entity<CloudSetup>) {
    let connections = persistence::app(cx).map(|app| app.remotes.clone()).unwrap_or_default();
    let model = cx.new(|cx| Setup::new(backend, connections, cx));
    let view = cx.new(|cx| {
        CloudSetup::new(
            model.clone(),
            tools,
            |made, window, cx| adopt(window, cx, made),
            |window, cx| window.close_dialog(cx),
            window,
            cx,
        )
    });
    super::show(window, cx, t!("Connect a cloud"), px(576.), view.clone());
    (model, view)
}

// ---- Tests --------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    fn question(name: &str) -> Question {
        Question {
            name: name.into(),
            help: format!("The {name}.\n\nMore about it, see https://rclone.org/docs/ (here)."),
            kind: "string".into(),
            ..Default::default()
        }
    }

    fn provider(name: &str, description: &str, oauth: bool, options: Vec<Question>) -> Provider {
        Provider {
            name: name.into(),
            description: description.into(),
            oauth,
            options,
        }
    }

    fn drive() -> Provider {
        provider(
            "drive",
            "Google Drive",
            true,
            vec![
                question("client_id"),
                question("client_secret"),
                question("token"),
                question("scope"),
            ],
        )
    }

    fn s3() -> Provider {
        let mut chooser = question("provider");
        chooser.exclusive = true;
        chooser.examples = vec![
            Example {
                value: "AWS".into(),
                help: "Amazon Web Services (AWS) S3\nmore".into(),
                provider: String::new(),
            },
            Example {
                value: "Minio".into(),
                help: "Minio".into(),
                provider: String::new(),
            },
        ];
        let mut bucket = question("bucket");
        bucket.required = true;
        let mut region = question("region");
        region.provider = "AWS".into();
        let mut endpoint = question("endpoint");
        endpoint.provider = "!AWS".into();
        provider(
            "s3",
            "Amazon S3 Compliant Storage Providers",
            false,
            vec![chooser, bucket, region, endpoint],
        )
    }

    fn sftp() -> Provider {
        provider("sftp", "SSH/SFTP", false, vec![question("host")])
    }

    #[derive(Default)]
    struct Script {
        status: Option<Status>,
        steps: Vec<Step>,
        calls: Vec<String>,
        test: Option<Result<String, String>>,
        /// Holds a delete until something is sent.
        hold_delete: Option<async_channel::Receiver<()>>,
        /// More services than the usual few.
        extra_providers: usize,
        /// How many times rclone was looked for.
        statuses: usize,
    }

    struct Fake {
        script: Mutex<Script>,
        remotes: Vec<Remote>,
    }

    fn step(state: &str, question: Option<Question>) -> Step {
        Step {
            state: state.into(),
            question,
            error: String::new(),
        }
    }

    impl Fake {
        fn new() -> Arc<Fake> {
            Arc::new(Fake {
                script: Mutex::new(Script {
                    status: Some(Status {
                        installed: true,
                        version: Some("1.68.0".into()),
                        config: Some("C:\\rclone.conf".into()),
                        locked: false,
                    }),
                    test: Some(Ok("Connected: 3 items at its top".into())),
                    ..Default::default()
                }),
                remotes: vec![Remote {
                    name: "work".into(),
                    kind: "onedrive".into(),
                    provider: String::new(),
                }],
            })
        }

        fn calls(&self) -> Vec<String> {
            self.script.lock().unwrap().calls.clone()
        }

        fn with(&self, change: impl FnOnce(&mut Script)) {
            change(&mut self.script.lock().unwrap());
        }

        fn with_read<T>(&self, read: impl FnOnce(&Script) -> T) -> T {
            read(&self.script.lock().unwrap())
        }

        fn next_step(&self) -> Result<Step, String> {
            let mut script = self.script.lock().unwrap();
            if script.steps.is_empty() {
                Ok(step("", None))
            } else {
                Ok(script.steps.remove(0))
            }
        }
    }

    impl Backend for Fake {
        fn status(&self) -> Fut<Status> {
            self.script.lock().unwrap().statuses += 1;
            let status = self.script.lock().unwrap().status.clone().unwrap_or_default();
            Box::pin(async move { Ok(status) })
        }

        fn providers(&self) -> Fut<Vec<Provider>> {
            let extra = self.script.lock().unwrap().extra_providers;
            Box::pin(async move {
                let mut all = vec![
                    sftp(),
                    s3(),
                    drive(),
                    provider("local", "Local Disk", false, vec![]),
                    provider("zzz", "Zzz Storage", false, vec![]),
                ];
                all.extend(
                    (0..extra).map(|i| provider(&format!("extra{i:02}"), &format!("Extra {i:02}"), false, vec![])),
                );
                Ok(all)
            })
        }

        fn remotes(&self) -> Fut<Vec<Remote>> {
            let remotes = self.remotes.clone();
            Box::pin(async move { Ok(remotes) })
        }

        fn unlock(&self, password: String) -> Fut<()> {
            self.script.lock().unwrap().calls.push(format!("unlock {password}"));
            let right = password == "open sesame";
            if right {
                self.script.lock().unwrap().status.as_mut().unwrap().locked = false;
            }
            Box::pin(async move {
                if right {
                    Ok(())
                } else {
                    Err("That isn't its password".into())
                }
            })
        }

        fn config_start(&self, name: String, kind: String, parameters: Map<String, Value>) -> Fut<Step> {
            let mut keys: Vec<String> = parameters
                .iter()
                .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
                .collect();
            keys.sort();
            self.script
                .lock()
                .unwrap()
                .calls
                .push(format!("start {name} {kind} {}", keys.join(",")));
            let next = self.next_step();
            Box::pin(async move { next })
        }

        fn config_answer(&self, name: String, state: String, answer: String) -> Fut<Step> {
            self.script
                .lock()
                .unwrap()
                .calls
                .push(format!("answer {name} {state} {answer}"));
            let next = self.next_step();
            Box::pin(async move { next })
        }

        fn authorize(&self, _: String) -> Fut<String> {
            self.script.lock().unwrap().calls.push("authorize".into());
            Box::pin(async { Ok("TOKEN".to_string()) })
        }

        fn authorize_cancel(&self) {
            self.script.lock().unwrap().calls.push("authorize-cancel".into());
        }

        fn delete(&self, name: String) -> Fut<()> {
            let hold = {
                let mut script = self.script.lock().unwrap();
                script.calls.push(format!("delete {name}"));
                script.hold_delete.clone()
            };
            Box::pin(async move {
                if let Some(hold) = hold {
                    let _ = hold.recv().await;
                }
                Ok(())
            })
        }

        fn test(&self, connection: RemoteConnection) -> Fut<String> {
            let answer = self.script.lock().unwrap().test.clone().unwrap_or(Ok(String::new()));
            self.script.lock().unwrap().calls.push(format!(
                "test {} {} {}",
                connection.id,
                connection.rclone.clone().unwrap_or_default(),
                connection.service.clone().unwrap_or_default()
            ));
            Box::pin(async move { answer })
        }
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(|cx| {
            testing::init(cx);
            let dir = std::env::temp_dir().join(format!(
                "glint-cloud-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join("state.json");
            let _ = std::fs::remove_file(&file);
            persistence::init_at(cx, file);
        });
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
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

    type Opened = (
        Entity<Setup>,
        Entity<CloudSetup>,
        Rc<std::cell::RefCell<Vec<RemoteConnection>>>,
    );

    fn open_it(cx: &mut VisualTestContext, fake: &Arc<Fake>) -> Opened {
        let backend: Arc<dyn Backend> = fake.clone();
        let done = Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = done.clone();
        let (model, view) = cx.update(|window, cx| {
            let model = cx.new(|cx| Setup::new(backend, Vec::new(), cx));
            let view = cx.new(|cx| {
                CloudSetup::new(
                    model.clone(),
                    None,
                    move |made, _, _| sink.borrow_mut().push(made),
                    |window, cx| window.close_dialog(cx),
                    window,
                    cx,
                )
            });
            super::super::show(window, cx, t!("Connect a cloud"), px(576.), view.clone());
            (model, view)
        });
        cx.run_until_parked();
        (model, view, done)
    }

    fn phase_is(cx: &mut VisualTestContext, model: &Entity<Setup>, check: fn(&Phase) -> bool) -> bool {
        model.read_with(cx, |m, _| check(m.phase()))
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Cloud,
            IconName::Server,
            IconName::Globe,
            IconName::LockOpen,
            IconName::KeyRound,
            IconName::ExternalLink,
            IconName::CircleCheck,
            IconName::TriangleAlert,
            IconName::Search,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn the_helpers_follow_the_typescript() {
        let _guard = testing::lock();
        // forProvider
        assert!(for_provider("", "AWS"));
        assert!(for_provider("AWS,Minio", "Minio"));
        assert!(!for_provider("AWS,Minio", "Other"));
        assert!(!for_provider("!AWS", "AWS"));
        assert!(for_provider("!AWS, Other", "Minio"));
        // helpOf: the first paragraph is the label (its lines joined), the rest is more.
        let mut q = question("x");
        q.help = "  First line\n  second line\n\nThe rest.\nMore.  ".into();
        assert_eq!(
            help_of(&q),
            ("First line second line".to_string(), "The rest.\nMore.".to_string())
        );
        q.help = String::new();
        assert_eq!(help_of(&q), ("x".to_string(), String::new()));
        // remoteNameFor
        assert_eq!(remote_name_for("Google Drive", &[]), "google-drive");
        assert_eq!(
            remote_name_for("Google Drive", &["Google-Drive".to_string()]),
            "google-drive-2"
        );
        assert_eq!(remote_name_for("صور", &[]), "cloud");
        assert_eq!(remote_name_for("  My_NAS!! ", &[]), "my_nas");
        assert_eq!(remote_name_for(&"a".repeat(40), &[]).len(), 32);
        // serviceName, serviceIcon
        assert_eq!(service_name("drive", &[]), "Google Drive");
        assert_eq!(
            service_name("zzz", &[provider("zzz", "Zzz Storage", false, vec![])]),
            "Zzz Storage"
        );
        assert_eq!(service_name("unknown", &[]), "unknown");
        assert!(matches!(service_icon("sftp"), IconName::Server));
        assert!(matches!(service_icon("drive"), IconName::Cloud));
        // offered
        assert!(!offered(&provider("local", "", false, vec![])));
        assert!(offered(&drive()));
        // Linked
        assert_eq!(
            split_links("see https://rclone.org/docs/ (and http://a.b/c)"),
            vec![
                (false, "see ".to_string()),
                (true, "https://rclone.org/docs/".to_string()),
                (false, " (and ".to_string()),
                (true, "http://a.b/c".to_string()),
                (false, ")".to_string()),
            ]
        );
        assert_eq!(split_links("none"), vec![(false, "none".to_string())]);
    }

    #[gpui_kit::test]
    fn the_services_are_listed_popular_first_without_wrappers_and_filtered_by_the_search(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        let (model, view, _) = open_it(&mut visual, &fake);
        settle(&mut visual, "the services", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Choose))
        });
        let names: Vec<String> = model.read_with(&visual, |m, _| m.listed().iter().map(|p| p.name.clone()).collect());
        assert_eq!(
            names,
            ["drive", "s3", "sftp", "zzz"],
            "popular order, then the rest; `local` is not offered"
        );
        // rclone's own remotes that Glint doesn't show.
        assert!(has(&mut visual, "Already in rclone 1.68.0"));
        assert_eq!(role_of(&mut visual, "work: OneDrive"), Some(Role::Button));
        assert_eq!(role_of(&mut visual, "Services"), Some(Role::List));
        assert_eq!(role_of(&mut visual, "Search services"), Some(Role::TextInput));
        // The search narrows by name and description.
        let query = view
            .read_with(&visual, |v, _| v.input("query").cloned())
            .expect("the search box");
        type_into(&mut visual, &query, "ssh");
        assert_eq!(model.read_with(&visual, |m, _| m.listed().len()), 1);
        type_into(&mut visual, &query, "google");
        assert_eq!(model.read_with(&visual, |m, _| m.listed()[0].name.clone()), "drive");
    }

    #[gpui_kit::test]
    fn other_clouds_lists_the_services_again_only_after_the_half_made_remote_is_deleted(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        let (release, hold) = async_channel::bounded::<()>(1);
        fake.with(|s| {
            s.steps = vec![step("s1", Some(question("config_is_local"))), step("", None)];
            s.hold_delete = Some(hold);
        });
        let (model, _, _) = open_it(&mut visual, &fake);
        settle(&mut visual, "the services", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Choose))
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("cloud-service", 0_usize), cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-start", cx);
        });
        visual.run_until_parked();
        let (before, looked) = (fake.calls().len(), fake.with_read(|s| s.statuses));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-other", cx);
        });
        visual.run_until_parked();
        // The delete is under way and the list has not been asked for.
        assert_eq!(fake.calls()[before..], ["authorize-cancel", "delete google-drive"]);
        assert!(phase_is(&mut visual, &model, |p| matches!(p, Phase::Checking)));
        assert_eq!(
            fake.with_read(|s| s.statuses),
            looked,
            "not looked for while the delete is under way"
        );
        release.try_send(()).unwrap();
        settle(&mut visual, "the services again", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Choose))
        });
        assert_eq!(fake.calls()[before..], ["authorize-cancel", "delete google-drive"]);
        assert_eq!(
            fake.with_read(|s| s.statuses),
            looked + 1,
            "looked for once the delete finished"
        );
    }

    #[gpui_kit::test]
    fn a_service_that_signs_in_in_the_browser_says_so_by_its_globe(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        let (model, _, _) = open_it(&mut visual, &fake);
        settle(&mut visual, "the services", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Choose))
        });
        assert_eq!(role_of(&mut visual, "Signs in in your browser"), Some(Role::Image));
    }

    #[gpui_kit::test]
    fn the_long_list_of_services_draws_only_the_rows_in_view(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.with(|s| s.extra_providers = 70);
        let (model, _, _) = open_it(&mut visual, &fake);
        settle(&mut visual, "the services", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Choose))
        });
        assert_eq!(model.read_with(&visual, |m, _| m.listed().len()), 74);
        assert_eq!(role_of(&mut visual, "Services"), Some(Role::List));
        assert!(has(&mut visual, "Google Drive"), "the first rows are drawn");
        assert!(!has(&mut visual, "Extra 69"), "the last is far below the view");
    }

    #[gpui_kit::test]
    fn an_oauth_cloud_signs_in_in_the_browser_and_is_added_under_the_name_typed(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.with(|s| {
            s.steps = vec![
                step("s1", Some(question("config_is_local"))),
                step("s2", Some(question("config_token"))),
                step("", None),
            ];
        });
        let (model, view, done) = open_it(&mut visual, &fake);
        settle(&mut visual, "the services", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Choose))
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("cloud-service", 0_usize), cx);
        });
        visual.run_until_parked();
        assert!(phase_is(&mut visual, &model, |p| matches!(p, Phase::Settings { .. })));
        assert_eq!(model.read_with(&visual, |m, _| m.name().to_string()), "Google Drive");
        // The client ID and secret are the "own app" settings: hidden until asked for.
        assert!(!has(&mut visual, "The client_id."));
        assert!(has(&mut visual, "The scope."));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-own", cx);
        });
        assert!(has(&mut visual, "The client_id."));
        assert_eq!(role_of(&mut visual, "My own app (client ID)"), Some(Role::CheckBox));
        // Name it, and sign in.
        let name = view
            .read_with(&visual, |v, _| v.input("name").cloned())
            .expect("the name box");
        type_into(&mut visual, &name, "ملفات العمل");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-start", cx);
        });
        settle(&mut visual, "the ready step", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Ready { .. }))
        });
        assert_eq!(
            fake.calls(),
            [
                "start cloud drive ",
                "answer cloud s1 false",
                "authorize",
                "answer cloud s2 TOKEN",
                // The id of an Arabic name has no letters to make it from.
                "test remote cloud drive",
            ]
        );
        assert!(has(&mut visual, "Connected: 3 items at its top"));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-add", cx);
        });
        let made = done.borrow().clone();
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].name, "ملفات العمل");
        assert_eq!(made[0].rclone.as_deref(), Some("cloud"));
        assert_eq!(made[0].service.as_deref(), Some("drive"));
        assert_eq!(made[0].kind, glint_model::store::RemoteKind::Rclone);
        assert!(model.read_with(&visual, |m, _| m.is_done()));
    }

    #[gpui_kit::test]
    fn a_service_needs_its_required_settings_and_asks_rclones_questions_one_at_a_time(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        let mut asked = question("access_key_id");
        asked.default = "AKIA".into();
        fake.with(|s| s.steps = vec![step("q1", Some(asked)), step("", None)]);
        let (model, view, _) = open_it(&mut visual, &fake);
        settle(&mut visual, "the services", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Choose))
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("cloud-service", 1_usize), cx);
        });
        visual.run_until_parked();
        // The bucket is needed: nothing starts without it. The endpoint is only for services other than AWS.
        assert!(!model.read_with(&visual, |m, _| m.can_start()));
        assert!(has(&mut visual, "The endpoint."));
        assert!(!has(&mut visual, "The region."));
        assert_eq!(role_of(&mut visual, "The provider."), Some(Role::RadioGroup));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-provider-choice-1", cx);
        });
        assert!(has(&mut visual, "The region."));
        assert!(!has(&mut visual, "The endpoint."));
        let bucket = view
            .read_with(&visual, |v, _| v.input("q:bucket").cloned())
            .expect("the bucket box");
        type_into(&mut visual, &bucket, "photos");
        assert!(model.read_with(&visual, |m, _| m.can_start()));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-start", cx);
        });
        settle(&mut visual, "rclone's question", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Asking { .. }))
        });
        assert_eq!(
            fake.calls()[0],
            "start amazon-s3-and-compatible s3 bucket=photos,provider=AWS"
        );
        // The question's default is in the box; Continue sends it.
        assert_eq!(model.read_with(&visual, |m, _| m.answer().to_string()), "AKIA");
        assert!(has(&mut visual, "rclone asks:"));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-continue", cx);
        });
        settle(&mut visual, "the end", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Ready { .. }))
        });
        assert_eq!(fake.calls()[1], "answer amazon-s3-and-compatible q1 AKIA");
    }

    #[gpui_kit::test]
    fn without_rclone_the_install_step_shows_and_a_locked_configuration_asks_for_its_password(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.with(|s| s.status = Some(Status::default()));
        let (model, _, _) = open_it(&mut visual, &fake);
        settle(&mut visual, "the install step", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Install))
        });
        assert!(has(&mut visual, "I've installed it"));
        // Now rclone is there but its configuration is encrypted.
        fake.with(|s| {
            s.status = Some(Status {
                installed: true,
                locked: true,
                config: Some("C:\\rclone.conf".into()),
                version: None,
            })
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-installed", cx);
        });
        settle(&mut visual, "the unlock step", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Unlock { .. }))
        });
        assert!(has(
            &mut visual,
            "Your rclone configuration is encrypted. Its password unlocks it until Glint closes (C:\\rclone.conf)."
        ));
        assert_eq!(
            role_of(&mut visual, "rclone's configuration password"),
            Some(Role::PasswordInput)
        );
    }

    #[gpui_kit::test]
    fn a_wrong_unlock_password_says_so_and_the_right_one_goes_on_to_the_services(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.with(|s| s.status.as_mut().unwrap().locked = true);
        let (model, view, _) = open_it(&mut visual, &fake);
        settle(&mut visual, "the unlock step", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Unlock { .. }))
        });
        let password = view
            .read_with(&visual, |v, _| v.input("password").cloned())
            .expect("the password box");
        type_into(&mut visual, &password, "nope");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        });
        settle(&mut visual, "the error", |cx| has(cx, "That isn't its password"));
        assert_eq!(role_of(&mut visual, "That isn't its password"), Some(Role::Alert));
        let password = view
            .read_with(&visual, |v, _| v.input("password").cloned())
            .expect("the password box");
        type_into(&mut visual, &password, "open sesame");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-unlock", cx);
        });
        settle(&mut visual, "the services", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Choose))
        });
        assert_eq!(fake.calls()[..2], ["unlock nope", "unlock open sesame"]);
    }

    #[gpui_kit::test]
    fn a_half_made_remote_is_taken_out_of_rclone_when_the_view_goes_but_not_once_added(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.with(|s| s.steps = vec![step("q1", Some(question("user")))]);
        let (model, view, _) = open_it(&mut visual, &fake);
        settle(&mut visual, "the services", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Choose))
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("cloud-service", 2_usize), cx);
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-start", cx);
        });
        settle(&mut visual, "the question", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Asking { .. }))
        });
        // Cancel closes the dialog: the view, the model and the half-made remote go.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-cancel", cx);
        });
        drop((model, view));
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        settle(&mut visual, "the clean-up", |_| {
            fake.calls().iter().any(|c| c == "delete sftp")
        });
        assert!(fake.calls().iter().any(|c| c == "authorize-cancel"));
    }

    #[gpui_kit::test]
    fn a_failure_is_an_alert_and_other_clouds_goes_back_to_the_list(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        fake.with(|s| s.test = Some(Err("403: forbidden".into())));
        let (model, _, _) = open_it(&mut visual, &fake);
        settle(&mut visual, "the services", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Choose))
        });
        // The remote rclone has already: tested as it is, and it fails.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("cloud-existing", 0_usize), cx);
        });
        settle(&mut visual, "the failure", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Failed { .. }))
        });
        assert_eq!(role_of(&mut visual, "403: forbidden"), Some(Role::Alert));
        assert_eq!(fake.calls(), ["test work work onedrive"]);
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("cloud-other", cx);
        });
        settle(&mut visual, "the list again", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Choose))
        });
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let fake = Fake::new();
        let (model, _, _) = open_it(&mut visual, &fake);
        settle(&mut visual, "the services", |cx| {
            phase_is(cx, &model, |p| matches!(p, Phase::Choose))
        });
        assert_eq!(role_of(&mut visual, "Connect a cloud"), Some(Role::Group));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("cloud-service", 0_usize), cx);
        });
        visual.run_until_parked();
        assert!(has(&mut visual, "Its name in Glint"));
        testing::set_rtl(false);
    }
}
