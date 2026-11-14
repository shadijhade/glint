//! Headless tests of searching inside files as part of a search: a fake disk behind the same `Source` the app uses,
//! and the test saying what core's content search says (`Explorer::inside_heard`), when it says it.

use super::fake::*;
use super::*;
use crate::testing;
use gpui_kit::{AppContext as _, Entity, TestAppContext};

struct Rig {
    explorer: Entity<Explorer>,
    disk: FakeDisk,
}

/// `C:\proj` with a few files, one of them named like what the tests look for.
fn rig(cx: &mut TestAppContext) -> Rig {
    cx.update(testing::init);
    let disk = FakeDisk::new();
    disk.folder("C:\\", vec![folder("C:\\", "proj")]);
    disk.folder(
        "C:\\proj",
        vec![
            folder("C:\\proj", "src"),
            file("C:\\proj", "needle-notes.txt", 10, 1),
            file("C:\\proj", "readme.md", 20, 2),
            file("C:\\proj", "report.pdf", 30, 3),
        ],
    );
    disk.folder(
        "C:\\proj\\src",
        vec![
            file("C:\\proj\\src", "main.rs", 40, 4),
            file("C:\\proj\\src", "lib.rs", 50, 5),
        ],
    );
    // The tests say what the content search finds.
    disk.live_inside();
    let explorer = cx.update(|cx| cx.new(|cx| Explorer::new(disk.source(), cx)));
    Rig { explorer, disk }
}

impl Rig {
    fn go(&self, cx: &mut TestAppContext, path: &str) {
        let task = self
            .explorer
            .update(cx, |e, cx| e.navigate_task(path, NavOptions::default(), cx));
        assert!(cx.foreground_executor().block_test(task));
    }

    /// Searches the active tab; the names are in when this returns, the contents still being searched.
    fn find(&self, cx: &mut TestAppContext, query: &str, wider: Wider) {
        // As the app does it: the words are in the box (where they filter what is shown by name), then Enter.
        let task = self.explorer.update(cx, |e, cx| {
            e.set_filter(query, None, cx);
            e.run_find(query, wider, cx)
        });
        cx.foreground_executor().block_test(task);
        cx.run_until_parked();
    }

    /// The id core's content search was started with, last.
    fn inside_id(&self) -> String {
        self.disk
            .inside_searches()
            .last()
            .map(|(_, request)| request.id.clone())
            .expect("a search inside files was started")
    }

    /// What the content search says, applied.
    fn say(&self, cx: &mut TestAppContext, said: glint_core::TextSaid) {
        let event = inside_event(&self.inside_id(), said);
        self.explorer.update(cx, |e, cx| e.inside_heard(&event, cx));
        for _ in 0..4 {
            cx.run_until_parked();
        }
        assert!(self.explorer.read_with(cx, |e, _| e.inside_idle()));
    }

    fn run_of(&self, cx: &TestAppContext) -> FindRun {
        self.explorer
            .read_with(cx, |e, _| e.active_tab().and_then(|t| t.find.clone()))
            .expect("a search is shown")
    }

    /// The list's lines, as words: a header as "# name", a row by its name, a line under a row as "  n: text"
    /// (or "  +n" for the count of the ones not shown).
    fn lines(&self, cx: &TestAppContext) -> Vec<String> {
        self.explorer.read_with(cx, |e, _| {
            let rows = &e.active_tab().expect("a tab").rows;
            (0..rows.line_count())
                .map(|line| match rows.line(line) {
                    Line::Row(i) => rows.get(i).map(|entry| entry.name.clone()).unwrap_or_default(),
                    Line::Header(_, header) => format!("# {} ({})", header.label, header.count),
                    Line::Hit(at) if at.more > 0 => format!("  +{}", at.more),
                    Line::Hit(at) => match rows.hit(at) {
                        Some((_, hit)) => format!(
                            "  {}: {}",
                            hit.line.unwrap_or(0),
                            hit.parts.iter().map(|p| p.text.as_str()).collect::<String>()
                        ),
                        None => "  ?".to_string(),
                    },
                    Line::Blank => String::new(),
                })
                .collect()
        })
    }
}

#[gpui_kit::test]
fn a_search_looks_inside_the_files_too_and_lists_them_with_their_lines(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\proj");
    r.find(cx, "needle", Wider::default());
    // The name search is done; ripgrep was asked for the same word in the same folder, and still runs.
    let asked = r.disk.inside_searches();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].0, InsideEngine::Ripgrep);
    assert_eq!(
        (asked[0].1.query.as_str(), asked[0].1.path.as_str()),
        ("needle", "C:\\proj")
    );
    let run = r.run_of(cx);
    assert!(!run.running && run.inside.running);
    assert_eq!(r.lines(cx), ["needle-notes.txt"]);

    // Two files hold it: one four times, and the one the name search listed, once.
    r.say(
        cx,
        glint_core::TextSaid::Matches(vec![
            inside_match("C:\\proj\\src\\main.rs", "    let needle = 1;\n", 3, 8, 14),
            inside_match("C:\\proj\\src\\main.rs", "needle();\n", 9, 0, 6),
            inside_match("C:\\proj\\src\\main.rs", "// needle\n", 12, 3, 9),
            inside_match("C:\\proj\\src\\main.rs", "end of needle\n", 20, 7, 13),
            inside_match("C:\\proj\\needle-notes.txt", "a needle in here\n", 1, 2, 8),
        ]),
    );
    assert_eq!(
        r.lines(cx),
        [
            "# Names (1)",
            "needle-notes.txt",
            "  1: a needle in here",
            "# Inside files (1)",
            "main.rs",
            "  3: let needle = 1;",
            "  9: needle();",
            "  12: // needle",
            "  +1",
        ]
    );
    let run = r.run_of(cx);
    assert_eq!((run.inside.matches, run.inside.files()), (5, 2));
    assert_eq!(
        run.entries.len(),
        2,
        "the file found by its contents is a result like any other"
    );
    assert!(run.inside.only.contains("C:\\proj\\src\\main.rs") && run.inside.only.len() == 1);
    assert!(run.inside.running);

    r.say(cx, glint_core::TextSaid::Finished);
    let run = r.run_of(cx);
    assert!(!run.inside.running && run.inside.error.is_empty() && !run.inside.limited);
}

#[gpui_kit::test]
fn the_filters_of_the_search_narrow_the_files_found_by_their_contents(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\proj");
    r.find(cx, "ext:rs needle", Wider::default());
    // Only the word is looked for inside the files.
    assert_eq!(r.disk.inside_searches()[0].1.query, "needle");
    r.say(
        cx,
        glint_core::TextSaid::Matches(vec![
            inside_match("C:\\proj\\src\\main.rs", "needle\n", 1, 0, 6),
            inside_match("C:\\proj\\readme.md", "needle\n", 2, 0, 6),
        ]),
    );
    assert_eq!(r.lines(cx), ["# Inside files (1)", "main.rs", "  1: needle"]);
    assert_eq!(r.run_of(cx).inside.matches, 1, "the Markdown file's line isn't counted");
}

#[gpui_kit::test]
fn a_search_with_nothing_to_look_for_inside_files_asks_no_content_search(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\proj");
    for query in ["*.rs", "ext:pdf", "src\\main", "folder: notes"] {
        r.find(cx, query, Wider::default());
        assert!(r.disk.inside_searches().is_empty(), "{query}");
        assert!(r.run_of(cx).inside.words.is_empty(), "{query}");
    }
}

#[gpui_kit::test]
fn a_whole_drive_is_searched_inside_only_when_asked(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\");
    r.find(cx, "needle", Wider::default());
    assert!(
        r.disk.inside_searches().is_empty(),
        "a drive's every file isn't read unasked"
    );
    let run = r.run_of(cx);
    assert!(run.inside.offered && !run.inside.running);

    // "Inside files too".
    r.find(cx, "needle", Wider::in_scope("C:\\").inside_too());
    assert_eq!(r.disk.inside_searches().len(), 1);
    let run = r.run_of(cx);
    assert!(run.inside.running && !run.inside.offered);
}

#[gpui_kit::test]
fn a_newer_search_and_leaving_the_results_end_the_content_search(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\proj");
    r.find(cx, "needle", Wider::default());
    let first = r.inside_id();
    let stops = r.disk.inside_cancels();
    r.find(cx, "report", Wider::default());
    assert!(
        r.disk.inside_cancels() > stops,
        "the older content search was told to stop"
    );
    assert_ne!(r.inside_id(), first);
    // What the older search still says is nobody's.
    let stale = inside_event(
        &first,
        glint_core::TextSaid::Matches(vec![inside_match("C:\\proj\\readme.md", "needle\n", 1, 0, 6)]),
    );
    r.explorer.update(cx, |e, cx| e.inside_heard(&stale, cx));
    cx.run_until_parked();
    assert_eq!(r.run_of(cx).inside.matches, 0);

    // Stop keeps what was found and the names.
    r.say(
        cx,
        glint_core::TextSaid::Matches(vec![inside_match("C:\\proj\\readme.md", "the report\n", 4, 4, 10)]),
    );
    let tab = r.explorer.read_with(cx, |e, _| e.active_tab_id());
    r.explorer.update(cx, |e, cx| e.stop_inside(tab, cx));
    cx.run_until_parked();
    let run = r.run_of(cx);
    assert!(!run.inside.running);
    assert_eq!(run.inside.matches, 1);
    assert_eq!(
        r.lines(cx),
        [
            "# Names (1)",
            "report.pdf",
            "# Inside files (1)",
            "readme.md",
            "  4: the report"
        ]
    );

    // Back to the folder: nothing of the search is left.
    r.explorer.update(cx, |e, cx| e.clear_find(cx));
    cx.run_until_parked();
    assert!(r
        .explorer
        .read_with(cx, |e, _| e.active_tab().is_some_and(|t| t.find.is_none())));
}

#[gpui_kit::test]
fn a_content_search_that_fails_says_why_and_keeps_the_names(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\proj");
    r.find(cx, "needle", Wider::default());
    r.say(cx, glint_core::TextSaid::Error("regex parse error".into()));
    let run = r.run_of(cx);
    assert!(!run.inside.running);
    assert_eq!(run.inside.error, "regex parse error");
    assert_eq!(r.lines(cx), ["needle-notes.txt"]);

    // One that can't start at all says so the same way.
    r.disk.refuse_next_inside("ripgrep is missing");
    r.find(cx, "needle", Wider::default());
    let run = r.run_of(cx);
    assert!(!run.inside.running);
    assert_eq!(run.inside.error, "ripgrep is missing");
}

#[gpui_kit::test]
fn a_search_as_it_is_typed_looks_inside_only_where_the_content_index_answers(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    // The name index has the drive, so names are searched as they are typed.
    r.disk.set_search_index(Some(SearchIndex {
        on: true,
        ready: vec!["C:".into()],
        ..Default::default()
    }));
    r.go(cx, "C:\\proj");
    let refreshed = r.explorer.update(cx, |e, cx| e.refresh_search_index(cx));
    cx.foreground_executor().block_test(refreshed);
    let typed = r.explorer.update(cx, |e, cx| {
        e.set_filter("needle", None, cx);
        e.find_as_typed("needle", cx)
    });
    cx.foreground_executor().block_test(typed);
    cx.run_until_parked();
    assert!(r.run_of(cx).entries.len() == 1, "the names came");
    assert!(r.disk.inside_searches().is_empty(), "ripgrep waits for Enter");

    // With the content index covering the folder, the contents are asked as typed too, of the index.
    r.disk.set_inside_covered(true);
    let typed = r.explorer.update(cx, |e, cx| {
        e.set_filter("needles", None, cx);
        e.find_as_typed("needles", cx)
    });
    cx.foreground_executor().block_test(typed);
    cx.run_until_parked();
    let asked = r.disk.inside_searches();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].0, InsideEngine::Index);
}

#[gpui_kit::test]
fn the_rows_keep_a_grouping_and_put_the_lines_under_their_files(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\proj");
    r.find(cx, "needle", Wider::default());
    r.say(
        cx,
        glint_core::TextSaid::Matches(vec![inside_match(
            "C:\\proj\\src\\lib.rs",
            "pub fn needle() {}\n",
            7,
            7,
            13,
        )]),
    );
    r.say(cx, glint_core::TextSaid::Finished);
    r.explorer
        .update(cx, |e, cx| e.set_group(glint_model::groups::GroupBy::Type, cx));
    cx.run_until_parked();
    let lines = r.lines(cx);
    // Grouped by type, there are no "Names" and "Inside files" parts; the line still follows its file.
    assert!(lines
        .iter()
        .all(|line| !line.starts_with("# Names") && !line.starts_with("# Inside files")));
    let at = lines
        .iter()
        .position(|line| line == "lib.rs")
        .expect("the file found inside is a row");
    assert_eq!(lines[at + 1], "  7: pub fn needle() {}");
}
