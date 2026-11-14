//! Ported from src/lib/color.ts.
//!
//! Colors: the theme's HSL form and readable text on a color.
//!
//! Both functions take "#rrggbb" and read the channels by position, as the TypeScript did. The positions are
//! UTF-16 code units, as JavaScript counts them, so a name with odd characters cannot make the slices panic.
//! Malformed input gives NaN parts (and so "NaN" text), as the TypeScript did, rather than a panic.

use crate::js::is_js_space;
#[cfg(test)]
use crate::js::{js_max, js_min, js_round, number_to_string};

/// "333 71% 51%", as the color tokens in index.css are written, for #rrggbb.
#[cfg(test)]
pub fn hsl_of(hex: &str) -> String {
    let units: Vec<u16> = hex.encode_utf16().collect();
    let [r, g, b] = [1, 3, 5].map(|i| channel(&units, i));
    let max = js_max(js_max(r, g), b);
    let min = js_min(js_min(r, g), b);
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d == 0.0 {
        0.0
    } else {
        d / (1.0 - (2.0 * l - 1.0).abs())
    };
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d + 6.0) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    format!(
        "{} {}% {}%",
        number_to_string(js_round(h * 60.0)),
        number_to_string(js_round(s * 100.0)),
        number_to_string(js_round(l * 100.0))
    )
}

/// White or near-black text on #rrggbb, whichever reads better (WCAG contrast), as a color token.
pub fn text_on(hex: &str) -> &'static str {
    let units: Vec<u16> = hex.encode_utf16().collect();
    let lum = |c: f64| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = [1, 3, 5].map(|i| lum(channel(&units, i)));
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    // Near-black is #121212 (luminance 0.0063).
    if 1.05 / (y + 0.05) >= (y + 0.05) / 0.0563 {
        "0 0% 100%"
    } else {
        "0 0% 7%"
    }
}

/// One channel as a fraction of 255: parseInt(hex.slice(i, i + 2), 16) / 255 in the TypeScript.
fn channel(units: &[u16], start: usize) -> f64 {
    let end = start.saturating_add(2).min(units.len());
    // An empty or reversed range (start past the end) reads as an empty pair, which parses to NaN.
    let pair = units.get(start..end).unwrap_or(&[]);
    parse_int_hex(&String::from_utf16_lossy(pair)) / 255.0
}

/// JavaScript's parseInt(text, 16): leading white space and a sign are skipped, an optional 0x prefix is
/// dropped, and digits are read until the first one that is not hex. No digits at all gives NaN.
fn parse_int_hex(text: &str) -> f64 {
    let text = text.trim_start_matches(is_js_space);
    let (sign, text) = match text.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, text.strip_prefix('+').unwrap_or(text)),
    };
    let text = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    let mut value = 0.0;
    let mut any = false;
    for c in text.chars() {
        match c.to_digit(16) {
            Some(digit) => {
                value = value * 16.0 + f64::from(digit);
                any = true;
            }
            None => break,
        }
    }
    if any {
        sign * value
    } else {
        f64::NAN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_of_writes_the_accent_as_the_theme_tokens_are_written() {
        assert_eq!(hsl_of("#db2777"), "333 71% 51%");
        assert_eq!(hsl_of("#0078d4"), "206 100% 42%");
        assert_eq!(hsl_of("#808080"), "0 0% 50%");
    }

    #[test]
    fn hsl_of_gives_no_hue_for_grey() {
        assert_eq!(hsl_of("#000000"), "0 0% 0%");
        assert_eq!(hsl_of("#ffffff"), "0 0% 100%");
    }

    #[test]
    fn text_on_picks_white_on_dark_blue() {
        assert_eq!(
            text_on("#005a9e"),
            "0 0% 100%",
            "white on Windows' default blue, darker shade"
        );
    }

    #[test]
    fn text_on_picks_near_black_on_light_colors() {
        assert_eq!(text_on("#99ebff"), "0 0% 7%", "near-black on its lighter shade");
        assert_eq!(text_on("#ffb900"), "0 0% 7%", "near-black on yellow");
    }

    #[test]
    fn parse_int_hex_reads_like_javascript() {
        assert_eq!(parse_int_hex("db"), 219.0);
        assert_eq!(parse_int_hex("4g"), 4.0, "stops at the first character that is not hex");
        assert!(parse_int_hex("0x").is_nan(), "a bare 0x has no digits");
        assert!(parse_int_hex("zz").is_nan());
    }

    #[test]
    fn a_short_or_odd_color_gives_nan_parts_and_does_not_panic() {
        // Fewer than seven characters, or non-ASCII, read as NaN parts, as the TypeScript printed them.
        assert_eq!(hsl_of(""), "NaN NaN% NaN%");
        assert_eq!(text_on(""), "0 0% 7%");
        assert_eq!(hsl_of("#é"), "NaN NaN% NaN%");
    }
}
