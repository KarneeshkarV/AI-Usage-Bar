use std::io::{self, IsTerminal, Write};

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::cli::status;
use crate::config::{Config, ResetStyle};
use crate::snapshot::Snapshot;
use crate::util::time::reset_label;

// Neutral chrome (readable on a dark terminal, not a theme).
const TEXT: (u8, u8, u8) = (228, 228, 228);
const SUBTEXT: (u8, u8, u8) = (148, 148, 148);
const OVERLAY: (u8, u8, u8) = (110, 110, 110);
const SURFACE: (u8, u8, u8) = (58, 58, 58);
const CRIT: (u8, u8, u8) = (232, 80, 80);

// Each provider's own harness accent.
const CODEX: (u8, u8, u8) = (255, 255, 255); // Codex / OpenAI: white
const CLAUDE: (u8, u8, u8) = (218, 119, 86); // Claude Code #DA7756
const GROK: (u8, u8, u8) = (187, 154, 247); // GrokNight magenta #BB9AF7
const CURSOR: (u8, u8, u8) = (245, 78, 0); // Cursor brand orange #F54E00
const OPENCODE: (u8, u8, u8) = (59, 130, 246); // OpenCode Go brand blue #3B82F6

const BAR_W: usize = 22;
const LABEL_W: usize = 8;

struct Theme {
    color: bool,
}

impl Theme {
    fn paint(&self, rgb: (u8, u8, u8), bold: bool, s: &str) -> String {
        if !self.color || s.is_empty() {
            return s.to_string();
        }
        let (r, g, b) = rgb;
        let mut out = format!("\x1b[38;2;{r};{g};{b}m");
        if bold {
            out.push_str("\x1b[1m");
        }
        out.push_str(s);
        out.push_str("\x1b[0m");
        out
    }
}

struct Row {
    label: &'static str,
    pct: Option<f64>,
    detail: String,
}

pub async fn run() -> Result<()> {
    let snap = status::load_snapshot().await?;
    let cfg = Config::load_or_default().unwrap_or_default();
    let text = render(&snap, cfg.display.reset_style, use_color(), term_width());
    let mut out = io::stdout().lock();
    out.write_all(text.as_bytes())?;
    if !text.ends_with('\n') {
        out.write_all(b"\n")?;
    }
    Ok(())
}

fn use_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var_os("FORCE_COLOR").is_some() {
        return true;
    }
    io::stdout().is_terminal()
}

fn term_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(72)
        .clamp(56, 88)
}

fn render(snap: &Snapshot, style: ResetStyle, color: bool, width: usize) -> String {
    let theme = Theme { color };
    let mut lines = Vec::new();

    lines.push(header(snap, &theme, width));
    lines.push(brand_rule(&theme, width));
    lines.push(String::new());

    // Usage-limit providers, always in this order.
    lines.extend(codex_block(snap, style, &theme, width));
    lines.push(String::new());
    lines.extend(claude_block(snap, style, &theme, width));
    lines.push(String::new());
    lines.extend(grok_block(snap, style, &theme, width));
    lines.push(String::new());
    lines.extend(opencode_block(snap, &theme, width));
    lines.push(String::new());
    lines.extend(cursor_block(snap, style, &theme, width));
    lines.push(String::new());

    lines.join("\n")
}

fn header(snap: &Snapshot, theme: &Theme, width: usize) -> String {
    let dots = format!(
        "{} {} {} {} {}",
        theme.paint(CODEX, true, "●"),
        theme.paint(CLAUDE, true, "●"),
        theme.paint(GROK, true, "●"),
        theme.paint(OPENCODE, true, "●"),
        theme.paint(CURSOR, true, "●"),
    );
    let title = theme.paint(TEXT, true, "limits");
    let left = format!("{dots}  {title}");
    let left_plain_len = "● ● ● ● ●  limits".chars().count();
    let age = relative_refresh(snap);
    let right = theme.paint(OVERLAY, false, &age);
    let right_len = age.chars().count();
    let gap = width.saturating_sub(left_plain_len + right_len).max(1);
    format!("{left}{}{right}", " ".repeat(gap))
}

/// Hairline rule split into the five harness colors.
fn brand_rule(theme: &Theme, width: usize) -> String {
    let colors = [CODEX, CLAUDE, GROK, OPENCODE, CURSOR];
    let n = width.max(colors.len());
    let chunk = n / colors.len();
    let rem = n - chunk * colors.len();
    let last = colors.len() - 1;
    let mut out = String::new();
    for (i, color) in colors.into_iter().enumerate() {
        let w = chunk + usize::from(i == last) * rem;
        out.push_str(&theme.paint(dim(color, 0.45), false, &"─".repeat(w)));
    }
    out
}

fn relative_refresh(snap: &Snapshot) -> String {
    let age = Utc::now()
        .signed_duration_since(snap.refreshed_at)
        .num_seconds()
        .max(0);
    match age {
        0..=10 => "updated just now".into(),
        11..=59 => format!("updated {age}s ago"),
        60..=3599 => format!("updated {}m ago", age / 60),
        _ => format!("updated {}h ago", age / 3600),
    }
}

fn section_title(name: &str, accent: (u8, u8, u8), plan: Option<&str>, theme: &Theme) -> String {
    let mut out = format!(
        "{} {}",
        theme.paint(accent, true, "▎"),
        theme.paint(accent, true, name)
    );
    if let Some(plan) = plan.filter(|p| !p.is_empty()) {
        out.push_str(&theme.paint(SURFACE, false, "  "));
        out.push_str(&theme.paint(SUBTEXT, false, plan));
    }
    out
}

fn render_rows(rows: &[Row], accent: (u8, u8, u8), theme: &Theme, width: usize) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let rail = theme.paint(dim(accent, 0.55), false, "▎");
            let label = pad_right(row.label, LABEL_W);
            let label = theme.paint(SUBTEXT, false, &label);
            match row.pct {
                Some(pct) => {
                    let (fill, track) = bar_parts(pct, BAR_W);
                    let bar = format!(
                        "{}{}",
                        theme.paint(accent, false, &fill),
                        theme.paint(dim(accent, 0.22), false, &track)
                    );
                    let pct_s = format!("{pct:>3.0}%");
                    let pct_color = if pct >= 90.0 { CRIT } else { accent };
                    let pct_c = theme.paint(pct_color, true, &pct_s);
                    // rail + space + label + space + bar + space + pct
                    let used = 2 + LABEL_W + 1 + BAR_W + 1 + pct_s.chars().count();
                    let meta_w = width.saturating_sub(used + 2);
                    let meta = fit(&row.detail, meta_w);
                    let meta_c = theme.paint(OVERLAY, false, &meta);
                    format!("{rail} {label} {bar} {pct_c}  {meta_c}")
                }
                None => {
                    let detail = theme.paint(OVERLAY, false, &row.detail);
                    format!("{rail} {label} {detail}")
                }
            }
        })
        .collect()
}

fn fallback_row(msg: &str, accent: (u8, u8, u8), theme: &Theme, width: usize) -> String {
    let rail = theme.paint(dim(accent, 0.45), false, "▎");
    let indent = 2; // rail + space
    let body = fit(msg, width.saturating_sub(indent));
    format!("{rail} {}", theme.paint(OVERLAY, false, &body))
}

fn codex_block(snap: &Snapshot, style: ResetStyle, theme: &Theme, width: usize) -> Vec<String> {
    let mut out = vec![section_title(
        "CODEX",
        CODEX,
        snap.codex.as_ref().and_then(|c| c.plan_type.as_deref()),
        theme,
    )];
    match &snap.codex {
        None => out.push(fallback_row("unavailable", CODEX, theme, width)),
        Some(c) if c.primary.is_none() && c.secondary.is_none() => {
            let msg = c.error.as_deref().unwrap_or("no data");
            out.push(fallback_row(msg, CODEX, theme, width));
        }
        Some(c) => {
            let mut rows = Vec::new();
            if let Some(w) = &c.primary {
                rows.push(window_row(
                    codex_window_label(w.window_minutes, "session"),
                    w.used_percent,
                    w.window_minutes,
                    w.resets_at,
                    style,
                ));
            }
            if let Some(w) = &c.secondary {
                rows.push(window_row(
                    codex_window_label(w.window_minutes, "weekly"),
                    w.used_percent,
                    w.window_minutes,
                    w.resets_at,
                    style,
                ));
            }
            out.extend(render_rows(&rows, CODEX, theme, width));
        }
    }
    out
}

fn claude_block(snap: &Snapshot, style: ResetStyle, theme: &Theme, width: usize) -> Vec<String> {
    let mut out = vec![section_title(
        "CLAUDE",
        CLAUDE,
        snap.claude.as_ref().and_then(|c| c.plan_type.as_deref()),
        theme,
    )];
    match &snap.claude {
        None => out.push(fallback_row("unavailable", CLAUDE, theme, width)),
        Some(c)
            if c.session.is_none()
                && c.weekly.is_none()
                && c.sonnet_weekly.is_none()
                && c.extra.is_none() =>
        {
            let msg = c.error.as_deref().unwrap_or("no data");
            out.push(fallback_row(msg, CLAUDE, theme, width));
        }
        Some(c) => {
            let mut rows = Vec::new();
            if let Some(w) = &c.session {
                rows.push(window_row(
                    "session",
                    w.used_percent,
                    Some(5 * 60),
                    w.resets_at,
                    style,
                ));
            }
            if let Some(w) = &c.weekly {
                rows.push(window_row(
                    "weekly",
                    w.used_percent,
                    Some(7 * 24 * 60),
                    w.resets_at,
                    style,
                ));
            }
            if let Some(w) = &c.sonnet_weekly {
                rows.push(window_row(
                    "sonnet",
                    w.used_percent,
                    Some(7 * 24 * 60),
                    w.resets_at,
                    style,
                ));
            }
            if let Some(e) = &c.extra {
                let pct = if e.limit_usd > 0.0 {
                    e.used_usd / e.limit_usd * 100.0
                } else {
                    0.0
                };
                rows.push(Row {
                    label: "extra",
                    pct: Some(pct),
                    detail: format!("${:.2} / ${:.2} {}", e.used_usd, e.limit_usd, e.currency),
                });
            }
            out.extend(render_rows(&rows, CLAUDE, theme, width));
        }
    }
    out
}

fn grok_block(snap: &Snapshot, style: ResetStyle, theme: &Theme, width: usize) -> Vec<String> {
    let mut out = vec![section_title(
        "GROK",
        GROK,
        snap.grok
            .as_ref()
            .and_then(|g| g.subscription_tier.as_deref()),
        theme,
    )];
    match &snap.grok {
        None => out.push(fallback_row("unavailable", GROK, theme, width)),
        Some(g) if g.primary.is_none() => {
            let msg = g.error.as_deref().unwrap_or("no data");
            out.push(fallback_row(msg, GROK, theme, width));
        }
        Some(g) => {
            let mut rows = Vec::new();
            if let Some(w) = &g.primary {
                let mut detail = window_detail(w.window_minutes, w.resets_at, style);
                if let (Some(used), Some(limit)) = (g.included_used_usd, g.monthly_limit_usd) {
                    let money = format!("${used:.2} / ${limit:.2}");
                    detail = if detail.is_empty() {
                        money
                    } else {
                        format!("{money} · {detail}")
                    };
                }
                rows.push(Row {
                    label: "monthly",
                    pct: Some(w.used_percent),
                    detail,
                });
            }
            out.extend(render_rows(&rows, GROK, theme, width));
        }
    }
    out
}

fn cursor_block(snap: &Snapshot, style: ResetStyle, theme: &Theme, width: usize) -> Vec<String> {
    let plan = snap
        .cursor
        .as_ref()
        .and_then(|c| c.membership_type.as_deref().map(format_membership));
    let mut out = vec![section_title("CURSOR", CURSOR, plan.as_deref(), theme)];
    match &snap.cursor {
        None => out.push(fallback_row("unavailable", CURSOR, theme, width)),
        Some(c) if c.primary.is_none() => {
            let msg = c.error.as_deref().unwrap_or("no data");
            out.push(fallback_row(msg, CURSOR, theme, width));
        }
        Some(c) => {
            let mut rows = Vec::new();
            if let Some(w) = &c.primary {
                let mut detail = window_detail(w.window_minutes, w.resets_at, style);
                let extra = if let (Some(used), Some(limit)) = (c.requests_used, c.requests_limit) {
                    Some(format!("{used} / {limit} req"))
                } else if let (Some(used), Some(limit)) = (c.plan_used_usd, c.plan_limit_usd) {
                    Some(format!("${used:.2} / ${limit:.2}"))
                } else {
                    None
                };
                if let Some(extra) = extra {
                    detail = if detail.is_empty() {
                        extra
                    } else {
                        format!("{extra} · {detail}")
                    };
                }
                rows.push(Row {
                    label: "plan",
                    pct: Some(w.used_percent),
                    detail,
                });
            }
            out.extend(render_rows(&rows, CURSOR, theme, width));
        }
    }
    out
}

fn opencode_block(snap: &Snapshot, theme: &Theme, width: usize) -> Vec<String> {
    let mut out = vec![section_title("OPENCODE", OPENCODE, None, theme)];
    match &snap.opencode {
        None => out.push(fallback_row("unavailable", OPENCODE, theme, width)),
        Some(o) if o.balance_usd.is_none() && o.local_30d_cost_usd.is_none() => {
            let msg = o.error.as_deref().unwrap_or("no data");
            out.push(fallback_row(msg, OPENCODE, theme, width));
        }
        Some(o) => {
            let mut rows = Vec::new();
            if let Some(b) = o.balance_usd {
                let detail = if b <= 0.0 {
                    format!("${b:.2} depleted")
                } else {
                    format!("${b:.2} remaining")
                };
                rows.push(Row {
                    label: "balance",
                    pct: None,
                    detail,
                });
            }
            if let Some(cost) = o.local_30d_cost_usd {
                rows.push(Row {
                    label: "last 30d",
                    pct: None,
                    detail: format!("${cost:.2} spent"),
                });
            }
            out.extend(render_rows(&rows, OPENCODE, theme, width));
        }
    }
    out
}

/// Codex CLI 0.149 Plus reports the weekly window as `primary` (10080 min)
/// with `secondary` null. Label from duration so a 7-day primary is not
/// shown as "session".
fn codex_window_label(window_minutes: Option<u32>, fallback: &'static str) -> &'static str {
    match window_minutes {
        Some(m) if m >= 24 * 60 => "weekly",
        Some(_) => "session",
        None => fallback,
    }
}

fn window_row(
    label: &'static str,
    pct: f64,
    window_minutes: Option<u32>,
    resets_at: Option<DateTime<Utc>>,
    style: ResetStyle,
) -> Row {
    Row {
        label,
        pct: Some(pct),
        detail: window_detail(window_minutes, resets_at, style),
    }
}

fn window_detail(
    window_minutes: Option<u32>,
    resets_at: Option<DateTime<Utc>>,
    style: ResetStyle,
) -> String {
    let mut parts = Vec::new();
    if let Some(m) = window_minutes {
        parts.push(window_span(m));
    }
    if let Some(t) = resets_at {
        parts.push(reset_label(t, style));
    }
    parts.join(" · ")
}

fn window_span(minutes: u32) -> String {
    if minutes >= 24 * 60 {
        format!("{}d", (minutes / (24 * 60)).max(1))
    } else {
        format!("{}h", (minutes / 60).max(1))
    }
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

fn dim(rgb: (u8, u8, u8), amount: f32) -> (u8, u8, u8) {
    let t = amount.clamp(0.0, 1.0);
    let (r, g, b) = rgb;
    (
        (r as f32 * t).round() as u8,
        (g as f32 * t).round() as u8,
        (b as f32 * t).round() as u8,
    )
}

/// Eighth-block fill + dim track. Both halves are `width` cells together.
fn bar_parts(pct: f64, width: usize) -> (String, String) {
    const PARTIALS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    if width == 0 {
        return (String::new(), String::new());
    }
    let ratio = (pct / 100.0).clamp(0.0, 1.0);
    let eighths = (ratio * width as f64 * 8.0).round() as usize;
    let full = (eighths / 8).min(width);
    let rem = eighths % 8;
    let mut fill = "█".repeat(full);
    let mut used = full;
    if rem > 0 && used < width {
        fill.push(PARTIALS[rem]);
        used += 1;
    }
    let track = "░".repeat(width - used);
    (fill, track)
}

fn pad_right(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(w - n))
    }
}

fn fit(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::claude::{self, ExtraSpend};
    use crate::providers::codex::{self, CodexSnapshot};
    use crate::providers::cursor::{self, CursorSnapshot};
    use crate::providers::grok::{self, GrokSnapshot};
    use crate::providers::opencode::OpenCodeSnapshot;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    fn sample_snap() -> Snapshot {
        let mut snap = Snapshot::new();
        snap.refreshed_at = Utc::now();
        snap.codex = Some(CodexSnapshot {
            account_email: Some("a@b.co".into()),
            plan_type: Some("plus".into()),
            primary: Some(codex::Window {
                used_percent: 42.0,
                window_minutes: Some(5 * 60),
                resets_at: Some(Utc::now() + chrono::Duration::hours(3)),
            }),
            secondary: Some(codex::Window {
                used_percent: 14.0,
                window_minutes: Some(7 * 24 * 60),
                resets_at: Some(Utc::now() + chrono::Duration::days(4)),
            }),
            credits: None,
            reset_credits: None,
            error: None,
        });
        snap.claude = Some(claude::ClaudeSnapshot {
            account_email: None,
            plan_type: Some("max".into()),
            session: Some(claude::Window {
                used_percent: 31.0,
                resets_at: Some(Utc::now() + chrono::Duration::hours(4)),
            }),
            weekly: Some(claude::Window {
                used_percent: 18.0,
                resets_at: Some(Utc::now() + chrono::Duration::days(5)),
            }),
            sonnet_weekly: Some(claude::Window {
                used_percent: 15.0,
                resets_at: Some(Utc::now() + chrono::Duration::days(5)),
            }),
            extra: Some(ExtraSpend {
                used_usd: 0.0,
                limit_usd: 10.0,
                currency: "USD".into(),
            }),
            source: Some("oauth_usage".into()),
            error: None,
        });
        snap.grok = Some(GrokSnapshot {
            primary: Some(grok::Window {
                used_percent: 40.0,
                window_minutes: Some(7 * 24 * 60),
                resets_at: Some(utc(2026, 8, 20, 0)),
            }),
            included_used_usd: Some(4.20),
            on_demand_used_usd: None,
            monthly_limit_usd: Some(30.0),
            on_demand_enabled: false,
            subscription_tier: Some("SuperGrok".into()),
            error: None,
        });
        snap.cursor = Some(CursorSnapshot {
            primary: Some(cursor::Window {
                used_percent: 40.0,
                window_minutes: Some(31 * 24 * 60),
                resets_at: Some(utc(2026, 9, 1, 0)),
            }),
            plan_used_usd: Some(8.0),
            plan_limit_usd: Some(20.0),
            on_demand_used_usd: None,
            on_demand_limit_usd: None,
            membership_type: Some("pro".into()),
            account_email: None,
            requests_used: None,
            requests_limit: None,
            error: None,
        });
        snap.opencode = Some(OpenCodeSnapshot {
            balance_usd: Some(12.4),
            local_30d_cost_usd: Some(10.20),
            error: None,
        });
        snap
    }

    #[test]
    fn bar_empty_half_full() {
        let (fill, track) = bar_parts(0.0, 10);
        assert_eq!(fill.chars().count() + track.chars().count(), 10);
        assert!(fill.is_empty());
        assert_eq!(track.chars().count(), 10);

        let (fill, track) = bar_parts(50.0, 10);
        assert_eq!(fill.chars().count() + track.chars().count(), 10);
        assert_eq!(fill.chars().count(), 5);

        let (fill, track) = bar_parts(100.0, 10);
        assert_eq!(fill, "█".repeat(10));
        assert!(track.is_empty());
    }

    #[test]
    fn render_lists_all_five_providers() {
        let text = render(&sample_snap(), ResetStyle::Countdown, false, 76);
        for name in ["CODEX", "CLAUDE", "GROK", "OPENCODE", "CURSOR"] {
            assert!(text.contains(name), "missing {name} in:\n{text}");
        }
        let opencode_pos = text.find("OPENCODE").expect("OPENCODE present");
        let cursor_pos = text.find("CURSOR").expect("CURSOR present");
        assert!(
            opencode_pos < cursor_pos,
            "OpenCode must render before Cursor"
        );
        assert!(text.contains("plus"));
        assert!(text.contains("max"));
        assert!(text.contains("SuperGrok"));
        assert!(text.contains("Pro"));
        assert!(text.contains("42%"));
        assert!(text.contains("31%"));
        assert!(text.contains("$4.20 / $30.00"));
        assert!(text.contains("$8.00 / $20.00"));
        assert!(text.contains("$12.40 remaining"));
        assert!(text.contains("$10.20 spent"));
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn render_marks_missing_providers() {
        let snap = Snapshot::new();
        let text = render(&snap, ResetStyle::Countdown, false, 72);
        assert_eq!(text.matches("unavailable").count(), 5);
        assert!(text.contains("CODEX"));
        assert!(text.contains("CLAUDE"));
        assert!(text.contains("GROK"));
        assert!(text.contains("CURSOR"));
        assert!(text.contains("OPENCODE"));
        assert!(text.contains("limits"));
        assert!(text.contains("──"));
    }

    #[test]
    fn long_errors_are_truncated() {
        let mut snap = Snapshot::new();
        snap.claude = Some(claude::ClaudeSnapshot {
            account_email: None,
            plan_type: None,
            session: None,
            weekly: None,
            sonnet_weekly: None,
            extra: None,
            source: None,
            error: Some("oauth_usage: no credentials; cookies: no Claude sessionKey found in any browser; pty: could not parse /usage panel".into()),
        });
        let text = render(&snap, ResetStyle::Countdown, false, 56);
        let claude_line = text
            .lines()
            .find(|l| l.contains("oauth_usage") || l.contains('…'))
            .expect("error line");
        assert!(claude_line.chars().count() <= 56);
        assert!(claude_line.contains('…'));
    }

    #[test]
    fn color_output_uses_ansi() {
        let text = render(&sample_snap(), ResetStyle::Countdown, true, 76);
        assert!(text.contains("\x1b[38;2;"));
        assert!(text.contains("CODEX"));
    }

    #[test]
    fn harness_colors_are_used() {
        let text = render(&sample_snap(), ResetStyle::Countdown, true, 76);
        // Codex white, Claude orange, Grok magenta, Cursor orange, OpenCode blue.
        assert!(text.contains("\x1b[38;2;255;255;255m"), "{text}");
        assert!(text.contains("\x1b[38;2;218;119;86m"), "{text}");
        assert!(text.contains("\x1b[38;2;187;154;247m"), "{text}");
        assert!(text.contains("\x1b[38;2;245;78;0m"), "{text}");
        assert!(text.contains("\x1b[38;2;59;130;246m"), "{text}");
    }

    #[test]
    fn opencode_depleted_balance_is_labeled() {
        let mut snap = Snapshot::new();
        snap.opencode = Some(OpenCodeSnapshot {
            balance_usd: Some(0.0),
            local_30d_cost_usd: None,
            error: None,
        });
        let text = render(&snap, ResetStyle::Countdown, false, 72);
        assert!(text.contains("OPENCODE"), "{text}");
        assert!(text.contains("$0.00 depleted"), "{text}");
    }

    #[test]
    fn codex_seven_day_primary_is_labeled_weekly() {
        let mut snap = Snapshot::new();
        snap.codex = Some(CodexSnapshot {
            account_email: None,
            plan_type: Some("plus".into()),
            primary: Some(codex::Window {
                used_percent: 54.0,
                window_minutes: Some(10080),
                resets_at: None,
            }),
            secondary: None,
            credits: None,
            reset_credits: None,
            error: None,
        });
        let text = render(&snap, ResetStyle::Countdown, false, 76);
        assert!(text.contains("weekly"), "{text}");
        assert!(!text.contains("session"), "{text}");
        assert!(text.contains("54%"), "{text}");
    }
}
