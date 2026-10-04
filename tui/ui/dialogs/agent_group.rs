//! Agent Group settings panel: session availability, delegation, and budgets.

use crate::tui::app::agent_group::{AGENT_GROUP_ROWS, AgentGroupRow};

use super::*;

pub(crate) fn draw_agent_group(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(72, 60, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.state.settings_working {
        " Agent Group · saving… · Esc close "
    } else {
        " Agent Group · ↑/↓ select · ←/→ adjust · Enter toggle · Esc close "
    };
    let block = theme::modal_block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let members = &app.state.agent_group.members;
    let running = members.iter().filter(|m| m.status == "running").count();
    let (status, status_style) = if app.agent_group_enabled() {
        ("ON", Style::default().fg(theme::accent()).bold())
    } else {
        ("OFF", Style::default().fg(theme::muted()).bold())
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(vec![
                Span::styled(" Group Agent  ", Style::default().fg(theme::text()).bold()),
                Span::styled(status, status_style),
                Span::styled(
                    format!("  ·  {running} running · {} live agents", members.len()),
                    Style::default().fg(theme::muted()),
                ),
            ]),
            Line::from(Span::styled(
                " A Group Agent coordinates researcher, implementer, and verifier members.",
                Style::default().fg(theme::muted()),
            )),
        ]))
        .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let agent_group = &app.state.runtime_settings.agent_group;
    let on_off = |value: bool| if value { "on" } else { "off" }.to_owned();
    let rows = AGENT_GROUP_ROWS.iter().map(|row| {
        let (label, value, detail) = match row {
            AgentGroupRow::Enabled => (
                "Group Agent (this session)",
                on_off(app.agent_group_enabled()),
                "group lifecycle tools for the Main Agent",
            ),
            AgentGroupRow::AutoDeploy => (
                "Auto-deploy",
                on_off(agent_group.auto_deploy),
                "allow automatic group delegation for new sessions",
            ),
            AgentGroupRow::Parallel => (
                "Parallel members",
                format!("‹ {} ›", agent_group.max_concurrent),
                "members working at once",
            ),
            AgentGroupRow::Pool => (
                "Member capacity",
                format!("‹ {} ›", agent_group.max_members),
                "members kept for follow-ups",
            ),
            AgentGroupRow::Tokens => (
                "Output token budget",
                format!("‹ {} ›", compact_number(agent_group.max_tokens)),
                "shared group output tokens",
            ),
            AgentGroupRow::Cost => (
                "Cost budget",
                format!("‹ ${:.2} ›", agent_group.max_cost_cents as f64 / 100.0),
                "shared group cost ceiling",
            ),
            AgentGroupRow::Writers => (
                "Writers",
                if agent_group.write_policy == "primary_only" {
                    "primary only".into()
                } else {
                    "one agent".into()
                },
                "who may edit the workspace",
            ),
            AgentGroupRow::OpenAgents => (
                "Agents view",
                "open".into(),
                "inspect the group and its members",
            ),
        };
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{label:<22}"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{value:<14}"), Style::default().fg(theme::accent())),
            Span::styled(detail, Style::default().fg(theme::muted())),
        ]))
    });
    let list = List::new(rows)
        .highlight_style(theme::selected())
        .highlight_symbol("▸ ");
    let mut state =
        ListState::default().with_selected(Some(app.popup_index.min(AGENT_GROUP_ROWS.len() - 1)));
    frame.render_stateful_widget(list, chunks[1], &mut state);

    let notice = app
        .state
        .settings_notice
        .as_deref()
        .unwrap_or("Limits are saved globally and apply to the next agent launched.");
    frame.render_widget(
        Paragraph::new(truncate_end(notice, chunks[2].width as usize)).fg(theme::muted()),
        chunks[2],
    );
}
