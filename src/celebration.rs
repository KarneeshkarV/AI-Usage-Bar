//! Weekly quota-reset celebration window.
//!
//! When a *weekly* usage window (Codex secondary / Claude weekly) rolls over —
//! detected as `resets_at` jumping forward past `now` — we hold a short
//! celebration flag on the snapshot for Waybar and the TUI.

use chrono::{DateTime, Duration, Utc};

use crate::snapshot::Snapshot;

/// How long the celebration banner / confetti stays active.
pub const CELEBRATE_SECS: i64 = 10 * 60;

/// Resolve `celebrating_until` for the next snapshot.
///
/// - Keeps an unexpired window from the previous snapshot.
/// - Starts a new 10-minute window when a weekly `resets_at` jumps forward
///   to a time still in the future (the same forward-jump signal used for
///   reset notifications, scoped to weekly windows only).
/// - Session / primary windows never trigger celebration.
pub fn resolve_celebrating_until(
    prev: Option<&Snapshot>,
    next: &Snapshot,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if let Some(until) = prev.and_then(|s| s.celebrating_until)
        && until > now
    {
        return Some(until);
    }

    if weekly_reset_jumped(prev, next, now) {
        return Some(now + Duration::seconds(CELEBRATE_SECS));
    }

    None
}

/// True while `now` is strictly before `celebrating_until`.
pub fn is_celebrating(until: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    until.is_some_and(|t| t > now)
}

/// Seed for deterministic confetti particles (epoch of celebration start).
pub fn confetti_seed(celebrating_until: DateTime<Utc>) -> u64 {
    let start = celebrating_until - Duration::seconds(CELEBRATE_SECS);
    start.timestamp().unsigned_abs()
}

fn weekly_reset_jumped(prev: Option<&Snapshot>, next: &Snapshot, now: DateTime<Utc>) -> bool {
    let Some(prev) = prev else {
        return false;
    };

    // Codex secondary ≈ weekly.
    if let (Some(p), Some(n)) = (
        prev.codex.as_ref().and_then(|c| c.secondary.as_ref()),
        next.codex.as_ref().and_then(|c| c.secondary.as_ref()),
    ) && resets_jumped_forward(p.resets_at, n.resets_at, now)
    {
        return true;
    }

    // Claude weekly.
    if let (Some(p), Some(n)) = (
        prev.claude.as_ref().and_then(|c| c.weekly.as_ref()),
        next.claude.as_ref().and_then(|c| c.weekly.as_ref()),
    ) && resets_jumped_forward(p.resets_at, n.resets_at, now)
    {
        return true;
    }

    false
}

/// Providers recompute weekly `resets_at` between polls with second-level
/// jitter; a genuine roll-over needs the old boundary passed AND a jump far
/// larger than jitter (same guard as the ping scheduler's `JUMP_MIN_SECS`).
const JUMP_MIN_SECS: i64 = 3600;

/// `resets_at` rolled over: the old reset has passed and the fresh one moved
/// forward past `now` by more than jitter allows.
fn resets_jumped_forward(
    prev: Option<DateTime<Utc>>,
    next: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    match (prev, next) {
        (Some(p), Some(n)) => p <= now && n > now && n - p > Duration::seconds(JUMP_MIN_SECS),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::claude::{ClaudeSnapshot, Window as ClaudeWindow};
    use crate::providers::codex::{CodexSnapshot, Window as CodexWindow};
    use chrono::TimeZone;

    fn t(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, day, hour, 0, 0).unwrap()
    }

    fn snap_codex_secondary(reset: Option<DateTime<Utc>>) -> Snapshot {
        let mut s = Snapshot::new();
        s.codex = Some(CodexSnapshot {
            account_email: None,
            plan_type: None,
            primary: Some(CodexWindow {
                used_percent: 10.0,
                window_minutes: Some(300),
                resets_at: Some(t(10, 6)),
            }),
            secondary: Some(CodexWindow {
                used_percent: 5.0,
                window_minutes: Some(10080),
                resets_at: reset,
            }),
            credits: None,
            reset_credits: None,
            error: None,
        });
        s
    }

    fn snap_claude_weekly(
        session_reset: Option<DateTime<Utc>>,
        weekly_reset: Option<DateTime<Utc>>,
    ) -> Snapshot {
        let mut s = Snapshot::new();
        s.claude = Some(ClaudeSnapshot {
            account_email: None,
            plan_type: None,
            session: Some(ClaudeWindow {
                used_percent: 10.0,
                resets_at: session_reset,
            }),
            weekly: Some(ClaudeWindow {
                used_percent: 5.0,
                resets_at: weekly_reset,
            }),
            sonnet_weekly: None,
            extra: None,
            source: None,
            error: None,
        });
        s
    }

    #[test]
    fn starts_on_codex_secondary_weekly_jump() {
        let now = t(10, 12);
        let prev = snap_codex_secondary(Some(t(10, 11))); // old weekly reset just past
        let next = snap_codex_secondary(Some(t(17, 11))); // new weekly reset next week
        let until = resolve_celebrating_until(Some(&prev), &next, now);
        assert_eq!(until, Some(now + Duration::seconds(CELEBRATE_SECS)));
        assert!(is_celebrating(until, now));
    }

    #[test]
    fn starts_on_claude_weekly_jump() {
        let now = t(10, 12);
        let prev = snap_claude_weekly(Some(t(10, 17)), Some(t(10, 11)));
        let next = snap_claude_weekly(Some(t(10, 17)), Some(t(17, 11)));
        let until = resolve_celebrating_until(Some(&prev), &next, now);
        assert_eq!(until, Some(now + Duration::seconds(CELEBRATE_SECS)));
    }

    #[test]
    fn does_not_trigger_on_weekly_jitter() {
        let now = t(10, 12);
        // Weekly reset still days away; fresh fetch recomputes it 30s later.
        let base = t(16, 11);
        let prev = snap_claude_weekly(Some(t(10, 17)), Some(base));
        let next = snap_claude_weekly(Some(t(10, 17)), Some(base + Duration::seconds(30)));
        let until = resolve_celebrating_until(Some(&prev), &next, now);
        assert!(until.is_none(), "jitter must not celebrate: {until:?}");
    }

    #[test]
    fn does_not_trigger_on_session_window_jump() {
        let now = t(10, 12);
        // Session jumps; weekly stays put.
        let prev = snap_claude_weekly(Some(t(10, 11)), Some(t(17, 11)));
        let next = snap_claude_weekly(Some(t(10, 17)), Some(t(17, 11)));
        let until = resolve_celebrating_until(Some(&prev), &next, now);
        assert!(
            until.is_none(),
            "session jump must not celebrate: {until:?}"
        );
    }

    #[test]
    fn does_not_trigger_on_codex_primary_jump() {
        let now = t(10, 12);
        let mut prev = snap_codex_secondary(Some(t(17, 11)));
        let mut next = snap_codex_secondary(Some(t(17, 11)));
        // Only primary rolls over.
        if let Some(c) = prev.codex.as_mut() {
            c.primary = Some(CodexWindow {
                used_percent: 90.0,
                window_minutes: Some(300),
                resets_at: Some(t(10, 11)),
            });
        }
        if let Some(c) = next.codex.as_mut() {
            c.primary = Some(CodexWindow {
                used_percent: 5.0,
                window_minutes: Some(300),
                resets_at: Some(t(10, 17)),
            });
        }
        let until = resolve_celebrating_until(Some(&prev), &next, now);
        assert!(until.is_none());
    }

    #[test]
    fn expires_after_window() {
        let start = t(10, 12);
        let until = start + Duration::seconds(CELEBRATE_SECS);
        assert!(is_celebrating(Some(until), start));
        assert!(is_celebrating(Some(until), start + Duration::minutes(9)));
        assert!(!is_celebrating(Some(until), until));
        assert!(!is_celebrating(Some(until), until + Duration::seconds(1)));
        assert!(!is_celebrating(None, start));
    }

    #[test]
    fn keeps_unexpired_celebration_across_polls() {
        let now = t(10, 12);
        let mut prev = snap_codex_secondary(Some(t(17, 11)));
        let next = snap_codex_secondary(Some(t(17, 11)));
        let until = now + Duration::seconds(CELEBRATE_SECS);
        prev.celebrating_until = Some(until);

        // No new jump; previous celebration still live.
        let mid = now + Duration::minutes(5);
        let resolved = resolve_celebrating_until(Some(&prev), &next, mid);
        assert_eq!(resolved, Some(until));

        // After expiry, gone.
        let after = until + Duration::seconds(1);
        let resolved = resolve_celebrating_until(Some(&prev), &next, after);
        assert!(resolved.is_none());
    }

    #[test]
    fn fresh_start_no_prev_does_not_celebrate() {
        let now = t(10, 12);
        let next = snap_codex_secondary(Some(t(17, 11)));
        assert!(resolve_celebrating_until(None, &next, now).is_none());
    }
}
