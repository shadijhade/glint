//! A Git repository's card in its folder's preview, and a folder's README above its list.
//! Ported from src/components/explorer/RepoCard.tsx and ReadmeCard.tsx.
//!
//! | TypeScript                                   | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `RepoCard`, the `read` cache                 | [`RepoCard`]; the cache is the Git model's (`cached_card`)     |
//! | `useTool("onefetch")`, `ToolNeeded`          | the shared [`Tools`] model and `settings::tools::needed`       |
//! | `COLORS`, `colorOf`                          | [`words::language_color`]                                      |
//! | `ReadmeCard`, `readmeOf`                     | [`ReadmeCard`], [`words::readme_of`]                           |
//! | `MarkdownView` in the README                 | `components::preview::markdown::MarkdownPreview`               |
//! | `settings.readmeOpen`, `showReadme`          | `state::settings(cx).readme_open`, `show_readme`               |

use super::model::{shared, Git, Source};
use super::words;
use crate::components::preview::markdown::MarkdownPreview;
use crate::components::preview::text::TextSource;
use crate::components::settings::tools::{needed, Tools};
use crate::kit::prelude::*;
use crate::{i18n, state, t, theme};
use glint_core::fsx::TextPreview as TextRead;
use glint_core::repo::RepoCard as Card;
use glint_model::entries::FileEntry;
use glint_model::format::{format_count, format_date, plural};
use glint_model::markdown::markdown_title;
use gpui_kit::Task;
use std::rc::Rc;
use std::sync::Arc;

// ---- The repository card ------------------------------------------------------------------------------------

/// The tools a card draws install cards for, and where each is now (`onefetch?.origin`, `tokei?.origin`): when
/// one is installed the card is read again.
type Origins = (Option<String>, Option<String>);

fn origins_of(tools: &Option<Entity<Tools>>, cx: &App) -> Origins {
    match tools {
        Some(tools) => {
            let tools = tools.read(cx);
            (
                tools.tool("onefetch").and_then(|t| t.origin.clone()),
                tools.tool("tokei").and_then(|t| t.origin.clone()),
            )
        }
        None => (None, None),
    }
}

/// A Git repository's card: what onefetch and tokei say of it, each installed on demand. It shows in the preview
/// of a folder that has a `.git`, and draws nothing for one that isn't a repository's own folder.
pub struct RepoCard {
    path: String,
    git: Entity<Git>,
    source: Rc<dyn Source>,
    /// `None` until the first answer (or the cache's); `Some(None)` when the folder has no card.
    card: Option<Option<Arc<Card>>>,
    tools: Option<Entity<Tools>>,
    origins: Origins,
    generation: u64,
    _read: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl RepoCard {
    /// The card of the repository whose folder is `path`. `tools` is the shared tools model (make it with
    /// `dialogs::action_run::tools_model`): without it no install cards are offered.
    pub fn new(path: String, tools: Option<Entity<Tools>>, cx: &mut Context<Self>) -> Self {
        let git = shared(cx);
        let source = git.read(cx).source();
        let mut subscriptions = Vec::new();
        if let Some(model) = &tools {
            // Read again once a tool is installed.
            subscriptions.push(cx.observe(model, |this, tools, cx| {
                let now = origins_of(&Some(tools), cx);
                if now != this.origins {
                    this.origins = now;
                    this.read(cx);
                }
            }));
        }
        let mut this = Self {
            card: git.read(cx).cached_card(&path),
            origins: origins_of(&tools, cx),
            path,
            git,
            source,
            tools,
            generation: 0,
            _read: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.read(cx);
        this
    }

    /// The folder the card is of.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another folder's card (the pane's selection moved).
    pub fn set_path(&mut self, path: String, cx: &mut Context<Self>) {
        if self.path == path {
            return;
        }
        self.card = self.git.read(cx).cached_card(&path);
        self.path = path;
        self.read(cx);
        cx.notify();
    }

    /// What was read: `None` while waiting, `Some(None)` for no card.
    pub fn card(&self) -> Option<&Option<Arc<Card>>> {
        self.card.as_ref()
    }

    /// Reads the card in the background; a folder previewed again shows its last card at once meanwhile.
    fn read(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let asked = self.source.card(self.path.clone());
        let path = self.path.clone();
        self._read = cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                let card = match result {
                    Ok(card) => {
                        let card = card.map(Arc::new);
                        this.git.update(cx, |git, _| git.remember_card(&path, card.clone()));
                        card
                    }
                    // A failed read shows nothing, and is not remembered.
                    Err(_) => None,
                };
                this.card = Some(card);
                cx.notify();
            })
            .ok();
        });
    }
}

impl Render for RepoCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(Some(card)) = self.card.clone() else {
            return div().into_any_element();
        };
        let t = theme::tokens(cx).clone();
        let shown: Vec<&(String, f64)> = card.languages.iter().take(6).collect();
        let authors: Vec<_> = card.authors.iter().take(3).collect();
        let lines: Vec<_> = card.lines.iter().take(3).collect();
        let site = card.url.as_deref().and_then(words::site_of);

        let mut body = col()
            .id("repo-card")
            .role(Role::Group)
            .aria_label(t!(
                "Repository: {name}",
                name = card.name.clone().unwrap_or_else(|| self.path.clone())
            ))
            .test_support()
            .w_full()
            .max_w(space(96.))
            .gap(space(2.))
            .p(space(3.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.card.opacity(0.6))
            .text_size(theme::size::XS)
            .text_start();

        if let (true, Some(name)) = (card.onefetch, card.name.as_ref()) {
            let pill = |text: SharedString, mono: bool| {
                div()
                    .flex_none()
                    .px(space(1.5))
                    .rounded(gpui_kit::px(4.))
                    .bg(t.muted)
                    .text_size(gpui_kit::rems(0.625))
                    .when(mono, |this| this.font_family(theme::mono_font(cx)))
                    .child(text)
            };
            body = body.child(
                row_start()
                    .items_center()
                    .gap(space(1.5))
                    .child(icon(IconName::GitBranch).size_3p5().flex_none().text_color(t.primary))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::size::SM)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.foreground)
                            .child(isolate(name.clone())),
                    )
                    .children(card.version.clone().map(|v| pill(v.into(), true)))
                    .children(card.license.clone().map(|l| pill(l.into(), false))),
            );
            if let Some(description) = &card.description {
                body = body.child(div().text_color(t.muted_foreground).child(isolate(description.clone())));
            }
            let mut facts: Vec<String> = Vec::new();
            facts.extend(card.branch.clone());
            facts.extend(card.head.clone());
            if let Some(commits) = card.commits {
                facts.push(match usize::try_from(commits) {
                    Ok(n) => plural(n, "commit"),
                    Err(_) => format!("{commits} commits"),
                });
            }
            if let Some(changed) = card.last_change.as_deref().and_then(words::parse_rfc3339) {
                facts.push(
                    t!(
                        "changed {date}",
                        date = format_date(changed, None, i18n::language()).to_lowercase()
                    )
                    .to_string(),
                );
            }
            body = body.child(div().text_color(t.muted_foreground).child(isolate(facts.join(" · "))));
            if !shown.is_empty() {
                let bar = row_start()
                    .id("repo-languages-bar")
                    .h(space(1.5))
                    .overflow_hidden()
                    .rounded_full()
                    .bg(t.muted)
                    .children(shown.iter().enumerate().map(|(i, (language, share))| {
                        div()
                            .h_full()
                            .w(gpui_kit::relative((*share as f32 / 100.).clamp(0., 1.)))
                            .bg(words::language_color(language, i))
                    }));
                let legend = row_start()
                    .id("repo-languages")
                    .role(Role::List)
                    .aria_label(t!("Languages"))
                    .test_support()
                    .flex_wrap()
                    .gap_x(space(2.5))
                    .gap_y(space(0.5))
                    .text_color(t.muted_foreground)
                    .children(shown.iter().enumerate().map(|(i, (language, share))| {
                        let text = format!("{language} {}", words::share_text(*share));
                        row_start()
                            .id(("repo-language", i))
                            .role(Role::ListItem)
                            .aria_label(text.clone())
                            .test_support()
                            .items_center()
                            .gap(space(1.))
                            .child(
                                div()
                                    .size_2()
                                    .flex_none()
                                    .rounded_full()
                                    .bg(words::language_color(language, i)),
                            )
                            .child(text)
                    }));
                body = body.child(col().gap(space(1.)).child(bar).child(legend));
            }
            if !authors.is_empty() {
                let mut line = authors
                    .iter()
                    .map(|a| format!("{} {}%", a.name, a.share))
                    .collect::<Vec<_>>()
                    .join(" · ");
                if let Some(contributors) = card.contributors {
                    let more = contributors.saturating_sub(authors.len() as u64);
                    if more > 0 {
                        line.push_str(&format!(
                            " · {}",
                            t!(
                                "{n} more",
                                n = format_count(usize::try_from(more).unwrap_or(usize::MAX))
                            )
                        ));
                    }
                }
                let tip = card
                    .authors
                    .iter()
                    .map(|a| {
                        t!(
                            "{name}: {n} commits",
                            name = a.name,
                            n = format_count(usize::try_from(a.commits).unwrap_or(usize::MAX))
                        )
                        .to_string()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                body = body.child(
                    div()
                        .id("repo-authors")
                        .truncate()
                        .text_color(t.muted_foreground)
                        .tooltip(move |window, cx| crate::kit::tooltip(tip.clone().into(), window, cx))
                        .child(isolate(line)),
                );
            }
        }

        if let Some(total) = card.total.as_ref().filter(|total| total.code > 0) {
            let code = format_count(usize::try_from(total.code).unwrap_or(usize::MAX));
            let mut text = t!("{n} lines of code", n = code).to_string();
            if !lines.is_empty() {
                text.push_str(" · ");
                text.push_str(
                    &lines
                        .iter()
                        .map(|l| {
                            format!(
                                "{} {}",
                                l.language,
                                format_count(usize::try_from(l.code).unwrap_or(usize::MAX))
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" · "),
                );
            }
            let tip = card
                .lines
                .iter()
                .map(|l| {
                    t!(
                        "{language}: {code} lines of code, {comments} of comments",
                        language = l.language,
                        code = format_count(usize::try_from(l.code).unwrap_or(usize::MAX)),
                        comments = format_count(usize::try_from(l.comments).unwrap_or(usize::MAX))
                    )
                    .to_string()
                })
                .collect::<Vec<_>>()
                .join("\n");
            body = body.child(
                div()
                    .id("repo-lines")
                    .text_color(t.muted_foreground)
                    .tooltip(move |window, cx| crate::kit::tooltip(tip.clone().into(), window, cx))
                    .child(text),
            );
        }

        if let (true, Some(site)) = (card.onefetch, site) {
            let address = site.clone();
            let shown_site = site.trim_start_matches("https://").to_string();
            let tip = t!("Open it in the browser");
            body = body.child(
                Button::new("repo-site")
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(t.accent.opacity(0.))
                            .foreground(t.primary)
                            .hover(t.accent)
                            .active(t.accent),
                    )
                    .compact()
                    .tooltip(tip)
                    .accessibility_label(shown_site.clone())
                    .on_click(move |_, _, cx| cx.open_url(&address))
                    .child(
                        row_start()
                            .items_center()
                            .gap(space(1.))
                            .max_w_full()
                            .child(div().truncate().child(shown_site))
                            .child(icon(IconName::ExternalLink).size_3().flex_none()),
                    ),
            );
        }

        if let Some(tools) = &self.tools {
            if !card.onefetch {
                body = body.children(needed(
                    tools,
                    "onefetch",
                    t!("onefetch says what this repository is: its languages, who works on it, when it last changed."),
                    IconName::GitBranch,
                    cx,
                ));
            }
            if !card.tokei {
                body = body.children(needed(
                    tools,
                    "tokei",
                    t!("tokei counts its lines of code, by language."),
                    IconName::Code,
                    cx,
                ));
            }
        }
        body.into_any_element()
    }
}

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

    /// The README shown.
    pub fn entry(&self) -> &FileEntry {
        &self.entry
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
    pub fn document(&self) -> Option<&Entity<MarkdownPreview>> {
        match &self.load {
            Load::Shown { view, .. } => view.as_ref(),
            _ => None,
        }
    }

    /// Whether the card draws anything: it doesn't for a README that is binary, empty or unreadable.
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
