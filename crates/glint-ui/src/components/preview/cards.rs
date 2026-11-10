//! The small previews that are a card in the middle of the pane: a folder's counts, an archive's summary, a file
//! that can't be read, a file that is only online, a folder Windows keeps closed (`FolderPreview`,
//! `ArchivePreview`, `LockedPreview` and the inline states of `EntryPreview` in FilePreview.tsx).

use super::sources::{sources, ArchiveFacts, Counts, Sources};
use super::states::{big_icon, centered, icon_and_text, sentence, Waiting};
use crate::components::git::RepoCard;
use crate::dialogs::action_run::tools_model;
use crate::dialogs::password::PasswordForm;
use crate::explorer::{self, ExplorerEvent, NavOptions};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::entries::{attr, FileEntry};
use glint_model::format::{format_count, format_size};
use glint_model::paths::{parent_path, same_path};
use gpui_kit::{Subscription, Task};

/// A state that is a line or two and an icon, with nothing to load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plain {
    /// A OneDrive-style placeholder: "This file is online-only. Opening it downloads it."
    OnlineOnly,
    /// In a folder Windows keeps closed to this account.
    Closed,
    /// Listed in an ncdu export: a name, a size and a date.
    ListOnly,
    /// The kind's icon, nothing else.
    Icon,
}

/// A [`Plain`] card for one file.
pub struct PlainCard {
    entry: FileEntry,
    plain: Plain,
}

impl PlainCard {
    pub fn new(entry: FileEntry, plain: Plain) -> Self {
        PlainCard { entry, plain }
    }
}

impl Render for PlainCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.plain {
            Plain::OnlineOnly => icon_and_text(
                IconName::Cloud,
                theme::tailwind("sky-500").unwrap_or_default(),
                "preview-online-only",
                t!("This file is online-only. Opening it downloads it."),
                None,
                cx,
            )
            .into_any_element(),
            Plain::Closed => icon_and_text(
                IconName::FolderLock,
                theme::tailwind(if theme::tokens(cx).dark { "amber-500" } else { "amber-600" }).unwrap_or_default(),
                "preview-closed",
                t!("Windows keeps the folder this file is in closed to your account, so it can't be previewed or opened. Get access to the folder first."),
                None,
                cx,
            )
            .into_any_element(),
            Plain::ListOnly => big_icon(
                &self.entry,
                Some(t!("This file is listed in an ncdu export, with its size and date. What's in it isn't there.")),
                None,
                cx,
            )
            .into_any_element(),
            Plain::Icon => big_icon(&self.entry, None, None, cx).into_any_element(),
        }
    }
}

/// An item's tint: Tailwind's amber, in the shade for the theme.
fn amber(cx: &App) -> Hsla {
    theme::tailwind(if theme::tokens(cx).dark {
        "amber-500"
    } else {
        "amber-600"
    })
    .unwrap_or_default()
}

/// `1 item`, `3 items` (the TypeScript's `${n} ${n === 1 ? "item" : "items"}`).
pub fn items(n: usize) -> String {
    format!("{} {}", format_count(n), if n == 1 { "item" } else { "items" })
}

// ---- A folder ---------------------------------------------------------------------------------------------

/// A folder: what is directly inside it, how big it is once measured, and for a repository's own folder its
/// card (`FolderPreview`).
pub struct FolderCard {
    entry: FileEntry,
    counts: Option<Result<Counts, String>>,
    /// A folder with a `.git` (not in the Recycle Bin) has the repository's card under its counts.
    repo: Option<Entity<RepoCard>>,
    _task: Task<()>,
    /// Folder sizes arriving (the explorer measures them) redraw the card.
    _sizes: Option<Subscription>,
}

impl FolderCard {
    pub fn new(entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let running = (sources(cx).counts)(&entry.path);
        let recycled = entry.attrs & attr::RECYCLED != 0;
        let task = cx.spawn_in(window, async move |this, cx| {
            let counts = running.await;
            this.update_in(cx, |this, window, cx| {
                // A repository's own folder has its .git (a file in a worktree): its card shows.
                if !recycled && matches!(&counts, Ok(counts) if counts.repo) {
                    let path = this.entry.path.clone();
                    let tools = tools_model(window, cx);
                    this.repo = Some(cx.new(|cx| RepoCard::new(path, tools, cx)));
                }
                this.counts = Some(counts);
                cx.notify();
            })
            .ok();
        });
        let sizes = explorer::try_explorer(cx).map(|explorer| {
            cx.subscribe(&explorer, |_, _, event: &ExplorerEvent, cx| {
                if matches!(
                    event,
                    ExplorerEvent::SizesChanged { .. }
                        | ExplorerEvent::TabChanged(_)
                        | ExplorerEvent::ListingChanged { .. }
                ) {
                    cx.notify();
                }
            })
        });
        FolderCard {
            entry,
            counts: None,
            repo: None,
            _task: task,
            _sizes: sizes,
        }
    }

    /// What is inside, once listed.
    pub fn counts(&self) -> Option<&Result<Counts, String>> {
        self.counts.as_ref()
    }

    /// The repository's card, for a repository's own folder.
    pub fn repo(&self) -> Option<&Entity<RepoCard>> {
        self.repo.as_ref()
    }

    /// The folder's size when it has been measured here (the explorer's folder sizes), else a Recycle Bin item's
    /// own, as Windows noted it (`sizes[parentPath(entry.path)]?.sizes.get(entry.name) ?? entry.size`); and
    /// whether "Calculate folder sizes" is offered (the folder is in the one on screen) and is running now.
    fn size(&self, cx: &App) -> (Option<u64>, bool, bool) {
        let recycled = self.entry.attrs & attr::RECYCLED != 0;
        let Some(explorer) = explorer::try_explorer(cx) else {
            return (recycled.then_some(self.entry.size), false, false);
        };
        let explorer = explorer.read(cx);
        let parent = parent_path(&self.entry.path);
        let measured = explorer.folder_size(&parent, &self.entry.name);
        (
            measured.or(recycled.then_some(self.entry.size)),
            same_path(&parent, explorer.current_path()),
            explorer.is_sizing(),
        )
    }
}

impl Render for FolderCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        // A folder in the Recycle Bin comes with its size, as Windows noted it.
        let (size, can_calculate, sizing) = self.size(cx);
        let mut card = centered(cx)
            .id("preview-folder")
            .role(Role::Group)
            .aria_label(isolate(self.entry.name.clone()))
            .test_support()
            .child(
                icon(IconName::FolderOpen)
                    .size(px(64.))
                    .flex_none()
                    .text_color(theme::tailwind("amber-500").unwrap_or_default()),
            );
        match &self.counts {
            None => {}
            Some(Err(error)) => card = card.child(sentence(error.clone())),
            Some(Ok(counts)) => {
                let what = if counts.folders + counts.files == 0 {
                    t!("Empty").to_string()
                } else {
                    let mut parts = Vec::new();
                    if counts.folders > 0 {
                        parts.push(glint_model::format::plural(counts.folders, "folder"));
                    }
                    if counts.files > 0 {
                        parts.push(glint_model::format::plural(counts.files, "file"));
                    }
                    parts.join(", ")
                };
                card = card.child(
                    col()
                        .child(line("preview-folder-counts", what))
                        .when(counts.files > 0, |c| {
                            c.child(div().text_size(theme::size::XS).child(line(
                                "preview-folder-bytes",
                                t!("{size} in files directly inside", size = format_size(counts.bytes)),
                            )))
                        }),
                );
            }
        }
        if let Some(size) = size {
            card = card.child(
                div()
                    .text_color(t.foreground)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(line(
                        "preview-folder-total",
                        t!("{size} in total", size = format_size(size)),
                    )),
            );
        } else if can_calculate {
            // Sized by the index or pdu, for every folder here at once; while it runs the button waits.
            card = card.child(
                Button::new("preview-folder-sizes")
                    .outline()
                    .small()
                    .icon(if sizing {
                        IconName::LoaderCircle
                    } else {
                        IconName::HardDrive
                    })
                    .label(t!("Calculate folder sizes"))
                    .accessibility_label(t!("Calculate folder sizes"))
                    .disabled(sizing)
                    .on_click(|_, _, cx| {
                        explorer::explorer(cx).update(cx, |explorer, cx| explorer.calc_sizes(cx));
                    }),
            );
        }
        card.children(self.repo.clone())
    }
}

// ---- An archive -------------------------------------------------------------------------------------------

/// An archive: what's in it, from its list of entries (`ArchivePreview`).
pub struct ArchiveCard {
    entry: FileEntry,
    facts: Option<Result<ArchiveFacts, String>>,
    _task: Task<()>,
}

impl ArchiveCard {
    pub fn new(entry: FileEntry, cx: &mut Context<Self>) -> Self {
        let running = (sources(cx).archive)(&entry.path);
        let task = cx.spawn(async move |this, cx| {
            let facts = running.await;
            this.update(cx, |this, cx| {
                this.facts = Some(facts);
                cx.notify();
            })
            .ok();
        });
        ArchiveCard {
            entry,
            facts: None,
            _task: task,
        }
    }

    /// What it holds, once read.
    pub fn facts(&self) -> Option<&Result<ArchiveFacts, String>> {
        self.facts.as_ref()
    }
}

impl Render for ArchiveCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let path = self.entry.path.clone();
        let summary = match &self.facts {
            None => None,
            Some(Err(error)) => Some(sentence(error.clone()).into_any_element()),
            Some(Ok(facts)) => Some(match (facts.entries, facts.unpacked) {
                // A big compressed tar keeps no list of what's in it: it's read when opened, not on selection.
                (None, _) | (_, None) => col()
                    .max_w(rems(24.))
                    .child(line(
                        "preview-archive-packed",
                        t!(
                            "{size} as a {format} file",
                            size = format_size(facts.packed),
                            format = facts.format.clone()
                        ),
                    ))
                    .child(div().text_size(theme::size::XS).child(line(
                        "preview-archive-note",
                        t!("Opening it reads it from start to end once, to list what's in it."),
                    )))
                    .into_any_element(),
                (Some(entries), Some(unpacked)) => col()
                    .child(line("preview-archive-entries", entries_text(entries)))
                    .child(
                        col()
                            .text_color(t.foreground)
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(line(
                                "preview-archive-unpacked",
                                t!("{size} when extracted", size = format_size(unpacked)),
                            )))
                            .child(
                                div()
                                    .text_size(theme::size::XS)
                                    .text_color(t.muted_foreground)
                                    .child(line(
                                        "preview-archive-packed",
                                        t!(
                                            "{size} as a {format} file",
                                            size = format_size(facts.packed),
                                            format = facts.format.clone()
                                        ),
                                    )),
                            ),
                    )
                    .into_any_element(),
            }),
        };
        centered(cx)
            .id("preview-archive")
            .role(Role::Group)
            .aria_label(isolate(self.entry.name.clone()))
            .test_support()
            .child(
                icon(IconName::FileArchive)
                    .size(px(64.))
                    .flex_none()
                    .text_color(amber(cx)),
            )
            .children(summary)
            .child(
                Button::new("preview-archive-open")
                    .outline()
                    .small()
                    .icon(IconName::FolderOpen)
                    .label(t!("Open like a folder"))
                    .accessibility_label(t!("Open like a folder"))
                    .on_click(move |_, _, cx| {
                        let path = path.clone();
                        explorer::explorer(cx)
                            .update(cx, |explorer, cx| explorer.navigate(path, NavOptions::default(), cx));
                    }),
            )
    }
}

/// A line of a card, named for the accessibility tree (`<p>`).
fn line(id: &'static str, text: impl Into<SharedString>) -> impl IntoElement {
    let text: SharedString = text.into();
    div()
        .id(id)
        .role(Role::Status)
        .aria_label(text.clone())
        .test_support()
        .child(text)
}

fn entries_text(entries: usize) -> String {
    if entries == 1 {
        t!("{n} file or folder", n = format_count(entries)).to_string()
    } else {
        t!("{n} files and folders", n = format_count(entries)).to_string()
    }
}

// ---- A file in an archive that can't be read --------------------------------------------------------------

/// A file in an archive that can't be read: password-protected, or why not as the archive says (a method, a
/// volume or cabinet that isn't there, a link UnRAR doesn't pass on…), asked of the backend, which knows
/// (`LockedPreview`).
pub struct LockedCard {
    encrypted: bool,
    reason: Option<SharedString>,
    form: Option<Entity<PasswordForm>>,
    waiting: Entity<Waiting>,
    _task: Option<Task<()>>,
}

impl LockedCard {
    pub fn new(entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let encrypted = entry.attrs & attr::ENCRYPTED != 0;
        let sources: Sources = sources(cx);
        let waiting = cx.new(|cx| Waiting::new(None, cx));
        let form = encrypted.then(|| {
            let path = entry.path.clone();
            cx.new(|cx| PasswordForm::new(path, window, cx))
        });
        let task = (!encrypted).then(|| {
            let running = (sources.why_locked)(&entry.path);
            cx.spawn(async move |this, cx| {
                let reason = running.await;
                this.update(cx, |this, cx| {
                    this.reason =
                        Some(reason.map_or_else(|| t!("Glint can't read this file from the archive."), Into::into));
                    cx.notify();
                })
                .ok();
            })
        });
        LockedCard {
            encrypted,
            reason: None,
            form,
            waiting,
            _task: task,
        }
    }

    /// The line shown, once there is one.
    pub fn sentence(&self) -> Option<SharedString> {
        if self.encrypted {
            Some(t!("This file is password-protected."))
        } else {
            self.reason.clone()
        }
    }
}

impl Render for LockedCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(line) = self.sentence() else {
            return self.waiting.clone().into_any_element();
        };
        centered(cx)
            .id("preview-locked")
            .role(Role::Group)
            .aria_label(line.clone())
            .test_support()
            .child(icon(IconName::FileLock).size(px(56.)).flex_none().text_color(amber(cx)))
            .child(sentence(line))
            .children(
                self.form
                    .clone()
                    .map(|form| div().w_full().flex().justify_center().child(form)),
            )
            .into_any_element()
    }
}
