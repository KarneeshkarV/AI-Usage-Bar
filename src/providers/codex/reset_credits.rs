//! Codex rate-limit **reset credits** (manual full resets of the 5h + weekly windows).
//!
//! Fetched via OAuth from ChatGPT's backend-api, same source CodexBar uses:
//! `GET https://chatgpt.com/backend-api/wham/rate-limit-reset-credits`
//!
//! These are distinct from automatic window `resets_at` times and from paid
//! overage "credits" balance.

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::auth;

const ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const USER_AGENT: &str = "ai-usage-bar/0.1 (+https://github.com/KarneeshkarV/AI-Usage-Bar)";
const TIMEOUT_SECS: u64 = 6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResetCreditsSnapshot {
    /// Count of still-available (non-expired) credits from the server inventory.
    pub available_count: u32,
    /// Available credits sorted by soonest expiry first (no-expiry last).
    pub credits: Vec<ResetCredit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResetCredit {
    pub status: String,
    pub title: Option<String>,
    pub granted_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    #[serde(default)]
    credits: Vec<ApiCredit>,
    #[serde(default)]
    available_count: i64,
}

#[derive(Debug, Deserialize)]
struct ApiCredit {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    granted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
}

/// Best-effort fetch. Returns `Err` when credentials are missing or the API fails.
pub async fn fetch() -> Result<ResetCreditsSnapshot> {
    let tokens = super::oauth_token_refresh::ensure_fresh_codex_access_token()
        .await
        .or_else(|e| {
            tracing::debug!(error = %e, "codex oauth refresh failed; using stored access token");
            auth::read_tokens()
        })?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()?;

    let mut req = client
        .get(ENDPOINT)
        .bearer_auth(&tokens.access_token)
        .header("Accept", "application/json")
        .header("OpenAI-Beta", "codex-1")
        .header("originator", "Codex Desktop");

    if let Some(account_id) = &tokens.account_id {
        // CodexBar sends both header casings; either works.
        req = req
            .header("ChatGPT-Account-Id", account_id)
            .header("ChatGPT-Account-ID", account_id);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!("reset-credits: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| anyhow!("reset-credits body: {e}"))?;
    if !status.is_success() {
        return Err(anyhow!(
            "reset-credits: {status}: {}",
            body.chars().take(200).collect::<String>()
        ));
    }

    let parsed: ApiResponse =
        serde_json::from_str(&body).map_err(|e| anyhow!("reset-credits parse: {e}"))?;
    Ok(from_api(parsed, Utc::now()))
}

/// Pure transform of the API payload into a display snapshot (testable).
fn from_api(parsed: ApiResponse, now: DateTime<Utc>) -> ResetCreditsSnapshot {
    let mut credits: Vec<ResetCredit> = parsed
        .credits
        .into_iter()
        .filter_map(|c| {
            let status = c.status.unwrap_or_else(|| "unknown".into());
            if status != "available" {
                return None;
            }
            if let Some(exp) = c.expires_at
                && exp <= now
            {
                return None;
            }
            Some(ResetCredit {
                status,
                title: c.title,
                granted_at: c.granted_at,
                expires_at: c.expires_at,
            })
        })
        .collect();

    credits.sort_by(|a, b| match (a.expires_at, b.expires_at) {
        (Some(la), Some(rb)) => la.cmp(&rb),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    // Prefer the filtered inventory length; fall back to server count when
    // the list is empty but the server reports a positive available_count
    // (older payloads sometimes omit detail).
    let available_count = if !credits.is_empty() {
        credits.len() as u32
    } else {
        parsed.available_count.max(0) as u32
    };

    ResetCreditsSnapshot {
        available_count,
        credits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn parse(raw: &str) -> ApiResponse {
        serde_json::from_str(raw).unwrap()
    }

    #[test]
    fn filters_expired_and_sorts_by_expiry() {
        let raw = r#"{
          "available_count": 3,
          "credits": [
            {
              "status": "available",
              "title": "Full reset",
              "granted_at": "2026-06-01T00:00:00Z",
              "expires_at": "2026-07-20T00:00:00Z"
            },
            {
              "status": "available",
              "title": "Full reset",
              "granted_at": "2026-06-10T00:00:00Z",
              "expires_at": "2026-07-15T00:00:00Z"
            },
            {
              "status": "available",
              "title": "Expired one",
              "granted_at": "2026-05-01T00:00:00Z",
              "expires_at": "2026-07-01T00:00:00Z"
            },
            {
              "status": "redeemed",
              "title": "Used",
              "granted_at": "2026-06-01T00:00:00Z",
              "expires_at": "2026-08-01T00:00:00Z"
            }
          ]
        }"#;
        let now = utc(2026, 7, 10, 12, 0);
        let snap = from_api(parse(raw), now);
        assert_eq!(snap.available_count, 2);
        assert_eq!(snap.credits.len(), 2);
        // Soonest expiry first
        assert_eq!(snap.credits[0].expires_at, Some(utc(2026, 7, 15, 0, 0)));
        assert_eq!(snap.credits[1].expires_at, Some(utc(2026, 7, 20, 0, 0)));
    }

    #[test]
    fn empty_inventory_uses_server_count() {
        let raw = r#"{"available_count": 2, "credits": []}"#;
        let snap = from_api(parse(raw), utc(2026, 7, 10, 12, 0));
        assert_eq!(snap.available_count, 2);
        assert!(snap.credits.is_empty());
    }

    #[test]
    fn next_expiring_is_first() {
        let raw = r#"{
          "available_count": 1,
          "credits": [
            {
              "status": "available",
              "expires_at": "2026-07-18T00:06:27.236445Z"
            }
          ]
        }"#;
        let snap = from_api(parse(raw), utc(2026, 7, 10, 12, 0));
        assert_eq!(snap.available_count, 1);
        assert!(snap.credits[0].expires_at.is_some());
    }
}
