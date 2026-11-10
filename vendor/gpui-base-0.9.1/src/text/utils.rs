use std::sync::Arc;

use data_url::DataUrl;
use gpui::{Image, ImageFormat};

const BULLETS: [&str; 5] = ["•", "◦", "▪", "‣", "⁃"];

/// Whether `ch` is a letter of a right-to-left script (Hebrew, Arabic, Syriac, Thaana, NKo and their forms).
fn is_rtl_letter(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF
    )
}

/// The direction the first strong character of `text` reads in (HTML's `dir="auto"`): `Some(true)` for a
/// right-to-left letter, `Some(false)` for any other letter, `None` when there is no letter.
///
/// An explicit right-to-left or left-to-right mark counts as a strong character, as it does where the
/// text is laid out (`paragraph_reads_right_to_left` in the platform's line layout), so a block is aligned
/// the way its rows are set.
pub(super) fn first_strong_rtl(text: &str) -> Option<bool> {
    for ch in text.chars() {
        match ch {
            '\u{200F}' => return Some(true),
            '\u{200E}' => return Some(false),
            _ if ch.is_alphabetic() => return Some(is_rtl_letter(ch)),
            _ => {}
        }
    }
    None
}

/// Whether `text` has a character of a right-to-left script, or a mark that says to read right to left. Text
/// that has none is laid out left to right, however its block is aligned.
pub(super) fn has_rtl_text(text: &str) -> bool {
    text.chars().any(|ch| is_rtl_letter(ch) || ch == '\u{200F}')
}

/// Returns an ordered-list item's ordinal, defaulting omitted starts to one.
pub(super) fn ordered_list_ordinal(start: Option<u32>, ix: usize) -> u32 {
    start
        .unwrap_or(1)
        .saturating_add(u32::try_from(ix).unwrap_or(u32::MAX))
}

/// Returns the prefix for a list item.
pub(super) fn list_item_prefix(
    ix: usize,
    start: Option<u32>,
    ordered: bool,
    depth: usize,
) -> String {
    if ordered {
        let ordinal = ordered_list_ordinal(start, ix);
        if depth == 0 || start == Some(0) {
            return format!("{ordinal}. ");
        }

        let first_letter = if depth == 1 { b'A' } else { b'a' };
        let mut remaining = ordinal;
        let mut letters = Vec::new();
        while remaining > 0 {
            remaining -= 1;
            letters.push((first_letter + (remaining % 26) as u8) as char);
            remaining /= 26;
        }
        let label: String = letters.into_iter().rev().collect();
        return format!("{label}. ");
    } else {
        let depth = depth.min(BULLETS.len() - 1);
        let bullet = BULLETS[depth];
        return format!("{} ", bullet);
    }
}

/// Decodes a `data:` URL whose mime type names an image format GPUI can
/// decode. Anything else (another scheme, a non-image body, malformed
/// base64) yields `None`, and the caller keeps the URL URI-backed so GPUI's
/// own loader reports the failure.
pub(super) fn data_url_image(url: &str) -> Option<Arc<Image>> {
    let data_url = DataUrl::process(url).ok()?;
    let mime = data_url.mime_type();
    let format = ImageFormat::from_mime_type(&format!("{}/{}", mime.type_, mime.subtype))?;
    let (bytes, _fragment) = data_url.decode_to_vec().ok()?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

#[cfg(test)]
mod tests {
    use gpui::ImageFormat;

    use crate::text::utils::{data_url_image, first_strong_rtl, has_rtl_text, list_item_prefix};

    #[test]
    fn the_first_strong_letter_gives_a_block_its_direction() {
        assert_eq!(first_strong_rtl("Hello"), Some(false));
        assert_eq!(first_strong_rtl("مرحبا بالعالم"), Some(true));
        assert_eq!(first_strong_rtl("שלום"), Some(true));
        // Digits, symbols and spaces are not strong: the first letter decides.
        assert_eq!(first_strong_rtl("123 - مرحبا"), Some(true));
        assert_eq!(first_strong_rtl("(1) Hello مرحبا"), Some(false));
        // No letters, no direction.
        assert_eq!(first_strong_rtl("1234 ..."), None);
        assert_eq!(first_strong_rtl(""), None);
        // The explicit marks are strong, as the platform's line layout reads them.
        assert_eq!(first_strong_rtl("1 \u{200F}2 abc"), Some(true));
        assert_eq!(first_strong_rtl("1 \u{200E}2 \u{0645}"), Some(false));
    }

    #[test]
    fn text_in_a_right_to_left_script_is_found_anywhere_in_it() {
        assert!(has_rtl_text("abc مرحبا"));
        assert!(has_rtl_text("\u{200F}1"));
        assert!(!has_rtl_text("abc 123 ..."));
        assert!(!has_rtl_text(""));
    }

    #[test]
    fn test_data_url_image() {
        fn image(url: &str) -> (ImageFormat, Vec<u8>) {
            let image = data_url_image(url)
                .unwrap_or_else(|| panic!("expected an embedded image for {url:?}"));
            (image.format(), image.bytes().to_vec())
        }

        assert_eq!(
            image("data:image/png;base64,iVBORw0KGgo="),
            (ImageFormat::Png, b"\x89PNG\r\n\x1a\n".to_vec())
        );
        // Legacy mime aliases and the percent-encoded (non-base64) body form.
        assert_eq!(
            image("data:image/jpg;base64,/9j/4A=="),
            (ImageFormat::Jpeg, b"\xff\xd8\xff\xe0".to_vec())
        );
        assert_eq!(
            image("data:image/svg+xml,%3Csvg%3E%3C/svg%3E"),
            (ImageFormat::Svg, b"<svg></svg>".to_vec())
        );

        assert!(data_url_image("https://example.com/logo.png").is_none());
        assert!(data_url_image("data:text/plain;base64,aGVsbG8=").is_none());
        assert!(data_url_image("data:image/png;base64,not*base64").is_none());
        assert!(data_url_image("data:image/png").is_none());
    }

    #[test]
    fn test_alphabetic_list_item_prefix() {
        for (ordinal, label) in [
            (1, "A"),
            (26, "Z"),
            (27, "AA"),
            (28, "AB"),
            (52, "AZ"),
            (53, "BA"),
            (702, "ZZ"),
            (703, "AAA"),
            (u32::MAX, "MWLQKWU"),
        ] {
            for depth in [1, 2, 3] {
                let label = if depth == 1 {
                    label.to_string()
                } else {
                    label.to_ascii_lowercase()
                };
                let expected = format!("{label}. ");
                assert_eq!(list_item_prefix(0, Some(ordinal), true, depth), expected);
                assert_eq!(
                    list_item_prefix((ordinal - 1) as usize, None, true, depth),
                    expected
                );
            }
        }
        assert_eq!(list_item_prefix(1, Some(26), true, 1), "AA. ");
        assert_eq!(list_item_prefix(1, Some(26), true, 2), "aa. ");
        assert_eq!(list_item_prefix(usize::MAX, Some(1), true, 1), "MWLQKWU. ");
        assert_eq!(list_item_prefix(26, Some(1), true, 0), "27. ");
        assert_eq!(list_item_prefix(27, Some(0), true, 1), "27. ");
    }

    #[test]
    fn test_list_item_prefix() {
        assert_eq!(list_item_prefix(0, Some(1), true, 0), "1. ");
        assert_eq!(list_item_prefix(1, Some(1), true, 0), "2. ");
        assert_eq!(list_item_prefix(2, Some(1), true, 0), "3. ");
        assert_eq!(list_item_prefix(10, Some(1), true, 0), "11. ");
        assert_eq!(list_item_prefix(0, Some(3), true, 0), "3. ");
        assert_eq!(list_item_prefix(1, Some(3), true, 0), "4. ");
        assert_eq!(list_item_prefix(0, Some(1), true, 1), "A. ");
        assert_eq!(list_item_prefix(1, Some(1), true, 1), "B. ");
        assert_eq!(list_item_prefix(0, Some(4), true, 1), "D. ");
        assert_eq!(list_item_prefix(1, Some(4), true, 1), "E. ");
        assert_eq!(list_item_prefix(0, Some(1), true, 2), "a. ");
        assert_eq!(list_item_prefix(1, Some(1), true, 2), "b. ");
        assert_eq!(list_item_prefix(6, Some(1), true, 2), "g. ");
        assert_eq!(list_item_prefix(0, Some(0), true, 1), "0. ");
        assert_eq!(list_item_prefix(1, Some(0), true, 1), "1. ");
        assert_eq!(list_item_prefix(0, None, false, 0), "• ");
        assert_eq!(list_item_prefix(0, None, false, 1), "◦ ");
        assert_eq!(list_item_prefix(0, None, false, 2), "▪ ");
        assert_eq!(list_item_prefix(0, None, false, 3), "‣ ");
        assert_eq!(list_item_prefix(0, None, false, 4), "⁃ ");
    }
}
