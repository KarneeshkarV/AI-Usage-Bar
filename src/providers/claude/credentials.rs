//! Read and write `~/.claude/.credentials.json` Claude Code OAuth tokens.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Refresh when the access token is within this many ms of expiry.
/// Anthropic rejects expired tokens with a 429 that looks like a generic rate-limit.
pub const ACCESS_TOKEN_REFRESH_GRACE_MS: i64 = 60_000;

#[derive(Deserialize)]
struct CredsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeAiOauth>,
}

#[derive(Deserialize)]
struct ClaudeAiOauth {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
    #[serde(rename = "refreshTokenExpiresAt")]
    refresh_token_expires_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub tier: Option<String>,
    /// Unix epoch milliseconds when the access token expires, if known.
    pub expires_at_ms: Option<i64>,
    /// Unix epoch milliseconds when the refresh token expires, if known.
    pub refresh_token_expires_at_ms: Option<i64>,
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
    parse_oauth_tokens(&raw).with_context(|| format!("parse {}", path.display()))
}

/// Parse Claude Code OAuth tokens from a credentials JSON blob.
pub fn parse_oauth_tokens(raw: &str) -> Result<TokenInfo> {
    let creds: CredsFile = serde_json::from_str(raw).context("parse Claude credentials JSON")?;
    let oauth = creds
        .claude_ai_oauth
        .context("no claudeAiOauth in credentials")?;
    let token = oauth
        .access_token
        .filter(|t| !t.is_empty())
        .context("no accessToken in credentials")?;
    Ok(TokenInfo {
        access_token: token,
        refresh_token: oauth.refresh_token.filter(|t| !t.is_empty()),
        tier: oauth.subscription_type,
        expires_at_ms: oauth.expires_at,
        refresh_token_expires_at_ms: oauth.refresh_token_expires_at,
    })
}

/// True when the access token is missing an expiry or is within the refresh grace window.
pub fn access_token_needs_refresh(expires_at_ms: Option<i64>, now_ms: i64) -> bool {
    match expires_at_ms {
        None => false,
        Some(exp) => exp - now_ms <= ACCESS_TOKEN_REFRESH_GRACE_MS,
    }
}

/// True when the refresh token itself is past `refreshTokenExpiresAt`.
pub fn refresh_token_is_expired(expires_at_ms: Option<i64>, now_ms: i64) -> bool {
    match expires_at_ms {
        None => false,
        Some(exp) => now_ms >= exp,
    }
}

/// Merge a refreshed access token into the existing credentials JSON.
///
/// Preserves unknown fields (`scopes`, `rateLimitTier`, extra keys) so Claude CLI
/// still reads the file after we rotate tokens.
pub fn merge_refreshed_oauth_json(
    raw: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at_ms: i64,
) -> Result<String> {
    let mut root: Value = serde_json::from_str(raw).context("parse credentials for write")?;
    let oauth = root
        .get_mut("claudeAiOauth")
        .filter(|v| v.is_object())
        .context("no claudeAiOauth object to update")?;
    oauth["accessToken"] = json!(access_token);
    oauth["expiresAt"] = json!(expires_at_ms);
    if let Some(rt) = refresh_token.filter(|t| !t.is_empty()) {
        oauth["refreshToken"] = json!(rt);
    }
    serde_json::to_string_pretty(&root).context("serialize refreshed Claude credentials")
}

/// Atomically replace `~/.claude/.credentials.json` with refreshed tokens (mode 0600).
pub fn write_refreshed_oauth_tokens(
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at_ms: i64,
) -> Result<()> {
    let path = credentials_path().context("no home directory")?;
    write_refreshed_oauth_tokens_at(&path, access_token, refresh_token, expires_at_ms)
}

fn write_refreshed_oauth_tokens_at(
    path: &Path,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at_ms: i64,
) -> Result<()> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let next = merge_refreshed_oauth_json(&raw, access_token, refresh_token, expires_at_ms)?;
    atomic_write_private(path, next.as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Unix epoch milliseconds from the system clock.
pub fn now_epoch_ms() -> Result<i64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock before unix epoch")?;
    i64::try_from(now.as_millis()).context("epoch ms overflow")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "claudeAiOauth": {
        "accessToken": "sk-ant-oat01-old",
        "refreshToken": "sk-ant-ort01-old",
        "expiresAt": 1000,
        "refreshTokenExpiresAt": 5000,
        "scopes": ["user:profile"],
        "subscriptionType": "pro",
        "rateLimitTier": "default_claude_ai"
      }
    }"#;

    #[test]
    fn parse_reads_refresh_and_expiry() {
        let t = parse_oauth_tokens(SAMPLE).unwrap();
        assert_eq!(t.access_token, "sk-ant-oat01-old");
        assert_eq!(t.refresh_token.as_deref(), Some("sk-ant-ort01-old"));
        assert_eq!(t.expires_at_ms, Some(1000));
        assert_eq!(t.refresh_token_expires_at_ms, Some(5000));
        assert_eq!(t.tier.as_deref(), Some("pro"));
    }

    #[test]
    fn needs_refresh_inside_grace_window() {
        assert!(access_token_needs_refresh(Some(1_000), 950));
        assert!(!access_token_needs_refresh(Some(100_000), 1_000));
        assert!(!access_token_needs_refresh(None, 1_000));
    }

    #[test]
    fn refresh_token_expiry_check() {
        assert!(refresh_token_is_expired(Some(1_000), 1_000));
        assert!(!refresh_token_is_expired(Some(2_000), 1_000));
        assert!(!refresh_token_is_expired(None, 1_000));
    }

    #[test]
    fn merge_preserves_unrelated_fields() {
        let next =
            merge_refreshed_oauth_json(SAMPLE, "sk-ant-oat01-new", Some("sk-ant-ort01-new"), 9999)
                .unwrap();
        let t = parse_oauth_tokens(&next).unwrap();
        assert_eq!(t.access_token, "sk-ant-oat01-new");
        assert_eq!(t.refresh_token.as_deref(), Some("sk-ant-ort01-new"));
        assert_eq!(t.expires_at_ms, Some(9999));
        let v: Value = serde_json::from_str(&next).unwrap();
        assert_eq!(v["claudeAiOauth"]["scopes"][0], "user:profile");
        assert_eq!(v["claudeAiOauth"]["rateLimitTier"], "default_claude_ai");
        assert_eq!(v["claudeAiOauth"]["refreshTokenExpiresAt"], 5000);
    }

    #[test]
    fn merge_keeps_old_refresh_token_when_omitted() {
        let next = merge_refreshed_oauth_json(SAMPLE, "sk-ant-oat01-new", None, 9999).unwrap();
        let t = parse_oauth_tokens(&next).unwrap();
        assert_eq!(t.refresh_token.as_deref(), Some("sk-ant-ort01-old"));
    }

    #[test]
    fn parse_rejects_missing_oauth() {
        assert!(parse_oauth_tokens(r#"{"other": true}"#).is_err());
    }

    #[test]
    fn write_roundtrip_on_tempfile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        std::fs::write(&path, SAMPLE).unwrap();
        write_refreshed_oauth_tokens_at(&path, "sk-ant-oat01-new", Some("sk-ant-ort01-new"), 42)
            .unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let t = parse_oauth_tokens(&raw).unwrap();
        assert_eq!(t.access_token, "sk-ant-oat01-new");
        assert_eq!(t.expires_at_ms, Some(42));
    }
}
