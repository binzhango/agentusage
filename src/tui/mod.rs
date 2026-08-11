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
    widgets::{Block, Borders, Paragraph, Sparkline, Wrap},
};

const PROVIDERS: [&str; 5] = ["codex", "claude_code", "opencode", "copilot", "pi"];

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
    fn index(self) -> usize {
        match self {
            Self::Today => 0,
            Self::SevenDays => 1,
            Self::ThirtyDays => 2,
            Self::All => 3,
        }
    }
    fn all() -> [Self; 4] {
        [Self::Today, Self::SevenDays, Self::ThirtyDays, Self::All]
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
    spending_by_window: [f64; 4],
    ai_credits: f64,
    lines_added: i64,
    lines_removed: i64,
    files_scanned: usize,
    files_with_usage: usize,
    token_records: usize,
    malformed_lines: usize,
    models: Vec<ModelUsage>,
    clients: Vec<(String, i64, f64)>,
    projects: Vec<(String, i64, f64)>,
    tools: Vec<(String, usize)>,
    languages: Vec<(String, usize)>,
    primary_used_percent: Option<f64>,
    primary_window_minutes: Option<i64>,
    desktop_signal: Option<(i64, i64)>,
    trend: Vec<crate::storage::DailyUsagePoint>,
    events: Vec<crate::storage::UsageEventDetail>,
    prompt_events: Vec<crate::storage::PromptDetail>,
    error: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ModelUsage {
    name: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    total_tokens: i64,
    cost_usd: f64,
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
    selected_event: usize,
    show_event_detail: bool,
    selected_prompt: usize,
    show_prompts: bool,
    show_prompt_detail: bool,
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
            selected_event: 0,
            show_event_detail: false,
            selected_prompt: 0,
            show_prompts: false,
            show_prompt_detail: false,
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
                    self.selected_event = 0;
                    self.show_event_detail = false;
                    self.selected_prompt = 0;
                    self.show_prompt_detail = false;
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
            KeyCode::Char('q') => true,
            KeyCode::Esc | KeyCode::Backspace if self.show_prompt_detail => {
                self.show_prompt_detail = false;
                self.detail_scroll = 0;
                false
            }
            KeyCode::Esc | KeyCode::Backspace if self.show_prompts => {
                self.show_prompts = false;
                self.detail_scroll = 0;
                false
            }
            KeyCode::Esc | KeyCode::Backspace if self.detail_focus => {
                self.detail_focus = false;
                self.show_event_detail = false;
                self.show_prompt_detail = false;
                false
            }
            KeyCode::Esc => true,
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
            KeyCode::Char('p') | KeyCode::Char('P') => {
                if !self.detail_focus {
                    self.detail_focus = true;
                    self.show_prompts = true;
                } else {
                    self.show_prompts = !self.show_prompts;
                }
                self.show_event_detail = false;
                self.show_prompt_detail = false;
                self.detail_scroll = 0;
                false
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.move_provider(-1);
                false
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.move_provider(1);
                false
            }
            KeyCode::Enter if self.detail_focus => {
                if self.show_prompts {
                    self.show_prompt_detail = !self.show_prompt_detail;
                } else {
                    self.show_event_detail = !self.show_event_detail;
                }
                false
            }
            KeyCode::Tab | KeyCode::Enter => {
                self.detail_focus = !self.detail_focus;
                self.show_event_detail = false;
                self.show_prompt_detail = false;
                false
            }
            KeyCode::Up | KeyCode::Char('k') if self.detail_focus => {
                if self.show_prompts {
                    self.selected_prompt = self.selected_prompt.saturating_sub(1);
                } else {
                    self.selected_event = self.selected_event.saturating_sub(1);
                }
                false
            }
            KeyCode::Down | KeyCode::Char('j') if self.detail_focus => {
                if let Some(provider) = self.snapshot.providers.get(self.selected) {
                    if self.show_prompts && !provider.prompt_events.is_empty() {
                        self.selected_prompt =
                            (self.selected_prompt + 1).min(provider.prompt_events.len() - 1);
                    } else if !self.show_prompts && !provider.events.is_empty() {
                        self.selected_event =
                            (self.selected_event + 1).min(provider.events.len() - 1);
                    }
                }
                false
            }
            KeyCode::PageUp if self.detail_focus => {
                self.detail_scroll = self.detail_scroll.saturating_sub(5);
                false
            }
            KeyCode::PageDown if self.detail_focus => {
                self.detail_scroll = self.detail_scroll.saturating_add(5);
                false
            }
            KeyCode::Home => {
                self.detail_scroll = 0;
                if self.detail_focus {
                    self.selected_event = 0;
                    self.selected_prompt = 0;
                    self.show_event_detail = false;
                    self.show_prompt_detail = false;
                } else {
                    self.selected = 0;
                }
                false
            }
            KeyCode::End => {
                self.detail_scroll = u16::MAX;
                if self.detail_focus {
                    if let Some(provider) = self.snapshot.providers.get(self.selected) {
                        self.selected_event = provider.events.len().saturating_sub(1);
                        self.selected_prompt = provider.prompt_events.len().saturating_sub(1);
                    }
                    self.show_event_detail = false;
                    self.show_prompt_detail = false;
                } else {
                    self.selected = self.snapshot.providers.len().saturating_sub(1);
                }
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

    fn move_provider(&mut self, delta: isize) {
        if self.snapshot.providers.is_empty() {
            return;
        }
        let last = self.snapshot.providers.len().saturating_sub(1) as isize;
        let next = (self.selected as isize + delta).clamp(0, last) as usize;
        if next != self.selected {
            self.selected = next;
            self.detail_scroll = 0;
            self.selected_event = 0;
            self.selected_prompt = 0;
            self.show_event_detail = false;
            self.show_prompt_detail = false;
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
            Paragraph::new(if self.detail_focus {
                if self.show_prompts {
                    "↑↓/jk prompt · Enter expand · p requests · PgUp/PgDn scroll · Tab/Esc back · q quit"
                } else {
                    "↑↓/jk request · Enter inspect · p prompts · PgUp/PgDn scroll · Tab/Esc back · q quit"
                }
            } else {
                "↑↓/jk or ←→/hl select · Enter requests · p prompts · w window · r refresh · q quit"
            })
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
        if self.show_prompts {
            self.render_prompt_dashboard(frame, area, provider, color);
            return;
        }
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
        let compact_layout = inner_width < 96;
        lines.push(Line::from("─".repeat(inner_width as usize)));
        lines.push(section_line("⚡ Usage", Color::Yellow, inner_width));
        if compact_layout {
            lines.extend([
                Line::from(format!(
                    "status {} · window {}",
                    usage_status(provider),
                    usage_window(provider)
                )),
                Line::from(format!(
                    "volume {} tok · requests {} · prompts {}",
                    compact(provider.total_tokens),
                    provider.requests,
                    provider.prompts
                )),
                Line::from(format!(
                    "token records {} · changes +{} / -{}",
                    provider.token_records, provider.lines_added, provider.lines_removed
                )),
            ]);
        } else {
            let usage_widths = [16, 26, 16, 26];
            lines.push(table_border(&usage_widths, '┌', '┬', '┐'));
            lines.push(table_header_row(
                &["metric", "value", "metric", "value"].map(str::to_owned),
                &usage_widths,
                &[false, false, false, false],
            ));
            lines.push(table_border(&usage_widths, '├', '┼', '┤'));
            for cells in [
                vec![
                    "status".to_owned(),
                    usage_status(provider),
                    "window".to_owned(),
                    usage_window(provider),
                ],
                vec![
                    "volume".to_owned(),
                    format!("{} tok", compact(provider.total_tokens)),
                    "requests".to_owned(),
                    provider.requests.to_string(),
                ],
                vec![
                    "token records".to_owned(),
                    provider.token_records.to_string(),
                    "prompts".to_owned(),
                    provider.prompts.to_string(),
                ],
                vec![
                    "code changes".to_owned(),
                    format!("+{} / -{}", provider.lines_added, provider.lines_removed),
                    "".to_owned(),
                    "".to_owned(),
                ],
            ] {
                lines.push(table_row(
                    &cells,
                    &usage_widths,
                    &[false, false, false, false],
                ));
            }
            lines.push(table_border(&usage_widths, '└', '┴', '┘'));
        }
        lines.push(Line::from("Quota remaining"));
        lines.push(rate_limit_bar(
            provider.primary_used_percent,
            area.width.saturating_sub(22) as usize,
        ));
        lines.push(Line::from(""));
        lines.push(section_line("💰 Spending", Color::Green, inner_width));
        let spending_widths = [14, 18];
        lines.push(table_border(&spending_widths, '┌', '┬', '┐'));
        lines.push(table_header_row(
            &["window", "cost"].map(str::to_owned),
            &spending_widths,
            &[false, true],
        ));
        lines.push(table_border(&spending_widths, '├', '┼', '┤'));
        for window in Window::all() {
            let style = if window == self.snapshot.window {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(
                table_row(
                    &[
                        window.label().to_owned(),
                        format!("${:.6}", provider.spending_by_window[window.index()]),
                    ],
                    &spending_widths,
                    &[false, true],
                )
                .style(style),
            );
        }
        lines.push(table_border(&spending_widths, '└', '┴', '┘'));
        lines.push(Line::from(format!("AI credits {:.4}", provider.ai_credits)));
        lines.push(Line::from(""));
        lines.push(section_line(
            "Model Burn",
            Color::Rgb(220, 190, 130),
            inner_width,
        ));
        lines.push(Line::from(vec![
            Span::styled("Total tokens ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                compact(provider.total_tokens),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  Cache rate ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.1}%", cache_rate(provider).unwrap_or(0.0)),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        if provider.models.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No model data for this time range",
                Style::default().fg(Color::DarkGray),
            )));
        } else if compact_layout {
            for (rank, model) in provider.models.iter().enumerate() {
                lines.push(Line::from(format!(
                    "{:>2}. {} · {} tok · ${:.5}",
                    rank + 1,
                    truncate(&model.name, inner_width.saturating_sub(29) as usize),
                    compact(model.total_tokens),
                    model.cost_usd
                )));
                lines.push(Line::from(format!(
                    "    in {} · out {} · cache r/w {}/{}",
                    compact(model.input_tokens),
                    compact(model.output_tokens),
                    compact(model.cache_read_tokens),
                    compact(model.cache_write_tokens)
                )));
            }
            lines.push(Line::from(format!(
                "breakdown in {} · out {} · cache {} · reason {} · total {}",
                compact(provider.input_tokens),
                compact(provider.output_tokens),
                compact(provider.cache_read_tokens),
                compact(provider.reasoning_tokens),
                compact(provider.total_tokens)
            )));
        } else {
            let longest_model = provider
                .models
                .iter()
                .map(|model| model.name.chars().count())
                .max()
                .unwrap_or(28);
            // The other model columns consume 68 terminal cells including
            // borders and padding. Use the remaining space for model names so
            // long names remain visible whenever the terminal can accommodate
            // them.
            let model_width = longest_model
                .min(inner_width.saturating_sub(80) as usize)
                .max(20);
            let widths = [3, model_width, 12, 12, 14, 14, 12, 12];
            lines.push(table_border(&widths, '┌', '┬', '┐'));
            lines.push(table_header_row(
                &[
                    "#",
                    "model",
                    "input",
                    "output",
                    "cache_read",
                    "cache_write",
                    "total",
                    "cost",
                ]
                .map(str::to_owned),
                &widths,
                &[true, false, true, true, true, true, true, true],
            ));
            lines.push(table_border(&widths, '├', '┼', '┤'));
            for (rank, model) in provider.models.iter().enumerate() {
                lines.push(table_row(
                    &[
                        (rank + 1).to_string(),
                        truncate(&model.name, model_width),
                        compact(model.input_tokens),
                        compact(model.output_tokens),
                        compact(model.cache_read_tokens),
                        compact(model.cache_write_tokens),
                        compact(model.total_tokens),
                        format!("${:.5}", model.cost_usd),
                    ],
                    &widths,
                    &[true, false, true, true, true, true, true, true],
                ));
            }
            lines.push(table_border(&widths, '└', '┴', '┘'));
            lines.push(Line::from("Token Breakdown"));
            let token_widths = [14, 14, 16, 14, 14];
            lines.push(table_border(&token_widths, '┌', '┬', '┐'));
            lines.push(table_header_row(
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
            lines.push(table_header_row(
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
            lines.push(table_header_row(
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
            lines.push(table_header_row(
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
            lines.push(table_header_row(
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
        if self.show_prompts {
            lines.push(section_line(
                "Recent Prompts",
                Color::Rgb(142, 209, 197),
                inner_width,
            ));
            if provider.prompt_events.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  No retrievable user prompts for this time range",
                    Style::default().fg(Color::DarkGray),
                )));
            } else if inner_width >= 90 {
                let widths = [3, 19, 20, 42];
                lines.push(table_border(&widths, '┌', '┬', '┐'));
                lines.push(table_header_row(
                    &["#", "timestamp", "model", "prompt"].map(str::to_owned),
                    &widths,
                    &[true, false, false, false],
                ));
                lines.push(table_border(&widths, '├', '┼', '┤'));
                for (index, prompt) in provider.prompt_events.iter().enumerate() {
                    let row = table_row(
                        &[
                            (index + 1).to_string(),
                            prompt
                                .usage
                                .occurred_at
                                .with_timezone(&Local)
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string(),
                            truncate(prompt.usage.model.as_deref().unwrap_or("unknown"), 20),
                            truncate(&single_line(&prompt.text), 42),
                        ],
                        &widths,
                        &[true, false, false, false],
                    );
                    lines.push(if index == self.selected_prompt {
                        row.style(
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        row
                    });
                }
                lines.push(table_border(&widths, '└', '┴', '┘'));
            } else {
                for (index, prompt) in provider.prompt_events.iter().enumerate() {
                    let line = Line::from(format!(
                        "{} {} · {} · {}",
                        if index == self.selected_prompt {
                            ">"
                        } else {
                            " "
                        },
                        prompt
                            .usage
                            .occurred_at
                            .with_timezone(&Local)
                            .format("%m-%d %H:%M"),
                        truncate(prompt.usage.model.as_deref().unwrap_or("unknown"), 18),
                        truncate(
                            &single_line(&prompt.text),
                            inner_width.saturating_sub(36) as usize
                        ),
                    ));
                    lines.push(if index == self.selected_prompt {
                        line.style(Style::default().fg(Color::Yellow))
                    } else {
                        line
                    });
                }
            }
            if self.show_prompt_detail
                && let Some(prompt) = provider.prompt_events.get(self.selected_prompt)
            {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Selected prompt",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )));
                for value in [
                    format!("prompt id      {}", prompt.usage.event_id),
                    format!(
                        "timestamp      {} local / {} UTC",
                        prompt
                            .usage
                            .occurred_at
                            .with_timezone(&Local)
                            .format("%Y-%m-%d %H:%M:%S %Z"),
                        prompt.usage.occurred_at.format("%Y-%m-%d %H:%M:%S")
                    ),
                    format!(
                        "session        {}",
                        prompt.usage.session_id.as_deref().unwrap_or("unavailable")
                    ),
                    format!(
                        "model/project  {} / {}",
                        prompt.usage.model.as_deref().unwrap_or("unknown"),
                        prompt.usage.project.as_deref().unwrap_or("unavailable")
                    ),
                    format!(
                        "source         {} / {}",
                        prompt.source_system, prompt.source_channel
                    ),
                    format!(
                        "source locator {}",
                        prompt.source_locator.as_deref().unwrap_or("unavailable")
                    ),
                ] {
                    lines.push(Line::from(value));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Prompt text",
                    Style::default().fg(Color::Rgb(142, 209, 197)),
                )));
                lines.extend(
                    prompt
                        .text
                        .lines()
                        .map(|line| Line::from(format!("  {line}"))),
                );
            }
        } else {
            lines.push(section_line(
                "Recent Requests",
                Color::Rgb(130, 180, 255),
                inner_width,
            ));
            if provider.events.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  No request events for this time range",
                    Style::default().fg(Color::DarkGray),
                )));
            } else if inner_width >= 90 {
                let widths = [3, 19, 28, 12, 14];
                lines.push(table_border(&widths, '┌', '┬', '┐'));
                lines.push(table_header_row(
                    &["#", "timestamp", "model", "status", "tokens"].map(str::to_owned),
                    &widths,
                    &[true, false, false, false, true],
                ));
                lines.push(table_border(&widths, '├', '┼', '┤'));
                for (index, event) in provider.events.iter().enumerate() {
                    let row = table_row(
                        &[
                            (index + 1).to_string(),
                            event
                                .usage
                                .occurred_at
                                .with_timezone(&Local)
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string(),
                            truncate(event.usage.model.as_deref().unwrap_or("unknown"), 28),
                            truncate(&event.status, 12),
                            compact(event.usage.total_tokens),
                        ],
                        &widths,
                        &[true, false, false, false, true],
                    );
                    lines.push(if index == self.selected_event {
                        row.style(
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        row
                    });
                }
                lines.push(table_border(&widths, '└', '┴', '┘'));
            } else {
                for (index, event) in provider.events.iter().enumerate() {
                    let line = Line::from(format!(
                        "{} {} · {} · {} · {} tok",
                        if index == self.selected_event {
                            ">"
                        } else {
                            " "
                        },
                        event
                            .usage
                            .occurred_at
                            .with_timezone(&Local)
                            .format("%m-%d %H:%M"),
                        truncate(event.usage.model.as_deref().unwrap_or("unknown"), 22),
                        truncate(&event.status, 10),
                        compact(event.usage.total_tokens),
                    ));
                    lines.push(if index == self.selected_event {
                        line.style(Style::default().fg(Color::Yellow))
                    } else {
                        line
                    });
                }
            }
            if self.show_event_detail
                && let Some(event) = provider.events.get(self.selected_event)
            {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Selected request",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )));
                for value in [
                    format!("event id       {}", event.usage.event_id),
                    format!(
                        "timestamp      {} local / {} UTC",
                        event
                            .usage
                            .occurred_at
                            .with_timezone(&Local)
                            .format("%Y-%m-%d %H:%M:%S %Z"),
                        event.usage.occurred_at.format("%Y-%m-%d %H:%M:%S")
                    ),
                    format!(
                        "request id     {}",
                        event.source_request_id.as_deref().unwrap_or("unavailable")
                    ),
                    format!(
                        "session        {}",
                        event.usage.session_id.as_deref().unwrap_or("unavailable")
                    ),
                    format!(
                        "model / status {} / {}",
                        event.usage.model.as_deref().unwrap_or("unknown"),
                        event.status
                    ),
                    format!(
                        "client/project {} / {}",
                        event.usage.client.as_deref().unwrap_or("unavailable"),
                        event.usage.project.as_deref().unwrap_or("unavailable")
                    ),
                    format!(
                        "source         {} / {}",
                        event.source_system, event.source_channel
                    ),
                    format!(
                        "source locator {}",
                        event.source_locator.as_deref().unwrap_or("unavailable")
                    ),
                    format!("total source   {}", event.total_source),
                    format!(
                        "duration        {}",
                        event
                            .duration_ms
                            .map(|value| format!("{value} ms"))
                            .unwrap_or_else(|| "unavailable".into())
                    ),
                    format!(
                        "tokens          in={} out={} reason={} cache-read={} cache-write={} total={}",
                        event.usage.input_tokens,
                        event.usage.output_tokens,
                        event.usage.reasoning_tokens,
                        event.usage.cache_read_tokens,
                        event.usage.cache_write_tokens,
                        event.usage.total_tokens,
                    ),
                    format!("cost            ${:.6}", event.usage.cost_usd),
                ] {
                    lines.push(Line::from(value));
                }
            }
        }
        lines.push(Line::from(""));
        lines.push(section_line("Other Data", Color::DarkGray, inner_width));
        if compact_layout {
            lines.extend([
                Line::from(format!(
                    "messages {} · requests {} · tokens {}",
                    provider.prompts,
                    provider.requests,
                    compact(provider.total_tokens)
                )),
                Line::from(format!(
                    "files {} scanned / {} with usage · malformed {}",
                    provider.files_scanned, provider.files_with_usage, provider.malformed_lines
                )),
            ]);
        } else {
            let other_widths = [20, 18, 20, 18];
            lines.push(table_border(&other_widths, '┌', '┬', '┐'));
            lines.push(table_header_row(
                &["metric", "value", "metric", "value"].map(str::to_owned),
                &other_widths,
                &[false, false, false, false],
            ));
            lines.push(table_border(&other_widths, '├', '┼', '┤'));
            for cells in [
                vec![
                    "today messages".to_owned(),
                    provider.prompts.to_string(),
                    "input tokens".to_owned(),
                    compact(provider.input_tokens),
                ],
                vec![
                    "window requests".to_owned(),
                    provider.requests.to_string(),
                    "output tokens".to_owned(),
                    compact(provider.output_tokens),
                ],
                vec![
                    "window tokens".to_owned(),
                    compact(provider.total_tokens),
                    "cache writes".to_owned(),
                    compact(provider.cache_write_tokens),
                ],
                vec![
                    "files scanned".to_owned(),
                    provider.files_scanned.to_string(),
                    "usage files".to_owned(),
                    provider.files_with_usage.to_string(),
                ],
                vec![
                    "token records".to_owned(),
                    provider.token_records.to_string(),
                    "malformed lines".to_owned(),
                    provider.malformed_lines.to_string(),
                ],
            ] {
                lines.push(table_row(
                    &cells,
                    &other_widths,
                    &[false, false, false, false],
                ));
            }
            lines.push(table_border(&other_widths, '└', '┴', '┘'));
        }
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
        let show_trend = !provider.trend.is_empty() && area.height >= 10;
        let (trend_area, body_area) = if show_trend {
            let areas = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(5), Constraint::Min(5)])
                .split(area);
            (Some(areas[0]), areas[1])
        } else {
            (None, area)
        };
        if let Some(trend_area) = trend_area {
            let data: Vec<u64> = provider
                .trend
                .iter()
                .map(|point| point.total_tokens.max(0) as u64)
                .collect();
            frame.render_widget(
                Sparkline::default()
                    .data(&data)
                    .style(Style::default().fg(color))
                    .block(
                        Block::default()
                            .title(format!(
                                " Daily token trend · {} to {} ",
                                provider.trend.first().map(|point| point.date).unwrap(),
                                provider.trend.last().map(|point| point.date).unwrap()
                            ))
                            .borders(Borders::ALL)
                            .border_style(color),
                    ),
                trend_area,
            );
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
            body_area,
        );
    }

    fn render_prompt_dashboard(
        &self,
        frame: &mut Frame,
        area: Rect,
        provider: &ProviderData,
        color: Color,
    ) {
        let inner_width = area.width.saturating_sub(2);
        let mut lines = vec![Line::from(aligned_header(
            &format!("● {}", provider_label(&provider.name)),
            &format!(
                "✦ Prompts · {} · {}",
                provider.name,
                self.snapshot.window.label()
            ),
            inner_width,
        ))];
        lines.push(Line::from(format!(
            "{} retrievable prompts · use ↑↓/jk, Home/End, Enter to expand",
            provider.prompt_events.len()
        )));
        lines.push(Line::from("─".repeat(inner_width as usize)));
        lines.push(section_line(
            "Recent Prompts",
            Color::Rgb(142, 209, 197),
            inner_width,
        ));
        if provider.prompt_events.is_empty() {
            lines.push(Line::from(Span::styled(
                "No retrievable user prompts for this time range",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(
                "Run `au sync <provider>` if history should be available.",
            ));
        } else if inner_width >= 90 {
            let prompt_width = inner_width.saturating_sub(50).max(24) as usize;
            let widths = [3, 19, 20, prompt_width];
            lines.push(table_border(&widths, '┌', '┬', '┐'));
            lines.push(table_header_row(
                &["#", "timestamp", "model", "prompt"].map(str::to_owned),
                &widths,
                &[true, false, false, false],
            ));
            lines.push(table_border(&widths, '├', '┼', '┤'));
            for (index, prompt) in provider.prompt_events.iter().enumerate() {
                let row = table_row(
                    &[
                        (index + 1).to_string(),
                        prompt
                            .usage
                            .occurred_at
                            .with_timezone(&Local)
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string(),
                        truncate(prompt.usage.model.as_deref().unwrap_or("unknown"), 20),
                        truncate(&single_line(&prompt.text), prompt_width),
                    ],
                    &widths,
                    &[true, false, false, false],
                );
                lines.push(if index == self.selected_prompt {
                    row.style(
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    row
                });
            }
            lines.push(table_border(&widths, '└', '┴', '┘'));
        } else {
            for (index, prompt) in provider.prompt_events.iter().enumerate() {
                let line = Line::from(format!(
                    "{} {} · {} · {}",
                    if index == self.selected_prompt {
                        ">"
                    } else {
                        " "
                    },
                    prompt
                        .usage
                        .occurred_at
                        .with_timezone(&Local)
                        .format("%m-%d %H:%M"),
                    truncate(prompt.usage.model.as_deref().unwrap_or("unknown"), 18),
                    truncate(
                        &single_line(&prompt.text),
                        inner_width.saturating_sub(36) as usize,
                    ),
                ));
                lines.push(if index == self.selected_prompt {
                    line.style(Style::default().fg(Color::Yellow))
                } else {
                    line
                });
            }
        }
        if self.show_prompt_detail
            && let Some(prompt) = provider.prompt_events.get(self.selected_prompt)
        {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Selected prompt",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(format!(
                "timestamp {} local / {} UTC",
                prompt
                    .usage
                    .occurred_at
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S %Z"),
                prompt.usage.occurred_at.format("%Y-%m-%d %H:%M:%S")
            )));
            lines.push(Line::from(format!(
                "session {} · model {} · project {}",
                prompt.usage.session_id.as_deref().unwrap_or("unavailable"),
                prompt.usage.model.as_deref().unwrap_or("unknown"),
                prompt.usage.project.as_deref().unwrap_or("unavailable")
            )));
            lines.push(Line::from(format!(
                "source {} / {} · locator {}",
                prompt.source_system,
                prompt.source_channel,
                prompt.source_locator.as_deref().unwrap_or("unavailable")
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Prompt text",
                Style::default().fg(Color::Rgb(142, 209, 197)),
            )));
            lines.extend(
                prompt
                    .text
                    .lines()
                    .map(|line| Line::from(format!("  {line}"))),
            );
        }
        frame.render_widget(
            Paragraph::new(lines)
                .scroll((self.detail_scroll, 0))
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .title(format!(
                            " {} Prompt History ",
                            provider_label(&provider.name)
                        ))
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
        let header_left = format!("{}{}", if selected { "▶ " } else { "● " }, name);
        let header_right = format!("◷ {} {}", self.snapshot.window.label(), status);
        let header = aligned_header(&header_left, &header_right, area.width.saturating_sub(2));
        let mut lines = vec![Line::from(vec![Span::styled(
            header,
            Style::default()
                .fg(if status == "OK" { color } else { Color::Yellow })
                .add_modifier(Modifier::BOLD),
        )])];
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
            let value_width = area.width.saturating_sub(28).max(18) as usize;
            let summary_widths = [18, value_width];
            lines.push(table_border(&summary_widths, '┌', '┬', '┐'));
            lines.push(table_header_row(
                &["metric", "value"].map(str::to_owned),
                &summary_widths,
                &[false, false],
            ));
            lines.push(table_border(&summary_widths, '├', '┼', '┤'));
            for cells in [
                vec!["prompts".to_owned(), provider.prompts.to_string()],
                vec!["token records".to_owned(), provider.requests.to_string()],
                vec![
                    "model burn".to_owned(),
                    format!("{} tok", compact(provider.total_tokens)),
                ],
                vec!["sessions".to_owned(), provider.sessions.to_string()],
                vec!["cost".to_owned(), format!("${:.5}", provider.cost_usd)],
                vec![
                    "credits / clients".to_owned(),
                    format!("{:.3} / {}", provider.ai_credits, provider.clients.len()),
                ],
            ] {
                lines.push(table_row(&cells, &summary_widths, &[false, false]));
            }
            lines.push(table_border(&summary_widths, '└', '┴', '┘'));
            lines.push(rate_limit_bar(
                provider.primary_used_percent,
                area.width.saturating_sub(26) as usize,
            ));
            lines.push(Line::from(format!(
                "Changes +{} / -{} · {}% cached",
                provider.lines_added,
                provider.lines_removed,
                cached
                    .map(|v| format!("{v:.0}"))
                    .unwrap_or_else(|| "n/a".into())
            )));
            let model_widths = [3, value_width.saturating_sub(12).max(12), 10];
            lines.push(table_header_row(
                &["#", "top models", "tokens"].map(str::to_owned),
                &model_widths,
                &[true, false, true],
            ));
            for model in provider.models.iter().take(3) {
                let rank = provider
                    .models
                    .iter()
                    .position(|value| value.name == model.name)
                    .map(|rank| rank + 1)
                    .unwrap_or_default();
                lines.push(table_row(
                    &[
                        rank.to_string(),
                        truncate(&model.name, model_widths[1]),
                        compact(model.total_tokens),
                    ],
                    &model_widths,
                    &[true, false, true],
                ));
            }
            if provider.models.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  No model data for this time range",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        frame.render_widget(
            Paragraph::new(lines)
                .style(card_style)
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).border_style(border)),
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
            let spending_by_window = load_spending_windows(name, backend);
            let (trend, events, prompt_events) = load_activity(name, start, end, backend);
            let rate_limit = if ingest {
                let cached = load_cached_provider(name, start, end, backend);
                (cached.primary_used_percent, cached.primary_window_minutes)
            } else {
                (None, None)
            };
            let mut models: Vec<_> = report
                .models
                .into_iter()
                .map(|(name, usage)| ModelUsage {
                    name,
                    input_tokens: usage.input,
                    output_tokens: usage.output,
                    cache_read_tokens: usage.cache_read,
                    cache_write_tokens: usage.cache_write,
                    total_tokens: usage.total,
                    cost_usd: usage.cost_usd,
                })
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
            models.sort_by(|a, b| {
                b.total_tokens
                    .cmp(&a.total_tokens)
                    .then_with(|| a.name.cmp(&b.name))
            });
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
                spending_by_window,
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
                trend,
                events,
                prompt_events,
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
    let result = crate::storage::Backend::open_read_only_for_agent(backend, name)
        .and_then(|mut store| store.agent_summary(crate::agent_name_for_report(name), from, to));
    match result {
        Ok(summary) => {
            let spending_by_window = load_spending_windows(name, backend);
            let (trend, events, prompt_events) = load_activity(name, start, end, backend);
            let mut models: Vec<_> = summary
                .models
                .into_iter()
                .map(|(name, usage)| ModelUsage {
                    name,
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read_tokens: usage.cache_read_tokens,
                    cache_write_tokens: usage.cache_write_tokens,
                    total_tokens: usage.total_tokens,
                    cost_usd: usage.cost_usd,
                })
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
            models.sort_by(|a, b| {
                b.total_tokens
                    .cmp(&a.total_tokens)
                    .then_with(|| a.name.cmp(&b.name))
            });
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
                spending_by_window,
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
                trend,
                events,
                prompt_events,
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

fn load_spending_windows(name: &str, backend: crate::storage::BackendMode) -> [f64; 4] {
    let Ok(mut store) = crate::storage::Backend::open_read_only_for_agent(backend, name) else {
        return [0.0; 4];
    };
    let agent = crate::agent_name_for_report(name);
    Window::all().map(|window| {
        let (start, end) = window.dates();
        let from = crate::local_midnight_utc(start);
        let to = crate::local_midnight_utc(end + ChronoDuration::days(1));
        store
            .agent_summary(agent, from, to)
            .map(|summary| summary.cost_usd)
            .unwrap_or_default()
    })
}

fn load_activity(
    name: &str,
    start: NaiveDate,
    end: NaiveDate,
    backend: crate::storage::BackendMode,
) -> (
    Vec<crate::storage::DailyUsagePoint>,
    Vec<crate::storage::UsageEventDetail>,
    Vec<crate::storage::PromptDetail>,
) {
    let Ok(mut store) = crate::storage::Backend::open_read_only_for_agent(backend, name) else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let from = crate::local_midnight_utc(start);
    let to = crate::local_midnight_utc(end + ChronoDuration::days(1));
    let agent = crate::agent_name_for_report(name);
    let trend = store
        .daily_trend_for_agent(agent, from, to)
        .unwrap_or_default();
    let events = store
        .usage_events(
            agent,
            &crate::storage::UsageEventQuery {
                from,
                to,
                before: None,
                limit: 25,
                model: None,
                session_id: None,
                status: None,
            },
        )
        .unwrap_or_default();
    let prompts = store
        .prompts(
            agent,
            &crate::storage::PromptQuery {
                from,
                to,
                before: None,
                limit: 25,
                session_id: None,
                search: None,
            },
        )
        .unwrap_or_default();
    (trend, events, prompts)
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
        let cell = truncate(cell, *width);
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

fn table_header_row(cells: &[String], widths: &[usize], right_aligned: &[bool]) -> Line<'static> {
    table_row(cells, widths, right_aligned).style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
}

fn aligned_header(left: &str, right: &str, width: u16) -> String {
    let width = width as usize;
    let left_len = left.chars().count();
    let right_len = right.chars().count();
    let gap = width.saturating_sub(left_len + right_len + 2).max(2);
    format!("{left}{}{right}", " ".repeat(gap))
}

fn cache_rate(provider: &ProviderData) -> Option<f64> {
    crate::core::token_semantics_for_agent(&provider.name).cache_hit_rate(
        provider.input_tokens,
        provider.cache_read_tokens,
        provider.cache_write_tokens,
    )
}

fn usage_status(provider: &ProviderData) -> String {
    match provider.primary_used_percent {
        Some(used) => format!(
            "{:.1}% left · {used:.1}% used",
            (100.0 - used).clamp(0.0, 100.0)
        ),
        _ => "quota unavailable".into(),
    }
}

fn usage_window(provider: &ProviderData) -> String {
    provider
        .primary_window_minutes
        .map(|minutes| format!("{}d window", minutes / 1440))
        .unwrap_or_else(|| "window unavailable".into())
}

fn rate_limit_bar(used: Option<f64>, width: usize) -> Line<'static> {
    let width = width.max(10);
    let Some(used) = used else {
        return Line::from(vec![
            Span::raw("Usage     "),
            Span::styled(
                format!("{} quota unavailable", "·".repeat(width)),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
    };
    let remaining = (100.0 - used).clamp(0.0, 100.0);
    let filled = (remaining / 100.0 * width as f64).round() as usize;
    let color = if remaining < 10.0 {
        Color::Red
    } else if remaining < 30.0 {
        Color::Yellow
    } else {
        Color::Green
    };
    Line::from(vec![
        Span::raw("Quota     "),
        Span::styled(
            "█".repeat(filled),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "·".repeat(width - filled),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(format!(" {:>4.1}% left", remaining)),
    ])
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

fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use ratatui::{Terminal, backend::TestBackend};
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

    #[test]
    fn prompt_history_is_reachable_from_grid_and_supports_flexible_navigation() {
        let mut dashboard = Dashboard::new(
            Vec::new(),
            crate::config::AppConfig {
                auto_sync: false,
                refresh_interval: Duration::from_secs(300),
            },
        );
        dashboard.snapshot.providers = vec![
            ProviderData {
                name: "codex".into(),
                prompt_events: vec![crate::storage::PromptDetail {
                    usage: crate::storage::UsageEvent {
                        event_id: "prompt-1".into(),
                        agent_name: "codex".into(),
                        ..Default::default()
                    },
                    text: "show prompt history".into(),
                    source_system: "codex".into(),
                    source_channel: "jsonl".into(),
                    source_locator: None,
                }],
                ..Default::default()
            },
            ProviderData {
                name: "pi".into(),
                ..Default::default()
            },
        ];

        dashboard.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert!(dashboard.detail_focus);
        assert!(dashboard.show_prompts);
        dashboard.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(dashboard.selected, 1);
        dashboard.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(dashboard.selected, 0);
        dashboard.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(dashboard.selected_prompt, 0);
        dashboard.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(dashboard.detail_scroll, 0);
        dashboard.handle_key(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::NONE));
        assert!(!dashboard.show_prompts);
        dashboard.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert!(dashboard.show_prompts);
        dashboard.show_prompt_detail = true;
        dashboard.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!dashboard.show_prompt_detail && dashboard.show_prompts);
        dashboard.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(dashboard.detail_focus && !dashboard.show_prompts);
        dashboard.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!dashboard.detail_focus);
    }

    #[test]
    fn renders_grid_and_detail_at_common_terminal_sizes() {
        for (width, height) in [(80, 24), (120, 40)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut dashboard = Dashboard::new(
                Vec::new(),
                crate::config::AppConfig {
                    auto_sync: false,
                    refresh_interval: Duration::from_secs(300),
                },
            );
            let occurred_at = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
            dashboard.snapshot.providers = vec![ProviderData {
                name: "codex".into(),
                requests: 1,
                sessions: 1,
                input_tokens: 100,
                output_tokens: 20,
                cache_read_tokens: 60,
                total_tokens: 120,
                trend: vec![crate::storage::DailyUsagePoint {
                    date: occurred_at.date_naive(),
                    total_tokens: 120,
                    ..Default::default()
                }],
                events: vec![crate::storage::UsageEventDetail {
                    usage: crate::storage::UsageEvent {
                        event_id: "event-1".into(),
                        occurred_at,
                        provider_id: "codex".into(),
                        agent_name: "codex".into(),
                        model: Some("gpt-5".into()),
                        input_tokens: 100,
                        output_tokens: 20,
                        total_tokens: 120,
                        ..Default::default()
                    },
                    source_system: "codex".into(),
                    source_channel: "jsonl".into(),
                    source_request_id: Some("request-1".into()),
                    status: "completed".into(),
                    duration_ms: Some(42),
                    source_locator: None,
                    total_source: "provider_reported".into(),
                    raw_payload: None,
                }],
                ..Default::default()
            }];
            terminal.draw(|frame| dashboard.render(frame)).unwrap();
            dashboard.detail_focus = true;
            terminal.draw(|frame| dashboard.render(frame)).unwrap();
            dashboard.show_prompts = true;
            terminal.draw(|frame| dashboard.render(frame)).unwrap();
            let prompt_view = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(prompt_view.contains("Prompt History"));
            let rendered = terminal.backend().buffer().content().iter().any(|cell| {
                let symbol = cell.symbol();
                symbol == "a" || symbol == "D"
            });
            assert!(rendered);
        }
    }
}
