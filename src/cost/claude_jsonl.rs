use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use walkdir::WalkDir;

use super::ReportAcc;
use super::pricing::PricingTable;

#[derive(Deserialize)]
struct Line {
    timestamp: Option<String>,
    #[serde(default, alias = "sessionId")]
    session_id: Option<String>,
    #[serde(default, alias = "messageId")]
    message_id: Option<String>,
    #[serde(default, alias = "requestId")]
    request_id: Option<String>,
    model: Option<String>,
    message: Option<serde_json::Value>,
    usage: Option<serde_json::Value>,
    #[serde(default, alias = "costNanos")]
    cost_nanos: Option<i64>,
    #[serde(default, alias = "costUSD", alias = "costUsd")]
    cost_usd: Option<f64>,
}

pub fn scan_dir(root: &Path, acc: &mut ReportAcc, pricing: &PricingTable) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let mut seen_keys: HashSet<String> = HashSet::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|r| r.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().map(|x| x == "jsonl").unwrap_or(false))
    {
        if let Err(e) = scan_file(entry.path(), acc, pricing, &mut seen_keys) {
            tracing::trace!(file=%entry.path().display(), error=%e, "skip");
        }
    }
    Ok(())
}

fn scan_file(
    path: &Path,
    acc: &mut ReportAcc,
    pricing: &PricingTable,
    seen_keys: &mut HashSet<String>,
) -> Result<()> {
    let f = File::open(path)?;
    let reader = BufReader::new(f);
    for line in reader.lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            _ => continue,
        };
        let parsed: Line = match serde_json::from_str(&line) {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Dedupe across files using message/request ids.
        let key = parsed
            .message_id
            .clone()
            .or(parsed.request_id.clone())
            .unwrap_or_else(|| {
                format!(
                    "{}|{}",
                    parsed.session_id.clone().unwrap_or_default(),
                    parsed.timestamp.clone().unwrap_or_default()
                )
            });
        if !key.is_empty() && !seen_keys.insert(key) {
            continue;
        }

        let day = parsed
            .timestamp
            .as_deref()
            .and_then(parse_ts)
            .map(|t| t.date_naive())
            .unwrap_or_else(|| Utc::now().date_naive());

        // Model can live on the top level or under `message.model`.
        let model = parsed
            .model
            .clone()
            .or_else(|| {
                parsed
                    .message
                    .as_ref()
                    .and_then(|m| m.get("model"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .unwrap_or_else(|| "unknown".into());

        // Prefer pre-computed cost when present.
        let mut usd = 0.0;
        if let Some(c) = parsed.cost_usd {
            usd = c;
        } else if let Some(n) = parsed.cost_nanos {
            usd = (n as f64) / 1_000_000_000.0;
        } else {
            // Fall back to pricing × token counts from `usage`.
            let usage = parsed.usage.clone().or_else(|| {
                parsed
                    .message
                    .as_ref()
                    .and_then(|m| m.get("usage").cloned())
            });
            if let (Some(usage), Some(price)) = (usage, pricing.get(&model)) {
                let input = json_u64(&usage, &["input_tokens", "input"]);
                let cache_read = json_u64(&usage, &["cache_read_input_tokens", "cacheRead"]);
                let cache_create =
                    json_u64(&usage, &["cache_creation_input_tokens", "cacheCreate"]);
                let output = json_u64(&usage, &["output_tokens", "output"]);
                let billable_input = input.saturating_sub(cache_read) as f64;
                usd = billable_input * price.input
                    + cache_read as f64 * price.cached_input
                    + cache_create as f64 * price.input
                    + output as f64 * price.output;
            }
        }
        if usd > 0.0 {
            acc.add("claude", day, &model, usd);
        } else {
            // ensure unused day var doesn't warn when nothing matches
            let _: NaiveDate = day;
        }
    }
    Ok(())
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn json_u64(v: &serde_json::Value, keys: &[&str]) -> u64 {
    for k in keys {
        if let Some(n) = v.get(*k).and_then(|x| x.as_u64()) {
            return n;
        }
    }
    0
}
