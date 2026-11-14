//! Debug builds only: drives the real app through file operations without synthetic input, so the coordinator
//! can check the real Windows paths (the clipboard, IFileOperation, undo) that headless tests fake.
//!
//! `GLINT_SELFTEST=ops:<folder>` (a scratch folder holding `a.txt` and `b.txt`): Glint opens it, selects
//! everything, copies and pastes it (through the window's Copy and Paste actions), renames `a.txt` to
//! `renamed.txt` (the Rename action, then what the file list's rename box sends), undoes the rename, searches
//! the folder for the copies, writes what it saw to `<folder>\selftest.log` and quits.
//!
//! `GLINT_SELFTEST=show:<file>` opens the file's folder with it selected and the preview pane on, and stays
//! open for a look (a capture). Release builds ignore the variable.
//!
//! `GLINT_SELFTEST=wave9:<folder>`: folder sizes and Git's marks (see `wave9_checks`).
//!
//! `GLINT_SELFTEST=wave8:<folder>` (a scratch folder holding `a.txt` with the word "needle" in it, and `b.txt`):
//! text search finds the needle (Ctrl+Shift+F, then a search as a recent one runs) and goes back to the files;
//! the disk usage map comes up and goes again; Compare opens on the two files; the two are compressed into
//! `a.zip` and that is extracted beside it. Nothing is recycled. The log is `<folder>\selftest.log`.

use crate::explorer;
use crate::kit::prelude::*;
use crate::{actions, startup};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Starts the self-test the environment asks for, if any (debug builds only).
pub(crate) fn start(cx: &mut App) {
    if !cfg!(debug_assertions) {
        return;
    }
    let Ok(script) = std::env::var("GLINT_SELFTEST") else {
        return;
    };
    // `show:<file>`: the file's folder with it selected and the preview pane on, left open for a look.
    if let Some(file) = script.strip_prefix("show:") {
        let folder = glint_model::paths::parent_path(file);
        let options = explorer::NavOptions {
            select: Some(vec![file.to_string()]),
            ..Default::default()
        };
        explorer::explorer(cx).update(cx, |e, cx| e.navigate(folder, options, cx));
        crate::state::update(cx, |settings| settings.show_preview_pane = true);
        return;
    }
    // `term:<folder>`: the terminal pane open in the folder, a command typed into it, left open for a look.
    if let Some(folder) = script.strip_prefix("term:") {
        let folder = folder.to_string();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(800)).await;
            cx.update(|cx| {
                let Some(terminal) = startup::main_workspace(cx).map(|w| w.read(cx).terminal()) else {
                    return;
                };
                if let Some(main) = startup::main_window(cx) {
                    let _ = main.update(cx, |_, window, cx| {
                        terminal.update(cx, |pane, cx| {
                            pane.run_task(&folder, "echo hello from Glint's terminal && dir", window, cx)
                        })
                    });
                }
            });
        })
        .detach();
        return;
    }
    // `look13:<grid|tags|palette>|<folder>` (a scratch folder holding `a.txt`), left open for a look: the grid with
    // the rename box on `a.txt`; the list, whose rows show the tags their files' streams hold; the Ctrl+K palette
    // asked for "run" (the folder's project tasks).
    if let Some(rest) = script.strip_prefix("look13:") {
        let (what, folder) = rest.split_once('|').unwrap_or(("grid", rest));
        let (what, folder) = (what.to_string(), folder.to_string());
        let a = Path::new(&folder).join("a.txt").to_string_lossy().into_owned();
        let options = explorer::NavOptions {
            select: Some(vec![a.clone()]),
            ..Default::default()
        };
        explorer::explorer(cx).update(cx, |e, cx| e.navigate(folder, options, cx));
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(800)).await;
            cx.update(|cx| {
                let explorer = explorer::explorer(cx);
                match what.as_str() {
                    "grid" => explorer.update(cx, |e, cx| {
                        e.set_view(explorer::ViewMode::Grid, None, cx);
                        e.begin_rename(Some(&a), cx);
                    }),
                    "palette" => {
                        if let Some(main) = startup::main_window(cx) {
                            let _ = main.update(cx, |_, window, cx| {
                                crate::components::palette::open(window, cx);
                                crate::components::palette::type_query("run", window, cx);
                            });
                        }
                    }
                    _ => explorer.update(cx, |e, cx| e.set_view(explorer::ViewMode::List, None, cx)),
                }
            });
        })
        .detach();
        return;
    }
    // `look14:<compare|deep>|<left>|<right>`, `look14:palette=<query>|<folder>`, `look14:open|<path>`, left open for a
    // look: the two folders compared in the panes (deep: subfolders too); the Ctrl+K palette asked for `query`;
    // a path opened (an archive with a password asks for it).
    if let Some(rest) = script.strip_prefix("look14:") {
        let parts: Vec<String> = rest.split('|').map(str::to_string).collect();
        let what = parts.first().cloned().unwrap_or_default();
        let first = parts.get(1).cloned().unwrap_or_default();
        let second = parts.get(2).cloned().unwrap_or_default();
        if what == "deep" {
            crate::state::update(cx, |settings| settings.compare_subfolders = true);
        }
        if what.starts_with("palette") || what == "open" {
            explorer::explorer(cx).update(cx, |e, cx| e.navigate(first.clone(), Default::default(), cx));
        }
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(800)).await;
            cx.update(|cx| {
                if let Some(query) = what.strip_prefix("palette=") {
                    if let Some(main) = startup::main_window(cx) {
                        let _ = main.update(cx, |_, window, cx| {
                            crate::components::palette::open(window, cx);
                            crate::components::palette::type_query(query, window, cx);
                        });
                    }
                } else if what == "compare" || what == "deep" {
                    explorer::explorer(cx).update(cx, |e, cx| e.compare_with_version(&first, &second, cx).detach());
                }
            });
        })
        .detach();
        return;
    }
    // `look15:window|<folder>`: Open in new window on the folder (a secondary Glint process). `look15:search|<folder>|
    // <query>`: the folder searched for the query, with the search bar over the pane. Both left open for a look.
    if let Some(rest) = script.strip_prefix("look15:") {
        let parts: Vec<String> = rest.split('|').map(str::to_string).collect();
        let what = parts.first().cloned().unwrap_or_default();
        let folder = parts.get(1).cloned().unwrap_or_default();
        let query = parts.get(2).cloned().unwrap_or_default();
        explorer::explorer(cx).update(cx, |e, cx| e.navigate(folder.clone(), Default::default(), cx));
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(900)).await;
            cx.update(|cx| {
                explorer::explorer(cx).update(cx, |e, cx| match what.as_str() {
                    "window" => e.open_in_new_window(Some(&folder), cx),
                    _ => e.run_find(&query, explorer::Wider::default(), cx).detach(),
                })
            });
        })
        .detach();
        return;
    }
    // `map:<folder>`: the folder's disk usage map, left open for a look.
    if let Some(folder) = script.strip_prefix("map:") {
        let folder = folder.to_string();
        explorer::explorer(cx).update(cx, |e, cx| e.navigate(folder, Default::default(), cx));
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(800)).await;
            dispatch(cx, Box::new(actions::DiskUsage));
        })
        .detach();
        return;
    }
    // `text:<folder>|<query>`: the folder searched for the query (names, and inside the files), left open for a look.
    if let Some(rest) = script.strip_prefix("text:") {
        let (folder, query) = rest.split_once('|').unwrap_or((rest, "needle"));
        let (target, query) = (folder.to_string(), query.to_string());
        explorer::explorer(cx).update(cx, |e, cx| e.navigate(target, Default::default(), cx));
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(800)).await;
            cx.update(|cx| {
                explorer::explorer(cx).update(cx, |e, cx| {
                    e.set_filter(&query, None, cx);
                    e.run_find(&query, Default::default(), cx).detach();
                })
            });
        })
        .detach();
        return;
    }
    let Some((name, folder)) = script.split_once(':') else {
        eprintln!("glint: GLINT_SELFTEST: unknown script {script:?}");
        return;
    };
    let (name, folder) = (name.to_string(), PathBuf::from(folder));
    if !["ops", "wave8", "wave9"].contains(&name.as_str()) {
        eprintln!("glint: GLINT_SELFTEST: unknown script {script:?}");
        return;
    }
    cx.spawn(async move |cx| {
        let log = match name.as_str() {
            "wave8" => wave8_checks(&folder, cx).await,
            "wave9" => wave9_checks(&folder, cx).await,
            _ => ops(&folder, cx).await,
        };
        let _ = std::fs::write(folder.join("selftest.log"), log.join("\r\n"));
        cx.update(|cx| cx.quit());
    })
    .detach();
}

/// The folder's file names, sorted (the log's evidence).
fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|dir| {
            dir.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n != "selftest.log")
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Waits until `ready` holds (polling), or `limit` passes. True if it held.
async fn wait_for(cx: &mut AsyncApp, limit: Duration, mut ready: impl FnMut(&mut AsyncApp) -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if ready(cx) {
            return true;
        }
        cx.background_executor().timer(Duration::from_millis(100)).await;
    }
    ready(cx)
}

/// Dispatches `action` in the main window, as a key or a menu would.
fn dispatch(cx: &mut AsyncApp, action: Box<dyn Action>) {
    cx.update(|cx| {
        if let Some(main) = startup::main_window(cx) {
            let _ = main.update(cx, |_, window, cx| window.dispatch_action(action, cx));
        }
    });
}

async fn ops(folder: &Path, cx: &mut AsyncApp) -> Vec<String> {
    let mut log = Vec::new();
    let dir = folder.to_string_lossy().into_owned();
    let check = |log: &mut Vec<String>, step: &str, ok: bool, folder: &Path| {
        log.push(format!(
            "{} {step}: {:?}",
            if ok { "OK  " } else { "FAIL" },
            names(folder)
        ));
        ok
    };

    // 1. The folder is open in the active tab.
    let target = dir.clone();
    cx.update(|cx| explorer::explorer(cx).update(cx, |e, cx| e.navigate(target, Default::default(), cx)));
    let opened = wait_for(cx, Duration::from_secs(10), |cx| {
        cx.update(|cx| {
            let e = explorer::explorer(cx).read(cx);
            e.active_tab()
                .is_some_and(|t| t.path.eq_ignore_ascii_case(&dir) && t.rows.len() >= 2)
        })
    })
    .await;
    if !check(&mut log, "open the folder", opened, folder) {
        return log;
    }

    // 2. Everything selected, copied (Ctrl+C), pasted (Ctrl+V): Windows names the copies.
    cx.update(|cx| {
        explorer::explorer(cx).update(cx, |e, cx| {
            if let Some(rows) = e.active_tab().map(|t| t.rows.clone()) {
                e.select_all(&rows, cx);
            }
        })
    });
    dispatch(cx, Box::new(actions::Copy));
    let copied = wait_for(cx, Duration::from_secs(5), |_| {
        glint_core::shell::clipboard_paths().len() >= 2
    })
    .await;
    check(&mut log, "copy to the clipboard", copied, folder);
    dispatch(cx, Box::new(actions::Paste));
    let pasted = wait_for(cx, Duration::from_secs(15), |_| names(folder).len() >= 4).await;
    check(&mut log, "paste", pasted, folder);

    // 3. a.txt renamed (F2, then the rename box's Enter), then the rename undone (Ctrl+Z).
    let a = folder.join("a.txt").to_string_lossy().into_owned();
    cx.update(|cx| explorer::explorer(cx).update(cx, |e, cx| e.select_only(Some(&a), cx)));
    dispatch(cx, Box::new(actions::Rename));
    let renaming = wait_for(cx, Duration::from_secs(5), |cx| {
        cx.update(|cx| {
            explorer::explorer(cx)
                .read(cx)
                .active_tab()
                .is_some_and(|t| t.renaming.is_some())
        })
    })
    .await;
    check(&mut log, "rename box opens", renaming, folder);
    cx.update(|cx| {
        explorer::explorer(cx).update(cx, |e, cx| {
            let tab = e.active_tab_id();
            e.commit_rename_in(tab, "renamed.txt", cx).detach()
        })
    });
    let renamed = wait_for(cx, Duration::from_secs(10), |_| folder.join("renamed.txt").exists()).await;
    check(&mut log, "rename", renamed, folder);
    // Windows renames before the operation returns and the explorer records its undo step: wait for the step,
    // or Ctrl+Z would take back the paste instead (a person can't press it that fast).
    let recorded = wait_for(cx, Duration::from_secs(5), |cx| {
        cx.update(|cx| {
            explorer::explorer(cx)
                .read(cx)
                .next_undo()
                .is_some_and(|(_, label)| label.starts_with("rename"))
        })
    })
    .await;
    if !check(&mut log, "the rename can be undone", recorded, folder) {
        return log;
    }
    dispatch(cx, Box::new(actions::Undo));
    let undone = wait_for(cx, Duration::from_secs(10), |_| {
        folder.join("a.txt").exists() && !folder.join("renamed.txt").exists()
    })
    .await;
    check(&mut log, "undo the rename", undone, folder);

    // 4. A search below the folder (as the address bar's Enter runs it) finds the two copies.
    cx.update(|cx| {
        explorer::explorer(cx).update(cx, |e, cx| e.run_find("Copy", explorer::Wider::default(), cx).detach())
    });
    let found = wait_for(cx, Duration::from_secs(20), |cx| {
        cx.update(|cx| {
            explorer::explorer(cx)
                .read(cx)
                .active_tab()
                .and_then(|t| t.find.as_ref())
                .is_some_and(|find| !find.running && find.entries.len() == 2)
        })
    })
    .await;
    let how = cx.update(|cx| {
        explorer::explorer(cx)
            .read(cx)
            .active_tab()
            .and_then(|t| t.find.as_ref())
            .map(|find| format!("{} found by {} ({})", find.entries.len(), find.engine, find.error))
            .unwrap_or_default()
    });
    check(&mut log, &format!("search: {how}"), found, folder);
    log
}

/// Runs `act` in the main window.
fn in_main(cx: &mut AsyncApp, act: impl FnOnce(&mut Window, &mut App)) {
    cx.update(|cx| {
        if let Some(main) = startup::main_window(cx) {
            let _ = main.update(cx, |_, window, cx| act(window, cx));
        }
    });
}

/// The active tab's search: (a search shows, it still runs by name or inside files, lines matched inside files).
fn search_state(cx: &mut AsyncApp) -> (bool, bool, usize) {
    cx.update(|cx| {
        let explorer = explorer::explorer(cx).read(cx);
        match explorer.active_tab().and_then(|tab| tab.find.as_ref()) {
            Some(find) => (true, find.running || find.inside.running, find.inside.matches),
            None => (false, false, 0),
        }
    })
}

async fn wave8_checks(folder: &Path, cx: &mut AsyncApp) -> Vec<String> {
    use crate::components::disk_usage;
    let mut log = Vec::new();
    let dir = folder.to_string_lossy().into_owned();
    let check = |log: &mut Vec<String>, step: &str, ok: bool| {
        log.push(format!("{} {step}", if ok { "OK  " } else { "FAIL" }));
        ok
    };

    // 1. The folder is open in the active tab.
    let target = dir.clone();
    cx.update(|cx| explorer::explorer(cx).update(cx, |e, cx| e.navigate(target, Default::default(), cx)));
    let opened = wait_for(cx, Duration::from_secs(10), |cx| {
        cx.update(|cx| {
            let e = explorer::explorer(cx).read(cx);
            e.active_tab()
                .is_some_and(|t| t.path.eq_ignore_ascii_case(&dir) && t.rows.len() >= 2)
        })
    })
    .await;
    if !check(&mut log, &format!("open the folder: {:?}", names(folder)), opened) {
        return log;
    }

    log.push(format!("     ripgrep: {:?}", glint_core::proc::locate("rg")));
    // 2. The one search: "needle" is looked for in names and inside the files, then the folder comes back.
    cx.update(|cx| {
        explorer::explorer(cx).update(cx, |e, cx| {
            e.set_filter("needle", None, cx);
            e.run_find("needle", Default::default(), cx).detach();
        })
    });
    let shown = wait_for(cx, Duration::from_secs(5), |cx| search_state(cx).0).await;
    check(&mut log, "the search's results come on screen", shown);
    let searched = wait_for(cx, Duration::from_secs(20), |cx| {
        let (_, searching, _) = search_state(cx);
        !searching
    })
    .await;
    let (_, _, matches) = search_state(cx);
    let said = cx.update(|cx| {
        let explorer = explorer::explorer(cx).read(cx);
        let find = explorer.active_tab()?.find.as_ref()?;
        Some(format!(
            "in {:?}, {} file(s) inside, error: {:?}, took {} ms",
            find.scope,
            find.inside.files(),
            find.inside.error,
            find.inside.elapsed
        ))
    });
    check(
        &mut log,
        &format!(
            "inside files: {matches} line(s) for “needle” ({})",
            said.unwrap_or_default()
        ),
        searched && matches == 1,
    );
    cx.update(|cx| explorer::explorer(cx).update(cx, |e, cx| e.clear_find(cx)));
    let back = wait_for(cx, Duration::from_secs(5), |cx| !search_state(cx).0).await;
    check(&mut log, "back to the folder", back);

    // 3. The disk usage map of the folder, then the list again. The files' keys come back with the next frame.
    cx.background_executor().timer(Duration::from_millis(500)).await;
    dispatch(cx, Box::new(actions::DiskUsage));
    let shows = wait_for(cx, Duration::from_secs(5), |cx| {
        cx.update(|cx| {
            let tab = explorer::explorer(cx).read(cx).active_tab_id();
            disk_usage::shows(cx, tab)
        })
    })
    .await;
    check(&mut log, "the tab shows its map", shows);
    let mapped = wait_for(cx, Duration::from_secs(30), |cx| {
        cx.update(|cx| {
            let tab = explorer::explorer(cx).read(cx).active_tab_id();
            disk_usage::status(cx, tab).is_some_and(|line| !line.busy)
        })
    })
    .await;
    let line = cx.update(|cx| {
        let tab = explorer::explorer(cx).read(cx).active_tab_id();
        disk_usage::status(cx, tab)
            .map(|line| line.text.to_string())
            .unwrap_or_default()
    });
    check(&mut log, &format!("disk usage map: {line}"), mapped);
    dispatch(cx, Box::new(actions::DiskUsage));
    let unmapped = wait_for(cx, Duration::from_secs(5), |cx| {
        cx.update(|cx| {
            let tab = explorer::explorer(cx).read(cx).active_tab_id();
            !disk_usage::shows(cx, tab)
        })
    })
    .await;
    check(&mut log, "back to the file list", unmapped);

    // 4. Compare on the two files, then closed.
    let (a, b) = (
        folder.join("a.txt").to_string_lossy().into_owned(),
        folder.join("b.txt").to_string_lossy().into_owned(),
    );
    in_main(cx, |window, cx| {
        crate::dialogs::compare_files::open(window, cx, a.clone(), b.clone())
    });
    let comparing = wait_for(cx, Duration::from_secs(5), |cx| {
        let mut open = false;
        in_main(cx, |window, cx| open = window.has_active_dialog(cx));
        open
    })
    .await;
    check(&mut log, "compare opens", comparing);
    in_main(cx, |window, cx| window.close_dialog(cx));

    // 5. The two compressed into a.zip, and that extracted beside it. What the explorer says meanwhile is logged.
    let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let sink = heard.clone();
    let _listening = cx.update(|cx| {
        let explorer = explorer::explorer(cx);
        cx.subscribe(&explorer, move |_, event: &explorer::ExplorerEvent, _| {
            let said = match event {
                explorer::ExplorerEvent::Archive(n) => Some(format!("archive: {n:?}")),
                explorer::ExplorerEvent::Operation(n) => Some(format!("operation: {n:?}")),
                explorer::ExplorerEvent::Notice(n) => Some(format!("notice: {n:?}")),
                explorer::ExplorerEvent::Action(n) => Some(format!("action: {n:?}")),
                _ => None,
            };
            if let Some(said) = said {
                sink.borrow_mut().push(said);
            }
        })
    });
    let paths = vec![a.clone(), b.clone()];
    cx.update(|cx| {
        explorer::explorer(cx).update(cx, |e, cx| {
            e.compress(paths, explorer::CompressFormat::Zip, None, cx).detach()
        })
    });
    let zip = folder.join("a.zip");
    let zipped = wait_for(cx, Duration::from_secs(30), |_| zip.exists()).await;
    check(&mut log, &format!("compress: {:?}", names(folder)), zipped);
    // The zip stays where it was made.
    cx.background_executor().timer(Duration::from_millis(1500)).await;
    log.push(format!("     1.5 s later: {:?}", names(folder)));
    for said in heard.borrow_mut().drain(..) {
        log.push(format!("     {said}"));
    }
    if zipped {
        let zip_path = zip.to_string_lossy().into_owned();
        cx.update(|cx| explorer::explorer(cx).update(cx, |e, cx| e.extract_here(&zip_path, cx).detach()));
        let out = folder.join("a");
        let extracted = wait_for(cx, Duration::from_secs(30), |_| {
            out.join("a.txt").exists() && out.join("b.txt").exists()
        })
        .await;
        check(
            &mut log,
            &format!("extract: {:?} in a: {:?}", names(folder), names(&out)),
            extracted,
        );
        for said in heard.borrow_mut().drain(..) {
            log.push(format!("     {said}"));
        }
    }
    log
}

/// `GLINT_SELFTEST=wave9:<folder>` (a scratch folder that is a Git repository with `a.txt` changed since its
/// commit, and a `photos` folder with files in it): folder sizes are measured and `photos` gets its size; Git's
/// status line comes and `a.txt` carries its "Changed" badge.
async fn wave9_checks(folder: &Path, cx: &mut AsyncApp) -> Vec<String> {
    use crate::components::git;
    let mut log = Vec::new();
    let dir = folder.to_string_lossy().into_owned();
    let check = |log: &mut Vec<String>, step: &str, ok: bool| {
        log.push(format!("{} {step}", if ok { "OK  " } else { "FAIL" }));
        ok
    };

    // 1. The folder is open in the active tab.
    let target = dir.clone();
    cx.update(|cx| explorer::explorer(cx).update(cx, |e, cx| e.navigate(target, Default::default(), cx)));
    let opened = wait_for(cx, Duration::from_secs(10), |cx| {
        cx.update(|cx| {
            let e = explorer::explorer(cx).read(cx);
            e.active_tab()
                .is_some_and(|t| t.path.eq_ignore_ascii_case(&dir) && t.rows.len() >= 2)
        })
    })
    .await;
    if !check(&mut log, &format!("open the folder: {:?}", names(folder)), opened) {
        return log;
    }

    // 2. Folder sizes (the command bar's Folder sizes, the folder menu's).
    cx.update(|cx| explorer::explorer(cx).update(cx, |e, cx| e.calc_sizes(cx)));
    let sized = wait_for(cx, Duration::from_secs(30), |cx| {
        cx.update(|cx| {
            let e = explorer::explorer(cx).read(cx);
            let path = e.active_tab().map(|t| t.path.clone()).unwrap_or_default();
            e.size_run(&path).is_some_and(|run| !run.running)
                && e.folder_size(&path, "photos").is_some_and(|size| size > 0)
        })
    })
    .await;
    let said = cx.update(|cx| {
        let e = explorer::explorer(cx).read(cx);
        let path = e.active_tab().map(|t| t.path.clone()).unwrap_or_default();
        let line = e
            .size_run(&path)
            .and_then(|run| run.sized())
            .map(|line| format!("{} sized {} B in {:.1} ms", line.engine, line.total, line.elapsed))
            .unwrap_or_default();
        format!("{line}; photos: {:?} B", e.folder_size(&path, "photos"))
    });
    check(&mut log, &format!("folder sizes: {said}"), sized);

    // 3. Git: the status bar's line, and a.txt's badge.
    let read = wait_for(cx, Duration::from_secs(20), |cx| {
        cx.update(|cx| {
            let tab = explorer::explorer(cx).read(cx).active_tab_id();
            git::marks(cx, tab).is_some_and(|marks| !marks.is_empty())
        })
    })
    .await;
    let (line, badge) = cx.update(|cx| {
        let e = explorer::explorer(cx).read(cx);
        let tab = e.active_tab_id();
        let line = git::status_line(cx, tab)
            .map(|l| l.said.to_string())
            .unwrap_or_default();
        let marks = git::marks(cx, tab);
        let badge = e
            .active_tab()
            .and_then(|t| t.rows.iter().find(|r| r.name == "a.txt"))
            .and_then(|entry| git::row_git(entry, marks.as_deref()).badge)
            .map(|b| b.label.to_string());
        (line, badge)
    });
    check(
        &mut log,
        &format!("git: {line}; a.txt: {badge:?}"),
        read && badge.as_deref() == Some("Changed"),
    );
    log
}


