use anyhow::Result;

use crate::config::{Config, ResetStyle};
use crate::provider_status;
use crate::providers::{claude, codex, cursor, grok, opencode};
use crate::snapshot::{self, Snapshot};
use crate::util::spark;

/// Cached Waybar snapshot when it is younger than 30 minutes, else a live poll.
pub async fn load_snapshot() -> Result<Snapshot> {
    match snapshot::read() {
        Ok(s) if !s.is_stale(std::time::Duration::from_secs(30 * 60)) => Ok(s),
        _ => one_shot().await,
    }
}

pub async fn run(detailed: bool) -> Result<()> {
    let snap = load_snapshot().await?;

    let cfg = Config::load_or_default().unwrap_or_default();
    let style = cfg.display.reset_style;
    let show_cost = cfg.display.show_cost;

    if detailed {
        print_detailed(&snap, style, show_cost);
    } else {
        print_compact(&snap, show_cost);
    }
    Ok(())
}

async fn one_shot() -> Result<Snapshot> {
    let cfg = Config::load_or_default()?;
    let mut snap = Snapshot::new();
    let mut codex_client = codex::Client::new(cfg.providers.codex.clone());
    let mut claude_client = claude::Client::new(cfg.providers.claude.clone());
    let mut grok_client = grok::Client::new(cfg.providers.grok.clone());
    let mut cursor_client = cursor::Client::new(cfg.providers.cursor.clone());
    let mut opencode_client = opencode::Client::new(cfg.providers.opencode.clone());

    let (codex_res, claude_res, grok_res, cursor_res, opencode_res) = tokio::join!(
        codex_client.refresh(),
        claude_client.refresh(),
        grok_client.refresh(),
        cursor_client.refresh(),
        opencode_client.refresh(),
    );
    snap.codex = codex_res.ok().flatten();
    snap.claude = claude_res.ok().flatten();
    snap.grok = grok_res.ok().flatten();
    snap.cursor = cursor_res.ok().flatten();
    snap.opencode = opencode_res.ok().flatten();
    snap.cost = crate::cost::scan_both().await.ok();
    snap.refreshed_at = chrono::Utc::now();
    Ok(snap)
}

fn print_compact(snap: &Snapshot, show_cost: bool) {
    println!("AI Usage Bar — {}", snap.refreshed_at.to_rfc3339());
    if let Some(c) = &snap.codex {
        println!("codex: {}", c.summary_line());
        print_status_lines(snap, "codex");
    } else {
        println!("codex: (unavailable)");
    }
    if let Some(c) = &snap.claude {
        println!("claude: {}", c.summary_line());
        print_status_lines(snap, "claude");
    } else {
        println!("claude: (unavailable)");
    }
    if let Some(g) = &snap.grok {
        println!("grok: {}", g.summary_line());
    } else {
        println!("grok: (unavailable)");
    }
    // Cursor / OpenCode: omit when inactive.
    if let Some(o) = &snap.opencode {
        println!("opencode: {}", o.summary_line());
    }
    if let Some(c) = &snap.cursor {
        println!("cursor: {}", c.summary_line());
        print_status_lines(snap, "cursor");
    }
    if show_cost && let Some(cost) = &snap.cost {
        println!("30d cost: ${:.2}", cost.total_usd);
    }
}

fn print_detailed(snap: &Snapshot, style: ResetStyle, show_cost: bool) {
    println!("╭─ AI Usage Bar — usage snapshot");
    println!("│  refreshed: {}", snap.refreshed_at.to_rfc3339());
    println!("├─ Codex");
    if let Some(c) = &snap.codex {
        for line in c.detail_lines(style) {
            println!("│  {line}");
        }
        print_status_lines_detail(snap, "codex");
    } else {
        println!("│  (unavailable)");
    }
    println!("├─ Claude");
    if let Some(c) = &snap.claude {
        for line in c.detail_lines(style) {
            println!("│  {line}");
        }
        print_status_lines_detail(snap, "claude");
    } else {
        println!("│  (unavailable)");
    }
    println!("├─ Grok");
    if let Some(g) = &snap.grok {
        for line in g.detail_lines(style) {
            println!("│  {line}");
        }
    } else {
        println!("│  (unavailable)");
    }
    if snap.opencode.is_some() {
        println!("├─ OpenCode");
        if let Some(o) = &snap.opencode {
            for line in o.detail_lines(style) {
                println!("│  {line}");
            }
        }
    }
    if snap.cursor.is_some() {
        println!("├─ Cursor");
        if let Some(c) = &snap.cursor {
            for line in c.detail_lines(style) {
                println!("│  {line}");
            }
            print_status_lines_detail(snap, "cursor");
        }
    }
    if show_cost {
        println!("├─ 30-day cost");
        if let Some(cost) = &snap.cost {
            for line in cost.detail_lines() {
                println!("│  {line}");
            }
            let today = chrono::Utc::now().date_naive();
            let series = spark::daily_series(&cost.by_day, today, 30);
            let values: Vec<f64> = series.iter().map(|(_, v)| *v).collect();
            let bars = spark::unicode_sparkline(&values);
            let caption = spark::cost_caption(&series, cost.total_usd, today);
            println!("│  {bars}");
            println!("│  {caption}");
        } else {
            println!("│  (not scanned)");
        }
    }
    println!("╰─");
}

fn print_status_lines(snap: &Snapshot, provider: &str) {
    if let Some(line) =
        provider_status::find(&snap.provider_status, provider).and_then(|s| s.display_line())
    {
        println!("{line}");
    }
}

fn print_status_lines_detail(snap: &Snapshot, provider: &str) {
    if let Some(line) =
        provider_status::find(&snap.provider_status, provider).and_then(|s| s.display_line())
    {
        // display_line already has two-space indent; strip and re-indent for the box.
        let body = line.trim_start();
        println!("│  {body}");
    }
}
