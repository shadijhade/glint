//! Checking files: passwords and keys (Kingfisher), YARA rules (YARA-X) and Microsoft Defender.
//! Ported from src/components/explorer/CheckDialog.tsx and src/lib/defender.ts, built the way
//! [`super::drive_health`] is.
//!
//! | TypeScript                                   | Here                                                              |
//! |----------------------------------------------|-------------------------------------------------------------------|
//! | `useCheck` store, `run`                      | [`Check`], a model entity (`Running`, `NoTool`, `NoRules`, ...)   |
//! | `checkForSecrets(target)`                    | [`open`] with [`What::Secrets`]                                   |
//! | `checkWithYara(target)`                      | [`open`] with [`What::Yara`]                                      |
//! | `scanWithDefender(paths)` (a toast)          | [`open`] with [`What::Defender`] (the same dialog, with Stop)     |
//! | `secretsCheckFirst(paths, label, go)`        | [`secrets_check_first`]                                           |
//! | `secretsScan`, `yaraScan`, `defenderScan`    | [`Backend`] ([`CoreBackend`] is glint-core, tests pass a fake)     |
//! | `ToolNeeded`                                 | `settings::tools::needed` (the install card)                      |
//! | `CheckDialog`                                | [`CheckDialog`]                                                   |
//!
//! **How the coordinator wires it.**
//! * `MenuEvent::Defender { paths }` is `dialogs::check::open(window, cx, paths, What::Defender)`.
//! * `MenuEvent::Secrets { path }` is `open(window, cx, vec![path], What::Secrets)`.
//! * `MenuEvent::Yara { path }` is `open(window, cx, vec![path], What::Yara)`.
//! * The status bar's check before compressing (and sharing) is
//!   `secrets_check_first(window, cx, paths, label, go)`: `label` is the button's words ("Zip anyway") and
//!   `go` runs when the person skips the check, when nothing is found (or Kingfisher is missing or fails),
//!   or when they choose `label` in the dialog that lists what was found.
//!
//! The findings list is virtualized: every row has the same height, a file's name and then its findings.

use crate::components::settings::tools::{self, Tools};
use crate::components::settings::{Backend as SettingsBackend, CoreBackend as SettingsCoreBackend};
use crate::kit::prelude::*;
use crate::{core_bridge, explorer, state, t, theme};
use glint_core::check::{CheckReport, Finding};
use glint_core::defender::{DefenderReport, Threat};
use glint_model::format::{format_count, format_duration, plural};
use glint_model::paths::{base_name, parent_path};
use gpui_kit::UniformListScrollHandle;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// Which check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    /// Passwords, keys and tokens left in files (Kingfisher, offline).
    Secrets,
    /// Files against the YARA rules chosen in Settings (YARA-X).
    Yara,
    /// A custom scan by Microsoft Defender.
    Defender,
}

impl What {
    /// The tool that does it (`TOOL`); Defender comes with Windows.
    fn tool(self) -> Option<&'static str> {
        match self {
            What::Secrets => Some("kingfisher"),
            What::Yara => Some("yara-x"),
            What::Defender => None,
        }
    }
}

/// The calls a check makes. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// Whether the tool `id` is installed.
    fn tool_installed(&self, id: &'static str) -> Fut<bool>;
    /// `secrets_scan`.
    fn secrets(&self, target: String) -> Fut<CheckReport>;
    /// `yara_scan`.
    fn yara(&self, rules: String, target: String) -> Fut<CheckReport>;
    /// `defender_scan`.
    fn defender(&self, target: String) -> Fut<DefenderReport>;
    /// Stops the scan of this kind that is running (`cancel_check`, `cancel_defender_scan`).
    fn cancel(&self, what: What);
    /// Asks for a folder (the YARA rules); `None` when the person cancels.
    fn pick_folder(&self, title: String, start: Option<String>, owner: isize) -> Fut<Option<String>>;
}

/// glint-core.
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
    fn tool_installed(&self, id: &'static str) -> Fut<bool> {
        Box::pin(async move {
            glint_core::fsx::blocking(move || {
                Ok(glint_core::tools::get(id).is_some_and(|tool| glint_core::proc::locate(tool.exe).is_some()))
            })
            .await
        })
    }

    fn secrets(&self, target: String) -> Fut<CheckReport> {
        Box::pin(glint_core::check::secrets_scan(target))
    }

    fn yara(&self, rules: String, target: String) -> Fut<CheckReport> {
        Box::pin(glint_core::check::yara_scan(rules, target))
    }

    fn defender(&self, target: String) -> Fut<DefenderReport> {
        Box::pin(glint_core::defender::defender_scan(target))
    }

    fn cancel(&self, what: What) {
        match what {
            What::Defender => glint_core::defender::cancel_defender_scan(),
            _ => glint_core::check::cancel_check(),
        }
    }

    fn pick_folder(&self, title: String, start: Option<String>, owner: isize) -> Fut<Option<String>> {
        SettingsCoreBackend::new(self.app.clone()).pick_folder(title, start, owner)
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn tool_installed(&self, _: &'static str) -> Fut<bool> {
        Box::pin(async { Ok(false) })
    }
    fn secrets(&self, _: String) -> Fut<CheckReport> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn yara(&self, _: String, _: String) -> Fut<CheckReport> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn defender(&self, _: String) -> Fut<DefenderReport> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn cancel(&self, _: What) {}
    fn pick_folder(&self, _: String, _: Option<String>, _: isize) -> Fut<Option<String>> {
        Box::pin(async { Ok(None) })
    }
}

/// The app's backend: core's if it is started.
fn core_backend(cx: &App) -> Arc<dyn Backend> {
    match cx.try_global::<core_bridge::Core>() {
        Some(core) => Arc::new(CoreBackend::new(core.app.clone())),
        None => Arc::new(Unavailable),
    }
}

// ---- The model --------------------------------------------------------------------------------------

/// What a finished check found, whatever the kind.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    /// Secrets, or YARA rules matched.
    pub findings: Vec<Finding>,
    /// Defender's threats.
    pub threats: Vec<Threat>,
    pub elapsed_ms: f64,
}

impl Found {
    /// How many things were found.
    pub fn count(&self) -> usize {
        self.findings.len() + self.threats.len()
    }
}

/// Where a check stands (`running`, `report` and `error` of the TypeScript's store, as one value).
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The tool is looking.
    Running,
    /// The tool isn't installed ("noTool").
    NoTool,
    /// YARA has no rules folder ("noRules").
    NoRules,
    /// The person stopped it.
    Stopped,
    /// It failed, with the reason.
    Failed(SharedString),
    /// It finished.
    Done(Found),
}

/// What runs after a check, once the person chose to go on.
pub type Go = Rc<dyn Fn(&mut Window, &mut App)>;

/// What the person asked for after the check, when it found something (Zip anyway).
#[derive(Clone)]
pub struct Then {
    pub label: SharedString,
    pub run: Go,
}

/// One check of some paths. Closing the dialog drops the model, which stops a scan under way.
pub struct Check {
    what: What,
    paths: Vec<String>,
    rules: String,
    outcome: Outcome,
    backend: Arc<dyn Backend>,
    /// The number of the run whose answer counts; an older run's answer is dropped.
    run: u64,
    stopped: Arc<AtomicBool>,
    rows: Arc<Vec<Row>>,
}

impl Check {
    /// A check of `paths` that has not started. `rules` is Settings' YARA rules folder.
    pub fn new(what: What, paths: Vec<String>, rules: String, backend: Arc<dyn Backend>) -> Self {
        Self {
            what,
            paths,
            rules,
            outcome: Outcome::Running,
            backend,
            run: 0,
            stopped: Arc::new(AtomicBool::new(false)),
            rows: Arc::new(Vec::new()),
        }
    }

    /// A check that is already done (the check before compressing found something).
    pub fn with_found(what: What, paths: Vec<String>, found: Found, backend: Arc<dyn Backend>) -> Self {
        let mut this = Self::new(what, paths, String::new(), backend);
        this.set_outcome(Outcome::Done(found));
        this
    }

    pub fn what(&self) -> What {
        self.what
    }

    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }

    /// The folder of YARA rules in use.
    pub fn rules(&self) -> &str {
        &self.rules
    }

    /// Where a finding is shown from: the file, or the folder of the first of several.
    pub fn target(&self) -> String {
        match self.paths.as_slice() {
            [one] => one.clone(),
            [first, ..] => parent_path(first),
            [] => String::new(),
        }
    }

    /// What is checked, for words: "“name”" or "3 items".
    pub fn subject(&self) -> SharedString {
        match self.paths.as_slice() {
            [one] => t!("“{name}”", name = name_of(one)),
            many => plural(many.len(), "item").into(),
        }
    }

    fn set_outcome(&mut self, outcome: Outcome) {
        self.rows = Arc::new(match &outcome {
            Outcome::Done(found) => rows_of(found, &self.target(), self.what == What::Defender),
            _ => Vec::new(),
        });
        self.outcome = outcome;
    }

    /// Starts the check (again): a run already under way is forgotten (`run`).
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.run += 1;
        let run = self.run;
        self.stopped = Arc::new(AtomicBool::new(false));
        self.set_outcome(Outcome::Running);
        cx.notify();
        let work = scan(
            self.backend.clone(),
            self.what,
            self.paths.clone(),
            self.rules.clone(),
            self.stopped.clone(),
        );
        cx.spawn(async move |this, cx| {
            let outcome = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.run == run {
                    this.set_outcome(outcome);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Stops the scan under way (`cancelCheck`).
    pub fn stop(&mut self) {
        if self.outcome == Outcome::Running {
            self.stopped.store(true, Ordering::SeqCst);
            self.backend.cancel(self.what);
        }
    }

    /// Uses another folder of rules and checks again.
    pub fn use_rules(&mut self, rules: String, cx: &mut Context<Self>) {
        self.rules = rules;
        self.start(cx);
    }
}

impl Drop for Check {
    fn drop(&mut self) {
        // The dialog closed: a scan still running is stopped, as `close` did.
        self.stop();
    }
}

/// Runs the check on core's runtime, one path after another, and says how it ended.
fn scan(
    backend: Arc<dyn Backend>,
    what: What,
    paths: Vec<String>,
    rules: String,
    stopped: Arc<AtomicBool>,
) -> Pin<Box<dyn Future<Output = Outcome> + Send>> {
    Box::pin(async move {
        if let Some(tool) = what.tool() {
            let installed = core_bridge::run(backend.tool_installed(tool)).await;
            if !matches!(installed, Ok(Ok(true))) {
                return Outcome::NoTool;
            }
            if what == What::Yara && rules.is_empty() {
                return Outcome::NoRules;
            }
        }
        let mut found = Found::default();
        for path in paths {
            if stopped.load(Ordering::SeqCst) {
                return Outcome::Stopped;
            }
            let answer = match what {
                What::Secrets => core_bridge::run(backend.secrets(path)).await.and_then(|r| r).map(|r| {
                    found.elapsed_ms += r.elapsed_ms;
                    found.findings.extend(r.findings);
                }),
                What::Yara => core_bridge::run(backend.yara(rules.clone(), path))
                    .await
                    .and_then(|r| r)
                    .map(|r| {
                        found.elapsed_ms += r.elapsed_ms;
                        found.findings.extend(r.findings);
                    }),
                What::Defender => core_bridge::run(backend.defender(path)).await.and_then(|r| r).map(|r| {
                    found.elapsed_ms += r.elapsed_ms;
                    found.threats.extend(r.threats);
                }),
            };
            match answer {
                Ok(()) => {}
                Err(reason) if reason == "cancelled" => return Outcome::Stopped,
                Err(reason) => return Outcome::Failed(reason.into()),
            }
        }
        if stopped.load(Ordering::SeqCst) {
            return Outcome::Stopped;
        }
        Outcome::Done(found)
    })
}

/// Where a finding is, from the folder checked (`relative`).
pub fn relative(path: &str, target: &str) -> String {
    let base = target.trim_end_matches('\\');
    let prefix = format!("{}\\", base.to_lowercase());
    if path.to_lowercase().starts_with(&prefix) {
        path[base.len() + 1..].to_string()
    } else {
        let name = base_name(path);
        if name.is_empty() { path } else { name }.to_string()
    }
}

fn name_of(path: &str) -> String {
    let name = base_name(path);
    if name.is_empty() { path } else { name }.to_string()
}

// ---- The rows ---------------------------------------------------------------------------------------

/// One line of the findings list. Every row is the same height, so the list can be virtualized.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// A file with findings: its path (shown from the folder checked), and how many.
    File {
        path: String,
        shown: String,
    },
    Finding(Finding),
    /// A threat Defender found.
    Threat(String),
    /// Where Defender found it.
    Resource(String),
}

/// The rows for a report: findings grouped by file in the order files first appear (`byFile`), or the
/// threats with their resources.
fn rows_of(found: &Found, target: &str, defender: bool) -> Vec<Row> {
    let mut rows = Vec::new();
    if defender {
        for threat in &found.threats {
            rows.push(Row::Threat(threat.name.clone()));
            rows.extend(threat.resources.iter().cloned().map(Row::Resource));
        }
        return rows;
    }
    let mut files: Vec<(&str, Vec<&Finding>)> = Vec::new();
    for finding in &found.findings {
        match files.iter_mut().find(|(path, _)| *path == finding.path) {
            Some((_, list)) => list.push(finding),
            None => files.push((&finding.path, vec![finding])),
        }
    }
    for (path, list) in files {
        rows.push(Row::File {
            path: path.to_string(),
            shown: relative(path, target),
        });
        rows.extend(list.into_iter().cloned().map(Row::Finding));
    }
    rows
}

/// How many files the findings are in.
pub fn files_in(found: &Found) -> usize {
    let mut paths: Vec<&str> = found.findings.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    paths.dedup();
    paths.len()
}

// ---- The dialog -------------------------------------------------------------------------------------

/// Every row's height, in rems.
const ROW_HEIGHT: f32 = 1.875;

/// What a check found (YARA rules matched, secrets, Defender's threats), by file.
pub struct CheckDialog {
    check: Entity<Check>,
    tools: Option<Entity<Tools>>,
    then: Option<Then>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl CheckDialog {
    /// The view of `check`; it starts the check unless it is done already.
    pub fn new(check: Entity<Check>, tools: Option<Entity<Tools>>, then: Option<Then>, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.observe(&check, |_, _, cx| cx.notify())];
        if let Some(tools) = &tools {
            subscriptions.push(cx.observe(tools, |_, _, cx| cx.notify()));
        }
        if matches!(check.read(cx).outcome(), Outcome::Running) {
            check.update(cx, |check, cx| check.start(cx));
        }
        Self {
            check,
            tools,
            then,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn choose_rules(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let start = Some(self.check.read(cx).rules().to_string()).filter(|s| !s.is_empty());
        let picked = {
            let backend = self.check.read(cx).backend.clone();
            backend.pick_folder(t!("A folder of YARA rules (.yar, .yara)").to_string(), start, owner)
        };
        let check = self.check.clone();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(folder))) = core_bridge::run(picked).await {
                let _ = cx.update(|_, cx| {
                    state::update(cx, |settings| settings.yara_rules = folder.clone());
                    check.update(cx, |check, cx| check.use_rules(folder, cx));
                });
            }
        })
        .detach();
    }

    fn title_line(what: What, subject: &SharedString) -> SharedString {
        match what {
            What::Secrets => t!("Passwords and keys in {name}", name = subject),
            What::Yara => t!("YARA rules in {name}", name = subject),
            What::Defender => t!("Microsoft Defender in {name}", name = subject),
        }
    }

    fn why_tool(what: What) -> SharedString {
        match what {
            What::Yara => t!("YARA-X (VirusTotal's) checks files against YARA rules."),
            _ => t!("Kingfisher, MongoDB's secret scanner, finds passwords, keys and tokens left in files. It runs on this PC only."),
        }
    }

    fn centered(
        id: &'static str,
        role: Role,
        label: SharedString,
        lead: AnyElement,
        lines: Vec<AnyElement>,
    ) -> impl IntoElement {
        col()
            .id(id)
            .role(role)
            .aria_label(label)
            .test_support()
            .items_center()
            .gap(space(2.))
            .py(space(10.))
            .text_center()
            .text_size(theme::size::SM)
            .child(lead)
            .children(lines)
    }

    fn rows_list(
        &self,
        rows: Arc<Vec<Row>>,
        secrets: bool,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = t.clone();
        let mono = theme::mono_font(cx);
        let count = rows.len();
        let list = uniform_list(
            "check-rows",
            count,
            cx.processor(move |_, range: Range<usize>, _, _| {
                range
                    .filter_map(|ix| {
                        rows.get(ix)
                            .map(|row| render_row(ix, row, &colors, secrets, &mono).into_any_element())
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();
        div()
            .relative()
            .h(rems(ROW_HEIGHT * count.clamp(4, 12) as f32))
            .child(
                div()
                    .id("check-findings")
                    .role(Role::List)
                    .aria_label(t!("Findings"))
                    .test_support()
                    .size_full()
                    .child(list),
            )
            .child(Scrollbar::vertical(&self.scroll).viewport_from_layout())
    }
}

/// One row: a file with its buttons, a finding, a threat or the place it was found.
fn render_row(ix: usize, row: &Row, t: &theme::Tokens, secrets: bool, mono: &SharedString) -> impl IntoElement {
    let base = row_start()
        .h(rems(ROW_HEIGHT))
        .items_center()
        .gap(space(2.))
        .px(space(3.))
        .text_size(theme::size::XS);
    match row {
        Row::File { path, shown } => {
            let open_path = path.clone();
            let reveal_path = path.clone();
            base.id(("check-file", ix))
                .role(Role::Group)
                .aria_label(shown.clone())
                .test_support()
                .border_t_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .child(isolate(shown.clone())),
                )
                .child(
                    Button::new(("check-open", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::ExternalLink)
                        .tooltip(t!("Open the file"))
                        .accessibility_label(format!("{}: {}", t!("Open the file"), shown))
                        .on_click(move |_, _, cx| {
                            if let Some(explorer) = explorer::try_explorer(cx) {
                                explorer.update(cx, |explorer, cx| explorer.open_file(&open_path, None, cx).detach());
                            }
                        }),
                )
                .child(
                    Button::new(("check-reveal", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::FolderInput)
                        .tooltip(t!("Show it in its folder"))
                        .accessibility_label(format!("{}: {}", t!("Show it in its folder"), shown))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            if let Some(explorer) = explorer::try_explorer(cx) {
                                let folder = parent_path(&reveal_path);
                                let select = Some(vec![reveal_path.clone()]);
                                explorer.update(cx, |explorer, cx| {
                                    explorer.navigate(
                                        folder,
                                        explorer::NavOptions {
                                            select,
                                            ..Default::default()
                                        },
                                        cx,
                                    )
                                });
                            }
                        }),
                )
                .into_any_element()
        }
        Row::Finding(f) => {
            let rule_text = finding_words(f, secrets);
            let line = (f.line > 0).then(|| t!("line {n}", n = f.line));
            let high = f.confidence == "high";
            base.id(("check-finding", ix))
                .role(Role::ListItem)
                .aria_label(finding_label(f, secrets))
                .test_support()
                .ps(5.)
                .text_color(t.muted_foreground)
                .children(line.map(|line| div().w(rems(3.5)).flex_none().child(line)))
                .child(div().flex_1().min_w_0().truncate().child(rule_text))
                .children((!f.value.is_empty()).then(|| {
                    div()
                        .flex_none()
                        .px(space(1.5))
                        .rounded(theme::size::RADIUS_SM)
                        .bg(t.muted)
                        .text_color(t.foreground)
                        .font_family(mono.clone())
                        .child(f.value.clone())
                }))
                .children((!f.confidence.is_empty()).then(|| {
                    div()
                        .flex_none()
                        .px(space(1.5))
                        .rounded(theme::size::RADIUS_SM)
                        .text_size(theme::size::XXS)
                        .font_weight(FontWeight::MEDIUM)
                        .bg(if high { t.destructive.opacity(0.15) } else { t.muted })
                        .text_color(if high { t.destructive } else { t.muted_foreground })
                        .child(f.confidence.to_uppercase())
                }))
                .into_any_element()
        }
        Row::Threat(name) => base
            .id(("check-threat", ix))
            .role(Role::ListItem)
            .aria_label(name.clone())
            .test_support()
            .border_t_1()
            .border_color(t.border)
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.destructive)
            .child(icon(IconName::ShieldAlert).size_4())
            .child(div().flex_1().min_w_0().truncate().child(name.clone()))
            .into_any_element(),
        Row::Resource(path) => base
            .id(("check-resource", ix))
            .role(Role::ListItem)
            .aria_label(path.clone())
            .test_support()
            .ps(6.)
            .text_color(t.muted_foreground)
            .child(div().flex_1().min_w_0().truncate().child(isolate(path.clone())))
            .into_any_element(),
    }
}

/// What a finding is called: a secret's kind in words, or a YARA rule's name and namespace.
fn finding_words(f: &Finding, secrets: bool) -> String {
    if secrets {
        if f.title.is_empty() {
            f.rule.clone()
        } else {
            f.title.clone()
        }
    } else if f.title.is_empty() {
        f.rule.clone()
    } else {
        format!("{} · {}", f.rule, f.title)
    }
}

/// A finding as a screen reader says it.
fn finding_label(f: &Finding, secrets: bool) -> String {
    let mut parts = Vec::new();
    if f.line > 0 {
        parts.push(t!("line {n}", n = f.line).to_string());
    }
    parts.push(finding_words(f, secrets));
    if !f.value.is_empty() {
        parts.push(f.value.clone());
    }
    if !f.confidence.is_empty() {
        parts.push(f.confidence.clone());
    }
    parts.join(", ")
}

impl Render for CheckDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let check = self.check.read(cx);
        let what = check.what();
        let outcome = check.outcome().clone();
        let rules = check.rules().to_string();
        let target = check.target();
        let subject = check.subject();
        let rows = check.rows.clone();
        let running = outcome == Outcome::Running;
        let secrets = what == What::Secrets;
        let found_count = match &outcome {
            Outcome::Done(found) => found.count(),
            _ => 0,
        };
        let offers_then = self.then.is_some() && found_count > 0 && !running;

        let header = align_row(3.)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(isolate(target.clone())),
            )
            .children(running.then(|| {
                Button::new("check-stop")
                    .outline()
                    .xsmall()
                    .icon(IconName::Square)
                    .label(t!("Stop"))
                    .on_click(cx.listener(|this, _, _, cx| this.check.update(cx, |check, _| check.stop())))
            }))
            .child(
                Button::new("check-refresh")
                    .ghost()
                    .xsmall()
                    .icon(IconName::RefreshCw)
                    .loading(running)
                    .disabled(running)
                    .tooltip(t!("Refresh"))
                    .accessibility_label(t!("Refresh"))
                    .on_click(cx.listener(|this, _, _, cx| this.check.update(cx, |check, cx| check.start(cx)))),
            );

        let body: AnyElement = match &outcome {
            Outcome::Running => {
                let words = match what {
                    What::Secrets => t!("Kingfisher is looking through {name}…", name = subject),
                    What::Yara => t!(
                        "YARA-X is checking {name} with {rules}…",
                        name = subject,
                        rules = name_of(&rules)
                    ),
                    What::Defender => t!("Microsoft Defender is scanning {name}…", name = subject),
                };
                Self::centered(
                    "check-running",
                    Role::Status,
                    words.clone(),
                    Spinner::new().color(t.primary).into_any_element(),
                    vec![div().text_color(t.muted_foreground).child(words).into_any_element()],
                )
                .into_any_element()
            }
            Outcome::NoTool => {
                let tool = what.tool().unwrap_or_default();
                let why = Self::why_tool(what);
                let installed = self
                    .tools
                    .as_ref()
                    .is_some_and(|tools| tools.read(cx).tool(tool).is_some_and(|info| info.origin.is_some()));
                let card = self
                    .tools
                    .as_ref()
                    .and_then(|tools| {
                        tools::needed(
                            tools,
                            tool,
                            why.clone(),
                            if secrets { IconName::KeyRound } else { IconName::ScanSearch },
                            cx,
                        )
                    })
                    .unwrap_or_else(|| {
                        div()
                            .id("check-no-tool")
                            .role(Role::Alert)
                            .aria_label(why.clone())
                            .test_support()
                            .p(space(3.))
                            .child(why.clone())
                            .into_any_element()
                    });
                col()
                    .id("check-needs-tool")
                    .gap(space(3.))
                    .child(card)
                    .children(installed.then(|| {
                        Button::new("check-now")
                            .primary()
                            .small()
                            .label(t!("Check now"))
                            .on_click(cx.listener(|this, _, _, cx| this.check.update(cx, |check, cx| check.start(cx))))
                    }))
                    .into_any_element()
            }
            Outcome::NoRules => Self::centered(
                "check-no-rules",
                Role::Group,
                t!("Which YARA rules?"),
                icon(IconName::ScanSearch).size_6().text_color(t.muted_foreground).into_any_element(),
                vec![
                    div().font_weight(FontWeight::MEDIUM).child(t!("Which YARA rules?")).into_any_element(),
                    div()
                        .max_w(rems(28.))
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(t!("A folder of .yar and .yara files, such as one of the community collections; it's kept in Settings → Check."))
                        .into_any_element(),
                    Button::new("check-choose-rules")
                        .primary()
                        .small()
                        .icon(IconName::FolderOpen)
                        .label(t!("Choose a rules folder"))
                        .on_click(cx.listener(|this, _, window, cx| this.choose_rules(window, cx)))
                        .into_any_element(),
                ],
            )
            .into_any_element(),
            Outcome::Stopped | Outcome::Failed(_) => {
                let (headline, reason) = match &outcome {
                    Outcome::Failed(reason) => (t!("Couldn't check"), Some(reason.clone())),
                    _ => (t!("Stopped"), None),
                };
                let label = match &reason {
                    Some(reason) => format!("{headline}. {reason}"),
                    None => headline.to_string(),
                };
                let mut lines = vec![div().font_weight(FontWeight::MEDIUM).child(headline).into_any_element()];
                if let Some(reason) = reason {
                    lines.push(
                        div()
                            .max_w(rems(32.))
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(reason)
                            .into_any_element(),
                    );
                }
                Self::centered(
                    "check-error",
                    if matches!(outcome, Outcome::Failed(_)) { Role::Alert } else { Role::Status },
                    label.into(),
                    icon(IconName::TriangleAlert).size_6().text_color(t.destructive).into_any_element(),
                    lines,
                )
                .into_any_element()
            }
            Outcome::Done(_) if found_count == 0 => {
                let (headline, note) = match what {
                    What::Secrets => (
                        t!("No passwords or keys found"),
                        t!("Kingfisher looked through the files as they are (not Git's history), and nothing left the PC."),
                    ),
                    What::Yara => (t!("No rule matched"), t!("Checked with {rules}.", rules = rules)),
                    What::Defender => (
                        t!("No threats found in {name}", name = subject),
                        t!("Microsoft Defender · {took}", took = match &outcome {
                            Outcome::Done(found) => format_duration(found.elapsed_ms),
                            _ => String::new(),
                        }),
                    ),
                };
                Self::centered(
                    "check-clean",
                    Role::Status,
                    format!("{headline}. {note}").into(),
                    icon(IconName::ShieldCheck).size_6().text_color(t.success).into_any_element(),
                    vec![
                        div().font_weight(FontWeight::MEDIUM).child(headline).into_any_element(),
                        div()
                            .max_w(rems(32.))
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(note)
                            .into_any_element(),
                    ],
                )
                .into_any_element()
            }
            Outcome::Done(found) => {
                let warning: Option<SharedString> = match what {
                    What::Secrets => Some(t!(
                        "These look like passwords, keys or tokens. Anyone who gets these files can use them: take them out, or change them where they work."
                    )),
                    What::Defender => {
                        let n = found.threats.len();
                        Some(t!(
                            "Microsoft Defender dealt with {them} as its settings say: Windows Security's Protection history lists {them}.",
                            them = if n == 1 { t!("it") } else { t!("them") }
                        ))
                    }
                    What::Yara => None,
                };
                col()
                    .gap(space(2.))
                    .children(warning.map(|warning| {
                        div()
                            .id("check-warning")
                            .role(Role::Alert)
                            .aria_label(warning.clone())
                            .test_support()
                            .px(space(3.))
                            .py(space(2.))
                            .rounded(theme::size::RADIUS)
                            .bg(t.warning.opacity(0.12))
                            .text_size(theme::size::XS)
                            .text_color(t.warning)
                            .child(warning)
                    }))
                    .child(
                        div()
                            .rounded(theme::size::RADIUS_LG)
                            .border_1()
                            .border_color(t.border)
                            .overflow_hidden()
                            .child(self.rows_list(rows, secrets, &t, cx)),
                    )
                    .into_any_element()
            }
        };

        let summary: SharedString = match &outcome {
            Outcome::Done(found) if found_count > 0 || what == What::Defender => {
                let files = files_in(found);
                let n = found_count;
                let what_found = if what == What::Defender {
                    if n == 1 {
                        t!("1 threat")
                    } else {
                        t!("{n} threats", n = format_count(n))
                    }
                } else if n == 1 {
                    t!("1 finding")
                } else {
                    t!("{n} findings", n = format_count(n))
                };
                let in_files = if what != What::Defender && files > 1 {
                    t!(" in {n} files", n = format_count(files))
                } else {
                    "".into()
                };
                format!("{what_found}{in_files} · {}", format_duration(found.elapsed_ms)).into()
            }
            _ => "".into(),
        };

        col()
            .id("check")
            .role(Role::Group)
            .aria_label(Self::title_line(what, &subject))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            .text_start()
            .pt(space(1.))
            .child(header)
            .child(body)
            .child(
                align_row(3.)
                    .pt(space(2.))
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(
                        row_start()
                            .flex_1()
                            .min_w_0()
                            .gap(space(2.))
                            .child(div().truncate().child(summary))
                            .children((what == What::Yara && !rules.is_empty()).then(|| {
                                Button::new("check-other-rules")
                                    .ghost()
                                    .xsmall()
                                    .label(t!("Other rules…"))
                                    .on_click(cx.listener(|this, _, window, cx| this.choose_rules(window, cx)))
                            })),
                    )
                    .children(self.then.clone().filter(|_| offers_then).map(|then| {
                        let go = then.run.clone();
                        Button::new("check-then")
                            .outline()
                            .small()
                            .label(then.label.clone())
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                go(window, cx);
                            })
                    }))
                    .child(
                        {
                            let button = Button::new("check-close").small();
                            if offers_then {
                                button.label(t!("Cancel"))
                            } else {
                                button.outline().label(t!("Close"))
                            }
                        }
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
            )
    }
}

// ---- Opening it -------------------------------------------------------------------------------------

/// Checks `paths` (files, or folders and what's in them) in a dialog: the secrets check, the YARA rules
/// chosen in Settings, or a Microsoft Defender scan. Nothing opens for no paths.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>, what: What) {
    if paths.is_empty() {
        return;
    }
    let backend = core_backend(cx);
    let tools = tools_for(what, window, cx);
    open_with(window, cx, backend, tools, paths, what, None);
}

/// [`open`] with a backend of your own (tests pass a fake), the install card's tools if any, and what to
/// offer after a check that found something. Returns the view.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    tools: Option<Entity<Tools>>,
    paths: Vec<String>,
    what: What,
    then: Option<Then>,
) -> Entity<CheckDialog> {
    let rules = state::settings(cx).yara_rules.clone();
    let check = cx.new(|_| Check::new(what, paths, rules, backend));
    show_check(window, cx, check, tools, then)
}

fn show_check(
    window: &mut Window,
    cx: &mut App,
    check: Entity<Check>,
    tools: Option<Entity<Tools>>,
    then: Option<Then>,
) -> Entity<CheckDialog> {
    let (what, subject) = {
        let check = check.read(cx);
        (check.what(), check.subject())
    };
    let view = cx.new(|cx| CheckDialog::new(check, tools, then, cx));
    super::show(
        window,
        cx,
        CheckDialog::title_line(what, &subject),
        px(720.),
        view.clone(),
    );
    view
}

/// The tools model the install card reads (none for Defender, which Windows has, or without core).
fn tools_for(what: What, window: &mut Window, cx: &mut App) -> Option<Entity<Tools>> {
    what.tool()?;
    let core = cx.try_global::<core_bridge::Core>()?;
    let backend: Arc<dyn SettingsBackend> = Arc::new(SettingsCoreBackend::new(core.app.clone()));
    Some(cx.new(|cx| Tools::new(backend, window, cx)))
}

/// Marks the "looking first" notification, so it can be taken down.
struct LookingFirst;

/// Before `paths` leave the PC (zipped, shared): with Kingfisher installed, looks for secrets in them first
/// and asks before going on when it finds any; otherwise `go` runs straight away. The notification's Skip
/// stops the check and goes on (`secretsCheckFirst`).
pub fn secrets_check_first(
    window: &mut Window,
    cx: &mut App,
    paths: Vec<String>,
    label: SharedString,
    go: impl Fn(&mut Window, &mut App) + 'static,
) {
    let backend = core_backend(cx);
    secrets_check_first_with(window, cx, backend, paths, label, go);
}

/// [`secrets_check_first`] with a backend of your own.
pub fn secrets_check_first_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    paths: Vec<String>,
    label: SharedString,
    go: impl Fn(&mut Window, &mut App) + 'static,
) {
    let go: Go = Rc::new(go);
    if paths.is_empty() {
        return go(window, cx);
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let skip_backend = backend.clone();
    let skip_flag = stopped.clone();
    let looking = t!("Looking for passwords and keys first…");
    window.push_notification(
        Notification::new()
            .id1::<LookingFirst>("looking-first")
            .title(looking)
            .autohide(false)
            .action(move |_, _, _| {
                let backend = skip_backend.clone();
                let flag = skip_flag.clone();
                Button::new("check-skip").label(t!("Skip")).on_click(move |_, _, _| {
                    flag.store(true, Ordering::SeqCst);
                    backend.cancel(What::Secrets);
                })
            }),
        cx,
    );
    let work = scan(
        backend.clone(),
        What::Secrets,
        paths.clone(),
        String::new(),
        stopped.clone(),
    );
    window
        .spawn(cx, async move |cx| {
            let outcome = work.await;
            let _ = cx.update(|window, cx| {
                window.remove_notification1::<LookingFirst>("looking-first", cx);
                match outcome {
                    // Nothing found, skipped, no Kingfisher, or it failed: it's an offer, not a gate.
                    Outcome::Done(found) if found.count() > 0 => {
                        let tools = tools_for(What::Secrets, window, cx);
                        let check = cx.new(|_| Check::with_found(What::Secrets, paths, found, backend));
                        let then = Then { label, run: go };
                        show_check(window, cx, check, tools, Some(then));
                    }
                    _ => go(window, cx),
                }
            });
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
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    use std::time::Duration;

    fn finding(path: &str, line: u32, rule: &str, value: &str, confidence: &str) -> Finding {
        Finding {
            path: path.into(),
            line,
            rule: rule.into(),
            title: String::new(),
            value: value.into(),
            confidence: confidence.into(),
        }
    }

    fn secret(path: &str, line: u32) -> Finding {
        Finding {
            title: "AWS access key".into(),
            ..finding(path, line, "kingfisher.aws.1", "AKIA••••K4PD", "high")
        }
    }

    /// A backend answering from what the test sets, counting the calls.
    struct Fake {
        installed: bool,
        secrets: Mutex<Result<CheckReport, String>>,
        yara: Mutex<Result<CheckReport, String>>,
        defender: Mutex<Result<DefenderReport, String>>,
        /// The scan waits for this to fire (Stop sends "cancelled" through it); `None` answers at once.
        gate: Mutex<Option<async_channel::Receiver<String>>>,
        gate_tx: async_channel::Sender<String>,
        picked: Mutex<Option<String>>,
        cancelled: AtomicUsize,
        asked: AtomicUsize,
        yara_rules: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new() -> Arc<Self> {
            let (gate_tx, _rx) = async_channel::bounded(1);
            Arc::new(Self {
                installed: true,
                secrets: Mutex::new(Ok(CheckReport {
                    findings: vec![],
                    elapsed_ms: 12.0,
                })),
                yara: Mutex::new(Ok(CheckReport {
                    findings: vec![],
                    elapsed_ms: 12.0,
                })),
                defender: Mutex::new(Ok(DefenderReport {
                    clean: true,
                    threats: vec![],
                    elapsed_ms: 1500.0,
                })),
                gate: Mutex::new(None),
                gate_tx,
                picked: Mutex::new(None),
                cancelled: AtomicUsize::new(0),
                asked: AtomicUsize::new(0),
                yara_rules: Mutex::new(Vec::new()),
            })
        }

        fn gated() -> Arc<Self> {
            let (tx, rx) = async_channel::bounded(1);
            let mut fake = Arc::try_unwrap(Self::new()).ok().unwrap();
            fake.gate_tx = tx;
            *fake.gate.get_mut().unwrap() = Some(rx);
            Arc::new(fake)
        }

        fn wait(&self) -> Option<async_channel::Receiver<String>> {
            self.gate.lock().unwrap().clone()
        }
    }

    impl Backend for Fake {
        fn tool_installed(&self, _: &'static str) -> Fut<bool> {
            let installed = self.installed;
            Box::pin(async move { Ok(installed) })
        }

        fn secrets(&self, _: String) -> Fut<CheckReport> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            let answer = self.secrets.lock().unwrap().clone();
            let gate = self.wait();
            Box::pin(async move {
                if let Some(gate) = gate {
                    return Err(gate.recv().await.unwrap_or_default());
                }
                answer
            })
        }

        fn yara(&self, rules: String, _: String) -> Fut<CheckReport> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.yara_rules.lock().unwrap().push(rules);
            let answer = self.yara.lock().unwrap().clone();
            Box::pin(async move { answer })
        }

        fn defender(&self, _: String) -> Fut<DefenderReport> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            let answer = self.defender.lock().unwrap().clone();
            let gate = self.wait();
            Box::pin(async move {
                if let Some(gate) = gate {
                    return Err(gate.recv().await.unwrap_or_default());
                }
                answer
            })
        }

        fn cancel(&self, _: What) {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
            let _ = self.gate_tx.try_send("cancelled".into());
        }

        fn pick_folder(&self, _: String, _: Option<String>, _: isize) -> Fut<Option<String>> {
            let picked = self.picked.lock().unwrap().clone();
            Box::pin(async move { Ok(picked) })
        }
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
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

    fn settle(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("never happened: {what}");
    }

    fn open_fake(
        cx: &mut VisualTestContext,
        fake: &Arc<Fake>,
        paths: &[&str],
        what: What,
        then: Option<Then>,
    ) -> Entity<CheckDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        cx.update(|window, cx| open_with(window, cx, backend, None, paths, what, then))
    }

    #[test]
    fn findings_are_shown_from_the_folder_checked() {
        assert_eq!(relative("C:\\proj\\src\\a.env", "C:\\proj"), "src\\a.env");
        assert_eq!(relative("C:\\proj\\src\\a.env", "C:\\PROJ\\"), "src\\a.env");
        assert_eq!(relative("C:\\other\\a.env", "C:\\proj"), "a.env");
        assert_eq!(relative("C:\\proj", "C:\\proj"), "proj");
    }

    #[test]
    fn rows_group_findings_by_file_in_the_order_files_appear() {
        let found = Found {
            findings: vec![
                secret("C:\\p\\b.env", 3),
                secret("C:\\p\\a.env", 1),
                secret("C:\\p\\b.env", 9),
            ],
            ..Default::default()
        };
        let rows = rows_of(&found, "C:\\p", false);
        let kinds: Vec<_> = rows
            .iter()
            .map(|r| match r {
                Row::File { shown, .. } => shown.clone(),
                Row::Finding(f) => format!("line {}", f.line),
                _ => "?".into(),
            })
            .collect();
        assert_eq!(kinds, ["b.env", "line 3", "line 9", "a.env", "line 1"]);
        assert_eq!(files_in(&found), 2);
        let threats = Found {
            threats: vec![Threat {
                name: "Virus:DOS/EICAR_Test_File".into(),
                resources: vec!["C:\\x\\eicar.com".into()],
            }],
            ..Default::default()
        };
        assert_eq!(
            rows_of(&threats, "C:\\x", true),
            [
                Row::Threat("Virus:DOS/EICAR_Test_File".into()),
                Row::Resource("C:\\x\\eicar.com".into())
            ]
        );
    }

    #[gpui_kit::test]
    fn secrets_are_listed_by_file_with_names_for_screen_readers(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        *fake.secrets.lock().unwrap() = Ok(CheckReport {
            findings: vec![secret("C:\\proj\\.env", 4), secret("C:\\proj\\src\\conf.ini", 12)],
            elapsed_ms: 2500.0,
        });
        open_fake(&mut visual, &fake, &["C:\\proj"], What::Secrets, None);
        assert_eq!(
            role_of(&mut visual, "Kingfisher is looking through “proj”…"),
            Some(Role::Status),
            "a status line while it looks"
        );
        settle(&mut visual, "the report", |cx| has(cx, ".env"));
        assert_eq!(role_of(&mut visual, "Passwords and keys in “proj”"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, ".env"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "src\\conf.ini"), Some(Role::Group));
        assert_eq!(
            role_of(&mut visual, "line 4, AWS access key, AKIA••••K4PD, high"),
            Some(Role::ListItem)
        );
        assert_eq!(role_of(&mut visual, "Open the file: .env"), Some(Role::Button));
        assert_eq!(
            role_of(&mut visual, "Show it in its folder: src\\conf.ini"),
            Some(Role::Button)
        );
        assert!(
            has(&mut visual, "These look like passwords, keys or tokens. Anyone who gets these files can use them: take them out, or change them where they work."),
            "the warning about leaked secrets"
        );
        assert!(!has(&mut visual, "Kingfisher is looking through “proj”…"));
    }

    #[gpui_kit::test]
    fn nothing_found_says_so_for_each_kind(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        open_fake(&mut visual, &fake, &["C:\\proj\\a.txt"], What::Secrets, None);
        settle(&mut visual, "clean secrets", |cx| {
            has(cx, "No passwords or keys found. Kingfisher looked through the files as they are (not Git's history), and nothing left the PC.")
        });
    }

    #[gpui_kit::test]
    fn a_clean_defender_scan_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        open_fake(&mut visual, &fake, &["C:\\proj\\a.txt"], What::Defender, None);
        settle(&mut visual, "clean defender", |cx| {
            has(cx, "No threats found in “a.txt”. Microsoft Defender · 1.50 s")
        });
    }

    #[gpui_kit::test]
    fn defender_lists_its_threats_and_where_it_found_them(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        *fake.defender.lock().unwrap() = Ok(DefenderReport {
            clean: false,
            threats: vec![Threat {
                name: "Virus:DOS/EICAR_Test_File".into(),
                resources: vec!["C:\\dl\\eicar.com".into()],
            }],
            elapsed_ms: 800.0,
        });
        open_fake(&mut visual, &fake, &["C:\\dl\\eicar.com"], What::Defender, None);
        settle(&mut visual, "threats", |cx| has(cx, "Virus:DOS/EICAR_Test_File"));
        assert_eq!(role_of(&mut visual, "C:\\dl\\eicar.com"), Some(Role::ListItem));
        assert!(has(
            &mut visual,
            "Microsoft Defender dealt with it as its settings say: Windows Security's Protection history lists it."
        ));
    }

    #[gpui_kit::test]
    fn yara_without_rules_asks_for_a_folder_then_checks_with_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        visual.update(|_, cx| state::update(cx, |s| s.yara_rules = String::new()));
        let fake = Fake::new();
        *fake.picked.lock().unwrap() = Some("C:\\rules".into());
        *fake.yara.lock().unwrap() = Ok(CheckReport {
            findings: vec![Finding {
                title: "malware".into(),
                ..finding("C:\\x\\bad.exe", 0, "Suspicious_Packer", "", "")
            }],
            elapsed_ms: 5.0,
        });
        open_fake(&mut visual, &fake, &["C:\\x"], What::Yara, None);
        settle(&mut visual, "the rules prompt", |cx| has(cx, "Which YARA rules?"));
        assert_eq!(fake.asked.load(Ordering::SeqCst), 0, "nothing is scanned without rules");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("check-choose-rules", cx);
        });
        settle(&mut visual, "the match", |cx| has(cx, "bad.exe"));
        assert_eq!(*fake.yara_rules.lock().unwrap(), ["C:\\rules"]);
        assert!(
            has(&mut visual, "Suspicious_Packer · malware"),
            "a YARA finding names its rule and namespace"
        );
        visual.update(|_, cx| assert_eq!(state::settings(cx).yara_rules, "C:\\rules", "kept in Settings"));
    }

    #[gpui_kit::test]
    fn a_missing_tool_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let mut fake = Arc::try_unwrap(Fake::new()).ok().unwrap();
        fake.installed = false;
        let fake = Arc::new(fake);
        open_fake(&mut visual, &fake, &["C:\\x"], What::Secrets, None);
        let why = "Kingfisher, MongoDB's secret scanner, finds passwords, keys and tokens left in files. It runs on this PC only.";
        settle(&mut visual, "the tool card", |cx| has(cx, why));
        assert_eq!(fake.asked.load(Ordering::SeqCst), 0);
    }

    #[gpui_kit::test]
    fn a_failure_is_shown_with_its_reason_and_refresh_asks_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        *fake.secrets.lock().unwrap() = Err("Kingfisher stopped with exit code Some(3)".into());
        open_fake(&mut visual, &fake, &["C:\\x"], What::Secrets, None);
        settle(&mut visual, "the error", |cx| {
            has(cx, "Couldn't check. Kingfisher stopped with exit code Some(3)")
        });
        assert_eq!(
            role_of(&mut visual, "Couldn't check. Kingfisher stopped with exit code Some(3)"),
            Some(Role::Alert)
        );
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("check-refresh", cx);
        });
        settle(&mut visual, "the second ask", |_| {
            fake.asked.load(Ordering::SeqCst) == 2
        });
    }

    #[gpui_kit::test]
    fn stop_cancels_the_scan_and_says_stopped(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::gated();
        open_fake(&mut visual, &fake, &["C:\\big"], What::Secrets, None);
        settle(&mut visual, "the scan to start", |_| {
            fake.asked.load(Ordering::SeqCst) == 1
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("check-stop", cx);
        });
        settle(&mut visual, "stopped", |cx| has(cx, "Stopped"));
        assert_eq!(fake.cancelled.load(Ordering::SeqCst), 1);
        assert_eq!(role_of(&mut visual, "Stopped"), Some(Role::Status));
    }

    #[gpui_kit::test]
    fn closing_the_dialog_stops_a_scan_under_way(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::gated();
        let view = open_fake(&mut visual, &fake, &["C:\\big"], What::Secrets, None);
        settle(&mut visual, "the scan to start", |_| {
            fake.asked.load(Ordering::SeqCst) == 1
        });
        drop(view);
        visual.update(|window, cx| window.close_dialog(cx));
        settle(&mut visual, "the cancel", |_| {
            fake.cancelled.load(Ordering::SeqCst) >= 1
        });
    }

    #[gpui_kit::test]
    fn the_check_before_zipping_offers_the_zip_when_it_finds_secrets(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        *fake.secrets.lock().unwrap() = Ok(CheckReport {
            findings: vec![secret("C:\\proj\\.env", 2)],
            elapsed_ms: 40.0,
        });
        let went = Rc::new(std::cell::Cell::new(0));
        let counter = went.clone();
        let backend: Arc<dyn Backend> = fake.clone();
        visual.update(|window, cx| {
            secrets_check_first_with(
                window,
                cx,
                backend,
                vec!["C:\\proj\\.env".into()],
                t!("Zip anyway"),
                move |_, _| counter.set(counter.get() + 1),
            )
        });
        settle(&mut visual, "the findings", |cx| has(cx, ".env"));
        assert_eq!(went.get(), 0, "asks before going on");
        assert!(has(&mut visual, "Cancel"), "Cancel replaces Close");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("check-then", cx);
        });
        assert_eq!(went.get(), 1);
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "the dialog closes first"));
    }

    #[gpui_kit::test]
    fn the_check_before_zipping_goes_straight_on_when_there_is_nothing(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        let went = Rc::new(std::cell::Cell::new(0));
        let counter = went.clone();
        let backend: Arc<dyn Backend> = fake.clone();
        visual.update(|window, cx| {
            secrets_check_first_with(
                window,
                cx,
                backend,
                vec!["C:\\proj".into()],
                t!("Zip anyway"),
                move |_, _| counter.set(counter.get() + 1),
            )
        });
        settle(&mut visual, "going on", |_| went.get() == 1);
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn escape_closes_it_and_arabic_names_render(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let fake = Fake::new();
        *fake.secrets.lock().unwrap() = Ok(CheckReport {
            findings: vec![secret("C:\\مشروع\\مفاتيح.env", 7)],
            elapsed_ms: 3.0,
        });
        open_fake(&mut visual, &fake, &["C:\\مشروع"], What::Secrets, None);
        settle(&mut visual, "the Arabic file", |cx| has(cx, "مفاتيح.env"));
        assert_eq!(role_of(&mut visual, "مفاتيح.env"), Some(Role::Group));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Escape closes it"));
        testing::set_rtl(false);
    }
}
