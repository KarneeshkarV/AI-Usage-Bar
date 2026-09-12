use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use rayon::prelude::*;
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
}

/// A token-bearing line, decoded but not yet priced.
enum Event {
    /// `session_meta`: makes this the current session for later lines.
    Session(String),
    /// `turn_context`: model for the current session.
    Model(String),
    Tokens(Box<Tokens>),
}

struct Tokens {
    sid: Option<String>,
    day: chrono::NaiveDate,
    model: Option<String>,
    /// Cumulative counters, when the line carries them.
    total: Option<(u64, u64, u64)>,
    /// Per-call counters, for older logs without cumulative ones.
    last: Option<(u64, u64, u64)>,
}

/// Files decoded per batch, bounding peak memory on a large history.
const BATCH: usize = 64;

pub fn scan_dir(root: &Path, acc: &mut ReportAcc, pricing: &PricingTable) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    // Every file is read, even ones older than the window. A forked session
    // replays its parent's cumulative counters under the fork's own
    // timestamp, so the parent's file has to be folded first for the replay
    // to be recognised at all. Rollout names embed a timestamp under a dated
    // directory, so sorting by path folds a parent before its forks.
    //
    // The fold then absorbs the replay by keeping a high-water mark per
    // session, so only usage past the parent's last total is billed.
    let mut files: Vec<_> = WalkDir::new(root)
        .into_iter()
        .filter_map(|r| r.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .map(|e| e.into_path())
        .collect();
    files.sort();

    // Decoding the JSON is the expensive part and each file decodes on its
    // own. Folding stays sequential because sessions carry state across
    // files. Batching bounds peak memory to one batch of decoded events.
    let mut sessions: HashMap<String, SessionState> = HashMap::new();
    for batch in files.chunks(BATCH) {
        let per_file: Vec<Vec<Event>> = batch
            .par_iter()
            .map(|path| match decode_file(path) {
                Ok(events) => events,
                Err(e) => {
                    tracing::trace!(file = %path.display(), error = %e, "skip");
                    Vec::new()
                }
            })
            .collect();
        for events in per_file {
            fold_file(events, acc, pricing, &mut sessions);
        }
    }
    Ok(())
}

/// Apply one file's events. `current_session` is file-scoped, matching the
/// order the lines were written in.
fn fold_file(
    events: Vec<Event>,
    acc: &mut ReportAcc,
    pricing: &PricingTable,
    sessions: &mut HashMap<String, SessionState>,
) {
    let mut current_session: Option<String> = None;
    for event in events {
        match event {
            Event::Session(sid) => {
                sessions.entry(sid.clone()).or_default();
                current_session = Some(sid);
            }
            Event::Model(model) => {
                if let Some(sid) = current_session.as_ref() {
                    sessions.entry(sid.clone()).or_default().model = Some(model);
                }
            }
            Event::Tokens(t) => {
                let sid = match t.sid.or_else(|| current_session.clone()) {
                    Some(s) => s,
                    None => continue,
                };
                let st = sessions.entry(sid).or_default();
                let model = t
                    .model
                    .or_else(|| st.model.clone())
                    .unwrap_or_else(|| "unknown".into());

                let (di, dc, do_) = if let Some((total_in, total_cached, total_out)) = t.total {
                    let di = total_in.saturating_sub(st.cum_input);
                    let dc = total_cached.saturating_sub(st.cum_cached);
                    let do_ = total_out.saturating_sub(st.cum_output);
                    // High-water mark, not assignment. A fork replays its
                    // parent's counters, which would otherwise drop the
                    // baseline and bill that history a second time.
                    st.cum_input = st.cum_input.max(total_in);
                    st.cum_cached = st.cum_cached.max(total_cached);
                    st.cum_output = st.cum_output.max(total_out);
                    (di, dc, do_)
                } else if let Some((di, dc, do_)) = t.last {
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
                let usd = billable_input * price.input
                    + dc as f64 * price.cached_input
                    + do_ as f64 * price.output;
                acc.add("codex", t.day, &model, usd);
            }
        }
    }
}

/// Decode one file's token lines. No pricing and no shared state, so this
/// runs on any thread.
fn decode_file(path: &Path) -> Result<Vec<Event>> {
    let f = File::open(path)?;
    let reader = BufReader::new(f);
    let mut events = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            _ => continue,
        };
        let parsed: Line = match serde_json::from_str(&line) {
            Ok(p) => p,
            Err(_) => continue,
        };

        match parsed.kind.as_deref() {
            // session_meta: register the session this file's lines belong to.
            Some("session_meta") => {
                let payload = match parsed.payload.as_ref() {
                    Some(p) => p,
                    None => continue,
                };
                let sid = payload
                    .get("session_id")
                    .or_else(|| payload.get("id"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(|| parsed.session_id.clone());
                if let Some(sid) = sid {
                    events.push(Event::Session(sid));
                }
            }

            // turn_context: pick up the model name.
            Some("turn_context") => {
                let payload = match parsed.payload.as_ref() {
                    Some(p) => p,
                    None => continue,
                };
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
                    events.push(Event::Model(m));
                }
            }

            // event_msg with a token_count payload.
            Some("event_msg") => {
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
                let day = parsed
                    .timestamp
                    .as_deref()
                    .and_then(parse_ts)
                    .map(|t| t.date_naive())
                    .unwrap_or_else(|| Utc::now().date_naive());
                events.push(Event::Tokens(Box::new(Tokens {
                    sid: parsed.session_id.clone(),
                    day,
                    model: parsed.model.clone(),
                    total: info.get("total_token_usage").map(triple),
                    last: info.get("last_token_usage").map(triple),
                })));
            }

            _ => {}
        }
    }
    Ok(events)
}

/// Read the (input, cached input, output) counters out of a usage object.
fn triple(v: &serde_json::Value) -> (u64, u64, u64) {
    let input = u64_from(v.get("input_tokens"));
    let cached = u64_from(
        v.get("cached_input_tokens")
            .or_else(|| v.get("cache_read_input_tokens")),
    );
    let output = u64_from(v.get("output_tokens"));
    (input, cached, output)
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn u64_from(v: Option<&serde_json::Value>) -> u64 {
    v.and_then(|x| x.as_u64()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::pricing::ModelPrice;
    use std::io::Write;

    fn pricing() -> PricingTable {
        let mut models = HashMap::new();
        models.insert(
            "gpt-5".to_string(),
            ModelPrice {
                input: 1e-6,
                cached_input: 0.0,
                output: 10e-6,
            },
        );
        PricingTable {
            models,
            source: "test".into(),
        }
    }

    fn write_rollout(dir: &Path, name: &str, sid: &str, ts: &str, totals: &[(u64, u64)]) {
        let mut f = File::create(dir.join(name)).unwrap();
        writeln!(
            f,
            r#"{{"type":"session_meta","timestamp":"{ts}","payload":{{"id":"{sid}"}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"turn_context","timestamp":"{ts}","payload":{{"model":"gpt-5"}}}}"#
        )
        .unwrap();
        for (input, output) in totals {
            writeln!(
                f,
                r#"{{"type":"event_msg","timestamp":"{ts}","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":{output}}}}}}}}}"#
            )
            .unwrap();
        }
    }

    fn scan(dir: &Path) -> f64 {
        let today = Utc::now().date_naive();
        let mut acc = ReportAcc::new(today - chrono::Duration::days(29), today, "test".into());
        scan_dir(dir, &mut acc, &pricing()).unwrap();
        acc.finalize_combined().total_usd
    }

    #[test]
    fn cumulative_totals_bill_only_the_delta() {
        let dir = tempfile::tempdir().unwrap();
        let ts = Utc::now().to_rfc3339();
        write_rollout(dir.path(), "rollout-a.jsonl", "a", &ts, &[(100, 10), (300, 30)]);
        // 300 input at 1e-6 plus 30 output at 10e-6.
        assert!((scan(dir.path()) - (300.0 * 1e-6 + 30.0 * 10e-6)).abs() < 1e-9);
    }

    /// A fork replays its parent's cumulative counters under the fork's own
    /// timestamp. Those replayed rows are already paid for, so only the usage
    /// past the parent's last total may be billed.
    #[test]
    fn forked_session_does_not_rebill_the_replayed_history() {
        let dir = tempfile::tempdir().unwrap();
        let ts = Utc::now().to_rfc3339();
        let sid = "019d0000-0000-7000-8000-000000000000";
        write_rollout(dir.path(), "rollout-1-parent.jsonl", sid, &ts, &[(100, 10), (300, 30)]);
        let parent_only = scan(dir.path());

        // The fork repeats (100,10) and (300,30), then adds real new usage.
        write_rollout(
            dir.path(),
            "rollout-2-fork.jsonl",
            sid,
            &ts,
            &[(100, 10), (300, 30), (500, 50)],
        );
        let with_fork = scan(dir.path());

        let new_usage = 200.0 * 1e-6 + 20.0 * 10e-6;
        assert!(
            (with_fork - parent_only - new_usage).abs() < 1e-9,
            "fork rebilled history: parent {parent_only}, with fork {with_fork}"
        );
    }
}
