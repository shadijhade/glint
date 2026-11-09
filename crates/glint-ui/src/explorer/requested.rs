//! Folders a later launch hands over while Glint runs (`openRequested`): File Explorer's "Open in Glint", a
//! double-click on a folder, the command line.

use super::model::Explorer;
use super::types::LaunchTarget;
use gpui_kit::Context;

impl Explorer {
    /// Opens each target in a tab of its own at the end of the active pane, selecting the file a target names,
    /// and shows the last (as [`Explorer::new_tab`] shows its tab). False when there was nothing to open.
    ///
    /// Call it once start-up has opened the first folders (the launch hub starts delivering after
    /// [`Explorer::init_explorer`]'s task): tabs placed earlier would be replaced by the restored session.
    /// The TypeScript also switched the app back to its files mode and closed the palette; those belong to
    /// the views that own them.
    pub fn open_requested(&mut self, targets: Vec<LaunchTarget>, cx: &mut Context<Self>) -> bool {
        let Some(last) = self.place_outside_tabs(&targets, false) else {
            return false;
        };
        self.activate_tab(last, cx);
        true
    }
}
