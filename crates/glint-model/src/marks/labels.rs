//! The labels: named colors, one of which an item may carry (F1, 5.5), and what a row draws for an item's
//! marks, old color tags included (5.6).

use super::Marks;
use serde::{Deserialize, Serialize};

/// A label as Settings → Files → Labels has it. Files carry its name; renaming it keeps the old names in `was`,
/// so a file that still says one of them is drawn as this label and found by its new name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
    /// `0xRRGGBB`, the same in both themes.
    pub color: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub was: Vec<String>,
}

/// The labels Glint starts with: 0.16's color tags (glint-ui's `COLOR_TAGS`), which are also XYplorer's seven.
pub fn default_labels() -> Vec<Label> {
    [
        ("Red", 0xef4444),
        ("Orange", 0xf97316),
        ("Yellow", 0xeab308),
        ("Green", 0x22c55e),
        ("Blue", 0x3b82f6),
        ("Purple", 0xa855f7),
        ("Gray", 0x9ca3af),
    ]
    .into_iter()
    .map(|(name, color)| Label {
        name: name.to_string(),
        color,
        was: Vec::new(),
    })
    .collect()
}

/// The label a file's `name` means, with its place in `labels`: by its name or one it `was`, in any case.
pub fn label_of<'a>(labels: &'a [Label], name: &str) -> Option<(usize, &'a Label)> {
    // S0: card A1 fills this in.
    let _ = (labels, name);
    None
}

/// What a row draws for an item's marks (5.5, 5.6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// The label's name to draw: the label's name now when this PC has it (by an old name too), else the file's.
    pub label: Option<String>,
    /// Its place in the labels, when this PC has it; without one its color comes from its name (`tag_color`).
    pub place: Option<usize>,
    /// The tags to draw: the item's, less the one drawn as its label (a 0.16 color tag).
    pub tags: Vec<String>,
}

/// What a row draws: the label, or for a file without one the first of its tags that names a label (a 0.16
/// color tag, then not drawn again among the tags), and the tags.
pub fn shown(labels: &[Label], marks: &Marks) -> Shown {
    // S0: card A1 fills this in.
    let _ = (labels, marks);
    Shown::default()
}
