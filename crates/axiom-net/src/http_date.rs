//! HTTP-date parsing / formatting (RFC 9110 §5.6.7).
//!
//! Accepts the preferred IMF-fixdate plus the obsolete RFC 850 and asctime forms.
//! Values are Unix seconds.

use std::time::{SystemTime, UNIX_EPOCH};

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn parse_http_date(s: &str) -> Option<i64> {
    let toks: Vec<&str> = s
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .collect();
    let (day, month, year, time) = match toks.as_slice() {
        // Sun, 06 Nov 1994 08:49:37 GMT
        [_, day, mon, year, time, gmt] if gmt.eq_ignore_ascii_case("GMT") => {
            (parse_num(day)?, month_index(mon)?, parse_num(year)?, *time)
        }
        // Sunday, 06-Nov-94 08:49:37 GMT
        [_, date, time, gmt] if gmt.eq_ignore_ascii_case("GMT") => {
            let mut parts = date.split('-');
            let day = parse_num(parts.next()?)?;
            let mon = month_index(parts.next()?)?;
            let yy = parse_num(parts.next()?)?;
            if parts.next().is_some() {
                return None;
            }
            let year = if yy >= 100 {
                yy
            } else if yy >= 70 {
                1900 + yy
            } else {
                2000 + yy
            };
            (day, mon, year, *time)
        }
        // Sun Nov  6 08:49:37 1994
        [_, mon, day, time, year] => (parse_num(day)?, month_index(mon)?, parse_num(year)?, *time),
        _ => return None,
    };
    let mut hms = time.split(':');
    let h = parse_num(hms.next()?)?;
    let m = parse_num(hms.next()?)?;
    let sec = parse_num(hms.next()?)?;
    if hms.next().is_some() || h > 23 || m > 59 || sec > 60 {
        return None;
    }
    if !(1..=31).contains(&day) || year < 1900 {
        return None;
    }
    let days = days_from_civil(year, month + 1, day);
    Some(days * 86_400 + h * 3600 + m * 60 + sec)
}

pub fn format_http_date(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (y, mo, d) = civil_from_days(days);
    let wd = WEEKDAYS[days.rem_euclid(7) as usize];
    let mon = MONTHS[(mo - 1) as usize];
    let mon = format!("{}{}", mon[..1].to_ascii_uppercase(), &mon[1..]);
    format!(
        "{wd}, {d:02} {mon} {y} {:02}:{:02}:{:02} GMT",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

fn parse_num(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn month_index(s: &str) -> Option<i64> {
    let lower = s.to_ascii_lowercase();
    MONTHS.iter().position(|m| *m == lower).map(|i| i as i64)
}

// Howard Hinnant's civil date algorithms.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED: i64 = 784_111_777;

    #[test]
    fn parses_all_three_formats() {
        assert_eq!(
            parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(EXPECTED)
        );
        assert_eq!(
            parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"),
            Some(EXPECTED)
        );
        assert_eq!(parse_http_date("Sun Nov  6 08:49:37 1994"), Some(EXPECTED));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_http_date("0"), None);
        assert_eq!(parse_http_date("-1"), None);
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 25:49:37 GMT"), None);
        assert_eq!(parse_http_date("Sun, 06 Foo 1994 08:49:37 GMT"), None);
        assert_eq!(parse_http_date(""), None);
    }

    #[test]
    fn format_round_trips() {
        assert_eq!(format_http_date(EXPECTED), "Sun, 06 Nov 1994 08:49:37 GMT");
        let now = now_unix();
        assert_eq!(parse_http_date(&format_http_date(now)), Some(now));
    }
}
