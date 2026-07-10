//! Backfill missing `resets_at` / `window_minutes` from a previously cached snapshot.
//!
//! When a fresh provider poll omits reset metadata but the last good snapshot still has a
//! future `resets_at` for the same window, reuse it so the UI does not flicker empty.
//! Mirrors CodexBar's `RateWindow.backfillingResetTime(from:now:)`.

use chrono::{DateTime, Utc};

use crate::providers::{
    claude::ClaudeSnapshot, codex::CodexSnapshot, cursor::CursorSnapshot, grok::GrokSnapshot,
};
use crate::snapshot::Snapshot;

/// Windows that expose the reset / duration fields we can backfill.
pub trait ResetWindow {
    fn resets_at(&self) -> Option<DateTime<Utc>>;
    fn set_resets_at(&mut self, t: DateTime<Utc>);
    fn window_minutes(&self) -> Option<u32> {
        None
    }
    fn set_window_minutes(&mut self, _mins: Option<u32>) {}
}

/// When `fresh.resets_at` is missing, copy a still-future cached reset (and duration).
///
/// Never overwrites a fresh `Some(resets_at)`. Never copies a reset that is already past.
/// `window_minutes` is filled from cache only when the fresh value is missing or zero.
pub fn backfill_window<W: ResetWindow>(fresh: &mut W, cached: Option<&W>, now: DateTime<Utc>) {
    if fresh.resets_at().is_some() {
        return;
    }
    let Some(cached) = cached else {
        return;
    };
    let Some(cached_reset) = cached.resets_at() else {
        return;
    };
    if cached_reset <= now {
        return;
    }
    fresh.set_resets_at(cached_reset);

    let keep_mins = fresh.window_minutes().is_some_and(|m| m > 0);
    if !keep_mins {
        fresh.set_window_minutes(cached.window_minutes());
    }
}

fn backfill_opt<W: ResetWindow>(fresh: &mut Option<W>, cached: Option<&W>, now: DateTime<Utc>) {
    if let Some(w) = fresh.as_mut() {
        backfill_window(w, cached, now);
    }
}

/// Pair every provider window on `fresh` against the previous snapshot and backfill.
/// OpenCode has no rate windows and is skipped.
pub fn backfill_snapshot(fresh: &mut Snapshot, cached: Option<&Snapshot>, now: DateTime<Utc>) {
    let Some(cached) = cached else {
        return;
    };

    if let (Some(f), Some(c)) = (fresh.codex.as_mut(), cached.codex.as_ref()) {
        backfill_codex(f, c, now);
    }
    if let (Some(f), Some(c)) = (fresh.claude.as_mut(), cached.claude.as_ref()) {
        backfill_claude(f, c, now);
    }
    if let (Some(f), Some(c)) = (fresh.grok.as_mut(), cached.grok.as_ref()) {
        backfill_grok(f, c, now);
    }
    if let (Some(f), Some(c)) = (fresh.cursor.as_mut(), cached.cursor.as_ref()) {
        backfill_cursor(f, c, now);
    }
}

fn backfill_codex(fresh: &mut CodexSnapshot, cached: &CodexSnapshot, now: DateTime<Utc>) {
    backfill_opt(&mut fresh.primary, cached.primary.as_ref(), now);
    backfill_opt(&mut fresh.secondary, cached.secondary.as_ref(), now);
}

fn backfill_claude(fresh: &mut ClaudeSnapshot, cached: &ClaudeSnapshot, now: DateTime<Utc>) {
    backfill_opt(&mut fresh.session, cached.session.as_ref(), now);
    backfill_opt(&mut fresh.weekly, cached.weekly.as_ref(), now);
    backfill_opt(&mut fresh.sonnet_weekly, cached.sonnet_weekly.as_ref(), now);
}

fn backfill_grok(fresh: &mut GrokSnapshot, cached: &GrokSnapshot, now: DateTime<Utc>) {
    backfill_opt(&mut fresh.primary, cached.primary.as_ref(), now);
}

fn backfill_cursor(fresh: &mut CursorSnapshot, cached: &CursorSnapshot, now: DateTime<Utc>) {
    backfill_opt(&mut fresh.primary, cached.primary.as_ref(), now);
}

// --- ResetWindow impls -------------------------------------------------------

impl ResetWindow for crate::providers::codex::Window {
    fn resets_at(&self) -> Option<DateTime<Utc>> {
        self.resets_at
    }
    fn set_resets_at(&mut self, t: DateTime<Utc>) {
        self.resets_at = Some(t);
    }
    fn window_minutes(&self) -> Option<u32> {
        self.window_minutes
    }
    fn set_window_minutes(&mut self, mins: Option<u32>) {
        self.window_minutes = mins;
    }
}

impl ResetWindow for crate::providers::claude::Window {
    fn resets_at(&self) -> Option<DateTime<Utc>> {
        self.resets_at
    }
    fn set_resets_at(&mut self, t: DateTime<Utc>) {
        self.resets_at = Some(t);
    }
}

impl ResetWindow for crate::providers::grok::Window {
    fn resets_at(&self) -> Option<DateTime<Utc>> {
        self.resets_at
    }
    fn set_resets_at(&mut self, t: DateTime<Utc>) {
        self.resets_at = Some(t);
    }
    fn window_minutes(&self) -> Option<u32> {
        self.window_minutes
    }
    fn set_window_minutes(&mut self, mins: Option<u32>) {
        self.window_minutes = mins;
    }
}

impl ResetWindow for crate::providers::cursor::Window {
    fn resets_at(&self) -> Option<DateTime<Utc>> {
        self.resets_at
    }
    fn set_resets_at(&mut self, t: DateTime<Utc>) {
        self.resets_at = Some(t);
    }
    fn window_minutes(&self) -> Option<u32> {
        self.window_minutes
    }
    fn set_window_minutes(&mut self, mins: Option<u32>) {
        self.window_minutes = mins;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::codex::Window;
    use chrono::TimeZone;

    fn utc(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn window(used: f64, mins: Option<u32>, resets: Option<DateTime<Utc>>) -> Window {
        Window {
            used_percent: used,
            window_minutes: mins,
            resets_at: resets,
        }
    }

    #[test]
    fn backfills_none_from_cached_future() {
        let now = utc(1_800_000_000);
        let reset = now + chrono::Duration::hours(1);
        let cached = window(50.0, Some(300), Some(reset));
        let mut fresh = window(62.0, None, None);

        backfill_window(&mut fresh, Some(&cached), now);

        assert_eq!(fresh.used_percent, 62.0);
        assert_eq!(fresh.resets_at, Some(reset));
        assert_eq!(fresh.window_minutes, Some(300));
    }

    #[test]
    fn backfills_zero_window_minutes_from_cache() {
        let now = utc(1_800_000_000);
        let reset = now + chrono::Duration::hours(1);
        let cached = window(50.0, Some(300), Some(reset));
        let mut fresh = window(62.0, Some(0), None);

        backfill_window(&mut fresh, Some(&cached), now);

        assert_eq!(fresh.resets_at, Some(reset));
        assert_eq!(fresh.window_minutes, Some(300));
    }

    #[test]
    fn skips_cached_reset_already_past() {
        let now = utc(1_800_000_000);
        let cached = window(50.0, Some(300), Some(now - chrono::Duration::seconds(60)));
        let mut fresh = window(62.0, None, None);

        backfill_window(&mut fresh, Some(&cached), now);

        assert_eq!(fresh.resets_at, None);
        assert_eq!(fresh.window_minutes, None);
    }

    #[test]
    fn never_overwrites_fresh_some_resets_at() {
        let now = utc(1_800_000_000);
        let fresh_reset = now + chrono::Duration::hours(2);
        let cached_reset = now + chrono::Duration::hours(1);
        let cached = window(50.0, Some(300), Some(cached_reset));
        let mut fresh = window(62.0, Some(120), Some(fresh_reset));

        backfill_window(&mut fresh, Some(&cached), now);

        assert_eq!(fresh.resets_at, Some(fresh_reset));
        assert_eq!(fresh.window_minutes, Some(120));
    }

    #[test]
    fn preserves_fresh_positive_window_minutes_when_backfilling_reset() {
        let now = utc(1_800_000_000);
        let reset = now + chrono::Duration::hours(1);
        let cached = window(50.0, Some(300), Some(reset));
        let mut fresh = window(62.0, Some(180), None);

        backfill_window(&mut fresh, Some(&cached), now);

        assert_eq!(fresh.resets_at, Some(reset));
        assert_eq!(fresh.window_minutes, Some(180));
    }

    #[test]
    fn snapshot_pairs_provider_windows() {
        let now = utc(1_800_000_000);
        let reset = now + chrono::Duration::hours(1);

        let mut cached = Snapshot::new();
        cached.codex = Some(CodexSnapshot {
            account_email: None,
            plan_type: None,
            primary: Some(window(40.0, Some(300), Some(reset))),
            secondary: Some(window(
                10.0,
                Some(10080),
                Some(reset + chrono::Duration::days(3)),
            )),
            credits: None,
            reset_credits: None,
            error: None,
        });
        cached.claude = Some(ClaudeSnapshot {
            account_email: None,
            plan_type: None,
            session: Some(crate::providers::claude::Window {
                used_percent: 20.0,
                resets_at: Some(reset),
            }),
            weekly: None,
            sonnet_weekly: None,
            extra: None,
            source: None,
            error: None,
        });

        let mut fresh = Snapshot::new();
        fresh.codex = Some(CodexSnapshot {
            account_email: Some("a@b.c".into()),
            plan_type: Some("plus".into()),
            primary: Some(window(66.0, None, None)),
            secondary: Some(window(12.0, None, None)),
            credits: None,
            reset_credits: None,
            error: None,
        });
        fresh.claude = Some(ClaudeSnapshot {
            account_email: None,
            plan_type: None,
            session: Some(crate::providers::claude::Window {
                used_percent: 55.0,
                resets_at: None,
            }),
            weekly: None,
            sonnet_weekly: None,
            extra: None,
            source: Some("api".into()),
            error: None,
        });

        backfill_snapshot(&mut fresh, Some(&cached), now);

        let c = fresh.codex.as_ref().unwrap();
        assert_eq!(c.primary.as_ref().unwrap().resets_at, Some(reset));
        assert_eq!(c.primary.as_ref().unwrap().window_minutes, Some(300));
        assert_eq!(c.primary.as_ref().unwrap().used_percent, 66.0);
        assert_eq!(
            c.secondary.as_ref().unwrap().resets_at,
            Some(reset + chrono::Duration::days(3))
        );
        assert_eq!(c.secondary.as_ref().unwrap().window_minutes, Some(10080));
        assert_eq!(c.account_email.as_deref(), Some("a@b.c"));

        let cl = fresh.claude.as_ref().unwrap();
        assert_eq!(cl.session.as_ref().unwrap().resets_at, Some(reset));
        assert_eq!(cl.session.as_ref().unwrap().used_percent, 55.0);
        assert_eq!(cl.source.as_deref(), Some("api"));
    }

    #[test]
    fn snapshot_noop_without_cache() {
        let now = utc(1_800_000_000);
        let mut fresh = Snapshot::new();
        fresh.codex = Some(CodexSnapshot {
            account_email: None,
            plan_type: None,
            primary: Some(window(50.0, None, None)),
            secondary: None,
            credits: None,
            reset_credits: None,
            error: None,
        });
        backfill_snapshot(&mut fresh, None, now);
        assert_eq!(
            fresh
                .codex
                .as_ref()
                .unwrap()
                .primary
                .as_ref()
                .unwrap()
                .resets_at,
            None
        );
    }
}
