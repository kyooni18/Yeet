//! Modal dialogs and settings forms.
use super::theme;
use super::{centered_rect, truncate_end, truncate_middle};
use crate::{
    app::{App, SettingsEditKind, SettingsSection},
    model::REASONING_LEVELS,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    prelude::{Line, Modifier, Span, Style, Stylize, Text},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

pub(super) fn draw_models(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(72, 72, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.debate_model_picker {
        let role = match app.debate_field {
            1 => "Pro",
            2 => "Con",
            3 => "Jury",
            _ => "Model",
        };
        format!(" {role} model · ↑/↓ choose · Enter select · Esc back ")
    } else {
        " Models · type to filter · Enter select · Esc close ".to_owned()
    };
    let block = theme::modal_block(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Double);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(inner);
    frame.render_widget(
        Paragraph::new(format!("Filter: {}", app.popup_filter))
            .style(Style::default().fg(theme::TEXT_DIM)),
        chunks[0],
    );

    if app.state.is_loading_models && app.state.available_models.is_empty() {
        frame.render_widget(
            Paragraph::new("Loading models…").fg(theme::TEXT_DIM),
            chunks[1],
        );
        return;
    }
    let models = app.filtered_models();
    if models.is_empty() {
        frame.render_widget(
            Paragraph::new("No matching models. Try another search.").fg(theme::MUTED),
            chunks[1],
        );
        return;
    }
    let items = models.iter().map(|model| {
        let marker = if **model == app.state.active_model {
            "●"
        } else {
            " "
        };
        ListItem::new(format!("{marker} {model}"))
    });
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme::SELECTED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state =
        ListState::default().with_selected((!models.is_empty()).then_some(app.popup_index));
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

pub(super) fn draw_sessions(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(78, 70, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(" Sessions · Enter open · n new · Esc close ")
        .borders(Borders::ALL)
        .border_type(BorderType::Double);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.sessions().is_empty() {
        frame.render_widget(Paragraph::new("No saved sessions."), inner);
        return;
    }
    let items = app.sessions().iter().map(|session| {
        let marker = if app.state.current_session_id.as_deref() == Some(session.id.as_str()) {
            "●"
        } else {
            " "
        };
        ListItem::new(Line::from(vec![
            Span::raw(format!("{marker} {}", session.title)),
            Span::styled(
                format!("  ·  {}  ·  {}", session.model, session.message_count),
                Style::default().fg(theme::MUTED),
            ),
        ]))
    });
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme::SELECTED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default().with_selected(Some(app.popup_index));
    frame.render_stateful_widget(list, inner, &mut state);
}

pub(super) fn draw_reasoning(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(58, 46, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(" Reasoning · arrows navigate · Enter select · Esc close ")
        .borders(Borders::ALL)
        .border_type(BorderType::Double);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let descriptions = [
        "Yeet default: low primary-request reasoning; Lead is an opt-in capability",
        "Faster, lighter reasoning",
        "Balanced reasoning depth",
        "Maximum supported reasoning depth",
    ];
    let items = REASONING_LEVELS
        .iter()
        .zip(descriptions)
        .map(|(level, description)| {
            let marker = if *level == app.state.active_reasoning_level {
                "●"
            } else {
                " "
            };
            ListItem::new(Line::from(vec![
                Span::raw(format!("{marker} {level:<7}")),
                Span::styled(description, Style::default().fg(theme::MUTED)),
            ]))
        });
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme::SELECTED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default().with_selected(Some(app.popup_index));
    frame.render_stateful_widget(list, inner, &mut state);
}

pub(super) fn draw_capabilities(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(82, 76, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(
        " Capabilities · type to filter · Space toggle · Enter details · Esc close ",
    )
    .borders(Borders::ALL)
    .border_type(BorderType::Double);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(inner);
    frame.render_widget(
        Paragraph::new(format!("Filter: {}", app.popup_filter))
            .style(Style::default().fg(theme::TEXT_DIM)),
        chunks[0],
    );

    if app.state.is_loading_capabilities && app.state.available_capabilities.is_empty() {
        frame.render_widget(
            Paragraph::new("Loading capabilities…").fg(theme::TEXT_DIM),
            chunks[1],
        );
        return;
    }

    let items = app.filtered_capabilities();
    if items.is_empty() {
        frame.render_widget(Paragraph::new("No matching capabilities."), chunks[1]);
        return;
    }

    let rows = items.iter().map(|item| {
        let marker = if item.enabled { "●" } else { "○" };
        ListItem::new(Line::from(vec![
            Span::raw(format!("{marker} {:<10} {:<24}", item.kind, item.name)),
            Span::styled(item.description.clone(), Style::default().fg(theme::MUTED)),
        ]))
    });
    let list = List::new(rows)
        .highlight_style(
            Style::default()
                .bg(theme::SELECTED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default().with_selected(Some(app.popup_index));
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

pub(super) fn draw_capability_detail(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(76, 62, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(" Capability detail · Space toggle · Enter/Esc back ")
        .borders(Borders::ALL)
        .border_type(BorderType::Double);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(item) = app.capability_detail() else {
        frame.render_widget(Paragraph::new("Capability is no longer available."), inner);
        return;
    };

    let status = if item.enabled { "Enabled" } else { "Disabled" };
    let text = Text::from(vec![
        Line::from(Span::styled(
            item.name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Status  ", Style::default().fg(theme::MUTED)),
            Span::raw(status),
        ]),
        Line::from(vec![
            Span::styled("Type    ", Style::default().fg(theme::MUTED)),
            Span::raw(item.kind.clone()),
        ]),
        Line::from(vec![
            Span::styled("ID      ", Style::default().fg(theme::MUTED)),
            Span::raw(item.id.clone()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Description",
            Style::default().fg(theme::MUTED),
        )),
        Line::from(""),
        Line::from(item.description.clone()),
    ]);
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
}

pub(super) fn draw_auth(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(82, 70, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.state.auth_working {
        " Authentication · working… · Esc close "
    } else {
        " Authentication · Enter/l sign in · k API key · x sign out · r refresh · Esc close "
    };
    let block = theme::modal_block(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Double);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);

    if app.state.auth_working && app.state.auth_providers.is_empty() {
        frame.render_widget(
            Paragraph::new("Loading providers…").fg(theme::TEXT_DIM),
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
            ListItem::new(Line::from(vec![
                Span::raw(format!("{marker} {:<18} {:<19}", item.provider, status)),
                Span::styled(truncate_end(detail, 32), Style::default().fg(theme::MUTED)),
            ]))
        });
        let list = List::new(rows)
            .highlight_style(
                Style::default()
                    .bg(theme::SELECTED)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        let mut state = ListState::default().with_selected(Some(app.popup_index));
        frame.render_stateful_widget(list, chunks[0], &mut state);
    }

    if let Some(notice) = app.state.auth_notice.as_deref() {
        frame.render_widget(
            Paragraph::new(truncate_end(notice, chunks[1].width as usize)).fg(theme::MUTED),
            chunks[1],
        );
    }
}

pub(super) fn draw_auth_key(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(64, 28, frame.area());
    theme::modal_backdrop(frame, area);
    let provider = app.active_auth_provider.as_deref().unwrap_or("provider");
    let block = theme::modal_block(format!(" API key · {provider} · Enter save · Esc cancel "))
        .borders(Borders::ALL)
        .border_type(BorderType::Double);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let key = app
        .editor_fields
        .first()
        .map(String::as_str)
        .unwrap_or_default();
    let masked = masked_secret(key);
    frame.render_widget(
        Paragraph::new("Paste or type the provider API key.").fg(theme::MUTED),
        Rect::new(inner.x, inner.y, inner.width, 2),
    );
    let field = Rect::new(
        inner.x,
        inner.y.saturating_add(3),
        inner.width,
        3.min(inner.height.saturating_sub(3)),
    );
    let field_block = theme::modal_block(" API key ")
        .borders(Borders::ALL)
        .border_type(BorderType::Double);
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
    let block =
        theme::modal_block(" Providers · n new · Enter edit · d delete · r refresh · Esc close ")
            .borders(Borders::ALL)
            .border_type(BorderType::Double);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
    if app.state.providers_working && app.state.provider_configurations.is_empty() {
        frame.render_widget(
            Paragraph::new("Loading custom providers…").fg(theme::TEXT_DIM),
            chunks[0],
        );
    } else if app.state.provider_configurations.is_empty() {
        frame.render_widget(
            Paragraph::new("No custom providers. Press n to add one."),
            chunks[0],
        );
    } else {
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
            ListItem::new(Line::from(vec![
                Span::raw(format!("{:<18} ", provider.id)),
                Span::styled(
                    truncate_middle(&provider.base_url, 42),
                    Style::default().fg(theme::TEXT_DIM),
                ),
                Span::styled(
                    format!("  {auth}{headers}"),
                    Style::default().fg(theme::MUTED),
                ),
            ]))
        });
        let list = List::new(rows)
            .highlight_style(
                Style::default()
                    .bg(theme::SELECTED)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        let mut state = ListState::default().with_selected(Some(app.popup_index));
        frame.render_stateful_widget(list, chunks[0], &mut state);
    }
    if let Some(notice) = app.state.providers_notice.as_deref() {
        frame.render_widget(
            Paragraph::new(truncate_end(notice, chunks[1].width as usize)).fg(theme::MUTED),
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
    ))
    .borders(Borders::ALL)
    .border_type(BorderType::Double);
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
    let area = centered_rect(78, 44, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.state.settings_working {
        " Settings · saving… · Esc close "
    } else {
        " Settings · Enter/Space toggle/open · r refresh · Esc close "
    };
    let block = theme::modal_block(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Double);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);

    let mut rows: Vec<ListItem<'static>> = Vec::new();
    if app.openai_provider_active() {
        let flex_available = app.openai_flex_available();
        rows.push(ListItem::new(Line::from(vec![
            Span::styled(
                "OpenAI Flex            ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if !flex_available {
                    "unavailable"
                } else if app.state.openai_flex {
                    "on"
                } else {
                    "off"
                },
                Style::default().fg(theme::TEXT_DIM),
            ),
            Span::styled(
                if flex_available {
                    " · API billing · service_tier=flex"
                } else {
                    " · browser login does not support Flex"
                },
                Style::default().fg(theme::MUTED),
            ),
        ])));
    }
    rows.push(ListItem::new(Line::from(vec![
        Span::styled(
            "Project Memory         ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if app.state.foundation_memory_enabled {
                "on"
            } else {
                "off"
            },
            Style::default().fg(theme::TEXT_DIM),
        ),
        Span::styled(
            format!(
                " · {} · {}",
                "Yeet",
                if app.state.foundation_memory_connected {
                    "ready"
                } else {
                    "unavailable"
                }
            ),
            Style::default().fg(theme::MUTED),
        ),
    ])));
    let sandbox_value = app
        .state
        .sandbox_settings
        .as_ref()
        .map(|settings| format!("preset: {} · open sandbox settings", settings.preset))
        .unwrap_or_else(|| "open sandbox settings".into());
    rows.push(ListItem::new(Line::from(vec![
        Span::styled(
            "Sandbox                ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(sandbox_value, Style::default().fg(theme::TEXT_DIM)),
    ])));

    let list = List::new(rows)
        .highlight_style(
            Style::default()
                .bg(theme::SELECTED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default().with_selected(Some(app.popup_index));
    frame.render_stateful_widget(list, chunks[0], &mut state);

    let status = if let Some(notice) = app.state.settings_notice.as_deref() {
        truncate_end(notice, chunks[1].width as usize)
    } else if app.openai_provider_active() {
        if app.openai_flex_available() {
            "Flex is applied only to API-key or environment-key OpenAI requests.".into()
        } else {
            "OpenAI browser login uses the ChatGPT/Codex path; Flex is disabled for it.".into()
        }
    } else {
        "OpenAI-specific settings appear when an OpenAI model is selected.".into()
    };
    frame.render_widget(Paragraph::new(status).fg(theme::MUTED), chunks[1]);
}

pub(super) fn draw_sandbox_presets(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(78, 54, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.state.sandbox_working {
        " Sandbox · saving… · Esc back "
    } else {
        " Sandbox · Enter apply/open · r refresh · Esc back "
    };
    let block = theme::modal_block(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Double);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
    let Some(settings) = app.state.sandbox_settings.as_ref() else {
        frame.render_widget(
            Paragraph::new("Loading project settings…").fg(theme::TEXT_DIM),
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
            Span::styled(value, Style::default().fg(theme::TEXT_DIM)),
        ]))
    });
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme::SELECTED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default().with_selected(Some(app.popup_index));
    frame.render_stateful_widget(list, chunks[0], &mut state);
    if let Some(notice) = app.state.sandbox_notice.as_deref() {
        frame.render_widget(
            Paragraph::new(truncate_end(notice, chunks[1].width as usize)).fg(theme::MUTED),
            chunks[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new("Applying a preset replaces advanced sandbox rules.").fg(theme::MUTED),
            chunks[1],
        );
    }
}

pub(super) fn draw_sandbox_policy(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(92, 80, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(
        " Sandbox policy · Tab/←/→ section · Enter/Space edit · n add · d remove · Esc back ",
    )
    .borders(Borders::ALL)
    .border_type(BorderType::Double);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(settings) = app.state.sandbox_settings.as_ref() else {
        frame.render_widget(
            Paragraph::new("Loading settings…").fg(theme::TEXT_DIM),
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

    let sections = [
        (SettingsSection::Core, "Core"),
        (SettingsSection::Workspace, "Workspace"),
        (SettingsSection::Network, "Network"),
        (SettingsSection::Environment, "Environment"),
        (SettingsSection::Secrets, "Secrets"),
        (SettingsSection::Limits, "Limits"),
    ];
    let current = app.settings_section.unwrap_or(SettingsSection::Core);
    let mut tabs = Vec::new();
    for (index, (section, label)) in sections.iter().enumerate() {
        if index > 0 {
            tabs.push(Span::styled("  ", Style::default().fg(theme::MUTED)));
        }
        let style = if *section == current {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::MUTED)
        };
        tabs.push(Span::styled(*label, style));
    }
    frame.render_widget(Paragraph::new(Line::from(tabs)), chunks[0]);

    let rows: Vec<ListItem<'static>> = match app.settings_section {
        Some(SettingsSection::Core) | None => vec![
            ListItem::new(Line::from(vec![
                Span::raw("Execution mode         "),
                Span::styled(
                    settings.execution_mode.clone(),
                    Style::default().fg(theme::TEXT_DIM),
                ),
            ])),
            ListItem::new(Line::from(vec![
                Span::raw("Auto approval          "),
                Span::styled(
                    if settings.auto_approve { "on" } else { "off" },
                    Style::default().fg(theme::TEXT_DIM),
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
                    Style::default().fg(theme::TEXT_DIM),
                ),
            ])),
            ListItem::new(Line::from(vec![
                Span::raw("Reset policy           "),
                Span::styled(
                    "restore Safe defaults",
                    Style::default().fg(theme::TEXT_DIM),
                ),
            ])),
        ],
        Some(SettingsSection::Workspace) => {
            let mut rows = vec![ListItem::new(Line::from(vec![
                Span::raw("Workspace read mode  "),
                Span::styled(
                    settings.workspace_mode.clone(),
                    Style::default().fg(theme::TEXT_DIM),
                ),
            ]))];
            rows.extend(settings.workspace_paths.iter().map(|path| {
                ListItem::new(Line::from(vec![
                    Span::raw("Path                 "),
                    Span::styled(path.clone(), Style::default().fg(theme::TEXT_DIM)),
                ]))
            }));
            rows
        }
        Some(SettingsSection::Network) => settings
            .network_allow
            .iter()
            .map(|item| {
                let port = item
                    .port
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "*".into());
                ListItem::new(format!("{}:{port}", item.host))
            })
            .collect(),
        Some(SettingsSection::Environment) => settings
            .environment
            .iter()
            .map(|item| {
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{:<24}", item.key)),
                    Span::styled(
                        truncate_end(&item.value, 44),
                        Style::default().fg(theme::TEXT_DIM),
                    ),
                ]))
            })
            .collect(),
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
        frame.render_widget(Paragraph::new(empty).fg(theme::TEXT_DIM), chunks[1]);
    } else {
        let list = List::new(rows)
            .highlight_style(
                Style::default()
                    .bg(theme::SELECTED)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        let mut state = ListState::default().with_selected(Some(app.popup_index));
        frame.render_stateful_widget(list, chunks[1], &mut state);
    }

    let status = if let Some(notice) = app.state.sandbox_notice.as_deref() {
        truncate_end(notice, chunks[2].width as usize)
    } else {
        format!(
            "Preset: {} · advanced changes are reported as custom",
            settings.preset
        )
    };
    frame.render_widget(Paragraph::new(status).fg(theme::MUTED), chunks[2]);
}

pub(super) fn draw_settings_edit(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(70, 40, frame.area());
    theme::modal_backdrop(frame, area);
    let (title, labels): (&str, Vec<&str>) = match app.settings_edit_kind.as_ref() {
        Some(SettingsEditKind::WorkspacePath) => ("Add workspace path", vec!["Relative path"]),
        Some(SettingsEditKind::Network) => ("Add network grant", vec!["Host", "Port (* = any)"]),
        Some(SettingsEditKind::Environment { .. }) => {
            ("Environment variable", vec!["Key", "Value"])
        }
        Some(SettingsEditKind::Secret) => ("Add secret ID", vec!["Secret ID"]),
        Some(SettingsEditKind::Limit { name }) => (name.as_str(), vec!["Value"]),
        None => ("Edit setting", vec!["Value"]),
    };
    let block = theme::modal_block(format!(" {title} · Tab fields · Enter save · Esc cancel "))
        .borders(Borders::ALL)
        .border_type(BorderType::Double);
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
}

fn draw_form_rows(
    frame: &mut Frame<'_>,
    area: Rect,
    rows: &[(String, String)],
    selected: usize,
    show_cursor: bool,
) {
    let items = rows.iter().map(|(label, value)| {
        ListItem::new(Line::from(vec![
            Span::styled(format!("{label:<22}"), Style::default().fg(theme::MUTED)),
            Span::raw(value.clone()),
        ]))
    });
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme::SELECTED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default()
        .with_selected((!rows.is_empty()).then_some(selected.min(rows.len().saturating_sub(1))));
    frame.render_stateful_widget(list, area, &mut state);
    if show_cursor && let Some((label, value)) = rows.get(selected) {
        let x = area
            .x
            .saturating_add(2)
            .saturating_add(label.chars().count().max(22) as u16)
            .saturating_add(value.chars().count() as u16)
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

pub(super) fn draw_help(frame: &mut Frame<'_>) {
    let area = centered_rect(64, 62, frame.area());
    theme::modal_backdrop(frame, area);
    let text = Text::from(vec![
        Line::from("Yeet".bold()),
        Line::from(""),
        Line::from("Enter          send"),
        Line::from("Shift+Enter    newline"),
        Line::from("Esc / Ctrl+C   interrupt"),
        Line::from("Alt+M          models"),
        Line::from("Alt+S          sessions"),
        Line::from("Alt+R          reasoning level"),
        Line::from("Alt+K          skills / capabilities / MCP"),
        Line::from("/capabilities  toggle skills / capabilities / MCP"),
        Line::from("/login         authentication"),
        Line::from("/settings      runtime and sandbox settings"),
        Line::from("Ctrl+N         new session"),
        Line::from("j / k          scroll transcript"),
        Line::from("Ctrl+B / Ctrl+F page up / page down"),
        Line::from("g / G          oldest / latest transcript"),
        Line::from("?              this help"),
        Line::from("Ctrl+D          detach / quit"),
        Line::from("Ctrl+C          quit when idle"),
    ]);
    frame.render_widget(
        Paragraph::new(text).block(
            theme::modal_block(" Help · Esc close ")
                .borders(Borders::ALL)
                .border_type(BorderType::Double),
        ),
        area,
    );
}

pub(super) fn draw_permission(frame: &mut Frame<'_>, app: &App) {
    if let Some(permission) = app.state.pending_native_app_permission.as_ref() {
        let area = centered_rect(78, 44, frame.area());
        theme::modal_backdrop(frame, area);
        let identity = match (&permission.app_name, &permission.bundle_id) {
            (Some(name), Some(bundle_id)) => format!("{name} ({bundle_id})"),
            (Some(name), None) => name.clone(),
            (None, Some(bundle_id)) => bundle_id.clone(),
            (None, None) => "Unknown native application".into(),
        };
        let source = if permission.server == "codex-computer-use" {
            format!("Computer Use: Codex / {}", permission.tool)
        } else {
            format!("MCP: {}/{}", permission.server, permission.tool)
        };
        let text = Text::from(vec![
            Line::from(vec![
                Span::styled(
                    "Native application approval: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(identity),
            ]),
            Line::from(format!("Operation: {}", permission.operation)).fg(theme::ACCENT_HOT),
            Line::from(source).fg(theme::MUTED),
            Line::from("Session-only access; no persistent approval will be saved.")
                .fg(theme::ACCENT),
            Line::from(""),
            Line::from(permission.reason.clone()).fg(theme::TEXT_DIM),
            Line::from(""),
            Line::from("Enter / y allow once    n / Esc deny").fg(theme::MUTED),
        ]);
        frame.render_widget(
            Paragraph::new(text).wrap(Wrap { trim: false }).block(
                Block::default()
                    .title(" Native app permission ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Double),
            ),
            area,
        );
        return;
    }
    let Some(permission) = app.state.pending_shell_permission.as_ref() else {
        return;
    };
    let area = centered_rect(72, 38, frame.area());
    theme::modal_backdrop(frame, area);
    let text = Text::from(vec![
        Line::from(vec![
            Span::styled(
                "Permission required: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(&permission.operation),
        ]),
        Line::from(format!("{} action", permission.kind)).fg(theme::MUTED),
        Line::from(""),
        Line::from(Span::styled(
            permission.command.clone(),
            Style::default().fg(theme::ACCENT_HOT),
        )),
        Line::from(""),
        Line::from(permission.reason.clone()).fg(theme::TEXT_DIM),
        Line::from(""),
        Line::from("Enter / y allow once    n / Esc deny").fg(theme::MUTED),
    ]);
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(
            theme::modal_block(" Permission ")
                .borders(Borders::ALL)
                .border_type(BorderType::Double),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_key_input_is_masked() {
        assert_eq!(masked_secret("secret"), "••••••");
        assert_eq!(masked_secret(""), "");
    }
}

pub(super) fn draw_debate(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(92, 90, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(" DEBATE · Pro / Con / Jury ")
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(theme::ACCENT_HOT))
        .style(Style::default().bg(theme::SURFACE));
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
        .style(Style::default().fg(theme::ACCENT)),
        rows[0],
    );
    let mut lines = Vec::new();
    if let Some(d) = &app.state.debate {
        lines.push(Line::from(Span::styled(
            &d.topic,
            Style::default().bold().fg(theme::TEXT),
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
                Style::default().fg(theme::ACCENT_HOT).bold(),
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
                Style::default().fg(theme::ACCENT).bold(),
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
                    theme::ACCENT_HOT
                } else {
                    theme::ACCENT
                }),
            )));
            lines.extend(speech.text.lines().map(|s| Line::from(s.to_owned())));
        }
        for checkpoint in &d.checkpoints {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Progress checkpoint · {}", d.stage_label(checkpoint.stage)),
                Style::default().fg(theme::ACCENT_HOT).bold(),
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
                Style::default().fg(theme::ACCENT),
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
                Style::default().fg(theme::ERROR),
            )));
        }
        if let Some(verdict) = &d.verdict {
            lines.push(Line::from(""));
            lines.extend(verdict.lines().map(|s| {
                Line::from(Span::styled(
                    s.to_owned(),
                    Style::default().fg(theme::ACCENT_HOT).bold(),
                ))
            }));
        }
        if let Some(summary) = &d.knowledge_summary {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Learned context retained for later chat",
                Style::default().fg(theme::ACCENT).bold(),
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
            "Default model: {} · Up to {} strengthening rounds · {} jury retries max",
            app.state.active_model,
            crate::debate::MAX_STRENGTHENING_ROUNDS,
            crate::debate::JURY_MAX_ATTEMPTS,
        )));
    }
    if let Some(error) = &app.state.error_message {
        lines.push(Line::from(Span::styled(
            error,
            Style::default().fg(theme::ERROR),
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
        ("Topic", app.popup_filter.as_str(), theme::TEXT),
        ("Pro model", models.pro.as_str(), theme::ACCENT_HOT),
        ("Con model", models.con.as_str(), theme::ACCENT),
        ("Jury model", models.jury.as_str(), theme::TEXT_DIM),
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
                    Style::default().fg(theme::TEXT).bg(theme::SELECTED)
                } else {
                    Style::default().fg(theme::TEXT_DIM)
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

#[cfg(test)]
mod debate_tests {
    use super::*;
    #[test]
    fn debate_popup_renders_at_small_and_large_sizes() {
        for (w, h) in [(40, 12), (100, 35)] {
            let backend = ratatui::backend::TestBackend::new(w, h);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            let app = App::default();
            terminal.draw(|frame| draw_debate(frame, &app)).unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            assert!(rendered.contains("DEBATE"));
            assert!(rendered.contains("Pro model"));
            assert!(rendered.contains("Con model"));
            assert!(rendered.contains("Jury model"));
            assert!(
                !rendered
                    .chars()
                    .any(|c| ('\u{ac00}'..='\u{d7a3}').contains(&c))
            );
        }
    }
}
