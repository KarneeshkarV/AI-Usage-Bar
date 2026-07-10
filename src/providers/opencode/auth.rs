//! OpenCode local credentials (`~/.local/share/opencode/auth.json`).

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// OpenCode always writes under `~/.local/share/opencode` (CodexBar / OpenCode CLI
/// convention), not under a custom `XDG_DATA_HOME`.
fn opencode_data_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".local").join("share").join("opencode"))
}

/// Default auth path: `~/.local/share/opencode/auth.json`.
pub fn default_auth_path() -> Option<PathBuf> {
    opencode_data_dir().map(|d| d.join("auth.json"))
}

/// Default SQLite DB: `~/.local/share/opencode/opencode.db`.
pub fn default_db_path() -> Option<PathBuf> {
    opencode_data_dir().map(|d| d.join("opencode.db"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeAuth {
    pub key: String,
    pub auth_type: Option<String>,
}

/// Read the `opencode-go` API key from an auth.json map.
pub fn load_auth(path: &Path) -> Result<OpenCodeAuth> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    parse_auth_json(&raw)
}

/// Parse auth.json contents (pure; for tests).
pub fn parse_auth_json(raw: &str) -> Result<OpenCodeAuth> {
    let value: Value = serde_json::from_str(raw).context("parse auth.json")?;
    parse_auth_value(&value)
}

fn parse_auth_value(value: &Value) -> Result<OpenCodeAuth> {
    let entry = value
        .get("opencode-go")
        .ok_or_else(|| anyhow::anyhow!("auth.json missing opencode-go entry"))?;

    #[derive(Deserialize)]
    struct Entry {
        #[serde(rename = "type")]
        auth_type: Option<String>,
        key: Option<String>,
    }

    let entry: Entry = serde_json::from_value(entry.clone()).context("decode opencode-go entry")?;
    let key = entry
        .key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .ok_or_else(|| anyhow::anyhow!("opencode-go key is empty"))?;

    Ok(OpenCodeAuth {
        key,
        auth_type: entry.auth_type,
    })
}

/// Whether a usable opencode-go key is present at the default path.
pub fn has_auth_key() -> bool {
    default_auth_path()
        .filter(|p| p.is_file())
        .and_then(|p| load_auth(&p).ok())
        .is_some()
}

/// Convenience: load from default path, or bail.
pub fn load_default_auth() -> Result<OpenCodeAuth> {
    let path = default_auth_path().ok_or_else(|| anyhow::anyhow!("no XDG data dir"))?;
    if !path.is_file() {
        bail!("auth.json not found at {}", path.display());
    }
    load_auth(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_opencode_go_key() {
        let raw = r#"{
          "opencode-go": {
            "type": "api",
            "key": "sk-test-key-123"
          },
          "other": { "type": "api", "key": "ignored" }
        }"#;
        let auth = parse_auth_json(raw).unwrap();
        assert_eq!(auth.key, "sk-test-key-123");
        assert_eq!(auth.auth_type.as_deref(), Some("api"));
    }

    #[test]
    fn rejects_empty_key() {
        let raw = r#"{ "opencode-go": { "type": "api", "key": "  " } }"#;
        assert!(parse_auth_json(raw).is_err());
    }

    #[test]
    fn rejects_missing_entry() {
        let raw = r#"{ "openai": { "type": "api", "key": "x" } }"#;
        assert!(parse_auth_json(raw).is_err());
    }
}
