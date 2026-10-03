//! Agent swarm panel: session switch, auto-deploy, and group limits.

use crate::tui::app::swarm::{SWARM_ROWS, SwarmRow};

use super::*;

pub(crate) fn draw_swarm(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(72, 60, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.state.settings_working {
        " Agent swarm · saving… · Esc close "
    } else {
        " Agent swarm · ↑/↓ select · ←/→ adjust · Enter toggle · Esc close "
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
    let (status, status_style) = if app.swarm_enabled() {
        ("ON", Style::default().fg(theme::accent()).bold())
    } else {
        ("OFF", Style::default().fg(theme::muted()).bold())
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(vec![
                Span::styled(" Swarm  ", Style::default().fg(theme::text()).bold()),
                Span::styled(status, status_style),
                Span::styled(
                    format!("  ·  {running} running · {} live agents", members.len()),
                    Style::default().fg(theme::muted()),
                ),
            ]),
            Line::from(Span::styled(
                " The primary agent runs researcher, implementer, and verifier agents in parallel.",
                Style::default().fg(theme::muted()),
            )),
        ]))
        .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let swarm = &app.state.runtime_settings.swarm;
    let on_off = |value: bool| if value { "on" } else { "off" }.to_owned();
    let rows = SWARM_ROWS.iter().map(|row| {
        let (label, value, detail) = match row {
            SwarmRow::Enabled => (
                "Swarm (this session)",
                on_off(app.swarm_enabled()),
                "agent tools for the primary agent",
            ),
            SwarmRow::AutoDeploy => (
                "Auto-deploy",
                on_off(swarm.auto_deploy),
                "fan out unprompted; new sessions start swarmed",
            ),
            SwarmRow::Parallel => (
                "Parallel agents",
                format!("‹ {} ›", swarm.max_concurrent),
                "agents working at once",
            ),
            SwarmRow::Pool => (
                "Agent pool",
                format!("‹ {} ›", swarm.max_members),
                "live agents kept for follow-ups",
            ),
            SwarmRow::Tokens => (
                "Token budget",
                format!("‹ {} ›", compact_number(swarm.max_tokens)),
                "delegated tokens per turn",
            ),
            SwarmRow::Cost => (
                "Cost budget",
                format!("‹ ${:.2} ›", swarm.max_cost_cents as f64 / 100.0),
                "delegated spend per turn",
            ),
            SwarmRow::Writers => (
                "Writers",
                if swarm.write_policy == "primary_only" {
                    "primary only".into()
                } else {
                    "one agent".into()
                },
                "who may edit the workspace",
            ),
            SwarmRow::OpenAgents => (
                "Agents view",
                "open".into(),
                "watch, steer, and stop agents",
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
        ListState::default().with_selected(Some(app.popup_index.min(SWARM_ROWS.len() - 1)));
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
