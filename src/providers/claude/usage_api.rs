//! Anthropic OAuth usage endpoint. Mirrors the request `~/.claude/statusline.sh`
//! makes: GET `/api/oauth/usage` with `Authorization: Bearer <accessToken>`
//! and `anthropic-beta: oauth-2025-04-20`.

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::time::Duration;

use super::{ClaudeData, ExtraSpend, Window};

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
// The OAuth usage endpoint gates by User-Agent: only the official Claude Code
// CLI UA is accepted; anything else is rate-limited (429). Mirror the same UA
// `~/.claude/statusline.sh` uses.
const USER_AGENT: &str = "claude-code/2.1.34";

pub async fn fetch(token: &str) -> Result<ClaudeData> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(USER_AGENT)
        .build()?;

    let resp = client
        .get(ENDPOINT)
        .bearer_auth(token)
        .header("anthropic-beta", OAUTH_BETA)
        .header("Accept", "application/json")
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "{status} from {ENDPOINT}: {}",
            body.chars().take(200).collect::<String>()
        ));
    }

    let raw: RawUsage = resp.json().await?;
    Ok(ClaudeData {
        account_email: None,
        plan_type: None,
        session: raw.five_hour.map(Window::from),
        weekly: raw.seven_day.map(Window::from),
        sonnet_weekly: raw.seven_day_sonnet.map(Window::from),
        extra: raw.extra_usage.and_then(extract_extra),
    })
}

#[derive(Deserialize)]
struct RawUsage {
    five_hour: Option<UsageWindowResp>,
    seven_day: Option<UsageWindowResp>,
    seven_day_sonnet: Option<UsageWindowResp>,
    extra_usage: Option<ExtraUsageResp>,
}

#[derive(Deserialize)]
struct UsageWindowResp {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

impl From<UsageWindowResp> for Window {
    fn from(w: UsageWindowResp) -> Self {
        let resets_at = w
            .resets_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        Window {
            used_percent: w.utilization.unwrap_or(0.0),
            resets_at,
        }
    }
}

#[derive(Deserialize)]
struct ExtraUsageResp {
    is_enabled: Option<bool>,
    used_credits: Option<f64>,
    monthly_limit: Option<f64>,
    currency: Option<String>,
}

fn extract_extra(o: ExtraUsageResp) -> Option<ExtraSpend> {
    if !o.is_enabled.unwrap_or(false) {
        return None;
    }
    let used = o.used_credits? / 100.0;
    let limit = o.monthly_limit? / 100.0;
    Some(ExtraSpend {
        used_usd: used,
        limit_usd: limit,
        currency: o.currency.unwrap_or_else(|| "USD".into()),
    })
}
