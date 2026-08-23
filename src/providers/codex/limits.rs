use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use tokio::time::Duration;

use super::rpc::RpcClient;
use super::{CodexSnapshot, Credits, Window};

pub async fn fetch(rpc: &mut RpcClient) -> Result<CodexSnapshot> {
    let account = rpc
        .call("account/read", json!({}), Duration::from_secs(5))
        .await
        .ok();
    // This call proxies a network round-trip to the ChatGPT backend; ~2s is
    // typical even warm, so 3s flaked under any latency.
    let rate = rpc
        .call(
            "account/rateLimits/read",
            json!({}),
            Duration::from_secs(15),
        )
        .await?;

    Ok(snapshot_from_rpc(account.as_ref(), &rate))
}

/// Map `account/read` + `account/rateLimits/read` JSON-RPC results into a snapshot.
/// Extra 0.149 fields (`rateLimitsByLimitId`, `rateLimitResetCredits`) are ignored here.
pub(crate) fn snapshot_from_rpc(account: Option<&Value>, rate: &Value) -> CodexSnapshot {
    let (email, plan) = account
        .map(|a| {
            let acct = a.get("account").cloned().unwrap_or(Value::Null);
            (
                acct.get("email").and_then(|v| v.as_str()).map(String::from),
                acct.get("planType")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            )
        })
        .unwrap_or((None, None));

    let rl = rate.get("rateLimits").cloned().unwrap_or(Value::Null);
    let primary = parse_window(rl.get("primary"));
    let secondary = parse_window(rl.get("secondary"));
    let credits = parse_credits(rl.get("credits"));
    let plan = plan.or_else(|| {
        rl.get("planType")
            .and_then(|v| v.as_str())
            .map(String::from)
    });

    CodexSnapshot {
        account_email: email,
        plan_type: plan,
        primary,
        secondary,
        credits,
        reset_credits: None,
        error: None,
    }
}

fn parse_window(v: Option<&Value>) -> Option<Window> {
    let v = v?;
    let used = v.get("usedPercent").and_then(|x| x.as_f64())?;
    let mins = v
        .get("windowDurationMins")
        .and_then(|x| x.as_u64())
        .map(|x| x as u32);
    let resets = v.get("resetsAt").and_then(|x| x.as_i64()).and_then(|secs| {
        match Utc.timestamp_opt(secs, 0) {
            chrono::LocalResult::Single(t) => Some(t),
            _ => None,
        }
    });
    let _: Option<DateTime<Utc>> = resets;
    Some(Window {
        used_percent: used,
        window_minutes: mins,
        resets_at: resets,
    })
}

fn parse_credits(v: Option<&Value>) -> Option<Credits> {
    let v = v?;
    Some(Credits {
        has_credits: v
            .get("hasCredits")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        unlimited: v
            .get("unlimited")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        balance: v.get("balance").and_then(|x| x.as_str()).map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Codex CLI 0.149 Plus payload: weekly usage is `primary`, `secondary` is null,
    /// and extra keys (`rateLimitsByLimitId`, `rateLimitResetCredits`) sit beside
    /// the original `rateLimits` object.
    #[test]
    fn parses_codex_0_149_plus_rate_limits() {
        let account: Value = serde_json::from_str(
            r#"{
              "account": {
                "type": "chatgpt",
                "email": "user@example.com",
                "planType": "plus"
              },
              "requiresOpenaiAuth": true
            }"#,
        )
        .unwrap();
        let rate: Value = serde_json::from_str(
            r#"{
              "rateLimits": {
                "limitId": "codex",
                "limitName": null,
                "primary": {
                  "usedPercent": 54,
                  "windowDurationMins": 10080,
                  "resetsAt": 1787831348
                },
                "secondary": null,
                "credits": {
                  "hasCredits": false,
                  "unlimited": false,
                  "balance": "0"
                },
                "planType": "plus"
              },
              "rateLimitsByLimitId": {
                "codex": { "limitId": "codex" }
              },
              "rateLimitResetCredits": {
                "availableCount": 1,
                "credits": []
              }
            }"#,
        )
        .unwrap();

        let snap = snapshot_from_rpc(Some(&account), &rate);
        assert_eq!(snap.account_email.as_deref(), Some("user@example.com"));
        assert_eq!(snap.plan_type.as_deref(), Some("plus"));
        let primary = snap.primary.expect("primary window");
        assert!((primary.used_percent - 54.0).abs() < 1e-9);
        assert_eq!(primary.window_minutes, Some(10080));
        assert!(snap.secondary.is_none());
        assert_eq!(snap.credits.as_ref().map(|c| c.has_credits), Some(false));
        assert!(snap.error.is_none());
    }
}
