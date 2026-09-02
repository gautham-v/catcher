//! Today's date, without a calendar crate.
//!
//! std knows the seconds since the epoch and nothing about time zones, so the
//! local offset comes from outside: `TZ` when it is a plain UTC spelling, and
//! otherwise one `date +%z` run at startup. That is a snapshot — a session
//! that lives across a DST change, or a machine whose zone changes, keeps the
//! offset it started with. Good enough for stamping a note.

use std::process::Command;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static OFFSET: OnceLock<i32> = OnceLock::new();

/// Work out the local offset once. Called from main so the shell-out happens
/// before the terminal is put into raw mode; `today` calls it too, so nothing
/// breaks if it is never called.
pub fn init() {
    OFFSET.get_or_init(local_offset_secs);
}

/// (year, month, day) in local time.
pub fn today() -> (i32, u32, u32) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let local = secs + i64::from(*OFFSET.get_or_init(local_offset_secs));
    civil_from_days(local.div_euclid(86_400))
}

/// `2026-09-01`.
pub fn iso(y: i32, m: u32, d: u32) -> String {
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to a proleptic Gregorian date. Howard Hinnant's
/// algorithm: shift the year to start in March so the leap day falls last.
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

fn local_offset_secs() -> i32 {
    if let Some(off) = std::env::var("TZ").ok().and_then(|tz| utc_offset(&tz)) {
        return off;
    }
    Command::new("date")
        .arg("+%z")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| parse_offset(String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or(0)
}

/// The `TZ` spellings that mean UTC and nothing else. A named zone
/// (`Europe/Berlin`) needs the tz database, which is `date`'s job.
fn utc_offset(tz: &str) -> Option<i32> {
    let t = tz.trim();
    let utc = ["UTC", "GMT", "UTC0", "GMT0", "Etc/UTC", "Etc/GMT", "Z"];
    utc.iter().any(|u| u.eq_ignore_ascii_case(t)).then_some(0)
}

/// `+0530` → 19800, `-0700` → -25200.
fn parse_offset(s: &str) -> Option<i32> {
    let (sign, digits) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let h: i32 = digits[..2].parse().ok()?;
    let m: i32 = digits[2..].parse().ok()?;
    Some(sign * (h * 3600 + m * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_and_the_days_around_it_come_out_right() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(1), (1970, 1, 2));
    }

    #[test]
    fn leap_days_and_century_rules_hold() {
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        // 1900 was not a leap year
        assert_eq!(civil_from_days(-25_509), (1900, 2, 28));
        assert_eq!(civil_from_days(-25_508), (1900, 3, 1));
        assert_eq!(civil_from_days(20_697), (2026, 9, 1));
    }

    #[test]
    fn iso_pads_every_field() {
        assert_eq!(iso(2026, 9, 1), "2026-09-01");
        assert_eq!(iso(999, 12, 25), "0999-12-25");
    }

    #[test]
    fn date_s_offset_spelling_is_read_and_utc_needs_no_shell() {
        assert_eq!(parse_offset("+0530"), Some(19_800));
        assert_eq!(parse_offset("-0700"), Some(-25_200));
        assert_eq!(parse_offset("+00:00"), None);
        assert_eq!(parse_offset("junk"), None);
        assert_eq!(utc_offset("UTC"), Some(0));
        assert_eq!(utc_offset("Europe/Berlin"), None);
    }

    #[test]
    fn today_is_a_plausible_date() {
        let (y, m, d) = today();
        assert!(y >= 2026);
        assert!((1..=12).contains(&m));
        assert!((1..=31).contains(&d));
    }
}
