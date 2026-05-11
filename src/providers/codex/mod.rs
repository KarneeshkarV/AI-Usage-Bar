pub mod limits;
pub mod rpc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{CodexProviderConfig, Config};
use crate::util::time::until;
use crate::waybar_proto::State;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexSnapshot {
    pub account_email: Option<String>,
    pub plan_type: Option<String>,
    pub primary: Option<Window>,
    pub secondary: Option<Window>,
    pub credits: Option<Credits>,
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
    pub fn worst_percent(&self) -> Option<u8> {
        let p = self.primary.as_ref().map(|w| w.used_percent).unwrap_or(0.0);
        let s = self.secondary.as_ref().map(|w| w.used_percent).unwrap_or(0.0);
        let pct = p.max(s).round().clamp(0.0, 100.0) as u8;
        if self.primary.is_some() || self.secondary.is_some() {
            Some(pct)
        } else {
            None
        }
    }

    pub fn state(&self, cfg: &Config) -> State {
        if self.error.is_some() && self.primary.is_none() && self.secondary.is_none() {
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

    pub fn tooltip_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut header = String::from("Codex");
        if let Some(plan) = &self.plan_type {
            header.push_str(&format!(" ({plan})"));
        }
        out.push(header);
        if let Some(w) = &self.primary {
            out.push(window_line("  primary", w));
        }
        if let Some(w) = &self.secondary {
            out.push(window_line("  secondary", w));
        }
        if let Some(c) = &self.credits
            && c.has_credits
        {
            let bal = c.balance.clone().unwrap_or_else(|| "—".into());
            out.push(format!("  credits: {bal}{}", if c.unlimited { " (unlimited)" } else { "" }));
        }
        if let Some(e) = &self.error {
            out.push(format!("  error: {e}"));
        }
        out
    }

    pub fn detail_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(email) = &self.account_email {
            out.push(format!("account: {email}"));
        }
        if let Some(plan) = &self.plan_type {
            out.push(format!("plan: {plan}"));
        }
        if let Some(w) = &self.primary {
            out.push(window_line("primary", w));
        }
        if let Some(w) = &self.secondary {
            out.push(window_line("secondary", w));
        }
        if let Some(c) = &self.credits {
            out.push(format!(
                "credits: has={} unlimited={} balance={}",
                c.has_credits,
                c.unlimited,
                c.balance.clone().unwrap_or_default()
            ));
        }
        if let Some(e) = &self.error {
            out.push(format!("error: {e}"));
        }
        out
    }
}

fn window_line(label: &str, w: &Window) -> String {
    let resets = match w.resets_at {
        Some(t) => format!(" (resets in {})", until(t)),
        None => String::new(),
    };
    let dur = w
        .window_minutes
        .map(|m| format!(" / {}h window", (m / 60).max(1)))
        .unwrap_or_default();
    format!("{label}: {:.1}%{dur}{resets}", w.used_percent)
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
                    return Ok(Some(CodexSnapshot {
                        account_email: None,
                        plan_type: None,
                        primary: None,
                        secondary: None,
                        credits: None,
                        error: Some(format!("spawn: {e}")),
                    }));
                }
            }
        }
        let rpc = self.rpc.as_mut().unwrap();
        let snap = match limits::fetch(rpc).await {
            Ok(s) => s,
            Err(e) => {
                // Recycle on error; next refresh will respawn.
                self.rpc = None;
                CodexSnapshot {
                    account_email: None,
                    plan_type: None,
                    primary: None,
                    secondary: None,
                    credits: None,
                    error: Some(format!("rpc: {e}")),
                }
            }
        };
        Ok(Some(snap))
    }
}
