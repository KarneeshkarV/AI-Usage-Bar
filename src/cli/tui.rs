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
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Padding, Paragraph, Tabs};

use crate::snapshot::{self, Snapshot};
use crate::util::time::until;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderTab {
    Codex,
    Claude,
}

impl ProviderTab {
    fn next(self) -> Self {
        match self {
            Self::Codex => Self::Claude,
            Self::Claude => Self::Codex,
        }
    }

    fn prev(self) -> Self {
        self.next()
    }

    fn index(self) -> usize {
        match self {
            Self::Codex => 0,
            Self::Claude => 1,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
        }
    }
}

struct App {
    selected: ProviderTab,
    snapshot: Option<Snapshot>,
    read_error: Option<String>,
    poll_interval: Duration,
    next_poll: Instant,
}

impl App {
    fn new(poll_secs: u64) -> Self {
        let interval = Duration::from_secs(poll_secs.clamp(1, 30));
        let mut app = Self {
            selected: ProviderTab::Claude,
            snapshot: None,
            read_error: None,
            poll_interval: interval,
            next_poll: Instant::now(),
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

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('r') => app.refresh(),
                    KeyCode::Left | KeyCode::Char('h') => app.selected = app.selected.prev(),
                    KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                        app.selected = app.selected.next()
                    }
                    KeyCode::Char('1') => app.selected = ProviderTab::Codex,
                    KeyCode::Char('2') => app.selected = ProviderTab::Claude,
                    _ => {}
                }
            }
        }

        if Instant::now() >= app.next_poll {
            app.refresh();
        }
    }

    Ok(())
}

fn draw(frame: &mut ratatui::Frame<'_>, app: &App) {
    let area = centered(frame.area(), 86, 34);
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_set(symbols::border::ROUNDED)
        .border_style(Style::default().fg(Color::Rgb(150, 154, 190)))
        .style(
            Style::default()
                .bg(Color::Rgb(225, 222, 255))
                .fg(Color::Rgb(34, 31, 44)),
        )
        .padding(Padding::new(2, 2, 1, 1));
    frame.render_widget(outer, area);

    let inner = area.inner(Margin {
        horizontal: 3,
        vertical: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Min(12),
            Constraint::Length(1),
            Constraint::Length(6),
            Constraint::Length(5),
        ])
        .split(inner);

    render_tabs(frame, chunks[0], app);
    render_header(frame, chunks[1], app);
    render_provider(frame, chunks[2], app);
    render_rule(frame, chunks[3]);
    render_cost(frame, chunks[4], app);
    render_actions(frame, chunks[5]);
}

fn render_tabs(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let titles = ["Codex", "Claude"]
        .into_iter()
        .map(|title| Line::from(vec![Span::raw(format!("  {title}  "))]))
        .collect::<Vec<_>>();
    let tabs = Tabs::new(titles)
        .select(app.selected.index())
        .highlight_style(
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(54, 121, 238))
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().fg(Color::Rgb(104, 99, 126)))
        .divider(Span::raw(" "));
    frame.render_widget(tabs, area);

    let hint = Paragraph::new("1/2 or arrows select  r refresh  q quit")
        .style(Style::default().fg(Color::Rgb(104, 99, 126)))
        .alignment(Alignment::Right);
    frame.render_widget(hint, area);
}

fn render_header(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let refreshed = app
        .snapshot
        .as_ref()
        .map(|s| relative_refresh(s))
        .unwrap_or_else(|| "No cached snapshot".into());
    let right = app
        .snapshot
        .as_ref()
        .and_then(|s| worst_percent(s, app.selected))
        .map(|pct| format!("{pct}% max"))
        .unwrap_or_else(|| "No data".into());

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Length(1)])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);

    frame.render_widget(
        Paragraph::new(app.selected.title()).style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Rgb(32, 29, 39)),
        ),
        cols[0],
    );
    frame.render_widget(
        Paragraph::new(right)
            .alignment(Alignment::Right)
            .style(Style::default().fg(Color::Rgb(104, 99, 126))),
        cols[1],
    );
    frame.render_widget(
        Paragraph::new(refreshed).style(Style::default().fg(Color::Rgb(104, 99, 126))),
        rows[1],
    );
}

fn render_provider(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let rows = provider_rows(app);
    let constraints = rows
        .iter()
        .map(|_| Constraint::Length(4))
        .chain(std::iter::once(Constraint::Min(0)))
        .collect::<Vec<_>>();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    for (idx, row) in rows.iter().enumerate() {
        render_usage_row(frame, chunks[idx], row);
    }

    if rows.is_empty() {
        let msg = app
            .read_error
            .as_deref()
            .unwrap_or("Waiting for ai_bar waybar to write a snapshot.");
        frame.render_widget(
            Paragraph::new(msg)
                .style(Style::default().fg(Color::Rgb(104, 99, 126)))
                .alignment(Alignment::Center),
            area,
        );
    }
}

fn render_rule(frame: &mut ratatui::Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new("─".repeat(area.width as usize))
            .style(Style::default().fg(Color::Rgb(188, 185, 220))),
        area,
    );
}

fn render_cost(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let block = Block::default()
        .title("Cost")
        .title_style(Style::default().add_modifier(Modifier::BOLD))
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::Rgb(188, 185, 220)));

    let mut lines = Vec::new();
    if let Some(cost) = app.snapshot.as_ref().and_then(|s| s.cost.as_ref()) {
        let today = chrono::Utc::now().date_naive().to_string();
        let today_cost = cost.by_day.get(&today).copied().unwrap_or(0.0);
        lines.push(Line::from(format!("Today: ${today_cost:.2}")));
        lines.push(Line::from(format!(
            "Last 30 days: ${:.2}  ·  {} models",
            cost.total_usd,
            cost.by_model.len()
        )));
        if !cost.by_provider.is_empty() {
            let providers = cost
                .by_provider
                .iter()
                .map(|(provider, usd)| format!("{provider} ${usd:.2}"))
                .collect::<Vec<_>>()
                .join("  ");
            lines.push(Line::from(providers));
        }
    } else {
        lines.push(Line::from("No local cost scan cached yet."));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .style(Style::default().fg(Color::Rgb(34, 31, 44))),
        area,
    );
}

fn render_actions(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let items = [
        ListItem::new("Add Account..."),
        ListItem::new("Usage Dashboard"),
        ListItem::new("Status Page"),
        ListItem::new("Settings..."),
        ListItem::new("About ai_bar"),
    ];
    let list = List::new(items).style(Style::default().fg(Color::Rgb(34, 31, 44)));
    frame.render_widget(list, area);
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

    frame.render_widget(
        Paragraph::new(row.label.as_str()).style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Rgb(32, 29, 39)),
        ),
        chunks[0],
    );

    let ratio = (row.pct / 100.0).clamp(0.0, 1.0);
    let gauge = Gauge::default()
        .gauge_style(
            Style::default()
                .fg(color_for_pct(row.pct))
                .bg(Color::Rgb(204, 202, 238)),
        )
        .ratio(ratio)
        .label("");
    frame.render_widget(gauge, chunks[1]);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(chunks[2]);
    frame.render_widget(
        Paragraph::new(format!("{:.0}% used", row.pct))
            .style(Style::default().fg(Color::Rgb(34, 31, 44))),
        cols[0],
    );
    frame.render_widget(
        Paragraph::new(row.reset.as_deref().unwrap_or(""))
            .alignment(Alignment::Right)
            .style(Style::default().fg(Color::Rgb(104, 99, 126))),
        cols[1],
    );
    frame.render_widget(
        Paragraph::new(row.detail.as_str()).style(Style::default().fg(Color::Rgb(104, 99, 126))),
        chunks[3],
    );
}

fn provider_rows(app: &App) -> Vec<UsageRow> {
    let Some(snap) = &app.snapshot else {
        return Vec::new();
    };

    match app.selected {
        ProviderTab::Codex => {
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
                        .unwrap_or_else(|| "Session window".into()),
                    reset: w.resets_at.map(|t| format!("Resets in {}", until(t))),
                });
            }
            if let Some(w) = &codex.secondary {
                rows.push(UsageRow {
                    label: "Secondary".into(),
                    pct: w.used_percent,
                    detail: w
                        .window_minutes
                        .map(|m| format!("{}h window", (m / 60).max(1)))
                        .unwrap_or_else(|| "Secondary window".into()),
                    reset: w.resets_at.map(|t| format!("Resets in {}", until(t))),
                });
            }
            if let Some(c) = &codex.credits {
                rows.push(UsageRow {
                    label: "Credits".into(),
                    pct: if c.unlimited { 0.0 } else { 100.0 },
                    detail: c
                        .balance
                        .clone()
                        .unwrap_or_else(|| "No balance reported".into()),
                    reset: if c.unlimited {
                        Some("Unlimited".into())
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
        ProviderTab::Claude => {
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
                        .unwrap_or_else(|| "Current session".into()),
                    reset: w.resets_at.map(|t| format!("Resets in {}", until(t))),
                });
            }
            if let Some(w) = &claude.weekly {
                rows.push(UsageRow {
                    label: "Weekly".into(),
                    pct: w.used_percent,
                    detail: "Weekly usage window".into(),
                    reset: w.resets_at.map(|t| format!("Resets in {}", until(t))),
                });
            }
            if let Some(w) = &claude.sonnet_weekly {
                rows.push(UsageRow {
                    label: "Sonnet".into(),
                    pct: w.used_percent,
                    detail: "Sonnet weekly usage".into(),
                    reset: w.resets_at.map(|t| format!("Resets in {}", until(t))),
                });
            }
            if let Some(extra) = &claude.extra {
                let pct = if extra.limit_usd > 0.0 {
                    extra.used_usd / extra.limit_usd * 100.0
                } else {
                    0.0
                };
                rows.push(UsageRow {
                    label: "Extra usage".into(),
                    pct,
                    detail: format!(
                        "This month: ${:.2} / ${:.2} {}",
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
    }
}

fn error_row(error: &str) -> UsageRow {
    UsageRow {
        label: "Unavailable".into(),
        pct: 0.0,
        detail: error.chars().take(90).collect(),
        reset: None,
    }
}

fn worst_percent(snap: &Snapshot, selected: ProviderTab) -> Option<u8> {
    match selected {
        ProviderTab::Codex => snap.codex.as_ref().and_then(|c| c.worst_percent()),
        ProviderTab::Claude => snap.claude.as_ref().and_then(|c| c.worst_percent()),
    }
}

fn relative_refresh(snap: &Snapshot) -> String {
    let age = chrono::Utc::now()
        .signed_duration_since(snap.refreshed_at)
        .num_seconds()
        .max(0);
    match age {
        0..=10 => "Updated just now".into(),
        11..=59 => format!("Updated {age}s ago"),
        60..=3599 => format!("Updated {}m ago", age / 60),
        _ => format!("Updated {}h ago", age / 3600),
    }
}

fn color_for_pct(pct: f64) -> Color {
    if pct >= 90.0 {
        Color::Rgb(255, 90, 95)
    } else if pct >= 70.0 {
        Color::Rgb(244, 183, 64)
    } else {
        Color::Rgb(78, 190, 174)
    }
}

fn centered(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = area.width.min(max_width).max(40);
    let height = area.height.min(max_height).max(20);
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
