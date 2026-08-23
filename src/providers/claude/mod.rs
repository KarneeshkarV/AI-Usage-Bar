pub mod api;
pub mod cookies;
pub mod credentials;
pub mod oauth;
pub mod oauth_token_refresh;
pub mod pty;
pub mod usage_api;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{ClaudeProviderConfig, Config, ResetStyle};
use crate::pace::{self, DEFAULT_SESSION_MINUTES, DEFAULT_WEEKLY_MINUTES};
use crate::util::time::reset_label;
use crate::waybar_proto::State;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeSnapshot {
    pub account_email: Option<String>,
    pub plan_type: Option<String>,
    pub session: Option<Window>,
    pub weekly: Option<Window>,
    pub sonnet_weekly: Option<Window>,
    pub extra: Option<ExtraSpend>,
    pub source: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub used_percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtraSpend {
    pub used_usd: f64,
    pub limit_usd: f64,
    pub currency: String,
}

/// Internal carrier returned by each source (cookies/api/pty/oauth_usage),
/// merged into the public `ClaudeSnapshot` by the orchestrator.
#[derive(Default)]
pub struct ClaudeData {
    pub account_email: Option<String>,
    pub plan_type: Option<String>,
    pub session: Option<Window>,
    pub weekly: Option<Window>,
    pub sonnet_weekly: Option<Window>,
    pub extra: Option<ExtraSpend>,
}

impl ClaudeSnapshot {
    pub fn worst_percent(&self) -> Option<u8> {
        let s = self.session.as_ref().map(|w| w.used_percent)?;
        Some(s.round().clamp(0.0, 100.0) as u8)
    }

    pub fn state(&self, cfg: &Config) -> State {
        if self.error.is_some() && self.session.is_none() {
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
        let mut header = String::from("Claude");
        if let Some(plan) = &self.plan_type {
            header.push_str(&format!(" ({plan})"));
        }
        out.push(header);

        let now = Utc::now();
        if let Some(w) = &self.session {
            out.push(window_line("  session", w, style));
            if let Some(line) = pace_line(w, DEFAULT_SESSION_MINUTES, now, "    ") {
                out.push(line);
            }
        }
        if let Some(w) = &self.weekly {
            out.push(window_line("  weekly", w, style));
            if let Some(line) = pace_line(w, DEFAULT_WEEKLY_MINUTES, now, "    ") {
                out.push(line);
            }
        }
        if let Some(w) = &self.sonnet_weekly {
            out.push(window_line("  sonnet", w, style));
            if let Some(line) = pace_line(w, DEFAULT_WEEKLY_MINUTES, now, "    ") {
                out.push(line);
            }
        }
        if let Some(e) = &self.extra {
            out.push(format!(
                "  extra: ${:.2} / ${:.2} {}",
                e.used_usd, e.limit_usd, e.currency
            ));
        }
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
        if let Some(src) = &self.source {
            out.push(format!("source: {src}"));
        }
        let now = Utc::now();
        if let Some(w) = &self.session {
            out.push(window_line("session", w, style));
            if let Some(line) = pace_line(w, DEFAULT_SESSION_MINUTES, now, "  ") {
                out.push(line);
            }
        }
        if let Some(w) = &self.weekly {
            out.push(window_line("weekly", w, style));
            if let Some(line) = pace_line(w, DEFAULT_WEEKLY_MINUTES, now, "  ") {
                out.push(line);
            }
        }
        if let Some(w) = &self.sonnet_weekly {
            out.push(window_line("sonnet (weekly)", w, style));
            if let Some(line) = pace_line(w, DEFAULT_WEEKLY_MINUTES, now, "  ") {
                out.push(line);
            }
        }
        if let Some(e) = &self.extra {
            let pct = if e.limit_usd > 0.0 {
                (e.used_usd / e.limit_usd * 100.0).round() as u8
            } else {
                0
            };
            out.push(format!(
                "extra: ${:.2} / ${:.2} {} ({pct}% used)",
                e.used_usd, e.limit_usd, e.currency
            ));
        }
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
    format!("{label}: {:.1}%{resets}", w.used_percent)
}

fn pace_line(w: &Window, default_mins: u32, now: DateTime<Utc>, indent: &str) -> Option<String> {
    pace::line_for_window(w.used_percent, None, w.resets_at, now, default_mins, indent)
}

pub struct Client {
    cfg: ClaudeProviderConfig,
    pty: Option<pty::PtySession>,
    cached_session_key: Option<cookies::SessionKey>,
}

impl Client {
    pub fn new(cfg: ClaudeProviderConfig) -> Self {
        Self {
            cfg,
            pty: None,
            cached_session_key: None,
        }
    }

    pub async fn refresh(&mut self) -> Result<Option<ClaudeSnapshot>> {
        if !self.cfg.enabled {
            return Ok(None);
        }

        let order: Vec<String> = if self.cfg.prefer.is_empty() {
            vec!["oauth_usage".into(), "cookies".into(), "pty".into()]
        } else {
            self.cfg.prefer.clone()
        };

        let mut errors: Vec<String> = Vec::new();
        let mut data: Option<(ClaudeData, String)> = None;

        for source in &order {
            match source.as_str() {
                "oauth_usage" => match self.try_oauth_usage().await {
                    Ok(Some(d)) => {
                        data = Some((d, "oauth_usage".into()));
                        break;
                    }
                    Ok(None) => errors.push("oauth_usage: no credentials".into()),
                    Err(e) => errors.push(format!("oauth_usage: {e}")),
                },
                "cookies" | "web" | "api" => match self.try_cookies().await {
                    Ok(d) => {
                        data = Some(d);
                        break;
                    }
                    Err(e) => errors.push(format!("cookies: {e}")),
                },
                "pty" => match self.try_pty().await {
                    Ok(d) => {
                        data = Some(d);
                        break;
                    }
                    Err(e) => errors.push(format!("pty: {e}")),
                },
                "oauth" => {
                    // OAuth alone has no rate windows; metadata-only side-source below.
                }
                _ => {}
            }
        }

        // Always try JWT metadata as a side-source for email/plan if the
        // primary didn't surface them.
        let oauth_meta = oauth::fetch().await.ok();

        match data {
            Some((mut d, source)) => {
                if d.account_email.is_none() {
                    d.account_email = oauth_meta.as_ref().and_then(|m| m.email.clone());
                }
                if d.plan_type.is_none() {
                    d.plan_type = oauth_meta.as_ref().and_then(|m| m.plan.clone());
                }
                Ok(Some(ClaudeSnapshot {
                    account_email: d.account_email,
                    plan_type: d.plan_type,
                    session: d.session,
                    weekly: d.weekly,
                    sonnet_weekly: d.sonnet_weekly,
                    extra: d.extra,
                    source: Some(source),
                    error: None,
                }))
            }
            None => Ok(Some(ClaudeSnapshot {
                account_email: oauth_meta.as_ref().and_then(|m| m.email.clone()),
                plan_type: oauth_meta.and_then(|m| m.plan),
                session: None,
                weekly: None,
                sonnet_weekly: None,
                extra: None,
                source: None,
                error: Some(errors.join("; ")),
            })),
        }
    }

    async fn try_oauth_usage(&self) -> Result<Option<ClaudeData>> {
        let info = match oauth_token_refresh::ensure_fresh_claude_access_token().await {
            Ok(i) => i,
            Err(e) => {
                // Missing credentials file stays a skip so cookies/pty can still run.
                if credentials::credentials_path()
                    .map(|p| !p.is_file())
                    .unwrap_or(true)
                {
                    return Ok(None);
                }
                return Err(e);
            }
        };

        let mut data = usage_api::fetch(&info.access_token).await?;
        if data.plan_type.is_none() {
            data.plan_type = info.tier;
        }
        Ok(Some(data))
    }

    async fn try_cookies(&mut self) -> Result<(ClaudeData, String)> {
        let key = match &self.cached_session_key {
            Some(k) => k.clone(),
            None => {
                let k = cookies::find_session_key().await?;
                self.cached_session_key = Some(k.clone());
                k
            }
        };
        match api::fetch(&key.value).await {
            Ok(data) => Ok((data, format!("cookies: {}", key.source))),
            Err(e) => {
                // Session probably expired — drop and let the next refresh
                // re-scan the browser store.
                self.cached_session_key = None;
                Err(e)
            }
        }
    }

    async fn try_pty(&mut self) -> Result<(ClaudeData, String)> {
        let probe = match self.pty.as_mut() {
            Some(s) => s.probe_usage().await,
            None => match pty::PtySession::spawn(self.cfg.binary.as_deref()) {
                Ok(mut s) => {
                    let r = s.probe_usage().await;
                    self.pty = Some(s);
                    r
                }
                Err(e) => Err(e),
            },
        };
        match probe {
            Ok(usage) => Ok((
                ClaudeData {
                    session: usage.session,
                    weekly: usage.weekly,
                    ..Default::default()
                },
                "pty".into(),
            )),
            Err(e) => {
                self.pty = None;
                Err(e)
            }
        }
    }
}
