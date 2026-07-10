//! Zen balance fetch + parse, ported from CodexBar's OpenCodeGoZenBalanceParser
//! / OpenCodeGoZenBalanceFetcher.
//!
//! Live fetch uses the API key as `Authorization: Bearer` against
//! `GET https://opencode.ai/zen/v1/balance`. The response body is run through
//! the same multi-shape parser as CodexBar (explicit balance keys, dollar
//! amounts, scaled billing-server balances).

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

const BALANCE_URL: &str = "https://opencode.ai/zen/v1/balance";
const TIMEOUT_SECS: u64 = 15;
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

/// Billing-server raw balances are scaled by 1e8 (CodexBar `billingScale`).
const BILLING_SCALE: f64 = 100_000_000.0;

const EXPLICIT_BALANCE_KEYS: &[&str] = &[
    "zenbalance",
    "zencurrentbalance",
    "currentbalance",
    "currentbalanceusd",
    "balanceusd",
    "usdbalance",
];

/// Fetch Zen balance dollars using an OpenCode API key.
pub async fn fetch_balance(api_key: &str) -> Result<f64> {
    let key = api_key.trim();
    if key.is_empty() {
        bail!("empty OpenCode API key");
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()
        .context("build reqwest client")?;

    let resp = client
        .get(BALANCE_URL)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await
        .context("GET /zen/v1/balance")?;

    let status = resp.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        bail!("auth: HTTP {status} (API key invalid or expired)");
    }
    if !status.is_success() {
        bail!("HTTP {status} from /zen/v1/balance");
    }

    let text = resp.text().await.context("read balance body")?;
    parse_balance_text(&text)
        .ok_or_else(|| anyhow::anyhow!("could not parse balance from response"))
}

/// Parse a balance response body (JSON, seroval/billing, or HTML dollar text).
pub fn parse_balance_text(text: &str) -> Option<f64> {
    if let Some(v) = parse_json_balance(text) {
        return Some(v);
    }
    if let Some(v) = parse_billing_server_response(text) {
        return Some(v);
    }
    parse_dollar_text(text)
}

fn parse_json_balance(text: &str) -> Option<f64> {
    let value: Value = serde_json::from_str(text).ok()?;
    find_balance_value(&value)
}

fn find_balance_value(value: &Value) -> Option<f64> {
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                if is_explicit_balance_key(key)
                    && let Some(n) = double_value(v)
                {
                    return Some(n);
                }
            }
            for v in map.values() {
                if let Some(found) = find_balance_value(v) {
                    return Some(found);
                }
            }
            // Flat `{ "balance": 12.34 }` without customerID is accepted for the
            // modern JSON balance endpoint (unlike the scaled billing parser).
            if let Some(n) = map.get("balance").and_then(double_value) {
                // Heuristic: values that look like micro-units are scaled.
                if n.abs() > 1_000_000.0 {
                    return Some(n / BILLING_SCALE);
                }
                return Some(n);
            }
            None
        }
        Value::Array(arr) => {
            for v in arr {
                if let Some(found) = find_balance_value(v) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// CodexBar `parseBillingServerResponse`: require customerID + raw balance / 1e8.
pub fn parse_billing_server_response(text: &str) -> Option<f64> {
    if let Ok(value) = serde_json::from_str::<Value>(text)
        && let Some(raw) = find_raw_billing_balance(&value)
    {
        return Some(raw / BILLING_SCALE);
    }

    // Seroval / JS-ish body: need a customerID and a balance number.
    let customer_re = customer_id_re();
    if !customer_re.is_match(text) {
        return None;
    }
    let balance_re = balance_number_re();
    let caps = balance_re.captures(text)?;
    let raw: f64 = caps.get(1)?.as_str().parse().ok()?;
    Some(raw / BILLING_SCALE)
}

fn find_raw_billing_balance(value: &Value) -> Option<f64> {
    match value {
        Value::Object(map) => {
            if map.contains_key("balance") {
                let _customer = map
                    .get("customerID")
                    .or_else(|| map.get("customerId"))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())?;
                return map.get("balance").and_then(double_value);
            }
            for v in map.values() {
                if let Some(found) = find_raw_billing_balance(v) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for v in arr {
                if let Some(found) = find_raw_billing_balance(v) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_dollar_text(text: &str) -> Option<f64> {
    let localized = localized_balance_re();
    if let Some(caps) = localized.captures(text) {
        return parse_dollar_group(caps.get(1)?.as_str());
    }
    let nearby = nearby_balance_re();
    if let Some(caps) = nearby.captures(text) {
        return parse_dollar_group(caps.get(1)?.as_str());
    }
    None
}

fn parse_dollar_group(s: &str) -> Option<f64> {
    let cleaned = s.replace(',', "");
    cleaned.parse().ok()
}

fn is_explicit_balance_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    EXPLICIT_BALANCE_KEYS.contains(&normalized.as_str())
}

fn double_value(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(_) => None,
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let cleaned = s.trim().replace(',', "");
            cleaned.parse().ok()
        }
        _ => None,
    }
}

fn localized_balance_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?:current\s+balance|zen\s+balance|現在の残高)[^$]{0,80}\$\s*([0-9][0-9,]*(?:\.[0-9]+)?)")
            .expect("localized balance regex")
    })
}

fn nearby_balance_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?:balance|残高)[\s\S]{0,120}?\$\s*([0-9][0-9,]*(?:\.[0-9]+)?)")
            .expect("nearby balance regex")
    })
}

fn customer_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?:"customerID"|customerID)\s*:\s*(?:\$R\[\d+\]\s*=\s*)?"[^"]+""#)
            .expect("customerID regex")
    })
}

fn balance_number_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?:"balance"|balance)\s*:\s*(?:\$R\[\d+\]\s*=\s*)?(-?[0-9]+(?:\.[0-9]+)?)"#)
            .expect("balance number regex")
    })
}

/// Format dollars for waybar / tooltips: `$12.40`.
pub fn format_dollars(amount: f64) -> String {
    format!("${amount:.2}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explicit_zen_balance_json() {
        let text = r#"{
          "data": {
            "billing": {
              "balanceEnabled": true,
              "zenBalance": "1,042.75"
            }
          }
        }"#;
        assert!((parse_balance_text(text).unwrap() - 1042.75).abs() < 1e-9);
    }

    #[test]
    fn parses_flat_balance_json() {
        let text = r#"{ "balance": 12.4, "currency": "USD" }"#;
        assert!((parse_balance_text(text).unwrap() - 12.4).abs() < 1e-9);
    }

    #[test]
    fn parses_scaled_billing_server_json() {
        let text = r#"{ "customerID": "cus_test", "balance": 2375000000 }"#;
        assert!((parse_billing_server_response(text).unwrap() - 23.75).abs() < 1e-9);
    }

    #[test]
    fn billing_parser_ignores_balance_without_customer() {
        let text = r#"{ "balance": 0, "customerID": null }"#;
        assert!(parse_billing_server_response(text).is_none());
    }

    #[test]
    fn parses_seroval_billing_text() {
        let text = r#";0x00000120;((self.$R=self.$R||{})["server-fn:test"]=[],($R=>$R[0]=$R[1]={customerID:"cus_test",balance:$R[2]=2375000000,reload:!1})($R["server-fn:test"]))"#;
        assert!((parse_billing_server_response(text).unwrap() - 23.75).abs() < 1e-9);
    }

    #[test]
    fn parses_dollar_from_html() {
        let text = r#"<main><h2>現在の残高 $1,234.56</h2></main>"#;
        assert!((parse_balance_text(text).unwrap() - 1234.56).abs() < 1e-9);
    }

    #[test]
    fn format_dollars_two_places() {
        assert_eq!(format_dollars(12.4), "$12.40");
        assert_eq!(format_dollars(0.0), "$0.00");
        assert_eq!(format_dollars(1042.75), "$1042.75");
    }

    #[test]
    fn ignores_metadata_keys_before_zen_balance() {
        let text = r#"{
          "data": {
            "billing": {
              "balanceUpdatedAt": 1800000000,
              "balanceRefreshInterval": 60,
              "zenBalance": "42.50"
            }
          }
        }"#;
        assert!((parse_balance_text(text).unwrap() - 42.50).abs() < 1e-9);
    }
}
