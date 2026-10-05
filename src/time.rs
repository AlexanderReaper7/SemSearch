//! Timestamps as the user reads and types them, in the system time zone.

use anyhow::{Context, Result, bail};
use jiff::{Timestamp, civil, tz::TimeZone};

fn zoned(unix: i64) -> Option<jiff::Zoned> {
    Some(Timestamp::from_second(unix).ok()?.to_zoned(TimeZone::system()))
}

/// `2026-09-22 14:03`, for result listings.
pub fn short(unix: i64) -> String {
    zoned(unix).filter(|_| unix > 0).map(|z| z.strftime("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

/// `Tuesday 22 September 2026, 14:03`, for embedded text. Weekday and month
/// are words so that a query naming them has something to match.
pub fn long(unix: i64) -> String {
    zoned(unix).filter(|_| unix > 0).map(|z| z.strftime("%A %-d %B %Y, %H:%M").to_string()).unwrap_or_default()
}

/// A bound for `--since` and `--before`: `2026-09-22`, `2026-09-22 14:00`,
/// or an age such as `3d` or `2w` counted back from now.
pub fn parse_bound(s: &str) -> Result<i64> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix('d').and_then(|n| n.parse::<i64>().ok()) {
        return Ok(Timestamp::now().as_second() - n * 86_400);
    }
    if let Some(n) = s.strip_suffix('w').and_then(|n| n.parse::<i64>().ok()) {
        return Ok(Timestamp::now().as_second() - n * 7 * 86_400);
    }
    let dt = if let Ok(d) = s.parse::<civil::Date>() {
        d.to_datetime(civil::time(0, 0, 0, 0))
    } else if let Ok(dt) = s.replace(' ', "T").parse::<civil::DateTime>() {
        dt
    } else {
        bail!("not a date, date and time, or age like 3d or 2w: {s}");
    };
    Ok(dt.to_zoned(TimeZone::system()).context("local time")?.timestamp().as_second())
}
