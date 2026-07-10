use anyhow::Result;
use chrono::{DateTime, Utc};
use std::io::Write;
use tokio::time::{Duration, interval};

use crate::config::Config;
use crate::cost;
use crate::ping;
use crate::providers::{claude, codex, cursor, grok, opencode};
use crate::snapshot::{self, Snapshot};
use crate::waybar_proto::WaybarLine;

pub async fn run() -> Result<()> {
    let cfg = Config::load_or_default()?;
    let mut codex_client = codex::Client::new(cfg.providers.codex.clone());
    let mut claude_client = claude::Client::new(cfg.providers.claude.clone());
    let mut grok_client = grok::Client::new(cfg.providers.grok.clone());
    let mut cursor_client = cursor::Client::new(cfg.providers.cursor.clone());
    let mut opencode_client = opencode::Client::new(cfg.providers.opencode.clone());

    let mut tick = interval(Duration::from_secs(cfg.refresh.interval_secs.max(30)));
    let mut cost_tick = interval(Duration::from_secs(cfg.refresh.cost_refresh_secs.max(60)));
    let mut last_cost: Option<cost::CostReport> = None;
    let mut last_ping_claude: Option<DateTime<Utc>> = None;
    let mut last_ping_codex: Option<DateTime<Utc>> = None;
    let mut prev_reset_claude: Option<DateTime<Utc>> = None;
    let mut prev_reset_codex: Option<DateTime<Utc>> = None;

    let stdout = std::io::stdout();

    loop {
        let mut snap = Snapshot::new();

        // Provider polling (independent, parallel)
        let (codex_res, claude_res, grok_res, cursor_res, opencode_res) = tokio::join!(
            codex_client.refresh(),
            claude_client.refresh(),
            grok_client.refresh(),
            cursor_client.refresh(),
            opencode_client.refresh(),
        );
        snap.codex = codex_res.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "codex refresh failed");
            None
        });
        snap.claude = claude_res.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "claude refresh failed");
            None
        });
        snap.grok = grok_res.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "grok refresh failed");
            None
        });
        snap.cursor = cursor_res.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "cursor refresh failed");
            None
        });
        snap.opencode = opencode_res.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "opencode refresh failed");
            None
        });

        // Cost: refresh on the slower cadence
        if last_cost.is_none() {
            last_cost = cost::scan_both().await.ok();
        }
        snap.cost = last_cost.clone();
        snap.refreshed_at = chrono::Utc::now();

        // Atomic snapshot write so `ai-usage-bar status` can read it
        if let Err(e) = snapshot::write(&snap) {
            tracing::warn!(error = %e, "snapshot write failed");
        }

        // Anchor the next session window with a headless ping near its reset.
        if cfg.ping.enabled {
            let claude_reset = snap
                .claude
                .as_ref()
                .and_then(|c| c.session.as_ref())
                .and_then(|w| w.resets_at);
            if let Some(reset) = claude_reset {
                maybe_ping_claude(&cfg, reset, prev_reset_claude, &mut last_ping_claude);
            }
            prev_reset_claude = claude_reset;

            let codex_reset = snap
                .codex
                .as_ref()
                .and_then(|c| c.primary.as_ref())
                .and_then(|w| w.resets_at);
            if let Some(reset) = codex_reset {
                maybe_ping_codex(&cfg, reset, prev_reset_codex, &mut last_ping_codex);
            }
            prev_reset_codex = codex_reset;
        }

        // Build + emit Waybar line
        let line = WaybarLine::from_snapshot(&snap, &cfg);
        let json = serde_json::to_string(&line)?;
        {
            let mut out = stdout.lock();
            writeln!(out, "{json}")?;
            out.flush().ok();
        }

        tokio::select! {
            _ = tick.tick() => {},
            _ = cost_tick.tick() => {
                last_cost = cost::scan_both().await.ok();
            }
        }
    }
}

/// Seconds past the window reset to wait before pinging, so we anchor the new
/// window only once we are safely *over* the old limit (clock-skew buffer).
const OVER_LIMIT_SECS: i64 = 10;

/// A forward jump in `resets_at` of at least this many seconds means the old
/// window reset slipped past between polls (a genuine ~5h window roll-over, not
/// a few seconds of API jitter).
const JUMP_MIN_SECS: i64 = 3600;

/// Decide whether to anchor the window that resets at `reset`, returning how
/// many seconds to wait before firing. `prev` is the `resets_at` seen on the
/// previous poll. Dedups on the reset timestamp; `None` means don't fire now.
fn ping_delay(
    cfg: &Config,
    reset: DateTime<Utc>,
    prev: Option<DateTime<Utc>>,
    last: &mut Option<DateTime<Utc>>,
) -> Option<u64> {
    // Fallback: `resets_at` jumped forward — the previous reset passed between
    // polls and a new far-out window appeared before we could schedule it. If
    // that old boundary was never anchored, fire immediately so the new window
    // still gets its ping. Mark the missed boundary handled (not `reset`, so the
    // new window is still anchored normally when it later reaches its own reset).
    if let Some(prev) = prev {
        let now = Utc::now();
        if prev <= now
            && reset > now
            && reset - prev > chrono::Duration::seconds(JUMP_MIN_SECS)
            && *last != Some(prev)
        {
            *last = Some(prev);
            return Some(0);
        }
    }

    // Normal path: schedule the ping ~10s past this reset.
    let fire_at = reset + chrono::Duration::seconds(OVER_LIMIT_SECS);
    let delay = fire_at.signed_duration_since(Utc::now()).num_seconds();
    // Eligible only within `threshold_secs` either side of the fire time: the
    // upper side lets us schedule a boundary seen on the prior poll, the lower
    // side drops stale far-past resets from an old snapshot.
    let grace = cfg.ping.threshold_secs as i64;
    if !(-grace..=grace).contains(&delay) {
        return None;
    }
    if *last == Some(reset) {
        return None;
    }
    *last = Some(reset);
    Some(delay.max(0) as u64)
}

/// Schedule a single Claude ping ~10s after the session window resets, so the
/// ping is the first activity of the fresh window and pins its start. Runs in
/// the background so the Waybar loop never blocks on the LLM call.
fn maybe_ping_claude(
    cfg: &Config,
    reset: DateTime<Utc>,
    prev: Option<DateTime<Utc>>,
    last: &mut Option<DateTime<Utc>>,
) {
    let Some(delay) = ping_delay(cfg, reset, prev, last) else {
        return;
    };
    let binary = cfg.providers.claude.binary.clone();
    let model = cfg.ping.claude_model.clone();
    tokio::spawn(async move {
        if delay > 0 {
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
        match ping::ping_claude(binary.as_deref(), &model).await {
            Ok(()) => tracing::info!("claude post-reset ping sent"),
            Err(e) => tracing::warn!(error = %e, "claude ping failed"),
        }
    });
}

/// Codex counterpart to [`maybe_ping_claude`], keyed off the primary 5h window.
fn maybe_ping_codex(
    cfg: &Config,
    reset: DateTime<Utc>,
    prev: Option<DateTime<Utc>>,
    last: &mut Option<DateTime<Utc>>,
) {
    let Some(delay) = ping_delay(cfg, reset, prev, last) else {
        return;
    };
    let binary = cfg.providers.codex.binary.clone();
    let model = cfg.ping.codex_model.clone();
    let reasoning = cfg.ping.codex_reasoning.clone();
    tokio::spawn(async move {
        if delay > 0 {
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
        match ping::ping_codex(binary.as_deref(), &model, &reasoning).await {
            Ok(()) => tracing::info!("codex post-reset ping sent"),
            Err(e) => tracing::warn!(error = %e, "codex ping failed"),
        }
    });
}
