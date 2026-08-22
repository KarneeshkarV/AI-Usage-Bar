//! Read and write Codex OAuth tokens from the local Codex home (`~/.codex/auth.json`).

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Refresh when the access-token JWT is within this many seconds of `exp`.
pub const ACCESS_TOKEN_REFRESH_GRACE_SECS: i64 = 60;
/// When the JWT has no `exp`, fall back to `last_refresh` older than this.
pub const LAST_REFRESH_MAX_AGE_DAYS: i64 = 8;

#[derive(Debug, Clone)]
pub struct CodexTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub account_id: Option<String>,
    pub last_refresh: Option<String>,
}

#[derive(Deserialize)]
struct AuthFile {
    tokens: Option<TokensBlock>,
    last_refresh: Option<String>,
}

#[derive(Deserialize)]
struct TokensBlock {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
}

/// Resolve the Codex home directory (`$CODEX_HOME` or `~/.codex`).
pub fn codex_home() -> Result<PathBuf> {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        let p = PathBuf::from(home);
        if !p.as_os_str().is_empty() {
            return Ok(p);
        }
    }
    let home = dirs::home_dir().context("no home directory")?;
    Ok(home.join(".codex"))
}

pub fn auth_path() -> Result<PathBuf> {
    Ok(codex_home()?.join("auth.json"))
}

/// Load access token + account id for ChatGPT backend-api calls.
pub fn read_tokens() -> Result<CodexTokens> {
    let path = auth_path()?;
    let raw = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    parse_auth_tokens(&raw).with_context(|| format!("parse {}", path.display()))
}

/// Parse Codex `auth.json` contents (pure; for tests).
pub fn parse_auth_tokens(raw: &str) -> Result<CodexTokens> {
    let auth: AuthFile = serde_json::from_str(raw).context("parse Codex auth.json")?;
    let tokens = auth.tokens.context("no tokens block in Codex auth.json")?;
    let access_token = tokens
        .access_token
        .filter(|t| !t.is_empty())
        .context("no access_token in Codex auth.json")?;
    Ok(CodexTokens {
        access_token,
        refresh_token: tokens.refresh_token.filter(|s| !s.is_empty()),
        id_token: tokens.id_token.filter(|s| !s.is_empty()),
        account_id: tokens.account_id.filter(|s| !s.is_empty()),
        last_refresh: auth.last_refresh.filter(|s| !s.is_empty()),
    })
}

/// JWT `exp` claim from a Codex access token, as unix seconds.
pub fn jwt_exp_unix_seconds(token: &str) -> Option<i64> {
    let mid = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(mid).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("exp")?.as_i64()
}

/// True when the access token should be rotated before an HTTP call.
pub fn access_token_needs_refresh(tokens: &CodexTokens, now: DateTime<Utc>) -> bool {
    if let Some(exp) = jwt_exp_unix_seconds(&tokens.access_token) {
        return exp - now.timestamp() <= ACCESS_TOKEN_REFRESH_GRACE_SECS;
    }
    match tokens.last_refresh.as_deref().and_then(parse_last_refresh) {
        Some(last) => now.signed_duration_since(last).num_days() >= LAST_REFRESH_MAX_AGE_DAYS,
        // No JWT exp and no last_refresh: treat as stale so a refresh is attempted.
        None => tokens.refresh_token.is_some(),
    }
}

fn parse_last_refresh(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Merge refreshed tokens into the existing auth.json object, preserving
/// `auth_mode`, `OPENAI_API_KEY`, and any other top-level keys.
pub fn merge_refreshed_auth_json(
    raw: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    id_token: Option<&str>,
    last_refresh: &str,
) -> Result<String> {
    let mut root: Value = serde_json::from_str(raw).context("parse auth.json for write")?;
    if !root.is_object() {
        anyhow::bail!("auth.json root is not an object");
    }
    {
        let tokens = root
            .get_mut("tokens")
            .filter(|v| v.is_object())
            .context("no tokens object to update")?;
        tokens["access_token"] = json!(access_token);
        if let Some(rt) = refresh_token.filter(|t| !t.is_empty()) {
            tokens["refresh_token"] = json!(rt);
        }
        if let Some(id) = id_token.filter(|t| !t.is_empty()) {
            tokens["id_token"] = json!(id);
        }
    }
    root["last_refresh"] = json!(last_refresh);
    serde_json::to_string_pretty(&root).context("serialize refreshed Codex auth.json")
}

/// Atomically replace `~/.codex/auth.json` with refreshed tokens (mode 0600).
pub fn write_refreshed_tokens(
    access_token: &str,
    refresh_token: Option<&str>,
    id_token: Option<&str>,
    last_refresh: &str,
) -> Result<()> {
    let path = auth_path()?;
    write_refreshed_tokens_at(&path, access_token, refresh_token, id_token, last_refresh)
}

fn write_refreshed_tokens_at(
    path: &Path,
    access_token: &str,
    refresh_token: Option<&str>,
    id_token: Option<&str>,
    last_refresh: &str,
) -> Result<()> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let next =
        merge_refreshed_auth_json(&raw, access_token, refresh_token, id_token, last_refresh)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fake_jwt(exp: i64) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#).as_bytes());
        format!("{header}.{payload}.sig")
    }

    const SAMPLE: &str = r#"{
      "auth_mode": "chatgpt",
      "OPENAI_API_KEY": null,
      "tokens": {
        "id_token": "id.old",
        "access_token": "access.old",
        "refresh_token": "refresh.old",
        "account_id": "acct-1"
      },
      "last_refresh": "2026-08-01T00:00:00Z"
    }"#;

    #[test]
    fn parse_reads_refresh_and_last_refresh() {
        let t = parse_auth_tokens(SAMPLE).unwrap();
        assert_eq!(t.access_token, "access.old");
        assert_eq!(t.refresh_token.as_deref(), Some("refresh.old"));
        assert_eq!(t.id_token.as_deref(), Some("id.old"));
        assert_eq!(t.account_id.as_deref(), Some("acct-1"));
        assert_eq!(t.last_refresh.as_deref(), Some("2026-08-01T00:00:00Z"));
    }

    #[test]
    fn jwt_exp_from_access_token() {
        let token = fake_jwt(1_700_000_000);
        assert_eq!(jwt_exp_unix_seconds(&token), Some(1_700_000_000));
        assert_eq!(jwt_exp_unix_seconds("not-a-jwt"), None);
    }

    #[test]
    fn needs_refresh_when_jwt_near_expiry() {
        let now = Utc.with_ymd_and_hms(2026, 8, 22, 12, 0, 0).unwrap();
        let mut tokens = parse_auth_tokens(SAMPLE).unwrap();
        tokens.access_token = fake_jwt(now.timestamp() + 10);
        assert!(access_token_needs_refresh(&tokens, now));
        tokens.access_token = fake_jwt(now.timestamp() + 3600);
        assert!(!access_token_needs_refresh(&tokens, now));
    }

    #[test]
    fn needs_refresh_from_stale_last_refresh_when_jwt_has_no_exp() {
        let now = Utc.with_ymd_and_hms(2026, 8, 22, 12, 0, 0).unwrap();
        let tokens = parse_auth_tokens(SAMPLE).unwrap();
        // SAMPLE access_token is not a JWT, last_refresh is 2026-08-01 (>= 8 days).
        assert!(access_token_needs_refresh(&tokens, now));
    }

    #[test]
    fn merge_preserves_auth_mode_and_account_id() {
        let next = merge_refreshed_auth_json(
            SAMPLE,
            "access.new",
            Some("refresh.new"),
            Some("id.new"),
            "2026-08-22T00:00:00Z",
        )
        .unwrap();
        let t = parse_auth_tokens(&next).unwrap();
        assert_eq!(t.access_token, "access.new");
        assert_eq!(t.refresh_token.as_deref(), Some("refresh.new"));
        assert_eq!(t.id_token.as_deref(), Some("id.new"));
        assert_eq!(t.account_id.as_deref(), Some("acct-1"));
        assert_eq!(t.last_refresh.as_deref(), Some("2026-08-22T00:00:00Z"));
        let v: Value = serde_json::from_str(&next).unwrap();
        assert_eq!(v["auth_mode"], "chatgpt");
        assert!(v["OPENAI_API_KEY"].is_null());
    }

    #[test]
    fn write_roundtrip_on_tempfile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, SAMPLE).unwrap();
        write_refreshed_tokens_at(
            &path,
            "access.new",
            Some("refresh.new"),
            None,
            "2026-08-22T00:00:00Z",
        )
        .unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let t = parse_auth_tokens(&raw).unwrap();
        assert_eq!(t.access_token, "access.new");
        assert_eq!(t.refresh_token.as_deref(), Some("refresh.new"));
        assert_eq!(t.id_token.as_deref(), Some("id.old"));
    }
}
