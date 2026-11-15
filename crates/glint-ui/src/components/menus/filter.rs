//! Finding a menu's rows by typing (docs/gpui/aware/PLAN.md, F3).
//!
//! Pure logic over the menu's data (`Node`s): no drawing here. The menu's view shows what [`matches`] returns.

use super::spec::{Entry, Node, Run};
use crate::kit::prelude::*;
use glint_model::rank::fuzzy_score;

/// A row found by typing.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The row, as it is in the menu (its label is its own: "Zip file").
    pub entry: Entry,
    /// The submenus it is under, outermost first, joined: "Compress to", or "A › B". None at the top.
    pub path: Option<SharedString>,
    pub score: f64,
}

/// The most rows a filter shows.
pub const MOST: usize = 12;

/// The rows of `nodes` that match `query`, best first, at most [`MOST`]. An empty or all-space query finds nothing.
pub fn matches(nodes: &[Node], query: &str) -> Vec<Found> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let mut rows: Vec<(&Entry, Option<String>)> = Vec::new();
    collect(nodes, &mut Vec::new(), &mut rows);
    // The Suggested section holds copies: the real row, with its place in a submenu, is the one shown. A copy
    // is passed over when the same thing comes again later; of two plain rows the first one wins.
    let wanted: Vec<bool> = rows
        .iter()
        .enumerate()
        .map(|(i, (entry, _))| !(entry.smart && rows[i + 1..].iter().any(|(later, _)| later.run == entry.run)))
        .collect();
    let mut kept: Vec<(&Entry, &Option<String>)> = Vec::new();
    for ((entry, path), wanted) in rows.iter().zip(wanted) {
        if wanted && !kept.iter().any(|(taken, _)| taken.run == entry.run) {
            kept.push((entry, path));
        }
    }
    let mut found: Vec<Found> = kept
        .into_iter()
        .filter_map(|(entry, path)| {
            // Letters close together only: a menu's rows are short, and letters scattered across one ("pass" in
            // "In place (still opens normally)") are not what was meant.
            let by_label = fuzzy_score(query, &entry.label, true).map(|score| score + 2.0);
            let by_path = path
                .as_ref()
                .and_then(|path| fuzzy_score(query, &format!("{path} {}", entry.label), true));
            let score = match (by_label, by_path) {
                (Some(a), Some(b)) => a.max(b),
                (a, b) => a.or(b)?,
            };
            let mut entry = entry.clone();
            entry.reason = None;
            entry.smart = false;
            Some(Found {
                entry,
                path: path.as_ref().map(|path| SharedString::from(path.clone())),
                score,
            })
        })
        .collect();
    // A stable sort: rows with the same score stay in the menu's order.
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    found.truncate(MOST);
    found
}

/// The rows that can be chosen, in the menu's order, with the submenus they are under.
fn collect<'a>(nodes: &'a [Node], under: &mut Vec<&'a str>, out: &mut Vec<(&'a Entry, Option<String>)>) {
    for node in nodes {
        let path = (!under.is_empty()).then(|| under.join(" › "));
        match node {
            Node::Item(entry) if choosable(entry) => out.push((entry, path)),
            Node::Strip(entries) => {
                for entry in entries.iter().filter(|entry| choosable(entry)) {
                    out.push((entry, path.clone()));
                }
            }
            Node::Menu {
                label,
                disabled: false,
                items,
                ..
            } => {
                under.push(label);
                collect(items, under, out);
                under.pop();
            }
            _ => {}
        }
    }
}

/// A row that does something and isn't greyed out.
fn choosable(entry: &Entry) -> bool {
    !entry.disabled && entry.run != Run::Nothing
}

#[cfg(test)]
mod tests {
    use super::super::spec::Task;
    use super::*;

    /// A row that does something of its own: `which` makes its `run` differ from the others'.
    fn row(label: &str, which: &str) -> Entry {
        Entry::task(label.to_string().into(), IconName::File, Task::Navigate(which.to_string()))
    }

    fn item(label: &str, which: &str) -> Node {
        Node::Item(row(label, which))
    }

    fn submenu(label: &str, disabled: bool, items: Vec<Node>) -> Node {
        Node::Menu {
            label: label.to_string().into(),
            icon: None,
            disabled,
            items,
        }
    }

    fn labels(found: &[Found]) -> Vec<String> {
        found.iter().map(|f| f.entry.label.to_string()).collect()
    }

    #[test]
    fn an_empty_query_and_a_query_of_spaces_find_nothing() {
        let nodes = vec![item("Rename", "a")];
        assert!(matches(&nodes, "").is_empty());
        assert!(matches(&nodes, "   ").is_empty());
    }

    #[test]
    fn ren_finds_rename_in_the_strip() {
        let nodes = vec![
            Node::Strip(vec![row("Copy", "a"), row("Rename", "b")]),
            Node::Separator,
            item("Open", "c"),
        ];
        let found = matches(&nodes, "ren");
        assert_eq!(labels(&found), ["Rename"]);
        assert_eq!(found[0].path, None);
    }

    #[test]
    fn zip_finds_the_zip_file_inside_compress_to_with_its_path() {
        let nodes = vec![
            item("Open", "a"),
            submenu("Compress to", false, vec![item("Zip file", "z"), item("7z file", "7")]),
        ];
        let found = matches(&nodes, "zip");
        assert_eq!(labels(&found), ["Zip file"]);
        assert_eq!(found[0].path.as_deref(), Some("Compress to"));
    }

    #[test]
    fn a_row_two_submenus_deep_has_both_names_in_its_path() {
        let nodes = vec![submenu("A", false, vec![submenu("B", false, vec![item("Deep row", "d")])])];
        let found = matches(&nodes, "deep");
        assert_eq!(found[0].path.as_deref(), Some("A › B"));
    }

    #[test]
    fn typing_the_submenu_and_the_row_finds_it_and_a_label_match_beats_a_path_only_one() {
        let nodes = vec![
            submenu("PDF tools", false, vec![item("Add a password…", "p")]),
            submenu("Passwords", false, vec![item("Open", "o")]),
            item("Password", "w"),
        ];
        let found = matches(&nodes, "pdf pass");
        assert_eq!(labels(&found), ["Add a password…"]);
        assert_eq!(found[0].path.as_deref(), Some("PDF tools"));

        let found = matches(&nodes, "pass");
        assert!(found.len() >= 3, "{:?}", labels(&found));
        assert_eq!(found[0].entry.label.as_ref(), "Password");
        let path_only = found.iter().find(|f| f.entry.label.as_ref() == "Open").expect("Open");
        assert!(found[0].score > path_only.score);
    }

    #[test]
    fn greyed_rows_notes_headings_separators_and_disabled_submenus_are_never_found() {
        let nodes = vec![
            Node::Heading("Zip heading".into()),
            Node::Separator,
            Node::Item(row("Zip off", "a").disabled(true)),
            Node::Item(Entry::note("Zip note".into(), IconName::File)),
            submenu("Zip menu", true, vec![item("Zip inside", "b")]),
            submenu("Other", false, vec![Node::Item(row("Zip nested off", "c").disabled(true))]),
            item("Zip on", "d"),
        ];
        assert_eq!(labels(&matches(&nodes, "zip")), ["Zip on"]);
    }

    #[test]
    fn a_suggested_copy_is_found_once_as_the_row_further_down() {
        let mut copy = row("Zip file", "z");
        copy.smart = true;
        copy.reason = Some("Where you usually put zip files".into());
        let nodes = vec![
            Node::Heading("Suggested".into()),
            Node::Item(copy),
            Node::Separator,
            submenu("Compress to", false, vec![item("Zip file", "z")]),
        ];
        let found = matches(&nodes, "zip");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path.as_deref(), Some("Compress to"));
    }

    #[test]
    fn of_two_plain_rows_with_the_same_run_the_first_is_kept() {
        let nodes = vec![item("Zip first", "z"), item("Zip second", "z")];
        assert_eq!(labels(&matches(&nodes, "zip")), ["Zip first"]);
    }

    #[test]
    fn fifteen_matches_give_twelve_best_first_and_equal_scores_keep_the_menus_order() {
        let mut nodes: Vec<Node> = ('a'..='o').map(|c| item(&format!("Item {c}"), &c.to_string())).collect();
        let found = matches(&nodes, "item");
        assert_eq!(found.len(), MOST);
        let expected: Vec<String> = ('a'..='l').map(|c| format!("Item {c}")).collect();
        assert_eq!(labels(&found), expected);
        assert!(found.windows(2).all(|pair| pair[0].score >= pair[1].score));

        // A better match later in the menu comes first.
        nodes.push(item("Item", "exact"));
        let found = matches(&nodes, "item");
        assert_eq!(found.len(), MOST);
        assert!(found.windows(2).all(|pair| pair[0].score >= pair[1].score));
    }

    #[test]
    fn a_found_row_has_no_reason_and_is_not_smart() {
        let mut entry = row("Zip file", "z");
        entry.smart = true;
        entry.reason = Some("You often do this".into());
        let found = matches(&[Node::Item(entry)], "zip");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].entry.reason, None);
        assert!(!found[0].entry.smart);
    }

    #[test]
    fn case_does_not_matter_and_arabic_is_found_as_it_is_written() {
        let nodes = vec![item("Zip file", "z"), item("ضغط", "a")];
        assert_eq!(labels(&matches(&nodes, "ZIP")), ["Zip file"]);
        assert_eq!(labels(&matches(&nodes, "ضغط")), ["ضغط"]);
    }
}
