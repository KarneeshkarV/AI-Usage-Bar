use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub refresh: RefreshConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub display: DisplayConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshConfig {
    pub interval_secs: u64,
    pub cost_refresh_secs: u64,
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            interval_secs: 300,
            cost_refresh_secs: 3600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvidersConfig {
    #[serde(default)]
    pub codex: CodexProviderConfig,
    #[serde(default)]
    pub claude: ClaudeProviderConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexProviderConfig {
    pub enabled: bool,
    pub binary: Option<String>,
}

impl Default for CodexProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            binary: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeProviderConfig {
    pub enabled: bool,
    pub binary: Option<String>,
    pub prefer: Vec<String>,
    #[serde(default)]
    pub accounts_paths: Option<Vec<PathBuf>>,
}

impl Default for ClaudeProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            binary: None,
            prefer: vec!["oauth_usage".into(), "cookies".into(), "pty".into()],
            accounts_paths: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub merge_text: bool,
    pub show_cost: bool,
    pub warn_threshold: u8,
    pub crit_threshold: u8,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            merge_text: true,
            show_cost: true,
            warn_threshold: 70,
            crit_threshold: 90,
        }
    }
}

pub fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no XDG config dir")?;
    Ok(base.join("ai-usage-bar").join("config.toml"))
}

pub fn cache_dir() -> Result<PathBuf> {
    let base = dirs::cache_dir().context("no XDG cache dir")?;
    let dir = base.join("ai-usage-bar");
    std::fs::create_dir_all(&dir).ok();
    Ok(dir)
}

impl Config {
    pub fn load_or_default() -> Result<Self> {
        let p = config_path()?;
        if !p.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
        let cfg: Config = toml::from_str(&raw).with_context(|| format!("parse {}", p.display()))?;
        Ok(cfg)
    }
}
