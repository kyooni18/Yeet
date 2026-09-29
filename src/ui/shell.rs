//! Responsive application shell around the conversation.
use super::{
    responsive,
    task::{self, TaskStatus},
    text::truncate_end,
    theme, yeet_brand,
};
use crate::{app::App, model::SessionSummary};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Padding, Paragraph},
};

const MIN_MAIN_PANE_WIDTH_WITH_SIDEBAR: u16 = 96;
type SidebarArea = (u16, u16, u16, u16);
type SidebarSessionTarget = (u16, String);

pub(super) fn draw_shell(frame: &mut Frame<'_>, app: &App) -> Rect {
    let bounds = frame.area();
    let adaptive = responsive::metrics(bounds);
    let sidebar_width = shell_sidebar_width(bounds, adaptive);
    let body = if let Some(sidebar_width) = sidebar_width {
        let columns = Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(1)])
            .split(bounds);
        sidebar(frame, app, columns[0]);
        columns[1]
    } else {
        bounds
    };
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::code_background())),
        body,
    );
    let rows = Layout::vertical([
        Constraint::Length(adaptive.header_height),
        Constraint::Min(1),
    ])
    .split(body);

    let available = rows[1].inner(Margin {
        horizontal: adaptive.horizontal_margin.min(rows[1].width / 2),
        vertical: 0,
    });
    let content_width = available.width.min(adaptive.content_max_width);
    let content_area = Rect::new(
        available.x + (available.width - content_width) / 2,
        available.y,
        content_width,
        available.height,
    );
    let header_area = Rect::new(
        content_area.x,
        rows[0].y,
        content_area.width,
        rows[0].height,
    );

    let title = conversation_title(app);
    let task_status = TaskStatus::for_app(app);
    let header = Block::default()
        .style(theme::surface())
        .padding(Padding::horizontal(2));
    let inner = header.inner(header_area);
    frame.render_widget(header, header_area);
    let badge = format!(" {} {} ", task_status.marker(app), task_status.label());
    let badge_width = Span::raw(&badge).width() as u16;
    let show_badge = inner.width >= badge_width + 28;
    let title_width = if show_badge {
        inner.width.saturating_sub(badge_width + 2)
    } else {
        inner.width
    };
    frame.render_widget(
        Paragraph::new(header_title_line(
            title,
            title_width as usize,
            sidebar_width.is_none(),
        )),
        Rect::new(inner.x, inner.y, title_width, inner.height.min(1)),
    );
    if show_badge {
        frame.render_widget(
            Paragraph::new(badge).style(
                Style::default()
                    .fg(if task_status == TaskStatus::Approval {
                        theme::pulse_color()
                    } else {
                        task_status.color()
                    })
                    .bg(theme::surface_raised())
                    .bold(),
            ),
            Rect::new(
                inner.right() - badge_width,
                inner.y,
                badge_width,
                inner.height.min(1),
            ),
        );
    }
    if inner.height > 1 {
        let location = if app.follow_tail || app.conversation.is_empty() {
            "LATEST"
        } else {
            "HISTORY"
        };
        let context = current_workspace_name(app);
        let location_text = format!(" {location} ");
        let location_width = Span::raw(&location_text).width() as u16;
        let show_location = inner.width >= location_width.saturating_add(20);
        let context_width = if show_location {
            inner.width.saturating_sub(location_width + 2)
        } else {
            inner.width
        };
        let available = context_width.saturating_sub(2) as usize;
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("⌁ ", Style::default().fg(theme::user()).bold()),
                Span::styled(
                    task::fit(&context, available),
                    Style::default().fg(theme::muted()),
                ),
            ])),
            Rect::new(inner.x, inner.y + 1, context_width, 1),
        );
        if show_location {
            frame.render_widget(
                Paragraph::new(location_text).style(theme::selected()),
                Rect::new(
                    inner.right().saturating_sub(location_width),
                    inner.y + 1,
                    location_width,
                    1,
                ),
            );
        }
    }

    content_area
}

/// Keep the brand distinct from the conversation, and normalize user-supplied
/// titles so pasted newlines or tabs cannot disrupt the single-line header.
fn header_title_line(title: &str, width: usize, show_brand: bool) -> Line<'static> {
    if !show_brand {
        return Line::from(Span::styled(
            task::fit(title, width),
            Style::default().fg(theme::text()).bold(),
        ));
    }

    let brand = task::fit("Yeet", width);
    let remaining = width.saturating_sub(Span::raw(&brand).width());
    let mut spans = vec![Span::styled(brand, theme::brand())];
    if remaining > 3 {
        spans.push(Span::styled(" · ", Style::default().fg(theme::muted())));
        spans.push(Span::styled(
            task::fit(title, remaining - 3),
            Style::default().fg(theme::text()).bold(),
        ));
    }
    Line::from(spans)
}

fn shell_sidebar_width(bounds: Rect, adaptive: responsive::Metrics) -> Option<u16> {
    adaptive.sidebar_width.filter(|sidebar_width| {
        bounds.width.saturating_sub(*sidebar_width) >= MIN_MAIN_PANE_WIDTH_WITH_SIDEBAR
    })
}

fn sidebar_block() -> Block<'static> {
    Block::default()
        .style(theme::base())
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(theme::border_dim()))
        .padding(Padding::new(1, 1, 1, 1))
}

fn sidebar_sections(inner: Rect) -> [Rect; 3] {
    let rows = Layout::vertical([
        Constraint::Length(6),
        Constraint::Min(1),
        Constraint::Length(5),
    ])
    .split(inner);
    [rows[0], rows[1], rows[2]]
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
    let area = Rect::new(bounds.x, bounds.y, sidebar_width, bounds.height);
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

fn sidebar(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let block = sidebar_block();
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = sidebar_sections(inner);
    frame.render_widget(Paragraph::new(sidebar_header(inner.width)), rows[0]);
    let nav_rows = sidebar_rows(app);
    let selected = sidebar_selected_index(&nav_rows);
    let offset = sidebar_list_offset(selected, nav_rows.len(), rows[1].height);
    let items = nav_rows
        .iter()
        .map(|row| sidebar_item(app, row, inner.width))
        .collect::<Vec<_>>();
    let mut state = ListState::default()
        .with_offset(offset)
        .with_selected(selected);
    frame.render_stateful_widget(
        List::new(items).highlight_style(theme::selected()),
        rows[1],
        &mut state,
    );
    let footer = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme::border_dim()));
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

fn sidebar_header(width: u16) -> Vec<Line<'static>> {
    let brand_width = width as usize;
    let prefix = if brand_width >= 2 { "◈ " } else { "" };
    let prefix_width = Span::raw(prefix).width();
    let yeet = task::fit("YEET", brand_width.saturating_sub(prefix_width));
    let mut brand = Vec::new();
    if !prefix.is_empty() {
        brand.push(Span::styled(
            prefix,
            Style::default().fg(theme::accent_warm()).bold(),
        ));
    }
    let used = prefix_width + Span::raw(&yeet).width();
    brand.push(Span::styled(yeet, theme::brand()));
    if brand_width.saturating_sub(used) > 12 {
        brand.push(Span::styled(" / ", Style::default().fg(theme::border())));
        brand.push(Span::styled(
            task::fit("WORKBENCH", brand_width.saturating_sub(used + 3)),
            Style::default().fg(theme::muted()),
        ));
    }

    vec![
        Line::from(brand),
        Line::default(),
        sidebar_action("+", "New session", "Ctrl+N", theme::user(), width),
        sidebar_action("≡", "Sessions", "Alt+S", theme::accent(), width),
        Line::default(),
        sidebar_section("WORKSPACE", width),
    ]
}

fn sidebar_action(icon: &str, label: &str, key: &str, color: Color, width: u16) -> Line<'static> {
    let prefix = format!("{icon} ");
    let prefix = truncate_end(&prefix, width as usize);
    let remaining = width.saturating_sub(Span::raw(&prefix).width() as u16);
    let mut line = aligned_line(
        label,
        key,
        remaining,
        Style::default().fg(theme::text_dim()).bold(),
        Style::default().fg(color),
    );
    line.spans
        .insert(0, Span::styled(prefix, Style::default().fg(color).bold()));
    line
}

fn sidebar_section(label: &str, width: u16) -> Line<'static> {
    let width = width as usize;
    let label = task::fit(label, width);
    let label_width = Span::raw(&label).width();
    let rule_width = width.saturating_sub(label_width + usize::from(label_width < width));
    let mut spans = vec![Span::styled(
        label,
        Style::default().fg(theme::muted()).bold(),
    )];
    if rule_width > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            "─".repeat(rule_width.saturating_sub(1)),
            Style::default().fg(theme::border_dim()),
        ));
    }
    Line::from(spans)
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
        rows.push(session_row(&sessions[index], true, app));
        rows.extend(
            sessions
                .iter()
                .enumerate()
                .filter(|(candidate, _)| *candidate != index)
                .take(4)
                .map(|(_, session)| session_row(session, false, app)),
        );
    } else {
        rows.push(SidebarRow {
            label: conversation_title(app).to_owned(),
            detail: TaskStatus::for_app(app).label().to_owned(),
            kind: SidebarRowKind::Session { current: true },
            session_id: None,
        });
        rows.extend(
            sessions
                .iter()
                .take(4)
                .map(|session| session_row(session, false, app)),
        );
    }
}

fn session_row(session: &SessionSummary, current: bool, app: &App) -> SidebarRow {
    SidebarRow {
        label: session.display_title(),
        detail: if current {
            TaskStatus::for_app(app).label().to_owned()
        } else {
            if chrono::DateTime::parse_from_rfc3339(&session.updated_at).is_ok() {
                session.updated_label()
            } else {
                format!("{} msg", session.message_count)
            }
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
            format!("▎ {} ", TaskStatus::for_app(app).marker(app)),
            Style::default().fg(TaskStatus::for_app(app).color()),
            Style::default().fg(theme::text()).bold(),
            Style::default().fg(TaskStatus::for_app(app).color()),
        ),
        SidebarRowKind::Session { current: false } => (
            "  · ".to_owned(),
            Style::default().fg(theme::muted()),
            Style::default().fg(theme::text_dim()),
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

fn current_workspace_name(app: &App) -> String {
    app.state
        .known_workspaces
        .iter()
        .find(|workspace| workspace.is_current)
        .map(|workspace| workspace_display_name(&workspace.display_name, &workspace.path))
        .unwrap_or_else(fallback_workspace_name)
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
    aligned_line(
        label,
        key,
        width,
        Style::default(),
        Style::default().fg(theme::muted()),
    )
}

pub(super) fn draw_welcome(frame: &mut Frame<'_>, area: Rect, viewport_shape: responsive::Shape) {
    yeet_brand::draw(frame, area, viewport_shape);
}

#[cfg(test)]
mod polish_tests {
    use super::*;

    #[test]
    fn sidebar_header_has_clear_sections_and_fits_available_width() {
        for width in 0..40 {
            let lines = sidebar_header(width);
            assert_eq!(lines.len(), 6);
            assert!(lines.iter().all(|line| line.width() <= width as usize));
            assert_eq!(lines[1].width(), 0);
            assert_eq!(lines[4].width(), 0);
        }
        let lines = sidebar_header(25);
        assert_eq!(lines[0].to_string(), "◈ YEET / WORKBENCH");
        assert_eq!(lines[0].spans[0].style.fg, Some(theme::accent_warm()));
        assert!(lines[2].to_string().ends_with("Ctrl+N"));
        assert!(lines[5].to_string().starts_with("WORKSPACE "));
    }

    #[test]
    fn sidebar_labels_cannot_break_the_single_line_hit_targets() {
        let line = aligned_line(
            "Fix\n the\t sidebar",
            "Ready",
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
            for (label, detail) in [
                ("  · Conversation", "Current · Working"),
                ("修正界面", "昨日"),
            ] {
                let line = aligned_line(label, detail, width, theme::base(), theme::brand());
                assert!(line.width() <= width as usize, "width {width}: {line:?}");
            }
            assert!(shortcut("Settings", "Ctrl+,", width).width() <= width as usize);
        }
        let line = aligned_line("Session", "2m ago", 24, theme::base(), theme::brand());
        assert_eq!(line.to_string(), "Session           2m ago");
        let narrow = aligned_line(
            "Session",
            "Current · Working",
            16,
            theme::base(),
            theme::brand(),
        );
        assert!(narrow.to_string().starts_with("Session"));
        assert_eq!(narrow.width(), 16);
    }

    #[test]
    fn header_preserves_brand_and_title_hierarchy() {
        let line = header_title_line("New conversation", 80, true);
        assert_eq!(line.to_string(), "Yeet · New conversation");
        assert_eq!(line.spans[0].style, theme::brand());
        assert_eq!(line.spans[1].style.fg, Some(theme::muted()));
        assert_eq!(line.spans[2].style.fg, Some(theme::text()));
        assert!(line.spans[2].style.add_modifier.contains(Modifier::BOLD));

        let sidebar_owned_brand = header_title_line("New conversation", 80, false);
        assert_eq!(sidebar_owned_brand.to_string(), "New conversation");
    }

    #[test]
    fn header_keeps_multiline_titles_on_one_line() {
        let line = header_title_line("  Fix\n  the\t layout\r\nplease  ", 80, true);
        assert_eq!(line.to_string(), "Yeet · Fix the layout please");
    }

    #[test]
    fn header_fits_even_narrow_and_unicode_viewports() {
        for title in [
            "A long conversation title",
            "修正界面布局",
            "Cafe\u{301} ☕",
        ] {
            for width in 0..80 {
                let line = header_title_line(title, width, true);
                assert!(line.width() <= width, "width {width}: {line:?}");
            }
        }
    }
}
