//! Git: the branch chip, the repository and README cards, the Changes view, the marks in rows (GitChip.tsx,
//! RepoCard.tsx, ReadmeCard.tsx, ChangesView.tsx, and the `useGit` store of src/lib/git.ts).
//!
//! | TypeScript                                   | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `useGit`, `refreshGit`, `trustRepository`    | [`model::Git`], one model entity in a global ([`shared`])      |
//! | `gitMarks` in Explorer.tsx                   | [`marks`]                                                      |
//! | `useGitMark`                                 | [`mark_of`]                                                    |
//! | `GIT_BADGE`, `gitOf` (FileList.tsx)          | [`words::badge`], [`words::row_git`], [`row::badge_element`]    |
//! | `GitWork` (StatusBar.tsx)                    | [`status_line`], [`stage`]                                     |
//! | `GitChip`                                    | [`chip::GitChip`]                                              |
//! | `RepoCard`                                   | [`cards::RepoCard`]                                            |
//! | `ReadmeCard`, `readmeOf`                     | [`cards::ReadmeCard`], [`readme_of`]                           |
//! | `ChangesView`                                | [`changes::ChangesView`]                                       |
//!
//! # How the coordinator wires it
//!
//! **The model.** Call [`init`] once at start-up (after `core_bridge::init` and `explorer::init`; [`shared`] makes it
//! on first use otherwise). It follows the explorer by itself: a tab whose folder is listed gets its Git view in
//! the background (status of a big repository can take seconds; 4 s at most, then what it found), a tab that
//! shows a search, an archive, a folder read from the index, This PC, the Recycle Bin or a remote has none, and a
//! closed tab's goes. F5 (`Explorer::refresh`) asks for status even in a repository too big to look through on every
//! visit (the model reads `Explorer::refreshed_by_hand` on the listing, as `refreshedByHand` was read). Core's
//! "git-changed" event (a save, `git add`, a checkout in a repository on screen) makes its tabs look again, and the
//! repositories of the tabs on screen are the ones watched. Observe the model (`cx.observe(&git::shared(cx), …)`)
//! to redraw, or subscribe to [`model::GitEvent::Changed`]`(tab)` to hear which tab's Git moved.
//!
//! **What a row draws.** [`marks`]`(cx, tab)` is the tab's marks (lowercase name to
//! [`GitMark`](glint_model::entries::GitMark)), `None` outside a repository. For each row:
//! - [`words::row_git`]`(entry, marks.as_deref())` says what to draw: a `badge` (or none) and `ignored`.
//! - A badge is a small pill after the name (and after the compare badge): conflicted is a red "Conflict", modified
//!   an amber "Changed", staged an emerald "Staged", untracked a sky "New"; its hover text says it of the item
//!   (a folder's says it of what is in it). [`row::badge_element`]`(id, &entry.name, &badge, cx)` draws it, named
//!   "Changed: report.pdf" so each row's is told apart.
//! - An `ignored` item (`.gitignore` leaves it out) draws no pill; the row is dimmed to 55% (as hidden and cut
//!   items are) and the name's tooltip is [`words::name_tooltip`]`(name, ignored)`, which adds "Git: left out by
//!   .gitignore".
//! - The file list's `PORT ME(git)` in `cells.rs` is both of these; the grid draws them on its tiles too (file_grid/tile.rs), as FileGrid.tsx did.
//!
//! **The `git:` filters and Group by Git.** They need the marks, and the explorer builds the rows without them
//! (`RowSpec` has no marks). Pass [`marks`] as the `git` argument of `glint_model::entries::matcher` (the filter box:
//! `git:changed`, `git:new`, `git:staged`, `git:modified`, `git:conflicted`, `git:ignored`) and of
//! `glint_model::groups::group_rows` (Group by "Git status"). `PORT ME(coordinator)`: that means `RowSpec` gets a
//! `git: Option<Arc<GitMarks>>` and `Tab::rows` are rebuilt when [`model::GitEvent::Changed`] arrives for a tab whose
//! filter holds `git:` or whose group is Git. Without a repository `git:…` matches nothing, as in the TypeScript.
//!
//! **The status bar.** [`status_line`]`(cx, tab)` is the work button's Git words ("git · 12 ms", "git · waits for
//! trust", "git · F5 for status") and its hover text; the bar draws it after "· ", from the `xl` width up, beside
//! the "read in … · shown in …" line. [`stage`]`(cx, tab)` is the "Git status" row of the popover's "How long it
//! took" (label, branch or "repo", milliseconds), after "Folder sizes".
//!
//! **The branch chip.** One [`chip::GitChip::new`]`(tab, cx)` per tab, made once, goes in the address bar after the
//! breadcrumbs (where `address_bar/mod.rs` has `PORT ME(git)`), `ms-1`, hidden below 26 rem of bar (the
//! TypeScript's `@max-[26rem]:hidden`). It draws nothing while [`chip::GitChip::is_shown`] is false, so leave no gap
//! for it then. [`in_repository`]`(cx, tab)` is what `AddressBar::set_in_repository` takes (the `git:` hint). The
//! popover hangs under the chip from its start edge; it closes on Escape, a press outside, or any button in it. Its
//! buttons act through the explorer (`set_filter`, `navigate` in the chip's tab) and the model (trust, status).
//!
//! **The cards.** In the preview pane:
//! - [`cards::RepoCard::new`]`(folder, tools, cx)` for a folder with a `.git` (the pane's `counts.repo`) that isn't
//!   in the Recycle Bin, under the folder's counts; `set_path` follows the selection. `tools` is the shared tools
//!   model (`dialogs::action_run::tools_model`): the card offers to install onefetch and tokei when they are missing
//!   and reads again once they are there.
//! - [`cards::ReadmeCard::new`]`(entry, window, cx)` above the file list, for [`readme_of`]`(listing entries)` when
//!   `settings.show_readme`, not over search results, the disk map, a closed folder, or a folder read from the
//!   index; keyed by the README's path (`set_entry` rereads when it changed). It draws nothing for a README that is
//!   binary, empty or unreadable. Folded it is one line (the title); open (`settings.readme_open`, kept from folder to
//!   folder) the Markdown shows under it through `components::preview::markdown::MarkdownPreview`.
//! - The **Changes** view of a file in the preview pane: show a "Changes" tab ("What changed since the last commit
//!   (Git)") when [`has_changes_view`]`(cx, &entry)`, and [`changes::ChangesView::new`]`(entry, cx)` as its body;
//!   `set_entry` follows the selection. Its "Side by side" button opens
//!   `dialogs::compare_files::open_sides` with a read-only "In the last commit" side and "Now", and removes the temp
//!   copy when the comparison closes.
//!
//! # Differences from the TypeScript
//!
//! - **Trusting** a repository asks for its status with `force`: the TypeScript asked again plainly and core hands
//!   back the view it worked out before (still "limited") while nothing in the repository has changed.
//! - **The README** opens at a share of the window's height (40%, at least 10 rem) and scrolls inside, where the
//!   TypeScript's was as tall as its text up to half of the files' height: a virtualized Markdown view cannot size
//!   itself to its content.
//! - **A line of the Changes view** is one row: one longer than 1,000 characters is cut with an ellipsis, where the
//!   TypeScript wrapped it. The view mirrors in the Arabic interface as the TypeScript's did (numbers at the end
//!   edge, the sign, then the text).
//! - **The bar's segments** in the chip's popover are mouse-only (the pills under them do the same from the
//!   keyboard, and have names).
//! - **The README's Markdown** is the preview's native one: raw HTML in it, Mermaid and math show plainer (see
//!   `preview/markdown.rs`).
//!
//! # Not ported
//!
//! - Nothing from the four components is left out. Not exercised by a test: the install cards of onefetch and tokei
//!   (they read the settings Tools model, which cannot be faked from outside its module), and the real glint-core
//!   calls (`CoreGit`), which need a repository on disk.

pub mod cards;
pub mod changes;
pub mod chip;
pub mod model;
pub mod row;
pub mod words;

#[cfg(test)]
mod tests;

use crate::explorer::TabId;
use gpui_kit::App;

pub use cards::{ReadmeCard, RepoCard};
pub use changes::ChangesView;
pub use chip::GitChip;
pub use model::{
    has_changes_view, in_repository, init, init_with, mark_of, marks, refresh, shared, trust_repository, try_shared,
    view_of, Counts, Git, GitEvent, Run, Source, View,
};
pub use words::{badge, name_tooltip, readme_of, row_git, Badge, RowGit, StatusLine, Tone};

/// The status bar's Git words for the tab's folder (`GitWork`), or `None` when the tab has no Git view.
pub fn status_line(cx: &App, tab: TabId) -> Option<StatusLine> {
    view_of(cx, tab).map(|view| words::status_line(&view))
}

/// The "Git status" stage of the status bar popover's timing, as (label, detail, milliseconds); `None` when the tab
/// has no view, or status waits for trust or F5.
pub fn stage(cx: &App, tab: TabId) -> Option<(gpui_kit::SharedString, gpui_kit::SharedString, f64)> {
    view_of(cx, tab).and_then(|view| words::stage(&view))
}
