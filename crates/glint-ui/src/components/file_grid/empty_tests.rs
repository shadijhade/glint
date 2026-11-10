//! The grid's search, Gallery and Recent empty states: Try again, the buttons that search wider, Add a folder, and
//! Recent when Windows keeps nothing. They mirror the list's (`file_list/tests/find_states.rs`).

use super::super::*;
use super::set_services;
use crate::components::file_grid::thumbnail::tests::Fake;
use crate::components::file_grid::thumbnail::{self, ThumbSource};
use crate::explorer::fake::{file, folder, FakeDisk, FindCall};
use crate::explorer::{NavOptions, ViewMode, Wider};
use crate::testing;
use glint_model::paths::{GALLERY, RECENT, RECYCLE_BIN, THIS_PC};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Ui {
    visual: VisualTestContext,
    explorer: Entity<Explorer>,
}

fn disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder("C:\\x", vec![folder("C:\\x", "docs"), file("C:\\x", "a.jpg", 2_000, 1)]);
    disk.folder("C:\\x\\docs", vec![file("C:\\x\\docs", "inner.txt", 1, 1)]);
    disk
}

/// A grid on `path`, in the active tab.
fn open(cx: &mut TestAppContext, disk: &FakeDisk, path: &str) -> Ui {
    cx.update(testing::init);
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    let source: Rc<dyn ThumbSource> = Rc::new(Rc::new(Fake::default()));
    cx.update(|cx| thumbnail::init_with(cx, source));
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let task = explorer.update(cx, |e, cx| e.navigate_task(path, NavOptions::default(), cx));
    cx.foreground_executor().block_test(task);
    explorer.update(cx, |e, cx| e.set_view(ViewMode::Grid, None, cx));
    let (visual, _grid) = testing::open_window(cx, |window, cx| cx.new(|cx| FileGrid::new(0, window, cx)));
    let mut ui = Ui { visual, explorer };
    ui.settle();
    ui
}

impl Ui {
    /// Lets the explorer's work, the background threads' and a frame finish.
    fn settle(&mut self) {
        for _ in 0..40 {
            self.visual.run_until_parked();
            self.visual.update(|window, cx| window.render_frame(cx));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn search(&mut self, query: &str) {
        let explorer = self.explorer.clone();
        let query = query.to_string();
        self.visual
            .update(|_, cx| explorer.update(cx, |e, cx| e.run_find(&query, Wider::default(), cx).detach()));
        self.settle();
    }

    fn click_id(&mut self, id: &'static str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        self.settle();
    }

    fn has(&mut self, id: &'static str) -> bool {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
    }

    fn role_of(&mut self, label: &str) -> Option<Role> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn says(&mut self, label: &str) -> bool {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn scope(&mut self) -> Option<String> {
        self.explorer.read_with(&self.visual, |e, _| {
            e.active_tab().and_then(|t| t.find.as_ref().map(|f| f.scope.clone()))
        })
    }
}

fn last(disk: &FakeDisk) -> FindCall {
    disk.finds().last().cloned().expect("a search was started")
}

fn nothing_found(cx: &mut TestAppContext) -> (Ui, FakeDisk) {
    let disk = disk();
    let mut ui = open(cx, &disk, "C:\\x");
    ui.search("zzz");
    (ui, disk)
}

#[gpui_kit::test]
fn a_search_under_way_with_nothing_yet_says_searching(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = disk();
    let mut ui = open(cx, &disk, "C:\\x");
    // The search stays open: it has found nothing yet.
    let _live = disk.live_find("slow");
    ui.search("slow");
    assert!(ui.says("Searching…"));
    assert!(!ui.says("Nothing named like “slow”"));
}

#[gpui_kit::test]
fn a_failed_search_offers_try_again_which_searches_the_same_place_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = disk();
    let mut ui = open(cx, &disk, "C:\\x");
    disk.refuse_next_find("bad filter");
    ui.search("zzz");
    assert_eq!(ui.role_of("The search failed"), Some(Role::Alert));
    assert_eq!(ui.role_of("Try again"), Some(Role::Button));
    ui.click_id("file-grid-search-try-again");
    assert_eq!(disk.finds().len(), 2);
    let again = last(&disk);
    assert_eq!(again.query, "zzz");
    assert_eq!(again.roots, ["C:\\x"]);
    // It ran fine this time: nothing is named like it.
    assert!(!ui.has("file-grid-search-try-again"));
    assert_eq!(ui.role_of("Nothing named like “zzz”"), Some(Role::Status));
}

#[gpui_kit::test]
fn nothing_found_offers_hidden_files_and_the_other_drives(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut ui, disk) = nothing_found(cx);
    assert_eq!(ui.role_of("Nothing named like “zzz”"), Some(Role::Status));
    assert_eq!(ui.role_of("Include hidden files"), Some(Role::Button));
    assert_eq!(ui.role_of("Search every drive"), Some(Role::Button));
    assert!(!last(&disk).options.hidden);
    ui.click_id("file-grid-search-hidden");
    let call = last(&disk);
    assert_eq!((disk.finds().len(), call.query.as_str()), (2, "zzz"));
    assert!(call.options.hidden, "Settings' Show hidden items is on now");
    assert_eq!(call.roots, ["C:\\x"]);
    // Hidden files are searched: that button is gone.
    assert!(!ui.has("file-grid-search-hidden"));
}

#[gpui_kit::test]
fn search_skipped_folders_too_searches_without_exclusions_or_gitignore(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = disk();
    let mut ui = open(cx, &disk, "C:\\x");
    ui.visual.update(|_, cx| {
        state::update(cx, |s| {
            s.find_respect_ignore = true;
            s.find_exclusions = vec!["node_modules".into()];
        })
    });
    ui.search("zzz");
    assert!(last(&disk).options.respect_ignore);
    assert_eq!(ui.role_of("Search skipped folders too"), Some(Role::Button));
    ui.click_id("file-grid-search-skipped");
    let call = last(&disk);
    assert_eq!(disk.finds().len(), 2);
    assert!(!call.options.respect_ignore);
    assert!(call.options.exclude.is_empty());
    assert_eq!(call.roots, ["C:\\x"], "the same place");
}

#[gpui_kit::test]
fn search_every_drive_searches_from_this_pc_and_then_offers_no_more_drives(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut ui, disk) = nothing_found(cx);
    assert_eq!(ui.scope().as_deref(), Some("C:\\x"));
    ui.click_id("file-grid-search-every-drive");
    assert_eq!(ui.scope().as_deref(), Some(THIS_PC));
    assert_eq!(disk.finds().len(), 2);
    assert!(!last(&disk).roots.contains(&"C:\\x".to_string()), "not just the folder");
    assert!(!ui.has("file-grid-search-every-drive"));
}

#[gpui_kit::test]
fn a_search_with_results_shows_its_tiles_and_no_message(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = disk();
    let mut ui = open(cx, &disk, "C:\\x");
    ui.search("inner");
    assert!(!ui.says("Nothing named like “inner”"));
    assert!(!ui.says("The search failed"));
}

#[gpui_kit::test]
fn the_recycle_bin_with_a_filter_still_has_no_subfolder_search(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = disk();
    let mut ui = open(cx, &disk, RECYCLE_BIN);
    let explorer = ui.explorer.clone();
    ui.visual
        .update(|_, cx| explorer.update(cx, |e, cx| e.set_filter("zz", None, cx)));
    ui.settle();
    assert_eq!(
        ui.role_of("Nothing in the Recycle Bin matches “zz”."),
        Some(Role::Status)
    );
    assert!(!ui.has("file-grid-search-subfolders"));
}

fn picking(
    path: Option<&'static str>,
) -> impl Fn(String, isize) -> crate::platform::pickers::Fut<Result<Option<String>, String>> {
    move |_, _| Box::pin(async move { Ok(path.map(str::to_string)) })
}

fn empty_gallery(cx: &mut TestAppContext, pick: Option<&'static str>) -> Ui {
    let disk = FakeDisk::new();
    disk.folder(GALLERY, vec![]);
    cx.update(testing::init);
    cx.update(|cx| set_services(cx, picking(pick), || true));
    open(cx, &disk, GALLERY)
}

fn gallery_folders(ui: &mut Ui) -> Vec<String> {
    ui.visual.update(|_, cx| state::settings(cx).gallery_folders.clone())
}

#[gpui_kit::test]
fn the_empty_gallery_adds_the_folder_picked_to_the_gallery_folders(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = empty_gallery(cx, Some("D:\\صور العائلة"));
    assert_eq!(ui.role_of("No photos or videos here yet"), Some(Role::Status));
    assert_eq!(ui.role_of("Add a folder"), Some(Role::Button));
    ui.click_id("file-grid-gallery-add-folder");
    assert_eq!(gallery_folders(&mut ui), ["D:\\صور العائلة"]);
    // Picking it again adds nothing.
    ui.click_id("file-grid-gallery-add-folder");
    assert_eq!(gallery_folders(&mut ui), ["D:\\صور العائلة"]);
}

#[gpui_kit::test]
fn cancelling_the_picker_adds_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = empty_gallery(cx, None);
    ui.click_id("file-grid-gallery-add-folder");
    assert!(gallery_folders(&mut ui).is_empty());
}

#[gpui_kit::test]
fn recent_says_so_when_windows_keeps_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    disk.folder(RECENT, vec![]);
    cx.update(testing::init);
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = asked.clone();
    cx.update(|cx| {
        set_services(cx, picking(None), move || {
            counter.fetch_add(1, Ordering::SeqCst);
            false
        })
    });
    let mut ui = open(cx, &disk, RECENT);
    assert_eq!(
        ui.role_of("Windows doesn't keep recently opened items"),
        Some(Role::Status)
    );
    assert!(!ui.says("Nothing opened lately"));
    assert_eq!(asked.load(Ordering::SeqCst), 1, "asked once, not every frame");
}

#[gpui_kit::test]
fn recent_is_just_empty_when_windows_keeps_items(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    disk.folder(RECENT, vec![]);
    cx.update(testing::init);
    cx.update(|cx| set_services(cx, picking(None), || true));
    let mut ui = open(cx, &disk, RECENT);
    assert_eq!(ui.role_of("Nothing opened lately"), Some(Role::Status));
    assert!(!ui.says("Windows doesn't keep recently opened items"));
}
