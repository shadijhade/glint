//! The keyboard shortcuts sheet. Ported from src/components/ShortcutsDialog.tsx, built the way
//! [`super::drive_health`] is (see [`super`]).
//!
//! | TypeScript                              | Here                                                        |
//! |-----------------------------------------|-------------------------------------------------------------|
//! | the `groups` table of key and text      | [`sheet`], built from [`actions::BINDINGS`] and [`DOCUMENTED`] |
//! | `openShortcuts()` (F1, Ctrl+/, palette) | [`open`], which the `ShowShortcuts` action calls            |
//! | `ShortcutsDialog`                       | [`Shortcuts`]                                               |
//! | `Kbd`                                   | [`key_cap`]                                                 |
//!
//! There is no core call here, so there is no model and no `Source`. The sheet reads [`actions::BINDINGS`]
//! (the table the keys are bound from, so what is shown is what is bound) and the Quick search shortcut from
//! the settings, and it redraws when the settings change.
//!
//! **Where it differs from the TypeScript's table, on purpose:**
//! - A row the TypeScript wrote as one (`Alt+← → ↑` for back, forward and up) is one row per binding, since
//!   each binding is one action. Bindings that do the same thing share a row, as they did: Space and Esc in
//!   Quick Look, the nine tab keys (shown as `Ctrl+1…9`).
//! - [`DOCUMENTED`] holds the rows the TypeScript shows that no binding carries yet (type to select, the search
//!   box's keys, Shift+drop, the compare editor's keys). Those parts will bind them and move them into
//!   `BINDINGS`; until then the sheet shows them as the TypeScript did.
//! - The TypeScript's Quick Look row for the four arrows says "The next item in the folder" for all of them.
//!   Left and up go back, so the sheet has two rows there, as the bindings do.

use crate::actions::{Binding, BINDINGS};
use crate::kit::prelude::*;
use crate::{i18n, state, t, theme};

/// A section of the sheet. The order is the TypeScript's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Files,
    Tabs,
    Text,
    Anywhere,
    QuickLook,
    Compare,
}

impl Group {
    /// Every section, in the order the sheet shows them.
    pub const ALL: [Group; 6] = [
        Group::Files,
        Group::Tabs,
        Group::Text,
        Group::Anywhere,
        Group::QuickLook,
        Group::Compare,
    ];

    /// The section's heading.
    fn title(self) -> SharedString {
        match self {
            Self::Files => t!("Files"),
            Self::Tabs => t!("Tabs and panes"),
            Self::Text => t!("Text"),
            Self::Anywhere => t!("Anywhere"),
            Self::QuickLook => t!("Quick Look"),
            Self::Compare => t!("Compare two files"),
        }
    }
}

/// The Workspace keys that belong to tabs and panes (the rest of Workspace is Files or Anywhere).
const TAB_KEYS: &[&str] = &[
    "ctrl-t",
    "ctrl-w",
    "ctrl-n",
    "ctrl-shift-t",
    "ctrl-tab",
    "ctrl-shift-tab",
    "ctrl-1",
    "ctrl-2",
    "ctrl-3",
    "ctrl-4",
    "ctrl-5",
    "ctrl-6",
    "ctrl-7",
    "ctrl-8",
    "ctrl-9",
    "ctrl-shift-d",
    "ctrl-m",
    "ctrl-shift-m",
    "f6",
    "ctrl-shift-u",
    "ctrl-`",
];

/// The Workspace keys about the app as a whole, not the folder or the tabs.
const ANYWHERE_KEYS: &[&str] = &["ctrl-shift-f", "ctrl-shift-e", "ctrl-,", "f1", "ctrl-/"];

/// Keys that do what another key of the same row does and are only there because the web app took them too (the
/// browser's ways to zoom): the sheet leaves them out so its rows stay short, and the key itself still works.
const NOT_LISTED: &[&str] = &["ctrl-shift-=", "ctrl-+", "ctrl-shift--"];

/// The section a binding is listed under. The context decides most of it; a few Workspace keys are tabs or
/// app-wide ones, and everything else in Workspace is about the folder.
pub fn group_of(binding: &Binding) -> Group {
    match binding.context {
        "TextSearch" => Group::Text,
        "QuickLook" => Group::QuickLook,
        "Compare" => Group::Compare,
        "FileList" if binding.keys == "ctrl-enter" => Group::Tabs,
        "FileList" => Group::Files,
        _ if TAB_KEYS.contains(&binding.keys) => Group::Tabs,
        _ if ANYWHERE_KEYS.contains(&binding.keys) => Group::Anywhere,
        _ => Group::Files,
    }
}

/// Rows the TypeScript's sheet shows that no binding carries yet: (section, keys, what, the row to put them
/// before). `before` names a row by its English text; a row not found goes last. They are in the order the
/// TypeScript lists them, so inserting each before the same anchor keeps that order.
const DOCUMENTED: &[(Group, &str, &str, &str)] = &[
    (
        Group::Files,
        "A–Z",
        "The next name starting with what's typed",
        "Rename (several selected: all at once)",
    ),
    (
        Group::Tabs,
        "Shift+drop",
        "Move what's dropped (a plain drop copies)",
        "The terminal, under the files (it follows the folder shown)",
    ),
    (Group::Text, "Enter", "Search, or open the result picked", "Match case"),
    (
        Group::Text,
        "↓ ↑",
        "From the search box on through the results",
        "Match case",
    ),
    (
        Group::Text,
        "Shift+Enter",
        "A new line: search for several lines at once",
        "Match case",
    ),
    (
        Group::Compare,
        "Double-click",
        "Edit a line (Enter keeps it, Esc doesn't)",
        "Save what's changed",
    ),
    (
        Group::Compare,
        "Ctrl+Z",
        "Undo the last copy or edit",
        "Save what's changed",
    ),
];

/// One row of the sheet: the keys (one label, or several joined with " / ") and what they do, as the English
/// text the catalog is keyed by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub keys: String,
    pub what: &'static str,
}

/// A GPUI keystroke as the sheet writes it: "ctrl-shift-r" is "Ctrl+Shift+R", "alt-left" is "Alt+←".
pub fn key_label(keys: &str) -> String {
    let mut rest = keys;
    let mut parts: Vec<String> = Vec::new();
    // Modifiers come first, in this order, in every binding.
    for (prefix, name) in [("ctrl-", "Ctrl"), ("shift-", "Shift"), ("alt-", "Alt")] {
        if let Some(after) = rest.strip_prefix(prefix) {
            parts.push(name.to_string());
            rest = after;
        }
    }
    parts.push(key_name(rest));
    parts.join("+")
}

/// One key's name as the sheet writes it: arrows as arrows, the words as the keyboard prints them.
fn key_name(key: &str) -> String {
    match key {
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "enter" => "Enter".to_string(),
        "escape" => "Esc".to_string(),
        "space" => "Space".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        "tab" => "Tab".to_string(),
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        "menu" => "Menu".to_string(),
        other => match other.strip_prefix('f') {
            // F1 to F12: the letter is upper case, the number as it is.
            Some(digits) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => {
                format!("F{digits}")
            }
            _ => other.to_uppercase(),
        },
    }
}

/// The labels of one row's keys, as one string: "Ctrl+1…9" for the nine tab keys, else joined with " / ".
fn join_keys(labels: &[String]) -> String {
    digit_run(labels).unwrap_or_else(|| labels.join(" / "))
}

/// "Ctrl+1", "Ctrl+2", … "Ctrl+9" as "Ctrl+1…9", the way the TypeScript wrote the run. None for any other list.
fn digit_run(labels: &[String]) -> Option<String> {
    if labels.len() < 3 {
        return None;
    }
    let mut prefix: Option<&str> = None;
    for (i, label) in labels.iter().enumerate() {
        // The label is its prefix and a last digit, which must be i + 1.
        let mut chars = label.chars();
        let digit = chars.next_back()?;
        let stem = chars.as_str();
        if digit.to_digit(10) != u32::try_from(i + 1).ok() {
            return None;
        }
        match prefix {
            Some(seen) if seen != stem => return None,
            _ => prefix = Some(stem),
        }
    }
    Some(format!("{}1…{}", prefix?, labels.len()))
}

/// Puts a row before the one whose text is `anchor`, or last when there is none.
fn insert_before(rows: &mut Vec<(Vec<String>, &'static str)>, anchor: &str, keys: Vec<String>, what: &'static str) {
    let at = rows.iter().position(|(_, text)| *text == anchor).unwrap_or(rows.len());
    rows.insert(at, (keys, what));
}

/// The whole sheet: every section with its rows, in order. `quick_shortcut` is the Quick search key from the
/// settings (none when it's off); it's listed under Anywhere, before Settings, as the TypeScript did.
pub fn sheet(quick_shortcut: Option<&str>) -> Vec<(Group, Vec<Row>)> {
    Group::ALL
        .into_iter()
        .map(|group| {
            // Bindings with the same text are one row, their keys in order.
            let mut rows: Vec<(Vec<String>, &'static str)> = Vec::new();
            for binding in BINDINGS
                .iter()
                .filter(|binding| group_of(binding) == group && !NOT_LISTED.contains(&binding.keys))
            {
                let label = key_label(binding.keys);
                match rows.iter_mut().find(|(_, what)| *what == binding.what) {
                    Some((keys, _)) => keys.push(label),
                    None => rows.push((vec![label], binding.what)),
                }
            }
            if group == Group::Anywhere {
                if let Some(keys) = quick_shortcut {
                    insert_before(
                        &mut rows,
                        "Settings",
                        vec![keys.to_string()],
                        "Quick search, from any app",
                    );
                }
            }
            for &(_, keys, what, before) in DOCUMENTED.iter().filter(|(in_group, ..)| *in_group == group) {
                insert_before(&mut rows, before, vec![keys.to_string()], what);
            }
            let rows = rows
                .into_iter()
                .map(|(keys, what)| Row {
                    keys: join_keys(&keys),
                    what,
                })
                .collect();
            (group, rows)
        })
        .collect()
}

// ---- The dialog -----------------------------------------------------------------------------------------

/// The sheet. It holds nothing but its subscription: the keys come from [`actions::BINDINGS`] and the settings.
pub struct Shortcuts {
    _redraw_with_settings: Subscription,
}

impl Shortcuts {
    /// A sheet that redraws when the settings change (the Quick search key can be turned off).
    pub fn new(cx: &mut Context<Self>) -> Self {
        let redraw = cx.observe_global::<state::UiState>(|_, cx| cx.notify());
        Self {
            _redraw_with_settings: redraw,
        }
    }
}

/// A key, drawn as a key cap (the TypeScript's `Kbd`). Its text is kept left to right, so "Ctrl+Shift+R" reads
/// the same in Arabic.
fn key_cap(keys: &str, t: &theme::Tokens) -> Div {
    div()
        .flex_none()
        .px(space(1.5))
        .py(space(0.5))
        .rounded(theme::size::RADIUS_SM)
        .border_1()
        .border_color(t.border)
        .bg(t.muted)
        .text_size(theme::size::XS)
        .text_color(t.foreground)
        .child(isolate(keys.to_string()))
}

/// One binding's row: its keys in a column that hugs the text, then what it does.
fn sheet_row(group: Group, index: usize, row: &Row, t: &theme::Tokens) -> impl IntoElement {
    let what = i18n::translate(row.what);
    let label = format!("{}: {}", row.keys, what);
    row_start()
        .id(("shortcut-row", group as usize * 64 + index))
        .role(Role::Group)
        .aria_label(label)
        .test_support()
        .items_center()
        .gap(space(3.))
        .text_size(theme::size::SM)
        .child(
            row_start()
                .w(space(36.))
                .flex_none()
                .justify_end()
                .child(key_cap(&row.keys, t)),
        )
        .child(div().flex_1().min_w_0().text_color(t.muted_foreground).child(what))
}

/// One section: its heading, then its rows.
fn sheet_section(group: Group, rows: &[Row], t: &theme::Tokens) -> impl IntoElement {
    let title = group.title();
    col()
        .id(("shortcut-group", group as usize))
        .role(Role::Group)
        .aria_label(title.clone())
        .test_support()
        .gap(space(2.))
        .child(
            div()
                .text_size(theme::size::XS)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted_foreground)
                .child(title),
        )
        .child(
            col()
                .gap(space(1.5))
                .children(rows.iter().enumerate().map(|(i, row)| sheet_row(group, i, row, t))),
        )
}

/// A column of sections, with space between them.
fn sheet_column(groups: Vec<(Group, Vec<Row>)>, t: &theme::Tokens) -> Div {
    col()
        .flex_1()
        .min_w_0()
        .gap(space(5.))
        .children(groups.iter().map(|(group, rows)| sheet_section(*group, rows, t)))
}

impl Render for Shortcuts {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let quick = state::settings(cx).quick_shortcut.clone();
        // Two columns, as the TypeScript's grid placed them: Files down the left with Quick Look under it,
        // and the rest on the right.
        let (left, right): (Vec<_>, Vec<_>) = sheet(quick.as_deref())
            .into_iter()
            .partition(|(group, _)| matches!(group, Group::Files | Group::QuickLook));

        let body = div()
            .id("shortcuts-body")
            .max_h(space(160.))
            .overflow_y_scrollbar()
            .child(
                row_start()
                    .items_start()
                    .gap(space(6.))
                    .child(sheet_column(left, &t))
                    .child(sheet_column(right, &t)),
            );

        col()
            .id("shortcuts")
            .role(Role::Group)
            .aria_label(t!("Keyboard shortcuts"))
            .test_support()
            .text_size(theme::size::SM)
            // The dialog layer sits outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .gap(space(4.))
            .child(
                div()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!("F1 or Ctrl+/ shows this sheet from anywhere in Glint")),
            )
            .child(body)
    }
}

/// Opens the sheet in `window`: from F1, Ctrl+/ or the `ShowShortcuts` action.
pub fn open(window: &mut Window, cx: &mut App) {
    let view = cx.new(Shortcuts::new);
    super::show(window, cx, t!("Keyboard shortcuts"), px(880.), view);
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};

    /// Every row of the sheet, with its section.
    fn all_rows(quick: Option<&str>) -> Vec<(Group, Row)> {
        sheet(quick)
            .into_iter()
            .flat_map(|(group, rows)| rows.into_iter().map(move |row| (group, row)))
            .collect()
    }

    fn rows_of(quick: Option<&str>, group: Group) -> Vec<Row> {
        sheet(quick)
            .into_iter()
            .find(|(g, _)| *g == group)
            .map(|(_, rows)| rows)
            .unwrap_or_default()
    }

    /// Completes a frame, then asks the accessibility tree. Queries read "the last completed frame".
    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    /// An app with the skeleton's globals, and a window with the dialog layer.
    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    #[test]
    fn key_labels_read_as_the_sheet_writes_them() {
        assert_eq!(key_label("ctrl-shift-r"), "Ctrl+Shift+R");
        assert_eq!(key_label("alt-left"), "Alt+←");
        assert_eq!(key_label("ctrl--"), "Ctrl+-");
        assert_eq!(key_label("ctrl-`"), "Ctrl+`");
        assert_eq!(key_label("ctrl-,"), "Ctrl+,");
        assert_eq!(key_label("escape"), "Esc");
        assert_eq!(key_label("pagedown"), "PageDown");
        assert_eq!(key_label("shift-delete"), "Shift+Delete");
        assert_eq!(key_label("f2"), "F2");
        assert_eq!(key_label("ctrl-shift-2"), "Ctrl+Shift+2");
    }

    #[test]
    fn the_nine_tab_keys_share_one_row_as_the_typescript_wrote_them() {
        let tabs = rows_of(None, Group::Tabs);
        let run: Vec<_> = tabs.iter().filter(|row| row.what == "Go to a tab").collect();
        assert_eq!(run.len(), 1, "one row for the nine keys");
        assert_eq!(run[0].keys, "Ctrl+1…9");
        assert_eq!(
            join_keys(&["Ctrl+1".into(), "Ctrl+3".into(), "Ctrl+4".into()]),
            "Ctrl+1 / Ctrl+3 / Ctrl+4"
        );
    }

    #[test]
    fn new_window_is_listed_under_tabs_and_panes_beside_the_other_tab_keys() {
        let tabs = rows_of(None, Group::Tabs);
        let whats: Vec<_> = tabs.iter().map(|row| row.what).collect();
        let at = whats.iter().position(|what| *what == "New window on the folder shown");
        let row = at.map(|at| &tabs[at]);
        assert_eq!(row.map(|row| row.keys.as_str()), Some("Ctrl+N"));
        // After Close the tab and before Reopen, where the bindings list it.
        let of = |text: &str| whats.iter().position(|what| *what == text).unwrap_or(usize::MAX);
        assert!(of("Close the tab") < at.unwrap_or(usize::MAX));
        assert!(at.unwrap_or(usize::MAX) < of("Reopen the last closed tab"));
        // It is no longer among the folder's keys.
        assert!(!rows_of(None, Group::Files)
            .iter()
            .any(|row| row.what == "New window on the folder shown"));
    }

    #[test]
    fn every_binding_is_listed_under_its_section() {
        for binding in BINDINGS {
            let group = group_of(binding);
            let rows = rows_of(None, group);
            assert!(
                rows.iter().any(|row| row.what == binding.what),
                "{:?} ({}) is not listed under {group:?}",
                binding.keys,
                binding.what
            );
        }
    }

    #[test]
    fn sections_keep_the_typescript_order_and_its_rows() {
        let groups: Vec<_> = sheet(None).into_iter().map(|(group, _)| group).collect();
        assert_eq!(groups, Group::ALL);
        // Files starts with Enter; Text starts with the search box's keys, as the TypeScript lists them.
        let files = rows_of(None, Group::Files);
        assert_eq!(files[0].keys, "Enter");
        assert_eq!(files[0].what, "Open");
        let text: Vec<_> = rows_of(None, Group::Text).into_iter().map(|row| row.keys).collect();
        assert_eq!(&text[..4], ["Enter", "↓ ↑", "Shift+Enter", "Alt+C"]);
        // The compare editor's two rows sit before Save, in the TypeScript's order.
        let compare: Vec<_> = rows_of(None, Group::Compare).into_iter().map(|row| row.what).collect();
        let at = |text: &str| compare.iter().position(|w| *w == text).unwrap_or(usize::MAX);
        assert!(at("Edit a line (Enter keeps it, Esc doesn't)") < at("Undo the last copy or edit"));
        assert!(at("Undo the last copy or edit") < at("Save what's changed"));
    }

    #[test]
    fn quick_search_is_listed_only_while_it_has_a_key_and_before_settings() {
        let anywhere = rows_of(Some("Ctrl+Alt+F"), Group::Anywhere);
        let quick = anywhere
            .iter()
            .position(|row| row.what == "Quick search, from any app")
            .expect("the quick search row");
        assert_eq!(anywhere[quick].keys, "Ctrl+Alt+F");
        assert_eq!(anywhere[quick + 1].what, "Settings");
        assert!(!rows_of(None, Group::Anywhere)
            .iter()
            .any(|row| row.what == "Quick search, from any app"));
    }

    #[test]
    fn keys_the_web_app_also_took_share_the_row_of_the_listed_one_except_the_zoom_aliases() {
        let row = |what: &str| {
            all_rows(None)
                .into_iter()
                .map(|(_, row)| row)
                .find(|row| row.what == what)
                .map(|row| row.keys)
        };
        assert_eq!(row("Type a path").as_deref(), Some("Ctrl+L / Alt+D / F4"));
        assert_eq!(
            row("Search this folder and its subfolders (by name or path)").as_deref(),
            Some("Ctrl+F / Ctrl+E / F3")
        );
        assert_eq!(row("Refresh").as_deref(), Some("F5 / Ctrl+R"));
        assert_eq!(row("Columns").as_deref(), Some("Ctrl+Shift+9"));
        assert_eq!(
            row("The menu of what's selected (or the menu key)").as_deref(),
            Some("Shift+F10 / Menu")
        );
        // The zoom aliases work but are not on the sheet.
        assert_eq!(row("Grid: larger tiles").as_deref(), Some("Ctrl+="));
        assert_eq!(row("Grid: smaller tiles").as_deref(), Some("Ctrl+-"));
    }

    #[test]
    fn the_sheet_shows_f1_and_ctrl_slash_as_one_row() {
        let row = all_rows(None)
            .into_iter()
            .map(|(_, row)| row)
            .find(|row| row.what == "This sheet")
            .expect("the sheet's own row");
        assert_eq!(row.keys, "F1 / Ctrl+/");
    }

    #[gpui_kit::test]
    fn the_sheet_draws_its_sections_and_rows_with_labels(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(false);
        visual.update(open);
        visual.run_until_parked();
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx), "the sheet is open");
        });
        assert_eq!(role_of(&mut visual, "Keyboard shortcuts"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Files"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Tabs and panes"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Compare two files"), Some(Role::Group));
        assert_eq!(
            role_of(&mut visual, "Ctrl+Shift+R: Rename several, with find and replace"),
            Some(Role::Group)
        );
        assert_eq!(
            role_of(&mut visual, "F2: Rename (several selected: all at once)"),
            Some(Role::Group)
        );
        assert!(has(&mut visual, "Ctrl+Alt+F: Quick search, from any app"));
    }

    #[gpui_kit::test]
    fn the_quick_search_row_follows_the_setting(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(false);
        visual.update(open);
        visual.run_until_parked();
        assert!(has(&mut visual, "Ctrl+Alt+F: Quick search, from any app"));
        // Turned off in the settings, the open sheet loses the row without being reopened.
        visual.update(|_, cx| state::update(cx, |settings| settings.quick_shortcut = None));
        assert!(!has(&mut visual, "Ctrl+Alt+F: Quick search, from any app"));
    }

    #[gpui_kit::test]
    fn escape_closes_the_sheet(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(false);
        visual.update(open);
        visual.run_until_parked();
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Escape must close the sheet"));
    }

    #[gpui_kit::test]
    fn arabic_sheet_is_titled_and_labelled_in_arabic(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        visual.update(open);
        visual.run_until_parked();
        assert_eq!(role_of(&mut visual, "اختصارات لوحة المفاتيح"), Some(Role::Group));
        // Every section and row is there in the mirrored layout (one group per section, one per row).
        visual.update(|window, cx| {
            window.render_frame(cx);
            let groups = window.find_all_by_role(Role::Group).len();
            assert!(groups > 60, "only {groups} labelled groups in the Arabic sheet");
        });
        testing::set_rtl(false);
    }
}
