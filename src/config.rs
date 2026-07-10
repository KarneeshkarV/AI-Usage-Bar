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
    /// Public statuspage.io incident polling (codex / claude / cursor).
    #[serde(default)]
    pub status: StatusConfig,
    /// Desktop notifications via `notify-send` (waybar loop only).
    #[serde(default)]
    pub notify: NotifyConfig,
}

/// Named refresh cadence. Explicit `interval_secs` / `cost_refresh_secs`
/// override the matching field from the preset (or from `normal` defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RefreshPreset {
    Fast,
    #[default]
    Normal,
    Slow,
}

impl RefreshPreset {
    pub fn usage_secs(self) -> u64 {
        match self {
            Self::Fast => 60,
            Self::Normal => 300,
            Self::Slow => 900,
        }
    }

    pub fn cost_secs(self) -> u64 {
        match self {
            Self::Fast => 900,
            Self::Normal => 3600,
            Self::Slow => 7200,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RefreshConfig {
    /// Optional named base cadence (`fast` / `normal` / `slow`).
    #[serde(default)]
    pub preset: Option<RefreshPreset>,
    /// Provider poll interval. Overrides the preset field when set.
    #[serde(default)]
    pub interval_secs: Option<u64>,
    /// Local cost-scan interval. Overrides the preset field when set.
    #[serde(default)]
    pub cost_refresh_secs: Option<u64>,
}

impl RefreshConfig {
    fn base_preset(&self) -> RefreshPreset {
        self.preset.unwrap_or(RefreshPreset::Normal)
    }

    /// Resolved provider-usage poll interval (seconds).
    pub fn usage_interval(&self) -> u64 {
        self.interval_secs
            .unwrap_or_else(|| self.base_preset().usage_secs())
    }

    /// Resolved local cost-scan interval (seconds).
    pub fn cost_interval(&self) -> u64 {
        self.cost_refresh_secs
            .unwrap_or_else(|| self.base_preset().cost_secs())
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
    #[serde(default)]
    pub opencode: OpenCodeProviderConfig,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenCodeProviderConfig {
    #[serde(default)]
    pub mode: ProviderMode,
}

impl Default for OpenCodeProviderConfig {
    fn default() -> Self {
        Self {
            mode: ProviderMode::Auto,
        }
    }
}

impl OpenCodeProviderConfig {
    /// Auto activates when `~/.local/share/opencode/auth.json` has an opencode-go key.
    pub fn is_active(&self) -> bool {
        match self.mode {
            ProviderMode::Off => false,
            ProviderMode::On => true,
            ProviderMode::Auto => crate::providers::opencode::auth::has_auth_key(),
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

/// Known provider ids accepted by `display.bar_providers`.
pub const BAR_PROVIDER_NAMES: &[&str] = &["codex", "claude", "grok", "cursor", "opencode"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub merge_text: bool,
    pub show_cost: bool,
    pub warn_threshold: u8,
    pub crit_threshold: u8,
    /// Countdown vs absolute local time for reset phrasing.
    #[serde(default)]
    pub reset_style: ResetStyle,
    /// Providers allowed in the Waybar *text* segment (and severity/percentage).
    /// When absent, every active provider appears (legacy behaviour). Tooltip /
    /// status / TUI always show all active providers.
    #[serde(default)]
    pub bar_providers: Option<Vec<String>>,
    /// Celebrate weekly quota resets with a Waybar emoji and TUI confetti.
    #[serde(default = "default_true")]
    pub confetti: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            merge_text: true,
            show_cost: true,
            warn_threshold: 70,
            crit_threshold: 90,
            reset_style: ResetStyle::Countdown,
            bar_providers: None,
            confetti: true,
        }
    }
}

impl DisplayConfig {
    /// Whether `provider` may appear in the compact Waybar text / severity.
    /// Unknown or empty filter lists are treated as "allow all".
    pub fn bar_provider_allowed(&self, provider: &str) -> bool {
        let Some(list) = &self.bar_providers else {
            return true;
        };
        if list.is_empty() {
            return true;
        }
        list.iter()
            .any(|p| p.eq_ignore_ascii_case(provider) && is_known_bar_provider(p))
    }

    /// Warn once about unknown `bar_providers` entries (call at waybar startup).
    pub fn warn_unknown_bar_providers(&self) {
        let Some(list) = &self.bar_providers else {
            return;
        };
        for name in list {
            if !is_known_bar_provider(name) {
                tracing::warn!(
                    provider = %name,
                    "unknown display.bar_providers entry; ignoring"
                );
            }
        }
    }
}

fn is_known_bar_provider(name: &str) -> bool {
    BAR_PROVIDER_NAMES
        .iter()
        .any(|k| name.eq_ignore_ascii_case(k))
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

/// Poll public status pages and surface non-`none` indicators in the bar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusConfig {
    /// When true (default), the waybar loop refreshes status pages on a slow cadence.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Desktop notifications for quota thresholds and window resets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyConfig {
    /// Master switch (default on). Also requires `notify-send` on PATH.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Emit a low-urgency notification when a window above warn resets.
    #[serde(default = "default_true")]
    pub on_reset: bool,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            on_reset: true,
        }
    }
}

fn default_true() -> bool {
    true
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

    #[test]
    fn opencode_provider_defaults_when_absent() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.providers.opencode.mode, ProviderMode::Auto);
    }

    #[test]
    fn opencode_provider_mode_parses() {
        let cfg: Config = toml::from_str(
            r#"
            [providers.opencode]
            mode = "off"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.providers.opencode.mode, ProviderMode::Off);
        assert!(!cfg.providers.opencode.is_active());
    }

    #[test]
    fn status_config_defaults_enabled() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.status.enabled);

        let cfg: Config = toml::from_str(
            r#"
            [status]
            enabled = false
            "#,
        )
        .unwrap();
        assert!(!cfg.status.enabled);
    }

    #[test]
    fn notify_config_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.notify.enabled);
        assert!(cfg.notify.on_reset);

        let cfg: Config = toml::from_str(
            r#"
            [notify]
            enabled = false
            on_reset = false
            "#,
        )
        .unwrap();
        assert!(!cfg.notify.enabled);
        assert!(!cfg.notify.on_reset);
    }

    #[test]
    fn refresh_no_preset_uses_normal_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.refresh.usage_interval(), 300);
        assert_eq!(cfg.refresh.cost_interval(), 3600);
        assert!(cfg.refresh.preset.is_none());
        assert!(cfg.refresh.interval_secs.is_none());
    }

    #[test]
    fn refresh_explicit_intervals_back_compat() {
        // Existing configs that set plain numbers must behave identically.
        let cfg: Config = toml::from_str(
            r#"
            [refresh]
            interval_secs = 120
            cost_refresh_secs = 1800
            "#,
        )
        .unwrap();
        assert_eq!(cfg.refresh.usage_interval(), 120);
        assert_eq!(cfg.refresh.cost_interval(), 1800);
    }

    #[test]
    fn refresh_preset_only() {
        for (preset, usage, cost) in [
            ("fast", 60, 900),
            ("normal", 300, 3600),
            ("slow", 900, 7200),
        ] {
            let raw = format!(
                r#"
                [refresh]
                preset = "{preset}"
                "#
            );
            let cfg: Config = toml::from_str(&raw).unwrap();
            assert_eq!(cfg.refresh.usage_interval(), usage, "preset={preset}");
            assert_eq!(cfg.refresh.cost_interval(), cost, "preset={preset}");
        }
    }

    #[test]
    fn refresh_preset_plus_field_override() {
        let cfg: Config = toml::from_str(
            r#"
            [refresh]
            preset = "fast"
            interval_secs = 45
            "#,
        )
        .unwrap();
        // Explicit usage overrides fast's 60; cost still from fast.
        assert_eq!(cfg.refresh.usage_interval(), 45);
        assert_eq!(cfg.refresh.cost_interval(), 900);

        let cfg: Config = toml::from_str(
            r#"
            [refresh]
            preset = "slow"
            cost_refresh_secs = 999
            "#,
        )
        .unwrap();
        assert_eq!(cfg.refresh.usage_interval(), 900);
        assert_eq!(cfg.refresh.cost_interval(), 999);
    }

    #[test]
    fn bar_providers_default_allows_all() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.display.bar_providers.is_none());
        assert!(cfg.display.bar_provider_allowed("codex"));
        assert!(cfg.display.bar_provider_allowed("opencode"));
    }

    #[test]
    fn bar_providers_filters_known_names() {
        let cfg: Config = toml::from_str(
            r#"
            [display]
            merge_text = true
            show_cost = true
            warn_threshold = 70
            crit_threshold = 90
            bar_providers = ["codex", "claude"]
            "#,
        )
        .unwrap();
        assert!(cfg.display.bar_provider_allowed("codex"));
        assert!(cfg.display.bar_provider_allowed("Claude"));
        assert!(!cfg.display.bar_provider_allowed("grok"));
        assert!(!cfg.display.bar_provider_allowed("cursor"));
    }

    #[test]
    fn bar_providers_unknown_names_ignored() {
        let cfg: Config = toml::from_str(
            r#"
            [display]
            merge_text = true
            show_cost = true
            warn_threshold = 70
            crit_threshold = 90
            bar_providers = ["not-a-provider", "codex"]
            "#,
        )
        .unwrap();
        assert!(cfg.display.bar_provider_allowed("codex"));
        assert!(!cfg.display.bar_provider_allowed("not-a-provider"));
    }

    #[test]
    fn confetti_defaults_on() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.display.confetti);

        let cfg: Config = toml::from_str(
            r#"
            [display]
            merge_text = true
            show_cost = true
            warn_threshold = 70
            crit_threshold = 90
            confetti = false
            "#,
        )
        .unwrap();
        assert!(!cfg.display.confetti);
    }
}
