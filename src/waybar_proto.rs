use serde::Serialize;

use crate::config::Config;
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

        if let Some(c) = &snap.codex {
            let pct = c.worst_percent().unwrap_or(0);
            parts.push(format!("C {}%", remaining_percent(pct)));
            worst = worst.max(pct);
            tooltip.extend(c.tooltip_lines(cfg.display.reset_style));
            state = state.combine(c.state(cfg));
        } else {
            parts.push("C —".into());
            tooltip.push("Codex: (unavailable)".into());
            state = state.combine(State::Auth);
        }

        if let Some(c) = &snap.claude {
            let pct = c.worst_percent().unwrap_or(0);
            parts.push(format!("Cl {}%", remaining_percent(pct)));
            worst = worst.max(pct);
            tooltip.extend(c.tooltip_lines(cfg.display.reset_style));
            state = state.combine(c.state(cfg));
        } else {
            parts.push("Cl —".into());
            tooltip.push("Claude: (unavailable)".into());
            state = state.combine(State::Auth);
        }

        // Grok: omit completely when inactive / not refreshed (auto-detect).
        if let Some(g) = &snap.grok {
            let pct = g.worst_percent().unwrap_or(0);
            parts.push(format!("G {}%", remaining_percent(pct)));
            worst = worst.max(pct);
            tooltip.extend(g.tooltip_lines(cfg.display.reset_style));
            state = state.combine(g.state(cfg));
        }

        if cfg.display.show_cost
            && let Some(cost) = &snap.cost
        {
            tooltip.push(format!("30d cost: ${:.2}", cost.total_usd));
        }

        if snap.is_stale(std::time::Duration::from_secs(
            cfg.refresh.interval_secs * 3,
        )) {
            state = state.combine(State::Stale);
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

fn remaining_percent(used: u8) -> u8 {
    100_u8.saturating_sub(used.min(100))
}

#[derive(Clone, Copy)]
pub enum State {
    Ok,
    Warn,
    Crit,
    Stale,
    Auth,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Ok => "ok",
            State::Warn => "warn",
            State::Crit => "crit",
            State::Stale => "stale",
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
        // Severity ordering: Crit > Warn > Auth > Stale > Ok
        fn rank(s: State) -> u8 {
            match s {
                State::Ok => 0,
                State::Stale => 1,
                State::Auth => 2,
                State::Warn => 3,
                State::Crit => 4,
            }
        }
        if rank(other) > rank(self) {
            other
        } else {
            self
        }
    }
}
