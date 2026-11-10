//! The grid's "Search subfolders too" button under a filter that matches nothing.

use super::*;
use crate::explorer::fake::FindCall;

fn filtered(env: &mut Env, filter: &str) {
    let explorer = env.explorer.clone();
    let filter = filter.to_string();
    env.visual
        .update(|_, cx| explorer.update(cx, |e, cx| e.set_filter(&filter, None, cx)));
    env.settle(|env| env.visual.update(|window, _| window.try_find("file-grid-empty").is_some()));
}

fn last(disk: &FakeDisk) -> Option<FindCall> {
    disk.finds().last().cloned()
}

#[gpui_kit::test]
fn a_filter_that_hides_every_tile_offers_to_search_subfolders(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = disk(0);
    let mut env = env(cx, &disk, plain());
    filtered(&mut env, "inner");
    env.visual.update(|window, _| {
        assert_eq!(
            window.find_by_label("Nothing in this folder matches “inner”.").role(),
            Some(Role::Status)
        );
        assert_eq!(window.find_by_label("Search subfolders too").role(), Some(Role::Button));
    });
    assert!(disk.finds().is_empty());
    env.visual.update(|window, cx| window.click("file-grid-search-subfolders", cx));
    env.settle(|env| !env.tile_names().is_empty());
    let call = last(&disk).expect("the search ran");
    assert_eq!((call.query.as_str(), call.roots.as_slice()), ("inner", ["C:\\x".to_string()].as_slice()));
    assert_eq!(env.tile_names(), ["inner.txt"]);
}

#[gpui_kit::test]
fn the_recycle_bin_offers_no_subfolder_search(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = disk(0);
    let mut env = env(cx, &disk, plain());
    env.go(glint_model::paths::RECYCLE_BIN);
    filtered(&mut env, "zz");
    env.visual.update(|window, _| {
        assert!(window.try_find("file-grid-search-subfolders").is_none());
        assert_eq!(
            window.find_by_label("Nothing in the Recycle Bin matches “zz”.").role(),
            Some(Role::Status)
        );
    });
}
