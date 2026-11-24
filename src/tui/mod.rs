mod activity;
mod overview;

use std::{
    io,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use chrono::{Duration as ChronoDuration, Local, NaiveDate};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseEvent, MouseEventKind,
    },
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
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
            Self::SevenDays => "Week",
            Self::ThirtyDays => "30 Days",
            Self::All => "All",
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
    trend: Vec<crate::storage::UsageTrendPoint>,
    events: Vec<crate::storage::UsageEventDetail>,
    prompt_events: Vec<crate::storage::PromptDetail>,
    error: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ModelUsage {
    name: String,
    requests: i64,
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

pub fn run(update_notice: Option<crate::version_check::UpdateNotice>) -> Result<()> {
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
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).context("enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    let result = Dashboard::new(backends, config, update_notice).event_loop(&mut terminal);
    disable_raw_mode().ok();
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )
    .ok();
    terminal.show_cursor().ok();
    result
}

#[derive(Debug, Default)]
enum View {
    #[default]
    Providers,
    Requests,
    Prompts,
    Request(Box<crate::storage::UsageEventDetail>),
    Prompt(Box<crate::storage::PromptDetail>),
    Report,
}

struct Dashboard {
    backends: Vec<(String, crate::storage::BackendMode)>,
    snapshot: DashboardSnapshot,
    tx: mpsc::Sender<RefreshResult>,
    rx: mpsc::Receiver<RefreshResult>,
    selected: usize,
    view: View,
    help_open: bool,
    detail_scroll: u16,
    selected_event: usize,
    selected_prompt: usize,
    mouse_capture: bool,
    refreshing: bool,
    pending: usize,
    generation: u64,
    startup_ingest_pending: bool,
    auto_sync: bool,
    auto_refresh_interval: Duration,
    last_auto_refresh: Instant,
    update_notice: Option<crate::version_check::UpdateNotice>,
}

impl Dashboard {
    fn new(
        backends: Vec<(String, crate::storage::BackendMode)>,
        config: crate::config::AppConfig,
        update_notice: Option<crate::version_check::UpdateNotice>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut dashboard = Self {
            backends,
            snapshot: DashboardSnapshot::empty(Window::Today),
            tx: tx.clone(),
            rx,
            selected: 0,
            view: View::Providers,
            help_open: false,
            detail_scroll: 0,
            selected_event: 0,
            selected_prompt: 0,
            mouse_capture: true,
            refreshing: false,
            pending: 0,
            generation: 0,
            startup_ingest_pending: config.auto_sync,
            auto_sync: config.auto_sync,
            auto_refresh_interval: config.refresh_interval,
            last_auto_refresh: Instant::now(),
            update_notice,
        };
        // Show the cached summary first, then backfill newly added dimensions
        // (such as projects) in the background.
        dashboard.refresh(tx, false);
        dashboard
    }

    fn refresh(&mut self, tx: mpsc::Sender<RefreshResult>, ingest: bool) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.pending = self.backends.len();
        self.refreshing = self.pending > 0;
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
        }
        for (index, (name, backend)) in self.backends.clone().into_iter().enumerate() {
            let tx = tx.clone();
            thread::spawn(move || {
                let (start, end) = window.dates();
                let data =
                    load_provider(&name, start, end, backend, ingest).unwrap_or_else(|error| {
                        ProviderData {
                            name,
                            error: Some(error.to_string()),
                            ..Default::default()
                        }
                    });
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
                    self.replace_provider(index, data);
                    self.snapshot.refreshed = SystemTime::now();
                }
                if generation == self.generation {
                    self.pending = self.pending.saturating_sub(1);
                    if self.pending == 0 {
                        self.refreshing = false;
                        if self.startup_ingest_pending {
                            self.startup_ingest_pending = false;
                            self.refresh(self.tx.clone(), true);
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
                let mouse_capture_before = self.mouse_capture;
                let should_quit = match event::read()? {
                    Event::Key(key) => self.handle_key(key),
                    Event::Mouse(mouse) => {
                        self.handle_mouse(mouse);
                        false
                    }
                    _ => false,
                };
                if self.mouse_capture != mouse_capture_before {
                    if self.mouse_capture {
                        execute!(terminal.backend_mut(), EnableMouseCapture)?;
                    } else {
                        execute!(terminal.backend_mut(), DisableMouseCapture)?;
                    }
                }
                if should_quit {
                    break;
                }
            }
        }
        Ok(())
    }

    fn replace_provider(&mut self, index: usize, data: ProviderData) {
        let Some(old) = self.snapshot.providers.get_mut(index) else {
            return;
        };
        if index == self.selected {
            let event_id = match &self.view {
                View::Request(event) => Some(&event.usage.event_id),
                View::Requests => old
                    .events
                    .get(self.selected_event)
                    .map(|event| &event.usage.event_id),
                _ => None,
            };
            self.selected_event = event_id
                .and_then(|id| data.events.iter().position(|e| &e.usage.event_id == id))
                .unwrap_or(0);
            let prompt_id = match &self.view {
                View::Prompt(prompt) => Some(&prompt.usage.event_id),
                View::Prompts => old
                    .prompt_events
                    .get(self.selected_prompt)
                    .map(|p| &p.usage.event_id),
                _ => None,
            };
            self.selected_prompt = prompt_id
                .and_then(|id| {
                    data.prompt_events
                        .iter()
                        .position(|p| &p.usage.event_id == id)
                })
                .unwrap_or(0);
            // An open record remains readable even after it ages out of the recent list.
            match &mut self.view {
                View::Request(open) => {
                    if let Some(event) = data.events.get(self.selected_event)
                        && event.usage.event_id == open.usage.event_id
                    {
                        **open = event.clone();
                    }
                }
                View::Prompt(open) => {
                    if let Some(prompt) = data.prompt_events.get(self.selected_prompt)
                        && prompt.usage.event_id == open.usage.event_id
                    {
                        **open = prompt.clone();
                    }
                }
                _ => {}
            }
        }
        *old = data;
    }

    fn set_view(&mut self, view: View) {
        self.view = view;
        self.detail_scroll = 0;
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Char('q')
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return true;
        }
        if self.help_open {
            if matches!(
                key.code,
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Backspace | KeyCode::Enter
            ) {
                self.help_open = false;
            }
            return false;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('?') => self.help_open = true,
            KeyCode::Esc | KeyCode::Backspace => {
                let parent = match self.view {
                    View::Request(_) | View::Prompts => View::Requests,
                    View::Prompt(_) => View::Prompts,
                    View::Requests | View::Report => View::Providers,
                    View::Providers => return key.code == KeyCode::Esc,
                };
                self.set_view(parent);
            }
            KeyCode::Char('r') if !self.refreshing => {
                self.last_auto_refresh = Instant::now();
                self.refresh(self.tx.clone(), true);
            }
            KeyCode::Char('m' | 'M') => self.mouse_capture = !self.mouse_capture,
            KeyCode::Char('w') => self.select_window(self.snapshot.window.next()),
            KeyCode::Char(n @ '1'..='4') => {
                self.select_window(Window::all()[n as usize - '1' as usize])
            }
            KeyCode::Char('p' | 'P') => {
                self.set_view(if matches!(self.view, View::Prompts | View::Prompt(_)) {
                    View::Requests
                } else {
                    View::Prompts
                })
            }
            KeyCode::Char('v') => self.set_view(if matches!(self.view, View::Report) {
                View::Providers
            } else {
                View::Report
            }),
            KeyCode::Left | KeyCode::Char('h') => self.move_provider(-1),
            KeyCode::Right | KeyCode::Char('l') => self.move_provider(1),
            KeyCode::Tab => self.set_view(if matches!(self.view, View::Providers) {
                View::Requests
            } else {
                View::Providers
            }),
            KeyCode::Enter => {
                let provider = self.snapshot.providers.get(self.selected);
                let next = match self.view {
                    View::Providers => Some(View::Requests),
                    View::Requests => provider
                        .and_then(|p| p.events.get(self.selected_event))
                        .cloned()
                        .map(|e| View::Request(Box::new(e))),
                    View::Prompts => provider
                        .and_then(|p| p.prompt_events.get(self.selected_prompt))
                        .cloned()
                        .map(|p| View::Prompt(Box::new(p))),
                    View::Request(_) => Some(View::Requests),
                    View::Prompt(_) => Some(View::Prompts),
                    View::Report => Some(View::Providers),
                };
                if let Some(view) = next {
                    self.set_view(view);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-5),
            KeyCode::PageDown => self.move_selection(5),
            KeyCode::Char('u') if ctrl => self.move_selection(-8),
            KeyCode::Char('d') if ctrl => self.move_selection(8),
            KeyCode::Char('f') if ctrl => self.move_selection(20),
            KeyCode::Char('b') => self.move_selection(if ctrl { -20 } else { -8 }),
            KeyCode::Char(' ') => self.move_selection(8),
            KeyCode::Home => self.move_selection(-isize::MAX),
            KeyCode::End => self.move_selection(isize::MAX),
            _ => {}
        }
        false
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.help_open {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_selection(-3),
            MouseEventKind::ScrollDown => self.move_selection(3),
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let provider = self.snapshot.providers.get(self.selected);
        match self.view {
            View::Providers => self.move_provider(delta),
            View::Requests => {
                let last = provider.map_or(0, |p| p.events.len().saturating_sub(1));
                self.selected_event = self.selected_event.saturating_add_signed(delta).min(last);
            }
            View::Prompts => {
                let last = provider.map_or(0, |p| p.prompt_events.len().saturating_sub(1));
                self.selected_prompt = self.selected_prompt.saturating_add_signed(delta).min(last);
            }
            _ => {
                self.detail_scroll = (self.detail_scroll as usize)
                    .saturating_add_signed(delta)
                    .min(u16::MAX as usize) as u16
            }
        }
    }

    fn move_provider(&mut self, delta: isize) {
        let last = self.snapshot.providers.len().saturating_sub(1);
        let next = self.selected.saturating_add_signed(delta).min(last);
        if next != self.selected {
            self.selected = next;
            self.selected_event = 0;
            self.selected_prompt = 0;
            self.detail_scroll = 0;
            match self.view {
                View::Request(_) => self.view = View::Requests,
                View::Prompt(_) => self.view = View::Prompts,
                _ => {}
            }
        }
    }

    fn select_window(&mut self, window: Window) {
        if window != self.snapshot.window {
            self.snapshot.window = window;
            // Display placeholders until this range's own results arrive.
            self.snapshot.providers.clear();
            self.selected_event = 0;
            self.selected_prompt = 0;
            self.detail_scroll = 0;
            match self.view {
                View::Request(_) => self.view = View::Requests,
                View::Prompt(_) => self.view = View::Prompts,
                _ => {}
            }
            self.refresh(self.tx.clone(), false);
        }
    }

    fn render_report(&mut self, frame: &mut Frame, area: Rect) {
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
                .map(|model| UnicodeWidthStr::width(model.name.as_str()))
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
        let block = Block::bordered()
            .title(format!(
                " {} · usage report ",
                provider_label(&provider.name)
            ))
            .border_style(color);
        overview::scrollable(frame, body_area, block, lines, &mut self.detail_scroll);
    }
}

fn load_provider(
    name: &str,
    start: NaiveDate,
    end: NaiveDate,
    backend: crate::storage::BackendMode,
    ingest: bool,
) -> Result<ProviderData> {
    let (files_scanned, files_with_usage, token_records, malformed_lines) = if ingest {
        let mut store = crate::storage::Backend::open_for_agent(backend, name)?;
        crate::ingest_provider(name, None, &mut store)?
    } else {
        (0, 0, 0, 0)
    };
    let agent = crate::agent_name_for_report(name);
    let from = crate::local_midnight_utc(start);
    let to = crate::local_midnight_utc(end + ChronoDuration::days(1));
    let mut store = crate::storage::Backend::open_read_only_for_agent(backend, name)?;
    let summary = store.agent_summary(agent, from, to)?;
    let mut spending_by_window = [0.0; 4];
    for window in Window::all() {
        let (first, last) = window.dates();
        spending_by_window[window.index()] = if (first, last) == (start, end) {
            summary.cost_usd
        } else {
            store
                .agent_summary(
                    agent,
                    crate::local_midnight_utc(first),
                    crate::local_midnight_utc(last + ChronoDuration::days(1)),
                )?
                .cost_usd
        };
    }
    let trend = store.trend_for_agent(agent, from, to, crate::storage::TrendInterval::Day)?;
    let events = store.usage_events(
        agent,
        &crate::storage::UsageEventQuery {
            requests_only: true,
            from,
            to,
            before: None,
            limit: 25,
            model: None,
            session_id: None,
            status: None,
        },
    )?;
    let prompt_events = store.prompts(
        agent,
        &crate::storage::PromptQuery {
            from,
            to,
            before: None,
            limit: 25,
            session_id: None,
            search: None,
        },
    )?;
    let mut models: Vec<_> = summary
        .models
        .into_iter()
        .map(|(name, usage)| ModelUsage {
            name,
            requests: usage.requests,
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
    Ok(ProviderData {
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
        files_scanned,
        files_with_usage,
        token_records,
        malformed_lines,
        error: None,
    })
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

fn window_selector(selected: Window) -> Line<'static> {
    let mut spans = vec![Span::raw(" Range  ")];
    for (index, window) in Window::all().into_iter().enumerate() {
        if selected == window {
            spans.push(Span::styled(
                format!("[{} {}]", index + 1, window.label()),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::raw(format!(" {} {} ", index + 1, window.label())));
        }
        spans.push(Span::raw("  "));
    }
    Line::from(spans)
}

fn section_line(title: &str, color: Color, width: u16) -> Line<'static> {
    let width = width as usize;
    let title_width = UnicodeWidthStr::width(title) + 2;
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
        let padding = width.saturating_sub(UnicodeWidthStr::width(cell.as_str()));
        value.push(' ');
        if *right_align {
            value.push_str(&" ".repeat(padding));
        }
        value.push_str(&cell);
        if !*right_align {
            value.push_str(&" ".repeat(padding));
        }
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
    let left_len = UnicodeWidthStr::width(left);
    let right_len = UnicodeWidthStr::width(right);
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
    if UnicodeWidthStr::width(value) <= max {
        return value.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let content_width = max - 1;
    let mut truncated = String::new();
    let mut width = 0;
    for character in value.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or_default();
        if width + character_width > content_width {
            break;
        }
        truncated.push(character);
        width += character_width;
    }
    truncated.push('…');
    truncated
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
        assert_eq!(
            Window::all().map(Window::label),
            ["Today", "Week", "30 Days", "All"]
        );
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
    fn table_columns_align_wide_text() {
        let widths = [3, 19, 20, 63];
        let row = table_row(
            &[
                "1".to_owned(),
                "2026-08-10 12:00:00".to_owned(),
                "模型".to_owned(),
                "分析这份报告并列出建议".to_owned(),
            ],
            &widths,
            &[true, false, false, false],
        );
        let rendered = row
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(UnicodeWidthStr::width(rendered.as_str()), 118);
    }

    #[test]
    fn number_keys_select_visible_date_ranges() {
        let mut dashboard = Dashboard::new(
            Vec::new(),
            crate::config::AppConfig {
                auto_sync: false,
                refresh_interval: Duration::from_secs(300),
            },
            None,
        );

        dashboard.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(dashboard.snapshot.window, Window::ThirtyDays);
        dashboard.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE));
        assert_eq!(dashboard.snapshot.window, Window::All);
        dashboard.handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
        assert_eq!(dashboard.snapshot.window, Window::Today);
    }

    #[test]
    fn prompt_history_opens_from_providers_and_returns_one_level_at_a_time() {
        let mut dashboard = Dashboard::new(
            Vec::new(),
            crate::config::AppConfig {
                auto_sync: false,
                refresh_interval: Duration::from_secs(300),
            },
            None,
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
        assert!(matches!(dashboard.view, View::Prompts));
        dashboard.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(dashboard.selected, 1);
        dashboard.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        dashboard.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(dashboard.view, View::Prompt(_)));
        for _ in 0..3 {
            dashboard.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        }
        assert!(matches!(dashboard.view, View::Providers));
    }

    #[test]
    fn refresh_keeps_the_selected_record_and_scroll_position() {
        fn event(id: &str) -> crate::storage::UsageEventDetail {
            crate::storage::usage_event_detail(
                crate::storage::UsageEvent {
                    event_id: id.into(),
                    ..Default::default()
                },
                "codex".into(),
                "jsonl".into(),
                serde_json::json!({}),
                false,
            )
        }
        let mut app = Dashboard::new(Vec::new(), crate::config::AppConfig::default(), None);
        let provider = |ids: &[&str]| ProviderData {
            name: "codex".into(),
            events: ids.iter().map(|id| event(id)).collect(),
            ..Default::default()
        };
        app.snapshot.providers = vec![provider(&["old"]), ProviderData::default()];
        app.set_view(View::Requests);
        app.replace_provider(0, provider(&["new", "old"]));
        assert_eq!(app.selected_event, 1);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.detail_scroll = 6;
        app.replace_provider(1, ProviderData::default());
        app.replace_provider(0, provider(&["newer", "new", "old"]));
        assert_eq!(app.selected_event, 2);
        app.replace_provider(0, provider(&["newest"]));
        assert_eq!(app.detail_scroll, 6);
        assert!(matches!(&app.view, View::Request(open) if open.usage.event_id == "old"));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.selected_event, 0);
        assert!(matches!(app.view, View::Requests));
    }

    #[test]
    fn scroll_supports_mac_keys_and_mouse_wheel() {
        let mut dashboard = Dashboard::new(
            Vec::new(),
            crate::config::AppConfig {
                auto_sync: false,
                refresh_interval: Duration::from_secs(300),
            },
            None,
        );
        dashboard.view = View::Report;

        dashboard.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(dashboard.detail_scroll, 8);
        dashboard.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert_eq!(dashboard.detail_scroll, 28);
        dashboard.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(dashboard.detail_scroll, 8);
        dashboard.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(dashboard.detail_scroll, 0);
        dashboard.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(dashboard.detail_scroll, 8);
        dashboard.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert_eq!(dashboard.detail_scroll, 0);

        let mouse = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        dashboard.handle_mouse(mouse);
        assert_eq!(dashboard.detail_scroll, 3);
        dashboard.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            ..mouse
        });
        assert_eq!(dashboard.detail_scroll, 0);

        assert!(dashboard.mouse_capture);
        dashboard.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        assert!(!dashboard.mouse_capture);
        dashboard.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE));
        assert!(dashboard.mouse_capture);
    }

    #[test]
    fn overview_and_request_inspector_fit_common_terminal_sizes() {
        for (width, height) in [(80, 24), (120, 40)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut dashboard = Dashboard::new(
                Vec::new(),
                crate::config::AppConfig {
                    auto_sync: false,
                    refresh_interval: Duration::from_secs(300),
                },
                Some(crate::version_check::UpdateNotice {
                    current: "1.4.0".into(),
                    latest: "1.5.0".into(),
                    url: "https://example.com/release".into(),
                }),
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
                trend: vec![crate::storage::UsageTrendPoint {
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
            let screen = |dashboard: &mut Dashboard, terminal: &mut Terminal<TestBackend>| {
                terminal.draw(|frame| dashboard.render(frame)).unwrap();
                terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|c| c.symbol())
                    .collect::<String>()
            };
            let overview = screen(&mut dashboard, &mut terminal);
            assert!(overview.contains("Providers") && overview.contains("120"));
            assert_eq!(overview.contains("Recent requests"), width >= 110);
            dashboard.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            let requests = screen(&mut dashboard, &mut terminal);
            assert!(requests.contains("Recent requests") && requests.contains("gpt-5"));
            dashboard.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            assert!(screen(&mut dashboard, &mut terminal).contains("request-1"));
            dashboard.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
            assert!(screen(&mut dashboard, &mut terminal).contains("Keyboard shortcuts"));
        }
    }
}
