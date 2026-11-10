//! Headless tests of Quick Search: a fake source behind the view, the real window, real keys and clicks.
//! The window plumbing (showing, placing, hiding the Win32 window) is checked in the real app.

use super::search::Collector;
use super::*;
use crate::testing;
use glint_model::entries::{with_details, RawEntry};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::cell::RefCell;

/// What the view asked of its source.
#[derive(Default)]
struct Log {
    asked: Vec<(Vec<String>, String, Options)>,
    cancelled: usize,
    opened: Vec<String>,
    jumped: Vec<String>,
    dragged: Vec<Vec<String>>,
}

/// A source with a few drives and a corpus of names, answering at once.
struct FakeSource {
    drives: Vec<String>,
    corpus: Vec<FileEntry>,
    zoxide: Vec<(String, f64)>,
    open_answer: Result<Option<String>, String>,
    dialog: bool,
    jump_answer: bool,
    refuse: Option<String>,
    log: Rc<RefCell<Log>>,
}

impl FakeSource {
    fn new() -> Self {
        Self {
            drives: vec!["C:\\".into(), "D:\\".into()],
            corpus: vec![
                file(r"C:\Users\me\Documents\report.pdf", 2048),
                file(r"C:\Users\me\Documents\annual report.docx", 4096),
                folder(r"D:\Work\Reports"),
                file(r"C:\Users\me\notes.txt", 12),
            ],
            zoxide: Vec::new(),
            open_answer: Ok(None),
            dialog: false,
            jump_answer: true,
            refuse: None,
            log: Rc::default(),
        }
    }
}

impl Source for FakeSource {
    fn drives(&self) -> Fut<Vec<String>> {
        let drives = self.drives.clone();
        Box::pin(async move { drives })
    }

    fn find(
        &self,
        roots: Vec<String>,
        query: String,
        options: Options,
    ) -> Fut<Result<async_channel::Receiver<Found>, String>> {
        self.log.borrow_mut().asked.push((roots, query.clone(), options));
        if let Some(refused) = self.refuse.clone() {
            return Box::pin(async move { Err(refused) });
        }
        let words = search::words_of(&query);
        let matching: Vec<FileEntry> = self
            .corpus
            .iter()
            .filter(|e| words.iter().all(|w| e.lower.contains(w.as_str())))
            .cloned()
            .collect();
        let count = matching.len();
        let (sender, receiver) = async_channel::unbounded();
        let _ = sender.try_send(Found::Started { engine: "index".into() });
        let _ = sender.try_send(Found::Results(Collector::new(&query).add(matching)));
        let _ = sender.try_send(Found::Finished {
            count,
            elapsed: 4.0,
            limited: false,
            fuzzy: false,
        });
        Box::pin(async move { Ok(receiver) })
    }

    fn cancel(&self) {
        self.log.borrow_mut().cancelled += 1;
    }

    fn zoxide(&self) -> Fut<Vec<(String, f64)>> {
        let folders = self.zoxide.clone();
        Box::pin(async move { folders })
    }

    fn open(&self, path: String) -> Fut<Result<Option<String>, String>> {
        self.log.borrow_mut().opened.push(path);
        let answer = self.open_answer.clone();
        Box::pin(async move { answer })
    }

    fn over_dialog(&self) -> bool {
        self.dialog
    }

    fn jump(&self, folder: String) -> Fut<bool> {
        self.log.borrow_mut().jumped.push(folder);
        let answer = self.jump_answer;
        Box::pin(async move { answer })
    }

    fn drag(&self, _: &gpui_kit::App, paths: Vec<String>) -> Fut<Result<String, String>> {
        self.log.borrow_mut().dragged.push(paths);
        Box::pin(async { Ok("copy".to_string()) })
    }
}

fn entry(path: &str, dir: bool, size: u64) -> FileEntry {
    let name = path.rsplit('\\').next().unwrap_or(path).to_string();
    with_details(
        RawEntry {
            name,
            path: Some(path.to_string()),
            dir,
            size,
            ..Default::default()
        },
        "",
    )
}

fn file(path: &str, size: u64) -> FileEntry {
    entry(path, false, size)
}

fn folder(path: &str) -> FileEntry {
    entry(path, true, 0)
}

struct Rig {
    visual: VisualTestContext,
    view: Entity<QuickSearch>,
    log: Rc<RefCell<Log>>,
    events: Rc<RefCell<Vec<QuickSearchEvent>>>,
}

fn rig(cx: &mut TestAppContext, rtl: bool, source: FakeSource) -> Rig {
    cx.update(|cx| {
        testing::init(cx);
        init(cx);
    });
    // After init, which starts every test in English.
    testing::set_rtl(rtl);
    let log = source.log.clone();
    let source = Rc::new(source);
    let (mut visual, view) = testing::open_window(cx, move |window, cx| {
        cx.new(|cx| QuickSearch::with_source(source, window, cx))
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    visual.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &QuickSearchEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    let mut rig = Rig {
        visual,
        view,
        log,
        events,
    };
    // Summoned: the box has the keyboard.
    rig.visual.update(|window, cx| {
        window.activate_window();
        rig.view.update(cx, |quick, cx| quick.shown(window, cx));
    });
    rig.settle();
    rig
}

impl Rig {
    fn settle(&mut self) {
        for _ in 0..3 {
            self.visual.run_until_parked();
            self.visual.update(|window, cx| window.render_frame(cx));
        }
    }

    /// Time passes (the pause before a search, the "Copied path" note) and what was waiting for it runs.
    fn wait(&mut self, ms: u64) {
        self.visual.executor().advance_clock(Duration::from_millis(ms));
        self.settle();
    }

    fn type_text(&mut self, text: &str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.input(text, cx);
        });
        self.settle();
    }

    /// Types and waits for the search that follows.
    fn search(&mut self, text: &str) {
        self.type_text(text);
        self.wait(150);
    }

    fn press(&mut self, key: &str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        self.settle();
    }

    fn element(&mut self, label: &str) -> Option<gpui_kit::test::ElementSnapshot> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label)
        })
    }

    fn role(&mut self, label: &str) -> Option<Role> {
        self.element(label).and_then(|e| e.role())
    }

    fn has(&mut self, label: &str) -> bool {
        self.element(label).is_some()
    }

    /// The names of the results, top to bottom.
    fn rows(&mut self) -> Vec<String> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window
                .find_all_by_role(Role::ListBoxOption)
                .into_iter()
                .filter_map(|e| e.label().map(str::to_string))
                .collect()
        })
    }

    fn events(&self) -> Vec<QuickSearchEvent> {
        self.events.borrow().clone()
    }

    fn value(&mut self) -> String {
        let input = self.view.read_with(&self.visual, |quick, _| quick.input.clone());
        input.read_with(&self.visual, |input, _| input.value().to_string())
    }

    fn clipboard(&mut self) -> Option<String> {
        self.visual
            .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
    }
}

#[gpui_kit::test]
fn the_window_is_a_named_group_with_a_labelled_box_a_results_list_and_the_keys_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    assert_eq!(rig.role("Glint quick search"), Some(Role::Group));
    assert_eq!(rig.role("Results"), Some(Role::ListBox));
    assert!(rig.has("Search file names on every drive"), "the box is labelled");
    // Nothing typed and no used folders: it says what to type.
    assert_eq!(
        rig.role("Type part of a name. Filters work too: ext:pdf, size:>10mb, dm:7d"),
        Some(Role::Status)
    );
    // The keys the footer shows.
    for hint in [
        "Enter: open",
        "Alt+Enter: show in Glint",
        "Ctrl+C: copy path",
        "Esc: close",
    ] {
        assert_eq!(rig.role(hint), Some(Role::Group), "{hint}");
    }
    // English reads left to right: the search icon before the results' column.
    let (lead, results) = rig.visual.update(|window, cx| {
        window.render_frame(cx);
        (
            window.find("quick-search-lead").bounds(),
            window.find("quick-search-results").bounds(),
        )
    });
    assert!(lead.origin.x < results.origin.x + results.size.width / 2.);
}

#[gpui_kit::test]
fn typing_searches_after_a_pause_and_lists_the_best_names_first(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.type_text("report");
    // The pause hasn't passed: nothing was asked yet.
    assert!(rig.log.borrow().asked.is_empty());
    rig.wait(150);
    {
        let log = rig.log.borrow();
        assert_eq!(log.asked.len(), 1);
        let (roots, query, options) = &log.asked[0];
        assert_eq!(roots, &["C:\\".to_string(), "D:\\".to_string()], "every fixed drive");
        assert_eq!(query, "report");
        assert!(!options.hidden);
        assert_eq!(options.exclude, ["node_modules", ".git"], "the settings' exclusions");
    }
    // report.pdf is the name typed, "Reports" starts with it, "annual report" has it as a word.
    assert_eq!(rig.rows(), ["report.pdf", "Reports", "annual report.docx"]);
    assert_eq!(rig.role("report.pdf"), Some(Role::ListBoxOption));
    // The footer says who answered and how many.
    assert_eq!(rig.role("index · 3 found in 4.0 ms"), Some(Role::Status));
}

#[gpui_kit::test]
fn a_query_with_nothing_found_says_so_and_a_refused_one_says_why(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.search("zzzz");
    assert!(rig.rows().is_empty());
    // The fake found nothing and said "0 found"; the status line is what is shown.
    assert_eq!(rig.role("index · 0 found in 4.0 ms"), Some(Role::Status));

    let mut refusing = FakeSource::new();
    refusing.refuse = Some("Unknown size: huge".into());
    let mut rig = self::rig(cx, false, refusing);
    rig.search("size:huge");
    assert_eq!(rig.role("Unknown size: huge"), Some(Role::Status));
}

#[gpui_kit::test]
fn clearing_the_box_ends_the_search_and_clears_the_results(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.search("report");
    assert_eq!(rig.rows().len(), 3);
    rig.press("ctrl-a");
    rig.press("backspace");
    assert_eq!(rig.value(), "");
    assert_eq!(rig.log.borrow().cancelled, 1, "the search in flight was ended");
    assert!(rig.rows().is_empty());
    assert!(rig.has("Type part of a name. Filters work too: ext:pdf, size:>10mb, dm:7d"));
}

#[gpui_kit::test]
fn a_newer_query_replaces_an_older_one_that_was_still_waiting(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.type_text("rep");
    rig.wait(60);
    rig.type_text("o");
    rig.wait(60);
    assert!(rig.log.borrow().asked.is_empty(), "each key restarts the pause");
    rig.wait(100);
    let log = rig.log.borrow();
    assert_eq!(log.asked.len(), 1);
    assert_eq!(log.asked[0].1, "repo");
}

#[gpui_kit::test]
fn the_arrows_move_the_highlight_and_stop_at_the_ends(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.search("report");
    let selected = |rig: &mut Rig| -> Option<usize> { rig.view.read_with(&rig.visual, |q, _| Some(q.active_row())) };
    assert_eq!(selected(&mut rig), Some(0));
    rig.press("down");
    rig.press("down");
    rig.press("down");
    assert_eq!(selected(&mut rig), Some(2), "Down stops at the last result");
    rig.press("up");
    assert_eq!(selected(&mut rig), Some(1));
    rig.press("up");
    rig.press("up");
    assert_eq!(selected(&mut rig), Some(0), "Up stops at the first");
    // The keyboard stayed in the box all along.
    assert_eq!(rig.value(), "report");
}

#[gpui_kit::test]
fn enter_opens_a_file_with_its_app_and_puts_the_window_away(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.search("report");
    rig.press("enter");
    assert_eq!(rig.log.borrow().opened, [r"C:\Users\me\Documents\report.pdf"]);
    assert_eq!(rig.events(), [QuickSearchEvent::Hide]);
}

#[gpui_kit::test]
fn a_shortcut_to_a_folder_opens_that_folder_in_glint_and_a_failure_shows_the_file(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut source = FakeSource::new();
    source.open_answer = Ok(Some(r"D:\Work".into()));
    let mut rig = rig(cx, false, source);
    rig.search("report");
    rig.press("enter");
    assert_eq!(
        rig.events().last(),
        Some(&QuickSearchEvent::Reveal {
            path: r"D:\Work".into(),
            dir: true
        })
    );

    let mut source = FakeSource::new();
    source.open_answer = Err("It isn't there anymore.".into());
    let mut rig = self::rig(cx, false, source);
    rig.search("report");
    rig.press("enter");
    assert_eq!(
        rig.events().last(),
        Some(&QuickSearchEvent::Reveal {
            path: r"C:\Users\me\Documents\report.pdf".into(),
            dir: false
        })
    );
}

#[gpui_kit::test]
fn alt_enter_and_enter_on_a_folder_show_it_in_glint(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.search("report");
    rig.press("alt-enter");
    assert_eq!(
        rig.events(),
        [QuickSearchEvent::Reveal {
            path: r"C:\Users\me\Documents\report.pdf".into(),
            dir: false
        }]
    );
    assert!(rig.log.borrow().opened.is_empty(), "Alt+Enter doesn't open the file");
    // A folder opens in Glint on Enter.
    rig.press("down");
    rig.press("enter");
    assert_eq!(
        rig.events().last(),
        Some(&QuickSearchEvent::Reveal {
            path: r"D:\Work\Reports".into(),
            dir: true
        })
    );
}

#[gpui_kit::test]
fn summoned_over_a_file_dialog_a_folder_goes_to_the_dialog(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut source = FakeSource::new();
    source.dialog = true;
    let mut rig = rig(cx, false, source);
    // The footer says what Enter does with a folder now.
    assert!(rig.has("Search file names on every drive"));
    rig.search("reports");
    rig.press("enter");
    assert_eq!(rig.log.borrow().jumped, [r"D:\Work\Reports"]);
    assert_eq!(
        rig.events(),
        [QuickSearchEvent::Hide],
        "the dialog took it: nothing opens in Glint"
    );

    // The dialog has gone: the folder opens in Glint instead.
    let mut source = FakeSource::new();
    source.dialog = true;
    source.jump_answer = false;
    let mut rig = self::rig(cx, false, source);
    rig.search("reports");
    rig.press("enter");
    assert_eq!(
        rig.events(),
        [QuickSearchEvent::Reveal {
            path: r"D:\Work\Reports".into(),
            dir: true
        }]
    );
    // Alt+Enter always means Glint, dialog or not.
    let mut source = FakeSource::new();
    source.dialog = true;
    let mut rig = self::rig(cx, false, source);
    rig.search("reports");
    rig.press("alt-enter");
    assert!(rig.log.borrow().jumped.is_empty());
}

#[gpui_kit::test]
fn escape_puts_the_window_away_and_so_does_losing_the_foreground(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.press("escape");
    assert_eq!(rig.events(), [QuickSearchEvent::Hide]);

    // Another window taking the foreground: the window was active, now it isn't.
    let mut rig = self::rig(cx, false, FakeSource::new());
    assert!(
        rig.view.read_with(&rig.visual, |quick, _| quick.was_active),
        "it was the active window"
    );
    rig.visual.deactivate_window();
    rig.settle();
    assert_eq!(rig.events(), [QuickSearchEvent::Hide]);
    assert!(!rig.view.read_with(&rig.visual, |quick, _| quick.was_active));
}

#[gpui_kit::test]
fn before_anything_is_typed_the_folders_used_most_are_offered(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut source = FakeSource::new();
    source.zoxide = vec![(r"C:\Users\me\Projects".into(), 9.0), (r"D:\Work".into(), 3.0)];
    let mut rig = rig(cx, false, source);
    // History is shared with zoxide (the setting is read each time the window is summoned).
    rig.visual.update(|window, cx| {
        state::update(cx, |s| s.zoxide = true);
        rig.view.update(cx, |quick, cx| quick.shown(window, cx));
    });
    rig.settle();
    assert!(rig.has("Folders you use most"));
    assert_eq!(rig.rows(), ["Projects", "Work"]);
    assert_eq!(rig.role("Projects"), Some(Role::ListBoxOption));
    // Enter opens the highlighted folder in Glint.
    rig.press("down");
    rig.press("enter");
    assert_eq!(
        rig.events(),
        [QuickSearchEvent::Reveal {
            path: r"D:\Work".into(),
            dir: true
        }]
    );
    // Typing replaces them with results, and the heading goes.
    rig.search("notes");
    assert_eq!(rig.rows(), ["notes.txt"]);
    assert!(!rig.has("Folders you use most"));
}

#[gpui_kit::test]
fn plain_words_offer_filters_that_tab_or_a_click_takes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.type_text("invoice pdfs");
    let chips = glint_model::plain::plain_query("invoice pdfs")
        .map(|p| p.chips.join(", "))
        .unwrap_or_default();
    assert!(!chips.is_empty());
    assert_eq!(rig.role(&format!("Use these filters: {chips}")), Some(Role::Button));
    rig.press("tab");
    assert_eq!(rig.value(), "invoice ext:pdf");
    // Taken: the filters are what is searched now, and nothing more is offered.
    rig.wait(150);
    assert_eq!(
        rig.log.borrow().asked.last().map(|a| a.1.clone()),
        Some("invoice ext:pdf".to_string())
    );
    assert!(!rig.has(&format!("Use these filters: {chips}")));
}

#[gpui_kit::test]
fn ctrl_c_copies_the_highlighted_path_unless_the_box_has_a_selection(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.search("report");
    rig.press("ctrl-c");
    assert_eq!(rig.clipboard().as_deref(), Some(r"C:\Users\me\Documents\report.pdf"));
    assert_eq!(rig.role("Copied path"), Some(Role::Status));
    // The note goes after a moment.
    rig.wait(1500);
    assert!(!rig.has("Copied path"));

    // With the typed text selected, Ctrl+C copies that, as a text box does.
    rig.press("ctrl-a");
    rig.press("ctrl-c");
    assert_eq!(rig.clipboard().as_deref(), Some("report"));
    assert!(!rig.has("Copied path"));
}

#[gpui_kit::test]
fn a_click_opens_a_result_and_an_alt_click_shows_it_in_glint(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.search("report");
    rig.visual.update(|window, cx| {
        window.render_frame(cx);
        window.click(("quick-search-row", 2usize), cx);
    });
    rig.settle();
    assert_eq!(rig.log.borrow().opened, [r"C:\Users\me\Documents\annual report.docx"]);
    rig.visual.update(|window, cx| {
        window.render_frame(cx);
        window.click_with_modifiers(
            ("quick-search-row", 0usize),
            Modifiers {
                alt: true,
                ..Default::default()
            },
            cx,
        );
    });
    rig.settle();
    assert_eq!(
        rig.events().last(),
        Some(&QuickSearchEvent::Reveal {
            path: r"C:\Users\me\Documents\report.pdf".into(),
            dir: false
        })
    );
}

#[gpui_kit::test]
fn hovering_a_result_highlights_it_and_dragging_one_out_starts_a_drag(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.search("report");
    rig.visual.update(|window, cx| {
        window.render_frame(cx);
        window.hover(("quick-search-row", 1usize), cx);
    });
    rig.settle();
    assert_eq!(rig.view.read_with(&rig.visual, |q, _| q.active_row()), 1);
    // Pressed on the first result and moved well past the threshold.
    rig.visual.update(|window, cx| {
        window.render_frame(cx);
        window.drag_to(("quick-search-row", 0usize), ("quick-search-row", 2usize), cx);
    });
    rig.settle();
    assert_eq!(
        rig.log.borrow().dragged,
        [vec![r"C:\Users\me\Documents\report.pdf".to_string()]]
    );
    // A press and release in place is a click, not a drag.
    assert_eq!(rig.log.borrow().dragged.len(), 1);
}

#[gpui_kit::test]
fn arabic_names_are_found_marked_and_named_as_they_are(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut source = FakeSource::new();
    source.corpus = vec![
        file("C:\\مستندات\\تقرير الربع 1.pdf", 5000),
        folder("D:\\عمل\\تقارير"),
        file(r"C:\Users\me\Documents\report.pdf", 1),
    ];
    let mut rig = rig(cx, true, source);
    rig.search("تقرير");
    // The names are exposed as they are, without the layout-only isolates.
    assert_eq!(rig.rows(), ["تقرير الربع 1.pdf"]);
    assert_eq!(rig.role("تقرير الربع 1.pdf"), Some(Role::ListBoxOption));
    // The window mirrors: the search icon is at the start edge, which is the right in Arabic.
    let (lead, input) = rig.visual.update(|window, cx| {
        window.render_frame(cx);
        (
            window.find("quick-search-lead").bounds(),
            window.find("quick-search-results").bounds(),
        )
    });
    assert!(lead.origin.x > input.origin.x, "the icon is on the right in Arabic");
    // Enter on the Arabic file opens it by its path.
    rig.press("enter");
    assert_eq!(rig.log.borrow().opened, ["C:\\مستندات\\تقرير الربع 1.pdf"]);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn summoned_over_a_file_dialog_the_footer_says_what_enter_does_with_a_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut over_dialog = FakeSource::new();
    over_dialog.dialog = true;
    let mut rig = rig(cx, false, over_dialog);
    assert!(rig.has("Enter: open (a folder: in the dialog)"));
    assert!(!rig.has("Enter: open"));
}

#[gpui_kit::test]
fn summoning_again_selects_the_last_query_to_type_over(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, FakeSource::new());
    rig.search("report");
    assert_eq!(rig.rows().len(), 3);
    // Put away and brought up again: the results are still there, the text is selected.
    rig.visual
        .update(|window, cx| rig.view.update(cx, |quick, cx| quick.shown(window, cx)));
    rig.settle();
    assert_eq!(rig.rows().len(), 3);
    rig.search("notes");
    assert_eq!(rig.value(), "notes", "typing replaced the selected query");
    assert_eq!(rig.rows(), ["notes.txt"]);
}

#[gpui_kit::test]
fn a_long_list_draws_only_the_rows_in_view_and_the_arrows_scroll_to_the_highlight(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut source = FakeSource::new();
    source.corpus = (0..150)
        .map(|n| file(&format!(r"C:\many\note-{n:03}.txt"), 10))
        .collect();
    let mut rig = rig(cx, false, source);
    rig.search("note");
    // At most the best hundred are kept, and far fewer than that are drawn.
    let drawn = rig.rows();
    assert!(!drawn.is_empty() && drawn.len() < 30, "{} rows drawn", drawn.len());
    assert_eq!(drawn[0], "note-000.txt");
    for _ in 0..60 {
        rig.press("down");
    }
    assert!(
        rig.rows().contains(&"note-060.txt".to_string()),
        "the list scrolled to the highlight"
    );
    assert!(!rig.rows().contains(&"note-000.txt".to_string()));
    let total = rig.view.read_with(&rig.visual, |quick, _| quick.found.len());
    assert_eq!(total, 100);
}
