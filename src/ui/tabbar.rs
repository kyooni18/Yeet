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
    Files,
    File(usize),
}

const NARROW: u16 = 70;

pub(super) fn draw(frame: &mut Frame<'_>, app: &App, area: Rect, active: Active) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Block::default().style(theme::base().bg(theme::code_background())),
        area,
    );
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

    let mut spans = Vec::new();
    // Each tab is `  icon label` padded to a minimum width; the active tab is
    // raised onto the rail surface.
    let mut push_tab = |icon: &str, label: &str, is_active: bool, min_width: usize| {
        let (style, icon_style) = if is_active {
            let raised = Style::default().bg(theme::background());
            (
                raised.fg(theme::text()).add_modifier(Modifier::BOLD),
                raised.fg(theme::secondary()),
            )
        } else {
            let plain = Style::default().fg(theme::secondary());
            (plain, plain)
        };
        let label = fit(label, tab_width);
        let used = 4 + Span::raw(&label).width();
        spans.push(Span::styled(format!("  {icon} "), icon_style));
        spans.push(Span::styled(label, style));
        spans.push(Span::styled(
            " ".repeat(min_width.max(used + 4).saturating_sub(used)),
            style,
        ));
    };
    let (home_width, session_width) = if narrow { (0, 0) } else { (23, 35) };
    push_tab(
        icons::home(),
        if area.width >= 100 { "Home" } else { "" },
        active == Active::Home,
        home_width,
    );
    let session_title = conversation_title(app).to_owned();
    if !narrow || active == Active::Session {
        push_tab(
            icons::session_tab(),
            &session_title,
            active == Active::Session,
            session_width,
        );
    }
    if active == Active::Files {
        push_tab(icons::folder(false), "Files", true, 0);
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
            push_tab(icons::file(&name), &name, is_active, 0);
        }
    }
    spans.push(Span::styled("+ ", Style::default().fg(theme::muted())));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Portrait session header: bold title left, clock right.
fn draw_title(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let clock = app
        .state
        .saved_sessions
        .iter()
        .find(|session| Some(session.id.as_str()) == app.state.current_session_id.as_deref())
        .filter(|session| chrono::DateTime::parse_from_rfc3339(&session.updated_at).is_ok())
        .map(|session| session.updated_label().replace(" ago", ""))
        .unwrap_or_else(|| chrono::Local::now().format("%H:%M").to_string());
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
