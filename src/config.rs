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
    #[serde(default)]
    pub ping: PingConfig,
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
    #[serde(default)]
    pub grok: GrokProviderConfig,
    #[serde(default)]
    pub cursor: CursorProviderConfig,
}

/// Shared enablement for providers that auto-detect credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProviderMode {
    /// Enable when local credentials / binary are present.
    #[default]
    Auto,
    /// Always attempt a refresh.
    On,
    /// Never poll; omit from Waybar entirely.
    Off,
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
}

impl Default for ClaudeProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            binary: None,
            prefer: vec!["oauth_usage".into(), "cookies".into(), "pty".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrokProviderConfig {
    #[serde(default)]
    pub mode: ProviderMode,
    #[serde(default)]
    pub binary: Option<String>,
}

impl Default for GrokProviderConfig {
    fn default() -> Self {
        Self {
            mode: ProviderMode::Auto,
            binary: None,
        }
    }
}

impl GrokProviderConfig {
    /// Whether the provider should be polled / shown.
    ///
    /// Auto activates when `~/.grok/auth.json` exists (or `$GROK_HOME/auth.json`)
    /// or the `grok` binary resolves on PATH / config override.
    pub fn is_active(&self) -> bool {
        match self.mode {
            ProviderMode::Off => false,
            ProviderMode::On => true,
            ProviderMode::Auto => {
                grok_auth_json_path().is_some_and(|p| p.is_file())
                    || crate::util::path::resolve_binary("grok", self.binary.as_deref()).is_some()
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorProviderConfig {
    #[serde(default)]
    pub mode: ProviderMode,
    /// Raw `Cookie` header value (typically `WorkosCursorSessionToken=...`).
    /// Prefer `AI_USAGE_BAR_CURSOR_COOKIE` so the secret stays out of the file.
    #[serde(default)]
    pub cookie: Option<String>,
}

impl Default for CursorProviderConfig {
    fn default() -> Self {
        Self {
            mode: ProviderMode::Auto,
            cookie: None,
        }
    }
}

impl CursorProviderConfig {
    /// Env override wins over the TOML `cookie` field.
    pub fn resolved_cookie(&self) -> Option<String> {
        if let Ok(env) = std::env::var("AI_USAGE_BAR_CURSOR_COOKIE") {
            let trimmed = env.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        self.cookie
            .as_ref()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
    }

    /// Auto activates when a session cookie is configured (config or env).
    pub fn is_active(&self) -> bool {
        match self.mode {
            ProviderMode::Off => false,
            ProviderMode::On => true,
            ProviderMode::Auto => self.resolved_cookie().is_some(),
        }
    }
}

/// CodexBar `GrokCredentialsStore.grokHomeURL`: `$GROK_HOME` or `~/.grok`.
pub fn grok_home_dir() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("GROK_HOME") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    dirs::home_dir().map(|h| h.join(".grok"))
}

pub fn grok_auth_json_path() -> Option<PathBuf> {
    grok_home_dir().map(|h| h.join("auth.json"))
}

/// How usage-window reset times are rendered in tooltips, TUI, and status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ResetStyle {
    /// Relative countdown, e.g. `in 2h 14m` / `now`.
    #[default]
    Countdown,
    /// Local absolute time, e.g. `14:30` / `tomorrow, 14:30` / `Feb 3, 14:30`.
    Absolute,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub merge_text: bool,
    pub show_cost: bool,
    pub warn_threshold: u8,
    pub crit_threshold: u8,
    /// Countdown vs absolute local time for reset phrasing.
    #[serde(default)]
    pub reset_style: ResetStyle,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            merge_text: true,
            show_cost: true,
            warn_threshold: 70,
            crit_threshold: 90,
            reset_style: ResetStyle::Countdown,
        }
    }
}

/// Auto-ping providers just after their 5h session window resets, so the ping
/// becomes the first activity of the new window and anchors its start at a
/// predictable time. Each ping is a headless one-shot (`claude -p ping`) run in
/// an empty scratch dir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingConfig {
    pub enabled: bool,
    /// How far (seconds) on either side of the `reset + 10s` fire time a
    /// boundary is still eligible to schedule. Must exceed the poll interval so
    /// a reset observed on the prior poll is still caught.
    pub threshold_secs: u64,
    pub claude_model: String,
    pub codex_model: String,
    pub codex_reasoning: String,
}

impl Default for PingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_secs: 600,
            claude_model: "sonnet".into(),
            codex_model: "gpt-5.4".into(),
            // `minimal` is rejected for gpt-5.4 (its image_gen/web_search tools
            // are incompatible with minimal effort); `low` is the cheapest that
            // the API accepts.
            codex_reasoning: "low".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_style_defaults_when_absent() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.display.reset_style, ResetStyle::Countdown);

        let cfg: Config = toml::from_str(
            r#"
            [display]
            merge_text = false
            show_cost = false
            warn_threshold = 50
            crit_threshold = 80
            "#,
        )
        .unwrap();
        assert_eq!(cfg.display.reset_style, ResetStyle::Countdown);
        assert!(!cfg.display.merge_text);
    }

    #[test]
    fn reset_style_parses_absolute() {
        let cfg: Config = toml::from_str(
            r#"
            [display]
            merge_text = true
            show_cost = true
            warn_threshold = 70
            crit_threshold = 90
            reset_style = "absolute"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.display.reset_style, ResetStyle::Absolute);
    }

    #[test]
    fn grok_provider_defaults_when_absent() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.providers.grok.mode, ProviderMode::Auto);
        assert!(cfg.providers.grok.binary.is_none());
    }

    #[test]
    fn grok_provider_mode_parses() {
        let cfg: Config = toml::from_str(
            r#"
            [providers.grok]
            mode = "off"
            binary = "/usr/local/bin/grok"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.providers.grok.mode, ProviderMode::Off);
        assert_eq!(
            cfg.providers.grok.binary.as_deref(),
            Some("/usr/local/bin/grok")
        );
        assert!(!cfg.providers.grok.is_active());
    }

    #[test]
    fn cursor_provider_defaults_when_absent() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.providers.cursor.mode, ProviderMode::Auto);
        assert!(cfg.providers.cursor.cookie.is_none());
        assert!(!cfg.providers.cursor.is_active());
    }

    #[test]
    fn cursor_provider_mode_and_cookie_parse() {
        let cfg: Config = toml::from_str(
            r#"
            [providers.cursor]
            mode = "on"
            cookie = "WorkosCursorSessionToken=abc"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.providers.cursor.mode, ProviderMode::On);
        assert_eq!(
            cfg.providers.cursor.cookie.as_deref(),
            Some("WorkosCursorSessionToken=abc")
        );
        assert!(cfg.providers.cursor.is_active());
        assert_eq!(
            cfg.providers.cursor.resolved_cookie().as_deref(),
            Some("WorkosCursorSessionToken=abc")
        );
    }
}
