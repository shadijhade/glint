//! The grid's compare marks and Git marks: the pills over the thumbnails, in both reading directions, and the
//! empty state of "diff only".

use super::*;
use crate::components::file_list::fake_git::{repo_view, FakeGit};
use glint_model::entries::GitMark;

const ARABIC: &str = "تقرير الربع.pdf";

/// `C:\x` (see [`disk`]) in a repository whose marks are `marks`.
fn repo_env(cx: &mut TestAppContext, marks: &[(&str, GitMark)]) -> Env {
    let disk = disk(0);
    let git = FakeGit::default();
    git.put("C:\\x", repo_view(marks));
    let mut env = env_with(cx, &disk, plain(), Some(&git));
    // Git's answer arrives in the background.
    env.settle(|env| {
        env.visual
            .update(|window, _| !window.find_all_by_role(Role::Group).is_empty())
    });
    env
}

impl Env {
    fn has_label(&mut self, label: &str) -> bool {
        self.visual
            .update(|window, _| window.try_find_by_label(label).is_some())
    }

    /// Where the pill with this id starts, from the window's left edge.
    fn left_edge(&mut self, id: (&'static str, usize)) -> f32 {
        self.visual
            .update(|window, _| f32::from(window.find(id).bounds().origin.x))
    }
}

#[gpui_kit::test]
fn tiles_show_a_pill_for_each_git_mark_and_an_ignored_tile_none(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = repo_env(
        cx,
        &[
            ("docs", GitMark::Modified),
            ("a.jpg", GitMark::Untracked),
            ("b.txt", GitMark::Ignored),
            (ARABIC, GitMark::Staged),
        ],
    );
    env.visual.update(|window, cx| window.render_frame(cx));
    assert!(env.has_label("Changed: docs"));
    assert!(env.has_label("New: a.jpg"));
    assert!(
        env.has_label(&format!("Staged: {ARABIC}")),
        "an Arabic name is told apart too"
    );
    assert!(
        !env.has_label("Ignored: b.txt"),
        "an ignored item shows faded, not as a pill"
    );
    env.visual.update(|window, _| {
        let pill = window.find_by_label("New: a.jpg");
        assert_eq!(pill.description(), Some("Git: new, not added yet"));
        assert_eq!(pill.role(), Some(Role::Group));
    });
    // The tiles are the same tiles.
    assert_eq!(env.tile_names().len(), 4);
}

#[gpui_kit::test]
fn a_folder_outside_a_repository_has_no_pills(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = repo_env(cx, &[("a.jpg", GitMark::Untracked)]);
    env.visual.update(|window, cx| window.render_frame(cx));
    assert!(env.has_label("New: a.jpg"));
    env.go("C:\\empty");
    env.settle(|env| !env.has_label("New: a.jpg"));
    assert!(!env.has_label("New: a.jpg"));
}

/// The folder `C:\x` of [`disk`] next to `C:\y`: `a.jpg` is the same, `b.txt` is newer there (so older here), the
/// Arabic file is only here and `docs` is a folder in both.
fn side_by_side(cx: &mut TestAppContext, git: Option<&FakeGit>) -> Env {
    let disk = disk(0);
    disk.folder(
        "C:\\y",
        vec![
            folder("C:\\y", "docs"),
            file("C:\\y", "a.jpg", 2_000, 1),
            file("C:\\y", "b.txt", 10, 200_000),
            file("C:\\y", "other.txt", 3, 3),
        ],
    );
    disk.folder(
        "C:\\twin",
        vec![
            folder("C:\\twin", "docs"),
            file("C:\\twin", "a.jpg", 2_000, 1),
            file("C:\\twin", "b.txt", 10, 2),
            file("C:\\twin", ARABIC, 5_000, 3),
        ],
    );
    let mut env = env_with(cx, &disk, plain(), git);
    env.explorer.update(&mut env.visual, |e, cx| e.toggle_dual(cx));
    env.go("C:\\y");
    env.explorer.update(&mut env.visual, |e, cx| e.toggle_compare_panes(cx));
    env.explorer.update(&mut env.visual, |e, cx| e.focus_pane(0, cx));
    env.settle(|env| env.has_label(&format!("Only here: {ARABIC}")));
    env
}

#[gpui_kit::test]
fn compare_panes_marks_the_tiles_that_differ_and_dims_the_same_ones(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = side_by_side(cx, None);
    assert!(env.has_label(&format!("Only here: {ARABIC}")));
    assert!(env.has_label("Older: b.txt"), "newer in the other pane");
    assert!(!env.has_label("Different: b.txt"));
    assert!(!env.has_label("Newer: a.jpg"), "the same has no pill");
    assert!(!env.has_label("Only here: a.jpg"));
    env.visual.update(|window, _| {
        let pill = window.find_by_label("Older: b.txt");
        assert_eq!(pill.description(), Some("Modified earlier than in the other pane"));
    });
    // Every tile is still there.
    assert_eq!(env.tile_names().len(), 4);
}

#[gpui_kit::test]
fn the_diff_pill_is_at_the_end_and_git_s_at_the_start_in_both_directions(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    for rtl in [false, true] {
        let git = FakeGit::default();
        git.put("C:\\x", repo_view(&[(ARABIC, GitMark::Modified)]));
        let mut env = side_by_side(cx, Some(&git));
        env.settle(|env| {
            env.visual
                .update(|window, _| window.try_find(("file-grid-git", 3usize)).is_some())
        });
        // The language is set once the app is (starting it resets it).
        testing::set_rtl(rtl);
        env.visual.update(|window, cx| window.render_frame(cx));
        // The Arabic file is the fourth tile.
        let diff = env.left_edge(("file-grid-diff", 3usize));
        let mark = env.left_edge(("file-grid-git", 3usize));
        if rtl {
            assert!(
                mark > diff,
                "Git's mark is at the start, on the right: {mark} against {diff}"
            );
        } else {
            assert!(
                mark < diff,
                "Git's mark is at the start, on the left: {mark} against {diff}"
            );
        }
        testing::set_rtl(false);
    }
}

#[gpui_kit::test]
fn diff_only_with_nothing_different_says_so_and_a_button_shows_the_identical_items(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = side_by_side(cx, None);
    env.explorer.update(&mut env.visual, |e, cx| e.toggle_diff_only(cx));
    env.settle(|env| env.tile_names().len() == 2);
    assert_eq!(env.tile_names().len(), 2, "b.txt and the Arabic file");
    // The left pane goes to a folder that matches the right one.
    env.explorer.update(&mut env.visual, |e, cx| e.focus_pane(1, cx));
    env.go("C:\\twin");
    env.explorer.update(&mut env.visual, |e, cx| e.focus_pane(0, cx));
    env.go("C:\\x");
    env.settle(|env| env.has_label("No differences in this pane"));
    env.visual.update(|window, _| {
        let note = window.find_by_label("No differences in this pane");
        assert_eq!(note.description(), Some("All 4 items match the other pane."));
    });
    env.visual
        .update(|window, cx| window.click("file-grid-show-identical", cx));
    env.settle(|env| env.tile_names().len() == 4);
    assert!(!env.has_label("No differences in this pane"));
    assert!(!env.explorer.read_with(&env.visual, |e, _| e.diff_only));
}
