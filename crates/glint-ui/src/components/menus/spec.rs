//! What a menu holds, as plain data (`MenuItem` in src/components/Menu.tsx), and what each item does when chosen.
//!
//! A menu is built as a [`Vec<Node>`] by [`item`](super::item) and [`folder`](super::folder), and drawn by
//! [`view`](super::view). Keeping it data makes the menus testable without a window: the tests read the labels,
//! their order, the separators and the conditions off the spec, and only the keyboard and mirroring tests need
//! a window.
//!
//! [`visible_items`] is `visibleItems` from Menu.tsx: what a menu shows once the hidden items are gone, with
//! no heading over nothing, no empty strip or submenu, and no leading, trailing or doubled separator.

use super::event::MenuEvent;
use crate::actions::{self, BINDINGS};
use crate::kit::prelude::*;
use glint_model::paths::THIS_PC;
use std::sync::Arc;

/// One thing in a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// A row.
    Item(Entry),
    /// A menu of its own: choosing it (or the arrow toward the end of the line) shows `items` in the menu's
    /// place, with a Back row.
    Menu {
        label: SharedString,
        icon: Option<IconName>,
        disabled: bool,
        items: Vec<Node>,
    },
    /// Starts a section, with a line above it unless it's first; it goes when nothing is under it.
    Heading(SharedString),
    /// A line between sections.
    Separator,
    /// A compact top strip of icon buttons for the frequent commands (Cut, Copy, Paste, Rename, Copy path,
    /// Pin, Delete): `actions` in the TypeScript.
    Strip(Vec<Entry>),
}

impl Node {
    /// A submenu.
    pub fn menu(label: SharedString, icon: IconName, items: Vec<Option<Node>>) -> Node {
        Node::Menu {
            label,
            icon: Some(icon),
            disabled: false,
            items: items.into_iter().flatten().collect(),
        }
    }
}

/// A row, or one button of the strip.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The words, already translated.
    pub label: SharedString,
    /// Its picture; none for a row that is only a check mark.
    pub icon: Option<IconName>,
    /// A picture of its own (an app's icon, as File Explorer shows it) drawn in the icon's place: the New menu's
    /// kinds of file (`image` in Menu.tsx). `icon` stays as what a row falls back to.
    pub picture: Option<Arc<RenderImage>>,
    /// The key it has elsewhere, as text ("Ctrl+C").
    pub hint: Option<SharedString>,
    /// Greyed out and not chosen.
    pub disabled: bool,
    /// In the warning color (Delete).
    pub danger: bool,
    /// In the accent color (a pinned folder's pin).
    pub active: bool,
    /// `Some` makes it one of a set or an on/off row: a check mark in place of the icon, checked or not.
    pub check: Option<bool>,
    /// What choosing it does.
    pub run: Run,
}

impl Entry {
    /// A row that does `run`.
    pub fn new(label: SharedString, icon: IconName, run: Run) -> Entry {
        Entry {
            label,
            icon: Some(icon),
            picture: None,
            hint: None,
            disabled: false,
            danger: false,
            active: false,
            check: None,
            run,
        }
    }

    /// A row that dispatches a workspace command; its key is the command's, from [`BINDINGS`].
    pub fn command(label: SharedString, icon: IconName, command: Command) -> Entry {
        let mut entry = Entry::new(label, icon, Run::Command(command));
        entry.hint = command.hint();
        entry
    }

    /// A row handled here.
    pub fn task(label: SharedString, icon: IconName, task: Task) -> Entry {
        Entry::new(label, icon, Run::Do(task))
    }

    /// A row the window answers, by a [`MenuEvent`].
    pub fn ask(label: SharedString, icon: IconName, event: MenuEvent) -> Entry {
        Entry::new(label, icon, Run::Ask(event))
    }

    /// A line of grey explanation that can't be chosen ("Read-only: inside an archive").
    pub fn note(label: SharedString, icon: IconName) -> Entry {
        let mut entry = Entry::new(label, icon, Run::Nothing);
        entry.disabled = true;
        entry
    }

    /// Draws `pixels` (an app's icon, as File Explorer shows it) in the icon's place. None, or pixels that don't
    /// make a picture, leave the row with its icon.
    pub fn with_picture(mut self, pixels: Option<&glint_core::thumbs::Pixels>) -> Entry {
        self.picture = pixels.and_then(crate::core_bridge::picture);
        self
    }

    /// Shows `text` as the key (for a row whose key isn't an action: "Shift+right-click").
    pub fn hinted(mut self, text: &'static str) -> Entry {
        self.hint = Some(text.into());
        self
    }

    /// Shows the key `command` has, for a row that does the same thing by other means.
    pub fn hint_of(mut self, command: Command) -> Entry {
        self.hint = command.hint();
        self
    }

    /// Greyed out when `yes`.
    pub fn disabled(mut self, yes: bool) -> Entry {
        self.disabled = yes;
        self
    }

    /// In the warning color when `yes`.
    pub fn danger(mut self, yes: bool) -> Entry {
        self.danger = yes;
        self
    }

    /// In the accent color when `yes`.
    pub fn active(mut self, yes: bool) -> Entry {
        self.active = yes;
        self
    }

    /// An on/off row, on when `yes`.
    pub fn checked(mut self, yes: bool) -> Entry {
        self.check = Some(yes);
        self
    }

    /// As a node.
    pub fn node(self) -> Option<Node> {
        Some(Node::Item(self))
    }
}

/// `Some(node)` when `condition`: the TypeScript's `condition && { ... }` in a menu's array.
pub fn when(condition: bool, node: impl FnOnce() -> Node) -> Option<Node> {
    condition.then(node)
}

/// A line between sections, for a menu's array.
pub fn separator() -> Option<Node> {
    Some(Node::Separator)
}

/// What choosing an item does.
#[derive(Clone, Debug, PartialEq)]
pub enum Run {
    /// Dispatches a workspace command on what had the keyboard when the menu opened, so the workspace's own
    /// handlers do it (and its undo, its copy queue, its confirmations).
    Command(Command),
    /// Done here, with the explorer and the app's services.
    Do(Task),
    /// Told to the window as a [`MenuEvent`], for the part that does it.
    Ask(MenuEvent),
    /// Nothing: a note.
    Nothing,
}

/// The workspace commands the menus dispatch: each is one of [`actions`]'s unit actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    OpenInNewTab,
    QuickLook,
    Rename,
    BatchRename,
    Cut,
    Copy,
    Paste,
    CopyPath,
    Delete,
    DeletePermanently,
    Undo,
    SelectAll,
    Refresh,
    NewFolder,
    Properties,
    CompareFiles,
}

impl Command {
    /// The action to dispatch.
    pub fn action(self) -> Box<dyn gpui_kit::Action> {
        match self {
            Command::Open => Box::new(actions::OpenItem),
            Command::OpenInNewTab => Box::new(actions::OpenInNewTab),
            Command::QuickLook => Box::new(actions::QuickLook),
            Command::Rename => Box::new(actions::Rename),
            Command::BatchRename => Box::new(actions::BatchRename),
            Command::Cut => Box::new(actions::Cut),
            Command::Copy => Box::new(actions::Copy),
            Command::Paste => Box::new(actions::Paste),
            Command::CopyPath => Box::new(actions::CopyPath),
            Command::Delete => Box::new(actions::Delete),
            Command::DeletePermanently => Box::new(actions::DeletePermanently),
            Command::Undo => Box::new(actions::Undo),
            Command::SelectAll => Box::new(actions::SelectAll),
            Command::Refresh => Box::new(actions::Refresh),
            Command::NewFolder => Box::new(actions::NewFolder),
            Command::Properties => Box::new(actions::Properties),
            Command::CompareFiles => Box::new(actions::CompareFiles),
        }
    }

    /// The key bound to it, as text: what the menus show, so what is shown is what is bound
    /// ([`BINDINGS`] is the one table). None for a command with no key.
    pub fn hint(self) -> Option<SharedString> {
        let action = self.action();
        BINDINGS
            .iter()
            .find(|binding| action.partial_eq(&*(binding.action)()))
            .map(|binding| key_text(binding.keys).into())
    }
}

/// A GPUI keystroke ("ctrl-shift-c") as menus word it ("Ctrl+Shift+C"; Delete is "Del", as File Explorer's menu has it).
pub fn key_text(keys: &str) -> String {
    let mut rest = keys;
    let mut parts: Vec<String> = Vec::new();
    // Modifiers come in any order in a binding; "ctrl--" is Ctrl and the minus key.
    loop {
        let mut found = false;
        for (prefix, word) in [("ctrl-", "Ctrl"), ("alt-", "Alt"), ("shift-", "Shift"), ("cmd-", "Win")] {
            if let Some(after) = rest.strip_prefix(prefix) {
                if !after.is_empty() {
                    parts.push(word.to_string());
                    rest = after;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            break;
        }
    }
    parts.push(match rest {
        "delete" => "Del".to_string(),
        key => {
            let mut chars = key.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    });
    parts.join("+")
}

/// What Glint does itself, with the explorer and the app's services, when an item is chosen.
#[derive(Clone, Debug, PartialEq)]
pub enum Task {
    /// Opens a folder (or an archive) in the active tab (`navigate`).
    Navigate(String),
    /// Opens each in a new tab (`openInNewTab`).
    OpenInNewTabs(Vec<String>),
    /// Opens a folder in the pane beside the one the menu is for, turning on two panes if need be
    /// (`openInOtherPane`).
    OpenInOtherPane { path: String, from: usize },
    /// Puts text on the clipboard and says "Copied path" or "Copied paths" (`copyText`).
    CopyText { text: String, paths: usize },
    /// Pins a folder to the sidebar, or unpins it (`togglePin`).
    TogglePin(String),
    /// Reads the drives again (`refreshDrives`): This PC's Refresh.
    RefreshDrives,
    /// Windows' "Continue": asks for access to a folder that is closed (`getAccess`).
    GetAccess(Option<crate::explorer::TabId>),
    /// "Show more options": Windows' own menu for these paths, at the point the menu opened (`windowsMenu`).
    WindowsMenu { paths: Vec<String> },
    /// A terminal in a folder (`openTerminal`).
    OpenTerminal(String),
    /// Windows' "Open with" picker (`openWith`).
    OpenWith(String),
    /// Runs a program or script as administrator (`runAsAdmin`).
    RunAsAdmin(String),
    /// Windows' Properties window (`showProperties`).
    ShowProperties(String),
    /// Opens a file or folder in an editor or terminal found on this PC (`openIn`): its id, its name for the
    /// message when it fails, and the path.
    OpenIn { opener: String, name: String, path: String },
    /// File Explorer's own window on a folder (`openPath`).
    OpenInFileExplorer(String),
}

/// `visibleItems` (Menu.tsx): the items a menu shows. Strips left with no button, submenus left with nothing,
/// headings with nothing under them, and separators that lead, trail, double up, follow a strip or precede
/// a heading are dropped. Submenus are cleaned the same way.
pub fn visible_items(items: Vec<Node>) -> Vec<Node> {
    let kept: Vec<Node> = items
        .into_iter()
        .filter_map(|node| match node {
            Node::Strip(entries) if entries.is_empty() => None,
            Node::Menu {
                label,
                icon,
                disabled,
                items,
            } => {
                let items = visible_items(items);
                (!items.is_empty()).then_some(Node::Menu {
                    label,
                    icon,
                    disabled,
                    items,
                })
            }
            node => Some(node),
        })
        .collect();
    // A heading needs a real item under it.
    let headed: Vec<Node> = kept
        .iter()
        .enumerate()
        .filter(|(i, node)| {
            !matches!(node, Node::Heading(_))
                || kept
                    .get(i + 1)
                    .is_some_and(|next| !matches!(next, Node::Separator | Node::Heading(_)))
        })
        .map(|(_, node)| node.clone())
        .collect();
    headed
        .iter()
        .enumerate()
        .filter(|(i, node)| {
            !matches!(node, Node::Separator)
                || (*i > 0
                    && *i + 1 < headed.len()
                    && !matches!(headed[i - 1], Node::Separator | Node::Strip(_))
                    && !matches!(headed[i + 1], Node::Heading(_)))
        })
        .map(|(_, node)| node.clone())
        .collect()
}

/// This PC's own menu is one row (`folderMenu` when the folder is This PC).
pub fn is_this_pc(folder: &str) -> bool {
    folder == THIS_PC
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icons_pixels_give_the_row_its_own_picture_and_nothing_keeps_the_icon() {
        let row = |pixels: Option<&glint_core::thumbs::Pixels>| {
            Entry::task("Word".into(), IconName::FilePlus, Task::RefreshDrives).with_picture(pixels)
        };
        let icon = glint_core::thumbs::Pixels {
            width: 2,
            height: 1,
            bgra: vec![1, 2, 3, 255, 4, 5, 6, 128],
        };
        let with = row(Some(&icon));
        let picture = with.picture.as_ref().expect("a picture");
        assert_eq!(picture.as_bytes(0), Some(&icon.bgra[..]));
        // The icon stays as what the row falls back to.
        assert_eq!(with.icon, Some(IconName::FilePlus));
        assert!(row(None).picture.is_none());
        let short = glint_core::thumbs::Pixels {
            width: 2,
            height: 2,
            bgra: vec![0; 4],
        };
        assert!(row(Some(&short)).picture.is_none());
    }

    fn item(label: &str) -> Node {
        Entry::task(label.to_string().into(), IconName::File, Task::RefreshDrives)
            .node()
            .expect("an item")
    }

    fn labels(items: &[Node]) -> Vec<String> {
        items
            .iter()
            .map(|node| match node {
                Node::Item(e) => e.label.to_string(),
                Node::Menu { label, .. } => format!("{label}>"),
                Node::Heading(h) => format!("# {h}"),
                Node::Separator => "-".into(),
                Node::Strip(entries) => format!("[{}]", entries.len()),
            })
            .collect()
    }

    #[test]
    fn keys_read_the_way_menus_word_them() {
        assert_eq!(key_text("ctrl-shift-c"), "Ctrl+Shift+C");
        assert_eq!(key_text("delete"), "Del");
        assert_eq!(key_text("f2"), "F2");
        assert_eq!(key_text("alt-enter"), "Alt+Enter");
        assert_eq!(key_text("ctrl--"), "Ctrl+-");
        assert_eq!(key_text("ctrl-="), "Ctrl+=");
        assert_eq!(key_text("space"), "Space");
    }

    #[test]
    fn a_commands_hint_is_the_key_the_bindings_table_gives_it() {
        assert_eq!(Command::Copy.hint().as_deref(), Some("Ctrl+C"));
        assert_eq!(Command::Cut.hint().as_deref(), Some("Ctrl+X"));
        assert_eq!(Command::Paste.hint().as_deref(), Some("Ctrl+V"));
        assert_eq!(Command::Delete.hint().as_deref(), Some("Del"));
        assert_eq!(Command::Rename.hint().as_deref(), Some("F2"));
        assert_eq!(Command::Open.hint().as_deref(), Some("Enter"));
        assert_eq!(Command::OpenInNewTab.hint().as_deref(), Some("Ctrl+Enter"));
        assert_eq!(Command::QuickLook.hint().as_deref(), Some("Space"));
        assert_eq!(Command::CopyPath.hint().as_deref(), Some("Ctrl+Shift+C"));
        assert_eq!(Command::Properties.hint().as_deref(), Some("Alt+Enter"));
        assert_eq!(Command::NewFolder.hint().as_deref(), Some("Ctrl+Shift+N"));
        assert_eq!(Command::Refresh.hint().as_deref(), Some("F5"));
        assert_eq!(Command::Undo.hint().as_deref(), Some("Ctrl+Z"));
        assert_eq!(Command::SelectAll.hint().as_deref(), Some("Ctrl+A"));
        assert_eq!(Command::CompareFiles.hint().as_deref(), Some("Ctrl+M"));
    }

    #[test]
    fn hidden_items_leave_no_stray_lines_headings_or_empty_menus() {
        let shown = visible_items(vec![
            Node::Separator,
            item("a"),
            Node::Separator,
            Node::Separator,
            item("b"),
            Node::Menu {
                label: "Empty".into(),
                icon: None,
                disabled: false,
                items: vec![Node::Separator],
            },
            Node::Separator,
        ]);
        assert_eq!(labels(&shown), ["a", "-", "b"]);
    }

    #[test]
    fn a_heading_needs_something_under_it_and_a_strip_needs_buttons() {
        let shown = visible_items(vec![
            Node::Strip(vec![]),
            Node::Heading("Columns".into()),
            Node::Separator,
            Node::Heading("Sort".into()),
            item("a"),
            Node::Heading("Last".into()),
        ]);
        assert_eq!(labels(&shown), ["# Sort", "a"]);
    }

    #[test]
    fn no_line_follows_the_strip_or_comes_before_a_heading() {
        let strip = Node::Strip(vec![Entry::task("Copy".into(), IconName::Copy, Task::RefreshDrives)]);
        let shown = visible_items(vec![
            strip,
            Node::Separator,
            item("a"),
            Node::Separator,
            Node::Heading("H".into()),
            item("b"),
        ]);
        assert_eq!(labels(&shown), ["[1]", "a", "# H", "b"]);
    }

    #[test]
    fn submenus_are_cleaned_too() {
        let shown = visible_items(vec![Node::Menu {
            label: "More".into(),
            icon: None,
            disabled: false,
            items: vec![Node::Separator, item("x"), Node::Separator],
        }]);
        match &shown[0] {
            Node::Menu { items, .. } => assert_eq!(labels(items), ["x"]),
            other => panic!("{other:?}"),
        }
    }
}
