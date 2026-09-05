//! Today's date, without a crate.
//!
//! std knows the seconds since the epoch and nothing about time zones, and
//! the daily note has to be dated where the user is, not at Greenwich. The
//! calendar arithmetic is Howard Hinnant's civil-from-days; the offset comes
//! from `TZ` when it is a fixed offset, and otherwise from one `date +%z` at
//! startup — the only source of the zone database std cannot read. A machine
//! without `date` falls back to UTC, and a zone that changes offset while
//! catcher is running is noticed at the next launch, not before.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// A date and a clock time: `((year, month, day), (hour, minute, second))`.
pub type Now = ((i32, u32, u32), (u32, u32, u32));

/// Today, as (year, month, day) in local time.
pub fn today() -> (i32, u32, u32) {
    now().0
}

/// The date and the time of day, in local time.
pub fn now() -> Now {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let local = secs + i64::from(local_offset());
    let day = civil_from_days(local.div_euclid(86_400));
    let s = local.rem_euclid(86_400) as u32;
    (day, (s / 3600, s / 60 % 60, s % 60))
}

/// A moment-style format filled in for `now`: `YYYY-MM-DD`, `dddd Do MMMM`,
/// `[week] W`... Tokens are `YYYY YY MM M DD D MMMM MMM dddd ddd Do HH H hh
/// h mm ss A a`; text in `[brackets]` is copied without being read for
/// tokens, and anything else is copied as it is.
pub fn format(fmt: &str, now: Now) -> String {
    const TOKENS: [&str; 19] = [
        "YYYY", "MMMM", "dddd", "MMM", "ddd", "YY", "MM", "DD", "Do", "HH", "hh", "mm", "ss", "M",
        "D", "H", "h", "A", "a",
    ];
    let ((y, m, d), (hh, mi, ss)) = now;
    let mut out = String::new();
    let mut rest = fmt;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('[') {
            let end = after.find(']').unwrap_or(after.len());
            out.push_str(&after[..end]);
            rest = after.get(end + 1..).unwrap_or("");
            continue;
        }
        let Some(tok) = TOKENS.iter().find(|t| rest.starts_with(**t)) else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        };
        rest = &rest[tok.len()..];
        match *tok {
            "YYYY" => out.push_str(&format!("{y:04}")),
            "YY" => out.push_str(&format!("{:02}", y.rem_euclid(100))),
            "MMMM" => out.push_str(month_name(m)),
            "MMM" => out.push_str(&month_name(m)[..3]),
            "MM" => out.push_str(&format!("{m:02}")),
            "M" => out.push_str(&m.to_string()),
            "dddd" => out.push_str(weekday(y, m, d)),
            "ddd" => out.push_str(&weekday(y, m, d)[..3]),
            "DD" => out.push_str(&format!("{d:02}")),
            "Do" => out.push_str(&ordinal(d)),
            "D" => out.push_str(&d.to_string()),
            "HH" => out.push_str(&format!("{hh:02}")),
            "H" => out.push_str(&hh.to_string()),
            "hh" => out.push_str(&format!("{:02}", twelve(hh))),
            "h" => out.push_str(&twelve(hh).to_string()),
            "mm" => out.push_str(&format!("{mi:02}")),
            "ss" => out.push_str(&format!("{ss:02}")),
            "A" => out.push_str(if hh < 12 { "AM" } else { "PM" }),
            "a" => out.push_str(if hh < 12 { "am" } else { "pm" }),
            _ => unreachable!(),
        }
    }
    out
}

fn twelve(h: u32) -> u32 {
    match h % 12 {
        0 => 12,
        n => n,
    }
}

/// `1st`, `2nd`, `3rd`, `4th`... `11th`, `12th`, `13th`, `21st`.
fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn month_name(m: u32) -> &'static str {
    MONTHS[(m as usize).saturating_sub(1) % 12]
}

/// `YYYY-MM-DD`.
pub fn iso(y: i32, m: u32, d: u32) -> String {
    format!("{y:04}-{m:02}-{d:02}")
}

/// The long form a heading wants: `Tuesday 1 September 2026`.
pub fn long(y: i32, m: u32, d: u32) -> String {
    format!("{} {d} {} {y}", weekday(y, m, d), month_name(m))
}

/// The date `n` days on (or back, when negative).
pub fn shift(y: i32, m: u32, d: u32, n: i64) -> (i32, u32, u32) {
    civil_from_days(days_from_civil(y, m, d) + n)
}

/// A `YYYY-MM-DD` and nothing else, or `None`. Strict on purpose: a value
/// that is a date and a bit more is prose, and the hint that follows a date
/// would be a lie under it.
pub fn parse_iso(text: &str) -> Option<(i32, u32, u32)> {
    let t = text.trim();
    let b = t.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i32 = t[..4].parse().ok()?;
    let m: u32 = t[5..7].parse().ok()?;
    let d: u32 = t[8..].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // round-tripping catches the 31st of a month that has no 31st
    (civil_from_days(days_from_civil(y, m, d)) == (y, m, d)).then_some((y, m, d))
}

/// Days from `from` to `to`, negative when `to` is the earlier date.
pub fn days_between(from: (i32, u32, u32), to: (i32, u32, u32)) -> i64 {
    days_from_civil(to.0, to.1, to.2) - days_from_civil(from.0, from.1, from.2)
}

/// A date said the way a person would beside `today`: `today`, `tomorrow`,
/// `in 5 weeks`, `3 days ago`. Coarser the further away it is, since nobody
/// counts a date next year in days.
pub fn relative(date: (i32, u32, u32), today: (i32, u32, u32)) -> String {
    let days = days_between(today, date);
    match days {
        0 => return "today".to_string(),
        1 => return "tomorrow".to_string(),
        -1 => return "yesterday".to_string(),
        _ => {}
    }
    let n = days.unsigned_abs();
    let span = match n {
        0..=13 => format!("{n} day{}", plural(n)),
        14..=59 => format!("{} week{}", n / 7, plural(n / 7)),
        60..=729 => format!("{} month{}", n / 30, plural(n / 30)),
        _ => format!("{} year{}", n / 365, plural(n / 365)),
    };
    if days > 0 {
        format!("in {span}")
    } else {
        format!("{span} ago")
    }
}

fn plural(n: u64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const WEEKDAYS: [&str; 7] = [
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
];

/// Day zero, 1970-01-01, was a Thursday.
fn weekday(y: i32, m: u32, d: u32) -> &'static str {
    WEEKDAYS[days_from_civil(y, m, d).rem_euclid(7) as usize]
}

/// Days since 1970-01-01 to (y, m, d): the Gregorian calendar as a sequence
/// of 400-year eras, each starting on the 1st of March so the leap day lands
/// last. Hinnant's `days_from_civil`.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse: (y, m, d) from days since the epoch. Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((y + i64::from(m <= 2)) as i32, m, d)
}

/// Work out the offset before the terminal goes raw: it costs a process, and
/// a child spawned under raw mode inherits a terminal it does not expect.
/// `today` finds it anyway if this was never called.
pub fn init() {
    local_offset();
}

/// Seconds east of UTC, found once and kept: the answer costs a process.
fn local_offset() -> i32 {
    static OFFSET: OnceLock<i32> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        std::env::var("TZ")
            .ok()
            .and_then(|tz| offset_from_tz(&tz))
            .or_else(|| date_z().and_then(|z| offset_from_z(&z)))
            .unwrap_or(0)
    })
}

/// What `date +%z` prints: `+0100`, `-0700`.
fn date_z() -> Option<String> {
    let out = std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `+HHMM` or `-HHMM` to seconds east.
fn offset_from_z(z: &str) -> Option<i32> {
    let (sign, rest) = match z.chars().next()? {
        '+' => (1, &z[1..]),
        '-' => (-1, &z[1..]),
        _ => return None,
    };
    if rest.len() != 4 || !rest.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h: i32 = rest[..2].parse().ok()?;
    let m: i32 = rest[2..].parse().ok()?;
    Some(sign * (h * 3600 + m * 60))
}

/// The fixed-offset spellings of `TZ` — `UTC`, `UTC0`, `EST5`, `UTC-3`,
/// `GMT+02:00`. POSIX puts the sign the other way round: `EST5` is five hours
/// *west*. A named zone (`Europe/London`) needs the database, so `None`.
fn offset_from_tz(tz: &str) -> Option<i32> {
    let tz = tz.trim().trim_start_matches(':');
    let name_len = tz.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    if name_len < 3 || tz.contains('/') {
        return None;
    }
    let rest = &tz[name_len..];
    if rest.is_empty() {
        return matches!(&tz[..3], "UTC" | "GMT").then_some(0);
    }
    let (west, rest) = match rest.chars().next()? {
        '-' => (false, &rest[1..]),
        '+' => (true, &rest[1..]),
        _ => (true, rest),
    };
    let mut parts = rest.split(':');
    let h: i32 = parts.next()?.parse().ok()?;
    let m: i32 = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    let secs = h * 3600 + m * 60;
    Some(if west { -secs } else { secs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_and_leap_days_round_trip_through_hinnant() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
        assert_eq!(civil_from_days(days_from_civil(2026, 9, 1)), (2026, 9, 1));
        // 2026-09-01 is 20,697 days on
        assert_eq!(days_from_civil(2026, 9, 1), 20_697);
    }

    #[test]
    fn dates_format_as_iso_and_as_a_heading() {
        assert_eq!(iso(2026, 9, 1), "2026-09-01");
        assert_eq!(long(2026, 9, 1), "Tuesday 1 September 2026");
        assert_eq!(long(1970, 1, 1), "Thursday 1 January 1970");
        assert_eq!(long(2024, 2, 29), "Thursday 29 February 2024");
    }

    #[test]
    fn shifting_crosses_month_and_year_ends() {
        assert_eq!(shift(2026, 9, 1, -1), (2026, 8, 31));
        assert_eq!(shift(2026, 12, 31, 1), (2027, 1, 1));
        assert_eq!(shift(2024, 2, 28, 1), (2024, 2, 29));
    }

    #[test]
    fn the_offset_reads_date_z_and_the_fixed_tz_spellings() {
        assert_eq!(offset_from_z("+0100"), Some(3600));
        assert_eq!(offset_from_z("-0730"), Some(-27_000));
        assert_eq!(offset_from_z("0100"), None);
        assert_eq!(offset_from_z("+1"), None);
        assert_eq!(offset_from_tz("UTC"), Some(0));
        assert_eq!(offset_from_tz("EST5"), Some(-18_000));
        assert_eq!(offset_from_tz("UTC-3"), Some(10_800));
        assert_eq!(offset_from_tz("GMT+02:00"), Some(-7200));
        // a named zone needs the database, so it is left to `date`
        assert_eq!(offset_from_tz("Europe/London"), None);
        assert_eq!(offset_from_tz("America/New_York"), None);
        assert_eq!(offset_from_tz(""), None);
    }

    #[test]
    fn every_format_token_is_filled_in() {
        let now = ((2026, 9, 1), (14, 5, 9));
        for (fmt, want) in [
            ("YYYY", "2026"),
            ("YY", "26"),
            ("MM", "09"),
            ("M", "9"),
            ("DD", "01"),
            ("D", "1"),
            ("MMMM", "September"),
            ("MMM", "Sep"),
            ("dddd", "Tuesday"),
            ("ddd", "Tue"),
            ("Do", "1st"),
            ("HH", "14"),
            ("H", "14"),
            ("hh", "02"),
            ("h", "2"),
            ("mm", "05"),
            ("ss", "09"),
            ("A", "PM"),
            ("a", "pm"),
            ("[Day] D [of] MMMM", "Day 1 of September"),
            ("YYYY-MM-DD", "2026-09-01"),
            ("YYYY/MM/DD-MM-YYYY", "2026/09/01-09-2026"),
            ("[unclosed", "unclosed"),
            ("", ""),
        ] {
            assert_eq!(format(fmt, now), want, "{fmt}");
        }
        // midnight and noon on the twelve-hour clock, and the ordinals
        assert_eq!(format("h A", ((2026, 9, 1), (0, 0, 0))), "12 AM");
        assert_eq!(format("hh a", ((2026, 9, 1), (12, 0, 0))), "12 pm");
        assert_eq!(format("Do", ((2026, 9, 2), (0, 0, 0))), "2nd");
        assert_eq!(format("Do", ((2026, 9, 3), (0, 0, 0))), "3rd");
        assert_eq!(format("Do", ((2026, 9, 11), (0, 0, 0))), "11th");
        assert_eq!(format("Do", ((2026, 9, 12), (0, 0, 0))), "12th");
        assert_eq!(format("Do", ((2026, 9, 13), (0, 0, 0))), "13th");
        assert_eq!(format("Do", ((2026, 9, 21), (0, 0, 0))), "21st");
        assert_eq!(format("Do", ((2026, 9, 22), (0, 0, 0))), "22nd");
    }

    #[test]
    fn today_is_a_real_date() {
        let (y, m, d) = today();
        assert!(y >= 2026);
        assert!((1..=12).contains(&m));
        assert!((1..=31).contains(&d));
        let (_, (h, mi, s)) = now();
        assert!(h < 24 && mi < 60 && s < 60);
    }

    #[test]
    fn an_iso_date_is_read_strictly_and_said_relative_to_today() {
        assert_eq!(parse_iso("2026-09-05"), Some((2026, 9, 5)));
        assert_eq!(parse_iso(" 2026-09-05 "), Some((2026, 9, 5)));
        assert_eq!(parse_iso("2026-09-05T10:00"), None);
        assert_eq!(parse_iso("2026-13-01"), None);
        assert_eq!(parse_iso("2026-02-30"), None);
        assert_eq!(parse_iso("launch"), None);
        let today = (2026, 9, 5);
        assert_eq!(relative((2026, 9, 5), today), "today");
        assert_eq!(relative((2026, 9, 6), today), "tomorrow");
        assert_eq!(relative((2026, 9, 4), today), "yesterday");
        assert_eq!(relative((2026, 9, 2), today), "3 days ago");
        assert_eq!(relative((2026, 10, 10), today), "in 5 weeks");
        assert_eq!(relative((2026, 12, 20), today), "in 3 months");
        assert_eq!(relative((2023, 1, 1), today), "3 years ago");
        assert_eq!(days_between(today, (2026, 9, 1)), -4);
    }
}
