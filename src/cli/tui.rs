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
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Sparkline};

use chrono::Utc;

use crate::celebration;
use crate::config::{Config, ResetStyle};
use crate::pace::{self, DEFAULT_SESSION_MINUTES, DEFAULT_WEEKLY_MINUTES, Pace, Stage};
use crate::provider_status;
use crate::snapshot::{self, Snapshot};
use crate::util::spark;
use crate::util::time::{reset_label, until_from};

// Catppuccin Mocha
const BASE: Color = Color::Rgb(24, 24, 37);
const SURFACE0: Color = Color::Rgb(49, 50, 68);
const SURFACE1: Color = Color::Rgb(69, 71, 90);
const OVERLAY0: Color = Color::Rgb(108, 112, 134);
const TEXT: Color = Color::Rgb(205, 214, 244);
const SUBTEXT0: Color = Color::Rgb(147, 153, 178);
const BLUE: Color = Color::Rgb(137, 180, 250);
const MAUVE: Color = Color::Rgb(203, 166, 247);
const GREEN: Color = Color::Rgb(166, 227, 161);
const YELLOW: Color = Color::Rgb(249, 226, 175);
const PEACH: Color = Color::Rgb(250, 179, 135);
const RED: Color = Color::Rgb(243, 139, 168);
const TEAL: Color = Color::Rgb(148, 226, 213);

/// Snapshot older than this is flagged as stale in the header.
const STALE_AFTER: Duration = Duration::from_secs(300);

struct App {
    snapshot: Option<Snapshot>,
    read_error: Option<String>,
    poll_interval: Duration,
    next_poll: Instant,
    reset_style: ResetStyle,
    show_cost: bool,
    confetti: bool,
    /// Monotonic tick for confetti particle animation.
    frame: u64,
}

impl App {
    fn new(poll_secs: u64) -> Self {
        let interval = Duration::from_secs(poll_secs.clamp(1, 30));
        let cfg = Config::load_or_default().unwrap_or_default();
        let mut app = Self {
            snapshot: None,
            read_error: None,
            poll_interval: interval,
            next_poll: Instant::now(),
            reset_style: cfg.display.reset_style,
            show_cost: cfg.display.show_cost,
            confetti: cfg.display.confetti,
            frame: 0,
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
        app.frame = app.frame.wrapping_add(1);
        terminal.draw(|frame| draw(frame, &app))?;

        // Faster tick while celebrating so confetti animates smoothly.
        let animating = app.confetti
            && app
                .snapshot
                .as_ref()
                .is_some_and(|s| celebration::is_celebrating(s.celebrating_until, Utc::now()));
        let tick = if animating {
            Duration::from_millis(80)
        } else {
            Duration::from_millis(250)
        };

        let timeout = app
            .next_poll
            .saturating_duration_since(Instant::now())
            .min(tick);

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

// ─── data model ──────────────────────────────────────────────────────────────

struct UsageRow {
    label: String,
    /// `Some(pct)` renders a gauge bar; `None` renders a single text line.
    bar: Option<f64>,
    detail: String,
    reset: Option<String>,
    pace: Option<Pace>,
}

impl UsageRow {
    fn height(&self, compact: bool) -> u16 {
        if compact || self.bar.is_none() {
            1
        } else {
            2 + u16::from(self.runout().is_some())
        }
    }

    /// Warning line shown only when the window is projected to run dry
    /// before its reset.
    fn runout(&self) -> Option<(String, Color)> {
        let p = self.pace.as_ref()?;
        if p.will_last_to_reset {
            return None;
        }
        let eta = p.eta_seconds?;
        if eta <= 1.0 {
            return Some(("⚠ limit reached".into(), RED));
        }
        let now = Utc::now();
        let target = now + chrono::Duration::milliseconds((eta * 1000.0).round() as i64);
        let color = if matches!(p.stage, Stage::FarAhead) {
            RED
        } else {
            PEACH
        };
        Some((format!("⚠ runs out in {}", until_from(target, now)), color))
    }

    /// Compact burn-rate badge, e.g. `▲ 9% over` / `▼ 6% spare`.
    fn pace_badge(&self) -> Option<(String, Color)> {
        let p = self.pace.as_ref()?;
        let d = p.delta_percent.round() as i64;
        Some(match p.stage {
            Stage::OnTrack => ("● on pace".into(), OVERLAY0),
            Stage::SlightlyAhead => (format!("▲ {d}% over"), YELLOW),
            Stage::Ahead => (format!("▲ {d}% over"), PEACH),
            Stage::FarAhead => (format!("▲ {d}% over"), RED),
            Stage::SlightlyBehind | Stage::Behind | Stage::FarBehind => {
                (format!("▼ {}% spare", d.abs()), GREEN)
            }
        })
    }
}

struct Card {
    title: &'static str,
    accent: Color,
    plan: Option<String>,
    rows: Vec<UsageRow>,
    incident: Option<String>,
    /// Shown centered when there are no rows (error / no data).
    fallback: String,
}

impl Card {
    fn height(&self, compact: bool) -> u16 {
        let body: u16 = if self.rows.is_empty() {
            1
        } else {
            self.rows.iter().map(|r| r.height(compact)).sum()
        };
        body + u16::from(self.incident.is_some()) + 2
    }
}

// ─── drawing ─────────────────────────────────────────────────────────────────

fn draw(frame: &mut ratatui::Frame<'_>, app: &App) {
    let size = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BASE)), size);

    if size.width < 44 || size.height < 12 {
        render_too_small(frame, size);
        return;
    }

    let cards = build_cards(app);

    let width = size.width.min(100);
    let inner_w = width.saturating_sub(4);
    let two_cols = inner_w >= 56 && cards.len() > 1;

    let layout_for = |compact: bool| -> (Vec<usize>, Vec<usize>, u16) {
        let (col_a, col_b) = pack_columns(&cards, two_cols, compact);
        let col_height = |idxs: &[usize]| -> u16 {
            let sum: u16 = idxs.iter().map(|&i| cards[i].height(compact)).sum();
            sum + idxs.len().saturating_sub(1) as u16
        };
        let cards_h = col_height(&col_a).max(col_height(&col_b)).max(4);
        (col_a, col_b, cards_h)
    };

    // Full rows first; fall back to compact single-line rows on short terminals.
    let (col_a, col_b, cards_h, compact) = {
        let (a, b, h) = layout_for(false);
        if h + 4 <= size.height {
            (a, b, h, false)
        } else {
            let (a, b, h) = layout_for(true);
            (a, b, h, true)
        }
    };

    let cost_available = app.show_cost && app.snapshot.as_ref().is_some_and(|s| s.cost.is_some());
    // header(1) + blank(1) + cards + borders(2); cost adds blank(1) + 4 lines.
    let base_h = cards_h + 4;
    let with_cost_h = base_h + 5;
    let show_cost = cost_available && with_cost_h <= size.height;
    let height = if show_cost { with_cost_h } else { base_h }.min(size.height);
    let area = centered(size, width, height);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_set(symbols::border::ROUNDED)
        .border_style(Style::default().fg(SURFACE1))
        .style(Style::default().bg(BASE).fg(TEXT))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled("◆", Style::default().fg(MAUVE)),
            Span::styled(
                " AI Usage ",
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]))
        .title_alignment(Alignment::Center)
        .title_bottom(
            Line::from(vec![
                Span::styled(
                    " r",
                    Style::default().fg(MAUVE).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" refresh", Style::default().fg(OVERLAY0)),
                Span::styled(" · ", Style::default().fg(SURFACE1)),
                Span::styled("q", Style::default().fg(MAUVE).add_modifier(Modifier::BOLD)),
                Span::styled(" quit ", Style::default().fg(OVERLAY0)),
            ])
            .centered(),
        );
    frame.render_widget(outer, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    let mut constraints = vec![
        Constraint::Length(1), // header strip
        Constraint::Length(1), // spacer
        Constraint::Length(cards_h),
    ];
    if show_cost {
        constraints.push(Constraint::Length(1)); // spacer
        constraints.push(Constraint::Length(4)); // cost strip
    }
    constraints.push(Constraint::Min(0));
    let chunks = Layout::vertical(constraints).split(inner);

    render_header(frame, chunks[0], app);
    if cards.is_empty() {
        render_empty_state(frame, chunks[2], app);
    } else {
        render_cards(frame, chunks[2], &cards, &col_a, &col_b, compact);
    }
    if show_cost {
        render_cost(frame, chunks[4], app);
    }

    // Confetti last so particles sit above panels without covering labels much.
    if app.confetti
        && let Some(until) = app.snapshot.as_ref().and_then(|s| s.celebrating_until)
        && celebration::is_celebrating(Some(until), Utc::now())
    {
        render_confetti(frame, area, celebration::confetti_seed(until), app.frame);
    }
}

fn render_too_small(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let msg = "terminal too small";
    let y = area.y + area.height / 2;
    frame.render_widget(
        Paragraph::new(Span::styled(msg, Style::default().fg(OVERLAY0)))
            .alignment(Alignment::Center),
        Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        },
    );
}

fn render_empty_state(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let mut lines = vec![Line::from(Span::styled(
        "waiting for snapshot…",
        Style::default().fg(SUBTEXT0),
    ))];
    if let Some(err) = &app.read_error {
        lines.push(Line::from(Span::styled(
            fit(err, area.width),
            Style::default().fg(OVERLAY0),
        )));
    }
    lines.push(Line::from(Span::styled(
        "run `ai-usage-bar waybar` to populate it",
        Style::default().fg(OVERLAY0),
    )));
    let top = area.height.saturating_sub(lines.len() as u16) / 2;
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        Rect {
            x: area.x,
            y: area.y + top,
            width: area.width,
            height: area.height.saturating_sub(top),
        },
    );
}

/// Header strip: refresh age on the left, per-provider summary chips right.
fn render_header(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let (refreshed, refreshed_color) = match &app.snapshot {
        Some(snap) if snap.is_stale(STALE_AFTER) => {
            (format!("⚠ stale · {}", relative_refresh(snap)), YELLOW)
        }
        Some(snap) => (format!("⟳ {}", relative_refresh(snap)), OVERLAY0),
        None => ("no snapshot".into(), OVERLAY0),
    };

    // (name, accent, value, value color)
    let mut items: Vec<(&'static str, Color, String, Color)> = Vec::new();
    if let Some(snap) = &app.snapshot {
        let mut pct_chip = |name: &'static str, accent: Color, pct: Option<u8>| {
            if let Some(p) = pct {
                items.push((name, accent, format!("{p}%"), color_for_pct(f64::from(p))));
            }
        };
        pct_chip(
            "codex",
            MAUVE,
            snap.codex.as_ref().and_then(|c| c.worst_percent()),
        );
        pct_chip(
            "claude",
            BLUE,
            snap.claude.as_ref().and_then(|c| c.worst_percent()),
        );
        pct_chip(
            "grok",
            GREEN,
            snap.grok.as_ref().and_then(|c| c.worst_percent()),
        );
        pct_chip(
            "cursor",
            YELLOW,
            snap.cursor.as_ref().and_then(|c| c.worst_percent()),
        );
        if let Some(bal) = snap.opencode.as_ref().and_then(|o| o.balance_usd) {
            items.push(("oc", TEAL, format!("${bal:.2}"), TEXT));
        }
    }

    let left_w = (refreshed.chars().count() as u16).min(area.width);

    // Drop trailing chips until the row fits instead of clipping mid-chip.
    let avail = area.width.saturating_sub(left_w + 2) as usize;
    let chips_width = |items: &[(&str, Color, String, Color)]| -> usize {
        items
            .iter()
            .enumerate()
            .map(|(i, (n, _, v, _))| n.len() + 1 + v.chars().count() + if i > 0 { 2 } else { 0 })
            .sum()
    };
    while !items.is_empty() && chips_width(&items) > avail {
        items.pop();
    }

    let mut chips: Vec<Span> = Vec::new();
    for (i, (name, accent, value, color)) in items.into_iter().enumerate() {
        if i > 0 {
            chips.push(Span::raw("  "));
        }
        chips.push(Span::styled(
            format!("{name} "),
            Style::default().fg(accent),
        ));
        chips.push(Span::styled(
            value,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }

    let cols = Layout::horizontal([Constraint::Length(left_w + 1), Constraint::Min(0)]).split(area);
    frame.render_widget(
        Paragraph::new(Span::styled(
            refreshed,
            Style::default().fg(refreshed_color),
        )),
        cols[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(chips)).alignment(Alignment::Right),
        cols[1],
    );
}

/// Greedily balance cards across two columns by rendered height,
/// preserving overall order.
fn pack_columns(cards: &[Card], two_cols: bool, compact: bool) -> (Vec<usize>, Vec<usize>) {
    if !two_cols {
        return ((0..cards.len()).collect(), Vec::new());
    }
    let (mut a, mut b) = (Vec::new(), Vec::new());
    let (mut ha, mut hb) = (0u16, 0u16);
    for (i, card) in cards.iter().enumerate() {
        if ha <= hb {
            a.push(i);
            ha += card.height(compact) + 1;
        } else {
            b.push(i);
            hb += card.height(compact) + 1;
        }
    }
    (a, b)
}

fn render_cards(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    cards: &[Card],
    col_a: &[usize],
    col_b: &[usize],
    compact: bool,
) {
    if col_b.is_empty() {
        render_column(frame, area, cards, col_a, compact);
        return;
    }
    let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .spacing(2)
        .split(area);
    render_column(frame, cols[0], cards, col_a, compact);
    render_column(frame, cols[1], cards, col_b, compact);
}

/// Stack cards top to bottom; whole cards that no longer fit are elided
/// (never squeezed or cut mid-row).
fn render_column(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    cards: &[Card],
    idxs: &[usize],
    compact: bool,
) {
    let bottom = area.y + area.height;
    let mut y = area.y;
    for &i in idxs {
        let h = cards[i].height(compact);
        if y >= bottom {
            break;
        }
        if y + h > bottom {
            frame.render_widget(
                Paragraph::new(Span::styled("…", Style::default().fg(OVERLAY0)))
                    .alignment(Alignment::Center),
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
            );
            break;
        }
        render_card(
            frame,
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: h,
            },
            &cards[i],
            compact,
        );
        y += h + 1;
    }
}

fn render_card(frame: &mut ratatui::Frame<'_>, area: Rect, card: &Card, compact: bool) {
    if area.height < 3 || area.width < 12 {
        return;
    }

    let mut title = vec![
        Span::raw(" "),
        Span::styled(
            card.title,
            Style::default()
                .fg(card.accent)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(plan) = &card.plan {
        title.push(Span::styled(
            format!(" · {plan}"),
            Style::default().fg(OVERLAY0),
        ));
    }
    title.push(Span::raw(" "));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(symbols::border::ROUNDED)
        .border_style(Style::default().fg(SURFACE1))
        .title(Line::from(title))
        .style(Style::default().bg(BASE));
    let inner = block.inner(area).inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    frame.render_widget(block, area);

    if card.rows.is_empty() {
        let mut constraints = vec![Constraint::Length(1)];
        if card.incident.is_some() {
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Min(0));
        let chunks = Layout::vertical(constraints).split(inner);
        frame.render_widget(
            Paragraph::new(Span::styled(
                fit(&card.fallback, inner.width),
                Style::default().fg(OVERLAY0),
            ))
            .alignment(Alignment::Center),
            chunks[0],
        );
        if let Some(inc) = &card.incident {
            render_incident(frame, chunks[1], inc);
        }
        return;
    }

    let mut constraints: Vec<Constraint> = card
        .rows
        .iter()
        .map(|r| Constraint::Length(r.height(compact)))
        .collect();
    if card.incident.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(0));
    let chunks = Layout::vertical(constraints).split(inner);

    for (i, row) in card.rows.iter().enumerate() {
        render_row(frame, chunks[i], row);
    }
    if let Some(inc) = &card.incident {
        render_incident(frame, chunks[card.rows.len()], inc);
    }
}

fn render_incident(frame: &mut ratatui::Frame<'_>, area: Rect, incident: &str) {
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(incident, area.width),
            Style::default().fg(YELLOW),
        )),
        area,
    );
}

fn line_rect(area: Rect, i: u16) -> Rect {
    Rect {
        x: area.x,
        y: area.y + i,
        width: area.width,
        height: 1,
    }
}

const LABEL_W: u16 = 10;

fn render_row(frame: &mut ratatui::Frame<'_>, area: Rect, row: &UsageRow) {
    if area.height == 0 || area.width < 8 {
        return;
    }

    let Some(pct) = row.bar else {
        // Single text line: label · detail · reset (right).
        let reset_w = row
            .reset
            .as_ref()
            .map(|r| r.chars().count() as u16 + 2)
            .unwrap_or(0);
        let cols = Layout::horizontal([
            Constraint::Length(LABEL_W),
            Constraint::Min(0),
            Constraint::Length(reset_w),
        ])
        .split(line_rect(area, 0));
        frame.render_widget(
            Paragraph::new(Span::styled(
                row.label.as_str(),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            )),
            cols[0],
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                fit(&row.detail, cols[1].width),
                Style::default().fg(SUBTEXT0),
            )),
            cols[1],
        );
        if let Some(reset) = &row.reset {
            frame.render_widget(
                Paragraph::new(Span::styled(reset.as_str(), Style::default().fg(OVERLAY0)))
                    .alignment(Alignment::Right),
                cols[2],
            );
        }
        return;
    };

    // Line 1: label + gauge + percent.
    let cols = Layout::horizontal([
        Constraint::Length(LABEL_W),
        Constraint::Min(4),
        Constraint::Length(5),
    ])
    .split(line_rect(area, 0));
    frame.render_widget(
        Paragraph::new(Span::styled(
            row.label.as_str(),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        )),
        cols[0],
    );
    frame.render_widget(Paragraph::new(bar_line(pct, cols[1].width)), cols[1]);
    frame.render_widget(
        Paragraph::new(format!("{pct:>4.0}%"))
            .alignment(Alignment::Right)
            .style(
                Style::default()
                    .fg(color_for_pct(pct))
                    .add_modifier(Modifier::BOLD),
            ),
        cols[2],
    );

    // Line 2: dim meta (detail · reset) with pace badge right-aligned.
    if area.height >= 2 {
        let badge = row.pace_badge();
        let badge_w = badge
            .as_ref()
            .map(|(s, _)| s.chars().count() as u16 + 2)
            .unwrap_or(0);
        let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(badge_w)])
            .split(line_rect(area, 1));
        let mut meta = row.detail.clone();
        if let Some(reset) = &row.reset {
            if !meta.is_empty() {
                meta.push_str(" · ");
            }
            meta.push_str(reset);
        }
        frame.render_widget(
            Paragraph::new(Span::styled(
                fit(&meta, cols[0].width),
                Style::default().fg(SUBTEXT0),
            )),
            cols[0],
        );
        if let Some((text, color)) = badge {
            frame.render_widget(
                Paragraph::new(Span::styled(text, Style::default().fg(color)))
                    .alignment(Alignment::Right),
                cols[1],
            );
        }
    }

    // Line 3 (only when projected to run dry before reset).
    if area.height >= 3
        && let Some((text, color)) = row.runout()
    {
        frame.render_widget(
            Paragraph::new(Span::styled(
                fit(&text, area.width),
                Style::default().fg(color),
            )),
            line_rect(area, 2),
        );
    }
}

/// Sub-cell-precision gauge: solid fill + eighth-block tip over a dim track.
fn bar_line(pct: f64, width: u16) -> Line<'static> {
    const PARTIALS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let w = width as usize;
    if w == 0 {
        return Line::default();
    }
    let ratio = (pct / 100.0).clamp(0.0, 1.0);
    let eighths = (ratio * w as f64 * 8.0).round() as usize;
    let full = (eighths / 8).min(w);
    let rem = eighths % 8;
    let mut fill = "█".repeat(full);
    let mut used = full;
    if rem > 0 && used < w {
        fill.push(PARTIALS[rem]);
        used += 1;
    }
    let track = "░".repeat(w - used);
    Line::from(vec![
        Span::styled(fill, Style::default().fg(color_for_pct(pct))),
        Span::styled(track, Style::default().fg(SURFACE0)),
    ])
}

// ─── card builders ───────────────────────────────────────────────────────────

fn build_cards(app: &App) -> Vec<Card> {
    let Some(snap) = &app.snapshot else {
        return Vec::new();
    };

    let mut cards = Vec::new();
    if let Some(codex) = &snap.codex {
        cards.push(Card {
            title: "CODEX",
            accent: MAUVE,
            plan: codex.plan_type.clone(),
            rows: codex_rows(app),
            incident: incident_line(snap, "codex"),
            fallback: codex.error.clone().unwrap_or_else(|| "no data".into()),
        });
    }
    if let Some(claude) = &snap.claude {
        cards.push(Card {
            title: "CLAUDE",
            accent: BLUE,
            plan: claude.plan_type.clone(),
            rows: claude_rows(app),
            incident: incident_line(snap, "claude"),
            fallback: claude.error.clone().unwrap_or_else(|| "no data".into()),
        });
    }
    if let Some(grok) = &snap.grok {
        cards.push(Card {
            title: "GROK",
            accent: GREEN,
            plan: grok.subscription_tier.clone(),
            rows: grok_rows(app),
            incident: None,
            fallback: grok.error.clone().unwrap_or_else(|| "no data".into()),
        });
    }
    if let Some(cursor) = &snap.cursor {
        cards.push(Card {
            title: "CURSOR",
            accent: YELLOW,
            plan: cursor.membership_type.clone(),
            rows: cursor_rows(app),
            incident: incident_line(snap, "cursor"),
            fallback: cursor.error.clone().unwrap_or_else(|| "no data".into()),
        });
    }
    if let Some(oc) = &snap.opencode {
        cards.push(Card {
            title: "OPENCODE",
            accent: TEAL,
            plan: None,
            rows: opencode_rows(app),
            incident: None,
            fallback: oc.error.clone().unwrap_or_else(|| "no data".into()),
        });
    }
    cards
}

fn incident_line(snap: &Snapshot, id: &str) -> Option<String> {
    provider_status::find(&snap.provider_status, id)
        .and_then(|s| s.display_line())
        .map(|l| l.trim_start().to_string())
}

fn codex_rows(app: &App) -> Vec<UsageRow> {
    let Some(codex) = app.snapshot.as_ref().and_then(|s| s.codex.as_ref()) else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    let now = Utc::now();
    if let Some(w) = &codex.primary {
        rows.push(UsageRow {
            label: "Session".into(),
            bar: Some(w.used_percent),
            detail: w
                .window_minutes
                .map(|m| format!("{}h window", (m / 60).max(1)))
                .unwrap_or_else(|| "session window".into()),
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
            pace: pace::for_window(
                w.used_percent,
                w.window_minutes,
                w.resets_at,
                now,
                DEFAULT_SESSION_MINUTES,
            ),
        });
    }
    if let Some(w) = &codex.secondary {
        rows.push(UsageRow {
            label: "Weekly".into(),
            bar: Some(w.used_percent),
            detail: w
                .window_minutes
                .map(|m| {
                    if m >= 24 * 60 {
                        format!("{}d window", (m / (24 * 60)).max(1))
                    } else {
                        format!("{}h window", (m / 60).max(1))
                    }
                })
                .unwrap_or_else(|| "secondary window".into()),
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
            pace: pace::for_window(
                w.used_percent,
                w.window_minutes,
                w.resets_at,
                now,
                DEFAULT_WEEKLY_MINUTES,
            ),
        });
    }
    if let Some(c) = &codex.credits {
        let detail = if c.unlimited {
            "unlimited".into()
        } else {
            format!(
                "balance {}",
                c.balance.clone().unwrap_or_else(|| "0".into())
            )
        };
        rows.push(UsageRow {
            label: "Credits".into(),
            bar: None,
            detail,
            reset: None,
            pace: None,
        });
    }
    if let Some(rc) = &codex.reset_credits
        && rc.available_count > 0
    {
        let n = rc.available_count;
        rows.push(UsageRow {
            label: "Resets".into(),
            bar: None,
            detail: format!("{n} available"),
            reset: rc
                .credits
                .iter()
                .find_map(|c| c.expires_at)
                .map(|t| crate::util::time::expires_label(t, app.reset_style)),
            pace: None,
        });
    }
    rows
}

fn claude_rows(app: &App) -> Vec<UsageRow> {
    let Some(claude) = app.snapshot.as_ref().and_then(|s| s.claude.as_ref()) else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    let now = Utc::now();
    if let Some(w) = &claude.session {
        rows.push(UsageRow {
            label: "Session".into(),
            bar: Some(w.used_percent),
            detail: "5h window".into(),
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
            pace: pace::for_window(
                w.used_percent,
                None,
                w.resets_at,
                now,
                DEFAULT_SESSION_MINUTES,
            ),
        });
    }
    if let Some(w) = &claude.weekly {
        rows.push(UsageRow {
            label: "Weekly".into(),
            bar: Some(w.used_percent),
            detail: "7d window".into(),
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
            pace: pace::for_window(
                w.used_percent,
                None,
                w.resets_at,
                now,
                DEFAULT_WEEKLY_MINUTES,
            ),
        });
    }
    if let Some(w) = &claude.sonnet_weekly {
        rows.push(UsageRow {
            label: "Sonnet".into(),
            bar: Some(w.used_percent),
            detail: "7d window".into(),
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
            pace: pace::for_window(
                w.used_percent,
                None,
                w.resets_at,
                now,
                DEFAULT_WEEKLY_MINUTES,
            ),
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
            bar: Some(pct),
            detail: format!(
                "${:.2} / ${:.2} {}",
                extra.used_usd, extra.limit_usd, extra.currency
            ),
            reset: None,
            pace: None,
        });
    }
    rows
}

fn grok_rows(app: &App) -> Vec<UsageRow> {
    let Some(grok) = app.snapshot.as_ref().and_then(|s| s.grok.as_ref()) else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    let now = Utc::now();
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
            label: "Included".into(),
            bar: Some(w.used_percent),
            detail,
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
            pace: pace::for_window(
                w.used_percent,
                w.window_minutes,
                w.resets_at,
                now,
                DEFAULT_WEEKLY_MINUTES,
            ),
        });
    }
    if let Some(used) = grok.on_demand_used_usd
        && (grok.on_demand_enabled || used > 0.0)
    {
        rows.push(UsageRow {
            label: "On-demand".into(),
            bar: None,
            detail: format!("${used:.2} used"),
            reset: None,
            pace: None,
        });
    }
    rows
}

fn cursor_rows(app: &App) -> Vec<UsageRow> {
    let Some(cursor) = app.snapshot.as_ref().and_then(|s| s.cursor.as_ref()) else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    let now = Utc::now();
    if let Some(w) = &cursor.primary {
        let detail =
            if let (Some(used), Some(limit)) = (cursor.requests_used, cursor.requests_limit) {
                format!("{used} / {limit} req")
            } else {
                match (cursor.plan_used_usd, cursor.plan_limit_usd) {
                    (Some(used), Some(limit)) => format!("${used:.2} / ${limit:.2}"),
                    _ => w
                        .window_minutes
                        .map(|m| {
                            let days = (m / (24 * 60)).max(1);
                            format!("{days}d window")
                        })
                        .unwrap_or_else(|| "plan".into()),
                }
            };
        rows.push(UsageRow {
            label: "Plan".into(),
            bar: Some(w.used_percent),
            detail,
            reset: w.resets_at.map(|t| reset_label(t, app.reset_style)),
            pace: pace::for_window(
                w.used_percent,
                w.window_minutes,
                w.resets_at,
                now,
                DEFAULT_WEEKLY_MINUTES,
            ),
        });
    }
    if let Some(used) = cursor.on_demand_used_usd
        && (used > 0.0 || cursor.on_demand_limit_usd.unwrap_or(0.0) > 0.0)
    {
        let detail = match cursor.on_demand_limit_usd {
            Some(limit) if limit > 0.0 => format!("${used:.2} / ${limit:.2}"),
            _ => format!("${used:.2}"),
        };
        rows.push(UsageRow {
            label: "On-demand".into(),
            bar: None,
            detail,
            reset: None,
            pace: None,
        });
    }
    rows
}

fn opencode_rows(app: &App) -> Vec<UsageRow> {
    let Some(oc) = app.snapshot.as_ref().and_then(|s| s.opencode.as_ref()) else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    if let Some(b) = oc.balance_usd {
        rows.push(UsageRow {
            label: "Balance".into(),
            bar: None,
            detail: format!("${b:.2}"),
            reset: (b <= 0.0).then(|| "depleted".into()),
            pace: None,
        });
    }
    if let Some(cost) = oc.local_30d_cost_usd {
        rows.push(UsageRow {
            label: "Last 30d".into(),
            bar: None,
            detail: format!("${cost:.2} spent"),
            reset: None,
            pace: None,
        });
    }
    rows
}

// ─── cost strip ──────────────────────────────────────────────────────────────

fn render_cost(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let Some(cost) = app.snapshot.as_ref().and_then(|s| s.cost.as_ref()) else {
        return;
    };

    let chunks = Layout::vertical([
        Constraint::Length(1), // rule with caption
        Constraint::Length(2), // sparkline
        Constraint::Length(1), // provider breakdown
    ])
    .split(area);

    let today = Utc::now().date_naive();
    let series = spark::daily_series(&cost.by_day, today, 30);
    let values: Vec<f64> = series.iter().map(|(_, v)| *v).collect();
    let caption = spark::cost_caption(&series, cost.total_usd, today);

    let caption = fit(&caption, area.width.saturating_sub(6));
    let rest = (area.width as usize).saturating_sub(caption.chars().count() + 4);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("── ", Style::default().fg(SURFACE1)),
            Span::styled(caption, Style::default().fg(SUBTEXT0)),
            Span::styled(
                format!(" {}", "─".repeat(rest)),
                Style::default().fg(SURFACE1),
            ),
        ])),
        chunks[0],
    );

    let data = spark::sparkline_data(&values);
    let take = area.width as usize;
    let recent = &data[data.len().saturating_sub(take)..];
    frame.render_widget(
        Sparkline::default()
            .data(recent)
            .style(Style::default().fg(MAUVE)),
        chunks[1],
    );

    if !cost.by_provider.is_empty() {
        let mut spans: Vec<Span> = Vec::new();
        for (i, (provider, usd)) in cost.by_provider.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" · ", Style::default().fg(SURFACE1)));
            }
            spans.push(Span::styled(
                format!("{provider} "),
                Style::default().fg(accent_for(provider)),
            ));
            spans.push(Span::styled(
                format!("${usd:.2}"),
                Style::default().fg(TEXT),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), chunks[2]);
    }
}

fn accent_for(provider: &str) -> Color {
    match provider {
        "codex" => MAUVE,
        "claude" => BLUE,
        "grok" => GREEN,
        "cursor" => YELLOW,
        "opencode" => TEAL,
        _ => SUBTEXT0,
    }
}

// ─── misc helpers ────────────────────────────────────────────────────────────

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

/// Truncate to `max` display cells, appending `…` when cut.
fn fit(s: &str, max: u16) -> String {
    let max = max as usize;
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

/// Lightweight confetti: a few dozen colored glyphs falling through the frame.
/// Positions are deterministic from the weekly-reset seed + frame index.
fn render_confetti(frame: &mut ratatui::Frame<'_>, area: Rect, seed: u64, tick: u64) {
    const GLYPHS: &[char] = &['•', '*', '▪', '·', '✦'];
    const COLORS: &[Color] = &[MAUVE, BLUE, GREEN, YELLOW, RED];
    const N: u64 = 36;

    let w = area.width as u64;
    let h = area.height as u64;
    if w < 4 || h < 4 {
        return;
    }

    let buf = frame.buffer_mut();
    for i in 0..N {
        // xorshift-ish mix per particle
        let mut s = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(i.wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
        s ^= s >> 30;
        s = s.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        s ^= s >> 27;

        let col = area.x + ((s % w) as u16);
        let speed = 1 + (s >> 8) % 3;
        let phase = (s >> 16) % h;
        let row_off = (tick.wrapping_mul(speed).wrapping_add(phase)) % h;
        let row = area.y + row_off as u16;

        // Keep particles off the outer border cells so they don't punch holes
        // in the rounded chrome; stay a cell inside.
        if col <= area.x || col >= area.x + area.width.saturating_sub(1) {
            continue;
        }
        if row <= area.y || row >= area.y + area.height.saturating_sub(1) {
            continue;
        }

        let glyph = GLYPHS[(s as usize) % GLYPHS.len()];
        let color = COLORS[((s >> 4) as usize) % COLORS.len()];
        if let Some(cell) = buf.cell_mut((col, row)) {
            cell.set_char(glyph);
            cell.set_style(Style::default().fg(color).bg(BASE));
        }
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}
