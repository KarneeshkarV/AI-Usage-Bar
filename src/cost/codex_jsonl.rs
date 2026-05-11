use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use walkdir::WalkDir;

use super::ReportAcc;
use super::pricing::PricingTable;

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    payload: Option<serde_json::Value>,
    #[serde(default, alias = "sessionId", alias = "id")]
    session_id: Option<String>,
    model: Option<String>,
}

#[derive(Default, Clone)]
struct SessionState {
    model: Option<String>,
    cum_input: u64,
    cum_cached: u64,
    cum_output: u64,
    parent_id: Option<String>,
    fork_ts: Option<DateTime<Utc>>,
}

pub fn scan_dir(root: &Path, acc: &mut ReportAcc, pricing: &PricingTable) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let mut sessions: HashMap<String, SessionState> = HashMap::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|r| r.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .map(|x| x == "jsonl")
                .unwrap_or(false)
        })
    {
        let path = entry.path();
        if let Err(e) = scan_file(path, acc, pricing, &mut sessions) {
            tracing::trace!(file = %path.display(), error = %e, "skip");
        }
    }
    Ok(())
}

fn scan_file(
    path: &Path,
    acc: &mut ReportAcc,
    pricing: &PricingTable,
    sessions: &mut HashMap<String, SessionState>,
) -> Result<()> {
    let f = File::open(path)?;
    let reader = BufReader::new(f);
    let mut current_session: Option<String> = None;

    for line in reader.lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            _ => continue,
        };
        let parsed: Line = match serde_json::from_str(&line) {
            Ok(p) => p,
            Err(_) => continue,
        };

        // session_meta: register session, capture fork lineage
        if parsed.kind.as_deref() == Some("session_meta") {
            if let Some(payload) = parsed.payload.as_ref() {
                let sid = payload
                    .get("session_id")
                    .or_else(|| payload.get("id"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(|| parsed.session_id.clone());
                if let Some(sid) = sid {
                    let parent = payload
                        .get("forked_from_id")
                        .or_else(|| payload.get("parent_session_id"))
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let fork_ts = parsed
                        .timestamp
                        .as_deref()
                        .or_else(|| {
                            payload
                                .get("timestamp")
                                .and_then(|v| v.as_str())
                        })
                        .and_then(parse_ts);
                    let st = sessions.entry(sid.clone()).or_default();
                    st.parent_id = parent;
                    st.fork_ts = fork_ts;
                    current_session = Some(sid);
                }
            }
            continue;
        }

        // turn_context: pick up the model name
        if parsed.kind.as_deref() == Some("turn_context") {
            if let (Some(sid), Some(payload)) = (current_session.as_ref(), parsed.payload.as_ref()) {
                let model = payload
                    .get("model")
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        payload
                            .get("info")
                            .and_then(|i| i.get("model"))
                            .and_then(|v| v.as_str())
                    })
                    .map(String::from);
                if let Some(m) = model {
                    sessions.entry(sid.clone()).or_default().model = Some(m);
                }
            }
            continue;
        }

        // event_msg with token_count payload
        if parsed.kind.as_deref() == Some("event_msg") {
            let payload = match parsed.payload.as_ref() {
                Some(p) => p,
                None => continue,
            };
            if payload.get("type").and_then(|v| v.as_str()) != Some("token_count") {
                continue;
            }
            let info = match payload.get("info") {
                Some(i) => i,
                None => continue,
            };
            // Determine current session id (line-scoped or last seen)
            let sid = parsed
                .session_id
                .clone()
                .or_else(|| current_session.clone());
            let sid = match sid {
                Some(s) => s,
                None => continue,
            };
            let ts = parsed.timestamp.as_deref().and_then(parse_ts);
            let day = ts.map(|t| t.date_naive()).unwrap_or_else(|| Utc::now().date_naive());

            let st = sessions.entry(sid.clone()).or_default();
            let model = parsed
                .model
                .or_else(|| st.model.clone())
                .unwrap_or_else(|| "unknown".into());

            let (di, dc, do_) = if let Some(total) = info.get("total_token_usage") {
                let total_in = u64_from(total.get("input_tokens"));
                let total_cached = u64_from(
                    total
                        .get("cached_input_tokens")
                        .or_else(|| total.get("cache_read_input_tokens")),
                );
                let total_out = u64_from(total.get("output_tokens"));
                let di = total_in.saturating_sub(st.cum_input);
                let dc = total_cached.saturating_sub(st.cum_cached);
                let do_ = total_out.saturating_sub(st.cum_output);
                st.cum_input = total_in;
                st.cum_cached = total_cached;
                st.cum_output = total_out;
                (di, dc, do_)
            } else if let Some(last) = info.get("last_token_usage") {
                let di = u64_from(last.get("input_tokens"));
                let dc = u64_from(
                    last.get("cached_input_tokens")
                        .or_else(|| last.get("cache_read_input_tokens")),
                );
                let do_ = u64_from(last.get("output_tokens"));
                st.cum_input += di;
                st.cum_cached += dc;
                st.cum_output += do_;
                (di, dc, do_)
            } else {
                continue;
            };

            let price = match pricing.get(&model) {
                Some(p) => p,
                None => continue,
            };
            let billable_input = di.saturating_sub(dc) as f64;
            let cached = dc as f64;
            let out = do_ as f64;
            let usd = billable_input * price.input + cached * price.cached_input + out * price.output;
            acc.add("codex", day, &model, usd);
        }
    }
    Ok(())
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn u64_from(v: Option<&serde_json::Value>) -> u64 {
    v.and_then(|x| x.as_u64()).unwrap_or(0)
}
