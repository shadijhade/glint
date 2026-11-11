//! What the bar knows about the active pane, worked out from the explorer when it changes, and how far the bar
//! folds for a width.
//!
//! The TypeScript took `selected` and `comparePair` from `useActiveView` and the rest from two stores; here
//! [`Facts::read`] gathers them in one value, so a draw doesn't scan the rows and the menus are plain functions
//! of it (`menus.rs`). [`Folding`] is `useFold`'s arithmetic without the DOM: it only decides, from widths the
//! view measured.

use crate::components::menus::on_disk;
use crate::explorer::{Explorer, TabId};
use glint_model::entries::FileEntry;
use glint_model::grid::TileSize;
use glint_model::paths::{is_folder_path, same_path};
use glint_model::store::{Settings, ViewMode};

/// Two files to compare (`comparePair`): the first pane's on the left.
#[derive(Clone, Debug, PartialEq)]
pub struct Pair {
    /// The left file's path.
    pub left: String,
    /// The left file's name.
    pub left_name: String,
    /// The right file's path.
    pub right: String,
    /// The right file's name.
    pub right_name: String,
}

/// Everything the bar's buttons and menus depend on.
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    /// The active tab.
    pub tab: Option<TabId>,
    /// Its folder (`""` is This PC).
    pub path: String,
    /// It shows a search's results.
    pub find: bool,
    /// List, grid or columns.
    pub view: ViewMode,
    /// Nothing in it can be changed: an archive, a remote, the Recycle Bin, a folder Windows keeps closed.
    pub read_only: bool,
    /// Windows keeps the folder closed to this account.
    pub closed: bool,
    /// The Recycle Bin itself (not a search in it).
    pub bin: bool,
    /// A folder on a drive, a share or in an archive: `isFolderPath`.
    pub in_folder: bool,
    /// At the top of an archive on disk, which can be extracted whole: the archive's file.
    pub whole_archive: Option<String>,
    /// How many items are selected.
    pub selected: usize,
    /// The two files Ctrl+M would compare.
    pub pair: Option<Pair>,
    /// Two panes are showing.
    pub dual: bool,
    /// Differences between the panes are highlighted (only while two panes show).
    pub compare_panes: bool,
    /// The other pane shows a folder that can take items.
    pub other: bool,
    /// The tab shows its disk usage map.
    pub mapped: bool,
    /// Show hidden items.
    pub show_hidden: bool,
    /// The preview pane is on.
    pub show_preview: bool,
    /// The grid's tile size.
    pub tile_size: TileSize,
}

impl Facts {
    /// Nothing known (no tab yet).
    pub fn none() -> Facts {
        Facts {
            tab: None,
            path: String::new(),
            find: false,
            view: ViewMode::List,
            read_only: false,
            closed: false,
            bin: false,
            in_folder: false,
            whole_archive: None,
            selected: 0,
            pair: None,
            dual: false,
            compare_panes: false,
            other: false,
            mapped: false,
            show_hidden: false,
            show_preview: true,
            tile_size: TileSize::Medium,
        }
    }

    /// The facts about the explorer's active tab. `mapped` is whether the tab shows its disk usage map (the
    /// map's state lives in `disk_usage`, not the explorer).
    pub fn read(explorer: &Explorer, mapped: bool, settings: &Settings) -> Facts {
        let Some(tab) = explorer.active_tab() else {
            return Facts::none();
        };
        let archive = tab.archive();
        let bin = tab.is_recycle_bin() && tab.find.is_none();
        let closed = tab.is_closed();
        Facts {
            tab: Some(tab.id),
            path: tab.path.clone(),
            find: tab.find.is_some(),
            view: tab.view,
            read_only: archive.is_some() || closed || bin || tab.is_remote(),
            closed,
            bin,
            in_folder: is_folder_path(&tab.path),
            // At the top of an archive on disk (not one inside another): it can be extracted whole.
            whole_archive: archive
                .filter(|a| same_path(&tab.path, &a.path) && same_path(&a.path, &a.outer))
                .map(|a| a.outer.clone()),
            selected: tab.selection.len(),
            pair: compare_pair(explorer),
            dual: explorer.is_dual(),
            compare_panes: explorer.is_dual() && explorer.compare_panes,
            // An archive in the other pane is read-only, and Windows may keep its folder closed: nothing goes into
            // either.
            other: explorer.other_pane_folder().is_some(),
            mapped,
            show_hidden: settings.show_hidden,
            show_preview: settings.show_preview_pane,
            tile_size: settings.tile_size,
        }
    }

    /// The tools of a folder you may read and change are offered (the Tools menu, Run): not in a read-only
    /// place, not while searching.
    pub fn can_run_tasks(&self) -> bool {
        self.in_folder && !self.read_only && !self.find
    }
}

/// The selected items of the active tab, in the order the rows show them (`selected` in Explorer.tsx).
pub fn selected_entries(explorer: &Explorer) -> Vec<FileEntry> {
    let Some(tab) = explorer.active_tab() else {
        return Vec::new();
    };
    tab.rows.chosen(&tab.selection).into_iter().cloned().collect()
}

/// The most items selected for the bar to look for a pair among (the pair is two files, or one beside a few folders).
const MOST_TO_COMPARE: usize = 64;

/// The two files Ctrl+M compares (`comparePair`): the two selected here, the one selected here against the one
/// selected in the other pane, or its namesake in the other pane's folder. The first pane's is on the left.
// TODO(coordinator): from workspace::compare_pair, which is private there; the logic is the same.
fn compare_pair(explorer: &Explorer) -> Option<Pair> {
    let here = explorer.active_pane().min(1);
    let tab = explorer.active_tab()?;
    let files_chosen = |tab: &crate::explorer::Tab| -> Vec<FileEntry> {
        // Two files, or one: a big selection is never a pair, and the bar doesn't scan a big folder for it.
        if tab.selection.is_empty() || tab.selection.len() > MOST_TO_COMPARE {
            return Vec::new();
        }
        tab.rows
            .chosen(&tab.selection)
            .into_iter()
            .filter(|e| !e.dir && on_disk(e))
            .cloned()
            .collect()
    };
    let pair = |a: &FileEntry, b: &FileEntry| Pair {
        left: a.path.clone(),
        left_name: a.name.clone(),
        right: b.path.clone(),
        right_name: b.name.clone(),
    };
    let own = files_chosen(tab);
    if let [a, b] = own.as_slice() {
        if tab.selection.len() == 2 {
            return Some(pair(a, b));
        }
    }
    let [one] = own.as_slice() else { return None };
    if !explorer.is_dual() {
        return None;
    }
    let other = explorer.pane_tab(1 - here)?;
    let ordered = |a: &FileEntry, b: &FileEntry| if here == 0 { pair(a, b) } else { pair(b, a) };
    if let [theirs] = files_chosen(other).as_slice() {
        if !same_path(&one.path, &theirs.path) {
            return Some(ordered(one, theirs));
        }
    }
    if other.archive().is_some() || same_path(&tab.path, &other.path) {
        return None;
    }
    let namesake = other
        .entries()
        .iter()
        .find(|e| !e.dir && e.lower == one.lower && on_disk(e))?;
    Some(ordered(one, namesake))
}

// ---- Folding --------------------------------------------------------------------------------------------------

/// The most the bar folds: see [`Folding`]. Fold 6 (the most) is the last resort and is never measured.
pub const MOST: usize = 6;

/// How far the bar folds to fit (`useFold`). 0 shows every label; 1 hides the labels that go first (icons and
/// tooltips stay); 2 hides them all; 3 also puts the view toggles in a menu; 4 puts Disk usage, Folder sizes and
/// Search text in the Tools menu too; 5 puts list or grid in the view menu and the palette in Tools; 6, a window
/// half a small screen wide, puts cut, copy, paste, rename and delete in a "…" menu.
///
/// The widths are measured, so it holds at any text size: [`Folding::settle`] is told what the bar needed the
/// last time it was drawn at some fold, remembers it, and picks the first fold whose need fits the bar. A fold
/// nobody has drawn yet is taken to fit (as far as anyone knows it does), so the bar tries it and measures.
#[derive(Clone, Debug, Default)]
pub struct Folding {
    fold: usize,
    needs: [f32; MOST],
    /// What the needs were measured for; another set of buttons, text size or language measures again.
    signature: Option<u64>,
}

impl Folding {
    /// The fold now.
    pub fn fold(&self) -> usize {
        self.fold
    }

    /// What the bar needed at each fold, in pixels (0 for a fold not drawn yet).
    pub fn needs(&self) -> [f32; MOST] {
        self.needs
    }

    /// Takes a drawing's measurements and decides the fold for `width`. `signature` says which buttons, text
    /// size and language the drawing had; `measured` is the fold it was drawn at (with its signature) and the
    /// width its content needed, if a drawing was measured at all. A bar not laid out yet (`width` 0) keeps its
    /// fold.
    pub fn settle(&mut self, signature: u64, width: f32, measured: Option<(u64, usize, f32)>) -> usize {
        if self.signature != Some(signature) {
            self.signature = Some(signature);
            self.needs = [0.; MOST];
        }
        if let Some((drawn_with, fold, need)) = measured {
            if drawn_with == signature && fold < MOST && need > 0. {
                self.needs[fold] = need;
            }
        }
        if width > 0. {
            // Half a pixel of slack, so a width that is exactly the need fits.
            self.fold = self.needs.iter().position(|&need| width + 0.5 >= need).unwrap_or(MOST);
        }
        self.fold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_that_fits_keeps_every_label() {
        let mut folding = Folding::default();
        assert_eq!(folding.settle(1, 1000., Some((1, 0, 800.))), 0);
        assert_eq!(folding.needs()[0], 800.);
    }

    #[test]
    fn a_narrower_bar_tries_the_next_fold_and_measures_it() {
        let mut folding = Folding::default();
        folding.settle(1, 600., Some((1, 0, 800.)));
        // Fold 1 was never drawn: it is taken to fit until it is drawn and measured.
        assert_eq!(folding.fold(), 1);
        // Drawn at fold 1 it needed 650: still too wide, so fold 2 is tried.
        assert_eq!(folding.settle(1, 600., Some((1, 1, 650.))), 2);
        assert_eq!(folding.settle(1, 600., Some((1, 2, 590.))), 2);
        // The window grows again: the first fold whose need is known to fit comes back.
        assert_eq!(folding.settle(1, 700., None), 1);
        assert_eq!(folding.settle(1, 900., None), 0);
    }

    #[test]
    fn nothing_fits_at_the_narrowest_and_the_last_resort_is_the_most() {
        let mut folding = Folding::default();
        let mut fold = 0;
        for _ in 0..MOST {
            fold = folding.settle(1, 100., Some((1, fold, 500.)));
        }
        assert_eq!(fold, MOST);
        // The last resort is never measured, and still holds when asked again.
        assert_eq!(folding.settle(1, 100., Some((1, MOST, 500.))), MOST);
    }

    #[test]
    fn other_buttons_or_text_size_measure_again() {
        let mut folding = Folding::default();
        folding.settle(1, 600., Some((1, 0, 800.)));
        assert_eq!(folding.fold(), 1);
        // Dual pane turned on: the old needs are forgotten, and a drawing from before the change is ignored.
        assert_eq!(folding.settle(2, 1000., Some((1, 0, 800.))), 0);
        assert_eq!(folding.needs(), [0.; MOST]);
        assert_eq!(folding.settle(2, 1000., Some((2, 0, 900.))), 0);
        assert_eq!(folding.needs()[0], 900.);
    }

    #[test]
    fn a_bar_not_laid_out_yet_keeps_its_fold() {
        let mut folding = Folding::default();
        folding.settle(1, 600., Some((1, 0, 800.)));
        assert_eq!(folding.settle(1, 0., None), 1);
    }
}
