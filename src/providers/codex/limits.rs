use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use tokio::time::Duration;

use super::rpc::RpcClient;
use super::{CodexSnapshot, Credits, Window};

pub async fn fetch(rpc: &mut RpcClient) -> Result<CodexSnapshot> {
    let account = rpc
        .call("account/read", json!({}), Duration::from_secs(3))
        .await
        .ok();
    let rate = rpc
        .call("account/rateLimits/read", json!({}), Duration::from_secs(3))
        .await?;

    let (email, plan) = account
        .as_ref()
        .map(|a| {
            let acct = a.get("account").cloned().unwrap_or(Value::Null);
            (
                acct.get("email").and_then(|v| v.as_str()).map(String::from),
                acct.get("planType").and_then(|v| v.as_str()).map(String::from),
            )
        })
        .unwrap_or((None, None));

    let rl = rate.get("rateLimits").cloned().unwrap_or(Value::Null);
    let primary = parse_window(rl.get("primary"));
    let secondary = parse_window(rl.get("secondary"));
    let credits = parse_credits(rl.get("credits"));

    Ok(CodexSnapshot {
        account_email: email,
        plan_type: plan,
        primary,
        secondary,
        credits,
        error: None,
    })
}

fn parse_window(v: Option<&Value>) -> Option<Window> {
    let v = v?;
    let used = v.get("usedPercent").and_then(|x| x.as_f64())?;
    let mins = v.get("windowDurationMins").and_then(|x| x.as_u64()).map(|x| x as u32);
    let resets = v
        .get("resetsAt")
        .and_then(|x| x.as_i64())
        .and_then(|secs| match Utc.timestamp_opt(secs, 0) {
            chrono::LocalResult::Single(t) => Some(t),
            _ => None,
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
        has_credits: v.get("hasCredits").and_then(|x| x.as_bool()).unwrap_or(false),
        unlimited: v.get("unlimited").and_then(|x| x.as_bool()).unwrap_or(false),
        balance: v.get("balance").and_then(|x| x.as_str()).map(String::from),
    })
}
