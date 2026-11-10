//! Text and Markdown: the start of the file is read first to tell text from bytes, so the frame can do what
//! `TextPreview` and `MarkdownPreview` in FilePreview.tsx did around the highlighted text: show why a file can't be
//! read, say a file is empty, and send a file that isn't text to Windows' previewer (or its bytes).
//!
//! The text itself is the preview-text agent's ([`TextPreview`], [`MarkdownPreview`]); this view builds one of
//! them once the start of the file is known to be text.

use super::hex::HexPreview;
use super::markdown::MarkdownPreview;
use super::shell::ShellHost;
use super::sources::{sources, Probe};
use super::states::{big_icon, empty, Waiting};
use super::text::TextPreview;
use crate::dialogs::password::PasswordForm;
use crate::kit::prelude::*;
use glint_model::archives::needs_password;
use glint_model::entries::{is_archived, FileEntry};
use gpui_kit::Task;

/// What the file is read to be shown as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum As {
    Text,
    Markdown,
}

/// How the start of the file read.
pub enum Probed {
    /// Being read.
    Reading,
    /// Couldn't be read, with why (and the password form when that is the reason, for an archive).
    Failed(SharedString, Option<Entity<PasswordForm>>),
    /// Nothing in it.
    Empty,
    /// Not text: Windows' previewer for it over its bytes (a file in an archive: its bytes).
    Binary(AnyView),
    /// Text: the previewer.
    Text(AnyView),
}

/// A file read to be shown as text or Markdown.
pub struct TextProbe {
    entry: FileEntry,
    waiting: Entity<Waiting>,
    state: Probed,
    _task: Task<()>,
}

impl TextProbe {
    /// Reads the start of `entry`, then shows it as `kind` for `claimant` (the pane's or Quick Look's).
    pub fn new(entry: FileEntry, kind: As, claimant: u32, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let waiting = cx.new(|cx| Waiting::new(None, cx));
        let running = (sources(cx).probe)(&entry.path);
        let shown = entry.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let probe = running.await;
            this.update_in(cx, |this, window, cx| this.probed(kind, claimant, probe, window, cx))
                .ok();
        });
        TextProbe {
            entry: shown,
            waiting,
            state: Probed::Reading,
            _task: task,
        }
    }

    /// How it read.
    pub fn state(&self) -> &Probed {
        &self.state
    }

    /// The text view once built.
    pub fn view(&self) -> Option<&AnyView> {
        match &self.state {
            Probed::Text(view) => Some(view),
            _ => None,
        }
    }

    fn probed(
        &mut self,
        kind: As,
        claimant: u32,
        probe: Result<Probe, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = self.entry.clone();
        let path = entry.path.clone();
        self.state = match probe {
            Err(error) => {
                let form = (is_archived(&entry) && needs_password(&error))
                    .then(|| cx.new(|cx| PasswordForm::new(path, window, cx)));
                Probed::Failed(error.into(), form)
            }
            Ok(Probe { binary: true, .. }) => {
                let hex: AnyView = cx.new(|cx| HexPreview::new(path, window, cx)).into();
                // A file of a kind Glint doesn't know, that isn't text: Windows' previewer for it, if it has one,
                // or its bytes. (Markdown that isn't text is just its bytes.)
                if is_archived(&entry) || kind == As::Markdown {
                    Probed::Binary(hex)
                } else {
                    let host = cx.new(|cx| ShellHost::new(&entry, claimant, true, hex, cx));
                    Probed::Binary(host.into())
                }
            }
            Ok(Probe { empty: true, .. }) => Probed::Empty,
            Ok(_) => match kind {
                As::Text => Probed::Text(cx.new(|cx| TextPreview::new(path, window, cx)).into()),
                As::Markdown => Probed::Text(cx.new(|cx| MarkdownPreview::new(path, window, cx)).into()),
            },
        };
        cx.notify();
    }
}

impl Render for TextProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.state {
            Probed::Reading => self.waiting.clone().into_any_element(),
            Probed::Failed(error, form) => big_icon(
                &self.entry,
                Some(error.clone()),
                form.clone().map(IntoElement::into_any_element),
                cx,
            )
            .into_any_element(),
            Probed::Empty => empty(cx).into_any_element(),
            Probed::Binary(view) | Probed::Text(view) => view.clone().into_any_element(),
        }
    }
}
