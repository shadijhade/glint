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
//!
//! The waves' own scripts went once the e2e harness (tests\e2e) checked what they did.

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
    if name != "ops" {
        eprintln!("glint: GLINT_SELFTEST: unknown script {script:?}");
        return;
    }
    cx.spawn(async move |cx| {
        let log = ops(&folder, cx).await;
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
