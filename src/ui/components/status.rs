//! One-row status bar: working directory on the left, model and context on the right.
use super::super::{
    task,
    text::{compact_number, truncate_middle},
    theme,
};
use crate::app::App;
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Span, Style},
    widgets::{Block, Paragraph},
};

pub(crate) fn draw(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    frame.render_widget(
        Block::default().style(theme::base().bg(theme::status_background())),
        area,
    );
    let line = if width >= DESKTOP_WIDTH {
        desktop_line(app, width)
    } else {
        status_line(app, width)
    };
    frame.render_widget(Paragraph::new(line), area);
}

const DESKTOP_WIDTH: usize = 100;

/// Desktop bottom bar: working directory left, clock right (usage lives in the pill above the composer).
fn desktop_line(app: &App, width: usize) -> Line<'static> {
    if !app.follow_tail && app.max_scroll > 0 {
        return status_line(app, width);
    }
    let clock = chrono::Local::now().format("%a %d %b  %H:%M").to_string();
    let left = task::fit(&working_directory(), width.saturating_sub(clock.len() + 3));
    let gap = width.saturating_sub(1 + Span::raw(&left).width() + clock.len() + 1);
    Line::from(vec![
        Span::raw(" "),
        Span::styled(left, Style::default().fg(theme::muted())),
        Span::raw(" ".repeat(gap)),
        Span::styled(clock, Style::default().fg(theme::muted())),
        Span::raw(" "),
    ])
}

/// Floating usage pill shown above the composer on desktop shapes.
pub(crate) fn usage_line(app: &App, width: usize) -> Option<Line<'static>> {
    let usage = &app.state.token_usage;
    let value = Style::default().fg(theme::secondary());
    let label = Style::default().fg(theme::muted());
    let mut parts: Vec<Vec<Span<'static>>> = Vec::new();
    if usage.input_tokens.is_some() || usage.output_tokens.is_some() {
        parts.push(vec![Span::styled(
            format!("↓ {}", compact_number(usage.input_tokens.unwrap_or(0))),
            value,
        )]);
        parts.push(vec![Span::styled(
            format!("↑ {}", compact_number(usage.output_tokens.unwrap_or(0))),
            value,
        )]);
        if let Some(rate) = usage.cache_measurement().hit_rate {
            parts.push(vec![Span::styled(
                format!("cache {:.0}%", rate * 100.0),
                value,
            )]);
        }
    }
    if let (Some(current), Some(total)) = (
        app.state.current_context_tokens,
        app.state
            .active_model_context_length
            .filter(|total| *total > 0),
    ) {
        const BAR: u64 = 11;
        let filled = ((current.saturating_mul(BAR) + total / 2) / total).min(BAR) as usize;
        parts.push(vec![
            Span::styled("ctx ", label),
            Span::styled("━".repeat(filled), Style::default().fg(theme::accent())),
            Span::styled(
                "━".repeat(BAR as usize - filled),
                Style::default().fg(theme::meter_track()),
            ),
            Span::styled(
                format!(
                    "  {}/{} ({:.0}%)",
                    compact_number(current),
                    compact_number(total),
                    current as f64 * 100.0 / total as f64
                ),
                value,
            ),
        ]);
    }
    if !app.state.active_model.is_empty() {
        let mut model = app.state.active_model.clone();
        if !app.state.active_reasoning_level.is_empty() {
            model = format!("{model} ({})", app.state.active_reasoning_level);
        }
        parts.push(vec![Span::styled(model, label)]);
    }
    if parts.is_empty() || width < 3 {
        return None;
    }
    let surface = theme::composer_info_surface();
    let mut spans = vec![Span::raw("  ")];
    for (index, part) in parts.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("   "));
        }
        spans.extend(part);
    }
    spans.push(Span::raw("  "));
    let mut body = Line::from(spans);
    if body.width() > width - 2 {
        let plain = task::fit(&body.to_string(), width - 2);
        body = Line::styled(plain, value);
    }
    let border = Style::default()
        .fg(theme::composer_info_border())
        .bg(surface);
    let mut spans = vec![Span::styled("│", border)];
    spans.extend(
        body.spans
            .into_iter()
            .map(|span| Span::styled(span.content, span.style.bg(surface))),
    );
    spans.push(Span::styled("│", border));
    Some(Line::from(spans))
}

pub(crate) fn working_directory() -> String {
    let dir = std::env::current_dir().unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from)
        && let Ok(rest) = dir.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~/{}", rest.display())
        };
    }
    dir.display().to_string()
}

pub(crate) fn status_line(app: &App, width: usize) -> Line<'static> {
    if !app.follow_tail && app.max_scroll > 0 {
        let remaining = app.max_scroll.saturating_sub(app.scroll_y);
        let candidates = [
            format!(" {remaining} lines below · Ctrl+End latest "),
            format!(" {remaining} below · Ctrl+End "),
            " Ctrl+End ".to_owned(),
        ];
        let text = candidates
            .iter()
            .find(|text| Span::raw(text.as_str()).width() <= width)
            .unwrap_or(&candidates[2]);
        return Line::styled(
            task::fit(text, width),
            Style::default().fg(theme::warning()),
        );
    }

    let current = app.state.current_context_tokens.unwrap_or(0);
    let total = app
        .state
        .active_model_context_length
        .filter(|total| *total > 0 && app.state.current_context_tokens.is_some());
    let mut right: Vec<Span<'static>> = Vec::new();
    if width >= 100 && !app.state.active_model.is_empty() {
        let model = app
            .state
            .active_model
            .rsplit_once('/')
            .map_or(app.state.active_model.as_str(), |(_, model)| model);
        right.push(Span::styled(
            format!("{}  ", truncate_middle(model, 24)),
            Style::default().fg(theme::muted()),
        ));
    }
    if let Some(total) = total {
        let ratio = current as f64 / total as f64;
        let color = if ratio >= 0.95 {
            theme::error()
        } else if ratio >= 0.80 {
            theme::warning()
        } else {
            theme::user()
        };
        let counts = if width >= 36 {
            format!(
                "CTX {} / {}  ",
                compact_number(current),
                compact_number(total)
            )
        } else {
            format!("{:.0}%  ", ratio * 100.0)
        };
        right.push(Span::styled(counts, Style::default().fg(theme::muted())));
        right.push(Span::styled(
            context_meter(current, Some(total), 6),
            Style::default().fg(color),
        ));
    }
    right.push(Span::raw(" "));
    let right_width: usize = right.iter().map(Span::width).sum();

    let left_budget = width.saturating_sub(right_width + 2);
    let left = task::fit(&working_directory(), left_budget);
    let gap = width.saturating_sub(1 + Span::raw(&left).width() + right_width);
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(left, Style::default().fg(theme::muted())),
        Span::raw(" ".repeat(gap)),
    ];
    spans.extend(right);
    Line::from(spans)
}

fn context_meter(current: u64, total: Option<u64>, cells: usize) -> String {
    let Some(total) = total.filter(|total| *total > 0) else {
        return "·".repeat(cells);
    };
    let filled =
        ((current.saturating_mul(cells as u64) + total / 2) / total).min(cells as u64) as usize;
    format!(
        "{}{}",
        "▰".repeat(filled),
        "▱".repeat(cells.saturating_sub(filled))
    )
}
