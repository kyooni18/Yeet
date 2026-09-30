//! Full-screen session picker for portrait terminals.
use super::super::{components::status, task::fit, theme};
use crate::app::App;
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Modifier, Span, Style},
    widgets::{Block, Paragraph},
};

enum Row {
    Blank,
    Divider,
    Workspace(String),
    Session {
        index: usize,
        title: String,
        age: String,
        current: bool,
    },
    SessionPadding {
        index: usize,
    },
    NewSession {
        index: usize,
    },
    NewSessionPadding {
        index: usize,
    },
}

pub(in crate::ui) fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(theme::base()), area);
    if area.width == 0 || area.height < 3 {
        return;
    }
    let top = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(
        Block::default().style(theme::base().bg(theme::code_background())),
        top,
    );
    if !app.popup_filter.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!(
                    " Filter: {}",
                    fit(&app.popup_filter, area.width.saturating_sub(9) as usize)
                ),
                Style::default().fg(theme::text()),
            )),
            top,
        );
    }

    let items = app.filtered_session_picker_items();
    let mut rows = vec![Row::Blank, Row::Blank];
    let mut previous_workspace = String::new();
    for (index, item) in items.iter().enumerate() {
        if item.workspace_name != previous_workspace {
            if !previous_workspace.is_empty() {
                rows.push(Row::Blank);
                rows.push(Row::Divider);
                rows.push(Row::Blank);
            }
            previous_workspace = item.workspace_name.clone();
            rows.push(Row::Workspace(previous_workspace.clone()));
            rows.push(Row::Blank);
        }
        rows.push(Row::Session {
            index,
            title: item.session.display_title(),
            age: if chrono::DateTime::parse_from_rfc3339(&item.session.updated_at).is_ok() {
                item.session.updated_label().replace(" ago", "")
            } else {
                String::new()
            },
            current: app.state.current_session_id.as_deref() == Some(item.session.id.as_str()),
        });
        rows.push(Row::SessionPadding { index });
        rows.push(Row::Blank);
    }
    rows.push(Row::Blank);
    rows.push(Row::NewSession { index: items.len() });
    rows.push(Row::NewSessionPadding { index: items.len() });

    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(2),
    );
    let selected_row = rows
        .iter()
        .position(|row| match row {
            Row::Session { index, .. } | Row::NewSession { index } => *index == app.popup_index,
            _ => false,
        })
        .unwrap_or(0);
    let offset = selected_row
        .saturating_sub(body.height.saturating_sub(2) as usize)
        .min(rows.len().saturating_sub(body.height as usize));
    for (line_number, row) in rows
        .iter()
        .skip(offset)
        .take(body.height as usize)
        .enumerate()
    {
        draw_row(
            frame,
            row,
            Rect::new(body.x, body.y + line_number as u16, body.width, 1),
            app.popup_index,
        );
    }
    draw_status(
        frame,
        app,
        Rect::new(area.x, area.bottom() - 1, area.width, 1),
    );
}

fn draw_row(frame: &mut Frame<'_>, row: &Row, area: Rect, selected_index: usize) {
    match row {
        Row::Blank => {}
        Row::Divider => {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "─".repeat(area.width.saturating_sub(2) as usize),
                    Style::default().fg(theme::border_dim()),
                )),
                Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), 1),
            );
        }
        Row::Workspace(name) => {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    format!(" {}", fit(name, area.width.saturating_sub(2) as usize)),
                    Style::default().fg(theme::muted()),
                )),
                area,
            );
        }
        Row::Session {
            index,
            title,
            age,
            current,
        } => {
            let selected = *index == selected_index;
            if selected {
                frame.render_widget(
                    Block::default().style(Style::default().bg(theme::surface_color())),
                    area,
                );
            }
            let marker = if *current { "/" } else { "·" };
            let age_width = if area.width >= 24 {
                age.chars().count().min(8)
            } else {
                0
            };
            let title_width = (area.width as usize).saturating_sub(age_width + 8);
            let title = fit(title, title_width);
            let gap =
                (area.width as usize).saturating_sub(4 + Span::raw(&title).width() + age_width + 1);
            let style = Style::default()
                .fg(if selected {
                    theme::text()
                } else {
                    theme::text_dim()
                })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                });
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!(" {marker}  "), Style::default().fg(theme::muted())),
                    Span::styled(title, style),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(
                        if age_width > 0 { age.as_str() } else { "" },
                        Style::default().fg(theme::muted()),
                    ),
                    Span::raw(" "),
                ])),
                area,
            );
        }
        Row::SessionPadding { index } => {
            if *index == selected_index {
                frame.render_widget(
                    Block::default().style(Style::default().bg(theme::surface_color())),
                    area,
                );
            }
        }
        Row::NewSession { index } => {
            let selected = *index == selected_index;
            frame.render_widget(
                Block::default().style(Style::default().bg(if selected {
                    theme::selected_color()
                } else {
                    theme::surface_color()
                })),
                area,
            );
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "     New session",
                    Style::default()
                        .fg(theme::text())
                        .add_modifier(if selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                )),
                area,
            );
        }
        Row::NewSessionPadding { index } => {
            frame.render_widget(
                Block::default().style(Style::default().bg(if *index == selected_index {
                    theme::selected_color()
                } else {
                    theme::surface_color()
                })),
                area,
            );
        }
    }
}

fn draw_status(frame: &mut Frame<'_>, app: &App, area: Rect) {
    frame.render_widget(
        Block::default().style(theme::base().bg(theme::code_background())),
        area,
    );
    let current = app
        .state
        .known_workspaces
        .iter()
        .find(|workspace| workspace.is_current);
    let count = current
        .map(|workspace| workspace.session_count)
        .unwrap_or_else(|| app.sessions().len());
    let right = format!(
        "{count} {} ",
        if count == 1 { "session" } else { "sessions" }
    );
    let location = current
        .map(|workspace| {
            let path = std::path::Path::new(&workspace.path);
            if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from)
                && let Ok(relative) = path.strip_prefix(home)
            {
                format!("~/{}", relative.display())
            } else {
                workspace.path.clone()
            }
        })
        .unwrap_or_else(status::working_directory);
    let left = fit(
        &location,
        area.width.saturating_sub(right.len() as u16 + 2) as usize,
    );
    let gap = (area.width as usize).saturating_sub(1 + Span::raw(&left).width() + right.len());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {left}"), Style::default().fg(theme::muted())),
            Span::raw(" ".repeat(gap)),
            Span::styled(right, Style::default().fg(theme::muted())),
        ])),
        area,
    );
}
