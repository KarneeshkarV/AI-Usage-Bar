pub mod cache;
pub mod claude_jsonl;
pub mod codex_jsonl;
pub mod pricing;

use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostReport {
    pub total_usd: f64,
    pub by_day: BTreeMap<String, f64>,
    pub by_model: BTreeMap<String, f64>,
    pub by_provider: BTreeMap<String, f64>,
    pub start: String,
    pub end: String,
    pub pricing_source: String,
}

impl CostReport {
    pub fn detail_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.push(format!("window: {} → {}", self.start, self.end));
        out.push(format!("pricing: {}", self.pricing_source));
        if !self.by_provider.is_empty() {
            out.push("by provider:".into());
            for (k, v) in &self.by_provider {
                out.push(format!("  {k:<8} ${v:.2}"));
            }
        }
        if !self.by_model.is_empty() {
            out.push("by model:".into());
            // Sort by cost descending for readability.
            let mut entries: Vec<_> = self.by_model.iter().collect();
            entries.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
            for (k, v) in entries.iter().take(10) {
                out.push(format!("  {:<30} ${:.2}", trim(k, 30), v));
            }
        }
        if !self.by_day.is_empty() {
            out.push("by day:".into());
            for (k, v) in self.by_day.iter().rev().take(10) {
                out.push(format!("  {k} ${v:.2}"));
            }
        }
        out
    }
}

fn trim(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.into()
    } else {
        let cut: String = s.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

pub async fn scan_codex() -> Result<CostReport> {
    let pricing = pricing::load_pricing().await?;
    let roots = codex_roots()?;
    let (start, end) = window();
    let mut acc = ReportAcc::new(start, end, pricing.source.clone());
    for root in roots {
        codex_jsonl::scan_dir(&root, &mut acc, &pricing)?;
    }
    Ok(acc.finalize("codex"))
}

pub async fn scan_claude() -> Result<CostReport> {
    let pricing = pricing::load_pricing().await?;
    let root = claude_root()?;
    let (start, end) = window();
    let mut acc = ReportAcc::new(start, end, pricing.source.clone());
    claude_jsonl::scan_dir(&root, &mut acc, &pricing)?;
    Ok(acc.finalize("claude"))
}

pub async fn scan_both() -> Result<CostReport> {
    let pricing = pricing::load_pricing().await?;
    let (start, end) = window();
    let mut acc = ReportAcc::new(start, end, pricing.source.clone());
    for root in codex_roots()? {
        if let Err(e) = codex_jsonl::scan_dir(&root, &mut acc, &pricing) {
            tracing::warn!(root=%root.display(), error=%e, "codex scan failed");
        }
    }
    let claude_root = claude_root()?;
    if let Err(e) = claude_jsonl::scan_dir(&claude_root, &mut acc, &pricing) {
        tracing::warn!(root=%claude_root.display(), error=%e, "claude scan failed");
    }
    Ok(acc.finalize_combined())
}

pub fn window() -> (NaiveDate, NaiveDate) {
    let today = Utc::now().date_naive();
    let start = today - chrono::Duration::days(29);
    (start, today)
}

fn codex_roots() -> Result<Vec<PathBuf>> {
    let home = dirs::home_dir().context("no home")?;
    let base = std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".codex"));
    let mut roots = Vec::new();
    let s = base.join("sessions");
    if s.exists() {
        roots.push(s);
    }
    let a = base.join("archived_sessions");
    if a.exists() {
        roots.push(a);
    }
    Ok(roots)
}

fn claude_root() -> Result<PathBuf> {
    let home = dirs::home_dir().context("no home")?;
    Ok(home.join(".claude").join("projects"))
}

pub struct ReportAcc {
    pub by_day: BTreeMap<String, f64>,
    pub by_model: BTreeMap<String, f64>,
    pub by_provider: BTreeMap<String, f64>,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub pricing_source: String,
}

impl ReportAcc {
    pub fn new(start: NaiveDate, end: NaiveDate, pricing_source: String) -> Self {
        Self {
            by_day: BTreeMap::new(),
            by_model: BTreeMap::new(),
            by_provider: BTreeMap::new(),
            start,
            end,
            pricing_source,
        }
    }

    pub fn add(&mut self, provider: &str, day: NaiveDate, model: &str, usd: f64) {
        if day < self.start || day > self.end || usd <= 0.0 {
            return;
        }
        *self.by_day.entry(day.to_string()).or_insert(0.0) += usd;
        *self.by_model.entry(model.to_string()).or_insert(0.0) += usd;
        *self.by_provider.entry(provider.to_string()).or_insert(0.0) += usd;
    }

    pub fn finalize(self, _provider: &str) -> CostReport {
        let total = self.by_provider.values().sum();
        CostReport {
            total_usd: total,
            by_day: self.by_day,
            by_model: self.by_model,
            by_provider: self.by_provider,
            start: self.start.to_string(),
            end: self.end.to_string(),
            pricing_source: self.pricing_source,
        }
    }

    pub fn finalize_combined(self) -> CostReport {
        let total = self.by_provider.values().sum();
        CostReport {
            total_usd: total,
            by_day: self.by_day,
            by_model: self.by_model,
            by_provider: self.by_provider,
            start: self.start.to_string(),
            end: self.end.to_string(),
            pricing_source: self.pricing_source,
        }
    }
}
