//! The right-click menu of a place or a folder in the tree.
//! Ported from `folderMenu` in src/components/explorer/Explorer.tsx, for a folder that is not the one shown
//! (`current` false), which is the only way the sidebar calls it.
//!
//! This is the part of that menu that belongs to the places themselves: open, open in a new tab, copy the
//! path, pin and unpin, and the connections' own items. What other parts do to a folder (paste, sizes, the
//! inspectors, clean up, share, Open in, Scripts, Properties) comes with those parts and is marked below.

use super::rows::Row;
use super::{Sidebar, SidebarEvent};
use crate::explorer::{explorer, NavOptions};
use crate::kit::prelude::*;
use crate::{persistence, t};
use glint_model::paths::{in_archive_path, is_recycle_bin, is_remote_path};

/// Builds the menu for `row` (a place or a tree folder) into `menu`.
pub(super) fn build(menu: PopupMenu, row: &Row, pinned: bool, sidebar: &WeakEntity<Sidebar>, cx: &App) -> PopupMenu {
    let (path, remote) = match row {
        Row::Place(p) => (p.path.clone(), p.remote.clone()),
        Row::Folder(f) => (f.path.clone(), None),
        _ => return menu,
    };
    let mut menu = menu;

    let open = |path: String| {
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            explorer(cx).update(cx, |e, cx| e.navigate(path.clone(), NavOptions::default(), cx));
        }
    };
    let open_in_new_tab = |path: String| {
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            explorer(cx).update(cx, |e, cx| e.open_in_new_tab(&path, cx));
        }
    };
    let copy_path = |path: String| {
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
            window.push_notification(Notification::success(t!("Copied path")), cx);
        }
    };
    let toggle_pin = |path: String| {
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            persistence::update_app(cx, |app| app.toggle_pin(&path));
        }
    };
    let pin_item = |path: String| {
        PopupMenuItem::new(if pinned {
            t!("Unpin from sidebar")
        } else {
            t!("Pin to sidebar")
        })
        .icon(if pinned { IconName::PinOff } else { IconName::Pin })
        .checked(false)
        .on_click(toggle_pin(path))
    };
    let _ = cx;

    // A remote's folder: read-only; its connection is changed from its top.
    if is_remote_path(&path) {
        menu = menu
            .item(
                PopupMenuItem::new(t!("Open"))
                    .icon(IconName::FolderOpen)
                    .on_click(open(path.clone())),
            )
            .item(
                PopupMenuItem::new(t!("Open in new tab"))
                    .icon(IconName::SquarePlus)
                    .on_click(open_in_new_tab(path.clone())),
            )
            .separator()
            .item(
                PopupMenuItem::new(t!("Copy path"))
                    .icon(IconName::ClipboardCopy)
                    .on_click(copy_path(path.clone())),
            );
        if remote.is_none() {
            menu = menu.item(pin_item(path.clone()));
        }
        if let Some(id) = remote {
            // PORT ME(remotes): "Show in File Explorer, on demand" / "Stop showing in File Explorer" belong to
            // the on-demand files part; the editor and the removal are the remote dialogs'.
            let edit = {
                let (sidebar, id) = (sidebar.clone(), id.clone());
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    let _ = sidebar.update(cx, |_, cx| cx.emit(SidebarEvent::EditRemote(id.clone())));
                }
            };
            let remove = {
                let sidebar = sidebar.clone();
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    let _ = sidebar.update(cx, |_, cx| cx.emit(SidebarEvent::RemoveRemote(id.clone())));
                }
            };
            menu = menu.separator();
            // A paired PC's connection is changed in Settings → Devices, not here.
            if !matches!(row, Row::Place(p) if matches!(p.glyph, super::rows::Glyph::Remote(glint_model::store::RemoteKind::Device, _)))
            {
                menu = menu.item(
                    PopupMenuItem::new(t!("Edit the connection…"))
                        .icon(IconName::PlugZap)
                        .on_click(edit),
                );
            }
            menu = menu.item(
                PopupMenuItem::new(t!("Remove the connection…"))
                    .icon(IconName::Trash)
                    .on_click(remove),
            );
        }
        return menu.separator().item(
            PopupMenuItem::new(t!("Read-only: on a remote connection"))
                .icon(IconName::PencilOff)
                .disabled(true),
        );
    }

    // The Recycle Bin: its items go back, or for good.
    if is_recycle_bin(&path) {
        // PORT ME(file operations): "Empty the Recycle Bin" (danger) is the file operations part's.
        return menu
            .item(
                PopupMenuItem::new(t!("Open"))
                    .icon(IconName::FolderOpen)
                    .on_click(open(path.clone())),
            )
            .item(
                PopupMenuItem::new(t!("Open in new tab"))
                    .icon(IconName::SquarePlus)
                    .on_click(open_in_new_tab(path)),
            );
    }

    // PORT ME(file operations): the TypeScript's first row also has "Paste into folder", and "Paste with the
    // copy queue" follows "Open in new tab"; both are the file operations part's.
    menu = menu
        .item(
            PopupMenuItem::new(t!("Copy path"))
                .icon(IconName::ClipboardCopy)
                .on_click(copy_path(path.clone())),
        )
        .item(pin_item(path.clone()))
        .separator()
        .item(
            PopupMenuItem::new(t!("Open"))
                .icon(IconName::FolderOpen)
                .on_click(open(path.clone())),
        )
        .item(
            PopupMenuItem::new(t!("Open in new tab"))
                .icon(IconName::SquarePlus)
                .on_click(open_in_new_tab(path.clone())),
        );
    if !in_archive_path(&path) {
        let sidebar = sidebar.clone();
        let folder = path.clone();
        menu = menu.separator().item(
            PopupMenuItem::new(t!("Search text"))
                .icon(IconName::TextSearch)
                .on_click(move |_, _, cx| {
                    let _ = sidebar.update(cx, |_, cx| cx.emit(SidebarEvent::SearchText(folder.clone())));
                }),
        );
    }
    // PORT ME(folder menu): "Inspect & check", "Clean up & share", "Open in File Explorer", "Open in
    // Terminal", "Open in", "Scripts" and "Properties" are other parts' (the same menu serves the folder shown
    // and This PC's tiles, so the coordinator owns one shared `folder_menu` once those parts exist).
    menu
}
