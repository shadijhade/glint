//! Headless tests of the bars over a pane: the search bar (running, finished, close matches, stopped at the limit,
//! Save), the bar for a folder listed from the index, nothing for a plain folder, and the same in the Arabic
//! interface. They run on the explorer's fake disk. The fake can't list a folder from the index, so the closed bar's
//! decision is tested on tabs built by hand, and its Get access button on a denied folder with the bar forced.

use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{Found, Listed, LoadState, NavOptions, NoAccess};
use crate::state;
use crate::testing;
use glint_model::store::ViewMode;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use std::sync::Arc;

struct Host {
    bars: Vec<Entity<PaneBars>>,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        col().w_full().children(self.bars.clone())
    }
}

struct Rig {
    visual: VisualTestContext,
    explorer: Entity<Explorer>,
    disk: FakeDisk,
    host: Entity<Host>,
}

fn state_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("glint-pane-bars-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp folder");
    let file = dir.join(format!("{name}.json"));
    let _ = std::fs::remove_file(&file);
    file
}

/// `C:\a` and `C:\b` with reports in both, an Arabic name in `b`; the explorer is in `C:\a`.
fn rig(cx: &mut TestAppContext, name: &str, rtl: bool) -> Rig {
    cx.update(testing::init);
    let disk = FakeDisk::new();
    disk.folder("C:\\", vec![folder("C:\\", "a"), folder("C:\\", "b")]);
    disk.folder(
        "C:\\a",
        vec![file("C:\\a", "report.pdf", 10, 1), file("C:\\a", "notes.md", 20, 2)],
    );
    disk.folder(
        "C:\\b",
        vec![
            file("C:\\b", "report2.pdf", 30, 3),
            file("C:\\b", "تقرير السنة.pdf", 40, 4),
        ],
    );
    disk.deny("C:\\locked");
    cx.update(|cx| {
        crate::explorer::init_with(cx, disk.source());
        persistence::init_at(cx, state_file(name));
    });
    // After the saved state loads: it sets the language, too.
    testing::set_rtl(rtl);
    let explorer = cx.update(|cx| crate::explorer::explorer(cx));
    let task = explorer.update(cx, |e, cx| e.navigate_task("C:\\a", NavOptions::default(), cx));
    cx.foreground_executor().block_test(task);
    let (mut visual, host) = testing::open_window(cx, |window, cx| {
        let bars = (0..2)
            .map(|pane| cx.new(|cx| PaneBars::new(pane, window, cx)))
            .collect();
        cx.new(|_| Host { bars })
    });
    settle(&mut visual);
    Rig {
        visual,
        explorer,
        disk,
        host,
    }
}

impl Drop for Rig {
    /// The language is process-wide: whatever a test did, the next one starts in English.
    fn drop(&mut self) {
        testing::set_rtl(false);
    }
}

fn settle(visual: &mut VisualTestContext) {
    for _ in 0..20 {
        visual.run_until_parked();
    }
}

impl Rig {
    fn act<R>(&mut self, f: impl FnOnce(&mut Explorer, &mut Context<Explorer>) -> R) -> R {
        let result = self.explorer.update(&mut self.visual, f);
        settle(&mut self.visual);
        result
    }

    /// Searches the active tab and waits for the search to end.
    fn find(&mut self, query: &str) {
        let query = query.to_string();
        let task = self
            .explorer
            .update(&mut self.visual, |e, cx| e.run_find(&query, Wider::default(), cx));
        task.detach();
        settle(&mut self.visual);
    }

    /// Starts a search that says what the test sends it, without waiting for it.
    fn start(&mut self, query: &str) -> (async_channel::Sender<Found>, gpui_kit::Task<()>) {
        let live = self.disk.live_find(query);
        let query = query.to_string();
        let task = self
            .explorer
            .update(&mut self.visual, |e, cx| e.run_find(&query, Wider::default(), cx));
        settle(&mut self.visual);
        (live, task)
    }

    fn view(&self, pane: usize) -> Entity<PaneBars> {
        self.host.read_with(&self.visual, |host, _| host.bars[pane].clone())
    }

    fn bar(&mut self, pane: usize) -> Bar {
        let view = self.view(pane);
        view.read_with(&self.visual, |bars, cx| bars.bar(cx))
    }

    /// The accessible labels inside the bar over `pane`, the frame's first, then top to bottom, start to end.
    fn labels(&mut self, pane: usize) -> Vec<String> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            let mut labels = Vec::new();
            for id in ["pane-search-bar", "pane-closed-bar"] {
                if let Some(frame) = window.try_find((id, pane)) {
                    labels.extend(frame.label().map(str::to_string));
                    labels.extend(
                        window
                            .within((id, pane))
                            .elements()
                            .into_iter()
                            .filter_map(|e| e.label().map(str::to_string)),
                    );
                }
            }
            labels
        })
    }

    fn has(&mut self, pane: usize, label: &str) -> bool {
        self.labels(pane).iter().any(|l| l == label)
    }

    /// Whether the element with this id is there.
    fn there(&mut self, id: (&'static str, usize)) -> bool {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
    }

    fn label_of(&mut self, id: (&'static str, usize)) -> Option<String> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).and_then(|e| e.label().map(str::to_string))
        })
    }

    fn click(&mut self, id: (&'static str, usize)) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        settle(&mut self.visual);
    }

    fn x_of(&mut self, id: (&'static str, usize)) -> f32 {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            f32::from(window.find(id).bounds().center().x)
        })
    }

    fn run(&mut self) -> Option<crate::explorer::FindRun> {
        self.explorer
            .read_with(&self.visual, |e, _| e.active_tab().and_then(|t| t.find.clone()))
    }

    fn saved(&mut self) -> Vec<SavedSearch> {
        self.visual.update(|_, cx| {
            persistence::app(cx)
                .map(|a| a.saved_searches.clone())
                .unwrap_or_default()
        })
    }
}

fn info() -> SearchInfo {
    SearchInfo {
        tab: TabId(1),
        query: "report".into(),
        scope: "C:\\a".into(),
        running: false,
        fuzzy: false,
        limited: false,
        saved: false,
        max_results: 50_000,
    }
}

// ---- Which bar, and what is in it ----------------------------------------------------------------------------

#[gpui_kit::test]
fn a_plain_folder_draws_no_bar(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "plain", false);
    assert_eq!(rig.bar(0), Bar::Hidden);
    assert!(!rig.there(("pane-search-bar", 0)) && !rig.there(("pane-closed-bar", 0)));
    // The second pane isn't shown: whatever the first one shows, it has nothing.
    assert_eq!(rig.bar(1), Bar::Hidden);
}

#[gpui_kit::test]
fn the_search_bar_while_it_runs_has_a_spinner_and_stop_ends_the_search(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "running", false);
    let (live, _task) = rig.start("rep");
    live.try_send(Found::Started { engine: "fd".into() }).expect("open");
    settle(&mut rig.visual);
    assert!(matches!(rig.bar(0), Bar::Search(SearchInfo { running: true, .. })));
    // A spinner (not the magnifier), the words, Save, Stop and Close.
    assert!(rig.there(("pane-search-spinner", 0)));
    assert_eq!(rig.label_of(("pane-search-spinner", 0)).as_deref(), Some("Searching"));
    assert_eq!(
        rig.label_of(("pane-search-text", 0)).as_deref(),
        Some("Results for “rep” in a and its subfolders")
    );
    for label in ["Save", "Stop", "Close"] {
        assert!(rig.has(0, label), "{label}: {:?}", rig.labels(0));
    }
    // Stop asks the source to end this tab's search.
    assert!(rig.disk.cancelled().is_empty());
    rig.click(("pane-search-stop", 0));
    assert_eq!(rig.disk.cancelled(), ["tab1"]);
    // The stream's last words end the run: the spinner and Stop go.
    live.try_send(Found::Finished {
        count: 0,
        elapsed: 1.0,
        limited: false,
        fuzzy: false,
    })
    .expect("open");
    settle(&mut rig.visual);
    assert!(!rig.there(("pane-search-spinner", 0)) && !rig.there(("pane-search-stop", 0)));
    assert!(rig.there(("pane-search-close", 0)));
}

#[gpui_kit::test]
fn a_finished_search_says_results_for_the_query_in_the_folder_and_close_ends_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "finished", false);
    rig.find("report");
    assert!(rig.run().is_some());
    assert_eq!(
        rig.label_of(("pane-search-text", 0)).as_deref(),
        Some("Results for “report” in a and its subfolders")
    );
    assert!(!rig.there(("pane-search-spinner", 0)) && !rig.there(("pane-search-stop", 0)));
    assert!(!rig.there(("pane-search-limit", 0)), "it stopped at nothing");
    rig.click(("pane-search-close", 0));
    assert!(rig.run().is_none(), "back to the folder");
    assert_eq!(rig.bar(0), Bar::Hidden);
    assert!(!rig.there(("pane-search-bar", 0)));
}

#[gpui_kit::test]
fn a_search_of_this_pc_says_all_drives_without_its_subfolders(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "this-pc", false);
    rig.act(|e, cx| e.navigate("", NavOptions::default(), cx));
    let task = rig
        .explorer
        .update(&mut rig.visual, |e, cx| e.run_find("report", Wider::in_scope(""), cx));
    task.detach();
    settle(&mut rig.visual);
    assert_eq!(
        rig.label_of(("pane-search-text", 0)).as_deref(),
        Some("Results for “report” in all drives")
    );
}

#[gpui_kit::test]
fn close_matches_are_said_when_nothing_matched_as_typed(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "fuzzy", false);
    let (live, task) = rig.start("repotr");
    live.try_send(Found::Started { engine: "index".into() }).expect("open");
    live.try_send(Found::Entries(vec![file("C:\\a", "report.pdf", 1, 1)]))
        .expect("open");
    live.try_send(Found::Finished {
        count: 1,
        elapsed: 2.0,
        limited: false,
        fuzzy: true,
    })
    .expect("open");
    settle(&mut rig.visual);
    task.detach();
    assert!(matches!(rig.bar(0), Bar::Search(SearchInfo { fuzzy: true, .. })));
    assert_eq!(
        rig.label_of(("pane-search-text", 0)).as_deref(),
        Some("Close matches for “repotr” in a and its subfolders")
    );
}

#[gpui_kit::test]
fn a_search_that_hit_the_limit_says_stopped_at_and_show_all_runs_it_without_the_limit(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "capped", false);
    rig.visual
        .update(|_, cx| state::update(cx, |s| s.find_max_results = Some(1)));
    rig.act(|e, cx| e.navigate("C:\\", NavOptions::default(), cx));
    rig.find("report");
    assert!(rig.run().is_some_and(|r| r.limited && r.entries.len() == 1));
    assert_eq!(rig.label_of(("pane-search-limit", 0)).as_deref(), Some("Stopped at 1"));
    assert!(rig.has(0, "Show all"));
    // Show all searches again in the same folder, with no limit.
    rig.click(("pane-search-show-all", 0));
    let call = rig.disk.finds().pop().expect("a search was started");
    assert_eq!(call.options.max_results, None);
    assert_eq!(
        (call.query.as_str(), call.roots.as_slice()),
        ("report", ["C:\\".to_string()].as_slice())
    );
    let run = rig.run().expect("the search is shown");
    assert!(!run.limited, "nothing cut it short this time");
    assert!(!rig.there(("pane-search-limit", 0)));
}

#[gpui_kit::test]
fn save_keeps_the_search_in_the_sidebar_and_saved_takes_it_out(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "save", false);
    rig.find("report");
    assert!(rig.has(0, "Save") && !rig.has(0, "Saved"));
    rig.click(("pane-search-save", 0));
    assert_eq!(
        rig.saved(),
        [SavedSearch {
            query: "report".into(),
            folder: "C:\\a".into()
        }]
    );
    assert!(rig.has(0, "Saved") && !rig.has(0, "Save"), "{:?}", rig.labels(0));
    // The folder compares ignoring case, as the sidebar's own list does.
    assert!(matches!(rig.bar(0), Bar::Search(SearchInfo { saved: true, .. })));
    rig.click(("pane-search-save", 0));
    assert!(rig.saved().is_empty());
    assert!(rig.has(0, "Save"));
}

#[test]
fn a_search_saved_in_another_case_still_counts_as_saved() {
    let mut tab = Tab::blank(TabId(1), "C:\\a", ViewMode::List, None);
    tab.find = Some(crate::explorer::FindRun {
        query: "report".into(),
        scope: "C:\\A".into(),
        ..Default::default()
    });
    let saved = [SavedSearch {
        query: "report".into(),
        folder: "c:\\a".into(),
    }];
    assert!(matches!(
        bar_of_tab(&tab, &saved, None),
        Bar::Search(SearchInfo { saved: true, .. })
    ));
    let other = [SavedSearch {
        query: "Report".into(),
        folder: "c:\\a".into(),
    }];
    assert!(
        matches!(
            bar_of_tab(&tab, &other, None),
            Bar::Search(SearchInfo { saved: false, .. })
        ),
        "the query compares as typed"
    );
}

// ---- The closed bar -----------------------------------------------------------------------------------------

/// A tab whose folder the index listed, with what Windows said about it.
fn from_index(no_access: Option<NoAccess>) -> Tab {
    let mut tab = Tab::blank(TabId(7), "C:\\locked", ViewMode::List, None);
    tab.listing = Some(Listed {
        path: "C:\\locked".into(),
        entries: Arc::from(Vec::new()),
        elapsed: 0.0,
        archive: None,
        from_index: true,
        remote: false,
    });
    if let Some(no_access) = no_access {
        tab.load = LoadState::NoAccess(no_access);
    }
    tab
}

#[test]
fn a_folder_listed_from_the_index_gets_the_closed_bar_with_get_access_where_it_can_help() {
    let closed = |tab: &Tab| bar_of_tab(tab, &[], None);
    assert_eq!(
        closed(&from_index(Some(NoAccess::default()))),
        Bar::Closed(ClosedInfo {
            tab: TabId(7),
            refused: false,
            can_get_access: true
        })
    );
    // Windows refused even administrators: it says "to itself" and offers nothing.
    assert_eq!(
        closed(&from_index(Some(NoAccess {
            refused: true,
            ..NoAccess::default()
        }))),
        Bar::Closed(ClosedInfo {
            tab: TabId(7),
            refused: true,
            can_get_access: false
        })
    );
    // On another computer, which decides who may open it: no Get access either.
    assert_eq!(
        closed(&from_index(Some(NoAccess {
            remote: true,
            ..NoAccess::default()
        }))),
        Bar::Closed(ClosedInfo {
            tab: TabId(7),
            refused: false,
            can_get_access: false
        })
    );
    // Without what Windows said, the bar still says why, and offers nothing (`canGet = !!info && ..`).
    assert_eq!(
        closed(&from_index(None)),
        Bar::Closed(ClosedInfo {
            tab: TabId(7),
            refused: false,
            can_get_access: false
        })
    );
}

#[test]
fn a_search_in_a_folder_from_the_index_shows_the_search_bar_not_the_closed_bar() {
    let mut tab = from_index(Some(NoAccess::default()));
    tab.find = Some(crate::explorer::FindRun {
        query: "x".into(),
        scope: "C:\\locked".into(),
        ..Default::default()
    });
    assert!(matches!(bar_of_tab(&tab, &[], None), Bar::Search(_)));
}

#[test]
fn a_folder_on_disk_or_denied_outright_has_no_closed_bar() {
    // Listed normally.
    let mut tab = from_index(None);
    if let Some(listing) = &mut tab.listing {
        listing.from_index = false;
    }
    assert_eq!(bar_of_tab(&tab, &[], None), Bar::Hidden);
    // Not listed at all: the folder's own view says why, not a bar over a list.
    let mut denied = Tab::blank(TabId(8), "C:\\locked", ViewMode::List, None);
    denied.load = LoadState::NoAccess(NoAccess::default());
    assert_eq!(bar_of_tab(&denied, &[], None), Bar::Hidden);
}

/// The closed bar over the denied `C:\locked`, as if the index had listed it.
fn closed_rig(cx: &mut TestAppContext, name: &str, rtl: bool, refused: bool, can_get_access: bool) -> Rig {
    let mut rig = rig(cx, name, rtl);
    rig.act(|e, cx| e.navigate("C:\\locked", NavOptions::default(), cx));
    let tab = rig.explorer.read_with(&rig.visual, |e, _| e.active_tab_id());
    let forced = Bar::Closed(ClosedInfo {
        tab,
        refused,
        can_get_access,
    });
    rig.view(0).update(&mut rig.visual, |bars, cx| {
        bars.forced = Some(forced);
        cx.notify();
    });
    settle(&mut rig.visual);
    rig
}

#[gpui_kit::test]
fn the_closed_bar_says_why_and_get_access_opens_the_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = closed_rig(cx, "closed", false, false, true);
    assert!(rig
        .explorer
        .read_with(&rig.visual, |e, _| e.active_tab().is_some_and(|t| t.is_closed())));
    assert_eq!(
        rig.label_of(("pane-closed-text", 0)).as_deref(),
        Some("Windows keeps this folder closed to you: shown read-only from instant search.")
    );
    assert!(rig.has(0, "Get access"), "{:?}", rig.labels(0));
    // Windows lets the folder open now; the button asks, and the folder opens in the tab.
    rig.disk
        .folder("C:\\locked", vec![file("C:\\locked", "inside.txt", 1, 0)]);
    rig.click(("pane-closed-get-access", 0));
    assert!(rig
        .explorer
        .read_with(&rig.visual, |e, _| e.active_tab().is_some_and(|t| !t.is_closed())));
    let working = rig.view(0).read_with(&rig.visual, |bars, _| bars.working());
    assert!(!working, "the button is ready again once Windows has answered");
}

#[gpui_kit::test]
fn the_closed_bar_says_to_itself_and_offers_nothing_when_windows_refused(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = closed_rig(cx, "refused", false, true, false);
    assert_eq!(
        rig.label_of(("pane-closed-text", 0)).as_deref(),
        Some("Windows keeps this folder to itself: shown read-only from instant search.")
    );
    assert!(!rig.there(("pane-closed-get-access", 0)));
}

// ---- Right to left ------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_search_bar_ends_with_close_in_english(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "ltr", false);
    rig.find("report");
    let (words, close) = (rig.x_of(("pane-search-text", 0)), rig.x_of(("pane-search-close", 0)));
    assert!(close > words, "Close ends the bar, on the right");
}

#[gpui_kit::test]
fn the_search_bar_mirrors_in_arabic_and_names_an_arabic_query(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, "rtl", true);
    rig.act(|e, cx| e.navigate(r"C:\b", NavOptions::default(), cx));
    rig.find("تقرير");
    let (words, close) = (rig.x_of(("pane-search-text", 0)), rig.x_of(("pane-search-close", 0)));
    assert!(
        close < words,
        "in Arabic the bar runs from the right: Close ends it on the left"
    );
    let label = rig.label_of(("pane-search-text", 0)).unwrap_or_default();
    assert!(label.contains("“تقرير”"), "{label}");
}

#[gpui_kit::test]
fn the_closed_bar_keeps_get_access_after_the_words_in_english(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = closed_rig(cx, "closed-ltr", false, false, true);
    let (words, get) = (
        rig.x_of(("pane-closed-text", 0)),
        rig.x_of(("pane-closed-get-access", 0)),
    );
    assert!(get > words);
}

#[gpui_kit::test]
fn the_closed_bar_mirrors_in_arabic(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = closed_rig(cx, "closed-rtl", true, false, true);
    let (words, get) = (
        rig.x_of(("pane-closed-text", 0)),
        rig.x_of(("pane-closed-get-access", 0)),
    );
    assert!(get < words, "Get access follows the words, on their left in Arabic");
}

// ---- The words ----------------------------------------------------------------------------------------------

#[test]
fn the_sentence_marks_the_query_in_bold_and_knows_every_wording() {
    let (text, bold) = search_words(&info(), false);
    assert_eq!(text, "Results for “report” in a and its subfolders");
    assert_eq!(&text[bold], "“report”");

    let this_pc = SearchInfo {
        scope: String::new(),
        ..info()
    };
    assert_eq!(search_words(&this_pc, false).0, "Results for “report” in all drives");

    let fuzzy = SearchInfo { fuzzy: true, ..info() };
    assert_eq!(
        search_words(&fuzzy, false).0,
        "Close matches for “report” in a and its subfolders"
    );

    // A query in Arabic: the bold range still covers exactly the quoted query, on character boundaries.
    let arabic = SearchInfo {
        query: "تقرير".into(),
        ..info()
    };
    let (text, bold) = search_words(&arabic, true);
    assert!(text.is_char_boundary(bold.start) && text.is_char_boundary(bold.end));
    assert!(text[bold].contains("تقرير"));
}
