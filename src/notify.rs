//! Desktop notifications for quota threshold crossings and window resets.
//!
//! Driven only from the waybar poll loop (the resident process). Uses
//! `notify-send` when present; if the binary is missing, notifications disable
//! silently.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;

use crate::config::{Config, cache_dir};
use crate::snapshot::Snapshot;
use crate::util::time::until;

/// How far usage must fall (percentage points) to count as a reset, together
/// with `resets_at` moving forward.
const RESET_DROP_POINTS: f64 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

impl Urgency {
    fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Warn,
    Crit,
    Reset,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotifyEvent {
    pub provider: String,
    pub provider_label: String,
    pub window: String,
    pub kind: EventKind,
    pub used_percent: u8,
    pub resets_at: Option<DateTime<Utc>>,
}

impl NotifyEvent {
    pub fn urgency(&self) -> Urgency {
        match self.kind {
            EventKind::Warn => Urgency::Normal,
            EventKind::Crit => Urgency::Critical,
            EventKind::Reset => Urgency::Low,
        }
    }

    pub fn summary(&self) -> String {
        match self.kind {
            EventKind::Warn | EventKind::Crit => {
                format!("{} usage {}%", self.provider_label, self.used_percent)
            }
            EventKind::Reset => {
                format!("{} window reset · 100% available", self.provider_label)
            }
        }
    }

    pub fn body(&self) -> String {
        match self.kind {
            EventKind::Warn | EventKind::Crit => {
                let mut body = format!("{} window", self.window);
                if let Some(t) = self.resets_at {
                    body.push_str(&format!(" · resets in {}", until(t)));
                }
                body
            }
            EventKind::Reset => format!("{} window", self.window),
        }
    }

    /// Dedupe key: provider + window + kind + resets_at (epoch secs or 0).
    pub fn dedupe_key(&self) -> String {
        let reset_key = self
            .resets_at
            .map(|t| t.timestamp().to_string())
            .unwrap_or_else(|| "0".into());
        format!(
            "{}:{}:{}:{}",
            self.provider,
            self.window,
            match self.kind {
                EventKind::Warn => "warn",
                EventKind::Crit => "crit",
                EventKind::Reset => "reset",
            },
            reset_key
        )
    }
}

/// Pure event detection: compare previous and fresh snapshots.
///
/// With no previous snapshot, returns nothing (fresh start / first poll).
pub fn detect_events(
    prev: Option<&Snapshot>,
    next: &Snapshot,
    warn_threshold: u8,
    crit_threshold: u8,
    on_reset: bool,
) -> Vec<NotifyEvent> {
    let Some(prev) = prev else {
        return Vec::new();
    };

    let mut events = Vec::new();

    for w in collect_windows(prev, next) {
        let prev_u = w.prev_pct.round().clamp(0.0, 100.0) as u8;
        let next_u = w.next_pct.round().clamp(0.0, 100.0) as u8;

        // Threshold crossings upward (highest first so a jump past crit is one event).
        if next_u >= crit_threshold && prev_u < crit_threshold {
            events.push(NotifyEvent {
                provider: w.provider.to_string(),
                provider_label: w.label.to_string(),
                window: w.window.to_string(),
                kind: EventKind::Crit,
                used_percent: next_u,
                resets_at: w.next_reset,
            });
        } else if next_u >= warn_threshold && prev_u < warn_threshold {
            events.push(NotifyEvent {
                provider: w.provider.to_string(),
                provider_label: w.label.to_string(),
                window: w.window.to_string(),
                kind: EventKind::Warn,
                used_percent: next_u,
                resets_at: w.next_reset,
            });
        }

        // Reset: was ≥ warn, dropped a lot, and resets_at moved forward.
        if on_reset
            && prev_u >= warn_threshold
            && w.next_pct + RESET_DROP_POINTS <= w.prev_pct
            && resets_moved_forward(w.prev_reset, w.next_reset)
        {
            events.push(NotifyEvent {
                provider: w.provider.to_string(),
                provider_label: w.label.to_string(),
                window: w.window.to_string(),
                kind: EventKind::Reset,
                used_percent: next_u,
                resets_at: w.next_reset,
            });
        }
    }

    events
}

fn resets_moved_forward(prev: Option<DateTime<Utc>>, next: Option<DateTime<Utc>>) -> bool {
    match (prev, next) {
        (Some(p), Some(n)) => n > p,
        _ => false,
    }
}

struct WindowPair {
    provider: &'static str,
    label: &'static str,
    window: &'static str,
    prev_pct: f64,
    prev_reset: Option<DateTime<Utc>>,
    next_pct: f64,
    next_reset: Option<DateTime<Utc>>,
}

fn collect_windows(prev: &Snapshot, next: &Snapshot) -> Vec<WindowPair> {
    let mut out = Vec::new();

    if let (Some(p), Some(n)) = (&prev.codex, &next.codex) {
        push_pair(
            &mut out,
            "codex",
            "Codex",
            "primary",
            p.primary.as_ref().map(|w| (w.used_percent, w.resets_at)),
            n.primary.as_ref().map(|w| (w.used_percent, w.resets_at)),
        );
        push_pair(
            &mut out,
            "codex",
            "Codex",
            "secondary",
            p.secondary.as_ref().map(|w| (w.used_percent, w.resets_at)),
            n.secondary.as_ref().map(|w| (w.used_percent, w.resets_at)),
        );
    }
    if let (Some(p), Some(n)) = (&prev.claude, &next.claude) {
        push_pair(
            &mut out,
            "claude",
            "Claude",
            "session",
            p.session.as_ref().map(|w| (w.used_percent, w.resets_at)),
            n.session.as_ref().map(|w| (w.used_percent, w.resets_at)),
        );
        push_pair(
            &mut out,
            "claude",
            "Claude",
            "weekly",
            p.weekly.as_ref().map(|w| (w.used_percent, w.resets_at)),
            n.weekly.as_ref().map(|w| (w.used_percent, w.resets_at)),
        );
        push_pair(
            &mut out,
            "claude",
            "Claude",
            "sonnet",
            p.sonnet_weekly
                .as_ref()
                .map(|w| (w.used_percent, w.resets_at)),
            n.sonnet_weekly
                .as_ref()
                .map(|w| (w.used_percent, w.resets_at)),
        );
    }
    if let (Some(p), Some(n)) = (&prev.grok, &next.grok) {
        push_pair(
            &mut out,
            "grok",
            "Grok",
            "primary",
            p.primary.as_ref().map(|w| (w.used_percent, w.resets_at)),
            n.primary.as_ref().map(|w| (w.used_percent, w.resets_at)),
        );
    }
    if let (Some(p), Some(n)) = (&prev.cursor, &next.cursor) {
        push_pair(
            &mut out,
            "cursor",
            "Cursor",
            "primary",
            p.primary.as_ref().map(|w| (w.used_percent, w.resets_at)),
            n.primary.as_ref().map(|w| (w.used_percent, w.resets_at)),
        );
    }

    out
}

fn push_pair(
    out: &mut Vec<WindowPair>,
    provider: &'static str,
    label: &'static str,
    window: &'static str,
    prev: Option<(f64, Option<DateTime<Utc>>)>,
    next: Option<(f64, Option<DateTime<Utc>>)>,
) {
    let Some((prev_pct, prev_reset)) = prev else {
        return;
    };
    let Some((next_pct, next_reset)) = next else {
        return;
    };
    out.push(WindowPair {
        provider,
        label,
        window,
        prev_pct,
        prev_reset,
        next_pct,
        next_reset,
    });
}

// ── Runtime notifier ────────────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, Deserialize)]
struct DedupeStore {
    /// Keys already notified: `provider:window:kind:resets_at`.
    keys: HashSet<String>,
}

pub struct Notifier {
    enabled: bool,
    on_reset: bool,
    warn_threshold: u8,
    crit_threshold: u8,
    notify_send: Option<PathBuf>,
    dedupe: DedupeStore,
    dedupe_path: PathBuf,
}

impl Notifier {
    pub fn from_config(cfg: &Config) -> Self {
        let notify_send = if cfg.notify.enabled {
            which::which("notify-send").ok()
        } else {
            None
        };
        if cfg.notify.enabled && notify_send.is_none() {
            tracing::debug!("notify-send not found; desktop notifications disabled");
        }
        let dedupe_path = cache_dir()
            .map(|d| d.join("notify-dedupe.json"))
            .unwrap_or_else(|_| PathBuf::from("notify-dedupe.json"));
        let dedupe = load_dedupe(&dedupe_path);
        Self {
            enabled: cfg.notify.enabled && notify_send.is_some(),
            on_reset: cfg.notify.on_reset,
            warn_threshold: cfg.display.warn_threshold,
            crit_threshold: cfg.display.crit_threshold,
            notify_send,
            dedupe,
            dedupe_path,
        }
    }

    /// Compare snapshots, fire new events, update dedupe state.
    pub async fn process(&mut self, prev: Option<&Snapshot>, next: &Snapshot) {
        if !self.enabled {
            return;
        }

        // Drop keys for windows whose resets_at no longer matches, so a later
        // re-cross in a new period can fire again.
        let live_periods = current_period_keys(next);
        self.dedupe
            .keys
            .retain(|k| period_still_live(k, &live_periods));

        let events = detect_events(
            prev,
            next,
            self.warn_threshold,
            self.crit_threshold,
            self.on_reset,
        );
        let mut dirty = false;
        for ev in events {
            let key = ev.dedupe_key();
            if self.dedupe.keys.contains(&key) {
                continue;
            }
            if self.send(&ev).await {
                self.dedupe.keys.insert(key);
                dirty = true;
            }
        }
        if dirty {
            save_dedupe(&self.dedupe_path, &self.dedupe);
        }
    }

    async fn send(&self, ev: &NotifyEvent) -> bool {
        let Some(bin) = &self.notify_send else {
            return false;
        };
        let summary = ev.summary();
        let body = ev.body();
        let urgency = ev.urgency().as_str();
        let bin = bin.clone();
        let result = tokio::process::Command::new(&bin)
            .arg("-a")
            .arg("AI Usage")
            .arg("-u")
            .arg(urgency)
            .arg(&summary)
            .arg(&body)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        match result {
            Ok(status) if status.success() => true,
            Ok(status) => {
                tracing::debug!(?status, "notify-send exited non-zero");
                false
            }
            Err(e) => {
                tracing::debug!(error = %e, "notify-send spawn failed");
                false
            }
        }
    }
}

/// Set of `provider:window:resets_at` for windows present in `snap`.
fn current_period_keys(snap: &Snapshot) -> HashSet<String> {
    let mut keys = HashSet::new();
    // Pair snap with itself so collect_windows yields every present window.
    for w in collect_windows(snap, snap) {
        let reset_key = w
            .next_reset
            .map(|t| t.timestamp().to_string())
            .unwrap_or_else(|| "0".into());
        keys.insert(format!("{}:{}:{reset_key}", w.provider, w.window));
    }
    keys
}

/// A dedupe key `provider:window:kind:resets_at` is live when its period still
/// matches a current window.
fn period_still_live(dedupe_key: &str, live_periods: &HashSet<String>) -> bool {
    let parts: Vec<&str> = dedupe_key.splitn(4, ':').collect();
    if parts.len() != 4 {
        return false;
    }
    let period = format!("{}:{}:{}", parts[0], parts[1], parts[3]);
    live_periods.contains(&period)
}

fn load_dedupe(path: &PathBuf) -> DedupeStore {
    let Ok(raw) = std::fs::read(path) else {
        return DedupeStore::default();
    };
    serde_json::from_slice(&raw).unwrap_or_default()
}

fn save_dedupe(path: &PathBuf, store: &DedupeStore) {
    if let Ok(json) = serde_json::to_vec_pretty(store) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::codex::{CodexSnapshot, Window as CodexWindow};
    use chrono::TimeZone;

    fn codex_snap(primary_pct: f64, resets: Option<DateTime<Utc>>) -> Snapshot {
        let mut s = Snapshot::new();
        s.codex = Some(CodexSnapshot {
            account_email: None,
            plan_type: None,
            primary: Some(CodexWindow {
                used_percent: primary_pct,
                window_minutes: Some(300),
                resets_at: resets,
            }),
            secondary: None,
            credits: None,
            error: None,
        });
        s
    }

    fn t(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 10, hour, 0, 0).unwrap()
    }

    #[test]
    fn fresh_start_fires_nothing() {
        let next = codex_snap(80.0, Some(t(12)));
        let events = detect_events(None, &next, 70, 90, true);
        assert!(events.is_empty());
    }

    #[test]
    fn crosses_warn_upward() {
        let prev = codex_snap(60.0, Some(t(12)));
        let next = codex_snap(72.0, Some(t(12)));
        let events = detect_events(Some(&prev), &next, 70, 90, true);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Warn);
        assert_eq!(events[0].used_percent, 72);
        assert_eq!(events[0].urgency(), Urgency::Normal);
        assert!(events[0].summary().contains("72%"));
        assert!(events[0].body().contains("primary"));
    }

    #[test]
    fn crosses_crit_upward() {
        let prev = codex_snap(85.0, Some(t(12)));
        let next = codex_snap(92.0, Some(t(12)));
        let events = detect_events(Some(&prev), &next, 70, 90, true);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Crit);
        assert_eq!(events[0].urgency(), Urgency::Critical);
    }

    #[test]
    fn jump_past_both_fires_crit_only() {
        let prev = codex_snap(50.0, Some(t(12)));
        let next = codex_snap(95.0, Some(t(12)));
        let events = detect_events(Some(&prev), &next, 70, 90, true);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Crit);
    }

    #[test]
    fn no_repeat_while_staying_above() {
        let prev = codex_snap(75.0, Some(t(12)));
        let next = codex_snap(80.0, Some(t(12)));
        let events = detect_events(Some(&prev), &next, 70, 90, true);
        assert!(
            events.is_empty(),
            "already above warn; should not re-fire: {events:?}"
        );
    }

    #[test]
    fn reset_detection() {
        let prev = codex_snap(80.0, Some(t(10)));
        let next = codex_snap(5.0, Some(t(15)));
        let events = detect_events(Some(&prev), &next, 70, 90, true);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Reset);
        assert_eq!(events[0].urgency(), Urgency::Low);
        assert!(events[0].summary().contains("100% available"));
    }

    #[test]
    fn reset_requires_on_reset_flag() {
        let prev = codex_snap(80.0, Some(t(10)));
        let next = codex_snap(5.0, Some(t(15)));
        let events = detect_events(Some(&prev), &next, 70, 90, false);
        assert!(events.is_empty());
    }

    #[test]
    fn reset_requires_drop_and_forward_resets_at() {
        // Drop without resets_at moving: not a reset.
        let prev = codex_snap(80.0, Some(t(12)));
        let next = codex_snap(5.0, Some(t(12)));
        let events = detect_events(Some(&prev), &next, 70, 90, true);
        assert!(events.is_empty());

        // resets_at moves but usage barely drops: not a reset.
        let prev = codex_snap(80.0, Some(t(10)));
        let next = codex_snap(75.0, Some(t(15)));
        let events = detect_events(Some(&prev), &next, 70, 90, true);
        assert!(events.is_empty());
    }

    #[test]
    fn re_cross_after_dip_fires_again() {
        let prev = codex_snap(60.0, Some(t(12)));
        let next = codex_snap(75.0, Some(t(12)));
        let events = detect_events(Some(&prev), &next, 70, 90, true);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Warn);
    }

    #[test]
    fn dedupe_key_stable() {
        let prev = codex_snap(60.0, Some(t(12)));
        let next = codex_snap(75.0, Some(t(12)));
        let events = detect_events(Some(&prev), &next, 70, 90, true);
        let k1 = events[0].dedupe_key();
        let k2 = events[0].dedupe_key();
        assert_eq!(k1, k2);
        assert!(k1.contains("codex"));
        assert!(k1.contains("warn"));
    }
}
