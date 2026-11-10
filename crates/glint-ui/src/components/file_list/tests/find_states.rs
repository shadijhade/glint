//! The list's search buttons and empty states: Try again, the three buttons that search wider, Search
//! subfolders too, the Gallery's Add a folder, and Recent when Windows keeps nothing.

use super::*;
use crate::components::file_list::states::set_services;
use crate::explorer::fake::FindCall;
use crate::explorer::Wider;
use crate::state;
use glint_model::paths::{GALLERY, RECENT, RECYCLE_BIN, THIS_PC};
use std::sync::Arc;
use std::time::Duration;

impl Ui {
    /// Lets the explorer's work, the background threads' and a frame finish.
    fn settle_all(&mut self) {
        for _ in 0..40 {
            self.frame();
            std::thread::sleep(Duration::from_millis(5));
        }
        self.frame();
    }

    fn search(&mut self, query: &str) {
        let explorer = self.explorer.clone();
        let query = query.to_string();
        self.visual
            .update(|_, cx| explorer.update(cx, |e, cx| e.run_find(&query, Wider::default(), cx).detach()));
        self.settle_all();
    }

    fn click_id(&mut self, id: &'static str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        self.settle_all();
    }

    fn has(&mut self, id: &'static str) -> bool {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
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

fn with_settings(ui: &mut Ui, change: impl FnOnce(&mut glint_model::store::Settings)) {
    ui.visual.update(|_, cx| state::update(cx, change));
}

fn nothing_found(cx: &mut TestAppContext) -> (Ui, FakeDisk) {
    let disk = small_disk();
    let mut ui = open(cx, &disk, "C:\\x");
    ui.search("zzz");
    (ui, disk)
}

#[gpui_kit::test]
fn a_failed_search_offers_try_again_which_searches_the_same_place_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = small_disk();
    let mut ui = open(cx, &disk, "C:\\x");
    disk.refuse_next_find("bad filter");
    ui.search("zzz");
    assert_eq!(ui.role_of("The search failed"), Some(Role::Alert));
    assert_eq!(ui.role_of("Try again"), Some(Role::Button));
    ui.click_id("file-list-search-try-again");
    assert_eq!(disk.finds().len(), 2);
    let again = last(&disk);
    assert_eq!(again.query, "zzz");
    assert_eq!(again.roots, ["C:\\x"]);
    // It ran fine this time: nothing is named like it.
    assert!(!ui.has("file-list-search-try-again"));
    assert_eq!(ui.role_of("Nothing named like “zzz”"), Some(Role::Status));
}

#[gpui_kit::test]
fn nothing_found_offers_hidden_files_and_the_other_drives(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut ui, disk) = nothing_found(cx);
    assert_eq!(ui.role_of("Nothing named like “zzz”"), Some(Role::Status));
    assert!(ui.has("file-list-search-hidden"));
    assert!(ui.has("file-list-search-every-drive"));
    // Settings skip some folders by default, so the skipped folders have their button too.
    assert!(ui.has("file-list-search-skipped"));
    assert_eq!(ui.role_of("Include hidden files"), Some(Role::Button));
    assert_eq!(ui.role_of("Search every drive"), Some(Role::Button));
    assert!(!last(&disk).options.hidden);
    ui.click_id("file-list-search-hidden");
    let call = last(&disk);
    assert_eq!((disk.finds().len(), call.query.as_str()), (2, "zzz"));
    assert!(call.options.hidden, "Settings' Show hidden items is on now");
    assert_eq!(call.roots, ["C:\\x"]);
    assert!(ui.visual.update(|_, cx| state::settings(cx).show_hidden));
    // Hidden files are searched: that button is gone.
    assert!(!ui.has("file-list-search-hidden"));
}

#[gpui_kit::test]
fn search_skipped_folders_too_searches_without_exclusions_or_gitignore(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = small_disk();
    let mut ui = open(cx, &disk, "C:\\x");
    with_settings(&mut ui, |s| {
        s.find_respect_ignore = true;
        s.find_exclusions = vec!["node_modules".into()];
    });
    ui.search("zzz");
    let first = last(&disk);
    assert!(first.options.respect_ignore);
    assert_eq!(first.options.exclude, ["node_modules"]);
    assert_eq!(ui.role_of("Search skipped folders too"), Some(Role::Button));
    ui.click_id("file-list-search-skipped");
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
    ui.click_id("file-list-search-every-drive");
    assert_eq!(ui.scope().as_deref(), Some(THIS_PC));
    let call = last(&disk);
    assert_eq!(disk.finds().len(), 2);
    assert_eq!(call.query, "zzz");
    assert!(!call.roots.contains(&"C:\\x".to_string()), "not just the folder");
    assert!(!ui.has("file-list-search-every-drive"));
}

#[gpui_kit::test]
fn a_filter_that_hides_everything_offers_to_search_subfolders(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = small_disk();
    let mut ui = open(cx, &disk, "C:\\x");
    let explorer = ui.explorer.clone();
    ui.visual
        .update(|_, cx| explorer.update(cx, |e, cx| e.set_filter("inner", None, cx)));
    ui.frame();
    assert_eq!(
        ui.role_of("Nothing in this folder matches “inner”."),
        Some(Role::Status)
    );
    assert_eq!(ui.role_of("Search subfolders too"), Some(Role::Button));
    assert!(disk.finds().is_empty());
    ui.click_id("file-list-search-subfolders");
    let call = last(&disk);
    assert_eq!(
        (call.query.as_str(), call.roots.as_slice()),
        ("inner", ["C:\\x".to_string()].as_slice())
    );
    assert_eq!(ui.labels(), ["inner.txt"]);
}

#[gpui_kit::test]
fn the_recycle_bin_has_nothing_below_it_to_search(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = small_disk();
    let mut ui = open(cx, &disk, RECYCLE_BIN);
    let explorer = ui.explorer.clone();
    ui.visual
        .update(|_, cx| explorer.update(cx, |e, cx| e.set_filter("zz", None, cx)));
    ui.frame();
    assert_eq!(
        ui.role_of("Nothing in the Recycle Bin matches “zz”."),
        Some(Role::Status)
    );
    assert!(!ui.has("file-list-search-subfolders"));
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
    ui.click_id("file-list-gallery-add-folder");
    assert_eq!(gallery_folders(&mut ui), ["D:\\صور العائلة"]);
    // Picking it again adds nothing.
    ui.click_id("file-list-gallery-add-folder");
    assert_eq!(gallery_folders(&mut ui), ["D:\\صور العائلة"]);
}

#[gpui_kit::test]
fn cancelling_the_picker_adds_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = empty_gallery(cx, None);
    ui.click_id("file-list-gallery-add-folder");
    assert!(gallery_folders(&mut ui).is_empty());
}

#[gpui_kit::test]
fn recent_says_so_when_windows_keeps_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    disk.folder(RECENT, vec![]);
    cx.update(testing::init);
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = asked.clone();
    cx.update(|cx| {
        set_services(cx, picking(None), move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            false
        })
    });
    let mut ui = open(cx, &disk, RECENT);
    ui.settle_all();
    assert_eq!(
        ui.role_of("Windows doesn't keep recently opened items"),
        Some(Role::Status)
    );
    assert_eq!(ui.role_of("Nothing opened lately"), None);
    assert_eq!(
        asked.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "asked once, not every frame"
    );
}

#[gpui_kit::test]
fn recent_is_just_empty_when_windows_keeps_items(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    disk.folder(RECENT, vec![]);
    cx.update(testing::init);
    cx.update(|cx| set_services(cx, picking(None), || true));
    let mut ui = open(cx, &disk, RECENT);
    ui.settle_all();
    assert_eq!(ui.role_of("Nothing opened lately"), Some(Role::Status));
    assert_eq!(ui.role_of("Windows doesn't keep recently opened items"), None);
}
