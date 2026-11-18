//! A Git folder's README above its list.
//! Ported from src/components/explorer/ReadmeCard.tsx.
//!
//! | TypeScript                                   | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `ReadmeCard`, `readmeOf`                     | [`ReadmeCard`], [`words::readme_of`]                           |
//! | `MarkdownView` in the README                 | `components::preview::markdown::MarkdownPreview`               |
//! | `settings.readmeOpen`, `showReadme`          | `state::settings(cx).readme_open`, `show_readme`               |

use crate::components::preview::markdown::MarkdownPreview;
use crate::components::preview::text::TextSource;
use crate::kit::prelude::*;
use crate::{i18n, state, t, theme};
use glint_core::fsx::TextPreview as TextRead;
use glint_model::entries::FileEntry;
use glint_model::markdown::markdown_title;
use gpui_kit::Task;
use std::sync::Arc;

// ---- The README ---------------------------------------------------------------------------------------------

/// How much of a README is read (`readTextPreview(entry.path, 256 * 1024)`).
pub const README_BYTES: usize = 256 * 1024;

/// What a README read gave, kept to hand to the Markdown view when it is opened.
struct Read {
    text: String,
    truncated: bool,
    encoding: Option<String>,
    guessed: bool,
}

/// Where a README is read: the folder's text source in the app, a fake in tests.
fn core_source() -> TextSource {
    Arc::new(|path, encoding| Box::pin(glint_core::fsx::read_text_preview(path, README_BYTES, encoding)))
}

/// A source that hands back what was already read, so the Markdown view doesn't read the file twice.
fn once_read(read: &Read) -> TextSource {
    let (text, truncated, encoding, guessed) = (read.text.clone(), read.truncated, read.encoding.clone(), read.guessed);
    Arc::new(move |_, _| {
        let answer = Ok(TextRead {
            text: text.clone(),
            truncated,
            binary: false,
            encoding: encoding.clone(),
            guessed,
        });
        Box::pin(async move { answer })
    })
}

enum Load {
    /// Waiting for the file: the header shows, with no title yet.
    Reading,
    /// Binary, empty, or unreadable: no card.
    Hidden,
    Shown {
        title: String,
        read: Read,
        /// Made when the card is first opened: a folded README needs only its title.
        view: Option<Entity<MarkdownPreview>>,
    },
}

/// A folder's README above its list, folded to one line (its title) until it's opened, then as it reads; opened
/// or folded stays from folder to folder (`settings.readme_open`).
pub struct ReadmeCard {
    entry: FileEntry,
    source: TextSource,
    load: Load,
    generation: u64,
    _read: Task<()>,
}

impl ReadmeCard {
    /// The card of `entry` (a folder's README: see [`words::readme_of`]), read in the background.
    pub fn new(entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(entry, core_source(), window, cx)
    }

    /// [`ReadmeCard::new`] reading through `source` (tests pass fakes).
    pub fn with_source(entry: FileEntry, source: TextSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            entry,
            source,
            load: Load::Reading,
            generation: 0,
            _read: Task::ready(()),
        };
        this.read(window, cx);
        this
    }

    /// Shows another README, or reads this one again when it changed on disk (the effect's `[path, modified]`).
    pub fn set_entry(&mut self, entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        let changed = entry.path != self.entry.path || entry.modified != self.entry.modified;
        self.entry = entry;
        if changed {
            self.read(window, cx);
        }
    }

    /// The title the folded card shows, once the file is read.
    pub fn title(&self) -> Option<&str> {
        match &self.load {
            Load::Shown { title, .. } => Some(title),
            _ => None,
        }
    }

    /// The Markdown view of the README, once the card has been opened.
    #[cfg(test)]
    pub fn document(&self) -> Option<&Entity<MarkdownPreview>> {
        match &self.load {
            Load::Shown { view, .. } => view.as_ref(),
            _ => None,
        }
    }

    /// Whether the card draws anything: it doesn't for a README that is binary, empty or unreadable.
    #[cfg(test)]
    pub fn is_shown(&self) -> bool {
        !matches!(self.load, Load::Hidden)
    }

    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.load = Load::Reading;
        let asked = (self.source)(self.entry.path.clone(), None);
        self._read = cx.spawn_in(window, async move |this, cx| {
            let result = crate::core_bridge::run(asked).await.and_then(|inner| inner);
            this.update_in(cx, |this, _, cx| {
                if this.generation != generation {
                    return;
                }
                this.load = match result {
                    Ok(read) if !read.binary && !read.text.is_empty() => Load::Shown {
                        title: markdown_title(&read.text),
                        read: Read {
                            text: read.text,
                            truncated: read.truncated,
                            encoding: read.encoding,
                            guessed: read.guessed,
                        },
                        view: None,
                    },
                    _ => Load::Hidden,
                };
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    /// Folds or opens the README, for every folder (`setSettings({ readmeOpen: !open })`).
    fn toggle(&mut self, cx: &mut Context<Self>) {
        let open = state::settings(cx).readme_open;
        state::update(cx, |settings| settings.readme_open = !open);
        cx.notify();
    }
}

impl Render for ReadmeCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if matches!(self.load, Load::Hidden) {
            return div().into_any_element();
        }
        let t = theme::tokens(cx).clone();
        let open = state::settings(cx).readme_open;
        // The document is made the first time it is opened.
        if let (true, Load::Shown { read, view, .. }) = (open, &mut self.load) {
            if view.is_none() {
                let (path, source) = (self.entry.path.clone(), once_read(read));
                *view = Some(cx.new(|cx| MarkdownPreview::with_source(path, source, window, cx)));
            }
        }
        let title = self.title().filter(|title| !title.is_empty()).map(str::to_string);
        let name = self.entry.name.clone();
        let tip = if open {
            t!("Fold the README")
        } else {
            t!("Show the README")
        };

        let chevron = if open {
            IconName::ChevronDown
        } else if i18n::is_rtl() {
            IconName::ChevronLeft
        } else {
            IconName::ChevronRight
        };
        let toggle = row_start()
            .id("readme-toggle")
            .role(Role::Button)
            .aria_label(t!("README: {name}", name = name.clone()))
            .aria_description(tip.clone())
            .aria_expanded(open)
            .test_support()
            .tab_index(0)
            .flex_1()
            .min_w_0()
            .self_stretch()
            .items_center()
            .gap(space(1.5))
            .px(space(1.))
            .rounded(theme::size::RADIUS_SM)
            .cursor_pointer()
            .hover(|style| style.text_color(t.foreground))
            .focus_visible(|style| style.bg(t.accent))
            .tooltip({
                let tip = tip.clone();
                move |window, cx| crate::kit::tooltip(tip.clone(), window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)))
            .child(icon(chevron).size_3p5().flex_none().text_color(t.muted_foreground))
            .child(icon(IconName::BookOpen).size_3p5().flex_none().text_color(t.primary))
            .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(isolate(name)))
            .children(title.map(|title| {
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(t.muted_foreground)
                    .child(isolate(title))
            }));

        let hide_tip = t!("Don't show READMEs above the list (Settings → Files)");
        let hide = Button::new("readme-hide")
            .ghost()
            .xsmall()
            .icon(IconName::X)
            .tooltip(hide_tip)
            .accessibility_label(t!("Don't show READMEs"))
            .on_click(|_, _, cx| state::update(cx, |settings| settings.show_readme = false));

        let header = row_start()
            .h(space(8.))
            .flex_none()
            .items_center()
            .gap(space(1.))
            .ps(2.)
            .pe(1.5)
            .text_size(theme::size::XS)
            .child(toggle)
            .child(hide);

        // It opens to a share of the window (the TypeScript's pane was at most half the files' height and as tall
        // as the text): the document scrolls inside it.
        let height = (window.viewport_size().height * 0.4).max(space_px(10., cx));
        let document = match &self.load {
            Load::Shown { view: Some(view), .. } if open => Some(
                div()
                    .id("readme-document")
                    .h(height)
                    .min_h_0()
                    .overflow_hidden()
                    .bg(t.card)
                    .child(view.clone()),
            ),
            _ => None,
        };

        col()
            .id("readme-card")
            .role(Role::Group)
            .aria_label(t!("README"))
            .test_support()
            .flex_none()
            .w_full()
            .border_b_1()
            .border_color(t.border)
            .child(header)
            .children(document)
            .into_any_element()
    }
}

/// `n` rems in pixels at the text-size setting.
fn space_px(n: f32, cx: &App) -> Pixels {
    theme::rem_size(cx) * n
}
