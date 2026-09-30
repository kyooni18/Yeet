//! Responsive application shell around the conversation.
use super::{
    components::tabbar,
    support::{responsive, text::truncate_end, theme, yeet_brand},
    task::{self, TaskStatus},
};
use crate::{app::App, model::SessionSummary};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    prelude::*,
    widgets::{Block, List, ListItem, ListState, Padding, Paragraph},
};

const MIN_MAIN_PANE_WIDTH_WITH_SIDEBAR: u16 = 96;
type SidebarArea = (u16, u16, u16, u16);
type SidebarSessionTarget = (u16, String);

/// Area under the tab bar that holds the rail and the conversation.
fn below_tabbar(bounds: Rect, adaptive: responsive::Metrics) -> Rect {
    let header = adaptive.header_height.min(bounds.height);
    Rect::new(
        bounds.x,
        bounds.y + header,
        bounds.width,
        bounds.height - header,
    )
}

/// Draws tabs and rail; returns the full conversation pane and its centered
/// reading column.
pub(super) fn draw_shell(frame: &mut Frame<'_>, app: &App, bounds: Rect) -> (Rect, Rect) {
    let adaptive = responsive::metrics(frame.area());
    let header_area = Rect::new(
        bounds.x,
        bounds.y,
        bounds.width,
        adaptive.header_height.min(bounds.height),
    );
    tabbar::draw(frame, app, header_area, tabbar::Active::Session);
    let rest = below_tabbar(bounds, adaptive);
    let sidebar_width = shell_sidebar_width(frame.area(), adaptive);
    let body = if let Some(sidebar_width) = sidebar_width {
        let columns =
            Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(1)]).split(rest);
        sidebar(frame, app, columns[0]);
        columns[1]
    } else {
        rest
    };
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::code_background())),
        body,
    );
    let available = body.inner(Margin {
        horizontal: adaptive.horizontal_margin.min(body.width / 2),
        vertical: 0,
    });
    let content_width = available.width.min(adaptive.content_max_width);
    let content = Rect::new(
        available.x + (available.width - content_width) / 2,
        available.y,
        content_width,
        available.height,
    );
    (body, content)
}

fn shell_sidebar_width(bounds: Rect, adaptive: responsive::Metrics) -> Option<u16> {
    adaptive.sidebar_width.filter(|sidebar_width| {
        bounds.width.saturating_sub(*sidebar_width) >= MIN_MAIN_PANE_WIDTH_WITH_SIDEBAR
    })
}

fn sidebar_block() -> Block<'static> {
    Block::default()
        .style(theme::base())
        .padding(Padding::new(1, 1, 1, 0))
}

fn sidebar_sections(inner: Rect) -> [Rect; 2] {
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(inner);
    [rows[0], rows[1]]
}

fn sidebar_selected_index(rows: &[SidebarRow]) -> Option<usize> {
    rows.iter()
        .position(|row| matches!(row.kind, SidebarRowKind::Session { current: true }))
}

fn sidebar_list_offset(selected: Option<usize>, row_count: usize, height: u16) -> usize {
    let visible = height as usize;
    if visible == 0 || row_count <= visible {
        return 0;
    }
    let selected = selected.unwrap_or(0).min(row_count.saturating_sub(1));
    selected
        .saturating_add(1)
        .saturating_sub(visible)
        .min(row_count.saturating_sub(visible))
}

pub(super) fn sidebar_session_targets(
    app: &App,
    bounds: Rect,
) -> (SidebarArea, Vec<SidebarSessionTarget>) {
    let adaptive = responsive::metrics(bounds);
    let Some(sidebar_width) = shell_sidebar_width(bounds, adaptive) else {
        return ((0, 0, 0, 0), Vec::new());
    };
    let rest = below_tabbar(bounds, adaptive);
    let area = Rect::new(rest.x, rest.y, sidebar_width, rest.height);
    let inner = sidebar_block().inner(area);
    let sections = sidebar_sections(inner);
    let list_area = sections[1];
    let rows = sidebar_rows(app);
    let offset = sidebar_list_offset(sidebar_selected_index(&rows), rows.len(), list_area.height);
    let targets = rows
        .iter()
        .skip(offset)
        .take(list_area.height as usize)
        .enumerate()
        .filter_map(|(visible_index, row)| match row.kind {
            SidebarRowKind::Session { current: false } => row
                .session_id
                .as_ref()
                .map(|id| (list_area.y.saturating_add(visible_index as u16), id.clone())),
            _ => None,
        })
        .collect();
    (
        (list_area.x, list_area.y, list_area.width, list_area.height),
        targets,
    )
}

pub(super) fn sidebar_nav_ids(app: &App) -> Vec<Option<String>> {
    sidebar_rows(app)
        .into_iter()
        .map(|row| row.session_id)
        .collect()
}

fn sidebar(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let block = sidebar_block();
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = sidebar_sections(inner);
    frame.render_widget(Paragraph::new(sidebar_header(inner.width)), rows[0]);
    let nav_rows = sidebar_rows(app);
    let selected = if app.sidebar_focus {
        Some(app.sidebar_cursor.min(nav_rows.len().saturating_sub(1)))
    } else {
        sidebar_selected_index(&nav_rows)
    };
    let offset = sidebar_list_offset(selected, nav_rows.len(), rows[1].height);
    let items = nav_rows
        .iter()
        .map(|row| sidebar_item(app, row, inner.width))
        .collect::<Vec<_>>();
    let mut state = ListState::default()
        .with_offset(offset)
        .with_selected(selected);
    frame.render_stateful_widget(
        List::new(items).highlight_style(
            Style::default()
                .bg(theme::rail_selected())
                .add_modifier(Modifier::BOLD),
        ),
        rows[1],
        &mut state,
    );
}

/// A single raised `+` row for starting a new session.
fn sidebar_header(width: u16) -> Vec<Line<'static>> {
    let width = width as usize;
    let left = width.saturating_sub(1) / 2;
    let plus = format!(
        "{}+{}",
        " ".repeat(left),
        " ".repeat(width.saturating_sub(left + 1))
    );
    vec![
        Line::styled(
            plus,
            Style::default()
                .fg(theme::text())
                .bg(theme::surface_color())
                .bold(),
        ),
        Line::default(),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidebarRow {
    label: String,
    detail: String,
    kind: SidebarRowKind,
    session_id: Option<String>,
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
            session_id: None,
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
                session_id: None,
            },
        );
        let mut sessions = Vec::new();
        append_workspace_sessions(app, app.sessions(), true, &mut sessions);
        rows.splice(1..1, sessions);
    }

    rows.retain(|row| matches!(row.kind, SidebarRowKind::Session { .. }));
    rows
}

fn append_workspace_sessions(
    app: &App,
    sessions: &[SessionSummary],
    current_workspace: bool,
    rows: &mut Vec<SidebarRow>,
) {
    if !current_workspace {
        return;
    }

    let current_index = sessions
        .iter()
        .position(|session| Some(session.id.as_str()) == app.state.current_session_id.as_deref());
    if let Some(index) = current_index {
        rows.push(session_row(&sessions[index], true));
        rows.extend(
            sessions
                .iter()
                .enumerate()
                .filter(|(candidate, _)| *candidate != index)
                .take(4)
                .map(|(_, session)| session_row(session, false)),
        );
    } else {
        rows.push(SidebarRow {
            label: conversation_title(app).to_owned(),
            detail: String::new(),
            kind: SidebarRowKind::Session { current: true },
            session_id: None,
        });
        rows.extend(
            sessions
                .iter()
                .take(4)
                .map(|session| session_row(session, false)),
        );
    }
}

fn session_row(session: &SessionSummary, current: bool) -> SidebarRow {
    SidebarRow {
        label: session.display_title(),
        detail: if chrono::DateTime::parse_from_rfc3339(&session.updated_at).is_ok() {
            session.updated_label().replace(" ago", "")
        } else {
            format!("{} msg", session.message_count)
        },
        kind: SidebarRowKind::Session { current },
        session_id: Some(session.id.clone()),
    }
}

fn sidebar_item(app: &App, row: &SidebarRow, width: u16) -> ListItem<'static> {
    let (prefix, marker_style, label_style, detail_style) = match row.kind {
        SidebarRowKind::Workspace { current: true } => (
            "▾ ".to_owned(),
            Style::default().fg(theme::accent_warm()),
            Style::default().fg(theme::text()).bold(),
            Style::default().fg(theme::muted()),
        ),
        SidebarRowKind::Workspace { current: false } => (
            "▸ ".to_owned(),
            Style::default().fg(theme::muted()),
            Style::default().fg(theme::text_dim()).bold(),
            Style::default().fg(theme::muted()),
        ),
        SidebarRowKind::Session { current: true } => (
            format!(" {} ", session_marker(app)),
            Style::default().fg(match TaskStatus::for_app(app) {
                TaskStatus::Working => theme::text(),
                status => status.color(),
            }),
            Style::default().fg(theme::text()).bold(),
            Style::default().fg(theme::muted()),
        ),
        SidebarRowKind::Session { current: false } => (
            " · ".to_owned(),
            Style::default().fg(theme::muted()),
            Style::default().fg(theme::secondary()),
            Style::default().fg(theme::muted()),
        ),
    };
    // Keep the navigation marker distinct from the title, without changing row
    // heights or the shared geometry used by session click targets.
    let prefix = truncate_end(&prefix, width as usize);
    let remaining = width.saturating_sub(Span::raw(&prefix).width() as u16);
    let mut line = aligned_line(
        &row.label,
        &row.detail,
        remaining,
        label_style,
        detail_style,
    );
    line.spans.insert(0, Span::styled(prefix, marker_style));
    ListItem::new(line)
}

/// The live session spins like the transcript's active tool group.
fn session_marker(app: &App) -> &'static str {
    match TaskStatus::for_app(app) {
        TaskStatus::Working => super::views::sessions::tools::spinner(),
        status => status.marker(app),
    }
}

fn aligned_line(
    label: &str,
    detail: &str,
    width: u16,
    label_style: Style,
    detail_style: Style,
) -> Line<'static> {
    let width = width as usize;
    // Keep metadata from crowding the session name out of narrow sidebars.
    // Reserve at least half the row for the label and keep the detail flush right.
    let detail = task::fit(detail, width.saturating_sub(1) / 2);
    let detail_width = Span::raw(&detail).width();
    let separator = usize::from(detail_width > 0);
    let label = task::fit(label, width.saturating_sub(detail_width + separator));
    let gap = width.saturating_sub(Span::raw(&label).width() + detail_width);
    Line::from(vec![
        Span::styled(label, label_style),
        Span::raw(" ".repeat(gap)),
        Span::styled(detail, detail_style),
    ])
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

pub(super) fn conversation_title(app: &App) -> &str {
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

pub(super) fn draw_welcome(frame: &mut Frame<'_>, area: Rect, viewport_shape: responsive::Shape) {
    yeet_brand::draw(frame, area, viewport_shape);
}

#[cfg(test)]
mod polish_tests {
    use super::*;

    #[test]
    fn sidebar_header_is_a_single_centered_plus_row() {
        for width in 1..40 {
            let lines = sidebar_header(width);
            assert_eq!(lines.len(), 2);
            assert_eq!(lines[0].width(), width as usize);
            assert_eq!(lines[0].to_string().trim(), "+");
        }
    }

    #[test]
    fn sidebar_labels_cannot_break_the_single_line_hit_targets() {
        let line = aligned_line(
            "Fix\n the\t sidebar",
            "3m",
            25,
            theme::base(),
            theme::brand(),
        );
        assert!(line.to_string().starts_with("Fix the sidebar"));
        assert_eq!(line.width(), 25);
    }

    #[test]
    fn sidebar_rows_fit_and_keep_both_columns_readable() {
        for width in 0..80 {
            for (label, detail) in [("  · Conversation", "21m"), ("修正界面", "昨日")] {
                let line = aligned_line(label, detail, width, theme::base(), theme::brand());
                assert!(line.width() <= width as usize, "width {width}: {line:?}");
            }
        }
        let line = aligned_line("Session", "2m", 24, theme::base(), theme::brand());
        assert_eq!(line.to_string(), "Session               2m");
    }
}
