//! Modal dialogs and settings forms.
use super::support::theme;
use super::support::{
    responsive,
    text::{cell_width, compact_number, truncate_end, truncate_middle},
};
use crate::{
    app::{App, SettingsEditKind, SettingsSection},
    model::{CapabilityToggleItem, reasoning_levels_for_model},
};
#[path = "agent_group.rs"]
mod agent_group;
#[path = "navigation.rs"]
mod navigation;
#[path = "status.rs"]
mod status;
pub(crate) use agent_group::draw_agent_group;
pub(crate) use status::draw_status_dialog;

pub(super) use navigation::{draw_goal, draw_models, draw_reasoning, draw_sessions};

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    prelude::{Line, Modifier, Span, Style, Stylize, Text},
    widgets::{List, ListItem, ListState, Paragraph, Wrap},
};

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    responsive::modal_rect(area, percent_x, percent_y)
}

#[path = "capabilities.rs"]
mod capabilities;
pub(super) use capabilities::{draw_capabilities, draw_capability_detail};

pub(super) fn draw_auth(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(90, 78, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.state.auth_working {
        " Authentication & usage · refreshing… · Esc close "
    } else {
        " Authentication & usage · ↑/↓ navigate · Enter/l sign in · k API key · x sign out · r refresh · Esc close "
    };
    let block = theme::modal_block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);

    if app.state.auth_working && app.state.auth_providers.is_empty() {
        frame.render_widget(
            Paragraph::new("Loading providers and quota headroom…").fg(theme::text_dim()),
            chunks[0],
        );
    } else if app.state.auth_providers.is_empty() {
        frame.render_widget(Paragraph::new("No providers are available."), chunks[0]);
    } else {
        let rows = app.state.auth_providers.iter().map(|item| {
            let (marker, status) = if item.error.is_some() {
                ("×", "error")
            } else if item.authenticated {
                ("●", "authenticated")
            } else {
                ("○", "not authenticated")
            };
            let detail = item.error.as_deref().unwrap_or(item.method.as_str());
            let mut lines = vec![Line::from(vec![
                Span::raw(format!("{marker} {:<18} {:<19}", item.provider, status)),
                Span::styled(
                    truncate_end(detail, 32),
                    Style::default().fg(theme::muted()),
                ),
            ])];
            if let Some(usage) = item.usage.as_ref() {
                if !usage.windows.is_empty() {
                    let mut summary = usage
                        .windows
                        .iter()
                        .take(3)
                        .map(|window| {
                            format!("{} {}% left", window.label, window.remaining_percent)
                        })
                        .collect::<Vec<_>>()
                        .join(" · ");
                    if usage.windows.len() > 3 {
                        summary.push_str(&format!(" · +{}", usage.windows.len() - 3));
                    }
                    let plan = usage
                        .plan
                        .as_deref()
                        .map(|plan| format!("{plan} · "))
                        .unwrap_or_default();
                    lines.push(Line::from(vec![
                        Span::styled("  usage  ", Style::default().fg(theme::text_dim())),
                        Span::styled(
                            truncate_end(&format!("{plan}{summary}"), 72),
                            Style::default().fg(theme::muted()),
                        ),
                    ]));
                } else if usage.source != "none" {
                    lines.push(Line::from(vec![
                        Span::styled("  usage  ", Style::default().fg(theme::text_dim())),
                        Span::styled(
                            truncate_end(
                                &format!(
                                    "{} · {}",
                                    usage.source,
                                    usage.message.as_deref().unwrap_or("unavailable")
                                ),
                                72,
                            ),
                            Style::default().fg(theme::muted()),
                        ),
                    ]));
                }
            }
            ListItem::new(lines)
        });
        let list = List::new(rows)
            .highlight_style(theme::selected())
            .highlight_symbol("▸ ");
        let mut state = ListState::default().with_selected(Some(app.popup_index));
        frame.render_stateful_widget(list, chunks[0], &mut state);
    }

    if let Some(notice) = app.state.auth_notice.as_deref() {
        frame.render_widget(
            Paragraph::new(truncate_end(notice, chunks[1].width as usize)).fg(theme::muted()),
            chunks[1],
        );
    }
}

pub(super) fn draw_auth_key(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(64, 28, frame.area());
    theme::modal_backdrop(frame, area);
    let provider = app.active_auth_provider.as_deref().unwrap_or("provider");
    let block = theme::modal_block(format!(" API key · {provider} · Enter save · Esc cancel "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let key = app
        .editor_fields
        .first()
        .map(String::as_str)
        .unwrap_or_default();
    let masked = masked_secret(key);
    frame.render_widget(
        Paragraph::new("Paste or type the provider API key.").fg(theme::muted()),
        Rect::new(inner.x, inner.y, inner.width, 2),
    );
    let field = Rect::new(
        inner.x,
        inner.y.saturating_add(3),
        inner.width,
        3.min(inner.height.saturating_sub(3)),
    );
    let field_block = theme::panel_block("API key");
    let field_inner = field_block.inner(field);
    frame.render_widget(field_block, field);
    frame.render_widget(Paragraph::new(masked.clone()), field_inner);
    let cursor_x = field_inner
        .x
        .saturating_add(masked.chars().count() as u16)
        .min(field_inner.right().saturating_sub(1));
    frame.set_cursor_position(Position::new(cursor_x, field_inner.y));
}

pub(super) fn draw_providers(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(88, 72, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(
        " Providers · ↑/↓ navigate · n new · Enter edit · d delete · r refresh · Esc close ",
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
    if app.state.providers_working && app.state.provider_configurations.is_empty() {
        frame.render_widget(
            Paragraph::new("Loading custom providers…").fg(theme::text_dim()),
            chunks[0],
        );
    } else if app.state.provider_configurations.is_empty() {
        frame.render_widget(
            Paragraph::new("No custom providers. Press n to add one."),
            chunks[0],
        );
    } else {
        let row_width = chunks[0].width.saturating_sub(2) as usize;
        let rows = app.state.provider_configurations.iter().map(|provider| {
            let auth = if provider.require_api_key {
                "key required"
            } else {
                "key optional"
            };
            let headers = if provider.header_count > 0 {
                format!(" · {} headers", provider.header_count)
            } else {
                String::new()
            };
            let metadata = if row_width >= 80 {
                format!("  {auth}{headers}")
            } else if row_width >= 64 {
                format!("  {auth}")
            } else {
                String::new()
            };
            let content_width = row_width.saturating_sub(cell_width(&metadata));
            let id_budget = content_width.saturating_sub(16).clamp(8, 18);
            let id = truncate_middle(&provider.id, id_budget);
            let id_padding = id_budget.saturating_sub(cell_width(&id));
            let url_budget = content_width
                .saturating_sub(id_budget.saturating_add(1))
                .min(42);
            ListItem::new(Line::from(vec![
                Span::raw(format!("{id}{} ", " ".repeat(id_padding))),
                Span::styled(
                    truncate_middle(&provider.base_url, url_budget),
                    Style::default().fg(theme::text_dim()),
                ),
                Span::styled(metadata, Style::default().fg(theme::muted())),
            ]))
        });
        let list = List::new(rows)
            .highlight_style(theme::selected())
            .highlight_symbol("▸ ");
        let mut state = ListState::default().with_selected(Some(app.popup_index));
        frame.render_stateful_widget(list, chunks[0], &mut state);
    }
    if let Some(id) = app.pending_provider_delete_id.as_deref() {
        let warning_width = chunks[1].width as usize;
        let id_budget = warning_width.saturating_sub(cell_width("Delete provider ?"));
        let identity = format!("Delete provider {}?", truncate_middle(id, id_budget));
        let action = truncate_end("d/Delete again to confirm · ↑/↓ cancels", warning_width);
        frame.render_widget(
            Paragraph::new(Text::from(vec![Line::from(identity), Line::from(action)]))
                .fg(theme::error()),
            chunks[1],
        );
    } else if let Some(notice) = app.state.providers_notice.as_deref() {
        frame.render_widget(
            Paragraph::new(truncate_end(notice, chunks[1].width as usize)).fg(theme::muted()),
            chunks[1],
        );
    }
}

pub(super) fn draw_provider_edit(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(72, 42, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.editing_provider_id.is_some() {
        " Edit provider "
    } else {
        " New provider "
    };
    let block = theme::modal_block(format!(
        "{title}· Tab fields · Space toggle · Enter save · Esc cancel "
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let id = app.editor_fields.first().cloned().unwrap_or_default();
    let url = app.editor_fields.get(1).cloned().unwrap_or_default();
    let rows = vec![
        (
            if app.editing_provider_id.is_some() {
                "Provider ID (locked)"
            } else {
                "Provider ID"
            }
            .to_owned(),
            id,
        ),
        ("Base URL".to_owned(), url),
        (
            "Require API key".to_owned(),
            if app.editor_toggle {
                "on".into()
            } else {
                "off".into()
            },
        ),
    ];
    draw_form_rows(
        frame,
        inner,
        &rows,
        app.editor_index,
        app.editor_index < 2 && !(app.editor_index == 0 && app.editing_provider_id.is_some()),
    );
}

pub(super) fn draw_settings(frame: &mut Frame<'_>, app: &App) {
    let view = app.application.settings_projection().view;
    let area = centered_rect(84, 72, frame.area());
    theme::modal_backdrop(frame, area);
    let title = format!(" {} · {} · ↑/↓ navigate · Enter/Space change/open · r refresh · Esc close ", view.title, if view.working { "saving…" } else { &view.subtitle });
    let block = theme::modal_block(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
    let mut rows = Vec::new();
    let mut selected = None;
    for section in &view.sections {
        rows.push(ListItem::new(Line::styled(section.label.clone(), Style::default().fg(theme::muted()).add_modifier(Modifier::BOLD))));
        for control in &section.controls {
            if view.selected.as_deref() == Some(control.id.as_str()) { selected = Some(rows.len()); }
            rows.push(ListItem::new(Line::from(vec![
                Span::styled(format!("{} {:<22}", settings_icon(control.icon), control.label), Style::default().fg(if control.enabled {theme::text()} else {theme::muted()}).add_modifier(Modifier::BOLD)),
                Span::styled(control.value.clone(), Style::default().fg(if control.enabled {theme::accent()} else {theme::muted()})),
                Span::styled(format!(" · {}", control.detail), Style::default().fg(theme::muted())),
            ])));
        }
    }
    let list = List::new(rows).highlight_style(theme::selected()).highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(list, chunks[0], &mut state);
    let status = view.notice.as_deref().unwrap_or(&view.subtitle);
    frame.render_widget(Paragraph::new(truncate_end(status, chunks[1].width as usize)).fg(theme::muted()), chunks[1]);
}
fn settings_icon(icon: crate::shared_ui::settings::SettingsIcon) -> &'static str {
    use crate::shared_ui::settings::SettingsIcon::*;
    match icon {
        Appearance | Theme => "◐", Settings | Policy => "⚙", Memory => "\u{f1c0}", Web => "\u{f0ac}",
        Model | Reasoning => "\u{f0eb}", Context => "\u{f15c}", Agents => "\u{f0c0}", Permissions => "\u{f023}",
        Provider => "\u{f084}", Capabilities => "\u{f0e7}",
    }
}

pub(super) fn draw_sandbox_presets(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(78, 54, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.state.sandbox_working {
        " Sandbox · saving… · Esc back "
    } else {
        " Sandbox · ↑/↓ navigate · Enter apply/open · r refresh · Esc back "
    };
    let block = theme::modal_block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
    let Some(settings) = app.state.sandbox_settings.as_ref() else {
        frame.render_widget(
            Paragraph::new("Loading project settings…").fg(theme::text_dim()),
            chunks[0],
        );
        return;
    };
    let rows = vec![
        (
            "safe",
            "Safe",
            "strict shell isolation · workspace shell access disabled".to_owned(),
        ),
        (
            "balanced",
            "Balanced",
            "sandboxed shell · workspace reads · restricted writes ask first".to_owned(),
        ),
        (
            "unlimited",
            "Unlimited",
            "normal user authority · approvals handled automatically".to_owned(),
        ),
        (
            "advanced",
            "Advanced sandbox rules",
            format!("current policy: {}", settings.preset),
        ),
    ];
    let items = rows.into_iter().map(|(id, name, value)| {
        let marker = if id == settings.preset { "●" } else { "○" };
        let marker = if id == "advanced" { "◇" } else { marker };
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{marker} {name:<25}"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(value, Style::default().fg(theme::text_dim())),
        ]))
    });
    let list = List::new(items)
        .highlight_style(theme::selected())
        .highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(app.popup_index));
    frame.render_stateful_widget(list, chunks[0], &mut state);
    if let Some(notice) = app.state.sandbox_notice.as_deref() {
        frame.render_widget(
            Paragraph::new(truncate_end(notice, chunks[1].width as usize)).fg(theme::muted()),
            chunks[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new("Applying a preset replaces advanced sandbox rules.").fg(theme::muted()),
            chunks[1],
        );
    }
}

fn sandbox_policy_tab_line(current: SettingsSection, width: usize) -> Line<'static> {
    let sections = [
        (SettingsSection::Core, "Core"),
        (SettingsSection::Workspace, "Workspace"),
        (SettingsSection::Network, "Network"),
        (SettingsSection::Environment, "Environment"),
        (SettingsSection::Secrets, "Secrets"),
        (SettingsSection::Limits, "Limits"),
    ];
    let current_index = sections
        .iter()
        .position(|(section, _)| *section == current)
        .unwrap_or(0);
    let window_width = |start: usize, end: usize| {
        let labels = sections[start..end]
            .iter()
            .map(|(_, label)| cell_width(label))
            .sum::<usize>();
        let gaps = end.saturating_sub(start + 1) * 2;
        labels + gaps + usize::from(start > 0) * 2 + usize::from(end < sections.len()) * 2
    };

    let mut start = 0usize;
    let mut end = sections.len();
    while end.saturating_sub(start) > 1 && window_width(start, end) > width {
        let hidden_left_distance = current_index.saturating_sub(start);
        let hidden_right_distance = end.saturating_sub(current_index + 1);
        if hidden_left_distance > hidden_right_distance {
            start += 1;
        } else {
            end -= 1;
        }
    }

    let mut tabs = Vec::new();
    if start > 0 {
        tabs.push(Span::styled("‹ ", Style::default().fg(theme::muted())));
    }
    for (visible_index, (section, label)) in sections[start..end].iter().enumerate() {
        if visible_index > 0 {
            tabs.push(Span::styled("  ", Style::default().fg(theme::muted())));
        }
        let style = if *section == current {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::muted())
        };
        tabs.push(Span::styled(*label, style));
    }
    if end < sections.len() {
        tabs.push(Span::styled(" ›", Style::default().fg(theme::muted())));
    }
    Line::from(tabs)
}

pub(super) fn draw_sandbox_policy(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(92, 80, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(
        " Sandbox policy · Tab/←/→ section · Enter/Space edit · n add · d remove · Esc back ",
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(settings) = app.state.sandbox_settings.as_ref() else {
        frame.render_widget(
            Paragraph::new("Loading settings…").fg(theme::text_dim()),
            inner,
        );
        return;
    };
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(inner);

    let current = app.settings_section.unwrap_or(SettingsSection::Core);
    frame.render_widget(
        Paragraph::new(sandbox_policy_tab_line(current, chunks[0].width as usize)),
        chunks[0],
    );

    let rows: Vec<ListItem<'static>> = match app.settings_section {
        Some(SettingsSection::Core) | None => vec![
            ListItem::new(Line::from(vec![
                Span::raw("Execution mode         "),
                Span::styled(
                    settings.execution_mode.clone(),
                    Style::default().fg(theme::text_dim()),
                ),
            ])),
            ListItem::new(Line::from(vec![
                Span::raw("Auto approval          "),
                Span::styled(
                    if settings.auto_approve { "on" } else { "off" },
                    Style::default().fg(theme::text_dim()),
                ),
            ])),
            ListItem::new(Line::from(vec![
                Span::raw("Scratch writes         "),
                Span::styled(
                    if settings.scratch_writable {
                        "on"
                    } else {
                        "off"
                    },
                    Style::default().fg(theme::text_dim()),
                ),
            ])),
            ListItem::new(Line::from(vec![
                Span::raw("Reset policy           "),
                Span::styled(
                    "restore Safe defaults",
                    Style::default().fg(theme::text_dim()),
                ),
            ])),
        ],
        Some(SettingsSection::Workspace) => {
            let row_width = chunks[1].width.saturating_sub(2) as usize;
            let mut rows = vec![ListItem::new(Line::from(vec![
                Span::raw("Workspace read mode  "),
                Span::styled(
                    settings.workspace_mode.clone(),
                    Style::default().fg(theme::text_dim()),
                ),
            ]))];
            let path_label = if row_width >= 64 {
                "Path                 "
            } else {
                "Path  "
            };
            let path_budget = row_width.saturating_sub(cell_width(path_label));
            rows.extend(settings.workspace_paths.iter().map(|path| {
                ListItem::new(Line::from(vec![
                    Span::raw(path_label),
                    Span::styled(
                        truncate_middle(path, path_budget),
                        Style::default().fg(theme::text_dim()),
                    ),
                ]))
            }));
            rows
        }
        Some(SettingsSection::Network) => {
            let row_width = chunks[1].width.saturating_sub(2) as usize;
            settings
                .network_allow
                .iter()
                .map(|item| {
                    let port = item
                        .port
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "*".into());
                    let suffix = format!(":{port}");
                    let host_budget = row_width.saturating_sub(cell_width(&suffix)).max(1);
                    ListItem::new(format!(
                        "{}{}",
                        truncate_middle(&item.host, host_budget),
                        suffix
                    ))
                })
                .collect()
        }
        Some(SettingsSection::Environment) => {
            let row_width = chunks[1].width.saturating_sub(2) as usize;
            settings
                .environment
                .iter()
                .map(|item| {
                    let key_budget = row_width.saturating_sub(10).clamp(1, 24);
                    let key = truncate_middle(&item.key, key_budget);
                    let key_padding = key_budget.saturating_sub(cell_width(&key));
                    let value_budget = row_width
                        .saturating_sub(key_budget.saturating_add(2))
                        .min(44);
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{key}{}  ", " ".repeat(key_padding))),
                        Span::styled(
                            truncate_end(&item.value, value_budget),
                            Style::default().fg(theme::text_dim()),
                        ),
                    ]))
                })
                .collect()
        }
        Some(SettingsSection::Secrets) => settings
            .secret_ids
            .iter()
            .cloned()
            .map(ListItem::new)
            .collect(),
        Some(SettingsSection::Limits) => vec![
            ListItem::new(format!(
                "Wall time             {} seconds",
                settings.limits.wall_time_seconds
            )),
            ListItem::new(format!(
                "Stdout                {} bytes",
                settings.limits.max_stdout_bytes
            )),
            ListItem::new(format!(
                "Stderr                {} bytes",
                settings.limits.max_stderr_bytes
            )),
            ListItem::new(format!(
                "Memory                {} bytes",
                settings.limits.max_memory_bytes
            )),
            ListItem::new(format!(
                "Processes             {}",
                settings.limits.max_processes
            )),
        ],
    };
    if rows.is_empty() {
        let empty = match app.settings_section {
            Some(SettingsSection::Network) => "No network grants. Press n to add one.",
            Some(SettingsSection::Environment) => "No environment variables. Press n to add one.",
            Some(SettingsSection::Secrets) => "No secret IDs. Press n to add one.",
            _ => "No entries.",
        };
        frame.render_widget(Paragraph::new(empty).fg(theme::text_dim()), chunks[1]);
    } else {
        let list = List::new(rows)
            .highlight_style(theme::selected())
            .highlight_symbol("▸ ");
        let mut state = ListState::default().with_selected(Some(app.popup_index));
        frame.render_stateful_widget(list, chunks[1], &mut state);
    }

    let (status, status_color) = if app.pending_sandbox_reset {
        (
            "Reset the sandbox policy to Safe defaults? Press Enter/Space again to confirm · navigation cancels".to_owned(),
            theme::error(),
        )
    } else if let Some(notice) = app.state.sandbox_notice.as_deref() {
        (
            truncate_end(notice, chunks[2].width as usize),
            theme::muted(),
        )
    } else {
        (
            format!(
                "Preset: {} · advanced changes are reported as custom",
                settings.preset
            ),
            theme::muted(),
        )
    };
    frame.render_widget(
        Paragraph::new(status)
            .fg(status_color)
            .wrap(Wrap { trim: false }),
        chunks[2],
    );
}

pub(super) fn draw_settings_edit(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(70, 40, frame.area());
    theme::modal_backdrop(frame, area);
    let view = app.application.settings_projection().view;
    let (title, labels): (&str, Vec<&str>) = if let Some(editor) = &view.editor {
        (&editor.label, vec![&editor.hint])
    } else { match app.settings_edit_kind.as_ref() {
        Some(SettingsEditKind::WorkspacePath) => ("Add workspace path", vec!["Relative path"]),
        Some(SettingsEditKind::Network) => ("Add network grant", vec!["Host", "Port (* = any)"]),
        Some(SettingsEditKind::Environment { .. }) => {
            ("Environment variable", vec!["Key", "Value"])
        }
        Some(SettingsEditKind::Secret) => ("Add secret ID", vec!["Secret ID"]),
        Some(SettingsEditKind::Limit { name }) => (name.as_str(), vec!["Value"]),
        _ => ("Edit setting", vec!["Value"]),
    }};
    let block = theme::modal_block(format!(" {title} · Tab fields · Enter save · Esc cancel "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = labels
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            (
                label.to_owned(),
                app.editor_fields.get(index).cloned().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    draw_form_rows(frame, inner, &rows, app.editor_index, true);
    if view.editor.is_some() {
        if let Some(notice) = &view.notice {
            frame.render_widget(Paragraph::new(notice.as_str()).fg(theme::error()).wrap(Wrap { trim: false }), Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 2.min(inner.height)));
        }
    }
}

fn draw_form_rows(
    frame: &mut Frame<'_>,
    area: Rect,
    rows: &[(String, String)],
    selected: usize,
    show_cursor: bool,
) {
    let row_width = area.width.saturating_sub(2) as usize;
    let label_width = row_width.saturating_sub(10).clamp(1, 22);
    let value_width = row_width
        .saturating_sub(label_width.saturating_add(2))
        .max(1);
    let items = rows.iter().map(|(label, value)| {
        let label = truncate_end(label, label_width);
        let label_padding = label_width.saturating_sub(cell_width(&label));
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{label}{}", " ".repeat(label_padding)),
                Style::default()
                    .fg(theme::muted())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("│ ", Style::default().fg(theme::border())),
            Span::styled(
                truncate_middle(value, value_width),
                Style::default().fg(theme::text()),
            ),
        ]))
    });
    let list = List::new(items)
        .highlight_style(theme::selected())
        .highlight_symbol("▸ ");
    let mut state = ListState::default()
        .with_selected((!rows.is_empty()).then_some(selected.min(rows.len().saturating_sub(1))));
    frame.render_stateful_widget(list, area, &mut state);
    if show_cursor && let Some((_, value)) = rows.get(selected) {
        let rendered_value_width = cell_width(&truncate_middle(value, value_width));
        let x = area
            .x
            .saturating_add(2)
            .saturating_add(label_width.min(u16::MAX as usize) as u16)
            .saturating_add(2)
            .saturating_add(rendered_value_width.min(u16::MAX as usize) as u16)
            .min(area.right().saturating_sub(1));
        let y = area
            .y
            .saturating_add(selected as u16)
            .min(area.bottom().saturating_sub(1));
        frame.set_cursor_position(Position::new(x, y));
    }
}

fn masked_secret(value: &str) -> String {
    "•".repeat(value.chars().count())
}

fn help_columns(left: &str, right: &str, width: u16) -> String {
    let gap = 2usize.min(width as usize);
    let left_width = (width as usize).saturating_sub(gap) / 2;
    let right_width = (width as usize).saturating_sub(gap + left_width);
    let left = truncate_end(left, left_width);
    let right = truncate_end(right, right_width);
    let padding = left_width
        .saturating_sub(left.chars().count())
        .saturating_add(gap);
    format!("{left}{}{right}", " ".repeat(padding))
}

fn help_rect(frame_area: Rect) -> Rect {
    let mut area = centered_rect(92, 100, frame_area);
    let height = frame_area.height.min(15);
    area.y = frame_area.y + frame_area.height.saturating_sub(height) / 2;
    area.height = height;
    area
}

fn compact_help_rows() -> [&'static str; 13] {
    [
        "Enter  send",
        "Shift+Enter  newline",
        "Esc/Ctrl+C  interrupt",
        "Ctrl+D  quit",
        "Alt+M/S/R/K  pickers",
        "j/k · g/G  navigate",
        "Alt+↑/↓  input history",
        "Ctrl+N  new session",
        "Ctrl+A/E · Ctrl+B/F",
        "Alt+←/→  word move",
        "Ctrl+W · Ctrl+U/K",
        "/settings /status /login",
        "/goal · ? help",
    ]
}

pub(super) fn draw_help(frame: &mut Frame<'_>) {
    let area = help_rect(frame.area());
    theme::modal_backdrop(frame, area);
    let width = area.width.saturating_sub(2);
    let lines = if area.width < 60 {
        compact_help_rows()
            .into_iter()
            .map(|row| Line::from(truncate_end(row, width as usize)))
            .collect()
    } else {
        let left = [
            "Enter  send",
            "Shift+Enter  newline",
            "Esc/Ctrl+C  interrupt",
            "Ctrl+A/E  line ends",
            "Ctrl+B/F  char move",
            "Alt+←/→  word move",
            "Ctrl+W  delete word",
            "Ctrl+U/K  kill sides",
            "Alt+↑/↓  input history",
            "?  help / close",
            "Ctrl+D  detach / quit",
            "/goal  strict success judged",
        ];
        let right = [
            "Ctrl+N  new session",
            "Alt+M  models",
            "Alt+S  sessions",
            "Alt+R  reasoning",
            "Alt+K  capabilities",
            "j/k  scroll transcript",
            "Ctrl+B/F  page empty",
            "g/G  oldest / latest",
            "/settings  runtime",
            "Ctrl+O  view launcher",
            "/login  auth",
            "Ctrl+Tab  switch tabs",
        ];
        let mut lines =
            vec![Line::from(help_columns("EDIT / SEND", "NAV / COMMANDS", width)).bold()];
        lines.extend(
            left.into_iter()
                .zip(right)
                .map(|(left, right)| Line::from(help_columns(left, right, width))),
        );
        lines
    };
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(theme::modal_block(" Help · Esc close ")),
        area,
    );
}

fn permission_action_line(permission: &crate::shared_ui::composer::PermissionView, primary: &crate::shared_ui::composer::ComposerControl) -> Line<'static> {
    use crate::shared_ui::composer::ComposerAction;
    let mut spans = Vec::new();
    for control in &permission.controls {
        if !spans.is_empty() { spans.push(Span::raw(" · ")); }
        let shortcut = match control.action {
            ComposerAction::RespondPermission { allow: true, .. } => "Enter/y",
            ComposerAction::RespondPermission { allow: false, .. } => "n/Esc",
            _ => "",
        };
        spans.push(Span::styled(format!("{shortcut} {}", control.label.to_lowercase()), Style::default().fg(if control.enabled { theme::accent() } else { theme::muted() })));
    }
    if matches!(primary.action, ComposerAction::Interrupt) {
        spans.push(Span::styled(format!(" · Ctrl+C {}", primary.label), Style::default().fg(theme::accent_hot())));
    }
    Line::from(spans)
}

pub(super) fn draw_permission(frame: &mut Frame<'_>, app: &App) {
    let projection = app.application.composer_projection();
    let Some(permission) = projection.view.permissions.first() else { return; };
    let text = Text::from(vec![
        Line::from(permission.title.clone()).bold(),
        Line::from(permission.operation.clone()).fg(theme::accent_hot()),
        Line::from(permission.detail.clone()).fg(theme::muted()),
        Line::from(permission.reason.clone()).fg(theme::text_dim()),
    ]);
    draw_permission_panel(frame, text, &permission.title, permission_action_line(permission, &projection.view.primary_control));
}

fn draw_permission_panel(frame: &mut Frame<'_>, text: Text<'_>, title: &str, actions: Line<'static>) {
    let mut area = centered_rect(82, 90, frame.area());
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    let content_height = paragraph.line_count(area.width.saturating_sub(4).max(1));
    let actions = Paragraph::new(actions).wrap(Wrap { trim: false });
    let action_height = actions.line_count(area.width.saturating_sub(4).max(1)) as u16;
    let height = (content_height.saturating_add(action_height as usize + 4))
        .min(area.height as usize) as u16;
    area.y += area.height.saturating_sub(height) / 2;
    area.height = height;
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block_with_accent(title, theme::warning());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows =
        Layout::vertical([Constraint::Min(1), Constraint::Length(action_height)]).split(inner);
    frame.render_widget(paragraph, rows[0]);
    frame.render_widget(actions, rows[1]);
}

pub(super) fn draw_debate(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(92, 90, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(" DEBATE · Pro / Con / Jury ")
        .border_style(Style::default().fg(theme::accent_hot()))
        .style(theme::modal_surface());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(7),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(
            "Frame → Research → Opening → Rebuttal → Research / Strengthening ↻ → Checkpoint → Closing → Jury",
        )
        .style(Style::default().fg(theme::accent())),
        rows[0],
    );
    let mut lines = Vec::new();
    if let Some(d) = &app.state.debate {
        lines.push(Line::from(Span::styled(
            &d.topic,
            Style::default().bold().fg(theme::text()),
        )));
        lines.push(Line::from(format!(
            "{} · {} speeches · Ballots {} (adaptive {}–{}) · Jury attempts {}",
            d.status,
            d.speeches.len(),
            d.ballots.len(),
            crate::debate::JURY_EARLY_BALLOTS,
            crate::debate::JURY_TARGET_BALLOTS,
            d.jury_attempts.len(),
        )));
        lines.push(Line::from(format!(
            "Pro: {} · Con: {}",
            d.models.pro, d.models.con
        )));
        lines.push(Line::from(format!("Jury: {}", d.models.jury)));
        if let Some(contract) = &d.contract {
            lines.push(Line::from(Span::styled(
                "Debate contract",
                Style::default().fg(theme::accent_hot()).bold(),
            )));
            lines.extend(
                contract
                    .render()
                    .lines()
                    .map(|line| Line::from(line.to_owned())),
            );
            lines.push(Line::from(""));
        }
        for research in d.research_records() {
            lines.push(Line::from(Span::styled(
                format!(
                    "Research · {} · {} · {} source reads",
                    if research.pro { "Pro" } else { "Con" },
                    d.stage_label(research.stage),
                    research.successful_reads
                ),
                Style::default().fg(theme::accent()).bold(),
            )));
            lines.extend(
                research
                    .notes
                    .lines()
                    .map(|line| Line::from(line.to_owned())),
            );
            lines.push(Line::from(""));
        }
        for speech in &d.speeches {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!(
                    "{}  {}",
                    if speech.pro { "● Pro" } else { "● Con" },
                    d.stage_label(speech.stage)
                ),
                Style::default().bold().fg(if speech.pro {
                    theme::accent_hot()
                } else {
                    theme::accent()
                }),
            )));
            lines.extend(speech.text.lines().map(|s| Line::from(s.to_owned())));
        }
        for checkpoint in &d.checkpoints {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Progress checkpoint · {}", d.stage_label(checkpoint.stage)),
                Style::default().fg(theme::accent_hot()).bold(),
            )));
            lines.extend(
                checkpoint
                    .render()
                    .lines()
                    .map(|line| Line::from(line.to_owned())),
            );
        }
        if let Some(reason) = &d.closing_reason {
            lines.push(Line::from(Span::styled(
                format!("Closing reason · {reason}"),
                Style::default().fg(theme::accent()),
            )));
        }
        for (index, ballot) in d.ballots.iter().enumerate() {
            lines.push(Line::from(format!(
                "Ballot {} · Pro {:?} / Con {:?}",
                index + 1,
                ballot.pro,
                ballot.con
            )));
            lines.extend(ballot.reason.lines().map(|s| Line::from(s.to_owned())));
        }
        for (index, attempt) in d
            .jury_attempts
            .iter()
            .enumerate()
            .filter(|(_, attempt)| !attempt.accepted)
        {
            lines.push(Line::from(Span::styled(
                format!(
                    "Rejected jury attempt {} · {}",
                    index + 1,
                    attempt.error.as_deref().unwrap_or("invalid ballot")
                ),
                Style::default().fg(theme::error()),
            )));
        }
        if let Some(verdict) = &d.verdict {
            lines.push(Line::from(""));
            lines.extend(verdict.lines().map(|s| {
                Line::from(Span::styled(
                    s.to_owned(),
                    Style::default().fg(theme::accent_hot()).bold(),
                ))
            }));
        }
        if let Some(summary) = &d.knowledge_summary {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Learned context retained for later chat",
                Style::default().fg(theme::accent()).bold(),
            )));
            lines.extend(
                summary
                    .render()
                    .lines()
                    .map(|line| Line::from(line.to_owned())),
            );
        }
    } else {
        lines.push(Line::from(
            "Two fixed positions. Bounded adaptive research with evidence deduplication.",
        ));
        lines.push(Line::from(
            "The jury starts with A-first and B-first ballots; close calls expand to four.",
        ));
        lines.push(Line::from(
            "Scores follow the debate contract's criteria; evidence and rebuttal quality are cross-checks.",
        ));
        lines.push(Line::from(format!(
            "Default model: {} · Adaptive strengthening · jury retries uncapped",
            app.state.active_model,
        )));
    }
    if let Some(error) = &app.state.error_message {
        lines.push(Line::from(Span::styled(
            error,
            Style::default().fg(theme::error()),
        )));
    }
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max = paragraph
        .line_count(rows[1].width.max(1))
        .saturating_sub(rows[1].height as usize);
    frame.render_widget(
        paragraph.scroll((app.popup_index.min(max).min(u16::MAX as usize) as u16, 0)),
        rows[1],
    );
    let models = if app.state.is_streaming {
        app.state
            .debate
            .as_ref()
            .map(|d| &d.models)
            .unwrap_or(&app.debate_models)
    } else {
        &app.debate_models
    };
    let fields = [
        ("Topic", app.popup_filter.as_str(), theme::text()),
        ("Pro model", models.pro.as_str(), theme::accent_hot()),
        ("Con model", models.con.as_str(), theme::accent()),
        ("Jury model", models.jury.as_str(), theme::text_dim()),
    ];
    let mut form = Vec::new();
    for (index, (label, value, color)) in fields.into_iter().enumerate() {
        let selected = index == app.debate_field && !app.state.is_streaming;
        let prefix = format!("{} {label:10} ", if selected { "›" } else { " " });
        let budget = rows[2].width.saturating_sub(prefix.chars().count() as u16) as usize;
        let value = if value.is_empty() {
            "(required)".to_owned()
        } else {
            truncate_middle(value, budget)
        };
        form.push(Line::from(vec![
            Span::styled(prefix, Style::default().fg(color).bold()),
            Span::styled(
                value,
                if selected {
                    Style::default()
                        .fg(theme::text())
                        .bg(theme::selected_color())
                } else {
                    Style::default().fg(theme::text_dim())
                },
            ),
        ]));
    }
    form.push(Line::from(if app.state.is_streaming {
        "Running · Model settings are locked"
    } else {
        "Tab: field · ↑/↓: model · Type ID · Ctrl+U: clear"
    }));
    form.push(Line::from("Enter: start · PgUp/PgDn: scroll"));
    form.push(Line::from("Ctrl+C: stop · Esc: close (keeps running)"));
    frame.render_widget(Paragraph::new(form), rows[2]);
}
