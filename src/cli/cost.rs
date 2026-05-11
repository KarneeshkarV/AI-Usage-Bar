use anyhow::Result;
use clap::ValueEnum;

use crate::cost;

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum Which {
    Codex,
    Claude,
    Both,
}

pub async fn run(which: Which) -> Result<()> {
    let report = match which {
        Which::Codex => cost::scan_codex().await?,
        Which::Claude => cost::scan_claude().await?,
        Which::Both => cost::scan_both().await?,
    };
    for line in report.detail_lines() {
        println!("{line}");
    }
    println!();
    println!("total: ${:.2}", report.total_usd);
    Ok(())
}
