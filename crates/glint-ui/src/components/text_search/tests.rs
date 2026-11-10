//! Headless tests of text search: a fake source behind the view, the real window, real keys and clicks, the
//! saved state in a temp file, and the Arabic interface. The installs (ripgrep, ast-grep, the meaning model)
//! are Settings' `Tools` model drawn by `tools::needed`, which Settings' own tests cover; Settings' backend
//! can't be faked from outside it (its progress type isn't exported), so no test here draws those cards.

use super::*;
use crate::testing;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::collections::HashMap;

/// What the view asked of its source.
#[derive(Default)]
struct Log {
    covers: Vec<String>,
    started: Vec<(Engine, Request)>,
    cancelled: usize,
    rewrites: Vec<Request>,
    opened: Vec<(String, Option<String>)>,
    in_explorer: Vec<String>,
    written: Vec<(String, String)>,
    folder_picks: Vec<Option<String>>,
}

/// A source with a few files, answering at once.
struct FakeSource {
    log: Rc<RefCell<Log>>,
    files: Rc<RefCell<HashMap<String, String>>>,
    covered: bool,
    start_error: Option<String>,
    folder_as_searched: Option<String>,
    documents: Vec<(String, String)>,
    pick_folder: Option<String>,
    pick_export: Option<String>,
    rewritten: u64,
}

impl FakeSource {
    fn new() -> Self {
        Self {
            log: Rc::default(),
            files: Rc::default(),
            covered: false,
            start_error: None,
            folder_as_searched: None,
            documents: Vec::new(),
            pick_folder: None,
            pick_export: None,
            rewritten: 0,
        }
    }

    fn with_file(self, path: &str, text: &str) -> Self {
        self.files.borrow_mut().insert(path.to_string(), text.to_string());
        self
    }
}

impl Source for FakeSource {
    fn covers(&self, path: &str) -> Fut<bool> {
        self.log.borrow_mut().covers.push(path.to_string());
        let covered = self.covered;
        Box::pin(async move { Ok(covered) })
    }

    fn start(&self, engine: Engine, request: Request) -> Fut<String> {
        let answer = match &self.start_error {
            Some(reason) => Err(reason.clone()),
            None => Ok(self.folder_as_searched.clone().unwrap_or_else(|| request.path.clone())),
        };
        self.log.borrow_mut().started.push((engine, request));
        Box::pin(async move { answer })
    }

    fn cancel(&self) {
        self.log.borrow_mut().cancelled += 1;
    }

    fn rewrite(&self, request: Request) -> Fut<u64> {
        self.log.borrow_mut().rewrites.push(request);
        let n = self.rewritten;
        Box::pin(async move { Ok(n) })
    }

    fn read(&self, path: &str) -> Fut<String> {
        let answer = self
            .files
            .borrow()
            .get(path)
            .cloned()
            .ok_or_else(|| format!("{path} isn't there"));
        Box::pin(async move { answer })
    }

    fn write(&self, path: &str, content: String) -> Fut<()> {
        self.log.borrow_mut().written.push((path.to_string(), content.clone()));
        self.files.borrow_mut().insert(path.to_string(), content);
        Box::pin(async { Ok(()) })
    }

    fn chunk(&self, path: &str, from: usize, to: usize) -> Fut<String> {
        let answer = self
            .files
            .borrow()
            .get(path)
            .map(|text| {
                text.lines()
                    .skip(from.saturating_sub(1))
                    .take(to + 1 - from)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .ok_or_else(|| format!("{path} isn't there"));
        Box::pin(async move { answer })
    }

    fn doc_lines(&self, _: &str, _: usize, _: usize) -> Fut<Vec<(String, String)>> {
        let lines = self.documents.clone();
        Box::pin(async move { Ok(lines) })
    }

    fn open(&self, path: &str, editor: Option<&str>) -> Fut<()> {
        self.log
            .borrow_mut()
            .opened
            .push((path.to_string(), editor.map(str::to_string)));
        Box::pin(async { Ok(()) })
    }

    fn show_in_file_explorer(&self, path: &str) {
        self.log.borrow_mut().in_explorer.push(path.to_string());
    }

    fn pick_folder(&self, _: String, start: Option<String>, _: isize) -> Fut<Option<String>> {
        self.log.borrow_mut().folder_picks.push(start);
        let answer = self.pick_folder.clone();
        Box::pin(async move { Ok(answer) })
    }

    fn pick_export(&self, _: String, _: isize) -> Fut<Option<String>> {
        let answer = self.pick_export.clone();
        Box::pin(async move { Ok(answer) })
    }
}

/// One of ripgrep's `match` lines.
fn rg_match(path: &str, text: &str, number: u64, needle: &str) -> String {
    let hits: Vec<serde_json::Value> = text
        .match_indices(needle)
        .map(|(start, found)| {
            serde_json::json!({ "match": { "text": found }, "start": start, "end": start + found.len() })
        })
        .collect();
    serde_json::json!({
        "type": "match",
        "data": {
            "path": { "text": path },
            "lines": { "text": format!("{text}\n") },
            "line_number": number,
            "absolute_offset": 0,
            "submatches": hits
        }
    })
    .to_string()
}

/// ripgrep's closing summary.
fn rg_summary(millis: u64, searched: u64) -> String {
    serde_json::json!({
        "type": "summary",
        "data": {
            "elapsed_total": { "secs": millis / 1000, "nanos": (millis % 1000) * 1_000_000, "human": "x" },
            "stats": { "searches": searched, "unreadable": 0 }
        }
    })
    .to_string()
}

fn state_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("glint-text-search-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp folder");
    let file = dir.join(format!("{name}.json"));
    let _ = std::fs::remove_file(&file);
    file
}

struct Rig {
    visual: VisualTestContext,
    view: Entity<TextSearch>,
    search: Entity<Search>,
    log: Rc<RefCell<Log>>,
    files: Rc<RefCell<HashMap<String, String>>>,
    events: Rc<RefCell<Vec<TextSearchEvent>>>,
}

fn rig(cx: &mut TestAppContext, name: &str, rtl: bool, source: FakeSource) -> Rig {
    rig_seeded(cx, name, rtl, source, |_| {})
}

/// A rig whose saved state `seed` fills before the view is made.
fn rig_seeded(cx: &mut TestAppContext, name: &str, rtl: bool, source: FakeSource, seed: impl FnOnce(&mut App)) -> Rig {
    cx.update(|cx| {
        testing::init(cx);
        persistence::init_at(cx, state_file(name));
        seed(cx);
    });
    // After init, which starts every test in English.
    testing::set_rtl(rtl);
    let (log, files) = (source.log.clone(), source.files.clone());
    let source: Rc<dyn Source> = Rc::new(source);
    let (mut visual, view) = testing::open_window(cx, move |window, cx| {
        cx.new(|cx| TextSearch::with_source(source, None, window, cx))
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    visual.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &TextSearchEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    let search = view.read_with(&visual, |view, _| view.search.clone());
    let mut rig = Rig {
        visual,
        view,
        search,
        log,
        files,
        events,
    };
    // The query has the keyboard, as it has when the window shows this view.
    rig.visual.update(|window, cx| {
        window.activate_window();
        rig.view.update(cx, |view, cx| view.focus_query(window, cx));
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

    /// Lets work on other threads (the parser, core's runtime) come back until `done`.
    fn settle_until(&mut self, mut done: impl FnMut(&mut Rig) -> bool) {
        for _ in 0..400 {
            self.settle();
            if done(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("what the test waited for never happened");
    }

    /// A window wide enough for the extension chips (`lg:`).
    fn wide(&mut self) {
        self.visual.simulate_resize(size(px(1200.), px(700.)));
        self.settle();
    }

    fn press(&mut self, key: &str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        self.settle();
    }

    fn click(&mut self, id: impl Into<ElementId>) {
        let id = id.into();
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
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

    fn labels_of(&mut self, role: Role) -> Vec<String> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window
                .find_all_by_role(role)
                .into_iter()
                .filter_map(|e| e.label().map(str::to_string))
                .collect()
        })
    }

    /// The list's rows, top to bottom: a file's name or a match's text.
    fn rows(&mut self) -> Vec<String> {
        self.labels_of(Role::ListBoxOption)
    }

    /// Types into the query box, which has the keyboard.
    fn type_query(&mut self, text: &str) {
        self.visual.update(|window, cx| {
            self.view.update(cx, |view, cx| view.focus_query(window, cx));
            window.render_frame(cx);
            window.input(text, cx);
        });
        self.settle();
    }

    fn set_query(&mut self, text: &str) {
        let query = self.view.read_with(&self.visual, |view, _| view.query.clone());
        self.visual
            .update(|window, cx| query.update(cx, |query, cx| query.set_value(text.to_string(), window, cx)));
        self.settle();
    }

    fn set_folder(&mut self, folder: &str) {
        self.visual
            .update(|window, cx| self.view.update(cx, |view, cx| view.set_folder(folder, window, cx)));
        self.settle();
    }

    /// Searches `query` in `folder` and waits for the source to be asked.
    fn search(&mut self, query: &str, folder: &str) {
        let asked = self.log.borrow().started.len();
        self.set_folder(folder);
        self.set_query(query);
        self.visual
            .update(|window, cx| self.view.update(cx, |view, cx| view.search_now(window, cx)));
        self.settle_until(|rig| rig.log.borrow().started.len() > asked);
    }

    fn run_id(&mut self) -> String {
        self.search.read_with(&self.visual, |search, _| {
            search.run().map(|r| r.id.clone()).unwrap_or_default()
        })
    }

    /// An event of search `id`, parsed off the UI thread and applied.
    fn event_of(&mut self, id: &str, kind: &str, lines: Vec<String>) {
        let event = RawEvent {
            id: id.to_string(),
            kind: kind.to_string(),
            lines,
        };
        self.search.update(&mut self.visual, |search, _| search.receive(event));
        self.settle_until(|rig| rig.search.read_with(&rig.visual, |s, _| s.idle()));
    }

    /// An event of the search on screen.
    fn event(&mut self, kind: &str, lines: Vec<String>) {
        let id = self.run_id();
        self.event_of(&id, kind, lines);
    }

    /// The search finds `lines` and ends, 12 ms after it began.
    fn finish_with(&mut self, mut lines: Vec<String>) {
        lines.push(rg_summary(12, 3));
        self.event("data", lines);
        self.event("finished", vec![]);
    }

    fn matches(&mut self) -> usize {
        self.search.read_with(&self.visual, |s, _| s.matches().len())
    }

    fn options(&mut self) -> SearchOptions {
        self.visual.update(|_, cx| TextSearch::options(cx))
    }

    fn events(&self) -> Vec<TextSearchEvent> {
        self.events.borrow().clone()
    }

    fn history(&mut self) -> Vec<(String, String)> {
        self.visual.update(|_, cx| {
            persistence::app(cx)
                .map(|app| app.history.iter().map(|h| (h.query.clone(), h.path.clone())).collect())
                .unwrap_or_default()
        })
    }
}

const FOLDER: &str = "C:\\proj";

/// A finished search of "hello" with three matches in two files.
fn three_matches(rig: &mut Rig) {
    rig.search("hello", FOLDER);
    rig.finish_with(vec![
        rg_match("C:\\proj\\a.txt", "hello world", 1, "hello"),
        rg_match("C:\\proj\\a.txt", "say hello", 4, "hello"),
        rg_match("C:\\proj\\src\\b.rs", "// hello", 2, "hello"),
    ]);
}

#[gpui_kit::test]
fn the_start_screen_lists_recent_searches_and_the_keys_and_the_bar_is_labelled(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_seeded(cx, "start", false, FakeSource::new(), |cx| {
        persistence::update_app(cx, |app| {
            for n in 0..10 {
                app.add_to_history(&format!("query {n}"), "C:\\a\\b\\c\\d", 1000 + n);
            }
        })
    });
    assert_eq!(rig.role("Search across your files"), Some(Role::Heading));
    assert_eq!(rig.role("Text search"), Some(Role::Group));
    for label in ["Search query", "Folder to search", "Search bar"] {
        assert!(rig.has(label), "{label}");
    }
    // The newest first, eight of them, with "View all 10" for the rest and the folder's end shown.
    let recent = rig.labels_of(Role::ListItem);
    assert_eq!(recent.len(), 8, "{recent:?}");
    assert_eq!(recent[0], "query 9");
    assert_eq!(rig.role("View all 10"), Some(Role::Button));
    assert_eq!(rig.role("Search for query 9 in C:\\a\\b\\c\\d"), Some(Role::Button));
    // The keys, each a labelled group.
    assert_eq!(
        rig.role("Alt+C / W / R: Match case, whole words, regular expression"),
        Some(Role::Group)
    );
    assert_eq!(
        rig.role("Esc: Leave the search box; again, stop the search"),
        Some(Role::Group)
    );
    // Search waits for a query.
    assert_eq!(rig.element("Search").and_then(|e| e.disabled()), Some(true));
}

#[gpui_kit::test]
fn enter_in_the_query_searches_with_the_settings_and_the_matches_stream_in_by_file(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "stream", false, FakeSource::new());
    rig.set_folder(FOLDER);
    rig.type_query("hello");
    rig.press("enter");
    rig.settle_until(|rig| !rig.log.borrow().started.is_empty());
    {
        let log = rig.log.borrow();
        let (engine, request) = &log.started[0];
        assert_eq!(*engine, Engine::Ripgrep);
        assert_eq!((request.query.as_str(), request.path.as_str()), ("hello", FOLDER));
        // The settings' limit and exclusions go along, and nothing is skipped nowhere.
        assert_eq!(request.max_results, Some(10000));
        assert!(request.exclusions.iter().any(|e| e == "node_modules"));
        assert!(!request.everything);
    }
    // It is in the history at once, and the Stop button stands where Search was.
    assert_eq!(rig.history(), [("hello".to_string(), FOLDER.to_string())]);
    assert!(rig.has("Stop"));
    assert_eq!(rig.role("Searching…"), Some(Role::Group));

    // A first batch arrives: two files' matches, grouped, while it still runs.
    let id = rig.run_id();
    rig.event_of(
        &id,
        "data",
        vec![
            r#"{"type":"begin","data":{"path":{"text":"C:\\proj\\a.txt"}}}"#.to_string(),
            rg_match("C:\\proj\\a.txt", "hello world", 1, "hello"),
            rg_match("C:\\proj\\a.txt", "say hello", 4, "hello"),
            r#"{"type":"end","data":{}}"#.to_string(),
            rg_match("C:\\proj\\src\\b.rs", "// hello", 2, "hello"),
        ],
    );
    assert_eq!(rig.rows(), ["a.txt", "hello world", "say hello", "b.rs", "// hello"]);
    assert_eq!(rig.role("3 results in 2 files"), Some(Role::Status));
    assert!(rig.has("Stop"), "still searching");
    // The summary and the end: the time is core's, and Search is back.
    rig.event("data", vec![rg_summary(12, 3)]);
    rig.event("finished", vec![]);
    assert_eq!(rig.role("3 results in 2 files · 12 ms"), Some(Role::Status));
    assert!(rig.has("Search") && !rig.has("Stop"));
    // A file's header says how many it has and where; a match says its line.
    assert_eq!(
        rig.element("a.txt").and_then(|e| e.description().map(str::to_string)),
        Some("C:\\proj\\a.txt: 2 matches".to_string())
    );
    assert_eq!(
        rig.element("say hello")
            .and_then(|e| e.description().map(str::to_string)),
        Some("C:\\proj\\a.txt: 4".to_string())
    );
    // The first match is the one shown in the preview (auto-preview is on).
    assert_eq!(rig.element("hello world").and_then(|e| e.selected()), Some(true));
}

#[gpui_kit::test]
fn the_switches_choose_the_engine_and_exclude_each_other(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut source = FakeSource::new();
    source.covered = true;
    let mut rig = rig(cx, "engines", false, source);
    // Plain words in a folder the index covers are answered by the index.
    rig.search("hello", FOLDER);
    assert_eq!(rig.log.borrow().covers, [FOLDER.to_string()]);
    assert_eq!(rig.log.borrow().started[0].0, Engine::Index);
    // Any switch that changes how words match asks ripgrep instead (the index isn't asked at all).
    rig.click(("text-toggle", 0_usize));
    assert!(rig.options().case_sensitive);
    rig.search("Hello", FOLDER);
    assert_eq!(rig.log.borrow().covers.len(), 1);
    assert_eq!(rig.log.borrow().started[1].0, Engine::Ripgrep);
    assert!(rig.log.borrow().started[1].1.options.case_sensitive);
    // Documents and Meaning are the documents' search; a code pattern is ast-grep's.
    rig.click(("text-toggle", 4_usize));
    assert!(rig.options().documents);
    rig.search("report", FOLDER);
    assert_eq!(rig.log.borrow().started[2].0, Engine::Documents);
    rig.click(("text-toggle", 6_usize));
    let o = rig.options();
    assert!(o.meaning && o.documents, "meaning is a documents search");
    rig.click(("text-toggle", 3_usize));
    let o = rig.options();
    assert!(
        o.code && !o.documents && !o.meaning,
        "a code pattern turns the others off"
    );
    rig.search("console.log($MSG)", FOLDER);
    assert_eq!(rig.log.borrow().started[3].0, Engine::Code);
    // A code pattern has no case, words or regular expression of its own, nor pictures.
    for ix in [0_usize, 1, 2, 5] {
        assert_eq!(
            rig.visual.update(|window, cx| {
                window.render_frame(cx);
                window.find(("text-toggle", ix)).disabled()
            }),
            Some(true),
            "switch {ix}"
        );
    }
}

#[gpui_kit::test]
fn alt_letters_flip_the_switches_from_the_query_and_ctrl_h_opens_replace(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "alt-keys", false, FakeSource::new());
    rig.type_query("x");
    rig.press("alt-c");
    assert!(rig.options().case_sensitive);
    rig.press("alt-w");
    rig.press("alt-r");
    let o = rig.options();
    assert!(o.whole_word && o.regex);
    rig.press("alt-c");
    assert!(!rig.options().case_sensitive, "the same key flips it back");
    rig.press("alt-d");
    assert!(rig.options().documents);
    // Pictures come with Documents.
    rig.press("alt-o");
    assert!(rig.options().ocr);
    rig.press("alt-m");
    assert!(rig.options().meaning);
    // The switches are labelled with their keys, and the one that's on says so.
    assert_eq!(
        rig.element("Whole word (Alt+W)").and_then(|e| e.role()),
        Some(Role::Button)
    );
    // Ctrl+H shows the Replacement box, and again hides it.
    assert!(!rig.has("Replacement"));
    rig.press("ctrl-h");
    assert!(rig.has("Replacement"));
    rig.press("ctrl-h");
    assert!(!rig.has("Replacement"));
    // The query box and the placeholder follow the kind of search.
    assert_eq!(
        bar::query_placeholder(&rig.options()),
        "What you're looking for, in your own words…  (documents and notes ranked by meaning)"
    );
}

#[gpui_kit::test]
fn stop_asks_the_source_to_cancel_and_what_was_found_stays(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "stop", false, FakeSource::new());
    rig.search("hello", FOLDER);
    rig.event("data", vec![rg_match("C:\\proj\\a.txt", "hello", 1, "hello")]);
    assert!(rig.has("Stop"));
    rig.click("text-stop");
    assert_eq!(rig.log.borrow().cancelled, 1);
    // Core says it has ended; the match stays and Search is back.
    rig.event("finished", vec![]);
    assert_eq!(rig.rows(), ["a.txt", "hello"]);
    assert!(rig.has("Search"));
    // Escape outside the boxes stops a search too.
    rig.search("hello", FOLDER);
    rig.visual.update(|window, cx| {
        rig.view.update(cx, |view, cx| view.results_focus.focus(window, cx));
    });
    rig.press("escape");
    assert_eq!(rig.log.borrow().cancelled, 2);
}

#[gpui_kit::test]
fn a_newer_search_replaces_the_older_one_and_the_older_ones_events_are_dropped(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "newer", false, FakeSource::new());
    rig.search("first", FOLDER);
    let first = rig.run_id();
    rig.event("data", vec![rg_match("C:\\proj\\old.txt", "first one", 1, "first")]);
    assert_eq!(rig.rows(), ["old.txt", "first one"]);
    rig.search("second", FOLDER);
    let second = rig.run_id();
    assert_ne!(first, second);
    // The results start empty again.
    assert!(rig.rows().is_empty());
    // A late batch and the end of the first search change nothing.
    rig.event_of(
        &first,
        "data",
        vec![rg_match("C:\\proj\\old.txt", "first two", 2, "first")],
    );
    rig.event_of(&first, "finished", vec![]);
    assert_eq!(rig.matches(), 0);
    assert!(rig.has("Stop"), "the second is still running");
    rig.event_of(
        &second,
        "data",
        vec![rg_match("C:\\proj\\new.txt", "second", 1, "second")],
    );
    rig.event_of(&second, "finished", vec![]);
    assert_eq!(rig.rows(), ["new.txt", "second"]);
    // Both are in the history, the newer on top.
    assert_eq!(
        rig.history(),
        [
            ("second".to_string(), FOLDER.to_string()),
            ("first".to_string(), FOLDER.to_string())
        ]
    );
}

#[gpui_kit::test]
fn a_search_that_fails_says_why_and_try_again_asks_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "failed", false, FakeSource::new());
    rig.search("(", FOLDER);
    rig.event("error", vec!["regex parse error: unclosed group".to_string()]);
    rig.event("finished", vec![]);
    assert_eq!(rig.role("The search failed"), Some(Role::Group));
    assert_eq!(rig.role("regex parse error: unclosed group"), Some(Role::Alert));
    rig.click("text-try-again");
    rig.settle_until(|rig| rig.log.borrow().started.len() == 2);
    // A source that refuses to start says so the same way, and a search ended by Stop as it began isn't one.
    let mut refusing = FakeSource::new();
    refusing.start_error = Some("Can't find “C:\\gone”. Check the folder path and try again.".into());
    let mut rig = self::rig(cx, "refused", false, refusing);
    rig.search("x", "C:\\gone");
    rig.settle_until(|rig| rig.has("Can't find “C:\\gone”. Check the folder path and try again."));
    let mut cancelled = FakeSource::new();
    cancelled.start_error = Some("cancelled".into());
    let mut rig = self::rig(cx, "cancelled", false, cancelled);
    rig.search("x", FOLDER);
    rig.settle_until(|rig| rig.search.read_with(&rig.visual, |s, _| !s.searching()));
    assert!(!rig.has("The search failed"));
}

#[gpui_kit::test]
fn no_results_says_what_was_skipped_and_offers_to_skip_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "none", false, FakeSource::new());
    rig.search("zzz", FOLDER);
    rig.finish_with(vec![]);
    assert_eq!(rig.role("No results"), Some(Role::Group));
    let why = rig
        .labels_of(Role::Status)
        .into_iter()
        .find(|l| l.starts_with("Skipped: "))
        .expect("what was skipped");
    assert!(
        why.contains("node_modules") && why.ends_with("hidden files, what .gitignore files list."),
        "{why}"
    );
    rig.click("text-search-everything");
    rig.settle_until(|rig| rig.log.borrow().started.len() == 2);
    assert!(rig.log.borrow().started[1].1.everything);
    assert!(rig.log.borrow().started[1].1.query == "zzz");
    rig.event("data", vec![rg_summary(3, 0)]);
    rig.event("finished", vec![]);
    // Nothing searched at all (a folder that isn't there) says to check it; nothing was skipped this time.
    let why = rig
        .labels_of(Role::Status)
        .into_iter()
        .find(|l| l.starts_with("Nothing was searched"))
        .expect("the folder warning");
    assert_eq!(
        why,
        "Nothing was searched: check that “C:\\proj” exists and isn't skipped. Nothing was skipped."
    );
    assert!(!rig.has("Search again, skipping nothing"), "it already skipped nothing");
}

#[gpui_kit::test]
fn the_folder_as_core_searched_it_is_what_paths_are_shown_relative_to(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut source = FakeSource::new();
    source.folder_as_searched = Some("C:\\Users\\me\\proj".into());
    let mut rig = rig(cx, "folder", false, source);
    rig.search("hello", "%USERPROFILE%\\proj");
    rig.settle_until(|rig| {
        rig.search.read_with(&rig.visual, |s, _| {
            s.run().is_some_and(|r| r.path == "C:\\Users\\me\\proj")
        })
    });
    rig.finish_with(vec![rg_match("C:\\Users\\me\\proj\\sub\\a.txt", "hello", 1, "hello")]);
    assert_eq!(rig.rows(), ["a.txt", "hello"]);
    assert_eq!(
        super::results::relative_to("C:\\Users\\me\\proj\\sub\\a.txt", "C:\\Users\\me\\proj"),
        "sub\\a.txt"
    );
    assert_eq!(super::results::relative_to("D:\\x\\a.txt", "C:\\Users"), "D:\\x\\a.txt");
}

#[gpui_kit::test]
fn the_filter_extension_chips_and_collapsing_narrow_the_rows(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "filters", false, FakeSource::new());
    rig.wide();
    three_matches(&mut rig);
    // Collapsing a file hides its matches; its header stays and says how many it has.
    rig.click(("text-file", 0_usize));
    assert_eq!(rig.rows(), ["a.txt", "b.rs", "// hello"]);
    assert_eq!(rig.element("a.txt").and_then(|e| e.expanded()), Some(false));
    // Collapse all, and expand all.
    rig.click("text-collapse-all");
    assert_eq!(rig.rows(), ["a.txt", "b.rs"]);
    rig.click("text-collapse-all");
    assert_eq!(rig.rows(), ["a.txt", "hello world", "say hello", "b.rs", "// hello"]);
    // The extension chips filter by file type.
    assert_eq!(rig.role(".txt 2"), Some(Role::Button));
    rig.click(("text-ext-chip", 0_usize));
    assert_eq!(rig.rows(), ["a.txt", "hello world", "say hello"]);
    rig.click(("text-ext-chip", 0_usize));
    assert_eq!(rig.rows().len(), 5);
    // The filter box looks in the line and in the path, case ignored.
    let filter = rig.view.read_with(&rig.visual, |v, _| v.filter.clone());
    rig.visual
        .update(|window, cx| filter.update(cx, |input, cx| input.set_value("SAY", window, cx)));
    rig.settle();
    assert_eq!(rig.rows(), ["a.txt", "say hello"]);
    rig.visual
        .update(|window, cx| filter.update(cx, |input, cx| input.set_value("src", window, cx)));
    rig.settle();
    assert_eq!(rig.rows(), ["b.rs", "// hello"]);
    rig.visual
        .update(|window, cx| filter.update(cx, |input, cx| input.set_value("nothing like it", window, cx)));
    rig.settle();
    assert!(rig.rows().is_empty());
    assert_eq!(rig.role("No results match the filter."), Some(Role::Group));
    rig.click("text-clear-filters");
    assert_eq!(rig.rows().len(), 5);
}

#[gpui_kit::test]
fn up_down_and_enter_walk_the_matches_and_open_the_file_in_the_editor_or_with_its_app(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "keys", false, FakeSource::new());
    three_matches(&mut rig);
    // Down from the query's only line goes on to the results; the first match is already shown.
    rig.type_query("hello");
    rig.press("down");
    let in_results = rig.visual.update(|window, cx| {
        rig.view
            .update(cx, |view, cx| view.results_focus.contains_focused(window, cx))
    });
    assert!(in_results, "the results have the keyboard");
    assert_eq!(rig.element("hello world").and_then(|e| e.selected()), Some(true));
    // Down and Up skip the file headers.
    rig.press("down");
    assert_eq!(rig.element("say hello").and_then(|e| e.selected()), Some(true));
    rig.press("down");
    assert_eq!(rig.element("// hello").and_then(|e| e.selected()), Some(true));
    rig.press("down");
    assert_eq!(
        rig.element("// hello").and_then(|e| e.selected()),
        Some(true),
        "the last stays"
    );
    rig.press("up");
    assert_eq!(rig.element("say hello").and_then(|e| e.selected()), Some(true));
    // Enter opens the picked match's file, as its app opens it.
    rig.press("enter");
    rig.settle_until(|rig| !rig.log.borrow().opened.is_empty());
    assert_eq!(rig.log.borrow().opened, [("C:\\proj\\a.txt".to_string(), None)]);
    // With an editor set in Settings the file opens in it; a document or picture never does.
    rig.visual
        .update(|_, cx| state::update(cx, |s| s.editor_path = "code".into()));
    rig.press("enter");
    rig.settle_until(|rig| rig.log.borrow().opened.len() == 2);
    assert_eq!(
        rig.log.borrow().opened[1],
        ("C:\\proj\\a.txt".to_string(), Some("code".to_string()))
    );
    assert!(is_document("C:\\x\\Report.PDF") && is_document("C:\\x\\a.jpeg") && !is_document("C:\\x\\a.rs"));
    // A double click opens too.
    rig.visual.update(|window, cx| {
        window.render_frame(cx);
        window.double_click(("text-match", 4_usize), cx);
    });
    rig.settle_until(|rig| rig.log.borrow().opened.len() == 3);
    assert_eq!(rig.log.borrow().opened[2].0, "C:\\proj\\src\\b.rs");
}

#[gpui_kit::test]
fn escape_leaves_the_query_and_the_context_menu_and_preview_reveal_files(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let source = FakeSource::new().with_file("C:\\proj\\a.txt", "one\ntwo\nhello world\nfour\nfive\nsay hello\n");
    let mut rig = rig(cx, "reveal", false, source);
    rig.search("hello", FOLDER);
    rig.finish_with(vec![
        rg_match("C:\\proj\\a.txt", "hello world", 3, "hello"),
        rg_match("C:\\proj\\a.txt", "say hello", 6, "hello"),
    ]);
    // The preview beside the results shows the file around the match, the match's line marked.
    rig.settle_until(|rig| rig.labels_of(Role::ListItem).iter().any(|l| l == "four"));
    let lines = rig.labels_of(Role::ListItem);
    assert_eq!(lines, ["one", "two", "hello world", "four", "five", "say hello"]);
    assert_eq!(
        rig.visual.update(|window, cx| {
            window.render_frame(cx);
            window
                .find(("text-preview-line", 2_usize))
                .description()
                .map(str::to_string)
        }),
        Some("line 3".to_string())
    );
    assert_eq!(rig.role("a.txt · line 3"), Some(Role::Heading));
    // Picking the other match moves the mark inside the same window without reading again.
    rig.click(("text-match", 2_usize));
    assert_eq!(rig.role("a.txt · line 6"), Some(Role::Heading));
    // "Show in Files" asks the window to reveal the file.
    rig.click("text-preview-reveal");
    assert_eq!(
        rig.events(),
        [TextSearchEvent::Reveal {
            path: "C:\\proj\\a.txt".into()
        }]
    );
    // Escape in the query leaves it for the results.
    rig.type_query("hello");
    rig.press("escape");
    let in_results = rig.visual.update(|window, cx| {
        rig.view
            .update(cx, |view, cx| view.results_focus.contains_focused(window, cx))
    });
    assert!(in_results);
}

#[gpui_kit::test]
fn replace_all_asks_first_then_changes_each_file_and_searches_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let source = FakeSource::new()
        .with_file("C:\\proj\\a.txt", "hello world\nsay hello\n")
        .with_file("C:\\proj\\src\\b.rs", "// hello\r\nfn main() {}\r\n");
    let mut rig = rig(cx, "replace", false, source);
    three_matches(&mut rig);
    rig.press("ctrl-h");
    let replacement = rig.view.read_with(&rig.visual, |v, _| v.replacement.clone());
    rig.visual
        .update(|window, cx| replacement.update(cx, |input, cx| input.set_value("bye", window, cx)));
    rig.settle();
    // The question names what changes, and Cancel changes nothing.
    rig.click("text-replace-all");
    assert!(rig.visual.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(
        rig.visual.update(|window, cx| {
            window.render_frame(cx);
            window.find("confirm").description().map(str::to_string)
        }),
        Some("Replace every match of “hello” with “bye” in 2 files?\n\nThis can't be undone.".to_string())
    );
    rig.click("confirm-cancel");
    assert!(rig.log.borrow().written.is_empty());
    // Yes: each file is read, replaced the way ripgrep found the matches (line ends kept) and written.
    rig.click("text-replace-all");
    rig.click("confirm-ok");
    rig.settle_until(|rig| rig.log.borrow().written.len() == 2);
    assert_eq!(
        rig.log.borrow().written,
        [
            ("C:\\proj\\a.txt".to_string(), "bye world\nsay bye\n".to_string()),
            (
                "C:\\proj\\src\\b.rs".to_string(),
                "// bye\r\nfn main() {}\r\n".to_string()
            ),
        ]
    );
    // What's left to replace is searched for again.
    rig.settle_until(|rig| rig.log.borrow().started.len() == 2);
    assert_eq!(rig.log.borrow().started[1].1.query, "hello");
    assert_eq!(
        rig.files.borrow().get("C:\\proj\\a.txt").map(String::as_str),
        Some("bye world\nsay bye\n")
    );
}

#[gpui_kit::test]
fn replace_all_goes_without_asking_when_settings_say_so_and_never_for_documents_or_a_bad_pattern(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let source = FakeSource::new().with_file("C:\\proj\\a.txt", "a.b a.b\n");
    let mut rig = rig(cx, "replace-quiet", false, source);
    rig.visual
        .update(|_, cx| state::update(cx, |s| s.confirm_before_replace = false));
    rig.search("a.b", FOLDER);
    rig.finish_with(vec![rg_match("C:\\proj\\a.txt", "a.b a.b", 1, "a.b")]);
    rig.press("ctrl-h");
    let replacement = rig.view.read_with(&rig.visual, |v, _| v.replacement.clone());
    rig.visual
        .update(|window, cx| replacement.update(cx, |input, cx| input.set_value("$&!", window, cx)));
    rig.settle();
    // Enter in the Replacement box replaces; no question; a plain search inserts the text as it is.
    rig.visual.update(|window, cx| {
        replacement.update(cx, |input, cx| input.focus(window, cx));
    });
    rig.press("enter");
    rig.settle_until(|rig| rig.log.borrow().written.len() == 1);
    assert_eq!(rig.log.borrow().written[0].1, "$&! $&!\n");
    rig.settle_until(|rig| rig.log.borrow().started.len() == 2);
    rig.finish_with(vec![]);

    // A regular expression reads $-groups; one that can't be used says so and changes nothing.
    rig.click(("text-toggle", 2_usize));
    rig.search("(a)\\.b", FOLDER);
    rig.finish_with(vec![rg_match("C:\\proj\\a.txt", "$&! $&!", 1, "$")]);
    rig.visual
        .update(|window, cx| replacement.update(cx, |input, cx| input.set_value("$1-x", window, cx)));
    let asked = rig.log.borrow().started.len();
    rig.visual
        .update(|window, cx| rig.view.update(cx, |view, cx| view.replace_all(window, cx)));
    rig.settle_until(|rig| rig.log.borrow().started.len() > asked);
    assert_eq!(
        rig.log.borrow().written.len(),
        1,
        "nothing matched (a.b is gone), nothing written"
    );
    rig.search("(", FOLDER);
    rig.finish_with(vec![rg_match("C:\\proj\\a.txt", "(", 1, "(")]);
    rig.visual
        .update(|window, cx| rig.view.update(cx, |view, cx| view.replace_all(window, cx)));
    rig.settle();
    assert_eq!(rig.log.borrow().written.len(), 1);
    assert!(!rig.visual.update(|window, cx| window.has_active_dialog(cx)));

    // Documents are zips and PDFs: no Replace for them.
    // (The error notice over the bar's corner would take a click, so the switch is flipped directly.)
    rig.visual.update(|window, cx| {
        rig.view.update(cx, |view, cx| {
            view.toggle_option(SearchOptionKey::Documents, window, cx)
        })
    });
    rig.search("report", FOLDER);
    rig.finish_with(vec![rg_match("C:\\proj\\a.pdf", "report", 1, "report")]);
    assert_eq!(
        rig.element("Replace all").and_then(|e| e.disabled()),
        Some(true),
        "Replace all is off for documents"
    );
    rig.visual
        .update(|window, cx| rig.view.update(cx, |view, cx| view.replace_all(window, cx)));
    rig.settle();
    assert_eq!(rig.log.borrow().written.len(), 1);
}

#[gpui_kit::test]
fn a_code_pattern_first_shows_what_each_match_becomes_then_rewrites_in_the_files(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut source = FakeSource::new();
    source.rewritten = 2;
    let mut rig = rig(cx, "rewrite", false, source);
    rig.press("alt-a");
    assert!(rig.options().code);
    rig.press("ctrl-h");
    let replacement = rig.view.read_with(&rig.visual, |v, _| v.replacement.clone());
    rig.visual
        .update(|window, cx| replacement.update(cx, |input, cx| input.set_value("logger.info($MSG)", window, cx)));
    rig.search("console.log($MSG)", FOLDER);
    // The replace box's text goes along as the rewrite, to show on each match.
    assert_eq!(
        rig.log.borrow().started[0].1.rewrite.as_deref(),
        Some("logger.info($MSG)")
    );
    let with_rewrite = serde_json::json!({
        "type": "match",
        "data": {
            "path": { "text": "C:\\proj\\a.js" },
            "lines": { "text": "console.log(x)\n" },
            "line_number": 1,
            "submatches": [{ "match": { "text": "console.log(x)" }, "start": 0, "end": 14 }],
            "replacement": "logger.info(x)"
        }
    })
    .to_string();
    rig.finish_with(vec![with_rewrite]);
    // Replace all with the same text on screen asks first, then ast-grep rewrites in the files.
    rig.click("text-replace-all");
    assert!(rig.visual.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(
        rig.visual.update(|window, cx| {
            window.render_frame(cx);
            window.find("confirm").description().map(str::to_string)
        }),
        Some(
            "Rewrite 1 match of “console.log($MSG)” as “logger.info($MSG)” in 1 file?\n\nThis can't be undone."
                .to_string()
        )
    );
    rig.click("confirm-ok");
    rig.settle_until(|rig| !rig.log.borrow().rewrites.is_empty());
    assert_eq!(
        rig.log.borrow().rewrites[0].rewrite.as_deref(),
        Some("logger.info($MSG)")
    );
    rig.settle_until(|rig| rig.log.borrow().started.len() == 2);

    // A rewrite that differs from the one on screen is shown first: it searches again with it.
    rig.finish_with(vec![]);
    rig.visual
        .update(|window, cx| replacement.update(cx, |input, cx| input.set_value("other($MSG)", window, cx)));
    rig.search("console.log($MSG)", FOLDER);
    rig.finish_with(vec![rg_match("C:\\proj\\a.js", "console.log(x)", 1, "console")]);
    rig.visual
        .update(|window, cx| replacement.update(cx, |input, cx| input.set_value("third($MSG)", window, cx)));
    let asked = rig.log.borrow().started.len();
    rig.visual
        .update(|window, cx| rig.view.update(cx, |view, cx| view.replace_all(window, cx)));
    rig.settle_until(|rig| rig.log.borrow().started.len() == asked + 1);
    assert_eq!(
        rig.log.borrow().started[asked].1.rewrite.as_deref(),
        Some("third($MSG)")
    );
    assert!(!rig.visual.update(|window, cx| window.has_active_dialog(cx)));
}

#[gpui_kit::test]
fn a_result_limit_says_so_and_show_all_searches_again_without_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "limit", false, FakeSource::new());
    rig.visual
        .update(|_, cx| state::update(cx, |s| s.max_results = Some(2)));
    rig.search("hello", FOLDER);
    assert_eq!(rig.log.borrow().started[0].1.max_results, Some(2));
    rig.finish_with(vec![
        rg_match("C:\\proj\\a.txt", "hello", 1, "hello"),
        rg_match("C:\\proj\\a.txt", "hello again", 2, "hello"),
    ]);
    assert_eq!(rig.role("Stopped at 2 · Show all"), Some(Role::Button));
    rig.click("text-results-limited");
    rig.settle_until(|rig| rig.log.borrow().started.len() == 2);
    assert_eq!(rig.log.borrow().started[1].1.max_results, None, "all of them");
}

#[gpui_kit::test]
fn export_writes_what_the_filters_show_as_csv_or_json(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut source = FakeSource::new();
    source.pick_export = Some("C:\\out\\results.csv".into());
    let mut rig = rig(cx, "export", false, source);
    rig.wide();
    three_matches(&mut rig);
    rig.click(("text-ext-chip", 0_usize));
    rig.click("text-export");
    rig.settle_until(|rig| !rig.log.borrow().written.is_empty());
    assert_eq!(
        rig.log.borrow().written[0],
        (
            "C:\\out\\results.csv".to_string(),
            "File,Line,Content\n\"C:\\proj\\a.txt\",\"1\",\"hello world\"\n\"C:\\proj\\a.txt\",\"4\",\"say hello\""
                .to_string()
        )
    );
}

#[gpui_kit::test]
fn recent_searches_run_again_forget_one_and_clear_with_undo(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "history", false, FakeSource::new());
    rig.visual.update(|_, cx| {
        persistence::update_app(cx, |app| {
            app.add_to_history("alpha", "C:\\a", 1);
            app.add_to_history("beta", "C:\\b", 2);
        })
    });
    rig.settle();
    // Clicking one fills the boxes and searches.
    rig.click(("text-home-history", 1001_usize));
    rig.settle_until(|rig| !rig.log.borrow().started.is_empty());
    {
        let log = rig.log.borrow();
        assert_eq!(
            (log.started[0].1.query.as_str(), log.started[0].1.path.as_str()),
            ("alpha", "C:\\a")
        );
    }
    assert_eq!(
        rig.history()[0],
        ("alpha".to_string(), "C:\\a".to_string()),
        "it moved to the top"
    );
    // The popover lists them all, filters, forgets one and clears the rest.
    rig.click("text-history-open");
    assert!(rig.has("Filter recent searches…"));
    rig.click(("text-history", 2000_usize));
    assert_eq!(rig.history().len(), 1);
    rig.click("text-history-clear");
    assert!(rig.history().is_empty());
    assert_eq!(rig.role("No searches yet"), Some(Role::Status));
    rig.press("escape");
    assert!(!rig.has("Filter recent searches…"));
}

#[gpui_kit::test]
fn browse_sets_the_folder_and_enter_takes_a_folder_from_the_window(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut source = FakeSource::new();
    source.pick_folder = Some("D:\\work".into());
    let mut rig = rig(cx, "browse", false, source);
    rig.set_folder("C:\\start");
    rig.click("text-browse");
    rig.settle_until(|rig| {
        rig.visual
            .update(|_, cx| persistence::app(cx).map(|a| a.path.clone()))
            .as_deref()
            == Some("D:\\work")
    });
    assert_eq!(rig.log.borrow().folder_picks, [Some("C:\\start".to_string())]);
    // A folder handed over by the window (the folder menu's "Search text") fills the box and gives the query the keyboard.
    let shown = rig.visual.update(|window, cx| {
        rig.view
            .update(cx, |view, cx| view.enter(Some("E:\\notes"), window, cx))
    });
    assert!(shown);
    let value = rig
        .view
        .read_with(&rig.visual, |v, cx| v.folder.read(cx).value().to_string());
    assert_eq!(value, "E:\\notes");
    // Not a folder (This PC): the box keeps what it had.
    let shown = rig
        .visual
        .update(|window, cx| rig.view.update(cx, |view, cx| view.enter(Some(THIS_PC), window, cx)));
    assert!(shown);
    let value = rig
        .view
        .read_with(&rig.visual, |v, cx| v.folder.read(cx).value().to_string());
    assert_eq!(value, "E:\\notes");
}

#[gpui_kit::test]
fn arabic_names_and_text_are_found_marked_and_the_mirrored_layout_holds(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let file = "C:\\مستندات\\تقرير.txt";
    let line = "قال مرحبا بالعالم كله";
    let source = FakeSource::new().with_file(file, &format!("{line}\nسطر آخر\n"));
    let mut rig = rig(cx, "arabic", true, source);
    rig.search("مرحبا", "C:\\مستندات");
    rig.finish_with(vec![rg_match(file, line, 1, "مرحبا")]);
    // The file's name and the line are exposed as they are, without the layout-only marks.
    assert_eq!(rig.rows(), ["تقرير.txt", line]);
    assert_eq!(rig.role("1 result in 1 file · 12 ms"), Some(Role::Status));
    assert_eq!(
        rig.visual.update(|window, cx| {
            window.render_frame(cx);
            window.find(("text-match", 1_usize)).description().map(str::to_string)
        }),
        Some(format!("{file}: 1"))
    );
    // The hit is marked on the Arabic word's bytes.
    let segments = rig.search.read_with(&rig.visual, |s, _| s.matches()[0].segments());
    let marked: Vec<&str> = segments.iter().filter(|s| s.hit).map(|s| s.text.as_str()).collect();
    assert_eq!(marked, ["مرحبا"]);
    // The preview shows the Arabic file's lines.
    rig.settle_until(|rig| rig.labels_of(Role::ListItem).iter().any(|l| l == "سطر آخر"));
    // Right to left: Search is on the left of the field, the preview on the left of the list.
    let (field, go, list, preview) = rig.visual.update(|window, cx| {
        window.render_frame(cx);
        (
            window.find("text-search-field").bounds(),
            window.find("text-search").bounds(),
            window.find("text-results-list").bounds(),
            window.find("text-preview").bounds(),
        )
    });
    assert!(
        go.origin.x < field.origin.x,
        "the button starts the row at the right edge's mirror"
    );
    assert!(
        preview.origin.x < list.origin.x,
        "the preview is at the end edge, the left"
    );
    testing::set_rtl(false);
}

#[test]
fn the_ripgrep_request_is_what_core_reads() {
    let request = Request {
        id: "x".into(),
        query: "مرحبا".into(),
        path: "C:\\مستندات".into(),
        options: SearchOptions {
            whole_word: true,
            ..Default::default()
        },
        rewrite: None,
        exclusions: vec!["node_modules".into()],
        everything: true,
        max_results: Some(5),
    };
    // Core deserializes this into its private `TextSearch`; a field it lacked or misnamed would fail here.
    assert!(serde_json::from_value::<glint_core::TextSearch>(source::ripgrep_json(&request)).is_ok());
}

#[gpui_kit::test]
fn core_search_events_reach_the_results_through_the_event_bridge(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "bridge", false, FakeSource::new());
    rig.search("hello", FOLDER);
    let id = rig.run_id();
    // As core's search thread emits them: from another thread, through the app handle.
    let app = rig.visual.update(|_, cx| cx.global::<core_bridge::Core>().app.clone());
    let lines = vec![rg_match("C:\\proj\\a.txt", "hello", 1, "hello"), rg_summary(7, 1)];
    std::thread::spawn(move || {
        use glint_core::host::Emitter as _;
        let _ = app.emit(
            "search-event",
            serde_json::json!({ "id": id, "type": "data", "lines": lines }),
        );
        let _ = app.emit(
            "search-event",
            serde_json::json!({ "id": id, "type": "finished", "lines": [] }),
        );
    })
    .join()
    .ok();
    rig.settle_until(|rig| rig.role("1 result in 1 file · 7 ms").is_some());
    assert_eq!(rig.rows(), ["a.txt", "hello"]);
}

/// The real thing, when ripgrep is on this PC: core runs it, its events come through the bridge, the pump
/// parses them, and the rows are drawn, with an Arabic line and an Arabic folder name.
#[gpui_kit::test]
fn the_real_core_searches_a_folder_and_streams_it_into_the_view(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    if glint_core::proc::locate("rg").is_none() {
        eprintln!("ripgrep isn't on this PC: the real-core search test is skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("glint-text-search-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("مجلد")).expect("a temp folder");
    std::fs::write(dir.join("a.txt"), "hello مرحبا\nsecond line\n").expect("a file");
    std::fs::write(dir.join("مجلد").join("ملف.txt"), "nothing\nhello again\n").expect("a file");
    cx.update(|cx| {
        testing::init(cx);
        persistence::init_at(cx, state_file("real"));
    });
    let (mut visual, view) = testing::open_window(cx, move |window, cx| {
        let app = cx.global::<core_bridge::Core>().app.clone();
        cx.new(|cx| TextSearch::with_source(Rc::new(CoreSource::new(app)), None, window, cx))
    });
    // A headless window has no handle to register, so core is given a stand-in for the main window.
    visual.update(|_, cx| {
        let app = cx.global::<core_bridge::Core>().app.clone();
        app.register_window(glint_core::host::Window::new(
            app.clone(),
            crate::workspace::MAIN,
            0,
            || true,
            || true,
            || true,
            || 1.0,
        ));
    });
    let folder = dir.to_string_lossy().to_string();
    let search = view.read_with(&visual, |v, _| v.search.clone());
    let query = view.read_with(&visual, |v, _| v.query.clone());
    visual.update(|window, cx| {
        query.update(cx, |q, cx| q.set_value("hello", window, cx));
        view.update(cx, |v, cx| {
            v.set_folder(&folder, window, cx);
            v.search_now(window, cx)
        });
    });
    let mut done = false;
    for _ in 0..600 {
        visual.run_until_parked();
        visual.update(|window, cx| window.render_frame(cx));
        let (searching, found, error) = search.read_with(&visual, |s, _| {
            (s.searching(), s.matches().len(), s.error().to_string())
        });
        assert!(error.is_empty(), "the search failed: {error}");
        if !searching && found > 0 {
            done = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(done, "the search never finished");
    let found: Vec<(String, Option<u64>)> = search.read_with(&visual, |s, _| {
        let mut all: Vec<_> = s
            .matches()
            .iter()
            .map(|m| (m.path.rsplit('\\').next().unwrap_or("").to_string(), m.line_number))
            .collect();
        all.sort();
        all
    });
    assert_eq!(
        found,
        [("a.txt".to_string(), Some(1)), ("ملف.txt".to_string(), Some(2))]
    );
    // The hit in the line with Arabic in it is on "hello", in bytes.
    let segments = search.read_with(&visual, |s, _| {
        s.matches()
            .iter()
            .find(|m| m.path.ends_with("a.txt"))
            .map(|m| m.segments())
            .unwrap_or_default()
    });
    assert_eq!(
        segments
            .iter()
            .filter(|s| s.hit)
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>(),
        ["hello"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
fn ctrl_f_and_ctrl_k_bring_the_keyboard_back_to_the_query_from_anywhere_in_the_view(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "ctrl-f", false, FakeSource::new());
    three_matches(&mut rig);
    let query_focused = |rig: &mut Rig| {
        let query = rig.view.read_with(&rig.visual, |v, _| v.query.clone());
        rig.visual
            .update(|window, cx| query.read(cx).focus_handle(cx).is_focused(window))
    };
    rig.visual
        .update(|window, cx| rig.view.update(cx, |view, cx| view.results_focus.focus(window, cx)));
    rig.settle();
    assert!(!query_focused(&mut rig));
    rig.press("ctrl-f");
    assert!(query_focused(&mut rig));
    // From the query itself, Escape leaves it and Ctrl+K comes back.
    rig.press("escape");
    assert!(!query_focused(&mut rig));
    rig.press("ctrl-k");
    assert!(query_focused(&mut rig));
    // The query's text is selected, ready to be typed over.
    let selected = rig.view.read_with(&rig.visual, |v, cx| {
        let query = v.query.read(cx);
        (query.selected_range().len(), query.value().len())
    });
    assert_eq!(selected.0, selected.1);
}

#[gpui_kit::test]
fn entering_text_search_takes_the_open_folder_from_the_explorer(cx: &mut TestAppContext) {
    use crate::explorer::fake::{file, folder, FakeDisk};
    use crate::explorer::{init_with, KnownFolder, KnownId};
    let _guard = testing::lock();
    cx.update(testing::init);
    let home = "C:\\Users\\me";
    let disk = FakeDisk::new();
    disk.set_known(vec![KnownFolder {
        id: KnownId::Home,
        name: "Home",
        path: home.into(),
    }]);
    disk.folder(home, vec![folder(home, "Docs"), file(home, "a.txt", 1, 1)]);
    disk.folder("C:\\Users\\me\\Docs", vec![file("C:\\Users\\me\\Docs", "b.txt", 1, 1)]);
    cx.update(|cx| {
        init_with(cx, disk.source());
        persistence::init_at(cx, state_file("enter"));
    });
    let explorer = cx.update(|cx| crate::explorer::explorer(cx));
    let (mut visual, view) = testing::open_window(cx, |window, cx| {
        cx.new(|cx| TextSearch::with_source(Rc::new(FakeSource::new()), None, window, cx))
    });
    let task = explorer.update(&mut visual, |e, cx| e.init_explorer(Vec::new(), cx));
    visual.foreground_executor().block_test(task);
    for _ in 0..100 {
        visual.run_until_parked();
        if explorer.read_with(&visual, |e, _| e.current_path() == home) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // The box was empty when the view was made, and the open folder wasn't known yet: Entering fills it.
    let shown = visual.update(|window, cx| view.update(cx, |view, cx| view.enter(None, window, cx)));
    assert!(shown);
    let folder_box = view.read_with(&visual, |v, cx| v.folder.read(cx).value().to_string());
    assert_eq!(folder_box, home);
    // Another folder open is the one searched.
    explorer.update(&mut visual, |e, cx| {
        e.navigate("C:\\Users\\me\\Docs", Default::default(), cx)
    });
    for _ in 0..100 {
        visual.run_until_parked();
        if explorer.read_with(&visual, |e, _| e.current_path() == "C:\\Users\\me\\Docs") {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    visual.update(|window, cx| view.update(cx, |view, cx| view.enter(None, window, cx)));
    let folder_box = view.read_with(&visual, |v, cx| v.folder.read(cx).value().to_string());
    assert_eq!(folder_box, "C:\\Users\\me\\Docs");
    // Settings can name a folder for it, which is the fallback when there is no explorer folder to use.
    visual.update(|_, cx| state::update(cx, |s| s.default_search_path = " D:\\Search ".into()));
    assert_eq!(visual.update(|_, cx| default_folder(cx)), "D:\\Search");
}

#[test]
fn every_icon_text_search_draws_is_in_the_embedded_assets() {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    // A missing icon draws nothing and says nothing, so each one this view uses is checked here.
    for name in [
        IconName::Search,
        IconName::CaseSensitive,
        IconName::WholeWord,
        IconName::Regex,
        IconName::Braces,
        IconName::FileText,
        IconName::ScanText,
        IconName::Sparkles,
        IconName::Replace,
        IconName::FolderOpen,
        IconName::Square,
        IconName::Download,
        IconName::ClockArrowLeft,
        IconName::X,
        IconName::ChevronRight,
        IconName::ChevronLeft,
        IconName::ChevronDown,
        IconName::ChevronsUpDown,
        IconName::ChevronsDownUp,
        IconName::Copy,
        IconName::ExternalLink,
        IconName::FileSearch,
        IconName::FingerprintPattern,
        IconName::FolderInput,
        IconName::FolderSearch,
        IconName::Lock,
        IconName::RotateCw,
        IconName::SearchX,
        IconName::TriangleAlert,
        IconName::TextSearch,
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}
