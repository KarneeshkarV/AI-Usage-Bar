pub mod api;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{Config, CursorProviderConfig, ResetStyle};
use crate::pace::{self, DEFAULT_WEEKLY_MINUTES};
use crate::util::time::reset_label;
use crate::waybar_proto::State;

use self::api::CursorUsage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorSnapshot {
    pub primary: Option<Window>,
    /// Included plan spend this period, in dollars.
    pub plan_used_usd: Option<f64>,
    /// Plan spend limit, in dollars.
    pub plan_limit_usd: Option<f64>,
    pub on_demand_used_usd: Option<f64>,
    pub on_demand_limit_usd: Option<f64>,
    #[serde(default)]
    pub membership_type: Option<String>,
    #[serde(default)]
    pub account_email: Option<String>,
    /// Legacy request-based plan fields.
    #[serde(default)]
    pub requests_used: Option<i64>,
    #[serde(default)]
    pub requests_limit: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub used_percent: f64,
    pub window_minutes: Option<u32>,
    pub resets_at: Option<DateTime<Utc>>,
}

impl CursorSnapshot {
    pub fn from_usage(info: &CursorUsage) -> Self {
        let window_minutes = match (info.billing_cycle_start, info.billing_cycle_end) {
            (Some(start), Some(end)) if end > start => {
                let secs = end.signed_duration_since(start).num_seconds();
                if secs > 0 {
                    Some((secs / 60) as u32)
                } else {
                    None
                }
            }
            _ => None,
        };

        let primary = Some(Window {
            used_percent: info.plan_percent_used.clamp(0.0, 100.0),
            window_minutes,
            resets_at: info.billing_cycle_end,
        });

        Self {
            primary,
            plan_used_usd: info.plan_used_usd,
            plan_limit_usd: info.plan_limit_usd,
            on_demand_used_usd: info.on_demand_used_usd,
            on_demand_limit_usd: info.on_demand_limit_usd,
            membership_type: info.membership_type.clone(),
            account_email: info.account_email.clone(),
            requests_used: info.requests_used,
            requests_limit: info.requests_limit,
            error: None,
        }
    }

    pub fn error_snap(msg: impl Into<String>) -> Self {
        Self {
            primary: None,
            plan_used_usd: None,
            plan_limit_usd: None,
            on_demand_used_usd: None,
            on_demand_limit_usd: None,
            membership_type: None,
            account_email: None,
            requests_used: None,
            requests_limit: None,
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
                let mut header = format!("Cursor: {p}% used");
                if let Some(m) = self.membership_type.as_deref() {
                    header.push_str(&format!(" ({})", format_membership(m)));
                }
                out.push(header);
            }
            None => out.push(format!(
                "Cursor: {}",
                self.error.as_deref().unwrap_or("no data")
            )),
        }
        if let Some(line) = self.primary_line("  primary  ", style) {
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
        if let Some(email) = &self.account_email {
            out.push(format!("account: {email}"));
        }
        if let Some(m) = &self.membership_type {
            out.push(format!("plan: {}", format_membership(m)));
        }
        if let Some(line) = self.primary_line("primary", style) {
            out.push(line);
        }
        if let Some(w) = &self.primary
            && let Some(line) = pace_line(w, Utc::now(), "  ")
        {
            out.push(line);
        }
        if let (Some(used), Some(limit)) = (self.plan_used_usd, self.plan_limit_usd) {
            out.push(format!("included: ${used:.2} / ${limit:.2}"));
        }
        if let (Some(used), Some(limit)) = (self.requests_used, self.requests_limit) {
            out.push(format!("requests: {used} / {limit}"));
        }
        if let Some(line) = self.on_demand_line("on-demand") {
            out.push(line);
        }
        if let Some(e) = &self.error {
            out.push(format!("error: {e}"));
        }
        out
    }

    fn primary_line(&self, label: &str, style: ResetStyle) -> Option<String> {
        let w = self.primary.as_ref()?;
        let pct = w.used_percent.round().clamp(0.0, 100.0) as u8;
        let mut parts = vec![format!("{label} {pct}% used")];
        if let (Some(used), Some(limit)) = (self.requests_used, self.requests_limit) {
            parts.push(format!("{used}/{limit} req"));
        } else if let (Some(used), Some(limit)) = (self.plan_used_usd, self.plan_limit_usd) {
            parts.push(format!("${used:.2}/${limit:.2}"));
        }
        if let Some(t) = w.resets_at {
            parts.push(reset_label(t, style));
        }
        Some(parts.join(" · "))
    }

    fn on_demand_line(&self, label: &str) -> Option<String> {
        let used = self.on_demand_used_usd.unwrap_or(0.0);
        let limit = self.on_demand_limit_usd;
        if used <= 0.0 && limit.unwrap_or(0.0) <= 0.0 {
            return None;
        }
        match limit {
            Some(l) if l > 0.0 => Some(format!("{label} ${used:.2}/${l:.2}")),
            _ => Some(format!("{label} ${used:.2}")),
        }
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

fn format_membership(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "enterprise" => "Enterprise".into(),
        "pro" => "Pro".into(),
        "hobby" => "Hobby".into(),
        "team" => "Team".into(),
        other => other.to_string(),
    }
}

pub struct Client {
    cfg: CursorProviderConfig,
}

impl Client {
    pub fn new(cfg: CursorProviderConfig) -> Self {
        Self { cfg }
    }

    pub async fn refresh(&mut self) -> Result<Option<CursorSnapshot>> {
        if !self.cfg.is_active() {
            return Ok(None);
        }
        let Some(cookie) = self.cfg.resolved_cookie() else {
            return Ok(Some(CursorSnapshot::error_snap(
                "no session cookie configured",
            )));
        };
        match api::fetch_usage(&cookie).await {
            Ok(usage) => Ok(Some(CursorSnapshot::from_usage(&usage))),
            Err(e) => Ok(Some(CursorSnapshot::error_snap(format!("{e}")))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ResetStyle;
    use chrono::TimeZone;

    fn sample_summary_json() -> serde_json::Value {
        serde_json::from_str(
            r#"{
              "billingCycleStart": "2026-07-01T00:00:00.000Z",
              "billingCycleEnd": "2026-08-01T00:00:00.000Z",
              "membershipType": "pro",
              "individualUsage": {
                "plan": {
                  "used": 800,
                  "limit": 2000,
                  "totalPercentUsed": 40.0,
                  "autoPercentUsed": 35.0,
                  "apiPercentUsed": 45.0
                },
                "onDemand": {
                  "used": 150,
                  "limit": 500
                }
              }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn usage_summary_maps_to_window() {
        let usage = api::decode_usage_summary(sample_summary_json()).unwrap();
        let snap = CursorSnapshot::from_usage(&usage);
        let w = snap.primary.as_ref().unwrap();
        assert!((w.used_percent - 40.0).abs() < 1e-9);
        assert_eq!(
            w.resets_at,
            Some(Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap())
        );
        // 31 days in July
        assert_eq!(w.window_minutes, Some(31 * 24 * 60));
        assert!((snap.plan_used_usd.unwrap() - 8.0).abs() < 1e-9);
        assert!((snap.plan_limit_usd.unwrap() - 20.0).abs() < 1e-9);
        assert!((snap.on_demand_used_usd.unwrap() - 1.5).abs() < 1e-9);
        assert_eq!(snap.worst_percent(), Some(40));
        assert_eq!(snap.membership_type.as_deref(), Some("pro"));
    }

    #[test]
    fn plan_ratio_when_percent_fields_absent() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
              "billingCycleStart": "2026-07-01T00:00:00Z",
              "billingCycleEnd": "2026-07-31T00:00:00Z",
              "individualUsage": {
                "plan": { "used": 500, "limit": 2000 }
              }
            }"#,
        )
        .unwrap();
        let usage = api::decode_usage_summary(v).unwrap();
        assert!((usage.plan_percent_used - 25.0).abs() < 1e-9);
        let snap = CursorSnapshot::from_usage(&usage);
        assert_eq!(snap.worst_percent(), Some(25));
    }

    #[test]
    fn legacy_request_usage_percent() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
              "gpt-4": {
                "numRequestsTotal": 120,
                "maxRequestUsage": 500
              }
            }"#,
        )
        .unwrap();
        let (used, limit, pct) = api::decode_legacy_usage(v).unwrap();
        assert_eq!(used, Some(120));
        assert_eq!(limit, Some(500));
        assert!((pct - 24.0).abs() < 1e-9);
    }

    #[test]
    fn tooltip_includes_primary_and_on_demand() {
        let usage = api::decode_usage_summary(sample_summary_json()).unwrap();
        let snap = CursorSnapshot::from_usage(&usage);
        let lines = snap.tooltip_lines(ResetStyle::Absolute);
        assert_eq!(lines[0], "Cursor: 40% used (Pro)");
        assert!(lines[1].starts_with("  primary  "));
        assert!(lines[1].contains("40% used"));
        assert!(lines[1].contains("$8.00/$20.00"));
        assert!(lines[1].contains(" · "));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("on-demand") && l.contains("$1.50/$5.00"))
        );
    }

    #[test]
    fn on_demand_omitted_when_zero() {
        let mut usage = api::decode_usage_summary(sample_summary_json()).unwrap();
        usage.on_demand_used_usd = Some(0.0);
        usage.on_demand_limit_usd = Some(0.0);
        let snap = CursorSnapshot::from_usage(&usage);
        let lines = snap.tooltip_lines(ResetStyle::Countdown);
        assert!(!lines.iter().any(|l| l.contains("on-demand")));
    }
}
