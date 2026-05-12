//! Discover and parse `~/.claude/.credentials*.json` files. The active
//! account is always at `.credentials.json`; backups live alongside it. The
//! `cl_cycle` zsh function rotates the contents through these slots.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AccountSlot {
    /// Short label derived from the filename: "live", "har", "val".
    pub label: String,
    /// True when this slot is the active credentials file.
    pub is_active: bool,
    pub path: PathBuf,
}

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

/// Return the credentials files that exist on disk, in display order
/// (active first, then backups). When `extra` is provided, those paths
/// replace the default set; the first existing path is treated as active.
pub fn discover_accounts(extra: Option<&[PathBuf]>) -> Vec<AccountSlot> {
    let candidates: Vec<(String, PathBuf)> = match extra {
        Some(paths) => paths
            .iter()
            .map(|p| (label_from_path(p), p.clone()))
            .collect(),
        None => default_candidates(),
    };

    let mut out = Vec::with_capacity(candidates.len());
    for (label, path) in candidates {
        if path.is_file() {
            out.push(AccountSlot {
                is_active: out.is_empty(),
                label,
                path,
            });
        }
    }
    out
}

fn default_candidates() -> Vec<(String, PathBuf)> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let claude = home.join(".claude");
    vec![
        ("live".into(), claude.join(".credentials.json")),
        ("har".into(), claude.join(".credentials.har.json")),
        ("val".into(), claude.join(".credentials.val.json")),
    ]
}

fn label_from_path(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("creds");
    // ".credentials.har" → "har", ".credentials" → "live"
    let trimmed = stem
        .trim_start_matches('.')
        .trim_start_matches("credentials");
    let cleaned = trimmed.trim_start_matches('.');
    if cleaned.is_empty() {
        "live".into()
    } else {
        cleaned.to_string()
    }
}

/// Read the access token and (optional) subscription tier from a credentials
/// file. Errors when the file is missing, malformed, or has no token.
pub fn read_token(path: &Path) -> Result<TokenInfo> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
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
