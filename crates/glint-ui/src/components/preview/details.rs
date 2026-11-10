//! The details under a preview: type, size, date and place, and what a photo, video or song says of itself
//! (`Details` in FilePreview.tsx).

use super::sources::Media;
use crate::t;
use glint_model::entries::{is_recycled, FileEntry};
use glint_model::file_types::type_label;
use glint_model::format::{format_count, format_full_date, format_size};
use glint_model::paths::parent_path;
use gpui_kit::SharedString;

const PICTURES: &str = "jpg jpeg jpe jfif heic heif avif png tif tiff webp dng cr2 cr3 nef nrw arw srf sr2 raf orf rw2 pef iiq srw gif bmp ico jxl psd qoi";
const VIDEOS: &str = "mp4 m4v mov 3gp 3g2 mkv webm";
const SONGS: &str = "mp3 flac m4a m4b ogg oga opus wav aif aiff aifc ape wv mpc spx aac";

/// Photos, videos and songs: the files with details to read (`hasDetails`).
pub fn has_details(entry: &FileEntry) -> bool {
    !entry.dir
        && [PICTURES, VIDEOS, SONGS]
            .iter()
            .any(|list| list.split(' ').any(|ext| ext == entry.ext))
}

/// One row: its label, and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub label: SharedString,
    pub value: String,
}

/// The rows under the preview: what the file says of itself, then Type, Size (not for a folder, except in the
/// Recycle Bin), Modified (Deleted there) and Location (Deleted from there).
pub fn rows(entry: &FileEntry, media: Option<&Media>) -> Vec<Row> {
    let recycled = is_recycled(entry);
    let mut rows: Vec<Row> = media
        .map(|media| {
            media
                .rows
                .iter()
                .map(|(label, value)| Row {
                    label: label.clone().into(),
                    value: value.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    rows.push(Row {
        label: t!("Type"),
        value: type_label(entry),
    });
    if !entry.dir || recycled {
        rows.push(Row {
            label: t!("Size"),
            value: format!(
                "{} ({} bytes)",
                format_size(entry.size),
                format_count(usize::try_from(entry.size).unwrap_or(usize::MAX))
            ),
        });
    }
    rows.push(Row {
        label: if recycled { t!("Deleted") } else { t!("Modified") },
        value: format_full_date(entry.modified),
    });
    rows.push(Row {
        label: if recycled { t!("Deleted from") } else { t!("Location") },
        value: if recycled {
            entry.origin.clone().unwrap_or_default()
        } else {
            parent_path(&entry.path)
        },
    });
    rows
}

/// A place on OpenStreetMap (`mapLink`).
pub fn map_link([lat, lon]: [f64; 2]) -> String {
    format!("https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map=16/{lat}/{lon}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, folder};
    use glint_model::entries::attr;

    #[test]
    fn a_file_has_type_size_date_and_location() {
        let entry = file("C:\\Users\\a", "notes.txt", 1536, 0);
        let rows = rows(&entry, None);
        let labels: Vec<_> = rows.iter().map(|r| r.label.to_string()).collect();
        assert_eq!(labels, ["Type", "Size", "Modified", "Location"]);
        assert_eq!(rows[1].value, "1.5 KB (1,536 bytes)");
        assert_eq!(rows[3].value, "C:\\Users\\a");
    }

    #[test]
    fn a_folder_has_no_size_unless_it_is_in_the_recycle_bin() {
        let labels = |entry: &FileEntry| {
            rows(entry, None)
                .iter()
                .map(|r| r.label.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(labels(&folder("C:\\x", "sub")), ["Type", "Modified", "Location"]);
        let mut binned = folder("C:\\x", "sub");
        binned.attrs |= attr::RECYCLED;
        binned.origin = Some("C:\\Users\\a".into());
        assert_eq!(labels(&binned), ["Type", "Size", "Deleted", "Deleted from"]);
        assert_eq!(rows(&binned, None)[3].value, "C:\\Users\\a");
    }

    #[test]
    fn what_a_photo_says_comes_first() {
        let media = Media {
            rows: vec![("Camera".into(), "Canon EOS".into()), ("Taken".into(), "Today".into())],
            place: Some([51.5, -0.12]),
        };
        let rows = rows(&file("C:\\x", "a.jpg", 10, 0), Some(&media));
        assert_eq!(rows[0].label, "Camera");
        assert_eq!(rows[1].value, "Today");
        assert_eq!(rows[2].label, "Type");
        assert!(has_details(&file("C:\\x", "a.JPG".to_lowercase().as_str(), 1, 0)));
        assert!(!has_details(&file("C:\\x", "a.txt", 1, 0)));
        assert!(map_link([51.5, -0.12]).starts_with("https://www.openstreetmap.org/?mlat=51.5&mlon=-0.12#map=16/"));
    }
}
