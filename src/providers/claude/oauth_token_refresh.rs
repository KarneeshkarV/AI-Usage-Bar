//! Refresh an expired Claude Code OAuth access token and write it back to
//! `~/.claude/.credentials.json` so Claude CLI keeps the same rotated tokens.
//!
//! Endpoint and client id match Claude Code / CodexBar:
//! `POST https://platform.claude.com/v1/oauth/token` with Claude Code's public
//! OAuth client id `9d1c250a-e61b-44d9-88ed-5944d1962f5e`.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::time::Duration;

use super::credentials::{self, TokenInfo};

const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
/// Claude Code CLI public OAuth client id (not a secret).
const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const TIMEOUT_SECS: u64 = 30;
const USER_AGENT: &str = "ai-usage-bar/0.1 (+https://github.com/KarneeshkarV/AI-Usage-Bar)";

#[derive(Debug, Deserialize)]
struct TokenRefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
}

/// Load Claude OAuth tokens, refreshing the access token when it is expired
/// or within the grace window. Missing credentials bubble up as `Err`.
pub async fn ensure_fresh_claude_access_token() -> Result<TokenInfo> {
    let tokens = credentials::read_token()?;
    let now_ms = credentials::now_epoch_ms()?;

    if !credentials::access_token_needs_refresh(tokens.expires_at_ms, now_ms) {
        return Ok(tokens);
    }

    if credentials::refresh_token_is_expired(tokens.refresh_token_expires_at_ms, now_ms) {
        bail!("claude oauth refresh: refresh token expired; run `claude` to log in again");
    }

    let refresh_token = tokens.refresh_token.as_deref().filter(|t| !t.is_empty());
    let Some(refresh_token) = refresh_token else {
        bail!("claude oauth refresh: no refresh token; run `claude` to log in again");
    };

    let refreshed = refresh_access_token(refresh_token).await?;
    let expires_at_ms = now_ms.saturating_add(refreshed.expires_in.saturating_mul(1000));
    let new_refresh = refreshed.refresh_token.as_deref();

    if let Err(e) = credentials::write_refreshed_oauth_tokens(
        &refreshed.access_token,
        new_refresh,
        expires_at_ms,
    ) {
        tracing::warn!(error = %e, "claude oauth refresh: token rotated but credentials file write failed");
    }

    Ok(TokenInfo {
        access_token: refreshed.access_token,
        refresh_token: refreshed.refresh_token.or(tokens.refresh_token),
        tier: tokens.tier,
        expires_at_ms: Some(expires_at_ms),
        refresh_token_expires_at_ms: tokens.refresh_token_expires_at_ms,
    })
}

async fn refresh_access_token(refresh_token: &str) -> Result<TokenRefreshResponse> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()
        .context("claude oauth refresh: build HTTP client")?;

    let body = form_encode(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", OAUTH_CLIENT_ID),
    ]);

    let resp = client
        .post(TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(body)
        .send()
        .await
        .context("claude oauth refresh: POST /v1/oauth/token")?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .context("claude oauth refresh: read token response")?;

    if !status.is_success() {
        let hint = oauth_error_hint(&text);
        bail!("claude oauth refresh: HTTP {status}{hint}");
    }

    serde_json::from_str(&text).with_context(|| {
        format!(
            "claude oauth refresh: parse token response: {}",
            text.chars().take(200).collect::<String>()
        )
    })
}

fn oauth_error_hint(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return String::new();
    };
    let code = v.get("error").and_then(|e| e.as_str()).or_else(|| {
        v.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str())
    });
    match code {
        Some("invalid_grant") => {
            " invalid_grant (refresh token rejected; run `claude` to log in again)".into()
        }
        Some(c) => format!(" ({c})"),
        None => String::new(),
    }
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encode_refresh_request() {
        let body = form_encode(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", "sk-ant-ort01-abc"),
            ("client_id", OAUTH_CLIENT_ID),
        ]);
        assert!(body.starts_with("grant_type=refresh_token"));
        assert!(body.contains("refresh_token=sk-ant-ort01-abc"));
        assert!(body.contains(&format!("client_id={OAUTH_CLIENT_ID}")));
    }

    #[test]
    fn urlencode_leaves_unreserved() {
        assert_eq!(urlencode("sk-ant_ort.01~"), "sk-ant_ort.01~");
        assert_eq!(urlencode("a b"), "a%20b");
    }

    #[test]
    fn invalid_grant_hint() {
        let hint = oauth_error_hint(r#"{"error":"invalid_grant"}"#);
        assert!(hint.contains("invalid_grant"));
        assert!(hint.contains("claude"));
    }

    #[test]
    fn parses_token_refresh_response() {
        let raw = r#"{
          "access_token": "sk-ant-oat01-new",
          "refresh_token": "sk-ant-ort01-new",
          "expires_in": 28800,
          "token_type": "Bearer"
        }"#;
        let r: TokenRefreshResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.access_token, "sk-ant-oat01-new");
        assert_eq!(r.refresh_token.as_deref(), Some("sk-ant-ort01-new"));
        assert_eq!(r.expires_in, 28800);
    }
}
