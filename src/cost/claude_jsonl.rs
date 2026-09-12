use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use rayon::prelude::*;

use super::pricing::PricingTable;
use super::{CostRow, ReportAcc, candidate_files};

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

/// Files decoded per batch. Caps peak memory at a batch's worth of rows
/// instead of the whole history, which matters on a machine with years of
/// logs.
const BATCH: usize = 64;

pub fn scan_dir(root: &Path, acc: &mut ReportAcc, pricing: &PricingTable) -> Result<()> {
    let files = candidate_files(root, acc.start);
    // Files parse independently; the cross-file dedupe happens on the merge,
    // in sorted file order so the result does not depend on thread timing.
    let mut seen_keys: HashSet<String> = HashSet::new();
    for batch in files.chunks(BATCH) {
        let per_file: Vec<Vec<(String, CostRow)>> = batch
            .par_iter()
            .map(|path| {
                let mut rows = Vec::new();
                if let Err(e) = scan_file(path, &mut rows, pricing) {
                    tracing::trace!(file=%path.display(), error=%e, "skip");
                }
                rows
            })
            .collect();
        for (key, row) in per_file.into_iter().flatten() {
            // A key is claimed by the first row that carries it, priced or
            // not, so an unpriced duplicate still suppresses later copies.
            if !key.is_empty() && !seen_keys.insert(key) {
                continue;
            }
            acc.add("claude", row.day, &row.model, row.usd);
        }
    }
    Ok(())
}

fn scan_file(path: &Path, rows: &mut Vec<(String, CostRow)>, pricing: &PricingTable) -> Result<()> {
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
        rows.push((key, CostRow { day, model, usd }));
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
