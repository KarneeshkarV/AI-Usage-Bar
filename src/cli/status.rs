use anyhow::Result;

use crate::config::Config;
use crate::providers::{claude, codex};
use crate::snapshot::{self, Snapshot};

pub async fn run(detailed: bool) -> Result<()> {
    // Prefer the cached snapshot written by `ai-usage-bar waybar`. Fall back to a
    // synchronous one-shot poll if no daemon has written one yet.
    let snap = match snapshot::read() {
        Ok(s) if !s.is_stale(std::time::Duration::from_secs(30 * 60)) => s,
        _ => one_shot().await?,
    };

    if detailed {
        print_detailed(&snap);
    } else {
        print_compact(&snap);
    }
    Ok(())
}

async fn one_shot() -> Result<Snapshot> {
    let cfg = Config::load_or_default()?;
    let mut snap = Snapshot::new();
    let mut codex_client = codex::Client::new(cfg.providers.codex.clone());
    let mut claude_client = claude::Client::new(cfg.providers.claude.clone());

    let (codex_res, claude_res) = tokio::join!(codex_client.refresh(), claude_client.refresh(),);
    snap.codex = codex_res.ok().flatten();
    snap.claude = claude_res.ok().flatten();
    snap.cost = crate::cost::scan_both().await.ok();
    snap.refreshed_at = chrono::Utc::now();
    Ok(snap)
}

fn print_compact(snap: &Snapshot) {
    println!("AI Usage Bar — {}", snap.refreshed_at.to_rfc3339());
    if let Some(c) = &snap.codex {
        println!("codex: {}", c.summary_line());
    } else {
        println!("codex: (unavailable)");
    }
    if let Some(c) = &snap.claude {
        println!("claude: {}", c.summary_line());
    } else {
        println!("claude: (unavailable)");
    }
    if let Some(cost) = &snap.cost {
        println!("30d cost: ${:.2}", cost.total_usd);
    }
}

fn print_detailed(snap: &Snapshot) {
    println!("╭─ AI Usage Bar — usage snapshot");
    println!("│  refreshed: {}", snap.refreshed_at.to_rfc3339());
    println!("├─ Codex");
    if let Some(c) = &snap.codex {
        for line in c.detail_lines() {
            println!("│  {line}");
        }
    } else {
        println!("│  (unavailable)");
    }
    println!("├─ Claude");
    if let Some(c) = &snap.claude {
        for line in c.detail_lines() {
            println!("│  {line}");
        }
    } else {
        println!("│  (unavailable)");
    }
    println!("├─ 30-day cost");
    if let Some(cost) = &snap.cost {
        for line in cost.detail_lines() {
            println!("│  {line}");
        }
    } else {
        println!("│  (not scanned)");
    }
    println!("╰─");
}
