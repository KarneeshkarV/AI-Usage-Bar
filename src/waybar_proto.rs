use serde::Serialize;

use crate::config::Config;
use crate::provider_status;
use crate::snapshot::Snapshot;

#[derive(Serialize)]
pub struct WaybarLine {
    pub text: String,
    pub alt: String,
    pub tooltip: String,
    pub class: String,
    pub percentage: u8,
}

impl WaybarLine {
    pub fn from_snapshot(snap: &Snapshot, cfg: &Config) -> Self {
        let mut parts: Vec<String> = Vec::new();
        let mut worst: u8 = 0;
        let mut tooltip = Vec::new();
        let mut state = State::Ok;
        let on_bar = |name: &str| cfg.display.bar_provider_allowed(name);

        if let Some(c) = &snap.codex {
            // Tooltip always includes every active provider.
            tooltip.extend(c.tooltip_lines(cfg.display.reset_style));
            push_status_line(&mut tooltip, &snap.provider_status, "codex");
            if on_bar("codex") {
                let pct = c.worst_percent().unwrap_or(0);
                parts.push(format!("C {}%", remaining_percent(pct)));
                worst = worst.max(pct);
                state = state.combine(c.state(cfg));
            }
        } else if on_bar("codex") {
            parts.push("C —".into());
            tooltip.push("Codex: (unavailable)".into());
            state = state.combine(State::Auth);
        } else {
            tooltip.push("Codex: (unavailable)".into());
        }

        if let Some(c) = &snap.claude {
            tooltip.extend(c.tooltip_lines(cfg.display.reset_style));
            push_status_line(&mut tooltip, &snap.provider_status, "claude");
            if on_bar("claude") {
                let pct = c.worst_percent().unwrap_or(0);
                parts.push(format!("Cl {}%", remaining_percent(pct)));
                worst = worst.max(pct);
                state = state.combine(c.state(cfg));
            }
        } else if on_bar("claude") {
            parts.push("Cl —".into());
            tooltip.push("Claude: (unavailable)".into());
            state = state.combine(State::Auth);
        } else {
            tooltip.push("Claude: (unavailable)".into());
        }

        // Grok: omit completely when inactive / not refreshed (auto-detect).
        if let Some(g) = &snap.grok {
            tooltip.extend(g.tooltip_lines(cfg.display.reset_style));
            if on_bar("grok") {
                let pct = g.worst_percent().unwrap_or(0);
                parts.push(format!("G {}%", remaining_percent(pct)));
                worst = worst.max(pct);
                state = state.combine(g.state(cfg));
            }
        }

        // Cursor: omit completely when inactive / not refreshed (auto-detect).
        if let Some(c) = &snap.cursor {
            tooltip.extend(c.tooltip_lines(cfg.display.reset_style));
            push_status_line(&mut tooltip, &snap.provider_status, "cursor");
            if on_bar("cursor") {
                let pct = c.worst_percent().unwrap_or(0);
                parts.push(format!("Cu {}%", remaining_percent(pct)));
                worst = worst.max(pct);
                state = state.combine(c.state(cfg));
            }
        }

        // OpenCode: omit when inactive; balance dollars (no percent window).
        if let Some(o) = &snap.opencode {
            tooltip.extend(o.tooltip_lines(cfg.display.reset_style));
            if on_bar("opencode") {
                if let Some(seg) = o.waybar_segment() {
                    parts.push(seg);
                }
                // Does not contribute to percent-based warn/crit.
                state = state.combine(o.state(cfg));
            }
        }

        if cfg.display.show_cost
            && let Some(cost) = &snap.cost
        {
            tooltip.push(format!("30d cost: ${:.2}", cost.total_usd));
        }

        if snap.is_stale(std::time::Duration::from_secs(
            cfg.refresh.usage_interval() * 3,
        )) {
            state = state.combine(State::Stale);
        }

        if provider_status::any_incident(&snap.provider_status) {
            state = state.combine(State::Incident);
        }

        let text = if cfg.display.merge_text {
            parts.join(" / ")
        } else {
            parts.join(" ")
        };

        Self {
            text,
            alt: state.label().into(),
            tooltip: tooltip.join("\n"),
            class: state.label().into(),
            percentage: worst,
        }
    }
}

fn push_status_line(
    tooltip: &mut Vec<String>,
    statuses: &[provider_status::ProviderStatus],
    id: &str,
) {
    if let Some(line) = provider_status::find(statuses, id).and_then(|s| s.display_line()) {
        tooltip.push(line);
    }
}

fn remaining_percent(used: u8) -> u8 {
    100_u8.saturating_sub(used.min(100))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ok,
    Warn,
    Crit,
    Stale,
    /// Public statuspage incident (indicator != none). Between Stale and Warn.
    Incident,
    Auth,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Ok => "ok",
            State::Warn => "warn",
            State::Crit => "crit",
            State::Stale => "stale",
            State::Incident => "incident",
            State::Auth => "auth",
        }
    }

    pub fn from_pct(pct: u8, cfg: &Config) -> Self {
        if pct >= cfg.display.crit_threshold {
            State::Crit
        } else if pct >= cfg.display.warn_threshold {
            State::Warn
        } else {
            State::Ok
        }
    }

    pub fn combine(self, other: State) -> State {
        // Severity: Crit > Warn > Auth > Incident > Stale > Ok
        fn rank(s: State) -> u8 {
            match s {
                State::Ok => 0,
                State::Stale => 1,
                State::Incident => 2,
                State::Auth => 3,
                State::Warn => 4,
                State::Crit => 5,
            }
        }
        if rank(other) > rank(self) {
            other
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_status::{Indicator, ProviderStatus};
    use chrono::Utc;

    #[test]
    fn severity_ranking_includes_incident() {
        assert_eq!(State::Ok.combine(State::Stale), State::Stale);
        assert_eq!(State::Stale.combine(State::Incident), State::Incident);
        assert_eq!(State::Incident.combine(State::Auth), State::Auth);
        assert_eq!(State::Incident.combine(State::Warn), State::Warn);
        assert_eq!(State::Incident.combine(State::Crit), State::Crit);
        // Incident loses to anything more severe; wins over Stale/Ok.
        assert_eq!(State::Warn.combine(State::Incident), State::Warn);
        assert_eq!(State::Ok.combine(State::Incident), State::Incident);
        assert_eq!(State::Incident.label(), "incident");
    }

    #[test]
    fn waybar_class_incident_when_only_incident() {
        let cfg = Config::default();
        let mut snap = Snapshot::new();
        snap.refreshed_at = Utc::now();
        // Minimal codex so we don't land on Auth from missing providers.
        snap.codex = Some(crate::providers::codex::CodexSnapshot {
            account_email: None,
            plan_type: None,
            primary: Some(crate::providers::codex::Window {
                used_percent: 10.0,
                window_minutes: Some(300),
                resets_at: None,
            }),
            secondary: None,
            credits: None,
            error: None,
        });
        snap.claude = Some(crate::providers::claude::ClaudeSnapshot {
            account_email: None,
            plan_type: None,
            session: Some(crate::providers::claude::Window {
                used_percent: 5.0,
                resets_at: None,
            }),
            weekly: None,
            sonnet_weekly: None,
            extra: None,
            source: None,
            error: None,
        });
        snap.provider_status = vec![ProviderStatus {
            provider: "codex".into(),
            indicator: Indicator::Minor,
            description: "Partial outage".into(),
        }];

        let line = WaybarLine::from_snapshot(&snap, &cfg);
        assert_eq!(line.class, "incident");
        assert_eq!(line.alt, "incident");
        assert!(
            line.tooltip.contains("⚠ minor incident: Partial outage"),
            "tooltip={}",
            line.tooltip
        );
    }

    #[test]
    fn waybar_warn_beats_incident() {
        let cfg = Config::default();
        let mut snap = Snapshot::new();
        snap.refreshed_at = Utc::now();
        snap.codex = Some(crate::providers::codex::CodexSnapshot {
            account_email: None,
            plan_type: None,
            primary: Some(crate::providers::codex::Window {
                used_percent: 75.0,
                window_minutes: Some(300),
                resets_at: None,
            }),
            secondary: None,
            credits: None,
            error: None,
        });
        snap.claude = Some(crate::providers::claude::ClaudeSnapshot {
            account_email: None,
            plan_type: None,
            session: Some(crate::providers::claude::Window {
                used_percent: 5.0,
                resets_at: None,
            }),
            weekly: None,
            sonnet_weekly: None,
            extra: None,
            source: None,
            error: None,
        });
        snap.provider_status = vec![ProviderStatus {
            provider: "codex".into(),
            indicator: Indicator::Major,
            description: "Major outage".into(),
        }];

        let line = WaybarLine::from_snapshot(&snap, &cfg);
        assert_eq!(line.class, "warn");
    }

    #[test]
    fn bar_providers_excludes_from_text_and_severity() {
        let mut cfg = Config::default();
        cfg.display.bar_providers = Some(vec!["claude".into()]);
        let mut snap = Snapshot::new();
        snap.refreshed_at = Utc::now();
        snap.codex = Some(crate::providers::codex::CodexSnapshot {
            account_email: None,
            plan_type: None,
            primary: Some(crate::providers::codex::Window {
                used_percent: 95.0,
                window_minutes: Some(300),
                resets_at: None,
            }),
            secondary: None,
            credits: None,
            error: None,
        });
        snap.claude = Some(crate::providers::claude::ClaudeSnapshot {
            account_email: None,
            plan_type: None,
            session: Some(crate::providers::claude::Window {
                used_percent: 10.0,
                resets_at: None,
            }),
            weekly: None,
            sonnet_weekly: None,
            extra: None,
            source: None,
            error: None,
        });

        let line = WaybarLine::from_snapshot(&snap, &cfg);
        assert!(
            line.text.contains("Cl "),
            "claude should be in text: {}",
            line.text
        );
        assert!(
            !line.text.contains("C "),
            "codex excluded from text: {}",
            line.text
        );
        assert_eq!(
            line.percentage, 10,
            "excluded codex must not drive percentage"
        );
        assert_eq!(line.class, "ok", "excluded codex crit must not drive class");
        // Tooltip still shows both providers.
        assert!(
            line.tooltip.to_lowercase().contains("codex")
                || line.tooltip.contains("primary")
                || line.tooltip.contains("C"),
            "tooltip should still mention codex windows: {}",
            line.tooltip
        );
    }
}
