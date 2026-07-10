//! Usage pace / burn-rate projection for a rate window.
//!
//! Ports CodexBar's `UsagePace.weekly(window:now:...)` (linear model only —
//! no workday-aware variant). Display copy is local to this crate.

use chrono::{DateTime, Duration, Utc};

use crate::util::time::until_from;

/// How far actual usage sits relative to a linear budget through the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    OnTrack,
    SlightlyAhead,
    Ahead,
    FarAhead,
    SlightlyBehind,
    Behind,
    FarBehind,
}

/// Projected burn rate against a window's remaining time.
#[derive(Debug, Clone, PartialEq)]
pub struct Pace {
    pub stage: Stage,
    pub delta_percent: f64,
    pub expected_used_percent: f64,
    pub actual_used_percent: f64,
    /// Seconds until projected exhaustion, when not lasting to reset.
    pub eta_seconds: Option<f64>,
    pub will_last_to_reset: bool,
}

/// Compute pace for a single rate window.
///
/// Returns `None` when there is no usable reset, the reset is past, the remaining
/// time exceeds the window duration (clock not yet inside the window), or usage
/// exists at zero elapsed time (degenerate).
///
/// `default_window_minutes` is used when `window_minutes` is missing (CodexBar
/// defaults: 300 for session, 10080 for weekly).
pub fn for_window(
    used_percent: f64,
    window_minutes: Option<u32>,
    resets_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    default_window_minutes: u32,
) -> Option<Pace> {
    let resets_at = resets_at?;
    let minutes = window_minutes
        .filter(|m| *m > 0)
        .unwrap_or(default_window_minutes);
    if minutes == 0 {
        return None;
    }

    let duration_secs = f64::from(minutes) * 60.0;
    let time_until_reset = (resets_at - now).num_milliseconds() as f64 / 1000.0;
    if time_until_reset <= 0.0 {
        return None;
    }
    if time_until_reset > duration_secs {
        return None;
    }

    let elapsed = (duration_secs - time_until_reset).clamp(0.0, duration_secs);
    let expected = ((elapsed / duration_secs) * 100.0).clamp(0.0, 100.0);
    let actual = used_percent.clamp(0.0, 100.0);

    if elapsed == 0.0 && actual > 0.0 {
        return None;
    }

    let delta = actual - expected;
    let stage = stage_for(delta);

    let mut eta_seconds: Option<f64> = None;
    let mut will_last_to_reset = false;

    if actual >= 100.0 {
        eta_seconds = Some(0.0);
    } else if elapsed > 0.0 && actual > 0.0 {
        let rate = actual / elapsed;
        if rate > 0.0 {
            let remaining = 100.0 - actual;
            let candidate = remaining / rate;
            if candidate >= time_until_reset {
                will_last_to_reset = true;
            } else {
                eta_seconds = Some(candidate);
            }
        }
    } else if elapsed > 0.0 && actual == 0.0 {
        will_last_to_reset = true;
    }

    Some(Pace {
        stage,
        delta_percent: delta,
        expected_used_percent: expected,
        actual_used_percent: actual,
        eta_seconds,
        will_last_to_reset,
    })
}

fn stage_for(delta: f64) -> Stage {
    let abs = delta.abs();
    if abs <= 2.0 {
        Stage::OnTrack
    } else if abs <= 6.0 {
        if delta >= 0.0 {
            Stage::SlightlyAhead
        } else {
            Stage::SlightlyBehind
        }
    } else if abs <= 12.0 {
        if delta >= 0.0 {
            Stage::Ahead
        } else {
            Stage::Behind
        }
    } else if delta >= 0.0 {
        Stage::FarAhead
    } else {
        Stage::FarBehind
    }
}

/// Compact user-facing pace line, e.g.
/// `pace: 6% in reserve · expected 47% used · lasts until reset`
///
/// `indent` is prepended (tooltip windows use four spaces; detail uses two).
pub fn format_line(pace: &Pace, now: DateTime<Utc>, indent: &str) -> String {
    let delta_val = pace.delta_percent.abs().round() as i64;
    let budget = if pace.delta_percent > 0.0 {
        format!("{delta_val}% over budget")
    } else {
        format!("{delta_val}% in reserve")
    };
    let expected = pace.expected_used_percent.round() as i64;

    let mut parts = vec![
        format!("pace: {budget}"),
        format!("expected {expected}% used"),
    ];

    if pace.will_last_to_reset {
        parts.push("lasts until reset".into());
    } else if let Some(eta) = pace.eta_seconds {
        let target = now + Duration::milliseconds((eta * 1000.0).round() as i64);
        let body = until_from(target, now);
        if body == "now" {
            parts.push("runs out now".into());
        } else {
            parts.push(format!("runs out in {body}"));
        }
    }

    format!("{indent}{}", parts.join(" · "))
}

/// Format a pace line for a window when enough data is present; `None` otherwise.
pub fn line_for_window(
    used_percent: f64,
    window_minutes: Option<u32>,
    resets_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    default_window_minutes: u32,
    indent: &str,
) -> Option<String> {
    let pace = for_window(
        used_percent,
        window_minutes,
        resets_at,
        now,
        default_window_minutes,
    )?;
    Some(format_line(&pace, now, indent))
}

/// Default session-window duration (5h), matching CodexBar.
pub const DEFAULT_SESSION_MINUTES: u32 = 300;
/// Default weekly-window duration (7d), matching CodexBar.
pub const DEFAULT_WEEKLY_MINUTES: u32 = 10_080;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    #[test]
    fn stage_boundaries() {
        assert_eq!(stage_for(0.0), Stage::OnTrack);
        assert_eq!(stage_for(2.0), Stage::OnTrack);
        assert_eq!(stage_for(-2.0), Stage::OnTrack);
        assert_eq!(stage_for(2.01), Stage::SlightlyAhead);
        assert_eq!(stage_for(6.0), Stage::SlightlyAhead);
        assert_eq!(stage_for(-6.0), Stage::SlightlyBehind);
        assert_eq!(stage_for(6.01), Stage::Ahead);
        assert_eq!(stage_for(12.0), Stage::Ahead);
        assert_eq!(stage_for(-12.0), Stage::Behind);
        assert_eq!(stage_for(12.01), Stage::FarAhead);
        assert_eq!(stage_for(-12.01), Stage::FarBehind);
    }

    #[test]
    fn expected_delta_and_eta() {
        // 7d window, 4d remaining → elapsed = 3d → expected ≈ 42.857%
        // actual 50% → delta ≈ 7.143 → Ahead; rate = 50/3d → remaining 50%
        // needs 3d; time left 4d → wait, candidate = 50/rate = 50/(50/(3*86400)) = 3d
        // 3d < 4d remaining → does NOT last; eta = 3d
        let now = utc(0);
        let resets = now + Duration::days(4);
        let pace = for_window(
            50.0,
            Some(10_080),
            Some(resets),
            now,
            DEFAULT_WEEKLY_MINUTES,
        )
        .expect("pace");
        assert!((pace.expected_used_percent - 42.857).abs() < 0.01);
        assert!((pace.delta_percent - 7.143).abs() < 0.01);
        assert_eq!(pace.stage, Stage::Ahead);
        assert!(!pace.will_last_to_reset);
        assert!((pace.eta_seconds.unwrap() - 3.0 * 24.0 * 3600.0).abs() < 1.0);
    }

    #[test]
    fn lasts_to_reset_when_usage_low() {
        let now = utc(0);
        let resets = now + Duration::days(4);
        let pace =
            for_window(5.0, Some(10_080), Some(resets), now, DEFAULT_WEEKLY_MINUTES).expect("pace");
        assert!(pace.will_last_to_reset);
        assert!(pace.eta_seconds.is_none());
        assert_eq!(pace.stage, Stage::FarBehind);
    }

    #[test]
    fn none_when_reset_missing() {
        let now = utc(0);
        assert!(for_window(10.0, Some(10_080), None, now, DEFAULT_WEEKLY_MINUTES).is_none());
    }

    #[test]
    fn none_when_reset_in_past() {
        let now = utc(1000);
        let past = now - Duration::seconds(1);
        assert!(for_window(10.0, Some(10_080), Some(past), now, DEFAULT_WEEKLY_MINUTES).is_none());
    }

    #[test]
    fn none_when_time_until_reset_exceeds_duration() {
        // 7d window but 9d until reset → outside window
        let now = utc(0);
        let far = now + Duration::days(9);
        assert!(for_window(10.0, Some(10_080), Some(far), now, DEFAULT_WEEKLY_MINUTES).is_none());
    }

    #[test]
    fn none_when_usage_with_zero_elapsed() {
        // reset exactly at end of full window → elapsed 0, but usage > 0
        let now = utc(0);
        let resets = now + Duration::seconds(10_080 * 60);
        assert!(
            for_window(
                12.0,
                Some(10_080),
                Some(resets),
                now,
                DEFAULT_WEEKLY_MINUTES
            )
            .is_none()
        );
    }

    #[test]
    fn actual_100_gives_eta_zero() {
        let now = utc(0);
        let resets = now + Duration::days(1);
        let pace = for_window(
            100.0,
            Some(10_080),
            Some(resets),
            now,
            DEFAULT_WEEKLY_MINUTES,
        )
        .expect("pace");
        assert_eq!(pace.eta_seconds, Some(0.0));
        assert!(!pace.will_last_to_reset);
    }

    #[test]
    fn actual_zero_with_elapsed_lasts_to_reset() {
        let now = utc(0);
        let resets = now + Duration::days(4);
        let pace =
            for_window(0.0, Some(10_080), Some(resets), now, DEFAULT_WEEKLY_MINUTES).expect("pace");
        assert!(pace.will_last_to_reset);
        assert!(pace.eta_seconds.is_none());
    }

    #[test]
    fn format_line_reserve_and_over() {
        let now = utc(0);
        // behind budget
        let pace_behind = Pace {
            stage: Stage::FarBehind,
            delta_percent: -6.0,
            expected_used_percent: 47.0,
            actual_used_percent: 41.0,
            eta_seconds: None,
            will_last_to_reset: true,
        };
        assert_eq!(
            format_line(&pace_behind, now, "    "),
            "    pace: 6% in reserve · expected 47% used · lasts until reset"
        );

        // ahead of budget, runs out
        let pace_ahead = Pace {
            stage: Stage::Ahead,
            delta_percent: 9.0,
            expected_used_percent: 31.0,
            actual_used_percent: 40.0,
            eta_seconds: Some(3.0 * 3600.0 + 10.0 * 60.0),
            will_last_to_reset: false,
        };
        assert_eq!(
            format_line(&pace_ahead, now, "    "),
            "    pace: 9% over budget · expected 31% used · runs out in 3h 10m"
        );
    }

    #[test]
    fn uses_default_window_minutes_when_missing() {
        let now = utc(0);
        // 5h default, 2.5h remaining → expected 50%
        let resets = now + Duration::seconds(150 * 60);
        let pace =
            for_window(50.0, None, Some(resets), now, DEFAULT_SESSION_MINUTES).expect("pace");
        assert!((pace.expected_used_percent - 50.0).abs() < 0.01);
        assert_eq!(pace.stage, Stage::OnTrack);
    }
}
