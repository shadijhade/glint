//! Comparing text the way people expect in their language, where the TypeScript used
//! `localeCompare(…, undefined, { sensitivity: "base", numeric: true })` (artists, albums): Windows' own
//! `CompareStringEx`, case and accents ignored, digits read as numbers. File names don't come through
//! here: they use `entries`' natural order, which matches the core's.

use std::cmp::Ordering;
use windows::core::PCWSTR;
use windows::Win32::Globalization::{
    CompareStringEx, COMPARE_STRING_FLAGS, CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN, LINGUISTIC_IGNORECASE,
    LINGUISTIC_IGNOREDIACRITIC, SORT_DIGITSASNUMBERS,
};

/// `a.localeCompare(b, undefined, { sensitivity: "base", numeric: true })` in the user's own locale.
pub fn compare_base_numeric(a: &str, b: &str) -> Ordering {
    compare_base_numeric_in(None, a, b)
}

#[link(name = "kernel32")]
extern "system" {
    // Declared here for its destination, which for a sort key is bytes (the `windows` crate's takes UTF-16).
    fn LCMapStringEx(
        locale: *const u16,
        flags: u32,
        source: *const u16,
        source_len: i32,
        dest: *mut u8,
        dest_len: i32,
        version: *const std::ffi::c_void,
        reserved: *const std::ffi::c_void,
        sort_handle: isize,
    ) -> i32;
}

/// `LCMAP_SORTKEY`: the answer is the text's sort key, in bytes.
const LCMAP_SORTKEY: u32 = 0x400;

/// What [`compare_base_numeric`] goes by, made once for a text: two texts' keys compare as plain bytes the way the
/// texts compare there (Windows says so of `LCMapStringEx`'s sort keys made with `CompareStringEx`'s flags). A sort
/// makes one key a row with it, instead of asking Windows about two texts at every comparison.
pub fn sort_key(text: &str) -> Vec<u8> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let Ok(len) = i32::try_from(wide.len()) else {
        return text.to_lowercase().into_bytes();
    };
    if len == 0 {
        return Vec::new();
    }
    let flags = LCMAP_SORTKEY | (LINGUISTIC_IGNORECASE | LINGUISTIC_IGNOREDIACRITIC | SORT_DIGITSASNUMBERS).0;
    let map = |dest: &mut [u8]| {
        // SAFETY: the source is passed with its length and the destination with its own (none asks for the size);
        // a null locale is the user's own, and the reserved parameters are null as documented.
        unsafe {
            LCMapStringEx(
                std::ptr::null(),
                flags,
                wide.as_ptr(),
                len,
                if dest.is_empty() { std::ptr::null_mut() } else { dest.as_mut_ptr() },
                i32::try_from(dest.len()).unwrap_or(0),
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        }
    };
    let mut key = vec![0u8; usize::try_from(map(&mut [])).unwrap_or(0)];
    let written = usize::try_from(map(&mut key)).unwrap_or(0);
    if written == 0 {
        // The call failed: plain order, as `compare_base_numeric` falls back to.
        return text.to_lowercase().into_bytes();
    }
    key.truncate(written);
    key
}

/// The same in a named locale ("en-US", "ar-SA"), for tests and callers that pick one.
pub fn compare_base_numeric_in(locale: Option<&str>, a: &str, b: &str) -> Ordering {
    // The same text needs no asking (a sort by a column most rows have nothing in compares "" with "").
    if a == b {
        return Ordering::Equal;
    }
    let flags: COMPARE_STRING_FLAGS = LINGUISTIC_IGNORECASE | LINGUISTIC_IGNOREDIACRITIC | SORT_DIGITSASNUMBERS;
    let wide = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let (a16, b16) = (wide(a), wide(b));
    let locale16: Option<Vec<u16>> = locale.map(|l| l.encode_utf16().chain(std::iter::once(0)).collect());
    let locale_ptr = locale16.as_ref().map_or(PCWSTR::null(), |l| PCWSTR(l.as_ptr()));
    // SAFETY: both strings are passed as slices with their lengths; the locale name, when given, is
    // NUL-terminated and outlives the call; the reserved parameters are null as documented.
    let result = unsafe { CompareStringEx(locale_ptr, flags, &a16, &b16, None, None, None) };
    match result {
        CSTR_LESS_THAN => Ordering::Less,
        CSTR_GREATER_THAN => Ordering::Greater,
        CSTR_EQUAL => Ordering::Equal,
        // The call failed (an unknown locale name, say): fall back to plain order, never panic.
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_accents_and_numbers_compare_as_people_read_them() {
        let en = Some("en-US");
        assert_eq!(compare_base_numeric_in(en, "abba", "ABBA"), Ordering::Equal);
        assert_eq!(compare_base_numeric_in(en, "Beyoncé", "Beyonce"), Ordering::Equal);
        assert_eq!(compare_base_numeric_in(en, "Track 9", "Track 10"), Ordering::Less);
        assert_eq!(compare_base_numeric_in(en, "b", "a"), Ordering::Greater);
    }

    #[test]
    fn sort_keys_order_texts_as_comparing_them_does() {
        let texts = [
            "", "a", "b", "abba", "ABBA", "Abba", "Beyoncé", "Beyonce", "beyond", "Track 9", "Track 10", "track 2", "9",
            "10", "a b", "a-b", "Über", "uber", "أحمد", "محمد", "work, urgent", "Work", "日本", "z",
        ];
        for a in texts {
            for b in texts {
                assert_eq!(sort_key(a).cmp(&sort_key(b)), compare_base_numeric(a, b), "{a:?} and {b:?}");
            }
        }
    }

    #[test]
    fn arabic_text_compares_without_panicking() {
        assert_ne!(compare_base_numeric_in(Some("ar-SA"), "أحمد", "محمد"), Ordering::Equal);
        // An unknown locale falls back instead of failing.
        assert_eq!(compare_base_numeric_in(Some("xx-NOPE-1"), "a", "b"), Ordering::Less);
    }
}
