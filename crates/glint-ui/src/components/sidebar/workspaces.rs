//! Named workspaces in the window: the whole layout (tabs and their folders, the dual-pane split and its ratio,
//! each tab's view, where each list was scrolled) kept under a name and put back with one choice. Not in the
//! TypeScript (they came with 0.16).
//!
//! The layout is the session that already remembers the tabs between runs, taken by name
//! ([`Explorer::workspace_layout`]) and kept in the saved state ([`glint_model::store::Workspace`]), so there is
//! nothing to run in the background: saving and switching are single choices. Switching keeps the tabs that
//! already show a wanted folder ([`Explorer::apply_workspace`]), so folders open in both layouts are not read again.
//!
//! Where it is offered: the **Workspaces** button at the top of the sidebar (a menu: one row per workspace, Save
//! current layout as…, Rename, Delete) and Ctrl+K ("Save workspace as…", "Workspace: name", "Rename workspace:
//! name", "Delete workspace: name"). Both call the functions here.

use super::*;
use crate::components::menus::{Entry, Node};
use crate::components::tab_strip::menu as tab_menu;
use glint_model::store::Workspace;
use std::rc::Rc;

/// One workspace as the menus and the palette list it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub id: String,
    pub name: String,
    /// What is on screen now is this layout.
    pub current: bool,
}

/// The saved workspaces, in the order they were made, and which one the window is showing now (if any).
pub fn listed(cx: &App) -> Vec<Listed> {
    let Some(app) = persistence::app(cx) else {
        return Vec::new();
    };
    let layout = layout_now(cx);
    app.workspaces
        .iter()
        .map(|w| Listed {
            id: w.id.clone(),
            name: w.name.clone(),
            current: layout.as_ref().is_some_and(|now| same_layout(w, now)),
        })
        .collect()
}

/// The window's layout now, as a workspace without a name.
fn layout_now(cx: &App) -> Option<Workspace> {
    let (session, scrolls) = explorer(cx).read(cx).workspace_layout();
    Some(Workspace {
        session,
        scrolls,
        dual_split: state::settings(cx).dual_split,
        ..Default::default()
    })
}

/// Whether `saved` is what `now` shows: the same tabs, folders, views, panes and split. Where lists were
/// scrolled doesn't tell workspaces apart.
fn same_layout(saved: &Workspace, now: &Workspace) -> bool {
    saved.session == now.session && saved.dual_split == now.dual_split
}

/// Keeps the window's layout under `name` (a name already used is replaced). The id, or `None` for a name with
/// nothing in it or when there are too many already.
pub fn save_as(name: &str, cx: &mut App) -> Option<String> {
    let layout = layout_now(cx)?;
    let mut saved = None;
    persistence::update_app(cx, |app| saved = app.save_workspace(name, layout));
    saved
}

/// Puts a saved workspace on screen: its split first, then its tabs. `false` if it is gone or empty.
pub fn switch_to(id: &str, cx: &mut App) -> bool {
    let Some(workspace) = persistence::app(cx).and_then(|app| app.workspace(id)).cloned() else {
        return false;
    };
    let shown = explorer(cx).update(cx, |e, cx| e.apply_workspace(&workspace, cx));
    if shown {
        state::update(cx, |settings| settings.dual_split = workspace.dual_split);
    }
    shown
}

/// Gives a workspace another name. `false` if the name is empty or another workspace has it.
pub fn rename(id: &str, name: &str, cx: &mut App) -> bool {
    let mut done = false;
    persistence::update_app(cx, |app| done = app.rename_workspace(id, name));
    done
}

/// Forgets a workspace (the tabs on screen stay as they are).
pub fn delete(id: &str, cx: &mut App) -> bool {
    let mut done = false;
    persistence::update_app(cx, |app| done = app.delete_workspace(id));
    done
}

fn name_of(id: &str, cx: &App) -> Option<String> {
    persistence::app(cx)
        .and_then(|app| app.workspace(id))
        .map(|w| w.name.clone())
}

// ---- The words ----------------------------------------------------------------------------------------

/// "Save workspace as…": asks for a name, then keeps the layout under it.
pub fn ask_save_as(window: &mut Window, cx: &mut App) {
    let taken = names(cx, None);
    let ask = Ask {
        title: t!("Save workspace as…"),
        initial: String::new(),
        ok: t!("Save"),
        taken,
        when_taken: Taken::Replaces,
    };
    ask_name(window, cx, ask, move |name, window, cx| match save_as(&name, cx) {
        Some(_) => window.push_notification(Notification::success(t!("Saved workspace {name}", name = name)), cx),
        None => window.push_notification(
            Notification::error(t!("There are too many workspaces: delete one first"))
                .title(t!("Couldn't save the workspace")),
            cx,
        ),
    });
}

/// "Rename workspace…": asks for another name.
pub fn ask_rename(window: &mut Window, cx: &mut App, id: String) {
    let Some(current) = name_of(&id, cx) else {
        return;
    };
    let taken = names(cx, Some(&id));
    let ask = Ask {
        title: t!("Rename workspace “{name}”", name = current.clone()),
        initial: current,
        ok: t!("Rename"),
        taken,
        when_taken: Taken::Refuses,
    };
    ask_name(window, cx, ask, move |name, window, cx| {
        if !rename(&id, &name, cx) {
            window.push_notification(
                Notification::error(t!("Another workspace has that name")).title(t!("Couldn't rename the workspace")),
                cx,
            );
        }
    });
}

/// "Delete workspace…": asks once, then forgets it.
pub fn ask_delete(window: &mut Window, cx: &mut App, id: String) {
    let Some(name) = name_of(&id, cx) else {
        return;
    };
    crate::dialogs::confirm(
        window,
        cx,
        t!("Delete workspace “{name}”?", name = name.clone()),
        t!("Its tabs and layout are forgotten. The folders and the tabs on screen are not touched."),
        t!("Delete"),
        true,
        move |_, cx| {
            delete(&id, cx);
        },
    );
}

/// "Workspace: name": switches, and says so (or that it could not).
pub fn switch_and_say(window: &mut Window, cx: &mut App, id: &str) {
    let name = name_of(id, cx).unwrap_or_default();
    if switch_to(id, cx) {
        window.push_notification(
            Notification::success(t!("Switched to workspace {name}", name = name)),
            cx,
        );
    } else {
        window.push_notification(
            Notification::error(t!("That workspace is gone or has no tabs")).title(t!("Couldn't switch")),
            cx,
        );
    }
}

fn names(cx: &App, except: Option<&str>) -> Vec<String> {
    persistence::app(cx)
        .map(|app| {
            app.workspaces
                .iter()
                .filter(|w| Some(w.id.as_str()) != except)
                .map(|w| w.name.to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

// ---- Asking for a name --------------------------------------------------------------------------------

/// What a name another workspace has means.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Taken {
    /// Saving: it takes that workspace's place.
    Replaces,
    /// Renaming: it can't.
    Refuses,
}

type Chosen = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// The box that asks for a workspace's name.
struct NameDialog {
    input: Entity<InputState>,
    ok: SharedString,
    /// The other workspaces' names, in lower case.
    taken: Vec<String>,
    when_taken: Taken,
    chosen: Chosen,
    _subscription: Subscription,
}

/// What a name dialog says and checks.
struct Ask {
    title: SharedString,
    initial: String,
    /// The button that takes the name.
    ok: SharedString,
    /// The other workspaces' names, in lower case.
    taken: Vec<String>,
    when_taken: Taken,
}

fn ask_name(window: &mut Window, cx: &mut App, ask: Ask, chosen: impl Fn(String, &mut Window, &mut App) + 'static) {
    let chosen: Chosen = Rc::new(chosen);
    let title = ask.title.clone();
    let view = cx.new(|cx| NameDialog::new(ask, chosen, window, cx));
    let input = view.read(cx).input.clone();
    crate::dialogs::show(window, cx, title, px(420.), view);
    window.focus(&input.focus_handle(cx), cx);
}

impl NameDialog {
    fn new(ask: Ask, chosen: Chosen, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let Ask {
            initial,
            ok,
            taken,
            when_taken,
            ..
        } = ask;
        let has_initial = !initial.is_empty();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(t!("Work, Photos, Release"));
            state.set_value(initial, window, cx);
            if has_initial {
                state.select_all(window, cx);
            }
            state
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.accept(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        NameDialog {
            input,
            ok,
            taken,
            when_taken,
            chosen,
            _subscription: subscription,
        }
    }

    /// The name as typed: the words with single spaces between, at most a line long.
    fn typed(&self, cx: &App) -> String {
        glint_model::store::clean_workspace_name(&self.input.read(cx).value())
    }

    /// Whether the name can be taken.
    fn allowed(&self, cx: &App) -> bool {
        let name = self.typed(cx);
        !name.is_empty() && !(self.when_taken == Taken::Refuses && self.taken.contains(&name.to_lowercase()))
    }

    fn accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.allowed(cx) {
            return;
        }
        let (name, chosen) = (self.typed(cx), self.chosen.clone());
        window.close_dialog(cx);
        chosen(name, window, cx);
    }
}

impl Render for NameDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let name = self.typed(cx);
        let used = !name.is_empty() && self.taken.contains(&name.to_lowercase());
        let hint = match (used, self.when_taken) {
            (true, Taken::Replaces) => Some((t!("A workspace with this name will be replaced"), t.muted_foreground)),
            (true, Taken::Refuses) => Some((t!("Another workspace has this name"), t.destructive)),
            _ => None,
        };
        col()
            .id("workspace-name")
            .role(Role::Group)
            .aria_label(t!("Name the workspace"))
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
                        "A workspace keeps the tabs and their folders, the two panes and their split, each tab's view and where its list was scrolled."
                    )),
            )
            .child(Input::new(&self.input).aria_label(t!("Workspace name")))
            .children(hint.map(|(line, color)| {
                div()
                    .id("workspace-name-hint")
                    .role(Role::Alert)
                    .aria_label(line.clone())
                    .test_support()
                    .text_size(theme::size::XS)
                    .text_color(color)
                    .child(line)
            }))
            .child(
                row_start()
                    .justify_end()
                    .gap(space(2.))
                    .child(
                        Button::new("workspace-cancel")
                            .ghost()
                            .label(t!("Cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("workspace-ok")
                            .primary()
                            .label(self.ok.clone())
                            .disabled(!self.allowed(cx))
                            .on_click(cx.listener(|this, _, window, cx| this.accept(window, cx))),
                    ),
            )
    }
}

// ---- The button and its menu ----------------------------------------------------------------------------

impl Sidebar {
    /// The Workspaces selector row at the top of the sidebar, below a subtle hairline.
    pub(super) fn draw_workspaces(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let rtl = crate::i18n::is_rtl();
        let active = listed(cx).into_iter().find(|w| w.current);
        let shown: SharedString = active
            .as_ref()
            .map(|w| SharedString::from(w.name.clone()))
            .unwrap_or_else(|| t!("Workspaces"));
        let open = self.workspace_menu.is_some();
        let bounds = self.workspace_bounds.clone();
        let click_bounds = bounds.clone();
        let measure = canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
            .absolute()
            .inset_0();
        let button = row_start()
            .id("workspaces")
            .role(Role::Button)
            .aria_label(t!("Workspaces"))
            .aria_description(shown.clone())
            .aria_expanded(open)
            .test_support()
            .tab_index(0)
            .relative()
            .w_full()
            .h(space(7.5))
            .px(space(2.))
            .gap(space(2.))
            .rounded(theme::size::RADIUS_ROW)
            .border_1()
            .border_color(if open { t.stroke } else { gpui_kit::transparent_black() })
            .when(open, |b| b.bg(t.accent))
            .cursor_pointer()
            .text_size(theme::size::SM)
            .text_color(if active.is_some() {
                t.foreground
            } else {
                t.muted_foreground
            })
            .when(active.is_some(), |b| b.font_weight(FontWeight::MEDIUM))
            .hover(|s| s.bg(t.accent).text_color(t.foreground))
            .active(|s| s.bg(t.pressed))
            .focus_visible(|s| s.border_color(t.ring))
            .tooltip(tab_menu::tip(t!(
                "Save the tabs and panes as a workspace, or switch to one"
            )))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                let b = click_bounds.get();
                let at = if b.size.width > px(0.) && b.size.height > px(0.) {
                    point(if rtl { b.right() } else { b.left() }, b.bottom() + px(2.))
                } else {
                    event.position()
                };
                this.open_workspaces_menu(at, window, cx);
            }))
            .child(measure)
            .child(
                icon(IconName::Layers)
                    .size_4()
                    .flex_none()
                    .text_color(if active.is_some() {
                        t.primary
                    } else {
                        t.muted_foreground
                    }),
            )
            .child(div().flex_1().min_w_0().truncate().child(isolate(shown)))
            .child(
                icon(IconName::ChevronsUpDown)
                    .size_3p5()
                    .flex_none()
                    .text_color(t.muted_foreground),
            );
        let button = crate::components::hop::mark(button, || {
            let view = cx.entity();
            let bounds = self.workspace_bounds.clone();
            crate::components::hop::Mark::new(
                crate::components::hop::Kind::Place,
                cx.entity_id().as_u64(),
                t!("Workspaces"),
                move |window, cx| {
                    let b = bounds.get();
                    let at = if b.size.width > px(0.) && b.size.height > px(0.) {
                        point(if rtl { b.right() } else { b.left() }, b.bottom() + px(2.))
                    } else {
                        point(px(12.), px(64.))
                    };
                    view.update(cx, |this, cx| this.open_workspaces_menu(at, window, cx));
                },
            )
        });
        div()
            .w_full()
            .pt(space(1.5))
            .mt(space(0.5))
            .border_t_1()
            .border_color(t.stroke)
            .pb(space(1.))
            .child(button)
    }

    /// Opens the Workspaces menu at `at`.
    pub(super) fn open_workspaces_menu(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let all = listed(cx);
        let mut nodes: Vec<Option<Node>> = Vec::new();
        if all.is_empty() {
            nodes.push(Entry::note(t!("No workspaces yet"), IconName::Layers).node());
        }
        for w in &all {
            let mut row = tab_menu::row(
                SharedString::from(w.name.clone()),
                IconName::Layers,
                "switch",
                w.id.clone(),
            );
            row.icon = None;
            row.hint = persistence::app(cx).and_then(|app| app.workspace(&w.id)).map(|ws| {
                let panes = if ws.session.dual {
                    2usize.min(ws.session.panes.len().max(1))
                } else {
                    1
                };
                let tabs: usize = ws.session.panes.iter().take(panes).map(|p| p.paths.len()).sum();
                SharedString::from(format!(
                    "{} · {}",
                    glint_model::format::plural(tabs, "tab"),
                    glint_model::format::plural(panes, "pane")
                ))
            });
            nodes.push(row.checked(w.current).node());
        }
        nodes.push(Some(Node::Separator));
        nodes.push(tab_menu::row(t!("Save current layout as…"), IconName::Plus, "save", "").node());
        if !all.is_empty() {
            let each = |tag: &str| -> Vec<Option<Node>> {
                all.iter()
                    .map(|w| {
                        let mut row =
                            tab_menu::row(SharedString::from(w.name.clone()), IconName::Layers, tag, w.id.clone());
                        row.icon = None;
                        row.node()
                    })
                    .collect()
            };
            nodes.push(Some(Node::menu(
                t!("Rename workspace"),
                IconName::PenLine,
                each("rename"),
            )));
            nodes.push(Some(Node::menu(
                t!("Delete workspace"),
                IconName::Trash,
                each("delete"),
            )));
        }
        self.portal_menu = None;
        self.workspace_menu = tab_menu::Popup::open(
            nodes.into_iter().flatten().collect(),
            t!("Workspaces"),
            at,
            window,
            cx,
            |this, chose, window, cx| {
                this.workspace_menu = None;
                window.focus(&this.focus, cx);
                cx.notify();
                let Some(chose) = chose else { return };
                match chose.tag.as_str() {
                    "switch" => switch_and_say(window, cx, &chose.arg),
                    "save" => ask_save_as(window, cx),
                    "rename" => ask_rename(window, cx, chose.arg),
                    "delete" => ask_delete(window, cx, chose.arg),
                    _ => {}
                }
            },
        );
        cx.notify();
    }
}
