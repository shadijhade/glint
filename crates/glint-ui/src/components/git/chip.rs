//! The branch a tab's folder is on, in the address bar ("main ↑2 ↓1"), when it's in a Git repository: a click shows
//! the repository, its upstream, what changed here, and Show changes (a `git:changed` filter).
//! Ported from src/components/explorer/GitChip.tsx.
//!
//! | TypeScript                                   | Here                                                       |
//! |----------------------------------------------|------------------------------------------------------------|
//! | `GitChip` (button and `popover="auto"`)      | [`GitChip`]: the button, and the popover under it          |
//! | `place` (below the chip, kept on screen)     | `anchored` under the chip's start edge, snapped to the window |
//! | `close` (`hidePopover`)                      | `GitChip::close`: Escape, a press outside, or any button in it |
//! | `copyText` and its check mark                | `GitChip::copy` and the 1.3 s check                         |
//! | `trustRepository`                            | [`super::trust_repository`]                                |
//! | `refresh()` (Show its status, F5)            | [`super::model::Git::refresh_forced`] for this tab          |
//! | `updateTab({ filter })`                      | `Explorer::set_filter` for this tab                        |
//! | `navigate(view.root)`                        | `Explorer::navigate` in this tab                           |

use super::model::{shared, Git, View};
use super::words::{self, Tone};
use crate::explorer::{explorer, Explorer, ExplorerEvent, NavOptions, TabId};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::paths::{base_name, same_path};
use gpui_kit::{
    anchored, canvas, deferred, point, Anchor, Bounds, ClickEvent, ClipboardItem, FocusHandle, HighlightStyle,
    KeyDownEvent, MouseDownEvent, Pixels, Point, StyledText,
};
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// How long the check mark stays after copying (`setTimeout(…, 1300)`).
const COPIED_FOR: Duration = Duration::from_millis(1300);

/// What was last copied from the popover, for the check mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Copied {
    Branch,
    Sha,
}

/// The branch chip of one tab. It draws nothing while the tab has no Git view of its folder. The coordinator
/// makes one per tab and puts it after the breadcrumbs (see [`super`]).
pub struct GitChip {
    tab: TabId,
    git: Entity<Git>,
    explorer: Entity<Explorer>,
    open: bool,
    /// Where the press that closed the popover happened: the chip's own click must not open it again.
    dismissed_at: Option<Point<Pixels>>,
    focus: FocusHandle,
    /// The chip's bounds, recorded as it is painted, to hang the popover from.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    copied: Option<(Copied, u64)>,
    copies: u64,
    _subscriptions: Vec<Subscription>,
}

impl GitChip {
    /// The chip of `tab`. Needs the explorer (`explorer::init`); makes the shared Git model if there is none.
    pub fn new(tab: TabId, cx: &mut Context<Self>) -> Self {
        let git = shared(cx);
        let explorer = explorer(cx);
        let subscriptions = vec![
            cx.observe(&git, |_, _, cx| cx.notify()),
            // The chip is for the tab's folder as it is now: it follows the path.
            cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| match event {
                ExplorerEvent::TabChanged(tab) | ExplorerEvent::ListingChanged { tab, .. } if *tab == this.tab => {
                    cx.notify()
                }
                ExplorerEvent::TabsChanged => cx.notify(),
                _ => {}
            }),
        ];
        Self {
            tab,
            git,
            explorer,
            open: false,
            dismissed_at: None,
            focus: cx.focus_handle(),
            bounds: Rc::new(Cell::new(Bounds::default())),
            copied: None,
            copies: 0,
            _subscriptions: subscriptions,
        }
    }

    /// The tab this chip is for.
    pub fn tab(&self) -> TabId {
        self.tab
    }

    /// The tab's view of its repository, when the chip shows: the view is for the folder the tab is in now
    /// (`samePath(run.folder, path)`), and whether status is still running.
    fn shown(&self, cx: &App) -> Option<(Arc<View>, bool)> {
        let run = self.git.read(cx).run(self.tab)?;
        let view = run.view.clone()?;
        let path = &self.explorer.read(cx).tab(self.tab)?.path;
        same_path(&run.folder, path).then_some((view, run.running))
    }

    /// Whether the chip draws anything now (the address bar leaves no gap for one that doesn't).
    pub fn is_shown(&self, cx: &App) -> bool {
        self.shown(cx).is_some()
    }

    /// Whether the popover is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens or closes the popover. The press that closed it is the press's own, so it doesn't open it again.
    fn toggle(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let closed_by_this_press = self.dismissed_at.take() == Some(at);
        if self.open {
            self.open = false;
        } else if !closed_by_this_press {
            self.open = true;
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Closes the popover (`close`).
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }

    /// Copies the branch's name or the commit's id and shows a check beside it for a moment (`copyText`).
    fn copy(&mut self, text: String, what: Copied, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copies += 1;
        let generation = self.copies;
        self.copied = Some((what, generation));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |this, cx| {
                if this.copied.is_some_and(|(_, g)| g == generation) {
                    this.copied = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Filters the tab's folder by a Git query (`updateTab({ filter })`) and closes the popover.
    fn filter(&mut self, query: &str, cx: &mut Context<Self>) {
        let tab = self.tab;
        self.explorer
            .update(cx, |explorer, cx| explorer.set_filter(query, Some(tab), cx));
        self.close(cx);
    }

    /// Opens the repository's own folder in this tab and closes the popover (`navigate(view.root)`).
    fn open_root(&mut self, root: &str, cx: &mut Context<Self>) {
        let tab = self.tab;
        self.explorer
            .update(cx, |explorer, cx| explorer.navigate(root, NavOptions::in_tab(tab), cx));
        self.close(cx);
    }

    /// Runs status now, where it waited to be asked (`refresh()` after "Show its status (F5)").
    fn show_status(&mut self, cx: &mut Context<Self>) {
        let tab = self.tab;
        self.git.update(cx, |git, cx| git.refresh_forced(tab, cx));
        self.close(cx);
    }

    /// Trusts the repository and shows its status (`trustRepository`).
    fn trust(&mut self, root: &str, cx: &mut Context<Self>) {
        let git = self.git.clone();
        let root = root.to_string();
        git.update(cx, |git, cx| git.trust_repository(&root, cx));
        self.close(cx);
    }
}

/// A small icon-and-label button in the popover's style: ghost, compact.
fn tinted(id: impl Into<ElementId>, tone: Tone, cx: &App) -> Button {
    let t = theme::tokens(cx);
    Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(tone.tint(0.1))
                .foreground(tone.pill_text(t.dark))
                .hover(tone.tint(0.2))
                .active(tone.tint(0.2)),
        )
        .compact()
}

/// Text with one stretch of it in a heavier weight (the upstream's name in "Tracks origin/main").
fn emphasised(text: SharedString, range: Option<Range<usize>>) -> StyledText {
    let styled = StyledText::new(text);
    match range {
        Some(range) => styled.with_highlights(vec![(
            range,
            HighlightStyle {
                font_weight: Some(FontWeight::MEDIUM),
                ..Default::default()
            },
        )]),
        None => styled,
    }
}

impl GitChip {
    /// The button in the address bar: the branch, the arrows, a shield when status waits, a spinner while it runs.
    fn button(&self, view: &View, running: bool, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let mut title = words::chip_title(view);
        if self.explorer.read(cx).is_dual() {
            // Two chips on screen (one per pane) are told apart by their pane.
            let pane = self.explorer.read(cx).pane_of(self.tab).map_or(1, |p| p + 1);
            title = t!("{title}, pane {n}", title = title, n = pane);
        }
        let arrow = |id: &'static str, icon_name: IconName, n: u32, tip: SharedString| {
            row_start()
                .id((id, self.tab.0 as usize))
                .items_center()
                .flex_none()
                .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
                .child(icon(icon_name).size_3())
                .child(words::count(n))
        };
        let name = isolate(words::name_of(view));
        let mut content = row_start()
            .items_center()
            .gap(space(1.))
            .child(icon(IconName::GitBranch).size_3p5().flex_none())
            .child(
                div()
                    .max_w(space(32.))
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(name),
            );
        if view.ahead > 0 {
            content = content.child(arrow(
                "git-ahead",
                IconName::ArrowUp,
                view.ahead,
                words::ahead_title(view),
            ));
        }
        if view.behind > 0 {
            content = content.child(arrow(
                "git-behind",
                IconName::ArrowDown,
                view.behind,
                words::behind_title(view),
            ));
        }
        if view.limited || view.on_request {
            content = content.child(icon(IconName::ShieldAlert).size_3().text_color(t.warning).flex_none());
        }
        if running {
            content = content.child(Spinner::new().xsmall().color(t.muted_foreground));
        }
        // The canvas only records where the chip is; the popover hangs from it when it opens.
        let bounds = self.bounds.clone();
        let probe = canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
            .absolute()
            .size_full();
        div().relative().flex_none().ms(1.).child(probe).child(
            Button::new(("git-chip", self.tab.0 as usize))
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(t.accent.opacity(0.))
                        .foreground(t.muted_foreground)
                        .hover(t.accent)
                        .active(t.accent),
                )
                .compact()
                .tooltip(title.clone())
                .accessibility_label(title)
                .on_click(cx.listener(|this, event: &ClickEvent, window, cx| this.toggle(event.position(), window, cx)))
                .child(content),
        )
    }

    /// The branch's name, which copies it, and the commit's id, which copies that.
    fn heading(&self, view: &View, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tab.0 as usize;
        let name = isolate(words::name_of(view));
        let branch_part = match view.branch.clone() {
            Some(branch) => {
                let tip = t!("Click to copy branch \"{branch}\"", branch = branch);
                let copied = self.copied.is_some_and(|(what, _)| what == Copied::Branch);
                Button::new(("git-copy-branch", tab))
                    .ghost()
                    .compact()
                    .tooltip(tip.clone())
                    .accessibility_label(tip)
                    .on_click(cx.listener(move |this, _, _, cx| this.copy(branch.clone(), Copied::Branch, cx)))
                    .child(
                        row_start()
                            .items_center()
                            .gap(space(1.))
                            .min_w_0()
                            .text_size(theme::size::SM)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(div().truncate().child(name))
                            .child(if copied {
                                icon(IconName::Check).size_3().text_color(t.success).flex_none()
                            } else {
                                icon(IconName::Copy).size_3().text_color(t.muted_foreground).flex_none()
                            }),
                    )
                    .into_any_element()
            }
            None => div()
                .min_w_0()
                .truncate()
                .text_size(theme::size::SM)
                .font_weight(FontWeight::SEMIBOLD)
                .child(name)
                .into_any_element(),
        };
        let sha_part = view.head.clone().filter(|_| view.branch.is_some()).map(|head| {
            let tip = t!("Click to copy commit SHA ({head})", head = head);
            let copied = self.copied.is_some_and(|(what, _)| what == Copied::Sha);
            let text = head.clone();
            Button::new(("git-copy-sha", tab))
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(t.muted.opacity(0.7))
                        .foreground(t.muted_foreground)
                        .hover(t.accent)
                        .active(t.accent),
                )
                .compact()
                .tooltip(tip.clone())
                .accessibility_label(tip)
                .on_click(cx.listener(move |this, _, _, cx| this.copy(text.clone(), Copied::Sha, cx)))
                .child(
                    row_start()
                        .items_center()
                        .gap(space(1.))
                        .font_family(theme::mono_font(cx))
                        .text_size(theme::size::XXS)
                        .children(copied.then(|| icon(IconName::Check).size_3().text_color(t.success)))
                        .child(head),
                )
        });
        row_start()
            .items_center()
            .gap(space(2.))
            .child(icon(IconName::GitBranch).size_4().text_color(t.primary).flex_none())
            .child(div().min_w_0().child(branch_part))
            .child(div().flex_1())
            .children(sha_part)
    }

    /// The bar of the kinds of change, and the pills under it; clicking either filters the folder to that kind.
    fn changes_bar(&self, view: &View, t: &theme::Tokens, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let parts = words::parts(&view.counts);
        let total: u32 = parts.iter().map(|p| p.count).sum();
        if parts.is_empty() || total == 0 {
            return None;
        }
        let segments = parts.iter().enumerate().map(|(i, part)| {
            let query = part.query;
            let tip = t!(
                "{label}: {n} — click to filter ({query})",
                label = part.label,
                n = words::count(part.count),
                query = part.query
            );
            div()
                .id(("git-segment", i))
                .h_full()
                .w(gpui_kit::relative(words::segment_share(part, total)))
                .bg(part.tone.bar())
                .cursor_pointer()
                .hover(|style| style.opacity(0.8))
                .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
                .on_click(cx.listener(move |this, _, _, cx| this.filter(query, cx)))
        });
        let pills = parts.iter().enumerate().map(|(i, part)| {
            let query = part.query;
            let tip = t!("Filter this folder by {query}", query = part.query);
            let label = t!("{n} {label}", n = words::count(part.count), label = part.label);
            tinted(("git-pill", i), part.tone, cx)
                .tooltip(tip)
                .accessibility_label(label.clone())
                .on_click(cx.listener(move |this, _, _, cx| this.filter(query, cx)))
                .child(
                    div()
                        .text_size(theme::size::XXS)
                        .font_weight(FontWeight::MEDIUM)
                        .child(label),
                )
        });
        Some(
            col()
                .gap(space(2.))
                .child(
                    row_start()
                        .h(space(1.5))
                        .w_full()
                        .overflow_hidden()
                        .rounded_full()
                        .bg(t.muted.opacity(0.6))
                        .children(segments),
                )
                .child(row_start().flex_wrap().gap(space(1.)).children(pills)),
        )
    }

    /// The popover: the repository, its upstream, what changed, and what to do about it.
    fn panel(&self, view: &View, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tab.0 as usize;
        let root = view.root.clone();
        let at_root = same_path(
            &view.root,
            self.explorer.read(cx).tab(self.tab).map_or("", |tab| &tab.path),
        );
        let repo_name = {
            let name = base_name(&view.root);
            if name.is_empty() {
                view.root.clone()
            } else {
                name.to_string()
            }
        };
        let (tracks, bold) = words::tracks_line(view);
        let changes = words::changes(&view.counts);

        let status: AnyElement = if view.limited {
            let trust_root = root.clone();
            col()
                .id("git-limited")
                .role(Role::Group)
                .aria_label(t!("Git status waits for trust"))
                .test_support()
                .mt(space(2.5))
                .p(space(2.5))
                .gap(space(2.))
                .rounded(theme::size::RADIUS)
                .bg(t.warning.opacity(0.1))
                .child(t!(
                    "This repository's own settings name programs for Git status to run (filter drivers, or core.fsmonitor), so Glint doesn't look at what changed in it. Trust it only if you trust where it came from."
                ))
                .child(
                    Button::new(("git-trust", tab))
                        .outline()
                        .small()
                        .icon(IconName::ShieldAlert)
                        .label(t!("Trust this repository"))
                        .on_click(cx.listener(move |this, _, _, cx| this.trust(&trust_root, cx))),
                )
                .into_any_element()
        } else if view.on_request {
            col()
                .id("git-on-request")
                .role(Role::Group)
                .aria_label(t!("Git status waits to be asked"))
                .test_support()
                .mt(space(2.5))
                .p(space(2.5))
                .gap(space(2.))
                .rounded(theme::size::RADIUS)
                .bg(t.muted)
                .child(t!(
                    "This repository has too many files (or is on another computer) to look through on every visit."
                ))
                .child(
                    Button::new(("git-status", tab))
                        .outline()
                        .small()
                        .icon(IconName::RotateCw)
                        .label(t!("Show its status (F5)"))
                        .on_click(cx.listener(|this, _, _, cx| this.show_status(cx))),
                )
                .into_any_element()
        } else {
            let line = if changes.is_empty() {
                t!("Nothing changed in this folder").to_string()
            } else {
                t!(
                    "{changes} in this folder and below",
                    changes = changes.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
                )
                .to_string()
            };
            let rest = format!(
                " · {}{}",
                glint_model::format::format_duration(view.elapsed),
                words::partial_note(view)
            );
            let split = line.len();
            let text: SharedString = format!("{line}{rest}").into();
            let muted = StyledText::new(text).with_highlights(vec![(
                split..split + rest.len(),
                HighlightStyle {
                    color: Some(t.muted_foreground),
                    ..Default::default()
                },
            )]);
            col()
                .id("git-changes")
                .role(Role::Group)
                .aria_label(t!("What changed"))
                .test_support()
                .mt(space(2.))
                .gap(space(2.))
                .child(muted)
                .children(self.changes_bar(view, t, cx))
                .into_any_element()
        };

        let show_changes = (!view.limited && !view.on_request && !changes.is_empty()).then(|| {
            Button::new(("git-show-changes", tab))
                .outline()
                .small()
                .icon(IconName::ListFilter)
                .label(t!("Show changes"))
                .tooltip(t!("Filter this folder to what changed (git:changed)"))
                .on_click(cx.listener(|this, _, _, cx| this.filter("git:changed", cx)))
        });
        let open_root = (!at_root).then(|| {
            let target = root.clone();
            Button::new(("git-open-root", tab))
                .ghost()
                .small()
                .icon(IconName::FolderGit2)
                .label(t!("Open the repository's folder"))
                .tooltip(SharedString::from(root.clone()))
                .on_click(cx.listener(move |this, _, _, cx| this.open_root(&target, cx)))
        });

        col()
            .id(("git-popover", tab))
            .role(Role::Group)
            .aria_label(t!("Git: {repository}", repository = repo_name.clone()))
            .test_support()
            .track_focus(&self.focus)
            .w(space(80.))
            .p(space(3.))
            .text_size(theme::size::XS)
            .text_start()
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .bg(t.popover)
            .text_color(t.popover_foreground)
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                this.dismissed_at = Some(event.position);
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close(cx);
                }
            }))
            .child(self.heading(view, t, cx))
            .child(
                div()
                    .id(("git-repository", tab))
                    .mt(space(1.))
                    .truncate()
                    .text_color(t.muted_foreground)
                    .tooltip({
                        let root = SharedString::from(root.clone());
                        move |window, cx| crate::kit::tooltip(root.clone(), window, cx)
                    })
                    .child(t!("In {name}", name = isolate(repo_name))),
            )
            .child(div().mt(space(2.)).child(emphasised(tracks, bold)))
            .child(status)
            .child(
                row_start()
                    .mt(space(2.5))
                    .flex_wrap()
                    .gap(space(1.5))
                    .children(show_changes)
                    .children(open_root),
            )
    }
}

impl Render for GitChip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((view, running)) = self.shown(cx) else {
            self.open = false;
            return div().into_any_element();
        };
        let t = theme::tokens(cx).clone();
        let button = self.button(&view, running, &t, cx);
        let popover = self.open.then(|| {
            // Under the chip, from its start edge, and kept on screen.
            let chip = self.bounds.get();
            let (position, corner) = if crate::i18n::is_rtl() {
                (point(chip.right(), chip.bottom() + gpui_kit::px(4.)), Anchor::TopRight)
            } else {
                (point(chip.left(), chip.bottom() + gpui_kit::px(4.)), Anchor::TopLeft)
            };
            deferred(
                anchored()
                    .position(position)
                    .anchor(corner)
                    .snap_to_window_with_margin(gpui_kit::px(4.))
                    .child(self.panel(&view, &t, cx)),
            )
            .with_priority(1)
        });
        div().flex_none().child(button).children(popover).into_any_element()
    }
}
