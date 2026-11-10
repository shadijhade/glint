//! Folder rules: files that come into a folder, sorted out as they come, while Glint runs.
//! Ported from src/components/explorer/RulesDialog.tsx, with the helpers of src/lib/rules.ts and glint-core's
//! `rules`; the rules themselves are kept in the saved state (`AppState.rules`).
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                    | Here                                                          |
//! |-----------------------------------------------|---------------------------------------------------------------|
//! | `useRulesDialog`, `openRules(folder?)`        | [`RulesDialog`], [`open`] (a folder opens a new rule for it)  |
//! | `useStore().rules`, `saveRule`, `removeRule`  | `persistence::app` / `update_app` with `AppState::save_rule`  |
//! | `invoke("rules_*")`                           | [`Backend`] (a fake in the tests), [`CoreBackend`]            |
//! | `List`, `Editor`, `Log`                       | the three bodies of [`RulesDialog`] ([`RuleEditor`] is a view of its own) |
//! | `cloudOf`, `targetOf`                         | [`cloud_of`], [`target_of`]                                   |
//! | `summary`, `didWhat`                          | [`summary`], [`did_what`]                                     |
//! | `megabytes`, `bytesOf`, `ruleId`, `blank`     | [`megabytes`], [`bytes_of`], [`rule_id`], [`blank`]           |
//! | `<select>`s (Then, With, When)                | radio groups (the kit has no drop-down)                       |
//! | Tauri's folder picker                         | `Backend::pick_folder`, through Settings' picker              |
//!
//! Rules are pushed to core (`rules_set`, which starts and stops the watchers) whenever this dialog changes
//! them, as the TypeScript's store subscription did for every change.
//!
//! Not here, because it is not the dialog: the toasts the TypeScript's `initRules` showed as rules do their
//! work (`rule-did` events, with Undo), and pushing the saved rules to core at start. PORT ME(coordinator):
//! `startup.rs` should call `glint_core::rules::rules_set` with `persistence::app(cx).rules` once at start
//! and subscribe to the `rule-did` core event.

use crate::kit::prelude::*;
use crate::{core_bridge, persistence, t, theme};
use glint_core::rules::Done;
use glint_model::format::{format_count, format_date, plural};
use glint_model::paths::base_name;
use glint_model::store::{FolderRule, RemoteConnection, RemoteKind, RuleAction, SyncMode};
use gpui_kit::UniformListScrollHandle;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// The calls into glint-core's rules. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// What `rule` would do now to what is in its folder (`rules_preview`).
    fn preview(&self, rule: &FolderRule) -> Fut<Vec<Done>>;
    /// Does it now (`rules_apply`): how many files were done, and how many could not be.
    fn apply(&self, rule: &FolderRule) -> Fut<(usize, usize)>;
    /// Everything rules did, oldest first (`rules_log`).
    fn log(&self) -> Fut<Vec<Done>>;
    /// Forgets the log (`rules_clear_log`).
    fn clear_log(&self) -> Fut<()>;
    /// Takes back one thing a rule did (`rules_undo`).
    fn undo(&self, id: u64) -> Fut<()>;
    /// Tells core the rules (`rules_set`): those turned on are watched.
    fn set_rules(&self, rules: Vec<FolderRule>) -> Fut<()>;
    /// A folder the person picks (`None`: cancelled). `owner` is the window's handle.
    fn pick_folder(&self, title: String, start: Option<String>, owner: isize) -> Fut<Option<String>>;
}

/// glint-core's rules through the app's handle.
pub struct CoreBackend {
    app: glint_core::host::AppHandle,
}

impl CoreBackend {
    /// The backend over core's handle (from `core_bridge::Core`).
    pub fn new(app: glint_core::host::AppHandle) -> Self {
        Self { app }
    }
}

/// core's own rule type for a saved one (the same JSON shape; the saved one types its choices).
pub fn to_core(rule: &FolderRule) -> glint_core::rules::Rule {
    serde_json::to_value(rule)
        .and_then(serde_json::from_value)
        .unwrap_or_default()
}

/// Waits for a core call and flattens the two layers of failure (the task panicking, the call failing).
fn flat<T: Send + 'static>(call: impl Future<Output = Result<T, String>> + Send + 'static) -> Fut<T> {
    let joined = core_bridge::run(call);
    Box::pin(async move { joined.await.and_then(|inner| inner) })
}

impl Backend for CoreBackend {
    fn preview(&self, rule: &FolderRule) -> Fut<Vec<Done>> {
        flat(glint_core::rules::rules_preview(to_core(rule)))
    }

    fn apply(&self, rule: &FolderRule) -> Fut<(usize, usize)> {
        flat(glint_core::rules::rules_apply(to_core(rule)))
    }

    fn log(&self) -> Fut<Vec<Done>> {
        flat(async { Ok(glint_core::rules::rules_log()) })
    }

    fn clear_log(&self) -> Fut<()> {
        flat(async {
            glint_core::rules::rules_clear_log();
            Ok(())
        })
    }

    fn undo(&self, id: u64) -> Fut<()> {
        flat(glint_core::rules::rules_undo(id))
    }

    fn set_rules(&self, rules: Vec<FolderRule>) -> Fut<()> {
        let rules = rules.iter().map(to_core).collect();
        flat(glint_core::rules::rules_set(self.app.clone(), rules))
    }

    fn pick_folder(&self, title: String, start: Option<String>, owner: isize) -> Fut<Option<String>> {
        use crate::components::settings::{Backend as _, CoreBackend as SettingsBackend};
        SettingsBackend::new(self.app.clone()).pick_folder(title, start, owner)
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn preview(&self, _: &FolderRule) -> Fut<Vec<Done>> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }

    fn apply(&self, _: &FolderRule) -> Fut<(usize, usize)> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }

    fn log(&self) -> Fut<Vec<Done>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn clear_log(&self) -> Fut<()> {
        Box::pin(async { Ok(()) })
    }

    fn undo(&self, _: u64) -> Fut<()> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }

    fn set_rules(&self, _: Vec<FolderRule>) -> Fut<()> {
        Box::pin(async { Ok(()) })
    }

    fn pick_folder(&self, _: String, _: Option<String>, _: isize) -> Fut<Option<String>> {
        Box::pin(async { Ok(None) })
    }
}

struct SharedBackend(Arc<dyn Backend>);

impl Global for SharedBackend {}

/// Sets the backend the dialog uses (tests pass a fake). [`backend`] makes core's if this was never called.
pub fn init_with(cx: &mut App, backend: Arc<dyn Backend>) {
    cx.set_global(SharedBackend(backend));
}

/// The backend in use.
pub fn backend(cx: &mut App) -> Arc<dyn Backend> {
    if let Some(shared) = cx.try_global::<SharedBackend>() {
        return shared.0.clone();
    }
    let Some(core) = cx.try_global::<core_bridge::Core>() else {
        return Arc::new(Unavailable);
    };
    let backend: Arc<dyn Backend> = Arc::new(CoreBackend::new(core.app.clone()));
    init_with(cx, backend.clone());
    backend
}

// ---- The words ------------------------------------------------------------------------------------------

/// What a rule can do, in the order the TypeScript listed them (`ACTIONS`), each with its words.
pub fn actions() -> [(RuleAction, SharedString); 6] {
    [
        (RuleAction::Sync, t!("Keep it in sync with a cloud (rclone)")),
        (RuleAction::Move, t!("Move it into a folder")),
        (RuleAction::Rename, t!("Rename it")),
        (
            RuleAction::Unblock,
            t!("Unblock it (take off Windows' downloaded-from-the-internet mark)"),
        ),
        (
            RuleAction::Extract,
            t!("Extract it (an archive) into a folder next to it"),
        ),
        (RuleAction::Recycle, t!("Put it in the Recycle Bin")),
    ]
}

/// How a sync rule syncs: its name and what that means (`SYNC_MODES`).
pub fn sync_modes() -> [(SyncMode, SharedString, SharedString); 3] {
    [
        (
            SyncMode::Copy,
            t!("Copy there"),
            t!("What's new or changed here goes to the cloud. Nothing there is deleted."),
        ),
        (
            SyncMode::Mirror,
            t!("Mirror there"),
            t!("The cloud's folder becomes what's here: what you delete here is deleted there too."),
        ),
        (
            SyncMode::Both,
            t!("Both ways"),
            t!("Changes on either side go to the other (rclone bisync). Where a file changed on both, the newer wins and the other is kept as name.conflict1."),
        ),
    ]
}

/// How often a rule that syncs looks again, in minutes, with its words (`EVERY`).
pub fn every_options() -> [(u32, SharedString); 5] {
    [
        (0, t!("Only when files here change")),
        (15, t!("Also every 15 minutes")),
        (60, t!("Also every hour")),
        (360, t!("Also every 6 hours")),
        (1440, t!("Also once a day")),
    ]
}

/// A rule in a few words: `In Downloads, *.pdf: move it into a folder D:\Docs\{year}` (`summary`).
pub fn summary(rule: &FolderRule) -> String {
    let folder = match base_name(&rule.folder) {
        "" => rule.folder.as_str(),
        name => name,
    };
    let below = if rule.subfolders { t!(" and below") } else { "".into() };
    let names = match rule.names.trim() {
        "" => t!("every file").to_string(),
        names => names.to_string(),
    };
    let place = t!("In {folder}", folder = folder);
    if rule.action == RuleAction::Sync {
        let how = sync_modes()
            .into_iter()
            .find(|(mode, _, _)| *mode == rule.sync.unwrap_or(SyncMode::Copy))
            .map_or_else(|| t!("sync").to_string(), |(_, label, _)| label.to_lowercase());
        let name = if base_name(&rule.folder).is_empty() {
            rule.folder.as_str()
        } else {
            base_name(&rule.folder)
        };
        return format!("{name}{below}, {names}: {how} ({})", rule.target);
    }
    let what = actions()
        .into_iter()
        .find(|(action, _)| *action == rule.action)
        .map(|(_, label)| label.split(" (").next().unwrap_or("").to_lowercase())
        .unwrap_or_default();
    let place_to = match rule.action {
        RuleAction::Move => format!(" {}", rule.target),
        RuleAction::Rename => t!(" to {pattern}", pattern = rule.pattern).to_string(),
        _ => String::new(),
    };
    let age = if rule.older_than_days > 0 {
        t!(
            ", unchanged {days}",
            days = plural(usize::try_from(rule.older_than_days).unwrap_or(usize::MAX), "day")
        )
        .to_string()
    } else {
        String::new()
    };
    format!("{place}{below}, {names}{age}: {what}{place_to}")
}

/// What a rule did, in a few words: `moved “a.pdf”`, or a sync's `synced “Photos” with gdrive:Photos: 3 up`
/// (`didWhat`).
pub fn did_what(done: &Done) -> String {
    let name = base_name(&done.from).to_string();
    if done.action == "sync" {
        let to = done.to.clone().unwrap_or_else(|| t!("the cloud").to_string());
        let mut said = t!("synced “{name}” with {to}", name = name, to = to).to_string();
        if let Some(summary) = done.summary.as_deref().filter(|s| !s.is_empty()) {
            said.push_str(&format!(": {}", summary.to_lowercase()));
        }
        return said;
    }
    let did = match done.action.as_str() {
        "move" => t!("moved"),
        "rename" => t!("renamed"),
        "recycle" => t!("put in the Recycle Bin"),
        "unblock" => t!("unblocked"),
        "extract" => t!("extracted"),
        other => other.to_string().into(),
    };
    format!("{did} “{name}”")
}

// ---- Clouds and numbers -------------------------------------------------------------------------------------

/// The rclone connections a rule can sync with, the one its target names (by index into the first), and the
/// folder below that connection's own (`cloudOf`).
pub fn cloud_of(target: &str, remotes: &[RemoteConnection]) -> (Vec<RemoteConnection>, Option<usize>, String) {
    let clouds: Vec<RemoteConnection> = remotes
        .iter()
        .filter(|r| r.kind == RemoteKind::Rclone && r.rclone.as_deref().is_some_and(|n| !n.is_empty()))
        .cloned()
        .collect();
    let (name, path) = match target.split_once(':') {
        Some((name, path)) => (name, path),
        None => (target, ""),
    };
    let named = |c: &RemoteConnection| c.rclone.as_deref() == Some(name);
    let found = clouds
        .iter()
        .position(|c| named(c) && (c.root.is_empty() || path == c.root || path.starts_with(&format!("{}/", c.root))))
        .or_else(|| clouds.iter().position(named));
    let below = match found.map(|ix| &clouds[ix]) {
        Some(cloud) if !cloud.root.is_empty() && path.starts_with(&cloud.root) => {
            path[cloud.root.len()..].trim_start_matches('/').to_string()
        }
        _ => path.to_string(),
    };
    (clouds, found, below)
}

/// A sync rule's target: the connection's remote, its folder, and the folder below it (`targetOf`).
pub fn target_of(cloud: &RemoteConnection, below: &str) -> String {
    let path: Vec<&str> = [cloud.root.as_str(), below]
        .into_iter()
        .map(|p| p.trim().trim_matches('/'))
        .filter(|p| !p.is_empty())
        .collect();
    format!("{}:{}", cloud.rclone.as_deref().unwrap_or(""), path.join("/"))
}

const MEGABYTE: f64 = 1_048_576.0;

/// A size in megabytes as the box shows it: empty for none, at most two decimals (`megabytes`).
pub fn megabytes(bytes: u64) -> String {
    if bytes == 0 {
        return String::new();
    }
    let value = (bytes as f64 / MEGABYTE * 100.0).round() / 100.0;
    format!("{value}")
}

/// The bytes a box of megabytes holds; a comma may be the decimal mark; nothing readable is none (`bytesOf`).
pub fn bytes_of(text: &str) -> u64 {
    let value = text.trim().replace(',', ".").parse::<f64>().unwrap_or(0.0);
    if value.is_finite() {
        (value * MEGABYTE).round().clamp(0.0, 1e18) as u64
    } else {
        0
    }
}

/// A new rule's id (`ruleId`): the time in base 36, and a counter so two in one millisecond differ.
pub fn rule_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let mut digits = Vec::new();
    let mut rest = millis;
    while rest > 0 {
        digits.push(char::from_digit((rest % 36) as u32, 36).unwrap_or('0'));
        rest /= 36;
    }
    let time: String = digits.iter().rev().collect();
    format!("rule-{time}-{:04x}", COUNTER.fetch_add(1, Ordering::Relaxed) % 0x1_0000)
}

/// A rule with nothing in it yet, for `folder` (`blank`).
pub fn blank(folder: &str) -> FolderRule {
    FolderRule {
        id: rule_id(),
        action: RuleAction::Move,
        folder: folder.to_string(),
        pattern: "{date} {name}{ext}".to_string(),
        ..FolderRule::default()
    }
}

// ---- The rules, kept in the saved state ----------------------------------------------------------------------------

/// The rules as the saved state has them.
pub fn saved_rules(cx: &App) -> Vec<FolderRule> {
    persistence::app(cx).map(|app| app.rules.clone()).unwrap_or_default()
}

/// Tells core the saved rules, as the TypeScript's store subscription did after every change.
fn push_rules(backend: &Arc<dyn Backend>, cx: &mut App) {
    let pushed = backend.set_rules(saved_rules(cx));
    cx.spawn(async move |_| {
        let _ = pushed.await;
    })
    .detach();
}

/// Saves `rule` (adding it or replacing the one with its id) and tells core.
fn save_rule(backend: &Arc<dyn Backend>, rule: FolderRule, cx: &mut App) {
    persistence::update_app(cx, |app| app.save_rule(rule));
    push_rules(backend, cx);
}

/// Deletes the rule with `id` and tells core.
fn remove_rule(backend: &Arc<dyn Backend>, id: &str, cx: &mut App) {
    persistence::update_app(cx, |app| app.remove_rule(id));
    push_rules(backend, cx);
}

/// Says something in `window` from a task that may outlive the dialog.
fn tell(cx: &mut AsyncApp, window: AnyWindowHandle, note: Notification) {
    let _ = cx.update(|cx| window.update(cx, |_, window, cx| window.push_notification(note, cx)));
}

// ---- Small pieces of drawing ----------------------------------------------------------------------------------

/// A radio button: a name and, if given, what it means; chosen ones are tinted.
fn radio<V: 'static>(
    id: impl Into<ElementId>,
    label: SharedString,
    hint: Option<SharedString>,
    chosen: bool,
    t: &theme::Tokens,
    cx: &mut Context<V>,
    on_pick: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> Button {
    Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(if chosen {
                    t.primary.opacity(0.1)
                } else {
                    t.background.opacity(0.)
                })
                .foreground(t.foreground)
                .hover(if chosen { t.primary.opacity(0.15) } else { t.accent })
                .active(t.accent),
        )
        .h_auto()
        .border_1()
        .border_color(if chosen { t.primary } else { t.border })
        .px(space(2.5))
        .py(space(1.))
        .role(Role::RadioButton)
        .toggled(chosen)
        .accessibility_label(match &hint {
            Some(hint) => format!("{label}. {hint}"),
            None => label.to_string(),
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            on_pick(this, window, cx);
            cx.notify();
        }))
        .child(div().text_start().child(isolate(label)))
}

/// A labelled row of the editor: a fixed-width label, then the control and its hint.
fn field(label: SharedString, hint: Option<SharedString>, content: impl IntoElement, t: &theme::Tokens) -> Div {
    let slot = format!("rules-hint-{label}");
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

// ---- The editor ---------------------------------------------------------------------------------------------

/// The editor says it is finished (saved, cancelled or deleted): the dialog goes back to the list.
pub enum EditorEvent {
    /// Back to the list.
    Close,
}

/// What a look at "what would it do" has found, or is still looking.
enum Preview {
    Running,
    Found(Vec<Done>),
    Failed(SharedString),
}

/// The boxes of the editor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    /// The rule's name.
    Name,
    /// The folder it watches.
    Folder,
    /// Name patterns.
    Names,
    /// Smallest size, in MB.
    Min,
    /// Biggest size, in MB.
    Max,
    /// Days unchanged.
    Days,
    /// Move: the folder it goes into.
    Into,
    /// Sync: the folder on the cloud.
    Below,
    /// Rename: the new name.
    Pattern,
}

const PREVIEW_ROW: f32 = 1.5;

/// One rule being made or changed: every setting, what the rule would do now, and the buttons that save it.
pub struct RuleEditor {
    backend: Arc<dyn Backend>,
    /// The rule as it was when the editor opened: its id, and whether it was on.
    start: FolderRule,
    owner: isize,
    // What isn't a text box.
    action: RuleAction,
    sync: Option<SyncMode>,
    every: Option<u32>,
    subfolders: bool,
    target: String,
    min_size: u64,
    max_size: u64,
    // The text boxes.
    name: Entity<InputState>,
    folder: Entity<InputState>,
    names: Entity<InputState>,
    min: Entity<InputState>,
    max: Entity<InputState>,
    days: Entity<InputState>,
    into: Entity<InputState>,
    below: Entity<InputState>,
    pattern: Entity<InputState>,
    preview: Option<Preview>,
    /// Which look is the latest: an older answer for settings that have changed since is dropped.
    looking: u64,
    /// The rule as last drawn, to tell when a change makes the preview stale.
    shown: FolderRule,
    scroll: UniformListScrollHandle,
    /// The fields and the look's result, above the buttons.
    body_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EditorEvent> for RuleEditor {}

impl RuleEditor {
    /// An editor for `start`. A rule that is saved and off is looked at at once (it is being turned on).
    pub fn new(backend: Arc<dyn Backend>, start: FolderRule, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let text = |placeholder: SharedString, value: String, window: &mut Window, cx: &mut Context<Self>| {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            if !value.is_empty() {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
            input
        };
        let (_, _, below) = cloud_of(
            &start.target,
            &persistence::app(cx).map(|a| a.remotes.clone()).unwrap_or_default(),
        );
        let name = text(t!("PDFs to Documents"), start.name.clone(), window, cx);
        let folder = text(t!("C:\\Users\\you\\Downloads"), start.folder.clone(), window, cx);
        let names = text(t!("*.pdf; invoice*"), start.names.clone(), window, cx);
        let min = text(t!("at least"), megabytes(start.min_size), window, cx);
        let max = text(t!("at most"), megabytes(start.max_size), window, cx);
        let days_text = if start.older_than_days > 0 {
            start.older_than_days.to_string()
        } else {
            String::new()
        };
        let days = text(t!("0"), days_text, window, cx);
        let into = text(t!("D:\\Documents\\Invoices\\{year}"), start.target.clone(), window, cx);
        let below_input = text(t!("Folder on it"), below, window, cx);
        let pattern = text(t!("{date} {name}{ext}"), start.pattern.clone(), window, cx);

        let mut subscriptions = Vec::new();
        for (field, input) in [
            (Field::Name, &name),
            (Field::Folder, &folder),
            (Field::Names, &names),
            (Field::Min, &min),
            (Field::Max, &max),
            (Field::Days, &days),
            (Field::Into, &into),
            (Field::Below, &below_input),
            (Field::Pattern, &pattern),
        ] {
            subscriptions.push(
                cx.subscribe_in(input, window, move |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.typed(field, window, cx);
                    }
                }),
            );
        }
        let mut this = Self {
            backend,
            owner,
            action: start.action,
            sync: start.sync,
            every: start.every,
            subfolders: start.subfolders,
            target: start.target.clone(),
            min_size: start.min_size,
            max_size: start.max_size,
            name,
            folder,
            names,
            min,
            max,
            days,
            into,
            below: below_input,
            pattern,
            preview: None,
            looking: 0,
            shown: start.clone(),
            scroll: UniformListScrollHandle::new(),
            body_scroll: ScrollHandle::new(),
            start,
            _subscriptions: subscriptions,
        };
        this.shown = this.rule(cx);
        // A rule that's off (asked to turn on, from the list): what it would do, at once.
        if this.known(cx) && !this.start.on {
            this.look(cx);
        }
        let first = this.name.clone();
        first.update(cx, |input, cx| input.focus(window, cx));
        this
    }

    /// The box for `field` (tests type into it).
    pub fn input(&self, field: Field) -> &Entity<InputState> {
        match field {
            Field::Name => &self.name,
            Field::Folder => &self.folder,
            Field::Names => &self.names,
            Field::Min => &self.min,
            Field::Max => &self.max,
            Field::Days => &self.days,
            Field::Into => &self.into,
            Field::Below => &self.below,
            Field::Pattern => &self.pattern,
        }
    }

    /// Whether the rule being edited is one already saved.
    pub fn known(&self, cx: &App) -> bool {
        persistence::app(cx).is_some_and(|app| app.rules.iter().any(|r| r.id == self.start.id))
    }

    /// The rule as the boxes and choices now say it.
    pub fn rule(&self, cx: &App) -> FolderRule {
        let value = |input: &Entity<InputState>| input.read(cx).value().to_string();
        FolderRule {
            id: self.start.id.clone(),
            name: value(&self.name),
            on: self.start.on,
            folder: value(&self.folder),
            subfolders: self.subfolders,
            names: value(&self.names),
            min_size: self.min_size,
            max_size: self.max_size,
            older_than_days: value(&self.days).parse().unwrap_or(0),
            action: self.action,
            target: self.target.clone(),
            pattern: value(&self.pattern),
            sync: self.sync,
            every: self.every,
        }
    }

    /// The rule with a name even when none was typed: `In Downloads`, or `A rule` (`named`).
    pub fn named(&self, cx: &App) -> FolderRule {
        let mut rule = self.rule(cx);
        let typed = rule.name.trim().to_string();
        rule.name = if !typed.is_empty() {
            typed
        } else {
            match base_name(&rule.folder) {
                "" => t!("A rule").to_string(),
                folder => t!("In {folder}", folder = folder).to_string(),
            }
        };
        rule
    }

    /// What a box's text means for the rule.
    fn typed(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input(field).read(cx).value().to_string();
        match field {
            Field::Min => self.min_size = bytes_of(&text),
            Field::Max => self.max_size = bytes_of(&text),
            Field::Days => {
                // Only digits count; the box shows only those.
                let digits: String = text.chars().filter(char::is_ascii_digit).collect();
                if digits != text {
                    self.days.update(cx, |input, cx| input.set_value(digits, window, cx));
                }
            }
            Field::Into => self.target = text,
            Field::Below => {
                let remotes = persistence::app(cx).map(|a| a.remotes.clone()).unwrap_or_default();
                let (clouds, cloud, below) = cloud_of(&self.target, &remotes);
                if let Some(cloud) = cloud.and_then(|ix| clouds.get(ix)) {
                    if text != below {
                        self.target = target_of(cloud, &text);
                    }
                }
            }
            _ => {}
        }
        self.edited(cx);
    }

    /// Something changed: a look at what the rule would do no longer describes it.
    fn edited(&mut self, cx: &mut Context<Self>) {
        let now = self.rule(cx);
        if now != self.shown {
            self.shown = now;
            self.preview = None;
            self.looking += 1;
        }
        cx.notify();
    }

    /// Chooses what the rule does (`chooseAction`); a sync rule takes subfolders, copies, and looks every hour.
    pub fn choose_action(&mut self, action: RuleAction, window: &mut Window, cx: &mut Context<Self>) {
        self.action = action;
        if action == RuleAction::Sync {
            let remotes = persistence::app(cx).map(|a| a.remotes.clone()).unwrap_or_default();
            let (clouds, _, _) = cloud_of(&self.target, &remotes);
            self.subfolders = true;
            self.sync = Some(self.sync.unwrap_or(SyncMode::Copy));
            self.every = Some(self.every.unwrap_or(60));
            if let (Some(first), false) = (clouds.first(), self.target.contains(':')) {
                self.target = target_of(first, "");
            }
        }
        // The two boxes that show the target show the same one.
        let (target, remotes) = (
            self.target.clone(),
            persistence::app(cx).map(|a| a.remotes.clone()).unwrap_or_default(),
        );
        let (_, _, below) = cloud_of(&target, &remotes);
        self.into.update(cx, |input, cx| input.set_value(target, window, cx));
        self.below.update(cx, |input, cx| input.set_value(below, window, cx));
        self.edited(cx);
    }

    /// Chooses the cloud a sync rule syncs with, keeping the folder on it.
    pub fn choose_cloud(&mut self, cloud: &RemoteConnection, cx: &mut Context<Self>) {
        let below = self.below.read(cx).value().to_string();
        self.target = target_of(cloud, &below);
        self.edited(cx);
    }

    /// Chooses how a sync rule syncs; both ways it looks every 15 minutes unless told otherwise.
    pub fn choose_sync(&mut self, mode: SyncMode, cx: &mut Context<Self>) {
        self.sync = Some(mode);
        if mode == SyncMode::Both && self.every.unwrap_or(0) == 0 {
            self.every = Some(15);
        }
        self.edited(cx);
    }

    /// Chooses how often a sync rule looks again.
    pub fn choose_every(&mut self, minutes: u32, cx: &mut Context<Self>) {
        self.every = Some(minutes);
        self.edited(cx);
    }

    /// Sets whether the rule takes the folder's subfolders too.
    pub fn set_subfolders(&mut self, on: bool, cx: &mut Context<Self>) {
        self.subfolders = on;
        self.edited(cx);
    }

    /// Looks at what the rule would do now to what is in its folder ("What would it do?").
    pub fn look(&mut self, cx: &mut Context<Self>) {
        self.looking += 1;
        let this_look = self.looking;
        self.preview = Some(Preview::Running);
        cx.notify();
        let asked = self.backend.preview(&self.named(cx));
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            let _ = this.update(cx, |this, cx| {
                if this.looking != this_look {
                    return;
                }
                this.preview = Some(match result {
                    Ok(found) => Preview::Found(found),
                    Err(reason) => Preview::Failed(reason.into()),
                });
                // What it found is under the fields: show it.
                this.body_scroll.scroll_to_bottom();
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves the rule, on or off, and goes back to the list.
    pub fn save(&mut self, on: bool, cx: &mut Context<Self>) {
        let rule = FolderRule { on, ..self.named(cx) };
        save_rule(&self.backend, rule, cx);
        cx.emit(EditorEvent::Close);
    }

    /// Turns the rule on, and does it to what is in the folder now (`turnOnWithThese`), saying how it went.
    pub fn turn_on_with_these(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rule = FolderRule {
            on: true,
            ..self.named(cx)
        };
        let handle = window.window_handle();
        let applied = self.backend.apply(&rule);
        self.save(true, cx);
        cx.spawn(async move |_, cx| {
            let note = match applied.await {
                Ok(_) if rule.action == RuleAction::Sync => {
                    vec![
                        Notification::success(t!("It's syncing now; What they did shows what it does."))
                            .title(t!("Rule “{name}” is on", name = rule.name)),
                    ]
                }
                Ok((did, failed)) => {
                    let mut notes = vec![Notification::success(t!(
                        "It did it to {files} there now; what comes later goes the same way.",
                        files = plural(did, "file")
                    ))
                    .title(t!("Rule “{name}” is on", name = rule.name))];
                    if failed > 0 {
                        notes.push(Notification::error(t!("What they did says why, for each.")).title(t!(
                            "Rule “{name}” couldn't do it to {files}",
                            name = rule.name,
                            files = plural(failed, "file")
                        )));
                    }
                    notes
                }
                Err(reason) => vec![Notification::error(reason).title(t!("Couldn't do it to what's there"))],
            };
            for note in note {
                tell(cx, handle, note);
            }
        })
        .detach();
    }

    /// Deletes the rule and goes back to the list.
    pub fn delete(&mut self, cx: &mut Context<Self>) {
        remove_rule(&self.backend, &self.start.id, cx);
        cx.emit(EditorEvent::Close);
    }

    /// Asks for a folder with Windows' picker and puts it in the box for `field` (`browse`).
    fn browse(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.input(field).read(cx).value().to_string();
        let start = Some(current).filter(|s| !s.trim().is_empty());
        let title = if field == Field::Folder {
            t!("Choose the folder")
        } else {
            t!("Choose where")
        };
        let picked = self.backend.pick_folder(title.to_string(), start, self.owner);
        let input = self.input(field).clone();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Some(path)) = picked.await {
                let _ = input.update_in(cx, |input, window, cx| input.set_value(path, window, cx));
            }
        })
        .detach();
    }
}

impl RuleEditor {
    /// The text box with its name for screen readers.
    fn text_box(input: &Entity<InputState>, label: SharedString) -> Input {
        Input::new(input).aria_label(label)
    }

    /// A folder box with its Browse button.
    fn folder_box(&self, field: Field, aria: SharedString, browse: SharedString, cx: &mut Context<Self>) -> Div {
        let id = if field == Field::Folder {
            "rules-browse-folder"
        } else {
            "rules-browse-target"
        };
        align_row(1.5)
            .child(div().flex_1().min_w_0().child(Self::text_box(self.input(field), aria)))
            .child(
                Button::new(id)
                    .outline()
                    .icon(IconName::FolderOpen)
                    .accessibility_label(browse)
                    .on_click(cx.listener(move |this, _, window, cx| this.browse(field, window, cx))),
            )
    }

    /// The sync rule's choices: the cloud, how, and when.
    fn sync_fields(
        &self,
        clouds: &[RemoteConnection],
        cloud: Option<usize>,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if clouds.is_empty() {
            let line = t!("Connect a cloud first: Add a connection, under Remote in the sidebar, signs in to Google Drive, OneDrive, Dropbox and 60 more.");
            return align_row(2.)
                .id("rules-no-cloud")
                .role(Role::Status)
                .aria_label(line.clone())
                .test_support()
                .items_start()
                .rounded(theme::size::RADIUS)
                .border_1()
                .border_color(t.border)
                .bg(t.primary.opacity(0.05))
                .px(space(3.))
                .py(space(2.))
                .text_size(theme::size::XS)
                .child(icon(IconName::Cloud).size_4().flex_none().text_color(t.primary))
                .child(div().flex_1().min_w_0().child(line))
                .into_any_element();
        }
        let mode = self.sync.unwrap_or(SyncMode::Copy);
        let how_hint = sync_modes()
            .into_iter()
            .find(|(m, _, _)| *m == mode)
            .map(|(_, _, hint)| hint);
        let with = field(
            t!("With"),
            None,
            row_start()
                .items_start()
                .gap(space(1.5))
                .child(
                    row_start()
                        .id("rules-cloud")
                        .role(Role::RadioGroup)
                        .aria_label(t!("Cloud"))
                        .test_support()
                        .flex_wrap()
                        .gap(space(1.5))
                        .when(cloud.is_none(), |this| {
                            this.child(
                                div()
                                    .text_size(theme::size::XS)
                                    .text_color(t.muted_foreground)
                                    .child(t!("Choose a cloud…")),
                            )
                        })
                        .children(clouds.iter().enumerate().map(|(ix, c)| {
                            let picked = c.clone();
                            radio(
                                ("rules-cloud-choice", ix),
                                SharedString::from(c.name.clone()),
                                None,
                                cloud == Some(ix),
                                t,
                                cx,
                                move |this: &mut Self, _, cx| this.choose_cloud(&picked, cx),
                            )
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Self::text_box(&self.below, t!("Folder on the cloud"))),
                ),
            t,
        );
        let how = field(
            t!("How"),
            how_hint,
            row_start()
                .id("rules-how")
                .role(Role::RadioGroup)
                .aria_label(t!("How"))
                .test_support()
                .flex_wrap()
                .gap(space(1.5))
                .children(sync_modes().into_iter().enumerate().map(|(ix, (m, label, _))| {
                    radio(
                        ("rules-how-choice", ix),
                        label,
                        None,
                        mode == m,
                        t,
                        cx,
                        move |this: &mut Self, _, cx| this.choose_sync(m, cx),
                    )
                })),
            t,
        );
        let every = self.every.unwrap_or(60);
        let when = field(
            t!("When"),
            Some(t!("Half a minute after files here stop changing, a minute after Glint starts, and as often as asked here (changes on the cloud come in then).")),
            row_start()
                .id("rules-when")
                .role(Role::RadioGroup)
                .aria_label(t!("When"))
                .test_support()
                .flex_wrap()
                .gap(space(1.5))
                .children(every_options().into_iter().enumerate().map(|(ix, (minutes, label))| {
                    radio(
                        ("rules-when-choice", ix),
                        label,
                        None,
                        every == minutes,
                        t,
                        cx,
                        move |this: &mut Self, _, cx| this.choose_every(minutes, cx),
                    )
                })),
            t,
        );
        col()
            .gap(space(3.))
            .child(with)
            .child(how)
            .child(when)
            .into_any_element()
    }

    /// What the rule would do: the heading, the files (up to 500), and the buttons that turn it on.
    fn preview_view(&self, found: &[Done], folder: &str, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let syncing = self.action == RuleAction::Sync;
        let many = found.len() >= 500;
        let heading: SharedString = if syncing {
            match (found.is_empty(), many) {
                (true, _) => t!("The two are in sync already: it would wait for changes."),
                (false, true) => t!("Its first run would do this (the first 500):"),
                (false, false) => t!("Its first run would do this:"),
            }
        } else {
            let files = plural(found.len(), "file");
            match (found.is_empty(), many) {
                (true, _) => t!("Nothing there now that it takes: it would wait for what comes."),
                (false, true) => t!("{files} there now it would take (the first 500):", files = files),
                (false, false) => t!("{files} there now it would take:", files = files),
            }
        };
        let rows: Arc<Vec<Done>> = Arc::new(found.to_vec());
        let folder_len = folder.trim_end_matches('\\').chars().count();
        let list = {
            let rows = rows.clone();
            let colors = t.clone();
            uniform_list(
                "rules-preview-list",
                rows.len(),
                cx.processor(move |_, range: Range<usize>, _, _| {
                    range
                        .filter_map(|ix| {
                            rows.get(ix)
                                .map(|row| preview_row(ix, row, syncing, folder_len, &colors).into_any_element())
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full()
        };
        let count = found.len();
        col()
            .id("rules-preview")
            .role(Role::Group)
            .aria_label(heading.clone())
            .test_support()
            .gap(space(2.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .p(space(3.))
            .child(div().font_weight(FontWeight::MEDIUM).child(heading))
            .when(count > 0, |this| {
                this.child(
                    div()
                        .relative()
                        .h(rems(PREVIEW_ROW * count.clamp(1, 10) as f32))
                        .child(
                            div()
                                .id("rules-preview-files")
                                .role(Role::List)
                                .aria_label(t!("What it would do"))
                                .test_support()
                                .size_full()
                                .child(list),
                        )
                        .child(Scrollbar::vertical(&self.scroll).viewport_from_layout()),
                )
            })
            .child(
                row_start().flex_wrap().gap(space(2.)).pt(space(1.)).child(if syncing {
                    Button::new("rules-turn-on-sync")
                        .primary()
                        .small()
                        .label(t!("Turn it on and sync now"))
                        .on_click(cx.listener(|this, _, window, cx| this.turn_on_with_these(window, cx)))
                } else {
                    Button::new("rules-turn-on")
                        .primary()
                        .small()
                        .label(t!("Turn it on: for what comes from now"))
                        .on_click(cx.listener(|this, _, _, cx| this.save(true, cx)))
                })
                .when(!syncing && count > 0, |this| {
                    this.child(
                        Button::new("rules-turn-on-now")
                            .outline()
                            .small()
                            .label(if count == 1 {
                                t!("Turn it on, and do it to this one now")
                            } else {
                                t!("Turn it on, and do it to these {n} now", n = format_count(count))
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.turn_on_with_these(window, cx))),
                    )
                }),
            )
            .child(
                div()
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(if syncing {
                        t!("It runs while Glint runs, through rclone. Each run that changes something is in What they did.")
                    } else {
                        t!("It runs while Glint runs. All it does is in What they did, each with Undo.")
                    }),
            )
            .into_any_element()
    }
}

/// One line of a preview: a sync's what and which; otherwise the file inside the folder, and where it would go.
fn preview_row(ix: usize, done: &Done, syncing: bool, folder_len: usize, t: &theme::Tokens) -> impl IntoElement {
    let base = align_row(1.5).h(rems(PREVIEW_ROW)).min_w_0().text_size(theme::size::XS);
    if syncing {
        let deletes = done.action.starts_with("Delete");
        let (arrow, tone) = if deletes {
            (IconName::X, t.destructive)
        } else if done.action.ends_with("down") {
            (IconName::ArrowDown, t.primary)
        } else {
            (IconName::ArrowUp, t.primary)
        };
        let label = format!("{} {}", done.action, done.from);
        base.id(("rules-preview-row", ix))
            .role(Role::ListItem)
            .aria_label(label)
            .test_support()
            .child(icon(arrow).size_3().flex_none().text_color(tone))
            .child(
                div()
                    .w(space(24.))
                    .flex_none()
                    .text_color(t.muted_foreground)
                    .child(SharedString::from(done.action.clone())),
            )
            .child(div().flex_1().min_w_0().truncate().child(isolate(done.from.clone())))
    } else {
        let inside: String = done.from.chars().skip(folder_len + 1).collect();
        let shown = if inside.is_empty() {
            base_name(&done.from).to_string()
        } else {
            inside
        };
        let to = done.to.clone().unwrap_or_else(|| done.action.clone());
        let label = format!("{shown} → {to}");
        base.id(("rules-preview-row", ix))
            .role(Role::ListItem)
            .aria_label(label)
            .test_support()
            .child(
                div()
                    .max_w(gpui_kit::relative(0.45))
                    .flex_none()
                    .truncate()
                    .child(isolate(shown)),
            )
            .child(icon(forward_icon()).size_3().flex_none().text_color(t.muted_foreground))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(t.muted_foreground)
                    .child(isolate(to)),
            )
    }
}

impl Render for RuleEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let known = self.known(cx);
        let rule = self.rule(cx);
        let folder_empty = rule.folder.trim().is_empty();
        let running = matches!(self.preview, Some(Preview::Running));
        let remotes = persistence::app(cx).map(|a| a.remotes.clone()).unwrap_or_default();
        let (clouds, cloud, _) = cloud_of(&self.target, &remotes);

        let then = field(
            t!("Then"),
            None,
            col()
                .id("rules-then")
                .role(Role::RadioGroup)
                .aria_label(t!("Then"))
                .test_support()
                .align_start()
                .gap(space(1.))
                .children(actions().into_iter().enumerate().map(|(ix, (action, label))| {
                    radio(
                        ("rules-then-choice", ix),
                        label,
                        None,
                        self.action == action,
                        &t,
                        cx,
                        move |this: &mut Self, window, cx| this.choose_action(action, window, cx),
                    )
                })),
            &t,
        );

        let size = field(
            t!("Size"),
            Some(t!("In MB; either can be empty.")),
            align_row(2.)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Self::text_box(&self.min, t!("At least (MB)"))),
                )
                .child(div().text_color(t.muted_foreground).child(t!("to")))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Self::text_box(&self.max, t!("At most (MB)"))),
                ),
            &t,
        );

        let preview = match &self.preview {
            None => None,
            Some(Preview::Running) => None,
            Some(Preview::Failed(reason)) => Some(
                align_row(2.)
                    .id("rules-preview-error")
                    .role(Role::Alert)
                    .aria_label(reason.clone())
                    .test_support()
                    .items_start()
                    .text_size(theme::size::XS)
                    .text_color(t.destructive)
                    .child(icon(IconName::TriangleAlert).size_3p5().flex_none().mt(space(0.5)))
                    .child(div().flex_1().min_w_0().child(reason.clone()))
                    .into_any_element(),
            ),
            Some(Preview::Found(found)) => Some(self.preview_view(found, &rule.folder, &t, cx).into_any_element()),
        };

        let fields = col()
            .gap(space(3.))
            .child(field(t!("Name"), None, Self::text_box(&self.name, t!("Name")), &t))
            .child(field(
                t!("Folder"),
                None,
                self.folder_box(Field::Folder, t!("Folder"), t!("Choose the folder"), cx),
                &t,
            ))
            .child(
                div().ps(31.).child(
                    Checkbox::new("rules-subfolders")
                        .label(t!("Its subfolders too"))
                        .checked(self.subfolders)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| this.set_subfolders(*checked, cx))),
                ),
            )
            .child(field(
                t!("Names"),
                Some(t!("Patterns with * and ?, separated by ; (empty for every file).")),
                Self::text_box(&self.names, t!("Names")),
                &t,
            ))
            .child(size)
            .child(field(
                t!("Unchanged for"),
                Some(t!("Days since it last changed (empty: as soon as it comes). Rules with an age are also checked every hour.")),
                Self::text_box(&self.days, t!("Unchanged for (days)")),
                &t,
            ))
            .child(then)
            .when(self.action == RuleAction::Move, |this| {
                this.child(field(
                    t!("Into"),
                    Some(t!("{year}, {month} and {day} of the file's date can make folders. A name that's taken gets “(2)”: nothing's replaced.")),
                    self.folder_box(Field::Into, t!("Into"), t!("Choose where"), cx),
                    &t,
                ))
            })
            .when(self.action == RuleAction::Sync, |this| {
                this.child(self.sync_fields(&clouds, cloud, &t, cx))
            })
            .when(self.action == RuleAction::Rename, |this| {
                this.child(field(
                    t!("New name"),
                    Some(t!("Of {name}, {ext}, {date}, {time}, {year}, {month} and {day} (the file's date). A name that's taken gets “(2)”.")),
                    Self::text_box(&self.pattern, t!("New name")),
                    &t,
                ))
            })
            .children(preview);
        // The buttons stay in view; the fields and what the look found scroll above them.
        let footer = align_row(2.)
            .flex_wrap()
            .pt(space(3.))
            .border_t_1()
            .border_color(t.border)
            .child(
                Button::new("rules-look")
                    .outline()
                    .small()
                    .icon(if running {
                        IconName::LoaderCircle
                    } else {
                        IconName::WandSparkles
                    })
                    .label(t!("What would it do?"))
                    .disabled(folder_empty || running)
                    .on_click(cx.listener(|this, _, _, cx| this.look(cx))),
            )
            .when(known, |this| {
                this.child(
                    Button::new("rules-delete")
                        .ghost()
                        .small()
                        .icon(IconName::Trash)
                        .text_color(t.destructive)
                        .label(t!("Delete the rule"))
                        .on_click(cx.listener(|this, _, _, cx| this.delete(cx))),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("rules-cancel")
                    .ghost()
                    .small()
                    .label(t!("Cancel"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(EditorEvent::Close))),
            )
            .child(
                Button::new("rules-save")
                    .outline()
                    .small()
                    .label(t!("Save"))
                    .tooltip(t!("Keep it as it is: on stays on, off stays off"))
                    .disabled(folder_empty)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let on = this.start.on && this.known(cx);
                        this.save(on, cx)
                    })),
            );
        col()
            .id("rules-editor")
            .role(Role::Group)
            .aria_label(t!("Edit a rule"))
            .test_support()
            .gap(space(3.))
            .child(
                div()
                    .id("rules-editor-body")
                    .max_h(rems(24.))
                    .overflow_y_scroll()
                    .track_scroll(&self.body_scroll)
                    .child(fields),
            )
            .child(footer)
    }
}

// ---- The dialog -----------------------------------------------------------------------------------------------

/// The two pages of the dialog.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    /// The rules, to turn on and off and change.
    Rules,
    /// What they did, each with Undo.
    Log,
}

/// What the log page has.
enum LogState {
    Loading,
    /// Newest first.
    Ready(Vec<Done>),
}

const LOG_ROW: f32 = 2.75;

/// Folder rules: the list, a rule's editor, and what the rules did.
pub struct RulesDialog {
    backend: Arc<dyn Backend>,
    tab: Tab,
    editor: Option<Entity<RuleEditor>>,
    log: LogState,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl RulesDialog {
    /// The dialog on the list, or (given a folder) on a new rule for it.
    pub fn new(backend: Arc<dyn Backend>, folder: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let redraw = cx.observe_global::<persistence::Persistence>(|_, cx| cx.notify());
        let mut this = Self {
            backend,
            tab: Tab::Rules,
            editor: None,
            log: LogState::Loading,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: vec![redraw],
        };
        if let Some(folder) = folder.filter(|f| !f.is_empty()) {
            this.edit(blank(&folder), window, cx);
        }
        this
    }

    /// The page shown.
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// The editor, while a rule is being made or changed.
    pub fn editor(&self) -> Option<&Entity<RuleEditor>> {
        self.editor.as_ref()
    }

    /// Opens the editor on `rule`.
    pub fn edit(&mut self, rule: FolderRule, window: &mut Window, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let editor = cx.new(|cx| RuleEditor::new(backend, rule, window, cx));
        let closed = cx.subscribe(&editor, |this, _, event: &EditorEvent, cx| match event {
            EditorEvent::Close => {
                this.editor = None;
                this._subscriptions.truncate(1);
                cx.notify();
            }
        });
        self._subscriptions.truncate(1);
        self._subscriptions.push(closed);
        self.editor = Some(editor);
        cx.notify();
    }

    /// Shows the rules or what they did.
    pub fn show_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.editor = None;
        self._subscriptions.truncate(1);
        if tab == Tab::Log {
            self.read_log(cx);
        }
        cx.notify();
    }

    /// Reads what the rules did, newest first; a failed reading is an empty log.
    fn read_log(&mut self, cx: &mut Context<Self>) {
        let reading = self.backend.log();
        cx.spawn(async move |this, cx| {
            let log = reading.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.log = LogState::Ready(log.into_iter().rev().collect());
                cx.notify();
            });
        })
        .detach();
    }

    /// Turns a rule off at once; turning one on goes through its preview first (the switch in the list).
    pub fn switch(&mut self, rule: FolderRule, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if on {
            self.edit(rule, window, cx);
        } else {
            save_rule(&self.backend, FolderRule { on: false, ..rule }, cx);
        }
    }

    /// Takes back one thing a rule did, says so, and reads the log again (`undoRule`).
    fn undo(&mut self, done: Done, window: &mut Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        let undoing = self.backend.undo(done.id);
        cx.spawn(async move |this, cx| {
            let note = match undoing.await {
                Ok(()) => Notification::success(t!("Took back: {what}", what = did_what(&done))),
                Err(reason) => Notification::error(reason).title(t!("Couldn't take it back")),
            };
            tell(cx, handle, note);
            let _ = this.update(cx, |this, cx| this.read_log(cx));
        })
        .detach();
    }

    fn clear_log(&mut self, cx: &mut Context<Self>) {
        let clearing = self.backend.clear_log();
        cx.spawn(async move |this, cx| {
            let _ = clearing.await;
            let _ = this.update(cx, |this, cx| this.read_log(cx));
        })
        .detach();
    }
}

impl RulesDialog {
    /// The list page: the rules with their switches, and New rule.
    fn list_view(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> Div {
        let rules = saved_rules(cx);
        col()
            .gap(space(3.))
            .when(rules.is_empty(), |this| {
                this.child(
                    div().text_color(t.muted_foreground).child(t!(
                        "A rule watches a folder, such as Downloads, and does something with the files that come into it: moves PDFs into Documents\\Invoices\\{year}, renames scans by their date, unblocks installers, extracts archives, or clears out what's been there a month. It's off until you turn it on, after a look at what it would do."
                    )),
                )
            })
            .children(rules.into_iter().enumerate().map(|(ix, rule)| {
                let said = summary(&rule);
                let name = if rule.name.is_empty() {
                    t!("A rule")
                } else {
                    SharedString::from(rule.name.clone())
                };
                let (for_switch, for_edit) = (rule.clone(), rule.clone());
                align_row(3.)
                    .id(("rules-rule", ix))
                    .role(Role::Group)
                    .aria_label(format!("{name}. {said}"))
                    .test_support()
                    .rounded(theme::size::RADIUS_LG)
                    .border_1()
                    .border_color(t.border)
                    .px(space(3.))
                    .py(space(2.))
                    .child(
                        Switch::new(("rules-on", ix))
                            .checked(rule.on)
                            .accessibility_label(format!("{}: on", rule.name))
                            .on_click(cx.listener(move |this, on: &bool, window, cx| {
                                this.switch(for_switch.clone(), *on, window, cx)
                            })),
                    )
                    .child(
                        col()
                            .flex_1()
                            .min_w_0()
                            .align_start()
                            .child(div().w_full().truncate().font_weight(FontWeight::MEDIUM).child(isolate(name)))
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_size(theme::size::XS)
                                    .text_color(t.muted_foreground)
                                    .child(isolate(said)),
                            ),
                    )
                    .child(
                        Button::new(("rules-edit", ix))
                            .outline()
                            .small()
                            .label(t!("Edit"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit(for_edit.clone(), window, cx)
                            })),
                    )
            }))
            .child(
                row_start().child(
                    Button::new("rules-new")
                        .primary()
                        .small()
                        .icon(IconName::Plus)
                        .label(t!("New rule"))
                        .on_click(cx.listener(|this, _, window, cx| this.edit(blank(""), window, cx))),
                ),
            )
    }

    /// The log page: each thing a rule did, newest first, with Undo.
    fn log_view(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let LogState::Ready(log) = &self.log else {
            return align_row(2.)
                .id("rules-log-loading")
                .role(Role::Status)
                .aria_label(t!("Reading what the rules did…"))
                .test_support()
                .justify_center()
                .py(space(6.))
                .text_color(t.muted_foreground)
                .child(Spinner::new())
                .into_any_element();
        };
        if log.is_empty() {
            return div()
                .id("rules-log-empty")
                .role(Role::Status)
                .aria_label(t!("Rules haven't done anything yet."))
                .test_support()
                .py(space(6.))
                .text_center()
                .text_color(t.muted_foreground)
                .child(t!("Rules haven't done anything yet."))
                .into_any_element();
        }
        let count = log.len();
        let colors = t.clone();
        let list = uniform_list(
            "rules-log-list",
            count,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                let LogState::Ready(log) = &this.log else {
                    return Vec::new();
                };
                let rows: Vec<(usize, Done)> = range
                    .filter_map(|ix| log.get(ix).cloned().map(|done| (ix, done)))
                    .collect();
                rows.into_iter()
                    .map(|(ix, done)| log_row(ix, done, &colors, cx).into_any_element())
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();
        col()
            .gap(space(2.))
            .child(
                div()
                    .relative()
                    .h(rems(LOG_ROW * count.clamp(3, 9) as f32))
                    .child(
                        div()
                            .id("rules-log")
                            .role(Role::List)
                            .aria_label(t!("What the rules did"))
                            .test_support()
                            .size_full()
                            .child(list),
                    )
                    .child(Scrollbar::vertical(&self.scroll).viewport_from_layout()),
            )
            .child(
                row_start().child(
                    Button::new("rules-clear-log")
                        .ghost()
                        .small()
                        .label(t!("Clear this list"))
                        .on_click(cx.listener(|this, _, _, cx| this.clear_log(cx))),
                ),
            )
            .into_any_element()
    }
}

/// One thing a rule did: when and which rule, what it did, where it went or why it failed, and Undo.
fn log_row(ix: usize, done: Done, t: &theme::Tokens, cx: &mut Context<RulesDialog>) -> impl IntoElement {
    let when = format_date(done.at, None, crate::i18n::language());
    let did = did_what(&done);
    let mut line = format!("{when} · {}: {did}", done.rule);
    if done.undone {
        line.push_str(&t!(" (taken back)"));
    }
    let can_undo = done.error.is_none() && !done.undone && done.to.is_some() && done.action != "sync";
    let second: Option<(SharedString, bool)> = match (&done.error, &done.to) {
        (Some(error), _) => Some((error.clone().into(), true)),
        (None, Some(to)) => Some((to.clone().into(), false)),
        _ => None,
    };
    let label = match &second {
        Some((second, _)) => format!("{line}. {second}"),
        None => line.clone(),
    };
    let for_undo = done.clone();
    align_row(3.)
        .id(("rules-log-row", ix))
        .role(Role::ListItem)
        .aria_label(label)
        .test_support()
        .h(rems(LOG_ROW))
        .px(space(2.))
        .when(done.undone, |this| this.opacity(0.6))
        .child(
            col()
                .flex_1()
                .min_w_0()
                .align_start()
                .child(div().w_full().truncate().child(isolate(line)))
                .when_some(second, |this, (second, error)| {
                    this.child(
                        div()
                            .w_full()
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(if error { t.destructive } else { t.muted_foreground })
                            .child(isolate(second)),
                    )
                }),
        )
        .when(can_undo, |this| {
            this.child(
                Button::new(("rules-undo", ix))
                    .outline()
                    .small()
                    .icon(IconName::Undo2)
                    .label(t!("Undo"))
                    .on_click(cx.listener(move |this, _, window, cx| this.undo(for_undo.clone(), window, cx))),
            )
        })
}

impl Render for RulesDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let tab = |id: &'static str, which: Tab, label: SharedString, cx: &mut Context<Self>| {
            let on = self.tab == which;
            Button::new(id)
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(if on {
                            t.primary.opacity(0.15)
                        } else {
                            t.background.opacity(0.)
                        })
                        .foreground(if on { t.primary } else { t.muted_foreground })
                        .hover(t.accent)
                        .active(t.accent),
                )
                .compact()
                .role(Role::Tab)
                .toggled(on)
                .accessibility_label(label.clone())
                .on_click(cx.listener(move |this, _, _, cx| this.show_tab(which, cx)))
                .child(div().text_size(theme::size::XS).child(label))
        };
        let body = match (&self.editor, self.tab) {
            (_, Tab::Log) => self.log_view(&t, cx),
            // The editor scrolls its fields itself and keeps its buttons in view.
            (Some(editor), _) => editor.clone().into_any_element(),
            (None, _) => div()
                .id("rules-body")
                .max_h(rems(30.))
                .overflow_y_scroll()
                .child(self.list_view(&t, cx))
                .into_any_element(),
        };
        col()
            .id("rules")
            .role(Role::Group)
            .aria_label(t!("Folder rules"))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .child(
                row_start()
                    .items_center()
                    .justify_between()
                    .gap(space(3.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(t!(
                                "Files that come into a folder, sorted out as they come, while Glint runs"
                            )),
                    )
                    .child(
                        row_start()
                            .id("rules-tabs")
                            .role(Role::TabList)
                            .aria_label(t!("Rules or what they did"))
                            .test_support()
                            .flex_none()
                            .gap(space(0.5))
                            .rounded(theme::size::RADIUS)
                            .border_1()
                            .border_color(t.border)
                            .p(space(0.5))
                            .child(tab("rules-tab-rules", Tab::Rules, t!("Rules"), cx))
                            .child(tab("rules-tab-log", Tab::Log, t!("What they did"), cx)),
                    ),
            )
            .child(div().id("rules-body").max_h(rems(34.)).overflow_y_scroll().child(body))
    }
}

/// Opens Folder rules in `window`: the list, or (given a folder) a new rule for it (`openRules`). The menu's
/// Rule… and Settings' Folder rules use it.
pub fn open(window: &mut Window, cx: &mut App, folder: Option<String>) {
    let backend = backend(cx);
    open_with(window, cx, backend, folder);
}

/// [`open`] for a given backend (tests pass a fake).
pub fn open_with(window: &mut Window, cx: &mut App, backend: Arc<dyn Backend>, folder: Option<String>) {
    let view = cx.new(|cx| RulesDialog::new(backend, folder, window, cx));
    super::show(window, cx, t!("Folder rules"), px(672.), view);
}

// ---- Tests ----------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A backend that answers from memory and remembers what it was asked.
    #[derive(Default)]
    struct Fake {
        found: Mutex<Vec<Done>>,
        preview_error: Mutex<Option<String>>,
        applied: Mutex<Vec<FolderRule>>,
        apply_result: Mutex<(usize, usize)>,
        log: Mutex<Vec<Done>>,
        calls: Mutex<Vec<String>>,
        pushed: Mutex<Vec<Vec<FolderRule>>>,
        picked: Mutex<Option<String>>,
    }

    impl Fake {
        fn said(&self, what: impl Into<String>) {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(what.into());
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        fn pushed(&self) -> Vec<Vec<FolderRule>> {
            self.pushed.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl Backend for Fake {
        fn preview(&self, rule: &FolderRule) -> Fut<Vec<Done>> {
            self.said(format!("preview {}", rule.name));
            let error = self.preview_error.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let found = self.found.lock().unwrap_or_else(|e| e.into_inner()).clone();
            Box::pin(async move { error.map_or(Ok(found), Err) })
        }

        fn apply(&self, rule: &FolderRule) -> Fut<(usize, usize)> {
            self.said(format!("apply {}", rule.name));
            self.applied
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(rule.clone());
            let result = *self.apply_result.lock().unwrap_or_else(|e| e.into_inner());
            Box::pin(async move { Ok(result) })
        }

        fn log(&self) -> Fut<Vec<Done>> {
            self.said("log");
            let log = self.log.lock().unwrap_or_else(|e| e.into_inner()).clone();
            Box::pin(async move { Ok(log) })
        }

        fn clear_log(&self) -> Fut<()> {
            self.said("clear");
            self.log.lock().unwrap_or_else(|e| e.into_inner()).clear();
            Box::pin(async { Ok(()) })
        }

        fn undo(&self, id: u64) -> Fut<()> {
            self.said(format!("undo {id}"));
            Box::pin(async { Ok(()) })
        }

        fn set_rules(&self, rules: Vec<FolderRule>) -> Fut<()> {
            self.pushed.lock().unwrap_or_else(|e| e.into_inner()).push(rules);
            Box::pin(async { Ok(()) })
        }

        fn pick_folder(&self, _: String, _: Option<String>, _: isize) -> Fut<Option<String>> {
            let picked = self.picked.lock().unwrap_or_else(|e| e.into_inner()).clone();
            Box::pin(async move { Ok(picked) })
        }
    }

    fn done(id: u64, rule: &str, action: &str, from: &str, to: Option<&str>) -> Done {
        Done {
            id,
            at: 1_700_000_000_000,
            rule: rule.into(),
            action: action.into(),
            from: from.into(),
            to: to.map(String::from),
            mark: None,
            error: None,
            undone: false,
            summary: None,
        }
    }

    fn rule(id: &str, name: &str, folder: &str) -> FolderRule {
        FolderRule {
            id: id.into(),
            name: name.into(),
            folder: folder.into(),
            names: "*.pdf".into(),
            action: RuleAction::Move,
            target: "D:\\Docs\\{year}".into(),
            ..FolderRule::default()
        }
    }

    fn cloud(id: &str, name: &str, rclone: &str, root: &str) -> RemoteConnection {
        RemoteConnection {
            id: id.into(),
            name: name.into(),
            kind: RemoteKind::Rclone,
            rclone: Some(rclone.into()),
            root: root.into(),
            ..RemoteConnection::default()
        }
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

    fn has_id(cx: &mut VisualTestContext, id: impl Into<ElementId>) -> bool {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
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

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the dialog never reached the expected state");
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

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    /// An app with a saved state in a temporary folder (holding `rules` and `remotes`), and a window.
    fn window(
        cx: &mut TestAppContext,
        rules: Vec<FolderRule>,
        remotes: Vec<RemoteConnection>,
    ) -> (VisualTestContext, Arc<Fake>) {
        cx.update(|cx| {
            testing::init(cx);
            let dir = std::env::temp_dir().join(format!(
                "glint-rules-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join("state.json");
            let _ = std::fs::remove_file(&file);
            persistence::init_at(cx, file);
            persistence::update_app(cx, |app| {
                app.rules = rules;
                app.remotes = remotes;
            });
        });
        let fake = Arc::new(Fake::default());
        (testing::open_window(cx, |_, cx| cx.new(|_| Host)).0, fake)
    }

    fn open_view(cx: &mut VisualTestContext, fake: &Arc<Fake>, folder: Option<&str>) -> Entity<RulesDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let folder = folder.map(String::from);
        let view = cx.update(|window, cx| {
            let view = cx.new(|cx| RulesDialog::new(backend, folder, window, cx));
            super::super::show(window, cx, t!("Folder rules"), px(672.), view.clone());
            view
        });
        cx.run_until_parked();
        view
    }

    fn editor_of(cx: &mut VisualTestContext, view: &Entity<RulesDialog>) -> Entity<RuleEditor> {
        view.read_with(cx, |v, _| v.editor().cloned())
            .expect("an editor is open")
    }

    fn saved(cx: &mut VisualTestContext) -> Vec<FolderRule> {
        cx.update(|_, cx| saved_rules(cx))
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Plus,
            IconName::FolderOpen,
            IconName::Cloud,
            IconName::Trash,
            IconName::Undo2,
            IconName::ArrowUp,
            IconName::ArrowDown,
            IconName::ArrowRight,
            IconName::ArrowLeft,
            IconName::X,
            IconName::WandSparkles,
            IconName::LoaderCircle,
            IconName::TriangleAlert,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn summaries_read_as_the_typescript_wrote_them() {
        let _guard = testing::lock();
        let mut move_rule = rule("r1", "Invoices", "C:\\Users\\me\\Downloads");
        assert_eq!(
            summary(&move_rule),
            "In Downloads, *.pdf: move it into a folder D:\\Docs\\{year}"
        );
        move_rule.subfolders = true;
        move_rule.names = "  ".into();
        move_rule.older_than_days = 30;
        assert_eq!(
            summary(&move_rule),
            "In Downloads and below, every file, unchanged 30 days: move it into a folder D:\\Docs\\{year}"
        );
        let renamer = FolderRule {
            action: RuleAction::Rename,
            pattern: "{date} {name}{ext}".into(),
            older_than_days: 1,
            ..rule("r2", "", "D:\\")
        };
        assert_eq!(
            summary(&renamer),
            "In D:, *.pdf, unchanged 1 day: rename it to {date} {name}{ext}"
        );
        let unblock = FolderRule {
            action: RuleAction::Unblock,
            ..rule("r3", "", "C:\\Installers")
        };
        assert_eq!(summary(&unblock), "In Installers, *.pdf: unblock it");
        let sync = FolderRule {
            action: RuleAction::Sync,
            sync: Some(SyncMode::Both),
            target: "gdrive:Backups".into(),
            subfolders: true,
            names: String::new(),
            ..rule("r4", "", "C:\\Photos")
        };
        assert_eq!(
            summary(&sync),
            "Photos and below, every file: both ways (gdrive:Backups)"
        );
        let copy = FolderRule { sync: None, ..sync };
        assert_eq!(
            summary(&copy),
            "Photos and below, every file: copy there (gdrive:Backups)"
        );
    }

    #[test]
    fn what_a_rule_did_is_said_in_a_few_words() {
        let _guard = testing::lock();
        assert_eq!(
            did_what(&done(1, "r", "move", "C:\\a\\x.pdf", Some("D:\\x.pdf"))),
            "moved “x.pdf”"
        );
        assert_eq!(
            did_what(&done(1, "r", "recycle", "C:\\a\\x.pdf", None)),
            "put in the Recycle Bin “x.pdf”"
        );
        assert_eq!(did_what(&done(1, "r", "weird", "C:\\a\\x.pdf", None)), "weird “x.pdf”");
        let sync = Done {
            summary: Some("3 Up, 1 Down".into()),
            ..done(1, "r", "sync", "C:\\Photos", Some("gdrive:Photos"))
        };
        assert_eq!(did_what(&sync), "synced “Photos” with gdrive:Photos: 3 up, 1 down");
        assert_eq!(
            did_what(&done(1, "r", "sync", "C:\\Photos", None)),
            "synced “Photos” with the cloud"
        );
    }

    #[test]
    fn clouds_and_their_folders_are_told_apart_as_the_typescript_did() {
        let remotes = vec![
            cloud("a", "Drive", "gdrive", ""),
            cloud("b", "Drive backups", "gdrive", "Backups"),
            RemoteConnection {
                kind: RemoteKind::S3,
                ..cloud("c", "Not rclone", "s3", "")
            },
        ];
        let (clouds, found, below) = cloud_of("gdrive:Backups/2026", &remotes);
        assert_eq!(clouds.len(), 2, "only rclone's connections");
        // The first that matches by name and root; an empty root matches anything.
        assert_eq!(found, Some(0));
        assert_eq!(below, "Backups/2026");
        let (_, found, below) = cloud_of("gdrive:Backups/2026", &remotes[1..]);
        assert_eq!(found, Some(0));
        assert_eq!(below, "2026");
        let (_, found, below) = cloud_of("other:Stuff", &remotes);
        assert_eq!((found, below.as_str()), (None, "Stuff"));
        let (_, found, below) = cloud_of("no colon", &remotes);
        assert_eq!((found, below.as_str()), (None, ""));
        assert_eq!(target_of(&remotes[1], ""), "gdrive:Backups");
        assert_eq!(target_of(&remotes[1], " /2026/ "), "gdrive:Backups/2026");
        assert_eq!(target_of(&remotes[0], "x"), "gdrive:x");
    }

    #[test]
    fn sizes_ids_and_the_core_rule_follow_the_typescript() {
        assert_eq!(megabytes(0), "");
        assert_eq!(megabytes(1 << 20), "1");
        assert_eq!(megabytes(1_572_864), "1.5");
        assert_eq!(megabytes(1000), "0");
        assert_eq!(bytes_of("1,5"), 1_572_864);
        assert_eq!(bytes_of(" 2 "), 2 << 20);
        assert_eq!(bytes_of("abc"), 0);
        assert_eq!(bytes_of(""), 0);
        assert_eq!(bytes_of("-3"), 0);
        assert_eq!(bytes_of("inf"), 0);
        let (a, b) = (rule_id(), rule_id());
        assert!(a.starts_with("rule-") && a != b);
        let fresh = blank("C:\\Downloads");
        assert_eq!(
            (fresh.action, fresh.folder.as_str(), fresh.pattern.as_str()),
            (RuleAction::Move, "C:\\Downloads", "{date} {name}{ext}")
        );
        assert!(!fresh.on && fresh.name.is_empty());
        // core reads the saved rule's JSON: choices as words, sizes as numbers.
        let sync = FolderRule {
            action: RuleAction::Sync,
            sync: Some(SyncMode::Mirror),
            every: Some(15),
            min_size: 5,
            older_than_days: 3,
            ..rule("r", "n", "C:\\f")
        };
        let core = to_core(&sync);
        assert_eq!(
            (core.action.as_str(), core.sync.as_str(), core.every),
            ("sync", "mirror", 15)
        );
        assert_eq!(
            (core.min_size, core.older_than_days, core.names.as_str()),
            (5, 3, "*.pdf")
        );
        assert_eq!(to_core(&rule("r", "n", "C:\\f")).sync, "");
    }

    #[gpui_kit::test]
    fn an_empty_list_explains_rules_and_new_rule_opens_the_editor(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = window(cx, Vec::new(), Vec::new());
        let view = open_view(&mut visual, &fake, None);
        assert_eq!(role_of(&mut visual, "Folder rules"), Some(Role::Group));
        assert!(has_id(&mut visual, "rules-tabs"));
        assert!(!has_id(&mut visual, ("rules-rule", 0_usize)));
        click(&mut visual, "rules-new");
        assert!(view.read_with(&visual, |v, _| v.editor().is_some()));
        assert_eq!(role_of(&mut visual, "Edit a rule"), Some(Role::Group));
        let editor = editor_of(&mut visual, &view);
        // A new rule is a move, off, with the TypeScript's rename pattern ready.
        let rule = editor.read_with(&visual, |e, cx| e.rule(cx));
        assert_eq!(
            (rule.action, rule.on, rule.pattern.as_str()),
            (RuleAction::Move, false, "{date} {name}{ext}")
        );
        // Nothing to save without a folder.
        click(&mut visual, "rules-save");
        assert!(saved(&mut visual).is_empty(), "Save is off while the folder is empty");
        click(&mut visual, "rules-cancel");
        assert!(view.read_with(&visual, |v, _| v.editor().is_none()));
    }

    #[gpui_kit::test]
    fn the_list_names_each_rule_and_its_switch_and_turning_off_is_at_once(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let on = FolderRule {
            on: true,
            ..rule("r1", "Invoices", "C:\\Users\\me\\Downloads")
        };
        let (mut visual, fake) = window(cx, vec![on, rule("r2", "", "D:\\Scans")], Vec::new());
        let view = open_view(&mut visual, &fake, None);
        assert_eq!(
            role_of(
                &mut visual,
                "Invoices. In Downloads, *.pdf: move it into a folder D:\\Docs\\{year}"
            ),
            Some(Role::Group)
        );
        assert!(has(
            &mut visual,
            "A rule. In Scans, *.pdf: move it into a folder D:\\Docs\\{year}"
        ));
        assert!(has(&mut visual, "Invoices: on"));
        click(&mut visual, ("rules-on", 0_usize));
        assert!(!saved(&mut visual)[0].on, "off is saved at once");
        assert!(
            view.read_with(&visual, |v, _| v.editor().is_none()),
            "no editor to turn off"
        );
        let pushed = fake.pushed();
        assert_eq!(pushed.len(), 1, "core is told");
        assert!(pushed[0].iter().all(|r| !r.on));
    }

    #[gpui_kit::test]
    fn turning_a_rule_on_looks_first_then_saves_it_on(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = window(cx, vec![rule("r1", "Invoices", "C:\\Users\\me\\Downloads")], Vec::new());
        *fake.found.lock().unwrap() = vec![
            done(
                0,
                "Invoices",
                "Move",
                "C:\\Users\\me\\Downloads\\a.pdf",
                Some("D:\\Docs\\2026\\a.pdf"),
            ),
            done(
                0,
                "Invoices",
                "Move",
                "C:\\Users\\me\\Downloads\\sub\\b.pdf",
                Some("D:\\Docs\\2026\\b.pdf"),
            ),
        ];
        let view = open_view(&mut visual, &fake, None);
        click(&mut visual, ("rules-on", 0_usize));
        // It was asked to look without a click, and says what it found.
        settle(&mut visual, |cx| has(cx, "2 files there now it would take:"));
        assert_eq!(fake.calls(), ["preview Invoices"]);
        assert_eq!(
            role_of(&mut visual, "2 files there now it would take:"),
            Some(Role::Group)
        );
        assert_eq!(
            role_of(&mut visual, "a.pdf → D:\\Docs\\2026\\a.pdf"),
            Some(Role::ListItem)
        );
        assert_eq!(
            role_of(&mut visual, "sub\\b.pdf → D:\\Docs\\2026\\b.pdf"),
            Some(Role::ListItem)
        );
        assert!(!saved(&mut visual)[0].on, "nothing is on until the person says so");
        assert!(has_id(&mut visual, "rules-turn-on-now"));
        click(&mut visual, "rules-turn-on");
        assert!(saved(&mut visual)[0].on);
        assert!(view.read_with(&visual, |v, _| v.editor().is_none()));
        assert!(fake.pushed().last().is_some_and(|rules| rules[0].on));
    }

    #[gpui_kit::test]
    fn a_new_rule_for_a_folder_is_looked_at_changed_and_saved_with_a_name(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = window(cx, Vec::new(), Vec::new());
        let view = open_view(&mut visual, &fake, Some("C:\\Users\\me\\Downloads"));
        let editor = editor_of(&mut visual, &view);
        let rule = editor.read_with(&visual, |e, cx| e.rule(cx));
        assert_eq!(
            (rule.folder.as_str(), rule.name.as_str()),
            ("C:\\Users\\me\\Downloads", "")
        );
        // Nothing found: it says it would wait.
        click(&mut visual, "rules-look");
        let nothing = "Nothing there now that it takes: it would wait for what comes.";
        settle(&mut visual, |cx| has(cx, nothing));
        assert!(!has_id(&mut visual, "rules-turn-on-now"), "nothing to do now");
        assert!(has_id(&mut visual, "rules-turn-on"));
        // Any change makes the look stale.
        let names = editor.read_with(&visual, |e, _| e.input(Field::Names).clone());
        type_into(&mut visual, &names, "*.zip");
        assert!(!has(&mut visual, nothing));
        assert_eq!(editor.read_with(&visual, |e, cx| e.rule(cx).names), "*.zip");
        // A look that fails says why.
        *fake.preview_error.lock().unwrap() = Some("That folder isn't there".into());
        click(&mut visual, "rules-look");
        settle(&mut visual, |cx| has(cx, "That folder isn't there"));
        assert_eq!(role_of(&mut visual, "That folder isn't there"), Some(Role::Alert));
        // The default name is "In <folder>"; Save keeps it off.
        click(&mut visual, "rules-save");
        let rules = saved(&mut visual);
        assert_eq!(rules.len(), 1);
        assert_eq!(
            (rules[0].name.as_str(), rules[0].on, rules[0].names.as_str()),
            ("In Downloads", false, "*.zip")
        );
        assert!(rules[0].id.starts_with("rule-"));
        assert!(view.read_with(&visual, |v, _| v.editor().is_none()));
    }

    #[gpui_kit::test]
    fn boxes_for_sizes_and_days_keep_only_what_they_can_mean(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = window(cx, Vec::new(), Vec::new());
        let view = open_view(&mut visual, &fake, Some("C:\\Downloads"));
        let editor = editor_of(&mut visual, &view);
        let (min, max, days) = editor.read_with(&visual, |e, _| {
            (
                e.input(Field::Min).clone(),
                e.input(Field::Max).clone(),
                e.input(Field::Days).clone(),
            )
        });
        type_into(&mut visual, &min, "1,5");
        type_into(&mut visual, &max, "20");
        type_into(&mut visual, &days, "12ab");
        let rule = editor.read_with(&visual, |e, cx| e.rule(cx));
        assert_eq!(
            (rule.min_size, rule.max_size, rule.older_than_days),
            (1_572_864, 20 << 20, 12)
        );
        assert_eq!(
            days.read_with(&visual, |i, _| i.value().to_string()),
            "12",
            "letters don't stay in the box"
        );
    }

    #[gpui_kit::test]
    fn turning_on_and_doing_it_now_applies_saves_and_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = window(cx, Vec::new(), Vec::new());
        *fake.found.lock().unwrap() = vec![done(0, "In Scans", "Move", "D:\\Scans\\a.pdf", Some("D:\\Docs\\a.pdf"))];
        *fake.apply_result.lock().unwrap() = (1, 1);
        let view = open_view(&mut visual, &fake, Some("D:\\Scans"));
        click(&mut visual, "rules-look");
        settle(&mut visual, |cx| has(cx, "1 file there now it would take:"));
        click(&mut visual, "rules-turn-on-now");
        settle(&mut visual, |_| !fake.applied.lock().unwrap().is_empty());
        let applied = fake.applied.lock().unwrap().clone();
        assert_eq!((applied[0].name.as_str(), applied[0].on), ("In Scans", true));
        assert!(saved(&mut visual)[0].on);
        // The result is told in notifications: that it is on, and that one file could not be done.
        settle(&mut visual, |cx| {
            cx.update(|window, cx| window.notifications(cx).len()) >= 2
        });
        assert!(view.read_with(&visual, |v, _| v.editor().is_none()));
    }

    #[gpui_kit::test]
    fn the_folder_button_puts_what_the_picker_chose_in_the_box(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = window(cx, Vec::new(), Vec::new());
        *fake.picked.lock().unwrap() = Some("E:\\Chosen".into());
        let view = open_view(&mut visual, &fake, Some("C:\\Downloads"));
        let editor = editor_of(&mut visual, &view);
        assert_eq!(role_of(&mut visual, "Choose the folder"), Some(Role::Button));
        click(&mut visual, "rules-browse-folder");
        settle(&mut visual, |cx| {
            editor.read_with(cx, |e, cx| e.rule(cx).folder == "E:\\Chosen")
        });
        // The other button, for where a move goes, is there for a move rule.
        assert!(has_id(&mut visual, "rules-browse-target"));
    }

    #[gpui_kit::test]
    fn a_sync_rule_needs_a_cloud_and_takes_its_defaults_and_choices(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let remotes = vec![
            cloud("c1", "Google Drive", "gdrive", "Backups"),
            cloud("c2", "OneDrive", "od", ""),
        ];
        let (mut visual, fake) = window(cx, Vec::new(), remotes);
        let view = open_view(&mut visual, &fake, Some("C:\\Photos"));
        let editor = editor_of(&mut visual, &view);
        click(&mut visual, ("rules-then-choice", 0_usize));
        // The defaults of the TypeScript: subfolders, copying, every hour, the first cloud.
        let rule = editor.read_with(&visual, |e, cx| e.rule(cx));
        assert_eq!(
            (
                rule.action,
                rule.subfolders,
                rule.sync,
                rule.every,
                rule.target.as_str()
            ),
            (RuleAction::Sync, true, Some(SyncMode::Copy), Some(60), "gdrive:Backups")
        );
        assert_eq!(role_of(&mut visual, "Cloud"), Some(Role::RadioGroup));
        assert_eq!(role_of(&mut visual, "Google Drive"), Some(Role::RadioButton));
        assert!(has(&mut visual, "Copy there"));
        assert!(has(&mut visual, "Also every hour"));
        // Another cloud, a folder on it, both ways, and a different time.
        let second = cloud("c2", "OneDrive", "od", "");
        editor.update(&mut visual, |e, cx| e.choose_cloud(&second, cx));
        let below = editor.read_with(&visual, |e, _| e.input(Field::Below).clone());
        type_into(&mut visual, &below, "Photos/2026");
        // These two are below the fold of the scrolling fields; the choices themselves are the same calls.
        editor.update(&mut visual, |e, cx| {
            e.choose_sync(SyncMode::Both, cx);
            e.choose_every(360, cx);
        });
        let rule = editor.read_with(&visual, |e, cx| e.rule(cx));
        assert_eq!(
            (rule.sync, rule.every, rule.target.as_str()),
            (Some(SyncMode::Both), Some(360), "od:Photos/2026")
        );
        assert!(has(
            &mut visual,
            "Changes on either side go to the other (rclone bisync). Where a file changed on both, the newer wins and the other is kept as name.conflict1."
        ));
        // The first look of a sync rule is rclone's dry run; its lines say what and which.
        *fake.found.lock().unwrap() = vec![
            done(0, "In Photos", "Copy up", "a.jpg", None),
            done(0, "In Photos", "Delete down", "b.jpg", None),
        ];
        click(&mut visual, "rules-look");
        settle(&mut visual, |cx| has(cx, "Its first run would do this:"));
        assert_eq!(role_of(&mut visual, "Copy up a.jpg"), Some(Role::ListItem));
        assert_eq!(role_of(&mut visual, "Delete down b.jpg"), Some(Role::ListItem));
        assert!(has_id(&mut visual, "rules-turn-on-sync"));
        assert!(!has_id(&mut visual, "rules-turn-on-now"));
        click(&mut visual, "rules-save");
        assert_eq!(saved(&mut visual)[0].target, "od:Photos/2026");
    }

    #[gpui_kit::test]
    fn a_sync_rule_without_a_cloud_says_to_connect_one(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = window(cx, Vec::new(), Vec::new());
        let view = open_view(&mut visual, &fake, Some("C:\\Photos"));
        click(&mut visual, ("rules-then-choice", 0_usize));
        assert!(has_id(&mut visual, "rules-no-cloud"));
        assert!(!has_id(&mut visual, "rules-how"));
        click(&mut visual, "rules-cancel");
        assert!(view.read_with(&visual, |v, _| v.editor().is_none()));
    }

    #[gpui_kit::test]
    fn deleting_a_saved_rule_removes_it_and_tells_core(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = window(cx, vec![rule("r1", "Invoices", "C:\\Downloads")], Vec::new());
        let view = open_view(&mut visual, &fake, None);
        click(&mut visual, ("rules-edit", 0_usize));
        assert!(has_id(&mut visual, "rules-delete"));
        click(&mut visual, "rules-delete");
        assert!(saved(&mut visual).is_empty());
        assert!(view.read_with(&visual, |v, _| v.editor().is_none()));
        assert!(fake.pushed().last().is_some_and(|rules| rules.is_empty()));
    }

    fn log_fake(fake: &Fake) {
        let moved = done(1, "Invoices", "move", "C:\\Downloads\\a.pdf", Some("D:\\Docs\\a.pdf"));
        let failed = Done {
            error: Some("Access is denied".into()),
            ..done(2, "Invoices", "move", "C:\\Downloads\\b.pdf", None)
        };
        let back = Done {
            undone: true,
            ..done(
                3,
                "Invoices",
                "rename",
                "C:\\Downloads\\c.pdf",
                Some("C:\\Downloads\\2026 c.pdf"),
            )
        };
        let sync = done(4, "Photos", "sync", "C:\\Photos", Some("gdrive:Photos"));
        *fake.log.lock().unwrap() = vec![moved, failed, back, sync];
    }

    fn log_label(cx: &mut VisualTestContext, ix: usize) -> String {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find(("rules-log-row", ix))
                .and_then(|e| e.label().map(str::to_string))
                .unwrap_or_default()
        })
    }

    #[gpui_kit::test]
    fn the_log_lists_newest_first_with_undo_where_it_can_be_and_clears(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = window(cx, Vec::new(), Vec::new());
        log_fake(&fake);
        let view = open_view(&mut visual, &fake, None);
        click(&mut visual, "rules-tab-log");
        assert_eq!(view.read_with(&visual, |v, _| v.tab()), Tab::Log);
        settle(&mut visual, |cx| has_id(cx, ("rules-log-row", 0_usize)));
        // Newest first: the sync, the taken-back rename, the failure, the move.
        let first = log_label(&mut visual, 0);
        assert!(first.contains("Photos: synced “Photos” with gdrive:Photos"), "{first}");
        assert!(log_label(&mut visual, 1).contains("renamed “c.pdf” (taken back)"));
        assert!(log_label(&mut visual, 2).ends_with("Access is denied"));
        let last = log_label(&mut visual, 3);
        assert!(
            last.contains("Invoices: moved “a.pdf”") && last.ends_with("D:\\Docs\\a.pdf"),
            "{last}"
        );
        // Undo only where something can be taken back: not a sync, a failure or what was taken back already.
        assert!(!has_id(&mut visual, ("rules-undo", 0_usize)));
        assert!(!has_id(&mut visual, ("rules-undo", 1_usize)));
        assert!(!has_id(&mut visual, ("rules-undo", 2_usize)));
        assert!(has_id(&mut visual, ("rules-undo", 3_usize)));
        click(&mut visual, ("rules-undo", 3_usize));
        settle(&mut visual, |_| fake.calls().iter().any(|c| c == "undo 1"));
        // The log is read again afterwards.
        settle(&mut visual, |_| {
            fake.calls().iter().filter(|c| *c == "log").count() == 2
        });
        click(&mut visual, "rules-clear-log");
        settle(&mut visual, |cx| has(cx, "Rules haven't done anything yet."));
        assert_eq!(
            role_of(&mut visual, "Rules haven't done anything yet."),
            Some(Role::Status)
        );
        // Back to the rules.
        click(&mut visual, "rules-tab-rules");
        assert_eq!(view.read_with(&visual, |v, _| v.tab()), Tab::Rules);
    }

    #[gpui_kit::test]
    fn the_tabs_are_named(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = window(cx, Vec::new(), Vec::new());
        open_view(&mut visual, &fake, None);
        assert_eq!(role_of(&mut visual, "Rules or what they did"), Some(Role::TabList));
        assert_eq!(role_of(&mut visual, "Rules"), Some(Role::Tab));
        assert_eq!(role_of(&mut visual, "What they did"), Some(Role::Tab));
    }

    #[gpui_kit::test]
    fn escape_closes_the_dialog_and_focus_starts_inside_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = window(cx, Vec::new(), Vec::new());
        open_view(&mut visual, &fake, None);
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            assert!(
                window.focused(cx).is_some(),
                "nothing has focus after the dialog opened"
            );
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Escape must close the dialog"));
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render_without_trouble(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let arabic = FolderRule {
            names: "*.pdf".into(),
            ..rule("r1", "فواتير", "C:\\Users\\أنا\\التنزيلات")
        };
        let (mut visual, fake) = window(cx, vec![arabic], Vec::new());
        testing::set_rtl(true);
        let view = open_view(&mut visual, &fake, None);
        assert!(has(&mut visual, "فواتير: on"));
        assert!(has(
            &mut visual,
            "فواتير. In التنزيلات, *.pdf: move it into a folder D:\\Docs\\{year}"
        ));
        click(&mut visual, ("rules-edit", 0_usize));
        let editor = editor_of(&mut visual, &view);
        assert_eq!(
            editor.read_with(&visual, |e, cx| e.rule(cx).folder),
            "C:\\Users\\أنا\\التنزيلات"
        );
        *fake.found.lock().unwrap() = vec![done(
            0,
            "فواتير",
            "Move",
            "C:\\Users\\أنا\\التنزيلات\\فاتورة.pdf",
            Some("D:\\Docs\\فاتورة.pdf"),
        )];
        click(&mut visual, "rules-look");
        settle(&mut visual, |cx| has(cx, "فاتورة.pdf → D:\\Docs\\فاتورة.pdf"));
        testing::set_rtl(false);
    }
}
