//! Poll provider statuspage.io feeds for public incidents.
//!
//! Appends `/api/v2/status.json` to each provider's status base URL. Failures are
//! logged at debug and ignored so a status endpoint outage never disturbs the bar.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::Config;

const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Slow cadence for status polls (piggybacks as a dedicated interval in the waybar loop).
pub const POLL_INTERVAL_SECS: u64 = 15 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Indicator {
    None,
    Minor,
    Major,
    Critical,
}

impl Indicator {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "minor" => Some(Self::Minor),
            "major" => Some(Self::Major),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minor => "minor",
            Self::Major => "major",
            Self::Critical => "critical",
        }
    }

    pub fn is_incident(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderStatus {
    /// Stable id: `codex`, `claude`, `cursor`.
    pub provider: String,
    pub indicator: Indicator,
    pub description: String,
}

impl ProviderStatus {
    /// Tooltip / status / tui line under the provider when `indicator != none`.
    pub fn display_line(&self) -> Option<String> {
        if !self.indicator.is_incident() {
            return None;
        }
        let desc = first_sentence(&self.description);
        if desc.is_empty() {
            Some(format!("  ⚠ {} incident", self.indicator.as_str()))
        } else {
            Some(format!("  ⚠ {} incident: {desc}", self.indicator.as_str()))
        }
    }
}

/// Statuspage bases for providers that expose a public feed.
pub fn status_pages_for(cfg: &Config) -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    if cfg.providers.codex.enabled {
        out.push(("codex", "https://status.openai.com"));
    }
    if cfg.providers.claude.enabled {
        out.push(("claude", "https://status.claude.com"));
    }
    if cfg.providers.cursor.is_active() {
        out.push(("cursor", "https://status.cursor.com"));
    }
    // grok and opencode have no statuspage — skip.
    out
}

/// Fetch status for every active provider that has a status page.
///
/// Successful fetches replace previous entries; failures keep the prior value
/// for that provider (and are logged at debug only).
pub async fn refresh_all(cfg: &Config, previous: &[ProviderStatus]) -> Vec<ProviderStatus> {
    let pages = status_pages_for(cfg);
    if pages.is_empty() {
        return Vec::new();
    }

    let client = match reqwest::Client::builder().timeout(FETCH_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "provider status: build client failed");
            return previous
                .iter()
                .filter(|s| pages.iter().any(|(id, _)| *id == s.provider))
                .cloned()
                .collect();
        }
    };

    let mut futs = Vec::with_capacity(pages.len());
    for (provider, base) in &pages {
        let client = client.clone();
        let provider = *provider;
        let base = *base;
        futs.push(async move {
            match fetch_one(&client, base).await {
                Ok(status) => Some(ProviderStatus {
                    provider: provider.to_string(),
                    indicator: status.indicator,
                    description: status.description,
                }),
                Err(e) => {
                    tracing::debug!(provider, error = %e, "provider status fetch failed");
                    None
                }
            }
        });
    }

    let results = futures::future::join_all(futs).await;
    let mut out = Vec::with_capacity(pages.len());
    for ((id, _), fresh) in pages.iter().zip(results) {
        if let Some(s) = fresh {
            out.push(s);
        } else if let Some(prev) = previous.iter().find(|s| s.provider == *id) {
            out.push(prev.clone());
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedStatus {
    pub indicator: Indicator,
    pub description: String,
}

async fn fetch_one(client: &reqwest::Client, base: &str) -> Result<ParsedStatus> {
    let url = format!("{}/api/v2/status.json", base.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status_code = resp.status();
    if !status_code.is_success() {
        anyhow::bail!("status page returned {status_code}");
    }
    let bytes = resp.bytes().await.context("read status body")?;
    parse_statuspage_json(&bytes)
}

/// Parse statuspage.io `/api/v2/status.json` body.
pub fn parse_statuspage_json(bytes: &[u8]) -> Result<ParsedStatus> {
    #[derive(Deserialize)]
    struct Envelope {
        status: StatusBody,
    }
    #[derive(Deserialize)]
    struct StatusBody {
        indicator: String,
        #[serde(default)]
        description: Option<String>,
    }

    let env: Envelope = serde_json::from_slice(bytes).context("decode statuspage json")?;
    let indicator = Indicator::parse(&env.status.indicator)
        .with_context(|| format!("unknown status indicator {:?}", env.status.indicator))?;
    Ok(ParsedStatus {
        indicator,
        description: env
            .status
            .description
            .unwrap_or_default()
            .trim()
            .to_string(),
    })
}

/// Look up a cached status entry by provider id.
pub fn find<'a>(statuses: &'a [ProviderStatus], provider: &str) -> Option<&'a ProviderStatus> {
    statuses.iter().find(|s| s.provider == provider)
}

/// True when any stored status is a non-`none` incident.
pub fn any_incident(statuses: &[ProviderStatus]) -> bool {
    statuses.iter().any(|s| s.indicator.is_incident())
}

/// First sentence of a status description (trimmed).
fn first_sentence(desc: &str) -> String {
    let trimmed = desc.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Split on `. ` / `.\n` so abbreviations like "Dr." keep working poorly but
    // status copy is usually short single sentences.
    let end = trimmed
        .find(". ")
        .or_else(|| trimmed.find(".\n"))
        .unwrap_or(trimmed.len());
    let mut sentence = trimmed[..end].trim().to_string();
    // Drop a trailing period if we split mid-string; keep it if it is the whole body.
    if end < trimmed.len() {
        // already excluded the period
    } else {
        sentence = sentence.trim_end_matches('.').trim().to_string();
    }
    sentence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_all_four_indicators() {
        for (raw, expected) in [
            ("none", Indicator::None),
            ("minor", Indicator::Minor),
            ("major", Indicator::Major),
            ("critical", Indicator::Critical),
        ] {
            let json = format!(
                r#"{{"status":{{"indicator":"{raw}","description":"All Systems Operational"}}}}"#
            );
            let parsed = parse_statuspage_json(json.as_bytes()).unwrap();
            assert_eq!(parsed.indicator, expected);
            assert_eq!(parsed.description, "All Systems Operational");
        }
    }

    #[test]
    fn parse_real_world_shape() {
        let json = r#"{
            "page":{"id":"x","name":"OpenAI","url":"https://status.openai.com/","updated_at":"2026-07-09T19:25:56Z"},
            "status":{"description":"Partial System Degradation","indicator":"minor"}
        }"#;
        let parsed = parse_statuspage_json(json.as_bytes()).unwrap();
        assert_eq!(parsed.indicator, Indicator::Minor);
        assert_eq!(parsed.description, "Partial System Degradation");
    }

    #[test]
    fn display_line_only_for_incidents() {
        let ok = ProviderStatus {
            provider: "codex".into(),
            indicator: Indicator::None,
            description: "All Systems Operational".into(),
        };
        assert!(ok.display_line().is_none());

        let minor = ProviderStatus {
            provider: "codex".into(),
            indicator: Indicator::Minor,
            description: "Partial System Degradation. More detail later.".into(),
        };
        assert_eq!(
            minor.display_line().as_deref(),
            Some("  ⚠ minor incident: Partial System Degradation")
        );

        let major = ProviderStatus {
            provider: "claude".into(),
            indicator: Indicator::Major,
            description: "  API outage  ".into(),
        };
        assert_eq!(
            major.display_line().as_deref(),
            Some("  ⚠ major incident: API outage")
        );
    }

    #[test]
    fn indicator_parse_is_case_insensitive() {
        assert_eq!(Indicator::parse("MINOR"), Some(Indicator::Minor));
        assert_eq!(Indicator::parse("Critical"), Some(Indicator::Critical));
        assert_eq!(Indicator::parse("weird"), None);
    }

    #[test]
    fn status_pages_skip_inactive() {
        let mut cfg = Config::default();
        cfg.providers.codex.enabled = true;
        cfg.providers.claude.enabled = false;
        cfg.providers.cursor.mode = crate::config::ProviderMode::Off;
        let pages = status_pages_for(&cfg);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].0, "codex");
    }
}
