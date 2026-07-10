use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph};

use crate::config::{Config, ResetStyle};
use crate::snapshot::{self, Snapshot};
use crate::util::time::reset_label;

// Catppuccin Mocha
const BASE: Color = Color::Rgb(24, 24, 37);
const SURFACE0: Color = Color::Rgb(49, 50, 68);
const OVERLAY0: Color = Color::Rgb(108, 112, 134);
const TEXT: Color = Color::Rgb(205, 214, 244);
const SUBTEXT0: Color = Color::Rgb(147, 153, 178);
const BLUE: Color = Color::Rgb(137, 180, 250);
const MAUVE: Color = Color::Rgb(203, 166, 247);
const GREEN: Color = Color::Rgb(166, 227, 161);
const YELLOW: Color = Color::Rgb(249, 226, 175);
const RED: Color = Color::Rgb(243, 139, 168);

struct App {
    snapshot: Option<Snapshot>,
    read_error: Option<String>,
    poll_interval: Duration,
    next_poll: Instant,
    reset_style: ResetStyle,
}

impl App {
    fn new(poll_secs: u64) -> Self {
        let interval = Duration::from_secs(poll_secs.clamp(1, 30));
        let reset_style = Config::load_or_default()
            .map(|c| c.display.reset_style)
            .unwrap_or_default();
        let mut app = Self {
            snapshot: None,
            read_error: None,
            poll_interval: interval,
            next_poll: Instant::now(),
            reset_style,
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        match snapshot::read() {
            Ok(snap) => {
                self.snapshot = Some(snap);
                self.read_error = None;
            }
            Err(e) => {
                self.read_error = Some(e.to_string());
            }
        }
        self.next_poll = Instant::now() + self.poll_interval;
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

pub async fn run(poll_secs: u64) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new(poll_secs);

    loop {
        terminal.draw(|frame| draw(frame, &app))?;

        let timeout = app
            .next_poll
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(250));

        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('r') => app.refresh(),
                _ => {}
            }
        }

        if Instant::now() >= app.next_poll {
            app.refresh();
        }
    }

    Ok(())
}

fn draw(frame: &mut ratatui::Frame<'_>, app: &App) {
    let area = centered(frame.area(), 94, 40);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_set(symbols::border::ROUNDED)
        .border_style(Style::default().fg(MAUVE))
        .style(Style::default().bg(BASE).fg(TEXT))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled("◆", Style::default().fg(MAUVE)),
            Span::styled(
                " ai bar ",
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]))
        .title_alignment(Alignment::Center);
    frame.render_widget(outer, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status
            Constraint::Length(1), // spacer
            Constraint::Min(18),   // providers
            Constraint::Length(1), // rule
            Constraint::Length(3), // cost
            Constraint::Length(1), // spacer
            Constraint::Length(1), // footer
        ])
        .split(inner);

    render_status(frame, chunks[0], app);
    render_providers(frame, chunks[2], app);
    render_rule(frame, chunks[3]);
    render_cost(frame, chunks[4], app);
    render_footer(frame, chunks[6]);
}

fn render_status(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let refreshed = app
        .snapshot
        .as_ref()
        .map(relative_refresh)
        .unwrap_or_else(|| "no snapshot".into());

    let codex_pct = app
        .snapshot
        .as_ref()
        .and_then(|s| s.codex.as_ref())
        .and_then(|c| c.worst_percent())
        .map(|p| format!("{p}%"))
        .unwrap_or_else(|| "—".into());

    let claude_pct = app
        .snapshot
        .as_ref()
        .and_then(|s| s.claude.as_ref())
        .and_then(|c| c.worst_percent())
        .map(|p| format!("{p}%"))
        .unwrap_or_else(|| "—".into());

    let grok_pct = app
        .snapshot
        .as_ref()
        .and_then(|s| s.grok.as_ref())
        .and_then(|c| c.worst_percent())
        .map(|p| format!("{p}%"))
        .unwrap_or_else(|| "—".into());

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    frame.render_widget(
        Paragraph::new(Span::styled(refreshed, Style::default().fg(OVERLAY0))),
        cols[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("codex ", Style::default().fg(OVERLAY0)),
            Span::styled(
                codex_pct,
                Style::default().fg(MAUVE).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  claude ", Style::default().fg(OVERLAY0)),
            Span::styled(
                claude_pct,
                Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  grok ", Style::default().fg(OVERLAY0)),
            Span::styled(
                grok_pct,
                Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
            ),
        ]))
        .alignment(Alignment::Right),
        cols[1],
    );
}

#[derive(Clone, Copy)]
enum ProviderKind {
    Codex,
    Claude,
    Grok,
}

fn render_providers(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    // Three columns with 1-cell gaps
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Length(1),
            Constraint::Percentage(33),
            Constraint::Length(1),
            Constraint::Percentage(34),
        ])
        .split(area);

    render_provider_panel(frame, cols[0], app, ProviderKind::Codex);
    render_provider_panel(frame, cols[2], app, ProviderKind::Claude);
    render_provider_panel(frame, cols[4], app, ProviderKind::Grok);
}

fn render_provider_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    app: &App,
    kind: ProviderKind,
) {
    let (title, accent) = match kind {
        ProviderKind::Codex => ("CODEX", MAUVE),
        ProviderKind::Claude => ("CLAUDE", BLUE),
        ProviderKind::Grok => ("GROK", GREEN),
    };

    let plan_suffix = match kind {
        ProviderKind::Claude => app
            .snapshot
            .as_ref()
            .and_then(|s| s.claude.as_ref())
            .and_then(|c| c.plan_type.as_ref())
            .map(|p| format!(" · {p}"))
            .unwrap_or_default(),
        ProviderKind::Codex => app
            .snapshot
            .as_ref()
            .and_then(|s| s.codex.as_ref())
            .and_then(|c| c.plan_type.as_ref())
            .map(|p| format!(" · {p}"))
            .unwrap_or_default(),
        ProviderKind::Grok => String::new(),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(symbols::border::ROUNDED)
        .border_style(Style::default().fg(SURFACE0))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                title,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(plan_suffix, Style::default().fg(OVERLAY0)),
            Span::raw(" "),
        ]))
        .style(Style::default().bg(BASE));

    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });

    let rows = match kind {
        ProviderKind::Claude => claude_rows(app),
        ProviderKind::Codex => codex_rows(app),
        ProviderKind::Grok => grok_rows(app),
    };

    if rows.is_empty() {
        let fallback = match kind {
            ProviderKind::Claude => app
                .snapshot
                .as_ref()
                .and_then(|s| s.claude.as_ref())
                .and_then(|c| c.error.as_deref())
                .unwrap_or("no data"),
            ProviderKind::Codex => app
                .snapshot
                .as_ref()
                .and_then(|s| s.codex.as_ref())
                .and_then(|c| c.error.as_deref())
                .or(app.read_error.as_deref())
                .unwrap_or("waiting for snapshot"),
            ProviderKind::Grok => app
                .snapshot
                .as_ref()
                .and_then(|s| s.grok.as_ref())
                .and_then(|c| c.error.as_deref())
                .unwrap_or("no data"),
        };
        frame.render_widget(
            Paragraph::new(Span::styled(fallback, Style::default().fg(OVERLAY0)))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let row_constraints = rows
        .iter()
        .map(|_| Constraint::Length(4))
        .chain([Constraint::Min(0)])
        .collect::<Vec<_>>();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(inner);

    for (idx, row) in rows.iter().enumerate() {
        render_usage_row(frame, chunks[idx], row);
    }
}

struct UsageRow {
    label: String,
    pct: f64,
    detail: String,
    reset: Option<String>,
}

fn render_usage_row(frame: &mut ratatui::Frame<'_>, area: Rect, row: &UsageRow) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    // Label row: name left, percentage right
    let label_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(6)])
        .split(chunks[0]);

    frame.render_widget(
        Paragraph::new(Span::styled(
            row.label.as_str(),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        )),
        label_cols[0],
    );
    frame.render_widget(
        Paragraph::new(format!("{:.0}%", row.pct))
            .alignment(Alignment::Right)
            .style(
                Style::default()
                    .fg(color_for_pct(row.pct))
                    .add_modifier(Modifier::BOLD),
            ),
        label_cols[1],
    );

    // Gauge bar
    let ratio = (row.pct / 100.0).clamp(0.0, 1.0);
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(color_for_pct(row.pct)).bg(SURFACE0))
        .ratio(ratio)
        .label("");
    frame.render_widget(gauge, chunks[1]);

    // Detail row: info left, reset right
    let detail_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[2]);

    frame.render_widget(
        Paragraph::new(Span::styled(
            row.detail.as_str(),
            Style::default().fg(SUBTEXT0),
        )),
        detail_cols[0],
    );
    if let Some(reset) = &row.reset {
        frame.render_widget(
            Paragraph::new(Span::styled(reset.as_str(), Style::default().fg(OVERLAY0)))
                .alignment(Alignment::Right),
            detail_cols[1],
        );
    }
}

fn codex_rows(app: &App) -> Vec<UsageRow> {
    let Some(snap) = &app.snapshot else {
        return Vec::new();
    };
    let Some(codex) = &snap.codex else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    if let Some(w) = &codex.primary {
        rows.push(UsageRow {
            label: "Primary".into(),
            pct: w.used_percent,
            detail: w
                .window_minutes
                .map(|m| format!("{}h window", (m / 60).max(1)))
                .unwrap_or_else(|| "session window".into()),
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
        });
    }
    if let Some(w) = &codex.secondary {
        rows.push(UsageRow {
            label: "Secondary".into(),
            pct: w.used_percent,
            detail: w
                .window_minutes
                .map(|m| format!("{}h window", (m / 60).max(1)))
                .unwrap_or_else(|| "secondary window".into()),
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
        });
    }
    if let Some(c) = &codex.credits {
        rows.push(UsageRow {
            label: "Credits".into(),
            pct: if c.unlimited { 0.0 } else { 100.0 },
            detail: c
                .balance
                .clone()
                .unwrap_or_else(|| "no balance reported".into()),
            reset: if c.unlimited {
                Some("unlimited".into())
            } else {
                None
            },
        });
    }
    if rows.is_empty()
        && let Some(e) = &codex.error
    {
        rows.push(error_row(e));
    }
    rows
}

fn claude_rows(app: &App) -> Vec<UsageRow> {
    let Some(snap) = &app.snapshot else {
        return Vec::new();
    };
    let Some(claude) = &snap.claude else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    if let Some(w) = &claude.session {
        rows.push(UsageRow {
            label: "Session".into(),
            pct: w.used_percent,
            detail: claude
                .source
                .clone()
                .unwrap_or_else(|| "current session".into()),
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
        });
    }
    if let Some(w) = &claude.weekly {
        rows.push(UsageRow {
            label: "Weekly".into(),
            pct: w.used_percent,
            detail: "weekly window".into(),
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
        });
    }
    if let Some(w) = &claude.sonnet_weekly {
        rows.push(UsageRow {
            label: "Sonnet".into(),
            pct: w.used_percent,
            detail: "sonnet weekly".into(),
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
        });
    }
    if let Some(extra) = &claude.extra {
        let pct = if extra.limit_usd > 0.0 {
            extra.used_usd / extra.limit_usd * 100.0
        } else {
            0.0
        };
        rows.push(UsageRow {
            label: "Extra".into(),
            pct,
            detail: format!(
                "${:.2} / ${:.2} {}",
                extra.used_usd, extra.limit_usd, extra.currency
            ),
            reset: Some(format!("{pct:.0}% used")),
        });
    }
    if rows.is_empty()
        && let Some(e) = &claude.error
    {
        rows.push(error_row(e));
    }
    rows
}

fn grok_rows(app: &App) -> Vec<UsageRow> {
    let Some(snap) = &app.snapshot else {
        return Vec::new();
    };
    let Some(grok) = &snap.grok else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    if let Some(w) = &grok.primary {
        let detail = match (grok.included_used_usd, grok.monthly_limit_usd) {
            (Some(used), Some(limit)) => format!("${used:.2} / ${limit:.2}"),
            _ => w
                .window_minutes
                .map(|m| {
                    let days = (m / (24 * 60)).max(1);
                    format!("{days}d window")
                })
                .unwrap_or_else(|| "monthly".into()),
        };
        rows.push(UsageRow {
            label: "Monthly".into(),
            pct: w.used_percent,
            detail,
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
        });
    }
    if let Some(used) = grok.on_demand_used_usd
        && (grok.on_demand_enabled || used > 0.0)
    {
        rows.push(UsageRow {
            label: "On-demand".into(),
            pct: 0.0,
            detail: format!("${used:.2}"),
            reset: None,
        });
    }
    if rows.is_empty()
        && let Some(e) = &grok.error
    {
        rows.push(error_row(e));
    }
    rows
}

fn error_row(error: &str) -> UsageRow {
    UsageRow {
        label: "Unavailable".into(),
        pct: 0.0,
        detail: error.chars().take(60).collect(),
        reset: None,
    }
}

fn render_rule(frame: &mut ratatui::Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new("─".repeat(area.width as usize)).style(Style::default().fg(SURFACE0)),
        area,
    );
}

fn render_cost(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::new();

    if let Some(cost) = app.snapshot.as_ref().and_then(|s| s.cost.as_ref()) {
        let today = chrono::Utc::now().date_naive().to_string();
        let today_cost = cost.by_day.get(&today).copied().unwrap_or(0.0);

        lines.push(Line::from(vec![
            Span::styled("today  ", Style::default().fg(SUBTEXT0)),
            Span::styled(
                format!("${today_cost:.2}"),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("   ·   30 days  ", Style::default().fg(SUBTEXT0)),
            Span::styled(
                format!("${:.2}", cost.total_usd),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("   ·   {} models", cost.by_model.len()),
                Style::default().fg(OVERLAY0),
            ),
        ]));

        if !cost.by_provider.is_empty() {
            let mut spans: Vec<Span> = Vec::new();
            for (i, (provider, usd)) in cost.by_provider.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled("   ", Style::default()));
                }
                spans.push(Span::styled(
                    format!("{provider} "),
                    Style::default().fg(SUBTEXT0),
                ));
                spans.push(Span::styled(
                    format!("${usd:.2}"),
                    Style::default().fg(TEXT),
                ));
            }
            lines.push(Line::from(spans));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "no cost data cached yet",
            Style::default().fg(OVERLAY0),
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("r", Style::default().fg(MAUVE).add_modifier(Modifier::BOLD)),
            Span::styled(" refresh   ", Style::default().fg(OVERLAY0)),
            Span::styled("q", Style::default().fg(MAUVE).add_modifier(Modifier::BOLD)),
            Span::styled(" quit", Style::default().fg(OVERLAY0)),
        ]))
        .alignment(Alignment::Center),
        area,
    );
}

fn relative_refresh(snap: &Snapshot) -> String {
    let age = chrono::Utc::now()
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

fn color_for_pct(pct: f64) -> Color {
    if pct >= 90.0 {
        RED
    } else if pct >= 70.0 {
        YELLOW
    } else {
        GREEN
    }
}

fn centered(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = area.width.min(max_width).max(80);
    let height = area.height.min(max_height).max(26);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(height)) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(width)) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical[1]);
    horizontal[1]
}
