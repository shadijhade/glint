//! Previous versions of a file or folder: Windows' shadow copies (restore points) and Glint's own backups.
//! Ported from src/components/explorer/VersionsDialog.tsx, built the way [`super::drive_health`] is.
//!
//! | TypeScript                                    | Here                                                      |
//! |-----------------------------------------------|-----------------------------------------------------------|
//! | `useVersions` store, `load`, `openVersions`   | [`Versions`] (model) and [`open`]                         |
//! | `previousVersions`, `restoreVersion`, `versionCopy`, `backup_*` commands | [`Source`]                    |
//! | `nameShadowCopy` for each version             | the model teaches `glint_model::paths::set_shadow_name`   |
//! | `VersionsDialog`                              | [`VersionsDialog`]                                        |
//! | toasts (with Show)                            | notifications raised by the view from [`VersionsEvent`]   |
//! | `openInNewTab`, `compareWithVersion`, `compareFiles`, `navigate(.., {select})` | [`Request`]s to the `on_request` callback [`open`] takes |
//! | `openVersionFile`                             | done here: `version_copy`, then the shell opens the copy (the explorer's own `Handoff::VersionFile` is what a double-click on a version in a folder emits) |
//!
//! The list is virtually always short (one entry per shadow copy that differs, at most a few dozen), so it
//! is a plain scrolling column.

use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::backup::BackupVersion;
use glint_core::host::Manager as _;
use glint_core::versions::Version;
use glint_model::format::{format_full_date, format_size};
use glint_model::paths::{base_name, parent_path, set_shadow_name};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// Where the answers come from: glint-core in the app, fakes in tests.
#[derive(Clone)]
pub struct Source {
    /// Shadow copies that have the path (`previous_versions`).
    pub previous: Arc<dyn Fn(String) -> Fut<Vec<Version>> + Send + Sync>,
    /// Glint's backups that hold the path (`backup_versions`).
    pub backups: Arc<dyn Fn(String) -> Fut<Vec<BackupVersion>> + Send + Sync>,
    /// `restore(from, to, replace)`: copies a version out; where it went.
    pub restore: Arc<dyn Fn(String, String, bool) -> Fut<String> + Send + Sync>,
    /// A version file copied to a temp folder, for its app to open.
    pub version_copy: Arc<dyn Fn(String) -> Fut<String> + Send + Sync>,
    /// Opens a file as a double-click does.
    pub open_item: Arc<dyn Fn(String) -> Fut<()> + Send + Sync>,
    /// `backup_open(backup, path)`: the item as it was, made in a temp folder; where.
    pub backup_open: Arc<dyn Fn(BackupVersion, String) -> Fut<String> + Send + Sync>,
    /// `backup_restore(backup, path, replace, label)`: where it went.
    pub backup_restore: Arc<dyn Fn(BackupVersion, String, bool, String) -> Fut<String> + Send + Sync>,
}

/// glint-core's functions, as a [`Source`]. The ones that need a window for shell dialogs use the main one;
/// without it (it never happens in the running app) they fail with a message.
pub fn core_source(cx: &App) -> Source {
    let host = cx.try_global::<core_bridge::Core>().and_then(|core| {
        core.app
            .get_webview_window(crate::workspace::MAIN)
            .or_else(|| core.app.webview_windows().into_values().next())
    });
    let need = move || host.clone().ok_or_else(|| "Windows didn't answer".to_string());
    let (h1, h2, h3) = (need.clone(), need.clone(), need);
    Source {
        previous: Arc::new(|path| Box::pin(glint_core::versions::previous_versions(path))),
        backups: Arc::new(|path| Box::pin(glint_core::backup::backup_versions(path))),
        restore: Arc::new(move |from, to, replace| {
            let window = h1();
            Box::pin(async move { glint_core::versions::restore_version(window?, from, to, replace).await })
        }),
        version_copy: Arc::new(|path| Box::pin(glint_core::versions::version_copy(path))),
        open_item: Arc::new(move |path| {
            let window = h2();
            Box::pin(async move { glint_core::shell::open_item(window?, path).await.map(drop) })
        }),
        backup_open: Arc::new(|b, path| Box::pin(glint_core::backup::backup_open(b.plan, b.snapshot, path, b.dir))),
        backup_restore: Arc::new(move |b, path, replace, label| {
            let window = h3();
            Box::pin(async move {
                glint_core::backup::backup_restore(window?, b.plan, b.snapshot, path, b.dir, replace, label).await
            })
        }),
    }
}

// ---- Words ------------------------------------------------------------------------------------------------

/// "About" for a time found from the copy itself rather than told by VSS.
pub fn when(v: &Version) -> SharedString {
    if v.created != 0 {
        let date = format_full_date(v.created);
        if v.exact {
            date.into()
        } else {
            format!("≈ {date}").into()
        }
    } else {
        t!("When isn't known")
    }
}

/// Where a backup is, in words.
pub fn place_of(repository: &str) -> SharedString {
    match repository.strip_prefix("rclone:") {
        Some(rest) => t!("{name} (a cloud)", name = rest.split(':').next().unwrap_or(rest)),
        None => repository.to_string().into(),
    }
}

/// "backup 2026-11-09", for a restored copy's name.
// PORT ME(coordinator): the TypeScript used the local date; glint-ui has no time-zone helper, so this is the UTC date.
pub fn backup_label(time_ms: i64) -> String {
    let days = time_ms.div_euclid(86_400_000);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("backup {year}-{month:02}-{day:02}")
}

// ---- The model --------------------------------------------------------------------------------------------

/// Where the list stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Loading,
    Failed(SharedString),
    Ready,
}

/// Said by the model; the view turns each into a notification or an action (the TypeScript's toasts).
#[derive(Clone, Debug)]
pub enum VersionsEvent {
    /// A busy toast: `Copying out “x”…`, `Getting it out of the backup…`.
    Working(String),
    /// Restored: `Restored “name”` (replace, with the Recycle Bin note) or `Restored a copy: “name”`, and Show.
    Restored { replace: bool, name: String, at: String },
    /// Something failed: `title` with the reason as description.
    Failed { title: String, reason: String },
    /// A folder or file out of a backup is ready in `made`; the view opens it in a tab or compares it.
    BackupMade {
        backup: BackupVersion,
        made: String,
        compare: bool,
    },
}

/// The versions of one path.
pub struct Versions {
    path: String,
    versions: Vec<Version>,
    backups: Vec<BackupVersion>,
    backup_error: Option<SharedString>,
    state: State,
    /// Bumped by every load; an older answer is dropped (the TypeScript's `asked`).
    generation: u64,
    source: Source,
}

impl EventEmitter<VersionsEvent> for Versions {}

impl Versions {
    /// A model for `path`, loading at once.
    pub fn new(path: String, source: Source, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            versions: Vec::new(),
            backups: Vec::new(),
            backup_error: None,
            state: State::Loading,
            generation: 0,
            source,
        };
        this.load(cx);
        this
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn versions(&self) -> &[Version] {
        &self.versions
    }

    pub fn backups(&self) -> &[BackupVersion] {
        &self.backups
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Reads the shadow copies and, alongside, Glint's backups. Both run on core's runtime.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.state = State::Loading;
        cx.notify();
        let backups = (self.source.backups)(self.path.clone());
        let previous = (self.source.previous)(self.path.clone());
        cx.spawn(async move |this, cx| {
            let backups = core_bridge::run(backups).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match backups {
                    Ok(list) => {
                        this.backups = list;
                        this.backup_error = None;
                    }
                    Err(reason) => {
                        this.backups = Vec::new();
                        this.backup_error = Some(reason.into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        let drive = self.path.get(..2).unwrap_or("").to_uppercase();
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(previous).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(versions) => {
                        for v in &versions {
                            set_shadow_name(&v.root, &shadow_name(&drive, v.created, v.exact));
                        }
                        this.versions = versions;
                        this.state = State::Ready;
                    }
                    Err(reason) => {
                        this.versions = Vec::new();
                        this.state = State::Failed(reason.into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Copies a shadow copy's item out beside what's there, or over it (`replace`; what's there goes to the
    /// Recycle Bin). Loads again afterwards, since a restored file is no longer different from now.
    pub fn restore(&mut self, version: &Version, replace: bool, cx: &mut Context<Self>) {
        let (from, now) = (version.path.clone(), self.path.clone());
        let work = (self.source.restore)(from, now.clone(), replace);
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(work).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| match result {
                Ok(at) => {
                    cx.emit(VersionsEvent::Restored {
                        replace,
                        name: base_name(if replace { &now } else { &at }).to_string(),
                        at,
                    });
                    this.load(cx);
                }
                Err(reason) => cx.emit(VersionsEvent::Failed {
                    title: t!("Couldn't restore it").to_string(),
                    reason,
                }),
            });
        })
        .detach();
    }

    /// Opens a file version: a read-only copy in a temp folder, opened by its app.
    pub fn open_file(&mut self, version_path: String, cx: &mut Context<Self>) {
        let name = base_name(&version_path).to_string();
        cx.emit(VersionsEvent::Working(
            t!("Copying out “{name}”…", name = name).to_string(),
        ));
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(async move {
                let copy = (source.version_copy)(version_path).await?;
                (source.open_item)(copy).await
            })
            .await
            .and_then(|inner| inner);
            if let Err(reason) = result {
                let _ = this.update(cx, |_, cx| {
                    cx.emit(VersionsEvent::Failed {
                        title: t!("Couldn't open “{name}”", name = name).to_string(),
                        reason,
                    })
                });
            }
        })
        .detach();
    }

    /// Takes a file or folder out of a backup into a temp folder. A file that is only being opened is opened
    /// here; a folder, or anything being compared, is handed to the view as [`VersionsEvent::BackupMade`].
    pub fn backup_open(&mut self, backup: &BackupVersion, compare: bool, cx: &mut Context<Self>) {
        cx.emit(VersionsEvent::Working(
            if compare {
                t!("Getting it out of the backup to compare…")
            } else {
                t!("Getting it out of the backup…")
            }
            .to_string(),
        ));
        let (source, path, backup) = (self.source.clone(), self.path.clone(), backup.clone());
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run({
                let (source, backup) = (source.clone(), backup.clone());
                async move {
                    let made = (source.backup_open)(backup.clone(), path).await?;
                    if !compare && !backup.dir {
                        (source.open_item)(made.clone()).await?;
                    }
                    Ok::<_, String>(made)
                }
            })
            .await
            .and_then(|inner| inner);
            let _ = this.update(cx, |_, cx| match result {
                Ok(made) if compare || backup.dir => cx.emit(VersionsEvent::BackupMade { backup, made, compare }),
                Ok(_) => {}
                Err(reason) => cx.emit(VersionsEvent::Failed {
                    title: t!("Couldn't get it out of the backup").to_string(),
                    reason,
                }),
            });
        })
        .detach();
    }

    /// Restores from a backup: over what's there (it goes to the Recycle Bin) or as a dated copy beside it.
    pub fn restore_backup(&mut self, backup: &BackupVersion, replace: bool, cx: &mut Context<Self>) {
        cx.emit(VersionsEvent::Working(t!("Restoring from the backup…").to_string()));
        let path = self.path.clone();
        let work_path = path;
        let this = cx.weak_entity();
        let source = self.source.clone();
        let (backup, label) = (backup.clone(), backup_label(backup.time));
        cx.spawn(async move |_, cx| {
            let work = (source.backup_restore)(backup, work_path.clone(), replace, label);
            let result = core_bridge::run(work).await.and_then(|inner| inner);
            let _ = this.update(cx, |_, cx| match result {
                Ok(at) => cx.emit(VersionsEvent::Restored {
                    replace,
                    name: base_name(if replace { &work_path } else { &at }).to_string(),
                    at,
                }),
                Err(reason) => cx.emit(VersionsEvent::Failed {
                    title: t!("Couldn't restore it").to_string(),
                    reason,
                }),
            });
        })
        .detach();
    }

    /// Opens Windows' System Protection settings (the empty state's button).
    pub fn open_system_protection(&mut self, cx: &mut Context<Self>) {
        let work = (self.source.open_item)("C:\\Windows\\System32\\SystemPropertiesProtection.exe".to_string());
        cx.spawn(async move |this, cx| {
            if let Err(reason) = core_bridge::run(work).await.and_then(|inner| inner) {
                let _ = this.update(cx, |_, cx| {
                    cx.emit(VersionsEvent::Failed {
                        title: t!("Couldn't open System Protection").to_string(),
                        reason,
                    })
                });
            }
        })
        .detach();
    }
}

/// What the shadow copy is called in tabs and crumbs: `C: on Oct 3, 2026 4:15:00 PM`.
fn shadow_name(drive: &str, created: i64, exact: bool) -> String {
    let drive = if drive.is_empty() {
        t!("Drive").to_string()
    } else {
        drive.to_string()
    };
    if created != 0 {
        let approx = if exact { "" } else { "≈ " };
        t!(
            "{drive} on {when}",
            drive = drive,
            when = format!("{approx}{}", format_full_date(created))
        )
        .to_string()
    } else {
        t!("{drive} (shadow copy)", drive = drive).to_string()
    }
}

// ---- The view ---------------------------------------------------------------------------------------------

/// Work the dialog hands to the explorer, which owns tabs, comparing and navigating.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// `openInNewTab(path)`: a folder as it was (in a shadow copy, or made from a backup).
    OpenFolderInNewTab(String),
    /// `compareWithVersion(now, then)`: the folder now beside how it was.
    CompareFolders { now: String, then: String },
    /// `compareFiles(then, now)`: what changed in a file since.
    CompareFiles { then: String, now: String },
    /// Show's `navigate(parentPath(at), { select: at })`: go to the restored item and select it.
    Show { folder: String, select: String },
}

type OnRequest = Rc<dyn Fn(Request, &mut Window, &mut App)>;

/// Previous versions of a file or folder, with Open, Compare, Restore a copy and Restore for each.
pub struct VersionsDialog {
    versions: Entity<Versions>,
    on_request: OnRequest,
    /// The row asking "Replace what's there now?" (a version's path, or a backup's snapshot).
    replacing: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl VersionsDialog {
    pub fn new(versions: Entity<Versions>, on_request: OnRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let redraw = cx.observe(&versions, |_, _, cx| cx.notify());
        let request = on_request.clone();
        let events = cx.subscribe_in(
            &versions,
            window,
            move |_, versions, event: &VersionsEvent, window, cx| match event {
                VersionsEvent::Working(text) => {
                    window.push_notification(Notification::info(SharedString::from(text.clone())), cx)
                }
                VersionsEvent::Restored { replace, name, at } => {
                    let title = if *replace {
                        t!("Restored “{name}”", name = name)
                    } else {
                        t!("Restored a copy: “{name}”", name = name)
                    };
                    let notification = if *replace {
                        Notification::success(t!("What was there is in the Recycle Bin.")).title(title)
                    } else {
                        Notification::success(title)
                    };
                    let (request, at) = (request.clone(), at.clone());
                    window.push_notification(
                        notification.action(move |_, _, _| {
                            let (request, at) = (request.clone(), at.clone());
                            Button::new("versions-show")
                                .label(t!("Show"))
                                .on_click(move |_, window, cx| {
                                    request(
                                        Request::Show {
                                            folder: parent_path(&at),
                                            select: at.clone(),
                                        },
                                        window,
                                        cx,
                                    );
                                })
                        }),
                        cx,
                    );
                }
                VersionsEvent::Failed { title, reason } => window.push_notification(
                    Notification::error(SharedString::from(reason.clone())).title(SharedString::from(title.clone())),
                    cx,
                ),
                VersionsEvent::BackupMade { backup, made, compare } => {
                    let now = versions.read(cx).path().to_string();
                    let what = match (*compare, backup.dir) {
                        (true, true) => Request::CompareFolders {
                            now,
                            then: made.clone(),
                        },
                        (true, false) => Request::CompareFiles {
                            then: made.clone(),
                            now,
                        },
                        (false, _) => Request::OpenFolderInNewTab(made.clone()),
                    };
                    if *compare {
                        window.close_dialog(cx);
                    }
                    request(what, window, cx);
                }
            },
        );
        Self {
            versions,
            on_request,
            replacing: None,
            _subscriptions: vec![redraw, events],
        }
    }

    fn name_of(path: &str) -> SharedString {
        let name = base_name(path);
        (if name.is_empty() { path } else { name }).to_string().into()
    }

    /// The "Replace what's there now?" question with its two answers.
    fn replace_question(
        &self,
        id: usize,
        t: &theme::Tokens,
        on_replace: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let on_replace = Rc::new(on_replace);
        row()
            .id(("versions-ask", id))
            .role(Role::Group)
            .aria_label(t!("Replace what's there now? It goes to the Recycle Bin."))
            .test_support()
            .flex_none()
            .gap(space(1.5))
            .text_size(theme::size::XS)
            .child(
                div()
                    .text_color(t.muted_foreground)
                    .child(t!("Replace what's there now? It goes to the Recycle Bin.")),
            )
            .child(
                Button::new(("versions-replace", id))
                    .danger()
                    .xsmall()
                    .label(t!("Replace"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.replacing = None;
                        on_replace(this, window, cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new(("versions-replace-cancel", id))
                    .ghost()
                    .xsmall()
                    .label(t!("Cancel"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.replacing = None;
                        cx.notify();
                    })),
            )
    }

    /// A ghost button with an icon and a label, for a row's four actions.
    fn action(id: (&'static str, usize), name: IconName, label: SharedString, tip: SharedString) -> Button {
        Button::new(id)
            .ghost()
            .xsmall()
            .icon(name)
            .label(label)
            .tooltip(tip.clone())
            .accessibility_label(tip)
    }

    fn version_row(
        &self,
        ix: usize,
        v: &Version,
        now: &str,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let when = when(v);
        let detail: SharedString = if v.dir {
            t!("The folder as it was")
        } else {
            t!(
                "{size} · modified {date}",
                size = format_size(v.size),
                date = format_full_date(v.modified)
            )
        };
        let replacing = self.replacing.as_deref() == Some(v.path.as_str());
        let (path, now) = (v.path.clone(), now.to_string());

        let actions: AnyElement = if replacing {
            let v = v.clone();
            self.replace_question(
                ix,
                t,
                move |this, _, cx| this.versions.update(cx, |m, cx| m.restore(&v, true, cx)),
                cx,
            )
            .into_any_element()
        } else {
            let request = self.on_request.clone();
            let open = {
                let (path, request, dir) = (path.clone(), request.clone(), v.dir);
                Self::action(
                    ("versions-open", ix),
                    if v.dir {
                        IconName::FolderOpen
                    } else {
                        IconName::ExternalLink
                    },
                    t!("Open"),
                    if v.dir {
                        t!("Open the folder as it was, in a new tab")
                    } else {
                        t!("Open a read-only copy of it")
                    },
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if dir {
                        request(Request::OpenFolderInNewTab(path.clone()), window, cx);
                    } else {
                        this.versions.update(cx, |m, cx| m.open_file(path.clone(), cx));
                    }
                }))
            };
            let compare = (!v.gone).then(|| {
                let (path, now, request, dir) = (path.clone(), now.clone(), request.clone(), v.dir);
                Self::action(
                    ("versions-compare", ix),
                    IconName::Columns2,
                    t!("Compare"),
                    if dir {
                        t!("This folder now beside how it was, compared")
                    } else {
                        t!("What changed in it since")
                    },
                )
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    request(
                        if dir {
                            Request::CompareFolders {
                                now: now.clone(),
                                then: path.clone(),
                            }
                        } else {
                            Request::CompareFiles {
                                then: path.clone(),
                                now: now.clone(),
                            }
                        },
                        window,
                        cx,
                    );
                })
            });
            let copy = {
                let v = v.clone();
                Self::action(
                    ("versions-copy", ix),
                    IconName::Copy,
                    t!("Restore a copy"),
                    t!("Copy it out beside what's there now, under a new name"),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.versions.update(cx, |m, cx| m.restore(&v, false, cx))))
            };
            let restore = (!v.dir).then(|| {
                let v = v.clone();
                Self::action(
                    ("versions-restore", ix),
                    IconName::ArchiveRestore,
                    t!("Restore"),
                    if v.gone {
                        t!("Put it back where it was")
                    } else {
                        t!("Put it back in place of what's there now")
                    },
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if v.gone {
                        this.versions.update(cx, |m, cx| m.restore(&v, true, cx));
                    } else {
                        this.replacing = Some(v.path.clone());
                        cx.notify();
                    }
                }))
            });
            row()
                .flex_none()
                .gap(space(1.))
                .child(open)
                .children(compare)
                .child(copy)
                .children(restore)
                .into_any_element()
        };

        row_start()
            .id(("versions-row", ix))
            .role(Role::Group)
            .aria_label(format!("{when}. {detail}"))
            .test_support()
            .gap(space(3.))
            .px(space(3.))
            .py(space(2.))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .bg(t.card.opacity(0.4))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .align_start()
                    .child(div().font_weight(FontWeight::MEDIUM).child(when))
                    .child(
                        align_row(1.5)
                            .max_w_full()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(div().min_w_0().truncate().child(detail))
                            .children(v.gone.then(|| {
                                div()
                                    .flex_none()
                                    .px(space(1.5))
                                    .py(space(0.5))
                                    .rounded(theme::size::RADIUS_SM)
                                    .bg(t.warning.opacity(0.15))
                                    .text_color(t.warning)
                                    .child(t!("gone now"))
                            })),
                    ),
            )
            .child(actions)
    }

    fn backup_row(&self, ix: usize, b: &BackupVersion, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let id = 1000 + ix;
        let when = SharedString::from(format_full_date(b.time));
        let place = t!("Backed up to {place}", place = place_of(&b.repository));
        let replacing = self.replacing.as_deref() == Some(b.snapshot.as_str());
        let actions: AnyElement = if replacing {
            let b = b.clone();
            self.replace_question(
                id,
                t,
                move |this, _, cx| this.versions.update(cx, |m, cx| m.restore_backup(&b, true, cx)),
                cx,
            )
            .into_any_element()
        } else {
            let open = {
                let b = b.clone();
                Self::action(
                    ("versions-open", id),
                    if b.dir {
                        IconName::FolderOpen
                    } else {
                        IconName::ExternalLink
                    },
                    t!("Open"),
                    if b.dir {
                        t!("Open the folder as it was, in a new tab")
                    } else {
                        t!("Open a copy of it as it was")
                    },
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.versions.update(cx, |m, cx| m.backup_open(&b, false, cx))),
                )
            };
            let compare = {
                let b = b.clone();
                Self::action(
                    ("versions-compare", id),
                    IconName::Columns2,
                    t!("Compare"),
                    if b.dir {
                        t!("This folder now beside how it was, compared")
                    } else {
                        t!("What changed in it since")
                    },
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.versions.update(cx, |m, cx| m.backup_open(&b, true, cx))),
                )
            };
            let copy =
                {
                    let b = b.clone();
                    Self::action(
                        ("versions-copy", id),
                        IconName::Copy,
                        t!("Restore a copy"),
                        t!("Copy it out beside what's there now, under a new name"),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.versions.update(cx, |m, cx| m.restore_backup(&b, false, cx))
                    }))
                };
            let restore = (!b.dir).then(|| {
                let snapshot = b.snapshot.clone();
                Self::action(
                    ("versions-restore", id),
                    IconName::ArchiveRestore,
                    t!("Restore"),
                    t!("Put it back in place of what's there now"),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.replacing = Some(snapshot.clone());
                    cx.notify();
                }))
            });
            row()
                .flex_none()
                .gap(space(1.))
                .child(open)
                .child(compare)
                .child(copy)
                .children(restore)
                .into_any_element()
        };
        row_start()
            .id(("versions-backup", ix))
            .role(Role::Group)
            .aria_label(format!("{when}. {place}"))
            .test_support()
            .gap(space(3.))
            .px(space(3.))
            .py(space(2.))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .bg(t.card.opacity(0.4))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .align_start()
                    .child(div().font_weight(FontWeight::MEDIUM).child(when))
                    .child(
                        div()
                            .max_w_full()
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(place),
                    ),
            )
            .child(actions)
    }
}

impl Render for VersionsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let model = self.versions.read(cx);
        let path = model.path().to_string();
        let versions = model.versions().to_vec();
        let backups = model.backups().to_vec();
        let backup_error = model.backup_error.clone();
        let state = model.state().clone();
        let name = Self::name_of(&path);
        let near = versions.iter().any(|v| !v.exact);

        let intro = div()
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(t!(
                "As Windows' shadow copies kept it: restore points and backups. They're read-only; restoring copies out of them."
            ));
        let refresh = Button::new("versions-refresh")
            .ghost()
            .xsmall()
            .icon(IconName::RefreshCw)
            .loading(state == State::Loading && !versions.is_empty())
            .tooltip(t!("Refresh"))
            .accessibility_label(t!("Refresh"))
            .on_click(cx.listener(|this, _, _, cx| this.versions.update(cx, |m, cx| m.load(cx))));

        let nothing = state == State::Ready && versions.is_empty() && backups.is_empty();
        let status: Option<AnyElement> = match &state {
            State::Loading if versions.is_empty() => Some(
                col()
                    .id("versions-loading")
                    .role(Role::Status)
                    .aria_label(t!("Looking for earlier versions…"))
                    .test_support()
                    .items_center()
                    .py(space(14.))
                    .child(Spinner::new().color(t.primary))
                    .into_any_element(),
            ),
            State::Failed(reason) => Some(
                div()
                    .id("versions-error")
                    .role(Role::Alert)
                    .aria_label(reason.clone())
                    .test_support()
                    .py(space(12.))
                    .px(space(6.))
                    .text_center()
                    .text_color(t.destructive)
                    .child(reason.clone())
                    .into_any_element(),
            ),
            _ if nothing => {
                let first = t!("No earlier version of “{name}” is kept on this PC.", name = name);
                Some(
                    col()
                        .id("versions-none")
                        .role(Role::Status)
                        .aria_label(first.clone())
                        .test_support()
                        .items_center()
                        .gap(space(3.))
                        .py(space(10.))
                        .px(space(8.))
                        .text_center()
                        .text_color(t.muted_foreground)
                        .child(isolate(first))
                        .child(div().text_size(theme::size::XS).child(t!(
                            "Windows keeps them when System Protection is on for the drive (its restore points) or a backup makes shadow copies; then a file's versions show here from the next one on."
                        )))
                        .child(
                            Button::new("versions-protection")
                                .outline()
                                .small()
                                .label(t!("System Protection…"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.versions.update(cx, |m, cx| m.open_system_protection(cx))
                                })),
                        )
                        .into_any_element(),
                )
            }
            _ => None,
        };

        let list = (!versions.is_empty()).then(|| {
            col()
                .id("versions-list")
                .role(Role::List)
                .aria_label(t!("Previous versions"))
                .test_support()
                .gap(space(1.))
                .children(
                    versions
                        .iter()
                        .enumerate()
                        .map(|(ix, v)| self.version_row(ix, v, &path, &t, cx)),
                )
        });

        let backups_section = (!backups.is_empty() || backup_error.is_some()).then(|| {
            col()
                .id("versions-backups")
                .role(Role::Group)
                .aria_label(t!("Glint's backups"))
                .test_support()
                .gap(space(1.))
                .mt(space(3.))
                .child(
                    align_row(1.5)
                        .px(space(1.))
                        .text_size(theme::size::XS)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.muted_foreground)
                        .child(icon(IconName::DatabaseBackup).size_3p5())
                        .child(t!("Glint's backups")),
                )
                .children(backup_error.map(|e| {
                    div()
                        .px(space(1.))
                        .text_size(theme::size::XS)
                        .text_color(t.destructive)
                        .child(e)
                }))
                .children(backups.iter().enumerate().map(|(ix, b)| self.backup_row(ix, b, &t, cx)))
        });

        col()
            .id("versions")
            .role(Role::Group)
            .aria_label(t!("Previous versions of “{name}”", name = name))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            .text_start()
            .pt(space(1.))
            .child(align_row(3.).child(div().flex_1().min_w_0().child(intro)).child(refresh))
            .child(
                div()
                    .id("versions-scroll")
                    .min_h(rems(12.))
                    .max_h(rems(28.))
                    .overflow_y_scroll()
                    .child(col().children(status).children(list).children(backups_section)),
            )
            .children(near.then(|| {
                div()
                    .pt(space(2.))
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!(
                        "Times marked ≈ are when the copy last changed, close to when it was taken. With instant search on, Windows says exactly."
                    ))
            }))
    }
}

/// Opens Previous versions of `path` (a file or folder on one of this PC's drives) in `window`. `on_request`
/// does what the explorer owns: tabs, comparing and going to a restored item (see [`Request`]).
pub fn open(
    window: &mut Window,
    cx: &mut App,
    path: String,
    on_request: impl Fn(Request, &mut Window, &mut App) + 'static,
) {
    let source = core_source(cx);
    open_with(window, cx, path, source, on_request);
}

/// [`open`] with a given [`Source`] (tests pass fakes). Nothing opens for an empty path, as in the TypeScript.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    path: String,
    source: Source,
    on_request: impl Fn(Request, &mut Window, &mut App) + 'static,
) {
    if path.is_empty() {
        return;
    }
    let title = t!("Previous versions of “{name}”", name = VersionsDialog::name_of(&path));
    let model = cx.new(|cx| Versions::new(path, source, cx));
    let on_request: OnRequest = Rc::new(on_request);
    let view = cx.new(|cx| VersionsDialog::new(model, on_request, window, cx));
    super::show(window, cx, title, px(768.), view);
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

    const FILE: &str = "C:\\Docs\\report.docx";

    fn version(n: u32, dir: bool, exact: bool, gone: bool) -> Version {
        Version {
            path: format!("\\\\?\\GLOBALROOT\\Device\\HarddiskVolumeShadowCopy{n}\\Docs\\report.docx"),
            root: format!("\\\\?\\GLOBALROOT\\Device\\HarddiskVolumeShadowCopy{n}\\"),
            created: 1_700_000_000_000 + i64::from(n) * 86_400_000,
            exact,
            dir,
            size: 2048,
            modified: 1_699_000_000_000,
            gone,
        }
    }

    fn backup(dir: bool) -> BackupVersion {
        BackupVersion {
            plan: "docs".into(),
            repository: "rclone:gdrive:backups".into(),
            snapshot: "abc123".into(),
            time: 1_700_000_000_000,
            dir,
        }
    }

    #[derive(Default)]
    struct Calls {
        restores: Vec<(String, String, bool)>,
        opened: Vec<String>,
        copies: Vec<String>,
        backup_restores: Vec<(String, bool, String)>,
    }

    struct Fake {
        source: Source,
        calls: Arc<Mutex<Calls>>,
    }

    fn fake(
        versions: Result<Vec<Version>, String>,
        backups: Result<Vec<BackupVersion>, String>,
        restore_fails: bool,
    ) -> Fake {
        let calls = Arc::new(Mutex::new(Calls::default()));
        let (c1, c2, c3, c5) = (calls.clone(), calls.clone(), calls.clone(), calls.clone());
        let source = Source {
            previous: Arc::new(move |_| {
                let answer = versions.clone();
                Box::pin(async move { answer })
            }),
            backups: Arc::new(move |_| {
                let answer = backups.clone();
                Box::pin(async move { answer })
            }),
            restore: Arc::new(move |from, to, replace| {
                c1.lock().unwrap().restores.push((from, to.clone(), replace));
                Box::pin(async move {
                    if restore_fails {
                        Err("Access is denied".to_string())
                    } else if replace {
                        Ok(to)
                    } else {
                        Ok("C:\\Docs\\report (2).docx".to_string())
                    }
                })
            }),
            version_copy: Arc::new(move |path| {
                c2.lock().unwrap().copies.push(path);
                Box::pin(async { Ok("C:\\Temp\\report.docx".to_string()) })
            }),
            open_item: Arc::new(move |path| {
                c3.lock().unwrap().opened.push(path);
                Box::pin(async { Ok(()) })
            }),
            backup_open: Arc::new(move |_, _| Box::pin(async { Ok("C:\\Temp\\backup\\report.docx".to_string()) })),
            backup_restore: Arc::new(move |b, path, replace, label| {
                c5.lock().unwrap().backup_restores.push((path, replace, label));
                let dir = b.dir;
                Box::pin(async move {
                    Ok(if dir {
                        "C:\\Docs".to_string()
                    } else {
                        "C:\\Docs\\report (backup).docx".to_string()
                    })
                })
            }),
        };
        Fake { source, calls }
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
        panic!("never arrived: {what}");
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

    fn open_fake(visual: &mut VisualTestContext, path: &str, source: Source) -> Rc<RefCell<Vec<Request>>> {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let log = requests.clone();
        let path = path.to_string();
        visual.update(|window, cx| open_with(window, cx, path, source, move |r, _, _| log.borrow_mut().push(r)));
        requests
    }

    fn click(cx: &mut VisualTestContext, id: (&'static str, usize)) {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        cx.run_until_parked();
    }

    fn row_label(v: &Version) -> String {
        let detail = if v.dir {
            "The folder as it was".to_string()
        } else {
            format!("{} · modified {}", format_size(v.size), format_full_date(v.modified))
        };
        format!("{}. {}", when(v), detail)
    }

    #[test]
    fn words_dates_and_places_follow_the_typescript() {
        let _guard = testing::lock();
        assert_eq!(backup_label(0), "backup 1970-01-01");
        assert_eq!(backup_label(1_700_000_000_000), "backup 2023-11-14");
        assert_eq!(place_of("rclone:gdrive:backups"), "gdrive (a cloud)");
        assert_eq!(place_of("D:\\Backups"), "D:\\Backups");
        let exact = version(1, false, true, false);
        let near = version(1, false, false, false);
        assert_eq!(when(&exact), format_full_date(exact.created));
        assert_eq!(when(&near), format!("≈ {}", format_full_date(near.created)));
        assert_eq!(when(&Version { created: 0, ..near }), "When isn't known");
        assert_eq!(shadow_name("C:", 0, true), "C: (shadow copy)");
        assert!(shadow_name("C:", 1_700_000_000_000, false).starts_with("C: on ≈ "));
    }

    #[gpui_kit::test]
    fn lists_versions_and_backups_with_labelled_rows(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (a, b, c) = (
            version(1, false, true, false),
            version(2, false, false, false),
            version(3, false, true, true),
        );
        let fake = fake(
            Ok(vec![a.clone(), b.clone(), c.clone()]),
            Ok(vec![backup(false)]),
            false,
        );
        open_fake(&mut visual, FILE, fake.source);
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            window.render_frame(cx);
            assert!(!window.find_all_by_role(Role::Dialog).is_empty());
        });
        settle(&mut visual, "the rows", |cx| has(cx, &row_label(&c)));
        assert_eq!(role_of(&mut visual, &row_label(&a)), Some(Role::Group));
        assert_eq!(role_of(&mut visual, &row_label(&b)), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Glint's backups"), Some(Role::Group));
        assert!(has(
            &mut visual,
            &format!("{}. Backed up to gdrive (a cloud)", format_full_date(1_700_000_000_000))
        ));
        // The shadow copies were named for tabs and crumbs.
        assert!(glint_model::paths::shadow_name(&a.path).starts_with("C: on "));
    }

    #[gpui_kit::test]
    fn restoring_over_a_file_asks_first_and_a_copy_does_not(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let v = version(1, false, true, false);
        let fake = fake(Ok(vec![v.clone()]), Ok(vec![]), false);
        let calls = fake.calls.clone();
        open_fake(&mut visual, FILE, fake.source);
        settle(&mut visual, "the row", |cx| has(cx, &row_label(&v)));

        // Restore asks; Cancel puts the buttons back and restores nothing.
        click(&mut visual, ("versions-restore", 0));
        assert_eq!(
            role_of(&mut visual, "Replace what's there now? It goes to the Recycle Bin."),
            Some(Role::Group)
        );
        assert!(calls.lock().unwrap().restores.is_empty());
        click(&mut visual, ("versions-replace-cancel", 0));
        assert!(!has(
            &mut visual,
            "Replace what's there now? It goes to the Recycle Bin."
        ));
        assert!(calls.lock().unwrap().restores.is_empty());

        // Replace restores in place.
        click(&mut visual, ("versions-restore", 0));
        click(&mut visual, ("versions-replace", 0));
        settle(&mut visual, "the restore", |_| {
            calls.lock().unwrap().restores.len() == 1
        });
        assert_eq!(
            calls.lock().unwrap().restores[0],
            (v.path.clone(), FILE.to_string(), true)
        );

        // A copy goes beside it without asking.
        settle(&mut visual, "the row again", |cx| has(cx, &row_label(&v)));
        click(&mut visual, ("versions-copy", 0));
        settle(&mut visual, "the copy", |_| calls.lock().unwrap().restores.len() == 2);
        assert!(!calls.lock().unwrap().restores[1].2);
    }

    #[gpui_kit::test]
    fn a_file_that_is_gone_is_put_back_without_asking(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let v = version(1, false, true, true);
        let fake = fake(Ok(vec![v.clone()]), Ok(vec![]), true);
        let calls = fake.calls.clone();
        open_fake(&mut visual, FILE, fake.source);
        settle(&mut visual, "the row", |cx| has(cx, &row_label(&v)));
        assert_eq!(role_of(&mut visual, "Put it back where it was"), Some(Role::Button));
        click(&mut visual, ("versions-restore", 0));
        settle(&mut visual, "the restore", |_| {
            calls.lock().unwrap().restores.len() == 1
        });
        assert!(calls.lock().unwrap().restores[0].2);
        // There is no Compare for what is gone.
        assert!(!has(&mut visual, "What changed in it since"));
    }

    #[gpui_kit::test]
    fn open_and_compare_go_to_the_right_place(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (file, folder) = (version(1, false, true, false), version(2, true, true, false));
        let fake = fake(Ok(vec![file.clone(), folder.clone()]), Ok(vec![]), false);
        let calls = fake.calls.clone();
        let requests = open_fake(&mut visual, FILE, fake.source);
        settle(&mut visual, "the rows", |cx| has(cx, &row_label(&folder)));

        // A file opens as a read-only copy, done here; a folder opens in a new tab, done by the explorer.
        click(&mut visual, ("versions-open", 0));
        settle(&mut visual, "the copy being opened", |_| {
            calls.lock().unwrap().opened == vec!["C:\\Temp\\report.docx".to_string()]
        });
        assert_eq!(calls.lock().unwrap().copies, vec![file.path.clone()]);
        click(&mut visual, ("versions-open", 1));
        assert_eq!(
            *requests.borrow(),
            vec![Request::OpenFolderInNewTab(folder.path.clone())]
        );

        // Compare closes the dialog and hands over.
        click(&mut visual, ("versions-compare", 1));
        assert_eq!(
            requests.borrow().last(),
            Some(&Request::CompareFolders {
                now: FILE.to_string(),
                then: folder.path.clone()
            })
        );
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn backups_open_compare_and_restore_as_the_typescript_did(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = fake(Ok(vec![]), Ok(vec![backup(false)]), false);
        let calls = fake.calls.clone();
        let requests = open_fake(&mut visual, FILE, fake.source);
        settle(&mut visual, "the backup row", |cx| has(cx, "Glint's backups"));

        // A file opens straight away from the temp folder.
        click(&mut visual, ("versions-open", 1000));
        settle(&mut visual, "the open", |_| !calls.lock().unwrap().opened.is_empty());
        assert_eq!(
            calls.lock().unwrap().opened,
            vec!["C:\\Temp\\backup\\report.docx".to_string()]
        );
        assert!(requests.borrow().is_empty());

        // Restore a copy is named for the backup's day.
        click(&mut visual, ("versions-copy", 1000));
        settle(&mut visual, "the restore", |_| {
            !calls.lock().unwrap().backup_restores.is_empty()
        });
        assert_eq!(
            calls.lock().unwrap().backup_restores[0],
            (FILE.to_string(), false, "backup 2023-11-14".to_string())
        );
        // Replace asks first.
        click(&mut visual, ("versions-restore", 1000));
        assert_eq!(calls.lock().unwrap().backup_restores.len(), 1);
        click(&mut visual, ("versions-replace", 1000));
        settle(&mut visual, "the replace", |_| {
            calls.lock().unwrap().backup_restores.len() == 2
        });
        assert!(calls.lock().unwrap().backup_restores[1].1);

        // Compare closes the dialog and compares the made file with the one now.
        click(&mut visual, ("versions-compare", 1000));
        settle(&mut visual, "the compare request", |_| !requests.borrow().is_empty());
        assert_eq!(
            *requests.borrow(),
            vec![Request::CompareFiles {
                then: "C:\\Temp\\backup\\report.docx".into(),
                now: FILE.into()
            }]
        );
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn nothing_kept_says_so_and_offers_system_protection(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = fake(Ok(vec![]), Ok(vec![]), false);
        let calls = fake.calls.clone();
        open_fake(&mut visual, FILE, fake.source);
        let label = "No earlier version of “report.docx” is kept on this PC.";
        settle(&mut visual, "the empty state", |cx| has(cx, label));
        assert_eq!(role_of(&mut visual, label), Some(Role::Status));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("versions-protection", cx);
        });
        settle(&mut visual, "the settings app", |_| {
            !calls.lock().unwrap().opened.is_empty()
        });
        assert_eq!(
            calls.lock().unwrap().opened,
            vec!["C:\\Windows\\System32\\SystemPropertiesProtection.exe".to_string()]
        );
    }

    #[gpui_kit::test]
    fn an_error_is_announced_and_refresh_reads_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let reason = "Previous versions are for what's on this PC's drives";
        let fake = fake(Err(reason.into()), Ok(vec![]), false);
        open_fake(&mut visual, "\\\\server\\share\\a.txt", fake.source);
        settle(&mut visual, "the error", |cx| has(cx, reason));
        assert_eq!(role_of(&mut visual, reason), Some(Role::Alert));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("versions-refresh", cx);
        });
        visual.run_until_parked();
    }

    #[gpui_kit::test]
    fn escape_closes_it_and_an_empty_path_opens_nothing(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        open_fake(&mut visual, "", fake(Ok(vec![]), Ok(vec![]), false).source);
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        open_fake(&mut visual, FILE, fake(Ok(vec![]), Ok(vec![]), false).source);
        visual.run_until_parked();
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            assert!(window.focused(cx).is_some());
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render_without_trouble(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let v = version(1, false, true, false);
        let fake = fake(Ok(vec![v.clone()]), Ok(vec![]), false);
        open_fake(&mut visual, "C:\\مستندات\\تقرير.docx", fake.source);
        settle(&mut visual, "the row", |cx| has(cx, &row_label(&v)));
        assert_eq!(role_of(&mut visual, &row_label(&v)), Some(Role::Group));
        testing::set_rtl(false);
    }
}
