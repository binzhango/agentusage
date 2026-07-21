use std::{
    io,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use chrono::{Duration as ChronoDuration, Local, NaiveDate};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

const PROVIDERS: [&str; 4] = ["codex", "claude_code", "opencode", "copilot"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Window {
    Today,
    SevenDays,
    ThirtyDays,
    All,
}

impl Window {
    fn next(self) -> Self {
        match self {
            Self::Today => Self::SevenDays,
            Self::SevenDays => Self::ThirtyDays,
            Self::ThirtyDays => Self::All,
            Self::All => Self::Today,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::SevenDays => "7 Days",
            Self::ThirtyDays => "30 Days",
            Self::All => "All Time",
        }
    }
    fn dates(self) -> (NaiveDate, NaiveDate) {
        let end = Local::now().date_naive();
        let start = match self {
            Self::Today => end,
            Self::SevenDays => end - ChronoDuration::days(6),
            Self::ThirtyDays => end - ChronoDuration::days(29),
            Self::All => NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
        };
        (start, end)
    }
}

#[derive(Debug, Clone, Default)]
struct ProviderData {
    name: String,
    loading: bool,
    updating: bool,
    sessions: i64,
    requests: i64,
    prompts: i64,
    total_tokens: i64,
    input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost_usd: f64,
    ai_credits: f64,
    lines_added: i64,
    lines_removed: i64,
    files_scanned: usize,
    files_with_usage: usize,
    token_records: usize,
    malformed_lines: usize,
    models: Vec<(String, i64, f64)>,
    clients: Vec<(String, i64, f64)>,
    projects: Vec<(String, i64, f64)>,
    tools: Vec<(String, usize)>,
    languages: Vec<(String, usize)>,
    primary_used_percent: Option<f64>,
    primary_window_minutes: Option<i64>,
    desktop_signal: Option<(i64, i64)>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct DashboardSnapshot {
    window: Window,
    providers: Vec<ProviderData>,
    refreshed: SystemTime,
}

impl DashboardSnapshot {
    fn empty(window: Window) -> Self {
        Self {
            window,
            providers: Vec::new(),
            refreshed: SystemTime::now(),
        }
    }
}

#[derive(Debug)]
enum RefreshResult {
    Provider {
        generation: u64,
        window: Window,
        index: usize,
        data: ProviderData,
    },
}

pub fn run() -> Result<()> {
    if !io::IsTerminal::is_terminal(&io::stdout()) || !io::IsTerminal::is_terminal(&io::stdin()) {
        anyhow::bail!(
            "the dashboard requires an interactive terminal; use a report subcommand for non-interactive output"
        )
    }

    // Codex keeps the existing interactive initialization behavior. Other
    // providers are discovered without prompting; an uninitialized provider
    // should not prevent the dashboard from opening for the providers that do
    // have usage storage.
    let config = crate::config::load()?;
    let codex_backend = crate::prepare_report_backend("codex")?;
    let mut backends = Vec::new();
    for provider in PROVIDERS {
        let backend = if provider == "codex" {
            Ok(codex_backend)
        } else {
            crate::storage::prepare_backend_for_agent(false, provider)
        };
        match backend {
            Ok(backend) => backends.push((provider.to_owned(), backend)),
            Err(error) => eprintln!(
                "[agentusage] skipping provider={provider}: storage unavailable ({error})"
            ),
        }
    }

    enable_raw_mode().context("enable terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    let result = Dashboard::new(backends, config).event_loop(&mut terminal);
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    result
}

struct Dashboard {
    backends: Vec<(String, crate::storage::BackendMode)>,
    snapshot: DashboardSnapshot,
    tx: mpsc::Sender<RefreshResult>,
    rx: mpsc::Receiver<RefreshResult>,
    selected: usize,
    detail_focus: bool,
    detail_scroll: u16,
    refreshing: bool,
    pending: usize,
    generation: u64,
    queued_window: Option<Window>,
    startup_ingest_pending: bool,
    auto_sync: bool,
    auto_refresh_interval: Duration,
    last_auto_refresh: Instant,
}

impl Dashboard {
    fn new(
        backends: Vec<(String, crate::storage::BackendMode)>,
        config: crate::config::AppConfig,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut dashboard = Self {
            backends,
            snapshot: DashboardSnapshot::empty(Window::Today),
            tx: tx.clone(),
            rx,
            selected: 0,
            detail_focus: false,
            detail_scroll: 0,
            refreshing: false,
            pending: 0,
            generation: 0,
            queued_window: None,
            startup_ingest_pending: config.auto_sync,
            auto_sync: config.auto_sync,
            auto_refresh_interval: config.refresh_interval,
            last_auto_refresh: Instant::now(),
        };
        // Show the cached summary first, then backfill newly added dimensions
        // (such as projects) in the background.
        dashboard.refresh(tx, false);
        dashboard
    }

    fn refresh(&mut self, tx: mpsc::Sender<RefreshResult>, ingest: bool) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.refreshing = true;
        self.pending = self.backends.len();
        let window = self.snapshot.window;
        if self.snapshot.providers.is_empty() {
            self.snapshot.providers = self
                .backends
                .iter()
                .map(|(name, _)| ProviderData {
                    name: name.clone(),
                    loading: true,
                    ..Default::default()
                })
                .collect();
        } else if ingest {
            for provider in &mut self.snapshot.providers {
                provider.updating = true;
            }
        }
        for (index, (name, backend)) in self.backends.clone().into_iter().enumerate() {
            let tx = tx.clone();
            thread::spawn(move || {
                let (start, end) = window.dates();
                let data = load_provider(&name, start, end, backend, ingest);
                let _ = tx.send(RefreshResult::Provider {
                    generation,
                    window,
                    index,
                    data,
                });
            });
        }
    }

    fn event_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        loop {
            while let Ok(RefreshResult::Provider {
                generation,
                window,
                index,
                data,
            }) = self.rx.try_recv()
            {
                if generation == self.generation && window == self.snapshot.window {
                    if let Some(provider) = self.snapshot.providers.get_mut(index) {
                        *provider = data;
                    }
                    self.snapshot.refreshed = SystemTime::now();
                    self.selected = self
                        .selected
                        .min(self.snapshot.providers.len().saturating_sub(1));
                    self.detail_scroll = 0;
                }
                if generation == self.generation {
                    self.pending = self.pending.saturating_sub(1);
                    if self.pending == 0 {
                        self.refreshing = false;
                        if self.startup_ingest_pending {
                            self.startup_ingest_pending = false;
                            self.refresh(self.tx.clone(), true);
                        } else if let Some(window) = self.queued_window.take() {
                            if window != self.snapshot.window {
                                self.snapshot.window = window;
                            }
                            self.refresh(self.tx.clone(), false);
                        }
                    }
                }
            }
            if self.auto_sync
                && !self.refreshing
                && self.last_auto_refresh.elapsed() >= self.auto_refresh_interval
            {
                self.last_auto_refresh = Instant::now();
                self.refresh(self.tx.clone(), true);
            }
            terminal.draw(|frame| self.render(frame))?;
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if self.handle_key(key) {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => true,
            KeyCode::Char('r') => {
                if !self.refreshing {
                    self.last_auto_refresh = Instant::now();
                    self.refresh(self.tx.clone(), true);
                }
                false
            }
            KeyCode::Char('w') => {
                let window = self.snapshot.window.next();
                self.snapshot.window = window;
                // A window switch must not wait behind a slow ingestion pass.
                // Bump the generation and immediately query the cached store;
                // any older worker result is discarded when it returns.
                self.queued_window = None;
                self.refresh(self.tx.clone(), false);
                false
            }
            KeyCode::Tab | KeyCode::Enter => {
                self.detail_focus = !self.detail_focus;
                false
            }
            KeyCode::Up if self.detail_focus => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
                false
            }
            KeyCode::Down if self.detail_focus => {
                self.detail_scroll = self.detail_scroll.saturating_add(1);
                false
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                false
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.snapshot.providers.is_empty() {
                    self.selected = (self.selected + 1).min(self.snapshot.providers.len() - 1);
                }
                false
            }
            _ => false,
        }
    }

    fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(2),
            ])
            .split(area);
        let status = if self.refreshing {
            "refreshing…"
        } else {
            "ready"
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    " agentusage ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    "{} providers · Grid · {} · {}",
                    self.snapshot.providers.len(),
                    self.snapshot.window.label(),
                    status
                )),
            ]))
            .block(Block::default().borders(Borders::BOTTOM)),
            chunks[0],
        );
        if self.detail_focus {
            self.render_detail_dashboard(frame, chunks[1]);
        } else {
            self.render_grid(frame, chunks[1]);
        }
        frame.render_widget(
            Paragraph::new("↑↓/jk select · Enter/Tab detail · w window · r refresh · q quit")
                .style(Style::default().fg(Color::DarkGray)),
            chunks[2],
        );
    }

    fn render_grid(&self, frame: &mut Frame, area: Rect) {
        let columns = if area.width >= 110 { 2 } else { 1 };
        let rows = self.snapshot.providers.len().div_ceil(columns).max(1);
        let row_constraints = vec![Constraint::Ratio(1, rows as u32); rows];
        let row_areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints(row_constraints)
            .split(area);
        for (row, row_area) in row_areas.iter().enumerate() {
            let col_areas = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(vec![Constraint::Ratio(1, columns as u32); columns])
                .split(*row_area);
            for col in 0..columns {
                let index = row * columns + col;
                if let Some(provider) = self.snapshot.providers.get(index) {
                    self.render_card(frame, col_areas[col], index, provider);
                }
            }
        }
    }

    fn render_detail_dashboard(&self, frame: &mut Frame, area: Rect) {
        let Some(provider) = self.snapshot.providers.get(self.selected) else {
            frame.render_widget(Paragraph::new("No provider selected"), area);
            return;
        };
        let color = provider_color(self.selected);
        let mut lines = vec![Line::from(aligned_header(
            &format!("● {}", provider_label(&provider.name)),
            &format!(
                "⚡ Usage · {} · {}",
                provider.name,
                self.snapshot.window.label()
            ),
            area.width.saturating_sub(2),
        ))];
        lines.push(Line::from(Span::styled(
            format!(
                "{} used · {} requests · {} sessions",
                compact(provider.total_tokens),
                provider.requests,
                provider.sessions
            ),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )));
        let inner_width = area.width.saturating_sub(2);
        lines.push(Line::from("─".repeat(inner_width as usize)));
        lines.push(section_line("⚡ Usage", Color::Yellow, inner_width));
        lines.push(Line::from(format!(
            "Status {} · volume {} · token records {} · prompts {} · +{} -{}",
            usage_status(provider),
            compact(provider.total_tokens),
            provider.requests,
            provider.prompts,
            provider.lines_added,
            provider.lines_removed
        )));
        lines.push(Line::from(rate_limit_bar(
            provider.primary_used_percent,
            area.width.saturating_sub(22) as usize,
        )));
        lines.push(Line::from(""));
        lines.push(section_line("💰 Spending", Color::Green, inner_width));
        lines.push(Line::from(format!(
            "All-Time Cost  ${:.6}    AI credits {:.4}",
            provider.cost_usd, provider.ai_credits
        )));
        lines.push(Line::from(""));
        lines.push(section_line(
            "Model Burn",
            Color::Rgb(220, 190, 130),
            inner_width,
        ));
        lines.push(Line::from(format!(
            "Total tokens {}  ·  Cache rate {:.1}%",
            compact(provider.total_tokens),
            cache_rate(provider).unwrap_or(0.0)
        )));
        if provider.models.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No model data for this time range",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            let widths = [3, 28, 9, 16, 12];
            lines.push(table_border(&widths, '┌', '┬', '┐'));
            lines.push(table_row(
                &["#", "model", "share", "tokens", "cost"].map(str::to_owned),
                &widths,
                &[true, false, true, true, true],
            ));
            lines.push(table_border(&widths, '├', '┼', '┤'));
            for (rank, (model, tokens, cost)) in provider.models.iter().take(8).enumerate() {
                let share = if provider.total_tokens > 0 {
                    *tokens as f64 / provider.total_tokens as f64 * 100.0
                } else {
                    0.0
                };
                lines.push(table_row(
                    &[
                        (rank + 1).to_string(),
                        truncate(model, 28),
                        format!("{share:.1}%"),
                        format!("{} tok", compact(*tokens)),
                        format!("${cost:.5}"),
                    ],
                    &widths,
                    &[true, false, true, true, true],
                ));
            }
            lines.push(table_border(&widths, '└', '┴', '┘'));
            lines.push(Line::from("Token Breakdown"));
            let token_widths = [14, 14, 16, 14, 14];
            lines.push(table_border(&token_widths, '┌', '┬', '┐'));
            lines.push(table_row(
                &["input", "output", "cache read", "reasoning", "total"].map(str::to_owned),
                &token_widths,
                &[false, false, false, false, false],
            ));
            lines.push(table_border(&token_widths, '├', '┼', '┤'));
            lines.push(table_row(
                &[
                    compact(provider.input_tokens),
                    compact(provider.output_tokens),
                    compact(provider.cache_read_tokens),
                    compact(provider.reasoning_tokens),
                    compact(provider.total_tokens),
                ],
                &token_widths,
                &[true, true, true, true, true],
            ));
            lines.push(table_border(&token_widths, '└', '┴', '┘'));
        }
        lines.push(Line::from(""));
        lines.push(section_line(
            "Clients",
            Color::Rgb(225, 130, 160),
            inner_width,
        ));
        if provider.clients.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No client data for this time range",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            let widths = [3, 32, 16, 12];
            lines.push(table_border(&widths, '┌', '┬', '┐'));
            lines.push(table_row(
                &["#", "client", "tokens", "cost"].map(str::to_owned),
                &widths,
                &[true, false, true, true],
            ));
            lines.push(table_border(&widths, '├', '┼', '┤'));
            for (rank, (client, tokens, cost)) in provider.clients.iter().take(8).enumerate() {
                lines.push(table_row(
                    &[
                        (rank + 1).to_string(),
                        truncate(client, 32),
                        format!("{} tok", compact(*tokens)),
                        format!("${cost:.5}"),
                    ],
                    &widths,
                    &[true, false, true, true],
                ));
            }
            lines.push(table_border(&widths, '└', '┴', '┘'));
        }
        lines.push(Line::from(""));
        lines.push(section_line(
            "Projects",
            Color::Rgb(120, 190, 220),
            inner_width,
        ));
        if provider.projects.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No project data for this time range",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            let widths = [3, 32, 16, 12];
            lines.push(table_border(&widths, '┌', '┬', '┐'));
            lines.push(table_row(
                &["#", "project", "tokens", "cost"].map(str::to_owned),
                &widths,
                &[true, false, true, true],
            ));
            lines.push(table_border(&widths, '├', '┼', '┤'));
            for (rank, (project, tokens, cost)) in provider.projects.iter().take(10).enumerate() {
                lines.push(table_row(
                    &[
                        (rank + 1).to_string(),
                        truncate(project, 32),
                        format!("{} tok", compact(*tokens)),
                        format!("${cost:.5}"),
                    ],
                    &widths,
                    &[true, false, true, true],
                ));
            }
            lines.push(table_border(&widths, '└', '┴', '┘'));
        }
        lines.push(Line::from(""));
        lines.push(section_line("🔧 Tool Usage", Color::Yellow, inner_width));
        if provider.tools.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No tool calls for this time range",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            let widths = [3, 32, 14];
            lines.push(table_border(&widths, '┌', '┬', '┐'));
            lines.push(table_row(
                &["#", "tool", "calls"].map(str::to_owned),
                &widths,
                &[true, false, true],
            ));
            lines.push(table_border(&widths, '├', '┼', '┤'));
            for (rank, (tool, calls)) in provider.tools.iter().take(10).enumerate() {
                lines.push(table_row(
                    &[
                        (rank + 1).to_string(),
                        truncate(tool, 32),
                        format!("{calls} calls"),
                    ],
                    &widths,
                    &[true, false, true],
                ));
            }
            lines.push(table_border(&widths, '└', '┴', '┘'));
        }
        lines.push(Line::from(""));
        lines.push(section_line(
            "📁 Language",
            Color::Rgb(255, 130, 20),
            inner_width,
        ));
        if provider.languages.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No language data for this time range",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            let total: usize = provider.languages.iter().map(|(_, count)| *count).sum();
            let widths = [22, 9, 12];
            lines.push(table_border(&widths, '┌', '┬', '┐'));
            lines.push(table_row(
                &["language", "share", "requests"].map(str::to_owned),
                &widths,
                &[false, true, true],
            ));
            lines.push(table_border(&widths, '├', '┼', '┤'));
            for (language, count) in provider.languages.iter().take(10) {
                let share = (*count as f64 / total.max(1) as f64) * 100.0;
                lines.push(table_row(
                    &[
                        truncate(language, 22),
                        format!("{share:.1}%"),
                        format!("{count} req"),
                    ],
                    &widths,
                    &[false, true, true],
                ));
            }
            lines.push(table_border(&widths, '└', '┴', '┘'));
        }
        lines.push(Line::from(""));
        lines.push(section_line("Other Data", Color::DarkGray, inner_width));
        lines.push(Line::from(format!(
            "  Today messages {} · input {} · output {}",
            provider.prompts,
            compact(provider.input_tokens),
            compact(provider.output_tokens)
        )));
        lines.push(Line::from(format!(
            "  Window requests {} · window tokens {}",
            provider.requests,
            compact(provider.total_tokens)
        )));
        lines.push(Line::from(format!(
            "  Files scanned {} · usage files {} · token records {}",
            provider.files_scanned, provider.files_with_usage, provider.token_records
        )));
        lines.push(Line::from(format!(
            "  Malformed lines {} · cache writes {}",
            provider.malformed_lines,
            compact(provider.cache_write_tokens)
        )));
        lines.push(Line::from(""));
        lines.push(section_line("⏰ Timers", Color::Red, inner_width));
        if let Some((five, seven)) = provider.desktop_signal {
            lines.push(Line::from(format!(
                "  Primary usage · 5h {} · 7d {}",
                five, seven
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "  Reset timers are not available for this provider",
                Style::default().fg(Color::DarkGray),
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .scroll((self.detail_scroll, 0))
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .title(format!(" {} Detail ", provider_label(&provider.name)))
                        .borders(Borders::ALL)
                        .border_style(color),
                ),
            area,
        );
    }

    fn render_card(&self, frame: &mut Frame, area: Rect, index: usize, provider: &ProviderData) {
        let color = provider_color(index);
        let selected = index == self.selected;
        let border = if selected {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(color)
        };
        let card_style = if selected {
            Style::default().bg(Color::Rgb(38, 42, 62))
        } else {
            Style::default()
        };
        let status = if provider.loading {
            "LOADING"
        } else if provider.updating
            && provider.total_tokens == 0
            && provider.requests == 0
            && provider.sessions == 0
        {
            "UPDATING"
        } else if provider.error.is_some() {
            "UNAVAILABLE"
        } else {
            "OK"
        };
        let name = provider_label(&provider.name);
        let mut lines = vec![Line::from(vec![
            Span::styled(
                if selected { "▶ " } else { "● " },
                Style::default()
                    .fg(if selected { Color::Cyan } else { color })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                name,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw("                                      "),
            Span::styled(
                format!("◷ {} {}", self.snapshot.window.label(), status),
                Style::default().fg(if status == "OK" {
                    Color::Green
                } else {
                    Color::Yellow
                }),
            ),
        ])];
        lines.push(Line::from(Span::styled(
            format!("⚡ Usage · {}", provider.name),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            "────────────────────────────────────────",
            Style::default().fg(color),
        )));
        if provider.loading {
            lines.push(Line::from("Loading provider data…"));
        } else if let Some(error) = &provider.error {
            lines.push(Line::from(Span::styled(
                error.as_str(),
                Style::default().fg(Color::Yellow),
            )));
        } else {
            let cached = cache_rate(provider);
            lines.push(Line::from(format!(
                "Prompts: {} prompts    Token records: {}",
                provider.prompts, provider.requests
            )));
            lines.push(Line::from(format!(
                "Model Burn   {} tok",
                compact(provider.total_tokens)
            )));
            lines.push(Line::from(rate_limit_bar(
                provider.primary_used_percent,
                area.width.saturating_sub(18) as usize,
            )));
            lines.push(Line::from(format!(
                "Sessions {} · {} token records · +{} -{}",
                provider.sessions, provider.requests, provider.lines_added, provider.lines_removed
            )));
            lines.push(Line::from(format!(
                "Token Breakdown · {}% cached",
                cached
                    .map(|v| format!("{v:.0}"))
                    .unwrap_or_else(|| "n/a".into())
            )));
            lines.push(Line::from(
                "              in        out      cache.r    reason     total",
            ));
            for (model, tokens, _) in provider.models.iter().take(3) {
                lines.push(Line::from(format!(
                    "  {:<20} {:>8}  {:>8}",
                    truncate(model, 20),
                    compact(*tokens),
                    ""
                )));
            }
            if provider.models.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  No model data for this time range",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            lines.push(Line::from(format!(
                "Cost ${:.5} · Credits {:.3} · Clients {}",
                provider.cost_usd,
                provider.ai_credits,
                provider.clients.len()
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .style(card_style)
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).border_style(border)),
            area,
        );
    }

    fn total_tokens(&self) -> i64 {
        self.snapshot
            .providers
            .iter()
            .filter(|provider| !provider.loading && provider.error.is_none())
            .map(|provider| provider.total_tokens)
            .sum()
    }

    fn render_wide(&self, frame: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
            .split(area);
        self.render_tiles(frame, cols[0]);
        self.render_detail(frame, cols[1]);
    }

    fn render_narrow(&self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(
                    (self.snapshot.providers.len() as u16 * 5)
                        .max(5)
                        .min(area.height.saturating_sub(8)),
                ),
                Constraint::Min(5),
            ])
            .split(area);
        self.render_tiles(frame, rows[0]);
        self.render_detail(frame, rows[1]);
    }

    fn render_tiles(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = if self.snapshot.providers.is_empty() {
            vec![ListItem::new("Loading provider data…")]
        } else {
            self.snapshot
                .providers
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let title = if i == self.selected {
                        format!("> {}", p.name)
                    } else {
                        format!("  {}", p.name)
                    };
                    let body = if p.loading {
                        "  loading provider data…".to_owned()
                    } else if let Some(error) = &p.error {
                        format!("  unavailable: {}", error)
                    } else {
                        format!(
                            "  {:>8} tokens · {:>4} req · ${:.4}",
                            compact(p.total_tokens),
                            p.requests,
                            p.cost_usd
                        )
                    };
                    ListItem::new(vec![
                        Line::from(Span::styled(
                            title,
                            if i == self.selected {
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::Cyan)
                            },
                        )),
                        Line::from(body),
                        Line::from(format!(
                            "  sessions {} · +{} -{}",
                            p.sessions, p.lines_added, p.lines_removed
                        )),
                        Line::from(""),
                    ])
                })
                .collect()
        };
        frame.render_widget(
            List::new(items).block(Block::default().title(" Providers ").borders(Borders::ALL)),
            area,
        );
    }

    fn render_detail(&self, frame: &mut Frame, area: Rect) {
        let Some(p) = self.snapshot.providers.get(self.selected) else {
            frame.render_widget(
                Paragraph::new("No provider data yet")
                    .block(Block::default().title(" Details ").borders(Borders::ALL)),
                area,
            );
            return;
        };
        if p.loading {
            frame.render_widget(
                Paragraph::new(format!("{}\n\nLoading provider data…", p.name))
                    .block(Block::default().title(" Details ").borders(Borders::ALL)),
                area,
            );
            return;
        }
        if let Some(error) = &p.error {
            frame.render_widget(
                Paragraph::new(format!("{}\n\n{}", p.name, error))
                    .block(Block::default().title(" Details ").borders(Borders::ALL)),
                area,
            );
            return;
        }
        let cache = if p.input_tokens + p.cache_read_tokens + p.cache_write_tokens > 0 {
            format!(
                "{:.2}%",
                p.cache_read_tokens as f64
                    / (p.input_tokens + p.cache_read_tokens + p.cache_write_tokens) as f64
                    * 100.0
            )
        } else {
            "n/a".into()
        };
        let mut lines = vec![
            Line::from(Span::styled(
                format!("{} · {}", p.name, self.snapshot.window.label()),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(format!(
                "requests {}   prompts {}   sessions {}",
                p.requests, p.prompts, p.sessions
            )),
            Line::from(format!(
                "tokens {}   input {}   output {}   reasoning {}",
                compact(p.total_tokens),
                compact(p.input_tokens),
                compact(p.output_tokens),
                compact(p.reasoning_tokens)
            )),
            Line::from(format!(
                "cache hit {}   cost ${:.6}   credits {:.4}",
                cache, p.cost_usd, p.ai_credits
            )),
            Line::from(format!("lines +{} / -{}", p.lines_added, p.lines_removed)),
            Line::from(""),
            Line::from("Top models"),
        ];
        for (name, tokens, cost) in p.models.iter().take(3) {
            lines.push(Line::from(format!(
                "  {:<28} {:>10}  ${:.5}",
                name,
                compact(*tokens),
                cost
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Clients"));
        for (name, tokens, cost) in p.clients.iter().take(3) {
            lines.push(Line::from(format!(
                "  {:<28} {:>10}  ${:.5}",
                name,
                compact(*tokens),
                cost
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Projects"));
        if p.projects.is_empty() {
            lines.push(Line::from("  No project data for this time range"));
        } else {
            for (rank, (name, tokens, cost)) in p.projects.iter().take(5).enumerate() {
                lines.push(Line::from(format!(
                    "  {:>2}  {:<24} {:>10}  ${:.5}",
                    rank + 1,
                    truncate(name, 24),
                    compact(*tokens),
                    cost
                )));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Tool Usage"));
        if p.tools.is_empty() {
            lines.push(Line::from("  No tool data for this time range"));
        } else {
            for (rank, (name, calls)) in p.tools.iter().take(6).enumerate() {
                lines.push(Line::from(format!(
                    "  {:>2}  {:<24} {:>8} calls",
                    rank + 1,
                    truncate(name, 24),
                    calls
                )));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Language"));
        if p.languages.is_empty() {
            lines.push(Line::from("  No language data for this time range"));
        } else {
            let total: usize = p.languages.iter().map(|(_, count)| *count).sum();
            for (name, count) in p.languages.iter().take(6) {
                lines.push(Line::from(format!(
                    "  {:<24} {:>5.1}% {:>6} req",
                    truncate(name, 24),
                    *count as f64 / total.max(1) as f64 * 100.0,
                    count
                )));
            }
        }
        if let Some((five, seven)) = p.desktop_signal {
            lines.push(Line::from(format!(
                "\nClaude desktop signals: 5h {} · 7d {}",
                five, seven
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .scroll((self.detail_scroll, 0))
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .title(if self.detail_focus {
                            " Details [focused] "
                        } else {
                            " Details "
                        })
                        .borders(Borders::ALL),
                ),
            area,
        );
    }
}

fn load_provider(
    name: &str,
    start: NaiveDate,
    end: NaiveDate,
    backend: crate::storage::BackendMode,
    ingest: bool,
) -> ProviderData {
    if !ingest {
        return load_cached_provider(name, start, end, backend);
    }
    match crate::report_for_period(name, start, end, backend) {
        Ok(report) => {
            let rate_limit = if ingest {
                let cached = load_cached_provider(name, start, end, backend);
                (cached.primary_used_percent, cached.primary_window_minutes)
            } else {
                (None, None)
            };
            let mut models: Vec<_> = report
                .models
                .into_iter()
                .map(|(n, u)| (n, u.total, u.cost_usd))
                .collect();
            let mut clients: Vec<_> = report
                .clients
                .into_iter()
                .map(|(n, u)| (n, u.total, u.cost_usd))
                .collect();
            let mut projects: Vec<_> = report
                .projects
                .into_iter()
                .map(|(n, u)| (project_label(&n), u.total, u.cost_usd))
                .collect();
            models.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            clients.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            projects.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let mut tools: Vec<_> = report.tools.into_iter().collect();
            let mut languages: Vec<_> = report.languages.into_iter().collect();
            tools.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            languages.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            ProviderData {
                name: name.to_owned(),
                loading: false,
                sessions: report.sessions as i64,
                requests: report.requests as i64,
                prompts: report.prompts as i64,
                total_tokens: report.total_tokens,
                input_tokens: report.input_tokens,
                output_tokens: report.output_tokens,
                reasoning_tokens: report.reasoning_tokens,
                cache_read_tokens: report.cached_input_tokens,
                cache_write_tokens: report.cache_write_tokens,
                cost_usd: report.cost_usd,
                ai_credits: report.ai_credits,
                lines_added: report.lines_added,
                lines_removed: report.lines_removed,
                files_scanned: report.files_scanned,
                files_with_usage: report.files_with_usage,
                token_records: report.token_records,
                malformed_lines: report.malformed_lines,
                models,
                clients,
                projects,
                tools,
                languages,
                primary_used_percent: rate_limit.0,
                primary_window_minutes: rate_limit.1,
                desktop_signal: None,
                ..Default::default()
            }
        }
        Err(error) => ProviderData {
            name: name.to_owned(),
            loading: false,
            error: Some(error.to_string()),
            ..Default::default()
        },
    }
}

fn load_cached_provider(
    name: &str,
    start: NaiveDate,
    end: NaiveDate,
    backend: crate::storage::BackendMode,
) -> ProviderData {
    let from = crate::local_midnight_utc(start);
    let to = crate::local_midnight_utc(end + ChronoDuration::days(1));
    let result =
        crate::storage::Backend::open_read_only_for_agent(backend, name).and_then(|mut store| {
            store.quick_summary_for_agent(crate::agent_name_for_report(name), from, to)
        });
    match result {
        Ok(summary) => {
            let mut models: Vec<_> = summary
                .models
                .into_iter()
                .map(|(n, u)| (n, u.total_tokens, u.cost_usd))
                .collect();
            let mut clients: Vec<_> = summary
                .clients
                .into_iter()
                .map(|(n, u)| (n, u.total_tokens, u.cost_usd))
                .collect();
            let mut projects: Vec<_> = summary
                .projects
                .into_iter()
                .map(|(n, u)| (project_label(&n), u.total_tokens, u.cost_usd))
                .collect();
            let mut tools: Vec<_> = summary
                .tools
                .into_iter()
                .map(|(name, count)| (name, count as usize))
                .collect();
            let mut languages: Vec<_> = summary
                .languages
                .into_iter()
                .map(|(name, count)| (name, count as usize))
                .collect();
            models.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            clients.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            projects.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            tools.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            languages.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            ProviderData {
                name: name.to_owned(),
                loading: false,
                sessions: summary.sessions,
                requests: summary.requests,
                prompts: summary.prompts,
                total_tokens: summary.total_tokens,
                input_tokens: summary.input_tokens,
                output_tokens: summary.output_tokens,
                reasoning_tokens: summary.reasoning_tokens,
                cache_read_tokens: summary.cache_read_tokens,
                cache_write_tokens: summary.cache_write_tokens,
                cost_usd: summary.cost_usd,
                ai_credits: summary.ai_credits,
                lines_added: summary.lines_added,
                lines_removed: summary.lines_removed,
                models,
                clients,
                projects,
                tools,
                languages,
                primary_used_percent: summary.primary_used_percent,
                primary_window_minutes: summary.primary_window_minutes,
                desktop_signal: None,
                ..Default::default()
            }
        }
        Err(error) => ProviderData {
            name: name.to_owned(),
            loading: false,
            error: Some(error.to_string()),
            ..Default::default()
        },
    }
}

fn compact(value: i64) -> String {
    let value = value as f64;
    if value.abs() >= 1_000_000_000.0 {
        format!("{:.1}B", value / 1_000_000_000.0)
    } else if value.abs() >= 1_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else if value.abs() >= 1_000.0 {
        format!("{:.1}K", value / 1_000.0)
    } else {
        format!("{}", value as i64)
    }
}

fn provider_label(name: &str) -> &str {
    match name {
        "codex" => "codex-cli",
        "claude_code" => "claude-code",
        "opencode" => "opencode",
        "copilot" => "copilot",
        other => other,
    }
}

fn project_label(value: &str) -> String {
    std::path::Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(value)
        .to_owned()
}

fn provider_color(index: usize) -> Color {
    match index % 4 {
        0 => Color::Rgb(241, 116, 157),
        1 => Color::Rgb(244, 219, 70),
        2 => Color::Rgb(110, 190, 160),
        _ => Color::Rgb(170, 140, 235),
    }
}

fn section_line(title: &str, color: Color, width: u16) -> Line<'static> {
    let width = width as usize;
    let title_width = title.chars().count() + 2;
    let fill = width.saturating_sub(title_width).max(1);
    Line::from(Span::styled(
        format!("{title} {}", "─".repeat(fill)),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

fn table_border(widths: &[usize], left: char, separator: char, right: char) -> Line<'static> {
    let mut value = left.to_string();
    for (index, width) in widths.iter().enumerate() {
        value.push_str(&"─".repeat(*width + 2));
        value.push(if index + 1 == widths.len() {
            right
        } else {
            separator
        });
    }
    Line::from(Span::styled(value, Style::default().fg(Color::DarkGray)))
}

fn table_row(cells: &[String], widths: &[usize], right_aligned: &[bool]) -> Line<'static> {
    let mut value = String::from("│");
    for ((cell, width), right_align) in cells.iter().zip(widths).zip(right_aligned) {
        let cell = if *right_align {
            format!("{cell:>width$}")
        } else {
            format!("{cell:<width$}")
        };
        value.push(' ');
        value.push_str(&cell);
        value.push_str(" │");
    }
    Line::from(value)
}

fn aligned_header(left: &str, right: &str, width: u16) -> String {
    let width = width as usize;
    let left_len = left.chars().count();
    let right_len = right.chars().count();
    let gap = width.saturating_sub(left_len + right_len + 2).max(2);
    format!("{left}{}{right}", " ".repeat(gap))
}

fn cache_rate(provider: &ProviderData) -> Option<f64> {
    let denominator =
        provider.input_tokens + provider.cache_read_tokens + provider.cache_write_tokens;
    (denominator > 0).then(|| provider.cache_read_tokens as f64 / denominator as f64 * 100.0)
}

fn usage_status(provider: &ProviderData) -> String {
    match (
        provider.primary_used_percent,
        provider.primary_window_minutes,
    ) {
        (Some(used), Some(window)) => {
            format!(
                "{:.1}% remaining ({used:.1}% used) · {}d window",
                (100.0 - used).clamp(0.0, 100.0),
                window / 1440
            )
        }
        (Some(used), None) => format!(
            "{:.1}% remaining ({used:.1}% used)",
            (100.0 - used).clamp(0.0, 100.0)
        ),
        _ => "quota unavailable".into(),
    }
}

fn rate_limit_bar(used: Option<f64>, width: usize) -> String {
    let width = width.max(10);
    let Some(used) = used else {
        return format!("Usage     {} quota unavailable", "·".repeat(width));
    };
    let remaining = (100.0 - used).clamp(0.0, 100.0);
    let filled = (remaining / 100.0 * width as f64).round() as usize;
    format!(
        "Quota     {}{} {:>4.1}% left",
        "█".repeat(filled),
        "·".repeat(width - filled),
        remaining
    )
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    value
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>()
        + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn window_cycles() {
        assert_eq!(Window::Today.next(), Window::SevenDays);
        assert_eq!(Window::All.next(), Window::Today);
    }
    #[test]
    fn compact_formats_large_values() {
        assert_eq!(compact(1_500), "1.5K");
        assert_eq!(compact(2_000_000), "2.0M");
    }

    #[test]
    fn provider_rows_are_sorted_by_tokens() {
        let mut rows = [
            ("small".to_owned(), 2_i64, 0.0),
            ("large".to_owned(), 20_i64, 0.0),
        ];
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        assert_eq!(rows[0].0, "large");
    }
}
