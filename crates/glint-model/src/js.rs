//! JavaScript's semantics where Glint's ported code depends on them: what counts as white space, how
//! `Math.round`, `Math.min`/`Math.max`, `String(number)` and `toFixed` behave. One copy for every module
//! (the coordinator's; ports call these instead of keeping private copies).

/// JavaScript's white space (`\s`, `trim()`): ASCII white space plus the Unicode space separators, the
/// line and paragraph separators and the byte order mark.
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{b}'
            | '\u{c}'
            | '\r'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// `text.trim()` as JavaScript trims.
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

/// `Math.round`: halves go toward +infinity, so 2.5 is 3 and -2.5 is -2 (Rust's `round` gives -3). A
/// whole number comes back as it is: adding 0.5 would round 2^53 - 1 up to 2^53.
pub fn js_round(x: f64) -> f64 {
    if x.fract() == 0.0 {
        x
    } else {
        (x + 0.5).floor()
    }
}

/// `text.length`: the length in UTF-16 code units, as JavaScript counts it.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `Math.round` for pixels.
pub fn js_round_f32(x: f32) -> f32 {
    (x + 0.5).floor()
}

/// `Math.min`: any NaN makes the result NaN, where `f64::min` would ignore it.
pub fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

/// `Math.max`: any NaN makes the result NaN, where `f64::max` would ignore it.
pub fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// `String(x)` for a number: the shortest digits that read back as `x`, in plain notation from 1e-7 up
/// to 1e21 and in exponent form outside it ("1e+21", "5e-7"), "NaN", "Infinity", and -0 as "0".
pub fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x == 0.0 {
        return "0".to_string();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-Infinity" } else { "Infinity" }.to_string();
    }
    let sign = if x < 0.0 { "-" } else { "" };
    // `{:e}` gives the shortest digits that round-trip, as JavaScript does.
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp) = match sci.split_once('e') {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (sci.as_str(), 0),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        let (a, b) = digits.split_at(n as usize);
        format!("{a}.{b}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let e_text = format!("{}{}", if e < 0 { '-' } else { '+' }, e.abs());
        if k == 1 {
            format!("{digits}e{e_text}")
        } else {
            let (a, b) = digits.split_at(1);
            format!("{a}.{b}e{e_text}")
        }
    };
    format!("{sign}{body}")
}

/// `x.toFixed(digits)`: decided on `x`'s exact binary value, as JavaScript does. So 0.15 (stored a hair
/// below) gives "0.1", while 1.25 (stored exactly, a true tie) gives "1.3": ties go to the larger
/// magnitude. From 1e21 up, JavaScript gives `String(x)`, and so does this.
pub fn to_fixed(x: f64, digits: usize) -> String {
    if !x.is_finite() || x.abs() >= 1e21 {
        return number_to_string(x);
    }
    // The exact decimal expansion: an f64 has at most 1074 fraction digits, so this many is exact.
    let exact = format!("{:.1100}", x.abs());
    let (whole, fraction) = exact.split_once('.').unwrap_or((exact.as_str(), ""));
    let kept = &fraction[..digits.min(fraction.len())];
    let rest = &fraction[digits.min(fraction.len())..];
    // Round half up on the exact digits: the first dropped digit 5 or more rounds up.
    let round_up = rest.as_bytes().first().is_some_and(|d| *d >= b'5');
    let mut number: Vec<u8> = whole.bytes().chain(kept.bytes()).collect();
    if round_up {
        let mut i = number.len();
        loop {
            if i == 0 {
                number.insert(0, b'1');
                break;
            }
            i -= 1;
            if number[i] == b'9' {
                number[i] = b'0';
            } else {
                number[i] += 1;
                break;
            }
        }
    }
    let split = number.len() - kept.len();
    let (w, f) = number.split_at(split);
    let w = String::from_utf8_lossy(w);
    let mut out = String::new();
    // JavaScript keeps the sign of a negative number even when it rounds to zero ("-0.0").
    if x < 0.0 {
        out.push('-');
    }
    out.push_str(if w.is_empty() { "0" } else { &w });
    if digits > 0 {
        out.push('.');
        out.push_str(&String::from_utf8_lossy(f));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_space_is_javascripts() {
        assert!(is_js_space('\u{a0}') && is_js_space('\u{3000}') && is_js_space('\u{feff}'));
        assert!(!is_js_space('\u{200b}') && !is_js_space('x'));
        assert_eq!(js_trim("\u{a0} hi \u{2028}"), "hi");
    }

    #[test]
    fn rounding_and_extremes_match_javascript() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert!(js_min(1.0, f64::NAN).is_nan() && js_max(f64::NAN, 1.0).is_nan());
        assert_eq!(js_max(1.0, 2.0), 2.0);
        assert_eq!(js_round(9007199254740991.0), 9007199254740991.0);
        assert_eq!(utf16_len("a😀"), 3);
    }

    #[test]
    fn numbers_print_as_javascript_prints_them() {
        assert_eq!(number_to_string(1.5), "1.5");
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(123456789012345680000.0), "123456789012345680000");
        assert_eq!(number_to_string(5e-7), "5e-7");
        assert_eq!(number_to_string(0.000001), "0.000001");
        assert_eq!(number_to_string(f64::NAN), "NaN");
    }

    #[test]
    fn to_fixed_decides_on_the_exact_value() {
        assert_eq!(to_fixed(0.15, 1), "0.1");
        assert_eq!(to_fixed(1.25, 1), "1.3");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(9.995, 2), "9.99");
        assert_eq!(to_fixed(99.95, 1), "100.0");
        assert_eq!(to_fixed(-1.25, 1), "-1.3");
        assert_eq!(to_fixed(12.0, 0), "12");
        assert_eq!(to_fixed(0.0, 2), "0.00");
        assert_eq!(to_fixed(f64::NAN, 1), "NaN");
    }
}
