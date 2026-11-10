//! The bars over a pane's list that say what it shows (wave 15, pane-bars): the search bar while a search's results
//! are shown, and the bar for a folder listed from instant search's index because Windows keeps it closed.
//! Drawn by the workspace above the compare bar; each draws nothing when it has nothing to say.

use crate::kit::prelude::*;

/// The bars over pane `pane`.
pub struct PaneBars {
    #[allow(dead_code)]
    pane: usize,
}

impl PaneBars {
    pub fn new(pane: usize, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        PaneBars { pane }
    }
}

impl Render for PaneBars {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
