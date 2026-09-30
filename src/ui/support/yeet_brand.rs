//! Responsive terminal wordmark and wordmark.
use super::{responsive, theme};
use crate::ui::task;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    prelude::*,
    widgets::Paragraph,
};

const WORDMARK: [&str; 4] = [
    "█   █  █▀▀▀  █▀▀▀  ▀▀█▀▀",
    "▀█ █▀  █▄▄   █▄▄     █  ",
    "  █    █     █       █  ",
    "  ▀    ▀▀▀▀  ▀▀▀▀    ▀  ",
];

pub(in crate::ui) fn draw(frame: &mut Frame<'_>, area: Rect, _shape: responsive::Shape) {
    if area.is_empty() {
        return;
    }
    let wordmark = area.width >= 30 && area.height >= 9;
    let height = if wordmark { 8 } else { 3 };
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
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), content);
}
