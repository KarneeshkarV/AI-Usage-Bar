//! Read the active `~/.claude/.credentials.json` OAuth token.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct CredsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeAiOauth>,
}

#[derive(Deserialize)]
struct ClaudeAiOauth {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub access_token: String,
    pub tier: Option<String>,
    /// Unix epoch milliseconds when the access token expires, if known.
    pub expires_at_ms: Option<i64>,
}

/// Path to the active Claude credentials file.
pub fn credentials_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(home.join(".claude").join(".credentials.json"))
}

/// Read the access token and (optional) subscription tier from the active
/// credentials file. Errors when the file is missing, malformed, or has no token.
pub fn read_token() -> Result<TokenInfo> {
    let path = credentials_path().context("no home directory")?;
    let raw = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let creds: CredsFile =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    let oauth = creds
        .claude_ai_oauth
        .with_context(|| format!("no claudeAiOauth in {}", path.display()))?;
    let token = oauth
        .access_token
        .filter(|t| !t.is_empty())
        .with_context(|| format!("no accessToken in {}", path.display()))?;
    Ok(TokenInfo {
        access_token: token,
        tier: oauth.subscription_type,
        expires_at_ms: oauth.expires_at,
    })
}