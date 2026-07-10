pub mod rpc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{Config, GrokProviderConfig, ResetStyle};
use crate::pace::{self, DEFAULT_WEEKLY_MINUTES};
use crate::util::time::reset_label;
use crate::waybar_proto::State;

use self::rpc::{BillingInfo, cents_to_dollars};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrokSnapshot {
    pub primary: Option<Window>,
    /// Included (subscription) spend this period, in dollars.
    pub included_used_usd: Option<f64>,
    /// On-demand spend this period, in dollars.
    pub on_demand_used_usd: Option<f64>,
    /// Monthly included limit, in dollars.
    pub monthly_limit_usd: Option<f64>,
    /// Whether on-demand billing is enabled / has spend.
    #[serde(default)]
    pub on_demand_enabled: bool,
    #[serde(default)]
    pub subscription_tier: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub used_percent: f64,
    pub window_minutes: Option<u32>,
    pub resets_at: Option<DateTime<Utc>>,
}

impl GrokSnapshot {
    pub fn from_billing(info: &BillingInfo) -> Self {
        let primary = info.monthly_used_percent().map(|pct| Window {
            used_percent: pct,
            window_minutes: info.billing_period_minutes(),
            resets_at: info.period_end,
        });

        Self {
            primary,
            included_used_usd: info.included_used_cents.map(cents_to_dollars),
            on_demand_used_usd: info.on_demand_used_cents.map(cents_to_dollars),
            monthly_limit_usd: info.monthly_limit_cents.map(cents_to_dollars),
            on_demand_enabled: info.on_demand_enabled,
            subscription_tier: info.subscription_tier.clone(),
            error: None,
        }
    }

    pub fn error_snap(msg: impl Into<String>) -> Self {
        Self {
            primary: None,
            included_used_usd: None,
            on_demand_used_usd: None,
            monthly_limit_usd: None,
            on_demand_enabled: false,
            subscription_tier: None,
            error: Some(msg.into()),
        }
    }

    pub fn worst_percent(&self) -> Option<u8> {
        let p = self.primary.as_ref().map(|w| w.used_percent)?;
        Some(p.round().clamp(0.0, 100.0) as u8)
    }

    pub fn state(&self, cfg: &Config) -> State {
        if self.error.is_some() && self.primary.is_none() {
            return State::Auth;
        }
        State::from_pct(self.worst_percent().unwrap_or(0), cfg)
    }

    pub fn summary_line(&self) -> String {
        match self.worst_percent() {
            Some(p) => format!("{p}% used"),
            None => self.error.clone().unwrap_or_else(|| "no data".into()),
        }
    }

    pub fn tooltip_lines(&self, style: ResetStyle) -> Vec<String> {
        let mut out = Vec::new();
        match self.worst_percent() {
            Some(p) => {
                let mut header = format!("Grok: {p}% used");
                if let Some(tier) = &self.subscription_tier {
                    header.push_str(&format!(" ({tier})"));
                }
                out.push(header);
            }
            None => out.push(format!(
                "Grok: {}",
                self.error.as_deref().unwrap_or("no data")
            )),
        }
        if let Some(line) = self.monthly_line("  monthly  ", style) {
            out.push(line);
        }
        if let Some(w) = &self.primary
            && let Some(line) = pace_line(w, Utc::now(), "    ")
        {
            out.push(line);
        }
        if let Some(line) = self.on_demand_line("  on-demand") {
            out.push(line);
        }
        if let Some(e) = &self.error
            && self.primary.is_some()
        {
            out.push(format!("  error: {e}"));
        }
        out
    }

    pub fn detail_lines(&self, style: ResetStyle) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(tier) = &self.subscription_tier {
            out.push(format!("plan: {tier}"));
        }
        if let Some(line) = self.monthly_line("monthly", style) {
            out.push(line);
        }
        if let Some(w) = &self.primary
            && let Some(line) = pace_line(w, Utc::now(), "  ")
        {
            out.push(line);
        }
        if let Some(line) = self.on_demand_line("on-demand") {
            out.push(line);
        }
        if let (Some(used), Some(limit)) = (self.included_used_usd, self.monthly_limit_usd) {
            out.push(format!("included: ${used:.2} / ${limit:.2}"));
        }
        if let Some(e) = &self.error {
            out.push(format!("error: {e}"));
        }
        out
    }

    fn monthly_line(&self, label: &str, style: ResetStyle) -> Option<String> {
        let w = self.primary.as_ref()?;
        let pct = w.used_percent.round().clamp(0.0, 100.0) as u8;
        let mut parts = vec![format!("{label} {pct}% used")];
        if let (Some(used), Some(limit)) = (self.included_used_usd, self.monthly_limit_usd) {
            parts.push(format!("${used:.2}/${limit:.2}"));
        }
        if let Some(t) = w.resets_at {
            parts.push(reset_label(t, style));
        }
        Some(parts.join(" · "))
    }

    fn on_demand_line(&self, label: &str) -> Option<String> {
        let used = self.on_demand_used_usd.unwrap_or(0.0);
        // Only when on-demand is enabled or there is nonzero spend.
        if !self.on_demand_enabled && used <= 0.0 {
            return None;
        }
        Some(format!("{label} ${used:.2}"))
    }
}

fn pace_line(w: &Window, now: DateTime<Utc>, indent: &str) -> Option<String> {
    pace::line_for_window(
        w.used_percent,
        w.window_minutes,
        w.resets_at,
        now,
        DEFAULT_WEEKLY_MINUTES,
        indent,
    )
}

pub struct Client {
    cfg: GrokProviderConfig,
}

impl Client {
    pub fn new(cfg: GrokProviderConfig) -> Self {
        Self { cfg }
    }

    pub async fn refresh(&mut self) -> Result<Option<GrokSnapshot>> {
        if !self.cfg.is_active() {
            return Ok(None);
        }
        match rpc::fetch_billing(self.cfg.binary.as_deref()).await {
            Ok(billing) => Ok(Some(GrokSnapshot::from_billing(&billing))),
            Err(e) => Ok(Some(GrokSnapshot::error_snap(format!("rpc: {e}")))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ResetStyle;
    use chrono::TimeZone;

    fn sample_legacy_info() -> BillingInfo {
        rpc::decode_billing(
            serde_json::from_str(
                r#"{
                  "billingCycle": {
                    "billingPeriodStart": "2026-07-01T00:00:00Z",
                    "billingPeriodEnd": "2026-07-31T00:00:00Z"
                  },
                  "monthlyLimit": { "val": 3000 },
                  "on_demand_enabled": true,
                  "usage": {
                    "includedUsed": { "val": 420 },
                    "onDemandUsed": { "val": 100 },
                    "totalUsed": { "val": 520 }
                  }
                }"#,
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn snapshot_from_billing_math() {
        let snap = GrokSnapshot::from_billing(&sample_legacy_info());
        let w = snap.primary.as_ref().unwrap();
        // 520/3000 * 100
        assert!((w.used_percent - (520.0 / 3000.0 * 100.0)).abs() < 1e-9);
        assert_eq!(w.window_minutes, Some(30 * 24 * 60));
        assert_eq!(
            w.resets_at,
            Some(Utc.with_ymd_and_hms(2026, 7, 31, 0, 0, 0).unwrap())
        );
        assert!((snap.included_used_usd.unwrap() - 4.20).abs() < 1e-9);
        assert!((snap.on_demand_used_usd.unwrap() - 1.0).abs() < 1e-9);
        assert!((snap.monthly_limit_usd.unwrap() - 30.0).abs() < 1e-9);
        assert!(snap.on_demand_enabled);
        assert_eq!(snap.worst_percent(), Some(17)); // 17.333 → 17
    }

    #[test]
    fn tooltip_includes_monthly_and_on_demand() {
        let snap = GrokSnapshot::from_billing(&sample_legacy_info());
        let lines = snap.tooltip_lines(ResetStyle::Absolute);
        assert_eq!(lines[0], "Grok: 17% used");
        assert!(lines[1].starts_with("  monthly  "));
        assert!(lines[1].contains("17% used"));
        assert!(lines[1].contains("$4.20/$30.00"));
        assert!(lines[1].contains(" · "));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("on-demand") && l.contains("$1.00"))
        );
    }

    #[test]
    fn on_demand_omitted_when_disabled_and_zero() {
        let mut info = sample_legacy_info();
        info.on_demand_enabled = false;
        info.on_demand_used_cents = Some(0);
        info.included_used_cents = Some(420);
        // recompute percent from cents only: included 420 / limit 3000
        info.used_percent = None;
        let snap = GrokSnapshot::from_billing(&info);
        let lines = snap.tooltip_lines(ResetStyle::Countdown);
        assert!(!lines.iter().any(|l| l.contains("on-demand")));
    }

    #[test]
    fn modern_billing_tooltip() {
        let info = rpc::decode_billing(
            serde_json::from_str(
                r#"{
                  "config": {
                    "creditUsagePercent": 40.0,
                    "billingPeriodStart": "2026-07-05T00:00:00Z",
                    "billingPeriodEnd": "2026-07-12T00:00:00Z",
                    "onDemandUsed": { "val": 0 }
                  },
                  "subscription_tier": "SuperGrok"
                }"#,
            )
            .unwrap(),
        )
        .unwrap();
        let snap = GrokSnapshot::from_billing(&info);
        assert_eq!(snap.worst_percent(), Some(40));
        let lines = snap.tooltip_lines(ResetStyle::Countdown);
        assert_eq!(lines[0], "Grok: 40% used (SuperGrok)");
        assert!(lines[1].contains("40% used"));
        assert!(lines[1].contains("resets"));
        // No dollar amounts when cents fields absent.
        assert!(!lines[1].contains("$"));
    }
}
