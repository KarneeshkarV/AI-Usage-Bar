pub mod auth;
pub mod limits;
pub mod reset_credits;
pub mod rpc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{CodexProviderConfig, Config, ResetStyle};
use crate::pace::{self, DEFAULT_SESSION_MINUTES, DEFAULT_WEEKLY_MINUTES};
use crate::util::time::{expires_label, reset_label};
use crate::waybar_proto::State;

pub use reset_credits::ResetCreditsSnapshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexSnapshot {
    pub account_email: Option<String>,
    pub plan_type: Option<String>,
    pub primary: Option<Window>,
    pub secondary: Option<Window>,
    pub credits: Option<Credits>,
    /// Manual rate-limit reset credits (full 5h + weekly resets), with expiries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_credits: Option<ResetCreditsSnapshot>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub used_percent: f64,
    pub window_minutes: Option<u32>,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credits {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

impl CodexSnapshot {
    fn empty_error(error: String) -> Self {
        Self {
            account_email: None,
            plan_type: None,
            primary: None,
            secondary: None,
            credits: None,
            reset_credits: None,
            error: Some(error),
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
            Some(p) => {
                if let Some(rc) = &self.reset_credits
                    && rc.available_count > 0
                {
                    format!("{p}% used · {} reset credits", rc.available_count)
                } else {
                    format!("{p}% used")
                }
            }
            None => self.error.clone().unwrap_or_else(|| "no data".into()),
        }
    }

    pub fn tooltip_lines(&self, style: ResetStyle) -> Vec<String> {
        let mut out = Vec::new();
        let mut header = String::from("Codex");
        if let Some(plan) = &self.plan_type {
            header.push_str(&format!(" ({plan})"));
        }
        out.push(header);
        let now = Utc::now();
        if let Some(w) = &self.primary {
            out.push(window_line("  primary", w, style));
            if let Some(line) = pace_line(w, DEFAULT_SESSION_MINUTES, now, "    ") {
                out.push(line);
            }
        }
        if let Some(w) = &self.secondary {
            out.push(window_line("  secondary", w, style));
            if let Some(line) = pace_line(w, DEFAULT_WEEKLY_MINUTES, now, "    ") {
                out.push(line);
            }
        }
        if let Some(c) = &self.credits
            && c.has_credits
        {
            let bal = c.balance.clone().unwrap_or_else(|| "—".into());
            out.push(format!(
                "  credits: {bal}{}",
                if c.unlimited { " (unlimited)" } else { "" }
            ));
        }
        out.extend(reset_credit_lines(
            self.reset_credits.as_ref(),
            style,
            "  ",
            false,
        ));
        if let Some(e) = &self.error {
            out.push(format!("  error: {e}"));
        }
        out
    }

    pub fn detail_lines(&self, style: ResetStyle) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(email) = &self.account_email {
            out.push(format!("account: {email}"));
        }
        if let Some(plan) = &self.plan_type {
            out.push(format!("plan: {plan}"));
        }
        let now = Utc::now();
        if let Some(w) = &self.primary {
            out.push(window_line("primary", w, style));
            if let Some(line) = pace_line(w, DEFAULT_SESSION_MINUTES, now, "  ") {
                out.push(line);
            }
        }
        if let Some(w) = &self.secondary {
            out.push(window_line("secondary", w, style));
            if let Some(line) = pace_line(w, DEFAULT_WEEKLY_MINUTES, now, "  ") {
                out.push(line);
            }
        }
        if let Some(c) = &self.credits {
            out.push(format!(
                "credits: has={} unlimited={} balance={}",
                c.has_credits,
                c.unlimited,
                c.balance.clone().unwrap_or_default()
            ));
        }
        out.extend(reset_credit_lines(
            self.reset_credits.as_ref(),
            style,
            "",
            true,
        ));
        if let Some(e) = &self.error {
            out.push(format!("error: {e}"));
        }
        out
    }
}

fn window_line(label: &str, w: &Window, style: ResetStyle) -> String {
    let resets = match w.resets_at {
        Some(t) => format!(" ({})", reset_label(t, style)),
        None => String::new(),
    };
    let dur = w
        .window_minutes
        .map(|m| format!(" / {}h window", (m / 60).max(1)))
        .unwrap_or_default();
    format!("{label}: {:.1}%{dur}{resets}", w.used_percent)
}

fn pace_line(w: &Window, default_mins: u32, now: DateTime<Utc>, indent: &str) -> Option<String> {
    pace::line_for_window(
        w.used_percent,
        w.window_minutes,
        w.resets_at,
        now,
        default_mins,
        indent,
    )
}

/// Format rate-limit reset credit inventory with expiry times.
///
/// `detailed` lists every credit expiry; compact mode only shows the next one.
fn reset_credit_lines(
    rc: Option<&ResetCreditsSnapshot>,
    style: ResetStyle,
    indent: &str,
    detailed: bool,
) -> Vec<String> {
    let Some(rc) = rc else {
        return Vec::new();
    };
    if rc.available_count == 0 && rc.credits.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let n = rc.available_count;
    out.push(format!("{indent}reset credits: {n} available"));

    if detailed {
        if rc.credits.is_empty() {
            // Server count only — no per-credit expiry known.
        } else {
            for credit in &rc.credits {
                if let Some(exp) = credit.expires_at {
                    let title = credit
                        .title
                        .as_deref()
                        .filter(|t| !t.is_empty())
                        .unwrap_or("full reset");
                    out.push(format!(
                        "{indent}  · {title} ({})",
                        expires_label(exp, style)
                    ));
                } else {
                    let title = credit
                        .title
                        .as_deref()
                        .filter(|t| !t.is_empty())
                        .unwrap_or("full reset");
                    out.push(format!("{indent}  · {title} (no expiry)"));
                }
            }
        }
    } else if let Some(next) = rc.credits.iter().find_map(|c| c.expires_at) {
        out.push(format!("{indent}  next {}", expires_label(next, style)));
    }

    out
}

pub struct Client {
    cfg: CodexProviderConfig,
    rpc: Option<rpc::RpcClient>,
}

impl Client {
    pub fn new(cfg: CodexProviderConfig) -> Self {
        Self { cfg, rpc: None }
    }

    pub async fn refresh(&mut self) -> Result<Option<CodexSnapshot>> {
        if !self.cfg.enabled {
            return Ok(None);
        }
        if self.rpc.is_none() {
            match rpc::RpcClient::spawn(self.cfg.binary.as_deref()).await {
                Ok(c) => self.rpc = Some(c),
                Err(e) => {
                    let mut snap = CodexSnapshot::empty_error(format!("spawn: {e}"));
                    enrich_reset_credits(&mut snap).await;
                    return Ok(Some(snap));
                }
            }
        }
        let rpc = self.rpc.as_mut().unwrap();
        let mut snap = match limits::fetch(rpc).await {
            Ok(s) => s,
            Err(e) => {
                // Recycle on error; next refresh will respawn.
                self.rpc = None;
                CodexSnapshot::empty_error(format!("rpc: {e}"))
            }
        };
        enrich_reset_credits(&mut snap).await;
        Ok(Some(snap))
    }
}

/// Attach reset-credit inventory when OAuth credentials are available.
/// Failures are silent — usage windows still render without this enrichment.
async fn enrich_reset_credits(snap: &mut CodexSnapshot) {
    match reset_credits::fetch().await {
        Ok(rc) => snap.reset_credits = Some(rc),
        Err(e) => {
            tracing::debug!(error = %e, "codex reset credits unavailable");
        }
    }
}

#[cfg(test)]
mod display_tests {
    use super::reset_credits::ResetCredit;
    use super::*;
    use crate::config::ResetStyle;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn detail_lists_each_credit_expiry() {
        let snap = CodexSnapshot {
            account_email: None,
            plan_type: Some("plus".into()),
            primary: None,
            secondary: None,
            credits: None,
            reset_credits: Some(ResetCreditsSnapshot {
                available_count: 2,
                credits: vec![
                    ResetCredit {
                        status: "available".into(),
                        title: Some("Full reset (Weekly + 5 hr)".into()),
                        granted_at: None,
                        expires_at: Some(utc(2026, 7, 18, 0, 6)),
                    },
                    ResetCredit {
                        status: "available".into(),
                        title: Some("Full reset (Weekly + 5 hr)".into()),
                        granted_at: None,
                        expires_at: Some(utc(2026, 7, 26, 23, 26)),
                    },
                ],
            }),
            error: None,
        };
        let lines = snap.detail_lines(ResetStyle::Absolute);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("reset credits: 2 available"))
        );
        assert!(lines.iter().any(|l| l.contains("expires")));
        assert_eq!(lines.iter().filter(|l| l.contains("Full reset")).count(), 2);
    }

    #[test]
    fn tooltip_shows_next_expiry_only() {
        let snap = CodexSnapshot {
            account_email: None,
            plan_type: None,
            primary: None,
            secondary: None,
            credits: None,
            reset_credits: Some(ResetCreditsSnapshot {
                available_count: 3,
                credits: vec![ResetCredit {
                    status: "available".into(),
                    title: None,
                    granted_at: None,
                    expires_at: Some(utc(2026, 7, 18, 0, 6)),
                }],
            }),
            error: None,
        };
        let lines = snap.tooltip_lines(ResetStyle::Countdown);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("reset credits: 3 available"))
        );
        assert!(lines.iter().any(|l| l.contains("next")));
        assert!(lines.iter().any(|l| l.contains("expires")));
    }
}
