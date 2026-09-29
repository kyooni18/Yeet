//! One-row tab bar shared by the session and files views: home, the session
//! tab, one tab per opened file, and a trailing `+`.
use super::{icons, shell::conversation_title, task::fit, theme};
use crate::app::App;
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Modifier, Span, Style},
    widgets::{Block, Paragraph},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Active {
    Home,
    Session,
    File(usize),
}

const NARROW: u16 = 70;

pub(super) fn draw(frame: &mut Frame<'_>, app: &App, area: Rect, active: Active) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Block::default().style(theme::surface()), area);
    let narrow = area.width < NARROW;
    if narrow && active == Active::Session {
        draw_title(frame, app, area);
        return;
    }
    let tab_width = if narrow {
        area.width.saturating_sub(10) as usize
    } else {
        26
    };

    let home_style = if active == Active::Home {
        Style::default()
            .fg(theme::muted())
            .bg(theme::surface_raised())
    } else {
        Style::default().fg(theme::muted())
    };
    let mut spans = vec![
        Span::styled(format!(" {} ", icons::home()), home_style),
        Span::styled(if area.width >= 100 { "Home " } else { "" }, home_style),
        Span::styled("│", Style::default().fg(theme::border_dim())),
    ];
    let mut push_tab = |icon: &str, label: &str, is_active: bool| {
        let style = if is_active {
            Style::default()
                .fg(theme::text())
                .bg(theme::surface_raised())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::muted())
        };
        let icon_style = if is_active {
            style.fg(theme::muted()).remove_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::muted())
        };
        spans.push(Span::styled(format!(" {icon} "), icon_style));
        spans.push(Span::styled(fit(label, tab_width), style));
        spans.push(Span::styled(" ", style));
    };

    let session_title = conversation_title(app).to_owned();
    if !narrow || active == Active::Session {
        push_tab(
            icons::session_tab(),
            &session_title,
            active == Active::Session,
        );
    }
    if let Some(files) = app.files.as_ref() {
        for (index, path) in files.tabs.iter().enumerate() {
            let is_active = active == Active::File(index);
            if narrow && !is_active {
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            push_tab(icons::file(&name), &name, is_active);
        }
    }
    spans.push(Span::styled(" + ", Style::default().fg(theme::muted())));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Portrait session header: bold title left, clock right.
fn draw_title(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let clock = chrono::Local::now().format("%H:%M").to_string();
    let title = fit(
        conversation_title(app),
        (area.width as usize).saturating_sub(clock.len() + 3),
    );
    let gap = (area.width as usize).saturating_sub(1 + Span::raw(&title).width() + clock.len() + 1);
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            title,
            Style::default()
                .fg(theme::text())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(gap)),
        Span::styled(clock, Style::default().fg(theme::muted())),
        Span::raw(" "),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
