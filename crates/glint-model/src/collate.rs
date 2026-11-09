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

/// The same in a named locale ("en-US", "ar-SA"), for tests and callers that pick one.
pub fn compare_base_numeric_in(locale: Option<&str>, a: &str, b: &str) -> Ordering {
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
    fn arabic_text_compares_without_panicking() {
        assert_ne!(compare_base_numeric_in(Some("ar-SA"), "أحمد", "محمد"), Ordering::Equal);
        // An unknown locale falls back instead of failing.
        assert_eq!(compare_base_numeric_in(Some("xx-NOPE-1"), "a", "b"), Ordering::Less);
    }
}
