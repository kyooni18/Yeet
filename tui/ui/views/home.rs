use super::super::{
    components::{composer, status, tabbar},
    support::{icons, theme},
};
use crate::{
    app::{App, HomeAction},
    workbench::{ResourceItem, ResourceKind, ResourceTarget},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    prelude::{Color, Line, Span, Style},
    style::Modifier,
    widgets::{Block, Paragraph, Wrap},
};

const ACTIVE: Modifier = Modifier::BOLD;

fn draw_compact(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    frame.render_widget(
        Block::default().style(theme::base().bg(theme::code_background())),
        area,
    );
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
    tabbar::draw(frame, app, rows[0], tabbar::Active::Home);
    if area.width >= 75 {
        let panes = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(rows[1]);
        draw_activity(frame, app, panes[0]);
        frame.render_widget(Block::default().style(theme::base()), panes[1]);
        draw_inspector(
            frame,
            app,
            Rect::new(
                panes[1].x + 2,
                panes[1].y,
                panes[1].width.saturating_sub(4),
                panes[1].height,
            ),
        );
    } else {
        draw_activity(frame, app, rows[1]);
    }
    draw_composer(frame, app, rows[2]);
    status::draw(frame, app, rows[3]);
}

fn draw_sessions_rail(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if area.width == 0 || area.height < 3 {
        return;
    }
    frame.render_widget(Block::default().style(theme::base()), area);
    let button = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1);
    frame.render_widget(
        Paragraph::new("+  New session")
            .style(theme::surface())
            .centered(),
        button,
    );
    app.home_targets.push((button, HomeAction::NewSession));
    let count = area.height.saturating_sub(4) as usize;
    let selected = app
        .home
        .content
        .sessions
        .iter()
        .position(|item| Some(&item.target) == app.home.selected.as_ref())
        .unwrap_or(0);
    let offset = crate::tui::kit::visible_start(selected, count);
    if app.home.content.sessions.is_empty() {
        frame.render_widget(
            Paragraph::new("No saved sessions")
                .style(Style::default().fg(theme::muted()))
                .wrap(Wrap { trim: true }),
            Rect::new(area.x + 1, area.y + 3, area.width.saturating_sub(2), 2),
        );
    }
    for (index, item) in app
        .home
        .content
        .sessions
        .iter()
        .skip(offset)
        .take(count)
        .enumerate()
    {
        let row = Rect::new(area.x, area.y + 3 + index as u16, area.width, 1);
        let selected = Some(&item.target) == app.home.selected.as_ref();
        let style = if selected {
            Style::default()
                .fg(theme::text())
                .bg(theme::surface_color())
                .add_modifier(ACTIVE)
        } else {
            Style::default().fg(theme::text_dim())
        };
        frame.render_widget(Block::default().style(style), row);
        let marker = if item.context == "running" {
            "●"
        } else if item.context == "permission" {
            "?"
        } else {
            "·"
        };
        let age = if area.width >= 23 {
            item.age.replace(" ago", "")
        } else {
            String::new()
        };
        let age_width = age.chars().count() as u16;
        let title_width = area.width.saturating_sub(5 + age_width);
        frame.render_widget(
            Paragraph::new(format!(
                " {marker} {}",
                super::super::task::fit(&item.title, title_width as usize)
            ))
            .style(style),
            row,
        );
        if age_width > 0 {
            frame.render_widget(
                Paragraph::new(age).style(Style::default().fg(theme::muted())),
                Rect::new(
                    row.right().saturating_sub(age_width + 1),
                    row.y,
                    age_width,
                    1,
                ),
            );
        }
        app.home_targets
            .push((row, HomeAction::Select(item.target.clone())));
    }
}

enum ActivityRow<'a> {
    Heading(&'static str),
    Item(&'a ResourceItem),
    Message(&'a str),
    Gap,
}

fn draw_activity(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if area.height < 3 || area.width < 8 {
        return;
    }
    let compact = frame.area().width < 110;
    let mut rows = Vec::new();
    if compact {
        rows.push(ActivityRow::Heading("Sessions"));
        rows.extend(app.home.content.sessions.iter().map(ActivityRow::Item));
        if app.home.content.sessions.is_empty() {
            rows.push(ActivityRow::Message("No saved sessions"));
        }
        rows.push(ActivityRow::Gap);
    }
    rows.push(ActivityRow::Heading("Recent views"));
    rows.extend(app.home.content.recent_views.iter().map(ActivityRow::Item));
    if app.home.content.recent_views.is_empty() {
        rows.push(ActivityRow::Message("No views opened yet"));
    }
    rows.push(ActivityRow::Gap);
    rows.push(ActivityRow::Heading("Agent tasks"));
    rows.extend(app.home.content.tasks.iter().map(ActivityRow::Item));
    if app.home.content.tasks.is_empty() {
        rows.push(ActivityRow::Message("No agent tasks"));
    }
    rows.push(ActivityRow::Gap);
    rows.push(ActivityRow::Heading("Changed files"));
    rows.extend(app.home.content.diffs.iter().map(ActivityRow::Item));
    if let Some(message) = &app.home.content.git_message {
        rows.push(ActivityRow::Message(message));
    } else if app.home.content.diffs.is_empty() {
        rows.push(ActivityRow::Message("Working tree clean"));
    }
    let usage_height = if area.height >= 16 { 4 } else { 0 };
    let capacity = area.height.saturating_sub(usage_height + 3) as usize;
    app.home.scroll = app.home.scroll.min(rows.len().saturating_sub(capacity));
    if let Some(selected) = rows.iter().position(|row| matches!(row, ActivityRow::Item(item) if Some(&item.target) == app.home.selected.as_ref())) {
        if selected < app.home.scroll { app.home.scroll = selected; }
        else if selected >= app.home.scroll + capacity { app.home.scroll = selected.saturating_sub(capacity.saturating_sub(1)); }
    }
    for (index, row) in rows.iter().skip(app.home.scroll).take(capacity).enumerate() {
        let rect = Rect::new(
            area.x + 2,
            area.y + 2 + index as u16,
            area.width.saturating_sub(4),
            1,
        );
        match row {
            ActivityRow::Heading(title) => frame.render_widget(
                Paragraph::new(*title)
                    .style(Style::default().fg(theme::muted()).add_modifier(ACTIVE)),
                rect,
            ),
            ActivityRow::Message(message) => frame.render_widget(
                Paragraph::new(super::super::task::fit(message, rect.width as usize))
                    .style(Style::default().fg(theme::muted())),
                rect,
            ),
            ActivityRow::Item(item) => {
                draw_resource(
                    frame,
                    item,
                    rect,
                    Some(&item.target) == app.home.selected.as_ref(),
                );
                app.home_targets
                    .push((rect, HomeAction::Select(item.target.clone())));
            }
            ActivityRow::Gap => {}
        }
    }
    if app.home.scroll + capacity < rows.len() {
        frame.render_widget(
            Paragraph::new("↓ more  ·  Esc then j/k").style(Style::default().fg(theme::muted())),
            Rect::new(
                area.x + 2,
                area.bottom().saturating_sub(usage_height + 1),
                area.width.saturating_sub(4),
                1,
            ),
        );
    }
    if usage_height > 0 {
        draw_provider_usage(
            frame,
            app,
            Rect::new(
                area.x + 2,
                area.bottom() - usage_height,
                area.width.saturating_sub(4),
                usage_height,
            ),
        );
    }
}

fn draw_resource(frame: &mut Frame<'_>, item: &ResourceItem, area: Rect, selected: bool) {
    let style = Style::default()
        .fg(if selected {
            theme::text()
        } else {
            theme::text_dim()
        })
        .bg(if selected {
            theme::surface_color()
        } else {
            theme::code_background()
        });
    frame.render_widget(Block::default().style(style), area);
    let icon = match item.kind {
        ResourceKind::Session => icons::session_tab(),
        ResourceKind::View => icons::workspace(),
        ResourceKind::File => icons::file(&item.title),
        ResourceKind::Diff => icons::diff_tab(),
        ResourceKind::Task => icons::agent_tab(),
    };
    frame.render_widget(
        Paragraph::new(icon).style(style),
        Rect::new(area.x, area.y, 1, 1),
    );
    let mut right = if let Some(stats) = item.changes {
        format!("+{} −{}", stats.added, stats.removed)
    } else if !item.age.is_empty() {
        item.age.replace(" ago", "")
    } else {
        item.context.clone()
    };
    // Task metadata is optional in the list; keep the objective readable and
    // show the full role/status in the inspector when both cannot fit.
    if item.kind == ResourceKind::Task
        && item.title.chars().count() + right.chars().count() + 5 > area.width as usize
    {
        right.clear();
    }
    let width = (right.chars().count() as u16).min(area.width / 2);
    let title_width = area.width.saturating_sub(width + 5);
    frame.render_widget(
        Paragraph::new(super::super::task::fit(&item.title, title_width as usize)).style(style),
        Rect::new(area.x + 3, area.y, title_width, 1),
    );
    if let Some(stats) = item.changes {
        let spans = vec![
            Span::styled(
                format!("+{}", stats.added),
                Style::default().fg(Color::Rgb(0x76, 0x94, 0x6a)),
            ),
            Span::raw(" "),
            Span::styled(
                format!("−{}", stats.removed),
                Style::default().fg(Color::Rgb(0xc3, 0x40, 0x43)),
            ),
        ];
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(style),
            Rect::new(area.right() - width, area.y, width, 1),
        );
    } else {
        frame.render_widget(
            Paragraph::new(super::super::task::fit(&right, width as usize))
                .style(style.fg(theme::muted())),
            Rect::new(area.right() - width, area.y, width, 1),
        );
    }
}

fn draw_provider_usage(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let muted = Style::default().fg(theme::muted());
    if app.home.content.providers.is_empty() {
        frame.render_widget(
            Paragraph::new("Provider usage not reported").style(muted),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
    }
    for (index, provider) in app.home.content.providers.iter().take(2).enumerate() {
        let y = area.y + index as u16;
        let mut spans = vec![Span::styled(format!("{}  ", provider.provider), muted)];
        if !provider.available || provider.windows.is_empty() {
            spans.push(Span::styled(
                provider
                    .message
                    .clone()
                    .unwrap_or_else(|| "Usage unavailable".into()),
                muted,
            ));
        } else {
            let windows = provider.windows.iter().take(2);
            let meter_width = (area.width.saturating_sub(24) / 4).min(12) as usize;
            for window in windows {
                let used = window.used_percent.min(100);
                let filled = (meter_width * usize::from(used)).div_ceil(100);
                spans.push(Span::styled(format!("{} ", window.label), muted));
                spans.push(Span::styled(
                    "━".repeat(filled),
                    Style::default().fg(theme::accent()),
                ));
                spans.push(Span::styled(
                    "━".repeat(meter_width.saturating_sub(filled)),
                    Style::default().fg(theme::surface_color()),
                ));
                spans.push(Span::styled(
                    format!(" {used}%  "),
                    Style::default().fg(theme::text_dim()),
                ));
            }
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, y, area.width, 1),
        );
    }
    let button = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    frame.render_widget(Paragraph::new("Usage details →").style(muted), button);
    app.home_targets
        .push((button, HomeAction::Open(ResourceTarget::Status)));
}

fn draw_inspector(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if area.width == 0 || area.height < 5 {
        return;
    }
    let Some(item) = app.home.selected_item() else {
        frame.render_widget(Paragraph::new(format!("{}\n\nSelect a session, recent view, task or changed file.\n\nEsc  Browse Home\ni    Write a prompt", app.home.content.workspace.display())).style(Style::default().fg(theme::text_dim())).wrap(Wrap { trim: true }), Rect::new(area.x, area.y + 2, area.width, area.height.saturating_sub(2)));
        return;
    };
    let title = Paragraph::new(item.title.clone())
        .style(Style::default().fg(theme::text()).add_modifier(ACTIVE))
        .wrap(Wrap { trim: true });
    let title_height = (title.line_count(area.width) as u16).min(3);
    frame.render_widget(
        title,
        Rect::new(area.x, area.y + 2, area.width, title_height),
    );
    let context_y = area.y + 2 + title_height;
    let context = [&item.context, &item.age]
        .into_iter()
        .filter(|text| !text.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ");
    frame.render_widget(
        Paragraph::new(context).style(Style::default().fg(theme::muted())),
        Rect::new(area.x, context_y, area.width, 1),
    );
    let detail_y = context_y + 3;
    let detail_height = area.bottom().saturating_sub(detail_y + 5);
    let detail = Paragraph::new(item.detail.clone())
        .style(Style::default().fg(theme::text_dim()))
        .wrap(Wrap { trim: true });
    let button_y =
        (detail_y + detail.line_count(area.width) as u16 + 2).min(area.bottom().saturating_sub(3));
    frame.render_widget(
        detail,
        Rect::new(area.x, detail_y, area.width, detail_height),
    );
    let label = match item.kind {
        ResourceKind::Session => "Open session →",
        ResourceKind::Task => "Open task session →",
        ResourceKind::Diff => "Review diff →",
        ResourceKind::File => "Open file →",
        ResourceKind::View => "Open view →",
    };
    let button = Rect::new(area.x, button_y, area.width, 1);
    frame.render_widget(
        Paragraph::new(label).style(Style::default().fg(theme::text())),
        button,
    );
    let target = item.target.clone();
    app.home_targets.push((button, HomeAction::Open(target)));
    frame.render_widget(
        Paragraph::new("Esc  Browse   j/k  Select   Enter  Open")
            .style(Style::default().fg(theme::muted()))
            .wrap(Wrap { trim: true }),
        Rect::new(area.x, area.bottom() - 2, area.width, 2),
    );
}

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    app.transcript_area = (0, 0, 0, 0);
    app.sidebar_area = (0, 0, 0, 0);
    app.sidebar_session_targets.clear();
    app.refresh_home(false);
    let bounds = frame.area();
    if bounds.width < 45 || bounds.height < 5 {
        return;
    }
    if bounds.width < 110 {
        draw_compact(frame, app, bounds);
        return;
    }
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::code_background())),
        bounds,
    );
    let scale_x = |px: u16| bounds.width as u32 * px as u32 / 1440;
    let scale_y = |px: u16| (bounds.height as u32 * px as u32 + 432) / 864;
    let tab_height = 1;
    let composer_height = scale_y(36).max(1) as u16;
    let status_height = scale_y(24).max(1) as u16;
    let rows = Layout::vertical([
        Constraint::Length(tab_height),
        Constraint::Min(1),
        Constraint::Length(composer_height),
        Constraint::Length(status_height),
    ])
    .split(bounds);
    tabbar::draw(frame, app, rows[0], tabbar::Active::Home);
    let body = Rect::new(
        bounds.x,
        bounds.y.saturating_add(tab_height),
        bounds.width,
        bounds
            .height
            .saturating_sub(tab_height + composer_height + status_height),
    );
    let columns = Layout::horizontal([Constraint::Length(scale_x(232) as u16), Constraint::Min(1)])
        .split(body);
    draw_sessions_rail(
        frame,
        app,
        Rect::new(
            columns[0].x,
            columns[0].y,
            columns[0].width,
            columns[0].height + composer_height,
        ),
    );
    let activity_width = scale_x(540).min(columns[1].width as u32) as u16;
    let content_inset = scale_x(30) as u16;
    let inspector_x = bounds.x + scale_x(832) as u16;
    let inspector = Rect::new(
        inspector_x,
        body.y,
        bounds.right().saturating_sub(inspector_x),
        body.height,
    );
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::background())),
        inspector,
    );
    let inspector_inset = scale_x(24) as u16;
    draw_activity(
        frame,
        app,
        Rect::new(
            columns[1].x + content_inset,
            body.y,
            activity_width,
            body.height,
        ),
    );
    draw_inspector(
        frame,
        app,
        Rect::new(
            inspector.x + inspector_inset,
            inspector.y,
            inspector.width.saturating_sub(inspector_inset * 2),
            inspector.height,
        ),
    );
    draw_composer(
        frame,
        app,
        Rect::new(
            bounds.x + scale_x(232) as u16,
            rows[2].y,
            bounds.width.saturating_sub(scale_x(232) as u16),
            rows[2].height,
        ),
    );
    status::draw(frame, app, rows[3]);
}

fn draw_composer(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if area.height == 0 {
        return;
    }
    let inset = (area.width as u32 * 30 / 1208).max(2) as u16;
    composer::draw(frame, app, area, area.x + inset + 3);
    if app.input.is_empty() {
        let (x, y, width, height) = app.composer_area;
        frame.render_widget(
            Paragraph::new("Ask Yeet...").style(Style::default().fg(theme::text())),
            Rect::new(x, y, width, height),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{
            AgentTaskItem, AuthProviderItem, FrontendCommand, ProviderUsageStatus,
            ProviderUsageWindow, SessionActivity, SessionSummary,
        },
        workbench::{ChangeStats, GitChange, GitSnapshot},
    };
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn home_renders_runtime_resources_after_resize_and_opens_selected_session() {
        let mut app = App::default();
        app.home_override = Some(true);
        app.input = "Keep my draft".into();
        app.cursor = 4;
        app.state.saved_sessions.push(SessionSummary {
            id: "real-session".into(),
            title: "Real workspace session".into(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            model: "test/model".into(),
            message_count: 12,
        });
        app.state
            .session_activity
            .insert("real-session".into(), SessionActivity::Running);
        app.state.agent_tasks.push(AgentTaskItem {
            id: "real-task".into(),
            role: "Reviewer".into(),
            objective: "Verify resource navigation".into(),
            status: "running".into(),
            summary: Some("Checking stable targets".into()),
        });
        let workspace = app.workspace_path();
        app.recent_views.visit(
            ResourceTarget::File(workspace.join("src/real.rs")),
            "src/real.rs",
        );
        app.home.set_git_snapshot(GitSnapshot {
            workspace: workspace.clone(),
            root: workspace,
            branch: "real-branch".into(),
            changes: vec![GitChange {
                path: "src/real.rs".into(),
                previous_path: None,
                status: " M".into(),
                stats: Some(ChangeStats {
                    added: 27,
                    removed: 8,
                }),
            }],
            message: None,
        });
        app.state.auth_providers.push(AuthProviderItem {
            provider: "test-provider".into(),
            authenticated: true,
            method: "oauth".into(),
            expires_at: None,
            error: None,
            usage: Some(ProviderUsageStatus {
                provider: "test-provider".into(),
                available: true,
                source: "test".into(),
                fetched_at: String::new(),
                plan: None,
                windows: vec![ProviderUsageWindow {
                    id: "primary".into(),
                    label: "5h".into(),
                    used_percent: 73,
                    remaining_percent: 27,
                    resets_at: None,
                }],
                message: None,
            }),
        });
        for (width, height) in [(45, 5), (60, 14), (80, 24), (110, 28), (144, 44), (203, 41)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| crate::tui::ui::draw(frame, &mut app))
                .unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .chunks(width as usize)
                .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(!text.contains("3.2 km"));
            assert!(!text.contains("MM305"));
            if width >= 110 {
                for expected in [
                    "Real workspace session",
                    "12 messages",
                    "src/real.rs",
                    "Verify resource navigation",
                    "+27",
                    "−8",
                    "73%",
                ] {
                    assert!(
                        text.contains(expected),
                        "Missing {expected} at {width}x{height}: {text}"
                    );
                }
            }
        }
        let click = |area: Rect| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        };
        let session = app
            .home_targets
            .iter()
            .find(|(_, action)| matches!(action, HomeAction::Select(ResourceTarget::Session(_))))
            .unwrap()
            .0;
        app.handle_mouse(click(session));
        assert!(!app.input_focused);
        assert!(app.handle_home_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(
            matches!(app.take_workbench_command(), Some(FrontendCommand::LoadSession { session_id }) if session_id == "real-session")
        );
        assert!(!app.home_visible());
        assert_eq!(app.input, "Keep my draft");
        assert_eq!(app.cursor, 4);
        app.activate_workbench_tab(crate::tui::app::WorkbenchTab::Home);
        let mut terminal = Terminal::new(TestBackend::new(144, 44)).unwrap();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &mut app))
            .unwrap();
        let new = app
            .home_targets
            .iter()
            .find(|(_, action)| matches!(action, HomeAction::NewSession))
            .unwrap()
            .0;
        app.handle_mouse(click(new));
        assert!(matches!(
            app.take_workbench_command(),
            Some(FrontendCommand::NewSession)
        ));
    }
}
