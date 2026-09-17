//! Modal dialogs and settings forms.
use super::theme;
use super::{cell_width, centered_rect, compact_number, truncate_end, truncate_middle};
use crate::{
    app::{App, SettingsEditKind, SettingsSection},
    model::REASONING_LEVELS,
};
mod navigation;

#[cfg(test)]
use navigation::model_picker_rows;
pub(super) use navigation::{draw_goal, draw_models, draw_reasoning, draw_sessions};

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    prelude::{Line, Modifier, Span, Style, Stylize, Text},
    widgets::{List, ListItem, ListState, Paragraph, Wrap},
};

pub(super) fn draw_capabilities(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(82, 76, frame.area());
    theme::modal_backdrop(frame, area);
    let block = if app.state.is_streaming {
        theme::modal_block(
            " Capabilities · changes locked while response runs · Enter details · Esc close ",
        )
    } else {
        theme::modal_block(
            " Capabilities · ↑/↓ navigate · type filter · Space toggle · Enter details · Esc close ",
        )
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(inner);
    let navigation_hint = if app.state.is_streaming {
        " · changes locked"
    } else if chunks[0].width >= 60 {
        " · ↑/↓ PgUp/PgDn"
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" FILTER// ", Style::default().fg(theme::ACCENT_WARM).bold()),
            Span::styled(
                if app.popup_filter.is_empty() {
                    "all".to_owned()
                } else {
                    app.popup_filter.clone()
                },
                Style::default().fg(theme::TEXT),
            ),
            Span::styled(navigation_hint, Style::default().fg(theme::MUTED)),
        ])),
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
        .highlight_style(theme::selected())
        .highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(app.popup_index));
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

pub(super) fn draw_capability_detail(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(76, 62, frame.area());
    theme::modal_backdrop(frame, area);
    let block = if app.state.is_streaming {
        theme::modal_block(
            " Capability detail · changes locked while response runs · Enter/Esc back ",
        )
    } else {
        theme::modal_block(" Capability detail · Space toggle · Enter/Esc back ")
    };
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
        if app.state.is_streaming {
            Line::styled(
                "Changes locked while a response is running.",
                Style::default().fg(theme::ACCENT_WARM),
            )
        } else {
            Line::from("")
        },
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
            Paragraph::new("Loading providers and quota headroom…").fg(theme::TEXT_DIM),
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
                Span::styled(truncate_end(detail, 32), Style::default().fg(theme::MUTED)),
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
                        Span::styled("  usage  ", Style::default().fg(theme::TEXT_DIM)),
                        Span::styled(
                            truncate_end(&format!("{plan}{summary}"), 72),
                            Style::default().fg(theme::MUTED),
                        ),
                    ]));
                } else if usage.source != "none" {
                    lines.push(Line::from(vec![
                        Span::styled("  usage  ", Style::default().fg(theme::TEXT_DIM)),
                        Span::styled(
                            truncate_end(
                                &format!(
                                    "{} · {}",
                                    usage.source,
                                    usage.message.as_deref().unwrap_or("unavailable")
                                ),
                                72,
                            ),
                            Style::default().fg(theme::MUTED),
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
            Paragraph::new(truncate_end(notice, chunks[1].width as usize)).fg(theme::MUTED),
            chunks[1],
        );
    }
}

pub(super) fn draw_status_dialog(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(92, 84, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.state.auth_working {
        " Status · refreshing provider usage… · r refresh · Esc close "
    } else {
        " Status · r refresh · Esc close "
    };
    let block = theme::modal_block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if inner.height < 12 {
        frame.render_widget(
            Paragraph::new(status_compact_lines(app, inner.width as usize))
                .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }

    // Keep the overview balanced: the two small cards share the top row while
    // provider headroom gets the wider, more useful lower card.
    let sections = Layout::vertical([Constraint::Length(5), Constraint::Min(6)]).split(inner);
    let overview = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(sections[0]);

    let runtime = theme::panel_block("Runtime");
    let runtime_inner = runtime.inner(overview[0]);
    frame.render_widget(runtime, overview[0]);
    frame.render_widget(
        Paragraph::new(runtime_status_lines(app, runtime_inner.width as usize)),
        runtime_inner,
    );

    let environment = theme::panel_block("Session & permissions");
    let environment_inner = environment.inner(overview[1]);
    frame.render_widget(environment, overview[1]);
    frame.render_widget(
        Paragraph::new(environment_status_lines(
            app,
            environment_inner.width as usize,
        )),
        environment_inner,
    );

    let usage = theme::panel_block("Usage & provider headroom");
    let usage_inner = usage.inner(sections[1]);
    frame.render_widget(usage, sections[1]);
    frame.render_widget(
        Paragraph::new(usage_status_lines(
            app,
            usage_inner.width as usize,
            usage_inner.height as usize,
        ))
        .wrap(Wrap { trim: false }),
        usage_inner,
    );
}

fn runtime_status_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let model = if app.state.active_model.is_empty() {
        "not selected".to_owned()
    } else {
        truncate_middle(&app.state.active_model, width.saturating_sub(16).max(12))
    };
    let reasoning = if app.state.active_reasoning_level.is_empty() {
        "auto"
    } else {
        app.state.active_reasoning_level.as_str()
    };
    let runtime = if app.state.is_streaming {
        "executing"
    } else {
        "idle"
    };
    let current = app.state.current_context_tokens.unwrap_or(0);
    let context = match app.state.active_model_context_length {
        Some(total) if total > 0 => {
            let percent = ((current as f64 * 100.0 / total as f64).round() as u8).min(100);
            format!(
                "{} {:>3}%  {}/{}",
                status_bar(percent, 10),
                percent,
                compact_number(current),
                compact_number(total)
            )
        }
        _ => format!("{} / unknown", compact_number(current)),
    };
    vec![
        Line::from(vec![
            Span::styled("Model      ", Style::default().fg(theme::MUTED)),
            Span::styled(model, Style::default().fg(theme::ACCENT).bold()),
        ]),
        Line::from(vec![
            Span::styled("Context    ", Style::default().fg(theme::MUTED)),
            Span::raw(truncate_end(&context, width.saturating_sub(11))),
        ]),
        Line::from(vec![
            Span::styled("Runtime    ", Style::default().fg(theme::MUTED)),
            Span::raw(runtime),
            Span::styled("  ·  reasoning ", Style::default().fg(theme::MUTED)),
            Span::raw(reasoning.to_owned()),
        ]),
    ]
}

fn usage_status_lines(app: &App, width: usize, height: usize) -> Vec<Line<'static>> {
    let usage = &app.state.token_usage;
    let cache = usage.cache_measurement();
    let cache_detail = if let Some(hit_rate) = cache.hit_rate {
        format!(
            "{} read · {:.0}% hit · {} write · {:.0}% measured",
            compact_number(cache.cached_input_tokens),
            hit_rate * 100.0,
            compact_number(cache.cache_write_input_tokens),
            cache.measurement_coverage_rate.unwrap_or(0.0) * 100.0,
        )
    } else if cache.unclassified_input_tokens > 0 {
        format!(
            "{} read · hit n/a · {} write · {} unclassified",
            compact_number(cache.cached_input_tokens),
            compact_number(cache.cache_write_input_tokens),
            compact_number(cache.unclassified_input_tokens),
        )
    } else if cache.unreported_input_tokens > 0 {
        format!(
            "{} read · hit n/a · {} write · {} unreported",
            compact_number(cache.cached_input_tokens),
            compact_number(cache.cache_write_input_tokens),
            compact_number(cache.unreported_input_tokens),
        )
    } else {
        format!(
            "{} read · {} write",
            compact_number(cache.cached_input_tokens),
            compact_number(cache.cache_write_input_tokens),
        )
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Tokens  ", Style::default().fg(theme::MUTED)),
            Span::raw(format!(
                "{} input · {} output · {} reasoning",
                compact_number(cache.input_tokens),
                compact_number(usage.output_tokens.unwrap_or(0)),
                compact_number(usage.reasoning_tokens.unwrap_or(0)),
            )),
        ]),
        Line::from(vec![
            Span::styled("Cache   ", Style::default().fg(theme::MUTED)),
            Span::raw(cache_detail),
        ]),
    ];
    if let Some(cost) = usage.estimated_cost_usd {
        lines.push(Line::from(vec![
            Span::styled("Cost    ", Style::default().fg(theme::MUTED)),
            Span::raw(format!("${cost:.4} estimated")),
        ]));
    }

    let active_provider = app
        .state
        .active_model
        .split_once('/')
        .map(|(provider, _)| provider);
    let available_rows = height.saturating_sub(lines.len());
    let mut found_usage = false;
    for item in app
        .state
        .auth_providers
        .iter()
        .filter(|item| item.usage.is_some())
        .take(available_rows.max(1))
    {
        let Some(provider_usage) = item.usage.as_ref() else {
            continue;
        };
        found_usage = true;
        let marker = if active_provider == Some(item.provider.as_str()) {
            "●"
        } else {
            "○"
        };
        let plan = provider_usage
            .plan
            .as_deref()
            .map(|plan| format!(" · {plan}"))
            .unwrap_or_default();
        let summary = if provider_usage.windows.is_empty() {
            provider_usage
                .message
                .clone()
                .unwrap_or_else(|| "usage unavailable".to_owned())
        } else {
            provider_usage
                .windows
                .iter()
                .take(3)
                .map(|window| format!("{} {}% left", window.label, window.remaining_percent))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let value = format!("{marker} {}{plan} · {summary}", item.provider);
        lines.push(Line::from(vec![
            Span::styled("Quota   ", Style::default().fg(theme::MUTED)),
            Span::raw(truncate_end(&value, width.saturating_sub(8))),
        ]));
        if lines.len() >= height {
            break;
        }
    }
    if !found_usage && lines.len() < height {
        let message = if app.state.auth_working {
            "refreshing provider usage…"
        } else {
            "provider usage unavailable · press r to refresh"
        };
        lines.push(Line::from(vec![
            Span::styled("Quota   ", Style::default().fg(theme::MUTED)),
            Span::styled(message, Style::default().fg(theme::TEXT_DIM)),
        ]));
    }
    lines
}

fn environment_status_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let (permission, sandbox) = app
        .state
        .sandbox_settings
        .as_ref()
        .map(|settings| {
            (
                settings.permission_mode(),
                format!(
                    "{} · {} · auto-approve {}",
                    settings.preset, settings.execution_mode, settings.auto_approve
                ),
            )
        })
        .unwrap_or(("unknown", "sandbox state unavailable".to_owned()));
    let session = app.state.current_session_id.as_deref().unwrap_or("none");
    let run = app.state.active_run_id.as_deref().unwrap_or("none");
    vec![
        Line::from(vec![
            Span::styled("Permission  ", Style::default().fg(theme::MUTED)),
            Span::styled(permission.to_owned(), Style::default().fg(theme::ACCENT)),
            Span::styled("  ·  ", Style::default().fg(theme::MUTED)),
            Span::raw(truncate_end(&sandbox, width.saturating_sub(20))),
        ]),
        Line::from(vec![
            Span::styled("Session     ", Style::default().fg(theme::MUTED)),
            Span::raw(truncate_middle(session, width.saturating_sub(12).max(8))),
        ]),
        Line::from(vec![
            Span::styled("Run         ", Style::default().fg(theme::MUTED)),
            Span::raw(truncate_middle(run, width.saturating_sub(12).max(8))),
        ]),
    ]
}

fn status_compact_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let model = if app.state.active_model.is_empty() {
        "not selected".to_owned()
    } else {
        truncate_middle(&app.state.active_model, width.saturating_sub(8).max(8))
    };
    let current = app.state.current_context_tokens.unwrap_or(0);
    let context = app
        .state
        .active_model_context_length
        .filter(|total| *total > 0)
        .map(|total| {
            format!(
                "{}/{} ({:.0}%)",
                compact_number(current),
                compact_number(total),
                current as f64 * 100.0 / total as f64
            )
        })
        .unwrap_or_else(|| format!("{} / ?", compact_number(current)));
    vec![
        Line::from(format!("Model   {model}")),
        Line::from(format!("Context {context}")),
        Line::from(format!(
            "Tokens  {}↑ {}↓",
            compact_number(app.state.token_usage.input_tokens.unwrap_or(0)),
            compact_number(app.state.token_usage.output_tokens.unwrap_or(0))
        )),
        Line::from(format!(
            "Mode    {} · {}",
            if app.state.active_reasoning_level.is_empty() {
                "auto"
            } else {
                app.state.active_reasoning_level.as_str()
            },
            app.state
                .sandbox_settings
                .as_ref()
                .map(|settings| settings.permission_mode())
                .unwrap_or("ask")
        )),
    ]
}

fn status_bar(percent: u8, cells: usize) -> String {
    let filled = ((percent.min(100) as usize * cells) + 50) / 100;
    format!(
        "{}{}",
        "━".repeat(filled),
        "─".repeat(cells.saturating_sub(filled))
    )
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
        Paragraph::new("Paste or type the provider API key.").fg(theme::MUTED),
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
            Paragraph::new("Loading custom providers…").fg(theme::TEXT_DIM),
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
                    Style::default().fg(theme::TEXT_DIM),
                ),
                Span::styled(metadata, Style::default().fg(theme::MUTED)),
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
                .fg(theme::ERROR),
            chunks[1],
        );
    } else if let Some(notice) = app.state.providers_notice.as_deref() {
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
    let area = centered_rect(78, 44, frame.area());
    theme::modal_backdrop(frame, area);
    let title = if app.state.settings_working {
        " Settings · saving… · Esc close "
    } else {
        " Settings · ↑/↓ navigate · Enter/Space toggle/open · r refresh · Esc close "
    };
    let block = theme::modal_block(title);
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
        .highlight_style(theme::selected())
        .highlight_symbol("▸ ");
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
        " Sandbox · ↑/↓ navigate · Enter apply/open · r refresh · Esc back "
    };
    let block = theme::modal_block(title);
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
        .highlight_style(theme::selected())
        .highlight_symbol("▸ ");
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
        tabs.push(Span::styled("‹ ", Style::default().fg(theme::MUTED)));
    }
    for (visible_index, (section, label)) in sections[start..end].iter().enumerate() {
        if visible_index > 0 {
            tabs.push(Span::styled("  ", Style::default().fg(theme::MUTED)));
        }
        let style = if *section == current {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::MUTED)
        };
        tabs.push(Span::styled(*label, style));
    }
    if end < sections.len() {
        tabs.push(Span::styled(" ›", Style::default().fg(theme::MUTED)));
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
            let row_width = chunks[1].width.saturating_sub(2) as usize;
            let mut rows = vec![ListItem::new(Line::from(vec![
                Span::raw("Workspace read mode  "),
                Span::styled(
                    settings.workspace_mode.clone(),
                    Style::default().fg(theme::TEXT_DIM),
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
                        Style::default().fg(theme::TEXT_DIM),
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
                            Style::default().fg(theme::TEXT_DIM),
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
        frame.render_widget(Paragraph::new(empty).fg(theme::TEXT_DIM), chunks[1]);
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
            theme::ERROR,
        )
    } else if let Some(notice) = app.state.sandbox_notice.as_deref() {
        (truncate_end(notice, chunks[2].width as usize), theme::MUTED)
    } else {
        (
            format!(
                "Preset: {} · advanced changes are reported as custom",
                settings.preset
            ),
            theme::MUTED,
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
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("│ ", Style::default().fg(theme::BORDER)),
            Span::styled(
                truncate_middle(value, value_width),
                Style::default().fg(theme::TEXT),
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
            "/status  usage",
            "/login  auth",
            "Ctrl+C  quit idle",
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

fn permission_action_line() -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "Enter/y",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" allow once  ·  ", Style::default().fg(theme::MUTED)),
        Span::styled(
            "n/Esc",
            Style::default()
                .fg(theme::ACCENT_HOT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" deny  ·  ", Style::default().fg(theme::MUTED)),
        Span::styled(
            "Ctrl+C",
            Style::default()
                .fg(theme::ACCENT_HOT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" stop task", Style::default().fg(theme::MUTED)),
    ])
}

pub(super) fn draw_permission(frame: &mut Frame<'_>, app: &App) {
    if let Some(permission) = app.state.pending_native_app_permission.as_ref() {
        let area = centered_rect(82, 70, frame.area());
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
            Line::from(permission.reason.clone()).fg(theme::TEXT_DIM),
        ]);
        let block = theme::modal_block(" Native app permission ")
            .border_style(Style::default().fg(theme::ACCENT_HOT));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), rows[0]);
        frame.render_widget(
            Paragraph::new(permission_action_line()).wrap(Wrap { trim: false }),
            rows[1],
        );
        return;
    }
    let Some(permission) = app.state.pending_shell_permission.as_ref() else {
        return;
    };
    let area = centered_rect(82, 70, frame.area());
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
        Line::from(Span::styled(
            permission.command.clone(),
            Style::default().fg(theme::ACCENT_HOT),
        )),
        Line::from(permission.reason.clone()).fg(theme::TEXT_DIM),
    ]);
    let block = theme::modal_block(" Permission ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), rows[0]);
    frame.render_widget(
        Paragraph::new(permission_action_line()).wrap(Wrap { trim: false }),
        rows[1],
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

    #[test]
    fn settings_form_cursor_tracks_visible_unicode_value_end() {
        use ratatui::{
            Terminal,
            backend::{Backend, TestBackend},
        };

        let rows = vec![("Value".to_owned(), "한글".to_owned())];
        let mut terminal = Terminal::new(TestBackend::new(60, 4)).unwrap();
        terminal
            .draw(|frame| draw_form_rows(frame, frame.area(), &rows, 0, true))
            .unwrap();

        let cursor = terminal.backend_mut().get_cursor_position().unwrap();
        assert_eq!(cursor, Position::new(30, 0));
    }

    #[test]
    fn settings_form_keeps_long_value_tail_visible_at_insertion_point() {
        use ratatui::{Terminal, backend::TestBackend};

        let rows = vec![(
            "Base URL".to_owned(),
            "https://api.example.test/a/very/long/provider/path/important-tail".to_owned(),
        )];
        let mut terminal = Terminal::new(TestBackend::new(60, 4)).unwrap();
        terminal
            .draw(|frame| draw_form_rows(frame, frame.area(), &rows, 0, true))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            rendered.contains("important-tail"),
            "the editable value tail should stay visible near the insertion point: {rendered}"
        );
    }

    #[test]
    fn permission_actions_stay_visible_when_content_wraps() {
        let mut app = App::default();
        app.state.pending_shell_permission = Some(crate::model::ShellPermission {
            id: "shell".into(),
            kind: "shell".into(),
            command: "git diff --stat && cargo test --workspace --all-features --very-long-placeholder-that-wraps-across-the-dialog".into(),
            operation: "Run a long shell command that requires explicit approval".into(),
            reason: "This deliberately long explanation should wrap over several visual lines so the fixed approval controls must remain visible at the bottom of a short terminal.".into(),
        });

        for (width, height) in [(40, 12), (48, 18), (80, 24)] {
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw_permission(frame, &app)).unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                rendered.contains("Enter/y"),
                "approval action should remain visible at {width}x{height}"
            );
            assert!(
                rendered.contains("n/Esc") && rendered.contains("deny"),
                "deny action should remain fully visible at {width}x{height}"
            );
            assert!(
                rendered.contains("Ctrl+C") && rendered.contains("stop task"),
                "interrupt action should remain fully visible at {width}x{height}"
            );
            if width >= 48 {
                assert!(
                    rendered.contains("This deliberately long explanation"),
                    "permission reason should remain visible at {width}x{height}"
                );
            }
        }

        app.state.pending_shell_permission = None;
        app.state.pending_native_app_permission = Some(crate::model::NativeAppPermission {
            id: "native".into(),
            server: "codex-computer-use".into(),
            tool: "native_app".into(),
            bundle_id: Some("com.example.extremely-long-native-application-bundle".into()),
            app_name: Some("Example Native Application With A Long Name".into()),
            operation: "Control this native application for a long-running interaction".into(),
            reason: "This deliberately long native-app reason must not displace the approval controls even when it wraps across several terminal rows.".into(),
        });
        for (width, height) in [(40, 12), (48, 18), (80, 24)] {
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw_permission(frame, &app)).unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                rendered.contains("Enter/y"),
                "native approval action should remain visible at {width}x{height}"
            );
            assert!(
                rendered.contains("n/Esc") && rendered.contains("deny"),
                "native deny action should remain fully visible at {width}x{height}"
            );
            assert!(
                rendered.contains("Ctrl+C") && rendered.contains("stop task"),
                "native interrupt action should remain fully visible at {width}x{height}"
            );
        }
    }

    #[test]
    fn providers_row_keeps_id_and_endpoint_visible_on_compact_terminals() {
        let mut app = App::default();
        app.state.provider_configurations = vec![crate::model::ProviderConfigurationItem {
            id: "custom-provider-with-a-very-long-identifier".into(),
            base_url: "https://very.long.custom.provider.api.example.test/v1".into(),
            require_api_key: true,
            header_count: 2,
        }];

        let backend = ratatui::backend::TestBackend::new(48, 18);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw_providers(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            rendered.contains("custom"),
            "provider id should remain identifiable: {rendered}"
        );
        assert!(
            rendered.contains("https://"),
            "provider endpoint should remain identifiable: {rendered}"
        );
        assert!(
            rendered.contains("/v1"),
            "provider endpoint tail should remain identifiable: {rendered}"
        );

        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw_providers(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("key required"));
        assert!(rendered.contains("2 headers"));
    }

    #[test]
    fn provider_delete_confirmation_is_visible_before_destructive_action() {
        let mut app = App::default();
        app.state.provider_configurations = vec![crate::model::ProviderConfigurationItem {
            id: "custom".into(),
            base_url: "https://custom.example".into(),
            require_api_key: true,
            header_count: 0,
        }];
        app.pending_provider_delete_id = Some("custom".into());

        for (width, height) in [(40, 12), (60, 18), (100, 30)] {
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw_providers(frame, &app)).unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                rendered.contains("Delete provider custom?"),
                "delete confirmation should name the provider at {width}x{height}"
            );
            assert!(
                rendered.contains("d/Delete") && rendered.contains("again"),
                "delete confirmation should explain the second key press at {width}x{height}"
            );
        }
    }

    #[test]
    fn provider_delete_confirmation_keeps_action_visible_for_long_ids() {
        let mut app = App::default();
        let id = "custom-provider-with-an-extremely-long-identifier-used-for-production";
        app.state.provider_configurations = vec![crate::model::ProviderConfigurationItem {
            id: id.into(),
            base_url: "https://provider.example".into(),
            require_api_key: true,
            header_count: 0,
        }];
        app.pending_provider_delete_id = Some(id.into());

        let backend = ratatui::backend::TestBackend::new(48, 18);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw_providers(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            rendered.contains("custom"),
            "confirmation should identify the provider: {rendered}"
        );
        assert!(
            rendered.contains("d/Delete") && rendered.contains("again"),
            "confirmation action must stay visible for long provider ids: {rendered}"
        );
    }

    #[test]
    fn sandbox_reset_confirmation_is_visible_before_full_policy_reset() {
        let mut app = App::default();
        app.state.sandbox_settings = Some(crate::model::SandboxSettingsState {
            preset: "custom".into(),
            execution_mode: "sandboxed".into(),
            auto_approve: false,
            workspace_mode: "all".into(),
            workspace_paths: vec!["src".into()],
            scratch_writable: true,
            network_allow: Vec::new(),
            environment: Vec::new(),
            secret_ids: Vec::new(),
            limits: crate::model::SandboxLimitsState {
                wall_time_seconds: 30,
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
                max_memory_bytes: 0,
                max_processes: 0,
            },
        });
        app.settings_section = Some(crate::app::SettingsSection::Core);
        app.popup_index = 3;
        app.pending_sandbox_reset = true;

        for (width, height) in [(40, 12), (60, 18), (100, 30)] {
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| draw_sandbox_policy(frame, &app))
                .unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                rendered.contains("Reset the sandbox policy"),
                "reset warning should stay visible at {width}x{height}"
            );
            assert!(
                rendered.contains("Enter/Space"),
                "reset warning should explain the second confirmation at {width}x{height}"
            );
        }
    }

    #[test]
    fn sandbox_policy_keeps_selected_compact_tab_visible() {
        let mut app = App::default();
        app.state.sandbox_settings = Some(crate::model::SandboxSettingsState {
            preset: "custom".into(),
            execution_mode: "sandboxed".into(),
            auto_approve: false,
            workspace_mode: "all".into(),
            workspace_paths: Vec::new(),
            scratch_writable: true,
            network_allow: Vec::new(),
            environment: Vec::new(),
            secret_ids: Vec::new(),
            limits: crate::model::SandboxLimitsState {
                wall_time_seconds: 30,
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
                max_memory_bytes: 0,
                max_processes: 0,
            },
        });
        app.settings_section = Some(crate::app::SettingsSection::Limits);

        let backend = ratatui::backend::TestBackend::new(48, 18);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_sandbox_policy(frame, &app))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            rendered.contains("Limits"),
            "selected Limits tab should remain visible at 48 columns: {rendered}"
        );
    }

    #[test]
    fn sandbox_workspace_row_keeps_project_tail_visible_on_compact_terminals() {
        let mut app = App::default();
        app.state.sandbox_settings = Some(crate::model::SandboxSettingsState {
            preset: "custom".into(),
            execution_mode: "sandboxed".into(),
            auto_approve: false,
            workspace_mode: "all".into(),
            workspace_paths: vec![
                "/Users/example/Projects/very/deep/source/tree/ImportantWorkspaceProject".into(),
            ],
            scratch_writable: true,
            network_allow: Vec::new(),
            environment: Vec::new(),
            secret_ids: Vec::new(),
            limits: crate::model::SandboxLimitsState {
                wall_time_seconds: 30,
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
                max_memory_bytes: 0,
                max_processes: 0,
            },
        });
        app.settings_section = Some(crate::app::SettingsSection::Workspace);
        app.popup_index = 1;

        let backend = ratatui::backend::TestBackend::new(48, 18);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_sandbox_policy(frame, &app))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            rendered.contains("Path"),
            "workspace path row should stay labeled: {rendered}"
        );
        assert!(
            rendered.contains("WorkspaceProject"),
            "workspace path tail should remain identifiable: {rendered}"
        );
    }

    #[test]
    fn sandbox_network_row_keeps_host_and_port_visible_on_compact_terminals() {
        let mut app = App::default();
        app.state.sandbox_settings = Some(crate::model::SandboxSettingsState {
            preset: "custom".into(),
            execution_mode: "sandboxed".into(),
            auto_approve: false,
            workspace_mode: "all".into(),
            workspace_paths: Vec::new(),
            scratch_writable: true,
            network_allow: vec![crate::model::SandboxNetworkItem {
                host: "very-long-service-name.with.many.subdomains.internal.example.test".into(),
                port: Some(443),
            }],
            environment: Vec::new(),
            secret_ids: Vec::new(),
            limits: crate::model::SandboxLimitsState {
                wall_time_seconds: 30,
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
                max_memory_bytes: 0,
                max_processes: 0,
            },
        });
        app.settings_section = Some(crate::app::SettingsSection::Network);

        let backend = ratatui::backend::TestBackend::new(48, 18);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_sandbox_policy(frame, &app))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            rendered.contains("very"),
            "network host should remain identifiable: {rendered}"
        );
        assert!(
            rendered.contains(":443"),
            "network port should remain visible: {rendered}"
        );
    }

    #[test]
    fn sandbox_environment_row_keeps_key_and_value_visible_on_compact_terminals() {
        let mut app = App::default();
        app.state.sandbox_settings = Some(crate::model::SandboxSettingsState {
            preset: "custom".into(),
            execution_mode: "sandboxed".into(),
            auto_approve: false,
            workspace_mode: "all".into(),
            workspace_paths: Vec::new(),
            scratch_writable: true,
            network_allow: Vec::new(),
            environment: vec![crate::model::SandboxEnvironmentItem {
                key: "YEET_EXTREMELY_LONG_ENVIRONMENT_VARIABLE_NAME_FOR_REMOTE_DEBUGGING".into(),
                value: "https://api.example.test/important-endpoint".into(),
            }],
            secret_ids: Vec::new(),
            limits: crate::model::SandboxLimitsState {
                wall_time_seconds: 30,
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
                max_memory_bytes: 0,
                max_processes: 0,
            },
        });
        app.settings_section = Some(crate::app::SettingsSection::Environment);

        let backend = ratatui::backend::TestBackend::new(48, 18);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_sandbox_policy(frame, &app))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            rendered.contains("YEET"),
            "environment key should remain identifiable: {rendered}"
        );
        assert!(
            rendered.contains("https://"),
            "environment value should remain identifiable: {rendered}"
        );
    }

    #[test]
    fn model_picker_groups_structured_provider_rows() {
        let mut app = App::default();
        app.state.available_models = vec![
            "openai/gpt-5.6-sol".into(),
            "openai/o4-mini".into(),
            "anthropic/claude-sonnet".into(),
        ];
        app.state.model_catalog = vec![
            crate::model::ModelCatalogItem {
                id: "openai/gpt-5.6-sol".into(),
                provider: "openai".into(),
                model: "gpt-5.6-sol".into(),
                context_length: Some(128_000),
            },
            crate::model::ModelCatalogItem {
                id: "openai/o4-mini".into(),
                provider: "openai".into(),
                model: "o4-mini".into(),
                context_length: Some(200_000),
            },
            crate::model::ModelCatalogItem {
                id: "anthropic/claude-sonnet".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet".into(),
                context_length: Some(200_000),
            },
        ];

        let rows = model_picker_rows(&app);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].provider_heading, "openai");
        assert_eq!(rows[1].provider_heading, "");
        assert_eq!(rows[2].provider_heading, "Claude (Anthropic)");
        assert_eq!(rows[0].context_length, Some(128_000));

        for (width, expect_hint) in [(48, false), (100, true)] {
            let backend = ratatui::backend::TestBackend::new(width, 24);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw_models(frame, &app)).unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert_eq!(
                rendered.contains("PgUp/PgDn"),
                expect_hint,
                "unexpected model navigation hint at {width} columns"
            );
        }
    }

    #[test]
    fn streaming_model_and_reasoning_pickers_explain_next_response_timing() {
        let mut app = App::default();
        app.state.is_streaming = true;
        app.state.active_model = "openai/gpt-5.6-sol".into();
        app.state.available_models = vec![
            "openai/gpt-5.6-sol".into(),
            "anthropic/claude-sonnet".into(),
        ];

        for draw in [draw_models as fn(&mut Frame<'_>, &App), draw_reasoning] {
            let backend = ratatui::backend::TestBackend::new(100, 24);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();

            assert!(
                rendered.contains("next response"),
                "streaming selector should explain deferred timing: {rendered}"
            );
        }
    }

    #[test]
    fn capabilities_picker_advertises_page_navigation_when_roomy() {
        let mut app = App::default();
        app.state.available_capabilities = vec![crate::model::CapabilityToggleItem {
            id: "skill:test".into(),
            kind: "skill".into(),
            name: "Test capability".into(),
            description: "Used to verify the capability picker navigation hint".into(),
            enabled: true,
        }];

        for (width, expect_hint) in [(48, false), (100, true)] {
            let backend = ratatui::backend::TestBackend::new(width, 24);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| draw_capabilities(frame, &app))
                .unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert_eq!(
                rendered.contains("PgUp/PgDn"),
                expect_hint,
                "unexpected capability navigation hint at {width} columns"
            );
        }
    }

    #[test]
    fn streaming_capabilities_are_visibly_read_only_in_list_and_detail() {
        let mut app = App::default();
        app.state.is_streaming = true;
        app.state.available_capabilities = vec![crate::model::CapabilityToggleItem {
            id: "skill:test".into(),
            kind: "skill".into(),
            name: "Test capability".into(),
            description: "Capability changes must wait for the active response".into(),
            enabled: true,
        }];
        app.capability_detail_id = Some("skill:test".into());

        for draw in [
            draw_capabilities as fn(&mut Frame<'_>, &App),
            draw_capability_detail,
        ] {
            let backend = ratatui::backend::TestBackend::new(80, 24);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            let rendered_lower = rendered.to_ascii_lowercase();
            assert!(rendered_lower.contains("changes locked"));
            assert!(!rendered.contains("Space toggle"));
        }
    }

    #[test]
    fn sessions_modal_preserves_cross_workspace_navigation_on_compact_terminals() {
        let mut app = App::default();
        app.state.known_workspaces = vec![
            crate::model::WorkspaceSummary {
                id: "one".into(),
                path: "/tmp/one".into(),
                display_name: "One".into(),
                updated_at: None,
                session_count: 1,
                is_current: true,
            },
            crate::model::WorkspaceSummary {
                id: "two".into(),
                path: "/tmp/two".into(),
                display_name: "Two".into(),
                updated_at: None,
                session_count: 1,
                is_current: false,
            },
        ];
        app.state.workspace_session_groups = vec![
            crate::model::WorkspaceSessionGroup {
                workspace_id: "one".into(),
                sessions: vec![crate::model::SessionSummary {
                    id: "current".into(),
                    title: "Current task".into(),
                    updated_at: String::new(),
                    model: "sol".into(),
                    message_count: 4,
                }],
            },
            crate::model::WorkspaceSessionGroup {
                workspace_id: "two".into(),
                sessions: vec![crate::model::SessionSummary {
                    id: "foreign".into(),
                    title: "Foreign task".into(),
                    updated_at: String::new(),
                    model: "sol".into(),
                    message_count: 7,
                }],
            },
        ];
        app.state.current_session_id = Some("foreign".into());
        app.popup_index = 1;

        let catalog = app.session_picker_items();
        assert_eq!(
            catalog
                .iter()
                .map(|item| item.session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["current", "foreign"]
        );
        assert_eq!(catalog[0].workspace_name, "One");
        assert_eq!(catalog[1].workspace_name, "Two");

        for (width, height) in [(80, 24), (120, 32)] {
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw_sessions(frame, &app)).unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();

            assert!(
                rendered.contains("One / Current task"),
                "current workspace missing at {width}x{height}"
            );
            assert!(
                rendered.contains("Two / Foreign task"),
                "other workspace missing at {width}x{height}"
            );
            assert!(
                rendered.contains("● Two / Foreign task"),
                "active cross-workspace session not identified at {width}x{height}"
            );
        }

        app.popup_filter = "foreign".into();
        app.popup_index = 0;
        let filtered = app.filtered_session_picker_items();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].session.id, "foreign");

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw_sessions(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("foreign  ·  1/2"));
        assert!(rendered.contains("Two / Foreign task"));
        assert!(!rendered.contains("One / Current task"));
    }

    #[test]
    fn sessions_modal_keeps_title_visible_when_workspace_name_is_long() {
        let mut app = App::default();
        app.state.known_workspaces = vec![crate::model::WorkspaceSummary {
            id: "long-workspace".into(),
            path: "/tmp/workspace-with-an-extremely-long-name".into(),
            display_name: "WorkspaceWithAnExtremelyLongName".into(),
            updated_at: None,
            session_count: 1,
            is_current: true,
        }];
        app.state.workspace_session_groups = vec![crate::model::WorkspaceSessionGroup {
            workspace_id: "long-workspace".into(),
            sessions: vec![crate::model::SessionSummary {
                id: "important-session".into(),
                title: "Important landing investigation".into(),
                updated_at: String::new(),
                model: "provider/model-with-an-extremely-long-name".into(),
                message_count: 42,
            }],
        }];
        app.state.current_session_id = Some("important-session".into());

        let backend = ratatui::backend::TestBackend::new(48, 18);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw_sessions(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            rendered.contains("Work"),
            "workspace should remain identifiable at 48 columns: {rendered}"
        );
        assert!(
            rendered.contains("Important"),
            "session title should remain identifiable at 48 columns: {rendered}"
        );
    }

    #[test]
    fn help_keeps_critical_shortcuts_visible_at_common_sizes() {
        for (width, height) in [(48, 18), (80, 24), (120, 32)] {
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(draw_help).unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();

            assert_eq!(
                help_rect(Rect::new(0, 0, width, height)).height,
                height.min(15)
            );
            for shortcut in [
                "Ctrl+W",
                "Alt+←/→",
                "Ctrl+N",
                "/status",
                "/goal",
                "Ctrl+D",
            ] {
                assert!(
                    rendered.contains(shortcut),
                    "{shortcut} should stay visible at {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn help_uses_readable_single_column_summary_on_narrow_terminals() {
        let width = 32;
        let height = 18;
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(draw_help).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(!rendered.contains("EDIT / SEND"));
        for shortcut in [
            "Shift+Enter",
            "Esc/Ctrl+C",
            "Alt+M/S/R/K",
            "Alt+←/→",
                "/settings",
            "/goal",
            "Ctrl+D",
        ] {
            assert!(
                rendered.contains(shortcut),
                "{shortcut} should remain readable at {width}x{height}: {rendered}"
            );
        }
    }
}

pub(super) fn draw_debate(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(92, 90, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(" DEBATE · Pro / Con / Jury ")
        .border_style(Style::default().fg(theme::ACCENT_HOT))
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
            "Default model: {} · Adaptive strengthening · jury retries uncapped",
            app.state.active_model,
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
