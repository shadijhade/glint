//! The box a row shows while it is being renamed (`RenameInput` in FileList.tsx).
//!
//! It holds the name with everything but the extension selected, as File Explorer does. Enter (or leaving
//! the box) asks to rename, Escape cancels, and a character no Windows name can hold is refused with the
//! hint. Committing is the explorer's rename part: this only says what was typed.

use super::{FileList, FileListEvent};
use crate::kit::prelude::*;
use crate::t;

/// The characters no Windows file name can contain (`INVALID_NAME_HINT` lists them).
pub const INVALID_NAME_CHARS: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

/// Where the selection ends in a name being renamed: before the extension of a file ("report" in
/// "report.pdf"), the whole name for a folder or a name that starts with its only dot (".gitignore").
/// A UTF-8 byte offset, which is what the input takes (the TypeScript's `lastIndexOf` counted UTF-16 units).
pub fn selection_end(name: &str, dir: bool) -> usize {
    match name.rfind('.') {
        Some(dot) if !dir && dot > 0 => dot,
        _ => name.len(),
    }
}

/// The rename box of the row being renamed.
pub struct RenameBox {
    /// The item being renamed.
    pub path: String,
    /// What is typed.
    pub state: Entity<InputState>,
    /// Set once the box has said what it has to say, so leaving it after Enter doesn't say it twice.
    pub done: bool,
    _subscription: Subscription,
}

impl FileList {
    /// Opens the rename box on the row of `path`, with its name typed in and selected.
    pub(super) fn start_rename(
        &mut self,
        path: &str,
        name: &str,
        dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = name.to_string();
        let state = cx.new(|cx| InputState::new(window, cx).default_value(text));
        let subscription = cx.subscribe_in(&state, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.finish_rename(true, cx),
            _ => {}
        });
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

    /// Enter or leaving the box commits what is typed; Escape cancels (`finish`). Says it once.
    pub(super) fn finish_rename(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.as_mut() else { return };
        if rename.done {
            return;
        }
        rename.done = true;
        let path = rename.path.clone();
        let name = rename.state.read(cx).value().to_string();
        cx.emit(if commit {
            FileListEvent::CommitRename { path, name }
        } else {
            FileListEvent::CancelRename
        });
    }

    /// The rename box as it sits in the row: the input, named, with Escape and the invalid characters handled.
    pub(super) fn rename_input(&self, rename: &RenameBox, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .min_w_0()
            .h(rems(1.5))
            // The row under it takes presses and double-clicks; the box keeps its own.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                if key == "escape" {
                    this.finish_rename(false, cx);
                    cx.stop_propagation();
                    return;
                }
                let typed = event.keystroke.key_char.as_deref().unwrap_or("");
                let mut chars = typed.chars();
                if let (Some(c), None) = (chars.next(), chars.next()) {
                    if INVALID_NAME_CHARS.contains(&c) && !event.keystroke.modifiers.control {
                        cx.stop_propagation();
                        window.push_notification(
                            Notification::error(t!(
                                "A name can't contain any of these characters: \\ / : * ? \" < > |"
                            ))
                            .id::<InvalidName>(),
                            cx,
                        );
                    }
                }
            }))
            .child(
                Input::new(&rename.state)
                    .aria_label(t!("New name"))
                    .h_full()
                    .into_any_element(),
            )
            .into_any_element()
    }
}

/// Marks the "invalid name" notification, so a second refusal replaces the first (`id: "invalid-name"`).
struct InvalidName;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_selected_without_its_extension() {
        assert_eq!(selection_end("report.pdf", false), 6);
        assert_eq!(selection_end("archive.tar.gz", false), 11);
        // A folder, a name without a dot and a name that is only an extension are selected whole.
        assert_eq!(selection_end("v1.2 folder", true), "v1.2 folder".len());
        assert_eq!(selection_end("Makefile", false), 8);
        assert_eq!(selection_end(".gitignore", false), 10);
    }

    #[test]
    fn the_selection_end_is_a_byte_offset_on_a_character_boundary() {
        let name = "تقرير.pdf";
        let end = selection_end(name, false);
        assert_eq!(&name[..end], "تقرير");
        assert!(name.is_char_boundary(end));
    }
}
