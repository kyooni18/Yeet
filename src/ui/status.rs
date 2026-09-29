//! One-row status bar: working directory on the left, model and context on the right.
use super::{
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

pub(super) fn draw(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    frame.render_widget(Block::default().style(theme::surface()), area);
    frame.render_widget(Paragraph::new(status_line(app, width)), area);
}

pub(super) fn working_directory() -> String {
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

pub(super) fn status_line(app: &App, width: usize) -> Line<'static> {
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
        let counts = if width >= 60 {
            format!("{}/{}  ", compact_number(current), compact_number(total))
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
