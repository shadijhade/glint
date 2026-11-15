//! Checking files with Microsoft Defender: a custom scan of what's chosen, in a dialog with Stop, that lists the
//! threats Defender found and where. Ported from src/components/explorer/CheckDialog.tsx and src/lib/defender.ts,
//! built the way [`super::drive_health`] is. (Its secrets and YARA checks went with Kingfisher and YARA-X.)
//!
//! | TypeScript                                   | Here                                                              |
//! |----------------------------------------------|-------------------------------------------------------------------|
//! | `useCheck` store, `run`                      | [`Check`], a model entity                                         |
//! | `scanWithDefender(paths)` (a toast)          | [`open`] (the dialog, with Stop)                                  |
//! | `defenderScan`                               | [`Backend`] ([`CoreBackend`] is glint-core, tests pass a fake)     |
//! | `CheckDialog`                                | [`CheckDialog`]                                                   |
//!
//! **How the coordinator wires it.** `MenuEvent::Defender { paths }` is `dialogs::check::open(window, cx, paths)`.
//!
//! The threats list is virtualized: every row has the same height, a threat and then where it was found.

use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::defender::{DefenderReport, Threat};
use glint_model::format::{format_count, format_duration, plural};
use glint_model::paths::{base_name, parent_path};
use gpui_kit::UniformListScrollHandle;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A Defender scan is running somewhere (`scanning` of defender.ts). Defender scans one thing at a time (Stop
/// cancels "the" scan), so a second one is refused with a notice instead of started.
static DEFENDER_BUSY: AtomicBool = AtomicBool::new(false);

/// The notice that a Defender scan is already running (the web app's toast).
struct DefenderBusyToast;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// The calls a scan makes. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// `defender_scan`.
    fn defender(&self, target: String) -> Fut<DefenderReport>;
    /// Stops the scan that is running (`cancel_defender_scan`).
    fn cancel(&self);
}

/// glint-core.
pub struct CoreBackend;

impl Backend for CoreBackend {
    fn defender(&self, target: String) -> Fut<DefenderReport> {
        Box::pin(glint_core::defender::defender_scan(target))
    }

    fn cancel(&self) {
        glint_core::defender::cancel_defender_scan();
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn defender(&self, _: String) -> Fut<DefenderReport> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn cancel(&self) {}
}

/// The app's backend: core's if it is started.
fn core_backend(cx: &App) -> Arc<dyn Backend> {
    if cx.has_global::<core_bridge::Core>() {
        Arc::new(CoreBackend)
    } else {
        Arc::new(Unavailable)
    }
}

// ---- The model --------------------------------------------------------------------------------------

/// What a finished scan found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub threats: Vec<Threat>,
    pub elapsed_ms: f64,
}

impl Found {
    /// How many threats were found.
    pub fn count(&self) -> usize {
        self.threats.len()
    }
}

/// Where a scan stands (`running`, `report` and `error` of the TypeScript's store, as one value).
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Defender is scanning.
    Running,
    /// The person stopped it.
    Stopped,
    /// It failed, with the reason.
    Failed(SharedString),
    /// It finished.
    Done(Found),
}

/// One scan of some paths. Closing the dialog drops the model, which stops a scan under way.
pub struct Check {
    paths: Vec<String>,
    outcome: Outcome,
    backend: Arc<dyn Backend>,
    /// The number of the run whose answer counts; an older run's answer is dropped.
    run: u64,
    stopped: Arc<AtomicBool>,
    rows: Arc<Vec<Row>>,
    /// This scan is the one [`DEFENDER_BUSY`] stands for.
    holds_defender: bool,
}

impl Check {
    /// A scan of `paths` that has not started.
    pub fn new(paths: Vec<String>, backend: Arc<dyn Backend>) -> Self {
        Self {
            paths,
            outcome: Outcome::Running,
            backend,
            run: 0,
            stopped: Arc::new(AtomicBool::new(false)),
            rows: Arc::new(Vec::new()),
            holds_defender: false,
        }
    }

    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }

    /// Where the scan is shown from: the file, or the folder of the first of several.
    pub fn target(&self) -> String {
        match self.paths.as_slice() {
            [one] => one.clone(),
            [first, ..] => parent_path(first),
            [] => String::new(),
        }
    }

    /// What is scanned, for words: "“name”" or "3 items".
    pub fn subject(&self) -> SharedString {
        match self.paths.as_slice() {
            [one] => t!("“{name}”", name = name_of(one)),
            many => plural(many.len(), "item").into(),
        }
    }

    /// Lets the next Defender scan start.
    fn release_defender(&mut self) {
        if std::mem::take(&mut self.holds_defender) {
            DEFENDER_BUSY.store(false, Ordering::SeqCst);
        }
    }

    fn set_outcome(&mut self, outcome: Outcome) {
        if !matches!(outcome, Outcome::Running) {
            self.release_defender();
        }
        self.rows = Arc::new(match &outcome {
            Outcome::Done(found) => rows_of(found),
            _ => Vec::new(),
        });
        self.outcome = outcome;
    }

    /// Starts the scan (again): a run already under way is forgotten (`run`).
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.run += 1;
        let run = self.run;
        self.stopped = Arc::new(AtomicBool::new(false));
        self.set_outcome(Outcome::Running);
        self.holds_defender = true;
        DEFENDER_BUSY.store(true, Ordering::SeqCst);
        cx.notify();
        let work = scan(self.backend.clone(), self.paths.clone(), self.stopped.clone());
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

    /// Stops the scan under way (`cancelDefenderScan`).
    pub fn stop(&mut self) {
        if self.outcome == Outcome::Running {
            self.stopped.store(true, Ordering::SeqCst);
            self.backend.cancel();
        }
    }
}

impl Drop for Check {
    fn drop(&mut self) {
        // The dialog closed: a scan still running is stopped, as `close` did.
        self.stop();
        self.release_defender();
    }
}

/// Runs the scan on core's runtime, one path after another, and says how it ended.
fn scan(
    backend: Arc<dyn Backend>,
    paths: Vec<String>,
    stopped: Arc<AtomicBool>,
) -> Pin<Box<dyn Future<Output = Outcome> + Send>> {
    Box::pin(async move {
        let mut found = Found::default();
        for path in paths {
            if stopped.load(Ordering::SeqCst) {
                return Outcome::Stopped;
            }
            match core_bridge::run(backend.defender(path)).await.and_then(|r| r) {
                Ok(report) => {
                    found.elapsed_ms += report.elapsed_ms;
                    found.threats.extend(report.threats);
                }
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

fn name_of(path: &str) -> String {
    let name = base_name(path);
    if name.is_empty() { path } else { name }.to_string()
}

// ---- The rows ---------------------------------------------------------------------------------------

/// One line of the threats list. Every row is the same height, so the list can be virtualized.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// A threat Defender found.
    Threat(String),
    /// Where Defender found it.
    Resource(String),
}

/// The rows for a report: each threat, then where it was found.
fn rows_of(found: &Found) -> Vec<Row> {
    let mut rows = Vec::new();
    for threat in &found.threats {
        rows.push(Row::Threat(threat.name.clone()));
        rows.extend(threat.resources.iter().cloned().map(Row::Resource));
    }
    rows
}

// ---- The dialog -------------------------------------------------------------------------------------

/// Every row's height, in rems.
const ROW_HEIGHT: f32 = 1.875;

/// What a Defender scan found.
pub struct CheckDialog {
    check: Entity<Check>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl CheckDialog {
    /// The view of `check`; it starts the scan.
    pub fn new(check: Entity<Check>, cx: &mut Context<Self>) -> Self {
        // F5 is the Refresh button's key, as in the web app's dialogs.
        super::refresh_on_f5(cx, |this, _, cx| {
            this.check.update(cx, |check, cx| {
                if *check.outcome() != Outcome::Running {
                    check.start(cx)
                }
            });
        });
        let subscriptions = vec![cx.observe(&check, |_, _, cx| cx.notify())];
        if matches!(check.read(cx).outcome(), Outcome::Running) {
            check.update(cx, |check, cx| check.start(cx));
        }
        Self {
            check,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn title_line(subject: &SharedString) -> SharedString {
        t!("Microsoft Defender in {name}", name = subject)
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

    fn rows_list(&self, rows: Arc<Vec<Row>>, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = t.clone();
        let count = rows.len();
        let list = uniform_list(
            "check-rows",
            count,
            cx.processor(move |_, range: Range<usize>, _, _| {
                range
                    .filter_map(|ix| rows.get(ix).map(|row| render_row(ix, row, &colors).into_any_element()))
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
                    .aria_label(t!("Threats"))
                    .test_support()
                    .size_full()
                    .child(list),
            )
            .child(Scrollbar::vertical(&self.scroll).viewport_from_layout())
    }
}

/// One row: a threat, or the place it was found.
fn render_row(ix: usize, row: &Row, t: &theme::Tokens) -> impl IntoElement {
    let base = row_start()
        .h(rems(ROW_HEIGHT))
        .items_center()
        .gap(space(2.))
        .px(space(3.))
        .text_size(theme::size::XS);
    match row {
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

impl Render for CheckDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let check = self.check.read(cx);
        let outcome = check.outcome().clone();
        let target = check.target();
        let subject = check.subject();
        let rows = check.rows.clone();
        let running = outcome == Outcome::Running;

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
                    .tooltip(t!("Refresh (F5)"))
                    .accessibility_label(t!("Refresh"))
                    .on_click(cx.listener(|this, _, _, cx| this.check.update(cx, |check, cx| check.start(cx)))),
            );

        let body: AnyElement = match &outcome {
            Outcome::Running => {
                let words = t!("Microsoft Defender is scanning {name}…", name = subject);
                Self::centered(
                    "check-running",
                    Role::Status,
                    words.clone(),
                    Spinner::new().color(t.primary).into_any_element(),
                    vec![div().text_color(t.muted_foreground).child(words).into_any_element()],
                )
                .into_any_element()
            }
            Outcome::Stopped | Outcome::Failed(_) => {
                // The words are defender.ts's toasts'.
                let (headline, reason) = match &outcome {
                    Outcome::Failed(reason) => (t!("Couldn't scan with Microsoft Defender"), Some(reason.clone())),
                    _ => (t!("Stopped scanning {name}", name = subject.clone()), None),
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
            Outcome::Done(found) if found.count() == 0 => {
                let headline = t!("No threats found in {name}", name = subject);
                let note = t!("Microsoft Defender · {took}", took = format_duration(found.elapsed_ms));
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
                let n = found.threats.len();
                // Defender says what it found first, then what it did about it.
                let headline = if n == 1 {
                    t!("Microsoft Defender found a threat in {name}", name = subject.clone())
                } else {
                    t!("Microsoft Defender found {n} threats in {name}", n = n, name = subject.clone())
                };
                let warning = t!(
                    "Microsoft Defender dealt with {them} as its settings say: Windows Security's Protection history lists {them}.",
                    them = if n == 1 { t!("it") } else { t!("them") }
                );
                col()
                    .gap(space(2.))
                    .child(
                        div()
                            .id("check-headline")
                            .role(Role::Heading)
                            .aria_label(headline.clone())
                            .test_support()
                            .font_weight(FontWeight::MEDIUM)
                            .child(headline),
                    )
                    .child(
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
                            .child(warning),
                    )
                    .child(
                        div()
                            .rounded(theme::size::RADIUS_LG)
                            .border_1()
                            .border_color(t.border)
                            .overflow_hidden()
                            .child(self.rows_list(rows, &t, cx)),
                    )
                    .into_any_element()
            }
        };

        let summary: SharedString = match &outcome {
            Outcome::Done(found) => {
                let n = found.count();
                let what_found = if n == 1 {
                    t!("1 threat")
                } else {
                    t!("{n} threats", n = format_count(n))
                };
                format!("{what_found} · {}", format_duration(found.elapsed_ms)).into()
            }
            _ => "".into(),
        };

        col()
            .id("check")
            .role(Role::Group)
            .aria_label(Self::title_line(&subject))
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
                    .child(div().flex_1().min_w_0().truncate().child(summary))
                    .child(
                        Button::new("check-close")
                            .small()
                            .outline()
                            .label(t!("Close"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
            )
    }
}

// ---- Opening it -------------------------------------------------------------------------------------

/// Scans `paths` (files, or folders and what's in them) with Microsoft Defender in a dialog. Nothing opens for no
/// paths; a second scan while one runs says so instead.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    if DEFENDER_BUSY.load(Ordering::SeqCst) {
        window.push_notification(
            Notification::info(t!("Microsoft Defender is already scanning")).id::<DefenderBusyToast>(),
            cx,
        );
        return;
    }
    let backend = core_backend(cx);
    open_with(window, cx, backend, paths);
}

/// [`open`] with a backend of your own (tests pass a fake). Returns the view.
pub fn open_with(window: &mut Window, cx: &mut App, backend: Arc<dyn Backend>, paths: Vec<String>) -> Entity<CheckDialog> {
    let check = cx.new(|_| Check::new(paths, backend));
    let subject = check.read(cx).subject();
    let view = cx.new(|cx| CheckDialog::new(check, cx));
    super::show(window, cx, CheckDialog::title_line(&subject), px(720.), view.clone());
    view
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

    /// A backend answering from what the test sets, counting the calls.
    struct Fake {
        defender: Mutex<Result<DefenderReport, String>>,
        /// The scan waits for this to fire (Stop sends "cancelled" through it); `None` answers at once.
        gate: Mutex<Option<async_channel::Receiver<String>>>,
        gate_tx: async_channel::Sender<String>,
        cancelled: AtomicUsize,
        asked: AtomicUsize,
    }

    impl Fake {
        fn new() -> Arc<Self> {
            let (gate_tx, _rx) = async_channel::bounded(1);
            Arc::new(Self {
                defender: Mutex::new(Ok(DefenderReport {
                    clean: true,
                    threats: vec![],
                    elapsed_ms: 1500.0,
                })),
                gate: Mutex::new(None),
                gate_tx,
                cancelled: AtomicUsize::new(0),
                asked: AtomicUsize::new(0),
            })
        }

        fn gated() -> Arc<Self> {
            let (tx, rx) = async_channel::bounded(1);
            let mut fake = Arc::try_unwrap(Self::new()).ok().unwrap();
            fake.gate_tx = tx;
            *fake.gate.get_mut().unwrap() = Some(rx);
            Arc::new(fake)
        }
    }

    impl Backend for Fake {
        fn defender(&self, _: String) -> Fut<DefenderReport> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            let answer = self.defender.lock().unwrap().clone();
            let gate = self.gate.lock().unwrap().clone();
            Box::pin(async move {
                if let Some(gate) = gate {
                    return Err(gate.recv().await.unwrap_or_default());
                }
                answer
            })
        }

        fn cancel(&self) {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
            let _ = self.gate_tx.try_send("cancelled".into());
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

    fn open_fake(cx: &mut VisualTestContext, fake: &Arc<Fake>, paths: &[&str]) -> Entity<CheckDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        cx.update(|window, cx| open_with(window, cx, backend, paths))
    }

    #[test]
    fn rows_list_each_threat_then_where_it_was_found() {
        let found = Found {
            threats: vec![
                Threat {
                    name: "Virus:A".into(),
                    resources: vec!["C:\\a.exe".into(), "C:\\b.exe".into()],
                },
                Threat {
                    name: "Virus:B".into(),
                    resources: vec![],
                },
            ],
            elapsed_ms: 1.0,
        };
        assert_eq!(
            rows_of(&found),
            vec![
                Row::Threat("Virus:A".into()),
                Row::Resource("C:\\a.exe".into()),
                Row::Resource("C:\\b.exe".into()),
                Row::Threat("Virus:B".into()),
            ]
        );
    }

    #[gpui_kit::test]
    fn a_clean_defender_scan_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        open_fake(&mut visual, &fake, &["C:\\proj\\a.txt"]);
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
        open_fake(&mut visual, &fake, &["C:\\dl\\eicar.com"]);
        settle(&mut visual, "threats", |cx| has(cx, "Virus:DOS/EICAR_Test_File"));
        assert_eq!(role_of(&mut visual, "C:\\dl\\eicar.com"), Some(Role::ListItem));
        assert!(has(
            &mut visual,
            "Microsoft Defender dealt with it as its settings say: Windows Security's Protection history lists it."
        ));
    }

    #[gpui_kit::test]
    fn defender_says_what_it_found_in_the_web_apps_words(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let one = |name: &str| Threat {
            name: name.into(),
            resources: vec![],
        };
        let fake = Fake::new();
        *fake.defender.lock().unwrap() = Ok(DefenderReport {
            clean: false,
            threats: vec![one("Virus:A")],
            elapsed_ms: 800.0,
        });
        open_fake(&mut visual, &fake, &["C:\\dl\\eicar.com"]);
        settle(&mut visual, "one threat", |cx| {
            has(cx, "Microsoft Defender found a threat in “eicar.com”")
        });
        visual.update(|window, cx| window.close_dialog(cx));
        visual.run_until_parked();
        *fake.defender.lock().unwrap() = Ok(DefenderReport {
            clean: false,
            threats: vec![one("Virus:A"), one("Virus:B")],
            elapsed_ms: 800.0,
        });
        open_fake(&mut visual, &fake, &["C:\\dl\\a.zip"]);
        settle(&mut visual, "two threats", |cx| {
            has(cx, "Microsoft Defender found 2 threats in “a.zip”")
        });
    }

    #[gpui_kit::test]
    fn a_failed_or_stopped_scan_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        *fake.defender.lock().unwrap() = Err("Defender isn't running".into());
        open_fake(&mut visual, &fake, &["C:\\dl\\a.zip"]);
        settle(&mut visual, "the failure", |cx| {
            has(cx, "Couldn't scan with Microsoft Defender. Defender isn't running")
        });
        assert_eq!(
            role_of(&mut visual, "Couldn't scan with Microsoft Defender. Defender isn't running"),
            Some(Role::Alert)
        );
        // Refresh asks again.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("check-refresh", cx);
        });
        settle(&mut visual, "the second ask", |_| fake.asked.load(Ordering::SeqCst) == 2);
        visual.update(|window, cx| window.close_dialog(cx));
        visual.run_until_parked();
        // Stop: "Stopped scanning “a.zip”".
        let gated = Fake::gated();
        open_fake(&mut visual, &gated, &["C:\\dl\\a.zip"]);
        settle(&mut visual, "the scan", |cx| {
            has(cx, "Microsoft Defender is scanning “a.zip”…")
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("check-stop", cx);
        });
        settle(&mut visual, "stopped", |cx| has(cx, "Stopped scanning “a.zip”"));
        assert_eq!(gated.cancelled.load(Ordering::SeqCst), 1);
        assert_eq!(role_of(&mut visual, "Stopped scanning “a.zip”"), Some(Role::Status));
    }

    #[gpui_kit::test]
    fn a_second_defender_scan_waits_for_the_one_running(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        DEFENDER_BUSY.store(false, Ordering::SeqCst);
        let mut visual = window(cx);
        let fake = Fake::gated();
        open_fake(&mut visual, &fake, &["C:\\dl\\a.zip"]);
        settle(&mut visual, "the scan", |cx| {
            has(cx, "Microsoft Defender is scanning “a.zip”…")
        });
        assert!(DEFENDER_BUSY.load(Ordering::SeqCst), "one is running");
        let notices = |visual: &mut VisualTestContext| visual.update(|window, cx| window.notifications(cx).len());
        assert_eq!(notices(&mut visual), 0);
        visual.update(|window, cx| open(window, cx, vec!["C:\\dl\\b.zip".into()]));
        settle(&mut visual, "the notice", |cx| notices(cx) == 1);
        assert!(
            !has(&mut visual, "Microsoft Defender is scanning “b.zip”…"),
            "the second scan didn't start"
        );
        // When the first one ends, the next may start.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("check-stop", cx);
        });
        settle(&mut visual, "the end", |_| !DEFENDER_BUSY.load(Ordering::SeqCst));
    }

    #[gpui_kit::test]
    fn closing_the_dialog_stops_a_scan_under_way(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        DEFENDER_BUSY.store(false, Ordering::SeqCst);
        let mut visual = window(cx);
        let fake = Fake::gated();
        let view = open_fake(&mut visual, &fake, &["C:\\big"]);
        settle(&mut visual, "the scan to start", |_| fake.asked.load(Ordering::SeqCst) == 1);
        drop(view);
        visual.update(|window, cx| window.close_dialog(cx));
        settle(&mut visual, "the cancel", |_| fake.cancelled.load(Ordering::SeqCst) >= 1);
    }

    #[gpui_kit::test]
    fn escape_closes_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        DEFENDER_BUSY.store(false, Ordering::SeqCst);
        let mut visual = window(cx);
        let fake = Fake::new();
        open_fake(&mut visual, &fake, &["C:\\proj\\a.txt"]);
        settle(&mut visual, "the result", |cx| {
            has(cx, "No threats found in “a.txt”. Microsoft Defender · 1.50 s")
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Escape closes it"));
    }
}
