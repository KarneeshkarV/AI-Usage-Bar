use anyhow::Result;
use std::io::Write;
use tokio::time::{Duration, interval};

use crate::config::Config;
use crate::cost;
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

        // Atomic snapshot write so `ai_bar status` can read it
        if let Err(e) = snapshot::write(&snap) {
            tracing::warn!(error = %e, "snapshot write failed");
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
