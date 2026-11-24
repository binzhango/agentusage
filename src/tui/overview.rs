use super::*;
use ratatui::{
    layout::Alignment,
    widgets::{BorderType, Cell, Padding, Row, Table, TableState},
};

const SPLIT_WIDTH: u16 = 110;

fn accent() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn panel(title: impl Into<String>, focused: bool) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        }))
        .title(Line::styled(
            format!(" {} ", title.into()),
            if focused {
                accent()
            } else {
                Style::default()
                    .fg(Color::Reset)
                    .add_modifier(Modifier::BOLD)
            },
        ))
        .padding(Padding::horizontal(1))
}

pub(super) fn scrollable(
    frame: &mut Frame,
    area: Rect,
    block: Block,
    lines: Vec<Line>,
    offset: &mut u16,
) {
    let inner = block.inner(area);
    let text = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max = text
        .line_count(inner.width)
        .saturating_sub(inner.height as usize);
    *offset = (*offset).min(max.min(u16::MAX as usize) as u16);
    frame.render_widget(block, area);
    frame.render_widget(text.scroll((*offset, 0)), inner);
}

fn number(value: impl Into<String>) -> Cell<'static> {
    Cell::from(Line::from(value.into()).alignment(Alignment::Right))
}

fn money(value: f64) -> String {
    if value > 0.0 && value < 0.01 {
        format!("${value:.5}")
    } else {
        format!("${value:.2}")
    }
}

fn missing_rate(provider: &str, model: &ModelUsage) -> bool {
    (model.total_tokens > 0 || model.requests > 0)
        && model.cost_usd == 0.0
        && lacks_usd_rate(provider, &model.name)
}

pub(super) fn lacks_usd_rate(provider: &str, model: &str) -> bool {
    match provider {
        "codex" => !crate::providers::codex::has_cost_estimate(model),
        "claude_code" => crate::providers::local::claude_rates(model).is_none(),
        "copilot" => true,
        _ => false,
    }
}

fn missing_rates(p: &ProviderData) -> usize {
    p.models.iter().filter(|m| missing_rate(&p.name, m)).count()
}

fn estimated_cost(cost: f64, missing: usize) -> String {
    match (missing, cost) {
        (0, _) => money(cost),
        (_, 0.0) => "—".into(),
        _ => format!("{}*", money(cost)),
    }
}

// A bar represents one day in Week and three calendar days in 30 Days.
// Empty dates occupy their own position even if the store omits that day.
fn token_trend<'a>(
    points: impl Iterator<Item = &'a crate::storage::UsageTrendPoint>,
    window: Window,
    end: NaiveDate,
) -> String {
    let (days, per_bar) = match window {
        Window::SevenDays => (7, 1),
        Window::ThirtyDays => (30, 3),
        _ => return String::new(),
    };
    let mut buckets = vec![0_i64; days / per_bar];
    for p in points {
        let age = (end - p.date).num_days();
        if (0..days as i64).contains(&age) {
            let bucket = &mut buckets[(days - 1 - age as usize) / per_bar];
            *bucket = bucket.saturating_add(p.total_tokens.max(0));
        }
    }
    let peak = buckets.iter().copied().max().unwrap_or(0).max(1);
    const BARS: [char; 9] = ['·', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    buckets
        .into_iter()
        .map(|v| BARS[((v as f64 / peak as f64) * 8.0).ceil() as usize])
        .collect()
}

impl Dashboard {
    pub(super) fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if self.help_open {
            let [body, footer] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
            let shortcuts = [
                ("↑ ↓  /  j k", "Select a row or scroll an open record"),
                ("← →  /  h l", "Switch provider"),
                ("Enter", "Open requests or inspect the selected row"),
                ("Tab", "Switch between providers and requests"),
                ("p", "Switch between prompts and requests"),
                ("v", "Open or close the provider usage report"),
                ("Esc / Backspace", "Go back one level"),
                ("1  2  3  4  /  w", "Select or cycle the date range"),
                ("r", "Synchronize and refresh"),
                ("PgUp / PgDn", "Move five rows"),
                ("Ctrl+U/D  ·  b/Space", "Move eight rows"),
                ("Ctrl+B/F", "Move twenty rows"),
                ("Home / End", "Jump to the beginning or end"),
                ("m", "Toggle mouse capture for terminal text selection"),
                ("q / Ctrl+C", "Quit"),
            ];
            let rows = shortcuts.map(|(key, action)| {
                Row::new([Cell::from(key).style(accent()), Cell::from(action)])
            });
            frame.render_widget(
                Table::new(rows, [Constraint::Length(21), Constraint::Fill(1)])
                    .block(panel("Keyboard shortcuts", true)),
                body,
            );
            frame.render_widget(Paragraph::new(" ? / Esc  Close help"), footer);
            return;
        }
        let show_summary = matches!(self.view, View::Providers)
            || (matches!(self.view, View::Requests) && area.width >= SPLIT_WIDTH);
        let [header, notice, summary, body, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(u16::from(self.update_notice.is_some())),
            Constraint::Length(if show_summary && area.height >= 18 {
                4
            } else {
                0
            }),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let failures = self
            .snapshot
            .providers
            .iter()
            .filter(|p| p.error.is_some())
            .count();
        let status = if self.refreshing {
            format!("Refreshing · {} pending", self.pending)
        } else if failures > 0 {
            format!("{failures} unavailable")
        } else {
            let time: chrono::DateTime<Local> = self.snapshot.refreshed.into();
            format!("Updated {}", time.format("%H:%M:%S"))
        };
        let header_line = Line::from(vec![
            Span::styled(" agentusage ", accent()),
            Span::raw("  "),
            Span::raw(status),
        ]);
        frame.render_widget(
            Paragraph::new(vec![header_line, window_selector(self.snapshot.window)]),
            header,
        );
        if let Some(update) = &self.update_notice {
            frame.render_widget(
                Paragraph::new(format!(
                    " Update available · v{} · {}",
                    update.latest, update.url
                ))
                .style(Style::default().fg(Color::Yellow)),
                notice,
            );
        }
        if summary.height != 0 {
            self.render_headlines(frame, summary);
        }
        match &self.view {
            View::Providers | View::Requests if area.width >= SPLIT_WIDTH => {
                let [left, right] =
                    Layout::horizontal([Constraint::Percentage(48), Constraint::Fill(1)])
                        .areas(body);
                self.render_comparison(frame, left);
                self.render_request_list(frame, right);
            }
            View::Providers => self.render_comparison(frame, body),
            View::Requests => self.render_request_list(frame, body),
            View::Prompts => self.render_prompt_list(frame, body),
            View::Report => self.render_report(frame, body),
            View::Request(event) => {
                super::activity::request_detail(frame, body, event, &mut self.detail_scroll)
            }
            View::Prompt(prompt) => {
                super::activity::prompt_detail(frame, body, prompt, &mut self.detail_scroll)
            }
        }
        let hints = match self.view {
            View::Providers => "↑↓ select   Enter requests   p prompts   v report   r refresh",
            View::Requests => "↑↓ select   Enter inspect   p prompts   v report   Esc back",
            View::Prompts => "↑↓ select   Enter read   p requests   Esc back",
            View::Request(_) | View::Prompt(_) | View::Report => {
                "↑↓ scroll   PgUp/PgDn page   Esc back"
            }
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ? help   ", accent()),
                Span::raw(hints),
            ])),
            footer,
        );
    }

    fn render_headlines(&self, frame: &mut Frame, area: Rect) {
        let providers = &self.snapshot.providers;
        let ready = providers.iter().filter(|p| !p.loading && p.error.is_none());
        let loaded = ready.clone().count();
        let missing = ready.clone().map(missing_rates).sum::<usize>();
        let total = compact(ready.clone().map(|p| p.total_tokens).sum());
        let requests = compact(ready.clone().map(|p| p.requests).sum());
        let sessions = ready.clone().map(|p| p.sessions).sum::<i64>();
        let cost = estimated_cost(ready.clone().map(|p| p.cost_usd).sum(), missing);
        let trend = token_trend(
            ready.flat_map(|p| &p.trend),
            self.snapshot.window,
            Local::now().date_naive(),
        );
        let token_detail = if trend.is_empty() {
            self.snapshot.window.label().into()
        } else {
            trend
        };
        let coverage = if missing == 1 {
            "1 unpriced model".into()
        } else if missing > 1 {
            format!("{missing} unpriced models")
        } else {
            "USD".into()
        };
        let parts = Layout::horizontal([Constraint::Fill(1); 3]).split(area);
        for (index, (title, value, detail)) in [
            ("Tokens", total, token_detail),
            ("Requests", requests, format!("{sessions} sessions")),
            ("Est. cost", cost, coverage),
        ]
        .into_iter()
        .enumerate()
        {
            let value = if loaded == 0 { "—".into() } else { value };
            let detail = if loaded < providers.len() {
                format!("{loaded}/{} providers", providers.len())
            } else {
                detail
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled(value, Style::default().add_modifier(Modifier::BOLD)),
                    Line::from(detail),
                ])
                .block(panel(title, false)),
                parts[index],
            );
        }
    }

    fn render_comparison(&self, frame: &mut Frame, area: Rect) {
        let providers = &self.snapshot.providers;
        let list_height = providers.len() as u16 + 3;
        let [list, models] = Layout::vertical([
            Constraint::Length(list_height.min(area.height)),
            Constraint::Fill(1),
        ])
        .areas(area);
        let with_trend = matches!(self.snapshot.window, Window::SevenDays | Window::ThirtyDays)
            && list.width >= 52;
        let mut widths = vec![
            Constraint::Fill(1),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(10),
        ];
        let mut headings = vec![
            Cell::from("Provider"),
            number("Tokens"),
            number("Requests"),
            number("Est. cost"),
        ];
        if with_trend {
            widths.push(Constraint::Length(
                if self.snapshot.window == Window::SevenDays {
                    7
                } else {
                    10
                },
            ));
            headings.push(Cell::from("Trend"));
        }
        let rows = providers.iter().map(|p| {
            let name = provider_label(&p.name);
            let mut cells = vec![Cell::from(name.to_owned())];
            if p.loading || p.error.is_some() {
                cells.extend([
                    number("—"),
                    number("—"),
                    number(if p.loading { "loading" } else { "error" }),
                ]);
            } else {
                cells.extend([
                    number(compact(p.total_tokens)),
                    number(compact(p.requests)),
                    number(estimated_cost(p.cost_usd, missing_rates(p))),
                ]);
                if with_trend {
                    cells.push(
                        Cell::from(token_trend(
                            p.trend.iter(),
                            self.snapshot.window,
                            Local::now().date_naive(),
                        ))
                        .style(Style::default().fg(Color::Cyan)),
                    );
                }
            }
            Row::new(cells)
        });
        let table = Table::new(rows, widths)
            .header(Row::new(headings).style(Style::default().add_modifier(Modifier::BOLD)))
            .highlight_symbol("▸ ")
            .row_highlight_style(accent())
            .block(panel("Providers", matches!(self.view, View::Providers)));
        frame.render_stateful_widget(
            table,
            list,
            &mut TableState::default().with_selected(Some(self.selected)),
        );
        let Some(p) = providers.get(self.selected) else {
            return;
        };
        if models.height < 3 {
            return;
        }
        let block = panel(format!("{} · models", provider_label(&p.name)), false);
        if let Some(message) =
            provider_message(p, p.models.is_empty(), "No model usage in this range")
        {
            frame.render_widget(
                Paragraph::new(message)
                    .wrap(Wrap { trim: true })
                    .block(block),
                models,
            );
            return;
        }
        // Use spare height for project attribution once the leading models fit.
        let model_height = p.models.len().min(6) as u16 + 3;
        let models = if !p.projects.is_empty() && models.height >= model_height + 6 {
            let [models, projects] =
                Layout::vertical([Constraint::Length(model_height), Constraint::Fill(1)])
                    .areas(models);
            render_projects(frame, projects, p);
            models
        } else {
            models
        };
        let rows = p.models.iter().map(|m| {
            Row::new([
                Cell::from(m.name.as_str()),
                number(compact(m.total_tokens)),
                number(estimated_cost(
                    m.cost_usd,
                    usize::from(missing_rate(&p.name, m)),
                )),
            ])
        });
        frame.render_widget(
            Table::new(
                rows,
                [
                    Constraint::Fill(1),
                    Constraint::Length(9),
                    Constraint::Length(12),
                ],
            )
            .header(
                Row::new([Cell::from("Model"), number("Tokens"), number("Est. cost")])
                    .style(Style::default().add_modifier(Modifier::BOLD)),
            )
            .block(block),
            models,
        );
    }
}

fn render_projects(frame: &mut Frame, area: Rect, provider: &ProviderData) {
    let rows = provider.projects.iter().map(|(name, tokens, _)| {
        let share = if provider.total_tokens > 0 {
            *tokens as f64 / provider.total_tokens as f64
        } else {
            0.0
        };
        let filled = (share.clamp(0.0, 1.0) * 10.0).round() as usize;
        let bar = Line::from(vec![
            Span::styled("━".repeat(filled), Style::default().fg(Color::Cyan)),
            Span::styled(
                "─".repeat(10 - filled),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        Row::new([
            Cell::from(name.as_str()),
            number(compact(*tokens)),
            number(format!("{:.0}%", share * 100.0)),
            Cell::from(bar),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Length(9),
            Constraint::Length(6),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new([
            Cell::from("Project"),
            number("Tokens"),
            number("Share"),
            Cell::from(""),
        ])
        .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(panel(
        format!("{} · projects", provider_label(&provider.name)),
        false,
    ));
    frame.render_widget(table, area);
}

pub(super) fn provider_message<'a>(
    p: &'a ProviderData,
    empty: bool,
    empty_text: &'a str,
) -> Option<&'a str> {
    if let Some(err) = &p.error {
        Some(err)
    } else if p.loading {
        Some("Loading usage…")
    } else if empty {
        Some(empty_text)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_trend_keeps_empty_days_and_combines_providers() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        let point = |days, total_tokens| crate::storage::UsageTrendPoint {
            date: date - ChronoDuration::days(days),
            total_tokens,
            ..Default::default()
        };
        let points = [point(7, 300), point(6, 10), point(0, 10), point(0, 10)];
        assert_eq!(
            token_trend(points.iter(), Window::SevenDays, date),
            "▄·····█"
        );
        assert_eq!(
            token_trend(points.iter(), Window::ThirtyDays, date)
                .chars()
                .count(),
            10
        );
    }

    #[test]
    fn partial_estimates_distinguish_missing_rates_from_recorded_zero() {
        let model = ModelUsage {
            name: "gpt-6.2-sol".into(),
            total_tokens: 100,
            ..Default::default()
        };
        assert!(missing_rate("codex", &model));
        assert!(!missing_rate("pi", &model));
        assert_eq!(estimated_cost(0.0, 1), "—");
        assert_eq!(estimated_cost(1.25, 1), "$1.25*");
        assert_eq!(estimated_cost(0.0, 0), "$0.00");
    }
}
