//! Dates as remotes write them (ISO 8601 in S3's listings, HTTP's in headers and WebDAV), and as SigV4
//! signs them. All UTC; in milliseconds since 1970.

use std::time::{SystemTime, UNIX_EPOCH};

/// Days since 1970-01-01 of a date (Howard Hinnant's days_from_civil).
fn days(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day since 1970-01-01 (civil_from_days).
fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// "20130524T000000Z": SigV4's x-amz-date.
pub fn amz_date(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

fn ms(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64, millis: i64) -> Option<i64> {
    let ok = (1..=12).contains(&month) && (1..=31).contains(&day) && (0..24).contains(&hour) && (0..60).contains(&minute) && (0..=60).contains(&second);
    ok.then(|| (((days(year, month, day) * 24 + hour) * 60 + minute) * 60 + second) * 1000 + millis)
}

/// "2024-07-14T10:30:05.250Z" (or with "+00:00", or no fraction).
pub fn iso_date(text: &str) -> Option<i64> {
    let t = text.trim();
    let n = |range: std::ops::Range<usize>| t.get(range)?.parse::<i64>().ok();
    let (year, month, day, hour, minute, second) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let rest = t.get(19..)?;
    let millis = match rest.strip_prefix('.') {
        Some(fraction) => {
            let digits: String = fraction.chars().take_while(char::is_ascii_digit).take(3).collect();
            format!("{digits:0<3}").parse().unwrap_or(0)
        }
        None => 0,
    };
    ms(year, month, day, hour, minute, second, millis)
}

/// "Wed, 23 Oct 2024 10:00:00 GMT" (RFC 7231's, which S3 and WebDAV send).
pub fn http_date(text: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let mut words = text.split_whitespace();
    let _weekday = words.next()?;
    let day: i64 = words.next()?.parse().ok()?;
    let month = MONTHS.iter().position(|m| words.clone().next().is_some_and(|w| w.to_ascii_lowercase().starts_with(m)))? as i64 + 1;
    words.next();
    let year: i64 = words.next()?.parse().ok()?;
    let mut clock = words.next()?.split(':').map(|p| p.parse::<i64>().ok());
    let (hour, minute, second) = (clock.next()??, clock.next()??, clock.next()??);
    ms(year, month, day, hour, minute, second, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn reads_and_writes_dates() {
        assert_eq!(amz_date(UNIX_EPOCH + Duration::from_secs(1_369_353_600)), "20130524T000000Z");
        assert_eq!(amz_date(UNIX_EPOCH + Duration::from_secs(951_782_400 + 3661)), "20000229T010101Z");
        assert_eq!(iso_date("2024-07-14T10:30:05.250Z"), Some(1_720_953_005_250));
        assert_eq!(iso_date("2024-07-14T10:30:05Z"), Some(1_720_953_005_000));
        assert_eq!(iso_date("1970-01-01T00:00:00.5Z"), Some(500));
        assert_eq!(http_date("Sun, 14 Jul 2024 10:30:05 GMT"), Some(1_720_953_005_000));
        assert_eq!(iso_date("yesterday"), None);
        assert_eq!(http_date("Sun, 14 Foo 2024 10:30:05 GMT"), None);
    }
}
