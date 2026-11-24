use super::overview::{panel, provider_message, scrollable};
use super::*;
use crate::storage::{PromptDetail, UsageEvent, UsageEventDetail};
use ratatui::layout::Alignment;
use ratatui::widgets::{Cell, Row, Table, TableState};

impl Dashboard {
    pub(super) fn render_request_list(&self, frame: &mut Frame, area: Rect) {
        let Some(p) = self.snapshot.providers.get(self.selected) else {
            return;
        };
        let focused = matches!(self.view, View::Requests);
        let block = panel(
            format!("Recent requests · {}", provider_label(&p.name)),
            focused,
        );
        if let Some(message) = provider_message(p, p.events.is_empty(), "No requests in this range")
        {
            frame.render_widget(
                Paragraph::new(message)
                    .wrap(Wrap { trim: true })
                    .block(block),
                area,
            );
            return;
        }
        let rows = p.events.iter().map(|event| {
            let usage = &event.usage;
            Row::new([
                Cell::from(local_time(usage)),
                Cell::from(usage.model.as_deref().unwrap_or("unknown")),
                Cell::from(usage.project.as_deref().unwrap_or("—")),
                Cell::from(Line::from(compact(usage.total_tokens)).alignment(Alignment::Right)),
            ])
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(11),
                Constraint::Fill(3),
                Constraint::Fill(2),
                Constraint::Length(7),
            ],
        )
        .header(
            Row::new([
                Cell::from("Local time"),
                Cell::from("Model"),
                Cell::from("Project"),
                Cell::from(Line::from("Tokens").right_aligned()),
            ])
            .style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ")
        .block(block);
        frame.render_stateful_widget(
            table,
            area,
            &mut TableState::default().with_selected(focused.then_some(self.selected_event)),
        );
    }

    pub(super) fn render_prompt_list(&self, frame: &mut Frame, area: Rect) {
        let Some(p) = self.snapshot.providers.get(self.selected) else {
            return;
        };
        let block = panel(
            format!("Prompt history · {}", provider_label(&p.name)),
            true,
        );
        if let Some(message) = provider_message(
            p,
            p.prompt_events.is_empty(),
            "No retrievable prompts in this range",
        ) {
            frame.render_widget(
                Paragraph::new(message)
                    .wrap(Wrap { trim: true })
                    .block(block),
                area,
            );
            return;
        }
        let rows = p.prompt_events.iter().map(|prompt| {
            Row::new([
                Cell::from(local_time(&prompt.usage)),
                Cell::from(prompt.usage.model.as_deref().unwrap_or("—")),
                Cell::from(single_line(&prompt.text)),
            ])
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(11),
                Constraint::Percentage(23),
                Constraint::Fill(1),
            ],
        )
        .header(
            Row::new(["Local time", "Model", "Prompt"])
                .style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ")
        .block(block);
        frame.render_stateful_widget(
            table,
            area,
            &mut TableState::default().with_selected(Some(self.selected_prompt)),
        );
    }
}

fn local_time(usage: &UsageEvent) -> String {
    usage
        .occurred_at
        .with_timezone(&Local)
        .format("%m-%d %H:%M")
        .to_string()
}

fn metadata(label: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<14}"), Style::default().fg(Color::Cyan)),
        Span::raw(value.into()),
    ])
}

fn record_metadata(usage: &UsageEvent) -> Vec<Line<'static>> {
    vec![
        metadata("Model", usage.model.as_deref().unwrap_or("unknown")),
        metadata("Project", usage.project.as_deref().unwrap_or("—")),
        metadata(
            "Time",
            format!(
                "{} / {} UTC",
                usage
                    .occurred_at
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S %Z"),
                usage.occurred_at.format("%Y-%m-%d %H:%M:%S")
            ),
        ),
        metadata("Session", usage.session_id.as_deref().unwrap_or("—")),
    ]
}

pub(super) fn request_detail(
    frame: &mut Frame,
    area: Rect,
    event: &UsageEventDetail,
    offset: &mut u16,
) {
    let u = &event.usage;
    let mut lines = record_metadata(u);
    lines.extend([
        metadata("Status", event.status.as_str()),
        metadata("Client", u.client.as_deref().unwrap_or("—")),
        metadata(
            "Tokens",
            format!(
                "{} total · {} input · {} output",
                u.total_tokens, u.input_tokens, u.output_tokens
            ),
        ),
        metadata(
            "Breakdown",
            format!(
                "{} reasoning · {} cache read · {} cache write",
                u.reasoning_tokens, u.cache_read_tokens, u.cache_write_tokens
            ),
        ),
        metadata(
            "Est. cost",
            if u.cost_usd == 0.0
                && super::overview::lacks_usd_rate(&u.agent_name, u.model.as_deref().unwrap_or(""))
            {
                "unpriced".into()
            } else {
                format!("${:.6}", u.cost_usd)
            },
        ),
        metadata(
            "Duration",
            event
                .duration_ms
                .map_or_else(|| "—".into(), |ms| format!("{ms} ms")),
        ),
        metadata(
            "Request ID",
            event.source_request_id.as_deref().unwrap_or("—"),
        ),
        metadata("Event ID", u.event_id.as_str()),
        metadata(
            "Source",
            format!("{} / {}", event.source_system, event.source_channel),
        ),
        metadata("Locator", event.source_locator.as_deref().unwrap_or("—")),
        metadata("Total source", event.total_source.as_str()),
    ]);
    scrollable(
        frame,
        area,
        panel(format!("Request · {}", provider_label(&u.agent_name)), true),
        lines,
        offset,
    );
}

pub(super) fn prompt_detail(
    frame: &mut Frame,
    area: Rect,
    prompt: &PromptDetail,
    offset: &mut u16,
) {
    let mut lines = record_metadata(&prompt.usage);
    lines.extend([
        metadata(
            "Source",
            format!("{} / {}", prompt.source_system, prompt.source_channel),
        ),
        metadata("Locator", prompt.source_locator.as_deref().unwrap_or("—")),
        Line::from(""),
        Line::styled("Prompt", Style::default().add_modifier(Modifier::BOLD)),
    ]);
    lines.extend(prompt.text.lines().map(Line::from));
    scrollable(
        frame,
        area,
        panel(
            format!("Prompt · {}", provider_label(&prompt.usage.agent_name)),
            true,
        ),
        lines,
        offset,
    );
}
