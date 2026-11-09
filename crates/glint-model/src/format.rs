//! Ported from src/lib/format.ts.
//!
//! Sizes, dates, times and durations as people read them, in their own locale.
//!
//! Numbers, dates and times come from Windows' own formatting (GetNumberFormatEx, GetDateFormatEx and
//! GetTimeFormatEx) for the user's locale, which is what Intl gave the WebView. The public functions use the
//! user's locale. The `_in` functions take a locale name such as "en-US" (or `None` for the user's locale), so
//! the tests do not depend on this PC's settings.
//!
//! Dates are local time, as in the TypeScript: "Today" and "Yesterday" are judged against the local midnight.
//! Those two words go through the i18n catalog for the given `Language`; the Arabic catalog has no entry for them
//! yet, so they stay English there until it does.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::i18n::{translate, Language};
use crate::js::{js_round, number_to_string, to_fixed};
use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Timelike};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::Globalization::{
    GetDateFormatEx, GetLocaleInfoEx, GetNumberFormatEx, GetTimeFormatEx, DATE_LONGDATE, ENUM_DATE_FORMATS_FLAGS,
    LOCALE_SDECIMAL, LOCALE_SGROUPING, LOCALE_SLONGDATE, LOCALE_STHOUSAND, NUMBERFMTW, TIME_FORMAT_FLAGS,
    TIME_NOSECONDS,
};

const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
const DAY: i64 = 86_400_000;
/// How many formatted dates the caches hold before they start over, as the TypeScript did.
const MAX_CACHE_SIZE: usize = 4096;

/// 1536 becomes "1.5 KB", and 12345678 becomes "12 MB". Plain bytes stay whole: "512 B".
// NOTE(port): the TypeScript comment says 12345678 gives "11.8 MB", but its code rounds any value of 10 or more
// to a whole number, so the code (and this port) gives "12 MB". The comment is the wrong part.
pub fn format_size(bytes: u64) -> String {
    format_size_in(bytes, None)
}

fn format_size_in(bytes: u64, locale: Option<&str>) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let number = if value < 10.0 {
        to_fixed(value, 1)
    } else {
        // Math.round for positive values: halves go up.
        group_digits(&format!("{}", js_round(value)), locale)
    };
    format!("{number} {}", UNITS[unit])
}

/// A count in the user's own grouping: 1234 becomes "1,234" in en-US.
pub fn format_count(n: usize) -> String {
    format_count_in(n, None)
}

fn format_count_in(n: usize, locale: Option<&str>) -> String {
    group_digits(&n.to_string(), locale)
}

/// 1 becomes "1 item", 3 becomes "3 items".
pub fn plural(n: usize, word: &str) -> String {
    plural_in(n, word, None)
}

fn plural_in(n: usize, word: &str, locale: Option<&str>) -> String {
    format!("{} {word}{}", format_count_in(n, locale), if n == 1 { "" } else { "s" })
}

/// 0.42 becomes "0.4 ms", 12.3 becomes "12 ms", 1234 becomes "1.23 s".
pub fn format_duration(ms: f64) -> String {
    if ms < 10.0 {
        format!("{} ms", to_fixed(ms, 1))
    } else if ms < 1000.0 {
        format!("{} ms", number_to_string(js_round(ms)))
    } else {
        format!("{} s", to_fixed(ms / 1000.0, 2))
    }
}

/// "Today 9:31 PM", "Yesterday 8:02 AM", otherwise "Oct 3, 2026 4:15 PM".
///
/// `ms` is Unix milliseconds, and 0 means no date at all, which gives an empty string. `now` is the time to
/// judge "today" against; `None` means the clock, and then the labels are cached as the TypeScript cached them
/// (per language, since "Today" is a word). `language` is the interface language for "Today" and "Yesterday".
pub fn format_date(ms: i64, now: Option<i64>, language: Language) -> String {
    format_date_in(ms, now, None, language)
}

fn format_date_in(ms: i64, now: Option<i64>, locale: Option<&str>, language: Language) -> String {
    if ms == 0 {
        return String::new();
    }
    match now {
        None => with_cache(|cache| {
            let today = today_start_cached(cache, now_millis());
            if let Some(hit) = cache.short.get(&(ms, language)) {
                return hit.clone();
            }
            let formatted = format_date_with_today(ms, today, locale, language);
            if cache.short.len() >= MAX_CACHE_SIZE {
                cache.short.clear();
            }
            cache.short.insert((ms, language), formatted.clone());
            formatted
        }),
        Some(now) => format_date_with_today(ms, local_midnight(now), locale, language),
    }
}

/// The long form with the date and the time in full, for tooltips: "Saturday, October 3, 2026 4:15:00 PM".
pub fn format_full_date(ms: i64) -> String {
    format_full_date_in(ms, None)
}

fn format_full_date_in(ms: i64, locale: Option<&str>) -> String {
    if ms == 0 {
        return String::new();
    }
    with_cache(|cache| {
        if let Some(hit) = cache.full.get(&ms) {
            return hit.clone();
        }
        let formatted = format_full(ms, locale);
        if cache.full.len() >= MAX_CACHE_SIZE {
            cache.full.clear();
        }
        cache.full.insert(ms, formatted.clone());
        formatted
    })
}

/// The day's start and the labels built for it, as the TypeScript kept them in module variables.
#[derive(Default)]
struct DateCache {
    today_start: i64,
    today_end: i64,
    short: HashMap<(i64, Language), String>,
    full: HashMap<i64, String>,
}

/// The one cache for the whole process. Mutex::new is const, so it needs no lazy setup.
static DATE_CACHE: Mutex<Option<DateCache>> = Mutex::new(None);

fn with_cache<R>(f: impl FnOnce(&mut DateCache) -> R) -> R {
    // A poisoned lock only means another thread panicked mid-update; the cache is still usable.
    let mut guard = DATE_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    f(guard.get_or_insert_with(DateCache::default))
}

/// The start of the local day that `now` falls in. The cache is kept for that day; when `now` leaves it,
/// the short labels built for the old day are dropped, since "Today" has moved on.
fn today_start_cached(cache: &mut DateCache, now: i64) -> i64 {
    if now >= cache.today_start && now < cache.today_end {
        return cache.today_start;
    }
    let start = local_midnight(now);
    cache.today_start = start;
    cache.today_end = start + DAY;
    cache.short.clear();
    start
}

fn format_date_with_today(ms: i64, today: i64, locale: Option<&str>, language: Language) -> String {
    let Some(date) = local_datetime(ms) else {
        return String::new();
    };
    let time = format_time(&date, locale, TIME_NOSECONDS);
    if ms >= today && ms < today + DAY {
        format!("{} {time}", translate(language, "Today"))
    } else if ms >= today - DAY && ms < today {
        format!("{} {time}", translate(language, "Yesterday"))
    } else {
        format!("{} {time}", format_short_date(&date, locale))
    }
}

fn format_full(ms: i64, locale: Option<&str>) -> String {
    let Some(date) = local_datetime(ms) else {
        return String::new();
    };
    // NOTE(port): Intl's full style joins date and time with a locale word ("at" in en-US). Windows has no such
    // word, so they are joined with a space.
    format!(
        "{} {}",
        format_long_date(&date, locale),
        format_time(&date, locale, TIME_FORMAT_FLAGS::default())
    )
}

/// The local time for Unix milliseconds, or None when they are out of range.
fn local_datetime(ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}

/// Unix milliseconds of local midnight at the start of the day `ms` falls in. Where a daylight-saving change
/// skips midnight, this uses 1 AM, which is what JavaScript's setHours(0) does too.
fn local_midnight(ms: i64) -> i64 {
    let Some(date) = local_datetime(ms).map(|d| d.date_naive()) else {
        return ms - ms.rem_euclid(DAY);
    };
    for hour in [0, 1] {
        if let Some(start) = local_start_of(date, hour) {
            return start;
        }
    }
    ms - ms.rem_euclid(DAY)
}

fn local_start_of(date: NaiveDate, hour: u32) -> Option<i64> {
    let naive = date.and_hms_opt(hour, 0, 0)?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.timestamp_millis())
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// A wide NUL-terminated copy of `text`, for the Win32 calls.
fn nul_terminated(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A locale name as the Win32 calls take it: a NUL-terminated buffer, or NULL for the user's own locale.
struct LocaleName(Option<Vec<u16>>);

impl LocaleName {
    fn new(locale: Option<&str>) -> Self {
        Self(locale.map(nul_terminated))
    }

    fn ptr(&self) -> PCWSTR {
        match &self.0 {
            Some(name) => PCWSTR(name.as_ptr()),
            None => PCWSTR(std::ptr::null()),
        }
    }
}

/// Runs a Win32 call that writes text into a buffer. The first call asks for the length, the second fills the
/// buffer. Returns None when Windows writes nothing, so the caller can fall back.
fn read_wide(call: impl Fn(Option<&mut [u16]>) -> i32) -> Option<String> {
    let needed = usize::try_from(call(None)).ok()?;
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u16; needed];
    let written = usize::try_from(call(Some(buf.as_mut_slice()))).ok()?;
    // The count includes the terminating NUL, which is not part of the text.
    let len = written.min(buf.len()).saturating_sub(1);
    String::from_utf16(&buf[..len]).ok()
}

/// One locale information string, such as the decimal separator (`lctype` is a LOCALE_* constant).
fn locale_string(name: &LocaleName, lctype: u32) -> Option<String> {
    read_wide(|buf| {
        // SAFETY: the locale name is NULL or a NUL-terminated buffer that `name` keeps alive, and `buf` is either
        // None or a writable slice whose length is passed along, so the call stays inside that memory.
        unsafe { GetLocaleInfoEx(name.ptr(), lctype, buf) }
    })
}

/// Windows' digit grouping string ("3;2;0", as the locale gives it) in NUMBERFMT's own form: "32". The sizes of
/// the groups from the right, with the 0 that ends the list left out. An empty or broken string means groups of
/// three, which is what nearly every locale uses.
fn grouping_code(spec: &str) -> u32 {
    let mut code = 0u32;
    for part in spec.split(';') {
        match part.trim().parse::<u32>() {
            Ok(0) | Err(_) => break,
            Ok(size @ 1..=9) => code = code.saturating_mul(10).saturating_add(size),
            Ok(_) => break,
        }
    }
    if code == 0 {
        3
    } else {
        code
    }
}

/// Groups the digits of a whole number the way the locale does: "1234" becomes "1,234" in en-US. `digits` is
/// plain ASCII digits. If Windows cannot format it, the plain digits are returned.
fn group_digits(digits: &str, locale: Option<&str>) -> String {
    let name = LocaleName::new(locale);
    let decimal = locale_string(&name, LOCALE_SDECIMAL).unwrap_or_else(|| ".".to_owned());
    let thousand = locale_string(&name, LOCALE_STHOUSAND).unwrap_or_else(|| ",".to_owned());
    let grouping = grouping_code(&locale_string(&name, LOCALE_SGROUPING).unwrap_or_default());
    let mut decimal_wide = nul_terminated(&decimal);
    let mut thousand_wide = nul_terminated(&thousand);
    // No digits after the point: a whole number. Windows' own default for the locale would add ".00".
    let number_format = NUMBERFMTW {
        NumDigits: 0,
        LeadingZero: 0,
        Grouping: grouping,
        lpDecimalSep: PWSTR(decimal_wide.as_mut_ptr()),
        lpThousandSep: PWSTR(thousand_wide.as_mut_ptr()),
        NegativeOrder: 0,
    };
    let value = nul_terminated(digits);
    read_wide(|buf| {
        // SAFETY: `number_format` points at the two separator buffers, which live until the call returns. `value` is a
        // NUL-terminated copy of the digits. The name and buffer rules are as in `locale_string`.
        unsafe {
            GetNumberFormatEx(
                name.ptr(),
                0,
                PCWSTR(value.as_ptr()),
                Some(&number_format as *const NUMBERFMTW),
                buf,
            )
        }
    })
    .unwrap_or_else(|| digits.to_owned())
}

/// The local date and time, as the SYSTEMTIME that Windows formats.
fn system_time(date: &DateTime<Local>) -> SYSTEMTIME {
    // Each field fits in u16 for any real date. try_from keeps an absurd one from wrapping around.
    SYSTEMTIME {
        wYear: u16::try_from(date.year()).unwrap_or(0),
        wMonth: u16::try_from(date.month()).unwrap_or(0),
        wDayOfWeek: u16::try_from(date.weekday().num_days_from_sunday()).unwrap_or(0),
        wDay: u16::try_from(date.day()).unwrap_or(0),
        wHour: u16::try_from(date.hour()).unwrap_or(0),
        wMinute: u16::try_from(date.minute()).unwrap_or(0),
        wSecond: u16::try_from(date.second()).unwrap_or(0),
        wMilliseconds: 0,
    }
}

/// The locale's own short date: its long-date picture without the weekday and with the month name shortened,
/// so en-US gives "Oct 3, 2026" and en-GB "3 Oct 2026". If Windows has no long-date picture, "MMM d, yyyy".
fn format_short_date(date: &DateTime<Local>, locale: Option<&str>) -> String {
    let name = LocaleName::new(locale);
    let st = system_time(date);
    let picture = locale_string(&name, LOCALE_SLONGDATE)
        .map(|long| short_picture(&long))
        .filter(|picture| !picture.is_empty())
        .unwrap_or_else(|| "MMM d, yyyy".to_owned());
    let picture = nul_terminated(&picture);
    read_wide(|buf| {
        // SAFETY: `st` lives across the call, and `picture` is a NUL-terminated picture (flags 0 are required with
        // a picture). The calendar is NULL for the default one. `buf` is as in `read_wide`.
        unsafe {
            GetDateFormatEx(
                name.ptr(),
                ENUM_DATE_FORMATS_FLAGS(0),
                Some(&st as *const SYSTEMTIME),
                PCWSTR(picture.as_ptr()),
                buf,
                PCWSTR(std::ptr::null()),
            )
        }
    })
    .unwrap_or_else(|| format!("{}-{:02}-{:02}", date.year(), date.month(), date.day()))
}

/// Turns a long-date picture ("dddd, MMMM d, yyyy") into the short one ("MMM d, yyyy"): the weekday (a run of
/// three or more `d`) goes with the separator that belongs to it, and a full month name (`MMMM`) becomes `MMM`.
/// Text in single quotes is literal and left alone.
fn short_picture(long: &str) -> String {
    let mut chars: Vec<char> = long.chars().collect();
    // A pattern letter stands for something; anything else (and quotes) is a separator or literal text.
    let is_separator = |c: char| !c.is_alphabetic() && c != '\'';
    // Weekday: find the first run of d's outside quotes that is three or longer.
    let mut in_quote = false;
    let mut weekday = None;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\'' {
            in_quote = !in_quote;
        } else if !in_quote && chars[i] == 'd' {
            let mut end = i;
            while end < chars.len() && chars[end] == 'd' {
                end += 1;
            }
            if end - i >= 3 {
                weekday = Some((i, end));
                break;
            }
            i = end;
            continue;
        }
        i += 1;
    }
    if let Some((start, end)) = weekday {
        let mut after = end;
        while after < chars.len() && is_separator(chars[after]) {
            after += 1;
        }
        if after < chars.len() {
            chars.drain(start..after);
        } else {
            // The weekday ends the picture: drop the separator before it instead.
            let mut before = start;
            while before > 0 && is_separator(chars[before - 1]) {
                before -= 1;
            }
            chars.truncate(before);
        }
    }
    // Month: a run of four or more M's outside quotes becomes MMM.
    let mut out = String::new();
    let mut in_quote = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            in_quote = !in_quote;
        } else if !in_quote && c == 'M' {
            let mut end = i;
            while end < chars.len() && chars[end] == 'M' {
                end += 1;
            }
            out.push_str(&"M".repeat((end - i).min(3)));
            i = end;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Saturday, October 3, 2026: the long date, as Windows writes it.
fn format_long_date(date: &DateTime<Local>, locale: Option<&str>) -> String {
    let name = LocaleName::new(locale);
    let st = system_time(date);
    read_wide(|buf| {
        // SAFETY: `st` lives across the call. The format picture and calendar are NULL, for Windows' long date.
        unsafe {
            GetDateFormatEx(
                name.ptr(),
                DATE_LONGDATE,
                Some(&st as *const SYSTEMTIME),
                PCWSTR(std::ptr::null()),
                buf,
                PCWSTR(std::ptr::null()),
            )
        }
    })
    .unwrap_or_else(|| format!("{}-{:02}-{:02}", date.year(), date.month(), date.day()))
}

/// The time of day, as Windows writes it: "9:31 PM", or with seconds when `flags` does not drop them.
fn format_time(date: &DateTime<Local>, locale: Option<&str>, flags: TIME_FORMAT_FLAGS) -> String {
    let name = LocaleName::new(locale);
    let st = system_time(date);
    read_wide(|buf| {
        // SAFETY: `st` lives across the call, and the format picture is NULL for Windows' own time format.
        unsafe {
            GetTimeFormatEx(
                name.ptr(),
                flags,
                Some(&st as *const SYSTEMTIME),
                PCWSTR(std::ptr::null()),
                buf,
            )
        }
    })
    .unwrap_or_else(|| format!("{}:{:02}", date.hour(), date.minute()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Local noon on 14 October 2026, as Unix milliseconds. The TypeScript test used the same local time.
    fn noon_oct_14_2026() -> i64 {
        Local
            .with_ymd_and_hms(2026, 10, 14, 12, 0, 0)
            .single()
            .map(|d| d.timestamp_millis())
            .expect("noon exists on 14 October 2026 in every zone")
    }

    fn local_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .map(|d| d.timestamp_millis())
            .expect("the test time exists in the local zone")
    }

    #[test]
    fn sizes_read_in_units_of_1024() {
        assert_eq!(format_size_in(0, Some("en-US")), "0 B");
        assert_eq!(format_size_in(1536, Some("en-US")), "1.5 KB");
        assert_eq!(format_size_in(5 * 1024u64.pow(3), Some("en-US")), "5.0 GB");
        assert_eq!(format_size_in(245_127_203_117, Some("en-US")), "228 GB");
    }

    #[test]
    fn sizes_round_to_whole_units_from_ten_up() {
        // 1280 bytes is 1.25 KB exactly. JavaScript's toFixed rounds that tie up, to "1.3".
        assert_eq!(format_size_in(1280, Some("en-US")), "1.3 KB");
        assert_eq!(format_size_in(1023, Some("en-US")), "1023 B");
        assert_eq!(format_size_in(12_345_678, Some("en-US")), "12 MB");
        // Ten or more units print whole, with the locale's grouping.
        assert_eq!(format_size_in(2000u64 * 1024u64.pow(4), Some("en-US")), "2.0 PB");
        assert_eq!(format_size_in(1023u64 * 1024u64.pow(5), Some("en-US")), "1,023 PB");
    }

    #[test]
    fn durations_switch_units_at_ten_and_a_thousand() {
        assert_eq!(format_duration(0.42), "0.4 ms");
        assert_eq!(format_duration(1234.0), "1.23 s");
        assert_eq!(format_duration(12.3), "12 ms");
        // A tie at 12.5 ms rounds up, as Math.round does.
        assert_eq!(format_duration(12.5), "13 ms");
    }

    #[test]
    fn dates_say_today_and_yesterday_against_the_given_now() {
        let noon = noon_oct_14_2026();
        assert!(format_date_in(noon - 3_600_000, Some(noon), Some("en-US"), Language::En).starts_with("Today "));
        assert!(format_date_in(noon - 86_400_000, Some(noon), Some("en-US"), Language::En).starts_with("Yesterday "));
    }

    #[test]
    fn older_dates_show_the_day_and_the_year() {
        let noon = noon_oct_14_2026();
        let january = local_ms(2026, 1, 2, 9, 5);
        let text = format_date_in(january, Some(noon), Some("en-US"), Language::En);
        assert!(text.contains("2026"), "got {text:?}");
        assert!(text.starts_with("Jan 2, 2026 "), "got {text:?}");
    }

    #[test]
    fn sizes_decide_on_the_exact_value_not_a_false_tie() {
        // This many bytes is a hair under 2.05 PB. Its value times ten lands on exactly 20.5 in floating point, so
        // rounding by "times ten, add a half, floor" gave "2.1 PB"; JavaScript's toFixed gives "2.0".
        assert_eq!(format_size_in(2_308_094_809_027_379, Some("en-US")), "2.0 PB");
        assert_eq!(format_size_in(2_871_044_762_448_691, Some("en-US")), "2.5 PB");
        // The same goes for durations: 0.15 is stored below 0.15, so it prints "0.1 ms".
        assert_eq!(format_duration(0.15), "0.1 ms");
    }

    #[test]
    fn short_pictures_drop_the_weekday_and_shorten_the_month() {
        assert_eq!(short_picture("dddd, MMMM d, yyyy"), "MMM d, yyyy");
        assert_eq!(short_picture("dddd, d MMMM yyyy"), "d MMM yyyy");
        assert_eq!(short_picture("dddd, d. MMMM yyyy"), "d. MMM yyyy");
        assert_eq!(short_picture("dddd d 'de' MMMM 'de' yyyy"), "d 'de' MMM 'de' yyyy");
        // The weekday at the end takes the separator before it.
        assert_eq!(short_picture("d MMMM yyyy, dddd"), "d MMM yyyy");
        // Quoted text is literal: a quoted "dddd" and "MMMM" stay.
        assert_eq!(short_picture("'dddd' MMMM d"), "'dddd' MMM d");
        assert_eq!(short_picture("yyyy MMM d"), "yyyy MMM d");
        assert_eq!(short_picture(""), "");
    }

    #[test]
    fn short_dates_follow_the_locales_order() {
        let noon = noon_oct_14_2026();
        let january = local_ms(2026, 1, 2, 9, 5);
        let us = format_date_in(january, Some(noon), Some("en-US"), Language::En);
        assert!(us.starts_with("Jan 2, 2026 "), "got {us:?}");
        let gb = format_date_in(january, Some(noon), Some("en-GB"), Language::En);
        // Windows writes the day as "02" here, older data as "2"; either way the day comes first.
        assert!(
            gb.starts_with("02 Jan 2026 ") || gb.starts_with("2 Jan 2026 "),
            "got {gb:?}"
        );
        // German writes the day first, with a dot after it.
        let de = format_date_in(january, Some(noon), Some("de-DE"), Language::En);
        assert!(de.starts_with("2. "), "got {de:?}");
        assert!(de.contains("Jan") && de.contains("2026"), "got {de:?}");
        // Arabic (Saudi Arabia) may use another calendar and other digits; it must give a date and not panic.
        let ar = format_date_in(january, Some(noon), Some("ar-SA"), Language::Ar);
        assert!(ar.chars().count() > 6, "got {ar:?}");
        assert!(!ar.starts_with("Today"), "got {ar:?}");
    }

    #[test]
    fn today_and_yesterday_go_through_the_catalog() {
        let noon = noon_oct_14_2026();
        for language in [Language::En, Language::Ar] {
            let today = translate(language, "Today").to_owned();
            let yesterday = translate(language, "Yesterday").to_owned();
            let a = format_date_in(noon - 3_600_000, Some(noon), Some("en-US"), language);
            let b = format_date_in(noon - 86_400_000, Some(noon), Some("en-US"), language);
            assert!(a.starts_with(&format!("{today} ")), "got {a:?}");
            assert!(b.starts_with(&format!("{yesterday} ")), "got {b:?}");
        }
        // The Arabic catalog has no entry for these words yet, so they stay English.
        assert_eq!(translate(Language::Ar, "Today"), "Today");
        assert_eq!(translate(Language::Ar, "Yesterday"), "Yesterday");
    }

    #[test]
    fn a_zero_date_is_empty() {
        assert_eq!(format_date(0, None, Language::En), "");
        assert_eq!(format_full_date(0), "");
    }

    #[test]
    fn counts_are_grouped_and_plural_for_anything_but_one() {
        assert_eq!(format_count_in(1234, Some("en-US")), "1,234");
        assert_eq!(plural_in(1, "item", Some("en-US")), "1 item");
        assert_eq!(plural_in(0, "item", Some("en-US")), "0 items");
        assert_eq!(plural_in(3, "item", Some("en-US")), "3 items");
    }

    #[test]
    fn grouping_strings_become_numfmt_codes() {
        assert_eq!(grouping_code("3;0"), 3);
        assert_eq!(grouping_code("3;2;0"), 32);
        assert_eq!(grouping_code(""), 3);
    }

    #[test]
    fn to_fixed_rounds_ties_up_like_javascript() {
        assert_eq!(to_fixed(1.25, 1), "1.3");
        assert_eq!(to_fixed(0.05, 1), "0.1");
        assert_eq!(to_fixed(-0.04, 1), "-0.0");
        assert_eq!(to_fixed(f64::NAN, 1), "NaN");
    }
}
