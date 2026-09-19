//! Responsive terminal wordmark and keyboard-first starting points.
use super::{responsive, task, theme};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    prelude::*,
    widgets::{Block, BorderType, Borders, Padding, Paragraph},
};

const WORDMARK: [&str; 4] = [
    "█   █  █▀▀▀  █▀▀▀  ▀▀█▀▀",
    "▀█ █▀  █▄▄   █▄▄     █  ",
    "  █    █     █       █  ",
    "  ▀    ▀▀▀▀  ▀▀▀▀    ▀  ",
];

pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, _shape: responsive::Shape) {
    if area.is_empty() {
        return;
    }
    let expanded = area.width >= 58 && area.height >= 15;
    let wordmark = area.width >= 30 && area.height >= 9;
    let height = if expanded {
        14
    } else if wordmark {
        8
    } else {
        3
    };
    let content = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(height) / 2,
        area.width,
        height.min(area.height),
    );
    let mut lines = if wordmark {
        WORDMARK
            .iter()
            .map(|line| Line::styled(*line, theme::brand()))
            .collect::<Vec<_>>()
    } else {
        vec![Line::styled("YEET /", theme::brand())]
    };
    lines.push(Line::default());
    lines.push(Line::styled(
        task::fit("What are we building?", area.width as usize),
        Style::default().fg(theme::text()).bold(),
    ));
    if wordmark {
        lines.push(Line::styled(
            task::fit(
                "Describe a task. Ask a question. Start here.",
                area.width as usize,
            ),
            Style::default().fg(theme::muted()),
        ));
    }
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), content);
    if !expanded {
        return;
    }

    let cards_width = area.width.min(78);
    let cards = Rect::new(
        area.x + (area.width - cards_width) / 2,
        content.y + 9,
        cards_width,
        4,
    );
    let columns = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .spacing(1)
    .split(cards);
    for (column, (title, key, description, color)) in columns.iter().zip([
        ("Model", "Alt+M", "Choose your model", theme::accent()),
        ("Sessions", "Alt+S", "Pick up a thread", theme::user()),
        ("Commands", "/", "Explore actions", theme::accent_warm()),
    ]) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::border()))
            .style(theme::surface())
            .padding(Padding::horizontal(1));
        let inner = block.inner(*column);
        frame.render_widget(block, *column);
        let key_width = key.len();
        let label = task::fit(title, (inner.width as usize).saturating_sub(key_width + 1));
        let gap = (inner.width as usize).saturating_sub(label.len() + key_width);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(label, Style::default().fg(color).bold()),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(key, Style::default().fg(color)),
                ]),
                Line::styled(
                    task::fit(description, inner.width as usize),
                    Style::default().fg(theme::muted()),
                ),
            ]),
            inner,
        );
    }
}
