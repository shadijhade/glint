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
    let want = fold(name);
    if want.is_empty() {
        return None;
    }
    labels
        .iter()
        .enumerate()
        .find(|(_, l)| fold(&l.name) == want)
        .or_else(|| labels.iter().enumerate().find(|(_, l)| l.was.iter().any(|w| fold(w) == want)))
}

/// A name as compared: trimmed, lower case, and Grey is Gray (XYplorer's seventh label is called Grey).
fn fold(name: &str) -> String {
    let n = name.trim().to_lowercase();
    if n == "grey" {
        "gray".to_string()
    } else {
        n
    }
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
    if let Some(l) = &marks.label {
        let found = label_of(labels, l);
        return Shown {
            label: Some(found.map_or_else(|| l.clone(), |(_, x)| x.name.clone())),
            place: found.map(|(i, _)| i),
            tags: marks.tags.clone(),
        };
    }
    for (n, tag) in marks.tags.iter().enumerate() {
        if let Some((i, label)) = label_of(labels, tag) {
            let mut tags = marks.tags.clone();
            tags.remove(n);
            return Shown { label: Some(label.name.clone()), place: Some(i), tags };
        }
    }
    Shown { label: None, place: None, tags: marks.tags.clone() }
}

/// What a file's marks are written as the next time they are written (5.5, 5.6): tidy, a 0.16 color tag made
/// the label, and an old name of a label made its new name. A label this PC lacks stays as the file says it.
pub fn settled(labels: &[Label], marks: &Marks) -> Marks {
    let mut m = super::tidy(marks);
    match m.label.clone() {
        None => {
            let s = shown(labels, &m);
            if s.place.is_some() {
                m.label = s.label;
                m.tags = s.tags;
            }
        }
        Some(l) => {
            if let Some((_, label)) = label_of(labels, &l) {
                if fold(&label.name) != fold(&l) {
                    m.label = Some(label.name.clone());
                }
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(label: Option<&str>, tags: &[&str]) -> Marks {
        Marks { label: label.map(String::from), tags: tags.iter().map(|t| t.to_string()).collect(), comment: None }
    }

    fn renamed() -> Vec<Label> {
        let mut l = default_labels();
        l[0].name = "Important".into();
        l[0].was = vec!["Red".into()];
        l
    }

    fn place(labels: &[Label], name: &str) -> Option<usize> {
        label_of(labels, name).map(|(i, _)| i)
    }

    #[test]
    fn default_labels_are_the_seven_red_to_gray_with_no_old_names() {
        let l = default_labels();
        let names: Vec<&str> = l.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Gray"]);
        assert!(l.iter().all(|l| l.was.is_empty()));
    }

    #[test]
    fn a_label_is_found_by_its_name_in_any_case_and_padding() {
        let l = default_labels();
        assert_eq!((place(&l, "red"), place(&l, " RED "), place(&l, "Red")), (Some(0), Some(0), Some(0)));
    }

    #[test]
    fn grey_reads_as_gray() {
        let l = default_labels();
        assert_eq!((place(&l, "grey"), place(&l, "Gray")), (Some(6), Some(6)));
    }

    #[test]
    fn an_unknown_or_empty_name_is_no_label() {
        let l = default_labels();
        assert_eq!((place(&l, "Mauve"), place(&l, ""), place(&l, "  ")), (None, None, None));
    }

    #[test]
    fn a_renamed_label_is_found_by_both_names_and_a_new_one_wins_the_old_name() {
        let mut l = renamed();
        assert_eq!((place(&l, "red"), place(&l, "important")), (Some(0), Some(0)));
        l.push(Label { name: "Red".into(), color: 1, was: vec![] });
        assert_eq!(place(&l, "red"), Some(7));
    }

    #[test]
    fn a_label_this_pc_has_is_drawn_by_its_name_now_with_its_place() {
        let s = shown(&renamed(), &m(Some("red"), &["x"]));
        assert_eq!((s.label.as_deref(), s.place, s.tags), (Some("Important"), Some(0), vec!["x".to_string()]));
    }

    #[test]
    fn a_label_this_pc_lacks_is_drawn_as_written_with_no_place() {
        let s = shown(&default_labels(), &m(Some("Mauve"), &[]));
        assert_eq!((s.label.as_deref(), s.place), (Some("Mauve"), None));
    }

    #[test]
    fn a_color_tag_without_a_label_is_drawn_as_the_label() {
        let s = shown(&default_labels(), &m(None, &["Work", "Red", "Blue"]));
        assert_eq!((s.label.as_deref(), s.place), (Some("Red"), Some(0)));
        assert_eq!(s.tags, vec!["Work", "Blue"]);
    }

    #[test]
    fn no_label_and_no_color_tag_draw_no_label() {
        let s = shown(&default_labels(), &m(None, &["Work"]));
        assert_eq!((s.label, s.place, s.tags), (None, None, vec!["Work".to_string()]));
    }

    #[test]
    fn settling_makes_a_color_tag_the_label() {
        assert_eq!(settled(&default_labels(), &m(None, &["Work", "red"])), m(Some("Red"), &["Work"]));
    }

    #[test]
    fn settling_makes_an_old_name_the_new_one() {
        assert_eq!(settled(&renamed(), &m(Some("Red"), &[])), m(Some("Important"), &[]));
    }

    #[test]
    fn settling_leaves_alone_what_needs_nothing() {
        let lacks = m(Some(" Mauve\n"), &["a", "A"]);
        assert_eq!(settled(&default_labels(), &lacks), super::super::tidy(&lacks));
        let plain = m(None, &["Work"]);
        assert_eq!(settled(&default_labels(), &plain), plain);
    }

    #[test]
    fn a_label_is_saved_as_json_with_was_only_when_it_has_some() {
        let mut l = Label { name: "Red".into(), color: 15680580, was: vec![] };
        assert_eq!(serde_json::to_string(&l).unwrap_or_default(), r#"{"name":"Red","color":15680580}"#);
        l.was = vec!["Red".into()];
        assert_eq!(serde_json::to_string(&l).unwrap_or_default(), r#"{"name":"Red","color":15680580,"was":["Red"]}"#);
        let back: Option<Label> = serde_json::from_str(r#"{"name":"X","color":1}"#).ok();
        assert_eq!(back, Some(Label { name: "X".into(), color: 1, was: vec![] }));
    }
}
