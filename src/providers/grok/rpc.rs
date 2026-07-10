//! JSON-RPC client for `grok agent stdio` (ACP), ported from CodexBar's GrokRPCClient.
//!
//! Protocol: newline-delimited JSON-RPC 2.0 over the child's stdin/stdout.
//! Handshake: `initialize` → `authenticate` (cached_token) → billing.
//!
//! Billing method name evolved: older CLIs use `x.ai/billing`; current (0.2.x)
//! ACP extensions use `_x.ai/billing`. We try modern first, then fall back.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::time::{Duration, timeout};

use crate::util::path::resolve_binary;

/// Hard upper bound for the whole spawn → initialize → auth → billing round-trip.
pub const OVERALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Normalized billing snapshot used by the provider layer (cents-based when available).
#[derive(Debug, Clone, PartialEq)]
pub struct BillingInfo {
    pub used_percent: Option<f64>,
    pub period_start: Option<chrono::DateTime<chrono::Utc>>,
    pub period_end: Option<chrono::DateTime<chrono::Utc>>,
    pub included_used_cents: Option<i64>,
    pub on_demand_used_cents: Option<i64>,
    pub monthly_limit_cents: Option<i64>,
    pub on_demand_enabled: bool,
    pub subscription_tier: Option<String>,
}

impl BillingInfo {
    pub fn monthly_used_percent(&self) -> Option<f64> {
        if let Some(p) = self.used_percent {
            return Some(p.clamp(0.0, 100.0));
        }
        let limit = self.monthly_limit_cents?;
        if limit <= 0 {
            return None;
        }
        // Prefer total-style: included + on-demand when both present.
        let used = match (self.included_used_cents, self.on_demand_used_cents) {
            (Some(i), Some(o)) => i + o,
            (Some(i), None) => i,
            (None, Some(o)) => o,
            (None, None) => return None,
        };
        Some(((used as f64) / (limit as f64) * 100.0).clamp(0.0, 100.0))
    }

    pub fn billing_period_minutes(&self) -> Option<u32> {
        let start = self.period_start?;
        let end = self.period_end?;
        if end <= start {
            return None;
        }
        let secs = end.signed_duration_since(start).num_seconds();
        if secs <= 0 {
            return None;
        }
        Some((secs / 60) as u32)
    }
}

/// Wire format for monetary amounts: `{ "val": <cents> }`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Cent {
    pub val: Option<i64>,
}

/// CodexBar-era `x.ai/billing` result.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LegacyBillingResponse {
    pub billing_cycle: Option<BillingCycle>,
    pub monthly_limit: Option<Cent>,
    pub on_demand_cap: Option<Cent>,
    #[serde(default, rename = "on_demand_enabled")]
    pub on_demand_enabled: Option<bool>,
    pub disabled_by_config: Option<bool>,
    pub usage: Option<BillingUsage>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BillingCycle {
    pub billing_period_start: Option<String>,
    pub billing_period_end: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BillingUsage {
    pub included_used: Option<Cent>,
    pub on_demand_used: Option<Cent>,
    pub total_used: Option<Cent>,
}

/// Current grok CLI `_x.ai/billing` result (`config` + tier).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ModernBillingResponse {
    pub config: Option<ModernBillingConfig>,
    #[serde(default)]
    pub subscription_tier: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModernBillingConfig {
    pub credit_usage_percent: Option<f64>,
    pub current_period: Option<UsagePeriod>,
    pub on_demand_cap: Option<Cent>,
    pub on_demand_used: Option<Cent>,
    pub prepaid_balance: Option<Cent>,
    pub is_unified_billing_user: Option<bool>,
    pub billing_period_start: Option<String>,
    pub billing_period_end: Option<String>,
    // Older monthly fields may still appear on some accounts.
    pub monthly_limit: Option<Cent>,
    pub usage: Option<BillingUsage>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct UsagePeriod {
    #[serde(rename = "type")]
    pub period_type: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
}

impl LegacyBillingResponse {
    pub fn monthly_used_percent(&self) -> Option<f64> {
        let limit = self.monthly_limit.as_ref().and_then(|c| c.val)?;
        if limit <= 0 {
            return None;
        }
        let used = self
            .usage
            .as_ref()
            .and_then(|u| u.total_used.as_ref())
            .and_then(|c| c.val)?;
        Some(((used as f64) / (limit as f64) * 100.0).clamp(0.0, 100.0))
    }

    pub fn to_info(&self) -> BillingInfo {
        BillingInfo {
            used_percent: self.monthly_used_percent(),
            period_start: self
                .billing_cycle
                .as_ref()
                .and_then(|c| c.billing_period_start.as_deref())
                .and_then(parse_iso8601),
            period_end: self
                .billing_cycle
                .as_ref()
                .and_then(|c| c.billing_period_end.as_deref())
                .and_then(parse_iso8601),
            included_used_cents: self
                .usage
                .as_ref()
                .and_then(|u| u.included_used.as_ref())
                .and_then(|c| c.val),
            on_demand_used_cents: self
                .usage
                .as_ref()
                .and_then(|u| u.on_demand_used.as_ref())
                .and_then(|c| c.val),
            monthly_limit_cents: self.monthly_limit.as_ref().and_then(|c| c.val),
            on_demand_enabled: self.on_demand_enabled.unwrap_or(false),
            subscription_tier: None,
        }
    }
}

impl ModernBillingResponse {
    pub fn to_info(&self) -> BillingInfo {
        let cfg = self.config.as_ref();
        let period_start = cfg
            .and_then(|c| c.billing_period_start.as_deref())
            .or_else(|| {
                cfg.and_then(|c| c.current_period.as_ref())
                    .and_then(|p| p.start.as_deref())
            })
            .and_then(parse_iso8601);
        let period_end = cfg
            .and_then(|c| c.billing_period_end.as_deref())
            .or_else(|| {
                cfg.and_then(|c| c.current_period.as_ref())
                    .and_then(|p| p.end.as_deref())
            })
            .and_then(parse_iso8601);

        let used_percent = cfg.and_then(|c| c.credit_usage_percent).or_else(|| {
            // Fall back to cents math when percent is absent but legacy fields exist.
            let limit = cfg
                .and_then(|c| c.monthly_limit.as_ref())
                .and_then(|x| x.val)?;
            if limit <= 0 {
                return None;
            }
            let used = cfg
                .and_then(|c| c.usage.as_ref())
                .and_then(|u| u.total_used.as_ref())
                .and_then(|x| x.val)?;
            Some(((used as f64) / (limit as f64) * 100.0).clamp(0.0, 100.0))
        });

        let on_demand_used = cfg
            .and_then(|c| c.on_demand_used.as_ref())
            .and_then(|x| x.val);
        let on_demand_cap = cfg
            .and_then(|c| c.on_demand_cap.as_ref())
            .and_then(|x| x.val);
        let on_demand_enabled =
            on_demand_cap.is_some_and(|c| c > 0) || on_demand_used.is_some_and(|u| u > 0);

        BillingInfo {
            used_percent,
            period_start,
            period_end,
            included_used_cents: cfg
                .and_then(|c| c.usage.as_ref())
                .and_then(|u| u.included_used.as_ref())
                .and_then(|x| x.val),
            on_demand_used_cents: on_demand_used,
            monthly_limit_cents: cfg
                .and_then(|c| c.monthly_limit.as_ref())
                .and_then(|x| x.val),
            on_demand_enabled,
            subscription_tier: self.subscription_tier.clone(),
        }
    }
}

/// Decode either modern (`config` wrapper) or legacy CodexBar billing JSON.
pub fn decode_billing(value: Value) -> Result<BillingInfo> {
    // Prefer modern when `config` is present.
    if value.get("config").is_some() {
        let modern: ModernBillingResponse =
            serde_json::from_value(value).context("decode modern billing")?;
        return Ok(modern.to_info());
    }
    let legacy: LegacyBillingResponse =
        serde_json::from_value(value).context("decode legacy billing")?;
    Ok(legacy.to_info())
}

/// ISO-8601 with optional fractional seconds (CodexBar `parseISO8601`).
pub fn parse_iso8601(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

pub fn cents_to_dollars(cents: i64) -> f64 {
    (cents as f64) / 100.0
}

/// Spawn `grok agent stdio`, initialize, authenticate, call billing, kill child.
pub async fn fetch_billing(binary_override: Option<&str>) -> Result<BillingInfo> {
    let bin = resolve_binary("grok", binary_override)
        .ok_or_else(|| anyhow!("grok binary not on PATH"))?;

    let mut child = tokio::process::Command::new(&bin)
        .args(["agent", "stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawn {}", bin.display()))?;

    let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;

    let result = timeout(OVERALL_TIMEOUT, run_session(stdin, stdout)).await;

    // Always reap: kill_on_drop covers drop, but terminate promptly on timeout/error.
    let _ = child.start_kill();
    let _ = child.wait().await;

    match result {
        Ok(Ok(billing)) => Ok(billing),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(anyhow!(
            "grok RPC timed out after {}s",
            OVERALL_TIMEOUT.as_secs()
        )),
    }
}

async fn run_session(mut stdin: ChildStdin, stdout: ChildStdout) -> Result<BillingInfo> {
    let mut lines = BufReader::new(stdout).lines();
    let next_id = AtomicI64::new(1);

    // CodexBar initialize params (GrokRPCClient.swift ~98–115).
    let init_params = json!({
        "protocolVersion": "1",
        "clientCapabilities": {
            "fs": { "readTextFile": false, "writeTextFile": false },
            "terminal": false,
        },
    });
    let _init = request(&mut stdin, &mut lines, &next_id, "initialize", init_params)
        .await
        .context("initialize")?;

    // Current CLIs require an explicit ACP authenticate before extension methods.
    // Older CLIs may reject this; ignore non-fatal failures and still try billing.
    if let Err(e) = request(
        &mut stdin,
        &mut lines,
        &next_id,
        "authenticate",
        json!({ "methodId": "cached_token" }),
    )
    .await
    {
        tracing::debug!(error = %e, "grok authenticate skipped/failed");
    }

    // Prefer modern ACP extension method; fall back to CodexBar's `x.ai/billing`.
    let billing_val =
        match request(&mut stdin, &mut lines, &next_id, "_x.ai/billing", json!({})).await {
            Ok(v) => v,
            Err(e) => {
                let msg = e.to_string();
                if msg.to_ascii_lowercase().contains("method not found") {
                    request(&mut stdin, &mut lines, &next_id, "x.ai/billing", json!({}))
                        .await
                        .context("x.ai/billing")?
                } else {
                    return Err(e).context("_x.ai/billing");
                }
            }
        };

    decode_billing(billing_val)
}

async fn request(
    stdin: &mut ChildStdin,
    lines: &mut tokio::io::Lines<BufReader<ChildStdout>>,
    next_id: &AtomicI64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let id = next_id.fetch_add(1, Ordering::Relaxed);
    // Explicit object build so method slashes are never JSON-escaped as `\/`
    // (Grok treats escaped slashes as a different method name).
    let req = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let mut bytes = serde_json::to_vec(&req)?;
    // Defense-in-depth: strip any accidental `\/` escapes from the wire form.
    if let Ok(s) = std::str::from_utf8(&bytes)
        && s.contains("\\/")
    {
        bytes = s.replace("\\/", "/").into_bytes();
    }
    bytes.push(b'\n');
    stdin.write_all(&bytes).await?;
    stdin.flush().await?;

    loop {
        let line = lines
            .next_line()
            .await
            .context("read grok stdout")?
            .ok_or_else(|| anyhow!("grok agent stdio closed stdout"))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                tracing::trace!(line = %line, "grok non-json line");
                continue;
            }
        };
        // Skip notifications (no id) and unrelated responses.
        let Some(msg_id) = v.get("id").and_then(|i| i.as_i64()) else {
            continue;
        };
        if msg_id != id {
            continue;
        }
        if let Some(err) = v.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown JSON-RPC error");
            bail!("{msg}");
        }
        return Ok(v.get("result").cloned().unwrap_or(Value::Null));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY_FIXTURE: &str = r#"{
      "billingCycle": {
        "billingPeriodStart": "2026-05-01T00:00:00Z",
        "billingPeriodEnd": "2026-06-01T00:00:00Z"
      },
      "monthlyLimit": { "val": 99900 },
      "onDemandCap": { "val": 0 },
      "on_demand_enabled": false,
      "disabledByConfig": false,
      "usage": {
        "includedUsed": { "val": 49950 },
        "onDemandUsed": { "val": 0 },
        "totalUsed": { "val": 49950 }
      }
    }"#;

    const MODERN_FIXTURE: &str = r#"{
      "config": {
        "creditUsagePercent": 40.0,
        "currentPeriod": {
          "type": "USAGE_PERIOD_TYPE_WEEKLY",
          "start": "2026-07-05T09:35:55.391616+00:00",
          "end": "2026-07-12T09:35:55.391616+00:00"
        },
        "onDemandCap": { "val": 0 },
        "onDemandUsed": { "val": 0 },
        "prepaidBalance": { "val": 0 },
        "isUnifiedBillingUser": true,
        "billingPeriodStart": "2026-07-05T09:35:55.391616+00:00",
        "billingPeriodEnd": "2026-07-12T09:35:55.391616+00:00"
      },
      "subscription_tier": "SuperGrok"
    }"#;

    #[test]
    fn parse_legacy_fixture_percent_window_reset() {
        let info = decode_billing(serde_json::from_str(LEGACY_FIXTURE).unwrap()).unwrap();
        assert_eq!(info.monthly_limit_cents, Some(99900));
        assert_eq!(info.included_used_cents, Some(49950));
        assert_eq!(info.monthly_used_percent(), Some(50.0));
        assert!(info.period_end.is_some());
        assert_eq!(info.billing_period_minutes(), Some(31 * 24 * 60));
    }

    #[test]
    fn parse_modern_fixture() {
        let info = decode_billing(serde_json::from_str(MODERN_FIXTURE).unwrap()).unwrap();
        assert_eq!(info.used_percent, Some(40.0));
        assert_eq!(info.monthly_used_percent(), Some(40.0));
        assert_eq!(info.subscription_tier.as_deref(), Some("SuperGrok"));
        assert_eq!(info.billing_period_minutes(), Some(7 * 24 * 60));
        assert!(info.period_end.is_some());
        assert!(!info.on_demand_enabled);
    }

    #[test]
    fn monthly_used_percent_nil_when_limit_missing() {
        let info =
            decode_billing(serde_json::from_str(r#"{"usage":{"totalUsed":{"val":100}}}"#).unwrap())
                .unwrap();
        assert_eq!(info.monthly_used_percent(), None);
    }

    #[test]
    fn monthly_used_percent_clamps_over_100() {
        let info = decode_billing(
            serde_json::from_str(
                r#"{"monthlyLimit":{"val":1000},"usage":{"totalUsed":{"val":5000}}}"#,
            )
            .unwrap(),
        )
        .unwrap();
        // Legacy path needs totalUsed + monthlyLimit; without billingCycle still works.
        // Our decode goes legacy; percent from cents.
        let legacy: LegacyBillingResponse = serde_json::from_str(
            r#"{"monthlyLimit":{"val":1000},"usage":{"totalUsed":{"val":5000}}}"#,
        )
        .unwrap();
        assert_eq!(legacy.monthly_used_percent(), Some(100.0));
        assert_eq!(info.monthly_used_percent(), Some(100.0));
    }

    #[test]
    fn cents_to_dollars_formats() {
        assert!((cents_to_dollars(420) - 4.20).abs() < 1e-9);
        assert!((cents_to_dollars(3000) - 30.0).abs() < 1e-9);
        assert!((cents_to_dollars(0) - 0.0).abs() < 1e-9);
        assert!((cents_to_dollars(1) - 0.01).abs() < 1e-9);
    }

    #[test]
    fn period_minutes_math() {
        let info = decode_billing(
            serde_json::from_str(
                r#"{
              "billingCycle": {
                "billingPeriodStart": "2026-07-01T00:00:00.000Z",
                "billingPeriodEnd": "2026-08-01T00:00:00.000Z"
              }
            }"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(info.billing_period_minutes(), Some(31 * 24 * 60));
    }

    #[test]
    fn empty_object_parses() {
        let info = decode_billing(serde_json::from_str("{}").unwrap()).unwrap();
        assert!(info.period_start.is_none());
        assert!(info.monthly_limit_cents.is_none());
        assert_eq!(info.monthly_used_percent(), None);
    }

    #[test]
    fn request_json_does_not_escape_slashes() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "x.ai/billing",
            "params": {},
        });
        let s = serde_json::to_string(&req).unwrap();
        assert!(
            s.contains("x.ai/billing"),
            "method must contain unescaped slash: {s}"
        );
        assert!(!s.contains("x.ai\\/billing"), "must not escape slash: {s}");
    }
}
