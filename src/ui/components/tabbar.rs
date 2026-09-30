//! One-row tab bar shared by the session and files views: home, the session
//! tab, one tab per opened file, and a trailing `+`.
use super::super::{
    shell::conversation_title,
    support::{icons, theme},
    task::fit,
};
use crate::app::App;
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Modifier, Span, Style},
    widgets::{Block, Paragraph},
};

pub(crate) use crate::app::WorkbenchTab as Active;
const NARROW: u16 = 70;

struct Tab {
    area: Rect,
    target: Active,
    text: String,
}

fn layout(app: &App, area: Rect, active: Active) -> Vec<Tab> {
    if area.width < 4 || area.height == 0 || (area.width < NARROW && active == Active::Session) {
        return Vec::new();
    }
    let mut entries: Vec<(Active, String, u16)> = app
        .workbench_tabs()
        .into_iter()
        .map(|target| {
            let (icon, label) = match target {
                Active::Home => (icons::home(), "Home".to_owned()),
                Active::Session => (icons::session_tab(), conversation_title(app).to_owned()),
                Active::Files => (icons::folder(false), "Files".to_owned()),
                Active::File(index) => {
                    let label = app
                        .files
                        .as_ref()
                        .and_then(|files| files.tabs.get(index))
                        .and_then(|path| path.file_name())
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    (
                        if target == active && app.files.as_ref().is_some_and(|files| files.diff) {
                            icons::diff_tab()
                        } else {
                            icons::file(&label)
                        },
                        label,
                    )
                }
                _ => unreachable!(),
            };
            let label = fit(&label, 26);
            let text = format!(" {icon} {label} ");
            let width = (Span::raw(&text).width() as u16
                + if matches!(target, Active::File(_)) {
                    2
                } else {
                    0
                })
            .max(if target == Active::Home {
                (area.width as u32 * 160 / 1440) as u16
            } else {
                0
            });
            (target, text, width)
        })
        .collect();
    if area.width < NARROW {
        entries.retain(|(target, _, _)| *target == active || *target == Active::Home);
    }
    let available = area.width.saturating_sub(4);
    while entries
        .iter()
        .map(|(_, _, width)| *width as u32)
        .sum::<u32>()
        > available as u32
        && entries.len() > 1
    {
        let Some(index) = entries
            .iter()
            .rposition(|(target, _, _)| *target != active && *target != Active::Home)
        else {
            break;
        };
        entries.remove(index);
    }
    let mut x = area.x;
    let mut tabs = Vec::new();
    for (target, text, requested) in entries {
        let remaining = area.x + available - x;
        let width = requested.min(remaining);
        if width == 0 {
            break;
        }
        tabs.push(Tab {
            area: Rect::new(x, area.y, width, area.height),
            target,
            text,
        });
        x += width;
    }
    tabs.push(Tab {
        area: Rect::new(
            x,
            area.y,
            4.min(area.right().saturating_sub(x)),
            area.height,
        ),
        target: Active::Launcher,
        text: " + ".into(),
    });
    tabs
}

pub(crate) fn targets(app: &App, area: Rect, active: Active) -> Vec<(Rect, Active)> {
    layout(app, area, active)
        .into_iter()
        .flat_map(|tab| {
            let mut targets = vec![(tab.area, tab.target)];
            if let Active::File(index) = tab.target {
                targets.insert(
                    0,
                    (
                        Rect::new(
                            tab.area.right().saturating_sub(2),
                            tab.area.y,
                            2,
                            tab.area.height,
                        ),
                        Active::CloseFile(index),
                    ),
                );
            }
            targets
        })
        .collect()
}

pub(crate) fn draw(frame: &mut Frame<'_>, app: &App, area: Rect, active: Active) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Block::default().style(theme::base().bg(theme::code_background())),
        area,
    );
    if area.width < NARROW && active == Active::Session {
        draw_title(frame, app, area);
        return;
    }
    for tab in layout(app, area, active) {
        let selected = tab.target == active;
        let style = if selected {
            Style::default()
                .fg(theme::text())
                .bg(theme::surface_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme::muted())
                .bg(theme::code_background())
        };
        frame.render_widget(Block::default().style(style), tab.area);
        let text_area = Rect::new(
            tab.area.x,
            tab.area.y,
            tab.area
                .width
                .saturating_sub(if matches!(tab.target, Active::File(_)) {
                    2
                } else {
                    0
                }),
            1,
        );
        frame.render_widget(Paragraph::new(tab.text).style(style), text_area);
        if matches!(tab.target, Active::File(_)) {
            frame.render_widget(
                Paragraph::new("×").style(style.fg(theme::muted())),
                Rect::new(tab.area.right().saturating_sub(2), tab.area.y, 1, 1),
            );
        }
    }
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
