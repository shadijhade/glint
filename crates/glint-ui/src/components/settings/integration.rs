//! Settings → Files → File Explorer: "Open in Glint" on its right-click menu, opening folders here when they are
//! double-clicked, and Win+E (`FileExplorerRows` of SettingsView.tsx). Core does the registry work (verbs.rs).

use super::backend::Backend;
use super::widgets::{self, bad};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_core::verbs::Integration;
use std::sync::Arc;

/// The three switches, divided like rows of the card they sit in.
pub struct FileExplorerRows {
    backend: Arc<dyn Backend>,
    state: Option<Integration>,
    working: bool,
}

impl FileExplorerRows {
    /// Reads how File Explorer is set up.
    pub fn new(backend: Arc<dyn Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let asked = backend.explorer_integration();
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update(cx, |this, cx| {
                if let Ok(state) = answer {
                    this.state = Some(state);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        Self {
            backend,
            state: None,
            working: false,
        }
    }

    /// (available, menu, by default, Win+E), as last read.
    pub fn flags(&self) -> (bool, bool, bool, bool) {
        self.state.as_ref().map_or((false, false, false, false), |s| {
            (s.available, s.menu, s.by_default, s.win_e)
        })
    }

    /// Changes the three together (`change`).
    pub fn change(&mut self, menu: bool, by_default: bool, win_e: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.working = true;
        cx.notify();
        let asked = self.backend.set_explorer_integration(menu, by_default, Some(win_e));
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| {
                this.working = false;
                match answer {
                    Ok(state) => this.state = Some(state),
                    Err(reason) => bad(window, cx, t!("Couldn't change File Explorer's settings"), reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for FileExplorerRows {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let (available, menu, by_default, win_e) = self.flags();
        let store = self.state.as_ref().is_some_and(|s| s.store);
        let disabled = !available || self.working;
        let unavailable = if store {
            t!("Windows doesn't let apps from the Microsoft Store change File Explorer's settings. Glint's setup program, from glint.rafd.tech, can")
        } else {
            t!("Needs Glint installed with its setup program or Scoop")
        };
        let hint = |text: SharedString| Some(if available { text } else { unavailable.clone() });
        let rows = [
            widgets::switch_row(
                "file-explorer-menu",
                t!("Open in Glint from File Explorer"),
                hint(t!("On the right-click menu of folders, drives and empty space in a folder (Show more options on Windows 11)")),
                menu,
                disabled,
                widgets::toggler(cx, move |this, on, window, cx| {
                    this.change(on, on && by_default, on && win_e, window, cx)
                }),
                &t,
            ),
            widgets::switch_row(
                "file-explorer-default",
                t!("Open folders in Glint by default"),
                hint(t!("Double-clicking a folder in File Explorer or on the desktop opens it here. Drives, This PC, the Recycle Bin and libraries stay in File Explorer.")),
                by_default,
                disabled,
                widgets::toggler(cx, move |this, on, window, cx| {
                    this.change(menu || on, on, win_e, window, cx)
                }),
                &t,
            ),
            widgets::switch_row(
                "file-explorer-win-e",
                t!("Win+E opens Glint"),
                hint(t!("Win+E opens This PC here instead of File Explorer. Turning it off, or uninstalling Glint, gives Win+E back to File Explorer.")),
                win_e,
                disabled,
                widgets::toggler(cx, move |this, on, window, cx| {
                    this.change(menu || on, by_default, on, window, cx)
                }),
                &t,
            ),
        ];
        col().children(rows.into_iter().enumerate().map(|(i, row)| {
            div()
                .when(i > 0, |this| this.border_t_1().border_color(t.border))
                .child(row)
        }))
    }
}
