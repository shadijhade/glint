//! Commands as GPUI actions, and the keys that run them.
//!
//! **One action per command**, named for what it does (`NewTab`, `FocusAddress`). The keys are the ones
//! src/components/ShortcutsDialog.tsx lists, in [`BINDINGS`], each in the key context where it applies:
//!
//! - `"Workspace"`: the main window, anywhere in it (tabs, panes, search, settings…).
//! - `"FileList"`: the file list or grid has focus (Enter, F2, Delete, Ctrl+C…). A text field inside the
//!   window has its own, deeper context, so typing Ctrl+A or Backspace there edits text and never reaches
//!   these.
//! - `"TextSearch"`, `"QuickLook"`, `"Compare"`: those panes, when they exist.
//!
//! **Pattern for a view that handles an action.** The view puts its context on its root element and
//! registers a listener; the skeleton's workspace does this for the commands it owns:
//!
//! ```ignore
//! div()
//!     .key_context("FileList")                       // makes FileList's bindings active under this element
//!     .track_focus(&self.focus_handle)
//!     .on_action(cx.listener(|this, _: &actions::Rename, window, cx| this.rename(window, cx)))
//! ```
//!
//! An action nobody handles does nothing. Handlers belong to the views that come with the component
//! waves; the skeleton defines every action and binds every key now so the shortcut sheet, the command
//! palette and menus can already list them.
//!
//! [`BINDINGS`] is data on purpose: the keyboard-shortcuts dialog, tooltips and the palette read the same
//! table, so what is shown is what is bound.

use gpui_kit::{actions, Action, App, KeyBinding, KeyBindingContextPredicate};
use std::rc::Rc;

actions!(
    glint,
    [
        // Tabs and panes
        NewTab,
        CloseTab,
        ReopenClosedTab,
        NextTab,
        PreviousTab,
        OpenInNewTab,
        DualPane,
        CompareFiles,
        ComparePanes,
        OtherPane,
        SwapPanes,
        ToggleTerminal,
        // Navigation and search
        CommandPalette,
        SearchHere,
        FocusAddress,
        Back,
        Forward,
        Up,
        Refresh,
        TextSearch,
        BackToFiles,
        // View
        ToggleHidden,
        TogglePreview,
        GridView,
        ListView,
        GridLarger,
        GridSmaller,
        GridReset,
        DiskUsage,
        // The file list
        OpenItem,
        QuickLook,
        Rename,
        BatchRename,
        Delete,
        DeletePermanently,
        Undo,
        SelectAll,
        Copy,
        Cut,
        Paste,
        CopyPath,
        NewFolder,
        ShowItemMenu,
        Properties,
        // Windows and dialogs
        OpenSettings,
        ShowShortcuts,
        ShowDriveHealth,
        // Text search
        TextMatchCase,
        TextWholeWord,
        TextRegex,
        TextCodePatterns,
        TextDocuments,
        TextPictures,
        TextMeaning,
        TextReplace,
        TextLeave,
        // Quick Look
        CloseQuickLook,
        QuickLookNext,
        QuickLookPrevious,
        QuickLookScrollUp,
        QuickLookScrollDown,
        // Compare two files
        ComparePreviousChange,
        CompareNextChange,
        CopyChangeRight,
        CopyChangeLeft,
        CompareSave,
        CompareFind,
    ]
);

/// Go to the tab at `index` (0-based; Ctrl+1…9). Not a unit action, so not in the macro above.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = glint, no_json)]
pub struct GoToTab {
    /// The tab's position, counting from 0.
    pub index: usize,
}

/// "Moved and deleted here": the history dialog for `query` (a folder's path, or empty for everywhere). Not a unit
/// action, so not in the macro above; menus and the palette dispatch it.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = glint, no_json)]
pub struct ShowHistory {
    /// The folder to show the history of, or "" for every drive.
    pub query: String,
}

/// One key binding: the keystroke, the context it applies in, the action it runs and what it means to a person.
pub struct Binding {
    /// GPUI keystroke syntax: `ctrl-shift-r`, `alt-left`, `f2`.
    pub keys: &'static str,
    /// The key context, or `""` for everywhere.
    pub context: &'static str,
    /// Makes the action.
    pub action: fn() -> Box<dyn Action>,
    /// What it does, as the shortcut sheet words it (English text, the `t!` key).
    pub what: &'static str,
}

/// A binding that runs a unit action or a `GoToTab`.
macro_rules! bind {
    ($keys:literal, $context:literal, $action:expr, $what:literal) => {
        Binding {
            keys: $keys,
            context: $context,
            action: || Box::new($action),
            what: $what,
        }
    };
}

/// Every key Glint binds. The wording is ShortcutsDialog.tsx's, so the sheet reads the same.
pub static BINDINGS: &[Binding] = &[
    // Files
    bind!("enter", "FileList", OpenItem, "Open"),
    bind!("space", "FileList", QuickLook, "Quick Look"),
    bind!(
        "ctrl-k",
        "Workspace",
        CommandPalette,
        "Command palette: folders and commands (= and a sum works it out)"
    ),
    // The web app opened the palette on Ctrl+P too.
    bind!(
        "ctrl-p",
        "Workspace",
        CommandPalette,
        "Command palette: folders and commands (= and a sum works it out)"
    ),
    bind!(
        "ctrl-f",
        "Workspace",
        SearchHere,
        "Search this folder and its subfolders (by name or path)"
    ),
    bind!("ctrl-l", "Workspace", FocusAddress, "Type a path"),
    bind!("alt-left", "Workspace", Back, "Back"),
    bind!("alt-right", "Workspace", Forward, "Forward"),
    bind!("alt-up", "Workspace", Up, "Up"),
    bind!("backspace", "FileList", Back, "Back"),
    bind!("f2", "FileList", Rename, "Rename (several selected: all at once)"),
    bind!(
        "ctrl-shift-r",
        "FileList",
        BatchRename,
        "Rename several, with find and replace"
    ),
    bind!("delete", "FileList", Delete, "To the Recycle Bin (Shift: for good)"),
    bind!("shift-delete", "FileList", DeletePermanently, "Delete for good"),
    bind!("ctrl-z", "FileList", Undo, "Undo"),
    bind!("ctrl-a", "FileList", SelectAll, "Select all"),
    bind!("ctrl-c", "FileList", Copy, "Copy"),
    bind!("ctrl-x", "FileList", Cut, "Cut"),
    bind!("ctrl-v", "FileList", Paste, "Paste"),
    bind!("ctrl-shift-c", "FileList", CopyPath, "Copy the path"),
    bind!("ctrl-shift-n", "Workspace", NewFolder, "New folder"),
    bind!(
        "shift-f10",
        "FileList",
        ShowItemMenu,
        "The menu of what's selected (or the menu key)"
    ),
    bind!("alt-enter", "FileList", Properties, "Properties"),
    bind!("ctrl-h", "Workspace", ToggleHidden, "Hidden items"),
    bind!("alt-p", "Workspace", TogglePreview, "Preview pane"),
    bind!("ctrl-shift-2", "Workspace", GridView, "Grid"),
    bind!("ctrl-shift-6", "Workspace", ListView, "List"),
    bind!("ctrl-=", "Workspace", GridLarger, "Grid: larger tiles"),
    bind!("ctrl--", "Workspace", GridSmaller, "Grid: smaller tiles"),
    bind!("ctrl-0", "Workspace", GridReset, "Grid: the usual size"),
    bind!("f5", "Workspace", Refresh, "Refresh"),
    // Tabs and panes
    bind!("ctrl-t", "Workspace", NewTab, "New tab"),
    bind!("ctrl-w", "Workspace", CloseTab, "Close the tab"),
    bind!(
        "ctrl-shift-t",
        "Workspace",
        ReopenClosedTab,
        "Reopen the last closed tab"
    ),
    bind!("ctrl-tab", "Workspace", NextTab, "Next tab (Shift: previous)"),
    bind!("ctrl-shift-tab", "Workspace", PreviousTab, "Previous tab"),
    bind!("ctrl-1", "Workspace", GoToTab { index: 0 }, "Go to a tab"),
    bind!("ctrl-2", "Workspace", GoToTab { index: 1 }, "Go to a tab"),
    bind!("ctrl-3", "Workspace", GoToTab { index: 2 }, "Go to a tab"),
    bind!("ctrl-4", "Workspace", GoToTab { index: 3 }, "Go to a tab"),
    bind!("ctrl-5", "Workspace", GoToTab { index: 4 }, "Go to a tab"),
    bind!("ctrl-6", "Workspace", GoToTab { index: 5 }, "Go to a tab"),
    bind!("ctrl-7", "Workspace", GoToTab { index: 6 }, "Go to a tab"),
    bind!("ctrl-8", "Workspace", GoToTab { index: 7 }, "Go to a tab"),
    bind!("ctrl-9", "Workspace", GoToTab { index: 8 }, "Go to a tab"),
    bind!("ctrl-enter", "FileList", OpenInNewTab, "Open in a new tab"),
    bind!("ctrl-shift-d", "Workspace", DualPane, "Two panes"),
    bind!(
        "ctrl-m",
        "Workspace",
        CompareFiles,
        "Compare two selected files (or one in each pane)"
    ),
    bind!("ctrl-shift-m", "Workspace", ComparePanes, "Compare the panes"),
    bind!("f6", "Workspace", OtherPane, "The other pane"),
    bind!("ctrl-shift-u", "Workspace", SwapPanes, "Swap the panes"),
    bind!(
        "ctrl-`",
        "Workspace",
        ToggleTerminal,
        "The terminal, under the files (it follows the folder shown)"
    ),
    // Text
    bind!("alt-c", "TextSearch", TextMatchCase, "Match case"),
    bind!("alt-w", "TextSearch", TextWholeWord, "Whole words"),
    bind!("alt-r", "TextSearch", TextRegex, "Regular expression"),
    bind!("alt-a", "TextSearch", TextCodePatterns, "Code patterns (ast-grep)"),
    bind!(
        "alt-d",
        "TextSearch",
        TextDocuments,
        "Documents (PDF, Office, OpenDocument, EPUB)"
    ),
    bind!("alt-o", "TextSearch", TextPictures, "Pictures too (OCR)"),
    bind!("alt-m", "TextSearch", TextMeaning, "Meaning"),
    bind!("ctrl-h", "TextSearch", TextReplace, "Replace"),
    bind!(
        "escape",
        "TextSearch",
        TextLeave,
        "Leave the search box; again, stop the search"
    ),
    // Anywhere
    bind!("ctrl-shift-f", "Workspace", TextSearch, "Search inside the files here"),
    bind!("ctrl-shift-e", "Workspace", BackToFiles, "Back to Files from Text"),
    bind!("ctrl-,", "Workspace", OpenSettings, "Settings"),
    bind!("f1", "Workspace", ShowShortcuts, "This sheet"),
    bind!("ctrl-/", "Workspace", ShowShortcuts, "This sheet"),
    // Quick Look
    bind!("escape", "QuickLook", CloseQuickLook, "Close it"),
    bind!("space", "QuickLook", CloseQuickLook, "Close it"),
    bind!(
        "left",
        "QuickLook",
        QuickLookPrevious,
        "The previous item in the folder"
    ),
    bind!("up", "QuickLook", QuickLookPrevious, "The previous item in the folder"),
    bind!("right", "QuickLook", QuickLookNext, "The next item in the folder"),
    bind!("down", "QuickLook", QuickLookNext, "The next item in the folder"),
    bind!("pageup", "QuickLook", QuickLookScrollUp, "Scroll it up"),
    bind!("pagedown", "QuickLook", QuickLookScrollDown, "Scroll it down"),
    // Compare two files
    bind!("alt-up", "Compare", ComparePreviousChange, "The previous change"),
    bind!("alt-down", "Compare", CompareNextChange, "The next change"),
    bind!("alt-right", "Compare", CopyChangeRight, "Copy the change to the right"),
    bind!("alt-left", "Compare", CopyChangeLeft, "Copy the change to the left"),
    bind!("ctrl-s", "Compare", CompareSave, "Save what's changed"),
    bind!("ctrl-f", "Compare", CompareFind, "Find text in both"),
];

/// GPUI's key bindings for [`BINDINGS`], and the text of each one it refused (none today; the test below
/// keeps it so). Nothing here panics on a bad entry.
fn load(cx: &App) -> (Vec<KeyBinding>, Vec<String>) {
    let (mut bindings, mut refused) = (Vec::new(), Vec::new());
    for binding in BINDINGS {
        let context = if binding.context.is_empty() {
            Ok(None)
        } else {
            KeyBindingContextPredicate::parse(binding.context).map(|predicate| Some(Rc::new(predicate)))
        };
        let loaded = context.map_err(|e| e.to_string()).and_then(|context| {
            KeyBinding::load(binding.keys, (binding.action)(), context, false, None, cx).map_err(|e| e.to_string())
        });
        match loaded {
            Ok(loaded) => bindings.push(loaded),
            Err(error) => refused.push(format!("{:?} in {:?}: {error}", binding.keys, binding.context)),
        }
    }
    (bindings, refused)
}

/// Start-up step 4: binds every key in [`BINDINGS`]. One GPUI can't parse is skipped and reported, never a panic.
pub fn init(cx: &mut App) {
    let (bindings, refused) = load(cx);
    for problem in refused {
        eprintln!("glint: skipping a key binding GPUI can't parse: {problem}");
    }
    cx.bind_keys(bindings);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[gpui_kit::test]
    fn every_binding_is_a_keystroke_gpui_can_parse(cx: &mut gpui_kit::TestAppContext) {
        let (bindings, refused) = cx.update(|cx| load(cx));
        assert!(refused.is_empty(), "{refused:?}");
        assert_eq!(bindings.len(), BINDINGS.len());
    }

    #[test]
    fn no_key_is_bound_twice_in_one_context() {
        let mut seen = HashSet::new();
        for binding in BINDINGS {
            assert!(
                seen.insert((binding.keys, binding.context)),
                "{:?} twice in {:?}",
                binding.keys,
                binding.context
            );
        }
    }

    #[test]
    fn bindings_make_their_actions_and_every_context_is_a_known_one() {
        for binding in BINDINGS {
            let action = (binding.action)();
            assert!(action.name().starts_with("glint::"), "{}", action.name());
            assert!(
                ["Workspace", "FileList", "TextSearch", "QuickLook", "Compare"].contains(&binding.context),
                "unknown context {:?}",
                binding.context
            );
        }
    }

    #[test]
    fn ctrl_digits_go_to_tabs_one_to_nine() {
        let tabs: Vec<_> = BINDINGS.iter().filter(|b| b.what == "Go to a tab").collect();
        assert_eq!(tabs.len(), 9);
        for (i, binding) in tabs.iter().enumerate() {
            assert_eq!(binding.keys, format!("ctrl-{}", i + 1));
            let action = (binding.action)();
            let go = action.as_any().downcast_ref::<GoToTab>().map(|go| go.index);
            assert_eq!(go, Some(i));
        }
    }

    #[test]
    fn keys_with_text_editing_meaning_are_never_bound_outside_the_file_list() {
        // Typing in an address bar or a rename box must keep its Ctrl+A/C/X/V/Z, Backspace and Delete.
        for binding in BINDINGS {
            if ["ctrl-a", "ctrl-c", "ctrl-x", "ctrl-v", "ctrl-z", "backspace", "delete"].contains(&binding.keys) {
                assert_eq!(binding.context, "FileList", "{:?}", binding.keys);
            }
        }
    }
}
