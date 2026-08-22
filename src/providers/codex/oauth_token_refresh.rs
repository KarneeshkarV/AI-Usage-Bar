//! Refresh an expired Codex ChatGPT OAuth access token and write it back to
//! `~/.codex/auth.json` so the Codex CLI keeps the same rotated tokens.
//!
//! Endpoint and client id match Codex CLI / CodexBar:
//! `POST https://auth.openai.com/oauth/token` with Codex's public client id
//! `app_EMoamEEZ73f0CkXaXp7hrann`.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::time::Duration;

use super::auth::{self, CodexTokens};

const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// Codex CLI public OAuth client id (not a secret).
const OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const TIMEOUT_SECS: u64 = 30;
const USER_AGENT: &str = "ai-usage-bar/0.1 (+https://github.com/KarneeshkarV/AI-Usage-Bar)";

#[derive(Debug, Deserialize)]
struct TokenRefreshResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Load Codex OAuth tokens, refreshing the access token when the JWT is expired
/// or `last_refresh` is stale. Missing credentials bubble up as `Err`.
pub async fn ensure_fresh_codex_access_token() -> Result<CodexTokens> {
    let tokens = auth::read_tokens()?;
    if !auth::access_token_needs_refresh(&tokens, chrono::Utc::now()) {
        return Ok(tokens);
    }

    let refresh_token = tokens.refresh_token.as_deref().filter(|t| !t.is_empty());
    let Some(refresh_token) = refresh_token else {
        bail!("codex oauth refresh: no refresh token; run `codex login` to sign in again");
    };

    let refreshed = refresh_access_token(refresh_token).await?;
    let access_token = refreshed
        .access_token
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| tokens.access_token.clone());
    let new_refresh = refreshed
        .refresh_token
        .clone()
        .or(tokens.refresh_token.clone());
    let new_id = refreshed.id_token.clone().or(tokens.id_token.clone());
    let last_refresh = chrono::Utc::now().to_rfc3339();

    if let Err(e) = auth::write_refreshed_tokens(
        &access_token,
        new_refresh.as_deref(),
        new_id.as_deref(),
        &last_refresh,
    ) {
        tracing::warn!(
            error = %e,
            "codex oauth refresh: token rotated but auth.json write failed"
        );
    }

    Ok(CodexTokens {
        access_token,
        refresh_token: new_refresh,
        id_token: new_id,
        account_id: tokens.account_id,
        last_refresh: Some(last_refresh),
    })
}

async fn refresh_access_token(refresh_token: &str) -> Result<TokenRefreshResponse> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()
        .context("codex oauth refresh: build HTTP client")?;

    let body = serde_json::json!({
        "client_id": OAUTH_CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "scope": "openid profile email",
    });

    let resp = client
        .post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .context("codex oauth refresh: POST /oauth/token")?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .context("codex oauth refresh: read token response")?;

    if !status.is_success() {
        let hint = oauth_error_hint(&text);
        bail!("codex oauth refresh: HTTP {status}{hint}");
    }

    let parsed: TokenRefreshResponse = serde_json::from_str(&text).with_context(|| {
        format!(
            "codex oauth refresh: parse token response: {}",
            text.chars().take(200).collect::<String>()
        )
    })?;
    if let Some(err) = parsed.error.as_deref().filter(|e| !e.is_empty()) {
        bail!("codex oauth refresh: {err}");
    }
    Ok(parsed)
}

fn oauth_error_hint(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return String::new();
    };
    let code = v
        .get("error")
        .and_then(|e| e.as_str())
        .or_else(|| {
            v.get("error")
                .and_then(|e| e.get("code"))
                .and_then(|c| c.as_str())
        })
        .or_else(|| v.get("code").and_then(|c| c.as_str()));
    match code {
        Some(c @ ("refresh_token_expired" | "invalid_grant" | "refresh_token_invalidated")) => {
            format!(" ({c}; run `codex login` to sign in again)")
        }
        Some("refresh_token_reused") => {
            " (refresh_token_reused; run `codex login` to sign in again)".into()
        }
        Some(c) => format!(" ({c})"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_grant_hint() {
        let hint = oauth_error_hint(r#"{"error":"invalid_grant"}"#);
        assert!(hint.contains("invalid_grant"));
        assert!(hint.contains("codex login"));
    }

    #[test]
    fn reused_hint() {
        let hint = oauth_error_hint(r#"{"error":"refresh_token_reused"}"#);
        assert!(hint.contains("refresh_token_reused"));
    }

    #[test]
    fn parses_token_refresh_response() {
        let raw = r#"{
          "access_token": "new-access",
          "refresh_token": "new-refresh",
          "id_token": "new-id",
          "token_type": "Bearer"
        }"#;
        let r: TokenRefreshResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.access_token.as_deref(), Some("new-access"));
        assert_eq!(r.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(r.id_token.as_deref(), Some("new-id"));
    }
}
