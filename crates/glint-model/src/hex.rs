//! Ported from src/lib/hex.ts.
//!
//! The hex view: bytes as hex and characters, offsets typed in.
//!
//! Offsets are `u64` byte positions. A file offset is never negative, and parse_offset returns one only when
//! the text reads as a whole number that fits.

use crate::js::is_js_space;

/// hexyl's classes of bytes, which the hex view colors: zero, ASCII text, whitespace, other ASCII, and the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteClass {
    /// The zero byte.
    Null,
    /// Printable ASCII, apart from the space.
    Text,
    /// The space and the ASCII white space (tab, newline, and so on).
    Space,
    /// Other ASCII: the control characters.
    Control,
    /// Bytes 0x80 and up.
    High,
}

/// Which class a byte is in, as hexyl colors it.
pub fn byte_class(byte: u8) -> ByteClass {
    if byte == 0 {
        ByteClass::Null
    } else if byte == 0x20 || (0x09..=0x0d).contains(&byte) {
        ByteClass::Space
    } else if byte > 0x20 && byte < 0x7f {
        ByteClass::Text
    } else if byte < 0x80 {
        ByteClass::Control
    } else {
        ByteClass::High
    }
}

/// A byte in the text column, as hexyl draws it: ⋄ for zero, _ for whitespace but a space, the byte itself for
/// printable ASCII, • for other ASCII, and × above it.
pub fn byte_char(byte: u8) -> char {
    match byte_class(byte) {
        ByteClass::Null => '⋄',
        ByteClass::Space => {
            if byte == 0x20 {
                ' '
            } else {
                '_'
            }
        }
        ByteClass::Text => char::from(byte),
        ByteClass::Control => '•',
        ByteClass::High => '×',
    }
}

/// A number in upper-case hex, padded with zeros to `width`: hex(255, 2) is "FF", hex(0x1f00, 8) is "00001F00".
pub fn hex(n: u64, width: usize) -> String {
    format!("{n:0width$X}")
}

/// An offset typed to go to: 0x1F00, 1F00h, or 1F00 (hexadecimal, having a letter), or 8192 (decimal).
///
/// Spaces and underscores are ignored, so " 1_000 " is 1000. Returns None when the text is none of these, or
/// when the number does not fit in a u64.
// NOTE(port): the TypeScript parsed to a JS number, which loses precision above 2^53 and never fails on size.
// Here a number above u64::MAX gives None, which the UI treats as "not an offset".
pub fn parse_offset(text: &str) -> Option<u64> {
    // JavaScript's \s and trim() both skip the same white space, so one filter covers the trim too.
    let value: String = text.chars().filter(|&c| !is_js_space(c) && c != '_').collect();
    let prefixed = strip_0x(&value)
        .filter(|digits| is_hex_digits(digits))
        .or_else(|| value.strip_suffix(['h', 'H']).filter(|digits| is_hex_digits(digits)));
    if let Some(digits) = prefixed {
        return u64::from_str_radix(digits, 16).ok();
    }
    if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) {
        return value.parse::<u64>().ok();
    }
    if is_hex_digits(&value) {
        return u64::from_str_radix(&value, 16).ok();
    }
    None
}

/// The digits after a 0x or 0X prefix, if there is one.
fn strip_0x(value: &str) -> Option<&str> {
    value.strip_prefix("0x").or_else(|| value.strip_prefix("0X"))
}

/// One or more ASCII hex digits, in either case.
fn is_hex_digits(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_classes_follow_hexyl() {
        assert_eq!(
            [0u8, 0x20, 0x0a, 0x41, 0x7f, 0x01, 0x80, 0xff].map(byte_class),
            [
                ByteClass::Null,
                ByteClass::Space,
                ByteClass::Space,
                ByteClass::Text,
                ByteClass::Control,
                ByteClass::Control,
                ByteClass::High,
                ByteClass::High,
            ]
        );
    }

    #[test]
    fn byte_chars_show_the_class_and_the_text() {
        let text: String = [0u8, 0x20, 0x09, 0x41, 0x1b, 0xe9].map(byte_char).iter().collect();
        assert_eq!(text, "⋄ _A•×");
    }

    #[test]
    fn hex_pads_to_the_width_in_upper_case() {
        assert_eq!(hex(255, 2), "FF");
        assert_eq!(hex(0x1f00, 8), "00001F00");
        assert_eq!(hex(0, 4), "0000");
        assert_eq!(hex(0x1_0000, 2), "10000", "a wider number is never cut");
    }

    #[test]
    fn offsets_read_as_hex_with_a_prefix_or_suffix() {
        assert_eq!(parse_offset("0x1F00"), Some(0x1f00));
        assert_eq!(parse_offset("1f00h"), Some(0x1f00));
        assert_eq!(parse_offset("1F00"), Some(0x1f00));
    }

    #[test]
    fn offsets_read_plain_digits_as_decimal() {
        assert_eq!(parse_offset("8192"), Some(8192));
        assert_eq!(parse_offset(" 1_000 "), Some(1000));
    }

    #[test]
    fn offsets_that_are_neither_give_none() {
        assert_eq!(parse_offset("12g"), None);
        assert_eq!(parse_offset(""), None);
        assert_eq!(parse_offset("0x"), None);
        assert_eq!(parse_offset("99999999999999999999999"), None, "too big for a u64");
    }
}
