use anyhow::Result;

use crate::config::{Config, ResetStyle};
use crate::providers::{claude, codex, cursor, grok, opencode};
use crate::snapshot::{self, Snapshot};

pub async fn run(detailed: bool) -> Result<()> {
    // Prefer the cached snapshot written by `ai-usage-bar waybar`. Fall back to a
    // synchronous one-shot poll if no daemon has written one yet.
    let snap = match snapshot::read() {
        Ok(s) if !s.is_stale(std::time::Duration::from_secs(30 * 60)) => s,
        _ => one_shot().await?,
    };

    let style = Config::load_or_default()
        .map(|c| c.display.reset_style)
        .unwrap_or_default();

    if detailed {
        print_detailed(&snap, style);
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
    if let Some(g) = &snap.grok {
        println!("grok: {}", g.summary_line());
    } else {
        println!("grok: (unavailable)");
    }
    // Cursor / OpenCode: omit when inactive.
    if let Some(c) = &snap.cursor {
        println!("cursor: {}", c.summary_line());
    }
    if let Some(o) = &snap.opencode {
        println!("opencode: {}", o.summary_line());
    }
    if let Some(cost) = &snap.cost {
        println!("30d cost: ${:.2}", cost.total_usd);
    }
}

fn print_detailed(snap: &Snapshot, style: ResetStyle) {
    println!("╭─ AI Usage Bar — usage snapshot");
    println!("│  refreshed: {}", snap.refreshed_at.to_rfc3339());
    println!("├─ Codex");
    if let Some(c) = &snap.codex {
        for line in c.detail_lines(style) {
            println!("│  {line}");
        }
    } else {
        println!("│  (unavailable)");
    }
    println!("├─ Claude");
    if let Some(c) = &snap.claude {
        for line in c.detail_lines(style) {
            println!("│  {line}");
        }
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
    if snap.cursor.is_some() {
        println!("├─ Cursor");
        if let Some(c) = &snap.cursor {
            for line in c.detail_lines(style) {
                println!("│  {line}");
            }
        }
    }
    if snap.opencode.is_some() {
        println!("├─ OpenCode");
        if let Some(o) = &snap.opencode {
            for line in o.detail_lines(style) {
                println!("│  {line}");
            }
        }
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
