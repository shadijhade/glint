//! Encrypting and decrypting with age: "Encrypt…" and "Decrypt…".
//! Ported from src/components/explorer/VaultDialog.tsx, with the parts of ActionParts.tsx and actions.ts it used
//! (where what is made goes, the run, Ctrl+Z), which now live in `dialogs::action_run`; the run's progress, result
//! and footer are drawn here.
//!
//! | TypeScript                           | Here                                                                 |
//! |--------------------------------------|----------------------------------------------------------------------|
//! | `canEncrypt`, `isAgeFile`            | [`can_encrypt`], [`is_age_file`] (by name; the menus know the rest)   |
//! | `openVault(entries, mode)`           | [`open`]                                                              |
//! | `VaultDialog` / `VaultModal`         | [`VaultDialog`]                                                       |
//! | `invoke("vault_run")`, `vault_cancel`| [`Backend::run`], [`Backend::cancel`]                                 |
//! | `useActionRun`                       | [`Run`] (action_run)                                                               |
//! | `WhereChooser`, `badOutput`          | [`Where`], [`bad_output`]                                             |
//! | `RunProgress`, `RunResult`, `RunFooter` | the dialog's own progress, result and footer                       |
//! | `offerActionUndo`                    | [`remember_undo`] (action_run; one Ctrl+Z step)                       |
//!
//! **How the coordinator wires it.** `MenuEvent::Vault { paths, decrypt }` (the Encrypt… and Decrypt… items):
//! call `dialogs::vault::open(window, cx, paths, decrypt)`. It keeps the paths the mode takes (the menus have
//! checked what is on disk, a file or folder, and not a Recycle Bin item, archive member or online-only file
//! already, as `onDisk` did) and says so in a notification when none is left.

use crate::dialogs::action_run::{bad_output, remember_undo, where_they_are, Run, Where, WhereKind};
use crate::kit::prelude::*;
use crate::{core_bridge, explorer, t, theme};
use glint_core::actions::{Output, Progress, Report};
use glint_core::host::Manager as _;
use glint_core::vault::VaultTask;
use glint_model::format::{format_count, format_duration, plural};
use glint_model::paths::{base_name, parent_path};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// The calls into glint-core's vault. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// Encrypts or decrypts `files` (`vault_run`), reporting progress on `progress`.
    fn run(
        &self,
        files: Vec<String>,
        task: VaultTask,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report>;
    /// Stops the run under way (`vault_cancel`).
    fn cancel(&self);
}

/// glint-core's vault, owned by the main window as the shell dialogs it opens are.
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
    fn run(
        &self,
        files: Vec<String>,
        task: VaultTask,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        let Some(window) = self.app.get_webview_window(crate::workspace::MAIN) else {
            return Box::pin(async { Err(t!("Glint's window isn't open").to_string()) });
        };
        let joined = core_bridge::run(glint_core::vault::vault_run(window, files, task, output, progress));
        Box::pin(async move { joined.await.and_then(|inner| inner) })
    }

    fn cancel(&self) {
        glint_core::vault::vault_cancel();
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

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn run(&self, _: Vec<String>, _: VaultTask, _: Output, _: glint_core::host::ipc::Channel<Progress>) -> Fut<Report> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }

    fn cancel(&self) {}
}

/// Shows `view` in the window's dialog layer as [`super::show`] does, except that Enter does not close it: the
/// boxes in these dialogs answer Enter themselves (Save, Open, Encrypt…), and the dialog layer's own Enter
/// would close the dialog on top of that, even when the answer was "not yet" (two passwords that differ).
fn show<V: Render>(window: &mut Window, cx: &mut App, title: SharedString, width: Pixels, view: Entity<V>) {
    window.open_dialog(cx, move |dialog, _window, _cx| {
        dialog
            .title(title.clone())
            .w(width)
            .on_ok(|_, _, _| false)
            .child(view.clone())
    });
}

// ---- What can be locked or unlocked -------------------------------------------------------------------

/// Whether Encrypt takes `path` (`canEncrypt`): anything already on disk that is not an `.age` file. The menus
/// have checked that it is on disk (not in an archive, closed, online-only or in the Recycle Bin).
pub fn can_encrypt(path: &str) -> bool {
    !base_name(path).to_lowercase().ends_with(".age")
}

/// Whether `path` is an age file, to decrypt (`isAgeFile`). The menus have checked that it is a file on disk.
pub fn is_age_file(path: &str) -> bool {
    base_name(path).to_lowercase().ends_with(".age")
}

/// An age public key: `age1` and 58 lowercase letters and digits (the TypeScript's `/^age1[0-9a-z]{58}$/`).
pub fn is_age_key(key: &str) -> bool {
    key.strip_prefix("age1")
        .is_some_and(|rest| rest.len() == 58 && rest.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()))
}

/// The keys in `text`: split by spaces, new lines and commas, empty pieces dropped.
pub fn recipients(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
        .collect()
}

/// Which way to lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    /// A typed passphrase.
    Passphrase,
    /// Someone's age keys.
    Keys,
}

/// Encrypt or Decrypt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Lock files.
    Encrypt,
    /// Unlock `.age` files.
    Decrypt,
}

// ---- The dialog -------------------------------------------------------------------------------------------

/// "Encrypt …" or "Decrypt …" for some files: how to lock, where the results go, the run and its result.
pub struct VaultDialog {
    files: Vec<String>,
    mode: Mode,
    lock: Lock,
    kind: WhereKind,
    open_after: bool,
    run: Run,
    backend: Arc<dyn Backend>,
    password: Entity<InputState>,
    again: Entity<InputState>,
    keys: Entity<InputState>,
    suffix: Entity<InputState>,
    folder: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl VaultDialog {
    /// A dialog for `files` in `mode`, calling `backend`.
    pub fn new(
        files: Vec<String>,
        mode: Mode,
        backend: Arc<dyn Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let password = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(t!("Passphrase")));
        let again = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t!("The passphrase again"))
        });
        let keys = cx.new(|cx| InputState::new(window, cx).placeholder(t!("age1…")));
        let suffix = cx.new(|cx| InputState::new(window, cx).placeholder(t!("nothing")));
        let folder = cx.new(|cx| InputState::new(window, cx).default_value("Converted"));
        let mut subscriptions = Vec::new();
        for input in [&password, &again, &keys, &suffix, &folder] {
            subscriptions.push(
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.start(window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }),
            );
        }
        Self {
            files,
            mode,
            lock: Lock::Passphrase,
            kind: WhereKind::Beside,
            open_after: true,
            run: Run::Setup,
            backend,
            password,
            again,
            keys,
            suffix,
            folder,
            _subscriptions: subscriptions,
        }
    }

    /// The files being locked or unlocked.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// Where the run stands.
    pub fn run_state(&self) -> &Run {
        &self.run
    }

    /// The passphrase box, for focusing.
    pub fn password_input(&self) -> &Entity<InputState> {
        &self.password
    }

    /// The second passphrase box.
    pub fn again_input(&self) -> &Entity<InputState> {
        &self.again
    }

    /// The age keys box.
    pub fn keys_input(&self) -> &Entity<InputState> {
        &self.keys
    }

    /// The suffix box of "Next to each…".
    pub fn suffix_input(&self) -> &Entity<InputState> {
        &self.suffix
    }

    /// Where results go, from the choice and its boxes.
    pub fn output(&self, cx: &App) -> Where {
        match self.kind {
            WhereKind::Beside => Where::Beside {
                suffix: self.suffix.read(cx).value().to_string(),
            },
            WhereKind::Folder => Where::Folder {
                name: self.folder.read(cx).value().to_string(),
            },
            WhereKind::Replace => Where::Replace,
        }
    }

    fn keys_bad(&self, cx: &App) -> bool {
        if self.mode != Mode::Encrypt || self.lock != Lock::Keys {
            return false;
        }
        let all = recipients(&self.keys.read(cx).value());
        all.is_empty() || all.iter().any(|k| !is_age_key(k))
    }

    fn password_bad(&self, cx: &App) -> bool {
        let password = self.password.read(cx).value();
        match self.mode {
            Mode::Decrypt => password.is_empty(),
            Mode::Encrypt => {
                self.lock == Lock::Passphrase && (password.is_empty() || password != self.again.read(cx).value())
            }
        }
    }

    /// Whether Encrypt/Decrypt can be pressed: the lock is complete and the output usable.
    pub fn can_start(&self, cx: &App) -> bool {
        matches!(self.run, Run::Setup) && !self.password_bad(cx) && !self.keys_bad(cx) && !bad_output(&self.output(cx))
    }

    /// Starts the run on core's runtime; progress and the result come back here.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let password = self.password.read(cx).value().to_string();
        let task = match self.mode {
            Mode::Decrypt => VaultTask::Decrypt { password },
            Mode::Encrypt if self.lock == Lock::Passphrase => VaultTask::Encrypt {
                password,
                recipients: Vec::new(),
            },
            Mode::Encrypt => VaultTask::Encrypt {
                password: String::new(),
                recipients: recipients(&self.keys.read(cx).value()),
            },
        };
        let output = self.output(cx).output();
        let (channel, receiver) = core_bridge::stream::<Progress>();
        let call = self.backend.run(self.files.clone(), task, output, channel);
        self.run = Run::Running(Progress {
            done: 0,
            total: self.files.len(),
            current: String::new(),
            part: 0.,
        });
        cx.notify();
        let mode = self.mode;
        let open_after = self.open_after && self.files.len() == 1;
        // Progress: keep only the newest message each time the task wakes, so a chatty run redraws once.
        cx.spawn_in(window, {
            let receiver = receiver.clone();
            async move |this, cx| {
                while let Ok(mut progress) = receiver.recv().await {
                    while let Ok(newer) = receiver.try_recv() {
                        progress = newer;
                    }
                    let alive = this.update(cx, |this, cx| {
                        if matches!(this.run, Run::Running(_)) {
                            this.run = Run::Running(progress);
                            cx.notify();
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(report) => {
                        let what = if mode == Mode::Encrypt {
                            t!("encryption of {items}", items = plural(report.made.len(), "item"))
                        } else {
                            t!("decryption of {items}", items = plural(report.made.len(), "item"))
                        };
                        remember_undo(&report, &what, cx);
                        // A folder that was encrypted comes back as a .tar.zst: Glint opens it like a folder.
                        let open = (mode == Mode::Decrypt && open_after && report.made.len() == 1)
                            .then(|| report.made[0].clone());
                        this.run = Run::Done(report);
                        cx.notify();
                        if let Some(made) = open {
                            window.close_dialog(cx);
                            show_made(&made, cx);
                        }
                    }
                    Err(reason) => {
                        this.run = Run::Failed(reason.into());
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }

    fn close(window: &mut Window, cx: &mut App) {
        window.close_dialog(cx);
    }
}

/// Opens what a decryption made: a `.tar.zst` like a folder, anything else in its folder with it selected.
fn show_made(made: &str, cx: &mut App) {
    let Some(explorer) = explorer::try_explorer(cx) else {
        return;
    };
    let made = made.to_string();
    explorer.update(cx, |explorer, cx| {
        if made.to_lowercase().ends_with(".tar.zst") {
            explorer.navigate(made, explorer::NavOptions::default(), cx);
        } else {
            let options = explorer::NavOptions::default().selecting(made.clone());
            explorer.navigate(parent_path(&made), options, cx);
        }
    });
}

/// The folder to show for "Show": the first thing made, in its folder, selected.
fn reveal(path: &str, cx: &mut App) {
    let Some(explorer) = explorer::try_explorer(cx) else {
        return;
    };
    let path = path.to_string();
    explorer.update(cx, |explorer, cx| {
        let options = explorer::NavOptions::default().selecting(path.clone());
        explorer.navigate(parent_path(&path), options, cx);
    });
}

// ---- Drawing -------------------------------------------------------------------------------------------------

impl VaultDialog {
    /// A labelled row: a fixed-width label, then the box.
    fn field(&self, label: SharedString, input: &Entity<InputState>, aria: SharedString, t: &theme::Tokens) -> Div {
        align_row(3.)
            .child(
                div()
                    .w(space(24.))
                    .flex_none()
                    .text_size(theme::size::XS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.muted_foreground)
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(Input::new(input).aria_label(aria)))
    }

    /// Two or three choices of which one is picked, drawn as buttons with a name and a hint each.
    fn choices<T: Copy + PartialEq + 'static>(
        id: &'static str,
        name: SharedString,
        value: T,
        options: Vec<(T, SharedString, Option<SharedString>)>,
        on_pick: impl Fn(&mut Self, T, &mut Context<Self>) + Clone + 'static,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        row_start()
            .id(id)
            .role(Role::RadioGroup)
            .aria_label(name)
            .test_support()
            .gap(space(1.5))
            .children(options.into_iter().enumerate().map(|(ix, (option, label, hint))| {
                let chosen = option == value;
                let on_pick = on_pick.clone();
                Button::new((id, ix))
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
                    .flex_1()
                    .min_w_0()
                    .h_auto()
                    .border_1()
                    .border_color(if chosen { t.primary } else { t.border })
                    .px(space(2.))
                    .py(space(1.5))
                    .role(Role::RadioButton)
                    .toggled(chosen)
                    .accessibility_label(match &hint {
                        Some(hint) => format!("{label}. {hint}"),
                        None => label.to_string(),
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        on_pick(this, option, cx);
                        cx.notify();
                    }))
                    .child(
                        col()
                            .w_full()
                            .align_start()
                            .text_start()
                            .child(div().font_weight(FontWeight::MEDIUM).child(label))
                            .when_some(hint, |this, hint| {
                                this.child(
                                    div()
                                        .text_size(theme::size::XXS)
                                        .text_color(t.muted_foreground)
                                        .child(hint),
                                )
                            }),
                    )
            }))
    }

    /// "Where": beside, into a folder, or in their place.
    fn where_chooser(
        &self,
        noun: &str,
        note: SharedString,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bad = bad_output(&self.output(cx));
        let radio = |ix: usize, kind: WhereKind, label: SharedString, cx: &mut Context<Self>| {
            let chosen = self.kind == kind;
            Button::new(("vault-where", ix))
                .ghost()
                .compact()
                .role(Role::RadioButton)
                .toggled(chosen)
                .accessibility_label(label.clone())
                .icon(if chosen { IconName::CircleDot } else { IconName::Circle })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kind = kind;
                    cx.notify();
                }))
        };
        let small_box = |input: &Entity<InputState>, aria: SharedString, wide: f32, on: bool| {
            div()
                .w(space(wide))
                .flex_none()
                .when(on && bad, |d| {
                    d.border_1().border_color(t.destructive).rounded(theme::size::RADIUS)
                })
                .child(Input::new(input).aria_label(aria))
        };
        let beside = t!("Next to each {noun}, its name and", noun = noun);
        let folder = t!("Into a folder next to them:");
        let replace = t!("In their place, the {noun}s moved to the Recycle Bin", noun = noun);
        col()
            .id("vault-where")
            .role(Role::RadioGroup)
            .aria_label(t!("Where"))
            .test_support()
            .gap(space(1.5))
            .child(
                div()
                    .text_size(theme::size::XS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.muted_foreground)
                    .child(t!("Where")),
            )
            .child(
                align_row(2.)
                    .child(radio(0, WhereKind::Beside, beside.clone(), cx))
                    .child(div().flex_none().child(beside))
                    .child(small_box(
                        &self.suffix,
                        t!("Added to the name"),
                        28.,
                        self.kind == WhereKind::Beside,
                    )),
            )
            .child(
                align_row(2.)
                    .child(radio(1, WhereKind::Folder, folder.clone(), cx))
                    .child(div().flex_none().child(folder))
                    .child(small_box(
                        &self.folder,
                        t!("Folder name"),
                        36.,
                        self.kind == WhereKind::Folder,
                    )),
            )
            .child(
                align_row(2.)
                    .child(radio(2, WhereKind::Replace, replace.clone(), cx))
                    .child(div().flex_1().min_w_0().child(replace)),
            )
            .child(
                div()
                    .ps(6.)
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(note),
            )
    }

    /// What a run under way shows: which of how many, and how far the one being made is.
    fn progress_view(&self, progress: &Progress, t: &theme::Tokens) -> impl IntoElement {
        let verb = if self.mode == Mode::Encrypt {
            t!("Encrypting")
        } else {
            t!("Decrypting")
        };
        let mut text = verb.to_string();
        if progress.total > 1 {
            let at = (progress.done + 1).min(progress.total);
            text.push_str(&format!(" {} of {}", format_count(at), format_count(progress.total)));
        }
        if !progress.current.is_empty() {
            text.push_str(&format!(" · {}", progress.current));
        }
        if progress.part > 0. {
            text.push_str(&format!(" · {}%", (progress.part * 100.).round()));
        }
        let whole = if progress.total == 0 {
            0.
        } else {
            (progress.done as f32 + progress.part.min(0.999)) / progress.total as f32
        };
        let text: SharedString = text.into();
        col()
            .id("vault-progress")
            .role(Role::ProgressIndicator)
            .aria_label(text.clone())
            .test_support()
            .gap(space(2.))
            .py(space(6.))
            .child(
                align_row(2.)
                    .text_color(t.muted_foreground)
                    .child(Spinner::new())
                    .child(div().flex_1().min_w_0().truncate().child(isolate(text))),
            )
            .child(
                row_start()
                    .h(space(1.5))
                    .w_full()
                    .overflow_hidden()
                    .rounded_full()
                    .bg(t.muted)
                    .child(
                        div()
                            .h_full()
                            .w(gpui_kit::relative(whole.clamp(0., 1.)))
                            .rounded_full()
                            .bg(t.primary),
                    ),
            )
    }

    /// What a finished run did: made how many, how long it took, and what failed.
    fn result_view(&self, report: &Report, t: &theme::Tokens) -> impl IntoElement {
        let made = report.made.len();
        let mut headline = if made > 0 {
            t!("Made {what}", what = plural(made, "file")).to_string()
        } else {
            t!("Nothing was made").to_string()
        };
        if report.stopped {
            headline.push_str(&t!(" before it was stopped"));
        }
        let headline: SharedString = headline.into();
        let failed = report.failed.clone();
        col()
            .id("vault-result")
            .role(Role::Status)
            .aria_label(headline.clone())
            .test_support()
            .gap(space(3.))
            .child(
                align_row(2.)
                    .items_start()
                    .child(
                        icon(IconName::CircleCheck)
                            .size_5()
                            .flex_none()
                            .text_color(if made > 0 { t.success } else { t.muted_foreground }),
                    )
                    .child(
                        col()
                            .align_start()
                            .child(div().font_weight(FontWeight::MEDIUM).child(headline))
                            .when(made > 0, |this| {
                                this.child(
                                    div()
                                        .text_size(theme::size::XS)
                                        .text_color(t.muted_foreground)
                                        .child(format_duration(report.elapsed_ms)),
                                )
                            }),
                    ),
            )
            .when(!failed.is_empty(), |this| {
                let title = t!("{what} not done:", what = plural(failed.len(), "file"));
                this.child(
                    col()
                        .id("vault-failed")
                        .role(Role::List)
                        .aria_label(title.clone())
                        .test_support()
                        .gap(space(0.5))
                        .rounded(theme::size::RADIUS)
                        .bg(t.warning.opacity(0.1))
                        .px(space(3.))
                        .py(space(2.))
                        .text_size(theme::size::XS)
                        .child(div().font_weight(FontWeight::MEDIUM).child(title))
                        .children(failed.iter().take(6).enumerate().map(|(ix, (path, why))| {
                            let line: SharedString = format!("{}: {why}", base_name(path)).into();
                            div()
                                .id(("vault-failed", ix))
                                .role(Role::ListItem)
                                .aria_label(line.clone())
                                .test_support()
                                .truncate()
                                .child(isolate(line))
                        }))
                        .when(failed.len() > 6, |this| {
                            this.child(t!("and {n} more", n = format_count(failed.len() - 6)))
                        }),
                )
            })
    }

    /// What a run that failed as a whole shows.
    fn error_view(&self, reason: SharedString, t: &theme::Tokens) -> impl IntoElement {
        let what = if self.mode == Mode::Encrypt {
            t!("Couldn't encrypt")
        } else {
            t!("Couldn't decrypt")
        };
        col()
            .id("vault-error")
            .role(Role::Alert)
            .aria_label(format!("{what}. {reason}"))
            .test_support()
            .items_center()
            .gap(space(2.))
            .py(space(6.))
            .text_center()
            .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
            .child(div().font_weight(FontWeight::MEDIUM).child(what))
            .child(
                div()
                    .max_w(space(112.))
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(reason),
            )
    }
}

impl Render for VaultDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let encrypt = self.mode == Mode::Encrypt;

        // ---- The choices (before the run starts)
        let body = match self.run.clone() {
            Run::Setup if encrypt => {
                let password = self.password.read(cx).value();
                let again = self.again.read(cx).value();
                let short =
                    self.lock == Lock::Passphrase && !password.is_empty() && password.encode_utf16().count() < 10;
                let mismatch = !again.is_empty() && again != password;
                let keys_wrong = !self.keys.read(cx).value().is_empty() && self.keys_bad(cx);
                let lock = self.lock;
                col()
                    .gap(space(4.))
                    .child(div().text_color(t.muted_foreground).child(t!(
                        "Each becomes an .age file (a folder is packed first, as .tar.zst.age), readable on any system with age or rage. Without the passphrase or the key, nobody can open it: Glint neither."
                    )))
                    .child(Self::choices(
                        "vault-lock",
                        t!("Locked with"),
                        lock,
                        vec![
                            (Lock::Passphrase, t!("A passphrase"), Some(t!("Typed to open it"))),
                            (Lock::Keys, t!("Someone's age key"), Some(t!("Only they can open it"))),
                        ],
                        |this, lock, _| this.lock = lock,
                        &t,
                        cx,
                    ))
                    .child(if lock == Lock::Passphrase {
                        col()
                            .gap(space(2.))
                            .child(self.field(t!("Passphrase"), &self.password, t!("Passphrase"), &t))
                            .child(
                                self.field(t!("Again"), &self.again, t!("The passphrase again"), &t)
                                    .when(mismatch, |d| d.border_b_1().border_color(t.destructive)),
                            )
                            .when(mismatch, |this| {
                                this.child(
                                    div()
                                        .id("vault-mismatch")
                                        .role(Role::Alert)
                                        .aria_label(t!("The two aren't the same."))
                                        .test_support()
                                        .ps(27.)
                                        .text_size(theme::size::XS)
                                        .text_color(t.destructive)
                                        .child(t!("The two aren't the same.")),
                                )
                            })
                            .when(short, |this| {
                                this.child(
                                    div()
                                        .id("vault-short")
                                        .role(Role::Status)
                                        .aria_label(t!("A few words are much harder to guess than a short password."))
                                        .test_support()
                                        .ps(27.)
                                        .text_size(theme::size::XS)
                                        .text_color(t.warning)
                                        .child(t!("A few words are much harder to guess than a short password.")),
                                )
                            })
                            .into_any_element()
                    } else {
                        col()
                            .gap(space(1.))
                            .child(
                                div()
                                    .text_size(theme::size::XS)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.muted_foreground)
                                    .child(t!("Their age public keys, one a line (age1…)")),
                            )
                            .child(
                                div()
                                    .font_family(theme::mono_font(cx))
                                    .when(keys_wrong, |d| {
                                        d.border_1().border_color(t.destructive).rounded(theme::size::RADIUS)
                                    })
                                    .child(Input::new(&self.keys).aria_label(t!("Age keys"))),
                            )
                            .when(keys_wrong, |this| {
                                this.child(
                                    div()
                                        .id("vault-keys-bad")
                                        .role(Role::Alert)
                                        .aria_label(t!("Each key starts with age1 and is 62 characters long."))
                                        .test_support()
                                        .text_size(theme::size::XS)
                                        .text_color(t.destructive)
                                        .child(t!("Each key starts with age1 and is 62 characters long.")),
                                )
                            })
                            .into_any_element()
                    })
                    .child(self.where_chooser(
                        "item",
                        t!("In their place, the originals go to the Recycle Bin (empty it to be rid of them), and Ctrl+Z takes it back."),
                        &t,
                        cx,
                    ))
                    .into_any_element()
            }
            Run::Setup => {
                let single = self.files.len() == 1;
                col()
                    .gap(space(4.))
                    .child(self.field(t!("Passphrase"), &self.password, t!("Passphrase"), &t))
                    .when(single, |this| {
                        this.child(
                            div().ps(27.).child(
                                Checkbox::new("vault-open")
                                    .label(t!("Show it once it's decrypted"))
                                    .checked(self.open_after)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.open_after = *checked;
                                        cx.notify();
                                    })),
                            ),
                        )
                    })
                    .child(self.where_chooser(
                        "file",
                        t!("A name that's taken gets “(2)”, and Ctrl+Z takes it all back."),
                        &t,
                        cx,
                    ))
                    .into_any_element()
            }
            Run::Running(progress) => self.progress_view(&progress, &t).into_any_element(),
            Run::Done(report) => self.result_view(&report, &t).into_any_element(),
            Run::Failed(reason) => self.error_view(reason, &t).into_any_element(),
        };

        // ---- The buttons: Stop while it runs; Show and Close after; Cancel and the action before.
        let running = matches!(self.run, Run::Running(_));
        let finished = matches!(self.run, Run::Done(_) | Run::Failed(_));
        let first_made = match &self.run {
            Run::Done(report) => report.made.first().cloned(),
            _ => None,
        };
        let can_start = self.can_start(cx);
        let action_label = if encrypt { t!("Encrypt") } else { t!("Decrypt") };
        let action_icon = if encrypt { IconName::Lock } else { IconName::LockOpen };
        let footer = row_start()
            .justify_end()
            .gap(space(2.))
            .pt(space(3.))
            .border_t_1()
            .border_color(t.border)
            .when(running, |this| {
                this.child(
                    Button::new("vault-stop")
                        .outline()
                        .icon(IconName::Square)
                        .label(t!("Stop"))
                        .on_click(cx.listener(|this, _, _, _| this.stop())),
                )
            })
            .when_some(first_made, |this, made| {
                this.child(
                    Button::new("vault-show")
                        .outline()
                        .icon(IconName::FolderInput)
                        .label(t!("Show"))
                        .on_click(move |_, window, cx| {
                            Self::close(window, cx);
                            reveal(&made, cx);
                        }),
                )
            })
            .when(finished, |this| {
                this.child(
                    Button::new("vault-close")
                        .primary()
                        .label(t!("Close"))
                        .on_click(|_, window, cx| Self::close(window, cx)),
                )
            })
            .when(!finished, |this| {
                this.child(
                    Button::new("vault-cancel")
                        .outline()
                        .label(t!("Cancel"))
                        .disabled(running)
                        .on_click(|_, window, cx| Self::close(window, cx)),
                )
                .child(
                    Button::new("vault-start")
                        .primary()
                        .icon(action_icon)
                        .label(action_label.clone())
                        .disabled(running || !can_start)
                        .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                )
            });

        col()
            .id("vault")
            .role(Role::Group)
            .aria_label(title(&self.files, self.mode))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .child(body)
            .child(footer)
    }
}

/// The dialog's title: "Encrypt name" / "Decrypt 3 items", and the folder under it.
pub fn title(files: &[String], mode: Mode) -> SharedString {
    let what = match files {
        [one] => isolate(base_name(one).to_string()),
        _ => t!("{n} items", n = format_count(files.len())),
    };
    match mode {
        Mode::Encrypt => t!("Encrypt {what}", what = what),
        Mode::Decrypt => t!("Decrypt {what}", what = what),
    }
}

/// Opens Encrypt (or Decrypt, when `decrypt`) on the paths among `paths` it takes (`openVault`). When none is
/// left it says so in a notification instead.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>, decrypt: bool) {
    let mode = if decrypt { Mode::Decrypt } else { Mode::Encrypt };
    let files: Vec<String> = paths
        .into_iter()
        .filter(|p| if decrypt { is_age_file(p) } else { can_encrypt(p) })
        .collect();
    if files.is_empty() {
        let message = if decrypt {
            t!("No .age files chosen")
        } else {
            t!("Nothing chosen to encrypt")
        };
        window.push_notification(Notification::info(message), cx);
        return;
    }
    let backend = backend(cx);
    open_with(window, cx, files, mode, backend);
}

/// [`open`] for a given backend, without filtering `files` (tests pass a fake).
pub fn open_with(window: &mut Window, cx: &mut App, files: Vec<String>, mode: Mode, backend: Arc<dyn Backend>) {
    let shown = title(&files, mode);
    let folder = where_they_are(&files);
    let view = cx.new(|cx| VaultDialog::new(files, mode, backend, window, cx));
    let input = view.read(cx).password_input().clone();
    let view = cx.new(|_| Titled { folder, inner: view });
    show(window, cx, shown, px(576.), view);
    input.update(cx, |input, cx| input.focus(window, cx));
}

/// The dialog with the folder its files are in under the title (`ModalHeader`'s subtitle).
struct Titled {
    folder: SharedString,
    inner: Entity<VaultDialog>,
}

impl Render for Titled {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        col()
            .gap(space(2.))
            .text_start()
            .child(
                div()
                    .id("vault-where-they-are")
                    .role(Role::Group)
                    .aria_label(self.folder.clone())
                    .test_support()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .truncate()
                    .child(isolate(self.folder.clone())),
            )
            .child(self.inner.clone())
    }
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use glint_core::actions::Recycled;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    /// What the fake was asked to run.
    type RunCall = (Vec<String>, VaultTask, Output);

    /// A backend that answers with `report` (or `error`) after sending one progress message.
    struct Fake {
        report: Report,
        error: Option<String>,
        calls: Mutex<Vec<RunCall>>,
        cancelled: Mutex<usize>,
    }

    impl Fake {
        fn new(report: Report) -> Arc<Self> {
            Arc::new(Self {
                report,
                error: None,
                calls: Mutex::new(Vec::new()),
                cancelled: Mutex::new(0),
            })
        }
    }

    impl Backend for Fake {
        fn run(
            &self,
            files: Vec<String>,
            task: VaultTask,
            output: Output,
            progress: glint_core::host::ipc::Channel<Progress>,
        ) -> Fut<Report> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push((files.clone(), task, output));
            }
            let _ = progress.send(Progress {
                done: 0,
                total: files.len(),
                current: "tax.pdf".into(),
                part: 0.5,
            });
            let report = self.report.clone();
            let error = self.error.clone();
            Box::pin(async move {
                match error {
                    Some(error) => Err(error),
                    None => Ok(report),
                }
            })
        }

        fn cancel(&self) {
            if let Ok(mut n) = self.cancelled.lock() {
                *n += 1;
            }
        }
    }

    fn report(made: &[&str]) -> Report {
        Report {
            made: made.iter().map(|s| (*s).to_string()).collect(),
            elapsed_ms: 1234.,
            ..Default::default()
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the vault dialog never reached the state the test waits for");
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

    /// Whether the button is disabled (an enabled one says nothing at all).
    fn disabled(cx: &mut VisualTestContext, id: &'static str) -> Option<bool> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            Some(window.find(id).disabled() == Some(true))
        })
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

    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    /// Opens the dialog and gives its view, to reach the boxes.
    fn open_view(cx: &mut VisualTestContext, files: &[&str], mode: Mode, fake: &Arc<Fake>) -> Entity<VaultDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let files = strings(files);
        let view = cx.update(|window, cx| {
            let view = cx.new(|cx| VaultDialog::new(files.clone(), mode, backend, window, cx));
            show(window, cx, title(&files, mode), px(576.), view.clone());
            view
        });
        cx.run_until_parked();
        view
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Lock,
            IconName::LockOpen,
            IconName::Square,
            IconName::FolderInput,
            IconName::CircleCheck,
            IconName::CircleDot,
            IconName::Circle,
            IconName::TriangleAlert,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn what_each_mode_takes_keys_and_outputs_read_as_the_typescript_did() {
        assert!(can_encrypt("C:\\a\\notes.txt") && can_encrypt("C:\\a\\Folder"));
        assert!(!can_encrypt("C:\\a\\secret.AGE") && is_age_file("C:\\a\\secret.AGE"));
        assert!(!is_age_file("C:\\a\\notes.txt"));
        let key = format!("age1{}", "q".repeat(58));
        assert!(is_age_key(&key));
        assert!(!is_age_key(&format!("age1{}", "q".repeat(57))));
        assert!(!is_age_key(&format!("age1{}", "Q".repeat(58))));
        assert!(!is_age_key(&format!("age2{}", "q".repeat(58))));
        assert_eq!(recipients(&format!(" {key},\n{key}  ")), [key.clone(), key]);
        assert!(recipients(" , \n").is_empty());
        // badOutput: characters names can't hold, a trailing dot or space, an empty folder name.
        assert!(!bad_output(&Where::Beside { suffix: String::new() }));
        assert!(!bad_output(&Where::Beside {
            suffix: "-locked".into()
        }));
        assert!(bad_output(&Where::Beside { suffix: "a/b".into() }));
        assert!(bad_output(&Where::Beside { suffix: "x.".into() }));
        assert!(bad_output(&Where::Folder { name: "  ".into() }));
        assert!(bad_output(&Where::Folder { name: "a?b".into() }));
        assert!(!bad_output(&Where::Folder {
            name: "Converted".into()
        }));
        assert!(!bad_output(&Where::Replace));
        assert_eq!(Where::Replace.output(), Output::Replace);
        assert_eq!(
            Where::Folder { name: "x".into() }.output(),
            Output::Folder { name: "x".into() }
        );
    }

    #[test]
    fn titles_and_places_follow_the_typescript() {
        let _guard = testing::lock();
        assert_eq!(
            title(&strings(&["C:\\a\\notes.txt"]), Mode::Encrypt),
            "Encrypt notes.txt"
        );
        assert_eq!(
            title(&strings(&["C:\\a\\x", "C:\\b\\y"]), Mode::Decrypt),
            "Decrypt 2 items"
        );
        assert_eq!(where_they_are(&strings(&["C:\\a\\x", "C:\\A\\y"])), "C:\\a");
        assert_eq!(where_they_are(&strings(&["C:\\a\\x", "C:\\b\\y"])), "In 2 folders");
    }

    #[gpui_kit::test]
    fn encrypt_waits_for_two_matching_passphrases_then_runs_and_reports(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report(&["C:\\a\\tax.pdf.age"]));
        let view = open_view(&mut visual, &["C:\\a\\tax.pdf"], Mode::Encrypt, &fake);
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert_eq!(role_of(&mut visual, "Encrypt tax.pdf"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Locked with"), Some(Role::RadioGroup));
        assert_eq!(role_of(&mut visual, "Where"), Some(Role::RadioGroup));
        // Nothing typed: Encrypt waits.
        assert_eq!(disabled(&mut visual, "vault-start"), Some(true));
        let (first, second) = view.read_with(&visual, |v, _| (v.password_input().clone(), v.again_input().clone()));
        type_into(&mut visual, &first, "short");
        assert!(has(
            &mut visual,
            "A few words are much harder to guess than a short password."
        ));
        type_into(&mut visual, &second, "shor");
        assert_eq!(role_of(&mut visual, "The two aren't the same."), Some(Role::Alert));
        assert_eq!(disabled(&mut visual, "vault-start"), Some(true));
        type_into(&mut visual, &first, "correct horse battery staple");
        type_into(&mut visual, &second, "correct horse battery staple");
        assert!(!has(
            &mut visual,
            "A few words are much harder to guess than a short password."
        ));
        assert_eq!(disabled(&mut visual, "vault-start"), Some(false));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("vault-start", cx);
        });
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(role_of(&mut visual, "Made 1 file"), Some(Role::Status));
        let calls = fake.calls.lock().map(|c| c.clone()).unwrap_or_default();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, ["C:\\a\\tax.pdf"]);
        match &calls[0].1 {
            VaultTask::Encrypt { password, recipients } => {
                assert_eq!(password, "correct horse battery staple");
                assert!(recipients.is_empty());
            }
            other => panic!("expected an encryption, got {other:?}"),
        }
        assert_eq!(calls[0].2, Output::Beside { suffix: String::new() });
        // After a run: Show and Close, no Cancel/Encrypt.
        assert!(
            has(&mut visual, "Show")
                || visual.update(|w, cx| {
                    w.render_frame(cx);
                    w.try_find("vault-show").is_some()
                })
        );
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("vault-start").is_none());
            window.click("vault-close", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn locking_to_keys_needs_valid_keys_and_sends_them_without_a_passphrase(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report(&["C:\\a\\x.age", "C:\\a\\y.age"]));
        let view = open_view(&mut visual, &["C:\\a\\x", "C:\\a\\y"], Mode::Encrypt, &fake);
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("vault-lock", 1_usize), cx);
        });
        visual.run_until_parked();
        assert_eq!(disabled(&mut visual, "vault-start"), Some(true));
        let keys = view.read_with(&visual, |v, _| v.keys_input().clone());
        type_into(&mut visual, &keys, "not a key");
        assert_eq!(
            role_of(&mut visual, "Each key starts with age1 and is 62 characters long."),
            Some(Role::Alert)
        );
        assert_eq!(disabled(&mut visual, "vault-start"), Some(true));
        let key = format!("age1{}", "q".repeat(58));
        type_into(&mut visual, &keys, &key);
        assert_eq!(disabled(&mut visual, "vault-start"), Some(false));
        // Into a folder instead of beside.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("vault-where", 1_usize), cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("vault-start", cx);
        });
        settle(&mut visual, |cx| has(cx, "Made 2 files"));
        let calls = fake.calls.lock().map(|c| c.clone()).unwrap_or_default();
        match &calls[0].1 {
            VaultTask::Encrypt { password, recipients } => {
                assert!(password.is_empty(), "keys lock without a passphrase");
                assert_eq!(recipients, &[key]);
            }
            other => panic!("expected an encryption, got {other:?}"),
        }
        assert_eq!(
            calls[0].2,
            Output::Folder {
                name: "Converted".into()
            }
        );
    }

    #[gpui_kit::test]
    fn decrypt_takes_one_passphrase_and_an_unusable_suffix_blocks_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report(&["C:\\a\\tax.pdf"]));
        let view = open_view(&mut visual, &["C:\\a\\tax.pdf.age"], Mode::Decrypt, &fake);
        assert_eq!(role_of(&mut visual, "Decrypt tax.pdf.age"), Some(Role::Group));
        assert_eq!(disabled(&mut visual, "vault-start"), Some(true));
        let password = view.read_with(&visual, |v, _| v.password_input().clone());
        let suffix = view.read_with(&visual, |v, _| v.suffix_input().clone());
        type_into(&mut visual, &password, "pass phrase");
        assert_eq!(disabled(&mut visual, "vault-start"), Some(false));
        type_into(&mut visual, &suffix, "a:b");
        assert_eq!(disabled(&mut visual, "vault-start"), Some(true));
        type_into(&mut visual, &suffix, "-open");
        assert_eq!(disabled(&mut visual, "vault-start"), Some(false));
        // Enter in the passphrase box starts it.
        visual.update(|window, cx| {
            password.update(cx, |input, cx| input.focus(window, cx));
            window.press("enter", cx);
        });
        settle(&mut visual, |_| {
            !fake.calls.lock().map(|c| c.is_empty()).unwrap_or(true)
        });
        let calls = fake.calls.lock().map(|c| c.clone()).unwrap_or_default();
        assert!(matches!(&calls[0].1, VaultTask::Decrypt { password } if password == "pass phrase"));
        // With a single file and "Show it" on, the dialog closes by itself to show the result.
        settle(&mut visual, |cx| cx.update(|window, cx| !window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn a_failed_run_says_why_and_failed_files_are_listed(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let mut value = Fake::new(Report::default());
        if let Some(inner) = Arc::get_mut(&mut value) {
            inner.error = Some("Wrong passphrase".into());
        }
        let fake = value;
        let view = open_view(&mut visual, &["C:\\a\\x.age"], Mode::Decrypt, &fake);
        let password = view.read_with(&visual, |v, _| v.password_input().clone());
        type_into(&mut visual, &password, "nope");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("vault-start", cx);
        });
        settle(&mut visual, |cx| has(cx, "Couldn't decrypt. Wrong passphrase"));
        assert_eq!(
            role_of(&mut visual, "Couldn't decrypt. Wrong passphrase"),
            Some(Role::Alert)
        );

        // A run that did some and failed some, with an Arabic name.
        let mut partial = report(&["C:\\a\\one.age"]);
        partial.failed = (0..8)
            .map(|i| (format!("C:\\a\\ملف{i}.txt"), "Couldn't read it".to_string()))
            .collect();
        partial.stopped = true;
        let fake = Fake::new(partial);
        testing::set_rtl(true);
        let view = open_view(&mut visual, &["C:\\a\\ملف0.txt", "C:\\a\\two"], Mode::Encrypt, &fake);
        let (first, second) = view.read_with(&visual, |v, _| (v.password_input().clone(), v.again_input().clone()));
        type_into(&mut visual, &first, "pass phrase here");
        type_into(&mut visual, &second, "pass phrase here");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("vault-start", cx);
        });
        settle(&mut visual, |cx| has(cx, "Made 1 file before it was stopped"));
        assert_eq!(role_of(&mut visual, "8 files not done:"), Some(Role::List));
        assert_eq!(role_of(&mut visual, "ملف0.txt: Couldn't read it"), Some(Role::ListItem));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find_all_by_role(Role::ListItem).len(),
                6,
                "six shown, the rest counted"
            );
        });
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn progress_shows_while_it_runs_stop_cancels_and_escape_closes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report(&[]));
        let view = open_view(&mut visual, &["C:\\a\\x.age"], Mode::Decrypt, &fake);
        // Put the dialog in a run by hand: a backend that never answers is a fake that waits; the view's state
        // is what is drawn, so set it directly.
        visual.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.run = Run::Running(Progress {
                    done: 1,
                    total: 3,
                    current: "tax.pdf".into(),
                    part: 0.5,
                });
                cx.notify();
            })
        });
        assert_eq!(
            role_of(&mut visual, "Decrypting 2 of 3 · tax.pdf · 50%"),
            Some(Role::ProgressIndicator)
        );
        assert_eq!(disabled(&mut visual, "vault-cancel"), Some(true));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("vault-stop", cx);
        });
        assert_eq!(fake.cancelled.lock().map(|n| *n).unwrap_or(0), 1);
        // Back to setup, and Escape closes it.
        visual.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.run = Run::Setup;
                cx.notify();
            })
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn opening_with_nothing_it_takes_says_so_and_what_it_takes_is_kept(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report(&[]));
        let backend: Arc<dyn Backend> = fake.clone();
        visual.update(|_, cx| init_with(cx, backend));
        visual.update(|window, cx| open(window, cx, strings(&["C:\\a\\x.txt"]), true));
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "no .age files, no dialog"));
        visual.update(|window, cx| open(window, cx, strings(&["C:\\a\\x.age", "C:\\a\\y.txt"]), false));
        visual.run_until_parked();
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
        // Only the file it takes is there, and its folder is named under the title.
        assert!(has(&mut visual, r"C:\a"));
        let _ = Recycled {
            path: String::new(),
            bin: String::new(),
        };
    }
}
