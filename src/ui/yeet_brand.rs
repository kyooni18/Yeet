//! Responsive terminal wordmark and keyboard-first starting points.
use super::{responsive, task, theme};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    prelude::*,
    widgets::{Block, Borders, Padding, Paragraph},
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
            .enumerate()
            .map(|(index, line)| {
                let color = match index {
                    0 | 1 => theme::accent_hot(),
                    _ => theme::accent(),
                };
                Line::styled(*line, Style::default().fg(color).bold())
            })
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
            task::fit("Describe the outcome. Stay in flow.", area.width as usize),
            Style::default().fg(theme::muted()),
        ));
    }
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), content);
    if !expanded {
        return;
    }

    let cards_width = area.width.min(84);
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
    .spacing(2)
    .split(cards);
    for (column, (title, key, description, color)) in columns.iter().zip([
        ("Model", "Alt+M", "Choose your model", theme::accent()),
        ("Sessions", "Alt+S", "Pick up a thread", theme::accent_hot()),
        ("Commands", "/", "Explore actions", theme::accent()),
    ]) {
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(color))
            .style(theme::modal_surface())
            .padding(Padding::horizontal(1));
        let inner = block.inner(*column);
        frame.render_widget(block, *column);
        let key_width = Span::raw(key).width();
        let label = task::fit(title, (inner.width as usize).saturating_sub(key_width + 2));
        let gap = (inner.width as usize).saturating_sub(Span::raw(&label).width() + key_width);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled("› ", Style::default().fg(color).bold()),
                    Span::styled(label, Style::default().fg(theme::text()).bold()),
                    Span::raw(" ".repeat(gap.saturating_sub(2))),
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
