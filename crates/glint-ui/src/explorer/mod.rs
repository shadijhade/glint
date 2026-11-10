//! The explorer's state: panes, tabs, history, listings, selection (port of src/lib/explorer.ts, part A).
//!
//! # What this is
//!
//! One [`Explorer`] entity holds everything about *what Glint is showing*: one or two [`Pane`]s of tabs,
//! the active pane, the drives, the known folders and the tabs that were closed. Each [`Tab`] holds its path,
//! back/forward history, listing (shared, never copied), load state, view mode, sort, group, selection,
//! scroll memory and the rows the list draws. Every central view (file list, grid, tab strip, address bar,
//! sidebar, status bar) builds on it; none of them keeps a copy of its own.
//!
//! The zustand store of the TypeScript (`useExplorer`) became this entity; its functions became methods.
//! The mapping is in each method's doc (the TypeScript name in parentheses).
//!
//! # Using it from a view
//!
//! ```ignore
//! let explorer = glint_ui::explorer::explorer(cx);          // Entity<Explorer>, from the global
//! // Redraw whenever anything changes, or only for the kinds of change you care about:
//! cx.observe(&explorer, |this, _, cx| cx.notify()).detach();
//! cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| match event { ... }).detach();
//!
//! // Read (every getter is `&self`):
//! let tab = explorer.read(cx).active_tab();                  // Option<&Tab>
//! let rows = &tab.rows;                                      // sorted, filtered, grouped; cheap to clone
//!
//! // Change:
//! explorer.update(cx, |explorer, cx| explorer.go_back(cx).detach());
//! explorer.update(cx, |explorer, cx| explorer.select_only(Some(&path), cx));
//! ```
//!
//! **Tasks.** Anything that reads the disk returns a `gpui::Task` (the TypeScript's promise) that resolves
//! `true` when it worked. `.detach()` it to let it run, or `.await` it. A dropped task is cancelled.
//!
//! **Events.** [`ExplorerEvent`] says what changed (`ListingChanged`, `SelectionChanged`, `TabsChanged`…).
//! It also carries what the TypeScript showed as toasts: [`Notice`]s, which the workspace turns into
//! notifications. The model itself never touches the interface.
//!
//! # How listings work
//!
//! [`Explorer::navigate_task`] asks the [`Source`] for the folder. In the app that is [`CoreSource`]: glint-core
//! lists and sorts the folder on its runtime and the binary buffer is decoded on a blocking thread, so the UI
//! thread never waits. The tab keeps its old folder on screen, flagged [`Tab::loading`], until the new one
//! arrives. Each tab numbers its navigations; a result that arrives for an older number is dropped. The
//! listing arrives presorted (folders first, natural order); [`Tab::rows`] is that, narrowed by the hidden
//! items setting and the filter and re-sorted for any other sort ([`rows`]). A listing of more than 20,000
//! entries that needs real sorting is sorted on a background thread.
//!
//! # Where the rest goes
//!
//! This is part A. Later parts add `impl Explorer { … }` blocks in files of their own (file operations,
//! undo, clipboard, rename, new items; find; sizes; the index; compare; archive actions) and own the
//! fields the model reserves for them ([`Tab::find`], [`Tab::renaming`], `compare_panes`, `diff_only`,
//! `quick_look`, `editing_address`). Add a file next to these, `mod` it here, and use the helpers marked
//! `pub(super)`: `update_tab`, `tab_changed`, `changed`, `rebuild_rows`.
//!
//! # Files
//!
//! | File | Holds |
//! |---|---|
//! | `types.rs` | ids, [`LoadState`], [`Drive`], [`ExplorerEvent`], [`Notice`], [`NavOptions`] |
//! | `source.rs` | [`Source`] (what the model reads from) and [`CoreSource`] (glint-core) |
//! | `tab.rs` | [`Tab`] and [`Selection`] |
//! | `rows.rs` | [`Rows`], [`RowSpec`] and [`build_rows`]: sorting, hidden items, filter, groups |
//! | `memory.rs` | [`Memory`]: last folder, session, visits, folders' own views |
//! | `model.rs` | the [`Explorer`] struct, getters, settings and rows plumbing |
//! | `nav.rs` | navigate, history, reload, prefetch, drives |
//! | `tabs.rs` | tabs and panes |
//! | `select.rs` | selection |
//! | `open.rs` | opening files and entries, go to, Get access |
//! | `start.rs` | start-up, session, watches, hints |
//! | `requested.rs` | folders a later launch hands over |
//! | `ops_types.rs` | file operations' types: [`FileOp`], [`OpNotice`], [`Ask`], [`UndoId`]… |
//! | `ops.rs` | copy, move, delete, drops, the busy toast (`fileOp`, `deletePaths`, `dropFiles`…) |
//! | `ops_state.rs` | the file operations' state in the [`Explorer`] |
//! | `undo.rs` | Ctrl+Z (`rememberUndo`, `undo`) |
//! | `clipboard.rs` | cut, copy and paste |
//! | `queue.rs` | the copy queue (`queueFiles`, a finished job) |
//! | `recycle.rs` | the Recycle Bin: restore, delete for good, empty |
//! | `rename.rs` | rename (F2, `renameEntry`) |
//! | `create.rs` | New folder, New file and the New menu's kinds |
//! | `view.rs` | focus, marquee selection, columns, the address bar's editing flag, eject |

mod clipboard;
mod create;
mod memory;
mod model;
mod nav;
mod open;
mod ops;
mod ops_state;
mod ops_types;
mod queue;
mod recycle;
mod rename;
mod requested;
mod rows;
mod select;
mod source;
mod start;
mod tab;
mod tabs;
mod types;
mod undo;
mod view;

/// A fake disk for tests: views' tests use it too (`crate::explorer::fake::{FakeDisk, file, folder}`).
#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod ops_tests;
#[cfg(test)]
mod tests;

pub use memory::Memory;
pub use model::Explorer;
pub use nav::Refresh;
pub use open::is_disk_image;
pub use ops_types::*;
pub use recycle::purge_what;
pub use rename::{clean_name, INVALID_NAME_CHARS, INVALID_NAME_HINT};
pub use rows::{build_rows, GroupLayout, RowSpec, Rows};
pub use source::{Access, CoreSource, Effect, Fut, Resolved, Source};
pub use tab::{PathSet, Selection, Tab};
pub use types::*;

use crate::core_bridge::Core;
use gpui_kit::{App, AppContext as _, Entity, Global};
use std::rc::Rc;

/// The one explorer, in a GPUI `Global`: `explorer(cx)` gets the entity.
pub struct ExplorerHandle(pub Entity<Explorer>);

impl Global for ExplorerHandle {}

/// Makes the explorer, reading from glint-core, and puts it in the global. Call it in `run` after
/// `core_bridge::init` (it needs core's app handle) and `state::init` (it follows the settings). It opens
/// nothing yet: when the saved state is loaded, give it to the explorer with [`Explorer::set_memory`], then
/// call [`Explorer::init_explorer`].
pub fn init(cx: &mut App) {
    let source: Rc<dyn Source> = Rc::new(CoreSource::new(cx.global::<Core>().app.clone()));
    init_with(cx, source);
}

/// [`init`] with a source of your own (tests, tools).
pub fn init_with(cx: &mut App, source: Rc<dyn Source>) {
    let explorer = cx.new(|cx| Explorer::new(source, cx));
    cx.set_global(ExplorerHandle(explorer));
}

/// The explorer. Panics if [`init`] hasn't run (a start-up order mistake, never a runtime condition).
pub fn explorer(cx: &App) -> Entity<Explorer> {
    cx.global::<ExplorerHandle>().0.clone()
}

/// The explorer, if [`init`] has run.
pub fn try_explorer(cx: &App) -> Option<Entity<Explorer>> {
    cx.try_global::<ExplorerHandle>().map(|h| h.0.clone())
}
