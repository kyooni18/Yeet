//! Responsive application shell around the conversation.
use super::{
    responsive,
    task::{self, TaskStatus},
    theme, truncate_end, yeet_brand,
};
use crate::{app::App, model::SessionSummary};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Padding, Paragraph},
};

pub(super) fn draw_shell(frame: &mut Frame<'_>, app: &App) -> Rect {
    let bounds = frame.area();
    let adaptive = responsive::metrics(bounds);
    let body = if let Some(sidebar_width) = adaptive.sidebar_width {
        let columns = Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(1)])
            .split(bounds);
        sidebar(frame, app, columns[0]);
        columns[1]
    } else {
        bounds
    };
    let rows = Layout::vertical([
        Constraint::Length(adaptive.header_height),
        Constraint::Min(1),
    ])
    .split(body);
    let title = conversation_title(app);
    let task_status = TaskStatus::for_app(app);
    let header_border = if matches!(task_status, TaskStatus::Working | TaskStatus::Approval) {
        theme::pulse_color()
    } else {
        theme::BORDER_DIM
    };
    let header = if rows[0].height > 1 {
        Block::default().borders(Borders::BOTTOM)
    } else {
        Block::default()
    }
    .style(theme::surface())
    .border_style(Style::default().fg(header_border))
    .padding(Padding::horizontal(2));
    let inner = header.inner(rows[0]);
    frame.render_widget(header, rows[0]);
    let badge = task_status.badge(app);
    let badge_width = badge.width() as u16;
    let show_badge = inner.width >= badge_width + 16;
    let title_width = inner
        .width
        .saturating_sub(if show_badge { badge_width + 2 } else { 0 });
    let header_title = if title == "New conversation" {
        "◢ YEET // NEW DIRECTIVE".to_owned()
    } else {
        format!("◢ YEET // {title}")
    };
    frame.render_widget(
        Paragraph::new(task::fit(&header_title, title_width as usize)).style(theme::brand()),
        Rect::new(inner.x, inner.y, title_width, inner.height.min(1)),
    );
    if show_badge {
        frame.render_widget(
            Paragraph::new(Line::from(badge)),
            Rect::new(inner.right() - badge_width, inner.y, badge_width, 1),
        );
    }
    if inner.height > 1 {
        let location = if app.follow_tail || app.conversation.is_empty() {
            "latest"
        } else {
            "history · Ctrl+End latest"
        };
        let workspace = current_workspace_name(app);
        let session = current_session_context(app);
        let context = format!("{workspace}  ╱  {session}  ╱  {location}");
        let available = inner.width.saturating_sub(2) as usize;
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("◆ ", Style::default().fg(theme::ACCENT_WARM).bold()),
                Span::styled(
                    task::fit(&context, available),
                    Style::default().fg(theme::MUTED),
                ),
            ])),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
    }
    let available = rows[1].inner(Margin {
        horizontal: adaptive.horizontal_margin.min(rows[1].width / 2),
        vertical: 0,
    });
    let width = available.width.min(adaptive.content_max_width);
    Rect::new(
        available.x + (available.width - width) / 2,
        available.y,
        width,
        available.height,
    )
}

fn sidebar(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let block = Block::default()
        .style(theme::surface())
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(theme::BORDER))
        .padding(Padding::new(1, 1, 1, 1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(1),
        Constraint::Length(5),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("◢ YEET", theme::brand()),
                Span::styled(
                    " // CONTROL",
                    Style::default().fg(theme::ACCENT_WARM).bold(),
                ),
            ]),
            Line::from(vec![
                Span::styled("WORKSPACES", Style::default().fg(theme::TEXT).bold()),
                Span::styled("  ·  RECENT", Style::default().fg(theme::MUTED)),
            ]),
            shortcut("New session", "Ctrl+N", inner.width),
            shortcut("Sessions", "Alt+S", inner.width),
        ]),
        rows[0],
    );
    let nav_rows = sidebar_rows(app);
    let selected = nav_rows
        .iter()
        .position(|row| matches!(row.kind, SidebarRowKind::Session { current: true }));
    let items = nav_rows
        .iter()
        .map(|row| sidebar_item(app, row, inner.width))
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(
        List::new(items).highlight_style(theme::selected()),
        rows[1],
        &mut state,
    );
    let footer = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme::BORDER_DIM));
    frame.render_widget(
        Paragraph::new(vec![
            shortcut("Models", "Alt+M", inner.width),
            shortcut("Settings", "/settings", inner.width),
            shortcut("Help", "?", inner.width),
        ])
        .block(footer),
        rows[2],
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidebarRow {
    label: String,
    detail: String,
    kind: SidebarRowKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarRowKind {
    Workspace { current: bool },
    Session { current: bool },
}

fn sidebar_rows(app: &App) -> Vec<SidebarRow> {
    let mut rows = Vec::new();
    let mut current_workspace_seen = false;

    for workspace in &app.state.known_workspaces {
        rows.push(SidebarRow {
            label: workspace_display_name(&workspace.display_name, &workspace.path),
            detail: session_count_label(workspace.session_count),
            kind: SidebarRowKind::Workspace {
                current: workspace.is_current,
            },
        });
        let grouped_sessions = app
            .state
            .workspace_session_groups
            .iter()
            .find(|group| group.workspace_id == workspace.id)
            .map(|group| group.sessions.as_slice())
            .unwrap_or_default();
        if workspace.is_current {
            current_workspace_seen = true;
            let sessions = if grouped_sessions.is_empty() {
                app.sessions()
            } else {
                grouped_sessions
            };
            append_workspace_sessions(app, sessions, true, &mut rows);
        } else {
            append_workspace_sessions(app, grouped_sessions, false, &mut rows);
        }
    }

    if !current_workspace_seen {
        rows.insert(
            0,
            SidebarRow {
                label: fallback_workspace_name(),
                detail: session_count_label(app.sessions().len()),
                kind: SidebarRowKind::Workspace { current: true },
            },
        );
        let mut sessions = Vec::new();
        append_workspace_sessions(app, app.sessions(), true, &mut sessions);
        rows.splice(1..1, sessions);
    }

    rows
}

fn append_workspace_sessions(
    app: &App,
    sessions: &[SessionSummary],
    current_workspace: bool,
    rows: &mut Vec<SidebarRow>,
) {
    if !current_workspace {
        rows.extend(
            sessions
                .iter()
                .take(1)
                .map(|session| session_row(session, false, app)),
        );
        return;
    }

    let current_index = sessions
        .iter()
        .position(|session| Some(session.id.as_str()) == app.state.current_session_id.as_deref());
    if let Some(index) = current_index {
        rows.push(session_row(&sessions[index], true, app));
        rows.extend(
            sessions
                .iter()
                .enumerate()
                .filter(|(candidate, _)| *candidate != index)
                .take(2)
                .map(|(_, session)| session_row(session, false, app)),
        );
    } else {
        rows.push(SidebarRow {
            label: conversation_title(app).to_owned(),
            detail: format!("Current · {}", TaskStatus::for_app(app).label()),
            kind: SidebarRowKind::Session { current: true },
        });
        rows.extend(
            sessions
                .iter()
                .take(2)
                .map(|session| session_row(session, false, app)),
        );
    }
}

fn session_row(session: &SessionSummary, current: bool, app: &App) -> SidebarRow {
    SidebarRow {
        label: session.title.clone(),
        detail: if current {
            format!("Current · {}", TaskStatus::for_app(app).label())
        } else {
            format!("{}m", session.message_count)
        },
        kind: SidebarRowKind::Session { current },
    }
}

fn sidebar_item(app: &App, row: &SidebarRow, width: u16) -> ListItem<'static> {
    let (prefix, label_style, detail_style) = match row.kind {
        SidebarRowKind::Workspace { current: true } => (
            "▾ ",
            Style::default().fg(theme::TEXT).bold(),
            Style::default().fg(theme::ACCENT),
        ),
        SidebarRowKind::Workspace { current: false } => (
            "▸ ",
            Style::default().fg(theme::MUTED),
            Style::default().fg(theme::MUTED),
        ),
        SidebarRowKind::Session { current: true } => (
            TaskStatus::for_app(app).marker(app),
            Style::default().fg(theme::TEXT).bold(),
            Style::default().fg(TaskStatus::for_app(app).color()),
        ),
        SidebarRowKind::Session { current: false } => (
            "·",
            Style::default().fg(theme::TEXT),
            Style::default().fg(theme::MUTED),
        ),
    };
    let indent = if matches!(row.kind, SidebarRowKind::Session { .. }) {
        "  "
    } else {
        ""
    };
    let label = format!("{indent}{prefix}{}", row.label);
    ListItem::new(aligned_line(
        &label,
        &row.detail,
        width,
        label_style,
        detail_style,
    ))
}

fn aligned_line(
    label: &str,
    detail: &str,
    width: u16,
    label_style: Style,
    detail_style: Style,
) -> Line<'static> {
    let detail_width = Span::raw(detail).width();
    let label = truncate_end(label, (width as usize).saturating_sub(detail_width + 1));
    let gap = (width as usize)
        .saturating_sub(Span::raw(&label).width() + detail_width)
        .max(1);
    Line::from(vec![
        Span::styled(label, label_style),
        Span::raw(" ".repeat(gap)),
        Span::styled(detail.to_owned(), detail_style),
    ])
}

fn current_workspace_name(app: &App) -> String {
    app.state
        .known_workspaces
        .iter()
        .find(|workspace| workspace.is_current)
        .map(|workspace| workspace_display_name(&workspace.display_name, &workspace.path))
        .unwrap_or_else(fallback_workspace_name)
}

fn current_session_context(app: &App) -> String {
    match app.state.current_session_id.as_deref() {
        Some(id) => format!("session {}", truncate_end(id, 10)),
        None => "new session".to_owned(),
    }
}

fn workspace_display_name(display_name: &str, path: &str) -> String {
    if !display_name.trim().is_empty() {
        return display_name.trim().to_owned();
    }
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_owned())
}

fn fallback_workspace_name() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Workspace".into())
}

fn session_count_label(count: usize) -> String {
    match count {
        1 => "1 session".to_owned(),
        count => format!("{count} sessions"),
    }
}

fn conversation_title(app: &App) -> &str {
    app.sessions()
        .iter()
        .find(|session| Some(session.id.as_str()) == app.state.current_session_id.as_deref())
        .map(|session| session.title.as_str())
        .or_else(|| {
            app.conversation.iter().find_map(|entry| match &entry.kind {
                crate::model::ConversationKind::User { content } => Some(content.as_str()),
                _ => None,
            })
        })
        .unwrap_or("New conversation")
}

fn shortcut(label: &str, key: &str, width: u16) -> Line<'static> {
    let key_width = Span::raw(key).width();
    let label = truncate_end(label, (width as usize).saturating_sub(key_width + 1));
    let gap = (width as usize).saturating_sub(Span::raw(&label).width() + key_width);
    Line::from(vec![
        Span::raw(label),
        Span::raw(" ".repeat(gap)),
        Span::styled(key.to_owned(), Style::default().fg(theme::MUTED)),
    ])
}

pub(super) fn draw_welcome(frame: &mut Frame<'_>, area: Rect, viewport_shape: responsive::Shape) {
    yeet_brand::draw(frame, area, viewport_shape);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SessionSummary, WorkspaceSessionGroup, WorkspaceSummary};

    #[test]
    fn workspace_catalog_keeps_all_workspaces_and_nests_current_sessions() {
        let mut app = App::default();
        app.state.known_workspaces = vec![
            WorkspaceSummary {
                id: "one".into(),
                path: "/tmp/one".into(),
                display_name: "One".into(),
                updated_at: None,
                session_count: 2,
                is_current: true,
            },
            WorkspaceSummary {
                id: "two".into(),
                path: "/tmp/two".into(),
                display_name: "Two".into(),
                updated_at: None,
                session_count: 4,
                is_current: false,
            },
        ];
        app.state.workspace_session_groups = vec![
            WorkspaceSessionGroup {
                workspace_id: "one".into(),
                sessions: vec![
                    SessionSummary {
                        id: "s1".into(),
                        title: "Current task".into(),
                        updated_at: String::new(),
                        model: "model".into(),
                        message_count: 3,
                    },
                    SessionSummary {
                        id: "s2".into(),
                        title: "Earlier task".into(),
                        updated_at: String::new(),
                        model: "model".into(),
                        message_count: 8,
                    },
                ],
            },
            WorkspaceSessionGroup {
                workspace_id: "two".into(),
                sessions: vec![SessionSummary {
                    id: "s3".into(),
                    title: "Other workspace task".into(),
                    updated_at: String::new(),
                    model: "model".into(),
                    message_count: 5,
                }],
            },
        ];
        app.state.saved_sessions = vec![
            SessionSummary {
                id: "s1".into(),
                title: "Current task".into(),
                updated_at: String::new(),
                model: "model".into(),
                message_count: 3,
            },
            SessionSummary {
                id: "s2".into(),
                title: "Earlier task".into(),
                updated_at: String::new(),
                model: "model".into(),
                message_count: 8,
            },
        ];
        app.state.current_session_id = Some("s1".into());

        let rows = sidebar_rows(&app);

        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].label, "One");
        assert!(matches!(
            rows[0].kind,
            SidebarRowKind::Workspace { current: true }
        ));
        assert_eq!(rows[1].label, "Current task");
        assert!(matches!(
            rows[1].kind,
            SidebarRowKind::Session { current: true }
        ));
        assert_eq!(rows[2].label, "Earlier task");
        assert_eq!(rows[3].label, "Two");
        assert_eq!(rows[3].detail, "4 sessions");
        assert_eq!(rows[4].label, "Other workspace task");
    }

    #[test]
    fn workspace_catalog_keeps_unsaved_session_visible() {
        let mut app = App::default();
        app.state.known_workspaces = vec![WorkspaceSummary {
            id: "one".into(),
            path: "/tmp/one".into(),
            display_name: "One".into(),
            updated_at: None,
            session_count: 0,
            is_current: true,
        }];

        let rows = sidebar_rows(&app);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].label, "New conversation");
        assert!(matches!(
            rows[1].kind,
            SidebarRowKind::Session { current: true }
        ));
    }
}
