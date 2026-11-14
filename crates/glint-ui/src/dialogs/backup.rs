//! Backing a folder up with restic: where to, a password, when, and the backup itself with its progress.
//! Ported from src/components/explorer/BackupDialog.tsx (the plans, runs and schedules are glint-core's
//! `backup.rs`).
//!
//! | TypeScript                                   | Here                                                         |
//! |----------------------------------------------|--------------------------------------------------------------|
//! | `useBackup`, `openBackup(entry)`             | [`open`] (the caller checks [`can_back_up`] and says why not)|
//! | `canBackUp(entry)`                           | [`can_back_up`]                                              |
//! | `describeRepository`                         | [`describe_repository`]                                      |
//! | `BackupModal`'s state and `backUp`, `setUp`  | [`Backup`], a model entity                                   |
//! | `BackupModal`'s markup                       | [`BackupDialog`]                                             |
//! | `BackupRunning`, `BackupDone`                | drawn by [`BackupDialog`] (`running`, `done`)                |
//! | `invoke("backup_plans")` and the others      | [`Backend`] (a fake in the tests), [`CoreBackend`]           |
//! | `<ToolNeeded id="restic" />`                 | `components::settings::tools::needed`                        |
//! | Tauri's folder picker                        | `Backend::pick_folder`, through Settings' picker             |
//! | `<select>` of clouds                         | a list of radio buttons (the kit has no drop-down)           |
//! | `openVersions(folder)`                       | `dialogs::versions::open`                                    |
//!
//! **How the coordinator wires it.** The item menu's `MenuEvent::BackUp { folder }` calls
//! `dialogs::backup::open(window, cx, folder)` (the menu only offers it for folders [`can_back_up`] accepts).
//! While a backup runs the dialog cannot be closed (Escape, the close button and the backdrop do nothing), as the
//! TypeScript's `onClose={() => !busy && close()}`; Stop ends the backup first.

use crate::components::settings::tools::{self, Tools};
use crate::components::settings::{Backend as SettingsBackend, CoreBackend as SettingsCoreBackend};
use crate::kit::prelude::*;
use crate::{core_bridge, explorer, t, theme};
use glint_core::backup::{BackupProgress, LastRun, Plan, Schedule};
use glint_core::rclone::Remote;
use glint_model::format::{format_count, format_date, format_size, plural};
use glint_model::js::{js_round, number_to_string};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// A backup under way: its answer, and its progress as it comes.
pub struct Running {
    /// The backup's result (`backup_run`).
    pub done: Fut<LastRun>,
    /// Progress messages; closed when the backup ends.
    pub progress: async_channel::Receiver<BackupProgress>,
}

/// What the dialog asks of the PC. [`CoreBackend`] is the real one; tests give a fake.
pub trait Backend: Send + Sync {
    /// Every folder that is backed up (`backup_plans`).
    fn plans(&self) -> Fut<Vec<Plan>>;
    /// Sets a folder's backup up, making the repository if it isn't there (`backup_setup`).
    fn setup(&self, source: String, repository: String, password: String, schedule: Schedule) -> Fut<Plan>;
    /// Backs the plan's folder up now (`backup_run`).
    fn run(&self, id: String) -> Running;
    /// Stops the backup that runs (`backup_cancel`).
    fn cancel(&self);
    /// Changes when the folder is backed up (`backup_schedule`).
    fn schedule(&self, id: String, schedule: Schedule) -> Fut<Plan>;
    /// Stops backing the folder up; the backups made stay (`backup_remove`).
    fn remove(&self, id: String) -> Fut<()>;
    /// The remotes in rclone's configuration (`rclone_remotes`).
    fn rclone_remotes(&self) -> Fut<Vec<Remote>>;
    /// A folder the person picks (`None`: cancelled). `owner` is the window's handle.
    fn pick_folder(&self, title: String, owner: isize) -> Fut<Option<String>>;
}

/// Waits for a core call and flattens the two layers of failure (the task panicking, the call failing).
fn flat<T: Send + 'static>(call: impl Future<Output = Result<T, String>> + Send + 'static) -> Fut<T> {
    let joined = core_bridge::run(call);
    Box::pin(async move { joined.await.and_then(|inner| inner) })
}

/// glint-core's backups through the app's handle.
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
    fn plans(&self) -> Fut<Vec<Plan>> {
        flat(glint_core::backup::backup_plans())
    }

    fn setup(&self, source: String, repository: String, password: String, schedule: Schedule) -> Fut<Plan> {
        flat(glint_core::backup::backup_setup(source, repository, password, schedule))
    }

    fn run(&self, id: String) -> Running {
        let (channel, progress) = core_bridge::stream::<BackupProgress>();
        Running {
            done: flat(glint_core::backup::backup_run(id, channel)),
            progress,
        }
    }

    fn cancel(&self) {
        glint_core::backup::backup_cancel();
    }

    fn schedule(&self, id: String, schedule: Schedule) -> Fut<Plan> {
        flat(glint_core::backup::backup_schedule(id, schedule))
    }

    fn remove(&self, id: String) -> Fut<()> {
        flat(glint_core::backup::backup_remove(id))
    }

    fn rclone_remotes(&self) -> Fut<Vec<Remote>> {
        flat(glint_core::rclone::rclone_remotes())
    }

    fn pick_folder(&self, title: String, owner: isize) -> Fut<Option<String>> {
        SettingsCoreBackend::new(self.app.clone()).pick_folder(title, None, owner)
    }
}

/// A folder Back up takes: one on this PC's drives, not in an archive, the Recycle Bin or in the cloud
/// (`canBackUp`). `online_only`, `archived`, `closed` and `recycled` are the entry's flags.
#[cfg(test)]
pub fn can_back_up(path: &str, is_dir: bool, online_only: bool, archived: bool, closed: bool, recycled: bool) -> bool {
    let b = path.as_bytes();
    let on_a_drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    is_dir && !archived && !closed && !recycled && !online_only && on_a_drive
}

/// Where a backup goes, in words (`describeRepository`).
pub fn describe_repository(repository: &str) -> SharedString {
    match repository.strip_prefix("rclone:") {
        Some(rest) => t!("{name} (a cloud)", name = rest.split(':').next().unwrap_or("")),
        None => repository.to_string().into(),
    }
}

/// The word for a stage core names in English (`stage` of the progress).
fn stage_text(stage: &str) -> SharedString {
    match stage {
        "Starting" => t!("Starting"),
        "Backing up" => t!("Backing up"),
        "Setting up the backup" => t!("Setting up the backup"),
        "Tidying older backups" => t!("Tidying older backups"),
        "Checking the backups" => t!("Checking the backups"),
        other => other.to_string().into(),
    }
}

/// A count from core as the dialog shows it.
fn count(n: u64) -> String {
    format_count(usize::try_from(n).unwrap_or(usize::MAX))
}

/// Where a backup goes: a drive's folder, or a cloud.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    /// "A drive or folder": a USB drive, another disk, a network folder.
    Drive,
    /// "A cloud": one added in Remote (rclone).
    Cloud,
}

/// Whether the folder has a backup plan yet.
#[derive(Clone, Debug, PartialEq)]
pub enum PlanState {
    /// Asking core.
    Looking,
    /// Not backed up: the form sets it up.
    Unplanned,
    /// Backed up: when, last time, and "Back up now".
    Planned(Box<Plan>),
}

/// What the model tells the view.
#[derive(Clone, Debug, PartialEq)]
pub enum BackupEvent {
    /// The folder's backup was set up: the password boxes are emptied.
    PlanMade,
    /// "Stop backing up" went through: say so, and close.
    Removed,
}

/// The dialog's data: the plan, the choices for a new one, and a backup's progress and result.
pub struct Backup {
    folder: String,
    backend: Arc<dyn Backend>,
    plan: PlanState,
    place: Where,
    /// The folder chosen to back up to (the backup goes into "Glint backup" in it).
    drive: String,
    /// rclone's remotes, once asked for.
    remotes: Option<Vec<Remote>>,
    asking_remotes: bool,
    /// The rclone remote chosen.
    remote: String,
    schedule: Schedule,
    busy: Option<BackupProgress>,
    result: Option<LastRun>,
    error: Option<SharedString>,
    /// restic and rclone are installed (the view tells, from the tools it watches).
    restic: bool,
    rclone: bool,
}

impl EventEmitter<BackupEvent> for Backup {}

impl Backup {
    /// A model for `folder` that asks core for its plan at once.
    pub fn new(folder: String, backend: Arc<dyn Backend>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            folder,
            backend,
            plan: PlanState::Looking,
            place: Where::Drive,
            drive: String::new(),
            remotes: None,
            asking_remotes: false,
            remote: String::new(),
            schedule: Schedule::Daily,
            busy: None,
            result: None,
            error: None,
            restic: false,
            rclone: false,
        };
        this.look_for_plan(cx);
        this
    }

    /// The folder.
    pub fn folder(&self) -> &str {
        &self.folder
    }

    /// The folder's name, for the title: its last part, or the whole path for a drive's root.
    pub fn name(&self) -> String {
        let name = self.folder.rsplit('\\').next().unwrap_or("");
        if name.is_empty() {
            self.folder.clone()
        } else {
            name.to_string()
        }
    }

    /// Whether the folder has a plan.
    pub fn plan(&self) -> &PlanState {
        &self.plan
    }

    /// A backup (or its setting up) is running, with its progress.
    pub fn busy(&self) -> Option<&BackupProgress> {
        self.busy.as_ref()
    }

    /// How the last backup run from here went.
    pub fn result(&self) -> Option<&LastRun> {
        self.result.as_ref()
    }

    /// What went wrong, if something did.
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Where the new backup would go.
    pub fn place(&self) -> Where {
        self.place
    }

    /// When the new backup runs.
    pub fn schedule(&self) -> Schedule {
        self.schedule
    }

    /// rclone's remotes, once known.
    pub fn remotes(&self) -> Option<&[Remote]> {
        self.remotes.as_deref()
    }

    /// The rclone remote chosen.
    pub fn remote(&self) -> &str {
        &self.remote
    }

    /// The folder chosen to back up to.
    pub fn drive(&self) -> &str {
        &self.drive
    }

    /// Whether restic is there.
    pub fn restic(&self) -> bool {
        self.restic
    }

    /// Where the backup goes, as restic is told: `E:\Glint backup`, or `rclone:drive:Glint backup`; empty until
    /// a place is chosen.
    pub fn repository(&self) -> String {
        match self.place {
            Where::Drive if self.drive.is_empty() => String::new(),
            Where::Drive => format!("{}\\Glint backup", self.drive.trim_end_matches('\\')),
            Where::Cloud if self.remote.is_empty() => String::new(),
            Where::Cloud => format!("rclone:{}:Glint backup", self.remote),
        }
    }

    /// Whether "Back up" can be pressed: restic (and rclone for a cloud) is there, a place is chosen and the
    /// password is good (`ready`).
    pub fn ready(&self, password_ok: bool) -> bool {
        self.restic && (self.place == Where::Drive || self.rclone) && !self.repository().is_empty() && password_ok
    }

    /// A password is good when it has 8 characters and the second box says the same (`badPassword`, inverted).
    pub fn password_ok(password: &str, again: &str) -> bool {
        password.chars().count() >= 8 && password == again
    }

    /// What the view saw of the tools: restic and rclone are installed.
    pub fn set_tools(&mut self, restic: bool, rclone: bool, cx: &mut Context<Self>) {
        if self.restic == restic && self.rclone == rclone {
            return;
        }
        self.restic = restic;
        self.rclone = rclone;
        self.ask_for_remotes(cx);
        cx.notify();
    }

    fn look_for_plan(&mut self, cx: &mut Context<Self>) {
        let asking = self.backend.plans();
        let folder = self.folder.to_lowercase();
        cx.spawn(async move |this, cx| {
            let answer = asking.await;
            let _ = this.update(cx, |this, cx| {
                this.plan = match answer {
                    Ok(all) => all
                        .into_iter()
                        .find(|p| p.source.to_lowercase() == folder)
                        .map_or(PlanState::Unplanned, |plan| PlanState::Planned(Box::new(plan))),
                    Err(_) => PlanState::Unplanned,
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// rclone's remotes are asked for the first time the cloud is chosen and rclone is there.
    fn ask_for_remotes(&mut self, cx: &mut Context<Self>) {
        if self.place != Where::Cloud || !self.rclone || self.remotes.is_some() || self.asking_remotes {
            return;
        }
        self.asking_remotes = true;
        let asking = self.backend.rclone_remotes();
        cx.spawn(async move |this, cx| {
            let answer = asking.await;
            let _ = this.update(cx, |this, cx| {
                this.asking_remotes = false;
                let all = answer.unwrap_or_default();
                this.remote = all.first().map(|r| r.name.clone()).unwrap_or_default();
                this.remotes = Some(all);
                cx.notify();
            });
        })
        .detach();
    }

    /// Chooses where the backup goes (`setWhere`).
    pub fn choose_place(&mut self, place: Where, cx: &mut Context<Self>) {
        self.place = place;
        self.ask_for_remotes(cx);
        cx.notify();
    }

    /// Chooses the drive's folder (`setDrive`).
    pub fn set_drive(&mut self, folder: String, cx: &mut Context<Self>) {
        self.drive = folder;
        cx.notify();
    }

    /// Chooses the rclone remote (`setRemote`).
    pub fn set_remote(&mut self, name: String, cx: &mut Context<Self>) {
        self.remote = name;
        cx.notify();
    }

    /// Chooses when a new backup runs (`setSchedule`).
    pub fn set_schedule(&mut self, schedule: Schedule, cx: &mut Context<Self>) {
        self.schedule = schedule;
        cx.notify();
    }

    /// Changes when the planned folder is backed up (`backup_schedule`).
    pub fn change_plan_schedule(&mut self, schedule: Schedule, cx: &mut Context<Self>) {
        let PlanState::Planned(plan) = &self.plan else { return };
        let asking = self.backend.schedule(plan.id.clone(), schedule);
        cx.spawn(async move |this, cx| {
            let answer = asking.await;
            let _ = this.update(cx, |this, cx| {
                match answer {
                    Ok(plan) => this.plan = PlanState::Planned(Box::new(plan)),
                    Err(reason) => this.error = Some(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Backs the plan `id` up now, with progress (`backUp`).
    pub fn back_up(&mut self, id: String, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.error = None;
        self.result = None;
        self.busy = Some(BackupProgress {
            stage: "Starting".into(),
            ..Default::default()
        });
        cx.notify();
        let Running { done, progress } = self.backend.run(id);
        // Progress: the latest message of those that have come, so the view redraws once per wake-up.
        cx.spawn(async move |this, cx| {
            while let Ok(mut latest) = progress.recv().await {
                while let Ok(newer) = progress.try_recv() {
                    latest = newer;
                }
                let alive = this
                    .update(cx, |this, cx| {
                        if this.busy.is_some() {
                            this.busy = Some(latest);
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let answer = done.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match answer {
                    Ok(run) => {
                        if !run.ok {
                            this.error = Some(
                                run.error
                                    .clone()
                                    .unwrap_or_else(|| t!("The backup didn't finish").to_string())
                                    .into(),
                            );
                        }
                        if let PlanState::Planned(plan) = &mut this.plan {
                            plan.last = Some(run.clone());
                        }
                        this.result = Some(run);
                    }
                    // Stop was pressed: nothing to report.
                    Err(reason) if reason == "cancelled" => {}
                    Err(reason) => this.error = Some(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Sets the backup up with `password`, then backs up (`setUp`).
    pub fn set_up(&mut self, password: String, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.error = None;
        self.busy = Some(BackupProgress {
            stage: "Setting up the backup".into(),
            ..Default::default()
        });
        cx.notify();
        let asking = self
            .backend
            .setup(self.folder.clone(), self.repository(), password, self.schedule);
        cx.spawn(async move |this, cx| {
            let answer = asking.await;
            let _ = this.update(cx, |this, cx| match answer {
                Ok(plan) => {
                    let id = plan.id.clone();
                    this.plan = PlanState::Planned(Box::new(plan));
                    this.busy = None;
                    cx.emit(BackupEvent::PlanMade);
                    this.back_up(id, cx);
                }
                Err(reason) => {
                    this.error = Some(reason.into());
                    this.busy = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Stops the backup that runs (`backup_cancel`).
    pub fn stop(&self) {
        self.backend.cancel();
    }

    /// Stops backing the folder up (`backup_remove`); the backups made stay.
    pub fn stop_backing_up(&mut self, cx: &mut Context<Self>) {
        let PlanState::Planned(plan) = &self.plan else { return };
        let asking = self.backend.remove(plan.id.clone());
        cx.spawn(async move |this, cx| {
            let answer = asking.await;
            let _ = this.update(cx, |this, cx| match answer {
                Ok(()) => cx.emit(BackupEvent::Removed),
                Err(reason) => {
                    this.error = Some(reason.into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Picks the drive's folder with Windows' picker (`pickFolder`).
    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let picked = self.backend.pick_folder(t!("Where to back up").to_string(), owner);
        cx.spawn(async move |this, cx| {
            if let Ok(Some(folder)) = picked.await {
                let _ = this.update(cx, |this, cx| this.set_drive(folder, cx));
            }
        })
        .detach();
    }
}

// ---- The dialog -----------------------------------------------------------------------------------------------

/// The tools model the install cards read (none without core, as in the tests).
fn tools_for(window: &mut Window, cx: &mut App) -> Option<Entity<Tools>> {
    let core = cx.try_global::<core_bridge::Core>()?;
    let backend: Arc<dyn SettingsBackend> = Arc::new(SettingsCoreBackend::new(core.app.clone()));
    Some(cx.new(|cx| Tools::new(backend, window, cx)))
}

/// Whether the tool `id` is there, according to the tools model (`useTool(id)?.origin`).
fn has_tool(tools: &Entity<Tools>, id: &str, cx: &App) -> bool {
    tools.read(cx).tool(id).is_some_and(|tool| tool.origin.is_some())
}

/// The Back up dialog.
pub struct BackupDialog {
    model: Entity<Backup>,
    tools: Option<Entity<Tools>>,
    password: Entity<InputState>,
    again: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl BackupDialog {
    /// A dialog over `model`. `tools` is the model the install cards read (None: no cards, and the tools are
    /// taken from what the model was told).
    pub fn new(
        model: Entity<Backup>,
        tools: Option<Entity<Tools>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let make = |placeholder: SharedString, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(placeholder))
        };
        let password = make(t!("At least 8 characters"), window, cx);
        let again = make(t!("Again"), window, cx);
        let mut subscriptions = vec![cx.observe(&model, |_, _, cx| cx.notify())];
        for input in [&password, &again] {
            subscriptions.push(
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }),
            );
        }
        subscriptions.push(
            cx.subscribe_in(&model, window, |this, _, event: &BackupEvent, window, cx| match event {
                BackupEvent::PlanMade => {
                    for input in [&this.password, &this.again] {
                        input.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                }
                BackupEvent::Removed => {
                    let name = this.model.read(cx).name();
                    window.close_dialog(cx);
                    window.push_notification(
                        Notification::info(t!("The backups made so far stay where they are."))
                            .title(t!("“{name}” isn't backed up any more", name = name)),
                        cx,
                    );
                }
            }),
        );
        if let Some(tools) = &tools {
            let tools_model = tools.clone();
            let backup = model.clone();
            let sync = move |cx: &mut App| {
                let (restic, rclone) = (
                    has_tool(&tools_model, "restic", cx),
                    has_tool(&tools_model, "rclone", cx),
                );
                backup.update(cx, |backup, cx| backup.set_tools(restic, rclone, cx));
            };
            let first = sync.clone();
            subscriptions.push(cx.observe(tools, move |_, _, cx| sync(cx)));
            first(cx);
        }
        Self {
            model,
            tools,
            password,
            again,
            _subscriptions: subscriptions,
        }
    }

    /// The password box, for a test to type into.
    pub fn password_input(&self) -> &Entity<InputState> {
        &self.password
    }

    /// The box that repeats it.
    #[cfg(test)]
    pub fn again_input(&self) -> &Entity<InputState> {
        &self.again
    }

    fn password_ok(&self, cx: &App) -> bool {
        Backup::password_ok(&self.password.read(cx).value(), &self.again.read(cx).value())
    }

    /// Enter: back up now (a plan) or set up (a ready form), unless it is busy (`onSubmit`).
    fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let ok = self.password_ok(cx);
        let password = self.password.read(cx).value().to_string();
        self.model.update(cx, |model, cx| {
            if model.busy.is_some() {
                return;
            }
            match model.plan.clone() {
                PlanState::Planned(plan) => model.back_up(plan.id, cx),
                PlanState::Unplanned if model.ready(ok) => model.set_up(password, cx),
                _ => {}
            }
        });
    }

    /// A row of options of which one is picked, each with its hint under it (`Choices`).
    fn choices<T: PartialEq + Copy + 'static>(
        id: &'static str,
        name: SharedString,
        current: T,
        options: Vec<(T, SharedString, SharedString)>,
        on_pick: impl Fn(&mut Backup, T, &mut Context<Backup>) + Clone + 'static,
        model: &Entity<Backup>,
        t: &theme::Tokens,
    ) -> impl IntoElement {
        row_start()
            .id(id)
            .role(Role::RadioGroup)
            .aria_label(name)
            .test_support()
            .gap(space(1.5))
            .children(options.into_iter().enumerate().map(|(i, (value, label, hint))| {
                let on = value == current;
                let model = model.clone();
                let on_pick = on_pick.clone();
                Button::new((id, i))
                    .outline()
                    .selected(on)
                    .toggled(on)
                    .h_auto()
                    .flex_1()
                    .min_w_0()
                    .py(space(1.5))
                    .accessibility_label(format!("{label}: {hint}"))
                    .on_click(move |_, _, cx| model.update(cx, |model, cx| on_pick(model, value, cx)))
                    .child(
                        col()
                            .min_w_0()
                            .align_start()
                            .text_start()
                            .child(div().font_weight(FontWeight::MEDIUM).child(label))
                            .child(
                                div()
                                    .text_size(theme::size::XXS)
                                    .text_color(t.muted_foreground)
                                    .child(hint),
                            ),
                    )
            }))
    }

    fn heading(text: SharedString, t: &theme::Tokens) -> impl IntoElement {
        div()
            .text_size(theme::size::XS)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.muted_foreground)
            .child(text)
    }

    /// The When choice, with `hint` under "When I ask".
    fn when_choice(
        id: &'static str,
        current: Schedule,
        hint: SharedString,
        on_pick: impl Fn(&mut Backup, Schedule, &mut Context<Backup>) + Clone + 'static,
        model: &Entity<Backup>,
        t: &theme::Tokens,
    ) -> impl IntoElement {
        col()
            .gap(space(1.5))
            .child(Self::heading(t!("When"), t))
            .child(Self::choices(
                id,
                t!("When"),
                current,
                vec![
                    (Schedule::Manual, t!("When I ask"), hint),
                    (Schedule::Daily, t!("Every day"), t!("While Glint runs")),
                    (Schedule::Weekly, t!("Every week"), t!("While Glint runs")),
                ],
                on_pick,
                model,
                t,
            ))
    }

    /// The progress bar and what is being backed up (`BackupRunning`).
    fn running(progress: &BackupProgress, t: &theme::Tokens) -> impl IntoElement {
        let stage = stage_text(&progress.stage);
        let line: SharedString = if progress.total_files > 0 {
            t!(
                "{stage} · {done} of {total} files, {bytes} of {size}",
                stage = stage,
                done = count(progress.files_done),
                total = count(progress.total_files),
                bytes = format_size(progress.bytes_done),
                size = format_size(progress.total_bytes)
            )
        } else {
            stage
        };
        let share = (progress.fraction * 100.0).clamp(0.0, 100.0);
        col()
            .id("backup-running")
            .role(Role::Group)
            .aria_label(line.clone())
            .test_support()
            .gap(space(2.))
            .py(space(4.))
            .child(
                align_row(2.)
                    .text_color(t.muted_foreground)
                    .child(Spinner::new())
                    .child(div().min_w_0().truncate().child(line.clone())),
            )
            .child(
                row_start()
                    .id("backup-progress")
                    .role(Role::ProgressIndicator)
                    .aria_label(t!("{title}: {n}%", title = t!("Back up"), n = js_round(share) as u64))
                    .test_support()
                    .h(space(1.5))
                    .w_full()
                    .overflow_hidden()
                    .rounded_full()
                    .bg(t.muted)
                    .child(
                        div()
                            .h_full()
                            .w(relative((share / 100.0) as f32))
                            .rounded_full()
                            .bg(t.primary),
                    ),
            )
            .when(!progress.current.is_empty(), |this| {
                this.child(
                    div()
                        .truncate()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(isolate(progress.current.clone())),
                )
            })
    }

    /// What a backup did (`BackupDone`).
    fn done(run: &LastRun, t: &theme::Tokens) -> impl IntoElement {
        let headline = t!(
            "Backed up {n} files, {size}",
            n = count(run.total_files),
            size = format_size(run.total_bytes)
        );
        let seconds = number_to_string(js_round(run.elapsed_ms / 100.0) / 10.0);
        let detail: SharedString = if run.files_new != 0 || run.files_changed != 0 {
            t!(
                "{new}, {changed}; {size} added · {seconds} s",
                new = plural(usize::try_from(run.files_new).unwrap_or(usize::MAX), "new file"),
                changed = plural(usize::try_from(run.files_changed).unwrap_or(usize::MAX), "changed file"),
                size = format_size(run.bytes_added),
                seconds = seconds
            )
        } else {
            t!(
                "Nothing had changed since the last backup · {seconds} s",
                seconds = seconds
            )
        };
        let skipped = (run.skipped_count > 0).then(|| {
            let n = usize::try_from(run.skipped_count).unwrap_or(usize::MAX);
            let words = if n == 1 {
                t!(
                    "{what} couldn't be read (an app holds it open):",
                    what = plural(n, "file")
                )
            } else {
                t!(
                    "{what} couldn't be read (an app holds them open):",
                    what = plural(n, "file")
                )
            };
            col()
                .id("backup-skipped")
                .role(Role::Group)
                .aria_label(words.clone())
                .test_support()
                .gap(space(1.))
                .rounded(theme::size::RADIUS)
                .bg(t.warning.opacity(0.1))
                .px(space(3.))
                .py(space(2.))
                .text_size(theme::size::XS)
                .child(div().font_weight(FontWeight::MEDIUM).child(words))
                .children(
                    run.skipped
                        .iter()
                        .take(5)
                        .map(|path| div().truncate().child(isolate(path.clone()))),
                )
        });
        col()
            .id("backup-done")
            .role(Role::Group)
            .aria_label(headline.clone())
            .test_support()
            .gap(space(2.))
            .child(
                align_row(2.)
                    .items_start()
                    .child(
                        icon(IconName::CircleCheck)
                            .size_5()
                            .flex_none()
                            .mt(space(0.5))
                            .text_color(t.success),
                    )
                    .child(
                        col()
                            .align_start()
                            .child(div().font_weight(FontWeight::MEDIUM).child(headline))
                            .child(
                                div()
                                    .text_size(theme::size::XS)
                                    .text_color(t.muted_foreground)
                                    .child(detail),
                            ),
                    ),
            )
            .children(skipped)
            .when_some(run.error.clone(), |this, error| {
                this.child(div().text_size(theme::size::XS).text_color(t.warning).child(error))
            })
    }
}

impl Render for BackupDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let model = self.model.clone();
        let m = self.model.read(cx);
        let busy = m.busy.clone();
        let plan = m.plan.clone();
        let result = m.result.clone();
        let error = m.error.clone();
        let place = m.place;
        let schedule = m.schedule;
        let ready = m.ready(self.password_ok(cx));
        let restic = m.restic;
        let folder = m.folder.clone();
        let repository = m.repository();
        let drive = m.drive.clone();
        let remotes = m.remotes.clone();
        let remote = m.remote.clone();
        let name = m.name();

        let tool_card = |id: &str, why: SharedString, icon_name: IconName, cx: &App| {
            self.tools
                .as_ref()
                .and_then(|tools| tools::needed(tools, id, why, icon_name, cx))
        };

        let mut body = col().gap(space(4.)).text_size(theme::size::SM);
        body = body.child(
            div()
                .id("backup-folder")
                .truncate()
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(isolate(folder.clone())),
        );
        body = body.children(tool_card(
            "restic",
            t!("restic backs folders up, encrypted, each backup adding only what changed."),
            IconName::DatabaseBackup,
            cx,
        ));
        if plan == PlanState::Looking {
            body = body.child(
                div()
                    .id("backup-looking")
                    .role(Role::Status)
                    .aria_label(t!("Looking…"))
                    .test_support()
                    .text_color(t.muted_foreground)
                    .child(t!("Looking…")),
            );
        }
        if let Some(progress) = &busy {
            body = body.child(Self::running(progress, &t));
        }
        if busy.is_none() {
            if let Some(run) = result.as_ref().filter(|r| r.ok) {
                body = body.child(Self::done(run, &t));
            }
            if let Some(error) = &error {
                body = body.child(
                    align_row(2.)
                        .id("backup-error")
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .items_start()
                        .rounded(theme::size::RADIUS)
                        .bg(t.destructive.opacity(0.1))
                        .px(space(3.))
                        .py(space(2.))
                        .text_size(theme::size::XS)
                        .text_color(t.destructive)
                        .child(icon(IconName::TriangleAlert).size_4().flex_none())
                        .child(div().flex_1().min_w_0().child(error.clone())),
                );
            }
        }
        if let (None, PlanState::Planned(plan)) = (&busy, &plan) {
            let last = plan.last.as_ref().map(|last| {
                let when = format_date(last.at, None, crate::i18n::language());
                if last.ok {
                    t!(
                        " · last backup {date}, {n} files, {size}",
                        date = when,
                        n = count(last.total_files),
                        size = format_size(last.total_bytes)
                    )
                } else {
                    t!(" · last try {date}", date = when)
                }
            });
            let headline = t!("Backed up to {where}", where = describe_repository(&plan.repository));
            let summary: SharedString = format!("{headline}{}", last.clone().unwrap_or_default()).into();
            body = body.child(
                col()
                    .gap(space(3.))
                    .child(
                        div()
                            .id("backup-plan")
                            .role(Role::Group)
                            .aria_label(summary.clone())
                            .test_support()
                            .child(div().font_weight(FontWeight::MEDIUM).child(headline))
                            .when_some(last, |this, last| {
                                this.child(div().text_color(t.muted_foreground).child(last))
                            }),
                    )
                    .when_some(
                        plan.last
                            .as_ref()
                            .filter(|last| !last.ok && error.is_none())
                            .and_then(|last| last.error.clone()),
                        |this, failed| {
                            this.child(div().text_size(theme::size::XS).text_color(t.destructive).child(failed))
                        },
                    )
                    .child(Self::when_choice(
                        "backup-plan-when",
                        plan.schedule,
                        t!("Back up now"),
                        |model, schedule, cx| model.change_plan_schedule(schedule, cx),
                        &model,
                        &t,
                    ))
                    .child(
                        div()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(t!(
                                "Glint keeps 7 daily, 4 weekly and 12 monthly backups, and checks them once a month. They show in Previous versions, beside Windows' own."
                            )),
                    ),
            );
        }
        if busy.is_none() && plan == PlanState::Unplanned && result.is_none() {
            body = body.child(self.setup_form(place, schedule, &repository, &drive, &remotes, &remote, &t, cx));
        }

        let planned = match (&busy, &plan) {
            (None, PlanState::Planned(plan)) => Some(plan.clone()),
            _ => None,
        };
        let footer = {
            let mut footer = align_row(2.)
                .px(space(4.))
                .py(space(2.5))
                .border_t_1()
                .border_color(t.border);
            if busy.is_some() {
                footer = footer.child(div().flex_1()).child(
                    Button::new("backup-stop")
                        .outline()
                        .small()
                        .icon(IconName::Square)
                        .label(t!("Stop"))
                        .on_click(cx.listener(|this, _, _, cx| this.model.read(cx).stop())),
                );
            } else {
                if planned.is_some() {
                    let folder = folder.clone();
                    footer = footer
                        .child(
                            Button::new("backup-stop-backing-up")
                                .ghost()
                                .small()
                                .label(t!("Stop backing up"))
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.model.update(cx, |m, cx| m.stop_backing_up(cx))),
                                ),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("backup-versions")
                                .outline()
                                .small()
                                .icon(IconName::RotateCcwClock)
                                .label(t!("Backups"))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    open_backups(window, cx, folder.clone());
                                }),
                        )
                        .child(
                            Button::new("backup-close")
                                .outline()
                                .small()
                                .label(t!("Close"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        );
                } else {
                    footer = footer.child(div().flex_1()).child(
                        Button::new("backup-close")
                            .outline()
                            .small()
                            .label(t!("Close"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    );
                }
                if let Some(plan) = planned {
                    footer = footer.child(
                        Button::new("backup-now")
                            .primary()
                            .small()
                            .icon(IconName::DatabaseBackup)
                            .label(t!("Back up now"))
                            .disabled(!restic)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = plan.id.clone();
                                this.model.update(cx, |m, cx| m.back_up(id, cx));
                            })),
                    );
                } else if plan == PlanState::Unplanned && result.is_none() {
                    footer = footer.child(
                        Button::new("backup-set-up")
                            .primary()
                            .small()
                            .icon(IconName::DatabaseBackup)
                            .label(t!("Back up"))
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    );
                }
            }
            footer
        };

        col()
            .id("backup")
            .role(Role::Group)
            .aria_label(t!("Back up “{name}”", name = name))
            .test_support()
            .text_start()
            .child(
                div()
                    .id("backup-body")
                    .max_h(rems(34.))
                    .overflow_y_scroll()
                    .px(space(1.))
                    .pb(space(4.))
                    .child(body),
            )
            .child(footer)
    }
}

impl BackupDialog {
    /// Where to, the password, and when, for a folder with no plan yet.
    #[allow(clippy::too_many_arguments)] // The form's parts, read once from the model in `render`.
    fn setup_form(
        &self,
        place: Where,
        schedule: Schedule,
        repository: &str,
        _drive: &str,
        remotes: &Option<Vec<Remote>>,
        remote: &str,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let model = self.model.clone();
        let again_differs = {
            let again = self.again.read(cx).value();
            !again.is_empty() && again != self.password.read(cx).value()
        };
        let where_picker = match place {
            Where::Drive => align_row(2.)
                .child(
                    div()
                        .id("backup-repository")
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(if repository.is_empty() {
                            t!("No place chosen yet")
                        } else {
                            isolate(repository.to_string())
                        }),
                )
                .child(
                    Button::new("backup-choose")
                        .outline()
                        .small()
                        .label(t!("Choose…"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.model.update(cx, |model, cx| model.choose_folder(window, cx));
                        })),
                )
                .into_any_element(),
            Where::Cloud => {
                let rclone_card = self.tools.as_ref().and_then(|tools| {
                    tools::needed(
                        tools,
                        "rclone",
                        t!("rclone reaches the clouds you've added in Remote."),
                        IconName::CloudUpload,
                        cx,
                    )
                });
                match (rclone_card, remotes) {
                    (Some(card), _) => card,
                    (None, Some(all)) if all.is_empty() => div()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(t!(
                            "No cloud has been added yet: add one under Remote in the sidebar, then come back."
                        ))
                        .into_any_element(),
                    (None, all) => col()
                        .id("backup-clouds")
                        .role(Role::RadioGroup)
                        .aria_label(t!("Cloud"))
                        .test_support()
                        .gap(space(1.))
                        .children(all.iter().flatten().enumerate().map(|(i, r)| {
                            let on = r.name == remote;
                            let label = format!("{} ({})", r.name, r.kind);
                            let (model, picked) = (model.clone(), r.name.clone());
                            Button::new(("backup-cloud", i))
                                .ghost()
                                .small()
                                .selected(on)
                                .toggled(on)
                                .justify_start()
                                .label(label)
                                .on_click(move |_, _, cx| {
                                    let picked = picked.clone();
                                    model.update(cx, |m, cx| m.set_remote(picked, cx));
                                })
                        }))
                        .into_any_element(),
                }
            }
        };

        col()
            .gap(space(4.))
            .child(div().text_color(t.muted_foreground).child(t!(
                "Each backup adds only what changed since the last, encrypted, so even a cloud sees nothing but scrambled data. Earlier backups of every file stay to go back to."
            )))
            .child(
                col()
                    .gap(space(1.5))
                    .child(Self::heading(t!("To"), t))
                    .child(Self::choices(
                        "backup-to",
                        t!("To"),
                        place,
                        vec![
                            (
                                Where::Drive,
                                t!("A drive or folder"),
                                t!("A USB drive, another disk, a network folder"),
                            ),
                            (Where::Cloud, t!("A cloud"), t!("One added in Remote (rclone)")),
                        ],
                        |model, place, cx| model.choose_place(place, cx),
                        &model,
                        t,
                    ))
                    .child(where_picker),
            )
            .child(
                col()
                    .gap(space(2.))
                    .child(Self::heading(t!("Password"), t))
                    .child(Input::new(&self.password).small().aria_label(t!("Password")))
                    .child(
                        div()
                            .rounded(theme::size::RADIUS)
                            .border_1()
                            .border_color(if again_differs { t.destructive } else { t.border.opacity(0.) })
                            .child(Input::new(&self.again).small().aria_label(t!("The password again"))),
                    )
                    .child(
                        div()
                            .id("backup-password-note")
                            .role(Role::Group)
                            .aria_label(t!("Without this password nobody can read the backup, Glint neither. It's kept in Windows Credential Manager on this PC: write it down somewhere else too. An existing backup at that place opens with its own password."))
                            .test_support()
                            .text_size(theme::size::XS)
                            .text_color(t.warning)
                            .child(t!("Without this password nobody can read the backup, Glint neither. It's kept in Windows Credential Manager on this PC: write it down somewhere else too. An existing backup at that place opens with its own password.")),
                    ),
            )
            .child(Self::when_choice(
                "backup-when",
                schedule,
                t!("From here"),
                |model, schedule, cx| model.set_schedule(schedule, cx),
                &model,
                t,
            ))
            .child(
                div()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!("Files an app keeps open (Outlook's mailbox, say) are skipped and listed.")),
            )
    }
}

/// "Backups": Previous versions of the folder, which lists the backups made of it. What it hands back is done
/// here: a backup's folder opens in a new tab, "Show" goes to the restored item.
fn open_backups(window: &mut Window, cx: &mut App, folder: String) {
    super::versions::open(window, cx, folder, |request, window, cx| {
        use super::versions::Request;
        let Some(tabs) = explorer::try_explorer(cx) else { return };
        match request {
            Request::OpenFolderInNewTab(path) => tabs.update(cx, |e, cx| e.open_in_new_tab(&path, cx)),
            Request::Show { folder, select } => tabs.update(cx, |e, cx| {
                let options = explorer::NavOptions {
                    select: Some(vec![select]),
                    ..Default::default()
                };
                e.navigate(folder, options, cx)
            }),
            // The dialog closed itself first; how it was goes on the left.
            Request::CompareFiles { then, now } => super::compare_files::open(window, cx, then, now),
            // This folder now beside how it was, compared in the two panes (`compareWithVersion`).
            Request::CompareFolders { now, then } => {
                tabs.update(cx, |e, cx| e.compare_with_version(&now, &then, cx).detach())
            }
        }
    });
}

/// Shows the dialog in the window's dialog layer. Like [`super::show`], except that Escape, the close button
/// and the backdrop do nothing while a backup runs (`!busy && close()`).
fn show(window: &mut Window, cx: &mut App, title: SharedString, view: Entity<BackupDialog>, model: Entity<Backup>) {
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let model = model.clone();
        dialog
            .title(title.clone())
            .w(px(576.))
            .on_ok(|_, _, _| false)
            .on_cancel(move |_, _, cx| model.read(cx).busy.is_none())
            .child(view.clone())
    });
}

/// Opens Back up for `folder` (`openBackup`). The caller has checked [`can_back_up`]; when it fails, the app
/// says "Back up works on folders on this PC's drives" instead ([`say_not_here`]).
pub fn open(window: &mut Window, cx: &mut App, folder: String) {
    let backend: Arc<dyn Backend> = match cx.try_global::<core_bridge::Core>() {
        Some(core) => Arc::new(CoreBackend::new(core.app.clone())),
        None => {
            // Never in the app; but a press that does nothing at all would look broken.
            window.push_notification(Notification::error(t!("Glint's core isn't running")), cx);
            return;
        }
    };
    let tools = tools_for(window, cx);
    open_with(window, cx, folder, backend, tools);
}

/// [`open`] with a backend and tools of your own (tests pass fakes). Returns the model and the view.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    folder: String,
    backend: Arc<dyn Backend>,
    tools: Option<Entity<Tools>>,
) -> (Entity<Backup>, Entity<BackupDialog>) {
    let model = cx.new(|cx| Backup::new(folder, backend, cx));
    let name = model.read(cx).name();
    let view = cx.new(|cx| BackupDialog::new(model.clone(), tools, window, cx));
    show(
        window,
        cx,
        t!("Back up “{name}”", name = name),
        view.clone(),
        model.clone(),
    );
    (model, view)
}

/// A scheduled backup that ran while Glint was open (core's `backup-finished`, which App.tsx listened for): a
/// quiet notice when it worked, a loud one when it didn't. Returns (worked, title, detail), or nothing for a
/// payload that names no source.
pub fn finished_words(payload: &serde_json::Value) -> Option<(bool, String, String)> {
    let source = payload.get("source")?.as_str()?;
    // The folder's own name, else all of it (`source.split("\\").pop() || source`).
    let name = source.rsplit('\\').next().filter(|n| !n.is_empty()).unwrap_or(source);
    let result = payload.get("result").filter(|r| !r.is_null());
    let result_error = result.and_then(|r| r.get("error")).and_then(|e| e.as_str());
    let worked = result
        .and_then(|r| r.get("ok"))
        .and_then(|ok| ok.as_bool())
        .unwrap_or(false);
    if worked {
        return Some((
            true,
            t!("Backed up “{name}”", name = name).to_string(),
            result_error.unwrap_or_default().to_string(),
        ));
    }
    let reason = result_error
        .or_else(|| payload.get("error").and_then(|e| e.as_str()))
        .unwrap_or_default();
    Some((
        false,
        t!("Couldn't back up “{name}”", name = name).to_string(),
        reason.to_string(),
    ))
}

/// Says in the main window what a scheduled backup did (`notify` shows a notification there).
pub fn tell_finished(payload: &serde_json::Value, notify: impl FnOnce(Notification)) {
    if let Some((worked, title, detail)) = finished_words(payload) {
        notify(if worked {
            Notification::info(detail).title(title)
        } else {
            Notification::error(detail).title(title)
        });
    }
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    #[test]
    fn a_scheduled_backup_that_finished_is_told_quietly_and_one_that_failed_loudly() {
        let _guard = testing::lock();
        let worked = serde_json::json!({
            "source": "C:\\Users\\me\\Documents",
            "result": { "ok": true, "error": null, "totalFiles": 12 },
            "error": null
        });
        assert_eq!(
            finished_words(&worked),
            Some((true, "Backed up “Documents”".to_string(), String::new()))
        );
        // A backup that worked with a warning keeps the warning.
        let warned = serde_json::json!({
            "source": "D:\\Photos",
            "result": { "ok": true, "error": "2 files were busy" },
            "error": null
        });
        assert_eq!(
            finished_words(&warned),
            Some((true, "Backed up “Photos”".to_string(), "2 files were busy".to_string()))
        );
        // Its result's own reason first, else the run's.
        let failed = serde_json::json!({
            "source": "D:\\Photos",
            "result": { "ok": false, "error": "The repository is locked" },
            "error": "ignored"
        });
        assert_eq!(
            finished_words(&failed),
            Some((
                false,
                "Couldn't back up “Photos”".to_string(),
                "The repository is locked".to_string()
            ))
        );
        let never_ran = serde_json::json!({ "source": "D:\\", "result": null, "error": "restic isn't installed" });
        assert_eq!(
            finished_words(&never_ran),
            Some((
                false,
                "Couldn't back up “D:\\”".to_string(),
                "restic isn't installed".to_string()
            ))
        );
        assert_eq!(finished_words(&serde_json::json!({ "result": null })), None);
    }

    fn plan(source: &str) -> Plan {
        Plan {
            id: "abc".into(),
            source: source.into(),
            repository: "rclone:drive:Glint backup".into(),
            schedule: Schedule::Daily,
            last: Some(LastRun {
                at: 1_700_000_000_000,
                ok: true,
                total_files: 1234,
                total_bytes: 5 * 1024 * 1024,
                ..Default::default()
            }),
            checked: 0,
        }
    }

    fn run_ok() -> LastRun {
        LastRun {
            at: 1_700_000_000_000,
            ok: true,
            snapshot: "deadbeef".into(),
            files_new: 2,
            files_changed: 1,
            bytes_added: 2048,
            total_files: 10,
            total_bytes: 4096,
            skipped: vec!["C:\\mail\\box.pst".into()],
            skipped_count: 1,
            error: None,
            elapsed_ms: 2540.0,
        }
    }

    struct Fake {
        plans: Vec<Plan>,
        remotes: Vec<Remote>,
        result: Mutex<Result<LastRun, String>>,
        calls: Mutex<Vec<String>>,
        picked: Option<String>,
    }

    impl Fake {
        fn new(plans: Vec<Plan>) -> Arc<Fake> {
            Arc::new(Fake {
                plans,
                remotes: vec![
                    Remote {
                        name: "gdrive".into(),
                        kind: "drive".into(),
                        provider: String::new(),
                    },
                    Remote {
                        name: "dropbox".into(),
                        kind: "dropbox".into(),
                        provider: String::new(),
                    },
                ],
                result: Mutex::new(Ok(run_ok())),
                calls: Mutex::new(Vec::new()),
                picked: Some("E:\\".into()),
            })
        }

        fn log(&self, what: impl Into<String>) {
            self.calls.lock().unwrap().push(what.into());
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Backend for Fake {
        fn plans(&self) -> Fut<Vec<Plan>> {
            let plans = self.plans.clone();
            Box::pin(async move { Ok(plans) })
        }

        fn setup(&self, source: String, repository: String, password: String, schedule: Schedule) -> Fut<Plan> {
            self.log(format!("setup {source} {repository} {password} {schedule:?}"));
            Box::pin(async move {
                Ok(Plan {
                    id: "new".into(),
                    source,
                    repository,
                    schedule,
                    last: None,
                    checked: 0,
                })
            })
        }

        fn run(&self, id: String) -> Running {
            self.log(format!("run {id}"));
            let (send, progress) = async_channel::unbounded();
            let _ = send.try_send(BackupProgress {
                stage: "Backing up".into(),
                fraction: 0.5,
                files_done: 5,
                total_files: 10,
                bytes_done: 2048,
                total_bytes: 4096,
                current: "C:\\data\\ملف.txt".into(),
            });
            let result = self.result.lock().unwrap().clone();
            Running {
                done: Box::pin(async move {
                    // Hold the sender until the run ends, as core's channel does.
                    let _keep = send;
                    result
                }),
                progress,
            }
        }

        fn cancel(&self) {
            self.log("cancel");
        }

        fn schedule(&self, id: String, schedule: Schedule) -> Fut<Plan> {
            self.log(format!("schedule {id} {schedule:?}"));
            let mut changed = self.plans.first().cloned().unwrap_or_else(|| plan("C:\\x"));
            changed.schedule = schedule;
            Box::pin(async move { Ok(changed) })
        }

        fn remove(&self, id: String) -> Fut<()> {
            self.log(format!("remove {id}"));
            Box::pin(async { Ok(()) })
        }

        fn rclone_remotes(&self) -> Fut<Vec<Remote>> {
            let remotes = self.remotes.clone();
            Box::pin(async move { Ok(remotes) })
        }

        fn pick_folder(&self, _: String, _: isize) -> Fut<Option<String>> {
            let picked = self.picked.clone();
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

    fn open_dialog(
        cx: &mut VisualTestContext,
        fake: &Arc<Fake>,
        folder: &str,
    ) -> (Entity<Backup>, Entity<BackupDialog>) {
        let backend: Arc<dyn Backend> = fake.clone();
        let folder = folder.to_string();
        let opened = cx.update(|window, cx| open_with(window, cx, folder, backend, None));
        cx.run_until_parked();
        opened
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::DatabaseBackup,
            IconName::CloudUpload,
            IconName::RotateCcwClock,
            IconName::Square,
            IconName::CircleCheck,
            IconName::TriangleAlert,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn only_folders_on_this_pcs_drives_can_be_backed_up() {
        assert!(can_back_up("C:\\Users\\me\\Photos", true, false, false, false, false));
        assert!(!can_back_up("C:\\Users\\me\\a.txt", false, false, false, false, false));
        assert!(!can_back_up("\\\\nas\\share", true, false, false, false, false));
        assert!(!can_back_up("remote:nas\\x", true, false, false, false, false));
        assert!(!can_back_up("C:\\Cloud", true, true, false, false, false));
        assert!(!can_back_up("C:\\a.zip\\in", true, false, true, false, false));
        assert!(!can_back_up("C:\\Recycle", true, false, false, false, true));
    }

    #[test]
    fn repositories_are_described_in_words() {
        let _guard = testing::lock();
        assert_eq!(describe_repository("rclone:gdrive:Glint backup"), "gdrive (a cloud)");
        assert_eq!(describe_repository("E:\\Glint backup"), "E:\\Glint backup");
    }

    #[test]
    fn passwords_need_eight_characters_that_match() {
        assert!(Backup::password_ok("12345678", "12345678"));
        assert!(!Backup::password_ok("1234567", "1234567"));
        assert!(!Backup::password_ok("12345678", "12345679"));
        // Characters, not bytes: eight Arabic letters are 16 bytes.
        assert!(Backup::password_ok("كلمةالسر!", "كلمةالسر!"));
    }

    #[gpui_kit::test]
    fn without_core_opening_backup_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        // The test app starts a core; take it away for this press.
        let core = visual.update(|_, cx| cx.remove_global::<core_bridge::Core>());
        visual.update(|window, cx| open(window, cx, r"C:\Data".to_string()));
        visual.run_until_parked();
        visual.update(|_, cx| cx.set_global(core));
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "no dialog without core"));
        visual.update(|window, cx| assert_eq!(window.notifications(cx).len(), 1, "it says so"));
    }

    #[gpui_kit::test]
    fn a_folder_with_a_plan_shows_it_and_backs_up_with_progress(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(vec![plan("C:\\Data")]);
        // A plan for another folder doesn't count; this one matches ignoring case.
        let (model, _) = open_dialog(&mut visual, &fake, "c:\\data");
        settle(&mut visual, "the plan", |cx| {
            matches!(model.read_with(cx, |m, _| m.plan().clone()), PlanState::Planned(_))
        });
        assert_eq!(role_of(&mut visual, "Back up “data”"), Some(Role::Group));
        let when = format_date(1_700_000_000_000, None, crate::i18n::language());
        let line = format!(
            "Backed up to drive (a cloud) · last backup {when}, {} files, {}",
            count(1234),
            format_size(5 * 1024 * 1024)
        );
        assert_eq!(role_of(&mut visual, &line), Some(Role::Group), "the plan line");
        assert_eq!(role_of(&mut visual, "When"), Some(Role::RadioGroup));

        visual.update(|_, cx| model.update(cx, |m, cx| m.set_tools(true, true, cx)));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("backup-now", cx);
        });
        // Busy: the progress is drawn, and the dialog cannot be closed.
        assert!(model.read_with(&visual, |m, _| m.busy().is_some()));
        settle(&mut visual, "the result", |cx| {
            model.read_with(cx, |m, _| m.result().is_some())
        });
        assert_eq!(fake.calls(), ["run abc"]);
        assert_eq!(role_of(&mut visual, "Backed up 10 files, 4.0 KB"), Some(Role::Group));
        assert!(has(&mut visual, "1 file couldn't be read (an app holds it open):"));
        assert!(!model.read_with(&visual, |m, _| m.busy().is_some()));
    }

    #[gpui_kit::test]
    fn a_failed_backup_says_why_and_stop_is_offered_while_it_runs(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(vec![plan("C:\\Data")]);
        *fake.result.lock().unwrap() = Err("The repository is locked".into());
        let (model, _) = open_dialog(&mut visual, &fake, "C:\\Data");
        settle(&mut visual, "the plan", |cx| {
            matches!(model.read_with(cx, |m, _| m.plan().clone()), PlanState::Planned(_))
        });
        visual.update(|_, cx| model.update(cx, |m, cx| m.set_tools(true, true, cx)));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("backup-now", cx);
        });
        settle(&mut visual, "the error", |cx| {
            model.read_with(cx, |m, _| m.error().is_some())
        });
        assert_eq!(role_of(&mut visual, "The repository is locked"), Some(Role::Alert));
        // A cancelled backup is not an error.
        *fake.result.lock().unwrap() = Err("cancelled".into());
        model.update_in(&mut visual, |m, _, cx| m.back_up("abc".into(), cx));
        settle(&mut visual, "the end", |cx| {
            model.read_with(cx, |m, _| m.busy().is_none())
        });
        assert!(model.read_with(&visual, |m, _| m.error().is_none()));
    }

    #[gpui_kit::test]
    fn a_new_backup_needs_a_place_and_a_matching_password_then_sets_up_and_runs(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(vec![]);
        let (model, view) = open_dialog(&mut visual, &fake, "C:\\Photos");
        settle(&mut visual, "the form", |cx| {
            model.read_with(cx, |m, _| *m.plan() == PlanState::Unplanned)
        });
        visual.update(|_, cx| model.update(cx, |m, cx| m.set_tools(true, true, cx)));
        let (password, again) = view.read_with(&visual, |v, _| (v.password_input().clone(), v.again_input().clone()));
        assert_eq!(role_of(&mut visual, "To"), Some(Role::RadioGroup));
        assert_eq!(role_of(&mut visual, "Password"), Some(Role::PasswordInput));
        // Nothing is chosen yet: Back up is disabled, and Enter sets nothing up.
        type_into(&mut visual, &password, "correct horse");
        type_into(&mut visual, &again, "correct horse");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        });
        assert!(fake.calls().is_empty(), "no place chosen: {:?}", fake.calls());
        // Choose the folder with the picker.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("backup-choose", cx);
        });
        settle(&mut visual, "the folder", |cx| {
            model.read_with(cx, |m, _| !m.drive().is_empty())
        });
        assert_eq!(model.read_with(&visual, |m, _| m.repository()), "E:\\Glint backup");
        // A password the second box doesn't repeat is not accepted.
        type_into(&mut visual, &again, "correct horsX");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        });
        assert!(fake.calls().is_empty());
        type_into(&mut visual, &again, "correct horse");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("backup-set-up", cx);
        });
        settle(&mut visual, "the run", |_| fake.calls().len() == 2);
        assert_eq!(
            fake.calls(),
            ["setup C:\\Photos E:\\Glint backup correct horse Daily", "run new"]
        );
        // The password is not kept in the boxes.
        assert!(password.read_with(&visual, |p, _| p.value().is_empty()));
    }

    #[gpui_kit::test]
    fn the_cloud_choice_lists_rclones_remotes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(vec![]);
        let (model, _) = open_dialog(&mut visual, &fake, "C:\\Photos");
        visual.update(|_, cx| model.update(cx, |m, cx| m.set_tools(true, true, cx)));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("backup-to", 1_usize), cx);
        });
        settle(&mut visual, "the remotes", |cx| {
            model.read_with(cx, |m, _| m.remotes().is_some())
        });
        assert_eq!(
            model.read_with(&visual, |m, _| m.repository()),
            "rclone:gdrive:Glint backup"
        );
        assert_eq!(role_of(&mut visual, "gdrive (drive)"), Some(Role::Button));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("backup-cloud", 1_usize), cx);
        });
        assert_eq!(
            model.read_with(&visual, |m, _| m.repository()),
            "rclone:dropbox:Glint backup"
        );
    }

    #[gpui_kit::test]
    fn without_restic_a_backup_cannot_start(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(vec![]);
        let (model, _) = open_dialog(&mut visual, &fake, "C:\\Photos");
        model.update_in(&mut visual, |m, _, cx| m.set_drive("E:\\".into(), cx));
        assert!(!model.read_with(&visual, |m, _| m.ready(true)));
        model.update_in(&mut visual, |m, _, cx| m.set_tools(true, false, cx));
        assert!(model.read_with(&visual, |m, _| m.ready(true)));
        assert!(!model.read_with(&visual, |m, _| m.ready(false)));
        // A cloud also needs rclone.
        model.update_in(&mut visual, |m, _, cx| {
            m.choose_place(Where::Cloud, cx);
            m.set_remote("gdrive".into(), cx);
        });
        assert!(!model.read_with(&visual, |m, _| m.ready(true)));
    }

    #[gpui_kit::test]
    fn changing_when_and_stopping_backing_up_reach_core_and_the_dialog_closes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(vec![plan("C:\\Data")]);
        let (model, _) = open_dialog(&mut visual, &fake, "C:\\Data");
        settle(&mut visual, "the plan", |cx| {
            matches!(model.read_with(cx, |m, _| m.plan().clone()), PlanState::Planned(_))
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("backup-plan-when", 2_usize), cx);
        });
        settle(&mut visual, "the schedule", |_| fake.calls().len() == 1);
        assert_eq!(fake.calls(), ["schedule abc Weekly"]);
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("backup-stop-backing-up", cx);
        });
        visual.run_until_parked();
        settle(&mut visual, "the removal", |_| fake.calls().len() == 2);
        assert_eq!(fake.calls()[1], "remove abc");
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "it closes"));
    }

    #[gpui_kit::test]
    fn escape_closes_it_unless_a_backup_is_running(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(vec![plan("C:\\Data")]);
        let (model, _) = open_dialog(&mut visual, &fake, "C:\\Data");
        settle(&mut visual, "the plan", |cx| {
            matches!(model.read_with(cx, |m, _| m.plan().clone()), PlanState::Planned(_))
        });
        // A run that never ends on its own.
        struct Endless;
        impl Backend for Endless {
            fn plans(&self) -> Fut<Vec<Plan>> {
                Box::pin(async { Ok(vec![]) })
            }
            fn setup(&self, _: String, _: String, _: String, _: Schedule) -> Fut<Plan> {
                Box::pin(async { Err("no".into()) })
            }
            fn run(&self, _: String) -> Running {
                let (_send, progress) = async_channel::unbounded();
                Running {
                    done: Box::pin(std::future::pending()),
                    progress,
                }
            }
            fn cancel(&self) {}
            fn schedule(&self, _: String, _: Schedule) -> Fut<Plan> {
                Box::pin(async { Err("no".into()) })
            }
            fn remove(&self, _: String) -> Fut<()> {
                Box::pin(async { Ok(()) })
            }
            fn rclone_remotes(&self) -> Fut<Vec<Remote>> {
                Box::pin(async { Ok(vec![]) })
            }
            fn pick_folder(&self, _: String, _: isize) -> Fut<Option<String>> {
                Box::pin(async { Ok(None) })
            }
        }
        visual.update(|window, cx| window.close_dialog(cx));
        visual.run_until_parked();
        let backend: Arc<dyn Backend> = Arc::new(Endless);
        let (model, _) = visual.update(|window, cx| open_with(window, cx, "C:\\Data".into(), backend, None));
        visual.run_until_parked();
        model.update_in(&mut visual, |m, _, cx| m.back_up("x".into(), cx));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(window.has_active_dialog(cx), "a backup is running"));
        assert_eq!(role_of(&mut visual, "Stop"), Some(Role::Button));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("backup-stop", cx);
        });
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let fake = Fake::new(vec![]);
        let (model, _) = open_dialog(&mut visual, &fake, "C:\\صور العائلة");
        settle(&mut visual, "the form", |cx| {
            model.read_with(cx, |m, _| *m.plan() == PlanState::Unplanned)
        });
        assert_eq!(model.read_with(&visual, |m, _| m.name()), "صور العائلة");
        // The name reaches the screen reader as it is, without the layout-only isolates.
        assert_eq!(role_of(&mut visual, "Back up “صور العائلة”"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "To"), Some(Role::RadioGroup));
        testing::set_rtl(false);
    }
}
