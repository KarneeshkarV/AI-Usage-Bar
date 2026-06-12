//! Anthropic OAuth usage endpoint. Mirrors the request `~/.claude/statusline.sh`
//! makes: GET `/api/oauth/usage` with `Authorization: Bearer <accessToken>`
//! and `anthropic-beta: oauth-2025-04-20`.

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::time::Duration;
use tokio::process::Command;

use super::{ClaudeData, ExtraSpend, Window};

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
// The OAuth usage endpoint gates by User-Agent: only the official Claude Code
// CLI UA is accepted; anything else is rate-limited (429). Mirror the same UA
// `~/.claude/statusline.sh` uses.
const USER_AGENT: &str = "claude-code/2.1.34";
const CURL_TIMEOUT_SECS: u64 = 8;

pub async fn fetch(token: &str) -> Result<ClaudeData> {
    let raw = match fetch_via_reqwest(token).await {
        Ok(body) => body,
        Err(FetchErr::Transport(e)) => fetch_via_curl(token)
            .await
            .map_err(|curl_err| anyhow!("reqwest: {e}; curl fallback: {curl_err}"))?,
        Err(FetchErr::Http(e)) => return Err(e),
    };

    let parsed: RawUsage = serde_json::from_str(&raw)
        .map_err(|e| anyhow!("parse {ENDPOINT} body: {e}"))?;
    Ok(ClaudeData {
        account_email: None,
        plan_type: None,
        session: parsed.five_hour.map(Window::from),
        weekly: parsed.seven_day.map(Window::from),
        sonnet_weekly: parsed.seven_day_sonnet.map(Window::from),
        extra: parsed.extra_usage.and_then(extract_extra),
    })
}

enum FetchErr {
    /// Connection/DNS/TLS/timeout — worth retrying with curl.
    Transport(anyhow::Error),
    /// HTTP-level error (4xx/5xx) — retrying won't help.
    Http(anyhow::Error),
}

async fn fetch_via_reqwest(token: &str) -> std::result::Result<String, FetchErr> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(CURL_TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| FetchErr::Transport(anyhow!(e)))?;

    let resp = client
        .get(ENDPOINT)
        .bearer_auth(token)
        .header("anthropic-beta", OAUTH_BETA)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| FetchErr::Transport(anyhow!(e)))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchErr::Http(anyhow!(
            "{status} from {ENDPOINT}: {}",
            body.chars().take(200).collect::<String>()
        )));
    }
    resp.text()
        .await
        .map_err(|e| FetchErr::Transport(anyhow!(e)))
}

/// Last-resort fallback when the embedded HTTP client cannot reach the
/// endpoint (DNS, TLS, IPv6 routing — typical "error sending request"
/// failures). System curl tends to succeed where the bundled stack fails.
async fn fetch_via_curl(token: &str) -> Result<String> {
    let output = Command::new("curl")
        .arg("--silent")
        .arg("--show-error")
        .arg("--fail-with-body")
        .arg("--max-time")
        .arg(CURL_TIMEOUT_SECS.to_string())
        .arg("--user-agent")
        .arg(USER_AGENT)
        .arg("-H")
        .arg(format!("Authorization: Bearer {token}"))
        .arg("-H")
        .arg(format!("anthropic-beta: {OAUTH_BETA}"))
        .arg("-H")
        .arg("Accept: application/json")
        .arg(ENDPOINT)
        .output()
        .await
        .map_err(|e| anyhow!("spawn curl: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let body = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.into_owned()
        } else {
            body.chars().take(200).collect()
        };
        return Err(anyhow!("curl exit {}: {}", output.status, detail.trim()));
    }
    String::from_utf8(output.stdout).map_err(|e| anyhow!("curl output not utf8: {e}"))
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
