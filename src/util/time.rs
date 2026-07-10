use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Utc};

use crate::config::ResetStyle;

/// Format a duration-from-now as "2h 14m" / "45m" / "5d 3h" / "now".
///
/// Minutes are **ceiled** so a remaining 30s never renders as a stale "0m".
pub fn until(target: DateTime<Utc>) -> String {
    until_from(target, Utc::now())
}

/// Same as [`until`], with an injectable `now` for tests.
pub fn until_from(target: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = target.signed_duration_since(now).num_seconds();
    if secs < 1 {
        return "now".into();
    }
    // Ceil to the next whole minute (CodexBar: max(1, ceil(seconds / 60))).
    let total_minutes = ((secs as f64) / 60.0).ceil() as i64;
    let total_minutes = total_minutes.max(1);
    let days = total_minutes / (24 * 60);
    let hours = (total_minutes / 60) % 24;
    let minutes = total_minutes % 60;

    if days > 0 {
        if hours > 0 {
            format!("{days}d {hours}h")
        } else if minutes > 0 {
            format!("{days}d {minutes}m")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        }
    } else {
        format!("{total_minutes}m")
    }
}

/// Absolute local-time phrasing for a reset instant:
/// - same local calendar day → `14:30`
/// - tomorrow → `tomorrow, 14:30`
/// - otherwise → `Feb 3, 14:30`
pub fn reset_description(target: DateTime<Utc>) -> String {
    let now = Local::now();
    reset_description_from(target.with_timezone(&Local), now)
}

/// Same as [`reset_description`], with injectable local times for tests.
pub fn reset_description_from<Tz: TimeZone>(target: DateTime<Tz>, now: DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let time_str = target.format("%H:%M").to_string();
    let target_day = target.date_naive();
    let now_day = now.date_naive();
    if target_day == now_day {
        return time_str;
    }
    if target_day == now_day + Duration::days(1) {
        return format!("tomorrow, {time_str}");
    }
    format!("{} {}, {time_str}", target.format("%b"), target.day())
}

/// Shared phrase used by display surfaces:
/// - countdown: `in 2h 14m` / `now`
/// - absolute: `14:30` / `tomorrow, 14:30` / `Feb 3, 14:30`
pub fn reset_phrase(target: DateTime<Utc>, style: ResetStyle) -> String {
    match style {
        ResetStyle::Countdown => {
            let body = until(target);
            if body == "now" {
                "now".into()
            } else {
                format!("in {body}")
            }
        }
        ResetStyle::Absolute => reset_description(target),
    }
}

/// Full reset clause for window lines / TUI: `resets in 2h 14m`, `resets now`,
/// `resets 14:30`, `resets tomorrow, 14:30`, …
pub fn reset_label(target: DateTime<Utc>, style: ResetStyle) -> String {
    format!("resets {}", reset_phrase(target, style))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn utc(y: i32, mon: u32, d: u32, h: u32, m: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mon, d, h, m, s).unwrap()
    }

    fn fixed(
        offset_hours: i32,
        y: i32,
        mon: u32,
        d: u32,
        h: u32,
        m: u32,
        s: u32,
    ) -> DateTime<FixedOffset> {
        let tz = FixedOffset::east_opt(offset_hours * 3600).unwrap();
        tz.with_ymd_and_hms(y, mon, d, h, m, s).unwrap()
    }

    #[test]
    fn until_ceil_avoids_zero_minutes() {
        let now = utc(2026, 2, 3, 12, 0, 0);
        // 30 seconds remaining → ceil to 1 minute, not "0m"
        assert_eq!(until_from(utc(2026, 2, 3, 12, 0, 30), now), "1m");
        // exactly 60s → 1m
        assert_eq!(until_from(utc(2026, 2, 3, 12, 1, 0), now), "1m");
        // 61s → ceil to 2m
        assert_eq!(until_from(utc(2026, 2, 3, 12, 1, 1), now), "2m");
    }

    #[test]
    fn until_now_and_compound_units() {
        let now = utc(2026, 2, 3, 12, 0, 0);
        assert_eq!(until_from(utc(2026, 2, 3, 11, 59, 59), now), "now");
        assert_eq!(until_from(now, now), "now");
        // 2h 14m exactly
        assert_eq!(until_from(utc(2026, 2, 3, 14, 14, 0), now), "2h 14m");
        // whole hours omit 0m
        assert_eq!(until_from(utc(2026, 2, 3, 15, 0, 0), now), "3h");
        // multi-day
        assert_eq!(until_from(utc(2026, 2, 6, 17, 0, 0), now), "3d 5h");
        // multi-day with zero hours (3d exactly)
        assert_eq!(until_from(utc(2026, 2, 6, 12, 0, 0), now), "3d");
    }

    #[test]
    fn reset_description_today_tomorrow_other() {
        let now = fixed(0, 2026, 2, 3, 10, 0, 0);
        // same calendar day
        assert_eq!(
            reset_description_from(fixed(0, 2026, 2, 3, 14, 30, 0), now),
            "14:30"
        );
        // tomorrow
        assert_eq!(
            reset_description_from(fixed(0, 2026, 2, 4, 9, 5, 0), now),
            "tomorrow, 09:05"
        );
        // other day
        assert_eq!(
            reset_description_from(fixed(0, 2026, 2, 10, 14, 30, 0), now),
            "Feb 10, 14:30"
        );
        // single-digit day (no leading zero)
        assert_eq!(
            reset_description_from(fixed(0, 2026, 3, 3, 8, 0, 0), now),
            "Mar 3, 08:00"
        );
    }

    fn phrase_from(target: DateTime<Utc>, style: ResetStyle, now: DateTime<Utc>) -> String {
        match style {
            ResetStyle::Countdown => {
                let body = until_from(target, now);
                if body == "now" {
                    "now".into()
                } else {
                    format!("in {body}")
                }
            }
            ResetStyle::Absolute => {
                let local_now = now.with_timezone(&Local);
                reset_description_from(target.with_timezone(&Local), local_now)
            }
        }
    }

    #[test]
    fn reset_phrase_countdown_and_absolute() {
        let now = utc(2026, 2, 3, 12, 0, 0);
        assert_eq!(
            phrase_from(utc(2026, 2, 3, 14, 14, 0), ResetStyle::Countdown, now),
            "in 2h 14m"
        );
        assert_eq!(
            phrase_from(utc(2026, 2, 3, 12, 0, 0), ResetStyle::Countdown, now),
            "now"
        );
        // Absolute: pure day/time phrasing with a fixed offset (deterministic).
        let local_now = fixed(-5, 2026, 2, 3, 10, 0, 0);
        assert_eq!(
            reset_description_from(fixed(-5, 2026, 2, 3, 14, 30, 0), local_now),
            "14:30"
        );
        assert_eq!(
            reset_description_from(fixed(-5, 2026, 2, 4, 9, 5, 0), local_now),
            "tomorrow, 09:05"
        );
    }

    #[test]
    fn reset_label_shapes() {
        let now = utc(2026, 2, 3, 12, 0, 0);
        let phrase = phrase_from(utc(2026, 2, 3, 14, 14, 0), ResetStyle::Countdown, now);
        assert_eq!(format!("resets {phrase}"), "resets in 2h 14m");
        let phrase_now = phrase_from(now, ResetStyle::Countdown, now);
        assert_eq!(format!("resets {phrase_now}"), "resets now");
        // Absolute label shape (no "in ").
        let abs = phrase_from(
            utc(2026, 2, 3, 14, 30, 0),
            ResetStyle::Absolute,
            // Same UTC instant as local 14:30 when offset is 0 — day label still
            // depends on Local; assert only that countdown vs absolute diverge.
            now,
        );
        assert!(!abs.starts_with("in "));
        assert_ne!(abs, "now");
    }
}
