//! Headless tests of the menu actions: opening with an app, Windows Sandbox, a new window, showing items, Recent,
//! the wallpaper, rotating, cloud pins, the ncdu export, scripts and remotes on demand. They run on the fake disk
//! behind the same `Source` the app uses for glint-core, with Arabic names where a name is shown.

use super::fake::*;
use super::*;
use crate::testing;
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

pub(super) struct Harness {
    pub explorer: Entity<Explorer>,
    pub disk: FakeDisk,
    pub events: Rc<RefCell<Vec<ExplorerEvent>>>,
}

/// An Arabic folder, an Arabic file and an Arabic script, so every name that is shown is also shown right to left.
const REPORTS: &str = "C:\\a\\تقارير";
const REPORT: &str = "C:\\a\\تقرير.pdf";
const SCRIPT: &str = "C:\\Scripts\\تنظيف.ps1";

pub(super) fn harness(cx: &mut TestAppContext) -> Harness {
    cx.update(testing::init);
    let disk = FakeDisk::new();
    let explorer = cx.update(|cx| cx.new(|cx| Explorer::new(disk.source(), cx)));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    cx.update(|cx| {
        cx.subscribe(&explorer, move |_, event: &ExplorerEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    fixture(&disk);
    Harness { explorer, disk, events }
}

/// `C:\a` holds folders (one with an Arabic name) and files; the folders hold files, so they have sizes.
fn fixture(disk: &FakeDisk) {
    disk.folder("C:\\", vec![folder("C:\\", "a"), folder("C:\\", "b")]);
    disk.folder(
        "C:\\a",
        vec![
            folder("C:\\a", "sub"),
            folder("C:\\a", "big"),
            folder("C:\\a", "تقارير"),
            file("C:\\a", "one.txt", 10, 10),
            file("C:\\a", "تقرير.pdf", 20, 20),
            file("C:\\a", "pic.jpg", 30, 30),
            file("C:\\a", "أرشيف.zip", 40, 40),
        ],
    );
    disk.folder(
        "C:\\a\\sub",
        vec![file("C:\\a\\sub", "deep.txt", 100, 1), folder("C:\\a\\sub", "inner")],
    );
    disk.folder("C:\\a\\sub\\inner", vec![file("C:\\a\\sub\\inner", "x.bin", 50, 1)]);
    disk.folder("C:\\a\\big", vec![file("C:\\a\\big", "y.bin", 5000, 1)]);
    disk.folder(REPORTS, vec![file(REPORTS, "z.txt", 7, 1)]);
    disk.folder("C:\\b", vec![file("C:\\b", "b1.txt", 1, 1)]);
    disk.folder("C:\\Scripts", vec![]);
}

impl Harness {
    pub fn go(&self, cx: &mut TestAppContext, path: &str) -> bool {
        let task = self
            .explorer
            .update(cx, |e, cx| e.navigate_task(path, NavOptions::default(), cx));
        let done = cx.foreground_executor().block_test(task);
        cx.run_until_parked();
        done
    }

    /// Runs an action that returns a task, and everything it sets going.
    pub fn task<T: 'static>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Explorer, &mut gpui_kit::Context<Explorer>) -> gpui_kit::Task<T>,
    ) -> T {
        let task = self.explorer.update(cx, f);
        let result = cx.foreground_executor().block_test(task);
        cx.run_until_parked();
        result
    }

    pub fn run<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Explorer, &mut gpui_kit::Context<Explorer>) -> R,
    ) -> R {
        let result = self.explorer.update(cx, f);
        cx.run_until_parked();
        result
    }

    pub fn read<R>(&self, cx: &TestAppContext, f: impl FnOnce(&Explorer) -> R) -> R {
        self.explorer.read_with(cx, |e, _| f(e))
    }

    pub fn path(&self, cx: &TestAppContext) -> String {
        self.read(cx, |e| e.current_path().to_string())
    }

    pub fn names(&self, cx: &TestAppContext) -> Vec<String> {
        self.read(cx, |e| {
            e.active_tab()
                .map(|t| t.rows.iter().map(|r| r.name.clone()).collect())
                .unwrap_or_default()
        })
    }

    pub fn events(&self) -> Vec<ExplorerEvent> {
        self.events.borrow().clone()
    }

    /// What the menu actions told the person, in order.
    pub fn said(&self) -> Vec<ActionNotice> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                ExplorerEvent::Action(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    pub fn requests(&self) -> Vec<Request> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                ExplorerEvent::Request(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    pub fn exported(&self) -> Vec<ExportNotice> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                ExplorerEvent::Export(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    pub fn notices(&self) -> Vec<Notice> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                ExplorerEvent::Notice(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    /// How many times a folder was listed from the disk.
    pub fn listed(&self, path: &str) -> usize {
        self.disk.listed().iter().filter(|p| p.as_str() == path).count()
    }
}

fn script(name: &str, path: &str, missing: Option<&str>) -> Script {
    Script {
        name: name.into(),
        path: path.into(),
        program: "C:\\bin\\pwsh.exe".into(),
        args: vec!["-File".into(), path.into()],
        missing: missing.map(str::to_string),
    }
}

// ---- Opening ------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_file_on_disk_opens_with_its_app_and_a_shortcut_to_a_folder_opens_that_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.disk.on_open(REPORT, None);
    h.task(cx, |e, cx| e.open_on_disk(REPORT, OpenHow::Default, cx));
    assert_eq!(h.path(cx), "C:\\a", "the file's app opened it, the tab stayed");
    assert!(h.notices().is_empty());

    h.disk.on_open("C:\\a\\one.txt", Some("C:\\b"));
    h.task(cx, |e, cx| e.open_on_disk("C:\\a\\one.txt", OpenHow::Default, cx));
    assert_eq!(h.path(cx), "C:\\b");
}

#[gpui_kit::test]
fn the_editor_and_the_app_picker_open_a_file_and_say_why_when_they_cannot(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.task(cx, |e, cx| {
        e.open_on_disk(REPORT, OpenHow::Editor("C:\\Code\\code.exe".into()), cx)
    });
    h.task(cx, |e, cx| e.open_on_disk(REPORT, OpenHow::Picker, cx));
    assert_eq!(
        h.disk.opened(),
        [
            (REPORT.to_string(), OpenHow::Editor("C:\\Code\\code.exe".into())),
            (REPORT.to_string(), OpenHow::Picker),
        ]
    );
    assert!(h.said().is_empty());

    h.disk.fail_open_next("The editor isn't there");
    h.task(cx, |e, cx| {
        e.open_on_disk(REPORT, OpenHow::Editor("C:\\gone.exe".into()), cx)
    });
    h.disk.fail_open_next("No picker");
    h.task(cx, |e, cx| e.open_on_disk(REPORT, OpenHow::Picker, cx));
    assert_eq!(
        h.said(),
        [
            ActionNotice::Failed {
                what: ActionFailure::OpenFile,
                message: "The editor isn't there".into()
            },
            ActionNotice::Failed {
                what: ActionFailure::AppPicker,
                message: "No picker".into()
            },
        ]
    );
}

#[gpui_kit::test]
fn sandbox_opens_a_copy_and_says_how_to_turn_it_on_where_windows_has_none(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.task(cx, |e, cx| e.run_in_sandbox("C:\\a\\setup.exe", cx));
    assert_eq!(h.disk.sandboxed(), ["C:\\a\\setup.exe"]);
    assert_eq!(h.said(), [ActionNotice::SandboxOpening]);

    h.disk.fail_action("sandbox", "notInstalled");
    h.task(cx, |e, cx| e.run_in_sandbox("C:\\a\\setup.exe", cx));
    h.disk
        .fail_action("sandbox", "Couldn't copy it for the sandbox: disk full");
    h.task(cx, |e, cx| e.run_in_sandbox(REPORT, cx));
    assert_eq!(
        h.said()[1..],
        [
            ActionNotice::SandboxNotOn,
            ActionNotice::Failed {
                what: ActionFailure::Sandbox,
                message: "Couldn't copy it for the sandbox: disk full".into()
            },
        ]
    );
    assert_eq!(h.disk.sandboxed().len(), 1, "a refused run is not a run");
}

#[gpui_kit::test]
fn optional_features_and_a_folder_open_through_the_shell_and_a_failure_is_told(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.task(cx, |e, cx| e.open_optional_features(cx));
    h.task(cx, |e, cx| {
        e.open_in_file_explorer("C:\\Users\\me\\Glint remotes\\مجلد", cx)
    });
    assert_eq!(
        h.disk.shell_opened(),
        [
            "C:\\Windows\\System32\\OptionalFeatures.exe".to_string(),
            "C:\\Users\\me\\Glint remotes\\مجلد".to_string()
        ]
    );
    h.disk.fail_action("shell_open", "no such program");
    h.task(cx, |e, cx| e.open_optional_features(cx));
    h.disk.fail_action("shell_open", "no such folder");
    h.task(cx, |e, cx| e.open_in_file_explorer("C:\\gone", cx));
    assert_eq!(
        h.said(),
        [
            ActionNotice::Failed {
                what: ActionFailure::OptionalFeatures,
                message: "no such program".into()
            },
            ActionNotice::Failed {
                what: ActionFailure::FileExplorer,
                message: "no such folder".into()
            },
        ]
    );
}

#[gpui_kit::test]
fn open_in_new_window_asks_for_a_window_on_a_folder_and_a_place_that_is_not_one_gets_home(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.open_in_new_window(Some(REPORTS), cx));
    // Ctrl+N: the folder shown.
    h.run(cx, |e, cx| e.open_in_new_window(None, cx));
    // The Recycle Bin and This PC are not folders a window opens on.
    h.run(cx, |e, cx| {
        e.open_in_new_window(Some(glint_model::paths::RECYCLE_BIN), cx)
    });
    let home = h.read(cx, |e| e.home_folder().to_string());
    assert_eq!(
        h.requests(),
        [
            Request::NewWindow { path: REPORTS.into() },
            Request::NewWindow { path: "C:\\a".into() },
            Request::NewWindow { path: home },
        ]
    );
}

// ---- Showing ---------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn open_file_location_opens_the_folder_the_item_is_in_with_it_selected(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\b"));
    assert!(h.task(cx, |e, cx| e.reveal_in_app(REPORT, cx)));
    assert_eq!(h.path(cx), "C:\\a");
    assert_eq!(
        h.read(cx, |e| e.active_tab().map(|t| t.selection.to_vec())),
        Some(vec![REPORT.to_string()])
    );
}

#[gpui_kit::test]
fn show_in_file_explorer_hands_over_the_items_and_says_when_it_could_not(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.task(cx, |e, cx| {
        e.reveal_in_file_explorer(vec![REPORT.to_string(), "C:\\b\\b1.txt".to_string()], cx)
    });
    assert_eq!(
        h.disk.revealed(),
        [vec![REPORT.to_string(), "C:\\b\\b1.txt".to_string()]]
    );
    h.disk.fail_action("reveal", "Windows doesn't know the folder");
    h.task(cx, |e, cx| e.reveal_in_file_explorer(vec!["C:\\gone\\x".into()], cx));
    assert_eq!(
        h.said(),
        [ActionNotice::Failed {
            what: ActionFailure::FileExplorer,
            message: "Windows doesn't know the folder".into()
        }]
    );
}

#[gpui_kit::test]
fn remove_from_recent_says_so_and_reads_the_folder_again_even_when_it_fails(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    let before = h.listed("C:\\a");
    let paths = vec![REPORT.to_string(), "C:\\a\\one.txt".to_string()];
    h.task(cx, |e, cx| e.forget_recent(paths.clone(), cx));
    assert_eq!(h.disk.forgotten(), std::slice::from_ref(&paths));
    assert_eq!(h.said(), [ActionNotice::TookOutOfRecent { paths }]);
    assert_eq!(h.listed("C:\\a"), before + 1, "the list is read again");

    h.disk.fail_action("forget_recent", "Recent is locked");
    h.task(cx, |e, cx| e.forget_recent(vec![REPORT.into()], cx));
    assert_eq!(
        h.said()[1],
        ActionNotice::Failed {
            what: ActionFailure::Recent,
            message: "Recent is locked".into()
        }
    );
    assert_eq!(h.listed("C:\\a"), before + 2);
}

#[gpui_kit::test]
fn rename_items_in_this_folder_opens_the_batch_rename_with_every_row(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.batch_rename_folder(cx));
    let wanted: Vec<String> = h.names(cx).iter().map(|n| format!("C:\\a\\{n}")).collect();
    assert_eq!(wanted.len(), 7);
    assert!(
        h.events().contains(&ExplorerEvent::BatchRename(wanted)),
        "every row of the folder, in the order shown"
    );
}

#[gpui_kit::test]
fn rename_items_in_this_folder_says_when_the_folder_is_read_only(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let outer = "C:\\a\\أرشيف.zip";
    h.disk
        .archive_folder(outer, outer, vec![file(outer, "inner.txt", 1, 1)]);
    assert!(h.go(cx, outer));
    h.run(cx, |e, cx| e.batch_rename_folder(cx));
    assert_eq!(h.notices(), [Notice::ReadOnly { remote: false }]);
    assert!(!h.events().iter().any(|e| matches!(e, ExplorerEvent::BatchRename(_))));
}

// ---- Pictures and cloud files -------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_picture_becomes_the_desktop_background_or_the_reason_it_could_not_is_told(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.task(cx, |e, cx| e.set_wallpaper("C:\\a\\صورة.png", cx));
    assert_eq!(h.disk.wallpapers(), ["C:\\a\\صورة.png"]);
    h.disk.fail_action("wallpaper", "Windows refused");
    h.task(cx, |e, cx| e.set_wallpaper("C:\\a\\pic.jpg", cx));
    assert_eq!(
        h.said(),
        [
            ActionNotice::WallpaperSet {
                path: "C:\\a\\صورة.png".into()
            },
            ActionNotice::Failed {
                what: ActionFailure::Wallpaper,
                message: "Windows refused".into()
            },
        ]
    );
}

#[gpui_kit::test]
fn rotating_turns_each_picture_says_nothing_when_all_turned_and_reads_the_folder_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    let before = h.listed("C:\\a");
    h.task(cx, |e, cx| {
        e.rotate_pictures(vec!["C:\\a\\pic.jpg".into(), "C:\\a\\two.png".into()], true, cx)
    });
    h.task(cx, |e, cx| e.rotate_pictures(vec!["C:\\a\\pic.jpg".into()], false, cx));
    assert_eq!(
        h.disk.rotated(),
        [
            ("C:\\a\\pic.jpg".to_string(), true),
            ("C:\\a\\two.png".to_string(), true),
            ("C:\\a\\pic.jpg".to_string(), false),
        ]
    );
    assert!(h.said().is_empty());
    assert_eq!(h.listed("C:\\a"), before + 2);
}

#[gpui_kit::test]
fn pictures_that_would_not_turn_are_told_together_with_the_first_three_reasons(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    let before = h.listed("C:\\a");
    let bad = ["C:\\a\\تقرير.jpg", "C:\\a\\b.png", "C:\\a\\c.bmp", "C:\\a\\d.tif"];
    for path in bad {
        h.disk.fail_rotating(path, "Not a picture");
    }
    let mut paths: Vec<String> = bad.iter().map(|p| p.to_string()).collect();
    paths.push("C:\\a\\pic.jpg".into());
    h.task(cx, |e, cx| e.rotate_pictures(paths, true, cx));
    assert_eq!(
        h.disk.rotated(),
        [("C:\\a\\pic.jpg".to_string(), true)],
        "the others went on"
    );
    assert_eq!(
        h.said(),
        [ActionNotice::RotateFailed {
            count: 4,
            detail: "تقرير.jpg: Not a picture\nb.png: Not a picture\nc.bmp: Not a picture".into()
        }]
    );
    assert_eq!(h.listed("C:\\a"), before + 1);
}

#[gpui_kit::test]
fn cloud_pins_are_set_told_and_read_again_and_each_failure_has_its_own_words(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    let before = h.listed("C:\\a");
    let paths = vec![REPORT.to_string()];
    h.task(cx, |e, cx| e.cloud_pin(paths.clone(), PinState::Keep, cx));
    h.task(cx, |e, cx| e.cloud_pin(paths.clone(), PinState::Free, cx));
    h.task(cx, |e, cx| e.cloud_pin(paths.clone(), PinState::Auto, cx));
    assert_eq!(
        h.disk.pins(),
        [
            (paths.clone(), PinState::Keep),
            (paths.clone(), PinState::Free),
            (paths.clone(), PinState::Auto)
        ]
    );
    assert_eq!(
        h.said(),
        [PinState::Keep, PinState::Free, PinState::Auto].map(|state| ActionNotice::CloudPinned {
            state,
            paths: paths.clone()
        })
    );
    assert_eq!(h.listed("C:\\a"), before + 3);

    h.disk.fail_action("pin", "OneDrive isn't running");
    h.task(cx, |e, cx| e.cloud_pin(paths.clone(), PinState::Free, cx));
    h.disk.fail_action("pin", "OneDrive isn't running");
    h.task(cx, |e, cx| e.cloud_pin(paths.clone(), PinState::Keep, cx));
    let said = h.said();
    assert_eq!(
        said[3..],
        [
            ActionNotice::Failed {
                what: ActionFailure::FreeUpSpace,
                message: "OneDrive isn't running".into()
            },
            ActionNotice::Failed {
                what: ActionFailure::CloudKept,
                message: "OneDrive isn't running".into()
            },
        ]
    );
    assert_eq!(
        h.listed("C:\\a"),
        before + 3,
        "a failure changed nothing, so nothing is read again"
    );
}

// ---- Export as ncdu JSON ---------------------------------------------------------------------------------------

#[test]
fn the_export_is_suggested_a_name_from_the_folder_without_colons_and_backslashes() {
    assert_eq!(Explorer::ncdu_file_name("C:\\a"), "a.ncdu");
    assert_eq!(Explorer::ncdu_file_name("C:\\a\\تقارير"), "تقارير.ncdu");
    // A drive is "C:" to `base_name`, and the colon goes; only a name that is nothing at all is "drive".
    assert_eq!(Explorer::ncdu_file_name("C:\\"), "C.ncdu");
    assert_eq!(Explorer::ncdu_file_name(""), "drive.ncdu");
}

#[gpui_kit::test]
fn an_export_writes_the_file_and_says_where(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.task(cx, |e, cx| e.export_ncdu(REPORTS, "C:\\out\\تقارير.ncdu", cx));
    assert_eq!(
        h.disk.exports(),
        [(
            REPORTS.to_string(),
            "C:\\out\\تقارير.ncdu".to_string(),
            "tab1".to_string()
        )]
    );
    let told = h.exported();
    assert!(matches!(
        &told[..],
        [
            ExportNotice::Ended {
                was_shown: false,
                idle: true,
                ..
            },
            ExportNotice::Done { .. }
        ]
    ));
    assert_eq!(
        told[1],
        ExportNotice::Done {
            name: "تقارير".into(),
            dest: "C:\\out\\تقارير.ncdu".into()
        }
    );
}

#[gpui_kit::test]
fn a_slow_export_shows_a_notice_that_counts_entries_and_stop_ends_it_saying_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let gate = h.disk.hold_export();
    h.disk.script_export_progress(&[(120, 0), (120, 0), (450, 0)]);
    let exporting = h
        .explorer
        .update(cx, |e, cx| e.export_ncdu("C:\\a", "C:\\out\\a.ncdu", cx));
    cx.run_until_parked();
    assert_eq!(h.read(cx, |e| e.busy_count()), 1, "an update waits for it");
    let counted: Vec<(u64, bool)> = h
        .exported()
        .into_iter()
        .filter_map(|n| match n {
            ExportNotice::Progress { done, shown, .. } => Some((done, shown)),
            _ => None,
        })
        .collect();
    assert_eq!(
        counted,
        [(120, false), (450, false)],
        "a repeated count is not told twice"
    );

    cx.executor().advance_clock(Duration::from_millis(450));
    cx.run_until_parked();
    let id = h
        .exported()
        .into_iter()
        .find_map(|n| match n {
            ExportNotice::Shown { id, name, done, total } => {
                assert_eq!((name.as_str(), done, total), ("a", 450, 0));
                Some(id)
            }
            _ => None,
        })
        .expect("the notice shows after 400 ms");

    h.run(cx, |e, _| e.stop_archive_work(id));
    assert_eq!(h.disk.stopped(), ["tab1"], "an export stops through its tab's lane");
    gate.close();
    cx.foreground_executor().block_test(exporting);
    cx.run_until_parked();
    let told = h.exported();
    assert!(!told
        .iter()
        .any(|n| matches!(n, ExportNotice::Failed { .. } | ExportNotice::Done { .. })));
    assert!(matches!(
        told.last(),
        Some(ExportNotice::Ended {
            was_shown: true,
            idle: true,
            ..
        })
    ));
    assert_eq!(h.read(cx, |e| e.busy_count()), 0);
}

#[gpui_kit::test]
fn a_failed_export_says_why(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.disk.fail_action("export", "Access is denied");
    h.task(cx, |e, cx| e.export_ncdu(REPORTS, "C:\\Windows\\x.ncdu", cx));
    assert!(h.exported().contains(&ExportNotice::Failed {
        name: "تقارير".into(),
        message: "Access is denied".into()
    }));
}

// ---- Scripts -----------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_script_runs_in_the_terminal_with_its_arguments_then_the_paths_chosen(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.disk.set_scripts("C:\\Scripts", vec![script("تنظيف", SCRIPT, None)]);
    let paths = vec![REPORT.to_string(), "C:\\a\\one.txt".to_string()];
    // The menu hands over the path as it listed it; a different case is the same file.
    h.task(cx, |e, cx| {
        e.run_script("c:\\scripts\\تنظيف.ps1", "C:\\a", paths.clone(), cx)
    });
    let requests = h.requests();
    assert_eq!(
        requests,
        [Request::Terminal(TerminalJob {
            cwd: "C:\\a".into(),
            title: "تنظيف".into(),
            program: "C:\\bin\\pwsh.exe".into(),
            args: vec!["-File".into(), SCRIPT.into(), REPORT.into(), "C:\\a\\one.txt".into()],
        })]
    );
    let Request::Terminal(job) = &requests[0] else {
        panic!("a terminal job");
    };
    assert_eq!(
        job.line("pwsh"),
        format!("& 'C:\\bin\\pwsh.exe' '-File' '{SCRIPT}' '{REPORT}' 'C:\\a\\one.txt'\r")
    );
    assert!(h.said().is_empty());
}

#[gpui_kit::test]
fn a_script_whose_tool_is_missing_asks_for_it_instead_of_running(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.disk
        .set_scripts("C:\\Scripts", vec![script("تنظيف", SCRIPT, Some("uv"))]);
    h.task(cx, |e, cx| e.run_script(SCRIPT, "C:\\a", vec![], cx));
    assert_eq!(h.requests(), [Request::InstallTool { tool: "uv".into() }]);
}

#[gpui_kit::test]
fn a_selection_too_big_for_a_command_line_is_refused_with_the_numbers(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.disk.set_scripts("C:\\Scripts", vec![script("تنظيف", SCRIPT, None)]);
    // 12 characters and 3 more each: 15 * 534 = 8010 is over 8000; 15 * 533 = 7995 is not.
    let many = |n: usize| vec!["C:\\a\\one.txt".to_string(); n];
    h.task(cx, |e, cx| e.run_script(SCRIPT, "C:\\a", many(533), cx));
    assert_eq!(h.requests().len(), 1, "533 paths fit");
    h.task(cx, |e, cx| e.run_script(SCRIPT, "C:\\a", many(534), cx));
    assert_eq!(h.requests().len(), 1, "534 paths do not");
    assert_eq!(
        h.said(),
        [ActionNotice::ScriptTooLong {
            name: "تنظيف".into(),
            paths: 534,
            length: 8010,
            longest: 8000
        }]
    );
}

#[gpui_kit::test]
fn a_script_that_is_gone_says_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.task(cx, |e, cx| e.run_script(SCRIPT, "C:\\a", vec![], cx));
    assert_eq!(h.said(), [ActionNotice::ScriptGone { path: SCRIPT.into() }]);
    assert!(h.requests().is_empty());
}

#[gpui_kit::test]
fn the_scripts_folder_opens_in_the_tab_and_a_failure_is_told(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.disk.set_scripts("C:\\Scripts", vec![]);
    assert!(h.task(cx, |e, cx| e.open_scripts_folder(cx)));
    assert_eq!(h.path(cx), "C:\\Scripts");

    h.disk
        .fail_action("scripts_folder", "Glint's data folder isn't available");
    assert!(!h.task(cx, |e, cx| e.open_scripts_folder(cx)));
    assert_eq!(h.path(cx), "C:\\Scripts");
    assert_eq!(
        h.said(),
        [ActionNotice::Failed {
            what: ActionFailure::ScriptsFolder,
            message: "Glint's data folder isn't available".into()
        }]
    );
}

// ---- Remotes in File Explorer, on demand --------------------------------------------------------------------

#[gpui_kit::test]
fn a_remote_is_shown_in_file_explorer_then_hidden_and_the_list_follows(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.read(cx, |e| e.on_demand().is_empty()));
    h.task(cx, |e, cx| e.show_on_demand("nas", "مخزن", cx));
    let folder = "C:\\Users\\me\\Glint remotes\\مخزن".to_string();
    assert_eq!(
        h.read(cx, |e| e.on_demand().to_vec()),
        [OnDemand {
            id: "nas".into(),
            folder: folder.clone()
        }]
    );
    assert_eq!(
        h.said(),
        [ActionNotice::OnDemandShown {
            name: "مخزن".into(),
            folder
        }]
    );
    assert!(h.events().contains(&ExplorerEvent::OnDemandChanged));

    h.task(cx, |e, cx| e.hide_on_demand("nas", cx));
    assert!(h.read(cx, |e| e.on_demand().is_empty()));
    assert_eq!(h.said().len(), 1, "taking it away says nothing");
}

#[gpui_kit::test]
fn showing_or_hiding_a_remote_that_fails_says_why_and_the_list_is_read_at_start(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.disk.set_mounts(vec![OnDemand {
        id: "ftp".into(),
        folder: "C:\\Users\\me\\Glint remotes\\ftp".into(),
    }]);
    h.task(cx, |e, cx| e.refresh_on_demand(cx));
    assert_eq!(h.read(cx, |e| e.on_demand().len()), 1);

    h.disk.fail_action("mount", "The cloud files provider isn't running");
    h.task(cx, |e, cx| e.show_on_demand("nas", "NAS", cx));
    h.disk.fail_action("unmount", "Still in use");
    h.task(cx, |e, cx| e.hide_on_demand("ftp", cx));
    assert_eq!(
        h.said(),
        [
            ActionNotice::Failed {
                what: ActionFailure::ShowOnDemand,
                message: "The cloud files provider isn't running".into()
            },
            ActionNotice::Failed {
                what: ActionFailure::HideOnDemand,
                message: "Still in use".into()
            },
        ]
    );
    assert_eq!(h.read(cx, |e| e.on_demand().len()), 1, "nothing changed");
}
