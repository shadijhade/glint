//! The box a tile shows while it is being renamed (`RenameInput` in FileList.tsx, placed by FileGrid.tsx in
//! place of the tile's name).
//!
//! It holds the name with everything but the extension selected, as File Explorer does. Enter (or leaving the
//! box: a click elsewhere) renames, Escape cancels, and a character no Windows name can hold is refused with the
//! hint. Committing is the explorer's rename part ([`Explorer::commit_rename_in`]): this only says what was
//! typed. The box is a small multi-line editor, so a long name wraps over the tile's two lines as its label
//! does; Enter still commits, and Shift+Enter adds nothing (a name has no line breaks).
//!
//! The box lives in the grid, not in the tile: tiles are drawn again every frame (and move to other places as
//! the grid reflows), but the box and its text stay, and each frame's tile for the item draws the same box.

use super::tile::Handler;
use super::{keys, FileGrid};
use crate::actions::{OpenInNewTab, OpenItem, Properties, Rename};
use crate::components::file_list::selection_end;
use crate::kit::prelude::*;
use crate::kit::{Textarea, TextareaState};
use crate::{t, theme};

/// The rename box of the tile being renamed.
pub(super) struct RenameBox {
    /// The item being renamed.
    pub path: String,
    /// What is typed.
    pub state: Entity<TextareaState>,
    /// Set once the box has said what it has to say, so leaving it after Enter doesn't say it twice.
    done: bool,
    _subscription: Subscription,
}

/// What a frame's tile needs to draw the box: which item it is for, its text, and where to report to.
#[derive(Clone)]
pub(super) struct RenameView {
    pub path: String,
    pub state: Entity<TextareaState>,
    /// Enter (or a click elsewhere) says `true`, Escape `false`.
    pub on_finish: Handler<bool>,
}

impl FileGrid {
    /// Opens the rename box for `path`, with its name typed in and selected without the extension.
    fn start_rename(&mut self, path: &str, name: &str, dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = name.to_string();
        // Two lines at most, like the label it replaces: a longer name scrolls inside the box.
        let state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 2)
                .submit_on_enter(true)
                .default_value(text)
        });
        let subscription = cx.subscribe_in(
            &state,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                // Shift+Enter has added a line break, which no name has: it comes out again, and the box stays.
                InputEvent::PressEnter { shift: true, .. } => drop_line_breaks(input, window, cx),
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.finish_rename(true, cx),
                _ => {}
            },
        );
        self.rename = Some(RenameBox {
            path: path.to_string(),
            state: state.clone(),
            done: false,
            _subscription: subscription,
        });
        let end = selection_end(name, dir);
        // Focus and selection after this frame: both change the window's focus, which isn't done mid-render.
        cx.defer_in(window, move |_, window, cx| {
            state.update(cx, |input, cx| {
                input.focus(window, cx);
                input.set_selected_range(0..end, cx);
            });
        });
    }

    /// Drops the rename box, and gives the keyboard back to the grid if the box held it: the input that had the
    /// focus is gone with the box, and with nothing focused Ctrl+L, F5 and the buttons that dispatch an action
    /// would do nothing until a click. A box closed because the person went to another control (the address
    /// bar, a dialog) leaves the focus where they put it.
    fn end_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else { return };
        let input = rename.state.read(cx).focus_handle(cx);
        // After this frame: the focus isn't changed mid-render.
        cx.defer_in(window, move |this, window, cx| {
            let holds_nothing = match window.focused(cx) {
                None => true,
                Some(focused) => focused == input || this.focus.contains_focused(window, cx),
            };
            if holds_nothing && !this.focus.is_focused(window) {
                this.focus.focus(window, cx);
            }
        });
    }

    /// Enter or leaving the box renames to what is typed; Escape cancels. Says it once, to the explorer's
    /// rename part, which closes the box (`Tab::renaming`) whatever comes of it.
    pub(super) fn finish_rename(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.as_mut() else { return };
        if rename.done {
            return;
        }
        rename.done = true;
        // Pasted text can hold line breaks too.
        let name = without_line_breaks(&rename.state.read(cx).value());
        let tab = self.explorer.read(cx).pane_tab_id(self.pane);
        self.explorer.update(cx, |explorer, cx| {
            if commit {
                explorer.commit_rename_in(tab, &name, cx).detach();
            } else {
                explorer.cancel_rename_in(tab, cx);
            }
        });
    }

    /// The box follows `Tab::renaming`: opened on the item it names (when the grid shows it), closed when the
    /// tab stops renaming. `entry` is that item as the grid holds it: its path, name and whether it is a folder.
    pub(super) fn sync_rename(
        &mut self,
        renaming: Option<&str>,
        entry: impl FnOnce(&str) -> Option<(String, String, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.rename.as_ref().map(|rename| rename.path.as_str());
        match renaming {
            Some(path) if current != Some(path) => {
                // Another item than the box is for: that box is over (it had its say when it lost the keyboard).
                if let Some((path, name, dir)) = entry(path) {
                    self.start_rename(&path, &name, dir, window, cx);
                }
            }
            None if current.is_some() => self.end_rename(window, cx),
            _ => {}
        }
    }

    /// The box, for the frame's tiles to draw on the item it is for.
    pub(super) fn rename_view(&self, cx: &mut Context<Self>) -> Option<RenameView> {
        let rename = self.rename.as_ref()?;
        Some(RenameView {
            path: rename.path.clone(),
            state: rename.state.clone(),
            on_finish: super::handler(cx, |this, commit: &bool, _, cx| this.finish_rename(*commit, cx)),
        })
    }

    /// Whether a rename box is open (it has its keys: the grid's own stay out of its way).
    pub(super) fn is_renaming(&self) -> bool {
        self.rename.is_some()
    }
}

/// `text` less its line breaks.
fn without_line_breaks(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect()
}

/// Takes the line breaks out of the box, keeping the caret where it was in the text.
fn drop_line_breaks(input: &Entity<TextareaState>, window: &mut Window, cx: &mut App) {
    input.update(cx, |input, cx| {
        let text = input.value().to_string();
        if !text.contains(['\n', '\r']) {
            return;
        }
        let caret = input.selected_range().end.min(text.len());
        let before = text
            .get(..caret)
            .map_or(0, |before| before.len() - without_line_breaks(before).len());
        input.set_value(without_line_breaks(&text), window, cx);
        let at = caret - before;
        input.set_selected_range(at..at, cx);
    });
}

/// The rename box as it sits in the tile, in place of the name: the input, named, with Escape and the invalid
/// characters handled.
pub(super) fn box_element(view: &RenameView, t: &theme::Tokens) -> AnyElement {
    let (commit, cancel) = (view.on_finish.clone(), view.on_finish.clone());
    div()
        .id("file-grid-rename")
        .test_support()
        .mt(space(1.5))
        .w_full()
        .min_w_0()
        .flex_none()
        // `rounded-sm border border-ring bg-background px-1`
        .rounded(theme::size::RADIUS_SM)
        .border_1()
        .border_color(t.ring)
        .bg(t.background)
        .text_size(theme::size::XS)
        .line_height(rems(1.))
        .text_center()
        // The tile under it takes presses and menus; the box keeps its own.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        // Enter is also passed on, and would run the grid's own Enter (open the item under the old name); the
        // box takes these keys and nothing above it does.
        .on_action(move |_: &OpenItem, window, cx| commit(&true, window, cx))
        .on_action(|_: &OpenInNewTab, _, _| {})
        .on_action(|_: &Rename, _, _| {})
        .on_action(|_: &Properties, _, _| {})
        .capture_key_down(move |event: &KeyDownEvent, window, cx| {
            let keystroke = &event.keystroke;
            if keystroke.key == "escape" {
                cancel(&false, window, cx);
                cx.stop_propagation();
            } else if keys::refuses_in_name(keystroke) {
                cx.stop_propagation();
                window.push_notification(
                    Notification::error(t!("A name can't contain any of these characters: \\ / : * ? \" < > |"))
                        .id::<InvalidName>(),
                    cx,
                );
            }
        })
        .child(
            Textarea::new(&view.state)
                .appearance(false)
                .bordered(false)
                .xsmall()
                .aria_label(t!("New name")),
        )
        .into_any_element()
}

/// Marks the "invalid name" notification, so a second refusal replaces the first (`id: "invalid-name"`).
struct InvalidName;
