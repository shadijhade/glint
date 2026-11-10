//! JSON, YAML and TOML as a tree; CSV and TSV as a table.
//!
//! Wave 7 stub: the preview-text agent owns this file. The frame builds it with `DataPreview::new(path, window, cx)`;
//! keep that constructor (add setters for anything more it needs).

use crate::kit::prelude::*;

/// JSON, YAML and TOML as a tree; CSV and TSV as a table.
pub struct DataPreview {
    path: String,
}

impl DataPreview {
    /// The previewer for the file at `path`. Reading happens in the background; the view shows its loading state
    /// until then.
    pub fn new(path: String, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { path }
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Render for DataPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}
