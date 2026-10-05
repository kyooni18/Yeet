//! Agent view: the active Agent Group as a rail of members, an activity
//! timeline for the group or one member, and a state inspector.
use super::super::{
    components::{composer, status, tabbar},
    shell::conversation_title,
    support::{icons, text::compact_number, theme},
    task::fit,
    views::sessions::tools::{spinner, tool_icon},
};
use crate::{
    model::{AgentActivityItem, AgentActivityKind, AgentMemberItem},
    tui::app::{agents::AgentAction, App},
};
use chrono::{DateTime, Utc};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    prelude::{Line, Modifier, Span, Style},
    widgets::{Block, Paragraph, Wrap},
    Frame,
};

const BOLD: Modifier = Modifier::BOLD;
const LABEL_WIDTH: usize = 11;

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    app.agents.targets.clear();
    app.transcript_area = (0, 0, 0, 0);
    let bounds = frame.area();
    if bounds.height < 6 || bounds.width < 20 {
        frame.render_widget(
            Paragraph::new("Resize the terminal to inspect the Agent Group.")
                .style(muted())
                .centered(),
            bounds,
        );
        return;
    }
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::code_background())),
        bounds,
    );
    let wide = bounds.width >= 110;
    let scale_x = |px: u16| (bounds.width as u32 * px as u32 / 1440) as u16;
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .split(bounds);
    tabbar::draw(frame, app, rows[0], tabbar::Active::Agents);
    let body = rows[1];
    let rail_width = if bounds.width >= 70 {
        scale_x(232).max(22)
    } else {
        0
    };
    let inspector_x = if wide {
        bounds.x + scale_x(1090)
    } else {
        bounds.right()
    };
    if rail_width > 0 {
        draw_rail(
            frame,
            app,
            Rect::new(body.x, body.y, rail_width, body.height + rows[2].height),
        );
    }
    let main_x = body.x + rail_width;
    frame.render_widget(
        Block::default().style(theme::base()),
        Rect::new(
            main_x,
            body.y,
            inspector_x.saturating_sub(main_x),
            body.height,
        ),
    );
    let inset = scale_x(30).max(2);
    let main = Rect::new(
        main_x + inset,
        body.y,
        inspector_x.saturating_sub(main_x + inset * 2),
        body.height,
    );
    draw_main(frame, app, main);
    if wide {
        let inspector = Rect::new(
            inspector_x,
            body.y,
            bounds.right() - inspector_x,
            body.height,
        );
        frame.render_widget(
            Block::default().style(Style::default().bg(theme::background())),
            inspector,
        );
        let pad = scale_x(24).max(2);
        draw_inspector(
            frame,
            app,
            Rect::new(
                inspector.x + pad,
                inspector.y,
                inspector.width.saturating_sub(pad * 2),
                inspector.height,
            ),
        );
    }
    let composer_area = Rect::new(
        main_x,
        rows[2].y,
        bounds.width.saturating_sub(rail_width),
        rows[2].height,
    );
    composer::draw(frame, app, composer_area, main_x + inset + 3);
    if app.input.is_empty() {
        let hint = match app.selected_agent() {
            _ if app.agents.creating_group => "Describe the shared objective · Esc cancel".into(),
            Some(member) if member.status == "stopped" => {
                format!("{} is stopped", member.description)
            }
            Some(member) => format!("Steer {}...", member.description),
            None if app.state.agent_group.objective.is_none() => {
                "Create a new shared objective...".into()
            }
            None => "Ask the Main Agent about this group...".into(),
        };
        let (x, y, width, height) = app.composer_area;
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(theme::muted())),
            Rect::new(x, y, width, height),
        );
    }
    status::draw(frame, app, rows[3]);
}

fn draw_rail(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    frame.render_widget(Block::default().style(theme::base()), area);
    if area.height < 3 {
        return;
    }
    let add = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1);
    let (add_label, add_style) = if app.agents.creating_group {
        (
            "Creating group objective".to_owned(),
            theme::surface().fg(theme::accent()).add_modifier(BOLD),
        )
    } else if app.state.agent_group.status == "running" {
        ("Group is running".to_owned(), muted())
    } else if app.state.agent_group.status == "paused" {
        ("Group is paused".to_owned(), muted())
    } else {
        ("+  New group".to_owned(), theme::surface())
    };
    frame.render_widget(Paragraph::new(add_label).style(add_style).centered(), add);
    if !matches!(app.state.agent_group.status.as_str(), "running" | "paused") {
        app.agents.targets.push((add, AgentAction::CreateGroup));
    }
    draw_rail_actions(frame, app, area);
    let members = app.agent_members();
    let running = counts(members).0;
    let title = group_title(app).to_owned();
    let count = format!("{running}/{}", members.len());
    let width = area.width as usize;
    let mut rows: Vec<(Option<String>, Line<'static>, bool)> = Vec::new();
    let group_selected = app.agents.selected.is_none() || app.selected_agent().is_none();
    let label = fit(&title, width.saturating_sub(count.len() + 6));
    let gap = width.saturating_sub(4 + Span::raw(&label).width() + count.len() + 1);
    rows.push((
        None,
        Line::from(vec![
            Span::styled(format!(" {} ", icons::chevron(true)), muted()),
            Span::raw(" "),
            Span::styled(label, Style::default().fg(theme::text()).add_modifier(BOLD)),
            Span::raw(" ".repeat(gap)),
            Span::styled(count, muted()),
        ]),
        group_selected,
    ));
    for member in members {
        let (glyph, state) = member_state(member);
        let state_width = state.len();
        let name = fit(&member.description, width.saturating_sub(state_width + 7));
        let gap = width.saturating_sub(5 + Span::raw(&name).width() + state_width + 1);
        let name_style = if matches!(member_state(member).1, "stopped" | "cancelled" | "failed") {
            muted()
        } else {
            Style::default().fg(theme::text_dim())
        };
        rows.push((
            Some(member.id.clone()),
            Line::from(vec![
                Span::raw("   "),
                Span::styled(glyph, glyph_style(member)),
                Span::raw(" "),
                Span::styled(name, name_style),
                Span::raw(" ".repeat(gap)),
                Span::styled(state, muted()),
            ]),
            !group_selected && app.agents.selected.as_deref() == Some(member.id.as_str()),
        ));
    }
    // Rows sit between the new group action and the Group Agent/action rows.
    let last = area
        .bottom()
        .saturating_sub(if area.height >= 10 { 5 } else { 4 });
    let first_row = area.y + 3;
    let visible = last.saturating_sub(first_row) as usize;
    let selected_index = rows
        .iter()
        .position(|(id, _, _)| id == &app.agents.selected)
        .unwrap_or(0);
    if visible > 0 {
        if selected_index < app.agents.scroll {
            app.agents.scroll = selected_index;
        } else if selected_index >= app.agents.scroll + visible {
            app.agents.scroll = selected_index + 1 - visible;
        }
        app.agents.scroll = app.agents.scroll.min(rows.len().saturating_sub(visible));
    } else {
        app.agents.scroll = 0;
    }
    for (offset, (id, line, selected)) in rows
        .into_iter()
        .skip(app.agents.scroll)
        .take(visible)
        .enumerate()
    {
        let y = first_row + offset as u16;
        let row = Rect::new(area.x, y, area.width, 1);
        if selected {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme::surface_color())),
                row,
            );
        }
        frame.render_widget(Paragraph::new(line), row);
        app.agents.targets.push((row, AgentAction::Select(id)));
    }
}

/// Stop and Remove for the selection, pinned to the bottom of the rail.
fn draw_rail_actions(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if area.height < 8 {
        return;
    }
    let selected = app.selected_agent().cloned();
    let status = app.state.agent_group.status.as_str();
    let group_action = match status {
        "running" => Some(("■ Cancel group", AgentAction::CancelGroup)),
        "paused" => Some(("▶ Resume group", AgentAction::RunGroup)),
        "created" | "completed" | "failed" if app.state.agent_group.objective.is_some() => {
            Some(("▶ Start group", AgentAction::RunGroup))
        }
        _ => None,
    };
    let y = area.bottom().saturating_sub(2);
    if area.height >= 10 {
        let agent_group = Rect::new(area.x + 2, y - 2, area.width.saturating_sub(4), 1);
        let settings = &app.state.runtime_settings.agent_group;
        let label = match (app.agent_group_enabled(), settings.auto_deploy) {
            (false, _) => "⚙ Group Agent off".to_owned(),
            (true, auto) => format!(
                "⚙ Group Agent {}×{}",
                settings.max_concurrent,
                if auto { " auto" } else { "" }
            ),
        };
        let label = fit(&label, agent_group.width as usize);
        let style = if app.agent_group_enabled() {
            Style::default().fg(theme::accent())
        } else {
            muted()
        };
        frame.render_widget(Paragraph::new(label).style(style), agent_group);
        app.agents
            .targets
            .push((agent_group, AgentAction::AgentGroup));
    }
    frame.render_widget(
        Paragraph::new("─".repeat(area.width as usize))
            .style(Style::default().fg(theme::hairline())),
        Rect::new(area.x, y - 1, area.width, 1),
    );
    if let Some(member) = selected {
        let (_, state) = member_state(&member);
        let stoppable = matches!(
            state,
            "running" | "reasoning" | "tool call" | "working" | "queued" | "input" | "review"
        );
        let stop = "■ Stop";
        let remove = "× Remove";
        let stop_area = Rect::new(area.x + 2, y, stop.chars().count() as u16, 1);
        let remove_width = remove.chars().count() as u16;
        let remove_area = Rect::new(
            area.right().saturating_sub(remove_width + 2),
            y,
            remove_width,
            1,
        );
        frame.render_widget(
            Paragraph::new(stop).style(if stoppable {
                Style::default().fg(theme::text_dim())
            } else {
                muted()
            }),
            stop_area,
        );
        if remove_area.x > stop_area.right() {
            frame.render_widget(
                Paragraph::new(remove).style(Style::default().fg(theme::text_dim())),
                remove_area,
            );
            app.agents.targets.push((remove_area, AgentAction::Remove));
        }
        if stoppable {
            app.agents.targets.push((stop_area, AgentAction::Stop));
        }
    } else if let Some((label, action)) = group_action {
        let action_area = Rect::new(
            area.x + 2,
            y,
            (label.chars().count() as u16).min(area.width.saturating_sub(4)),
            1,
        );
        frame.render_widget(
            Paragraph::new(label).style(Style::default().fg(theme::text_dim())),
            action_area,
        );
        app.agents.targets.push((action_area, action));
    }
}

fn draw_main(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if area.width < 10 || area.height < 4 {
        return;
    }
    let selected = app.selected_agent().cloned();
    let members = &app.agent_members().to_vec();
    let headline = match &selected {
        Some(member) => {
            let task = task_of(app, member).unwrap_or(member.description.as_str());
            format!("{} {} {}", member.description, verb(member), task)
        }
        None if app.state.agent_group.objective.is_none() => {
            "No Agent Group objective yet.".to_owned()
        }
        None => {
            let (running, waiting) = counts(members);
            format!(
                "Group is {} · {running} active · {waiting} waiting · {} members",
                app.state.agent_group.status,
                members.len(),
            )
        }
    };
    let headline = Paragraph::new(headline)
        .style(Style::default().fg(theme::text()).add_modifier(BOLD))
        .wrap(Wrap { trim: true });
    let headline_height = (headline.line_count(area.width) as u16).clamp(1, 2);
    frame.render_widget(
        headline,
        Rect::new(area.x, area.y + 1, area.width, headline_height),
    );
    if members.is_empty() {
        let message = if app.state.agent_group.objective.is_some() {
            "This shared objective is ready. Start the group from the rail to let the coordinator plan and delegate member tasks."
        } else {
            "Create a Group Agent objective with + New group. The coordinator will plan and delegate member tasks around it."
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(muted())
                .wrap(Wrap { trim: true }),
            Rect::new(area.x, area.y + 3, area.width, 3),
        );
        return;
    }

    let controls_y = area.y + 2 + headline_height;
    let steer = Rect::new(area.x, controls_y, 9.min(area.width), 1);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(if icons_enabled() { "\u{f040} " } else { "› " }, muted()),
            Span::styled("Steer", Style::default().fg(theme::text_dim())),
        ])),
        steer,
    );
    app.agents.targets.push((steer, AgentAction::Steer));
    let stop_label = if selected.is_some() {
        "Stop"
    } else {
        "Stop group"
    };
    let stoppable = match &selected {
        Some(member) => matches!(
            member_state(member).1,
            "running" | "reasoning" | "tool call" | "working" | "queued" | "input" | "review"
        ),
        None => matches!(app.state.agent_group.status.as_str(), "running" | "paused"),
    };
    let stop_width = stop_label.chars().count() as u16 + 2;
    let stop = Rect::new(
        area.right().saturating_sub(stop_width),
        controls_y,
        stop_width.min(area.width),
        1,
    );
    frame.render_widget(
        Paragraph::new(format!("■ {stop_label}")).style(if stoppable {
            Style::default().fg(theme::text_dim())
        } else {
            muted()
        }),
        stop,
    );
    if stoppable {
        app.agents.targets.push((
            stop,
            if selected.is_some() {
                AgentAction::Stop
            } else {
                AgentAction::StopGroup
            },
        ));
    }
    frame.render_widget(
        Paragraph::new("─".repeat(area.width as usize))
            .style(Style::default().fg(theme::hairline())),
        Rect::new(area.x, controls_y + 1, area.width, 1),
    );

    let pill_y = area.bottom().saturating_sub(2);
    let top = controls_y + 3;
    draw_timeline(
        frame,
        app,
        selected.as_ref(),
        Rect::new(area.x, top, area.width, pill_y.saturating_sub(top + 1)),
    );
    let pill = match &selected {
        _ if app.agents.creating_group => " New group objective · Enter to create ".to_owned(),
        Some(member) => format!(
            " {} · {} ",
            member.description,
            elapsed(&member.started_at).unwrap_or_else(|| "—".into())
        ),
        None => {
            let (running, waiting) = counts(members);
            if waiting > 0 {
                format!(" {waiting} waiting · {running} running ")
            } else {
                format!(" {running} running · {} total ", members.len())
            }
        }
    };
    let pill_width = (Span::raw(&pill).width() as u16).min(area.width);
    frame.render_widget(
        Paragraph::new(pill).style(theme::surface().fg(theme::text_dim())),
        Rect::new(area.x, pill_y, pill_width, 1),
    );
}

fn draw_timeline(frame: &mut Frame<'_>, app: &App, selected: Option<&AgentMemberItem>, area: Rect) {
    if area.height == 0 {
        return;
    }
    let members = app.agent_members();
    let name = |id: &Option<String>, kind: AgentActivityKind| match id {
        Some(id) => members
            .iter()
            .find(|member| &member.id == id)
            .map_or_else(|| "agent".to_owned(), |member| member.description.clone()),
        None if kind == AgentActivityKind::Steer => "You".to_owned(),
        None => "Yeet".to_owned(),
    };
    let entries: Vec<&AgentActivityItem> = app
        .state
        .agent_group
        .activity
        .iter()
        .rev()
        .filter(|entry| {
            selected.is_none_or(|member| {
                entry.from.as_deref() == Some(member.id.as_str())
                    || entry.to.as_deref() == Some(member.id.as_str())
            })
        })
        .take(area.height as usize)
        .collect();
    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new("No activity yet.").style(muted()),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return;
    }
    let actor_width = (area.width as usize / 4).clamp(8, 24);
    // A running member's newest entry, when it is a tool, is what it is
    // doing now.
    let activity = &app.state.agent_group.activity;
    let current: Vec<&AgentActivityItem> = members
        .iter()
        .filter(|member| member.activity_state == "tool_call")
        .filter_map(|member| {
            activity
                .iter()
                .rev()
                .find(|entry| entry.from.as_deref() == Some(member.id.as_str()))
        })
        .filter(|entry| entry.kind == AgentActivityKind::Tool)
        .collect();
    for (index, entry) in entries.iter().enumerate() {
        let actor = match &entry.to {
            Some(_) => format!(
                "{} → {}",
                name(&entry.from, entry.kind),
                name(&entry.to, entry.kind)
            ),
            None => name(&entry.from, entry.kind),
        };
        let running = current.iter().any(|now| std::ptr::eq(*now, *entry));
        let (icon, text_style) = match entry.kind {
            _ if running => (
                spinner(),
                Style::default().fg(theme::text()).add_modifier(BOLD),
            ),
            AgentActivityKind::Message | AgentActivityKind::Steer => {
                (message_icon(), Style::default().fg(theme::text_dim()))
            }
            AgentActivityKind::Tool => (
                if icons_enabled() {
                    tool_icon(entry.tool.as_deref().unwrap_or_default())
                } else {
                    "·"
                },
                Style::default().fg(theme::text_dim()),
            ),
            AgentActivityKind::Finished => (
                if icons_enabled() { "\u{f00c}" } else { "✓" },
                Style::default().fg(theme::text()).add_modifier(BOLD),
            ),
            AgentActivityKind::Failed => (
                if icons_enabled() { "\u{f00d}" } else { "✗" },
                Style::default().fg(theme::error()),
            ),
            AgentActivityKind::Stopped => ("■", muted()),
        };
        let text_width = (area.width as usize).saturating_sub(7 + actor_width + 3);
        let line = Line::from(vec![
            Span::styled(format!("{:<6} ", age(&entry.at)), muted()),
            Span::styled(
                format!("{:<actor_width$} ", fit(&actor, actor_width)),
                muted(),
            ),
            Span::styled(icon, if running { glyph_running() } else { muted() }),
            Span::raw(" "),
            Span::styled(fit(&entry.text, text_width), text_style),
        ]);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x, area.y + index as u16, area.width, 1),
        );
    }
}

fn draw_inspector(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if area.width < 12 || area.height < 6 {
        return;
    }
    let members = app.agent_members();
    let group = &app.state.agent_group;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let value_width = (area.width as usize).saturating_sub(LABEL_WIDTH).max(8);
    let field = |label: &str, value: String| -> Vec<Line<'static>> {
        wrap(&value, value_width)
            .into_iter()
            .enumerate()
            .map(|(index, part)| {
                let label = if index == 0 { label } else { "" };
                Line::from(vec![
                    Span::styled(format!("{label:<LABEL_WIDTH$}"), muted()),
                    Span::styled(part, Style::default().fg(theme::text())),
                ])
            })
            .collect()
    };
    let section = |title: &str| {
        Line::from(Span::styled(
            title.to_owned(),
            Style::default().fg(theme::muted()).add_modifier(BOLD),
        ))
    };
    let tokens = |input: u64, output: u64| {
        format!(
            "{} in · {} out",
            compact_number(input),
            compact_number(output)
        )
    };
    match app.selected_agent() {
        Some(member) => {
            lines.push(title_line(&member.description));
            lines.push(Line::default());
            lines.push(section("STATE"));
            lines.extend(field("State", member_state(member).1.to_owned()));
            if let Some(task) = task_of(app, member) {
                lines.extend(field("Task", task.to_owned()));
            }
            lines.extend(field("Role", member.role.clone()));
            lines.extend(field("Model", member.model.clone()));
            lines.extend(field(
                "Tokens",
                tokens(member.input_tokens, member.output_tokens),
            ));
            if let Some(elapsed) = elapsed(&member.started_at) {
                lines.extend(field("Elapsed", elapsed));
            }
            let (sends, gets) = peers(app, member);
            if !sends.is_empty() || !gets.is_empty() {
                lines.push(Line::default());
                lines.push(section("WORKS WITH"));
                if !sends.is_empty() {
                    lines.extend(field("Sends to", sends.join(", ")));
                }
                if !gets.is_empty() {
                    lines.extend(field("Gets from", gets.join(", ")));
                }
            }
            if let Some(summary) = member.summary.as_deref().filter(|s| !s.trim().is_empty()) {
                lines.push(Line::default());
                lines.push(section("RESULT"));
                lines.extend(summary.lines().map(|line| {
                    Line::from(Span::styled(
                        line.to_owned(),
                        Style::default().fg(theme::text_dim()),
                    ))
                }));
            }
        }
        None => {
            lines.push(title_line(group_title(app)));
            lines.push(Line::default());
            lines.push(section("STATE"));
            let (running, waiting) = counts(members);
            lines.extend(field("Group", group.status.clone()));
            lines.extend(field(
                "Members",
                format!(
                    "{running} active · {waiting} waiting · {} total",
                    members.len()
                ),
            ));
            if let Some(objective) = group.objective.as_deref() {
                lines.push(Line::default());
                lines.push(section("OBJECTIVE"));
                lines.extend(objective.lines().map(|line| {
                    Line::from(Span::styled(
                        line.to_owned(),
                        Style::default().fg(theme::text_dim()),
                    ))
                }));
            }
            let mut models: Vec<&str> = members.iter().map(|m| m.model.as_str()).collect();
            models.sort_unstable();
            models.dedup();
            if !models.is_empty() {
                lines.extend(field(
                    if models.len() == 1 { "Model" } else { "Models" },
                    models.join(", "),
                ));
            }
            lines.extend(field(
                "Tokens",
                tokens(group.input_tokens, group.output_tokens),
            ));
            if let Some(elapsed) = group.started_at.as_deref().and_then(elapsed) {
                lines.extend(field("Elapsed", elapsed));
            }
            if let Some(cost) = group.estimated_cost_usd {
                lines.extend(field("Est. cost", format!("${cost:.2}")));
            }
            if group.budget.output_limit_tokens > 0 {
                lines.push(Line::default());
                lines.push(section("SHARED BUDGET"));
                lines.extend(field(
                    "Output",
                    format!(
                        "{} / {} tokens",
                        compact_number(group.budget.output_used_tokens),
                        compact_number(group.budget.output_limit_tokens),
                    ),
                ));
                lines.extend(field(
                    "Reserved",
                    format!(
                        "{} coordination · {} synthesis",
                        compact_number(group.budget.coordination_reserve_tokens),
                        compact_number(group.budget.synthesis_reserve_tokens),
                    ),
                ));
            }
            if group.budget.cost_limit_usd > 0.0 {
                lines.extend(field(
                    "Cost limit",
                    format!("${:.2}", group.budget.cost_limit_usd),
                ));
                if let Some(used) = group.budget.estimated_cost_used_usd {
                    lines.extend(field("Cost used", format!("${used:.2}")));
                }
            }
            if let Some(result) = group
                .final_result
                .as_deref()
                .or(group.checkpoint_summary.as_deref())
                .filter(|result| !result.trim().is_empty())
            {
                lines.push(Line::default());
                lines.push(section(if group.final_result.is_some() {
                    "GROUP RESULT"
                } else {
                    "LATEST CHECKPOINT"
                }));
                lines.extend(result.lines().map(|line| {
                    Line::from(Span::styled(
                        line.to_owned(),
                        Style::default().fg(theme::text_dim()),
                    ))
                }));
            }
            if !group.shared_findings.is_empty() {
                lines.push(Line::default());
                lines.push(section("SHARED FINDINGS"));
                for finding in group.shared_findings.iter().rev().take(4).rev() {
                    let member = members
                        .iter()
                        .find(|member| member.id == finding.member_id)
                        .map_or("member", |member| member.description.as_str());
                    lines.extend(field("From", member.to_owned()));
                    lines.extend(field("Finding", finding.summary.clone()));
                }
            }
            let review: Vec<&AgentMemberItem> = members
                .iter()
                .filter(|member| member.task_status == "needs_verification")
                .collect();
            if !review.is_empty() {
                lines.push(Line::default());
                lines.push(section("NEEDS REVIEW"));
                for member in review {
                    lines.extend(field("From", member.description.clone()));
                    if let Some(summary) = member.summary.as_deref() {
                        lines.extend(field(
                            "Reports",
                            summary.lines().next().unwrap_or_default().to_owned(),
                        ));
                    }
                }
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        ),
    );
}

/// Greedy word wrap; long words are split at the width.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let mut word = word.to_owned();
        loop {
            let line = lines.last_mut().expect("at least one line");
            let used = Span::raw(line.as_str()).width();
            let needed = Span::raw(word.as_str()).width() + usize::from(used > 0);
            if used + needed <= width {
                if used > 0 {
                    line.push(' ');
                }
                line.push_str(&word);
                break;
            }
            if used > 0 {
                lines.push(String::new());
                continue;
            }
            let cut = word
                .char_indices()
                .nth(width.max(1))
                .map_or(word.len(), |(cut, _)| cut);
            line.push_str(&word[..cut]);
            word = word[cut..].to_owned();
            if word.is_empty() {
                break;
            }
            lines.push(String::new());
        }
    }
    lines
}

fn title_line(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        title.to_owned(),
        Style::default().fg(theme::text()).add_modifier(BOLD),
    ))
}

fn group_title(app: &App) -> &str {
    app.state
        .agent_group
        .objective
        .as_deref()
        .unwrap_or_else(|| conversation_title(app))
}

/// The first message the primary agent sent a member: its assignment.
fn task_of<'a>(app: &'a App, member: &AgentMemberItem) -> Option<&'a str> {
    app.state
        .agent_group
        .activity
        .iter()
        .find(|entry| {
            entry.from.is_none()
                && entry.to.as_deref() == Some(member.id.as_str())
                && entry.kind == AgentActivityKind::Message
        })
        .map(|entry| entry.text.as_str())
}

/// Members this one has messaged, and who has messaged it.
fn peers(app: &App, member: &AgentMemberItem) -> (Vec<String>, Vec<String>) {
    let name = |id: Option<&str>| {
        id.and_then(|id| app.agent_members().iter().find(|m| m.id == id))
            .map_or_else(|| "Yeet".to_owned(), |m| m.description.clone())
    };
    let (mut sends, mut gets) = (Vec::new(), Vec::new());
    for entry in &app.state.agent_group.activity {
        if entry.kind != AgentActivityKind::Message {
            continue;
        }
        if entry.from.as_deref() == Some(member.id.as_str()) && entry.to.is_some() {
            sends.push(name(entry.to.as_deref()));
        } else if entry.to.as_deref() == Some(member.id.as_str()) {
            gets.push(name(entry.from.as_deref()));
        }
    }
    for list in [&mut sends, &mut gets] {
        let mut seen = std::collections::HashSet::new();
        list.retain(|name| seen.insert(name.clone()));
    }
    (sends, gets)
}

/// Rail glyph and state word for a member.
fn member_state(member: &AgentMemberItem) -> (&'static str, &'static str) {
    match (
        member.status.as_str(),
        member.activity_state.as_str(),
        member.task_status.as_str(),
    ) {
        (_, _, "failed") => ("!", "failed"),
        (_, _, "cancelled") => ("·", "cancelled"),
        ("stopped", _, _) => ("·", "stopped"),
        (_, "reasoning", _) => (spinner(), "reasoning"),
        (_, "tool_call", _) => (spinner(), "tool call"),
        (_, "provider_activity", _) => (spinner(), "working"),
        (_, "waiting_for_input", _) => ("?", "input"),
        (_, "queued", _) => (spinner(), "queued"),
        (_, _, "needs_verification") => ("?", "review"),
        (_, _, "verified" | "reported" | "done") => ("·", "done"),
        ("running", _, _) => (spinner(), "running"),
        _ => ("·", "idle"),
    }
}

fn verb(member: &AgentMemberItem) -> &'static str {
    match member_state(member).1 {
        "running" | "reasoning" | "working" | "queued" => "is working on:",
        "tool call" => "is calling a tool for:",
        "input" => "is waiting for input on:",
        "review" => "is waiting for review of:",
        "failed" => "failed:",
        "stopped" | "cancelled" => "stopped working on:",
        _ => "finished:",
    }
}

fn counts(members: &[AgentMemberItem]) -> (usize, usize) {
    members.iter().fold((0, 0), |(running, waiting), member| {
        match member_state(member).1 {
            "running" | "reasoning" | "tool call" | "working" | "queued" => (running + 1, waiting),
            "input" | "review" => (running, waiting + 1),
            _ => (running, waiting),
        }
    })
}

fn glyph_style(member: &AgentMemberItem) -> Style {
    match member_state(member).1 {
        "running" | "reasoning" | "tool call" | "working" | "queued" => glyph_running(),
        "input" | "review" => Style::default().fg(theme::warning()),
        "failed" => Style::default().fg(theme::error()),
        _ => muted(),
    }
}

fn glyph_running() -> Style {
    Style::default().fg(theme::accent()).add_modifier(BOLD)
}

fn muted() -> Style {
    Style::default().fg(theme::muted())
}

fn icons_enabled() -> bool {
    icons::home() != "⌂"
}

fn message_icon() -> &'static str {
    if icons_enabled() {
        "\u{f075}"
    } else {
        "›"
    }
}

fn since(at: &str) -> Option<chrono::Duration> {
    let at = DateTime::parse_from_rfc3339(at).ok()?;
    Some((Utc::now() - at.with_timezone(&Utc)).max(chrono::Duration::zero()))
}

/// Compact age for timeline rows: `now`, `41s`, `2m`, `3h`, `2d`.
fn age(at: &str) -> String {
    let Some(delta) = since(at) else {
        return String::new();
    };
    let seconds = delta.num_seconds();
    match seconds {
        0..=9 => "now".into(),
        10..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Inspector elapsed time: `14m 08s`, `2h 05m`.
fn elapsed(at: &str) -> Option<String> {
    let seconds = since(at)?.num_seconds();
    Some(if seconds >= 3600 {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentGroupItem;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn group_and_member_views_render_rail_timeline_and_inspector() {
        let mut app = App::default();
        let now = Utc::now().to_rfc3339();
        let member = |id: &str, status: &str, task_status: &str| AgentMemberItem {
            id: id.into(),
            description: id.into(),
            role: "researcher".into(),
            model: "opus-5.5".into(),
            status: status.into(),
            task_status: task_status.into(),
            activity_state: if id == "Planner" {
                "reasoning".into()
            } else {
                String::new()
            },
            started_at: now.clone(),
            input_tokens: 312_000,
            output_tokens: 41_000,
            ..Default::default()
        };
        let entry = |from: Option<&str>, to: Option<&str>, kind, text: &str| AgentActivityItem {
            at: now.clone(),
            from: from.map(Into::into),
            to: to.map(Into::into),
            kind,
            tool: None,
            text: text.into(),
        };
        app.state.agent_group = AgentGroupItem {
            group_id: "group-1".into(),
            objective: Some("Inspect feasibility across the candidate radii".into()),
            status: "running".into(),
            members: vec![
                member("Planner", "idle", "running"),
                member("Verification", "idle", "needs_verification"),
            ],
            activity: vec![
                entry(
                    None,
                    Some("Planner"),
                    AgentActivityKind::Message,
                    "Sweep feasible radii",
                ),
                entry(
                    Some("Verification"),
                    None,
                    AgentActivityKind::Finished,
                    "Offline heading sweep accepted",
                ),
                entry(
                    Some("Planner"),
                    None,
                    AgentActivityKind::Tool,
                    "NTRS TAEM energy notes",
                ),
            ],
            started_at: Some(now.clone()),
            input_tokens: 1_200_000,
            output_tokens: 148_000,
            budget: crate::model::AgentGroupBudgetItem {
                output_limit_tokens: 500_000,
                output_used_tokens: 148_000,
                cost_limit_usd: 15.0,
                estimated_cost_used_usd: Some(4.25),
                coordination_reserve_tokens: 30_000,
                synthesis_reserve_tokens: 45_000,
                ..Default::default()
            },
            final_result: Some("Candidate radius 1.8 remains feasible.".into()),
            shared_findings: vec![crate::model::AgentGroupFindingItem {
                member_id: "Planner".into(),
                task_id: "task-1".into(),
                at: now.clone(),
                summary: "1.8 is the best candidate radius".into(),
            }],
            ..Default::default()
        };
        app.open_agents();
        let mut terminal = Terminal::new(TestBackend::new(144, 40)).unwrap();
        let screen = |terminal: &mut Terminal<TestBackend>, app: &mut App| {
            terminal
                .draw(|frame| crate::tui::ui::draw(frame, app))
                .unwrap();
            let buffer = terminal.backend().buffer();
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let group = screen(&mut terminal, &mut app);
        for expected in [
            "1/2",
            "Group is running",
            "waiting",
            "Cancel group",
            "Yeet → Planner",
            "NTRS TAEM energy notes",
            "1.2M in · 148k out",
            "Inspect feasibility",
            "GROUP RESULT",
            "SHARED BUDGET",
            "SHARED FINDINGS",
            "NEEDS REVIEW",
            "1 waiting · 1 running",
        ] {
            assert!(group.contains(expected), "missing {expected:?}:\n{group}");
        }

        let planner = app
            .agents
            .targets
            .iter()
            .find(|(_, action)| *action == AgentAction::Select(Some("Planner".into())))
            .unwrap()
            .0;
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: planner.x + 2,
            row: planner.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        let detail = screen(&mut terminal, &mut app);
        for expected in [
            "Planner is working on: Sweep feasible radii",
            "Gets from  Yeet",
            "312k in · 41k out",
        ] {
            assert!(detail.contains(expected), "missing {expected:?}:\n{detail}");
        }
        assert!(
            !detail.contains("Offline heading sweep"),
            "member filter:\n{detail}"
        );
    }
}
