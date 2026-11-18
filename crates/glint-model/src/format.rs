//! Ported from src/lib/format.ts.
//!
//! Sizes, dates, times and durations as people read them, in their own locale.
//!
//! Dates and times come from Windows' own formatting (GetDateFormatEx and GetTimeFormatEx) for the user's
//! locale, which is what Intl gave the WebView. Whole numbers are grouped here, with the separator and the
//! grouping Windows gives the locale: they are drawn for every row of every frame, and asking Windows each time
//! cost eight calls into it. The public functions use the user's locale. The `_in` functions take a locale name
//! such as "en-US" (or `None` for the user's locale), so the tests do not depend on this PC's settings.
//!
//! Dates are local time, as in the TypeScript: "Today" and "Yesterday" are judged against the local midnight.
//! Those two words go through the i18n catalog for the given `Language`; the Arabic catalog has no entry for them
//! yet, so they stay English there until it does.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::i18n::{translate, Language};
use crate::js::{js_round, number_to_string, to_fixed};
use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Timelike};
use windows::core::PCWSTR;
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::Globalization::{
    GetDateFormatEx, GetLocaleInfoEx, GetTimeFormatEx, DATE_LONGDATE, ENUM_DATE_FORMATS_FLAGS, LOCALE_SGROUPING,
    LOCALE_SLONGDATE, LOCALE_STHOUSAND, TIME_FORMAT_FLAGS, TIME_NOSECONDS,
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

// The cells of every visible row and tile ask for this on every frame, so it costs one allocation (the text) and
// no call into Windows: the TypeScript's toFixed and Math.round are worked out on integers, and the digits are
// grouped from the locale's style, read once.
fn format_size_in(bytes: u64, locale: Option<&str>) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    // The size as the TypeScript held it, a float (whole, and exact below 2^53). Its value in a unit is that over
    // a power of two, which a float divides exactly, so integers give the same answers without rounding errors.
    let whole = bytes as f64 as u128;
    let mut unit = 1;
    while unit < UNITS.len() - 1 && whole >> (10 * (unit + 1)) != 0 {
        unit += 1;
    }
    let shift = 10 * unit as u32;
    let half = 1u128 << (shift - 1);
    let mut text = String::with_capacity(16);
    if whole < 10 << shift {
        // toFixed(1): the tenths closest to the exact value, the larger of two as close.
        let tenths = u64::try_from((whole * 10 + half) >> shift).unwrap_or(u64::MAX);
        let _ = write!(text, "{}.{}", tenths / 10, tenths % 10);
    } else {
        // Math.round for positive values: halves go up.
        let rounded = u64::try_from((whole + half) >> shift).unwrap_or(u64::MAX);
        let mut digits = [0u8; 20];
        push_grouped(&mut text, ascii_digits(rounded, &mut digits), locale);
    }
    text.push(' ');
    text.push_str(UNITS[unit]);
    text
}

/// A count in the user's own grouping: 1234 becomes "1,234" in en-US.
pub fn format_count(n: usize) -> String {
    format_count_in(n, None)
}

fn format_count_in(n: usize, locale: Option<&str>) -> String {
    let mut digits = [0u8; 20];
    group_digits(ascii_digits(n as u64, &mut digits), locale)
}

/// `n` in plain ASCII digits, written into `buf` (20 digits hold any u64).
fn ascii_digits(mut n: u64, buf: &mut [u8; 20]) -> &str {
    let mut at = buf.len();
    loop {
        at -= 1;
        buf[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    // Only ASCII digits were written.
    std::str::from_utf8(&buf[at..]).unwrap_or_default()
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
/// the groups from the right, with the 0 that ends the list left out; the last size goes on repeating. A list
/// that starts with 0 is no grouping at all (Region settings' "123456789"), which is 0. An empty or broken
/// string means groups of three, which is what nearly every locale uses.
fn grouping_code(spec: &str) -> u32 {
    let mut code = 0u32;
    for (i, part) in spec.split(';').enumerate() {
        match part.trim().parse::<u32>() {
            Ok(0) if i == 0 => return 0,
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

/// How a locale groups the digits of whole numbers: the separator between groups, and the sizes of the groups
/// as `grouping_code` gives them.
struct DigitGroups {
    separator: String,
    grouping: u32,
}

impl DigitGroups {
    fn read(locale: Option<&str>) -> DigitGroups {
        let name = LocaleName::new(locale);
        DigitGroups {
            separator: locale_string(&name, LOCALE_STHOUSAND).unwrap_or_else(|| ",".to_owned()),
            grouping: grouping_code(&locale_string(&name, LOCALE_SGROUPING).unwrap_or_default()),
        }
    }

    /// The user's own, read once a run: a change to Windows' region settings shows from Glint's next start.
    fn user() -> &'static DigitGroups {
        static USER: OnceLock<DigitGroups> = OnceLock::new();
        USER.get_or_init(|| DigitGroups::read(None))
    }

    /// Writes `digits` (plain ASCII digits) onto `out` with the separator between groups, as NUMBERFMT groups
    /// them: the first size is the group nearest the end, each size after it the next group left, and the last
    /// size repeats. A number no longer than the first group (any under 1,000, in every locale) is copied as is.
    fn push(&self, out: &mut String, digits: &str) {
        let mut sizes = [0usize; 10];
        let mut count = 0;
        let mut rest = self.grouping;
        while rest > 0 && count < sizes.len() {
            sizes[count] = (rest % 10) as usize;
            rest /= 10;
            count += 1;
        }
        // The code's first digit is the group nearest the end.
        sizes[..count].reverse();
        // Where separators go, counted from the start, the last one first.
        let mut cuts = [0usize; 40];
        let mut cut_count = 0;
        let mut left = digits.len();
        while count > 0 && cut_count < cuts.len() {
            let size = sizes[cut_count.min(count - 1)];
            if size == 0 || left <= size {
                break;
            }
            left -= size;
            cuts[cut_count] = left;
            cut_count += 1;
        }
        let mut from = 0;
        for &cut in cuts[..cut_count].iter().rev() {
            out.push_str(&digits[from..cut]);
            out.push_str(&self.separator);
            from = cut;
        }
        out.push_str(&digits[from..]);
    }
}

/// Writes the digits of a whole number onto `out`, grouped the way the locale does.
fn push_grouped(out: &mut String, digits: &str, locale: Option<&str>) {
    match locale {
        None => DigitGroups::user().push(out, digits),
        Some(_) => DigitGroups::read(locale).push(out, digits),
    }
}

/// Groups the digits of a whole number the way the locale does: "1234" becomes "1,234" in en-US. `digits` is
/// plain ASCII digits.
fn group_digits(digits: &str, locale: Option<&str>) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / 2 + 4);
    push_grouped(&mut out, digits, locale);
    out
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
        let ar = format_date_in(january, Some(noon), Some("ar-SA"), Language::En);
        assert!(ar.chars().count() > 6, "got {ar:?}");
        assert!(!ar.starts_with("Today"), "got {ar:?}");
    }

    #[test]
    fn today_and_yesterday_go_through_the_catalog() {
        let noon = noon_oct_14_2026();
        let language = Language::En;
        let today = translate(language, "Today").to_owned();
        let yesterday = translate(language, "Yesterday").to_owned();
        let a = format_date_in(noon - 3_600_000, Some(noon), Some("en-US"), language);
        let b = format_date_in(noon - 86_400_000, Some(noon), Some("en-US"), language);
        assert!(a.starts_with(&format!("{today} ")), "got {a:?}");
        assert!(b.starts_with(&format!("{yesterday} ")), "got {b:?}");
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
        // Region settings' "123456789": no grouping.
        assert_eq!(grouping_code("0"), 0);
        assert_eq!(grouping_code("0;0"), 0);
    }

    #[test]
    fn groups_follow_their_sizes_from_the_right() {
        let grouped = |grouping, digits: &str| {
            let mut out = String::new();
            DigitGroups {
                separator: ",".into(),
                grouping,
            }
            .push(&mut out, digits);
            out
        };
        assert_eq!(grouped(3, "1234567"), "1,234,567");
        assert_eq!(grouped(3, "123456"), "123,456");
        assert_eq!(grouped(32, "1234567"), "12,34,567");
        assert_eq!(grouped(32, "123456789"), "12,34,56,789");
        assert_eq!(grouped(0, "1234567"), "1234567");
        assert_eq!(grouped(3, "999"), "999");
        assert_eq!(grouped(3, "0"), "0");
    }

    /// What Windows' GetNumberFormatEx makes of `digits` with the locale's separator and grouping, as Glint asked
    /// it until the grouping was done here: what `DigitGroups::push` is checked against.
    fn windows_grouped(digits: &str, locale: Option<&str>) -> String {
        use windows::core::PWSTR;
        use windows::Win32::Globalization::{GetNumberFormatEx, NUMBERFMTW};
        let groups = DigitGroups::read(locale);
        let name = LocaleName::new(locale);
        let mut decimal = nul_terminated(".");
        let mut separator = nul_terminated(&groups.separator);
        let number_format = NUMBERFMTW {
            NumDigits: 0,
            LeadingZero: 0,
            Grouping: groups.grouping,
            lpDecimalSep: PWSTR(decimal.as_mut_ptr()),
            lpThousandSep: PWSTR(separator.as_mut_ptr()),
            NegativeOrder: 0,
        };
        let value = nul_terminated(digits);
        read_wide(|buf| {
            // SAFETY: `number_format` points at the two separator buffers, which live until the call returns, and
            // `value` is a NUL-terminated copy of the digits. The name and buffer rules are as in `locale_string`.
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
        .expect("Windows formats a whole number")
    }

    #[test]
    fn digits_are_grouped_as_windows_groups_them() {
        for locale in ["en-US", "de-DE", "fr-FR", "de-CH", "hi-IN", "ar-SA", "ja-JP"] {
            for digits in [
                "0",
                "7",
                "42",
                "999",
                "1000",
                "1234",
                "12345",
                "123456",
                "1234567",
                "12345678901",
                "18446744073709551615",
            ] {
                let mut ours = String::new();
                DigitGroups::read(Some(locale)).push(&mut ours, digits);
                assert_eq!(ours, windows_grouped(digits, Some(locale)), "{digits} in {locale}");
            }
        }
    }

    /// What a size costs to write, as every visible row and tile asks for one on every frame, against the way it
    /// was written before (toFixed's exact expansion, and the locale read and the digits grouped by Windows on
    /// every call). Run with --ignored --nocapture, in release.
    #[test]
    #[ignore]
    fn sizes_cost_little() {
        // Sizes spread over every unit, as a folder's files are.
        let sizes: Vec<u64> = (0..200_000u64)
            .map(|i| i.wrapping_mul(2_654_435_761) % (1u64 << (10 + i % 34)))
            .collect();
        let before = |bytes: u64| {
            if bytes < 1024 {
                return format!("{bytes} B");
            }
            let (mut value, mut unit) = (bytes as f64, 0);
            while value >= 1024.0 && unit < UNITS.len() - 1 {
                value /= 1024.0;
                unit += 1;
            }
            let number = if value < 10.0 {
                to_fixed(value, 1)
            } else {
                windows_grouped(&format!("{}", js_round(value)), None)
            };
            format!("{number} {}", UNITS[unit])
        };
        let time = |write: &dyn Fn(u64) -> String| {
            let started = std::time::Instant::now();
            let length: usize = sizes.iter().map(|&bytes| write(bytes).len()).sum();
            (started.elapsed().as_nanos() as f64 / sizes.len() as f64, length)
        };
        assert!(
            sizes.iter().all(|&bytes| format_size(bytes) == before(bytes)),
            "the same text"
        );
        let (now, _) = time(&format_size);
        let (then, _) = time(&before);
        println!("a size: {now:.0} ns now, {then:.0} ns before ({:.0}x)", then / now);
    }

    #[test]
    fn sizes_round_exactly_at_the_edges() {
        // Just under ten units rounds up to "10.0", as toFixed did; ten units exactly is whole.
        assert_eq!(format_size_in(10_239, Some("en-US")), "10.0 KB");
        assert_eq!(format_size_in(10_240, Some("en-US")), "10 KB");
        assert_eq!(format_size_in(1_048_575, Some("en-US")), "1,024 KB");
        assert_eq!(format_size_in(1_048_576, Some("en-US")), "1.0 MB");
        assert_eq!(format_size_in(u64::MAX, Some("en-US")), "16,384 PB");
        assert_eq!(format_size_in(1023u64 * 1024u64.pow(5), Some("de-DE")), "1.023 PB");
    }
}
