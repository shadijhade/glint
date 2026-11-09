//! Ported from src/lib/cast.ts.
//!
//! Asciinema recordings (.cast): parsing and playing back to a point.
//!
//! A recording is a JSON header line saying the terminal's size, then one JSON line per event. Version 2
//! gives each event's time from the start; version 3 gives it since the previous event. Only output ("o")
//! is played. A pause longer than the file's idle limit is cut to that limit, so a recording left running
//! for an hour plays in seconds. Times are seconds as `f64`, and nothing here indexes text.

use crate::js::{js_max, js_min, js_trim, number_to_string};
use serde_json::Value;

/// The idle limit, in seconds, for a recording that doesn't set its own.
pub const DEFAULT_IDLE_SECONDS: f64 = 3.0;

/// One piece of output, at `t` seconds from the start of the playback (after the long pauses are cut).
#[derive(Debug, Clone, PartialEq)]
pub struct CastEvent {
    /// When this output is printed, in seconds from the start of the playback.
    pub t: f64,
    /// The output text, exactly as the recording holds it (escape codes and all).
    pub data: String,
}

/// An asciinema recording, ready to play: its terminal's size, and what it printed when.
#[derive(Debug, Clone, PartialEq)]
pub struct Cast {
    /// Terminal columns, from 1 to 1000.
    pub cols: usize,
    /// Terminal rows, from 1 to 500.
    pub rows: usize,
    /// The recording's title from the header, or "" when it has none.
    pub title: String,
    /// Output, at seconds from the start (long pauses cut short).
    pub events: Vec<CastEvent>,
    /// The playback length in seconds: the time of the last event after the pauses are cut.
    pub duration: f64,
}

/// The output up to a moment, all joined, and how many events that is.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    /// The output text of every event up to the moment, joined in order.
    pub data: String,
    /// How many events the text came from.
    pub count: usize,
}

/// An asciinema .cast file: a line of JSON saying the terminal's size, then a line per event, `[time,
/// code, data]`, the time from the start (version 2) or since the last event (version 3). Only output
/// ("o") is played. Pauses longer than the file's idle_time_limit (else `idle` seconds) are cut to it, as
/// asciinema's player does when asked. None when it isn't one.
///
/// Lines that aren't JSON, or aren't an event, are skipped; a bad header is None. Never panics.
///
/// NOTE(port): serde_json refuses a number too large for an f64 (1e400) and a lone surrogate escape
/// (\ud800). JavaScript's JSON.parse keeps both, so those lines are skipped here where the TypeScript
/// played them. A non-string "title" becomes "" here; the TypeScript kept the value as it was.
pub fn parse_cast(text: &str, idle: f64) -> Option<Cast> {
    // Lines end at "\n" or "\r\n", as the TypeScript's /\r?\n/ splits them.
    let mut lines = text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line));
    // split always yields at least one item, so the empty fallback only matters for an impossible input.
    let header_line = lines.next().unwrap_or("");
    let header: Value = serde_json::from_str(header_line).ok()?;
    if !header.is_object() {
        return None;
    }
    let relative = match header.get("version").and_then(Value::as_f64) {
        Some(2.0) => false,
        Some(3.0) => true,
        _ => return None,
    };

    // Version 3 keeps the size under "term"; version 2 keeps it at the top level.
    let (cols_value, rows_value) = if relative {
        (
            header.get("term").and_then(|term| term.get("cols")),
            header.get("term").and_then(|term| term.get("rows")),
        )
    } else {
        (header.get("width"), header.get("height"))
    };
    let cols = header_size(cols_value, 80.0, 1000.0);
    let rows = header_size(rows_value, 24.0, 500.0);

    // A limit that isn't a number above zero (missing, zero, negative, or not a number) means the default.
    let limit = match header.get("idle_time_limit").map(js_number) {
        Some(n) if n > 0.0 => n,
        _ => idle,
    };

    let title = header.get("title").and_then(Value::as_str).unwrap_or("").to_string();

    let mut events = Vec::new();
    // The TypeScript calls this `clock`; it is the playback position, named apart from the `clock` function.
    let mut playback = 0.0_f64;
    let mut last = 0.0_f64;
    for line in lines {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(fields) = event.as_array() else {
            continue;
        };
        let Some(time) = fields.first().and_then(Value::as_f64) else {
            continue;
        };
        let at = if relative { last + time } else { time };
        // The pause before this event, cut short to the limit.
        playback += js_min(js_max(0.0, at - last), limit);
        last = at;
        if fields.get(1).and_then(Value::as_str) == Some("o") {
            if let Some(data) = fields.get(2).and_then(Value::as_str) {
                events.push(CastEvent {
                    t: playback,
                    data: data.to_string(),
                });
            }
        }
    }

    Some(Cast {
        cols,
        rows,
        title,
        events,
        duration: playback,
    })
}

/// The output up to `seconds` in, all at once (to show a moment of it), and how many events that is.
pub fn output_until(cast: &Cast, seconds: f64) -> Output {
    let mut count = 0;
    let mut data = String::new();
    for event in cast.events.iter().take_while(|e| e.t <= seconds) {
        data.push_str(&event.data);
        count += 1;
    }
    Output { data, count }
}

/// 1:05
///
/// Minutes are not padded, seconds are two digits. Like the TypeScript, a negative or NaN input gives the
/// text JavaScript would (`-1:-1`, `NaN:NaN`), so the caller can see the bad value. Minutes past 1e21 print in
/// exponent form, as JavaScript's `String` does.
pub fn clock(seconds: f64) -> String {
    let minutes = number_to_string((seconds / 60.0).floor());
    let secs = number_to_string((seconds % 60.0).floor());
    format!("{minutes}:{secs:0>2}")
}

/// A terminal size from the header: `value ?? default`, then clamped from 1 to `high`, as the TypeScript did.
///
/// NOTE(port): the TypeScript keeps fractions (80.5 columns). A terminal can't draw half a column, so this
/// rounds down. A size that isn't a number at all falls back to the default, where the TypeScript made NaN.
fn header_size(value: Option<&Value>, default: f64, high: f64) -> usize {
    let n = match value {
        None | Some(Value::Null) => default,
        Some(v) => js_number(v),
    };
    let clamped = js_max(1.0, js_min(high, n));
    if clamped.is_nan() {
        // The default is a small constant, so the cast can't truncate.
        return default as usize;
    }
    // `clamped` is between 1 and `high`, so flooring it fits in a usize.
    clamped.floor() as usize
}

/// JavaScript's `Number(value)` for a value from a JSON file.
///
/// NOTE(port): arrays and objects are NaN here, where JavaScript would read a one-element array as its
/// element. Strings are read the way Number reads them, with the words that JavaScript doesn't accept
/// (Infinity, NaN) rejected, and an empty string is zero.
fn js_number(value: &Value) -> f64 {
    match value {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::String(s) => js_string_number(s),
        Value::Null | Value::Array(_) | Value::Object(_) => f64::NAN,
    }
}

fn js_string_number(text: &str) -> f64 {
    // Number() trims JavaScript's white space, which Rust's trim() does not match (it has U+0085, and not U+FEFF).
    let trimmed = js_trim(text);
    if trimmed.is_empty() {
        return 0.0;
    }
    if trimmed.contains(['i', 'I', 'n', 'N']) {
        return f64::NAN;
    }
    trimmed.parse::<f64>().unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cast_reads_version_2_with_times_from_the_start_and_cuts_long_pauses() {
        let v2 = parse_cast(
            "{\"version\": 2, \"width\": 100, \"height\": 30, \"title\": \"demo\"}\n[0.5, \"o\", \"$ \"]\n[1.0, \"i\", \"l\"]\n[1.5, \"o\", \"ls\\r\\n\"]\n[60.0, \"o\", \"done\"]\nnot json\n",
            DEFAULT_IDLE_SECONDS,
        )
        .expect("a version 2 recording parses");
        assert_eq!((v2.cols, v2.rows, v2.title.as_str()), (100, 30, "demo"));
        // The 58.5 s pause plays as 3.
        assert_eq!(v2.events.iter().map(|e| e.t).collect::<Vec<f64>>(), vec![0.5, 1.5, 4.5]);
        assert_eq!(v2.duration, 4.5);
        assert_eq!(
            output_until(&v2, 1.5),
            Output {
                data: "$ ls\r\n".to_string(),
                count: 2
            }
        );
    }

    #[test]
    fn parse_cast_reads_version_3_with_times_since_the_last_event() {
        let v3 = parse_cast(
            "{\"version\": 3, \"term\": {\"cols\": 80, \"rows\": 24}, \"idle_time_limit\": 1}\n[0.2, \"o\", \"a\"]\n[5, \"o\", \"b\"]\n[0.3, \"r\", \"90x30\"]\n[0.1, \"o\", \"c\"]\n",
            DEFAULT_IDLE_SECONDS,
        )
        .expect("a version 3 recording parses");
        // Rounded to thousandths, as the check file does; the values are positive, so f64::round is Math.round.
        let rounded: Vec<(f64, &str)> = v3
            .events
            .iter()
            .map(|e| ((e.t * 1000.0).round() / 1000.0, e.data.as_str()))
            .collect();
        assert_eq!(rounded, vec![(0.2, "a"), (1.2, "b"), (1.6, "c")]);
    }

    #[test]
    fn parse_cast_rejects_what_is_not_a_recording() {
        assert_eq!(parse_cast("{\"version\": 1}", DEFAULT_IDLE_SECONDS), None);
        assert_eq!(parse_cast("plain text", DEFAULT_IDLE_SECONDS), None);
    }

    #[test]
    fn parse_cast_uses_the_default_size_and_idle_limit_when_the_header_has_none() {
        let bare = parse_cast(
            "{\"version\": 2}\n[0.0, \"o\", \"x\"]\n[9.0, \"o\", \"y\"]\n",
            DEFAULT_IDLE_SECONDS,
        )
        .expect("a header with only a version parses");
        assert_eq!((bare.cols, bare.rows, bare.title.as_str()), (80, 24, ""));
        // The second event's 9 s pause is cut to the default 3 s.
        assert_eq!(bare.duration, 3.0);
    }

    #[test]
    fn parse_cast_clamps_the_size_and_skips_lines_that_are_not_events() {
        // Sizes clamp to 1..=1000 columns and 1..=500 rows; CRLF line ends and comment lines are fine.
        let clamped = parse_cast(
            "{\"version\": 2, \"width\": 5000, \"height\": 0}\r\n# a comment\r\n[1.0, \"o\", \"hi\"]\r\n[2.0, \"x\"]\r\n",
            DEFAULT_IDLE_SECONDS,
        )
        .expect("a header with odd sizes parses");
        assert_eq!((clamped.cols, clamped.rows), (1000, 1));
        assert_eq!(clamped.events.len(), 1);
        assert_eq!(clamped.duration, 2.0);
    }

    #[test]
    fn clock_shows_minutes_and_two_digit_seconds() {
        assert_eq!(clock(65.4), "1:05");
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(600.0), "10:00");
    }

    #[test]
    fn clock_writes_huge_and_bad_values_as_javascript_does() {
        // 6e22 seconds is 1e21 minutes; JavaScript's String gives exponent form there, not every digit.
        assert_eq!(clock(6e22), "1e+21:00");
        assert_eq!(clock(-1.0), "-1:-1");
        assert_eq!(clock(f64::NAN), "NaN:NaN");
        assert_eq!(clock(-0.0), "0:00");
    }

    #[test]
    fn a_string_number_trims_javascript_white_space_only() {
        // Number("﻿50") is 50, and Number("\u0085") is NaN: U+FEFF is white space to JavaScript, U+0085 is not.
        assert_eq!(js_string_number("\u{feff}50"), 50.0);
        assert_eq!(js_string_number("\u{a0}7\u{3000}"), 7.0);
        assert!(js_string_number("\u{85}").is_nan());
        assert_eq!(js_string_number("  "), 0.0);
    }
}
