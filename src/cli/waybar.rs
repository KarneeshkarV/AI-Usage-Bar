use anyhow::Result;
use chrono::{DateTime, Utc};
use std::io::Write;
use tokio::time::{Duration, interval};

use crate::config::Config;
use crate::cost;
use crate::ping;
use crate::providers::{claude, codex};
use crate::snapshot::{self, Snapshot};
use crate::waybar_proto::WaybarLine;

pub async fn run() -> Result<()> {
    let cfg = Config::load_or_default()?;
    let mut codex_client = codex::Client::new(cfg.providers.codex.clone());
    let mut claude_client = claude::Client::new(cfg.providers.claude.clone());

    let mut tick = interval(Duration::from_secs(cfg.refresh.interval_secs.max(30)));
    let mut cost_tick = interval(Duration::from_secs(cfg.refresh.cost_refresh_secs.max(60)));
    let mut last_cost: Option<cost::CostReport> = None;
    let mut last_ping_claude: Option<DateTime<Utc>> = None;

    let stdout = std::io::stdout();

    loop {
        let mut snap = Snapshot::new();

        // Provider polling (independent, parallel)
        let (codex_res, claude_res) =
            tokio::join!(codex_client.refresh(), claude_client.refresh(),);
        snap.codex = codex_res.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "codex refresh failed");
            None
        });
        snap.claude = claude_res.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "claude refresh failed");
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
                maybe_ping_claude(&cfg, reset, &mut last_ping_claude);
            }
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

/// Fire a single Claude ping per session-window boundary as it approaches reset.
/// Dedups on the `reset` timestamp; runs the ping in the background so the
/// Waybar loop never blocks on the LLM call.
fn maybe_ping_claude(cfg: &Config, reset: DateTime<Utc>, last: &mut Option<DateTime<Utc>>) {
    let secs = reset.signed_duration_since(Utc::now()).num_seconds();
    // Anchor the NEXT window: only fire once the reset has passed, so this ping
    // is the first activity of the fresh window and pins its start. `secs <= 0`
    // means reset reached; the lower bound ignores stale far-past snapshots.
    if !(-(cfg.ping.threshold_secs as i64)..=0).contains(&secs) {
        return;
    }
    if *last == Some(reset) {
        return;
    }
    *last = Some(reset);

    let binary = cfg.providers.claude.binary.clone();
    let model = cfg.ping.claude_model.clone();
    tokio::spawn(async move {
        match ping::ping_claude(binary.as_deref(), &model).await {
            Ok(()) => tracing::info!("claude pre-reset ping sent"),
            Err(e) => tracing::warn!(error = %e, "claude ping failed"),
        }
    });
}
