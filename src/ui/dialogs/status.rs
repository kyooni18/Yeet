//! Status and provider-usage dialog presentation.
use super::super::support::{
    responsive,
    text::{compact_number, truncate_end, truncate_middle},
    theme,
};
use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    prelude::{Line, Span, Style, Stylize},
    widgets::{Paragraph, Wrap},
};
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    responsive::modal_rect(area, percent_x, percent_y)
}

pub(crate) fn draw_status_dialog(frame: &mut Frame<'_>, app: &App) {
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
            Span::styled("Model      ", Style::default().fg(theme::muted())),
            Span::styled(model, Style::default().fg(theme::accent()).bold()),
        ]),
        Line::from(vec![
            Span::styled("Context    ", Style::default().fg(theme::muted())),
            Span::raw(truncate_end(&context, width.saturating_sub(11))),
        ]),
        Line::from(vec![
            Span::styled("Runtime    ", Style::default().fg(theme::muted())),
            Span::raw(runtime),
            Span::styled("  ·  reasoning ", Style::default().fg(theme::muted())),
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
            Span::styled("Tokens  ", Style::default().fg(theme::muted())),
            Span::raw(format!(
                "{} input · {} output · {} reasoning",
                compact_number(cache.input_tokens),
                compact_number(usage.output_tokens.unwrap_or(0)),
                compact_number(usage.reasoning_tokens.unwrap_or(0)),
            )),
        ]),
        Line::from(vec![
            Span::styled("Cache   ", Style::default().fg(theme::muted())),
            Span::raw(cache_detail),
        ]),
    ];
    if let Some(cost) = usage.estimated_cost_usd {
        lines.push(Line::from(vec![
            Span::styled("Cost    ", Style::default().fg(theme::muted())),
            Span::raw(format!("${cost:.4} estimated")),
        ]));
    }

    let active_provider = app
        .state
        .active_model
        .split_once('/')
        .map(|(provider, _)| provider);
    let mut found_usage = false;
    for item in app
        .state
        .auth_providers
        .iter()
        .filter(|item| item.usage.is_some())
    {
        if lines.len() >= height {
            break;
        }
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
        lines.push(Line::from(vec![
            Span::styled("Quota   ", Style::default().fg(theme::muted())),
            Span::styled(
                format!("{marker} {}{plan}", item.provider),
                Style::default().fg(if active_provider == Some(item.provider.as_str()) {
                    theme::accent()
                } else {
                    theme::text()
                }),
            ),
        ]));
        if lines.len() >= height {
            break;
        }
        if provider_usage.windows.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("        "),
                Span::styled(
                    truncate_end(
                        provider_usage
                            .message
                            .as_deref()
                            .unwrap_or("usage unavailable"),
                        width.saturating_sub(8),
                    ),
                    Style::default().fg(theme::text_dim()),
                ),
            ]));
            continue;
        }
        for window in &provider_usage.windows {
            if lines.len() >= height {
                break;
            }
            let remaining = window.remaining_percent.min(100);
            let bar_cells = width.saturating_sub(34).clamp(8, 20);
            let filled = ((remaining as usize * bar_cells) + 50) / 100;
            let quota_color = if remaining >= 50 {
                theme::success()
            } else if remaining >= 20 {
                theme::warning()
            } else {
                theme::error()
            };
            let reset = window
                .resets_at
                .as_deref()
                .map(|value| format!(" · reset {value}"))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                Span::raw("        "),
                Span::styled(
                    format!("{:<8}", truncate_end(&window.label, 8)),
                    Style::default().fg(theme::muted()),
                ),
                Span::styled("━".repeat(filled), Style::default().fg(quota_color)),
                Span::styled(
                    "─".repeat(bar_cells.saturating_sub(filled)),
                    Style::default().fg(theme::border_dim()),
                ),
                Span::styled(
                    format!(" {:>3}% left", remaining),
                    Style::default().fg(quota_color).bold(),
                ),
                Span::styled(
                    truncate_end(&reset, width.saturating_sub(24 + bar_cells)),
                    Style::default().fg(theme::text_dim()),
                ),
            ]));
        }
    }
    if !found_usage && lines.len() < height {
        let message = if app.state.auth_working {
            "refreshing provider usage…"
        } else {
            "provider usage unavailable · press r to refresh"
        };
        lines.push(Line::from(vec![
            Span::styled("Quota   ", Style::default().fg(theme::muted())),
            Span::styled(message, Style::default().fg(theme::text_dim())),
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
            Span::styled("Permission  ", Style::default().fg(theme::muted())),
            Span::styled(permission.to_owned(), Style::default().fg(theme::accent())),
            Span::styled("  ·  ", Style::default().fg(theme::muted())),
            Span::raw(truncate_end(&sandbox, width.saturating_sub(20))),
        ]),
        Line::from(vec![
            Span::styled("Session     ", Style::default().fg(theme::muted())),
            Span::raw(truncate_middle(session, width.saturating_sub(12).max(8))),
        ]),
        Line::from(vec![
            Span::styled("Run         ", Style::default().fg(theme::muted())),
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
    let mut lines = vec![
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
    ];
    let active_provider = app
        .state
        .active_model
        .split_once('/')
        .map(|(provider, _)| provider);
    if let Some(window) = app
        .state
        .auth_providers
        .iter()
        .find(|provider| Some(provider.provider.as_str()) == active_provider)
        .and_then(|provider| provider.usage.as_ref())
        .and_then(|usage| usage.windows.first())
    {
        let remaining = window.remaining_percent.min(100);
        let cells = width.saturating_sub(25).clamp(4, 14);
        let filled = ((remaining as usize * cells) + 50) / 100;
        let color = if remaining >= 50 {
            theme::success()
        } else if remaining >= 20 {
            theme::warning()
        } else {
            theme::error()
        };
        lines.push(Line::from(vec![
            Span::styled("Quota   ", Style::default().fg(theme::muted())),
            Span::styled("━".repeat(filled), Style::default().fg(color)),
            Span::styled(
                "─".repeat(cells.saturating_sub(filled)),
                Style::default().fg(theme::border_dim()),
            ),
            Span::styled(
                format!(" {:>3}% left", remaining),
                Style::default().fg(color).bold(),
            ),
        ]));
    }
    lines
}

fn status_bar(percent: u8, cells: usize) -> String {
    let filled = ((percent.min(100) as usize * cells) + 50) / 100;
    format!(
        "{}{}",
        "━".repeat(filled),
        "─".repeat(cells.saturating_sub(filled))
    )
}
