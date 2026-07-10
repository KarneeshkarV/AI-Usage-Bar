//! Read Codex OAuth tokens from the local Codex home (`~/.codex/auth.json`).

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CodexTokens {
    pub access_token: String,
    pub account_id: Option<String>,
}

#[derive(Deserialize)]
struct AuthFile {
    tokens: Option<TokensBlock>,
}

#[derive(Deserialize)]
struct TokensBlock {
    access_token: Option<String>,
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
    let auth: AuthFile =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    let tokens = auth
        .tokens
        .context("no tokens block in Codex auth.json")?;
    let access_token = tokens
        .access_token
        .filter(|t| !t.is_empty())
        .context("no access_token in Codex auth.json")?;
    Ok(CodexTokens {
        access_token,
        account_id: tokens.account_id.filter(|s| !s.is_empty()),
    })
}
