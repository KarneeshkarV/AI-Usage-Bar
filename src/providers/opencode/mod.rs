pub mod auth;
pub mod balance;
pub mod local;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::{Config, OpenCodeProviderConfig, ResetStyle};
use crate::waybar_proto::State;

use self::balance::format_dollars;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenCodeSnapshot {
    /// Remaining Zen balance in USD (if the balance API responded).
    pub balance_usd: Option<f64>,
    /// Local opencode-go spend over the last 30 days, if the DB was readable.
    #[serde(default)]
    pub local_30d_cost_usd: Option<f64>,
    pub error: Option<String>,
}

impl OpenCodeSnapshot {
    pub fn error_snap(msg: impl Into<String>) -> Self {
        Self {
            balance_usd: None,
            local_30d_cost_usd: None,
            error: Some(msg.into()),
        }
    }

    /// OpenCode has no percent-based window — never trips warn/crit thresholds.
    #[allow(dead_code)]
    pub fn worst_percent(&self) -> Option<u8> {
        None
    }

    pub fn state(&self, _cfg: &Config) -> State {
        // Zero / negative balance surfaces as crit.
        if let Some(b) = self.balance_usd
            && b <= 0.0
        {
            return State::Crit;
        }
        // Auth only when we have nothing useful (no balance and no local spend).
        if self.error.is_some() && self.balance_usd.is_none() && self.local_30d_cost_usd.is_none() {
            return State::Auth;
        }
        State::Ok
    }

    pub fn summary_line(&self) -> String {
        if let Some(b) = self.balance_usd {
            return format!("{} balance", format_dollars(b));
        }
        if let Some(cost) = self.local_30d_cost_usd {
            return format!("last 30d {}", format_dollars(cost));
        }
        self.error.clone().unwrap_or_else(|| "no data".into())
    }

    pub fn tooltip_lines(&self, _style: ResetStyle) -> Vec<String> {
        let mut out = Vec::new();
        match self.balance_usd {
            Some(b) => out.push(format!("OpenCode: {} balance", format_dollars(b))),
            None if self.local_30d_cost_usd.is_some() => {
                out.push("OpenCode: balance unavailable".into());
            }
            None => out.push(format!(
                "OpenCode: {}",
                self.error.as_deref().unwrap_or("no data")
            )),
        }
        if let Some(cost) = self.local_30d_cost_usd {
            out.push(format!("  last 30d  {}", format_dollars(cost)));
        }
        if let Some(e) = &self.error
            && self.balance_usd.is_some()
        {
            out.push(format!("  error: {e}"));
        }
        out
    }

    pub fn detail_lines(&self, _style: ResetStyle) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(b) = self.balance_usd {
            out.push(format!("balance: {}", format_dollars(b)));
        }
        if let Some(cost) = self.local_30d_cost_usd {
            out.push(format!("last 30d: {}", format_dollars(cost)));
        }
        if let Some(e) = &self.error {
            out.push(format!("error: {e}"));
        }
        if out.is_empty() {
            out.push("no data".into());
        }
        out
    }

    /// Waybar text segment, e.g. `O $12.40`. `None` when nothing to show.
    pub fn waybar_segment(&self) -> Option<String> {
        if let Some(b) = self.balance_usd {
            return Some(format!("O {}", format_dollars(b)));
        }
        // Balance unavailable (the Zen balance API is not public yet) — the
        // local 30d spend is the next most useful figure for the bar.
        if let Some(c) = self.local_30d_cost_usd {
            return Some(format!("O {}", format_dollars(c)));
        }
        // Active but no data at all — keep a segment so tooltip/auth stay wired.
        if self.error.is_some() {
            return Some("O —".into());
        }
        None
    }
}

pub struct Client {
    cfg: OpenCodeProviderConfig,
}

impl Client {
    pub fn new(cfg: OpenCodeProviderConfig) -> Self {
        Self { cfg }
    }

    pub async fn refresh(&mut self) -> Result<Option<OpenCodeSnapshot>> {
        if !self.cfg.is_active() {
            return Ok(None);
        }

        let auth = match auth::load_default_auth() {
            Ok(a) => a,
            Err(e) => {
                return Ok(Some(OpenCodeSnapshot::error_snap(format!("auth: {e}"))));
            }
        };

        let balance_res = balance::fetch_balance(&auth.key).await;
        let local = auth::default_db_path()
            .filter(|p| p.is_file())
            .and_then(|p| local::read_local_usage(&p).ok());

        match balance_res {
            Ok(b) => Ok(Some(OpenCodeSnapshot {
                balance_usd: Some(b),
                local_30d_cost_usd: local.map(|u| u.last_30d_cost_usd),
                error: None,
            })),
            Err(e) => {
                // Degrade: still surface local usage when the balance call fails.
                if let Some(u) = local {
                    Ok(Some(OpenCodeSnapshot {
                        balance_usd: None,
                        local_30d_cost_usd: Some(u.last_30d_cost_usd),
                        error: Some(format!("balance: {e}")),
                    }))
                } else {
                    Ok(Some(OpenCodeSnapshot::error_snap(format!("balance: {e}"))))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ResetStyle;

    #[test]
    fn worst_percent_is_none() {
        let snap = OpenCodeSnapshot {
            balance_usd: Some(12.4),
            local_30d_cost_usd: Some(3.0),
            error: None,
        };
        assert_eq!(snap.worst_percent(), None);
        assert!(matches!(snap.state(&Config::default()), State::Ok));
    }

    #[test]
    fn zero_balance_is_crit() {
        let snap = OpenCodeSnapshot {
            balance_usd: Some(0.0),
            local_30d_cost_usd: None,
            error: None,
        };
        assert!(matches!(snap.state(&Config::default()), State::Crit));
    }

    #[test]
    fn tooltip_and_waybar_format() {
        let snap = OpenCodeSnapshot {
            balance_usd: Some(12.4),
            local_30d_cost_usd: Some(10.20226683),
            error: None,
        };
        assert_eq!(snap.waybar_segment().as_deref(), Some("O $12.40"));
        let lines = snap.tooltip_lines(ResetStyle::Countdown);
        assert_eq!(lines[0], "OpenCode: $12.40 balance");
        assert_eq!(lines[1], "  last 30d  $10.20");
        assert_eq!(snap.summary_line(), "$12.40 balance");
    }

    #[test]
    fn auth_error_without_balance() {
        let snap = OpenCodeSnapshot::error_snap("auth: missing");
        assert!(matches!(snap.state(&Config::default()), State::Auth));
        assert_eq!(snap.waybar_segment().as_deref(), Some("O —"));
    }
}
