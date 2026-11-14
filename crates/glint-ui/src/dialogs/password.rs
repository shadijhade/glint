//! Passwords asked for or set: an archive's, and the one to compress with.
//! Ported from src/components/explorer/PasswordDialog.tsx and the stores it uses in src/lib/archives.ts.
//!
//! | TypeScript                                  | Here                                                           |
//! |---------------------------------------------|----------------------------------------------------------------|
//! | `PasswordForm`                              | [`PasswordForm`], a view the previews embed (and the dialog)   |
//! | `usePasswordAsk`, `askArchivePassword`      | [`ask_archive_password`], [`ask_for_notice`]                   |
//! | `PasswordDialog`                            | [`PasswordDialog`] (a form with its heading)                   |
//! | `useCompressAsk` + `CompressPasswordDialog` | [`ask_compress_password`], [`CompressPasswordDialog`]          |
//! | `useUnlocks().count`                        | [`Unlocks::count`] of [`unlocks`], and its [`Unlocked`] event  |
//! | `archiveUnlock(path, password)`             | [`Backend::unlock`] (glint-core `archive_unlock`)              |
//!
//! **How the coordinator wires it.**
//! - `Notice::ArchivePassword { path, tab, select, .. }` (the explorer asks for an archive's password): call
//!   `dialogs::password::ask_for_notice(window, cx, path, tab, select)`. When the password opens the archive,
//!   it navigates to `path` again in `tab`, selecting `select`, as `askArchivePassword(path, retry)` did.
//! - "Compress to > With a password…" (`MenuEvent::CompressWithPassword { paths }`, after the secrets check):
//!   call `dialogs::password::ask_compress_password(window, cx, what, Format::Zip, run)`; `run` gets the
//!   format and the password, and starts the compression (`compressEntries(.., format, password)`).
//! - Previews of an encrypted archive or file embed the form: `cx.new(|cx| PasswordForm::new(path, window, cx))`,
//!   and read again when [`Unlocked`] is emitted by [`unlocks`] (`useUnlocks`).
//!
//! The password is kept in memory by glint-core until Glint closes (that is core's doing); the compress password
//! is never kept: it lives in the dialog's two boxes, which go with the dialog.

use crate::kit::prelude::*;
use crate::{core_bridge, explorer, t, theme};
use glint_model::paths::base_name;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// The call into glint-core. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// Tries `password` on what `path` leads to in an archive (`archive_unlock`): kept in memory if it opens it.
    fn unlock(&self, path: String, password: String) -> Fut<()>;
}

/// glint-core's archive passwords.
pub struct CoreBackend;

impl Backend for CoreBackend {
    fn unlock(&self, path: String, password: String) -> Fut<()> {
        Box::pin(glint_core::archive::archive_unlock(path, password))
    }
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

// ---- What the app shares: the unlock count ----------------------------------------------------------------

/// Said by [`unlocks`] when a password opened something: what failed for want of one is read again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unlocked;

/// Counts the passwords that opened something (`useUnlocks`), and holds the call into core.
pub struct Unlocks {
    backend: Arc<dyn Backend>,
    count: usize,
}

impl EventEmitter<Unlocked> for Unlocks {}

impl Unlocks {
    /// How many passwords have opened something.
    pub fn count(&self) -> usize {
        self.count
    }
}

struct SharedUnlocks(Entity<Unlocks>);

impl Global for SharedUnlocks {}

/// Makes the shared model with `backend` (tests pass a fake). [`unlocks`] makes one on core if this was not called.
pub fn init_with(cx: &mut App, backend: Arc<dyn Backend>) {
    let unlocks = cx.new(|_| Unlocks { backend, count: 0 });
    cx.set_global(SharedUnlocks(unlocks));
}

/// The shared model.
pub fn unlocks(cx: &mut App) -> Entity<Unlocks> {
    if let Some(shared) = cx.try_global::<SharedUnlocks>() {
        return shared.0.clone();
    }
    init_with(cx, Arc::new(CoreBackend));
    cx.global::<SharedUnlocks>().0.clone()
}

// ---- The form ---------------------------------------------------------------------------------------------

/// What runs once a password opened what it was for.
type OnUnlocked = Rc<dyn Fn(&mut Window, &mut App)>;

/// The password for what `path` leads to in an archive (a file in it, or an archive whose list of files is
/// encrypted), tried as it is given: kept in memory until Glint closes once it opens it. By default the folder
/// shown is read again after, without the locks (`PasswordForm`).
pub struct PasswordForm {
    path: String,
    input: Entity<InputState>,
    unlocks: Entity<Unlocks>,
    busy: bool,
    error: Option<SharedString>,
    on_unlocked: OnUnlocked,
    _subscriptions: Vec<Subscription>,
}

impl PasswordForm {
    /// A form for `path` that reads the folder on screen again once it unlocks.
    pub fn new(path: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_callback(path, reload_active_tab, window, cx)
    }

    /// A form for `path` that runs `on_unlocked` once it unlocks (`onUnlocked`).
    pub fn with_callback(
        path: impl Into<String>,
        on_unlocked: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(t!("Password")));
        let on_input = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.submit(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        let unlocks = unlocks(cx);
        Self {
            path: path.into(),
            input,
            unlocks,
            busy: false,
            error: None,
            on_unlocked: Rc::new(on_unlocked),
            _subscriptions: vec![on_input],
        }
    }

    /// The password box, so the owner can focus it (`autoFocus`).
    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    /// Tries the password.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let password = self.input.read(cx).value().to_string();
        if password.is_empty() || self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let call = self.unlocks.read(cx).backend.unlock(self.path.clone(), password);
        cx.spawn_in(window, async move |this, cx| {
            let result = core_bridge::run(call).await.and_then(|inner| inner);
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    this.unlocks.update(cx, |unlocks, cx| {
                        unlocks.count += 1;
                        cx.emit(Unlocked);
                        cx.notify();
                    });
                    // Like the TypeScript, the form stays busy: what it was for goes away or reloads.
                    let on_unlocked = this.on_unlocked.clone();
                    on_unlocked(window, cx);
                }
                Err(reason) => {
                    this.error = Some(reason.into());
                    this.busy = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// Reads the folder on screen again (`reload()`), the form's default once it unlocks.
fn reload_active_tab(_window: &mut Window, cx: &mut App) {
    if let Some(explorer) = explorer::try_explorer(cx) {
        explorer.update(cx, |explorer, cx| explorer.reload(None, cx).detach());
    }
}

impl Render for PasswordForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let empty = self.input.read(cx).value().is_empty();
        col()
            .id("password-form")
            .w_full()
            .max_w(space(72.))
            .gap(space(1.5))
            .text_size(theme::size::SM)
            .text_start()
            .child(
                align_row(1.5)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.input).aria_label(t!("Password"))),
                    )
                    .child(
                        Button::new("password-open")
                            .primary()
                            .label(t!("Open"))
                            .when(!self.busy, |b| b.icon(IconName::KeyRound))
                            .loading(self.busy)
                            .disabled(empty || self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("password-error")
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.destructive)
                        .child(error),
                )
            })
    }
}

// ---- The password of an archive, asked for -------------------------------------------------------------

/// "“name” is password-protected": a form for `path`, then `retry` once it opens (`PasswordDialog`).
pub struct PasswordDialog {
    title: SharedString,
    form: Entity<PasswordForm>,
}

impl PasswordDialog {
    /// The form, for focusing.
    pub fn form(&self) -> &Entity<PasswordForm> {
        &self.form
    }
}

impl Render for PasswordDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        col()
            .id("password-dialog")
            .role(Role::Group)
            .aria_label(self.title.clone())
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            .text_start()
            .pt(space(1.))
            .child(
                div()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!("Its password stays in memory until Glint closes.")),
            )
            .child(self.form.clone())
    }
}

/// Asks for the password of what `path` leads to in an archive, then runs `retry` (`askArchivePassword`).
pub fn ask_archive_password(
    window: &mut Window,
    cx: &mut App,
    path: String,
    retry: impl Fn(&mut Window, &mut App) + 'static,
) {
    let name = base_name(&path).to_string();
    let form = cx.new(|cx| {
        PasswordForm::with_callback(
            path,
            move |window, cx| {
                window.close_dialog(cx);
                retry(window, cx);
            },
            window,
            cx,
        )
    });
    let input = form.read(cx).input().clone();
    let title = t!("“{name}” is password-protected", name = isolate(name));
    let view = cx.new(|_| PasswordDialog {
        title: title.clone(),
        form,
    });
    show(window, cx, title, px(384.), view);
    input.update(cx, |input, cx| input.focus(window, cx));
}

/// Answers `Notice::ArchivePassword`: asks for the password of `path`, then opens it again in `tab`,
/// selecting `select` (what `navigate` was doing when it failed).
pub fn ask_for_notice(
    window: &mut Window,
    cx: &mut App,
    path: String,
    tab: explorer::TabId,
    select: Option<Vec<String>>,
) {
    let target = path.clone();
    ask_archive_password(window, cx, path, move |_, cx| {
        if let Some(explorer) = explorer::try_explorer(cx) {
            let options = explorer::NavOptions {
                select: select.clone(),
                tab: Some(tab),
                ..Default::default()
            };
            explorer.update(cx, |explorer, cx| explorer.navigate(target.clone(), options, cx));
        }
    });
}

// ---- Compress with a password -----------------------------------------------------------------------------

/// What the archive is: AES-256 in a zip (its file names show) or a 7z (its file names are encrypted too).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// A .zip: the names of its files can be read without the password.
    Zip,
    /// A .7z: its names are encrypted too.
    SevenZ,
}

impl Format {
    /// core's name for it (`"zip"`, `"7z"`).
    pub fn code(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::SevenZ => "7z",
        }
    }

    fn label(self) -> SharedString {
        match self {
            Format::SevenZ => t!("7z (names hidden too)"),
            Format::Zip => t!("zip (names show)"),
        }
    }
}

/// What runs when Compress is pressed: the format and the password.
type RunCompress = Rc<dyn Fn(Format, String, &mut Window, &mut App)>;

/// Compress with a password: the format, the password asked twice, and never kept (`CompressPasswordDialog`).
pub struct CompressPasswordDialog {
    format: Format,
    password: Entity<InputState>,
    again: Entity<InputState>,
    run: RunCompress,
    _subscriptions: Vec<Subscription>,
}

impl CompressPasswordDialog {
    /// A dialog that calls `run(format, password, ..)` on Compress, starting with `format` chosen.
    pub fn new(
        format: Format,
        run: impl Fn(Format, String, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let password = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(t!("Password")));
        let again = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t!("The password again"))
        });
        let mut subscriptions = Vec::new();
        for input in [&password, &again] {
            subscriptions.push(
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }),
            );
        }
        Self {
            format,
            password,
            again,
            run: Rc::new(run),
            _subscriptions: subscriptions,
        }
    }

    /// The first box, for focusing.
    pub fn password_input(&self) -> &Entity<InputState> {
        &self.password
    }

    /// The second box.
    #[cfg(test)]
    pub fn again_input(&self) -> &Entity<InputState> {
        &self.again
    }

    /// The chosen format.
    pub fn format(&self) -> Format {
        self.format
    }

    /// Both boxes hold the same, non-empty password.
    fn ready(&self, cx: &App) -> bool {
        let password = self.password.read(cx).value();
        !password.is_empty() && password == self.again.read(cx).value()
    }

    /// Compresses, if the two passwords are the same.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        let password = self.password.read(cx).value().to_string();
        let format = self.format;
        let run = self.run.clone();
        window.close_dialog(cx);
        run(format, password, window, cx);
    }
}

impl Render for CompressPasswordDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let again = self.again.read(cx).value();
        let mismatch = !again.is_empty() && again != self.password.read(cx).value();
        let ready = self.ready(cx);
        col()
            .id("compress-password")
            .role(Role::Group)
            .aria_label(t!("Compress with a password"))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            .text_start()
            .pt(space(1.))
            .child(
                div()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!(
                        "AES-256. Glint doesn't keep the password: without it, nothing in the archive can be read."
                    )),
            )
            .child(
                align_row(1.5)
                    .id("compress-format")
                    .role(Role::RadioGroup)
                    .aria_label(t!("Format"))
                    .test_support()
                    .children([Format::SevenZ, Format::Zip].into_iter().map(|format| {
                        let chosen = self.format == format;
                        Button::new(("compress-format", format as usize))
                            .small()
                            .when(chosen, |b| b.primary())
                            .when(!chosen, |b| b.outline())
                            .role(Role::RadioButton)
                            .toggled(chosen)
                            .accessibility_label(format.label())
                            .label(format.label())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.format = format;
                                cx.notify();
                            }))
                    })),
            )
            .child(Input::new(&self.password).aria_label(t!("Password")))
            .child(Input::new(&self.again).aria_label(t!("The password again")))
            .when(mismatch, |this| {
                this.child(
                    div()
                        .id("compress-mismatch")
                        .role(Role::Alert)
                        .aria_label(t!("The two aren't the same."))
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.destructive)
                        .child(t!("The two aren't the same.")),
                )
            })
            .child(
                row_start()
                    .justify_end()
                    .gap(space(2.))
                    .child(
                        Button::new("compress-cancel")
                            .ghost()
                            .label(t!("Cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("compress-go")
                            .primary()
                            .label(t!("Compress"))
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

/// Asks for a password to compress `what` ("3 items", "“report.docx”") with, then calls `run(format, password)`
/// (`useCompressAsk`). `format` is the one chosen to start with.
pub fn ask_compress_password(
    window: &mut Window,
    cx: &mut App,
    what: impl Into<SharedString>,
    format: Format,
    run: impl Fn(Format, String, &mut Window, &mut App) + 'static,
) {
    let view = cx.new(|cx| CompressPasswordDialog::new(format, run, window, cx));
    let input = view.read(cx).password_input().clone();
    let title = t!("Compress {what} with a password", what = isolate(what));
    show(window, cx, title, px(384.), view);
    input.update(cx, |input, cx| input.focus(window, cx));
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::sync::Mutex;
    use std::time::Duration;

    /// A backend that opens with one password and records what it was asked.
    struct Fake {
        right: &'static str,
        asked: Mutex<Vec<(String, String)>>,
    }

    impl Backend for Fake {
        fn unlock(&self, path: String, password: String) -> Fut<()> {
            if let Ok(mut asked) = self.asked.lock() {
                asked.push((path, password.clone()));
            }
            let right = self.right;
            Box::pin(async move {
                if password == right {
                    Ok(())
                } else {
                    Err("The password doesn't open it".to_string())
                }
            })
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
        panic!("the password dialog never reached the state the test waits for");
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

    fn window(cx: &mut TestAppContext, right: &'static str) -> (VisualTestContext, Arc<Fake>) {
        cx.update(testing::init);
        let fake = Arc::new(Fake {
            right,
            asked: Mutex::new(Vec::new()),
        });
        let backend: Arc<dyn Backend> = fake.clone();
        cx.update(|cx| init_with(cx, backend));
        (testing::open_window(cx, |_, cx| cx.new(|_| Host)).0, fake)
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        let path = IconName::KeyRound.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }

    #[gpui_kit::test]
    fn the_dialog_asks_tries_and_says_when_the_password_is_wrong(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = window(cx, "s3cret");
        let retried = Rc::new(RefCell::new(0));
        let counted = retried.clone();
        visual.update(|window, cx| {
            ask_archive_password(window, cx, "C:\\a\\locked.7z".into(), move |_, _| {
                *counted.borrow_mut() += 1
            })
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert_eq!(
            role_of(&mut visual, "“locked.7z” is password-protected"),
            Some(Role::Group)
        );
        // The box is named, and Open waits for something typed.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("password-open").disabled(), Some(true));
        });
        // Find the form's box through the shared model's last use: type via the focused input.
        visual.update(|window, cx| {
            window.press("ctrl-a", cx);
            window.input("wrong", cx);
            window.press("enter", cx);
        });
        settle(&mut visual, |cx| has(cx, "The password doesn't open it"));
        assert_eq!(role_of(&mut visual, "The password doesn't open it"), Some(Role::Alert));
        visual.update(|window, cx| assert!(window.has_active_dialog(cx), "a wrong password keeps the dialog"));
        visual.update(|window, cx| {
            window.press("ctrl-a", cx);
            window.input("s3cret", cx);
            window.press("enter", cx);
        });
        settle(&mut visual, |_| *retried.borrow() == 1);
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "the right one closes it"));
        let asked = fake.asked.lock().map(|a| a.clone()).unwrap_or_default();
        assert_eq!(
            asked,
            vec![
                ("C:\\a\\locked.7z".to_string(), "wrong".to_string()),
                ("C:\\a\\locked.7z".to_string(), "s3cret".to_string())
            ]
        );
        // The count of passwords that opened something went up once, for whoever reads again.
        let unlocks = visual.update(|_, cx| unlocks(cx));
        assert_eq!(unlocks.read_with(&visual, |u, _| u.count()), 1);
    }

    #[gpui_kit::test]
    fn the_form_alone_waits_for_a_password_and_calls_back_once(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let done = Rc::new(RefCell::new(0));
        let counted = done.clone();
        cx.update(testing::init);
        let fake = Arc::new(Fake {
            right: "pw",
            asked: Mutex::new(Vec::new()),
        });
        let backend: Arc<dyn Backend> = fake;
        cx.update(|cx| init_with(cx, backend));
        // The form is the window's content, so typing reaches its box.
        let (mut visual, form) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| PasswordForm::with_callback(r"C:\x.zip", move |_, _| *counted.borrow_mut() += 1, window, cx))
        });
        let input = form.read_with(&visual, |f, _| f.input().clone());
        // Submitting nothing does nothing.
        visual.update(|window, cx| form.update(cx, |f, cx| f.submit(window, cx)));
        visual.run_until_parked();
        assert_eq!(*done.borrow(), 0);
        type_into(&mut visual, &input, "pw");
        visual.update(|window, cx| form.update(cx, |f, cx| f.submit(window, cx)));
        settle(&mut visual, |_| *done.borrow() == 1);
    }

    #[gpui_kit::test]
    fn compress_waits_for_two_matching_passwords_and_hands_them_over(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, _) = window(cx, "");
        let got: Rc<RefCell<Vec<(Format, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = got.clone();
        let view = visual.update(|window, cx| {
            let view = cx.new(|cx| {
                let sink = sink.clone();
                CompressPasswordDialog::new(
                    Format::Zip,
                    move |format, password, _, _| sink.borrow_mut().push((format, password)),
                    window,
                    cx,
                )
            });
            show(
                window,
                cx,
                t!("Compress {what} with a password", what = "3 items"),
                px(384.),
                view.clone(),
            );
            view
        });
        visual.run_until_parked();
        let (first, second) = view.read_with(&visual, |v, _| (v.password_input().clone(), v.again_input().clone()));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("compress-go").disabled(), Some(true));
        });
        assert_eq!(role_of(&mut visual, "Format"), Some(Role::RadioGroup));
        assert_eq!(role_of(&mut visual, "zip (names show)"), Some(Role::RadioButton));
        type_into(&mut visual, &first, "kalimat-sirr");
        type_into(&mut visual, &second, "kalimat");
        assert_eq!(role_of(&mut visual, "The two aren't the same."), Some(Role::Alert));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("compress-go").disabled(), Some(true));
        });
        // Enter on a mismatch does nothing.
        visual.update(|window, cx| window.press("enter", cx));
        visual.run_until_parked();
        assert!(got.borrow().is_empty());
        visual.update(|window, cx| assert!(window.has_active_dialog(cx), "Enter on a mismatch keeps it open"));
        type_into(&mut visual, &second, "kalimat-sirr");
        assert!(!has(&mut visual, "The two aren't the same."));
        // Pick 7z, then Compress.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("compress-format", Format::SevenZ as usize), cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("compress-go", cx);
        });
        visual.run_until_parked();
        assert_eq!(*got.borrow(), vec![(Format::SevenZ, "kalimat-sirr".to_string())]);
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render_and_escape_closes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, _) = window(cx, "x");
        testing::set_rtl(true);
        visual.update(|window, cx| ask_archive_password(window, cx, "C:\\ملفات\\أرشيف.7z".into(), |_, _| {}));
        visual.run_until_parked();
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Escape closes it"));
        visual.update(|window, cx| ask_compress_password(window, cx, "ملف التقرير", Format::SevenZ, |_, _, _, _| {}));
        visual.run_until_parked();
        assert!(has(&mut visual, "Compress with a password"));
        testing::set_rtl(false);
    }
}
